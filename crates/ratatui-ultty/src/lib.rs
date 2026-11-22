//! Bridge crate allowing an existing `ratatui` application to opt specific
//! areas (such as a transcript or log pane) into Ultty native regions while
//! keeping `ratatui` for the rest of the screen and falling back cleanly to
//! ANSI everywhere else.
//!
//! # Example
//!
//! ```no_run
//! use std::io;
//! use std::time::Duration;
//!
//! use ratatui::backend::CrosstermBackend;
//! use ratatui::layout::{Constraint, Layout};
//! use ratatui::widgets::Paragraph;
//! use ratatui_ultty::{NativeRegion, UlttyBridge};
//!
//! let mut bridge = UlttyBridge::connect(CrosstermBackend::new(io::stdout()))?;
//! bridge.draw(|frame| {
//!     let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(3)])
//!         .split(frame.area());
//!     frame.render_widget(Paragraph::new("Status: running"), chunks[0]);
//!     frame.native_region(
//!         chunks[1],
//!         NativeRegion::log("logs", "line 1\nline 2\nline 3").title("Logs"),
//!     );
//! })?;
//! let _event = bridge.poll_event(Duration::from_millis(50))?;
//! # Ok::<(), ratatui_ultty::Error>(())
//! ```

mod convert;
mod grid;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::{Deref, DerefMut};
use std::time::{Duration, Instant};

use crossterm::event::Event as CrosstermEvent;
use ratatui::backend::{Backend, TestBackend};
use ratatui::layout::{Rect, Size};
use ratatui::style::Style as RatStyle;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use ultty_client::UlttyClient;
use ultty_protocol::{
    BorderStyle, BorderType, Color, Constraints, Content, Dimension, Display, Live, NamedColor,
    Overflow, OverflowAxis, RegionId, RichText, Role, Semantics, Style, TerminalMessage, TextAlign,
    TextContent, TextOverflow, TextWrap, CAP_APPEND_CONTENT, CAP_REGION_METADATA, CAP_SEMANTICS,
    EVENT_KEY, EVENT_MOUSE, EVENT_SCROLL,
};

use crate::convert::{buffer_to_rich_text, proto_color_to_ratatui, terminal_message_to_crossterm};
use crate::grid::OverlayGrid;

/// Errors returned by [`UlttyBridge`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Terminal or backend I/O failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Ultty native client operation failed.
    #[error("Ultty client error: {0}")]
    Client(#[from] ultty_client::Error),

    /// Backend operation returned a non-I/O error.
    #[error("terminal backend error: {0}")]
    Backend(String),
}

/// Convenience result alias for `ratatui-ultty`.
pub type Result<T> = std::result::Result<T, Error>;

/// Active rendering mode of an [`UlttyBridge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Connected to Ultty: opted-in areas use native scrollable/copyable regions.
    Native,
    /// Running outside Ultty (or forced ANSI): all areas render through ratatui.
    Ansi,
}

/// Content carried by a [`NativeRegion`].
#[derive(Debug, Clone, PartialEq)]
pub enum RegionContent {
    /// Plain text content (supports incremental `AppendContent` streaming).
    Text(String),
    /// Styled `RichText` spans.
    RichText(RichText),
}

impl RegionContent {
    fn plain_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::RichText(rich) => rich.spans.iter().map(|s| s.text.as_str()).collect(),
        }
    }

    fn to_protocol_content(&self, wrap: TextWrap) -> Content {
        match self {
            Self::Text(text) => Content::Text(TextContent {
                text: text.clone(),
                wrap,
                align: TextAlign::Left,
                overflow: TextOverflow::Clip,
            }),
            Self::RichText(rich) => Content::RichText(rich.clone()),
        }
    }
}

