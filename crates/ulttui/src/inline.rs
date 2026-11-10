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
//! order with cards. Text another thread prints to stdout meanwhile can land
//! between a card's anchor and its fallback lines, and push the card off
//! its rows.
//!
//! **Keepalive.** Natively a background thread answers the terminal's
//! keepalive every second, so an app may sit in a long call (waiting for a
//! model, say) without its session timing out. If the session is lost
//! anyway (the terminal closed it, or a reply did not come within 5 s), the
//! console carries on in [`Mode::Fallback`] for good: open cards become
//! final where they stand, and new ones are printed as text. One slow reply
//! is enough.
//!
//! **Keys, in-band.** Over ssh the session runs in-band on the app's own
//! tty, and the console reads it for the terminal's replies whenever it is
//! called. Keys typed meanwhile are kept for [`Console::take_input`]; read
//! them there, not from stdin, while the console is native. Over the socket
//! the console never reads stdin.
//!
//! **Fallback redraws** move the cursor up over the card's lines, so they
//! assume the console saw everything on the screen since: output it did not
//! print (stderr, `tracing`, a subprocess) makes the card final only if it
//! went through the console. Send such output through [`Console::print`],
//! or expect a redraw to land on the wrong lines. When stdout is not a
//! terminal (a pipe, a log file) cards are never redrawn: each is written
//! once, as plain text, in its last state, when something is printed after
//! it, it is closed or the console is dropped, and lines end with `\n`. A
//! card still waiting is lost if the app ends with `std::process::exit`,
//! which skips `Drop`: close it, or drop the console, first.
//!
//! **Width changes.** Natively the terminal lays a live card out again at
//! the new width and clips or pads its rows; frozen cards are text and
//! re-wrap like any text. In fallback, a card is drawn at the width of the
//! moment; once the width changes it is final (a redraw by cursor movement
//! would land on rows the terminal may have re-wrapped). New cards use the
//! current width.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::io::{IsTerminal, Write};

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color as TuiColor, Modifier};
use ratatui::Terminal;
use ultty_client::UlttyClient;
use ultty_protocol::{
    AppMessage, Constraints, Dimension, RegionId, Style, TerminalMessage, CAP_APPEND_CONTENT,
    CAP_INLINE_SURFACES, CAP_SEMANTICS, MAX_SURFACE_ROWS, MIN_SURFACE_ROWS,
};

use crate::backend::native_diff::{
    begin_native_key_recording, optimize_frame_messages, take_native_keys, FrameCapabilities,
    RetainedTree,
};
use crate::widgets::{render_view_to_native, View};

/// What a [`CardEvent`] reports.
pub use ultty_protocol::SurfaceEventKind;

/// How often the keepalive thread reads from the terminal. The terminal
/// ends an in-band session after 10 s of silence, and a socket session that
/// does not answer a ping within 10 s.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);

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
    /// Card events from a session since lost, still to be reported.
    events: Vec<CardEvent>,
    /// Keys a lost in-band session had read, still to be taken.
    input: Vec<u8>,
}

enum Link {
    Native(NativeLink),
    Fallback(FallbackLink),
}

/// A native session, shared with the keepalive thread.
struct NativeLink {
    session: Arc<Mutex<Session>>,
    /// Dropping it wakes the keepalive thread to end.
    stop: Option<mpsc::Sender<()>>,
    keepalive: Option<JoinHandle<()>>,
    /// What the console becomes if the session is lost.
    fallback: Option<FallbackLink>,
}

struct Session {
    client: UlttyClient,
    /// Each live card's regions as the terminal holds them.
    cards: HashMap<RegionId, RetainedTree>,
    events: VecDeque<CardEvent>,
}

impl Session {
    /// Read what the terminal sent: answer pings (and owe the in-band
    /// heartbeat), keep card events, forget cards that take no more
    /// updates. False once the terminal has hung up.
    fn pump(&mut self) -> bool {
        let messages = match self.client.poll_events() {
            Ok(messages) => messages,
            Err(err) => {
                tracing::warn!("inline cards: reading from the terminal failed: {err}");
                return false;
            }
        };
        for message in messages {
            match message {
                TerminalMessage::SurfaceEvent { id, kind } => {
                    if matches!(kind, SurfaceEventKind::Closed | SurfaceEventKind::Evicted) {
                        self.cards.remove(&id);
                    }
                    self.events.push_back(CardEvent { card: id, kind });
                }
                TerminalMessage::Error { code, message } => {
                    tracing::warn!("terminal error {code}: {message}");
                }
                _ => {}
            }
        }
        !self.client.is_closed()
    }
}

