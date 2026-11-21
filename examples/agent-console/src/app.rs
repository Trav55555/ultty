//! The transcript: turns agent events into what a [`Screen`] shows.
//!
//! - A prompt is printed as `> ` lines between blank lines.
//! - Prose is printed a line at a time, as Markdown styled in place
//!   ([`Markdown`]), once the line is complete; the line still being typed
//!   is a live line. A tool call, a diff or an approval ends the prose
//!   before it, and is set off from prose by blank lines.
//! - A tool call is a block of [`TOOL_ROWS`] rows: a title with status and
//!   elapsed time, and the last lines of output. Closing it leaves that
//!   view as the record.
//! - A diff is a block holding the hunk, closed at once.
//! - An approval is a block of the summary and the choices, answered by an
//!   [`Approver`] and closed showing the answer.
//! - The latest status is the bottom live line, and is printed when the
//!   session ends.
//!
//! Open blocks are redrawn and the live lines shown only from
//! [`App::refresh`], which the replay calls when it is about to wait, so a
//! replay as fast as possible prints only finished text.

use std::collections::VecDeque;
use std::time::Duration;

use crate::event::{Event, EventKind, Stream, ToolStatus};
use crate::keys::{Key, Keys};
use crate::markdown::Markdown;
use crate::screen::{BlockId, Result, Screen};
use crate::style::{sanitize, Line, Tone};

/// Height of a tool call's block: the title and this many lines less one
/// of output.
pub const TOOL_ROWS: u16 = 8;

/// The most rows a diff's block takes; a longer diff ends in a line saying
/// how many lines are not shown.
pub const MAX_DIFF_ROWS: usize = 40;

/// How often an approval waiting for a key looks for one.
const KEY_POLL: Duration = Duration::from_millis(100);

/// How an approval is answered.
pub enum Approver {
    /// Pick the choice with this text (or this 1-based number) after
    /// showing the request for `delay`.
    Auto {
        /// The choice's text, any case, or its number.
        choice: String,
        /// How long the request stays on screen first.
        delay: Duration,
    },
    /// Ask the user, reading keys from `keys`.
    Ask(Box<dyn Keys>),
    /// Nobody can be asked: take the first choice and say so.
    First,
}

/// A running tool call.
struct Tool {
    call: String,
    block: BlockId,
    tool: String,
    title: String,
    started: u64,
    /// The last complete output lines, at most a block's worth.
    tail: VecDeque<(Stream, String)>,
    /// Output after the last newline, per stream.
    partial: [String; 2],
    lines: u64,
    dirty: bool,
}

impl Tool {
    fn append(&mut self, stream: Stream, text: &str) {
        let partial = &mut self.partial[stream_index(stream)];
        partial.push_str(text);
        while let Some(end) = partial.find('\n') {
            let line = sanitize(&partial[..end]).trim_end().to_string();
            partial.drain(..=end);
            self.tail.push_back((stream, line));
            if self.tail.len() > usize::from(TOOL_ROWS - 1) {
                self.tail.pop_front();
            }
            self.lines += 1;
        }
        self.dirty = true;
    }

    /// The block's lines: title and status, then the last output.
    fn view(&self, state: &ToolState) -> Vec<Line> {
        let (status, tone) = match *state {
            ToolState::Running { elapsed_ms } => {
                (format!("running, {}", seconds(elapsed_ms)), Tone::Muted)
            }
            ToolState::Done {
                status,
                exit_code,
                duration_ms,
            } => {
                let (word, tone) = match status {
                    ToolStatus::Ok => ("ok", Tone::Success),
                    ToolStatus::Error => ("failed", Tone::Error),
                };
                let exit = exit_code
                    .filter(|&code| code != 0)
                    .map(|code| format!(", exit {code}"))
                    .unwrap_or_default();
                let lines = match self.lines {
                    1 => ", 1 line".to_string(),
                    n => format!(", {n} lines"),
                };
                (
                    format!("{word}{exit}{lines}, {}", seconds(duration_ms)),
                    tone,
                )
            }
        };
        let mut lines = vec![
            Line::styled(format!("[{}] {}", self.tool, self.title), Tone::Accent)
                .bold()
                .push(" (", Tone::Muted)
                .push(status, tone)
                .push(")", Tone::Muted),
        ];
        let partial: Vec<(Stream, String)> = [Stream::Stdout, Stream::Stderr]
            .into_iter()
            .filter_map(|stream| {
                let text = sanitize(&self.partial[stream_index(stream)]);
                let text = text.trim_end();
                (!text.is_empty()).then(|| (stream, text.to_string()))
            })
            .collect();
        let shown: Vec<&(Stream, String)> = self.tail.iter().chain(&partial).collect();
        let skip = shown.len().saturating_sub(usize::from(TOOL_ROWS - 1));
        for (stream, text) in &shown[skip..] {
            let tone = match stream {
                Stream::Stdout => Tone::Plain,
                Stream::Stderr => Tone::Warn,
            };
            lines.push(Line::styled(text.clone(), tone));
        }
        lines
    }
}