/// Description of a screen area opted into an Ultty native region.
///
/// Outside Ultty, [`BridgeFrame::native_region`] renders this as a standard
/// ratatui [`Paragraph`] with borders, title, wrapping, and scroll offset.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeRegion {
    /// Stable identifier used for region retention, `scroll_region`, and `ulttyctl regions`.
    pub id: String,
    /// Optional title shown on the fallback block border and as accessibility label.
    pub title: Option<String>,
    /// Text or rich-text content shown in the region.
    pub content: RegionContent,
    /// Border style drawn around the region.
    pub border: BorderStyle,
    /// Optional border color.
    pub border_color: Option<Color>,
    /// Optional foreground text color.
    pub fg: Option<Color>,
    /// Optional background color (defaults to terminal background in native mode).
    pub bg: Option<Color>,
    /// Enable vertical scrolling (native scrollbar and mouse wheel in Ultty).
    pub scrollable: bool,
    /// Fallback scroll offset (rows from top) when rendering via ratatui ANSI mode.
    pub fallback_scroll_y: u16,
    /// Text wrapping mode.
    pub wrap: TextWrap,
    /// Accessibility role advertised to Ultty when `CAP_SEMANTICS` is negotiated.
    pub role: Role,
    /// Accessibility live announcement politeness.
    pub live: Live,
}

impl NativeRegion {
    /// Create a scrollable, word-wrapped log/transcript region with a thin border.
    pub fn log(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: None,
            content: RegionContent::Text(text.into()),
            border: BorderStyle::thin(),
            border_color: None,
            fg: None,
            bg: Some(Color::Named(NamedColor::Background)),
            scrollable: true,
            fallback_scroll_y: 0,
            wrap: TextWrap::Word,
            role: Role::Log,
            live: Live::Polite,
        }
    }

    /// Create a plain text native region without borders.
    pub fn text(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: None,
            content: RegionContent::Text(text.into()),
            border: BorderStyle::NONE,
            border_color: None,
            fg: None,
            bg: Some(Color::Named(NamedColor::Background)),
            scrollable: false,
            fallback_scroll_y: 0,
            wrap: TextWrap::Word,
            role: Role::Text,
            live: Live::Off,
        }
    }

    /// Create a styled rich-text native region.
    pub fn rich_text(id: impl Into<String>, rich: RichText) -> Self {
        Self {
            id: id.into(),
            title: None,
            content: RegionContent::RichText(rich),
            border: BorderStyle::thin(),
            border_color: None,
            fg: None,
            bg: Some(Color::Named(NamedColor::Background)),
            scrollable: true,
            fallback_scroll_y: 0,
            wrap: TextWrap::Word,
            role: Role::Log,
            live: Live::Off,
        }
    }

    /// Set a title for the region (used in ANSI fallback borders and accessibility label).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the border style.
    pub fn border(mut self, border: BorderStyle) -> Self {
        self.border = border;
        self
    }

    /// Set the border color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }

    /// Set the foreground color.
    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// Set the background color.
    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Enable or disable native vertical scrolling.
    pub fn scrollable(mut self, scrollable: bool) -> Self {
        self.scrollable = scrollable;
        self
    }

    /// Set the vertical scroll offset used in ANSI fallback rendering.
    pub fn fallback_scroll(mut self, scroll_y: u16) -> Self {
        self.fallback_scroll_y = scroll_y;
        self
    }

    /// Set the text wrap mode.
    pub fn wrap(mut self, wrap: TextWrap) -> Self {
        self.wrap = wrap;
        self
    }

    /// Set the accessibility role.
    pub fn role(mut self, role: Role) -> Self {
        self.role = role;
        self
    }

    /// Set the accessibility live politeness level.
    pub fn live(mut self, live: Live) -> Self {
        self.live = live;
        self
    }

    fn protocol_style(&self) -> Style {
        Style {
            fg: self.fg,
            bg: self.bg.or(Some(Color::Named(NamedColor::Background))),
            border: self.border,
            border_color: self.border_color,
            ..Style::default()
        }
    }

    fn render_ansi(&self, frame: &mut Frame<'_>, area: Rect) {
        let mut style = RatStyle::default();
        if let Some(fg) = self.fg {
            style = style.fg(proto_color_to_ratatui(fg));
        }
        if let Some(bg) = self.bg {
            style = style.bg(proto_color_to_ratatui(bg));
        }

        let mut paragraph = Paragraph::new(self.content.plain_text()).style(style);
        if self.wrap != TextWrap::None {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        if self.scrollable {
            paragraph = paragraph.scroll((self.fallback_scroll_y, 0));
        }
        if self.border.style != BorderType::None {
            let mut block = Block::default().borders(Borders::ALL);
            if let Some(title) = &self.title {
                block = block.title(title.as_str());
            }
            if let Some(bc) = self.border_color {
                block = block.border_style(RatStyle::default().fg(proto_color_to_ratatui(bc)));
            }
            paragraph = paragraph.block(block);
        }
        frame.render_widget(paragraph, area);
    }
}

