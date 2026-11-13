//! Deferred control-plane wait matching.

use std::time::{Duration, Instant};

use crate::event_loop::TerminalEvent;
use crate::terminal_manager::TerminalId;
use regex::Regex;

/// A bounded pending control request pinned to one pane and deadline.
pub struct PendingControlWait {
    client_key: usize,
    /// The pane being watched, fixed when the request arrived.
    pane: TerminalId,
    condition: Condition,
    /// When the wait gives up.
    expires: Instant,
}

/// What a pending wait is waiting for.
pub enum Condition {
    /// Text on screen or in scrollback; checked whenever the UI wakes.
    Text(WaitMatcher),
    /// The pane's next OSC 133;D.
    CommandFinished,
    /// Completion of a particular atomically submitted shell line.
    ShellCommand {
        /// Sequence returned by the atomic shell submission.
        sequence: u64,
    },
    /// A direct child job reaching an owner-observed terminal state.
    ProcessJob,
    /// The pane's next bell.
    Bell,
    /// No output from the pane for `period`; `until` moves on each output.
    Quiet {
        /// Required duration without observed output.
        period: Duration,
        /// Current quiet deadline, extended by output.
        until: Instant,
    },
}

impl Condition {
    /// Start a quiet period from the supplied observation time.
    pub fn quiet(period: Duration, now: Instant) -> Condition {
        Condition::Quiet {
            period,
            until: now + period,
        }
    }
}

impl PendingControlWait {
    /// Construct a pane-pinned wait with an explicit timeout.
    pub fn new(
        client_key: usize,
        pane: TerminalId,
        condition: Condition,
        timeout: Duration,
        now: Instant,
    ) -> PendingControlWait {
        PendingControlWait {
            client_key,
            pane,
            condition,
            expires: now + timeout,
        }
    }

    /// Control connection waiting for this result.
    pub fn client_key(&self) -> usize {
        self.client_key
    }

    /// Owner-scoped pane fixed when the wait was accepted.
    pub fn pane(&self) -> TerminalId {
        self.pane
    }

    /// Whether this wait follows the direct child lifecycle.
    pub fn is_process_job(&self) -> bool {
        matches!(self.condition, Condition::ProcessJob)
    }

    /// The exact fenced shell submission, when applicable.
    pub fn command_sequence(&self) -> Option<u64> {
        match self.condition {
            Condition::ShellCommand { sequence } => Some(sequence),
            _ => None,
        }
    }

    /// The text matcher, for a wait on text.
    pub fn matcher(&self) -> Option<&WaitMatcher> {
        match &self.condition {
            Condition::Text(matcher) => Some(matcher),
            _ => None,
        }
    }

    /// The next instant this wait needs the UI awake for: its timeout, or
    /// the end of its quiet period. Text and events wake the UI themselves.
    pub fn deadline(&self) -> Instant {
        match self.condition {
            Condition::Quiet { until, .. } => until.min(self.expires),
            _ => self.expires,
        }
    }

    /// Whether the request deadline has elapsed.
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires
    }

    /// Whether a quiet wait has seen no output for its whole period.
    pub fn is_quiet(&self, now: Instant) -> bool {
        matches!(self.condition, Condition::Quiet { until, .. } if now >= until)
    }

    /// Note an event from this wait's pane. Returns true when it is the
    /// event being waited for; output restarts a quiet period.
    pub fn observe(&mut self, event: &TerminalEvent, now: Instant) -> bool {
        match (&mut self.condition, event) {
            (
                Condition::ShellCommand { sequence: expected },
                TerminalEvent::ShellCommandFinished { sequence, .. },
            ) => expected == sequence,
            (Condition::CommandFinished, TerminalEvent::CommandFinished { .. })
            | (Condition::Bell, TerminalEvent::Bell) => true,
            (Condition::Quiet { period, until }, TerminalEvent::Redraw) => {
                *until = now + *period;
                false
            }
            _ => false,
        }
    }
}

