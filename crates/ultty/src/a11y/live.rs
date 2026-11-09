//! Live regions (ADR 0021 §3): text appended to a region whose `live` is
//! not `Off` waits in a queue and goes to assistive technology as
//! announcements, each a new `Label` child of the region. Both platform
//! adapters announce a live node when it is added, so a new announcement
//! gets a new node id and the ones already sent keep theirs.
//!
//! A region's queue is flushed when a line break arrives or
//! [`FLUSH_DELAY`] after its first unflushed byte, whichever is first. A
//! flush with a line break in it takes the complete lines only; the line
//! begun after them waits for its own break, or the delay from its own
//! first byte, so lines held back by the cap below do not drag a fragment
//! of the next one with them. A session flushes at most once per
//! [`FLUSH_SPACING`] (four a second), oldest queue first; text that
//! arrives meanwhile joins the next flush. A flush is at most
//! [`MAX_FLUSH_BYTES`], cut at the last line break inside that (else the
//! last blank, else a character boundary), and the rest waits. A region
//! keeps its last [`MAX_ANNOUNCEMENTS`]; the region's own text nodes carry
//! the whole transcript for review.
//!
//! Memory is bounded twice: a region queues at most [`MAX_QUEUED_BYTES`],
//! and a session's regions hold at most [`MAX_SESSION_BYTES`], queued and
//! announced together, the oldest going first.
//!
//! Nothing here runs unless a screen reader is listening, and the clock is
//! the caller's, so tests script it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use ultty_protocol::Live;

/// How long text without a line break waits before it is flushed anyway.
pub const FLUSH_DELAY: Duration = Duration::from_millis(250);

/// The shortest time between two flushes of one session: at most four a
/// second.
pub const FLUSH_SPACING: Duration = Duration::from_millis(250);

/// The most bytes one announcement carries.
pub const MAX_FLUSH_BYTES: usize = 2048;

/// Announcements kept under a region; older ones leave the tree.
pub const MAX_ANNOUNCEMENTS: usize = 8;

/// The most unannounced bytes a region queues. Text arriving faster than
/// four flushes a second can carry is dropped oldest first, from a line
/// start where one is near: nobody can listen that fast, and the region's
/// text keeps all of it.
pub const MAX_QUEUED_BYTES: usize = 16 * 1024;

/// What a queue over [`MAX_QUEUED_BYTES`] is cut to, so that a flood
/// trims once per few KiB appended rather than on every append.
const TRIMMED_QUEUE_BYTES: usize = MAX_QUEUED_BYTES / 4 * 3;

/// How far past a queue's cut a trim looks for a line break to cut after.
const LINE_SEARCH_BYTES: usize = 1024;

/// The most bytes one session's live regions hold, queued and announced.
pub const MAX_SESSION_BYTES: usize = 64 * 1024;

/// What a session over [`MAX_SESSION_BYTES`] is cut to.
const TRIMMED_SESSION_BYTES: usize = MAX_SESSION_BYTES / 4 * 3;

/// One flush of a region's queue: the node the platform announces.
#[derive(Debug, Clone, PartialEq)]
pub struct Announcement {
    /// Counts every flush since assistive technology arrived and names
    /// the node, so a kept announcement keeps its node and a new one never
    /// takes an id that was in the tree, not even after its region was
    /// deleted and created again.
    pub serial: u64,
    /// The text, without leading or trailing blanks.
    pub text: String,
    /// The region's politeness when the text was appended.
    pub live: Live,
}

/// The queues and announcements of the live regions on screen, keyed by
/// (session, region).
#[derive(Debug, Default)]
pub struct LiveRegions {
    regions: BTreeMap<(u64, u32), LiveRegion>,
    /// Each session's last flush, while it still holds the next one back.
    last_flush: HashMap<u64, Instant>,
    /// The bytes each session's regions hold ([`LiveRegion::size`]).
    bytes: HashMap<u64, usize>,
    /// The next announcement's serial.
    next_serial: u64,
}

