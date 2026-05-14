use super::super::{Renderer, Vertex};
use super::code_spans::{plan_code_content_spans, CodeColor, CodeSpan};

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

        for spans in plan_code_content_spans(source, language, line_numbers) {
            self.render_code_spans(&spans, x, cursor_y, text_vertices, text_indices);
            cursor_y += line_height;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_code_spans(
        &mut self,
        spans: &[CodeSpan],
        x: f32,
        y: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let line_num_color = self.color_scheme.named[8]; // bright black (dim)
        let keyword_color = self.color_scheme.named[5]; // magenta
        let string_color = self.color_scheme.named[2]; // green
        let comment_color = self.color_scheme.named[8]; // bright black (dim)
        let number_color = self.color_scheme.named[3]; // yellow
        let type_color = self.color_scheme.named[6]; // cyan
        let operator_color = self.color_scheme.named[1]; // red
        let code_color = self.color_scheme.named[7]; // white/default code
        let mut cursor_x = x;

        for span in spans {
            let color = match span.color {
                CodeColor::LineNumber => line_num_color,
                CodeColor::Keyword => keyword_color,
                CodeColor::String => string_color,
                CodeColor::Comment => comment_color,
                CodeColor::Number => number_color,
                CodeColor::Type => type_color,
                CodeColor::Operator => operator_color,
                CodeColor::Plain => code_color,
            };
            let advance = self.text_renderer.append_styled_text_vertices(
                &span.text,
                cursor_x,
                y,
                color,
                span.bold,
                span.italic,
                text_vertices,
                text_indices,
            );
            cursor_x += advance;
        }
    }
}
