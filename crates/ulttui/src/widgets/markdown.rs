use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style as RStyle};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use super::View;
use ultty_protocol::{
    AppMessage, BorderEdges, BorderStyle, BorderType, Color, Constraints, Content, Dimension,
    Display, NamedColor, RegionId, RichText, Style, TextContent,
};

const STREAMING_CURSOR: &str = "|";

#[derive(Debug, Clone)]
pub struct MarkdownView {
    source: String,
    blocks: Vec<Block>,
    streaming: bool,
    wrap_width_cols: Option<usize>,
}

#[derive(Debug, Clone)]
struct StyledSpan {
    text: String,
    style: Style,
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum Block {
    Heading {
        level: HeadingLevel,
        spans: Vec<StyledSpan>,
    },
    Paragraph {
        spans: Vec<StyledSpan>,
    },
    CodeBlock {
        text: String,
    },
    ListItem {
        indent: usize,
        prefix: String,
        spans: Vec<StyledSpan>,
    },
    HorizontalRule,
    BlockQuote {
        text: String,
    },
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum CurrentBlock {
    Heading {
        level: HeadingLevel,
        spans: Vec<StyledSpan>,
        quoted: bool,
    },
    Paragraph {
        spans: Vec<StyledSpan>,
        quoted: bool,
    },
    CodeBlock {
        text: String,
        quoted: bool,
    },
    ListItem {
        indent: usize,
        prefix: String,
        spans: Vec<StyledSpan>,
        quoted: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineStyleMarker {
    Strong,
    Emphasis,
    Link,
}

#[derive(Debug, Clone)]
struct ListState {
    ordered: bool,
    next_number: u64,
}

impl MarkdownView {
    pub fn new(source: impl Into<String>) -> Self {
        let source = source.into();
        let blocks = Self::parse_blocks(&source);
        Self {
            source,
            blocks,
            streaming: false,
            wrap_width_cols: None,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn append_chunk(&mut self, chunk: impl AsRef<str>) {
        let chunk = chunk.as_ref();
        if chunk.is_empty() {
            return;
        }

        let was_in_open_code_block = Self::is_in_open_fenced_code_block(&self.source);
        self.source.push_str(chunk);
        if !self.try_append_chunk_incrementally(chunk, was_in_open_code_block) {
            self.blocks = Self::parse_blocks(&self.source);
        }
    }

    pub fn with_appended(mut self, chunk: impl AsRef<str>) -> Self {
        self.append_chunk(chunk);
        self
    }

    /// Enable streaming mode.
    ///
    /// When `true`, a cursor is appended to the rendered output
    /// to indicate content is still arriving. Toggle off to finalize.
    pub fn streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    /// Set the width, in terminal cells, used to estimate native wrapped height.
    pub fn wrap_width_cols(mut self, cols: usize) -> Self {
        self.wrap_width_cols = Some(cols.max(1));
        self
    }

    fn parse_blocks(source: &str) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut current: Option<CurrentBlock> = None;
        let mut inline_style_stack: Vec<InlineStyleMarker> = Vec::new();
        let mut list_stack: Vec<ListState> = Vec::new();
        let mut quote_depth = 0usize;

        let parser = Parser::new_ext(source, Options::all());
        for event in parser {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Paragraph => {
                        if let Some(done) = current.take() {
                            blocks.push(Self::finish_block(done));
                        }
                        current = Some(CurrentBlock::Paragraph {
                            spans: Vec::new(),
                            quoted: quote_depth > 0,
                        });
                    }
                    Tag::Heading { level, .. } => {
                        if let Some(done) = current.take() {
                            blocks.push(Self::finish_block(done));
                        }
                        current = Some(CurrentBlock::Heading {
                            level,
                            spans: Vec::new(),
                            quoted: quote_depth > 0,
                        });
                    }
                    Tag::CodeBlock(_) => {
                        if let Some(done) = current.take() {
                            blocks.push(Self::finish_block(done));
                        }
                        current = Some(CurrentBlock::CodeBlock {
                            text: String::new(),
                            quoted: quote_depth > 0,
                        });
                    }
                    Tag::List(start) => {
                        let (ordered, first) = match start {
                            Some(n) => (true, n),
                            None => (false, 1),
                        };
                        list_stack.push(ListState {
                            ordered,
                            next_number: first,
                        });
                    }
                    Tag::Item => {
                        if let Some(done) = current.take() {
                            blocks.push(Self::finish_block(done));
                        }
                        let depth = list_stack.len().saturating_sub(1);
                        let indent = depth * 2;
                        let prefix = if let Some(last) = list_stack.last_mut() {
                            if last.ordered {
                                let p = format!("{}. ", last.next_number);
                                last.next_number += 1;
                                p
                            } else {
                                "• ".to_string()
                            }
                        } else {
                            "• ".to_string()
                        };
                        current = Some(CurrentBlock::ListItem {
                            indent,
                            prefix,
                            spans: Vec::new(),
                            quoted: quote_depth > 0,
                        });
                    }
                    Tag::BlockQuote(_) => {
                        quote_depth += 1;
                    }
                    Tag::Strong => inline_style_stack.push(InlineStyleMarker::Strong),
                    Tag::Emphasis => inline_style_stack.push(InlineStyleMarker::Emphasis),
                    Tag::Link { .. } => inline_style_stack.push(InlineStyleMarker::Link),
                    _ => {}
                },
                Event::End(tag) => match tag {
                    TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item => {
                        if let Some(done) = current.take() {
                            blocks.push(Self::finish_block(done));
                        }
                    }
                    TagEnd::List(_) => {
                        list_stack.pop();
                    }
                    TagEnd::BlockQuote(_) => {
                        quote_depth = quote_depth.saturating_sub(1);
                    }
                    TagEnd::Strong => {
                        Self::pop_inline_style(&mut inline_style_stack, InlineStyleMarker::Strong);
                    }
                    TagEnd::Emphasis => {
                        Self::pop_inline_style(
                            &mut inline_style_stack,
                            InlineStyleMarker::Emphasis,
                        );
                    }
                    TagEnd::Link => {
                        Self::pop_inline_style(&mut inline_style_stack, InlineStyleMarker::Link);
                    }
                    _ => {}
                },
                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                    Self::push_text_span(&mut current, &inline_style_stack, text.as_ref(), false);
                }
                Event::Code(text) => {
                    Self::push_text_span(&mut current, &inline_style_stack, text.as_ref(), true);
                }
                Event::SoftBreak | Event::HardBreak => {
                    Self::push_text_span(&mut current, &inline_style_stack, "\n", false);
                }
                Event::Rule => blocks.push(Block::HorizontalRule),
                Event::TaskListMarker(checked) => {
                    let marker = if checked { "[x] " } else { "[ ] " };
                    Self::push_text_span(&mut current, &inline_style_stack, marker, false);
                }
                _ => {}
            }
        }

        if let Some(done) = current.take() {
            blocks.push(Self::finish_block(done));
        }

        blocks
    }

    fn try_append_chunk_incrementally(
        &mut self,
        chunk: &str,
        was_in_open_code_block: bool,
    ) -> bool {
        if was_in_open_code_block {
            if Self::chunk_closes_fenced_code_block(chunk) {
                return false;
            }
            if let Some(Block::CodeBlock { text }) = self.blocks.last_mut() {
                text.push_str(chunk);
                return true;
            }
        }

        if Self::is_plain_paragraph_append(chunk) {
            if let Some(Block::Paragraph { spans }) = self.blocks.last_mut() {
                append_plain_text_span(spans, chunk);
                return true;
            }
        }

        false
    }

    fn is_plain_paragraph_append(chunk: &str) -> bool {
        !chunk.is_empty()
            && !chunk.contains('\n')
            && !chunk.chars().any(|ch| {
                matches!(
                    ch,
                    '*' | '_' | '`' | '[' | ']' | '<' | '>' | '#' | '!' | '|'
                )
            })
    }

    fn is_in_open_fenced_code_block(source: &str) -> bool {
        source
            .lines()
            .filter(|line| line.trim_start().starts_with("```"))
            .count()
            % 2
            == 1
    }

    fn chunk_closes_fenced_code_block(chunk: &str) -> bool {
        chunk
            .lines()
            .any(|line| line.trim_start().starts_with("```"))
    }

    fn pop_inline_style(stack: &mut Vec<InlineStyleMarker>, marker: InlineStyleMarker) {
        if let Some(pos) = stack.iter().rposition(|m| *m == marker) {
            stack.remove(pos);
        }
    }

    fn push_text_span(
        current: &mut Option<CurrentBlock>,
        inline_style_stack: &[InlineStyleMarker],
        text: &str,
        inline_code: bool,
    ) {
        if let Some(CurrentBlock::CodeBlock { text: code, .. }) = current {
            code.push_str(text);
            return;
        }

        let mut style = Style::new();
        if inline_style_stack
            .iter()
            .any(|m| matches!(m, InlineStyleMarker::Strong))
        {
            style.bold = true;
        }
        if inline_style_stack
            .iter()
            .any(|m| matches!(m, InlineStyleMarker::Emphasis))
        {
            style.italic = true;
        }
        if inline_style_stack
            .iter()
            .any(|m| matches!(m, InlineStyleMarker::Link))
        {
            style.underline = true;
            style.fg = Some(Color::Named(NamedColor::Cyan));
        }
        if inline_code {
            style.dim = true;
        }

        let span = StyledSpan {
            text: text.to_string(),
            style,
        };

        match current {
            Some(CurrentBlock::Heading { spans, .. })
            | Some(CurrentBlock::Paragraph { spans, .. })
            | Some(CurrentBlock::ListItem { spans, .. }) => spans.push(span),
            _ => {}
        }
    }

    fn finish_block(current: CurrentBlock) -> Block {
        match current {
            CurrentBlock::Heading {
                level,
                mut spans,
                quoted,
            } => {
                if level == HeadingLevel::H1 {
                    for span in &mut spans {
                        span.text = span.text.to_uppercase();
                    }
                }
                if quoted {
                    Block::BlockQuote {
                        text: spans_to_plain_text(&spans),
                    }
                } else {
                    Block::Heading { level, spans }
                }
            }
            CurrentBlock::Paragraph { spans, quoted } => {
                if quoted {
                    Block::BlockQuote {
                        text: spans_to_plain_text(&spans),
                    }
                } else {
                    Block::Paragraph { spans }
                }
            }
            CurrentBlock::CodeBlock { text, quoted } => {
                if quoted {
                    Block::BlockQuote { text }
                } else {
                    Block::CodeBlock { text }
                }
            }
            CurrentBlock::ListItem {
                indent,
                prefix,
                spans,
                quoted,
            } => {
                if quoted {
                    let mut content = " ".repeat(indent);
                    content.push_str(&prefix);
                    content.push_str(&spans_to_plain_text(&spans));
                    Block::BlockQuote { text: content }
                } else {
                    Block::ListItem {
                        indent,
                        prefix,
                        spans,
                    }
                }
            }
        }
    }

    fn line_count(&self) -> usize {
        let total: usize = self
            .blocks
            .iter()
            .map(|block| block.line_count(self.wrap_width_cols))
            .sum();
        if self.streaming && self.blocks.is_empty() {
            return 1;
        }
        total.max(1)
    }
}

impl Block {
    fn line_count(&self, wrap_width_cols: Option<usize>) -> usize {
        match self {
            Block::Heading { spans, .. } | Block::Paragraph { spans } => {
                count_lines_from_spans(spans, wrap_width_cols)
            }
            Block::CodeBlock { text } | Block::BlockQuote { text } => count_lines(text),
            Block::ListItem {
                indent,
                prefix,
                spans,
            } => {
                let mut text = " ".repeat(*indent);
                text.push_str(prefix);
                text.push_str(&spans_to_plain_text(spans));
                count_lines_wrapped(&text, wrap_width_cols)
            }
            Block::HorizontalRule => 1,
        }
    }
}

impl View for MarkdownView {
    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(self.line_count() as f32),
            flex_shrink: 1.0,
            ..Default::default()
        }
    }