impl NativeLink {
    fn session(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for NativeLink {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(keepalive) = self.keepalive.take() {
            if keepalive.join().is_err() {
                tracing::warn!("inline cards: the keepalive thread panicked");
            }
        }
    }
}

struct FallbackLink {
    out: Box<dyn Write + Send>,
    size: SizeSource,
    /// Whether `out` is a terminal, where a card can be redrawn. If not,
    /// the card waits in `pending` and is written once.
    redraw: bool,
    pending: Option<Pending>,
}

/// A card not yet written to a fallback that cannot redraw.
struct Pending {
    id: RegionId,
    lines: Vec<String>,
}

/// Where the fallback learns the terminal's size.
enum SizeSource {
    /// The terminal on stdout, asked each time (80 x 24 if it cannot say).
    Terminal,
    /// A fixed size: columns, lines.
    Fixed(u16, u16),
}

impl FallbackLink {
    fn stdout(out: Box<dyn Write + Send>) -> Self {
        Self {
            out,
            size: SizeSource::Terminal,
            redraw: std::io::stdout().is_terminal(),
            pending: None,
        }
    }

    fn size(&self) -> (u16, u16) {
        match self.size {
            SizeSource::Terminal => crossterm::terminal::size().unwrap_or((80, 24)),
            SizeSource::Fixed(cols, rows) => (cols, rows),
        }
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.out.write_all(bytes)?;
        self.out.flush()
    }

    /// Write the pending card, if any, as it stands.
    fn flush_pending(&mut self) -> std::io::Result<()> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        let mut text = String::new();
        for line in pending.lines {
            text.push_str(&line);
            text.push('\n');
        }
        self.write(text.as_bytes())
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
        if std::io::stdout().is_terminal() {
            match UlttyClient::connect() {
                Ok(client) if client.supports_capability(CAP_INLINE_SURFACES) => {
                    let fallback = FallbackLink::stdout(Box::new(std::io::stdout()));
                    return Self::native(client, fallback, KEEPALIVE_INTERVAL);
                }
                Ok(_) => tracing::debug!("the terminal does not offer inline surfaces"),
                Err(err) => tracing::debug!("no Ultty session: {err}"),
            }
        }
        Self::with_link(Link::Fallback(FallbackLink::stdout(Box::new(
            std::io::stdout(),
        ))))
    }

    /// A fallback console writing to `out` as if to a terminal of `cols`
    /// columns and `lines` lines.
    pub fn fallback(out: impl Write + Send + 'static, cols: u16, lines: u16) -> Self {
        Self::with_link(Link::Fallback(FallbackLink {
            out: Box::new(out),
            size: SizeSource::Fixed(cols, lines),
            redraw: true,
            pending: None,
        }))
    }

    /// A native console on `client`, read every `keepalive` by a thread of
    /// its own, carrying on as `fallback` if the session is lost.
    fn native(client: UlttyClient, fallback: FallbackLink, keepalive: Duration) -> Self {
        let session = Arc::new(Mutex::new(Session {
            client,
            cards: HashMap::new(),
            events: VecDeque::new(),
        }));
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = {
            let session = Arc::clone(&session);
            std::thread::Builder::new()
                .name("ulttui-keepalive".into())
                .spawn(move || {
                    // Ends when the console drops `stop`.
                    while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(keepalive) {
                        let mut session = session.lock().unwrap_or_else(PoisonError::into_inner);
                        if session.client.is_in_band() {
                            // In-band the terminal never pings, and reading
                            // the tty here would take keys from under the
                            // app; events wait for the console's next call.
                            if let Err(err) = session.client.heartbeat() {
                                tracing::debug!("inline cards: heartbeat failed: {err}");
                                break;
                            }
                        } else if !session.pump() {
                            break;
                        }
                    }
                })
        };
        let keepalive = match thread {
            Ok(handle) => Some(handle),
            Err(err) => {
                tracing::warn!("inline cards: no keepalive thread: {err}");
                None
            }
        };
        Self::with_link(Link::Native(NativeLink {
            session,
            stop: Some(stop),
            keepalive,
            fallback: Some(fallback),
        }))
    }

