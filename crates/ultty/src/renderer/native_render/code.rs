use super::super::{Renderer, Vertex};
use super::syntax::{code_keywords, is_line_comment, tokenize_code_line, TokenKind};

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

        // If we're inside a block comment, render entire line as comment
        // and check for closing */
        if *in_block_comment {
            if let Some(end_pos) = line.find("*/") {
                // Comment ends on this line
                let comment_part = &line[..end_pos + 2];
                let rest = &line[end_pos + 2..];
                let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                    comment_part,
                    x,
                    y,
                    comment_color,
                    false,
                    true,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                *in_block_comment = false;
                if !rest.is_empty() {
                    self.render_code_line(
                        rest,
                        language,
                        keywords,
                        in_block_comment,
                        x + advance,
                        y,
                        text_vertices,
                        text_indices,
                    );
                }
            } else {
                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    line,
                    x,
                    y,
                    comment_color,
                    false,
                    true,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
            }
            return;
        }

        let trimmed = line.trim_start();

        // Full-line comment detection (language-aware)
        if is_line_comment(trimmed, language) {
            let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                line,
                x,
                y,
                comment_color,
                false,
                true,
            );
            let base_idx = text_vertices.len() as u32;
            text_vertices.extend(verts);
            text_indices.extend(idxs.iter().map(|i| i + base_idx));
            return;
        }

        // Tokenize the line into segments
        let tokens = tokenize_code_line(line, keywords, in_block_comment);
        let mut cursor_x = x;

        for token in &tokens {
            let (color, bold, italic) = match token.kind {
                TokenKind::Keyword => (keyword_color, true, false),
                TokenKind::String => (string_color, false, false),
                TokenKind::Comment => (comment_color, false, true),
                TokenKind::Number => (number_color, false, false),
                TokenKind::Type => (type_color, false, false),
                TokenKind::Operator => (operator_color, false, false),
                TokenKind::Plain => (code_color, false, false),
            };
            let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                &token.text,
                cursor_x,
                y,
                color,
                bold,
                italic,
            );
            let base_idx = text_vertices.len() as u32;
            text_vertices.extend(verts);
            text_indices.extend(idxs.iter().map(|i| i + base_idx));
            cursor_x += advance;
        }
    }
}