    fn content(&self) -> Content {
        Content::None
    }

    fn children_count(&self) -> usize {
        if self.streaming && self.blocks.is_empty() {
            1
        } else {
            self.blocks.len()
        }
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        let blocks = &self.blocks;
        let is_last = index + 1 == self.children_count();
        let append_cursor = self.streaming && is_last;

        if blocks.is_empty() && self.streaming && index == 0 {
            let region_id = RegionId::new(*next_id);
            *next_id += 1;
            send_message(&AppMessage::CreateRegion {
                id: region_id,
                parent,
                insert_index: -1,
                constraints: Constraints {
                    display: Display::Flex,
                    width: Dimension::Percent(100.0),
                    height: Dimension::Cells(1.0),
                    flex_shrink: 1.0,
                    ..Default::default()
                },
                style: Style::new(),
            })?;
            send_message(&AppMessage::SetContent {
                id: region_id,
                content: Content::Text(TextContent::new(STREAMING_CURSOR)),
            })?;
            return Ok(Some(region_id));
        }

        let Some(block) = blocks.get(index) else {
            return Ok(None);
        };

        let region_id = RegionId::new(*next_id);
        *next_id += 1;

        let mut style = Style::new();
        if matches!(block, Block::CodeBlock { .. } | Block::BlockQuote { .. }) {
            style.dim = true;
        }
        if matches!(block, Block::CodeBlock { .. }) {
            style.border = BorderStyle {
                style: BorderType::Thin,
                edges: BorderEdges {
                    left: true,
                    ..BorderEdges::NONE
                },
                radius: 0,
            };
        }

        send_message(&AppMessage::CreateRegion {
            id: region_id,
            parent,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(block.line_count(self.wrap_width_cols) as f32),
                flex_shrink: 1.0,
                ..Default::default()
            },
            style,
        })?;

