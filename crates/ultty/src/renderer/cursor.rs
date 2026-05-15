//! Cursor rendering helpers.
//!
//! Provides underline and bar cursor vertex emitters, both with and
//! without pane offset, for use by the background-vertex builders.

use super::{Renderer, Rgba, Vertex};

const MIN_CURSOR_STROKE: f32 = 2.0;
const MAX_CURSOR_STROKE: f32 = 3.0;
const CURSOR_STROKE_RATIO: f32 = 0.16;

fn cursor_stroke(size: f32) -> f32 {
    if size <= 0.0 {
        return 0.0;
    }

    (size * CURSOR_STROKE_RATIO).clamp(MIN_CURSOR_STROKE, MAX_CURSOR_STROKE)
}

impl Renderer {
    /// Emit a thin underline cursor at the bottom of a cell.
    pub(super) fn emit_cursor_underline(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        col: u16,
        row: u16,
        color: Rgba,
    ) {
        let x0 = col as f32 * self.cell_width;
        let x1 = (col + 1) as f32 * self.cell_width;
        let underline_height = cursor_stroke(self.cell_height);
        let y0 = (row + 1) as f32 * self.cell_height + self.tab_bar_height - underline_height;
        let y1 = (row + 1) as f32 * self.cell_height + self.tab_bar_height;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex {
            position: [x0, y0],
            tex_coord: [0.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y0],
            tex_coord: [1.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y1],
            tex_coord: [1.0, 1.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x0, y1],
            tex_coord: [0.0, 1.0],
            color: color_arr,
            is_color: 0,
        });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Emit a thin bar cursor at the left of a cell.
    pub(super) fn emit_cursor_bar(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        col: u16,
        row: u16,
        color: Rgba,
    ) {
        let bar_width = cursor_stroke(self.cell_width);
        let x0 = col as f32 * self.cell_width;
        let x1 = x0 + bar_width;
        let y0 = row as f32 * self.cell_height + self.tab_bar_height;
        let y1 = (row + 1) as f32 * self.cell_height + self.tab_bar_height;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex {
            position: [x0, y0],
            tex_coord: [0.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y0],
            tex_coord: [1.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y1],
            tex_coord: [1.0, 1.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x0, y1],
            tex_coord: [0.0, 1.0],
            color: color_arr,
            is_color: 0,
        });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Emit a thin underline cursor with pane offset.
    pub(super) fn emit_cursor_underline_with_offset(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        col: u16,
        row: u16,
        color: Rgba,
    ) {
        let x0 = col as f32 * self.cell_width + self.pane_offset_x;
        let x1 = (col + 1) as f32 * self.cell_width + self.pane_offset_x;
        let underline_height = cursor_stroke(self.cell_height);
        let y0 = (row + 1) as f32 * self.cell_height + self.pane_offset_y - underline_height;
        let y1 = (row + 1) as f32 * self.cell_height + self.pane_offset_y;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex {
            position: [x0, y0],
            tex_coord: [0.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y0],
            tex_coord: [1.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y1],
            tex_coord: [1.0, 1.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x0, y1],
            tex_coord: [0.0, 1.0],
            color: color_arr,
            is_color: 0,
        });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Emit a thin bar cursor with pane offset.
    pub(super) fn emit_cursor_bar_with_offset(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        col: u16,
        row: u16,
        color: Rgba,
    ) {
        let bar_width = cursor_stroke(self.cell_width);
        let x0 = col as f32 * self.cell_width + self.pane_offset_x;
        let x1 = x0 + bar_width;
        let y0 = row as f32 * self.cell_height + self.pane_offset_y;
        let y1 = (row + 1) as f32 * self.cell_height + self.pane_offset_y;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex {
            position: [x0, y0],
            tex_coord: [0.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y0],
            tex_coord: [1.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y1],
            tex_coord: [1.0, 1.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x0, y1],
            tex_coord: [0.0, 1.0],
            color: color_arr,
            is_color: 0,
        });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }
}

#[cfg(test)]
mod tests {
    //! Tests for cursor position and size calculations.
    //!
    //! The Renderer methods above require a GPU context so we test the
    //! pure arithmetic that computes cursor geometry directly.

    use super::cursor_stroke;

    /// Replicate underline cursor geometry math from `emit_cursor_underline`.
    fn underline_geometry(
        col: u16,
        row: u16,
        cell_width: f32,
        cell_height: f32,
        tab_bar_height: f32,
    ) -> (f32, f32, f32, f32) {
        let x0 = col as f32 * cell_width;
        let x1 = (col + 1) as f32 * cell_width;
        let underline_height = cursor_stroke(cell_height);
        let y0 = (row + 1) as f32 * cell_height + tab_bar_height - underline_height;
        let y1 = (row + 1) as f32 * cell_height + tab_bar_height;
        (x0, y0, x1, y1)
    }

    /// Replicate bar cursor geometry math from `emit_cursor_bar`.
    fn bar_geometry(
        col: u16,
        row: u16,
        cell_width: f32,
        cell_height: f32,
        tab_bar_height: f32,
    ) -> (f32, f32, f32, f32) {
        let bar_width = cursor_stroke(cell_width);
        let x0 = col as f32 * cell_width;
        let x1 = x0 + bar_width;
        let y0 = row as f32 * cell_height + tab_bar_height;
        let y1 = (row + 1) as f32 * cell_height + tab_bar_height;
        (x0, y0, x1, y1)
    }

