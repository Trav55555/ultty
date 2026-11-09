//! Inline cards: native regions in the shell's scrollback (ADR 0020).
//!
//! A transcript app, such as an agent CLI or a build tool, does not own the
//! screen: it prints lines that scroll away with the shell's, and now and
//! then a card (a tool call streaming its output, a diff) that it keeps
//! updating for a while. [`Console`] prints like a CLI and opens cards:
//!
//! ```no_run
//! use ulttui::inline::Console;
//! use ulttui::widgets::Text;
//!
//! let mut console = Console::connect();
//! console.println("Running tests...")?;
//! let card = console.open(3, &Text::new("cargo test"))?;
//! console.update(&card, &Text::new("cargo test\n  running 12 tests"))?;
//! console.close(card)?;
//! # Ok::<(), ulttui::Error>(())
//! ```
//!
//! In Ultty a card is an inline surface: a region tree anchored to its rows
//! of the main screen, drawn natively, scrolling with the text, and frozen
//! into those rows as text when closed, so copy and `ulttyctl text` read it
//! in order with the lines around it. Anywhere else ([`Mode::Fallback`]: not
//! Ultty, inside tmux or screen, the probe failed, or stdout is not a
//! terminal) the card is the view's ANSI rendering, printed as `rows` lines.
//!
//! Cards have a fixed height of `rows` lines and the terminal's width.
//! Print through the console rather than `println!`: in-band the terminal is
//! in raw mode, and the console ends lines with `\r\n` and keeps them in
//! order with cards.
//!
//! **Width changes.** Natively the terminal lays a live card out again at
//! the new width and clips or pads its rows; frozen cards are text and
//! re-wrap like any text. In fallback, a card is drawn at the width of the
//! moment; once the width changes it is final (a redraw by cursor movement
//! would land on rows the terminal may have re-wrapped). New cards use the
//! current width.

use std::collections::{HashMap, VecDeque};
use std::io::Write;

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color as TuiColor, Modifier};
use ratatui::Terminal;
use ultty_client::UlttyClient;
use ultty_protocol::{
    AppMessage, Constraints, Dimension, RegionId, Style, SurfaceEventKind, TerminalMessage,
    CAP_APPEND_CONTENT, CAP_INLINE_SURFACES, MAX_SURFACE_ROWS, MIN_SURFACE_ROWS,
};

use crate::backend::native_diff::{
    begin_native_key_recording, optimize_frame_messages, take_native_keys, RetainedTree,
};
use crate::widgets::{render_view_to_native, View};

/// How a [`Console`] shows its cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Inline surfaces in Ultty.
    Native,
    /// ANSI text, in any other terminal.
    Fallback,
}

/// An open card. Pass it to [`Console::update`] and [`Console::close`].
#[derive(Debug, PartialEq, Eq)]
pub struct Card {
    id: RegionId,
    rows: u16,
}

impl Card {
    /// The card's surface id, which [`CardEvent`]s name.
    pub fn id(&self) -> RegionId {
        self.id
    }

    /// The card's height in lines.
    pub fn rows(&self) -> u16 {
        self.rows
    }
}

/// What the terminal reported about a card (native mode only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardEvent {
    /// The card's id ([`Card::id`]).
    pub card: RegionId,
    /// What happened: placed, closed, evicted from scrollback, hidden or
    /// shown again. A card evicted or closed takes no more updates.
    pub kind: SurfaceEventKind,
}

/// A terminal transcript with inline cards. See the [module docs](self).
pub struct Console {
    link: Link,
    /// The next region id. Every card's regions take ids from this one
    /// counter, so ids never collide within the session.
    next_id: u32,
    last: Last,
    events: VecDeque<CardEvent>,
}

enum Link {
    Native(Box<NativeLink>),
    Fallback(FallbackLink),
}

struct NativeLink {
    client: UlttyClient,
    /// Each live card's regions as the terminal holds them.
    cards: HashMap<RegionId, RetainedTree>,
}

struct FallbackLink {
    out: Box<dyn Write>,
    size: SizeSource,
}

/// Where the fallback learns the terminal's size.
enum SizeSource {
    /// The terminal on stdout, asked each time (80 x 24 if it cannot say).
    Terminal,
    /// A fixed size: columns, lines.
    Fixed(u16, u16),
}