        let content = match block {
            Block::Heading { level, spans } => {
                let mut rich = RichText::new();
                for span in spans {
                    let mut span_style = span.style.clone();
                    span_style.bold = true;
                    let text = if *level == HeadingLevel::H1 {
                        span.text.to_uppercase()
                    } else {
                        span.text.clone()
                    };
                    rich = rich.span(text, span_style);
                }
                Content::RichText(rich)
            }
            Block::Paragraph { spans } => {
                let mut rich = RichText::new();
                for span in spans {
                    rich = rich.span(span.text.clone(), span.style.clone());
                }
                Content::RichText(rich)
            }
            Block::CodeBlock { text } => Content::Text(TextContent::new(text.clone())),
            Block::ListItem {
                indent,
                prefix,
                spans,
            } => {
                let mut rich = RichText::new()
                    .span(format!("{}{}", " ".repeat(*indent), prefix), Style::new());
                for span in spans {
                    rich = rich.span(span.text.clone(), span.style.clone());
                }
                Content::RichText(rich)
            }
            Block::HorizontalRule => Content::Text(TextContent::new("─")),
            Block::BlockQuote { text } => {
                let quoted = text
                    .lines()
                    .map(|line| format!("│ {}", line))
                    .collect::<Vec<_>>()
                    .join("\n");
                Content::Text(TextContent::new(if quoted.is_empty() {
                    "│ ".to_string()
                } else {
                    quoted
                }))
            }
        };

