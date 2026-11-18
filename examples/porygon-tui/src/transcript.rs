//! Cached transcript rows shared by native and ANSI views.

use ulttui::prelude::{Color, Column, Dimension, MarkdownView, Style};
use ulttui::widgets::{PreparedMarkdownRow, RichText};
type Row = PreparedMarkdownRow;

fn plain_row(content: ultty_protocol::RichText) -> Row {
    Row::new(content)
}

use crate::types::{ChatMessage, ToolCall, ToolStatus};
use crate::views::format_tool_result_with_diff;

#[derive(Default)]
pub(crate) struct Transcript {
    width: usize,
    entries: Vec<Entry>,
    ends: Vec<usize>,
    logical_ends: Vec<usize>,
    /// Entry index of each message; tool entries sit between them.
    message_entries: Vec<usize>,
    revision: u64,
    conversation_id: Option<String>,
}

struct Entry {
    heading: String,
    body: String,
    color: Color,
    rows: Vec<Row>,
    starts: Vec<usize>,
    logical_len: usize,
    assistant: bool,
}

impl Entry {
    fn rendered_bytes(&self) -> impl Iterator<Item = u8> + '_ {
        self.rows.iter().flat_map(|row| {
            row.content
                .spans
                .iter()
                .flat_map(|span| span.text.bytes())
                .chain(std::iter::once(b'\n').take(usize::from(row.hard_break)))
        })
    }

    fn matches(&self, heading: &str, body: &str, color: Color) -> bool {
        self.heading == heading && self.body == body && self.color == color
    }

    fn new(heading: String, body: String, color: Color, width: usize, assistant: bool) -> Self {
        let header = || {
            let mut row = plain_row(
                ultty_protocol::RichText::new().span(&heading, Style::new().fg(color).bold()),
            );
            row.hard_break = true;
            row
        };
        let markdown = MarkdownView::new(&body)
            .with_visible_link_targets()
            .word_wrap_prose();
        let mut rows = vec![header()];
        rows.extend(markdown.prepared_rows(width));
        if let Some(last) = rows.last_mut() {
            last.hard_break = true;
        }
        let mut separator = plain_row(ultty_protocol::RichText::new());
        separator.hard_break = true;
        rows.push(separator);
        let mut logical_len = 0usize;
        let mut starts = Vec::with_capacity(rows.len());
        for row in &rows {
            starts.push(logical_len);
            logical_len += row
                .content
                .spans
                .iter()
                .map(|span| span.text.len())
                .sum::<usize>();
            logical_len += usize::from(row.hard_break);
        }
        Self {
            heading,
            body,
            color,
            rows,
            starts,
            logical_len,
            assistant,
        }
    }
}

impl Transcript {
    /// Rebuild entries from messages and tool calls, placing each tool call
    /// before the message it is anchored to so the transcript reads in the
    /// order things happened.
    pub(crate) fn sync(
        &mut self,
        width: usize,
        messages: &[ChatMessage],
        tools: &[ToolCall],
        conversation_id: Option<&str>,
    ) {
        let width_changed = self.width != width;
        self.width = width;
        let total = messages.len() + tools.len();
        let mut changed =
            self.conversation_id.as_deref() != conversation_id || self.entries.len() != total;
        if self.conversation_id.as_deref() != conversation_id {
            self.conversation_id = conversation_id.map(str::to_owned);
        }
        self.entries.truncate(total);
        self.message_entries.clear();
        let mut tools = tools.iter().peekable();
        let mut index = 0;
        for (message_index, message) in messages.iter().enumerate() {
            while let Some(tool) = tools.next_if(|tool| tool.before_message <= message_index) {
                changed |= self.update_tool(index, tool, width_changed);
                index += 1;
            }
            self.message_entries.push(index);
            changed |= self.update_message(index, message, width_changed);
            index += 1;
        }
        for tool in tools {
            changed |= self.update_tool(index, tool, width_changed);
            index += 1;
        }
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        self.ends.clear();
        self.logical_ends.clear();
        let mut logical_end = 0;
        let mut end = 0;
        for entry in &self.entries {
            end += entry.rows.len();
            self.ends.push(end);
            logical_end += entry.logical_len;
            self.logical_ends.push(logical_end);
        }
    }