/// Where a tool call stands.
enum ToolState {
    Running {
        elapsed_ms: u64,
    },
    Done {
        status: ToolStatus,
        exit_code: Option<i32>,
        duration_ms: u64,
    },
}

fn stream_index(stream: Stream) -> usize {
    match stream {
        Stream::Stdout => 0,
        Stream::Stderr => 1,
    }
}

fn seconds(ms: u64) -> String {
    format!("{}.{}s", ms / 1000, ms % 1000 / 100)
}

/// The agent console's transcript, drawn on `S`.
pub struct App<S> {
    screen: S,
    approver: Approver,
    sleep: fn(Duration),
    markdown: Markdown,
    /// Prose after the last newline.
    partial: String,
    tools: Vec<Tool>,
    status: Option<Line>,
    /// The time of the latest event, in ms.
    clock: u64,
    next_block: u32,
    printed_any: bool,
    last_blank: bool,
    /// A block ended: prose that follows starts after a blank line.
    gap: bool,
}

impl<S: Screen> App<S> {
    /// A transcript on `screen`, answering approvals with `approver` and
    /// waiting with `sleep` (`std::thread::sleep`, or nothing in tests).
    pub fn new(screen: S, approver: Approver, sleep: fn(Duration)) -> Self {
        Self {
            screen,
            approver,
            sleep,
            markdown: Markdown::new(),
            partial: String::new(),
            tools: Vec::new(),
            status: None,
            clock: 0,
            next_block: 1,
            printed_any: false,
            last_blank: true,
            gap: false,
        }
    }

    /// The screen, once the session is over.
    pub fn into_screen(self) -> S {
        self.screen
    }

    /// Show `event`.
    pub fn handle(&mut self, event: &Event) -> Result<()> {
        self.clock = self.clock.max(event.t);
        match &event.kind {
            EventKind::Prompt { text } => self.prompt(text),
            EventKind::TextDelta { text, .. } => self.prose(text),
            EventKind::TurnEnd { .. } => {
                self.end_prose()?;
                self.markdown.end();
                Ok(())
            }
            EventKind::ToolStart { call, tool, title } => self.tool_start(call, tool, title),
            EventKind::ToolOutput { call, stream, text } => {
                if let Some(tool) = self.tools.iter_mut().find(|t| &t.call == call) {
                    tool.append(*stream, text);
                }
                Ok(())
            }
            EventKind::ToolEnd {
                call,
                status,
                exit_code,
                duration_ms,
            } => self.tool_end(
                call,
                ToolState::Done {
                    status: *status,
                    exit_code: *exit_code,
                    duration_ms: *duration_ms,
                },
            ),
            EventKind::Diff { path, diff } => self.diff(path, diff),
            EventKind::Approval {
                summary, choices, ..
            } => self.approval(summary, choices),
            EventKind::Status {
                model,
                input_tokens,
                output_tokens,
                state,
                elapsed_ms,
            } => {
                self.status = Some(Line::styled(
                    sanitize(&format!(
                        "{model} | in {input_tokens} | out {output_tokens} | {state} | {}",
                        seconds(*elapsed_ms)
                    )),
                    Tone::Muted,
                ));
                Ok(())
            }
        }
    }

    /// The replay is about to wait `wait`: bring open blocks and the live
    /// lines up to date, then wait.
    pub fn idle(&mut self, wait: Duration) -> Result<()> {
        self.refresh()?;
        (self.sleep)(wait);
        Ok(())
    }

