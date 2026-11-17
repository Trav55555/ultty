//! The actions a user can bind to a key: one name, label and keybinding
//! field each.
//!
//! [`Action`] is the single source of the labels that the keyboard
//! shortcuts overlay and the command palette show, and of the
//! `[keybindings]` names the config file uses. A new variant must be added
//! to [`Action::ALL`] and to every `match` here; the tests check that the
//! config's keybinding names and [`Action::ALL`] agree.

use crate::config::KeybindingConfig;

/// Something a keybinding, the command palette or the shortcuts overlay
/// refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Copy,
    Paste,
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    /// `split_horizontal`: the new pane opens to the right.
    SplitRight,
    /// `split_vertical`: the new pane opens below.
    SplitDown,
    FocusPaneLeft,
    FocusPaneRight,
    FocusPaneUp,
    FocusPaneDown,
    ClosePane,
    CopyMode,
    Find,
    ReloadConfig,
    ShowShortcuts,
    ProtocolStatus,
    CommandPalette,
    CycleTheme,
    FontIncrease,
    FontDecrease,
    FontReset,
    Screenshot,
    DebugDump,
    ScrollPageUp,
    ScrollPageDown,
    JumpPrevPrompt,
    JumpNextPrompt,
    SelectLastOutput,
}

impl Action {
    /// Every action, in the order the shortcuts overlay and the command
    /// palette list them. A key bound to two actions runs the first.
    pub const ALL: [Action; 30] = [
        Action::NewTab,
        Action::CloseTab,
        Action::NextTab,
        Action::PrevTab,
        Action::SplitRight,
        Action::SplitDown,
        Action::FocusPaneRight,
        Action::FocusPaneDown,
        Action::FocusPaneLeft,
        Action::FocusPaneUp,
        Action::ClosePane,
        Action::Find,
        Action::CopyMode,
        Action::Copy,
        Action::Paste,
        Action::ScrollPageUp,
        Action::ScrollPageDown,
        Action::JumpPrevPrompt,
        Action::JumpNextPrompt,
        Action::SelectLastOutput,
        Action::FontIncrease,
        Action::FontDecrease,
        Action::FontReset,
        Action::CycleTheme,
        Action::ShowShortcuts,
        Action::CommandPalette,
        Action::ProtocolStatus,
        Action::ReloadConfig,
        Action::Screenshot,
        Action::DebugDump,
    ];

