//! The command palette: a filterable list of every [`Action`] the palette
//! offers, with its key, that runs the chosen one.
//!
//! This module holds the state and the matching, with no window or GPU
//! types: the app turns key events into [`PaletteKey`]s and runs the
//! [`Action`] a [`PaletteOutcome::Run`] names, and the renderer and the
//! accessibility tree read [`CommandPalette::rows`].

use crate::action::{display_binding, Action};
use crate::config::KeybindingConfig;

/// A key, as the open palette sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteKey<'a> {
    /// Up, or Ctrl+P.
    Up,
    /// Down, or Ctrl+N.
    Down,
    Enter,
    Escape,
    Backspace,
    /// Text typed into the query.
    Text(&'a str),
    /// Anything else. The palette takes it and does nothing.
    Other,
}

/// What a key did to the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteOutcome {
    /// The palette took the key and stays open.
    Consumed,
    /// The palette closed without running anything.
    Closed,
    /// The palette closed; run this action.
    Run(Action),
}

/// One action the palette offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    pub action: Action,
    pub label: &'static str,
    /// The key bound to it, for display (`"Ctrl+Shift+T"`); empty when
    /// unbound.
    pub key: String,
}

/// An entry that matches the query, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteRow<'a> {
    pub entry: &'a PaletteEntry,
    /// Character indices into the label that the query matched.
    pub matched: &'a [usize],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Match {
    entry: usize,
    score: i32,
    positions: Vec<usize>,
}

/// The palette: whether it is open, its query, the matches and the
/// selected one, and the actions run from it this session.
#[derive(Debug, Clone, Default)]
pub struct CommandPalette {
    open: bool,
    query: String,
    entries: Vec<PaletteEntry>,
    matches: Vec<Match>,
    selected: usize,
    /// Actions run from the palette, most recent first.
    recent: Vec<Action>,
}

impl CommandPalette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open with an empty query, listing the actions with the keys `keys`
    /// binds them to.
    pub fn open(&mut self, keys: &KeybindingConfig) {
        self.entries = Action::ALL
            .iter()
            .copied()
            .filter(|action| action.in_palette())
            .map(|action| PaletteEntry {
                action,
                label: action.label(),
                key: display_binding(action.binding(keys)),
            })
            .collect();
        self.query.clear();
        self.open = true;
        self.refilter();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.matches.clear();
        self.selected = 0;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Index of the selected row in [`Self::rows`]; meaningless when there
    /// are none.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The entries that match the query, best first. With no query, the
    /// recently run actions come first, then the rest in table order.
    pub fn rows(&self) -> impl ExactSizeIterator<Item = PaletteRow<'_>> {
        self.matches.iter().map(|m| PaletteRow {
            entry: &self.entries[m.entry],
            matched: &m.positions,
        })
    }

    pub fn selected_action(&self) -> Option<Action> {
        let m = self.matches.get(self.selected)?;
        Some(self.entries[m.entry].action)
    }

    /// Apply `key`. Closed, the palette ignores keys.
    pub fn handle(&mut self, key: PaletteKey<'_>) -> PaletteOutcome {
        if !self.open {
            return PaletteOutcome::Closed;
        }
        let count = self.matches.len();
        match key {
            PaletteKey::Up if count > 0 => self.selected = (self.selected + count - 1) % count,
            PaletteKey::Down if count > 0 => self.selected = (self.selected + 1) % count,
            PaletteKey::Enter => {
                if let Some(action) = self.selected_action() {
                    self.recent.retain(|&a| a != action);
                    self.recent.insert(0, action);
                    self.close();
                    return PaletteOutcome::Run(action);
                }
            }
            PaletteKey::Escape => {
                self.close();
                return PaletteOutcome::Closed;
            }
            PaletteKey::Backspace => {
                if self.query.pop().is_some() {
                    self.refilter();
                }
            }
            PaletteKey::Text(text) => {
                let before = self.query.len();
                self.query.extend(text.chars().filter(|c| !c.is_control()));
                if self.query.len() != before {
                    self.refilter();
                }
            }
            PaletteKey::Up | PaletteKey::Down | PaletteKey::Other => {}
        }
        PaletteOutcome::Consumed
    }

    fn refilter(&mut self) {
        self.selected = 0;
        self.matches.clear();
        if self.query.trim().is_empty() {
            let recent_rank = |action: Action| {
                self.recent
                    .iter()
                    .position(|&a| a == action)
                    .unwrap_or(usize::MAX)
            };
            self.matches = (0..self.entries.len())
                .map(|entry| Match {
                    entry,
                    score: 0,
                    positions: Vec::new(),
                })
                .collect();
            // Stable: entries never run keep table order.
            self.matches
                .sort_by_key(|m| recent_rank(self.entries[m.entry].action));
            return;
        }
        for (entry, e) in self.entries.iter().enumerate() {
            if let Some((score, positions)) = fuzzy_match(&self.query, e.label) {
                self.matches.push(Match {
                    entry,
                    score,
                    positions,
                });
            }
        }
        // Stable: equal scores keep table order.
        self.matches.sort_by_key(|m| std::cmp::Reverse(m.score));
    }
}