/// Frame handle passed to [`UlttyBridge::draw`].
///
/// Dereferences directly to [`ratatui::Frame`], so existing ratatui widget calls
/// (`frame.area()`, `frame.render_widget(...)`, `frame.render_stateful_widget(...)`)
/// work unchanged while [`BridgeFrame::native_region`] opts specific areas into
/// Ultty native regions.
pub struct BridgeFrame<'a, 'b> {
    frame: &'b mut Frame<'a>,
    native_mode: bool,
    overlays: &'b mut Vec<(Rect, NativeRegion)>,
}

impl<'a, 'b> BridgeFrame<'a, 'b> {
    /// Opt `area` into an Ultty native region.
    ///
    /// In [`Mode::Native`], `area` is cleared in the ratatui backdrop and
    /// mounted as a native Ultty region with native scroll and clean text copy.
    /// In [`Mode::Ansi`], `region` is rendered directly with ratatui widgets.
    pub fn native_region(&mut self, area: Rect, region: NativeRegion) {
        if self.native_mode {
            self.frame.render_widget(Clear, area);
            self.overlays.push((area, region));
        } else {
            region.render_ansi(self.frame, area);
        }
    }
}

impl<'a> Deref for BridgeFrame<'a, '_> {
    type Target = Frame<'a>;

    fn deref(&self) -> &Self::Target {
        self.frame
    }
}

impl DerefMut for BridgeFrame<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.frame
    }
}

struct MountedRegion {
    id: RegionId,
    constraints: Constraints,
    style: Style,
    content: RegionContent,
    wrap: TextWrap,
    title: Option<String>,
    role: Role,
    live: Live,
}

struct NativeState {
    client: UlttyClient,
    scratch: Terminal<TestBackend>,
    grid_root: Option<RegionId>,
    grid_root_constraints: Option<Constraints>,
    backdrop_id: Option<RegionId>,
    backdrop_constraints: Option<Constraints>,
    backdrop_rich_text: Option<RichText>,
    mounted: HashMap<String, MountedRegion>,
    pending_events: VecDeque<CrosstermEvent>,
}

impl NativeState {
    fn new(mut client: UlttyClient) -> Result<Self> {
        client.subscribe(client.root_region(), EVENT_KEY | EVENT_MOUSE | EVENT_SCROLL)?;
        let width = client.viewport_width().max(1);
        let height = client.viewport_height().max(1);
        let Ok(scratch) = Terminal::new(TestBackend::new(width, height));
        Ok(Self {
            client,
            scratch,
            grid_root: None,
            grid_root_constraints: None,
            backdrop_id: None,
            backdrop_constraints: None,
            backdrop_rich_text: None,
            mounted: HashMap::new(),
            pending_events: VecDeque::new(),
        })
    }

    fn sync_scratch_size(&mut self) -> Result<()> {
        let width = self.client.viewport_width().max(1);
        let height = self.client.viewport_height().max(1);
        let Ok(current) = self.scratch.backend().size();
        if current.width != width || current.height != height {
            self.scratch.backend_mut().resize(width, height);
            let Ok(()) = self.scratch.clear();
        }
        Ok(())
    }

