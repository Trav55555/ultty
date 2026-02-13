use super::super::{Renderer, Vertex};
use ultty_protocol::BorderType;

impl Renderer {
    /// Generate border quads for a region's border style.
    ///
    /// Appends to the provided bg_vertices/bg_indices buffers.
    /// Thickness: Thin/Rounded/Dashed/Ascii → 1.0px, Thick → 2.0px,
    /// Double → 1.0px outer + 1.0px inner with 1px gap.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn generate_border_quads(
        &self,
        border: &ultty_protocol::BorderStyle,
        border_color: &Option<ultty_protocol::Color>,
        fg_color: &Option<ultty_protocol::Color>,
        opacity: f32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        bg_vertices: &mut Vec<Vertex>,
        bg_indices: &mut Vec<u32>,
    ) {
        if border.style == BorderType::None {
            return;
        }

        // Resolve color: border_color -> fg_color -> foreground default
        let color = border_color
            .as_ref()
            .map(|c| self.color_scheme.resolve_protocol_color(c))
            .or_else(|| {
                fg_color
                    .as_ref()
                    .map(|c| self.color_scheme.resolve_protocol_color(c))
            })
            .unwrap_or(self.color_scheme.foreground);

        let color_arr = [color.r, color.g, color.b, color.a * opacity];

        let thickness = match border.style {
            BorderType::Thick => 2.0,
            _ => 1.0,
        };

        let edges = &border.edges;

        // Helper to emit a single quad
        let mut emit_quad = |qx: f32, qy: f32, qw: f32, qh: f32| {
            let base_idx = bg_vertices.len() as u32;
            bg_vertices.push(Vertex {
                position: [qx, qy],
                tex_coord: [0.0, 0.0],
                color: color_arr,
                is_color: 0,
            });
            bg_vertices.push(Vertex {
                position: [qx + qw, qy],
                tex_coord: [1.0, 0.0],
                color: color_arr,
                is_color: 0,
            });
            bg_vertices.push(Vertex {
                position: [qx + qw, qy + qh],
                tex_coord: [1.0, 1.0],
                color: color_arr,
                is_color: 0,
            });
            bg_vertices.push(Vertex {
                position: [qx, qy + qh],
                tex_coord: [0.0, 1.0],
                color: color_arr,
                is_color: 0,
            });
            bg_indices.extend_from_slice(&[
                base_idx,
                base_idx + 1,
                base_idx + 2,
                base_idx,
                base_idx + 2,
                base_idx + 3,
            ]);
        };

        // Outer border edges
        if edges.top {
            emit_quad(x, y, w, thickness);
        }
        if edges.bottom {
            emit_quad(x, y + h - thickness, w, thickness);
        }
        if edges.left {
            emit_quad(x, y, thickness, h);
        }
        if edges.right {
            emit_quad(x + w - thickness, y, thickness, h);
        }

        // Double border: additional inner border with 1px gap
        if border.style == BorderType::Double {
            let offset = 2.0; // 1px outer + 1px gap
            if edges.top {
                emit_quad(x + offset, y + offset, w - 2.0 * offset, thickness);
            }
            if edges.bottom {
                emit_quad(
                    x + offset,
                    y + h - thickness - offset,
                    w - 2.0 * offset,
                    thickness,
                );
            }
            if edges.left {
                emit_quad(x + offset, y + offset, thickness, h - 2.0 * offset);
            }
            if edges.right {
                emit_quad(
                    x + w - thickness - offset,
                    y + offset,
                    thickness,
                    h - 2.0 * offset,
                );
            }
        }
    }
}
