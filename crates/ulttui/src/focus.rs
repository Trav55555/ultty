//! Focus management for interactive widgets.
//!
//! Focus state is thread-local and owned by the UI/render thread.
//! Do not access `with_current`/`with_current_mut` from background tasks.

use crate::widgets::View;

/// Tracks which widget is currently focused.
#[derive(Debug, Default)]
pub struct FocusManager {
    /// Ordered list of focusable widget IDs (collected during each render).
    focusable_ids: Vec<String>,
    /// Currently focused widget ID.
    focused_id: Option<String>,
    /// Index of the last focused widget (for nearest-neighbor fallback on removal).
    last_focused_index: Option<usize>,
}

impl FocusManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the focusable list (called before each render pass).
    pub fn begin_frame(&mut self) {
        self.focusable_ids.clear();
    }

    /// Register a focusable widget ID for this frame.
    ///
    /// Returns `false` (and registers nothing) when the same ID was already
    /// registered this frame: duplicate IDs would make Tab navigation loop on
    /// one widget, so the second registration is a bug in the view.
    pub fn register(&mut self, id: String) -> bool {
        if self.focusable_ids.contains(&id) {
            tracing::warn!("focusable widget id {id:?} registered twice in one frame; ignoring");
            return false;
        }
        self.focusable_ids.push(id);
        true
    }

    /// Register every focusable widget in `view`'s tree, in render order.
    ///
    /// This is the single, backend-independent registration point: the runner
    /// calls it once per frame before rendering, so ANSI and native modes
    /// always agree on the Tab order.
    pub fn register_view_tree(&mut self, view: &dyn View) {
        if view.is_focusable() {
            if let Some(id) = view.widget_id() {
                self.register(id.to_string());
            }
        }
        view.for_each_child(&mut |child| self.register_view_tree(child));
    }

    /// Focusable widget IDs registered this frame, in Tab order.
    pub fn focusable_ids(&self) -> &[String] {
        &self.focusable_ids
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
                self.last_focused_index = Some(0);
                self.focused_id = self.focusable_ids.first().cloned();
            }
            Some(current) => {
                let idx = self.focusable_ids.iter().position(|id| id == current);
                let next_idx = match idx {
                    Some(i) => (i + 1) % self.focusable_ids.len(),
                    None => 0,
                };
                self.last_focused_index = Some(next_idx);
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
                let last = self.focusable_ids.len() - 1;
                self.last_focused_index = Some(last);
                self.focused_id = self.focusable_ids.last().cloned();
            }
            Some(current) => {
                let idx = self.focusable_ids.iter().position(|id| id == current);
                let prev_idx = match idx {
                    Some(0) => self.focusable_ids.len() - 1,
                    Some(i) => i - 1,
                    None => self.focusable_ids.len() - 1,
                };
                self.last_focused_index = Some(prev_idx);
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
    /// If it doesn't (widget removed), pick the nearest neighbor by position rather than
    /// jumping to the first widget.
    pub fn end_frame(&mut self) {
        if self.focusable_ids.is_empty() {
            return;
        }
        if let Some(ref id) = self.focused_id {
            if !self.focusable_ids.contains(id) {
                // Use the previous position hint to find the nearest neighbor.
                let fallback_idx = self
                    .last_focused_index
                    .unwrap_or(0)
                    .min(self.focusable_ids.len() - 1);
                self.focused_id = self.focusable_ids.get(fallback_idx).cloned();
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
    fn register_rejects_duplicate_id_in_same_frame() {
        let mut focus = FocusManager::new();
        assert!(focus.register("a".to_string()));
        assert!(!focus.register("a".to_string()));
        assert_eq!(focus.focusable_ids(), ["a".to_string()]);

        focus.begin_frame();
        assert!(focus.register("a".to_string()));
    }

    #[test]
    fn register_view_tree_collects_focusable_ids_in_render_order() {
        use crate::widgets::{Button, Column, Row, ScrollView, Text, TextInput};

        let view = Column::new()
            .child(TextInput::new().id("name"))
            .child(Text::new("not focusable"))
            .child(
                Row::new()
                    .child(Button::new("Save").id("save"))
                    .child(ScrollView::new().child(TextInput::new().id("email"))),
            )
            .child(Button::new("anonymous"));

        let mut focus = FocusManager::new();
        focus.register_view_tree(&view);

        assert_eq!(
            focus.focusable_ids(),
            ["name".to_string(), "save".to_string(), "email".to_string()]
        );
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

        // No previous position hint, so falls back to index 0 ("a").
        assert_eq!(focus.focused_id(), Some("a"));
    }

    #[test]
    fn end_frame_picks_nearest_neighbor_on_removal() {
        let mut focus = FocusManager::new();
        // Simulate frame 1: a, b, c, d — user focuses "c" (index 2).
        focus.register("a".to_string());
        focus.register("b".to_string());
        focus.register("c".to_string());
        focus.register("d".to_string());
        focus.focus_next(); // a
        focus.focus_next(); // b
        focus.focus_next(); // c
        assert_eq!(focus.focused_id(), Some("c"));

        // Simulate frame 2: "c" removed, only a, b, d remain.
        focus.begin_frame();
        focus.register("a".to_string());
        focus.register("b".to_string());
        focus.register("d".to_string());
        focus.end_frame();

        // Should pick index 2 clamped to len-1 = 2, which is "d" (nearest neighbor).
        assert_eq!(focus.focused_id(), Some("d"));
    }

    #[test]
    fn end_frame_clamps_to_last_when_list_shrinks() {
        let mut focus = FocusManager::new();
        focus.register("a".to_string());
        focus.register("b".to_string());
        focus.register("c".to_string());
        focus.register("d".to_string());
        focus.register("e".to_string());
        // Focus "e" at index 4
        focus.focus_prev(); // wraps to "e"
        assert_eq!(focus.focused_id(), Some("e"));

        // Next frame: only 2 items
        focus.begin_frame();
        focus.register("a".to_string());
        focus.register("b".to_string());
        focus.end_frame();

        // Index 4 clamped to 1 → "b"
        assert_eq!(focus.focused_id(), Some("b"));
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