        let content = if append_cursor {
            append_cursor_to_content(content)
        } else {
            content
        };

        send_message(&AppMessage::SetContent {
            id: region_id,
            content,
        })?;

        Ok(Some(region_id))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let blocks = &self.blocks;
        let mut lines: Vec<Line<'static>> = Vec::new();

        for block in blocks {
            match block {
                Block::Heading { level, spans } => {
                    let heading_style = Style::new().bold();
                    lines.extend(spans_to_lines(
                        spans,
                        heading_style,
                        *level == HeadingLevel::H1,
                        None,
                    ));
                }
                Block::Paragraph { spans } => {
                    lines.extend(spans_to_lines(spans, Style::new(), false, None));
                }
                Block::CodeBlock { text } => {
                    let mut dim_style = Style::new();
                    dim_style.dim = true;
                    let code_style = proto_style_to_ratatui_style(&dim_style);
                    for line in text.lines() {
                        lines.push(Line::from(vec![Span::styled(
                            format!("│ {}", line),
                            code_style,
                        )]));
                    }
                    if text.is_empty() {
                        lines.push(Line::from(vec![Span::styled("│ ", code_style)]));
                    }
                }
                Block::ListItem {
                    indent,
                    prefix,
                    spans,
                } => {
                    lines.extend(spans_to_lines(
                        spans,
                        Style::new(),
                        false,
                        Some(format!("{}{}", " ".repeat(*indent), prefix)),
                    ));
                }
                Block::HorizontalRule => {
                    let width = area.width.max(1) as usize;
                    lines.push(Line::from(vec![Span::raw("─".repeat(width))]));
                }
                Block::BlockQuote { text } => {
                    let mut dim_style = Style::new();
                    dim_style.dim = true;
                    let quote_style = proto_style_to_ratatui_style(&dim_style);
                    for line in text.lines() {
                        lines.push(Line::from(vec![Span::styled(
                            format!("│ {}", line),
                            quote_style,
                        )]));
                    }
                    if text.is_empty() {
                        lines.push(Line::from(vec![Span::styled("│ ", quote_style)]));
                    }
                }
            }
        }