    fn update_message(&mut self, index: usize, message: &ChatMessage, width_changed: bool) -> bool {
        let (name, color) = match message.role.as_str() {
            "user" => ("You", Color::Rgb(58, 224, 221)),
            "assistant" => ("Porygon", Color::Rgb(255, 93, 110)),
            _ => ("System", Color::Rgb(136, 136, 136)),
        };
        self.update(
            index,
            format!("{name}  {}", message.timestamp),
            &message.content,
            color,
            message.role == "assistant",
            width_changed,
        )
    }

    fn update_tool(&mut self, index: usize, tool: &ToolCall, width_changed: bool) -> bool {
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
            index,
            format!("{}  {status}", tool.name),
            &body,
            color,
            false,
            width_changed,
        )
    }

    fn update(
        &mut self,
        index: usize,
        heading: String,
        body: &str,
        color: Color,
        assistant: bool,
        width_changed: bool,
    ) -> bool {
        let same = self.entries.get(index).is_some_and(|entry| {
            entry.matches(&heading, body, color) && entry.assistant == assistant
        });
        if same && !width_changed {
            return false;
        }
        let entry = Entry::new(heading, body.into(), color, self.width, assistant);
        let changed = !same
            || self
                .entries
                .get(index)
                .is_none_or(|old| !old.rendered_bytes().eq(entry.rendered_bytes()));
        if let Some(slot) = self.entries.get_mut(index) {
            *slot = entry;
        } else {
            self.entries.push(entry);
        }
        changed
    }

    /// Rows of the assistant message being streamed, if `active_response` is one.
    fn live_range(&self, active_response: Option<usize>) -> std::ops::Range<usize> {
        active_response
            .and_then(|message| self.message_entries.get(message).copied())
            .filter(|&index| self.entries[index].assistant)
            .map_or(0..0, |index| {
                let first = index
                    .checked_sub(1)
                    .map_or(0, |previous| self.ends[previous]);
                first..self.ends[index]
            })
    }

    #[cfg(test)]
    pub(crate) fn plain_text(&self) -> String {
        String::from_utf8(
            self.entries
                .iter()
                .flat_map(Entry::rendered_bytes)
                .collect(),
        )
        .expect("rendered rows are UTF-8")
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

    pub(crate) fn view(
        &self,
        start: usize,
        height: usize,
        working: bool,
        active_response: Option<usize>,
        follow_output: bool,
    ) -> Column {
        let mut column = Column::new()
            .id("conversation-transcript")
            .width(Dimension::Percent(100.0))
            .height(Dimension::Cells(height as f32))
            .flex_shrink(0.0)
            .overflow_y_hidden();
        let mut visible = 0;
        let live_range = (working && follow_output).then(|| self.live_range(active_response));
        for (offset, row) in self.rows(start, height).enumerate() {
            let row = row.clone();
            let row = if live_range
                .as_ref()
                .is_some_and(|range| range.contains(&(start + offset)))
            {
                row.streaming()
            } else {
                row
            };
            let index = start + offset;
            let entry = self.ends.partition_point(|end| *end <= index);
            let previous_rows = entry
                .checked_sub(1)
                .map_or(0, |previous| self.ends[previous]);
            let previous_bytes = entry
                .checked_sub(1)
                .map_or(0, |previous| self.logical_ends[previous]);
            let logical_start = previous_bytes + self.entries[entry].starts[index - previous_rows];
            let metadata = format!(
                "{}:{}:{}",
                self.revision,
                logical_start,
                u8::from(row.hard_break)
            );
            column = column.child(row.with_selection_metadata(metadata));
            visible += 1;
        }
        if working && visible < height && start + visible >= self.height(false) {
            column = column.child(RichText::new().text("Working...").fill_width());
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

    fn message(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
            timestamp: "12:00".into(),
        }
    }

    fn tool(result: &str, before_message: usize) -> ToolCall {
        ToolCall {
            name: "shell".into(),
            args: String::new(),
            status: ToolStatus::Completed,
            result: Some(result.into()),
            before_message,
        }
    }

    #[test]
    fn tool_entries_render_between_the_messages_they_happened_between() {
        let mut transcript = Transcript::default();
        let source = [
            message("user", "question"),
            message("assistant", "Let me look."),
            message("assistant", "final answer"),
        ];
        let tools = [tool("first output", 2), tool("second output", 2)];
        transcript.sync(80, &source, &tools, None);
        let text = transcript.plain_text();
        let order: Vec<_> = [
            "question",
            "Let me look.",
            "first output",
            "second output",
            "final answer",
        ]
        .iter()
        .map(|needle| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {text}"))
        })
        .collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
        let live = transcript.live_range(Some(2));
        let live_text: String = transcript
            .rows(live.start, live.len())
            .flat_map(|row| &row.content.spans)
            .map(|span| span.text.as_str())
            .collect();
        assert!(live_text.contains("final answer"), "{live_text}");
        assert!(!live_text.contains("second output"), "{live_text}");
    }

    #[test]
    fn tools_after_the_last_message_render_last() {
        let mut transcript = Transcript::default();
        transcript.sync(
            80,
            &[message("user", "question")],
            &[tool("output", 1)],
            None,
        );
        let text = transcript.plain_text();
        assert!(text.find("question") < text.find("output"), "{text}");
    }

    #[test]
    fn prepared_rows_preserve_checkbox_stream_and_copy_text() {
        use ultty_protocol::{Live, Role, Toggled};
        let mut transcript = Transcript::default();
        let message = ChatMessage {
            role: "assistant".into(),
            timestamp: "now".into(),
            content: "- [x] done\n\n```\nlet x = 1;\n```".into(),
        };
        transcript.sync(80, &[message], &[], None);
        let rows: Vec<_> = transcript.rows(0, usize::MAX).collect();
        let checkbox = rows
            .iter()
            .find(|r| r.semantics.role == Role::Checkbox)
            .unwrap();
        assert_eq!(checkbox.semantics.toggled, Toggled::True);
        assert!(checkbox.semantics.read_only);
        assert_eq!(checkbox.semantics.label.as_deref(), Some("done"));
        let code = rows
            .iter()
            .find(|r| r.content.spans.iter().any(|s| s.text.contains("let x")))
            .unwrap();
        assert_eq!(
            code.content
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            "let x = 1;"
        );
        let live: Vec<_> = transcript
            .rows(0, usize::MAX)
            .cloned()
            .map(Row::streaming)
            .collect();
        assert!(live.iter().any(|r| r.semantics.role == Role::Log
            && r.semantics.live == Live::Polite
            && r.semantics.busy));
        assert!(!live
            .iter()
            .any(|r| r.content.spans.iter().any(|s| s.text.starts_with("│ "))));
    }

    #[test]
    fn transcript_displays_link_destination() {
        let mut transcript = Transcript::default();
        let message = ChatMessage {
            role: "user".into(),
            timestamp: "now".into(),
            content: "See [guide](https://example.org/path)".into(),
        };
        transcript.sync(80, &[message], &[], None);
        let text = transcript
            .rows(0, usize::MAX)
            .flat_map(|row| &row.content.spans)
            .map(|span| span.text.as_str())
            .collect::<String>();
        assert!(text.contains("guide (https://example.org/path)"), "{text}");
    }

    #[test]
    fn ten_thousand_messages_reach_both_ends_with_bounded_visible_tree() {
        let mut transcript = Transcript::default();
        transcript.sync(76, &messages(10_000), &[], None);
        assert!(transcript.height(false) > 4096);
        let bottom = transcript.height(false) - 20;
        let visible = transcript
            .rows(bottom, 20)
            .flat_map(|row| &row.content.spans)
            .map(|span| span.text.as_str())
            .collect::<String>();
        assert!(visible.contains("unique message 9999"));
        let first = transcript
            .rows(0, 20)
            .flat_map(|row| &row.content.spans)
            .map(|span| span.text.as_str())
            .collect::<String>();
        assert!(first.contains("unique message 0"));
        assert_eq!(
            transcript
                .view(bottom, 20, false, None, false)
                .children_count(),
            20
        );
    }

    #[test]
    fn width_dependent_rule_invalidates_selection_revision() {
        let mut transcript = Transcript::default();
        let mut source = messages(1);
        source[0].content = "before\n\n---\n\nafter".into();
        transcript.sync(80, &source, &[], None);
        let revision = transcript.revision;
        transcript.sync(20, &source, &[], None);
        assert_ne!(transcript.revision, revision);
    }

    #[test]
    fn tab_rendered_text_and_offsets_do_not_change_with_soft_wrap() {
        let mut transcript = Transcript::default();
        let mut source = messages(1);
        source[0].content = "```\naaaaa\tX\n```".into();
        transcript.sync(80, &source, &[], None);
        let flatten = |transcript: &Transcript| {
            transcript.entries[0]
                .rows
                .iter()
                .map(|row| {
                    let mut text = row
                        .content
                        .spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>();
                    if row.hard_break {
                        text.push('\n');
                    }
                    text
                })
                .collect::<String>()
        };
        let expected = flatten(&transcript);
        assert!(expected.contains("aaaaa   X"));
        let revision = transcript.revision;
        transcript.sync(4, &source, &[], None);
        assert_eq!(flatten(&transcript), expected);
        assert_eq!(transcript.revision, revision);
    }

    #[test]
    fn prose_soft_wrap_keeps_words_and_copy_offsets_contiguous() {
        let mut transcript = Transcript::default();
        let mut source = messages(1);
        source[0].content = "Alpha beta gamma".into();
        transcript.sync(8, &source, &[], None);

        let entry = &transcript.entries[0];
        let body_rows = &entry.rows[1..entry.rows.len() - 1];
        let rendered: Vec<_> = body_rows
            .iter()
            .map(|row| {
                row.content
                    .spans
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(rendered, ["Alpha ", "beta ", "gamma"]);
        assert!(body_rows[..body_rows.len() - 1]
            .iter()
            .all(|row| !row.hard_break));
        assert!(body_rows.last().unwrap().hard_break);
        assert_eq!(rendered.concat(), source[0].content);
        for (index, pair) in body_rows.windows(2).enumerate() {
            let current_bytes: usize = pair[0]
                .content
                .spans
                .iter()
                .map(|span| span.text.len())
                .sum();
            assert_eq!(
                entry.starts[index + 2],
                entry.starts[index + 1] + current_bytes
            );
        }
    }

    #[test]
    fn selection_revision_stays_stable_on_reflow_and_changes_on_edit() {
        let mut transcript = Transcript::default();
        let mut source = messages(1);
        source[0].content = "width-sensitive é 雪 and more".into();
        transcript.sync(80, &source, &[], None);
        let revision = transcript.revision;
        let length = transcript.logical_ends[0];
        transcript.sync(6, &source, &[], None);
        assert_eq!(transcript.revision, revision);
        assert_eq!(transcript.logical_ends[0], length);
        source[0].content.push('!');
        transcript.sync(6, &source, &[], None);
        assert_ne!(transcript.revision, revision);
    }

    #[test]
    fn source_replacement_width_and_truncation_refresh_cached_rows() {
        let mut transcript = Transcript::default();
        let mut source = messages(3);
        transcript.sync(76, &source, &[], None);
        let original = transcript.height(false);
        source[1].content = "changed\n\nsecond paragraph".into();
        transcript.sync(76, &source, &[], None);
        assert!(transcript
            .rows(0, usize::MAX)
            .flat_map(|row| &row.content.spans)
            .any(|span| span.text.contains("changed")));
        transcript.sync(4, &source, &[], None);
        assert!(transcript.height(false) > original);
        source.clear();
        transcript.sync(4, &source, &[], None);
        assert_eq!(transcript.height(false), 0);
    }
}
