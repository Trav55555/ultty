//! Text appended to live regions, held for the accessibility bridge
//! (ADR 0021 §3) until the app's next pass takes it, with the deletion of
//! those regions in order between. Kept only while a screen reader listens
//! ([`NativeSession::set_live_announcements`]), and bounded: a pass takes
//! it right after the poll that read it, so the bound only matters for a
//! flood read in one poll, of which the oldest text goes first.

use std::collections::{HashSet, VecDeque};

use ultty_protocol::{Live, RegionId};

use super::NativeSession;

/// The text held between two passes is cut back to this once it reaches
/// twice as much, so a flood is trimmed once per 64 KiB, not per append.
const MAX_PENDING_BYTES: usize = 64 * 1024;

/// How far past a cut a trim looks for a line break to cut after.
const LINE_SEARCH_BYTES: usize = 1024;

/// What happened to a live region, for the accessibility bridge.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveChange {
    /// Text was appended to it while it was live.
    Appended {
        region: RegionId,
        /// The region's politeness when it was appended.
        live: Live,
        text: String,
    },
    /// It was deleted: what is queued or announced for it must go, or a
    /// region created again with its id would speak the old one's text.
    Removed(RegionId),
}

#[derive(Debug, Default)]
pub(super) struct LiveAppends {
    changes: VecDeque<LiveChange>,
    /// Bytes of appended text in `changes`.
    bytes: usize,
    /// Regions with appended text since announcing began, whose deletion
    /// the bridge must hear of.
    appended_to: HashSet<RegionId>,
}

impl LiveAppends {
    fn push(&mut self, region: RegionId, live: Live, text: &str) {
        self.appended_to.insert(region);
        self.bytes += text.len();
        match self.changes.back_mut() {
            Some(LiveChange::Appended {
                region: last,
                live: last_live,
                text: last_text,
            }) if *last == region && *last_live == live => last_text.push_str(text),
            _ => self.changes.push_back(LiveChange::Appended {
                region,
                live,
                text: text.to_owned(),
            }),
        }
        if self.bytes >= 2 * MAX_PENDING_BYTES {
            self.trim();
        }
    }

    /// Drop the oldest text down to [`MAX_PENDING_BYTES`], from a line
    /// start where one is near the cut. Deletions are kept.
    fn trim(&mut self) {
        let mut excess = self.bytes.saturating_sub(MAX_PENDING_BYTES);
        let mut kept = VecDeque::with_capacity(self.changes.len());
        for mut change in self.changes.drain(..) {
            if let LiveChange::Appended { text, .. } = &mut change {
                if excess >= text.len() {
                    excess -= text.len();
                    self.bytes -= text.len();
                    continue;
                }
                if excess > 0 {
                    let from = ceil_char_boundary(text, excess);
                    let window = &text[from..ceil_char_boundary(text, from + LINE_SEARCH_BYTES)];
                    let cut = window.find('\n').map_or(from, |at| from + at + 1);
                    text.drain(..cut);
                    self.bytes -= cut;
                    excess = 0;
                }
            }
            kept.push_back(change);
        }
        self.changes = kept;
    }
}

/// The smallest character boundary of `text` at or after `at`.
fn ceil_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

impl NativeSession {
    /// Whether to keep text appended to live regions for
    /// [`NativeSession::take_live_appends`]: on while a screen reader
    /// listens. Off drops what is held.
    pub fn set_live_announcements(&mut self, on: bool) {
        match (on, self.live_appends.is_some()) {
            (true, false) => self.live_appends = Some(LiveAppends::default()),
            (false, true) => self.live_appends = None,
            _ => {}
        }
    }

    /// What happened to live regions since the last call, oldest first.
    pub fn take_live_appends(&mut self) -> Vec<LiveChange> {
        match &mut self.live_appends {
            Some(held) => {
                held.bytes = 0;
                std::mem::take(&mut held.changes).into()
            }
            None => Vec::new(),
        }
    }

    /// Hold `text`, just appended to `region`, if the region is live and
    /// a screen reader listens.
    pub(super) fn note_live_append(&mut self, region: RegionId, text: &str) {
        let Some(held) = &mut self.live_appends else {
            return;
        };
        let live = self
            .scene
            .get(region)
            .map_or(Live::Off, |region| region.semantics().live);
        if live != Live::Off {
            held.push(region, live, text);
        }
    }

    /// Note the deletion of those `removed` regions that had live text.
    pub(super) fn forget_live_appends(&mut self, removed: &[RegionId]) {
        let Some(held) = &mut self.live_appends else {
            return;
        };
        if held.appended_to.is_empty() {
            return;
        }
        for id in removed {
            if held.appended_to.remove(id) {
                held.changes.push_back(LiveChange::Removed(*id));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_bytes(held: &LiveAppends) -> usize {
        held.changes
            .iter()
            .map(|change| match change {
                LiveChange::Appended { text, .. } => text.len(),
                LiveChange::Removed(_) => 0,
            })
            .sum()
    }

    #[test]
    fn a_flood_keeps_the_newest_text_within_the_bound_from_a_line_start() {
        let mut held = LiveAppends::default();
        for wide in ["é", "中", "🦀"] {
            for n in 0..20_000 {
                held.push(RegionId::new(1), Live::Polite, &format!("a{wide}{n}\n"));
                // Another region now and then, so entries do not all merge.
                if n % 1000 == 0 {
                    held.push(RegionId::new(2), Live::Polite, "x");
                }
            }
            assert!(held.bytes < 2 * MAX_PENDING_BYTES);
            assert_eq!(held.bytes, text_bytes(&held));
            let Some(LiveChange::Appended { text, .. }) = held.changes.back() else {
                panic!("no text held");
            };
            assert!(text.ends_with(&format!("a{wide}19999\n")));
            let Some(LiveChange::Appended { text, .. }) = held.changes.front() else {
                panic!("no text held");
            };
            assert!(text.starts_with('a') || text == "x", "{:?}", &text[..8]);
        }
    }
}