        if lines.is_empty() {
            lines.push(Line::from(""));
        }

        if self.streaming {
            if let Some(last) = lines.last_mut() {
                last.spans.push(Span::raw(STREAMING_CURSOR));
            }
        }

        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn append_cursor_to_content(content: Content) -> Content {
    match content {
        Content::Text(mut tc) => {
            tc.text.push_str(STREAMING_CURSOR);
            Content::Text(tc)
        }
        Content::RichText(mut rt) => {
            rt.spans.push(ultty_protocol::Span {
                text: STREAMING_CURSOR.to_string(),
                style: Style::new(),
            });
            Content::RichText(rt)
        }
        other => other,
    }
}

fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        1
    } else {
        text.lines().count().max(1)
    }
}

fn count_lines_from_spans(spans: &[StyledSpan], wrap_width_cols: Option<usize>) -> usize {
    count_lines_wrapped(&spans_to_plain_text(spans), wrap_width_cols)
}

fn count_lines_wrapped(text: &str, wrap_width_cols: Option<usize>) -> usize {
    let Some(max_cols) = wrap_width_cols else {
        return count_lines(text);
    };
    wrapped_line_count(text, max_cols)
}

/// Number of terminal rows `text` occupies when hard-wrapped at `max_cols`
/// display columns (wide characters count as their display width).
///
/// Explicit newlines always start a new row, an empty text is one row, and
/// `max_cols == 0` disables wrapping. Apps that size regions to wrapped text
/// (chat transcripts, logs) should use this rather than re-deriving it.
pub fn wrapped_line_count(text: &str, max_cols: usize) -> usize {
    if max_cols == 0 {
        return count_lines(text);
    }

    let mut count = 0usize;
    for line in text.split('\n') {
        count += line_wrapped_count(line, max_cols);
    }
    count.max(1)
}

fn line_wrapped_count(line: &str, max_cols: usize) -> usize {
    if line.is_empty() {
        return 1;
    }

    let mut lines = 1usize;
    let mut cols = 0usize;
    for ch in line.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if cols > 0 && cols + width > max_cols {
            lines += 1;
            cols = 0;
        }
        cols += width;
        while cols > max_cols {
            lines += 1;
            cols -= max_cols;
        }
    }
    lines
}

fn spans_to_plain_text(spans: &[StyledSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        out.push_str(&span.text);
    }
    out
}