    fn with_link(link: Link) -> Self {
        Self {
            link,
            next_id: 1,
            last: Last::LineStart,
            events: Vec::new(),
            input: Vec::new(),
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
            Link::Native(native) => native.session().client.viewport_width(),
            Link::Fallback(fallback) => fallback.size().0,
        }
    }

    /// Print `text` as it is; each `\n` ends a line.
    pub fn print(&mut self, text: &str) -> crate::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.check_session();
        let lines = text.replace("\r\n", "\n").replace('\n', self.newline());
        self.write(lines.as_bytes())?;
        self.last = if text.ends_with('\n') {
            Last::LineStart
        } else {
            Last::MidLine
        };
        Ok(())
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
        // Reading first also takes in a resize, so the card gets the width
        // the terminal has now.
        self.check_session();
        if self.last == Last::MidLine {
            self.write(self.newline().as_bytes())?;
        }
        let width = self.width().max(1);
        let buffer = render_buffer(view, width, rows);
        let id = RegionId::new(self.next_id);
        self.next_id += 1;
        match &mut self.link {
            Link::Native(native) => {
                let fallback: Vec<String> = (0..rows).map(|y| plain_line(&buffer, y)).collect();
                let mut session = native.session();
                let mut tree = RetainedTree::new();
                let contents = surface_messages(
                    &mut self.next_id,
                    id,
                    view,
                    &mut tree,
                    frame_capabilities(&session.client),
                )?;
                let opened = session.client.open_surface(
                    id,
                    rows,
                    &fallback,
                    Constraints::default(),
                    Style::default(),
                    contents,
                );
                match opened {
                    Ok(()) => {
                        session.cards.insert(id, tree);
                    }
                    Err(err) => {
                        // The fallback text is out (unless stdout failed,
                        // which the next print reports): it is the card.
                        drop(session);
                        self.lose_session(&err);
                    }
                }
            }
            Link::Fallback(fallback) => {
                fallback.flush_pending()?;
                if fallback.redraw {
                    let mut text = String::new();
                    for y in 0..rows {
                        text.push_str(&ansi_line(&buffer, y));
                        text.push_str("\r\n");
                    }
                    fallback.write(text.as_bytes())?;
                } else {
                    let lines = (0..rows).map(|y| plain_line(&buffer, y)).collect();
                    fallback.pending = Some(Pending { id, lines });
                }
            }
        }
        self.last = Last::Card { id, rows, width };
        Ok(Card { id, rows })
    }

    /// Show `view` in `card` instead of what it showed.
    ///
    /// Natively, the view is diffed against the card's regions and only the
    /// changes are sent, in one frame; a card the terminal evicted or closed
    /// is left alone. In fallback, the card is redrawn in place with
    /// cursor-up only while it is the last thing the console printed, the
    /// width is unchanged and it is shorter than the screen; otherwise it is
    /// final and this does nothing. When stdout is not a terminal, the card
    /// is not written until something follows it, so this only changes what
    /// will be written.
    pub fn update(&mut self, card: &Card, view: &impl View) -> crate::Result<()> {
        self.check_session();
        match &mut self.link {
            Link::Native(native) => {
                let mut session = native.session();
                let capabilities = frame_capabilities(&session.client);
                let Some(tree) = session.cards.get_mut(&card.id) else {
                    return Ok(());
                };
                let messages =
                    surface_messages(&mut self.next_id, card.id, view, tree, capabilities)?;
                if messages.is_empty() {
                    return Ok(());
                }
                let sent = send_frame(&mut session.client, &messages);
                drop(session);
                if let Err(err) = sent {
                    self.lose_session(&err);
                }
            }
            Link::Fallback(fallback) => {
                let width = fallback.size().0.max(1);
                if let Some(pending) = fallback.pending.as_mut().filter(|p| p.id == card.id) {
                    let rows = card.rows;
                    let buffer = render_buffer(view, width, rows);
                    pending.lines = (0..rows).map(|y| plain_line(&buffer, y)).collect();
                    return Ok(());
                }
                let Last::Card { id, rows, width } = self.last else {
                    return Ok(());
                };
                if id != card.id || !fallback.redraw {
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
                fallback.write(text.as_bytes())?;
            }
        }
        Ok(())
    }

    /// Close `card`. Natively the terminal freezes it into its rows as
    /// text; in fallback its lines simply stay as last drawn (or, when
    /// stdout is not a terminal, are written now).
    pub fn close(&mut self, card: Card) -> crate::Result<()> {
        self.check_session();
        match &mut self.link {
            Link::Native(native) => {
                let mut session = native.session();
                if session.cards.remove(&card.id).is_some() {
                    let closed = session.client.close_surface(card.id);
                    drop(session);
                    if let Err(err) = closed {
                        self.lose_session(&err);
                    }
                }
            }
            Link::Fallback(fallback) => {
                if fallback.pending.as_ref().is_some_and(|p| p.id == card.id) {
                    fallback.flush_pending()?;
                }
            }
        }
        if matches!(self.last, Last::Card { id, .. } if id == card.id) {
            self.last = Last::LineStart;
        }
        Ok(())
    }

    /// Bytes typed on the terminal since the last call, in order.
    ///
    /// Natively in-band (over ssh, say) the console reads the app's own tty
    /// for the terminal's replies, so keys typed meanwhile arrive here and
    /// not on stdin; this also reads what is waiting now. Over the socket
    /// and in fallback the console never reads stdin, and this returns only
    /// what an in-band session held when it was lost.
    pub fn take_input(&mut self) -> Vec<u8> {
        let mut input = std::mem::take(&mut self.input);
        if let Link::Native(native) = &self.link {
            ultty_client::append_input(&mut input, &native.session().client.take_input());
        }
        input
    }

    /// What the terminal reported about cards since the last call (always
    /// empty in fallback, apart from what a lost session reported first).
    pub fn poll_events(&mut self) -> crate::Result<Vec<CardEvent>> {
        self.check_session();
        let mut events = std::mem::take(&mut self.events);
        if let Link::Native(native) = &self.link {
            events.extend(native.session().events.drain(..));
        }
        Ok(events)
    }

    /// Read from the terminal now, and fall back if the session is gone.
    fn check_session(&mut self) {
        let Link::Native(native) = &self.link else {
            return;
        };
        let alive = native.session().pump();
        if !alive {
            self.lose_session(&"the terminal closed the session");
        }
    }

    /// Carry on in fallback after the native session failed: live cards
    /// stay as the terminal froze them, and new output is plain text.
    fn lose_session(&mut self, why: &dyn std::fmt::Display) {
        let Link::Native(native) = &mut self.link else {
            return;
        };
        tracing::warn!("inline cards: lost the Ultty session ({why}); printing cards as text");
        let fallback = native
            .fallback
            .take()
            .unwrap_or_else(|| FallbackLink::stdout(Box::new(std::io::stdout())));
        let mut session = native.session();
        self.events.extend(session.events.drain(..));
        ultty_client::append_input(&mut self.input, &session.client.take_input());
        drop(session);
        self.link = Link::Fallback(fallback);
        // A live card can no longer be redrawn; the cursor is where the
        // session left it, at the start of a line.
        if matches!(self.last, Last::Card { .. }) {
            self.last = Last::LineStart;
        }
    }

    /// How a line ends: `\r\n` on a terminal (in-band it is in raw mode),
    /// `\n` in a file or pipe.
    fn newline(&self) -> &'static str {
        match &self.link {
            Link::Fallback(fallback) if !fallback.redraw => "\n",
            _ => "\r\n",
        }
    }

    fn write(&mut self, bytes: &[u8]) -> crate::Result<()> {
        match &mut self.link {
            Link::Native(native) => native.session().client.write_output(bytes)?,
            Link::Fallback(fallback) => {
                fallback.flush_pending()?;
                fallback.write(bytes)?;
            }
        }
        Ok(())
    }
}

