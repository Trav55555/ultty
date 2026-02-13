//! Diff widget for rendering unified diffs.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style as RStyle};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    AppMessage, Color, Constraints, Content, Dimension, Display, FlexDirection, RegionId,
    Style as ProtoStyle, TextContent,
};

const GUTTER_WIDTH: f32 = 12.0;

/// The kind of a diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// Unchanged context line.
    Context,
    /// Added line.
    Added,
    /// Removed line.
    Removed,
    /// Diff header (file names, hunk headers).
    Header,
}

/// Display mode for the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffMode {
    /// Standard unified diff format.
    #[default]
    Unified,
}

/// A single line in a diff.
#[derive(Debug, Clone)]
pub struct DiffLine {
    /// Type of this line.
    pub kind: DiffKind,
    /// Raw diff line content.
    pub content: String,
    /// Source line number in the old file, if applicable.
    pub old_line: Option<usize>,
    /// Source line number in the new file, if applicable.
    pub new_line: Option<usize>,
}

impl DiffLine {
    /// Create a new diff line with no line numbers.
    pub fn new(kind: DiffKind, content: impl Into<String>) -> Self {
        Self {
            kind,
            content: content.into(),
            old_line: None,
            new_line: None,
        }
    }

    /// Set the old-file line number.
    pub fn old_line(mut self, n: usize) -> Self {
        self.old_line = Some(n);
        self
    }

    /// Set the new-file line number.
    pub fn new_line(mut self, n: usize) -> Self {
        self.new_line = Some(n);
        self
    }
}

/// Widget for rendering file diffs with colored +/- lines.
#[derive(Debug, Clone)]
pub struct DiffView {
    lines: Vec<DiffLine>,
    mode: DiffMode,
    id: Option<String>,
}

impl DiffView {
    /// Create a new diff view from pre-parsed diff lines.
    pub fn new(lines: Vec<DiffLine>) -> Self {
        Self {
            lines,
            mode: DiffMode::Unified,
            id: None,
        }
    }

