//! Cursor rendering helpers.
//!
//! Provides underline and bar cursor vertex emitters, both with and
//! without pane offset, for use by the background-vertex builders.

use super::{Renderer, Rgba, Vertex};

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
        let underline_height = 2.0f32.min(self.cell_height * 0.15);
        let y0 = (row + 1) as f32 * self.cell_height + self.tab_bar_height - underline_height;
        let y1 = (row + 1) as f32 * self.cell_height + self.tab_bar_height;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y0], tex_coord: [1.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y1], tex_coord: [1.0, 1.0], color: color_arr });
        vertices.push(Vertex { position: [x0, y1], tex_coord: [0.0, 1.0], color: color_arr });

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
        let bar_width = 2.0f32.min(self.cell_width * 0.15);
        let x0 = col as f32 * self.cell_width;
        let x1 = x0 + bar_width;
        let y0 = row as f32 * self.cell_height + self.tab_bar_height;
        let y1 = (row + 1) as f32 * self.cell_height + self.tab_bar_height;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y0], tex_coord: [1.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y1], tex_coord: [1.0, 1.0], color: color_arr });
        vertices.push(Vertex { position: [x0, y1], tex_coord: [0.0, 1.0], color: color_arr });

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
        let underline_height = 2.0f32.min(self.cell_height * 0.15);
        let y0 = (row + 1) as f32 * self.cell_height + self.pane_offset_y - underline_height;
        let y1 = (row + 1) as f32 * self.cell_height + self.pane_offset_y;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y0], tex_coord: [1.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y1], tex_coord: [1.0, 1.0], color: color_arr });
        vertices.push(Vertex { position: [x0, y1], tex_coord: [0.0, 1.0], color: color_arr });

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
        let bar_width = 2.0f32.min(self.cell_width * 0.15);
        let x0 = col as f32 * self.cell_width + self.pane_offset_x;
        let x1 = x0 + bar_width;
        let y0 = row as f32 * self.cell_height + self.pane_offset_y;
        let y1 = (row + 1) as f32 * self.cell_height + self.pane_offset_y;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y0], tex_coord: [1.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y1], tex_coord: [1.0, 1.0], color: color_arr });
        vertices.push(Vertex { position: [x0, y1], tex_coord: [0.0, 1.0], color: color_arr });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }
}
