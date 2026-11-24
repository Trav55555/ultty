//! Drawing the command palette: a panel near the top of the window with
//! the query, then the matching actions, each with its key at the right.
//!
//! Its colors come from the theme, mixed between the background and the
//! foreground the way the main menu's are, so it reads in light and dark
//! themes alike. Sizes are in cells, so it follows the font size and the
//! display scale. Text goes through [`super::text::TextRenderer::append_cell_text`],
//! which reuses the grid's shaping cache: a frame with the palette open
//! shapes nothing it has drawn before.

use super::tabbar::mix;
use super::{Renderer, Rgba, Vertex};
use crate::command_palette::CommandPalette;

/// Most rows listed at once; more scroll.
const MAX_VISIBLE_ROWS: usize = 12;
/// Widest the panel gets, in cells.
const MAX_WIDTH_CELLS: f32 = 72.0;
const PROMPT: &str = "> ";
const PLACEHOLDER: &str = "Type a command";
const NO_MATCHES: &str = "No matching commands";

/// Where the palette's parts go, in window pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PaletteLayout {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Space between the panel's edge and its text, on every side.
    pub padding: f32,
    pub row_height: f32,
    /// Top of the query line.
    pub query_y: f32,
    /// Top of the first listed row.
    pub list_y: f32,
    /// Rows listed at once (at least one, for the "no matches" line).
    pub visible_rows: usize,
    /// Index of the first listed row: the list scrolls to keep the
    /// selected row in view.
    pub first_row: usize,
}

impl PaletteLayout {
    /// The palette in a `window` (width, height) whose content starts at
    /// `top`, with cells of `cell` (width, height), listing `rows` rows
    /// with `selected` selected.
    pub(super) fn new(
        (window_width, window_height): (f32, f32),
        top: f32,
        (cell_width, cell_height): (f32, f32),
        rows: usize,
        selected: usize,
    ) -> Self {
        let margin = (2.0 * cell_width).min(window_width * 0.05);
        let width = (window_width - 2.0 * margin)
            .min(MAX_WIDTH_CELLS * cell_width)
            .max(0.0);
        let x = ((window_width - width) / 2.0).round();
        let y = (top + cell_height).min(window_height * 0.25).round();
        let padding = (cell_width).round();
        let row_height = (cell_height * 1.5).round();
        let rule_gap = (cell_height * 0.5).round();

        // The query line and the rule under it, then the rows.
        let fixed = 2.0 * padding + row_height + rule_gap;
        let room = window_height - y - cell_height - fixed;
        let fit = (room / row_height).floor().max(1.0) as usize;
        let visible_rows = rows.clamp(1, MAX_VISIBLE_ROWS).min(fit);
        let first_row = if selected < visible_rows {
            0
        } else {
            selected + 1 - visible_rows
        };

        let query_y = y + padding;
        let list_y = query_y + row_height + rule_gap;
        let height = fixed + visible_rows as f32 * row_height;
        Self {
            x,
            y,
            width,
            height,
            padding,
            row_height,
            query_y,
            list_y,
            visible_rows,
            first_row,
        }
    }
}

/// Cells from the start of the panel's text area to the end of the typed
/// query (where the caret and an input method's candidates belong), in a
/// text area `inner_cells` wide, with `list_len` matches of which
/// `visible_rows` show (a match count takes the right edge when the list
/// scrolls). A long query shows only its tail.
fn caret_cells(
    query_chars: usize,
    inner_cells: usize,
    list_len: usize,
    visible_rows: usize,
    selected: usize,
) -> usize {
    let prompt = PROMPT.chars().count();
    let count = if list_len > visible_rows {
        format!("{} of {}", selected + 1, list_len).chars().count() + 1
    } else {
        0
    };
    let room = inner_cells.saturating_sub(prompt + count + 1);
    prompt + query_chars.min(room)
}

/// The palette's colors, from the theme.
struct PaletteColors {
    panel: Rgba,
    border: Rgba,
    rule: Rgba,
    selected: Rgba,
    text: Rgba,
    muted: Rgba,
    matched: Rgba,
}

