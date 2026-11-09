//! Text appended to live regions, held for the accessibility bridge
//! (ADR 0021 §3) until the app's next pass takes it. Kept only while a
//! screen reader listens ([`NativeSession::set_live_announcements`]), and
//! bounded: a pass takes it right after the poll that read it, so the bound
//! only matters for a flood read in one poll, of which the oldest text
//! goes first.

use std::collections::VecDeque;

use ultty_protocol::{Live, RegionId};

use super::NativeSession;

/// The most bytes held between two passes.
const MAX_PENDING_BYTES: usize = 64 * 1024;

/// Text appended to one live region.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveAppend {
    pub region: RegionId,
    /// The region's politeness when it was appended.
    pub live: Live,
    pub text: String,
}

#[derive(Debug, Default)]
pub(super) struct LiveAppends {
    appends: VecDeque<LiveAppend>,
    bytes: usize,
}

impl LiveAppends {
    fn push(&mut self, region: RegionId, live: Live, text: &str) {
        self.bytes += text.len();
        match self.appends.back_mut() {
            Some(last) if last.region == region && last.live == live => last.text.push_str(text),
            _ => self.appends.push_back(LiveAppend {
                region,
                live,
                text: text.to_owned(),
            }),
        }
        while self.bytes > MAX_PENDING_BYTES {
            let Some(first) = self.appends.front_mut() else {
                break;
            };
            let excess = self.bytes - MAX_PENDING_BYTES;
            if first.text.len() <= excess {
                self.bytes -= first.text.len();
                self.appends.pop_front();
            } else {
                let mut cut = excess;
                while !first.text.is_char_boundary(cut) {
                    cut += 1;
                }
                first.text.drain(..cut);
                self.bytes -= cut;
            }
        }
    }
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

    /// Text appended to live regions since the last call, oldest first.
    pub fn take_live_appends(&mut self) -> Vec<LiveAppend> {
        match &mut self.live_appends {
            Some(held) => {
                held.bytes = 0;
                std::mem::take(&mut held.appends).into()
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

    /// Drop what is held for regions that were deleted, so an id created
    /// again does not announce the old region's text.
    pub(super) fn forget_live_appends(&mut self, removed: &[RegionId]) {
        if let Some(held) = &mut self.live_appends {
            held.appends
                .retain(|append| !removed.contains(&append.region));
            held.bytes = held.appends.iter().map(|append| append.text.len()).sum();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_keeps_the_newest_text_within_the_bound() {
        let mut held = LiveAppends::default();
        let region = RegionId::new(1);
        for n in 0..20_000 {
            held.push(region, Live::Polite, &format!("é{n}\n"));
            // Another region between, so entries do not all merge.
            if n % 1000 == 0 {
                held.push(RegionId::new(2), Live::Polite, "x");
            }
        }
        assert!(held.bytes <= MAX_PENDING_BYTES);
        assert_eq!(
            held.bytes,
            held.appends.iter().map(|a| a.text.len()).sum::<usize>()
        );
        assert!(held
            .appends
            .back()
            .is_some_and(|last| last.text.ends_with("é19999\n")));
    }
}
