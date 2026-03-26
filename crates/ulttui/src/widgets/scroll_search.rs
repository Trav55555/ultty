//! Search state for ScrollView.

/// Tracks search query, match positions, and current match index.
#[derive(Debug, Default)]
pub(crate) struct ScrollSearch {
    pub query: Option<String>,
    pub matches: Vec<usize>,
    pub current: usize,
}

impl ScrollSearch {
    /// Start a new search, clearing previous matches.
    pub fn search(&mut self, query: &str) {
        self.query = Some(query.to_string());
        self.matches.clear();
        self.current = 0;
    }

    /// Add a match at the given line index.
    pub fn add_match(&mut self, line: usize) {
        self.matches.push(line);
    }

    /// Move to the next match (wrapping). Returns the line index if any.
    pub fn goto_next(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        self.current = (self.current + 1) % self.matches.len();
        Some(self.matches[self.current])
    }

    /// Move to the previous match (wrapping). Returns the line index if any.
    pub fn goto_prev(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        if self.current == 0 {
            self.current = self.matches.len() - 1;
        } else {
            self.current -= 1;
        }
        Some(self.matches[self.current])
    }

    /// Clear all search state.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.query = None;
        self.matches.clear();
        self.current = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_clears_previous() {
        let mut ss = ScrollSearch::default();
        ss.add_match(5);
        ss.add_match(10);
        ss.search("new");
        assert!(ss.matches.is_empty());
        assert_eq!(ss.current, 0);
        assert_eq!(ss.query.as_deref(), Some("new"));
    }

    #[test]
    fn goto_next_wraps() {
        let mut ss = ScrollSearch::default();
        ss.add_match(3);
        ss.add_match(7);
        assert_eq!(ss.goto_next(), Some(7)); // 0→1
        assert_eq!(ss.goto_next(), Some(3)); // 1→0 (wrap)
    }

    #[test]
    fn goto_prev_wraps() {
        let mut ss = ScrollSearch::default();
        ss.add_match(3);
        ss.add_match(7);
        assert_eq!(ss.goto_prev(), Some(7)); // 0→1 (wrap back)
        assert_eq!(ss.goto_prev(), Some(3)); // 1→0
    }

    #[test]
    fn empty_returns_none() {
        let mut ss = ScrollSearch::default();
        assert_eq!(ss.goto_next(), None);
        assert_eq!(ss.goto_prev(), None);
    }

    #[test]
    fn clear_resets_all() {
        let mut ss = ScrollSearch::default();
        ss.search("test");
        ss.add_match(5);
        ss.current = 1;
        ss.clear();
        assert!(ss.query.is_none());
        assert!(ss.matches.is_empty());
        assert_eq!(ss.current, 0);
    }
}
