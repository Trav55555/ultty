//! ANSI mode: what a careful agent CLI does in any terminal.
//!
//! Finished lines are printed into the normal scrollback, whole, and the
//! terminal wraps them. Open blocks and the live lines form one live region
//! at the bottom, redrawn in place: the cursor goes back up over it, the
//! region is erased and printed again, each redraw inside a synchronized
//! update (DEC mode 2026) so the terminal never shows it half drawn. A
//! closed block is printed as finished text. The alternate screen is never
//! used.
//!
//! When stdout is not a terminal nothing is redrawn and nothing is styled:
//! finished lines and closed blocks are written as plain text, once.

use std::io::Write;

use crate::screen::{BlockId, Result, Screen};
use crate::style::Line;

/// Begin a synchronized update.
const BEGIN_SYNC: &str = "\x1b[?2026h";
/// End a synchronized update.
const END_SYNC: &str = "\x1b[?2026l";

/// Where the terminal's size comes from.
enum Size {
    /// Asked each redraw, falling back to 80 x 24.
    Terminal,
    /// Fixed: columns, lines.
    Fixed(u16, u16),
}

/// A [`Screen`] that writes ANSI text to `out`.
pub struct AnsiScreen<W> {
    out: W,
    /// `None` when `out` is not a terminal: plain text, no redraws.
    size: Option<Size>,
    blocks: Vec<(BlockId, Vec<Line>)>,
    live: Vec<Line>,
    /// Whether the live region is drawn at all; set by the first
    /// [`Screen::live`] and cleared by [`Screen::finish`].
    shown: bool,
    /// Rows of the live region on screen now. The cursor is at the end of
    /// the last one.
    drawn: usize,
}

impl<W: Write> AnsiScreen<W> {
    /// ANSI mode on the terminal `out`, asking it for its size.
    pub fn terminal(out: W) -> Self {
        Self::with_size(out, Some(Size::Terminal))
    }

    /// ANSI mode on `out` as if it were a terminal of `cols` x `lines`.
    pub fn fixed(out: W, cols: u16, lines: u16) -> Self {
        Self::with_size(out, Some(Size::Fixed(cols, lines)))
    }

    /// Plain text on `out`, a pipe or a file.
    pub fn pipe(out: W) -> Self {
        Self::with_size(out, None)
    }

    fn with_size(out: W, size: Option<Size>) -> Self {
        Self {
            out,
            size,
            blocks: Vec::new(),
            live: Vec::new(),
            shown: false,
            drawn: 0,
        }
    }

    /// The writer, for tests that render into memory.
    pub fn into_inner(self) -> W {
        self.out
    }

    fn size(&self) -> (u16, u16) {
        match self.size {
            Some(Size::Fixed(cols, lines)) => (cols, lines),
            Some(Size::Terminal) | None => crossterm::terminal::size().unwrap_or((80, 24)),
        }
    }

    /// The live region's lines, clipped to the width and to one row less
    /// than the screen (keeping the bottom), so redrawing never needs to
    /// reach into scrollback.
    fn region(&self) -> Vec<Line> {
        if !self.shown {
            return Vec::new();
        }
        let (cols, rows) = self.size();
        let lines: Vec<Line> = self
            .blocks
            .iter()
            .flat_map(|(_, lines)| lines)
            .chain(&self.live)
            .map(|line| line.clip(usize::from(cols)))
            .collect();
        let keep = usize::from(rows.saturating_sub(1)).max(1);
        lines[lines.len().saturating_sub(keep)..].to_vec()
    }

    /// Print `finished` lines above the live region and draw the region
    /// again, in one synchronized update. In a pipe, write `finished` only.
    fn redraw(&mut self, finished: &[Line]) -> Result<()> {
        if self.size.is_none() {
            let mut text = String::new();
            for line in finished {
                text.push_str(&line.text());
                text.push('\n');
            }
            self.out.write_all(text.as_bytes())?;
            return Ok(self.out.flush()?);
        }
        let mut text = String::from(BEGIN_SYNC);
        if self.drawn > 0 {
            text.push('\r');
            if self.drawn > 1 {
                text.push_str(&format!("\x1b[{}A", self.drawn - 1));
            }
            text.push_str("\x1b[J");
        }
        for line in finished {
            text.push_str(&line.to_ansi());
            text.push_str("\r\n");
        }
        let region = self.region();
        for (i, line) in region.iter().enumerate() {
            if i > 0 {
                text.push_str("\r\n");
            }
            text.push_str(&line.to_ansi());
        }
        text.push_str(END_SYNC);
        self.out.write_all(text.as_bytes())?;
        self.out.flush()?;
        self.drawn = region.len();
        Ok(())
    }