    /// Redraw the open blocks that changed and show the live lines: the
    /// prose line being typed and the status, either blank if the other is
    /// not; nothing while both are.
    pub fn refresh(&mut self) -> Result<()> {
        for i in 0..self.tools.len() {
            if self.tools[i].dirty {
                self.tools[i].dirty = false;
                let elapsed_ms = self.clock.saturating_sub(self.tools[i].started);
                let view = self.tools[i].view(&ToolState::Running { elapsed_ms });
                self.screen.update(self.tools[i].block, &view)?;
            }
        }
        let width = usize::from(self.screen.width().max(2));
        let typing = self
            .markdown
            .peek(&sanitize(&self.partial))
            .trim_end()
            .tail(width - 1);
        let status = self.status.clone().unwrap_or_default();
        if typing.spans.is_empty() && status.spans.is_empty() {
            return self.screen.live(&[]);
        }
        self.screen.live(&[typing, status])
    }

    /// End the session: finish the prose, close calls still running as
    /// they stand, and print the last status.
    pub fn finish(&mut self) -> Result<()> {
        self.end_prose()?;
        for tool in std::mem::take(&mut self.tools) {
            let elapsed_ms = self.clock.saturating_sub(tool.started);
            let view = tool.view(&ToolState::Running { elapsed_ms });
            self.screen.close(tool.block, &view)?;
        }
        self.screen.finish()?;
        if let Some(status) = self.status.take() {
            self.blank_line()?;
            self.print(&[status])?;
        }
        Ok(())
    }

    fn print(&mut self, lines: &[Line]) -> Result<()> {
        let Some(last) = lines.last() else {
            return Ok(());
        };
        self.last_blank = last.spans.is_empty();
        self.printed_any = true;
        self.screen.print(lines)
    }

    /// Print a blank line unless the last line printed was one.
    fn blank_line(&mut self) -> Result<()> {
        if self.printed_any && !self.last_blank {
            self.print(&[Line::new()])?;
        }
        Ok(())
    }

    /// A block was closed: it is the last thing in the transcript.
    fn block_closed(&mut self) {
        self.printed_any = true;
        self.last_blank = false;
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        id
    }

    fn prompt(&mut self, text: &str) -> Result<()> {
        self.end_prose()?;
        self.markdown.end();
        self.blank_line()?;
        let lines: Vec<Line> = text
            .lines()
            .map(|line| Line::styled(format!("> {}", sanitize(line)), Tone::Accent).bold())
            .map(Line::trim_end)
            .collect();
        self.print(&lines)?;
        self.print(&[Line::new()])?;
        self.gap = false;
        Ok(())
    }

