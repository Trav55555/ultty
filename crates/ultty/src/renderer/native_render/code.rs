use super::super::{Renderer, Vertex};
use super::code_spans::{plan_code_line_spans, CodeColor};
use super::syntax::code_keywords;

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_code_content(
        &mut self,
        source: &str,
        language: &str,
        line_numbers: bool,
        x: f32,
        y: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let mut cursor_y = y;
        let line_height = self.cell_height;
        let line_num_color = self.color_scheme.named[8];
        let keywords = code_keywords(language);
        let mut in_block_comment = false;

        for (idx, line) in source.lines().enumerate() {
            let mut cursor_x = x;

            if line_numbers {
                let line_num = format!("{:4} ", idx + 1);
                let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                    &line_num,
                    cursor_x,
                    cursor_y,
                    line_num_color,
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                cursor_x += advance;
            }

            self.render_code_line(
                line,
                language,
                &keywords,
                &mut in_block_comment,
                cursor_x,
                cursor_y,
                text_vertices,
                text_indices,
            );
            cursor_y += line_height;
        }
    }

    /// Render a single line of code with basic syntax highlighting.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_code_line(
        &mut self,
        line: &str,
        language: &str,
        keywords: &[&str],
        in_block_comment: &mut bool,
        x: f32,
        y: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let keyword_color = self.color_scheme.named[5]; // magenta
        let string_color = self.color_scheme.named[2]; // green
        let comment_color = self.color_scheme.named[8]; // bright black (dim)
        let number_color = self.color_scheme.named[3]; // yellow
        let type_color = self.color_scheme.named[6]; // cyan
        let operator_color = self.color_scheme.named[1]; // red
        let code_color = self.color_scheme.named[7]; // white/default code

        let spans = plan_code_line_spans(line, language, keywords, in_block_comment);
        let mut cursor_x = x;

        for span in &spans {
            let color = match span.color {
                CodeColor::Keyword => keyword_color,
                CodeColor::String => string_color,
                CodeColor::Comment => comment_color,
                CodeColor::Number => number_color,
                CodeColor::Type => type_color,
                CodeColor::Operator => operator_color,
                CodeColor::Plain => code_color,
            };
            let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                &span.text,
                cursor_x,
                y,
                color,
                span.bold,
                span.italic,
            );
            let base_idx = text_vertices.len() as u32;
            text_vertices.extend(verts);
            text_indices.extend(idxs.iter().map(|i| i + base_idx));
            cursor_x += advance;
        }
    }
}
