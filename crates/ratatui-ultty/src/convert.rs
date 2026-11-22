//! Conversions between ratatui/crossterm types and Ultty wire protocol types.

use crossterm::event::{
    Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
    MouseButton as CtMouseButton, MouseEvent, MouseEventKind,
};
use ratatui::buffer::Buffer;
use ratatui::style::{Color as RatColor, Modifier};
use ultty_protocol::{
    Color as ProtoColor, InputEvent, InputType, Key, Modifiers, MouseButton, NamedColor, RichText,
    Span, Style, TerminalMessage,
};

/// Convert a ratatui color into an Ultty protocol color.
pub(crate) fn ratatui_color_to_proto(color: RatColor) -> Option<ProtoColor> {
    match color {
        RatColor::Reset => None,
        RatColor::Black => Some(ProtoColor::Named(NamedColor::Black)),
        RatColor::Red => Some(ProtoColor::Named(NamedColor::Red)),
        RatColor::Green => Some(ProtoColor::Named(NamedColor::Green)),
        RatColor::Yellow => Some(ProtoColor::Named(NamedColor::Yellow)),
        RatColor::Blue => Some(ProtoColor::Named(NamedColor::Blue)),
        RatColor::Magenta => Some(ProtoColor::Named(NamedColor::Magenta)),
        RatColor::Cyan => Some(ProtoColor::Named(NamedColor::Cyan)),
        RatColor::Gray => Some(ProtoColor::Named(NamedColor::White)),
        RatColor::DarkGray => Some(ProtoColor::Named(NamedColor::BrightBlack)),
        RatColor::LightRed => Some(ProtoColor::Named(NamedColor::BrightRed)),
        RatColor::LightGreen => Some(ProtoColor::Named(NamedColor::BrightGreen)),
        RatColor::LightYellow => Some(ProtoColor::Named(NamedColor::BrightYellow)),
        RatColor::LightBlue => Some(ProtoColor::Named(NamedColor::BrightBlue)),
        RatColor::LightMagenta => Some(ProtoColor::Named(NamedColor::BrightMagenta)),
        RatColor::LightCyan => Some(ProtoColor::Named(NamedColor::BrightCyan)),
        RatColor::White => Some(ProtoColor::Named(NamedColor::BrightWhite)),
        RatColor::Rgb(r, g, b) => Some(ProtoColor::Rgb(r, g, b)),
        RatColor::Indexed(i) => Some(ProtoColor::Indexed(i)),
    }
}

/// Convert an Ultty protocol color into a ratatui color for ANSI fallback rendering.
pub(crate) fn proto_color_to_ratatui(color: ProtoColor) -> RatColor {
    match color {
        ProtoColor::Rgb(r, g, b) => RatColor::Rgb(r, g, b),
        ProtoColor::Indexed(i) => RatColor::Indexed(i),
        ProtoColor::Named(named) => match named {
            NamedColor::Black => RatColor::Black,
            NamedColor::Red => RatColor::Red,
            NamedColor::Green => RatColor::Green,
            NamedColor::Yellow => RatColor::Yellow,
            NamedColor::Blue => RatColor::Blue,
            NamedColor::Magenta => RatColor::Magenta,
            NamedColor::Cyan => RatColor::Cyan,
            NamedColor::White => RatColor::Gray,
            NamedColor::BrightBlack => RatColor::DarkGray,
            NamedColor::BrightRed => RatColor::LightRed,
            NamedColor::BrightGreen => RatColor::LightGreen,
            NamedColor::BrightYellow => RatColor::LightYellow,
            NamedColor::BrightBlue => RatColor::LightBlue,
            NamedColor::BrightMagenta => RatColor::LightMagenta,
            NamedColor::BrightCyan => RatColor::LightCyan,
            NamedColor::BrightWhite => RatColor::White,
            NamedColor::Foreground | NamedColor::Background => RatColor::Reset,
        },
    }
}

/// Convert a rendered ratatui `Buffer` into coalesced Ultty `RichText` spans.
///
/// Adjacent cells with identical styles are merged into a single span to keep
/// wire frames compact. Wide-character continuation cells are skipped.
pub(crate) fn buffer_to_rich_text(buffer: &Buffer) -> RichText {
    let area = buffer.area;
    let mut spans: Vec<Span> = Vec::new();
    let mut current_style: Option<Style> = None;
    let mut current_text = String::new();

    let flush = |spans: &mut Vec<Span>, style: &mut Option<Style>, text: &mut String| {
        if !text.is_empty() {
            spans.push(Span {
                text: std::mem::take(text),
                style: style.take().unwrap_or_default(),
            });
        }
    };

    for y in area.top()..area.bottom() {
        if y > area.top() {
            flush(&mut spans, &mut current_style, &mut current_text);
            spans.push(Span {
                text: "\n".to_string(),
                style: Style::default(),
            });
        }
        let mut skip = 0usize;
        for x in area.left()..area.right() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buffer[(x, y)];
            let symbol = cell.symbol();
            let width = unicode_width::UnicodeWidthStr::width(symbol);
            if width > 1 {
                skip = width - 1;
            }
            let reversed = cell.modifier.contains(Modifier::REVERSED);
            let fg_raw = if reversed { cell.bg } else { cell.fg };
            let bg_raw = if reversed { cell.fg } else { cell.bg };
            let style = Style {
                fg: ratatui_color_to_proto(fg_raw),
                bg: ratatui_color_to_proto(bg_raw),
                bold: cell.modifier.contains(Modifier::BOLD),
                italic: cell.modifier.contains(Modifier::ITALIC),
                underline: cell.modifier.contains(Modifier::UNDERLINED),
                dim: cell.modifier.contains(Modifier::DIM),
                ..Style::default()
            };
            if current_style.as_ref() != Some(&style) {
                flush(&mut spans, &mut current_style, &mut current_text);
                current_style = Some(style);
            }
            if symbol.is_empty() {
                current_text.push(' ');
            } else {
                current_text.push_str(symbol);
            }
        }
    }
    flush(&mut spans, &mut current_style, &mut current_text);

    RichText { spans }
}

