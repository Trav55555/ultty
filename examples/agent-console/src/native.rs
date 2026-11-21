//! Native mode shows live output in an inline card at the end of the transcript.
//! The card is retired before finished text is printed, so a completed block
//! keeps its source through terminal reflow and copy.
//!
//! [`Cards`] is the seam between the real [`Console`] and the renderer tests.

use ulttui::inline::Console;
use ulttui::prelude::{Color, Column, Style};
use ulttui::widgets::RichText;

use crate::screen::{BlockId, Result, Screen};
use crate::style::Line;

/// Background of the cards that hold blocks.
const BLOCK_BG: Color = Color::Rgb(28, 32, 44);

/// What a card is for, which decides how it looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    /// A tool call, a diff or an approval.
    Block,
    /// The live lines at the bottom.
    Live,
}

/// Inline cards and the text around them: the part of
/// [`ulttui::inline::Console`] native mode uses.
pub trait Cards {
    /// An open card.
    type Card;

    /// The terminal's width in columns.
    fn width(&self) -> u16;

    /// The maximum number of rows the live card may occupy.
    fn live_row_limit(&self) -> u16;

    /// Print `text` into the transcript; `\n` ends a line.
    fn print(&mut self, text: &str) -> Result<()>;

    /// Open a card `rows` high on the next line, showing `lines`.
    fn open(&mut self, rows: u16, lines: &[Line], kind: CardKind) -> Result<Self::Card>;

    /// Show `lines` in `card`.
    fn update(&mut self, card: &Self::Card, lines: &[Line], kind: CardKind) -> Result<()>;

    /// Close `card`, freezing it into its rows.
    fn close(&mut self, card: Self::Card) -> Result<()>;

    /// Keys the session read from the tty (in-band only).
    fn take_input(&mut self) -> Vec<u8>;

    /// Read and log what the terminal reported about cards, so reports do
    /// not pile up over a long session.
    fn drain_events(&mut self) -> Result<()>;
}

/// The view a card shows: one rich-text row per line.
pub fn card_view(lines: &[Line], kind: CardKind) -> Column {
    let mut column = Column::new();
    if kind == CardKind::Block {
        column = column.bg(BLOCK_BG);
    }
    for line in lines {
        let mut text = RichText::new().fill_width();
        for span in &line.spans {
            let style = Style {
                fg: span.tone.palette_index().map(Color::Indexed),
                bold: span.bold,
                italic: span.italic,
                ..Style::default()
            };
            text = text.span(span.text.clone(), style);
        }
        column = column.child(text);
    }
    column
}

impl Cards for Console {
    type Card = ulttui::inline::Card;

    fn width(&self) -> u16 {
        Console::width(self)
    }

    fn live_row_limit(&self) -> u16 {
        self.tail_card_rows()
    }

    fn print(&mut self, text: &str) -> Result<()> {
        Ok(Console::print(self, text)?)
    }

    fn open(&mut self, rows: u16, lines: &[Line], kind: CardKind) -> Result<Self::Card> {
        Ok(Console::open(self, rows, &card_view(lines, kind))?)
    }

    fn update(&mut self, card: &Self::Card, lines: &[Line], kind: CardKind) -> Result<()> {
        Ok(Console::update(self, card, &card_view(lines, kind))?)
    }

    fn close(&mut self, card: Self::Card) -> Result<()> {
        Ok(Console::close(self, card)?)
    }

    fn take_input(&mut self) -> Vec<u8> {
        Console::take_input(self)
    }

    fn drain_events(&mut self) -> Result<()> {
        for event in self.poll_events()? {
            tracing::debug!("card {} {:?}", event.card.0, event.kind);
        }
        Ok(())
    }
}

struct Block {
    id: BlockId,
    rows: usize,
    lines: Vec<Line>,
}

struct LiveCard<C> {
    card: C,
    kind: CardKind,
    lines: Vec<Line>,
}

/// A transcript followed by an inline card for the live output.
pub struct NativeScreen<C: Cards> {
    cards: C,
    blocks: Vec<Block>,
    live: Option<LiveCard<C::Card>>,
}

impl<C: Cards> NativeScreen<C> {
    /// Native mode on `cards`.
    pub fn new(cards: C) -> Self {
        Self {
            cards,
            blocks: Vec::new(),
            live: None,
        }
    }

    /// The cards, for tests that inspect what was sent.
    pub fn into_inner(self) -> C {
        self.cards
    }

    fn retire_live(&mut self) -> Result<()> {
        if let Some(live) = self.live.take() {
            self.cards.close(live.card)?;
            self.cards
                .print(&format!("\x1b[{}A\r\x1b[2K\x1b[J\r", live.lines.len()))?;
            self.cards.drain_events()?;
        }
        Ok(())
    }
}

impl<C: Cards> Screen for NativeScreen<C> {
    fn width(&self) -> u16 {
        self.cards.width()
    }

    fn print(&mut self, lines: &[Line]) -> Result<()> {
        self.retire_live()?;
        let mut text = String::new();
        for line in lines {
            text.push_str(&line.to_ansi());
            text.push('\n');
        }
        if !text.is_empty() {
            self.cards.print(&text)?;
        }
        Ok(())
    }

    fn open(&mut self, id: BlockId, rows: u16, lines: &[Line]) -> Result<()> {
        self.blocks.push(Block {
            id,
            rows: usize::from(rows),
            lines: lines.to_vec(),
        });
        Ok(())
    }

    fn update(&mut self, id: BlockId, lines: &[Line]) -> Result<()> {
        if let Some(block) = self.blocks.iter_mut().find(|block| block.id == id) {
            block.lines = lines.to_vec();
        }
        Ok(())
    }

    fn close(&mut self, id: BlockId, lines: &[Line]) -> Result<()> {
        if let Some(index) = self.blocks.iter().position(|block| block.id == id) {
            self.blocks.remove(index);
            self.print(lines)?;
        }
        Ok(())
    }

    fn live(&mut self, status: &[Line]) -> Result<()> {
        let limit = usize::from(self.cards.live_row_limit());
        let mut lines: Vec<_> = status.iter().rev().take(limit).cloned().collect();
        for block in self.blocks.iter().rev() {
            let count = block.rows.min(limit - lines.len());
            lines.extend(
                (block.rows - count..block.rows)
                    .rev()
                    .map(|row| block.lines.get(row).cloned().unwrap_or_default()),
            );
            if lines.len() == limit {
                break;
            }
        }
        lines.reverse();
        let kind = if self.blocks.is_empty() {
            CardKind::Live
        } else {
            CardKind::Block
        };
        let rows = lines.len();

        if let Some(live) = &mut self.live {
            if live.kind == kind && live.lines.len() == rows {
                if live.lines != lines {
                    self.cards.update(&live.card, &lines, kind)?;
                    live.lines = lines;
                }
                return Ok(());
            }
        }
        self.retire_live()?;
        if rows > 0 {
            let card = self.cards.open(rows as u16, &lines, kind)?;
            self.live = Some(LiveCard { card, kind, lines });
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.retire_live()
    }

    fn take_input(&mut self) -> Vec<u8> {
        self.cards.take_input()
    }
}