    fn prose(&mut self, text: &str) -> Result<()> {
        self.partial.push_str(text);
        while let Some(end) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=end).collect();
            self.prose_line(&line[..end])?;
        }
        Ok(())
    }

    fn prose_line(&mut self, source: &str) -> Result<()> {
        if self.gap {
            self.blank_line()?;
            self.gap = false;
        }
        let line = self.markdown.line(&sanitize(source)).trim_end();
        self.print(&[line])
    }

    /// Print the line being typed as it stands; a block or a new turn
    /// follows.
    fn end_prose(&mut self) -> Result<()> {
        if self.partial.is_empty() {
            return Ok(());
        }
        let line = std::mem::take(&mut self.partial);
        self.prose_line(&line)
    }

    /// Prepare to open a block: end the prose, leave a blank line.
    fn before_block(&mut self) -> Result<BlockId> {
        self.end_prose()?;
        self.blank_line()?;
        self.gap = true;
        Ok(self.new_block())
    }

    fn tool_start(&mut self, call: &str, tool: &str, title: &str) -> Result<()> {
        let block = self.before_block()?;
        let tool = Tool {
            call: call.to_string(),
            block,
            tool: sanitize(tool),
            title: sanitize(title),
            started: self.clock,
            tail: VecDeque::new(),
            partial: Default::default(),
            lines: 0,
            dirty: false,
        };
        self.screen.open(
            block,
            TOOL_ROWS,
            &tool.view(&ToolState::Running { elapsed_ms: 0 }),
        )?;
        self.tools.push(tool);
        Ok(())
    }

    fn tool_end(&mut self, call: &str, state: ToolState) -> Result<()> {
        let Some(index) = self.tools.iter().position(|t| t.call == call) else {
            return Ok(());
        };
        let mut tool = self.tools.remove(index);
        for stream in [Stream::Stdout, Stream::Stderr] {
            if !tool.partial[stream_index(stream)].is_empty() {
                tool.append(stream, "\n");
            }
        }
        self.screen.close(tool.block, &tool.view(&state))?;
        self.block_closed();
        Ok(())
    }

    fn diff(&mut self, path: &str, diff: &str) -> Result<()> {
        let block = self.before_block()?;
        let mut lines =
            vec![Line::styled(format!("[diff] {}", sanitize(path)), Tone::Accent).bold()];
        let hunk: Vec<&str> = diff.lines().collect();
        let room = MAX_DIFF_ROWS - 1;
        let shown = if hunk.len() > room {
            room - 1
        } else {
            hunk.len()
        };
        for raw in &hunk[..shown] {
            let tone = if raw.starts_with("+++") || raw.starts_with("---") {
                Tone::Muted
            } else if raw.starts_with("@@") {
                Tone::Accent
            } else if raw.starts_with('+') {
                Tone::Added
            } else if raw.starts_with('-') {
                Tone::Removed
            } else {
                Tone::Plain
            };
            lines.push(Line::styled(sanitize(raw), tone).trim_end());
        }
        if shown < hunk.len() {
            lines.push(Line::styled(
                format!("... {} more lines", hunk.len() - shown),
                Tone::Muted,
            ));
        }
        let rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        self.screen.open(block, rows, &lines)?;
        self.screen.close(block, &lines)?;
        self.block_closed();
        Ok(())
    }

    fn approval(&mut self, summary: &str, choices: &[String]) -> Result<()> {
        let block = self.before_block()?;
        let summary = sanitize(summary);
        let choices: Vec<String> = choices.iter().map(|c| sanitize(c)).collect();
        if choices.is_empty() {
            let lines = [Line::styled(format!("[approve] {summary}"), Tone::Warn).bold()];
            self.screen.open(block, 1, &lines)?;
            self.screen.close(block, &lines)?;
            self.block_closed();
            return Ok(());
        }
        let rows = u16::try_from(choices.len() + 1).unwrap_or(u16::MAX);
        self.screen
            .open(block, rows, &approval_view(&summary, &choices, 0, None))?;
        let (answer, note) = self.answer(block, &summary, &choices)?;
        self.screen.close(
            block,
            &approval_view(&summary, &choices, answer, Some(answer)),
        )?;
        self.block_closed();
        if let Some(note) = note {
            self.print(&[Line::styled(note, Tone::Muted)])?;
        }
        Ok(())
    }

    /// The index of the choice picked, and a line to print about how.
    fn answer(
        &mut self,
        block: BlockId,
        summary: &str,
        choices: &[String],
    ) -> Result<(usize, Option<String>)> {
        match &mut self.approver {
            Approver::First => Ok((
                0,
                Some(format!("(no terminal to ask on; chose \"{}\")", choices[0])),
            )),
            Approver::Auto { choice, delay } => {
                let delay = *delay;
                let picked = pick(choices, choice);
                let note = match picked {
                    Some(_) => None,
                    None => Some(format!(
                        "(--auto-approve {choice:?} is not a choice; chose \"{}\")",
                        choices[0]
                    )),
                };
                self.refresh()?;
                (self.sleep)(delay);
                Ok((picked.unwrap_or(0), note))
            }
            Approver::Ask(_) => {
                let answer = self.ask(block, summary, choices);
                if let Approver::Ask(keys) = &mut self.approver {
                    keys.done();
                }
                Ok((answer?, None))
            }
        }
    }

    /// Wait for the user to pick one of `choices`, moving the selection in
    /// `block` as they go.
    fn ask(&mut self, block: BlockId, summary: &str, choices: &[String]) -> Result<usize> {
        let mut selected = 0;
        loop {
            self.refresh()?;
            let input = self.screen.take_input();
            let key = match &mut self.approver {
                Approver::Ask(keys) => keys.next_key(&input, KEY_POLL)?,
                Approver::Auto { .. } | Approver::First => Some(Key::Enter),
            };
            let moved = match key {
                None => continue,
                Some(Key::Interrupt) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "interrupted at an approval",
                    )
                    .into())
                }
                Some(Key::Enter) => return Ok(selected),
                Some(Key::Digit(n)) if (1..=choices.len()).contains(&n) => return Ok(n - 1),
                Some(Key::Up) => selected.checked_sub(1),
                Some(Key::Down) => (selected + 1 < choices.len()).then_some(selected + 1),
                Some(_) => None,
            };
            if let Some(moved) = moved {
                selected = moved;
                self.screen
                    .update(block, &approval_view(summary, choices, selected, None))?;
            }
        }
    }
}