#[derive(Debug, Default)]
struct LiveRegion {
    live: Live,
    /// Text appended and not yet announced.
    queue: String,
    /// Where the queue's last line break is.
    last_break: Option<usize>,
    /// When the queue's first byte arrived; `None` while it is empty.
    since: Option<Instant>,
    /// When the first byte after the queue's last line break arrived;
    /// `None` when the queue is empty or ends with a line break.
    line_since: Option<Instant>,
    announced: VecDeque<Announcement>,
    /// Bytes of text in `announced`.
    announced_bytes: usize,
}

impl LiveRegions {
    /// Whether no region has text queued or announcements kept.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Forget every queue and announcement: assistive technology came or
    /// went, and nodes it has not seen must not be announced as new.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// `text` was appended at `now` to `region` of `session`, whose `live`
    /// is as given. Ignored for a region that is not live.
    pub fn push(&mut self, session: u64, region: u32, live: Live, text: &str, now: Instant) {
        if live == Live::Off || text.is_empty() {
            return;
        }
        let entry = self.regions.entry((session, region)).or_default();
        let before = entry.size();
        entry.live = live;
        entry.append(text, now);
        account(&mut self.bytes, session, before, entry.size());
        self.keep_session_within_budget(session);
    }

    /// `region` of `session` was deleted: drop its queue and announcements,
    /// so a region created again with its id starts with none.
    pub fn forget(&mut self, session: u64, region: u32) {
        if let Some(gone) = self.regions.remove(&(session, region)) {
            account(&mut self.bytes, session, gone.size(), 0);
        }
    }

    /// Flush what is due at `now`: per session at most one region, the one
    /// waiting longest, and none within [`FLUSH_SPACING`] of its last
    /// flush. Returns whether a new announcement was made.
    pub fn flush(&mut self, now: Instant) -> bool {
        self.last_flush
            .retain(|_, at| now.saturating_duration_since(*at) < FLUSH_SPACING);
        // Each session's region whose queue has waited longest, if due.
        let mut due: HashMap<u64, ((u64, u32), Instant)> = HashMap::new();
        for (&key, region) in &self.regions {
            let Some(ready) = region.ready_at().filter(|&ready| ready <= now) else {
                continue;
            };
            if self.last_flush.contains_key(&key.0) {
                continue;
            }
            let best = due.entry(key.0).or_insert((key, ready));
            if ready < best.1 {
                *best = (key, ready);
            }
        }
        let mut announced = false;
        for (session, (key, _)) in due {
            let Some(region) = self.regions.get_mut(&key) else {
                continue;
            };
            let before = region.size();
            let flushed = region.flush(now, self.next_serial);
            account(&mut self.bytes, session, before, region.size());
            if flushed {
                self.next_serial += 1;
                self.last_flush.insert(session, now);
                announced = true;
            }
        }
        announced
    }

    /// When [`LiveRegions::flush`] next has something to do, if any queue
    /// holds text.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.regions
            .iter()
            .filter_map(|(&(session, _), region)| {
                let ready = region.ready_at()?;
                Some(match self.last_flush.get(&session) {
                    Some(&last) => ready.max(last + FLUSH_SPACING),
                    None => ready,
                })
            })
            .min()
    }

    /// The announcements kept for `region` of `session`, oldest first.
    pub fn announcements(&self, session: u64, region: u32) -> impl Iterator<Item = &Announcement> {
        self.regions
            .get(&(session, region))
            .into_iter()
            .flat_map(|region| region.announced.iter())
    }

    /// Keep only the regions `shown` says are in the tree. A region that
    /// leaves the tree loses its queue and announcements, so that coming
    /// back does not add, and announce, old nodes again.
    pub fn retain(&mut self, mut shown: impl FnMut(u64, u32) -> bool) {
        let bytes = &mut self.bytes;
        self.regions.retain(|&(session, region), live| {
            let keep = shown(session, region);
            if !keep {
                account(bytes, session, live.size(), 0);
            }
            keep
        });
    }

    /// Bring `session` back under [`MAX_SESSION_BYTES`] once it passes it,
    /// to [`TRIMMED_SESSION_BYTES`]: the oldest announcements leave first,
    /// then the queues that have waited longest.
    fn keep_session_within_budget(&mut self, session: u64) {
        if self.bytes.get(&session).copied().unwrap_or(0) <= MAX_SESSION_BYTES {
            return;
        }
        while self.bytes.get(&session).copied().unwrap_or(0) > TRIMMED_SESSION_BYTES {
            let range = (session, 0)..=(session, u32::MAX);
            let oldest_announced = self
                .regions
                .range(range.clone())
                .filter_map(|(&key, region)| Some((region.announced.front()?.serial, key)))
                .min();
            let victim = match oldest_announced {
                Some((_, key)) => key,
                None => match self
                    .regions
                    .range(range)
                    .filter_map(|(&key, region)| Some((region.since?, key)))
                    .min()
                {
                    Some((_, key)) => key,
                    None => break,
                },
            };
            let Some(region) = self.regions.get_mut(&victim) else {
                break;
            };
            let before = region.size();
            if let Some(dropped) = region.announced.pop_front() {
                region.announced_bytes -= dropped.text.len();
            } else {
                region.drop_front(region.queue.len());
            }
            account(&mut self.bytes, session, before, region.size());
        }
    }
}