    /// Set the display mode.
    pub fn mode(mut self, mode: DiffMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set widget ID for focus management.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// Parse a unified diff string into a vector of `DiffLine`s.
///
/// Recognizes:
/// - `---` / `+++` as file headers
/// - `@@` as hunk headers
/// - Lines starting with `+` as Added
/// - Lines starting with `-` as Removed
/// - Lines starting with ` ` or anything else as Context
pub fn parse_unified_diff(text: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let mut old_line = None;
    let mut new_line = None;

    for raw in text.lines() {
        if raw.starts_with("---") || raw.starts_with("+++") {
            lines.push(DiffLine::new(DiffKind::Header, raw));
            continue;
        }

        if raw.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_starts(raw) {
                old_line = Some(old_start);
                new_line = Some(new_start);
            }
            lines.push(DiffLine::new(DiffKind::Header, raw));
            continue;
        }

        if raw.starts_with('+') {
            let mut line = DiffLine::new(DiffKind::Added, raw);
            if let Some(current) = new_line {
                line = line.new_line(current);
                new_line = Some(current + 1);
            }
            lines.push(line);
            continue;
        }

        if raw.starts_with('-') {
            let mut line = DiffLine::new(DiffKind::Removed, raw);
            if let Some(current) = old_line {
                line = line.old_line(current);
                old_line = Some(current + 1);
            }
            lines.push(line);
            continue;
        }

        let mut line = DiffLine::new(DiffKind::Context, raw);
        if let Some(current) = old_line {
            line = line.old_line(current);
            old_line = Some(current + 1);
        }
        if let Some(current) = new_line {
            line = line.new_line(current);
            new_line = Some(current + 1);
        }
        lines.push(line);
    }

    lines
}

impl View for DiffView {
    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            ..Default::default()
        }
    }

    fn style(&self) -> ProtoStyle {
        ProtoStyle::default()
    }

    fn children_count(&self) -> usize {
        self.lines.len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        let Some(line) = self.lines.get(index) else {
            return Ok(None);
        };

        let theme = Theme::current();

        let row_style = match line.kind {
            DiffKind::Added => ProtoStyle::new().bg(Color::Rgb(20, 45, 20)),
            DiffKind::Removed => ProtoStyle::new().bg(Color::Rgb(45, 20, 20)),
            _ => ProtoStyle::default(),
        };

        let content_style = match line.kind {
            DiffKind::Added => ProtoStyle::new()
                .fg(Color::Rgb(0, 200, 0))
                .bg(Color::Rgb(20, 45, 20)),
            DiffKind::Removed => ProtoStyle::new()
                .fg(Color::Rgb(200, 0, 0))
                .bg(Color::Rgb(45, 20, 20)),
            DiffKind::Header => ProtoStyle::new().fg(Color::Rgb(0, 200, 200)).bold(),
            DiffKind::Context => ProtoStyle::new().fg(theme.foreground),
        };

        let row_id = RegionId::new(*next_id);
        *next_id += 1;

        send_message(&AppMessage::CreateRegion {
            id: row_id,
            parent,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(1.0),
                ..Default::default()
            },
            style: row_style,
        })?;

        let gutter_id = RegionId::new(*next_id);
        *next_id += 1;
        send_message(&AppMessage::CreateRegion {
            id: gutter_id,
            parent: row_id,
            insert_index: -1,
            constraints: Constraints {
                width: Dimension::Cells(GUTTER_WIDTH),
                flex_shrink: 0.0,
                ..Default::default()
            },
            style: ProtoStyle::new().fg(theme.muted),
        })?;
        send_message(&AppMessage::SetContent {
            id: gutter_id,
            content: Content::Text(TextContent::new(format_gutter(
                line.old_line,
                line.new_line,
            ))),
        })?;

        let content_id = RegionId::new(*next_id);
        *next_id += 1;
        send_message(&AppMessage::CreateRegion {
            id: content_id,
            parent: row_id,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                flex_grow: 1.0,
                width: Dimension::Percent(100.0),
                ..Default::default()
            },
            style: content_style,
        })?;
        send_message(&AppMessage::SetContent {
            id: content_id,
            content: Content::Text(TextContent::new(&line.content)),
        })?;

        Ok(Some(row_id))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let theme = Theme::current();
        let gutter_style = RStyle::default()
            .fg(to_ratatui_color(theme.muted))
            .add_modifier(Modifier::DIM);

        let rows: Vec<Line> = self
            .lines
            .iter()
            .map(|line| {
                let content_style = match line.kind {
                    DiffKind::Added => RStyle::default()
                        .fg(to_ratatui_color(Color::Rgb(0, 200, 0)))
                        .bg(to_ratatui_color(Color::Rgb(20, 45, 20))),
                    DiffKind::Removed => RStyle::default()
                        .fg(to_ratatui_color(Color::Rgb(200, 0, 0)))
                        .bg(to_ratatui_color(Color::Rgb(45, 20, 20))),
                    DiffKind::Header => RStyle::default()
                        .fg(to_ratatui_color(Color::Rgb(0, 200, 200)))
                        .add_modifier(Modifier::BOLD),
                    DiffKind::Context => RStyle::default()
                        .fg(to_ratatui_color(theme.foreground))
                        .add_modifier(Modifier::DIM),
                };

                Line::from(vec![
                    Span::styled(format_gutter(line.old_line, line.new_line), gutter_style),
                    Span::styled(format!(" {}", line.content), content_style),
                ])
            })
            .collect();

        frame.render_widget(Paragraph::new(rows), area);
    }

    fn is_focusable(&self) -> bool {
        self.id.is_some()
    }

    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

fn parse_hunk_starts(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace();
    let at = parts.next()?;
    if at != "@@" {
        return None;
    }

    let old_part = parts.next()?;
    let new_part = parts.next()?;

    let old_start = parse_hunk_side(old_part, '-')?;
    let new_start = parse_hunk_side(new_part, '+')?;
    Some((old_start, new_start))
}

fn parse_hunk_side(side: &str, expected_prefix: char) -> Option<usize> {
    let mut chars = side.chars();
    if chars.next()? != expected_prefix {
        return None;
    }
    let rest: String = chars.collect();
    let start = rest.split(',').next()?;
    start.parse::<usize>().ok()
}

