//! Native mode: inline cards in Ultty (ADR 0020).
//!
//! Finished lines are printed into the transcript as styled text, so they
//! scroll, wrap, re-wrap and copy like any terminal output. Each block is
//! an inline card: drawn natively, updated by diff, frozen into its rows as
//! text when closed. The live lines (the prose line being typed and the
//! status) are a card of their own at the bottom.
//!
//! A card's height is fixed and anything printed goes below it, so the
//! live card cannot stay under the transcript as it grows. Before anything
//! else is printed or opened it is **retired**: closed, and its rows
//! erased (`EL 2` on the first, `ED 0` below) so the new text takes their
//! place. Erasing a row drops its anchor tag, so whichever comes first, the
//! terminal's last write of the card or the erase, the rows end up holding
//! the new text. The live card is opened again, under the new text, when
//! the console is next idle. The rows it covers are anchored and never
//! re-wrap, so moving the cursor up over them is exact at any width.
//!
//! [`Cards`] is the seam: [`Console`] in the app, a recorder in tests.

use std::collections::HashMap;

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

/// A [`Screen`] of inline cards.
pub struct NativeScreen<C: Cards> {
    cards: C,
    blocks: HashMap<BlockId, C::Card>,
    /// The live card and its height, while it is open.
    live: Option<(C::Card, u16, Vec<Line>)>,
}

impl<C: Cards> NativeScreen<C> {
    /// Native mode on `cards`.
    pub fn new(cards: C) -> Self {
        Self {
            cards,
            blocks: HashMap::new(),
            live: None,
        }
    }

    /// The cards, for tests that inspect what was sent.
    pub fn into_inner(self) -> C {
        self.cards
    }

    /// Close the live card, if open, and print `text` over its rows.
    ///
    /// With no text the cursor goes up one more line and ends the line, so
    /// it is back at the start of the card's first row with the console
    /// knowing it is at the start of a line; otherwise the next card would
    /// open one line lower.
    fn retire_then_print(&mut self, text: &str) -> Result<()> {
        let mut out = String::new();
        if let Some((card, rows, _)) = self.live.take() {
            self.cards.close(card)?;
            out.push_str(&format!("\x1b[{rows}A\r\x1b[2K\x1b[J"));
            if text.is_empty() {
                out.push_str("\x1b[A\n");
            }
            self.cards.drain_events()?;
        }
        out.push_str(text);
        if out.is_empty() {
            return Ok(());
        }
        self.cards.print(&out)
    }
}

/// `lines` as styled text, each ending its line.
fn printed(lines: &[Line]) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(&line.to_ansi());
        text.push('\n');
    }
    text
}

impl<C: Cards> Screen for NativeScreen<C> {
    fn width(&self) -> u16 {
        self.cards.width()
    }

    fn print(&mut self, lines: &[Line]) -> Result<()> {
        self.retire_then_print(&printed(lines))
    }

    fn open(&mut self, id: BlockId, rows: u16, lines: &[Line]) -> Result<()> {
        self.retire_then_print("")?;
        let card = self.cards.open(rows, lines, CardKind::Block)?;
        self.blocks.insert(id, card);
        Ok(())
    }

    fn update(&mut self, id: BlockId, lines: &[Line]) -> Result<()> {
        match self.blocks.get(&id) {
            Some(card) => self.cards.update(card, lines, CardKind::Block),
            None => Ok(()),
        }
    }

    fn close(&mut self, id: BlockId, lines: &[Line]) -> Result<()> {
        let Some(card) = self.blocks.remove(&id) else {
            return Ok(());
        };
        self.cards.update(&card, lines, CardKind::Block)?;
        self.cards.close(card)
    }

    fn live(&mut self, lines: &[Line]) -> Result<()> {
        let rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        match &self.live {
            Some((_, open_rows, shown)) if *open_rows == rows => {
                if shown.as_slice() != lines {
                    let (card, _, shown) = self.live.as_mut().expect("matched above");
                    self.cards.update(card, lines, CardKind::Live)?;
                    *shown = lines.to_vec();
                }
                Ok(())
            }
            _ => {
                self.retire_then_print("")?;
                if rows > 0 {
                    let card = self.cards.open(rows, lines, CardKind::Live)?;
                    self.live = Some((card, rows, lines.to_vec()));
                }
                Ok(())
            }
        }
    }

    fn finish(&mut self) -> Result<()> {
        self.retire_then_print("")
    }

    fn take_input(&mut self) -> Vec<u8> {
        self.cards.take_input()
    }
}
