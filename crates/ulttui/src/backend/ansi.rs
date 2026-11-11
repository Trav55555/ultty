//! ANSI fallback backend using Ratatui.

mod input;

use std::fs::File;
use std::io::{self, Stdout};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Once;

use crossterm::cursor::Show;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::event::Event;
use crate::widgets::View;
use input::InputReader;

type RatatuiTerminal = Terminal<CrosstermBackend<Stdout>>;

pub struct AnsiBackend {
    terminal: Option<RatatuiTerminal>,
    /// Where input is read: stdin, or `/dev/tty` when stdin is not a
    /// terminal (the choice crossterm makes too). `None` once it hung up.
    input: Option<Input>,
    reader: InputReader,
    /// The size last reported, to notice a resize (SIGWINCH wakes the loop).
    size: Option<(u16, u16)>,
}

enum Input {
    Stdin,
    DevTty(File),
}

impl AnsiBackend {
    pub fn new() -> Self {
        Self {
            terminal: None,
            input: None,
            reader: InputReader::default(),
            size: None,
        }
    }

    pub fn init(&mut self) -> crate::Result<()> {
        // SAFETY: isatty only inspects the descriptor.
        self.input = Some(if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
            Input::Stdin
        } else {
            Input::DevTty(File::open("/dev/tty")?)
        });
        install_panic_hook();
        terminal::enable_raw_mode()?;
        io::stdout().execute(EnterAlternateScreen)?;
        io::stdout().execute(crossterm::event::EnableMouseCapture)?;

        let backend = CrosstermBackend::new(io::stdout());
        self.terminal = Some(Terminal::new(backend)?);
        self.size = self.window_size();

        Ok(())
    }

    pub fn shutdown(&mut self) -> crate::Result<()> {
        if self.terminal.is_some() {
            restore_terminal()?;
            self.terminal = None;
        }
        Ok(())
    }

    pub fn render(&mut self, view: &impl View) -> crate::Result<()> {
        let terminal = self.terminal.as_mut().ok_or(crate::Error::NotInitialized)?;

        terminal.draw(|frame| {
            let area = frame.area();
            crate::widgets::render_view_ansi(view, frame, area);
        })?;

        Ok(())
    }

    /// The terminal's input, readable when a key (or other input) arrives.
    pub fn wait_fd(&self) -> Option<RawFd> {
        match self.input.as_ref()? {
            Input::Stdin => Some(libc::STDIN_FILENO),
            Input::DevTty(tty) => Some(tty.as_raw_fd()),
        }
    }

    /// Whether events were decoded that `poll_event` has not returned yet.
    pub fn has_pending_events(&mut self) -> crate::Result<bool> {
        Ok(self.reader.has_events())
    }

    /// The next event: a resize, or decoded input. Reads all the input that
    /// is waiting, and never waits for more.
    pub fn poll_event(&mut self) -> crate::Result<Option<Event>> {
        let size = self.window_size();
        if let Some((width, height)) = size.filter(|_| size != self.size) {
            self.size = size;
            return Ok(Some(Event::Resize { width, height }));
        }
        if !self.reader.has_events() {
            if let Some(fd) = self.wait_fd() {
                if !self.reader.read_available(fd)? {
                    tracing::info!("the terminal hung up; quitting");
                    self.input = None;
                    return Ok(Some(Event::Quit));
                }
            }
        }
        Ok(self.reader.next_event())
    }

    /// The terminal's size in cells, asked of the input side (stdout may be
    /// redirected).
    fn window_size(&self) -> Option<(u16, u16)> {
        let fd = self.wait_fd()?;
        // SAFETY: `winsize` is plain data that TIOCGWINSZ fills in.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is open and `ws` is a valid out-pointer.
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } != 0 || ws.ws_col == 0 {
            return None;
        }
        Some((ws.ws_col, ws.ws_row))
    }
}

impl Default for AnsiBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Put the terminal back the way we found it: mouse capture off, main screen,
/// cooked mode, cursor visible. Safe to call more than once and when nothing
/// was ever enabled.
pub fn restore_terminal() -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.execute(crossterm::event::DisableMouseCapture)?;
    stdout.execute(LeaveAlternateScreen)?;
    if terminal::is_raw_mode_enabled()? {
        terminal::disable_raw_mode()?;
    }
    stdout.execute(Show)?;
    Ok(())
}

/// Restore the terminal before the default panic output is printed.
///
/// The release profile uses `panic = "abort"`, so `Drop` impls never run on a
/// panic; without this hook the shell would be left in raw mode on the
/// alternate screen with the panic message invisible. Installed once per
/// process; later calls are no-ops.
pub fn install_panic_hook() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Err(err) = restore_terminal() {
                tracing::error!("failed to restore terminal in panic hook: {err}");
            }
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_terminal_is_idempotent_without_an_active_session() {
        // Nothing was enabled, so both calls must be no-ops that succeed.
        restore_terminal().unwrap();
        restore_terminal().unwrap();
    }

    #[test]
    fn install_panic_hook_is_installed_once_and_chains_previous_hook() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let previous_calls = Arc::new(AtomicUsize::new(0));
        let counter = previous_calls.clone();
        std::panic::set_hook(Box::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        install_panic_hook();
        install_panic_hook();

        let result = std::panic::catch_unwind(|| panic!("boom"));
        assert!(result.is_err());
        // Our hook restored the terminal and then called the previous hook
        // exactly once (the second install was a no-op).
        assert_eq!(previous_calls.load(Ordering::SeqCst), 1);
        let _ = std::panic::take_hook();
    }
}