impl FallbackLink {
    fn size(&self) -> (u16, u16) {
        match self.size {
            SizeSource::Terminal => crossterm::terminal::size().unwrap_or((80, 24)),
            SizeSource::Fixed(cols, rows) => (cols, rows),
        }
    }
}

/// What the console printed last, which decides where the cursor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    /// Nothing yet, or text that ended a line: the cursor is at column 0.
    LineStart,
    /// Text that did not end its line.
    MidLine,
    /// A card's rows, drawn `width` columns wide; the cursor is at column 0
    /// of the line under them.
    Card { id: RegionId, rows: u16, width: u16 },
}

impl Console {
    /// Connect to Ultty for native cards, or fall back to ANSI text.
    ///
    /// Native mode needs stdout to be the terminal, an Ultty session (the
    /// tab's socket, or the in-band probe; see [`UlttyClient::connect`]) and
    /// `CAP_INLINE_SURFACES`. Anything else, including tmux, is
    /// [`Mode::Fallback`]; this never fails.
    pub fn connect() -> Self {
        // SAFETY: isatty only inspects the fd.
        let stdout_is_tty = unsafe { libc::isatty(1) == 1 };
        if stdout_is_tty {
            match UlttyClient::connect() {
                Ok(client) if client.supports_capability(CAP_INLINE_SURFACES) => {
                    return Self::native(client);
                }
                Ok(_) => tracing::debug!("the terminal does not offer inline surfaces"),
                Err(err) => tracing::debug!("no Ultty session: {err}"),
            }
        }
        Self::with_link(Link::Fallback(FallbackLink {
            out: Box::new(std::io::stdout()),
            size: SizeSource::Terminal,
        }))
    }

    /// A fallback console writing to `out` as if to a terminal of `cols`
    /// columns and `lines` lines.
    pub fn fallback(out: impl Write + 'static, cols: u16, lines: u16) -> Self {
        Self::with_link(Link::Fallback(FallbackLink {
            out: Box::new(out),
            size: SizeSource::Fixed(cols, lines),
        }))
    }

    fn native(client: UlttyClient) -> Self {
        Self::with_link(Link::Native(Box::new(NativeLink {
            client,
            cards: HashMap::new(),
        })))
    }

    fn with_link(link: Link) -> Self {
        Self {
            link,
            next_id: 1,
            last: Last::LineStart,
            events: VecDeque::new(),
        }
    }

    /// Whether cards are native surfaces or ANSI text.
    pub fn mode(&self) -> Mode {
        match self.link {
            Link::Native(_) => Mode::Native,
            Link::Fallback(_) => Mode::Fallback,
        }
    }

    /// The terminal's width in columns, which new cards take.
    pub fn width(&self) -> u16 {
        match &self.link {
            Link::Native(native) => native.client.viewport_width(),
            Link::Fallback(fallback) => fallback.size().0,
        }
    }

