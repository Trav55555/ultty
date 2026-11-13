//! Read-only native rendering input, independent of application transports.

use ultty_protocol::RegionId;

use crate::protocol::session::PlacedSurface;
use crate::protocol::NativeSession;
use crate::scene::SceneTree;

/// A committed native scene that a renderer can inspect but cannot mutate.
/// Live owners and attachment replicas implement the same rendering contract.
pub trait NativeRenderSource {
    /// Owner-scoped native session identity, used for renderer caches.
    fn session_id(&self) -> u64;
    /// Monotonic displayed-scene revision within this session.
    fn revision(&self) -> u64;
    /// Last committed scene, including full-scene and inline-surface roots.
    fn scene(&self) -> &SceneTree;
    /// Canonical viewport in cells.
    fn viewport(&self) -> (f32, f32);
    /// Whether this scene replaces the pane's VT grid.
    fn is_full_scene(&self) -> bool;
    /// Scroll offset for one region, in cells.
    fn scroll_offset(&self, id: RegionId) -> (f32, f32);
    /// Whether a region has an unfinished content stream.
    fn is_streaming(&self, id: RegionId) -> bool;
    /// Whether geometry/content caches need rebuilding.
    fn is_render_dirty(&self) -> bool;
    /// Whether scroll/clip caches need rebuilding.
    fn is_scroll_dirty(&self) -> bool;
    /// Visit placed surfaces in creation order, without allocating an iterator.
    fn placed_surfaces(&self, visit: &mut dyn FnMut(PlacedSurface));
}

impl NativeRenderSource for NativeSession {
    fn session_id(&self) -> u64 {
        NativeSession::session_id(self)
    }
    fn revision(&self) -> u64 {
        self.shown_generation()
    }
    fn scene(&self) -> &SceneTree {
        NativeSession::scene(self)
    }
    fn viewport(&self) -> (f32, f32) {
        NativeSession::viewport(self)
    }
    fn is_full_scene(&self) -> bool {
        NativeSession::is_full_scene(self)
    }
    fn scroll_offset(&self, id: RegionId) -> (f32, f32) {
        NativeSession::scroll_offset(self, id)
    }
    fn is_streaming(&self, id: RegionId) -> bool {
        NativeSession::is_streaming(self, id)
    }
    fn is_render_dirty(&self) -> bool {
        NativeSession::is_render_dirty(self)
    }
    fn is_scroll_dirty(&self) -> bool {
        NativeSession::is_scroll_dirty(self)
    }
    fn placed_surfaces(&self, visit: &mut dyn FnMut(PlacedSurface)) {
        for surface in NativeSession::placed_surfaces(self) {
            visit(surface);
        }
    }
}

/// Hit-test the displayed full scene, including ancestor scrolling and clipping.
/// Later overlapping siblings take precedence. Hidden and disabled subtrees do
/// not receive input. Coordinates in the result are local to the painted region.
pub fn hit_test(source: &dyn NativeRenderSource, x: f32, y: f32) -> (RegionId, f32, f32) {
    struct HitTest<'a> {
        source: &'a dyn NativeRenderSource,
        point: (f32, f32),
        best: (RegionId, f32, f32),
    }
    impl HitTest<'_> {
        fn visit(&mut self, id: RegionId, offset: (f32, f32), clip: [f32; 4]) {
            use ultty_protocol::{Display, OverflowAxis};
            let Some(region) = self.source.scene().get(id) else {
                return;
            };
            if region.constraints.display == Display::None
                || region.semantics().hidden
                || region.semantics().disabled
                || !region.style.opacity.is_finite()
                || region.style.opacity <= 0.0
            {
                return;
            }
            let (x, y) = self.point;
            if x < clip[0] || y < clip[1] || x >= clip[2] || y >= clip[3] {
                return;
            }
            let layout = region.computed;
            let left = layout.x - offset.0;
            let top = layout.y - offset.1;
            let right = left + layout.width;
            let bottom = top + layout.height;
            if x >= left && x < right && y >= top && y < bottom {
                self.best = (id, x - left, y - top);
            }
            let mut child_clip = clip;
            let mut child_offset = offset;
            let scroll = self.source.scroll_offset(id);
            if matches!(
                region.constraints.overflow.x,
                OverflowAxis::Scroll | OverflowAxis::Hidden
            ) {
                child_clip[0] = clip[0].max(left);
                child_clip[2] = clip[2].min(right);
            }
            if matches!(
                region.constraints.overflow.y,
                OverflowAxis::Scroll | OverflowAxis::Hidden
            ) {
                child_clip[1] = clip[1].max(top);
                child_clip[3] = clip[3].min(bottom);
            }
            if region.constraints.overflow.x == OverflowAxis::Scroll {
                child_offset.0 += scroll.0;
            }
            if region.constraints.overflow.y == OverflowAxis::Scroll {
                child_offset.1 += scroll.1;
            }
            for child in &region.children {
                self.visit(*child, child_offset, child_clip);
            }
        }
    }
    let mut test = HitTest {
        source,
        point: (x, y),
        best: (RegionId::ROOT, x, y),
    };
    if x.is_finite() && y.is_finite() {
        let (width, height) = source.viewport();
        test.visit(source.scene().root(), (0.0, 0.0), [0.0, 0.0, width, height]);
    }
    test.best
}