    /// What the action does, as the shortcuts overlay and the command
    /// palette show it.
    pub fn label(self) -> &'static str {
        match self {
            Action::Copy => "Copy selection",
            Action::Paste => "Paste from clipboard",
            Action::NewTab => "New tab",
            Action::CloseTab => "Close tab",
            Action::NextTab => "Next tab",
            Action::PrevTab => "Previous tab",
            Action::SplitRight => "Split right",
            Action::SplitDown => "Split down",
            Action::FocusPaneLeft => "Focus pane left",
            Action::FocusPaneRight => "Focus pane right",
            Action::FocusPaneUp => "Focus pane up",
            Action::FocusPaneDown => "Focus pane down",
            Action::ClosePane => "Close pane",
            Action::CopyMode => "Toggle copy mode",
            Action::Find => "Search scrollback",
            Action::ReloadConfig => "Reload config",
            Action::ShowShortcuts => "Show keyboard shortcuts",
            Action::ProtocolStatus => "Show protocol status",
            Action::CommandPalette => "Command palette",
            Action::CycleTheme => "Cycle color theme",
            Action::FontIncrease => "Increase font size",
            Action::FontDecrease => "Decrease font size",
            Action::FontReset => "Reset font size",
            Action::Screenshot => "Take screenshot",
            Action::DebugDump => "Debug dump",
            Action::ScrollPageUp => "Scroll back a page",
            Action::ScrollPageDown => "Scroll forward a page",
            Action::JumpPrevPrompt => "Jump to previous prompt",
            Action::JumpNextPrompt => "Jump to next prompt",
            Action::SelectLastOutput => "Select last command output",
        }
    }

    /// The action's name in `[keybindings]`.
    pub fn config_key(self) -> &'static str {
        match self {
            Action::Copy => "copy",
            Action::Paste => "paste",
            Action::NewTab => "new_tab",
            Action::CloseTab => "close_tab",
            Action::NextTab => "next_tab",
            Action::PrevTab => "prev_tab",
            Action::SplitRight => "split_horizontal",
            Action::SplitDown => "split_vertical",
            Action::FocusPaneLeft => "focus_pane_left",
            Action::FocusPaneRight => "focus_pane_right",
            Action::FocusPaneUp => "focus_pane_up",
            Action::FocusPaneDown => "focus_pane_down",
            Action::ClosePane => "close_pane",
            Action::CopyMode => "vim_mode",
            Action::Find => "find",
            Action::ReloadConfig => "reload_config",
            Action::ShowShortcuts => "show_shortcuts",
            Action::ProtocolStatus => "protocol_status",
            Action::CommandPalette => "command_palette",
            Action::CycleTheme => "cycle_theme",
            Action::FontIncrease => "font_increase",
            Action::FontDecrease => "font_decrease",
            Action::FontReset => "font_reset",
            Action::Screenshot => "screenshot",
            Action::DebugDump => "debug_dump",
            Action::ScrollPageUp => "scroll_page_up",
            Action::ScrollPageDown => "scroll_page_down",
            Action::JumpPrevPrompt => "jump_prev_prompt",
            Action::JumpNextPrompt => "jump_next_prompt",
            Action::SelectLastOutput => "select_last_output",
        }
    }

    /// The key `keys` binds to the action, as written in the config
    /// (`"ctrl+shift+t"`); empty when it is unbound.
    pub fn binding(self, keys: &KeybindingConfig) -> &str {
        match self {
            Action::Copy => &keys.copy,
            Action::Paste => &keys.paste,
            Action::NewTab => &keys.new_tab,
            Action::CloseTab => &keys.close_tab,
            Action::NextTab => &keys.next_tab,
            Action::PrevTab => &keys.prev_tab,
            Action::SplitRight => &keys.split_horizontal,
            Action::SplitDown => &keys.split_vertical,
            Action::FocusPaneLeft => &keys.focus_pane_left,
            Action::FocusPaneRight => &keys.focus_pane_right,
            Action::FocusPaneUp => &keys.focus_pane_up,
            Action::FocusPaneDown => &keys.focus_pane_down,
            Action::ClosePane => &keys.close_pane,
            Action::CopyMode => &keys.vim_mode,
            Action::Find => &keys.find,
            Action::ReloadConfig => &keys.reload_config,
            Action::ShowShortcuts => &keys.show_shortcuts,
            Action::ProtocolStatus => &keys.protocol_status,
            Action::CommandPalette => &keys.command_palette,
            Action::CycleTheme => &keys.cycle_theme,
            Action::FontIncrease => &keys.font_increase,
            Action::FontDecrease => &keys.font_decrease,
            Action::FontReset => &keys.font_reset,
            Action::Screenshot => &keys.screenshot,
            Action::DebugDump => &keys.debug_dump,
            Action::ScrollPageUp => &keys.scroll_page_up,
            Action::ScrollPageDown => &keys.scroll_page_down,
            Action::JumpPrevPrompt => &keys.jump_prev_prompt,
            Action::JumpNextPrompt => &keys.jump_next_prompt,
            Action::SelectLastOutput => &keys.select_last_output,
        }
    }

    /// Whether the command palette offers the action. Not the palette
    /// itself: it is already open.
    pub fn in_palette(self) -> bool {
        self != Action::CommandPalette
    }

    /// Whether the key runs the action only when no modal overlay or copy
    /// mode has taken the keyboard: copy and paste act on the pane's
    /// selection and program, which those modes own.
    pub fn waits_for_modes(self) -> bool {
        matches!(self, Action::Copy | Action::Paste)
    }
}