    /// Print `text` as it is; each `\n` ends a line.
    pub fn print(&mut self, text: &str) -> crate::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let crlf = text.replace("\r\n", "\n").replace('\n', "\r\n");
        self.write(crlf.as_bytes())?;
        self.last = if text.ends_with('\n') {
            Last::LineStart
        } else {
            Last::MidLine
        };
        self.pump()
    }

    /// Print `text` and end the line.
    pub fn println(&mut self, text: &str) -> crate::Result<()> {
        self.print(&format!("{text}\n"))
    }

    /// Open a card `rows` lines high showing `view`, on the next line.
    ///
    /// The view is rendered into `rows` lines of the terminal's width
    /// ([`View::render_ansi`]): that is the card in fallback, and natively
    /// the fallback text under the surface (plain), which shows in a
    /// recording or wherever the anchor is not understood. Natively the view
    /// is then sent as the surface's region tree. `rows` is 1 to 128.
    pub fn open(&mut self, rows: u16, view: &impl View) -> crate::Result<Card> {
        if !(MIN_SURFACE_ROWS..=MAX_SURFACE_ROWS).contains(&rows) {
            return Err(crate::Error::InvalidCard(format!(
                "{rows} rows; a card is {MIN_SURFACE_ROWS} to {MAX_SURFACE_ROWS} rows"
            )));
        }
        if self.last == Last::MidLine {
            self.write(b"\r\n")?;
        }
        let width = self.width().max(1);
        let buffer = render_buffer(view, width, rows);
        let id = RegionId::new(self.next_id);
        self.next_id += 1;
        match &mut self.link {
            Link::Native(native) => {
                let fallback: Vec<String> = (0..rows).map(|y| plain_line(&buffer, y)).collect();
                let mut tree = RetainedTree::new();
                let contents = surface_messages(
                    &mut self.next_id,
                    id,
                    view,
                    &mut tree,
                    native.client.supports_capability(CAP_APPEND_CONTENT),
                )?;
                native.client.open_surface(
                    id,
                    rows,
                    &fallback,
                    Constraints::default(),
                    Style::default(),
                    contents,
                )?;
                native.cards.insert(id, tree);
            }
            Link::Fallback(fallback) => {
                let mut text = String::new();
                for y in 0..rows {
                    text.push_str(&ansi_line(&buffer, y));
                    text.push_str("\r\n");
                }
                fallback.out.write_all(text.as_bytes())?;
                fallback.out.flush()?;
            }
        }
        self.last = Last::Card { id, rows, width };
        self.pump()?;
        Ok(Card { id, rows })
    }

    /// Show `view` in `card` instead of what it showed.
    ///
    /// Natively, the view is diffed against the card's regions and only the
    /// changes are sent, in one frame; a card the terminal evicted or closed
    /// is left alone. In fallback, the card is redrawn in place with
    /// cursor-up only while it is the last thing the console printed, the
    /// width is unchanged and it is shorter than the screen; otherwise it is
    /// final and this does nothing.
    pub fn update(&mut self, card: &Card, view: &impl View) -> crate::Result<()> {
        match &mut self.link {
            Link::Native(native) => {
                let Some(tree) = native.cards.get_mut(&card.id) else {
                    return Ok(());
                };
                let messages = surface_messages(
                    &mut self.next_id,
                    card.id,
                    view,
                    tree,
                    native.client.supports_capability(CAP_APPEND_CONTENT),
                )?;
                if !messages.is_empty() {
                    let frame = native.client.begin_frame()?;
                    for message in &messages {
                        native.client.send(message)?;
                    }
                    native.client.end_frame(frame)?;
                }
            }
            Link::Fallback(fallback) => {
                let Last::Card { id, rows, width } = self.last else {
                    return Ok(());
                };
                if id != card.id {
                    return Ok(());
                }
                let (cols, lines) = fallback.size();
                if cols != width || rows >= lines {
                    // Final from now on, even if the width comes back.
                    self.last = Last::LineStart;
                    return Ok(());
                }
                let buffer = render_buffer(view, width, rows);
                let mut text = format!("\x1b[{rows}A");
                for y in 0..rows {
                    text.push_str("\r\x1b[2K");
                    text.push_str(&ansi_line(&buffer, y));
                    text.push_str("\r\n");
                }
                fallback.out.write_all(text.as_bytes())?;
                fallback.out.flush()?;
            }
        }
        self.pump()
    }

    /// Close `card`. Natively the terminal freezes it into its rows as
    /// text; in fallback its lines simply stay as last drawn.
    pub fn close(&mut self, card: Card) -> crate::Result<()> {
        if let Link::Native(native) = &mut self.link {
            if native.cards.remove(&card.id).is_some() {
                native.client.close_surface(card.id)?;
            }
        }
        if matches!(self.last, Last::Card { id, .. } if id == card.id) {
            self.last = Last::LineStart;
        }
        self.pump()
    }

    /// What the terminal reported about cards since the last call (always
    /// empty in fallback). Call this every second or two while otherwise
    /// idle: it also answers the terminal's keepalive, without which an
    /// in-band session ends after 10 s of silence and its cards freeze.
    pub fn poll_events(&mut self) -> crate::Result<Vec<CardEvent>> {
        self.pump()?;
        Ok(self.events.drain(..).collect())
    }

    /// Read what the terminal sent: answer pings, keep card events, and
    /// forget cards that can take no more updates.
    fn pump(&mut self) -> crate::Result<()> {
        let Link::Native(native) = &mut self.link else {
            return Ok(());
        };
        for message in native.client.poll_events()? {
            match message {
                TerminalMessage::SurfaceEvent { id, kind } => {
                    if matches!(kind, SurfaceEventKind::Closed | SurfaceEventKind::Evicted) {
                        native.cards.remove(&id);
                    }
                    self.events.push_back(CardEvent { card: id, kind });
                }
                TerminalMessage::Error { code, message } => {
                    tracing::warn!("terminal error {code}: {message}");
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> crate::Result<()> {
        match &mut self.link {
            Link::Native(native) => native.client.write_output(bytes)?,
            Link::Fallback(fallback) => {
                fallback.out.write_all(bytes)?;
                fallback.out.flush()?;
            }
        }
        Ok(())
    }
}

/// The messages that turn `tree` (the card's regions as the terminal holds
/// them) into `view`, which fills surface `surface`. Updates `tree`, and
/// `next_id` past every id the tree now uses.
fn surface_messages(
    next_id: &mut u32,
    surface: RegionId,
    view: &impl View,
    tree: &mut RetainedTree,
    allow_append_content: bool,
) -> crate::Result<Vec<AppMessage>> {
    tree.reserve(RegionId::new(next_id.saturating_sub(1)));
    let mut constraints = view.constraints();
    constraints.width = Dimension::Percent(100.0);
    constraints.height = Dimension::Percent(100.0);
    let mut messages = Vec::new();
    // Provisional ids only need to differ from the surface's, the one
    // parent outside the walk.
    let mut provisional = surface.0.wrapping_add(1);
    begin_native_key_recording();
    let walked = render_view_to_native(
        view,
        &mut |message| {
            messages.push(message.clone());
            Ok(())
        },
        surface,
        &mut provisional,
        Some(constraints),
    );
    let keys = take_native_keys();
    walked?;
    let (outgoing, next) = optimize_frame_messages(tree, messages, &keys, allow_append_content);
    *next_id = (*next_id).max(next.next_id());
    *tree = next;
    Ok(outgoing)
}

/// `view` drawn by its ANSI renderer into `cols` x `rows` cells.
fn render_buffer(view: &impl View, cols: u16, rows: u16) -> Buffer {
    let Ok(mut terminal) = Terminal::new(TestBackend::new(cols, rows));
    let Ok(_) = terminal.draw(|frame| view.render_ansi(frame, frame.area()));
    terminal.backend().buffer().clone()
}

/// The cells of line `y` a terminal would print: the cells a wide character
/// covers after its first are left out.
fn line_cells(buffer: &Buffer, y: u16) -> Vec<&ratatui::buffer::Cell> {
    let mut cells = Vec::new();
    let mut covered = 0;
    for x in 0..buffer.area.width {
        let cell = &buffer[(x, y)];
        if covered > 0 {
            covered -= 1;
            continue;
        }
        covered = unicode_width::UnicodeWidthStr::width(cell.symbol()).saturating_sub(1);
        cells.push(cell);
    }
    cells
}

/// Line `y` as plain text, without trailing blanks.
fn plain_line(buffer: &Buffer, y: u16) -> String {
    let text: String = line_cells(buffer, y).iter().map(|c| c.symbol()).collect();
    text.trim_end_matches(' ').to_string()
}

/// Line `y` with SGR sequences for its colours and attributes, without
/// trailing blank cells, ending with the attributes reset.
fn ansi_line(buffer: &Buffer, y: u16) -> String {
    let cells = line_cells(buffer, y);
    let shown = cells
        .iter()
        .rposition(|c| {
            c.symbol() != " "
                || c.bg != TuiColor::Reset
                || c.modifier
                    .intersects(Modifier::UNDERLINED | Modifier::REVERSED | Modifier::CROSSED_OUT)
        })
        .map_or(0, |last| last + 1);
    let mut line = String::new();
    let plain = (TuiColor::Reset, TuiColor::Reset, Modifier::empty());
    let mut current = plain;
    for cell in &cells[..shown] {
        let style = (cell.fg, cell.bg, cell.modifier);
        if style != current {
            line.push_str(&sgr(style.0, style.1, style.2));
            current = style;
        }
        line.push_str(cell.symbol());
    }
    if current != plain {
        line.push_str("\x1b[0m");
    }
    line
}

/// One SGR sequence that sets exactly these colours and attributes.
fn sgr(fg: TuiColor, bg: TuiColor, modifier: Modifier) -> String {
    let mut codes = vec!["0".to_string()];
    for (flag, code) in [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
        (Modifier::REVERSED, "7"),
        (Modifier::CROSSED_OUT, "9"),
    ] {
        if modifier.contains(flag) {
            codes.push(code.to_string());
        }
    }
    codes.extend(colour_code(fg, false));
    codes.extend(colour_code(bg, true));
    format!("\x1b[{}m", codes.join(";"))
}

fn colour_code(colour: TuiColor, background: bool) -> Option<String> {
    let base = |n: u8| (if background { n + 10 } else { n }).to_string();
    let extended = if background { "48" } else { "38" };
    Some(match colour {
        TuiColor::Reset => return None,
        TuiColor::Black => base(30),
        TuiColor::Red => base(31),
        TuiColor::Green => base(32),
        TuiColor::Yellow => base(33),
        TuiColor::Blue => base(34),
        TuiColor::Magenta => base(35),
        TuiColor::Cyan => base(36),
        TuiColor::Gray => base(37),
        TuiColor::DarkGray => base(90),
        TuiColor::LightRed => base(91),
        TuiColor::LightGreen => base(92),
        TuiColor::LightYellow => base(93),
        TuiColor::LightBlue => base(94),
        TuiColor::LightMagenta => base(95),
        TuiColor::LightCyan => base(96),
        TuiColor::White => base(97),
        TuiColor::Indexed(n) => format!("{extended};5;{n}"),
        TuiColor::Rgb(r, g, b) => format!("{extended};2;{r};{g};{b}"),
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    use ratatui::layout::Rect;
    use ratatui::style::Style as TuiStyle;
    use ultty_protocol::{
        ComputedRegionLayout, Decode, Encode, FrameId, LayoutTrigger, CLIENT_DEFAULT_CAPABILITIES,
        PROTOCOL_VERSION,
    };

    use super::*;
    use crate::widgets::{Column, Text};

    /// What a fallback console wrote, readable while it is in use.
    #[derive(Clone, Default)]
    struct Output(Rc<RefCell<Vec<u8>>>);

    impl Output {
        fn take(&self) -> String {
            String::from_utf8(std::mem::take(&mut *self.0.borrow_mut())).unwrap()
        }
    }

    impl Write for Output {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn fallback_console(cols: u16, lines: u16) -> (Console, Output) {
        let output = Output::default();
        (Console::fallback(output.clone(), cols, lines), output)
    }

    #[test]
    fn fallback_card_is_exactly_rows_lines_clipped_to_the_width() {
        let (mut console, output) = fallback_console(10, 24);
        assert_eq!(console.mode(), Mode::Fallback);
        console.println("before").unwrap();
        let card = console
            .open(3, &Text::new("0123456789ABCDEF\nsecond"))
            .unwrap();
        assert_eq!(card.rows(), 3);
        assert_eq!(
            output.take(),
            "before\r\n0123456789\r\nsecond\r\n\r\n",
            "three lines, the first clipped at 10 columns"
        );
    }

    #[test]
    fn fallback_card_opens_on_a_new_line_after_a_partial_one() {
        let (mut console, output) = fallback_console(20, 24);
        console.print("$ ").unwrap();
        console.open(1, &Text::new("card")).unwrap();
        assert_eq!(output.take(), "$ \r\ncard\r\n");
    }

    #[test]
    fn fallback_update_redraws_with_cursor_up_while_the_card_is_last() {
        let (mut console, output) = fallback_console(20, 24);
        let card = console.open(2, &Text::new("one")).unwrap();
        output.take();
        console.update(&card, &Text::new("two\nthree")).unwrap();
        assert_eq!(output.take(), "\x1b[2A\r\x1b[2Ktwo\r\n\r\x1b[2Kthree\r\n");
        // Still last, so it can be redrawn again.
        console.update(&card, &Text::new("four")).unwrap();
        assert_eq!(output.take(), "\x1b[2A\r\x1b[2Kfour\r\n\r\x1b[2K\r\n");
    }

    #[test]
    fn fallback_card_is_final_once_something_is_printed_after_it() {
        let (mut console, output) = fallback_console(20, 24);
        let first = console.open(1, &Text::new("first")).unwrap();
        console.println("later").unwrap();
        output.take();
        console.update(&first, &Text::new("changed")).unwrap();
        assert_eq!(output.take(), "", "the card is no longer last");

        // A newer card is last; the older one stays final.
        let second = console.open(1, &Text::new("second")).unwrap();
        output.take();
        console.update(&first, &Text::new("changed")).unwrap();
        assert_eq!(output.take(), "");
        console.update(&second, &Text::new("again")).unwrap();
        assert_eq!(output.take(), "\x1b[1A\r\x1b[2Kagain\r\n");
    }

    #[test]
    fn fallback_card_is_final_after_the_width_changes() {
        let (mut console, output) = fallback_console(20, 24);
        let card = console.open(1, &Text::new("card")).unwrap();
        output.take();
        let Link::Fallback(fallback) = &mut console.link else {
            unreachable!()
        };
        fallback.size = SizeSource::Fixed(30, 24);
        console.update(&card, &Text::new("wider")).unwrap();
        let Link::Fallback(fallback) = &mut console.link else {
            unreachable!()
        };
        fallback.size = SizeSource::Fixed(20, 24);
        console.update(&card, &Text::new("back")).unwrap();
        assert_eq!(output.take(), "", "rows may have re-wrapped meanwhile");
        // New cards take the new width.
        console.open(1, &Text::new("x".repeat(40))).unwrap();
        assert_eq!(output.take(), format!("{}\r\n", "x".repeat(20)));
    }

    #[test]
    fn fallback_card_as_tall_as_the_screen_is_not_redrawn() {
        let (mut console, output) = fallback_console(20, 4);
        let card = console.open(4, &Text::new("tall")).unwrap();
        output.take();
        console.update(&card, &Text::new("taller")).unwrap();
        assert_eq!(output.take(), "", "cursor-up cannot reach its top line");
    }

    #[test]
    fn card_height_must_be_1_to_128_rows() {
        let (mut console, output) = fallback_console(20, 24);
        for rows in [0, 129] {
            assert!(matches!(
                console.open(rows, &Text::new("x")),
                Err(crate::Error::InvalidCard(_))
            ));
        }
        assert_eq!(output.take(), "");
    }

    #[test]
    fn ansi_lines_carry_styles_and_wide_characters() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 1));
        buffer.set_string(
            0,
            0,
            "ab",
            TuiStyle::default()
                .fg(TuiColor::Red)
                .add_modifier(Modifier::BOLD),
        );
        buffer.set_string(2, 0, "\u{4e16}x", TuiStyle::default());
        assert_eq!(
            ansi_line(&buffer, 0),
            "\x1b[0;1;31mab\x1b[0m\u{4e16}x",
            "styled run, reset, a wide character taking two cells"
        );
        assert_eq!(plain_line(&buffer, 0), "ab\u{4e16}x");
        assert_eq!(
            sgr(
                TuiColor::Rgb(1, 2, 3),
                TuiColor::Indexed(17),
                Modifier::empty()
            ),
            "\x1b[0;38;2;1;2;3;48;5;17m"
        );
    }

    /// A fake Ultty on a socket: answers the handshake (session 5, 40 x 10,
    /// every capability) and every `EndFrame`, and returns all it received.
    fn fake_ultty(listener: UnixListener) -> std::thread::JoinHandle<Vec<AppMessage>> {
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let send = |stream: &mut UnixStream, msg: TerminalMessage| {
                stream.write_all(&msg.encode_to_bytes()).unwrap();
            };
            let mut received = Vec::new();
            loop {
                let mut header = [0u8; 8];
                if stream.read_exact(&mut header).is_err() {
                    return received;
                }
                let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
                let mut frame = header.to_vec();
                frame.resize(8 + len as usize, 0);
                stream.read_exact(&mut frame[8..]).unwrap();
                let msg = AppMessage::decode(&mut frame.as_slice()).unwrap();
                match &msg {
                    AppMessage::ClientHello { .. } => {
                        send(
                            &mut stream,
                            TerminalMessage::ServerHello {
                                version: PROTOCOL_VERSION,
                                capabilities: CLIENT_DEFAULT_CAPABILITIES,
                            },
                        );
                        send(
                            &mut stream,
                            TerminalMessage::SessionReady {
                                session_id: 5,
                                root_region: RegionId::ROOT,
                                width: 40,
                                height: 10,
                            },
                        );
                    }
                    AppMessage::EndFrame { frame_id } => send(
                        &mut stream,
                        TerminalMessage::LayoutComputed {
                            frame_id: Some(FrameId(frame_id.0)),
                            trigger: LayoutTrigger::ContentChange,
                            regions: vec![ComputedRegionLayout {
                                id: RegionId::ROOT,
                                x: 0.0,
                                y: 0.0,
                                width: 40.0,
                                height: 10.0,
                                content_x: 0.0,
                                content_y: 0.0,
                                content_width: 40.0,
                                content_height: 10.0,
                                visible: true,
                            }],
                        },
                    ),
                    _ => {}
                }
                received.push(msg);
            }
        })
    }

    #[derive(Clone, Default)]
    struct SharedOutput(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedOutput {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn tool_card(lines: &[&str]) -> Column {
        Column::new()
            .child(Text::new("$ cargo test"))
            .child(Text::new(lines.join("\n")))
    }

    /// The frames in `messages`, each as the messages between its
    /// `BeginFrame` and `EndFrame`.
    fn frames(messages: &[AppMessage]) -> Vec<Vec<AppMessage>> {
        let mut frames = Vec::new();
        let mut current = None;
        for msg in messages {
            match msg {
                AppMessage::BeginFrame { .. } => current = Some(Vec::new()),
                AppMessage::EndFrame { .. } => frames.extend(current.take()),
                other => {
                    if let Some(frame) = current.as_mut() {
                        frame.push(other.clone());
                    }
                }
            }
        }
        frames
    }

    fn created(frame: &[AppMessage]) -> Vec<(RegionId, RegionId)> {
        frame
            .iter()
            .filter_map(|m| match m {
                AppMessage::CreateRegion { id, parent, .. } => Some((*id, *parent)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn native_cards_open_with_their_view_update_by_diff_and_close() {
        let dir = std::env::temp_dir().join(format!("ulttui-inline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s");
        let _ = std::fs::remove_file(&path);
        let terminal = fake_ultty(UnixListener::bind(&path).unwrap());

        let output = SharedOutput::default();
        let mut client = UlttyClient::connect_to(&path).unwrap();
        client.set_output(Box::new(output.clone()));
        let mut console = Console::native(client);
        assert_eq!(console.mode(), Mode::Native);

        console.println("prose").unwrap();
        let first = console.open(3, &tool_card(&["Compiling a"])).unwrap();
        console
            .update(&first, &tool_card(&["Compiling a", "Compiling b"]))
            .unwrap();
        console
            .update(&first, &tool_card(&["Compiling a", "Compiling b"]))
            .unwrap();
        let second = console.open(2, &Text::new("diff")).unwrap();
        console.close(first).unwrap();
        console.close(second).unwrap();
        drop(console);
        let received = terminal.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert_eq!(
            text,
            "prose\r\n\
             \x1b_u5;1;3\x1b\\$ cargo test\r\nCompiling a\r\n\r\n\
             \x1b_u5;5;2\x1b\\diff\r\n\r\n",
            "prose, then each card's anchor and its plain fallback lines"
        );

        let frames = frames(&received);
        assert_eq!(
            frames.len(),
            5,
            "open, one changed update, open, two closes"
        );
        let surface = RegionId::new(1);
        assert!(matches!(
            frames[0].first(),
            Some(AppMessage::CreateSurface { id, rows: 3, .. }) if *id == surface
        ));
        let first_regions = created(&frames[0]);
        assert_eq!(first_regions.len(), 3, "the column and its two texts");
        assert_eq!(first_regions[0].1, surface, "the view fills the surface");

        assert!(
            created(&frames[1]).is_empty(),
            "an update reuses the card's regions: {:?}",
            frames[1]
        );
        assert!(frames[1].iter().any(|m| matches!(
            m,
            AppMessage::AppendContent { id, text } | AppMessage::SetContent { id, content: ultty_protocol::Content::Text(ultty_protocol::TextContent { text, .. }) }
                if *id == first_regions[2].0 && text.contains("Compiling b")
        )));

        let second_surface = RegionId::new(5);
        assert!(matches!(
            frames[2].first(),
            Some(AppMessage::CreateSurface { id, rows: 2, .. }) if *id == second_surface
        ));
        for (id, _) in created(&frames[2]) {
            assert!(
                id != surface && id != second_surface && !first_regions.iter().any(|r| r.0 == id),
                "region ids are unique across cards: {id:?}"
            );
        }
        assert_eq!(frames[3], vec![AppMessage::CloseSurface { id: surface }]);
        assert_eq!(
            frames[4],
            vec![AppMessage::CloseSurface { id: second_surface }]
        );
    }
}