/// Move `session`'s count in `bytes` from a region's `before` to `after`.
fn account(bytes: &mut HashMap<u64, usize>, session: u64, before: usize, after: usize) {
    let total = bytes.entry(session).or_default();
    *total = (*total + after).saturating_sub(before);
    if *total == 0 {
        bytes.remove(&session);
    }
}

impl LiveRegion {
    /// Bytes the region holds, queued and announced.
    fn size(&self) -> usize {
        self.queue.len() + self.announced_bytes
    }

    /// Queue `text`, arrived at `now`, keeping the queue within
    /// [`MAX_QUEUED_BYTES`].
    fn append(&mut self, text: &str, now: Instant) {
        if self.queue.is_empty() {
            self.since = Some(now);
        }
        match text.rfind('\n') {
            Some(last) => {
                self.last_break = Some(self.queue.len() + last);
                self.line_since = (last + 1 < text.len()).then_some(now);
            }
            None => {
                self.line_since.get_or_insert(now);
            }
        }
        self.queue.push_str(text);
        if self.queue.len() > MAX_QUEUED_BYTES {
            // Keep the newest, from a line start if one is near the cut.
            let from = ceil_char_boundary(&self.queue, self.queue.len() - TRIMMED_QUEUE_BYTES);
            let window =
                &self.queue[from..ceil_char_boundary(&self.queue, from + LINE_SEARCH_BYTES)];
            let cut = window.find('\n').map_or(from, |at| from + at + 1);
            self.drop_front(cut);
        }
    }

    /// Drop the queue's first `len` bytes, which end on a character
    /// boundary.
    fn drop_front(&mut self, len: usize) {
        self.queue.drain(..len);
        self.last_break = self.last_break.and_then(|at| at.checked_sub(len));
        if self.queue.is_empty() {
            self.since = None;
            self.line_since = None;
        }
    }

    /// When the queue is due: at once with a complete line in it, else
    /// [`FLUSH_DELAY`] after its first byte. `None` when it is empty.
    fn ready_at(&self) -> Option<Instant> {
        let since = self.since?;
        Some(if self.last_break.is_some() {
            since
        } else {
            since + FLUSH_DELAY
        })
    }

    /// Move the due part of the queue into a new announcement numbered
    /// `serial`. Returns false, with that part dropped, when it is only
    /// blanks.
    fn flush(&mut self, now: Instant, serial: u64) -> bool {
        let Some(since) = self.since else {
            return false;
        };
        // Complete lines go out alone: a line begun after them waits for
        // its own break or delay, so it is not cut mid-word just because
        // the lines before it were held back.
        let mut end = match self.last_break {
            Some(last) => last + 1,
            None if now.saturating_duration_since(since) >= FLUSH_DELAY => self.queue.len(),
            None => return false,
        };
        if end > MAX_FLUSH_BYTES {
            end = cut_point(&self.queue, MAX_FLUSH_BYTES);
        }
        let text = self.queue[..end].trim().to_owned();
        self.drop_front(end);
        if !self.queue.is_empty() && self.last_break.is_none() {
            // What is left is the start of a line; it waits from when that
            // line began.
            self.since = Some(self.line_since.unwrap_or(now));
        }
        if text.is_empty() {
            return false;
        }
        self.announced_bytes += text.len();
        self.announced.push_back(Announcement {
            serial,
            text,
            live: self.live,
        });
        while self.announced.len() > MAX_ANNOUNCEMENTS {
            if let Some(dropped) = self.announced.pop_front() {
                self.announced_bytes -= dropped.text.len();
            }
        }
        true
    }
}