impl PaletteColors {
    fn new(scheme: &super::ColorScheme) -> Self {
        let bg = Rgba {
            a: 1.0,
            ..scheme.background
        };
        let fg = scheme.foreground;
        let accent = scheme.cursor;
        Self {
            panel: mix(bg, fg, 0.05),
            border: mix(bg, fg, 0.25),
            rule: mix(bg, fg, 0.14),
            selected: mix(mix(bg, fg, 0.05), accent, 0.22),
            text: fg,
            muted: mix(bg, fg, 0.6),
            // The theme's blue, pulled toward the text color so it keeps
            // its contrast on either background.
            matched: mix(scheme.named[4], fg, 0.3),
        }
    }
}

impl Renderer {
    /// The query caret's cell as `[x, y, width, height]` in window pixels,
    /// for the input method; `None` while the palette is closed.
    pub fn command_palette_caret_cell(&self, palette: &CommandPalette) -> Option<[f32; 4]> {
        if !palette.is_open() {
            return None;
        }
        let (cell_width, cell_height) = (self.cell_width, self.cell_height);
        let window = (self.config.width as f32, self.config.height as f32);
        let list_len = palette.rows().len();
        let layout = PaletteLayout::new(
            window,
            self.tab_bar_height,
            (cell_width, cell_height),
            list_len,
            palette.selected(),
        );
        let inner_cells = ((layout.width - 2.0 * layout.padding) / cell_width)
            .floor()
            .max(0.0) as usize;
        let cells = caret_cells(
            palette.query().chars().count(),
            inner_cells,
            list_len,
            layout.visible_rows,
            palette.selected(),
        );
        let text_offset = ((layout.row_height - cell_height) / 2.0).round();
        Some([
            layout.x + layout.padding + cells as f32 * cell_width,
            layout.query_y + text_offset,
            cell_width,
            cell_height,
        ])
    }

