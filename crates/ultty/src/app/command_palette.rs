//! The command palette's keyboard: while it is open it takes every key, so
//! nothing reaches the pane's program, and runs the chosen action through
//! [`App::perform_action`], as its keybinding does.

use ultty::command_palette::{PaletteKey, PaletteOutcome};
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::App;
use ultty::action::Action;

impl App {
    /// Open the palette over everything else: the other overlays and the
    /// main menu close. Pane focus does not change.
    pub(super) fn open_command_palette(&mut self) {
        self.shortcuts_visible = false;
        self.protocol_status_visible = false;
        if self.tab_bar.menu_open {
            self.close_menu();
        }
        self.command_palette.open(&self.config.keybindings);
        self.needs_redraw = true;
    }

    pub(super) fn close_command_palette(&mut self) {
        self.command_palette.close();
        self.needs_redraw = true;
    }

    /// A key pressed while the palette is open. Its own keybinding closes
    /// it again.
    pub(super) fn handle_palette_key(&mut self, event: &KeyEvent) {
        let toggles = self
            .bound_actions(&event.logical_key)
            .contains(&Action::CommandPalette);
        if toggles {
            self.close_command_palette();
            return;
        }
        let key = palette_key(&event.logical_key, event.text.as_deref(), self.modifiers);
        self.palette_input(key);
    }

    /// Give the palette `key`, and run the action it chose.
    pub(super) fn palette_input(&mut self, key: PaletteKey<'_>) {
        let outcome = self.command_palette.handle(key);
        self.needs_redraw = true;
        if let PaletteOutcome::Run(action) = outcome {
            self.perform_action(action);
        }
    }
}

/// Most list rows one wheel event moves, so a fast flick cannot spin the
/// selection around the list many times.
const MAX_WHEEL_STEPS: usize = 8;

/// What `lines` of wheel scroll (positive: wheel rolled away from the user,
/// toward the top of the list) does in the palette: a key and how many
/// times to press it. The wheel moves the selection, which scrolls the list.
pub(super) fn wheel_key(lines: i32) -> Option<(PaletteKey<'static>, usize)> {
    let steps = (lines.unsigned_abs() as usize).min(MAX_WHEEL_STEPS);
    match lines {
        0 => None,
        1.. => Some((PaletteKey::Up, steps)),
        _ => Some((PaletteKey::Down, steps)),
    }
}

/// The cell the input method should sit at: the palette's query caret while
/// it is open, otherwise the terminal cursor.
pub(super) fn ime_cell(
    palette_caret: Option<[f32; 4]>,
    cursor: Option<super::render::CellRect>,
) -> Option<super::render::CellRect> {
    palette_caret
        .map(|[x, y, width, height]| super::render::CellRect {
            x,
            y,
            width,
            height,
        })
        .or(cursor)
}

