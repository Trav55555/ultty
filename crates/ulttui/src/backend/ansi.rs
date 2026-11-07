//! ANSI fallback backend using Ratatui.

use std::io::{self, Stdout};
use std::sync::Once;
use std::time::Duration;

use crossterm::cursor::Show;
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::event::{Event, Key, KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind};
use crate::widgets::View;

type RatatuiTerminal = Terminal<CrosstermBackend<Stdout>>;

pub struct AnsiBackend {
    terminal: Option<RatatuiTerminal>,
}

impl AnsiBackend {
    pub fn new() -> Self {
        Self { terminal: None }
    }

    pub fn init(&mut self) -> crate::Result<()> {
        install_panic_hook();
        terminal::enable_raw_mode()?;
        io::stdout().execute(EnterAlternateScreen)?;
        io::stdout().execute(crossterm::event::EnableMouseCapture)?;

        let backend = CrosstermBackend::new(io::stdout());
        self.terminal = Some(Terminal::new(backend)?);

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
            view.render_ansi(frame, area);
        })?;

        Ok(())
    }

    pub fn poll_event(&mut self) -> crate::Result<Option<Event>> {
        if event::poll(Duration::from_millis(16))? {
            let crossterm_event = event::read()?;
            Ok(convert_event(crossterm_event))
        } else {
            Ok(None)
        }
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

fn convert_event(event: CrosstermEvent) -> Option<Event> {
    match event {
        CrosstermEvent::Key(key) => {
            let modifiers = convert_modifiers(key.modifiers);

            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return Some(Event::Quit);
            }

            let key_code = match key.code {
                KeyCode::Char(c) => Key::Char(c),
                KeyCode::Enter => Key::Enter,
                KeyCode::Esc => Key::Escape,
                KeyCode::Backspace => Key::Backspace,
                KeyCode::Tab => Key::Tab,
                KeyCode::Up => Key::Up,
                KeyCode::Down => Key::Down,
                KeyCode::Left => Key::Left,
                KeyCode::Right => Key::Right,
                KeyCode::Home => Key::Home,
                KeyCode::End => Key::End,
                KeyCode::PageUp => Key::PageUp,
                KeyCode::PageDown => Key::PageDown,
                KeyCode::Delete => Key::Delete,
                KeyCode::Insert => Key::Insert,
                KeyCode::F(n) => Key::F(n),
                _ => return None,
            };

            Some(Event::Key(KeyEvent {
                key: key_code,
                modifiers,
                focused_id: None,
            }))
        }
        CrosstermEvent::Mouse(mouse) => {
            let modifiers = convert_modifiers(mouse.modifiers);
            let kind = match mouse.kind {
                event::MouseEventKind::Down(btn) => MouseEventKind::Down(convert_mouse_button(btn)),
                event::MouseEventKind::Up(btn) => MouseEventKind::Up(convert_mouse_button(btn)),
                event::MouseEventKind::Drag(btn) => MouseEventKind::Drag(convert_mouse_button(btn)),
                event::MouseEventKind::Moved => MouseEventKind::Moved,
                event::MouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
                event::MouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
                _ => return None,
            };

            Some(Event::Mouse(MouseEvent {
                kind,
                x: mouse.column,
                y: mouse.row,
                modifiers,
            }))
        }
        CrosstermEvent::Resize(width, height) => Some(Event::Resize { width, height }),
        _ => None,
    }
}

fn convert_modifiers(mods: KeyModifiers) -> Modifiers {
    Modifiers {
        shift: mods.contains(KeyModifiers::SHIFT),
        ctrl: mods.contains(KeyModifiers::CONTROL),
        alt: mods.contains(KeyModifiers::ALT),
        meta: mods.contains(KeyModifiers::META),
    }
}