fn append_plain_text_span(spans: &mut Vec<StyledSpan>, chunk: &str) {
    let default_style = Style::new();
    if let Some(last) = spans.last_mut() {
        if last.style == default_style {
            last.text.push_str(chunk);
            return;
        }
    }

    spans.push(StyledSpan {
        text: chunk.to_string(),
        style: default_style,
    });
}

fn spans_to_lines(
    spans: &[StyledSpan],
    base_style: Style,
    uppercase: bool,
    prefix: Option<String>,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();

    if let Some(p) = prefix {
        current_spans.push(Span::raw(p));
    }

    for span in spans {
        let mut merged_style = span.style.clone();
        if base_style.bold {
            merged_style.bold = true;
        }
        if base_style.italic {
            merged_style.italic = true;
        }
        if base_style.underline {
            merged_style.underline = true;
        }
        if base_style.dim {
            merged_style.dim = true;
        }

        let rstyle = proto_style_to_ratatui_style(&merged_style);
        let text = if uppercase {
            span.text.to_uppercase()
        } else {
            span.text.clone()
        };

        for (idx, part) in text.split('\n').enumerate() {
            if idx > 0 {
                lines.push(Line::from(std::mem::take(&mut current_spans)));
            }
            if !part.is_empty() {
                current_spans.push(Span::styled(part.to_string(), rstyle));
            }
        }
    }

    lines.push(Line::from(current_spans));
    lines
}