    fn block_mut(&mut self, id: BlockId) -> Option<&mut Vec<Line>> {
        self.blocks
            .iter_mut()
            .find(|(block, _)| *block == id)
            .map(|(_, lines)| lines)
    }
}

impl<W: Write> Screen for AnsiScreen<W> {
    fn width(&self) -> u16 {
        self.size().0
    }

    fn print(&mut self, lines: &[Line]) -> Result<()> {
        self.redraw(lines)
    }

    fn open(&mut self, id: BlockId, _rows: u16, lines: &[Line]) -> Result<()> {
        self.blocks.push((id, lines.to_vec()));
        if self.shown {
            self.redraw(&[])?;
        }
        Ok(())
    }

    fn update(&mut self, id: BlockId, lines: &[Line]) -> Result<()> {
        let Some(block) = self.block_mut(id) else {
            return Ok(());
        };
        if block.as_slice() == lines {
            return Ok(());
        }
        *block = lines.to_vec();
        if self.shown {
            self.redraw(&[])?;
        }
        Ok(())
    }

    fn close(&mut self, id: BlockId, lines: &[Line]) -> Result<()> {
        self.blocks.retain(|(block, _)| *block != id);
        self.redraw(lines)
    }

    fn live(&mut self, lines: &[Line]) -> Result<()> {
        if self.shown && self.live == lines {
            return Ok(());
        }
        self.live = lines.to_vec();
        self.shown = true;
        self.redraw(&[])
    }

    fn finish(&mut self) -> Result<()> {
        if self.drawn == 0 {
            return Ok(());
        }
        self.shown = false;
        self.redraw(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Tone;

    fn text(lines: &[&str]) -> Vec<Line> {
        lines
            .iter()
            .map(|l| Line::styled(*l, Tone::Plain))
            .collect()
    }

    fn output(screen: AnsiScreen<Vec<u8>>) -> String {
        String::from_utf8(screen.into_inner()).unwrap()
    }

    #[test]
    fn test_nothing_is_redrawn_before_the_first_live_call() {
        let mut screen = AnsiScreen::fixed(Vec::new(), 20, 10);
        screen.print(&text(&["one"])).unwrap();
        screen.open(BlockId(1), 3, &text(&["[tool] a"])).unwrap();
        screen.update(BlockId(1), &text(&["[tool] b"])).unwrap();
        screen.close(BlockId(1), &text(&["[tool] c"])).unwrap();
        assert_eq!(
            output(screen),
            "\x1b[?2026hone\r\n\x1b[?2026l\x1b[?2026h[tool] c\r\n\x1b[?2026l"
        );
    }

    #[test]
    fn test_the_live_region_is_erased_and_redrawn_around_printed_lines() {
        let mut screen = AnsiScreen::fixed(Vec::new(), 20, 10);
        screen
            .open(BlockId(1), 3, &text(&["[tool] a", "out"]))
            .unwrap();
        screen.live(&text(&["status"])).unwrap();
        screen.print(&text(&["done"])).unwrap();
        screen.finish().unwrap();
        assert_eq!(
            output(screen),
            concat!(
                "\x1b[?2026h[tool] a\r\nout\r\nstatus\x1b[?2026l",
                "\x1b[?2026h\r\x1b[2A\x1b[Jdone\r\n[tool] a\r\nout\r\nstatus\x1b[?2026l",
                "\x1b[?2026h\r\x1b[2A\x1b[J\x1b[?2026l",
            )
        );
    }

    #[test]
    fn test_live_lines_are_clipped_and_the_region_kept_shorter_than_the_screen() {
        let mut screen = AnsiScreen::fixed(Vec::new(), 4, 3);
        screen.live(&text(&["abcdef", "b", "c"])).unwrap();
        assert_eq!(output(screen), "\x1b[?2026hb\r\nc\x1b[?2026l");
        let mut screen = AnsiScreen::fixed(Vec::new(), 4, 5);
        screen.live(&text(&["abcdef"])).unwrap();
        assert_eq!(output(screen), "\x1b[?2026habcd\x1b[?2026l");
    }

    #[test]
    fn test_a_pipe_gets_plain_finished_text_only() {
        let mut screen = AnsiScreen::pipe(Vec::new());
        screen.live(&text(&["status"])).unwrap();
        screen
            .print(&[Line::styled("styled", Tone::Accent)])
            .unwrap();
        screen.open(BlockId(1), 2, &text(&["live"])).unwrap();
        screen.close(BlockId(1), &text(&["final"])).unwrap();
        screen.finish().unwrap();
        assert_eq!(output(screen), "styled\nfinal\n");
    }
}