/// A config keybinding as people write it: `"ctrl+shift+pageup"` reads
/// `"Ctrl+Shift+PageUp"`. Empty stays empty.
pub fn display_binding(binding: &str) -> String {
    binding
        .trim()
        .split('+')
        .filter(|part| !part.is_empty())
        .map(|part| match part.to_lowercase().as_str() {
            "ctrl" | "control" => "Ctrl".to_string(),
            "pageup" => "PageUp".to_string(),
            "pagedown" => "PageDown".to_string(),
            "super" | "cmd" | "meta" | "win" => "Super".to_string(),
            lower => {
                let mut chars = lower.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_lists_every_action_once() {
        let unique: HashSet<Action> = Action::ALL.iter().copied().collect();
        assert_eq!(unique.len(), Action::ALL.len(), "an action is listed twice");
    }

    /// A config that binds every action to its own key, by the action's
    /// config name.
    fn config_binding_each_action_uniquely() -> KeybindingConfig {
        let toml: String = Action::ALL
            .iter()
            .enumerate()
            .map(|(i, action)| format!("{} = \"ctrl+alt+f{i}\"\n", action.config_key()))
            .collect();
        toml::from_str(&toml).expect("keybindings parse")
    }

    /// Each action's config name is a real keybinding field (it does not
    /// fall into `custom`), and `binding` reads that field.
    #[test]
    fn every_action_names_and_reads_its_own_config_field() {
        let keys = config_binding_each_action_uniquely();
        assert!(keys.custom.is_empty(), "unknown names: {:?}", keys.custom);
        for (i, action) in Action::ALL.iter().enumerate() {
            assert_eq!(
                action.binding(&keys),
                format!("ctrl+alt+f{i}"),
                "{action:?}"
            );
        }
    }

    /// Adding a keybinding field without an action (and so without a
    /// label) stops this compiling: list it here and give it an action.
    #[test]
    fn every_config_keybinding_field_belongs_to_an_action() {
        let keys = config_binding_each_action_uniquely();
        let KeybindingConfig {
            copy,
            paste,
            new_tab,
            close_tab,
            next_tab,
            prev_tab,
            split_horizontal,
            split_vertical,
            focus_pane_left,
            focus_pane_right,
            focus_pane_up,
            focus_pane_down,
            close_pane,
            vim_mode,
            find,
            reload_config,
            show_shortcuts,
            protocol_status,
            command_palette,
            cycle_theme,
            font_increase,
            font_decrease,
            font_reset,
            screenshot,
            debug_dump,
            scroll_page_up,
            scroll_page_down,
            jump_prev_prompt,
            jump_next_prompt,
            select_last_output,
            custom: _,
        } = keys.clone();
        let fields: HashSet<String> = [
            copy,
            paste,
            new_tab,
            close_tab,
            next_tab,
            prev_tab,
            split_horizontal,
            split_vertical,
            focus_pane_left,
            focus_pane_right,
            focus_pane_up,
            focus_pane_down,
            close_pane,
            vim_mode,
            find,
            reload_config,
            show_shortcuts,
            protocol_status,
            command_palette,
            cycle_theme,
            font_increase,
            font_decrease,
            font_reset,
            screenshot,
            debug_dump,
            scroll_page_up,
            scroll_page_down,
            jump_prev_prompt,
            jump_next_prompt,
            select_last_output,
        ]
        .into_iter()
        .collect();
        let bound: HashSet<String> = Action::ALL
            .iter()
            .map(|action| action.binding(&keys).to_string())
            .collect();
        assert_eq!(fields, bound);
    }

    #[test]
    fn every_action_has_a_distinct_nonempty_label() {
        let labels: HashSet<&str> = Action::ALL.iter().map(|a| a.label()).collect();
        assert_eq!(labels.len(), Action::ALL.len(), "two actions share a label");
        assert!(labels.iter().all(|label| !label.trim().is_empty()));
    }

    #[test]
    fn every_default_binding_parses() {
        let keys = KeybindingConfig::default();
        for action in Action::ALL {
            assert!(
                KeybindingConfig::parse_keybinding(action.binding(&keys)).is_some(),
                "{action:?} has an unparsable default"
            );
        }
    }

    #[test]
    fn bindings_display_with_capitalized_names() {
        assert_eq!(display_binding("ctrl+shift+p"), "Ctrl+Shift+P");
        assert_eq!(display_binding("ctrl+shift+pageup"), "Ctrl+Shift+PageUp");
        assert_eq!(display_binding("ctrl+="), "Ctrl+=");
        assert_eq!(display_binding("cmd+space"), "Super+Space");
        assert_eq!(display_binding(""), "");
    }

    #[test]
    fn the_palette_offers_everything_but_itself() {
        let offered: Vec<Action> = Action::ALL
            .iter()
            .copied()
            .filter(|a| a.in_palette())
            .collect();
        assert_eq!(offered.len(), Action::ALL.len() - 1);
        assert!(!offered.contains(&Action::CommandPalette));
    }
}