fn proto_style_to_ratatui_style(style: &Style) -> RStyle {
    let mut out = RStyle::default();

    if let Some(fg) = &style.fg {
        out = out.fg(convert_color(fg));
    }
    if let Some(bg) = &style.bg {
        out = out.bg(convert_color(bg));
    }
    if style.bold {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.italic {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.underline {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    if style.dim {
        out = out.add_modifier(Modifier::DIM);
    }

    out
}

fn convert_color(color: &Color) -> ratatui::style::Color {
    use ratatui::style::Color as RColor;

    match color {
        Color::Named(named) => match named {
            NamedColor::Black => RColor::Black,
            NamedColor::Red => RColor::Red,
            NamedColor::Green => RColor::Green,
            NamedColor::Yellow => RColor::Yellow,
            NamedColor::Blue => RColor::Blue,
            NamedColor::Magenta => RColor::Magenta,
            NamedColor::Cyan => RColor::Cyan,
            NamedColor::White => RColor::White,
            NamedColor::BrightBlack => RColor::DarkGray,
            NamedColor::BrightRed => RColor::LightRed,
            NamedColor::BrightGreen => RColor::LightGreen,
            NamedColor::BrightYellow => RColor::LightYellow,
            NamedColor::BrightBlue => RColor::LightBlue,
            NamedColor::BrightMagenta => RColor::LightMagenta,
            NamedColor::BrightCyan => RColor::LightCyan,
            NamedColor::BrightWhite => RColor::White,
            NamedColor::Foreground | NamedColor::Background => RColor::Reset,
        },
        Color::Rgb(r, g, b) => RColor::Rgb(*r, *g, *b),
        Color::Indexed(idx) => RColor::Indexed(*idx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn collect_native_messages(view: &MarkdownView, index: usize) -> Vec<AppMessage> {
        let mut messages = Vec::new();
        let mut send = |msg: &AppMessage| -> crate::Result<()> {
            messages.push(msg.clone());
            Ok(())
        };
        let mut next_id = 1;
        let _ = view
            .render_child_native(index, &mut send, RegionId::new(0), &mut next_id)
            .expect("render child should succeed");
        messages
    }

    fn render_ansi_text(view: &MarkdownView, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                view.render_ansi(frame, frame.area());
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn empty_string_has_no_children() {
        let view = MarkdownView::new("");
        assert_eq!(view.children_count(), 0);
        assert_eq!(view.constraints().height, Dimension::Cells(1.0));
    }

    #[test]
    fn plain_text_renders_single_paragraph_block() {
        let view = MarkdownView::new("hello world");
        assert_eq!(view.children_count(), 1);

        let messages = collect_native_messages(&view, 0);
        assert!(matches!(messages[1], AppMessage::SetContent { .. }));

        match &messages[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => {
                assert_eq!(rich.spans.len(), 1);
                assert_eq!(rich.spans[0].text, "hello world");
            }
            _ => panic!("expected rich text content"),
        }
    }

    #[test]
    fn wrap_width_cols_increases_native_paragraph_height() {
        let view = MarkdownView::new("abcdefghijkl").wrap_width_cols(5);
        assert_eq!(view.constraints().height, Dimension::Cells(3.0));

        let messages = collect_native_messages(&view, 0);
        match &messages[0] {
            AppMessage::CreateRegion { constraints, .. } => {
                assert_eq!(constraints.height, Dimension::Cells(3.0));
            }
            _ => panic!("expected create region message"),
        }
    }

    #[test]
    fn wrap_width_cols_preserves_unwrapped_code_block_height() {
        let view = MarkdownView::new("```\nabcdefghijkl\n```").wrap_width_cols(5);
        assert_eq!(view.constraints().height, Dimension::Cells(1.0));
    }

    #[test]
    fn h1_heading_is_uppercase_and_bold() {
        let view = MarkdownView::new("# hello heading");
        let messages = collect_native_messages(&view, 0);

        match &messages[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => {
                assert_eq!(rich.spans[0].text, "HELLO HEADING");
                assert!(rich.spans[0].style.bold);
            }
            _ => panic!("expected heading rich text"),
        }
    }

    #[test]
    fn bold_and_italic_spans_are_styled() {
        let view = MarkdownView::new("**bold** and *italic*");
        let messages = collect_native_messages(&view, 0);

        match &messages[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => {
                assert!(rich.spans.iter().any(|s| s.text == "bold" && s.style.bold));
                assert!(rich
                    .spans
                    .iter()
                    .any(|s| s.text == "italic" && s.style.italic));
            }
            _ => panic!("expected rich text for inline styles"),
        }
    }

    #[test]
    fn code_block_uses_dim_style_and_left_border() {
        let view = MarkdownView::new("```rust\nlet x = 1;\n```");
        let messages = collect_native_messages(&view, 0);

        match &messages[0] {
            AppMessage::CreateRegion { style, .. } => {
                assert!(style.dim);
                assert_eq!(style.border.style, BorderType::Thin);
                assert!(style.border.edges.left);
                assert!(!style.border.edges.top);
            }
            _ => panic!("expected create region message"),
        }

        match &messages[1] {
            AppMessage::SetContent {
                content: Content::Text(text),
                ..
            } => {
                assert!(text.text.contains("let x = 1;"));
            }
            _ => panic!("expected text content for code block"),
        }
    }

    #[test]
    fn lists_render_prefixes_and_indentation() {
        let view = MarkdownView::new("- first\n    - nested");
        assert_eq!(view.children_count(), 2);

        let first = collect_native_messages(&view, 0);
        match &first[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => {
                assert_eq!(rich.spans[0].text, "• ");
            }
            _ => panic!("expected rich text for list item"),
        }

        let second = collect_native_messages(&view, 1);
        match &second[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => {
                assert_eq!(rich.spans[0].text, "  • ");
            }
            _ => panic!("expected nested list rich text"),
        }
    }

    #[test]
    fn ordered_list_prefixes_are_numbered() {
        let view = MarkdownView::new("1. first\n2. second");
        assert_eq!(view.children_count(), 2);

        let first = collect_native_messages(&view, 0);
        match &first[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => assert_eq!(rich.spans[0].text, "1. "),
            _ => panic!("expected ordered list rich text"),
        }

        let second = collect_native_messages(&view, 1);
        match &second[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => assert_eq!(rich.spans[0].text, "2. "),
            _ => panic!("expected ordered list rich text"),
        }
    }

    #[test]
    fn mixed_content_counts_multiple_blocks() {
        let view = MarkdownView::new("# Title\n\nParagraph\n\n---\n\n> quote\n\n```\ncode\n```");
        assert_eq!(view.children_count(), 5);
        assert_eq!(view.constraints().height, Dimension::Cells(5.0));
    }

    #[test]
    fn streaming_empty_renders_cursor() {
        let view = MarkdownView::new("").streaming(true);
        assert_eq!(view.children_count(), 1);

        let messages = collect_native_messages(&view, 0);
        match &messages[1] {
            AppMessage::SetContent {
                content: Content::Text(tc),
                ..
            } => assert_eq!(tc.text, "|"),
            _ => panic!("expected text content with cursor"),
        }
    }

    #[test]
    fn streaming_appends_cursor_to_last_block() {
        let view = MarkdownView::new("hello").streaming(true);
        assert_eq!(view.children_count(), 1);

        let messages = collect_native_messages(&view, 0);
        match &messages[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => {
                let last = rich.spans.last().expect("should have spans");
                assert_eq!(last.text, "|");
            }
            _ => panic!("expected rich text with cursor"),
        }
    }

    #[test]
    fn streaming_false_has_no_cursor() {
        let view = MarkdownView::new("hello").streaming(false);
        let messages = collect_native_messages(&view, 0);
        match &messages[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => {
                assert!(rich.spans.iter().all(|s| s.text != "|"));
            }
            _ => panic!("expected rich text without cursor"),
        }
    }

    #[test]
    fn append_chunk_updates_source_and_blocks() {
        let mut view = MarkdownView::new("hello");
        view.append_chunk(" world");

        assert_eq!(view.source(), "hello world");
        assert_eq!(view.children_count(), 1);

        let messages = collect_native_messages(&view, 0);
        match &messages[1] {
            AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } => {
                assert_eq!(rich.spans[0].text, "hello world");
            }
            _ => panic!("expected rich text content"),
        }
    }

    #[test]
    fn append_chunk_incrementally_extends_plain_paragraph() {
        let mut view = MarkdownView::new("hello");

        assert!(view.try_append_chunk_incrementally(" world", false));

        match view.blocks.last().expect("paragraph should exist") {
            Block::Paragraph { spans } => {
                assert_eq!(spans_to_plain_text(spans), "hello world");
            }
            other => panic!("expected paragraph, got {:?}", other),
        }
    }

    #[test]
    fn append_chunk_incrementally_extends_open_code_block() {
        let mut view = MarkdownView::new("```rust\nlet x = 1;");

        assert!(view.try_append_chunk_incrementally("\nlet y = 2;", true));

        match view.blocks.last().expect("code block should exist") {
            Block::CodeBlock { text } => {
                assert!(text.contains("let x = 1;"));
                assert!(text.contains("let y = 2;"));
            }
            other => panic!("expected code block, got {:?}", other),
        }
    }

    #[test]
    fn append_chunk_reparses_when_closing_code_fence_arrives() {
        let mut view = MarkdownView::new("```rust\nlet x = 1;");
        view.append_chunk("\n```");

        assert_eq!(view.children_count(), 1);
        match view.blocks.last().expect("code block should exist") {
            Block::CodeBlock { text } => {
                assert_eq!(text.trim(), "let x = 1;");
                assert!(!text.contains("```"));
            }
            other => panic!("expected code block, got {:?}", other),
        }
    }

    #[test]
    fn append_chunk_reparses_split_inline_markdown_boundary() {
        let mut view = MarkdownView::new("**bo");
        view.append_chunk("ld**");

        match view.blocks.last().expect("paragraph should exist") {
            Block::Paragraph { spans } => {
                assert!(spans
                    .iter()
                    .any(|span| span.text == "bold" && span.style.bold));
            }
            other => panic!("expected paragraph, got {:?}", other),
        }
    }

    #[test]
    fn with_appended_returns_updated_view() {
        let view = MarkdownView::new("first").with_appended(" second");

        assert_eq!(view.source(), "first second");
        assert_eq!(view.children_count(), 1);
    }

    #[test]
    fn append_chunk_updates_ansi_fallback_render() {
        let mut view = MarkdownView::new("hello");
        view.append_chunk(" world");

        let ansi = render_ansi_text(&view, 40, 4);
        assert!(ansi.contains("hello world"));
    }
}