    /// Background and text vertices of the open palette.
    pub(super) fn build_command_palette_vertices(
        &mut self,
        palette: &CommandPalette,
    ) -> (Vec<Vertex>, Vec<u32>, Vec<Vertex>, Vec<u32>) {
        let (mut bg_v, mut bg_i, mut text_v, mut text_i) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        if !palette.is_open() {
            return (bg_v, bg_i, text_v, text_i);
        }
        let (cell_width, cell_height) = (self.cell_width, self.cell_height);
        let window = (self.config.width as f32, self.config.height as f32);
        let row_count = palette.rows().len();
        let layout = PaletteLayout::new(
            window,
            self.tab_bar_height,
            (cell_width, cell_height),
            row_count,
            palette.selected(),
        );
        let colors = PaletteColors::new(&self.color_scheme);
        let radius = (cell_height * 0.4).round();
        let line = (cell_height / 16.0).round().max(1.0);

        // Dim what is under it, as the other modal overlays do, but less:
        // the palette is a quick pick, not a page to read.
        self.emit_rect(
            &mut bg_v,
            &mut bg_i,
            0.0,
            0.0,
            window.0,
            window.1,
            Rgba::new(0.0, 0.0, 0.0, 0.35),
        );
        self.emit_rounded_rect(
            &mut bg_v,
            &mut bg_i,
            layout.x,
            layout.y,
            layout.width,
            layout.height,
            radius + line,
            colors.border,
        );
        self.emit_rounded_rect(
            &mut bg_v,
            &mut bg_i,
            layout.x + line,
            layout.y + line,
            layout.width - 2.0 * line,
            layout.height - 2.0 * line,
            radius,
            colors.panel,
        );

        let inner_x = layout.x + layout.padding;
        let inner_width = layout.width - 2.0 * layout.padding;
        let inner_cells = (inner_width / cell_width).floor().max(0.0) as usize;
        let text_offset = ((layout.row_height - cell_height) / 2.0).round();

        // The query line: a prompt, the query and a caret, and the match
        // count at the right when the list scrolls.
        let query_text_y = layout.query_y + text_offset;
        let prompt_cells = self.text_renderer.append_cell_text(
            PROMPT,
            inner_x,
            query_text_y,
            inner_cells,
            |_| colors.matched,
            &mut text_v,
            &mut text_i,
        );
        let count = (row_count > layout.visible_rows)
            .then(|| format!("{} of {}", palette.selected() + 1, row_count));
        let count_cells = count.as_ref().map_or(0, |c| c.chars().count() + 1);
        let query_x = inner_x + prompt_cells as f32 * cell_width;
        let query_room = inner_cells.saturating_sub(prompt_cells + count_cells + 1);
        let query = palette.query();
        let typed = if query.is_empty() {
            self.text_renderer.append_cell_text(
                PLACEHOLDER,
                query_x + cell_width,
                query_text_y,
                query_room.saturating_sub(1),
                |_| colors.muted,
                &mut text_v,
                &mut text_i,
            );
            0
        } else {
            // Keep the end of a long query, where the typing is, in view.
            let chars = query.chars().count();
            let skip = chars.saturating_sub(query_room);
            let shown: String = query.chars().skip(skip).collect();
            self.text_renderer.append_cell_text(
                &shown,
                query_x,
                query_text_y,
                query_room,
                |_| colors.text,
                &mut text_v,
                &mut text_i,
            )
        };
        self.emit_rect(
            &mut bg_v,
            &mut bg_i,
            query_x + typed as f32 * cell_width,
            query_text_y,
            (cell_width / 8.0).round().max(2.0),
            cell_height,
            colors.text,
        );
        if let Some(count) = &count {
            let cells = count.chars().count();
            self.text_renderer.append_cell_text(
                count,
                inner_x + (inner_cells - cells.min(inner_cells)) as f32 * cell_width,
                query_text_y,
                cells,
                |_| colors.muted,
                &mut text_v,
                &mut text_i,
            );
        }
        self.emit_rect(
            &mut bg_v,
            &mut bg_i,
            layout.x + line,
            layout.list_y - (layout.list_y - layout.query_y - layout.row_height) / 2.0,
            layout.width - 2.0 * line,
            line,
            colors.rule,
        );

        if row_count == 0 {
            self.text_renderer.append_cell_text(
                NO_MATCHES,
                inner_x,
                layout.list_y + text_offset,
                inner_cells,
                |_| colors.muted,
                &mut text_v,
                &mut text_i,
            );
            return (bg_v, bg_i, text_v, text_i);
        }

        let underline = line;
        for (index, row) in palette
            .rows()
            .enumerate()
            .skip(layout.first_row)
            .take(layout.visible_rows)
        {
            let row_y = layout.list_y + (index - layout.first_row) as f32 * layout.row_height;
            let text_y = row_y + text_offset;
            if index == palette.selected() {
                self.emit_rounded_rect(
                    &mut bg_v,
                    &mut bg_i,
                    layout.x + layout.padding / 2.0,
                    row_y,
                    layout.width - layout.padding,
                    layout.row_height,
                    (radius - line).max(0.0),
                    colors.selected,
                );
            }
            // The key at the right, then the label in what room is left.
            let key_cells = row.entry.key.chars().count().min(inner_cells / 2);
            if key_cells > 0 {
                self.text_renderer.append_cell_text(
                    &row.entry.key,
                    inner_x + (inner_cells - key_cells) as f32 * cell_width,
                    text_y,
                    key_cells,
                    |_| colors.muted,
                    &mut text_v,
                    &mut text_i,
                );
            }
            let label_cells =
                inner_cells.saturating_sub(key_cells + usize::from(key_cells > 0) * 2);
            let matched = row.matched;
            self.text_renderer.append_cell_text(
                &row.entry.label,
                inner_x,
                text_y,
                label_cells,
                |i| {
                    if matched.contains(&i) {
                        colors.matched
                    } else {
                        colors.text
                    }
                },
                &mut text_v,
                &mut text_i,
            );
            // Matched characters are underlined too, so they show without
            // telling colors apart. Labels are one cell per character.
            for &i in matched.iter().filter(|&&i| i < label_cells) {
                self.emit_rect(
                    &mut bg_v,
                    &mut bg_i,
                    inner_x + i as f32 * cell_width,
                    text_y + cell_height - underline,
                    cell_width,
                    underline,
                    colors.matched,
                );
            }
        }
        (bg_v, bg_i, text_v, text_i)
    }