    fn commit(&mut self, overlays: Vec<(Rect, NativeRegion)>) -> Result<()> {
        let viewport = self.scratch.backend().buffer().area;
        let overlay_rects: Vec<Rect> = overlays.iter().map(|(rect, _)| *rect).collect();
        let grid = OverlayGrid::build(viewport, &overlay_rects);
        let root_constraints = grid.root_constraints();
        let (full_col, full_row) = grid.full_placement();
        let backdrop_constraints = Constraints {
            display: Display::Block,
            width: Dimension::Percent(100.0),
            height: Dimension::Percent(100.0),
            grid_column: full_col,
            grid_row: full_row,
            overflow: Overflow {
                x: OverflowAxis::Hidden,
                y: OverflowAxis::Hidden,
            },
            ..Constraints::default()
        };
        let backdrop_content = buffer_to_rich_text(self.scratch.backend().buffer());

        let frame_id = self.client.begin_frame()?;

        let grid_root = match self.grid_root {
            Some(id) => {
                if self.grid_root_constraints.as_ref() != Some(&root_constraints) {
                    self.client.set_constraints(id, root_constraints.clone())?;
                    self.grid_root_constraints = Some(root_constraints);
                }
                id
            }
            None => {
                let id = self.client.create_region(
                    self.client.root_region(),
                    root_constraints.clone(),
                    Style::new().bg(Color::Named(NamedColor::Background)),
                )?;
                self.grid_root = Some(id);
                self.grid_root_constraints = Some(root_constraints);
                id
            }
        };

        let backdrop_id = match self.backdrop_id {
            Some(id) => {
                if self.backdrop_constraints.as_ref() != Some(&backdrop_constraints) {
                    self.client
                        .set_constraints(id, backdrop_constraints.clone())?;
                    self.backdrop_constraints = Some(backdrop_constraints);
                }
                id
            }
            None => {
                let id = self.client.create_region(
                    grid_root,
                    backdrop_constraints.clone(),
                    Style::default(),
                )?;
                self.backdrop_id = Some(id);
                self.backdrop_constraints = Some(backdrop_constraints);
                id
            }
        };

        if self.backdrop_rich_text.as_ref() != Some(&backdrop_content) {
            self.client
                .set_content(backdrop_id, Content::RichText(backdrop_content.clone()))?;
            self.backdrop_rich_text = Some(backdrop_content);
        }

        let mut seen = HashSet::with_capacity(overlays.len());
        let supports_append = self.client.supports_capability(CAP_APPEND_CONTENT);
        let supports_metadata = self.client.supports_capability(CAP_REGION_METADATA);
        let supports_semantics = self.client.supports_capability(CAP_SEMANTICS);

        for (rect, spec) in overlays {
            let Some((grid_column, grid_row)) = grid.place(rect) else {
                continue;
            };
            seen.insert(spec.id.clone());

            let constraints = Constraints {
                display: Display::Block,
                width: Dimension::Percent(100.0),
                height: Dimension::Percent(100.0),
                grid_column,
                grid_row,
                overflow: Overflow {
                    x: OverflowAxis::Hidden,
                    y: if spec.scrollable {
                        OverflowAxis::Scroll
                    } else {
                        OverflowAxis::Hidden
                    },
                },
                ..Constraints::default()
            };
            let style = spec.protocol_style();

            if let Some(existing) = self.mounted.get_mut(&spec.id) {
                if existing.constraints != constraints {
                    self.client
                        .set_constraints(existing.id, constraints.clone())?;
                    existing.constraints = constraints;
                }
                if existing.style != style {
                    self.client.set_style(existing.id, style.clone())?;
                    existing.style = style;
                }
                if existing.content != spec.content || existing.wrap != spec.wrap {
                    let appended = match (&existing.content, &spec.content) {
                        (RegionContent::Text(old), RegionContent::Text(new))
                            if supports_append
                                && existing.wrap == spec.wrap
                                && new.len() > old.len()
                                && new.starts_with(old.as_str()) =>
                        {
                            self.client.append_content(existing.id, &new[old.len()..])?;
                            true
                        }
                        _ => false,
                    };
                    if !appended {
                        self.client.set_content(
                            existing.id,
                            spec.content.to_protocol_content(spec.wrap),
                        )?;
                    }
                    existing.content = spec.content;
                    existing.wrap = spec.wrap;
                }
                if supports_semantics
                    && (existing.title != spec.title
                        || existing.role != spec.role
                        || existing.live != spec.live)
                {
                    let semantics = Semantics {
                        role: spec.role,
                        label: spec.title.clone(),
                        live: spec.live,
                        ..Semantics::default()
                    };
                    self.client
                        .send(&ultty_protocol::AppMessage::SetSemantics {
                            id: existing.id,
                            semantics,
                        })?;
                    existing.title = spec.title;
                    existing.role = spec.role;
                    existing.live = spec.live;
                }
            } else {
                let id =
                    self.client
                        .create_region(grid_root, constraints.clone(), style.clone())?;
                self.client
                    .set_content(id, spec.content.to_protocol_content(spec.wrap))?;
                if supports_metadata {
                    self.client
                        .send(&ultty_protocol::AppMessage::SetRegionMetadata {
                            id,
                            metadata: vec![("id".to_string(), spec.id.clone())],
                        })?;
                }
                if supports_semantics {
                    let semantics = Semantics {
                        role: spec.role,
                        label: spec.title.clone(),
                        live: spec.live,
                        ..Semantics::default()
                    };
                    self.client
                        .send(&ultty_protocol::AppMessage::SetSemantics { id, semantics })?;
                }
                self.mounted.insert(
                    spec.id,
                    MountedRegion {
                        id,
                        constraints,
                        style,
                        content: spec.content,
                        wrap: spec.wrap,
                        title: spec.title,
                        role: spec.role,
                        live: spec.live,
                    },
                );
            }
        }

        let stale: Vec<String> = self
            .mounted
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect();
        for key in stale {
            if let Some(removed) = self.mounted.remove(&key) {
                self.client.delete_region(removed.id)?;
            }
        }

        self.client.end_frame(frame_id)?;
        Ok(())
    }

