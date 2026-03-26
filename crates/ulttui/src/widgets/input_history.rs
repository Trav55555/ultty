//! Input history for TextInput.

/// Manages a history of text entries with up/down navigation.
#[derive(Debug, Default, Clone)]
pub(crate) struct InputHistory {
    entries: Vec<String>,
    index: Option<usize>,
    temp_buffer: String,
}

impl InputHistory {
    /// Add an entry to the history.
    pub fn push(&mut self, entry: impl Into<String>) {
        self.entries.push(entry.into());
        self.index = None;
    }

    /// Navigate up (older entries). Returns the value to set, or None if at top/empty.
    /// On first up, saves `current_value` as temp buffer.
    pub fn navigate_up(&mut self, current_value: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        let next_index = match self.index {
            Some(0) => return None,
            Some(i) => i - 1,
            None => {
                self.temp_buffer = current_value.to_string();
                self.entries.len() - 1
            }
        };
        self.index = Some(next_index);
        Some(&self.entries[next_index])
    }

    /// Navigate down (newer entries). Returns the value to set, or None if at bottom.
    pub fn navigate_down(&mut self) -> Option<&str> {
        let index = self.index?;
        if index + 1 < self.entries.len() {
            let next_index = index + 1;
            self.index = Some(next_index);
            Some(&self.entries[next_index])
        } else {
            self.index = None;
            let val = &self.temp_buffer;
            Some(val)
        }
    }

    /// Reset navigation state (e.g., on new input).
    pub fn reset(&mut self) {
        self.index = None;
        self.temp_buffer.clear();
    }

    /// Access entries directly (for assertions in tests).
    #[allow(dead_code)]
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_adds_entry() {
        let mut h = InputHistory::default();
        h.push("hello");
        assert_eq!(h.entries(), &["hello"]);
    }

    #[test]
    fn navigate_up_returns_last() {
        let mut h = InputHistory::default();
        h.push("first");
        h.push("second");
        let val = h.navigate_up("current").unwrap();
        assert_eq!(val, "second");
    }

    #[test]
    fn navigate_up_then_down_restores() {
        let mut h = InputHistory::default();
        h.push("first");
        h.push("second");
        h.navigate_up("current");
        let val = h.navigate_down().unwrap();
        assert_eq!(val, "current"); // restores temp buffer
    }

    #[test]
    fn navigate_up_at_top_returns_none() {
        let mut h = InputHistory::default();
        h.push("only");
        h.navigate_up("cur");
        assert!(h.navigate_up("cur").is_none());
    }

    #[test]
    fn navigate_down_without_up_returns_none() {
        let mut h = InputHistory::default();
        h.push("entry");
        assert!(h.navigate_down().is_none());
    }

    #[test]
    fn empty_history_up_returns_none() {
        let mut h = InputHistory::default();
        assert!(h.navigate_up("cur").is_none());
    }

    #[test]
    fn reset_clears_navigation() {
        let mut h = InputHistory::default();
        h.push("entry");
        h.navigate_up("cur");
        h.reset();
        assert!(h.navigate_down().is_none());
    }
}