    /// Draw the open palette over everything else.
    pub(super) fn render_command_palette(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        palette: &CommandPalette,
    ) {
        let (bg_v, bg_i, text_v, text_i) = self.build_command_palette_vertices(palette);
        self.draw_overlay_layers(encoder, view, (&bg_v, &bg_i), (&text_v, &text_i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: (f32, f32) = (9.0, 18.0);

    #[test]
    fn the_caret_follows_the_prompt_and_the_typed_query() {
        // "> " is two cells; nothing typed puts the caret right after it.
        assert_eq!(caret_cells(0, 60, 5, 5, 0), 2);
        assert_eq!(caret_cells(3, 60, 5, 5, 0), 5);
    }

    #[test]
    fn a_long_query_stops_the_caret_before_the_match_count() {
        // 29 matches, 12 listed: "1 of 29" and a gap take 8 cells, the
        // prompt 2, one is kept free.
        assert_eq!(caret_cells(500, 40, 29, 12, 0), 2 + (40 - 2 - 8 - 1));
        // Without a count the whole width but the free cell is available.
        assert_eq!(caret_cells(500, 40, 5, 5, 0), 2 + (40 - 2 - 1));
    }

    fn inside(layout: &PaletteLayout, (width, height): (f32, f32)) -> bool {
        layout.x >= 0.0
            && layout.y >= 0.0
            && layout.x + layout.width <= width
            && layout.y + layout.height <= height
    }

    #[test]
    fn a_large_window_gets_a_capped_centered_panel_listing_twelve_rows() {
        let window = (1920.0, 1080.0);
        let layout = PaletteLayout::new(window, 40.0, CELL, 29, 0);
        assert_eq!(layout.width, MAX_WIDTH_CELLS * CELL.0);
        assert_eq!(layout.x, ((1920.0 - layout.width) / 2.0).round());
        assert_eq!(layout.visible_rows, MAX_VISIBLE_ROWS);
        assert!(layout.y > 40.0, "below the tab bar");
        assert!(inside(&layout, window));
    }

    #[test]
    fn a_small_window_keeps_the_panel_inside_it() {
        for window in [(320.0, 200.0), (200.0, 120.0), (640.0, 240.0)] {
            let layout = PaletteLayout::new(window, 30.0, CELL, 29, 0);
            assert!(layout.visible_rows >= 1);
            assert!(layout.visible_rows < MAX_VISIBLE_ROWS, "{window:?}");
            assert!(layout.width > 0.0);
            if window.1 >= 200.0 {
                assert!(inside(&layout, window), "{window:?}: {layout:?}");
            }
        }
    }

    #[test]
    fn short_lists_shrink_the_panel_and_empty_ones_keep_a_row() {
        let window = (1280.0, 800.0);
        let three = PaletteLayout::new(window, 40.0, CELL, 3, 0);
        assert_eq!(three.visible_rows, 3);
        let none = PaletteLayout::new(window, 40.0, CELL, 0, 0);
        assert_eq!(none.visible_rows, 1);
        assert!(none.height < three.height);
    }

    #[test]
    fn the_list_scrolls_to_keep_the_selection_in_view() {
        let window = (1280.0, 800.0);
        let top = PaletteLayout::new(window, 40.0, CELL, 29, 11);
        assert_eq!(top.first_row, 0);
        let scrolled = PaletteLayout::new(window, 40.0, CELL, 29, 20);
        assert_eq!(scrolled.first_row, 20 + 1 - MAX_VISIBLE_ROWS);
        let last = PaletteLayout::new(window, 40.0, CELL, 29, 28);
        assert_eq!(last.first_row + last.visible_rows, 29);
    }
}