impl Drop for Console {
    /// Write a card still waiting for a fallback that cannot redraw.
    fn drop(&mut self) {
        if let Link::Fallback(fallback) = &mut self.link {
            if let Err(err) = fallback.flush_pending() {
                tracing::debug!("inline cards: writing the last card failed: {err}");
            }
        }
    }
}

/// What the card diff may send to the terminal `client` is connected to.
fn frame_capabilities(client: &UlttyClient) -> FrameCapabilities {
    FrameCapabilities {
        append_content: client.supports_capability(CAP_APPEND_CONTENT),
        semantics: client.supports_capability(CAP_SEMANTICS),
    }
}

/// Send `messages` as one frame and wait for its layout.
fn send_frame(
    client: &mut UlttyClient,
    messages: &[AppMessage],
) -> Result<(), ultty_client::Error> {
    let frame = client.begin_frame()?;
    for message in messages {
        client.send(message)?;
    }
    client.end_frame(frame)
}

/// The messages that turn `tree` (the card's regions as the terminal holds
/// them) into `view`, which fills surface `surface`. Updates `tree`, and
/// `next_id` past every id the tree now uses.
fn surface_messages(
    next_id: &mut u32,
    surface: RegionId,
    view: &impl View,
    tree: &mut RetainedTree,
    capabilities: FrameCapabilities,
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
    let (outgoing, next) = optimize_frame_messages(tree, messages, &keys, capabilities);
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

/// A cell's symbol without control characters, which a view's text could
/// carry into the output as escape sequences; a cell left empty is a space.
fn printable(symbol: &str) -> Cow<'_, str> {
    if !symbol.chars().any(char::is_control) {
        return Cow::Borrowed(symbol);
    }
    let kept: String = symbol.chars().filter(|ch| !ch.is_control()).collect();
    Cow::Owned(if kept.is_empty() { " ".into() } else { kept })
}

