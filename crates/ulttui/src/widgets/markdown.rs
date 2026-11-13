use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style as RStyle};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use super::View;
use crate::backend::native_diff::record_unspoken;
use ultty_protocol::{
    AppMessage, BorderEdges, BorderStyle, BorderType, Color, Constraints, Content, Dimension,
    Display, Live, NamedColor, RegionId, RichText, Role, Semantics, Style, TextContent, Toggled,
};

const STREAMING_CURSOR: &str = "|";

#[derive(Debug, Clone)]
pub struct MarkdownView {
    id: Option<String>,
    source: String,
    blocks: Vec<Block>,
    /// Where parsing starts again when text is appended; `None` when the
    /// source defines link references or footnotes, which reach back to
    /// earlier blocks, so all of it is parsed again.
    reparse_from: Option<ReparsePoint>,
    streaming: bool,
    wrap_width_cols: Option<usize>,
    accessible: super::Accessible,
}

/// The start of the line the last top-level block of a source starts on,
/// or the last item of a top-level list, and how many blocks come before
/// it. Markdown appended after it cannot change the blocks before it (but
/// for reference definitions), so parsing from there gives the rest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ReparsePoint {
    offset: usize,
    blocks: usize,
    /// When it is a list item: the list it goes on, whose numbering the
    /// item continues.
    list: Option<ListState>,
}

#[derive(Debug, Clone, PartialEq)]
struct StyledSpan {
    text: String,
    style: Style,
}