fn format_gutter(old_line: Option<usize>, new_line: Option<usize>) -> String {
    let old = old_line.map(|n| n.to_string()).unwrap_or_default();
    let new = new_line.map(|n| n.to_string()).unwrap_or_default();
    format!("{old:>4} | {new:>4}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_line_new() {
        let line = DiffLine::new(DiffKind::Context, "hello");
        assert_eq!(line.kind, DiffKind::Context);
        assert_eq!(line.content, "hello");
        assert_eq!(line.old_line, None);
        assert_eq!(line.new_line, None);
    }

    #[test]
    fn test_diff_line_with_line_numbers() {
        let line = DiffLine::new(DiffKind::Added, "+hello")
            .old_line(2)
            .new_line(3);
        assert_eq!(line.old_line, Some(2));
        assert_eq!(line.new_line, Some(3));
    }

    #[test]
    fn test_diff_kind_equality() {
        assert_eq!(DiffKind::Added, DiffKind::Added);
        assert_ne!(DiffKind::Added, DiffKind::Removed);
    }

    #[test]
    fn test_diff_mode_default() {
        assert_eq!(DiffMode::default(), DiffMode::Unified);
    }

    #[test]
    fn test_diff_view_new() {
        let view = DiffView::new(vec![DiffLine::new(DiffKind::Context, "line")]);
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.mode, DiffMode::Unified);
        assert_eq!(view.id, None);
    }

    #[test]
    fn test_diff_view_children_count() {
        let view = DiffView::new(vec![
            DiffLine::new(DiffKind::Context, "a"),
            DiffLine::new(DiffKind::Added, "+b"),
            DiffLine::new(DiffKind::Removed, "-c"),
        ]);
        assert_eq!(<DiffView as View>::children_count(&view), 3);
    }

    #[test]
    fn test_diff_view_constraints_are_column() {
        let view = DiffView::new(Vec::new());
        let constraints = <DiffView as View>::constraints(&view);
        assert_eq!(constraints.display, Display::Flex);
        assert_eq!(constraints.flex_direction, FlexDirection::Column);
        assert_eq!(constraints.width, Dimension::Percent(100.0));
    }

    #[test]
    fn test_diff_view_with_id_is_focusable() {
        let view = DiffView::new(Vec::new()).id("diff");
        assert!(view.is_focusable());
        assert_eq!(view.widget_id(), Some("diff"));
    }

    #[test]
    fn test_diff_view_without_id_not_focusable() {
        let view = DiffView::new(Vec::new());
        assert!(!view.is_focusable());
        assert_eq!(view.widget_id(), None);
    }

    #[test]
    fn test_parse_unified_diff_empty() {
        let lines = parse_unified_diff("");
        assert!(lines.is_empty());
    }

    #[test]
    fn test_parse_unified_diff_simple() {
        let lines = parse_unified_diff(" context\n+added\n-removed");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].kind, DiffKind::Context);
        assert_eq!(lines[1].kind, DiffKind::Added);
        assert_eq!(lines[2].kind, DiffKind::Removed);
    }

    #[test]
    fn test_parse_unified_diff_with_headers() {
        let diff = "--- a/file.txt\n+++ b/file.txt\n@@ -1,2 +1,2 @@\n line";
        let lines = parse_unified_diff(diff);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].kind, DiffKind::Header);
        assert_eq!(lines[1].kind, DiffKind::Header);
        assert_eq!(lines[2].kind, DiffKind::Header);
        assert_eq!(lines[3].kind, DiffKind::Context);
    }

    #[test]
    fn test_parse_unified_diff_line_numbers() {
        let diff = "@@ -10,2 +20,3 @@\n context\n-removed\n+added\n context2";
        let lines = parse_unified_diff(diff);

        assert_eq!(lines[0].kind, DiffKind::Header);
        assert_eq!(lines[1].old_line, Some(10));
        assert_eq!(lines[1].new_line, Some(20));
        assert_eq!(lines[2].old_line, Some(11));
        assert_eq!(lines[2].new_line, None);
        assert_eq!(lines[3].old_line, None);
        assert_eq!(lines[3].new_line, Some(21));
        assert_eq!(lines[4].old_line, Some(12));
        assert_eq!(lines[4].new_line, Some(22));
    }

    #[test]
    fn test_parse_unified_diff_multiple_hunks() {
        let diff = "@@ -1,1 +1,1 @@\n-old\n+new\n@@ -10,2 +20,2 @@\n a\n-b\n+c";
        let lines = parse_unified_diff(diff);

        assert_eq!(lines[0].kind, DiffKind::Header);
        assert_eq!(lines[1].old_line, Some(1));
        assert_eq!(lines[2].new_line, Some(1));
        assert_eq!(lines[3].kind, DiffKind::Header);
        assert_eq!(lines[4].old_line, Some(10));
        assert_eq!(lines[4].new_line, Some(20));
        assert_eq!(lines[5].old_line, Some(11));
        assert_eq!(lines[5].new_line, None);
        assert_eq!(lines[6].old_line, None);
        assert_eq!(lines[6].new_line, Some(21));
    }
}