/// The index of the choice `wanted` names: its text in any case, or its
/// 1-based number.
fn pick(choices: &[String], wanted: &str) -> Option<usize> {
    choices
        .iter()
        .position(|c| c.eq_ignore_ascii_case(wanted))
        .or_else(|| {
            wanted
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=choices.len()).contains(n))
                .map(|n| n - 1)
        })
}

/// An approval's lines: the summary, then the choices numbered, the
/// selected one marked. Once `answered`, the title says which.
fn approval_view(
    summary: &str,
    choices: &[String],
    selected: usize,
    answered: Option<usize>,
) -> Vec<Line> {
    let mut title = Line::styled(format!("[approve] {summary}"), Tone::Warn).bold();
    if let Some(answer) = answered {
        title = title
            .push(" (", Tone::Muted)
            .push(choices[answer].clone(), Tone::Accent)
            .push(")", Tone::Muted);
    }
    let mut lines = vec![title];
    for (i, choice) in choices.iter().enumerate() {
        let line = if i == selected {
            Line::styled(format!("> {}. {choice}", i + 1), Tone::Accent).bold()
        } else {
            Line::styled(format!("  {}. {choice}", i + 1), Tone::Plain)
        };
        lines.push(line.trim_end());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_matches_text_in_any_case_or_a_number() {
        let choices = ["Allow once".to_string(), "Deny".to_string()];
        assert_eq!(pick(&choices, "deny"), Some(1));
        assert_eq!(pick(&choices, "1"), Some(0));
        assert_eq!(pick(&choices, "3"), None);
        assert_eq!(pick(&choices, "maybe"), None);
    }

    #[test]
    fn test_tool_keeps_only_a_block_of_output_and_counts_every_line() {
        let mut tool = Tool {
            call: "c".into(),
            block: BlockId(1),
            tool: "bash".into(),
            title: "t".into(),
            started: 0,
            tail: VecDeque::new(),
            partial: Default::default(),
            lines: 0,
            dirty: false,
        };
        for i in 0..100 {
            tool.append(Stream::Stdout, &format!("line {i}\n"));
        }
        tool.append(Stream::Stderr, "half");
        assert_eq!(tool.tail.len(), usize::from(TOOL_ROWS - 1));
        assert_eq!(tool.lines, 100);
        let view = tool.view(&ToolState::Running { elapsed_ms: 1234 });
        let text: Vec<String> = view.iter().map(Line::text).collect();
        assert_eq!(text[0], "[bash] t (running, 1.2s)");
        assert_eq!(text.len(), usize::from(TOOL_ROWS));
        assert_eq!(text[1], "line 94");
        assert_eq!(text[7], "half");
        assert_eq!(view[7].spans[0].tone, Tone::Warn);
    }

    #[test]
    fn test_finished_tool_title_says_how_it_ended() {
        let tool = Tool {
            call: "c".into(),
            block: BlockId(1),
            tool: "bash".into(),
            title: "cargo test".into(),
            started: 0,
            tail: VecDeque::new(),
            partial: Default::default(),
            lines: 3,
            dirty: false,
        };
        let done = |status, exit_code| ToolState::Done {
            status,
            exit_code,
            duration_ms: 3900,
        };
        assert_eq!(
            tool.view(&done(ToolStatus::Error, Some(101)))[0].text(),
            "[bash] cargo test (failed, exit 101, 3 lines, 3.9s)"
        );
        assert_eq!(
            tool.view(&done(ToolStatus::Ok, Some(0)))[0].text(),
            "[bash] cargo test (ok, 3 lines, 3.9s)"
        );
    }

    #[test]
    fn test_approval_view_marks_the_selection_and_the_answer() {
        let choices = ["Allow once".to_string(), "Deny".to_string()];
        let text = |lines: Vec<Line>| lines.iter().map(Line::text).collect::<Vec<_>>();
        assert_eq!(
            text(approval_view("rm x", &choices, 1, None)),
            ["[approve] rm x", "  1. Allow once", "> 2. Deny"]
        );
        assert_eq!(
            text(approval_view("rm x", &choices, 0, Some(0)))[0],
            "[approve] rm x (Allow once)"
        );
    }
}
