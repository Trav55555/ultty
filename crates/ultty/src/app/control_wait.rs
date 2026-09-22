//! Deferred control-plane wait matching.

use std::time::{Duration, Instant};

use regex::Regex;

pub(super) struct PendingControlWait {
    client_key: usize,
    matcher: WaitMatcher,
    start: Instant,
    timeout: Duration,
}

impl PendingControlWait {
    pub(super) fn new(
        client_key: usize,
        matcher: WaitMatcher,
        timeout: Duration,
    ) -> PendingControlWait {
        PendingControlWait {
            client_key,
            matcher,
            start: Instant::now(),
            timeout,
        }
    }

    pub(super) fn client_key(&self) -> usize {
        self.client_key
    }

    pub(super) fn matcher(&self) -> &WaitMatcher {
        &self.matcher
    }

    pub(super) fn is_expired(&self) -> bool {
        self.start.elapsed() >= self.timeout
    }
}

pub(super) enum WaitMatcher {
    Plain { pattern_lower: String },
    Regex(Regex),
}

impl WaitMatcher {
    pub(super) fn new(pattern: &str, use_regex: bool) -> Result<Self, String> {
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
    pub(super) fn find_in(&self, text: &str) -> Option<usize> {
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
pub(super) struct IgnoreCaseMatch {
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
pub(super) fn find_ignore_case<'a>(
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
