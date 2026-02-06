//! Focus management for interactive widgets.
//!
//! Focus state is thread-local and owned by the UI/render thread.
//! Do not access `with_current`/`with_current_mut` from background tasks.

/// Tracks which widget is currently focused.
#[derive(Debug, Default)]
pub struct FocusManager {
    /// Ordered list of focusable widget IDs (collected during each render).
    focusable_ids: Vec<String>,
    /// Currently focused widget ID.
    focused_id: Option<String>,
}

impl FocusManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the focusable list (called before each render pass).
    pub fn begin_frame(&mut self) {
        self.focusable_ids.clear();
    }

    /// Register a focusable widget ID (called during render for each focusable widget).
    pub fn register(&mut self, id: String) {
        self.focusable_ids.push(id);
    }

    /// Get the currently focused widget ID.
    pub fn focused_id(&self) -> Option<&str> {
        self.focused_id.as_deref()
    }

    /// Check if a specific widget ID is focused.
    pub fn is_focused(&self, id: &str) -> bool {
        self.focused_id.as_deref() == Some(id)
    }

    /// Move focus to the next widget.
    pub fn focus_next(&mut self) {
        if self.focusable_ids.is_empty() {
            return;
        }
        match &self.focused_id {
            None => {
                self.focused_id = self.focusable_ids.first().cloned();
            }
            Some(current) => {
                let idx = self.focusable_ids.iter().position(|id| id == current);
                let next_idx = match idx {
                    Some(i) => (i + 1) % self.focusable_ids.len(),
                    None => 0,
                };
                self.focused_id = self.focusable_ids.get(next_idx).cloned();
            }
        }
    }

    /// Move focus to the previous widget.
    pub fn focus_prev(&mut self) {
        if self.focusable_ids.is_empty() {
            return;
        }
        match &self.focused_id {
            None => {
                self.focused_id = self.focusable_ids.last().cloned();
            }
            Some(current) => {
                let idx = self.focusable_ids.iter().position(|id| id == current);
                let prev_idx = match idx {
                    Some(0) => self.focusable_ids.len() - 1,
                    Some(i) => i - 1,
                    None => self.focusable_ids.len() - 1,
                };
                self.focused_id = self.focusable_ids.get(prev_idx).cloned();
            }
        }
    }

    /// Focus a specific widget by ID.
    pub fn focus(&mut self, id: impl Into<String>) {
        self.focused_id = Some(id.into());
    }

    /// Clear focus.
    pub fn blur(&mut self) {
        self.focused_id = None;
    }

    /// After a render frame, validate that focused_id still exists in the registered list.
    /// If it doesn't (widget removed), move to next valid or clear.
    pub fn end_frame(&mut self) {
        if let Some(ref id) = self.focused_id {
            if !self.focusable_ids.contains(id) {
                // Focused widget no longer exists, try to keep focus nearby
                self.focused_id = self.focusable_ids.first().cloned();
            }
        }
    }
}

use std::cell::RefCell;

thread_local! {
    static CURRENT_FOCUS: RefCell<FocusManager> = RefCell::new(FocusManager::new());
}

impl FocusManager {
    /// Access the current thread-local FocusManager (read-only).
    pub fn with_current<R>(f: impl FnOnce(&FocusManager) -> R) -> R {
        CURRENT_FOCUS.with(|fm| f(&fm.borrow()))
    }

    /// Access the current thread-local FocusManager (mutable).
    pub fn with_current_mut<R>(f: impl FnOnce(&mut FocusManager) -> R) -> R {
        CURRENT_FOCUS.with(|fm| f(&mut fm.borrow_mut()))
    }