fn convert_mouse_button(btn: event::MouseButton) -> MouseButton {
    match btn {
        event::MouseButton::Left => MouseButton::Left,
        event::MouseButton::Right => MouseButton::Right,
        event::MouseButton::Middle => MouseButton::Middle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent as CtKeyEvent, KeyEventKind, KeyEventState};

    fn make_key_event(code: KeyCode, modifiers: KeyModifiers) -> CrosstermEvent {
        CrosstermEvent::Key(CtKeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

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

    #[test]
    fn ctrl_c_produces_quit() {
        let event = make_key_event(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let result = convert_event(event);
        assert!(matches!(result, Some(Event::Quit)));
    }

    #[test]
    fn escape_key_converts() {
        let event = make_key_event(KeyCode::Esc, KeyModifiers::NONE);
        let result = convert_event(event).unwrap();
        match result {
            Event::Key(ke) => assert!(matches!(ke.key, Key::Escape)),
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn char_key_converts() {
        let event = make_key_event(KeyCode::Char('a'), KeyModifiers::NONE);
        let result = convert_event(event).unwrap();
        match result {
            Event::Key(ke) => assert!(matches!(ke.key, Key::Char('a'))),
            _ => panic!("Expected Key event"),
        }
    }

    #[test]
    fn function_keys_convert() {
        for n in 1..=12 {
            let event = make_key_event(KeyCode::F(n), KeyModifiers::NONE);
            let result = convert_event(event).unwrap();
            match result {
                Event::Key(ke) => assert!(matches!(ke.key, Key::F(x) if x == n)),
                _ => panic!("Expected Key event for F{}", n),
            }
        }
    }

    #[test]
    fn arrow_keys_convert() {
        let cases = [
            (KeyCode::Up, Key::Up),
            (KeyCode::Down, Key::Down),
            (KeyCode::Left, Key::Left),
            (KeyCode::Right, Key::Right),
        ];
        for (code, expected) in cases {
            let event = make_key_event(code, KeyModifiers::NONE);
            let result = convert_event(event).unwrap();
            match result {
                Event::Key(ke) => assert_eq!(
                    std::mem::discriminant(&ke.key),
                    std::mem::discriminant(&expected)
                ),
                _ => panic!("Expected Key event"),
            }
        }
    }

    #[test]
    fn navigation_keys_convert() {
        let cases = [
            (KeyCode::Home, Key::Home),
            (KeyCode::End, Key::End),
            (KeyCode::PageUp, Key::PageUp),
            (KeyCode::PageDown, Key::PageDown),
            (KeyCode::Insert, Key::Insert),
            (KeyCode::Delete, Key::Delete),
        ];
        for (code, expected) in cases {
            let event = make_key_event(code, KeyModifiers::NONE);
            let result = convert_event(event).unwrap();
            match result {
                Event::Key(ke) => assert_eq!(
                    std::mem::discriminant(&ke.key),
                    std::mem::discriminant(&expected)
                ),
                _ => panic!("Expected Key event"),
            }
        }
    }

    #[test]
    fn modifiers_convert_correctly() {
        let all_mods =
            KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::META;
        let result = convert_modifiers(all_mods);
        assert!(result.shift);
        assert!(result.ctrl);
        assert!(result.alt);
        assert!(result.meta);

        let no_mods = convert_modifiers(KeyModifiers::NONE);
        assert!(!no_mods.shift);
        assert!(!no_mods.ctrl);
        assert!(!no_mods.alt);
        assert!(!no_mods.meta);
    }

    #[test]
    fn shift_only_modifier() {
        let result = convert_modifiers(KeyModifiers::SHIFT);
        assert!(result.shift);
        assert!(!result.ctrl);
        assert!(!result.alt);
        assert!(!result.meta);
    }

    #[test]
    fn mouse_buttons_convert() {
        assert!(matches!(
            convert_mouse_button(event::MouseButton::Left),
            MouseButton::Left
        ));
        assert!(matches!(
            convert_mouse_button(event::MouseButton::Right),
            MouseButton::Right
        ));
        assert!(matches!(
            convert_mouse_button(event::MouseButton::Middle),
            MouseButton::Middle
        ));
    }

    #[test]
    fn resize_event_converts() {
        let event = CrosstermEvent::Resize(120, 40);
        let result = convert_event(event).unwrap();
        match result {
            Event::Resize { width, height } => {
                assert_eq!(width, 120);
                assert_eq!(height, 40);
            }
            _ => panic!("Expected Resize event"),
        }
    }

    #[test]
    fn mouse_down_converts() {
        let event = CrosstermEvent::Mouse(event::MouseEvent {
            kind: event::MouseEventKind::Down(event::MouseButton::Left),
            column: 10,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        let result = convert_event(event).unwrap();
        match result {
            Event::Mouse(me) => {
                assert!(matches!(me.kind, MouseEventKind::Down(MouseButton::Left)));
                assert_eq!(me.x, 10);
                assert_eq!(me.y, 5);
            }
            _ => panic!("Expected Mouse event"),
        }
    }

    #[test]
    fn mouse_scroll_converts() {
        let scroll_up = CrosstermEvent::Mouse(event::MouseEvent {
            kind: event::MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let result = convert_event(scroll_up).unwrap();
        match result {
            Event::Mouse(me) => assert!(matches!(me.kind, MouseEventKind::ScrollUp)),
            _ => panic!("Expected Mouse event"),
        }

        let scroll_down = CrosstermEvent::Mouse(event::MouseEvent {
            kind: event::MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let result = convert_event(scroll_down).unwrap();
        match result {
            Event::Mouse(me) => assert!(matches!(me.kind, MouseEventKind::ScrollDown)),
            _ => panic!("Expected Mouse event"),
        }
    }

    #[test]
    fn unknown_key_returns_none() {
        let event = make_key_event(KeyCode::Null, KeyModifiers::NONE);
        let result = convert_event(event);
        assert!(result.is_none());
    }

    #[test]
    fn focus_events_return_none() {
        let focus_gained = CrosstermEvent::FocusGained;
        let focus_lost = CrosstermEvent::FocusLost;
        assert!(convert_event(focus_gained).is_none());
        assert!(convert_event(focus_lost).is_none());
    }
}