/// Case-insensitive text matcher shared by window and host automation.
pub enum WaitMatcher {
    /// Literal matching against a pre-lowercased pattern.
    Plain {
        /// Lowercased search text.
        pattern_lower: String,
    },
    /// Compiled case-insensitive regular expression.
    Regex(Regex),
}

impl WaitMatcher {
    /// Compile a literal or regular expression matcher.
    pub fn new(pattern: &str, use_regex: bool) -> Result<Self, String> {
        if use_regex {
            let regex_pattern = format!("(?i){}", pattern);
            Regex::new(&regex_pattern)
                .map(WaitMatcher::Regex)
                .map_err(|e| format!("Invalid regex: {}", e))
        } else {
            Ok(WaitMatcher::Plain {
                pattern_lower: pattern.to_lowercase(),
            })
        }
    }

    /// Find the first match and return its character column in `text`.
    pub fn find_in(&self, text: &str) -> Option<usize> {
        match self {
            WaitMatcher::Plain { pattern_lower } => find_ignore_case(text, pattern_lower)
                .next()
                .map(|m| m.char_col),
            WaitMatcher::Regex(re) => re.find(text).map(|m| text[..m.start()].chars().count()),
        }
    }
}

/// A case-insensitive match located in the original (not lowercased) text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IgnoreCaseMatch {
    /// Zero-based character column of the match start.
    pub char_col: usize,
    /// Byte offset of the match start in the original text (char boundary).
    pub byte_start: usize,
    /// Byte offset of the match end in the original text (char boundary).
    pub byte_end: usize,
}

