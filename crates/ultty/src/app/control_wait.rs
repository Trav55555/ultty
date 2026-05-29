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

    pub(super) fn find_in(&self, text: &str) -> Option<usize> {
        match self {
            WaitMatcher::Plain { pattern_lower } => {
                let text_lower = text.to_lowercase();
                text_lower.find(pattern_lower)
            }
            WaitMatcher::Regex(re) => re.find(text).map(|m| m.start()),
        }
    }
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
    fn invalid_regex_reports_error() {
        let Err(error) = WaitMatcher::new("[", true) else {
            panic!("invalid regex should be rejected");
        };

        assert!(error.contains("Invalid regex"));
    }
}