    fn drain_protocol_events(&mut self) -> Result<()> {
        for msg in self.client.poll_events()? {
            if let TerminalMessage::Resize(_) = &msg {
                self.sync_scratch_size()?;
            }
            if let Some(ev) = terminal_message_to_crossterm(&msg) {
                self.pending_events.push_back(ev);
            }
        }
        Ok(())
    }
}

/// Dual-mode bridge wrapping a `ratatui::Terminal<B>`.
///
/// Inside Ultty, opted-in regions render natively with terminal-owned scrolling
/// and clean copy while the rest of the UI stays pure `ratatui`. Outside Ultty,
/// everything delegates directly to `ratatui::Terminal<B>`.
pub struct UlttyBridge<B: Backend> {
    terminal: Terminal<B>,
    native: Option<NativeState>,
}

impl<B: Backend> UlttyBridge<B>
where
    B::Error: std::fmt::Display,
{
    /// Connect to Ultty when available, or fall back transparently to `backend`.
    pub fn connect(backend: B) -> Result<Self> {
        let terminal = Terminal::new(backend).map_err(|e| Error::Backend(e.to_string()))?;
        let native = match UlttyClient::connect() {
            Ok(client) => match NativeState::new(client) {
                Ok(state) => {
                    tracing::info!("ratatui-ultty connected in native mode");
                    Some(state)
                }
                Err(err) => {
                    tracing::debug!("native bridge setup failed, falling back to ANSI: {err}");
                    None
                }
            },
            Err(err) => {
                tracing::debug!("Ultty unavailable, using ratatui ANSI backend: {err}");
                None
            }
        };
        Ok(Self { terminal, native })
    }

    /// Create a bridge forced into pure ratatui ANSI mode.
    pub fn ansi(backend: B) -> Result<Self> {
        let terminal = Terminal::new(backend).map_err(|e| Error::Backend(e.to_string()))?;
        Ok(Self {
            terminal,
            native: None,
        })
    }

    /// Create a bridge attached to an already-connected [`UlttyClient`].
    pub fn with_client(backend: B, client: UlttyClient) -> Result<Self> {
        let terminal = Terminal::new(backend).map_err(|e| Error::Backend(e.to_string()))?;
        let native = NativeState::new(client)?;
        Ok(Self {
            terminal,
            native: Some(native),
        })
    }

    /// Active mode ([`Mode::Native`] or [`Mode::Ansi`]).
    pub fn mode(&self) -> Mode {
        if self.native.is_some() {
            Mode::Native
        } else {
            Mode::Ansi
        }
    }

    /// Access the underlying `ratatui::Terminal`.
    pub fn terminal(&self) -> &Terminal<B> {
        &self.terminal
    }

    /// Mutably access the underlying `ratatui::Terminal`.
    pub fn terminal_mut(&mut self) -> &mut Terminal<B> {
        &mut self.terminal
    }

    /// Current viewport size in cells.
    pub fn size(&self) -> Result<Size> {
        if let Some(native) = &self.native {
            Ok(Size::new(
                native.client.viewport_width(),
                native.client.viewport_height(),
            ))
        } else {
            self.terminal
                .size()
                .map_err(|e| Error::Backend(e.to_string()))
        }
    }

    /// Draw a frame using ratatui widgets and any opted-in [`NativeRegion`]s.
    pub fn draw<F>(&mut self, render_fn: F) -> Result<()>
    where
        F: FnOnce(&mut BridgeFrame<'_, '_>),
    {
        if let Some(native) = &mut self.native {
            native.sync_scratch_size()?;
            let mut overlays = Vec::new();
            let Ok(_) = native.scratch.draw(|frame| {
                let mut bridge_frame = BridgeFrame {
                    frame,
                    native_mode: true,
                    overlays: &mut overlays,
                };
                render_fn(&mut bridge_frame);
            });
            native.commit(overlays)?;
            return Ok(());
        }

        let mut ignored = Vec::new();
        self.terminal
            .draw(|frame| {
                let mut bridge_frame = BridgeFrame {
                    frame,
                    native_mode: false,
                    overlays: &mut ignored,
                };
                render_fn(&mut bridge_frame);
            })
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(())
    }

    /// Poll for the next terminal event up to `timeout`.
    ///
    /// In native mode, Ultty delivers keyboard, mouse, and resize events over
    /// the protocol socket; this method translates them into standard
    /// [`crossterm::event::Event`] values so app event loops stay unchanged.
    pub fn poll_event(&mut self, timeout: Duration) -> Result<Option<CrosstermEvent>> {
        if let Some(native) = &mut self.native {
            let deadline = Instant::now() + timeout;
            loop {
                native.drain_protocol_events()?;
                if let Some(event) = native.pending_events.pop_front() {
                    return Ok(Some(event));
                }
                let now = Instant::now();
                if now >= deadline || native.client.is_closed() {
                    return Ok(None);
                }
                let sleep_for = (deadline - now).min(Duration::from_millis(5));
                std::thread::sleep(sleep_for);
            }
        }

        if crossterm::event::poll(timeout)? {
            Ok(Some(crossterm::event::read()?))
        } else {
            Ok(None)
        }
    }

    /// Programmatically scroll an opted-in native region by id (no-op in ANSI mode).
    pub fn scroll_region(&mut self, id: &str, y: f32, animate: bool) -> Result<()> {
        if let Some(native) = &mut self.native {
            if let Some(mounted) = native.mounted.get(id) {
                native.client.scroll(mounted.id, 0.0, y, animate)?;
            }
        }
        Ok(())
    }
}