#[derive(Debug, Clone, PartialEq)]
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
        /// What is drawn before the text: the bullet or number, or for a
        /// task the checkbox.
        prefix: String,
        spans: Vec<StyledSpan>,
        /// A task list item's checkbox: whether it is checked.
        task: Option<bool>,
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
        ordered: bool,
        quoted: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineStyleMarker {
    Strong,
    Emphasis,
    Link,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListState {
    ordered: bool,
    next_number: u64,
}

impl MarkdownView {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new(source: impl Into<String>) -> Self {
        let source = source.into();
        let (blocks, reparse_from) = parse(&source, None);
        Self {
            id: None,
            source,
            blocks,
            reparse_from,
            streaming: false,
            wrap_width_cols: None,
            accessible: super::Accessible::default(),
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Append `chunk` to the source. Only the last top-level block, the
    /// one text can still arrive in, is parsed again, so the cost is that
    /// block's size and the result is what parsing the whole source gives.
    pub fn append_chunk(&mut self, chunk: impl AsRef<str>) {
        let chunk = chunk.as_ref();
        if chunk.is_empty() {
            return;
        }
        self.source.push_str(chunk);

        if let Some(from) = self.reparse_from {
            let (tail, tail_from) = parse(&self.source[from.offset..], from.list);
            if let Some(tail_from) = tail_from {
                self.blocks.truncate(from.blocks);
                self.blocks.extend(tail);
                self.reparse_from = Some(ReparsePoint {
                    offset: from.offset + tail_from.offset,
                    blocks: from.blocks + tail_from.blocks,
                    list: tail_from.list,
                });
                return;
            }
        }
        (self.blocks, self.reparse_from) = parse(&self.source, None);
    }

    pub fn with_appended(mut self, chunk: impl AsRef<str>) -> Self {
        self.append_chunk(chunk);
        self
    }

    /// Enable streaming mode.
    ///
    /// When `true`, content is still arriving: the view is a busy, polite
    /// live log for assistive technology, and natively it is one region
    /// whose text grows by `AppendContent`, so a screen reader announces
    /// what arrives. In ANSI mode a cursor ends the output. Toggle off to
    /// finalize.
    pub fn streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    /// The accessible name, such as "Reply".
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.accessible.label = Some(label.into());
        self
    }

    /// How text appended while streaming is announced: `Polite` by
    /// default, `Assertive` to interrupt, `Off` for silence.
    pub fn live(mut self, live: Live) -> Self {
        self.accessible.live = Some(live);
        self
    }

    /// Set the width, in terminal cells, used to estimate native wrapped height.
    pub fn wrap_width_cols(mut self, cols: usize) -> Self {
        self.wrap_width_cols = Some(cols.max(1));
        self
    }

    /// Prepare styled, cell-wrapped rows for a virtualized transcript.
    ///
    /// Call when the source or width changes, then retain the rows and render
    /// only the visible range. Code and quotes wrap too, so narrow terminals
    /// can read their complete contents. Wrapping preserves grapheme clusters.
    pub fn wrapped_rows(&self, width: usize) -> Vec<RichText> {
        let width = width.max(1);
        let mut rows = Vec::new();
        for block in &self.blocks {
            let mut rich = match block_content(block) {
                Content::RichText(rich) => rich,
                Content::Text(text) => RichText::new().span(text.text, Style::default()),
                _ => continue,
            };
            if matches!(block, Block::HorizontalRule) {
                rich = RichText::new().span("─".repeat(width), Style::default());
            }
            if matches!(block, Block::CodeBlock { .. }) {
                rich.spans.insert(
                    0,
                    ultty_protocol::Span {
                        text: "│ ".into(),
                        style: Style::default(),
                    },
                );
            }
            let dim = matches!(block, Block::CodeBlock { .. } | Block::BlockQuote { .. });
            let mut row = RichText::new();
            let mut column = 0;
            for span in rich.spans {
                let mut style = span.style;
                style.dim |= dim;
                for cluster in span.text.graphemes(true) {
                    if cluster == "\n" || cluster == "\r\n" {
                        rows.push(std::mem::replace(&mut row, RichText::new()));
                        column = 0;
                        continue;
                    }
                    let expanded;
                    let cluster = if cluster == "\t" {
                        expanded = " ".repeat(8 - column % 8);
                        expanded.as_str()
                    } else {
                        cluster
                    };
                    // Expanded tabs may span several narrow rows.
                    for grapheme in cluster.graphemes(true) {
                        let cells = grapheme.width();
                        if column > 0 && column + cells > width {
                            rows.push(std::mem::replace(&mut row, RichText::new()));
                            column = 0;
                        }
                        if let Some(last) = row.spans.last_mut().filter(|last| last.style == style)
                        {
                            last.text.push_str(grapheme);
                        } else {
                            row.spans.push(ultty_protocol::Span {
                                text: grapheme.into(),
                                style: style.clone(),
                            });
                        }
                        column += cells;
                    }
                }
            }
            rows.push(row);
        }
        if rows.is_empty() {
            rows.push(RichText::new());
        }
        rows
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
                mut prefix,
                mut spans,
                ordered,
                quoted,
            } => {
                if quoted {
                    let mut content = " ".repeat(indent);
                    content.push_str(&prefix);
                    content.push_str(&spans_to_plain_text(&spans));
                    Block::BlockQuote { text: content }
                } else {
                    let task = take_task_marker(&mut spans);
                    if let Some(checked) = task {
                        // The checkbox in place of a bullet, after a number.
                        if !ordered {
                            prefix.clear();
                        }
                        prefix.push_str(if checked { "[x] " } else { "[ ] " });
                    }
                    Block::ListItem {
                        indent,
                        prefix,
                        spans,
                        task,
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

/// The blocks of `source`, and where to parse again from when text is
/// appended to it (`None` when it defines link references or footnotes).
/// With `list`, `source` starts with an item that goes on that list.
fn parse(source: &str, list: Option<ListState>) -> (Vec<Block>, Option<ReparsePoint>) {
    // Without metadata blocks: a reply that starts with a rule would have
    // the text up to the next one hidden as front matter. Without
    // definition lists: pulldown-cmark 0.12 panics on one in a block quote
    // (`> _\n> : \n\n`); they are shown as the text they are. Without
    // heading attributes: it panics on a heading a task marker was carried
    // into (`1. [x] # x\n[r]: http://r\n  \t\n===\n`), and `{#id}` is
    // shown as written.
    let options = Options::all()
        - Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        - Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
        - Options::ENABLE_DEFINITION_LIST
        - Options::ENABLE_HEADING_ATTRIBUTES
        - Options::ENABLE_TASKLISTS;
    if may_define_footnote(source) {
        // A footnote definition reaches back to the references before it
        // like a link reference, and pulldown-cmark 0.12's offsets around
        // one can be out of order (its debug assertion fails), so they are
        // not asked for.
        let mut builder = BlockBuilder {
            continue_list: list,
            ..BlockBuilder::default()
        };
        for event in Parser::new_ext(source, options) {
            builder.event(event);
        }
        return (builder.finish(), None);
    }
    let parser = Parser::new_ext(source, options).into_offset_iter();
    let defines = parser.reference_definitions().iter().next().is_some();
    let mut builder = BlockBuilder {
        continue_list: list,
        ..BlockBuilder::default()
    };
    // From the start, `source` goes on `list` again.
    let mut reparse_from = ReparsePoint {
        list,
        ..ReparsePoint::default()
    };
    let mut depth = 0usize;
    // A block after a list can still turn into one of its items (`1`
    // becoming `1. x`).
    let mut after_list = false;
    let mut top_level_items = 0usize;
    for (event, range) in parser {
        if depth == 0 && builder.current.is_none() && !after_list {
            // Only a block after a blank line: one that interrupts the
            // paragraph before it may parse differently on its own.
            let line_start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
            let after_blank_line = line_start == 0
                || source[..line_start - 1]
                    .rsplit('\n')
                    .next()
                    .is_some_and(|line| line.trim_matches([' ', '\t', '\r']).is_empty());
            if after_blank_line
                && source[line_start..range.start]
                    .chars()
                    .all(|c| c == ' ' || c == '\t')
            {
                reparse_from = ReparsePoint {
                    offset: line_start,
                    blocks: builder.blocks.len(),
                    // The start goes on `list` still.
                    list: list.filter(|_| line_start == 0),
                };
            }
        }
        // An item of a top-level list after its first, once the one before
        // it is done: the items before it are finished, and its list's
        // numbering goes on. Not the first, which may have interrupted a
        // paragraph.
        let top_level_item = depth == 1 && matches!(event, Event::Start(Tag::Item));
        if top_level_item
            && top_level_items > 0
            && builder.current.is_none()
            && item_line_is_settled(source, range.start)
        {
            let line_start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
            if source[line_start..range.start]
                .chars()
                .all(|c| c == ' ' || c == '\t')
            {
                reparse_from = ReparsePoint {
                    offset: line_start,
                    blocks: builder.blocks.len(),
                    list: builder.list_stack.last().copied(),
                };
            }
        }
        if depth == 0 {
            after_list = matches!(event, Event::Start(Tag::List(_) | Tag::DefinitionList));
            top_level_items = 0;
        }
        if top_level_item {
            top_level_items += 1;
        }
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        builder.event(event);
    }
    let blocks = builder.finish();
    (blocks, (!defines).then_some(reparse_from))
}

/// Whether the first line of the list item starting at byte `start` of
/// `source` (at its marker) keeps it a list item whatever follows: it has
/// text after the marker (`1.` may become `1.5`), and not only `-`, `*`
/// and `_` (`* *` may become the rule `* * *`).
fn item_line_is_settled(source: &str, start: usize) -> bool {
    let line = source[start..].split(['\n', '\r']).next().unwrap_or("");
    let after_marker = line
        .trim_start_matches([' ', '\t'])
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .get(1..)
        .unwrap_or("");
    !after_marker.trim_matches([' ', '\t', '\r']).is_empty()
        && line
            .chars()
            .any(|c| !matches!(c, ' ' | '\t' | '\r' | '-' | '*' | '_'))
}

/// Whether `source` may define a footnote: it has a `[^label]:`. It may
/// say so for text that is not a definition (`[^a-z]:` in code), which
/// only costs parsing all of it again; a `[^` without the `]:` (a regex,
/// a reference) does not.
fn may_define_footnote(source: &str) -> bool {
    source.match_indices("[^").any(|(at, _)| {
        let mut chars = source[at + 2..].chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                ']' => return chars.as_str().starts_with(':'),
                _ => {}
            }
        }
        false
    })
}

/// How long a task marker is, drawn (`[x] `) or written.
const TASK_MARKER_LEN: usize = 4;

/// Take a task marker (`[ ] `, `[x] `, `[X] `) off the start of a list
/// item's text, and say whether it is checked. Only plain text makes one,
/// with more text after it: `[x](url)` is a link, and `[ ]` alone stays
/// as written.
///
/// pulldown-cmark finds task markers itself, but 0.12 gives the marker of
/// an item that starts with another block (`- [ ] # x`) to whichever
/// paragraph comes next, and panics on some of those; so its task lists
/// are off and the item's own text is looked at instead.
fn take_task_marker(spans: &mut Vec<StyledSpan>) -> Option<bool> {
    let plain = Style::new();
    let lead: String = spans
        .iter()
        .take_while(|span| span.style == plain)
        .flat_map(|span| span.text.chars())
        .take(TASK_MARKER_LEN)
        .collect();
    let checked = match lead.as_bytes() {
        [b'[', b' ' | b'\t', b']', b' ' | b'\t'] => false,
        [b'[', b'x' | b'X', b']', b' ' | b'\t'] => true,
        _ => return None,
    };
    let text_len: usize = spans.iter().map(|span| span.text.len()).sum();
    if text_len == TASK_MARKER_LEN {
        return None;
    }
    let mut rest = TASK_MARKER_LEN;
    while rest > 0 {
        let first = &mut spans[0];
        if first.text.len() <= rest {
            rest -= first.text.len();
            spans.remove(0);
        } else {
            first.text.drain(..rest);
            rest = 0;
        }
    }
    Some(checked)
}

/// Turns parser events into blocks.
#[derive(Default)]
struct BlockBuilder {
    blocks: Vec<Block>,
    current: Option<CurrentBlock>,
    inline_style_stack: Vec<InlineStyleMarker>,
    list_stack: Vec<ListState>,
    quote_depth: usize,
    table_depth: usize,
    /// The list the first top-level list goes on, when parsing starts at
    /// one of its items.
    continue_list: Option<ListState>,
}

impl BlockBuilder {
    fn finish_current(&mut self) {
        if let Some(done) = self.current.take() {
            self.blocks.push(MarkdownView::finish_block(done));
        }
    }

    fn push_text(&mut self, text: &str, inline_code: bool) {
        MarkdownView::push_text_span(
            &mut self.current,
            &self.inline_style_stack,
            text,
            inline_code,
        );
    }

    fn event(&mut self, event: Event<'_>) {
        // Only the list the source starts with goes on another.
        if !matches!(event, Event::Start(Tag::List(_))) {
            self.continue_list = None;
        }
        let quoted = self.quote_depth > 0;
        match event {
            Event::Start(tag) => match tag {
                // A loose list item's first paragraph is its text.
                Tag::Paragraph if self.item_is_empty() => {}
                Tag::Paragraph => {
                    self.finish_current();
                    self.current = Some(CurrentBlock::Paragraph {
                        spans: Vec::new(),
                        quoted,
                    });
                }
                Tag::Heading { level, .. } => {
                    self.finish_current();
                    self.current = Some(CurrentBlock::Heading {
                        level,
                        spans: Vec::new(),
                        quoted,
                    });
                }
                Tag::CodeBlock(_) => {
                    self.finish_current();
                    self.current = Some(CurrentBlock::CodeBlock {
                        text: String::new(),
                        quoted,
                    });
                }
                Tag::List(start) => {
                    let (ordered, first) = match start {
                        Some(n) => (true, n),
                        None => (false, 1),
                    };
                    let list = self.continue_list.take().unwrap_or(ListState {
                        ordered,
                        next_number: first,
                    });
                    self.list_stack.push(list);
                }
                Tag::Item => {
                    self.finish_current();
                    let depth = self.list_stack.len().saturating_sub(1);
                    let indent = depth * 2;
                    let prefix = match self.list_stack.last_mut() {
                        Some(last) if last.ordered => {
                            let p = format!("{}. ", last.next_number);
                            last.next_number += 1;
                            p
                        }
                        _ => "• ".to_string(),
                    };
                    let ordered = self.list_stack.last().is_some_and(|list| list.ordered);
                    self.current = Some(CurrentBlock::ListItem {
                        indent,
                        prefix,
                        spans: Vec::new(),
                        ordered,
                        quoted,
                    });
                }
                Tag::BlockQuote(_) => self.quote_depth += 1,
                Tag::Table(_) => self.table_depth += 1,
                Tag::Strong => self.inline_style_stack.push(InlineStyleMarker::Strong),
                Tag::Emphasis => self.inline_style_stack.push(InlineStyleMarker::Emphasis),
                Tag::Link { .. } => self.inline_style_stack.push(InlineStyleMarker::Link),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item => {
                    self.finish_current();
                }
                TagEnd::List(_) => {
                    self.list_stack.pop();
                }
                TagEnd::Table => self.table_depth = self.table_depth.saturating_sub(1),
                TagEnd::BlockQuote(_) => {
                    self.quote_depth = self.quote_depth.saturating_sub(1);
                }
                TagEnd::Strong => self.pop_inline_style(InlineStyleMarker::Strong),
                TagEnd::Emphasis => self.pop_inline_style(InlineStyleMarker::Emphasis),
                TagEnd::Link => self.pop_inline_style(InlineStyleMarker::Link),
                _ => {}
            },
            Event::Html(text) => self.push_text(text.as_ref(), false),
            Event::Text(text) | Event::InlineHtml(text) => {
                self.begin_tight_paragraph(quoted);
                self.push_text(text.as_ref(), false);
            }
            Event::Code(text) => {
                self.begin_tight_paragraph(quoted);
                self.push_text(text.as_ref(), true);
            }
            Event::SoftBreak | Event::HardBreak => {
                self.begin_tight_paragraph(quoted);
                self.push_text("\n", false);
            }
            Event::Rule => self.blocks.push(Block::HorizontalRule),
            _ => {}
        }
    }

    /// A tight list shows no paragraph events: text in an item after
    /// another of its blocks (`- ```` ... ```` then text) begins a
    /// paragraph, as it does in a loose list, so that the blocks do not
    /// depend on whether a later blank line makes the list loose.
    fn begin_tight_paragraph(&mut self, quoted: bool) {
        if self.current.is_none() && !self.list_stack.is_empty() && self.table_depth == 0 {
            self.current = Some(CurrentBlock::Paragraph {
                spans: Vec::new(),
                quoted,
            });
        }
    }

    /// Whether the block being built is a list item with no text yet.
    fn item_is_empty(&self) -> bool {
        matches!(&self.current, Some(CurrentBlock::ListItem { spans, .. }) if spans.is_empty())
    }

    fn pop_inline_style(&mut self, marker: InlineStyleMarker) {
        if let Some(pos) = self.inline_style_stack.iter().rposition(|m| *m == marker) {
            self.inline_style_stack.remove(pos);
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.finish_current();
        self.blocks
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
                ..
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
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(self.line_count() as f32),
            flex_shrink: 1.0,
            ..Default::default()
        }
    }

    /// While streaming, natively, the whole document is this region's
    /// own text, so that what arrives is appended to it (and announced,
    /// the region being a live log); finished, each block is a region of
    /// its own.
    fn content(&self) -> Content {
        Content::None
    }

    /// A streaming view is a busy, polite live log (ADR 0021 §4); a
    /// finished one leaves its blocks to the terminal.
    fn semantics(&self) -> Semantics {
        let semantics = if self.streaming {
            Semantics {
                role: Role::Log,
                live: Live::Polite,
                busy: true,
                ..Semantics::default()
            }
        } else {
            Semantics::default()
        };
        self.accessible.apply(semantics)
    }

    fn children_count(&self) -> usize {
        self.blocks.len()
    }

    /// One region per block. While streaming, the last block, the one
    /// still arriving, is live itself: what it grows by reaches it as
    /// `AppendContent` and is announced, and a block that is finished
    /// stops being live, so each region stays one block's size.
    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        let Some(block) = self.blocks.get(index) else {
            return Ok(None);
        };

        let region_id = RegionId::new(*next_id);
        *next_id += 1;

        let style = block_style(block);

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

        let arriving = self.streaming && index + 1 == self.blocks.len();
        // An item still arriving that may yet turn out a task is drawn
        // empty until that is known: its bullet would be said, or go.
        let content = match block {
            Block::ListItem {
                spans, task: None, ..
            } if arriving && may_become_task(spans) => Content::RichText(RichText::new()),
            _ => block_content(block),
        };
        let live = if arriving {
            self.accessible.live.unwrap_or(Live::Polite)
        } else {
            Live::Off
        };
        if let Block::ListItem {
            indent,
            prefix,
            task: Some(_),
            ..
        } = block
        {
            // The checkbox drawn before the text is the item's state; a
            // bullet's indent goes with it, a number is said.
            let end = indent + prefix.len();
            let start = if prefix.len() > TASK_MARKER_LEN {
                end - TASK_MARKER_LEN
            } else {
                0
            };
            record_unspoken(region_id, start..end);
        }
        let block_semantics = match block {
            // A read-only checkbox named by its text, its state `toggled`.
            // Only once it has stopped arriving: a live node is announced
            // again whenever its name changes, and a checkbox not named is
            // named by its text, which grows.
            Block::ListItem {
                prefix,
                spans,
                task: Some(checked),
                ..
            } if !arriving => Semantics {
                role: Role::Checkbox,
                label: Some(format!(
                    "{}{}",
                    &prefix[..prefix.len() - TASK_MARKER_LEN],
                    spans_to_plain_text(spans).trim()
                )),
                toggled: if *checked {
                    Toggled::True
                } else {
                    Toggled::False
                },
                read_only: true,
                ..Semantics::default()
            },
            _ => Semantics {
                live,
                ..Semantics::default()
            },
        };
        super::send_semantics(send_message, region_id, block_semantics, &content)?;
        // Prose may still hold an inline opener that has not turned into
        // styling yet; code is shown as it comes.
        let prose = matches!(
            block,
            Block::Paragraph { .. } | Block::Heading { .. } | Block::ListItem { .. }
        );
        if arriving && prose {
            crate::backend::native_diff::record_arriving_prose(region_id);
        }
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
                    ..
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

/// Whether a list item's text so far is empty or could still be the start
/// of a task marker (`[`, `[x`, `[ ]`).
fn may_become_task(spans: &[StyledSpan]) -> bool {
    let text = spans_to_plain_text(spans);
    text.is_empty() || (text.starts_with('[') && text.len() <= 3)
}

/// A block region's style: code and quotes dim, code with a left rule.
fn block_style(block: &Block) -> Style {
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
    style
}

/// What a block region shows.
fn block_content(block: &Block) -> Content {
    match block {
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
        // Without the fence's closing newline, which would be one more row
        // than the block's height.
        Block::CodeBlock { text } => {
            Content::Text(TextContent::new(text.strip_suffix('\n').unwrap_or(text)))
        }
        Block::ListItem {
            indent,
            prefix,
            spans,
            ..
        } => {
            let mut rich =
                RichText::new().span(format!("{}{}", " ".repeat(*indent), prefix), Style::new());
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
    #[test]
    fn virtual_rows_preserve_unicode_styles_and_code_beyond_canvas_limit() {
        let rows = super::MarkdownView::new("**a界e\u{301}**\n\n```\nlong-code-value\n```")
            .wrapped_rows(4);
        let text: String = rows
            .iter()
            .flat_map(|row| &row.spans)
            .map(|span| span.text.as_str())
            .collect();
        assert!(text.contains("a界e\u{301}"));
        assert!(text.contains("long-code-value"));
        assert!(rows[0].spans[0].style.bold);
        for row in &rows {
            let width: usize = row
                .spans
                .iter()
                .map(|span| unicode_width::UnicodeWidthStr::width(span.text.as_str()))
                .sum();
            assert!(width <= 4);
        }
        let source = format!("```\n{}\nlast-row\n```", "row\n".repeat(5000));
        let rows = super::MarkdownView::new(source).wrapped_rows(20);
        assert!(rows.len() > 4096);
        assert!(rows
            .last()
            .unwrap()
            .spans
            .iter()
            .any(|span| span.text.contains("last-row")));
    }

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
    fn streaming_is_a_busy_polite_log_and_finished_is_left_to_the_terminal() {
        let semantics = MarkdownView::new("hi").streaming(true).semantics();
        assert_eq!(semantics.role, Role::Log);
        assert_eq!(semantics.live, Live::Polite);
        assert!(semantics.busy);
        assert_eq!(MarkdownView::new("hi").semantics(), Semantics::default());

        let quiet = MarkdownView::new("hi")
            .streaming(true)
            .live(Live::Off)
            .label("Reply");
        assert_eq!(quiet.semantics().live, Live::Off);
        assert_eq!(quiet.semantics().label.as_deref(), Some("Reply"));
    }

    use crate::backend::native_diff::{
        begin_frame_notes, optimize_frame_messages, take_frame_notes, FrameCapabilities,
        FrameNotes, RetainedTree,
    };
    use crate::widgets::render_view_messages;
    use std::collections::HashMap;
    use ultty_protocol::Toggled;

    const ALL: FrameCapabilities = FrameCapabilities {
        append_content: true,
        semantics: true,
        input_cursor: true,
    };

    /// The view's native frame and what the walk noted, as the backend
    /// collects them.
    fn walk(view: &MarkdownView) -> (Vec<AppMessage>, FrameNotes) {
        begin_frame_notes();
        let frame = render_view_messages(view);
        (frame.unwrap(), take_frame_notes())
    }

    /// Everything a stream of `chunks` announces, ending the stream after
    /// the last one in the same frame.
    fn spoken(chunks: &[&str]) -> String {
        let (view, tree, frames) = stream(chunks);
        let (frame, notes) = walk(&view.streaming(false));
        let (last, _) = optimize_frame_messages(&tree, frame, &notes, ALL);
        frames
            .iter()
            .chain(std::iter::once(&last))
            .flat_map(|frame| appends(frame))
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn a_long_link_and_a_long_bold_phrase_are_spoken_once_whole() {
        let link = "See [the docs](https://github.com/example/project/blob/main/docs/architecture/overview.md";
        assert_eq!(
            spoken(&["", link, ") **for** the details", " of it all."]),
            "See |the docs for the details| of it all."
        );
        let phrase =
            "this is a long bold phrase that goes on for well over eighty bytes before it ends";
        assert_eq!(
            spoken(&["", "Start **", phrase, "** and after."]),
            format!("Start |{phrase} and after.")
        );
        // A link that never resolves on screen is said at the end.
        assert_eq!(
            spoken(&["", "Go [here", "](http://x"]),
            "Go |[here](http://x"
        );
    }

    #[test]
    fn a_link_or_setext_heading_streamed_in_small_chunks_is_spoken_as_shown() {
        // Neither the address nor the brackets.
        assert_eq!(
            spoken(&["", "See [te", "xt](ht", "tp://x) now"]),
            "See |text now"
        );
        assert_eq!(
            spoken(&["", "See [te", "xt](ht", "tp://x \"ti", "tle\") now"]),
            "See |text now"
        );
        // The underline is not said, nor the line again in capitals.
        assert_eq!(spoken(&["", "Title", "\n", "====="]), "Title");
        assert_eq!(spoken(&["", "Title", "\n", "-", "--"]), "Title");
    }

    #[test]
    fn code_is_not_held_back() {
        let said = spoken(&["", "```\n", "let a = [1, 2];\n", "let b = `x`;\n"]);
        assert_eq!(said.replace('|', ""), "let a = [1, 2];\nlet b = `x`;");
        assert!(said.starts_with("let a = [1, 2];|"), "{said:?}");
    }

    /// Stream `chunks` into a view, one frame each, through the backend
    /// diff; the view, the retained tree and each frame's messages.
    fn stream(chunks: &[&str]) -> (MarkdownView, RetainedTree, Vec<Vec<AppMessage>>) {
        let mut tree = RetainedTree::new();
        let mut frames = Vec::new();
        let mut view = MarkdownView::new("").streaming(true);
        for chunk in chunks {
            view.append_chunk(chunk);
            let (frame, notes) = walk(&view);
            let (outgoing, next) = optimize_frame_messages(&tree, frame, &notes, ALL);
            tree = next;
            frames.push(outgoing);
        }
        (view, tree, frames)
    }

    fn appends(messages: &[AppMessage]) -> Vec<String> {
        messages
            .iter()
            .filter_map(|m| match m {
                AppMessage::AppendContent { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn streaming_natively_is_a_region_per_block_and_only_the_last_is_live() {
        let view = MarkdownView::new("# Title\n\nSome **bold** text\n\n```\ncode\n```\n\nafter")
            .streaming(true);
        assert_eq!(view.content(), Content::None);
        assert_eq!(view.children_count(), 4);
        let messages = render_view_messages(&view).unwrap();
        let live: Vec<RegionId> = messages
            .iter()
            .filter_map(|m| match m {
                AppMessage::SetSemantics { id, semantics }
                    if semantics.live == Live::Polite && semantics.role == Role::Auto =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            live,
            vec![RegionId::new(5)],
            "only the block still arriving"
        );

        // Each block region's text fits the rows it is given.
        let mut heights = HashMap::new();
        for message in &messages {
            match message {
                AppMessage::CreateRegion {
                    id, constraints, ..
                } => {
                    heights.insert(*id, constraints.height);
                }
                AppMessage::SetContent { id, content } => {
                    let text = match content {
                        Content::Text(text) => text.text.clone(),
                        Content::RichText(rich) => {
                            rich.spans.iter().map(|s| s.text.as_str()).collect()
                        }
                        _ => continue,
                    };
                    let rows = text.split('\n').count() as f32;
                    assert!(
                        Dimension::Cells(rows) == heights[id]
                            || matches!(heights[id], Dimension::Cells(h) if rows <= h),
                        "{text:?} in {:?}",
                        heights[id]
                    );
                    assert!(!text.contains(STREAMING_CURSOR));
                }
                _ => {}
            }
        }
    }

    #[test]
    fn streamed_text_is_appended_to_the_block_arriving_and_spoken_once() {
        let (mut view, tree, frames) = stream(&[
            "",
            "Hello",
            " world.\n\nA ",
            "**bo",
            "ld** move",
            "\n\n- item",
        ]);
        assert!(frames[0].iter().any(|m| matches!(
            m,
            AppMessage::SetSemantics { semantics, .. }
                if semantics.role == Role::Log && semantics.live == Live::Polite && semantics.busy
        )));
        let spoken: Vec<String> = frames.iter().flat_map(|frame| appends(frame)).collect();
        assert_eq!(
            spoken.concat(),
            "Hello world.A bold move• item",
            "{spoken:?}"
        );
        assert!(spoken.iter().all(|text| !text.contains('*')), "{spoken:?}");

        // The last chunk and the end of the stream in one frame: the rest
        // is still appended, and nothing is created or removed.
        view.append_chunk(" done.");
        let finished = view.streaming(false);
        let (frame, notes) = walk(&finished);
        let (outgoing, _) = optimize_frame_messages(&tree, frame, &notes, ALL);
        assert_eq!(
            appends(&outgoing),
            vec![" done.".to_string()],
            "{outgoing:?}"
        );
        assert!(!outgoing.iter().any(|m| matches!(
            m,
            AppMessage::CreateRegion { .. } | AppMessage::DeleteRegion { .. }
        )));
    }

    #[test]
    fn a_long_streamed_document_stays_within_codec_limits_and_costs_its_growth() {
        use ultty_protocol::{Decode, Encode};

        let items: Vec<String> = (0..3_500)
            .map(|n| format!("- item **{n}** of `many`\n"))
            .collect();
        let chunks: Vec<String> = items.chunks(100).map(|chunk| chunk.concat()).collect();
        let chunk_refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
        let (mut view, tree, frames) = stream(&chunk_refs);
        for message in frames.iter().flatten() {
            if let AppMessage::SetContent {
                content: Content::RichText(rich),
                ..
            } = message
            {
                assert!(rich.spans.len() < 100, "one block per region");
            }
            let bytes = message.encode_to_bytes();
            AppMessage::decode(&mut &bytes[..]).expect("the terminal decodes every message");
        }

        // One more item on a 3,500-item document costs that item.
        view.append_chunk("- one more\n");
        let (frame, notes) = walk(&view);
        let (outgoing, _) = optimize_frame_messages(&tree, frame, &notes, ALL);
        let bytes: usize = outgoing.iter().map(|m| m.encode_to_bytes().len()).sum();
        assert!(bytes < 1024, "{bytes} bytes for one item: {outgoing:?}");
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
    fn append_chunk_extends_a_plain_paragraph() {
        let mut view = MarkdownView::new("hello");
        view.append_chunk(" world");

        match view.blocks.last().expect("paragraph should exist") {
            Block::Paragraph { spans } => {
                assert_eq!(spans_to_plain_text(spans), "hello world");
            }
            other => panic!("expected paragraph, got {:?}", other),
        }
    }

    #[test]
    fn append_chunk_extends_an_open_code_block() {
        let mut view = MarkdownView::new("```rust\nlet x = 1;");
        view.append_chunk("\nlet y = 2;");

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

    /// A view streamed `chunks`, one `append_chunk` each.
    fn streamed(chunks: &[&str]) -> MarkdownView {
        let mut view = MarkdownView::new("");
        for chunk in chunks {
            view.append_chunk(chunk);
        }
        view
    }

    #[test]
    fn a_link_streamed_in_small_chunks_resolves() {
        let view = streamed(&["See [te", "xt](ht", "tp://x) now"]);
        assert_eq!(
            view.blocks,
            MarkdownView::new("See [text](http://x) now").blocks
        );
        let Block::Paragraph { spans } = &view.blocks[0] else {
            panic!("{:?}", view.blocks);
        };
        assert_eq!(spans_to_plain_text(spans), "See text now");
        assert!(spans.iter().any(|s| s.text == "text" && s.style.underline));
    }

    #[test]
    fn a_setext_underline_streamed_after_its_line_makes_a_heading() {
        for underline in ["=", "=====", "---"] {
            let view = streamed(&["Title", "\n", underline]);
            assert_eq!(view.blocks.len(), 1, "{underline:?}");
            assert!(
                matches!(&view.blocks[0], Block::Heading { spans, .. }
                    if spans_to_plain_text(spans).eq_ignore_ascii_case("title")),
                "{underline:?}: {:?}",
                view.blocks
            );
        }
    }

    #[test]
    fn appending_parses_again_from_the_last_top_level_block_only() {
        let mut view = MarkdownView::new("# One\n\n- a\n- b\n\nmiddle\n\nlast para");
        assert_eq!(
            view.reparse_from,
            Some(ReparsePoint {
                offset: "# One\n\n- a\n- b\n\nmiddle\n\n".len(),
                blocks: 4,
                list: None,
            })
        );
        view.append_chunk(" grows\n\nnext");
        assert_eq!(
            view.reparse_from.map(|p| p.offset),
            Some("# One\n\n- a\n- b\n\nmiddle\n\nlast para grows\n\n".len())
        );
        // In a long list, from its last item, which goes on its numbering.
        let items: String = (0..2_000).map(|n| format!("1. item {n}\n")).collect();
        let mut list = MarkdownView::new(&items);
        let last = items.rfind("1. ").unwrap();
        assert_eq!(
            list.reparse_from,
            Some(ReparsePoint {
                offset: last,
                blocks: 1_999,
                list: Some(ListState {
                    ordered: true,
                    next_number: 2_000,
                }),
            })
        );
        list.append_chunk("1. one more\n\n- [ ] a\n- ");
        assert_eq!(list.blocks, MarkdownView::new(list.source()).blocks);
        // Text after another block of a tight item is a paragraph, as when
        // a blank line later makes the list loose.
        let tight = "- a\n- ```\n  x\n  ```\n  text\n";
        let mut view = MarkdownView::new(tight);
        view.append_chunk("\n- c");
        assert_eq!(view.blocks, MarkdownView::new(view.source()).blocks);
        assert!(MarkdownView::new(tight).blocks.iter().any(
            |b| matches!(b, Block::Paragraph { spans } if spans_to_plain_text(spans) == "text")
        ));
        // A regex is not a footnote definition; a definition is.
        let regex = MarkdownView::new("Use `[^a-z]` or [^1].\n\nlast");
        assert_eq!(
            regex.reparse_from.map(|p| p.offset),
            Some("Use `[^a-z]` or [^1].\n\n".len())
        );
        let mut noted = regex.clone();
        noted.append_chunk("\n\n[^1]: a note");
        assert_eq!(noted.reparse_from, None);
        assert_eq!(noted.blocks, MarkdownView::new(noted.source()).blocks);
        // Nor does an item whose text is indented code.
        let code = MarkdownView::new("-     [ ] a\n\npara\n\nlast");
        assert_eq!(
            code.reparse_from.map(|p| p.offset),
            Some("-     [ ] a\n\npara\n\n".len())
        );
        // A task list whose markers were given does not hold it back.
        let tasks = MarkdownView::new("- [ ] a\n- [x] b\n\npara\n\nlast");
        assert_eq!(
            tasks.reparse_from.map(|p| p.offset),
            Some("- [ ] a\n- [x] b\n\npara\n\n".len())
        );
        // A block after a list may still become one of its items.
        let mut list = MarkdownView::new("- a\n\n1");
        assert_eq!(list.reparse_from.map(|p| p.offset), Some(0));
        list.append_chunk(". b");
        assert_eq!(list.blocks, MarkdownView::new("- a\n\n1. b").blocks);
        assert_eq!(view.blocks, MarkdownView::new(view.source()).blocks);
    }

    #[test]
    fn a_reference_definition_arriving_later_resolves_earlier_links() {
        let view = streamed(&["See [docs].\n\nMore.\n\n", "[docs]: http://x\n"]);
        assert_eq!(view.reparse_from, None);
        let Block::Paragraph { spans } = &view.blocks[0] else {
            panic!("{:?}", view.blocks);
        };
        assert!(spans.iter().any(|s| s.text == "docs" && s.style.underline));
        assert_eq!(view.blocks, MarkdownView::new(view.source()).blocks);
    }

    /// Markdown pieces random documents are made of: inline and block
    /// markers, words, and the line breaks between them.
    const PIECES: &[&str] = &[
        "word",
        "two words",
        " ",
        " ",
        "\n",
        "\n",
        "\n\n",
        "**",
        "*",
        "_",
        "__",
        "`",
        "``",
        "~~",
        "[",
        "]",
        "](",
        ")",
        "(http://x.y/z)",
        "http://x",
        "\"title\"",
        "# ",
        "## ",
        "=",
        "===",
        "-",
        "---",
        "- ",
        "* ",
        "+ ",
        "1. ",
        "2) ",
        "[ ] ",
        "[x] ",
        "- [ ] ",
        "- [x] ",
        "```",
        "```rust",
        "~~~",
        "    ",
        "  ",
        "\t",
        "> ",
        "| a | b |",
        "|---|---|",
        "<b>",
        "</b>",
        "<div>",
        "<!--",
        "-->",
        "\\",
        "&amp;",
        "é",
        "中文",
        "$x$",
        ": ",
        "+++",
        "[r]",
        "[r]: http://r\n",
        "[^1]",
        "[^1]: note\n",
        "![alt](i.png)",
        "<http://a>",
        "\u{a0}",
        "\u{3000}",
        "\r\n",
        "\r",
        "[^a-z]",
        "`[^a-z]`",
    ];

    fn markdown_document() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        proptest::collection::vec(proptest::sample::select(PIECES), 0..48)
            .prop_map(|pieces| pieces.concat())
    }

    /// Whole lines random documents are made of: every kind of block, in
    /// and out of containers, with the line endings markdown knows.
    const LINES: &[&str] = &[
        "",
        "",
        "",
        "word",
        "two words",
        "    indented code",
        "\tcode tab",
        "  two",
        "   three",
        "> quote",
        "> - quoted item",
        "> > deeper",
        ">",
        "- item",
        "- [ ] task",
        "- [x] done",
        "  - nested",
        "    - deep nested",
        "1. one",
        "2. two",
        "10) ten",
        "* star",
        "+ plus",
        "-",
        "1.",
        "- # head",
        "<!--",
        "-->",
        "<pre>",
        "</pre>",
        "<div>",
        "</div>",
        "<script>",
        "</script>",
        "<?php",
        "?>",
        "| a | b |",
        "|---|---|",
        "| 1 | 2 |",
        "a | b",
        "---",
        "***",
        "___",
        "===",
        "# h",
        "## h2",
        "```",
        "~~~",
        "```rust",
        "lazy *em",
        "em* end",
        "[r]: http://r",
        "[r]",
        "[^1]",
        "[^1]: note",
        "`[^a-z]` regex",
        "\u{a0}",
        "\u{a0}- [ ] x",
        "\u{3000}",
        "Intro\u{3000}",
        "  \t",
        "term",
        ": def",
        "$$",
        "x = 1",
        "foo\\",
        "  ",
        "- [ ]",
        "- [ ] ",
        "1. [x] # x",
        "-     [ ] a",
        "- ```",
        "  code in item",
        "<http://x>",
        "<b>bold",
        "<pa ok",
        "é中文",
        "+++",
        "- a\r",
        "b\r",
        "\r",
        "- [ ] item\r",
        "-\t[ ] tab task",
        "1.\t[x]\tdone",
        "- [ ] \u{3000}",
        ">\t- [ ] q",
        "* *",
        "- -",
        "1. 2. nested",
        " - one space",
        "3. three",
        "  text",
        "```",
    ];

    fn line_document() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        let ending = proptest::sample::select(&["\n", "\n", "\n", "\r\n", "  \n", "\r"][..]);
        proptest::collection::vec((proptest::sample::select(LINES), ending), 0..24).prop_map(
            |lines| {
                lines
                    .into_iter()
                    .map(|(line, ending)| format!("{line}{ending}"))
                    .collect()
            },
        )
    }

    /// Stream `document` into a view, starting from its first `first`
    /// characters and appending chunks of `sizes` characters in turn;
    /// after each chunk, the blocks must be what parsing all of it so far
    /// at once gives.
    fn check_stream(document: &str, first: usize, sizes: &[usize]) -> Result<(), String> {
        let chars: Vec<usize> = document.char_indices().map(|(i, _)| i).collect();
        let split = |n: usize| chars.get(n).copied().unwrap_or(document.len());
        let mut char_at = first.min(chars.len());
        let mut at = split(char_at);
        let mut view = MarkdownView::new(&document[..at]);
        for size in sizes.iter().cycle() {
            if at == document.len() {
                break;
            }
            char_at = (char_at + size).min(chars.len());
            let end = split(char_at);
            view.append_chunk(&document[at..end]);
            at = end;
            let whole = MarkdownView::new(&document[..at]);
            if view.blocks != whole.blocks {
                return Err(format!(
                    "after {:?} (chunks {sizes:?})\nstreamed {:?}\nwhole    {:?}",
                    &document[..at],
                    view.blocks,
                    whole.blocks
                ));
            }
        }
        Ok(())
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

        /// However a document is cut into chunks, appending them one by
        /// one gives, after each, what parsing all of it at once gives.
        #[test]
        fn streaming_in_any_chunks_matches_parsing_all_at_once(
            document in markdown_document(),
            sizes in proptest::collection::vec(1usize..8, 1..6),
            first in 0usize..3,
        ) {
            let checked = check_stream(&document, first, &sizes);
            proptest::prop_assert!(checked.is_ok(), "{}", checked.unwrap_err());
        }

        /// The same for documents made of whole lines of every kind of
        /// block, also streamed a character at a time.
        #[test]
        fn streaming_lines_in_any_chunks_matches_parsing_all_at_once(
            document in line_document(),
            sizes in proptest::collection::vec(1usize..12, 1..6),
        ) {
            let checked = check_stream(&document, 0, &sizes);
            proptest::prop_assert!(checked.is_ok(), "{}", checked.unwrap_err());
            let checked = check_stream(&document, 0, &[1]);
            proptest::prop_assert!(checked.is_ok(), "{}", checked.unwrap_err());
        }
    }

    #[test]
    fn syntax_pulldown_cmark_panics_on_is_shown_as_text() {
        // pulldown-cmark 0.12 panics on this with definition lists on.
        let quoted = MarkdownView::new("> _\n> : \n\n");
        assert_eq!(quoted.blocks.len(), 1);
        // And on this with heading attributes on.
        MarkdownView::new("1. [x] # x\n[r]: http://r\n  \t\n===\n");
        let heading = MarkdownView::new("# Title {#id}");
        assert!(
            matches!(&heading.blocks[0], Block::Heading { spans, .. } if spans_to_plain_text(spans) == "TITLE {#ID}")
        );
        let view = MarkdownView::new("term\n: definition");
        let Block::Paragraph { spans } = &view.blocks[0] else {
            panic!("{:?}", view.blocks);
        };
        assert_eq!(spans_to_plain_text(spans), "term\n: definition");
    }

    #[test]
    fn streaming_known_hard_documents_matches_parsing_all_at_once() {
        let documents = [
            "a\n\n- [ ] x\n\nb\n\n- [x] y",
            "\u{a0}\n- [ ] x y\n",
            "para\n\n    code\n\n    more\nlazy",
            "<!--\n\nfoo\n\n-->\n\nbar",
            "> - a\n>\n> - b\n\nc\n\nd",
            "a\r\n\r\n- [ ] b\r\n\r\nc",
            "- a\n\n\n\nb\n\nc\n  d",
            "| a | b |\n|---|---|\n| 1 | 2 |\n\nx\n\n| c |\n|---|",
            "x\n\n1) a\n\n2) b\n\nq",
            "a\n\n<div>\n\nb\n\n</div>\n\nc",
            "a\n\n- [ ]\n  # h\n\nz",
            // A blank line is spaces and tabs only: U+3000 and U+00A0 are
            // text.
            "Intro\u{3000}\n\u{3000}\n<pa ok",
            "\u{a0}\n<path to it",
            "Note:\u{3000}\n\u{3000}\n<p\nclass is a word",
            "intro\u{3000}\n\u{3000}\n<div\nmore",
            // A list's items parsed from its last: tight on their own, loose
            // in the whole, numbered on.
            "1. one\n\n1. [x] # x\n    indented code\n",
            "1. one\n1. 2. nested\n",
            "2) \n2) **word",
            // A bare CR ends the line a task marker is on.
            "1. [x] # x\n- [ ] \r-\n  \t\n- [ ]\n",
            // No front matter: the text between two rules stays.
            "---\ntitle: x\n\nbody\n---\nafter",
            "---\n\nbody\n---\n",
        ];
        for document in documents {
            for size in 1..=4 {
                if let Err(problem) = check_stream(document, 0, &[size]) {
                    panic!("{problem}");
                }
            }
        }
    }

    #[test]
    fn a_task_list_item_draws_a_checkbox_in_place_of_its_bullet() {
        let view = MarkdownView::new("- [ ] buy milk\n- [x] done\n1. [X] first");
        let items: Vec<(&str, Option<bool>)> = view
            .blocks
            .iter()
            .map(|block| match block {
                Block::ListItem { prefix, task, .. } => (prefix.as_str(), *task),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            items,
            [
                ("[ ] ", Some(false)),
                ("[x] ", Some(true)),
                ("1. [x] ", Some(true))
            ]
        );
        let ansi = render_ansi_text(&view, 30, 3);
        assert!(ansi.contains("[ ] buy milk"), "{ansi}");
        assert!(ansi.contains("[x] done"), "{ansi}");
        assert!(!ansi.contains('•'), "{ansi}");

        // Loose, the item's paragraph is its text all the same.
        let loose = MarkdownView::new("- [ ] a\n\n- [x] b");
        assert!(matches!(
            &loose.blocks[..],
            [
                Block::ListItem { task: Some(false), spans: a, .. },
                Block::ListItem { task: Some(true), spans: b, .. },
            ] if spans_to_plain_text(a) == "a" && spans_to_plain_text(b) == "b"
        ));
    }

    #[test]
    fn a_task_list_item_is_a_read_only_checkbox_named_by_its_text() {
        let view = MarkdownView::new("- [ ] buy milk\n- [x] done\n- plain\n\n3. [x] third");
        let semantics: Vec<Semantics> = render_view_messages(&view)
            .unwrap()
            .into_iter()
            .filter_map(|m| match m {
                AppMessage::SetSemantics { semantics, .. } => Some(semantics),
                _ => None,
            })
            .collect();
        let checkbox = |label: &str, toggled| Semantics {
            role: Role::Checkbox,
            label: Some(label.to_string()),
            toggled,
            read_only: true,
            ..Semantics::default()
        };
        assert_eq!(
            semantics,
            [
                checkbox("buy milk", Toggled::False),
                checkbox("done", Toggled::True),
                checkbox("3. third", Toggled::True)
            ]
        );
    }

    #[test]
    fn only_an_items_own_plain_text_makes_a_task() {
        for source in [
            "- [x](http://x) a link",
            "- `[ ]` code",
            "- [ ]",
            "- [y] no",
            "> - [ ] quoted",
            "a\n\n[ ] not in a list",
        ] {
            let view = MarkdownView::new(source);
            assert!(
                view.blocks
                    .iter()
                    .all(|b| !matches!(b, Block::ListItem { task: Some(_), .. })),
                "{source:?}: {:?}",
                view.blocks
            );
        }
        let view = MarkdownView::new("- [X] **bold** after");
        assert!(matches!(
            &view.blocks[0],
            Block::ListItem { task: Some(true), spans, .. }
                if spans_to_plain_text(spans) == "bold after"
        ));
    }

    #[test]
    fn an_ordered_task_items_number_is_spoken_and_its_checkbox_not() {
        let said = spoken(&["", "1. [ ] fir", "st\n2. [x] second"]);
        assert_eq!(said.replace('|', ""), "1. first2. second", "{said:?}");
    }

    #[test]
    fn a_streamed_task_list_item_is_spoken_without_its_checkbox() {
        let said = spoken(&[
            "",
            "- [ ] buy",
            " milk\n- [x",
            "] done\n",
            "- ",
            "[",
            " ] la",
            "st\n- ",
            "plain",
        ]);
        assert!(!said.contains('['), "{said:?}");
        assert_eq!(said.replace('|', ""), "buy milkdonelast• plain", "{said:?}");

        // The terminal announces a live node when it is added and when its
        // name changes: an item is a checkbox only once it is finished, and
        // a region created live shows nothing it has not appended.
        let (_, _, frames) = stream(&["", "- [ ] buy", " milk\n- ", "[x", "] done"]);
        for frame in &frames {
            for message in frame {
                match message {
                    AppMessage::SetSemantics { semantics, .. } if semantics.live != Live::Off => {
                        assert_ne!(semantics.role, Role::Checkbox, "{frame:?}");
                    }
                    AppMessage::CreateRegion { id, .. } => {
                        let appended = frame.iter().any(
                            |m| matches!(m, AppMessage::AppendContent { id: a, .. } if a == id),
                        );
                        let shows_text = frame.iter().any(|m| {
                            matches!(m, AppMessage::SetContent { id: s, content: Content::RichText(r) }
                                if s == id && r.spans.iter().any(|span| !span.text.is_empty()))
                        });
                        assert!(appended || !shows_text, "{frame:?}");
                    }
                    _ => {}
                }
            }
        }
    }
}
