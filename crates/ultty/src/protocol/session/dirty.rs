//! Native session dirty-state transitions.

use ultty_protocol::{LayoutTrigger, RegionId};

use super::NativeSession;

impl NativeSession {
    /// Note that what the session shows may have changed; see
    /// [`NativeSession::shown_generation`].
    pub(super) fn mark_shown_changed(&mut self) {
        self.shown_generation = self.shown_generation.wrapping_add(1);
    }

    /// Mark a whole-scene rebuild where the region tree changed shape.
    pub(super) fn mark_full_scene_rebuild(&mut self) {
        self.scene_dirty = true;
        self.constraints_dirty = true;
        self.tree_dirty = true;
        self.render_dirty = true;
    }

    /// Mark a whole-scene rebuild where scroll/clip caches also changed.
    pub(super) fn mark_full_scene_scroll_rebuild(&mut self) {
        self.mark_full_scene_rebuild();
        self.scroll_dirty = true;
    }

    /// Mark a region whose constraints changed.
    pub(super) fn mark_region_constraints_changed(&mut self, id: RegionId) {
        self.scene_dirty = true;
        self.constraints_dirty = true;
        self.render_dirty = true;
        self.scroll_dirty = true;
        self.dirty_regions.insert(id);
    }

    /// Mark a region whose content or style changed without constraint changes.
    pub(super) fn mark_region_visual_changed(&mut self, id: RegionId) {
        self.scene_dirty = true;
        self.render_dirty = true;
        self.dirty_regions.insert(id);
    }

    /// Mark a scroll command update.
    pub(super) fn mark_scroll_position_changed(&mut self) {
        self.scene_dirty = true;
        self.scroll_dirty = true;
    }

    /// Mark an animated scroll tick.
    pub(super) fn mark_scroll_animation_changed(&mut self) {
        self.scene_dirty = true;
        self.render_dirty = true;
        self.scroll_dirty = true;
    }

    /// Mark an active visual animation tick.
    pub(super) fn mark_animation_visual_changed(&mut self) {
        self.scene_dirty = true;
        self.render_dirty = true;
    }

    /// Clear layout-scene dirty flags after layout has been computed.
    pub(super) fn mark_scene_layout_clean(&mut self) {
        self.constraints_dirty = false;
        self.tree_dirty = false;
        self.scene_dirty = false;
    }

    /// Why the pending scene change needs a layout pass, most structural first.
    pub(super) fn layout_trigger_for_scene_change(&self) -> LayoutTrigger {
        if self.tree_dirty {
            LayoutTrigger::TreeChange
        } else if self.constraints_dirty {
            LayoutTrigger::ConstraintChange
        } else {
            LayoutTrigger::ContentChange
        }
    }
}