    /// Replace the thread-local FocusManager. Returns the old one.
    pub fn set_current(new: FocusManager) -> FocusManager {
        CURRENT_FOCUS.with(|fm| {
            let mut borrowed = fm.borrow_mut();
            std::mem::replace(&mut *borrowed, new)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::FocusManager;

    #[test]
    fn focus_next_cycles_through_widgets() {
        let mut focus = FocusManager::new();
        focus.register("a".to_string());
        focus.register("b".to_string());
        focus.register("c".to_string());

        focus.focus_next();
        assert_eq!(focus.focused_id(), Some("a"));
        focus.focus_next();
        assert_eq!(focus.focused_id(), Some("b"));
        focus.focus_next();
        assert_eq!(focus.focused_id(), Some("c"));
        focus.focus_next();
        assert_eq!(focus.focused_id(), Some("a"));
    }

    #[test]
    fn focus_prev_cycles_backward() {
        let mut focus = FocusManager::new();
        focus.register("a".to_string());
        focus.register("b".to_string());
        focus.register("c".to_string());

        focus.focus_prev();
        assert_eq!(focus.focused_id(), Some("c"));
        focus.focus_prev();
        assert_eq!(focus.focused_id(), Some("b"));
        focus.focus_prev();
        assert_eq!(focus.focused_id(), Some("a"));
        focus.focus_prev();
        assert_eq!(focus.focused_id(), Some("c"));
    }

    #[test]
    fn focus_next_on_empty_is_noop() {
        let mut focus = FocusManager::new();
        focus.focus_next();
        assert_eq!(focus.focused_id(), None);
    }

    #[test]
    fn focus_prev_on_empty_is_noop() {
        let mut focus = FocusManager::new();
        focus.focus_prev();
        assert_eq!(focus.focused_id(), None);
    }

    #[test]
    fn focus_starts_at_none() {
        let focus = FocusManager::new();
        assert_eq!(focus.focused_id(), None);
    }

    #[test]
    fn focus_next_from_none_selects_first() {
        let mut focus = FocusManager::new();
        focus.register("first".to_string());
        focus.register("second".to_string());

        focus.focus_next();
        assert_eq!(focus.focused_id(), Some("first"));
    }

    #[test]
    fn focus_prev_from_none_selects_last() {
        let mut focus = FocusManager::new();
        focus.register("first".to_string());
        focus.register("second".to_string());

        focus.focus_prev();
        assert_eq!(focus.focused_id(), Some("second"));
    }

    #[test]
    fn is_focused_returns_correct_value() {
        let mut focus = FocusManager::new();
        focus.register("a".to_string());
        focus.register("b".to_string());

        focus.focus("b");
        assert!(focus.is_focused("b"));
        assert!(!focus.is_focused("a"));
    }

    #[test]
    fn begin_frame_clears_ids() {
        let mut focus = FocusManager::new();
        focus.register("a".to_string());
        focus.begin_frame();
        assert_eq!(focus.focusable_ids.len(), 0);
    }

    #[test]
    fn end_frame_validates_focused_id() {
        let mut focus = FocusManager::new();
        focus.register("a".to_string());
        focus.register("b".to_string());
        focus.focus("missing");
        focus.end_frame();

        assert_eq!(focus.focused_id(), Some("a"));
    }

    #[test]
    fn focus_specific_id() {
        let mut focus = FocusManager::new();
        focus.register("a".to_string());
        focus.focus("a");
        assert_eq!(focus.focused_id(), Some("a"));
    }

    #[test]
    fn blur_clears_focus() {
        let mut focus = FocusManager::new();
        focus.register("a".to_string());
        focus.focus("a");
        focus.blur();
        assert_eq!(focus.focused_id(), None);
    }

    #[test]
    fn thread_local_with_current_reads() {
        FocusManager::with_current_mut(|fm| {
            fm.register("a".to_string());
            fm.register("b".to_string());
            fm.focus("a");
        });
        let focused = FocusManager::with_current(|fm| fm.is_focused("a"));
        assert!(focused);
        // Clean up thread-local state
        FocusManager::set_current(FocusManager::new());
    }

    #[test]
    fn thread_local_set_and_restore() {
        let mut custom = FocusManager::new();
        custom.register("x".to_string());
        custom.focus("x");

        let old = FocusManager::set_current(custom);
        let is_x_focused = FocusManager::with_current(|fm| fm.is_focused("x"));
        assert!(is_x_focused);

        // Restore
        FocusManager::set_current(old);
    }
}
