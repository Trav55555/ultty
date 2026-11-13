//! Cached transcript rows shared by native and ANSI views.

use ulttui::prelude::{Color, Column, Dimension, MarkdownView, Style};
use ulttui::widgets::RichText;
use ultty_protocol::RichText as Row;

use crate::types::{ChatMessage, ToolCall, ToolStatus};
use crate::views::format_tool_result_with_diff;

#[derive(Default)]
pub(crate) struct Transcript {
    width: usize,
    entries: Vec<Entry>,
    ends: Vec<usize>,
}

struct Entry {
    heading: String,
    body: String,
    color: Color,
    rows: Vec<Row>,
}

impl Entry {
    fn matches(&self, heading: &str, body: &str, color: Color) -> bool {
        self.heading == heading && self.body == body && self.color == color
    }

    fn new(heading: String, body: String, color: Color, width: usize) -> Self {
        let mut rows = vec![Row::new().span(&heading, Style::new().fg(color).bold())];
        rows.extend(MarkdownView::new(&body).wrapped_rows(width));
        rows.push(Row::new());
        Self {
            heading,
            body,
            color,
            rows,
        }
    }
}

impl Transcript {
    pub(crate) fn sync(&mut self, width: usize, messages: &[ChatMessage], tools: &[ToolCall]) {
        if self.width != width {
            self.entries.clear();
            self.width = width;
        }
        self.entries.truncate(messages.len() + tools.len());
        for (index, message) in messages.iter().enumerate() {
            let (name, color) = match message.role.as_str() {
                "user" => ("You", Color::Rgb(58, 224, 221)),
                "assistant" => ("Porygon", Color::Rgb(255, 93, 110)),
                _ => ("System", Color::Rgb(136, 136, 136)),
            };
            let heading = format!("{name}  {}", message.timestamp);
            self.update(index, heading, &message.content, color);
        }
        for (index, tool) in tools.iter().enumerate() {
            let (status, color) = match tool.status {
                ToolStatus::Pending => ("pending", Color::Rgb(249, 226, 175)),
                ToolStatus::Approved => ("approved", Color::Rgb(166, 227, 161)),
                ToolStatus::Rejected => ("rejected", Color::Rgb(255, 93, 110)),
                ToolStatus::Running => ("running", Color::Rgb(58, 224, 221)),
                ToolStatus::Completed => ("completed", Color::Rgb(166, 227, 161)),
                ToolStatus::Failed => ("failed", Color::Rgb(255, 93, 110)),
            };
            let mut body = String::new();
            if !tool.args.is_empty() {
                body.push_str(&format!("```json\n{}\n```\n", tool.args));
            }
            if let Some(result) = &tool.result {
                body.push_str(&format_tool_result_with_diff(result));
            }
            self.update(
                messages.len() + index,
                format!("{}  {status}", tool.name),
                &body,
                color,
            );
        }
        self.ends.clear();
        let mut end = 0;
        for entry in &self.entries {
            end += entry.rows.len();
            self.ends.push(end);
        }
    }

    fn update(&mut self, index: usize, heading: String, body: &str, color: Color) {
        if self
            .entries
            .get(index)
            .is_some_and(|entry| entry.matches(&heading, body, color))
        {
            return;
        }
        let entry = Entry::new(heading, body.into(), color, self.width);
        if let Some(slot) = self.entries.get_mut(index) {
            *slot = entry;
        } else {
            self.entries.push(entry);
        }
    }

    pub(crate) fn height(&self, working: bool) -> usize {
        self.ends.last().copied().unwrap_or(0) + usize::from(working)
    }

    fn rows(&self, start: usize, count: usize) -> impl Iterator<Item = &Row> {
        let entry = self.ends.partition_point(|end| *end <= start);
        let before = entry.checked_sub(1).map_or(0, |index| self.ends[index]);
        self.entries[entry..]
            .iter()
            .flat_map(|entry| entry.rows.iter())
            .skip(start - before)
            .take(count)
    }

    pub(crate) fn view(&self, start: usize, height: usize, working: bool) -> Column {
        let mut column = Column::new()
            .id("conversation-transcript")
            .width(Dimension::Percent(100.0))
            .height(Dimension::Cells(height as f32))
            .flex_shrink(0.0)
            .overflow_y_hidden();
        let mut visible = 0;
        for row in self.rows(start, height) {
            let mut text = RichText::new().span(" ", Style::default()).fill_width();
            for span in &row.spans {
                text = text.span(&span.text, span.style.clone());
            }
            column = column.child(text);
            visible += 1;
        }
        if working && visible < height && start + visible >= self.height(false) {
            column = column.child(RichText::new().text(" Working...").fill_width());
        }
        column
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(count: usize) -> Vec<ChatMessage> {
        (0..count)
            .map(|index| ChatMessage {
                role: "assistant".into(),
                content: format!("unique message {index}"),
                timestamp: "12:00".into(),
            })
            .collect()
    }

    #[test]
    fn ten_thousand_messages_reach_both_ends_with_bounded_visible_tree() {
        let mut transcript = Transcript::default();
        transcript.sync(76, &messages(10_000), &[]);
        assert!(transcript.height(false) > 4096);
        let bottom = transcript.height(false) - 20;
        let visible = transcript
            .rows(bottom, 20)
            .flat_map(|row| &row.spans)
            .map(|span| span.text.as_str())
            .collect::<String>();
        assert!(visible.contains("unique message 9999"));
        let first = transcript
            .rows(0, 20)
            .flat_map(|row| &row.spans)
            .map(|span| span.text.as_str())
            .collect::<String>();
        assert!(first.contains("unique message 0"));
        assert_eq!(transcript.view(bottom, 20, false).children_count(), 20);
    }

    #[test]
    fn source_replacement_width_and_truncation_refresh_cached_rows() {
        let mut transcript = Transcript::default();
        let mut source = messages(3);
        transcript.sync(76, &source, &[]);
        let original = transcript.height(false);
        source[1].content = "changed\n\nsecond paragraph".into();
        transcript.sync(76, &source, &[]);
        assert!(transcript
            .rows(0, usize::MAX)
            .flat_map(|row| &row.spans)
            .any(|span| span.text.contains("changed")));
        transcript.sync(4, &source, &[]);
        assert!(transcript.height(false) > original);
        source.clear();
        transcript.sync(4, &source, &[]);
        assert_eq!(transcript.height(false), 0);
    }
}
