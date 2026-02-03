//! Scrollbar rendering.
//!
//! Provides the `ScrollbarState` type and the vertex-building logic
//! for the scrollbar track and thumb.

use super::{Renderer, Rgba, Vertex};

/// Scrollbar state for rendering.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollbarState {
    /// Total number of lines (history + viewport)
    pub total_lines: usize,
    /// Number of visible lines (viewport rows)
    pub viewport_lines: usize,
    /// Current scroll offset (0 = bottom, max = top of history)
    pub scroll_offset: usize,
}

impl ScrollbarState {
    /// Create a new scrollbar state.
    pub fn new(total_lines: usize, viewport_lines: usize, scroll_offset: usize) -> Self {
        Self {
            total_lines,
            viewport_lines,
            scroll_offset,
        }
    }

    /// Check if the scrollbar should be visible.
    pub fn is_visible(&self) -> bool {
        self.total_lines > self.viewport_lines
    }

    /// Calculate the thumb position and size as fractions (0.0-1.0).
    /// Returns (position, size) where position is from top.
    pub fn thumb_geometry(&self) -> (f32, f32) {
        if self.total_lines <= self.viewport_lines {
            return (0.0, 1.0);
        }

        let total = self.total_lines as f32;
        let viewport = self.viewport_lines as f32;
        let max_offset = (self.total_lines - self.viewport_lines) as f32;
        let offset = self.scroll_offset as f32;

        // Thumb size proportional to viewport / total
        let thumb_size = (viewport / total).clamp(0.05, 1.0);

        // Position: 0 when at bottom (offset=0), 1-size when at top (offset=max)
        let thumb_pos = if max_offset > 0.0 {
            (offset / max_offset) * (1.0 - thumb_size)
        } else {
            0.0
        };

        // Invert because 0 offset = at bottom = thumb at bottom
        let thumb_pos = (1.0 - thumb_size) - thumb_pos;

        (thumb_pos, thumb_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_visible_when_content_fits() {
        let sb = ScrollbarState::new(24, 24, 0);
        assert!(!sb.is_visible());
    }

    #[test]
    fn not_visible_when_fewer_lines() {
        let sb = ScrollbarState::new(10, 24, 0);
        assert!(!sb.is_visible());
    }

    #[test]
    fn visible_when_content_exceeds_viewport() {
        let sb = ScrollbarState::new(100, 24, 0);
        assert!(sb.is_visible());
    }

    #[test]
    fn thumb_full_when_content_fits() {
        let sb = ScrollbarState::new(20, 24, 0);
        let (pos, size) = sb.thumb_geometry();
        assert_eq!(pos, 0.0);
        assert_eq!(size, 1.0);
    }

    #[test]
    fn thumb_at_bottom_when_offset_zero() {
        let sb = ScrollbarState::new(100, 24, 0);
        let (pos, size) = sb.thumb_geometry();
        // Thumb should be at bottom: pos + size ~= 1.0
        assert!((pos + size - 1.0).abs() < 0.001, "pos={pos}, size={size}");
    }

    #[test]
    fn thumb_at_top_when_offset_max() {
        let sb = ScrollbarState::new(100, 24, 76); // max offset = 100-24 = 76
        let (pos, size) = sb.thumb_geometry();
        // Thumb should be at top: pos ~= 0.0
        assert!(pos.abs() < 0.001, "pos={pos}, size={size}");
    }

    #[test]
    fn thumb_size_proportional_to_viewport() {
        let sb = ScrollbarState::new(100, 50, 0);
        let (_, size) = sb.thumb_geometry();
        // viewport/total = 0.5
        assert!((size - 0.5).abs() < 0.001, "size={size}");
    }

    #[test]
    fn thumb_size_minimum_clamp() {
        // Very large scrollback, tiny viewport
        let sb = ScrollbarState::new(100_000, 24, 0);
        let (_, size) = sb.thumb_geometry();
        // Should be clamped to min 0.05
        assert!((size - 0.05).abs() < 0.001, "size={size}");
    }

    #[test]
    fn thumb_midpoint_at_half_scroll() {
        let sb = ScrollbarState::new(100, 24, 38); // half of 76
        let (pos, size) = sb.thumb_geometry();
        // Thumb center should be near the middle
        let center = pos + size / 2.0;
        assert!((center - 0.5).abs() < 0.05, "center={center}, pos={pos}, size={size}");
    }

    #[test]
    fn default_scrollbar_state() {
        let sb = ScrollbarState::default();
        assert_eq!(sb.total_lines, 0);
        assert_eq!(sb.viewport_lines, 0);
        assert_eq!(sb.scroll_offset, 0);
        assert!(!sb.is_visible());
    }
}

impl Renderer {
    /// Emit scrollbar vertices.
    pub(super) fn emit_scrollbar(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        scrollbar: &ScrollbarState,
    ) {
        let scrollbar_width = 8.0;
        let scrollbar_margin = 2.0;
        let terminal_height = self.config.height as f32 - self.tab_bar_height;

        // Scrollbar track position (right side of terminal)
        let track_x = self.config.width as f32 - scrollbar_width - scrollbar_margin;
        let track_y = self.tab_bar_height + scrollbar_margin;
        let track_height = terminal_height - scrollbar_margin * 2.0;

        // Draw track (darker background)
        let track_color = Rgba::new(0.15, 0.15, 0.15, 0.6);
        self.emit_rect(
            vertices,
            indices,
            track_x,
            track_y,
            scrollbar_width,
            track_height,
            track_color,
        );

        // Calculate thumb position and size
        let (thumb_pos, thumb_size) = scrollbar.thumb_geometry();
        let thumb_y = track_y + thumb_pos * track_height;
        let thumb_height = (thumb_size * track_height).max(20.0); // Minimum thumb size

        // Draw thumb (brighter)
        let thumb_color = if scrollbar.scroll_offset > 0 {
            Rgba::new(0.5, 0.5, 0.5, 0.8) // Active scroll position
        } else {
            Rgba::new(0.35, 0.35, 0.35, 0.6) // At bottom
        };

        self.emit_rect(
            vertices,
            indices,
            track_x + 1.0,
            thumb_y,
            scrollbar_width - 2.0,
            thumb_height,
            thumb_color,
        );
    }
}