/// Where to cut `text` so the first part is at most `limit` bytes: after
/// the last line break in it, else after the last blank, else at the last
/// character boundary. Never 0 for a non-empty text, so a flush always
/// makes progress.
fn cut_point(text: &str, limit: usize) -> usize {
    let head = &text[..floor_char_boundary(text, limit)];
    if let Some(at) = head.rfind('\n') {
        return at + 1;
    }
    match head.rfind(char::is_whitespace) {
        Some(at) if at > 0 => at + head[at..].chars().next().map_or(1, char::len_utf8),
        _ if !head.is_empty() => head.len(),
        // A first character longer than the limit; there is none today.
        _ => ceil_char_boundary(text, 1),
    }
}

/// The largest character boundary of `text` at or before `at`.
fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The smallest character boundary of `text` at or after `at`.
fn ceil_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The texts announced for region 1 of session 7.
    fn texts(live: &LiveRegions) -> Vec<String> {
        live.announcements(7, 1).map(|a| a.text.clone()).collect()
    }

    #[test]
    fn a_line_break_flushes_the_complete_lines_at_once() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Polite, "Compiling ultty\nFinis", t0);
        assert_eq!(live.next_deadline(), Some(t0), "due at once");
        assert!(live.flush(t0));
        assert_eq!(texts(&live), ["Compiling ultty"]);
        // The rest of the line waits for its own break or the delay.
        assert_eq!(live.next_deadline(), Some(t0 + FLUSH_SPACING));
        live.push(7, 1, Live::Polite, "hed\n", t0 + ms(100));
        assert!(!live.flush(t0 + ms(100)), "within the spacing");
        assert!(live.flush(t0 + ms(250)));
        assert_eq!(texts(&live), ["Compiling ultty", "Finished"]);
        assert_eq!(live.next_deadline(), None);
    }

    #[test]
    fn text_without_a_line_break_is_flushed_after_the_delay() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Polite, "Thinking", t0);
        live.push(7, 1, Live::Polite, " about it", t0 + ms(200));
        assert_eq!(live.next_deadline(), Some(t0 + FLUSH_DELAY));
        assert!(!live.flush(t0 + ms(249)));
        assert!(texts(&live).is_empty());
        assert!(live.flush(t0 + FLUSH_DELAY));
        assert_eq!(texts(&live), ["Thinking about it"]);
    }

    #[test]
    fn complete_lines_held_back_by_the_cap_leave_a_young_partial_line_behind() {
        // As streamed over AT-SPI: line 2 waited out the session's spacing,
        // and line 3 had begun when the slot came.
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Polite, "Line 1\n", t0);
        assert!(live.flush(t0));
        live.push(7, 1, Live::Polite, "Line 2", t0 + ms(20));
        live.push(7, 1, Live::Polite, "\nLine 3 of the re", t0 + ms(200));
        assert!(live.flush(t0 + ms(300)), "line 2 is overdue");
        assert_eq!(texts(&live), ["Line 1", "Line 2"], "not cut mid-word");
        // Line 3 waits for its break, or 250 ms from its first byte.
        assert_eq!(live.next_deadline(), Some(t0 + ms(550)));
        live.push(7, 1, Live::Polite, "ply\n", t0 + ms(400));
        assert!(live.flush(t0 + ms(550)));
        assert_eq!(texts(&live)[2], "Line 3 of the reply");
    }

    #[test]
    fn the_delay_counts_from_when_the_unflushed_line_began() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Polite, "one\ntw", t0 + ms(100));
        assert!(live.flush(t0 + ms(100)));
        // "tw" arrived at 100 ms: due at 350, not 250 after the flush.
        assert_eq!(live.next_deadline(), Some(t0 + ms(350)));
        live.push(7, 1, Live::Polite, "o", t0 + ms(300));
        assert!(live.flush(t0 + ms(350)));
        assert_eq!(texts(&live), ["one", "two"]);
    }

    #[test]
    fn a_session_flushes_at_most_four_times_a_second_and_text_accumulates() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        let mut flushes = Vec::new();
        // A line every 50 ms for two seconds, into two regions.
        for step in 0..40u64 {
            let now = t0 + ms(step * 50);
            let region = 1 + (step % 2) as u32;
            live.push(7, region, Live::Polite, &format!("line {step}\n"), now);
            if live.flush(now) {
                flushes.push(now);
            }
        }
        for pair in flushes.windows(2) {
            assert!(pair[1] - pair[0] >= FLUSH_SPACING, "{flushes:?}");
        }
        assert_eq!(flushes.len(), 8, "{flushes:?}");
        // Nothing was lost: the lines between flushes joined the next one.
        let announced: Vec<String> = [1, 2]
            .into_iter()
            .flat_map(|region| live.announcements(7, region))
            .map(|a| a.text.clone())
            .collect();
        let lines = announced.iter().flat_map(|text| text.lines()).count();
        let pending = live
            .regions
            .values()
            .map(|r| r.queue.lines().count())
            .sum::<usize>();
        assert_eq!(lines + pending, 40);
        // Another session is not held back.
        live.push(8, 1, Live::Polite, "other\n", t0 + ms(1950));
        assert!(live.flush(t0 + ms(1950)));
    }

    #[test]
    fn a_flush_over_two_kib_is_cut_at_its_last_line_break() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        let line = "x".repeat(99) + "\n";
        let text = line.repeat(30); // 3000 bytes
        live.push(7, 1, Live::Polite, &text, t0);
        assert!(live.flush(t0));
        let first = &texts(&live)[0];
        assert_eq!(first.lines().count(), 20, "20 lines of 100 bytes fit");
        assert!(first.len() <= MAX_FLUSH_BYTES);
        // The rest waits, overdue, for the next slot.
        assert_eq!(live.next_deadline(), Some(t0 + FLUSH_SPACING));
        assert!(live.flush(t0 + FLUSH_SPACING));
        assert_eq!(texts(&live)[1].lines().count(), 10);
    }

    #[test]
    fn a_long_line_with_no_break_is_cut_at_a_blank_or_a_character() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        let words = "word ".repeat(600); // 3000 bytes, no line break
        live.push(7, 1, Live::Polite, &words, t0);
        assert!(live.flush(t0 + FLUSH_DELAY));
        let first = &texts(&live)[0];
        assert!(first.len() <= MAX_FLUSH_BYTES);
        assert!(first.ends_with("word"), "cut after a blank, not mid-word");

        // No blank at all: at a character boundary, never inside one.
        let mut live = LiveRegions::default();
        let wide = "é".repeat(1500); // 3000 bytes, 2 each
        live.push(7, 1, Live::Polite, &wide, t0);
        assert!(live.flush(t0 + FLUSH_DELAY));
        assert_eq!(texts(&live)[0].len(), MAX_FLUSH_BYTES);
        assert!(live.flush(t0 + FLUSH_DELAY + FLUSH_SPACING));
        assert_eq!(texts(&live).concat(), wide);
    }

    #[test]
    fn a_region_keeps_its_last_eight_announcements_with_their_serials() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        for n in 0..12u64 {
            let now = t0 + FLUSH_SPACING * n as u32;
            live.push(7, 1, Live::Polite, &format!("{n}\n"), now);
            assert!(live.flush(now));
        }
        let kept: Vec<(u64, String)> = live
            .announcements(7, 1)
            .map(|a| (a.serial, a.text.clone()))
            .collect();
        assert_eq!(kept.len(), MAX_ANNOUNCEMENTS);
        assert_eq!(kept[0], (4, "4".to_owned()));
        assert_eq!(kept[7], (11, "11".to_owned()));
    }

    #[test]
    fn blanks_alone_are_not_announced() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Polite, "\n\n  \n", t0);
        assert!(!live.flush(t0));
        assert!(texts(&live).is_empty());
        assert_eq!(live.next_deadline(), None);
        // And they do not use up the session's slot.
        live.push(7, 1, Live::Polite, "ok\n", t0);
        assert!(live.flush(t0));
    }

    #[test]
    fn regions_that_are_not_live_queue_nothing() {
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Off, "quiet\n", Instant::now());
        assert!(live.is_empty());
        assert_eq!(live.next_deadline(), None);
    }

    #[test]
    fn assertive_is_passed_through() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Assertive, "error: boom\n", t0);
        assert!(live.flush(t0));
        assert_eq!(
            live.announcements(7, 1).map(|a| a.live).collect::<Vec<_>>(),
            [Live::Assertive]
        );
    }

    #[test]
    fn a_flood_is_bounded_and_resumes_at_a_line_start() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        for n in 0..10_000 {
            live.push(7, 1, Live::Polite, &format!("line {n}\n"), t0);
        }
        let queue = &live.regions[&(7, 1)].queue;
        assert!(queue.len() <= MAX_QUEUED_BYTES);
        assert!(queue.starts_with("line "), "{}", &queue[..20]);
        assert!(queue.ends_with("line 9999\n"));
    }

    #[test]
    fn retain_drops_regions_left_out_of_the_tree() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Polite, "a\n", t0);
        live.push(7, 2, Live::Polite, "b\n", t0);
        live.retain(|_, region| region == 2);
        assert_eq!(live.announcements(7, 1).count(), 0);
        assert!(live.flush(t0));
        assert_eq!(live.announcements(7, 2).count(), 1);
    }

    #[test]
    fn a_flood_of_multi_byte_text_is_trimmed_on_character_boundaries() {
        // 2-, 3- and 4-byte characters, after an odd number of ASCII bytes,
        // so a cut by byte count would land inside one.
        for wide in ["é", "中", "🦀"] {
            for lead in ["a", "ab", "abc"] {
                let t0 = Instant::now();
                let mut live = LiveRegions::default();
                live.push(7, 1, Live::Polite, lead, t0);
                live.push(7, 1, Live::Polite, &wide.repeat(8192), t0);
                live.push(7, 1, Live::Polite, "a", t0);
                live.push(
                    7,
                    1,
                    Live::Polite,
                    &format!("{lead}\n{}", wide.repeat(5000)),
                    t0,
                );
                let queue = &live.regions[&(7, 1)].queue;
                assert!(queue.len() <= MAX_QUEUED_BYTES, "{wide} after {lead}");
                // And it still flushes on boundaries.
                let mut at = t0 + FLUSH_DELAY;
                for _ in 0..40 {
                    live.flush(at);
                    at += FLUSH_SPACING;
                }
            }
        }
    }

    #[test]
    fn a_session_holds_at_most_its_budget_across_regions_oldest_first() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        // Many regions, each announcing and queueing.
        let line = "x".repeat(1000) + "\n";
        let mut now = t0;
        for region in 0..200u32 {
            live.push(7, region, Live::Polite, &line, now);
            live.flush(now);
            live.push(7, region, Live::Polite, &line.repeat(3), now);
            now += FLUSH_SPACING;
            let held: usize = live.regions.values().map(LiveRegion::size).sum();
            assert!(
                held <= MAX_SESSION_BYTES,
                "{held} bytes after region {region}"
            );
            assert_eq!(live.bytes.get(&7).copied().unwrap_or(0), held);
        }
        // The newest region kept its text; the oldest lost theirs.
        assert!(live.regions[&(7, 199)].size() > 0);
        assert_eq!(live.regions[&(7, 0)].size(), 0);
        // Other sessions are not charged for it.
        live.push(8, 1, Live::Polite, "ok\n", now);
        assert!(live.flush(now));
        assert_eq!(live.announcements(8, 1).count(), 1);
    }

    #[test]
    fn a_forgotten_region_starts_again_with_nothing() {
        let t0 = Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Polite, "old\n", t0);
        assert!(live.flush(t0));
        live.push(7, 1, Live::Polite, "old, unflushed", t0);
        live.forget(7, 1);
        assert_eq!(live.announcements(7, 1).count(), 0);
        assert_eq!(live.next_deadline(), None);
        assert!(live.bytes.is_empty());
        live.push(7, 1, Live::Polite, "new\n", t0 + FLUSH_SPACING);
        assert!(live.flush(t0 + FLUSH_SPACING));
        let new: Vec<(u64, String)> = live
            .announcements(7, 1)
            .map(|a| (a.serial, a.text.clone()))
            .collect();
        assert_eq!(new, [(1, "new".to_owned())], "a serial never reused");
    }
}
