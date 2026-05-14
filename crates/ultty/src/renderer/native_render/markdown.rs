use super::super::{Renderer, Vertex};
use super::markdown_blocks::{plan_markdown_blocks, MarkdownBlock};

impl Renderer {
    pub(super) fn render_markdown_content(
        &mut self,
        source: &str,
        x: f32,
        y: f32,
        _width: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let mut cursor_y = y;
        let line_height = self.cell_height;
        for block in plan_markdown_blocks(source) {
            let line_count = block.line_count();
            match block {
                MarkdownBlock::CodeBlock { language, lines } => {
                    if !lines.is_empty() {
                        let code_source = lines.join("\n");
                        self.render_code_content(
                            &code_source,
                            language,
                            false,
                            x + self.cell_width,
                            cursor_y,
                            text_vertices,
                            text_indices,
                        );
                    }
                }
                MarkdownBlock::Rule => {
                    let count = (_width / self.cell_width.max(1.0)) as usize;
                    let rule = "─".repeat(count.max(1));
                    let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                        &rule,
                        x,
                        cursor_y,
                        self.color_scheme.named[8],
                        false,
                        false,
                    );
                    let base_idx = text_vertices.len() as u32;
                    text_vertices.extend(verts);
                    text_indices.extend(idxs.iter().map(|i| i + base_idx));
                }
                MarkdownBlock::Table(lines) => {
                    self.render_markdown_table(&lines, x, cursor_y, text_vertices, text_indices);
                }
                MarkdownBlock::Header { level: 3, text } => {
                    self.render_markdown_inline(
                        text,
                        x,
                        cursor_y,
                        true,
                        self.color_scheme.named[6],
                        text_vertices,
                        text_indices,
                    );
                }
                MarkdownBlock::Header { level: 2, text } => {
                    self.render_markdown_inline(
                        text,
                        x,
                        cursor_y,
                        true,
                        self.color_scheme.named[5],
                        text_vertices,
                        text_indices,
                    );
                }
                MarkdownBlock::Header { level: 1, text } => {
                    self.render_markdown_inline(
                        text,
                        x,
                        cursor_y,
                        true,
                        self.color_scheme.named[4],
                        text_vertices,
                        text_indices,
                    );
                }
                MarkdownBlock::Header { level: _, text } => {
                    self.render_markdown_inline(
                        text,
                        x,
                        cursor_y,
                        true,
                        self.color_scheme.named[4],
                        text_vertices,
                        text_indices,
                    );
                }
                MarkdownBlock::Task { checked, text } => {
                    let checkbox = if checked { "☑ " } else { "☐ " };
                    let color = if checked {
                        self.color_scheme.named[2]
                    } else {
                        self.color_scheme.named[8]
                    };
                    let (verts, idxs, advance) = self
                        .text_renderer
                        .build_styled_text_vertices(checkbox, x, cursor_y, color, false, false);
                    let base_idx = text_vertices.len() as u32;
                    text_vertices.extend(verts);
                    text_indices.extend(idxs.iter().map(|i| i + base_idx));
                    self.render_markdown_inline(
                        text,
                        x + advance,
                        cursor_y,
                        false,
                        self.color_scheme.foreground,
                        text_vertices,
                        text_indices,
                    );
                }
                MarkdownBlock::BlockQuote { depth, text } => {
                    let indent = self.cell_width * 2.0 * depth as f32;
                    // Render blockquote bars
                    for depth in 0..depth {
                        let bar_x = x + self.cell_width * 2.0 * depth as f32;
                        let bar = "│";
                        let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                            bar,
                            bar_x,
                            cursor_y,
                            self.color_scheme.named[8],
                            false,
                            false,
                        );
                        let base_idx = text_vertices.len() as u32;
                        text_vertices.extend(verts);
                        text_indices.extend(idxs.iter().map(|i| i + base_idx));
                    }
                    if !text.is_empty() {
                        self.render_markdown_inline(
                            text,
                            x + indent,
                            cursor_y,
                            false,
                            self.color_scheme.foreground,
                            text_vertices,
                            text_indices,
                        );
                    }
                }
                MarkdownBlock::OrderedList { prefix, text } => {
                    let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                        &prefix,
                        x,
                        cursor_y,
                        self.color_scheme.named[8],
                        false,
                        false,
                    );
                    let base_idx = text_vertices.len() as u32;
                    text_vertices.extend(verts);
                    text_indices.extend(idxs.iter().map(|i| i + base_idx));
                    self.render_markdown_inline(
                        text,
                        x + advance,
                        cursor_y,
                        false,
                        self.color_scheme.foreground,
                        text_vertices,
                        text_indices,
                    );
                }
                MarkdownBlock::UnorderedList { text } => {
                    let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                        "• ",
                        x,
                        cursor_y,
                        self.color_scheme.named[8],
                        false,
                        false,
                    );
                    let base_idx = text_vertices.len() as u32;
                    text_vertices.extend(verts);
                    text_indices.extend(idxs.iter().map(|i| i + base_idx));
                    self.render_markdown_inline(
                        text,
                        x + advance,
                        cursor_y,
                        false,
                        self.color_scheme.foreground,
                        text_vertices,
                        text_indices,
                    );
                }
                MarkdownBlock::Plain(text) => {
                    self.render_markdown_inline(
                        text,
                        x,
                        cursor_y,
                        false,
                        self.color_scheme.foreground,
                        text_vertices,
                        text_indices,
                    );
                }
            }
            cursor_y += line_height * line_count as f32;
        }
    }

    /// Render a markdown table (rows of `|`-delimited cells).
    pub(super) fn render_markdown_table(
        &mut self,
        lines: &[&str],
        x: f32,
        y: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let line_height = self.cell_height;
        let mut cursor_y = y;

        // Parse cells from each row
        let parse_row = |line: &str| -> Vec<String> {
            line.split('|')
                .skip(1) // skip leading empty
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() || line.ends_with('|'))
                .collect::<Vec<_>>()
        };

        // Compute column widths from all non-separator rows
        let mut col_widths: Vec<usize> = Vec::new();
        for &line in lines {
            let trimmed = line.trim();
            // Skip separator rows like |---|---|
            if trimmed.contains("---") && !trimmed.chars().any(|c| c.is_alphanumeric()) {
                continue;
            }
            let cells = parse_row(line);
            for (ci, cell) in cells.iter().enumerate() {
                let w = cell.len();
                if ci >= col_widths.len() {
                    col_widths.push(w);
                } else if w > col_widths[ci] {
                    col_widths[ci] = w;
                }
            }
        }

        for (row_idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            // Separator row: render a horizontal line
            if trimmed.contains("---") && !trimmed.chars().any(|c| c.is_alphanumeric()) {
                let total_chars: usize = col_widths.iter().map(|w| w + 3).sum::<usize>() + 1;
                let rule = "─".repeat(total_chars);
                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    &rule,
                    x,
                    cursor_y,
                    self.color_scheme.named[8],
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                cursor_y += line_height;
                continue;
            }

            let cells = parse_row(line);
            let is_header = row_idx == 0;
            let mut cursor_x = x;

            for (ci, cell) in cells.iter().enumerate() {
                let col_w = col_widths.get(ci).copied().unwrap_or(cell.len());
                let padded = format!(" {:width$} │", cell, width = col_w);

                let color = if is_header {
                    self.color_scheme.named[6] // cyan for header
                } else {
                    self.color_scheme.foreground
                };

                let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                    &padded, cursor_x, cursor_y, color, is_header, false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                cursor_x += advance;
            }
            cursor_y += line_height;
        }
    }

    /// Render a line of markdown with inline formatting (**bold**, *italic*, `code`, [links](url)).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_markdown_inline(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        line_bold: bool,
        line_color: super::super::color::Rgba,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let mut cursor_x = x;
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let len = chars.len();
        let mut i = 0;
        let mut segment = String::new();

        while i < len {
            // **bold**
            if i + 1 < len && chars[i].1 == '*' && chars[i + 1].1 == '*' {
                if !segment.is_empty() {
                    cursor_x += self.emit_md_segment(
                        &segment,
                        cursor_x,
                        y,
                        line_bold,
                        false,
                        line_color,
                        text_vertices,
                        text_indices,
                    );
                    segment.clear();
                }
                i += 2;
                let mut bold_text = String::new();
                while i + 1 < len && !(chars[i].1 == '*' && chars[i + 1].1 == '*') {
                    bold_text.push(chars[i].1);
                    i += 1;
                }
                if i + 1 < len {
                    i += 2;
                } // skip closing **
                cursor_x += self.emit_md_segment(
                    &bold_text,
                    cursor_x,
                    y,
                    true,
                    false,
                    line_color,
                    text_vertices,
                    text_indices,
                );
                continue;
            }

            // *italic*
            if chars[i].1 == '*' && (i + 1 >= len || chars[i + 1].1 != '*') {
                if !segment.is_empty() {
                    cursor_x += self.emit_md_segment(
                        &segment,
                        cursor_x,
                        y,
                        line_bold,
                        false,
                        line_color,
                        text_vertices,
                        text_indices,
                    );
                    segment.clear();
                }
                i += 1;
                let mut italic_text = String::new();
                while i < len && chars[i].1 != '*' {
                    italic_text.push(chars[i].1);
                    i += 1;
                }
                if i < len {
                    i += 1;
                } // skip closing *
                cursor_x += self.emit_md_segment(
                    &italic_text,
                    cursor_x,
                    y,
                    line_bold,
                    true,
                    line_color,
                    text_vertices,
                    text_indices,
                );
                continue;
            }

            // ~~strikethrough~~
            if i + 1 < len && chars[i].1 == '~' && chars[i + 1].1 == '~' {
                if !segment.is_empty() {
                    cursor_x += self.emit_md_segment(
                        &segment,
                        cursor_x,
                        y,
                        line_bold,
                        false,
                        line_color,
                        text_vertices,
                        text_indices,
                    );
                    segment.clear();
                }
                i += 2;
                let mut strike_text = String::new();
                while i + 1 < len && !(chars[i].1 == '~' && chars[i + 1].1 == '~') {
                    strike_text.push(chars[i].1);
                    i += 1;
                }
                if i + 1 < len {
                    i += 2;
                } // skip closing ~~
                  // Render strikethrough text with dim color and strikethrough decoration
                  // Since we don't have actual text decoration support, render with dim + overstrike
                let strike_color = self.color_scheme.named[8]; // dim
                cursor_x += self.emit_md_segment(
                    &strike_text,
                    cursor_x,
                    y,
                    false,
                    false,
                    strike_color,
                    text_vertices,
                    text_indices,
                );
                continue;
            }

            // `inline code`
            if chars[i].1 == '`' {
                if !segment.is_empty() {
                    cursor_x += self.emit_md_segment(
                        &segment,
                        cursor_x,
                        y,
                        line_bold,
                        false,
                        line_color,
                        text_vertices,
                        text_indices,
                    );
                    segment.clear();
                }
                i += 1;
                let mut code_text = String::new();
                while i < len && chars[i].1 != '`' {
                    code_text.push(chars[i].1);
                    i += 1;
                }
                if i < len {
                    i += 1;
                } // skip closing `
                cursor_x += self.emit_md_segment(
                    &code_text,
                    cursor_x,
                    y,
                    false,
                    false,
                    self.color_scheme.named[7],
                    text_vertices,
                    text_indices,
                );
                continue;
            }

            // [link text](url) — render text as underlined
            if chars[i].1 == '[' {
                let mut link_text = String::new();
                let mut j = i + 1;
                while j < len && chars[j].1 != ']' {
                    link_text.push(chars[j].1);
                    j += 1;
                }
                if j + 1 < len && chars[j].1 == ']' && chars[j + 1].1 == '(' {
                    // Skip past (url)
                    j += 2;
                    while j < len && chars[j].1 != ')' {
                        j += 1;
                    }
                    if j < len {
                        j += 1;
                    }

                    if !segment.is_empty() {
                        cursor_x += self.emit_md_segment(
                            &segment,
                            cursor_x,
                            y,
                            line_bold,
                            false,
                            line_color,
                            text_vertices,
                            text_indices,
                        );
                        segment.clear();
                    }
                    // Render link text in blue/cyan
                    cursor_x += self.emit_md_segment(
                        &link_text,
                        cursor_x,
                        y,
                        false,
                        false,
                        self.color_scheme.named[6],
                        text_vertices,
                        text_indices,
                    );
                    i = j;
                    continue;
                }
            }

            segment.push(chars[i].1);
            i += 1;
        }

        if !segment.is_empty() {
            self.emit_md_segment(
                &segment,
                cursor_x,
                y,
                line_bold,
                false,
                line_color,
                text_vertices,
                text_indices,
            );
        }
    }

    /// Emit a styled text segment and return the x advance.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_md_segment(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        bold: bool,
        italic: bool,
        color: super::super::color::Rgba,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) -> f32 {
        let (verts, idxs, advance) = self
            .text_renderer
            .build_styled_text_vertices(text, x, y, color, bold, italic);
        let base_idx = text_vertices.len() as u32;
        text_vertices.extend(verts);
        text_indices.extend(idxs.iter().map(|i| i + base_idx));
        advance
    }
}