const SCORE_MATCH: i32 = 16;
const BONUS_FIRST_CHAR: i32 = 12;
const BONUS_WORD_START: i32 = 10;
const BONUS_CONSECUTIVE: i32 = 8;
const PENALTY_GAP: i32 = 2;
/// The most a gap before the first matched character costs, so a word
/// late in a label is not ranked far below one early in another.
const MAX_LEADING_PENALTY: i32 = 3;

/// Match `query` against `text` as a case-insensitive subsequence,
/// ignoring whitespace in the query. Returns the best score and the
/// character indices into `text` that matched, or `None` when `query` is
/// not a subsequence.
///
/// The score rewards matches at the start of the text and of words, and
/// runs of consecutive characters, and charges for characters skipped
/// between matches, so `"sd"` ranks "Split down" above "Scroll forward".
pub fn fuzzy_match(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(fold_case)
        .collect();
    let text_chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = text_chars.iter().copied().map(fold_case).collect();
    let (n, m) = (query.len(), folded.len());
    if n == 0 {
        return Some((0, Vec::new()));
    }
    if n > m {
        return None;
    }
    let bonus = |j: usize| -> i32 {
        if j == 0 {
            return BONUS_FIRST_CHAR;
        }
        let prev = text_chars[j - 1];
        let here = text_chars[j];
        let word_start = !prev.is_alphanumeric() && here.is_alphanumeric()
            || prev.is_lowercase() && here.is_uppercase();
        if word_start {
            BONUS_WORD_START
        } else {
            0
        }
    };

    // score[i][j]: best score with query[i] matched at text[j];
    // from[i][j]: where query[i - 1] was matched for that score.
    let mut score = vec![vec![None::<i32>; m]; n];
    let mut from = vec![vec![0usize; m]; n];
    for j in 0..m {
        if folded[j] == query[0] {
            let leading = (j as i32 * PENALTY_GAP).min(MAX_LEADING_PENALTY);
            score[0][j] = Some(SCORE_MATCH + bonus(j) - leading);
        }
    }
    for i in 1..n {
        // Best of score[i - 1][k] + k over k < j - 1, for a gap from k to
        // j costing (j - k - 1) * PENALTY_GAP.
        let mut best_gapped: Option<(i32, usize)> = None;
        for j in i..m {
            if j >= 2 {
                let k = j - 2;
                if let Some(s) = score[i - 1][k] {
                    let value = s + k as i32 * PENALTY_GAP;
                    if best_gapped.is_none_or(|(best, _)| value > best) {
                        best_gapped = Some((value, k));
                    }
                }
            }
            if folded[j] != query[i] {
                continue;
            }
            let consecutive = score[i - 1][j - 1].map(|s| (s + BONUS_CONSECUTIVE, j - 1));
            let gapped = best_gapped.map(|(value, k)| (value - (j as i32 - 1) * PENALTY_GAP, k));
            let best = match (consecutive, gapped) {
                (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
                (a, b) => a.or(b),
            };
            if let Some((s, k)) = best {
                score[i][j] = Some(s + SCORE_MATCH + bonus(j));
                from[i][j] = k;
            }
        }
    }

    let (mut j, best) = (0..m)
        .filter_map(|j| score[n - 1][j].map(|s| (j, s)))
        .max_by_key(|&(j, s)| (s, std::cmp::Reverse(j)))?;
    let mut positions = vec![0; n];
    for i in (0..n).rev() {
        positions[i] = j;
        if i > 0 {
            j = from[i][j];
        }
    }
    Some((best, positions))
}

fn fold_case(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(palette: &CommandPalette) -> Vec<&'static str> {
        palette.rows().map(|row| row.entry.label).collect()
    }

    fn open() -> CommandPalette {
        let mut palette = CommandPalette::default();
        palette.open(&KeybindingConfig::default());
        palette
    }

    fn type_text(palette: &mut CommandPalette, text: &str) {
        assert_eq!(
            palette.handle(PaletteKey::Text(text)),
            PaletteOutcome::Consumed
        );
    }

    #[test]
    fn matches_a_case_insensitive_subsequence() {
        let (_, positions) = fuzzy_match("SPRT", "Split right").unwrap();
        assert_eq!(positions, vec![0, 1, 6, 10]);
        assert!(fuzzy_match("xyz", "Split right").is_none());
        assert!(
            fuzzy_match("tilps", "Split right").is_none(),
            "order matters"
        );
    }

    #[test]
    fn whitespace_in_the_query_is_ignored() {
        let (_, positions) = fuzzy_match("split  r", "Split right").unwrap();
        assert_eq!(positions, vec![0, 1, 2, 3, 4, 6]);
    }

    #[test]
    fn prefers_word_starts_and_runs_over_scattered_letters() {
        // "nt" could take the n and t of "New" and "tab", or of "content".
        let (_, positions) = fuzzy_match("nt", "New tab").unwrap();
        assert_eq!(positions, vec![0, 4]);
        let (run, _) = fuzzy_match("tab", "New tab").unwrap();
        let (scattered, _) = fuzzy_match("tab", "Take a break").unwrap();
        assert!(run > scattered);
    }

    #[test]
    fn ranks_the_closest_label_first() {
        let mut palette = open();
        type_text(&mut palette, "sd");
        assert_eq!(labels(&palette)[0], "Split down");

        let mut palette = open();
        type_text(&mut palette, "new tab");
        assert_eq!(labels(&palette)[0], "New tab");

        let mut palette = open();
        type_text(&mut palette, "font");
        assert_eq!(
            &labels(&palette)[..3],
            &[
                "Increase font size",
                "Decrease font size",
                "Reset font size"
            ],
            "equal scores keep table order"
        );
    }

    #[test]
    fn rows_carry_the_matched_characters_and_the_bound_key() {
        let mut palette = open();
        type_text(&mut palette, "cltab");
        let row = palette.rows().next().unwrap();
        assert_eq!(row.entry.label, "Close tab");
        assert_eq!(row.entry.key, "Ctrl+Shift+W");
        assert_eq!(row.matched, &[0, 1, 6, 7, 8]);
    }

    #[test]
    fn lists_every_palette_action_once_and_not_itself() {
        let palette = open();
        let listed: Vec<Action> = palette.rows().map(|row| row.entry.action).collect();
        let offered: Vec<Action> = Action::ALL
            .iter()
            .copied()
            .filter(|a| a.in_palette())
            .collect();
        assert_eq!(listed, offered);
        assert!(!listed.contains(&Action::CommandPalette));
    }

    #[test]
    fn arrows_move_the_selection_and_wrap() {
        let mut palette = open();
        let count = palette.rows().len();
        assert_eq!(palette.selected(), 0);
        palette.handle(PaletteKey::Down);
        palette.handle(PaletteKey::Down);
        assert_eq!(palette.selected(), 2);
        palette.handle(PaletteKey::Up);
        assert_eq!(palette.selected(), 1);
        palette.handle(PaletteKey::Up);
        palette.handle(PaletteKey::Up);
        assert_eq!(palette.selected(), count - 1, "up from the top wraps");
        palette.handle(PaletteKey::Down);
        assert_eq!(palette.selected(), 0, "down from the bottom wraps");
    }

    #[test]
    fn typing_resets_the_selection_to_the_best_match() {
        let mut palette = open();
        palette.handle(PaletteKey::Down);
        type_text(&mut palette, "c");
        assert_eq!(palette.selected(), 0);
    }

    #[test]
    fn enter_runs_the_selected_action_and_closes() {
        let mut palette = open();
        type_text(&mut palette, "split");
        palette.handle(PaletteKey::Down);
        assert_eq!(
            palette.handle(PaletteKey::Enter),
            PaletteOutcome::Run(Action::SplitDown)
        );
        assert!(!palette.is_open());
        assert_eq!(palette.query(), "");
    }

    #[test]
    fn enter_with_no_match_does_nothing() {
        let mut palette = open();
        type_text(&mut palette, "zzzz");
        assert_eq!(palette.rows().len(), 0);
        assert_eq!(palette.handle(PaletteKey::Enter), PaletteOutcome::Consumed);
        assert!(palette.is_open());
    }

    #[test]
    fn escape_closes_without_running() {
        let mut palette = open();
        type_text(&mut palette, "new");
        assert_eq!(palette.handle(PaletteKey::Escape), PaletteOutcome::Closed);
        assert!(!palette.is_open());
    }

    #[test]
    fn backspace_edits_the_query() {
        let mut palette = open();
        type_text(&mut palette, "spx");
        assert_eq!(palette.rows().len(), 0);
        palette.handle(PaletteKey::Backspace);
        assert_eq!(palette.query(), "sp");
        assert!(palette.rows().len() > 0);
        palette.handle(PaletteKey::Backspace);
        palette.handle(PaletteKey::Backspace);
        palette.handle(PaletteKey::Backspace);
        assert_eq!(palette.query(), "");
    }

    #[test]
    fn control_characters_are_not_typed() {
        let mut palette = open();
        type_text(&mut palette, "\r\u{1b}\t");
        assert_eq!(palette.query(), "");
    }

    #[test]
    fn other_keys_are_taken_and_ignored() {
        let mut palette = open();
        assert_eq!(palette.handle(PaletteKey::Other), PaletteOutcome::Consumed);
        assert!(palette.is_open());
    }

    #[test]
    fn recently_run_actions_come_first_with_an_empty_query() {
        let mut palette = open();
        let first = labels(&palette)[0];
        assert_eq!(first, "New tab");
        for query in ["reload", "screenshot"] {
            type_text(&mut palette, query);
            assert!(matches!(
                palette.handle(PaletteKey::Enter),
                PaletteOutcome::Run(_)
            ));
            palette.open(&KeybindingConfig::default());
        }
        assert_eq!(
            &labels(&palette)[..3],
            &["Take screenshot", "Reload config", "New tab"]
        );
        // A query ranks by match alone.
        type_text(&mut palette, "copy");
        assert_eq!(
            labels(&palette)[..2],
            ["Copy selection", "Toggle copy mode"]
        );
    }

    #[test]
    fn keys_reflect_the_config() {
        let keys = KeybindingConfig {
            new_tab: "super+t".to_string(),
            ..Default::default()
        };
        let mut palette = CommandPalette::default();
        palette.open(&keys);
        type_text(&mut palette, "new tab");
        assert_eq!(palette.rows().next().unwrap().entry.key, "Super+T");
    }

    #[test]
    fn a_closed_palette_takes_no_keys() {
        let mut palette = CommandPalette::default();
        assert_eq!(
            palette.handle(PaletteKey::Text("a")),
            PaletteOutcome::Closed
        );
        assert_eq!(palette.query(), "");
    }
}