    /// Replicate underline with offset geometry from `emit_cursor_underline_with_offset`.
    fn underline_with_offset_geometry(
        col: u16,
        row: u16,
        cell_width: f32,
        cell_height: f32,
        pane_offset_x: f32,
        pane_offset_y: f32,
    ) -> (f32, f32, f32, f32) {
        let x0 = col as f32 * cell_width + pane_offset_x;
        let x1 = (col + 1) as f32 * cell_width + pane_offset_x;
        let underline_height = cursor_stroke(cell_height);
        let y0 = (row + 1) as f32 * cell_height + pane_offset_y - underline_height;
        let y1 = (row + 1) as f32 * cell_height + pane_offset_y;
        (x0, y0, x1, y1)
    }

    /// Replicate bar with offset geometry from `emit_cursor_bar_with_offset`.
    fn bar_with_offset_geometry(
        col: u16,
        row: u16,
        cell_width: f32,
        cell_height: f32,
        pane_offset_x: f32,
        pane_offset_y: f32,
    ) -> (f32, f32, f32, f32) {
        let bar_width = cursor_stroke(cell_width);
        let x0 = col as f32 * cell_width + pane_offset_x;
        let x1 = x0 + bar_width;
        let y0 = row as f32 * cell_height + pane_offset_y;
        let y1 = (row + 1) as f32 * cell_height + pane_offset_y;
        (x0, y0, x1, y1)
    }

    #[test]
    fn underline_position_at_origin() {
        let (x0, y0, x1, y1) = underline_geometry(0, 0, 10.0, 20.0, 0.0);
        assert_eq!(x0, 0.0);
        assert_eq!(x1, 10.0);
        // Underline at bottom of cell row 0: y1 = 20, height = 3.
        assert_eq!(y1, 20.0);
        assert_eq!(y0, 17.0);
    }

    #[test]
    fn underline_position_with_tab_bar() {
        let (_, y0, _, y1) = underline_geometry(0, 0, 10.0, 20.0, 30.0);
        // Bottom of row 0 with 30px tab bar = 20 + 30 = 50
        assert_eq!(y1, 50.0);
        assert_eq!(y0, 47.0);
    }

    #[test]
    fn underline_position_at_col_row() {
        let (x0, _, x1, y1) = underline_geometry(5, 3, 8.0, 16.0, 0.0);
        assert_eq!(x0, 40.0); // 5 * 8
        assert_eq!(x1, 48.0); // 6 * 8
        assert_eq!(y1, 64.0); // 4 * 16
    }

    #[test]
    fn underline_height_clamped_to_max() {
        let (_, y0, _, y1) = underline_geometry(0, 0, 10.0, 100.0, 0.0);
        assert!((y1 - y0 - 3.0).abs() < 1e-5);
    }

    #[test]
    fn underline_height_keeps_minimum_for_small_cells() {
        let (_, y0, _, y1) = underline_geometry(0, 0, 10.0, 10.0, 0.0);
        assert!((y1 - y0 - 2.0).abs() < 1e-5);
    }

    #[test]
    fn bar_position_at_origin() {
        let (x0, y0, x1, y1) = bar_geometry(0, 0, 10.0, 20.0, 0.0);
        assert_eq!(x0, 0.0);
        assert_eq!(y0, 0.0);
        assert_eq!(y1, 20.0);
        assert!((x1 - 2.0).abs() < 1e-5);
    }

    #[test]
    fn bar_width_clamped_to_max() {
        let (x0, _, x1, _) = bar_geometry(0, 0, 100.0, 20.0, 0.0);
        assert!((x1 - x0 - 3.0).abs() < 1e-5);
    }

    #[test]
    fn cursor_stroke_scales_between_minimum_and_maximum() {
        assert_eq!(cursor_stroke(0.0), 0.0);
        assert_eq!(cursor_stroke(8.0), 2.0);
        assert!((cursor_stroke(16.0) - 2.56).abs() < 1e-5);
        assert_eq!(cursor_stroke(100.0), 3.0);
    }

    #[test]
    fn bar_full_cell_height() {
        let (_, y0, _, y1) = bar_geometry(3, 7, 8.0, 16.0, 0.0);
        assert_eq!(y0, 112.0); // 7 * 16
        assert_eq!(y1, 128.0); // 8 * 16
    }

    #[test]
    fn underline_with_offset_shifts_position() {
        let (x0, _, x1, y1) = underline_with_offset_geometry(0, 0, 10.0, 20.0, 50.0, 100.0);
        assert_eq!(x0, 50.0);
        assert_eq!(x1, 60.0);
        assert_eq!(y1, 120.0); // 20 + 100
    }

    #[test]
    fn bar_with_offset_shifts_position() {
        let (x0, y0, _, y1) = bar_with_offset_geometry(0, 0, 10.0, 20.0, 50.0, 100.0);
        assert_eq!(x0, 50.0);
        assert_eq!(y0, 100.0);
        assert_eq!(y1, 120.0);
    }

    #[test]
    fn offset_variants_consistent_with_non_offset() {
        let cell_w = 9.0;
        let cell_h = 18.0;
        let col = 4;
        let row = 2;

        // With zero offsets, offset variant should match the non-offset variant
        // (substituting tab_bar_height for pane_offset_y)
        let (ux0, uy0, ux1, uy1) = underline_geometry(col, row, cell_w, cell_h, 0.0);
        let (ox0, oy0, ox1, oy1) =
            underline_with_offset_geometry(col, row, cell_w, cell_h, 0.0, 0.0);
        assert_eq!(ux0, ox0);
        assert_eq!(uy0, oy0);
        assert_eq!(ux1, ox1);
        assert_eq!(uy1, oy1);

        let (bx0, by0, bx1, by1) = bar_geometry(col, row, cell_w, cell_h, 0.0);
        let (obx0, oby0, obx1, oby1) = bar_with_offset_geometry(col, row, cell_w, cell_h, 0.0, 0.0);
        assert_eq!(bx0, obx0);
        assert_eq!(by0, oby0);
        assert_eq!(bx1, obx1);
        assert_eq!(by1, oby1);
    }
}