/// Find every non-overlapping occurrence of `pattern_lower` (already
/// lowercased) in `text`, ignoring case.
///
/// Lowercasing can change byte lengths (`İ` becomes two chars), so offsets
/// found in the lowercased copy are mapped back to char boundaries of the
/// original text instead of being used to slice it directly.
pub fn find_ignore_case<'a>(
    text: &'a str,
    pattern_lower: &'a str,
) -> impl Iterator<Item = IgnoreCaseMatch> + 'a {
    let mut lower = String::with_capacity(text.len());
    // lower byte offset -> original byte offset, one entry per lower byte
    // plus a sentinel for the end.
    let mut lower_to_orig: Vec<usize> = Vec::with_capacity(text.len() + 1);
    for (orig_byte, c) in text.char_indices() {
        let before = lower.len();
        for lc in c.to_lowercase() {
            lower.push(lc);
        }
        lower_to_orig.extend(std::iter::repeat_n(orig_byte, lower.len() - before));
    }
    lower_to_orig.push(text.len());

    let matches: Vec<(usize, usize)> = if pattern_lower.is_empty() {
        Vec::new()
    } else {
        lower
            .match_indices(pattern_lower)
            .map(|(start, matched)| (start, start + matched.len()))
            .collect()
    };

    matches.into_iter().map(move |(lower_start, lower_end)| {
        let byte_start = lower_to_orig[lower_start];
        let byte_end = lower_to_orig[lower_end];
        IgnoreCaseMatch {
            char_col: text[..byte_start].chars().count(),
            byte_start,
            byte_end,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(condition: Condition, now: Instant) -> PendingControlWait {
        PendingControlWait::new(
            1,
            TerminalId::new(0),
            condition,
            Duration::from_secs(10),
            now,
        )
    }

    #[test]
    fn event_waits_answer_only_their_event() {
        let now = Instant::now();
        let mut command = wait(Condition::CommandFinished, now);
        assert!(!command.observe(&TerminalEvent::Bell, now));
        assert!(!command.observe(&TerminalEvent::CommandStarted, now));
        assert!(command.observe(&TerminalEvent::CommandFinished { exit_code: Some(0) }, now));

        let mut bell = wait(Condition::Bell, now);
        assert!(!bell.observe(&TerminalEvent::Redraw, now));
        assert!(bell.observe(&TerminalEvent::Bell, now));
    }

    #[test]
    fn test_fenced_wait_rejects_legacy_and_earlier_completion() {
        let now = Instant::now();
        let mut fenced = wait(Condition::ShellCommand { sequence: 2 }, now);
        assert!(!fenced.observe(&TerminalEvent::CommandFinished { exit_code: Some(0) }, now));
        assert!(!fenced.observe(
            &TerminalEvent::ShellCommandFinished {
                sequence: 1,
                exit_code: Some(0)
            },
            now
        ));
        assert!(fenced.observe(
            &TerminalEvent::ShellCommandFinished {
                sequence: 2,
                exit_code: Some(7)
            },
            now
        ));
        let mut legacy = wait(Condition::CommandFinished, now);
        assert!(!legacy.observe(
            &TerminalEvent::ShellCommandFinished {
                sequence: 2,
                exit_code: Some(7)
            },
            now
        ));
        assert!(legacy.observe(&TerminalEvent::CommandFinished { exit_code: Some(7) }, now));
    }

    #[test]
    fn output_restarts_a_quiet_period() {
        let now = Instant::now();
        let period = Duration::from_millis(500);
        let mut quiet = wait(Condition::quiet(period, now), now);
        assert_eq!(quiet.deadline(), now + period);

        let later = now + Duration::from_millis(400);
        assert!(!quiet.observe(&TerminalEvent::Redraw, later));
        assert!(!quiet.is_quiet(now + period));
        assert_eq!(quiet.deadline(), later + period);
        assert!(quiet.is_quiet(later + period));
    }

    #[test]
    fn deadline_is_the_timeout_unless_a_quiet_period_ends_first() {
        let now = Instant::now();
        let matcher = WaitMatcher::new("x", false).unwrap();
        assert_eq!(
            wait(Condition::Text(matcher), now).deadline(),
            now + Duration::from_secs(10)
        );
        let long = wait(Condition::quiet(Duration::from_secs(60), now), now);
        assert_eq!(long.deadline(), now + Duration::from_secs(10));
        assert!(long.is_expired(now + Duration::from_secs(10)));
    }

    #[test]
    fn plain_match_is_case_insensitive() {
        let matcher = WaitMatcher::new("Ready", false).unwrap();

        assert_eq!(matcher.find_in("system ready"), Some(7));
    }

    #[test]
    fn regex_match_is_case_insensitive() {
        let matcher = WaitMatcher::new("r[a-z]+y", true).unwrap();

        assert_eq!(matcher.find_in("SYSTEM READY"), Some(7));
    }

    #[test]
    fn plain_match_reports_char_column_for_multibyte_rows() {
        // 'İ' lowercases to two chars, shifting byte offsets in the
        // lowercased copy; the column must still be a char index.
        let matcher = WaitMatcher::new("ready", false).unwrap();

        assert_eq!(matcher.find_in("aİé ready"), Some(4));
    }

    #[test]
    fn regex_match_reports_char_column() {
        let matcher = WaitMatcher::new("ready", true).unwrap();

        assert_eq!(matcher.find_in("日本 READY"), Some(3));
    }

    #[test]
    fn find_ignore_case_maps_back_to_original_char_boundaries() {
        let text = "aİé ready";
        let found: Vec<_> = find_ignore_case(text, "ready").collect();

        assert_eq!(found.len(), 1);
        let m = found[0];
        assert_eq!(m.char_col, 4);
        assert_eq!(&text[m.byte_start..m.byte_end], "ready");
    }

    #[test]
    fn find_ignore_case_finds_multiple_matches() {
        let found: Vec<_> = find_ignore_case("Ab ab AB", "ab")
            .map(|m| m.char_col)
            .collect();

        assert_eq!(found, vec![0, 3, 6]);
    }

    #[test]
    fn find_ignore_case_handles_empty_pattern() {
        assert_eq!(find_ignore_case("anything", "").count(), 0);
    }

    #[test]
    fn invalid_regex_reports_error() {
        let Err(error) = WaitMatcher::new("[", true) else {
            panic!("invalid regex should be rejected");
        };

        assert!(error.contains("Invalid regex"));
    }
}