/// Line `y` as plain text, without trailing blanks.
fn plain_line(buffer: &Buffer, y: u16) -> String {
    let text: String = line_cells(buffer, y)
        .iter()
        .map(|c| printable(c.symbol()))
        .collect();
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
        line.push_str(&printable(cell.symbol()));
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
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

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
    struct Output(Arc<Mutex<Vec<u8>>>);

    impl Output {
        fn take(&self) -> String {
            String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
        }
    }

    impl Write for Output {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
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

    /// A fallback console whose output is not a terminal (a pipe, a log).
    fn log_console(cols: u16) -> (Console, Output) {
        let output = Output::default();
        let link = FallbackLink {
            out: Box::new(output.clone()),
            size: SizeSource::Fixed(cols, 24),
            redraw: false,
            pending: None,
        };
        (Console::with_link(Link::Fallback(link)), output)
    }

    #[test]
    fn without_a_terminal_a_card_is_written_once_in_its_last_state() {
        let (mut console, output) = log_console(20);
        console.println("before").unwrap();
        let card = console.open(2, &Text::new("queued")).unwrap();
        console.update(&card, &Text::new("running")).unwrap();
        console.update(&card, &Text::new("done\nok")).unwrap();
        assert_eq!(output.take(), "before\n", "the card waits");
        console.println("after").unwrap();
        // Once something follows it, the card is final.
        console.update(&card, &Text::new("too late")).unwrap();
        console.close(card).unwrap();
        assert_eq!(
            output.take(),
            "done\nok\nafter\n",
            "LF, not CRLF, in a file"
        );
    }

    #[test]
    fn without_a_terminal_a_card_is_written_on_close_or_drop() {
        let (mut console, output) = log_console(20);
        let first = console.open(1, &Text::new("first")).unwrap();
        console.update(&first, &Text::new("first, done")).unwrap();
        console.close(first).unwrap();
        assert_eq!(output.take(), "first, done\n");
        console.open(1, &Text::new("second")).unwrap();
        drop(console);
        assert_eq!(output.take(), "second\n", "never left unwritten");
    }

    #[test]
    fn control_characters_in_a_view_do_not_reach_the_output() {
        let (mut console, output) = fallback_console(20, 24);
        console.open(1, &Text::new("a\x1b]0;x\x07b")).unwrap();
        let text = output.take();
        assert!(!text.contains('\x1b') && !text.contains('\x07'), "{text:?}");
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

    /// What the fake Ultty does besides answering the handshake (session 5,
    /// 40 x 10, every capability) and every `EndFrame`.
    #[derive(Clone, Copy, PartialEq)]
    enum Script {
        Plain,
        /// Send a `Ping` after answering the first frame.
        PingAfterFirstFrame,
        /// Hang up after answering the first frame.
        HangUpAfterFirstFrame,
        /// Report a resize to this width right after the handshake.
        ResizeTo(u16),
    }

    /// A fake Ultty on a socket, playing `script`; returns all it received.
    fn fake_ultty(
        listener: UnixListener,
        script: Script,
        seen: mpsc::Sender<AppMessage>,
    ) -> std::thread::JoinHandle<Vec<AppMessage>> {
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
                let first_frame = !received
                    .iter()
                    .any(|m| matches!(m, AppMessage::EndFrame { .. }));
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
                        if let Script::ResizeTo(width) = script {
                            send(
                                &mut stream,
                                TerminalMessage::Resize(ultty_protocol::ResizeEvent {
                                    width,
                                    height: 10,
                                }),
                            );
                        }
                    }
                    AppMessage::EndFrame { frame_id } => {
                        send(&mut stream, layout_reply(*frame_id));
                        if first_frame && script == Script::PingAfterFirstFrame {
                            send(&mut stream, TerminalMessage::Ping);
                        }
                        if first_frame && script == Script::HangUpAfterFirstFrame {
                            received.push(msg);
                            return received;
                        }
                    }
                    _ => {}
                }
                let _ = seen.send(msg.clone());
                received.push(msg);
            }
        })
    }

    fn layout_reply(frame_id: FrameId) -> TerminalMessage {
        TerminalMessage::LayoutComputed {
            frame_id: Some(frame_id),
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
        }
    }

    /// A native console against a fake Ultty playing `script`, keeping the
    /// session alive every `keepalive`. Its anchors and text, and what it
    /// prints after falling back, go to the returned output.
    struct NativeRig {
        console: Console,
        output: Output,
        terminal: std::thread::JoinHandle<Vec<AppMessage>>,
        /// Each message as the terminal receives it.
        seen: mpsc::Receiver<AppMessage>,
        dir: std::path::PathBuf,
    }

    impl NativeRig {
        fn new(name: &str, script: Script, keepalive: Duration) -> Self {
            let dir =
                std::env::temp_dir().join(format!("ulttui-inline-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("s");
            let _ = std::fs::remove_file(&path);
            let (tx, seen) = mpsc::channel();
            let terminal = fake_ultty(UnixListener::bind(&path).unwrap(), script, tx);
            let output = Output::default();
            let mut client = UlttyClient::connect_to(&path).unwrap();
            client.set_output(Box::new(output.clone()));
            // As if stdout were a log, so lines end with LF however the
            // tests are run.
            let fallback = FallbackLink {
                out: Box::new(output.clone()),
                size: SizeSource::Fixed(40, 10),
                redraw: false,
                pending: None,
            };
            let console = Console::native(client, fallback, keepalive);
            Self {
                console,
                output,
                terminal,
                seen,
                dir,
            }
        }

        /// Drop the console and return what the terminal received.
        fn finish(self) -> (Vec<AppMessage>, Output) {
            drop(self.console);
            let received = self.terminal.join().unwrap();
            std::fs::remove_dir_all(&self.dir).unwrap();
            (received, self.output)
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
    fn console_is_send() {
        fn send<T: Send>() {}
        send::<Console>();
    }

    #[test]
    fn native_cards_open_with_their_view_update_by_diff_and_close() {
        let mut rig = NativeRig::new("cards", Script::Plain, KEEPALIVE_INTERVAL);
        let console = &mut rig.console;
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
        let (received, output) = rig.finish();

        assert_eq!(
            output.take(),
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

    #[test]
    fn an_idle_console_answers_the_keepalive_itself() {
        let mut rig = NativeRig::new(
            "keepalive",
            Script::PingAfterFirstFrame,
            Duration::from_millis(10),
        );
        let card = rig.console.open(1, &Text::new("one")).unwrap();
        // Idle, as while waiting for a model: no call into the console
        // until the terminal has its pong.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut answered = false;
        while !answered {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match rig.seen.recv_timeout(left) {
                Ok(msg) => answered = msg == AppMessage::Pong,
                Err(_) => panic!("the ping was not answered while the console was idle"),
            }
        }
        rig.console.update(&card, &Text::new("two")).unwrap();
        assert_eq!(rig.console.mode(), Mode::Native);
        let (received, _) = rig.finish();
        assert_eq!(
            frames(&received).len(),
            2,
            "the update went out: {received:?}"
        );
    }

    #[test]
    fn a_lost_session_falls_back_and_cards_become_final() {
        let mut rig = NativeRig::new(
            "lost",
            Script::HangUpAfterFirstFrame,
            Duration::from_millis(10),
        );
        let card = rig.console.open(1, &Text::new("one")).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        rig.output.take();

        rig.console.update(&card, &Text::new("two")).unwrap();
        assert_eq!(rig.console.mode(), Mode::Fallback);
        rig.console.close(card).unwrap();
        rig.console.println("after").unwrap();
        let (_, output) = rig.finish();
        assert_eq!(
            output.take(),
            "after\n",
            "the card stays as the terminal froze it; printing carries on"
        );
    }

    #[test]
    fn a_card_takes_the_width_of_a_resize_that_just_arrived() {
        let mut rig = NativeRig::new("resize", Script::ResizeTo(6), Duration::from_secs(60));
        // The resize is on the socket, not yet read by anyone.
        std::thread::sleep(Duration::from_millis(100));
        rig.console.open(1, &Text::new("abcdefghij")).unwrap();
        let (_, output) = rig.finish();
        assert_eq!(output.take(), "\x1b_u5;1;1\x1b\\abcdef\r\n");
    }

    /// Keys typed while an in-band console sits idle reach the app intact
    /// and in order, with the terminal's in-band strings taken out.
    #[test]
    fn keys_typed_during_an_idle_in_band_console_reach_the_app() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use ultty_protocol::in_band::encode_to_vec;

        let (mut master, slave) = {
            let (mut m, mut s) = (0, 0);
            // SAFETY: openpty writes two fds; the other arguments may be null.
            let rc = unsafe {
                libc::openpty(
                    &mut m,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            assert_eq!(rc, 0, "openpty");
            // SAFETY: both fds were just opened and are owned here.
            unsafe {
                (
                    std::fs::File::from(OwnedFd::from_raw_fd(m)),
                    OwnedFd::from_raw_fd(s),
                )
            }
        };
        let (typed_tx, typed_rx) = mpsc::channel::<()>();
        let (connected_tx, connected_rx) = mpsc::channel::<()>();
        let fake = std::thread::spawn(move || {
            // Answer the probe as Ultty does: its name, then the hello reply
            // before DA1.
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            let da1 = |seen: &[u8]| seen.windows(3).filter(|w| w == b"\x1b[c").count();
            while da1(&seen) < 1 {
                let n = master.read(&mut buf).unwrap();
                seen.extend_from_slice(&buf[..n]);
            }
            master
                .write_all(b"\x1bP>|ultty 0.1.0\x1b\\\x1b[?62;22c")
                .unwrap();
            while da1(&seen) < 2 {
                let n = master.read(&mut buf).unwrap();
                seen.extend_from_slice(&buf[..n]);
            }
            let mut hello = Vec::new();
            for msg in [
                TerminalMessage::ServerHello {
                    version: PROTOCOL_VERSION,
                    capabilities: CLIENT_DEFAULT_CAPABILITIES,
                },
                TerminalMessage::SessionReady {
                    session_id: 9,
                    root_region: RegionId::ROOT,
                    width: 40,
                    height: 10,
                },
            ] {
                hello.extend_from_slice(&msg.encode_to_bytes());
            }
            let mut wire = encode_to_vec(&hello);
            wire.extend_from_slice(b"\x1b[?62;22c");
            master.write_all(&wire).unwrap();

            // Type once the session is up (keys typed during the probe are
            // the probe's), with one of the terminal's strings in the middle.
            connected_rx.recv().unwrap();
            let event = TerminalMessage::SurfaceEvent {
                id: RegionId::new(3),
                kind: SurfaceEventKind::Shown,
            };
            let mut typed = b"y\r".to_vec();
            typed.extend(encode_to_vec(&event.encode_to_bytes()));
            typed.extend_from_slice(b"abc\x1b[A");
            master.write_all(&typed).unwrap();
            typed_tx.send(()).unwrap();
            // Drain what the app writes (heartbeats) until it hangs up.
            while matches!(master.read(&mut buf), Ok(n) if n > 0) {}
        });

        let tty = ultty_client::in_band::InBandTty::open_on(slave.as_raw_fd(), slave.as_raw_fd())
            .unwrap();
        let client = UlttyClient::connect_in_band_on(tty).unwrap();
        connected_tx.send(()).unwrap();
        let fallback = FallbackLink::stdout(Box::new(Output::default()));
        let mut console = Console::native(client, fallback, Duration::from_millis(10));
        typed_rx.recv().unwrap();
        // Idle while the keepalive thread runs many times.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(console.mode(), Mode::Native);
        assert_eq!(console.take_input(), b"y\rabc\x1b[A");
        assert_eq!(
            console.poll_events().unwrap(),
            vec![CardEvent {
                card: RegionId::new(3),
                kind: SurfaceEventKind::Shown
            }]
        );
        drop(console);
        drop(slave);
        fake.join().unwrap();
    }
}
