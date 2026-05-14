//! Native session dirty-state transitions.

use ultty_protocol::RegionId;

use super::NativeSession;

impl NativeSession {
    /// Mark a whole-scene rebuild where layout constraints changed.
    pub(super) fn mark_full_scene_rebuild(&mut self) {
        self.scene_dirty = true;
        self.constraints_dirty = true;
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
        self.scene_dirty = false;
    }
}