/// Convert an Ultty `TerminalMessage` into a `crossterm::event::Event` when applicable.
pub(crate) fn terminal_message_to_crossterm(msg: &TerminalMessage) -> Option<CtEvent> {
    match msg {
        TerminalMessage::Resize(r) => Some(CtEvent::Resize(r.width, r.height)),
        TerminalMessage::Input(input) => input_event_to_crossterm(input),
        _ => None,
    }
}

fn proto_mods_to_crossterm(mods: Modifiers) -> KeyModifiers {
    let mut out = KeyModifiers::NONE;
    if mods.shift() {
        out |= KeyModifiers::SHIFT;
    }
    if mods.ctrl() {
        out |= KeyModifiers::CONTROL;
    }
    if mods.alt() {
        out |= KeyModifiers::ALT;
    }
    if mods.meta() {
        out |= KeyModifiers::SUPER;
    }
    out
}

fn proto_key_to_crossterm(key: &Key) -> Option<KeyCode> {
    match key {
        Key::Char('\0') => None,
        Key::Char(c) => Some(KeyCode::Char(*c)),
        Key::Function(n) => Some(KeyCode::F(*n)),
        Key::Escape => Some(KeyCode::Esc),
        Key::Enter => Some(KeyCode::Enter),
        Key::Tab => Some(KeyCode::Tab),
        Key::Backspace => Some(KeyCode::Backspace),
        Key::Delete => Some(KeyCode::Delete),
        Key::Insert => Some(KeyCode::Insert),
        Key::Home => Some(KeyCode::Home),
        Key::End => Some(KeyCode::End),
        Key::PageUp => Some(KeyCode::PageUp),
        Key::PageDown => Some(KeyCode::PageDown),
        Key::Up => Some(KeyCode::Up),
        Key::Down => Some(KeyCode::Down),
        Key::Left => Some(KeyCode::Left),
        Key::Right => Some(KeyCode::Right),
    }
}

fn proto_mouse_button(button: MouseButton) -> CtMouseButton {
    match button {
        MouseButton::Left | MouseButton::Button4 | MouseButton::Button5 => CtMouseButton::Left,
        MouseButton::Middle => CtMouseButton::Middle,
        MouseButton::Right => CtMouseButton::Right,
    }
}

fn input_event_to_crossterm(input: &InputEvent) -> Option<CtEvent> {
    let col = input.x.max(0.0) as u16;
    let row = input.y.max(0.0) as u16;
    let base_mods = proto_mods_to_crossterm(input.modifiers);

    match &input.event_type {
        InputType::KeyDown { key, modifiers } => {
            let code = proto_key_to_crossterm(key)?;
            Some(CtEvent::Key(KeyEvent {
                code,
                modifiers: proto_mods_to_crossterm(*modifiers),
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }))
        }
        InputType::KeyUp { .. } => None,
        InputType::TextInput { text } => {
            let ch = text.chars().next()?;
            Some(CtEvent::Key(KeyEvent {
                code: KeyCode::Char(ch),
                modifiers: base_mods,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }))
        }
        InputType::MouseDown { button } => Some(CtEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(proto_mouse_button(*button)),
            column: col,
            row,
            modifiers: base_mods,
        })),
        InputType::MouseUp { button } => Some(CtEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Up(proto_mouse_button(*button)),
            column: col,
            row,
            modifiers: base_mods,
        })),
        InputType::MouseMove => Some(CtEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: base_mods,
        })),
        InputType::MouseScroll { delta_y, .. } => {
            let kind = if *delta_y < 0.0 {
                MouseEventKind::ScrollUp
            } else {
                MouseEventKind::ScrollDown
            };
            Some(CtEvent::Mouse(MouseEvent {
                kind,
                column: col,
                row,
                modifiers: base_mods,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style as RatStyle;
    use ultty_protocol::{RegionId, ResizeEvent};

    #[test]
    fn test_buffer_to_rich_text_coalesces_adjacent_cells() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 2));
        buf.set_string(0, 0, "Hi", RatStyle::default().fg(RatColor::Cyan));
        buf.set_string(0, 1, "Ok", RatStyle::default());

        let rich = buffer_to_rich_text(&buf);
        let combined: String = rich.spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(combined, "Hi   \nOk   ");
        assert_eq!(rich.spans[0].text, "Hi");
        assert_eq!(
            rich.spans[0].style.fg,
            Some(ProtoColor::Named(NamedColor::Cyan))
        );
    }

    #[test]
    fn test_terminal_message_to_crossterm_key_and_resize() {
        let resize = TerminalMessage::Resize(ResizeEvent {
            width: 100,
            height: 30,
        });
        assert_eq!(
            terminal_message_to_crossterm(&resize),
            Some(CtEvent::Resize(100, 30))
        );

        let key_msg = TerminalMessage::Input(InputEvent {
            region_id: RegionId::ROOT,
            event_type: InputType::KeyDown {
                key: Key::Char('q'),
                modifiers: Modifiers::CTRL,
            },
            x: 0.0,
            y: 0.0,
            modifiers: Modifiers::CTRL,
        });
        assert_eq!(
            terminal_message_to_crossterm(&key_msg),
            Some(CtEvent::Key(KeyEvent::new(
                KeyCode::Char('q'),
                KeyModifiers::CONTROL,
            )))
        );
    }
}