impl<B: Backend> Drop for UlttyBridge<B> {
    fn drop(&mut self) {
        if let Some(native) = &mut self.native {
            if let Err(err) = native.client.disconnect() {
                tracing::debug!("ratatui-ultty disconnect failed during drop: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use ratatui::layout::{Constraint, Layout};
    use ultty_protocol::{
        AppMessage, Decode, Encode, InputEvent, InputType, Key, LayoutTrigger, Modifiers,
        CAP_ALL_KNOWN, PROTOCOL_VERSION,
    };

    static SOCK_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_sock_path() -> std::path::PathBuf {
        let n = SOCK_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ratatui-ultty-test-{}-{n}.sock",
            std::process::id()
        ))
    }

    #[test]
    fn test_ansi_fallback_renders_ratatui_and_native_region() {
        let mut bridge = UlttyBridge::ansi(TestBackend::new(40, 8)).unwrap();
        assert_eq!(bridge.mode(), Mode::Ansi);

        bridge
            .draw(|frame| {
                let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(3)])
                    .split(frame.area());
                frame.render_widget(Paragraph::new("Header Bar"), chunks[0]);
                frame.native_region(
                    chunks[1],
                    NativeRegion::log("logs", "first line\nsecond line").title("Logs"),
                );
            })
            .unwrap();

        let buf = bridge.terminal().backend().buffer();
        let text: String = (0..8)
            .map(|y| {
                (0..40)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Header Bar"));
        assert!(text.contains("Logs"));
        assert!(text.contains("first line"));
        assert!(text.contains("second line"));
    }

    #[test]
    fn test_native_mode_mounts_grid_overlay_streams_append_and_translates_events() {
        let sock_path = temp_sock_path();
        let _ = std::fs::remove_file(&sock_path);
        let listener = UnixListener::bind(&sock_path).unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut received = Vec::new();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];

            loop {
                match stream.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    Err(_) => break,
                }
                while !buf.is_empty() {
                    let mut cursor = buf.as_slice();
                    let start_len = cursor.len();
                    match AppMessage::decode(&mut cursor) {
                        Ok(msg) => {
                            let consumed = start_len - cursor.len();
                            buf.drain(..consumed);
                            let is_disconnect = matches!(msg, AppMessage::Disconnect);
                            match &msg {
                                AppMessage::ClientHello { .. } => {
                                    let hello = TerminalMessage::ServerHello {
                                        version: PROTOCOL_VERSION,
                                        capabilities: CAP_ALL_KNOWN,
                                    };
                                    let ready = TerminalMessage::SessionReady {
                                        session_id: 7,
                                        root_region: RegionId::ROOT,
                                        width: 80,
                                        height: 24,
                                    };
                                    stream.write_all(&hello.encode_to_bytes()).unwrap();
                                    stream.write_all(&ready.encode_to_bytes()).unwrap();
                                    stream.flush().unwrap();
                                }
                                AppMessage::EndFrame { frame_id } => {
                                    let reply = TerminalMessage::LayoutComputed {
                                        frame_id: Some(*frame_id),
                                        trigger: LayoutTrigger::TreeChange,
                                        regions: vec![],
                                    };
                                    stream.write_all(&reply.encode_to_bytes()).unwrap();
                                    if frame_id.0 == 1 {
                                        let key = TerminalMessage::Input(InputEvent {
                                            region_id: RegionId::ROOT,
                                            event_type: InputType::KeyDown {
                                                key: Key::Char('j'),
                                                modifiers: Modifiers::NONE,
                                            },
                                            x: 0.0,
                                            y: 0.0,
                                            modifiers: Modifiers::NONE,
                                        });
                                        stream.write_all(&key.encode_to_bytes()).unwrap();
                                    }
                                    stream.flush().unwrap();
                                }
                                _ => {}
                            }
                            received.push(msg);
                            if is_disconnect {
                                return received;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            received
        });

        let client = UlttyClient::connect_to(&sock_path).unwrap();
        let mut bridge = UlttyBridge::with_client(TestBackend::new(80, 24), client).unwrap();
        assert_eq!(bridge.mode(), Mode::Native);

        bridge
            .draw(|frame| {
                let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(3)])
                    .split(frame.area());
                frame.render_widget(Paragraph::new("Status"), chunks[0]);
                frame.native_region(
                    chunks[1],
                    NativeRegion::log("logs", "line 1\n").title("Log"),
                );
            })
            .unwrap();

        let ev = bridge.poll_event(Duration::from_millis(500)).unwrap();
        assert!(matches!(ev, Some(CrosstermEvent::Key(_))));

        bridge
            .draw(|frame| {
                let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(3)])
                    .split(frame.area());
                frame.render_widget(Paragraph::new("Status"), chunks[0]);
                frame.native_region(
                    chunks[1],
                    NativeRegion::log("logs", "line 1\nline 2\n").title("Log"),
                );
            })
            .unwrap();

        bridge.scroll_region("logs", 1.0, false).unwrap();
        drop(bridge);

        let received = server.join().unwrap();
        let _ = std::fs::remove_file(&sock_path);

        assert!(received
            .iter()
            .any(|m| matches!(m, AppMessage::CreateRegion { constraints, .. } if constraints.display == Display::Grid)));
        assert!(received
            .iter()
            .any(|m| matches!(m, AppMessage::CreateRegion { constraints, .. } if constraints.overflow.y == OverflowAxis::Scroll)));
        assert!(received
            .iter()
            .any(|m| matches!(m, AppMessage::SetRegionMetadata { metadata, .. } if metadata == &[("id".into(), "logs".into())])));
        assert!(received
            .iter()
            .any(|m| matches!(m, AppMessage::AppendContent { text, .. } if text == "line 2\n")));
        assert!(received
            .iter()
            .any(|m| matches!(m, AppMessage::Scroll { y, .. } if (*y - 1.0).abs() < f32::EPSILON)));
        assert!(received.iter().any(|m| matches!(m, AppMessage::Disconnect)));
    }
}