/// What a key pressed with `mods` (and typing `text`) does in the palette.
/// Ctrl+N and Ctrl+P move like Down and Up; other keys with Ctrl, Alt or
/// Super type nothing.
pub(super) fn palette_key<'a>(
    key: &Key,
    text: Option<&'a str>,
    mods: ModifiersState,
) -> PaletteKey<'a> {
    let chord = mods.control_key() || mods.alt_key() || mods.super_key();
    match key {
        Key::Named(NamedKey::ArrowUp) => PaletteKey::Up,
        Key::Named(NamedKey::ArrowDown) => PaletteKey::Down,
        Key::Named(NamedKey::Enter) => PaletteKey::Enter,
        Key::Named(NamedKey::Escape) => PaletteKey::Escape,
        Key::Named(NamedKey::Backspace) => PaletteKey::Backspace,
        Key::Character(c) if mods.control_key() && !mods.alt_key() && !mods.super_key() => {
            match c.to_lowercase().as_str() {
                "p" => PaletteKey::Up,
                "n" => PaletteKey::Down,
                _ => PaletteKey::Other,
            }
        }
        _ if chord => PaletteKey::Other,
        _ => match text {
            Some(text) if !text.is_empty() => PaletteKey::Text(text),
            _ => PaletteKey::Other,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::input::bound_actions;
    use ultty::command_palette::CommandPalette;
    use ultty::config::KeybindingConfig;
    use winit::keyboard::SmolStr;

    fn ch(c: &str) -> Key {
        Key::Character(SmolStr::new(c))
    }

    const NONE: ModifiersState = ModifiersState::empty();
    const CTRL: ModifiersState = ModifiersState::CONTROL;

    #[test]
    fn the_wheel_moves_the_selection_and_never_scrolls_past_a_flick() {
        assert_eq!(wheel_key(0), None);
        assert_eq!(wheel_key(2), Some((PaletteKey::Up, 2)));
        assert_eq!(wheel_key(-3), Some((PaletteKey::Down, 3)));
        assert_eq!(wheel_key(-500), Some((PaletteKey::Down, MAX_WHEEL_STEPS)));
        assert_eq!(
            wheel_key(i32::MIN),
            Some((PaletteKey::Down, MAX_WHEEL_STEPS))
        );
    }

    #[test]
    fn the_input_method_follows_the_palette_caret_while_it_is_open() {
        use crate::app::render::CellRect;
        let cursor = Some(CellRect {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
        });
        let caret = ime_cell(Some([10.0, 20.0, 9.0, 18.0]), cursor).unwrap();
        assert_eq!(
            (caret.x, caret.y, caret.width, caret.height),
            (10.0, 20.0, 9.0, 18.0)
        );
        let back = ime_cell(None, cursor).unwrap();
        assert_eq!((back.x, back.y), (1.0, 2.0));
        assert!(ime_cell(None, None).is_none());
    }

    #[test]
    fn arrows_and_ctrl_n_p_move() {
        assert_eq!(
            palette_key(&Key::Named(NamedKey::ArrowUp), None, NONE),
            PaletteKey::Up
        );
        assert_eq!(
            palette_key(&Key::Named(NamedKey::ArrowDown), None, NONE),
            PaletteKey::Down
        );
        assert_eq!(palette_key(&ch("p"), Some("\u{10}"), CTRL), PaletteKey::Up);
        assert_eq!(palette_key(&ch("n"), Some("\u{e}"), CTRL), PaletteKey::Down);
    }

    #[test]
    fn enter_escape_and_backspace() {
        assert_eq!(
            palette_key(&Key::Named(NamedKey::Enter), Some("\r"), NONE),
            PaletteKey::Enter
        );
        assert_eq!(
            palette_key(&Key::Named(NamedKey::Escape), Some("\u{1b}"), NONE),
            PaletteKey::Escape
        );
        assert_eq!(
            palette_key(&Key::Named(NamedKey::Backspace), Some("\u{8}"), NONE),
            PaletteKey::Backspace
        );
    }

    #[test]
    fn printable_keys_type_and_chords_do_not() {
        assert_eq!(
            palette_key(&ch("a"), Some("a"), NONE),
            PaletteKey::Text("a")
        );
        assert_eq!(
            palette_key(&ch("A"), Some("A"), ModifiersState::SHIFT),
            PaletteKey::Text("A")
        );
        assert_eq!(
            palette_key(&Key::Named(NamedKey::Space), Some(" "), NONE),
            PaletteKey::Text(" ")
        );
        assert_eq!(
            palette_key(&ch("c"), Some("\u{3}"), CTRL),
            PaletteKey::Other
        );
        assert_eq!(
            palette_key(&ch("x"), Some("x"), ModifiersState::ALT),
            PaletteKey::Other
        );
        assert_eq!(
            palette_key(&Key::Named(NamedKey::F5), None, NONE),
            PaletteKey::Other
        );
    }

    /// Every key, typed into an open palette, is taken by the palette:
    /// none is left over to send to the pane's program.
    #[test]
    fn the_open_palette_swallows_every_key() {
        let mut palette = CommandPalette::default();
        palette.open(&KeybindingConfig::default());
        let keys = [
            (ch("l"), Some("l"), NONE),
            (ch("c"), Some("\u{3}"), CTRL),
            (ch("d"), Some("\u{4}"), CTRL),
            (Key::Named(NamedKey::Tab), Some("\t"), NONE),
            (Key::Named(NamedKey::F1), None, NONE),
            (Key::Named(NamedKey::ArrowDown), None, NONE),
            (Key::Named(NamedKey::Backspace), None, NONE),
        ];
        for (key, text, mods) in &keys {
            let outcome = palette.handle(palette_key(key, *text, *mods));
            assert_eq!(outcome, PaletteOutcome::Consumed, "{key:?}");
            assert!(palette.is_open());
        }
        // Ctrl+C typed no text.
        assert_eq!(palette.query(), "");
    }

    /// The parts of a config keybinding string as the key event winit
    /// would deliver for it.
    fn key_for(binding: &str) -> (Key, ModifiersState) {
        let parsed = KeybindingConfig::parse_keybinding(binding).unwrap();
        let mut mods = ModifiersState::empty();
        mods.set(ModifiersState::CONTROL, parsed.ctrl);
        mods.set(ModifiersState::SHIFT, parsed.shift);
        mods.set(ModifiersState::ALT, parsed.alt);
        mods.set(ModifiersState::SUPER, parsed.super_key);
        let key = match parsed.key.as_str() {
            "space" => Key::Named(NamedKey::Space),
            "tab" => Key::Named(NamedKey::Tab),
            "pageup" => Key::Named(NamedKey::PageUp),
            "pagedown" => Key::Named(NamedKey::PageDown),
            "up" => Key::Named(NamedKey::ArrowUp),
            "down" => Key::Named(NamedKey::ArrowDown),
            "left" => Key::Named(NamedKey::ArrowLeft),
            "right" => Key::Named(NamedKey::ArrowRight),
            other => ch(other),
        };
        (key, mods)
    }

    /// Choosing an action in the palette hands `perform_action` the same
    /// action its default key does.
    #[test]
    fn running_from_the_palette_dispatches_what_the_key_does() {
        let keys = KeybindingConfig::default();
        let mut palette = CommandPalette::default();
        for action in Action::ALL.into_iter().filter(|a| a.in_palette()) {
            let (key, mods) = key_for(action.binding(&keys));
            assert_eq!(
                bound_actions(&keys, &key, mods).first(),
                Some(&action),
                "{action:?}'s key"
            );

            palette.open(&keys);
            while palette.selected_action() != Some(action) {
                palette.handle(PaletteKey::Down);
            }
            assert_eq!(
                palette.handle(PaletteKey::Enter),
                PaletteOutcome::Run(action)
            );
        }
    }

    #[test]
    fn ctrl_shift_p_opens_the_palette() {
        let keys = KeybindingConfig::default();
        assert_eq!(
            bound_actions(&keys, &ch("P"), CTRL | ModifiersState::SHIFT),
            vec![Action::CommandPalette]
        );
        assert!(bound_actions(&keys, &ch("p"), CTRL).is_empty());
    }
}
