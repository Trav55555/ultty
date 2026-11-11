//! An inline surface's scene as styled cells (ADR 0020 §2): what its rows
//! hold for copy, search and scrollback, live and after it is closed.
//!
//! The tree under a surface root, as last laid out, is drawn into `rows`
//! rows of `cols` cells the way the GPU renderer draws it, region by region
//! in drawing order: the background as cell `bg`, then the content; then
//! every border as box-drawing characters, over any content on its cells
//! (borders take no room in the layout, so a bordered region needs padding
//! to keep its text clear of them). Text breaks where the renderer breaks
//! it ([`super::text_layout`]) and is written through a one-row [`Grid`],
//! so wide characters and grapheme clusters take the cells the grid gives
//! them. Everything is clipped to the surface and to each region's clip
//! rect, as on the GPU; a wide character cut by a clip edge is blanked.
//!
//! Layout sizes are not bounded, so the work is: per region, the cells of
//! it that are visible (at most `rows × cols`), plus its text up to the
//! surface's last row, each visible line costing its length and at most
//! `cols` more however far left of the clip it starts. Coordinates are
//! held to ±2^20 cells.
//!
//! Styles map to cells as follows: `RichText` spans and region styles give
//! `fg`, bold, italic, underline and dim; `Foreground` and `Background`
//! named colours are the cell's default colours; opacity is all or nothing
//! (a region at 0 draws nothing of its own). Widgets give their label or
//! value: a progress bar its label or percentage, a spinner its label after
//! the frame's two columns, an input its value (masked for a password) or
//! placeholder, a button its centred label, deprecated Markdown and Code
//! widgets their source; an image is blank cells. What only moves on the
//! GPU (spinner frames, the streaming cursor, search highlights) is not
//! drawn.

use ultty_protocol::{BorderType, Color as ProtoColor, Content, NamedColor, RegionId, Style};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::geometry::{rebuild_scroll_geometry, ClipRects, ScrollAdjustments};
use super::region::Region;
use super::text_layout::{next_display_width_chunk_end, wrap_line};
use super::tree::SceneTree;
use crate::grid::{Cell, CellFlags, Color, CursorPosition, Dimensions, Grid, Row};

/// Columns kept past the last visible one when writing text, so characters
/// that overflow the line land where nothing is copied.
const PEN_MARGIN: i64 = 4;

/// How far left of its clip text may start before the part that cannot
/// show is skipped rather than written; see `Canvas::put_text`.
const SKIP_FROM: i64 = 1024;

/// Draw the tree under surface root `root` into `rows` rows of `cols`
/// cells, from the regions' computed layout and the `scroll_offset` of
/// each scrolling region.
pub fn rasterise_surface(
    scene: &SceneTree,
    root: RegionId,
    cols: u16,
    rows: u16,
    scroll_offset: impl Fn(RegionId) -> (f32, f32),
) -> Vec<Row> {
    let mut canvas = Canvas::new(cols, rows);
    let mut adjustments = ScrollAdjustments::new();
    let mut clips = ClipRects::new();
    rebuild_scroll_geometry(
        scene,
        root,
        scroll_offset,
        1.0,
        1.0,
        (0.0, 0.0),
        Some((0.0, 0.0, f32::from(cols), f32::from(rows))),
        &mut adjustments,
        &mut clips,
    );
    let placed = || {
        scene.iter_tree(root).filter_map(|region| {
            let adjust = adjustments.get(&region.id).copied().unwrap_or_default();
            let clip = Clip::from_rect(*clips.get(&region.id)?);
            let rect = Rect::of(region, adjust)?;
            (!clip.is_empty()).then_some((region, rect, clip))
        })
    };
    for (region, rect, clip) in placed() {
        canvas.draw_region(region, rect, clip);
    }
    // Borders take no room in the layout, so content can sit on them;
    // they are drawn last, over it, as the GPU draws its lines.
    for (region, rect, clip) in placed() {
        canvas.draw_border(&region.style, rect, clip);
    }
    canvas.rows
}

/// The furthest a region's edge is taken to be from the surface, in cells:
/// layout sizes are not bounded, and nothing past this can be on screen.
const MAX_EXTENT: f32 = 1_048_576.0;

/// A layout coordinate in whole cells, held to [`MAX_EXTENT`] so the
/// arithmetic on it cannot overflow.
fn cells(value: f32) -> i32 {
    value.clamp(-MAX_EXTENT, MAX_EXTENT).round() as i32
}

/// `base` moved right or down by `offset` cells, saturating.
fn offset(base: i32, offset: usize) -> i32 {
    base.saturating_add(i32::try_from(offset).unwrap_or(i32::MAX))
}

/// Where a region is drawn, in cells: its box, its content's origin and
/// the columns its text wraps at.
#[derive(Debug, Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    cx: i32,
    cy: i32,
    max_cols: usize,
}

impl Rect {
    /// `region`'s laid-out box, shifted by the scroll `adjust`ment of its
    /// ancestors; `None` when it is empty or fully transparent.
    fn of(region: &Region, adjust: (f32, f32)) -> Option<Self> {
        let layout = &region.computed;
        if !(layout.width > 0.0 && layout.height > 0.0) || region.style.opacity <= 0.0 {
            return None;
        }
        Some(Self {
            x: cells(layout.x - adjust.0),
            y: cells(layout.y - adjust.1),
            w: cells(layout.width),
            h: cells(layout.height),
            cx: cells(layout.content_x - adjust.0),
            cy: cells(layout.content_y - adjust.1),
            max_cols: layout.content_width.clamp(0.0, MAX_EXTENT) as usize,
        })
    }

    /// The columns and rows of the box inside `clip`.
    fn clipped(self, clip: Clip) -> (std::ops::Range<i32>, std::ops::Range<i32>) {
        (
            self.x.max(clip.x0)..(self.x + self.w).min(clip.x1),
            self.y.max(clip.y0)..(self.y + self.h).min(clip.y1),
        )
    }
}

/// A clip rect in whole cells: columns `x0..x1` of rows `y0..y1`.
#[derive(Debug, Clone, Copy)]
struct Clip {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Clip {
    fn from_rect((x0, y0, x1, y1): (f32, f32, f32, f32)) -> Self {
        Self {
            x0: x0.round() as i32,
            y0: y0.round() as i32,
            x1: x1.round() as i32,
            y1: y1.round() as i32,
        }
    }

    fn is_empty(self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    fn contains(self, x: i32, y: i32) -> bool {
        (self.x0..self.x1).contains(&x) && (self.y0..self.y1).contains(&y)
    }
}

/// The cells being drawn, and the one-row grid text is written through.
struct Canvas {
    rows: Vec<Row>,
    cols: u16,
    pen: Option<Grid>,
}

impl Canvas {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            rows: (0..rows).map(|_| Row::new(cols)).collect(),
            cols,
            pen: None,
        }
    }

    /// Draw `region`'s background and content at `rect`, clipped to
    /// `clip`.
    fn draw_region(&mut self, region: &Region, rect: Rect, clip: Clip) {
        let Rect {
            cx, cy, max_cols, ..
        } = rect;
        let style = &region.style;
        if let Some(bg) = &style.bg {
            self.fill_bg(rect, clip, cell_color(bg));
        }

        match &region.content {
            Content::None => {}
            Content::Text(text) => {
                let template = template(style.fg.as_ref(), style);
                let mut line_y = cy;
                for line in text.text.split('\n') {
                    if line_y >= clip.y1 {
                        break;
                    }
                    // A line of one character fills the width, as a rule.
                    let mut chars = line.chars();
                    if let (Some(c), None, true) = (chars.next(), chars.next(), max_cols > 1) {
                        line_y = self.put_fill(c, cx, line_y, max_cols, template, clip);
                        continue;
                    }
                    if line.is_empty() {
                        line_y += 1;
                        continue;
                    }
                    for piece in wrap_line(line, max_cols) {
                        if line_y >= clip.y1 {
                            break;
                        }
                        if line_y >= clip.y0 {
                            self.put_text(cx, line_y, &piece, template, clip);
                        }
                        line_y += 1;
                    }
                }
            }
            Content::RichText(rich) => {
                let mut col = 0usize;
                let mut line = 0usize;
                'spans: for span in &rich.spans {
                    if offset(cy, line) >= clip.y1 {
                        break;
                    }
                    let mut span_style = span.style.clone();
                    span_style.bold |= style.bold;
                    span_style.italic |= style.italic;
                    span_style.underline |= style.underline;
                    span_style.dim |= style.dim;
                    let template =
                        template(span.style.fg.as_ref().or(style.fg.as_ref()), &span_style);
                    if max_cols == 0 {
                        col += self.put_text(
                            offset(cx, col),
                            offset(cy, line),
                            &span.text,
                            template,
                            clip,
                        );
                        continue;
                    }
                    for (index, segment) in span.text.split('\n').enumerate() {
                        if index > 0 {
                            col = 0;
                            line += 1;
                        }
                        let mut pos = 0;
                        while pos < segment.len() {
                            if offset(cy, line) >= clip.y1 {
                                break 'spans;
                            }
                            let cols_left = max_cols.saturating_sub(col);
                            if cols_left == 0 {
                                col = 0;
                                line += 1;
                                continue;
                            }
                            let Some(end) = next_display_width_chunk_end(segment, pos, cols_left)
                            else {
                                break;
                            };
                            col += self.put_text(
                                offset(cx, col),
                                offset(cy, line),
                                &segment[pos..end],
                                template,
                                clip,
                            );
                            pos = end;
                            if pos < segment.len() {
                                col = 0;
                                line += 1;
                            }
                        }
                    }
                }
            }
            Content::Widget(widget) => self.draw_widget(widget, style, rect, clip),
        }
    }

    /// Draw a fill line, `c` repeated `max_cols` times from `(x, y)` and
    /// broken into rows as `wrap_line` breaks it (a wide character takes
    /// two rows, or three for an odd width), without building it: only the
    /// part of each row inside `clip` is written. Returns the row after it.
    fn put_fill(
        &mut self,
        c: char,
        x: i32,
        y: i32,
        max_cols: usize,
        template: Cell,
        clip: Clip,
    ) -> i32 {
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        let per_row = if width * max_cols <= max_cols {
            max_cols
        } else {
            max_cols / width
        };
        let rows = max_cols.div_ceil(per_row);
        for (row, row_y) in (0..rows).zip(y..) {
            if row_y >= clip.y1 {
                break;
            }
            if row_y >= clip.y0 {
                let count = per_row.min(max_cols - row * per_row) as i64;
                let step = width.max(1) as i64;
                let from = ((i64::from(clip.x0) - i64::from(x)) / step).clamp(0, count);
                let to = ((i64::from(clip.x1) - i64::from(x) + step - 1) / step).clamp(from, count);
                let start = x.saturating_add((from * step) as i32);
                let piece: String = std::iter::repeat_n(c, (to - from) as usize).collect();
                self.put_text(start, row_y, &piece, template, clip);
            }
        }
        y.saturating_add(i32::try_from(rows).unwrap_or(i32::MAX))
    }

    fn draw_widget(
        &mut self,
        widget: &ultty_protocol::WidgetContent,
        style: &Style,
        rect: Rect,
        clip: Clip,
    ) {
        let Rect { w, h, cx, cy, .. } = rect;
        use ultty_protocol::{ButtonStyle, WidgetContent};

        let plain = Style::default();
        match widget {
            WidgetContent::Progress(progress) => {
                let text = match &progress.label {
                    Some(label) => label.clone(),
                    None if progress.indeterminate => return,
                    None => format!("{:.0}%", progress.value.clamp(0.0, 1.0) * 100.0),
                };
                self.put_text(cx, cy, &text, template(None, &plain), clip);
            }
            WidgetContent::Spinner(spinner) => {
                if let Some(label) = &spinner.label {
                    let fg = style.fg.unwrap_or(ProtoColor::Named(NamedColor::Cyan));
                    self.put_text(cx + 2, cy, label, template(Some(&fg), &plain), clip);
                }
            }
            WidgetContent::Input(input) => {
                let fg = input
                    .value
                    .is_empty()
                    .then_some(ProtoColor::Named(NamedColor::BrightBlack));
                let text = super::caret::shown_text(input);
                self.put_text(cx, cy, &text, template(fg.as_ref(), &plain), clip);
                if let Some((col, row)) = super::caret::caret_offset(input) {
                    self.put_caret(
                        cx.saturating_add(col as i32),
                        cy.saturating_add(row as i32),
                        clip,
                    );
                }
            }
            WidgetContent::Markdown(markdown) => self.put_lines(cx, cy, &markdown.source, clip),
            WidgetContent::Code(code) => self.put_lines(cx, cy, &code.source, clip),
            WidgetContent::Image(_) => {
                let (cols, rows) = rect.clipped(clip);
                for row in rows {
                    for col in cols.clone() {
                        self.put(col, row, Cell::default(), None);
                    }
                }
            }
            WidgetContent::Button(button) => {
                let fg = match button.style {
                    ButtonStyle::Primary => Some(ProtoColor::Named(NamedColor::Blue)),
                    ButtonStyle::Danger => Some(ProtoColor::Named(NamedColor::Red)),
                    ButtonStyle::Success => Some(ProtoColor::Named(NamedColor::Green)),
                    ButtonStyle::Default => None,
                };
                let label_cols = i32::try_from(button.label.chars().count()).unwrap_or(i32::MAX);
                let offset_x = ((w - label_cols) / 2).max(0);
                let offset_y = ((h - 1) / 2).max(0);
                let bold = Style::new().bold();
                self.put_text(
                    cx + offset_x,
                    cy + offset_y,
                    &button.label,
                    template(fg.as_ref(), &bold),
                    clip,
                );
            }
        }
    }

    /// Draw a caret at cell `(x, y)`: the cell reversed, a blank one past
    /// the end of the text.
    fn put_caret(&mut self, x: i32, y: i32, clip: Clip) {
        if x < clip.x0 || x >= clip.x1 || y < clip.y0 || y >= clip.y1 {
            return;
        }
        let (Ok(col), Ok(row)) = (u16::try_from(x), usize::try_from(y)) else {
            return;
        };
        let cols = self.cols;
        let Some(line) = self.rows.get_mut(row) else {
            return;
        };
        if col >= cols {
            return;
        }
        line[col].set_flag(CellFlags::REVERSE);
        // The second cell of a wide character is reversed with its first.
        if line[col].is_wide() && col + 1 < cols {
            line[col + 1].set_flag(CellFlags::REVERSE);
        }
    }

    /// `source` one line per row from `(x, y)`, in the default colours.
    fn put_lines(&mut self, x: i32, y: i32, source: &str, clip: Clip) {
        let template = template(None, &Style::default());
        for (row, line) in source.lines().enumerate() {
            let y = offset(y, row);
            if y >= clip.y1 {
                break;
            }
            self.put_text(x, y, line, template, clip);
        }
    }

    /// Set the background of the cells of `rect` inside `clip`,
    /// keeping what they show.
    fn fill_bg(&mut self, rect: Rect, clip: Clip, bg: Color) {
        let (cols, rows) = rect.clipped(clip);
        for row in rows {
            for col in cols.clone() {
                if let Some(cell) = self.cell_mut(col, row) {
                    cell.set_bg(bg);
                }
            }
        }
    }

    /// Box-drawing characters on the edges `style.border` names around
    /// `rect`, in its border colour, else its foreground, else the default.
    /// Only the edge cells inside `clip` are visited.
    fn draw_border(&mut self, style: &Style, rect: Rect, clip: Clip) {
        let Some(glyphs) = BorderGlyphs::of(style.border.style) else {
            return;
        };
        let edges = style.border.edges;
        let template = template(
            style.border_color.as_ref().or(style.fg.as_ref()),
            &Style::default(),
        );
        let Rect { x, y, w, h, .. } = rect;
        let (right, bottom) = (x + w - 1, y + h - 1);
        let glyph = |col: i32, row: i32| {
            let top = edges.top && row == y;
            let below = edges.bottom && row == bottom;
            let left = edges.left && col == x;
            let side = edges.right && col == right;
            match (top, below, left, side) {
                (true, _, true, _) => Some(glyphs.top_left),
                (true, _, _, true) => Some(glyphs.top_right),
                (_, true, true, _) => Some(glyphs.bottom_left),
                (_, true, _, true) => Some(glyphs.bottom_right),
                (true, ..) | (_, true, ..) => Some(glyphs.horizontal),
                (.., true, _) | (.., true) => Some(glyphs.vertical),
                _ => None,
            }
        };
        let put = |canvas: &mut Self, col: i32, row: i32| {
            if let Some(c) = glyph(col, row) {
                let mut cell = template;
                cell.set_char(c);
                canvas.put_cell(col, row, cell, clip);
            }
        };
        let (cols, rows) = rect.clipped(clip);
        for row in rows {
            let ruled = (edges.top && row == y) || (edges.bottom && row == bottom);
            if ruled {
                for col in cols.clone() {
                    put(self, col, row);
                }
            } else {
                put(self, x, row);
                if right != x {
                    put(self, right, row);
                }
            }
        }
    }

    /// Write `text` from `(x, y)` in `template`'s colours and attributes,
    /// clipped to `clip`, the way the grid would print it. Returns the
    /// columns it took, clipped or not.
    ///
    /// The text goes through a pen grid no wider than the text can be (two
    /// columns a character) or than what of it can show, so its cost
    /// follows the text, not how far left of the clip it starts. Text more
    /// than [`SKIP_FROM`] columns left of the clip starts at the first
    /// character near the clip's edge, counted by Unicode width; wide
    /// clusters (emoji sequences) skipped that way can shift what shows by
    /// the difference between their Unicode and grid widths.
    fn put_text(&mut self, x: i32, y: i32, text: &str, template: Cell, clip: Clip) -> usize {
        let unicode_width = || UnicodeWidthStr::width(text);
        if y < clip.y0 || y >= clip.y1 {
            // Nothing of it shows, so its width by Unicode will do.
            return unicode_width();
        }
        let mut visible_end = i64::from(clip.x1) - i64::from(x);
        let mut first_visible = (i64::from(clip.x0) - i64::from(x)).max(0);
        let mut bound = 2 * text.chars().count() as i64;
        if visible_end <= 0 || bound <= first_visible {
            return unicode_width();
        }
        let (mut skipped, mut start) = (0i64, 0usize);
        if first_visible > SKIP_FROM {
            let target = first_visible - PEN_MARGIN;
            for (index, c) in text.char_indices() {
                let width = UnicodeWidthChar::width(c).unwrap_or(0) as i64;
                if width > 0 && skipped + width > target {
                    break;
                }
                skipped += width;
                start = index + c.len_utf8();
            }
            visible_end -= skipped;
            first_visible -= skipped;
            bound = 2 * text[start..].chars().count() as i64;
        }
        let needed = visible_end.min(bound) + PEN_MARGIN;
        let width = u16::try_from(needed).unwrap_or(u16::MAX);
        let mut pen = match self.pen.take() {
            Some(pen)
                if (width..=width.saturating_mul(2).max(self.cols))
                    .contains(&pen.dimensions().cols) =>
            {
                pen
            }
            _ => {
                let mut grid = Grid::new(Dimensions::new(width, 1));
                grid.set_auto_wrap(false);
                grid
            }
        };
        pen.clear();
        pen.cursor_mut().position = CursorPosition::origin();
        pen.set_attrs(template);
        let mut written = 0u16;
        for c in text[start..].chars().filter(|c| !c.is_control()) {
            pen.write_char(c);
            written = written.max(pen.cursor().position.col);
        }
        let last = visible_end.min(i64::from(written));
        if let Some(line) = pen.row(0) {
            for p in first_visible..last {
                let Some(cell) = line.cell(p as u16) else {
                    break;
                };
                let col = x.saturating_add((skipped + p) as i32);
                let cut = (cell.is_wide() && p + 1 >= visible_end)
                    || (cell.is_wide_spacer() && p == first_visible);
                if cut {
                    let mut blank = template;
                    blank.set_char(' ');
                    blank.unset_flag(CellFlags::WIDE_CHAR);
                    blank.unset_flag(CellFlags::WIDE_CHAR_SPACER);
                    blank.unset_flag(CellFlags::HAS_COMBINING);
                    self.put_cell(col, y, blank, clip);
                } else {
                    self.put(col, y, cell, line.grapheme_extra(p as u16));
                }
            }
        }
        self.pen = Some(pen);
        usize::try_from(skipped).unwrap_or(0) + usize::from(written)
    }

    /// Write one cell inside `clip`.
    fn put_cell(&mut self, x: i32, y: i32, cell: Cell, clip: Clip) {
        if clip.contains(x, y) {
            self.put(x, y, cell, None);
        }
    }

    /// Write `cell` (with the grapheme `extra` after its character) at
    /// `(x, y)` on the background already there. A wide character it cuts
    /// in half loses its other half too.
    fn put(&mut self, x: i32, y: i32, mut cell: Cell, extra: Option<&[char]>) {
        let (Ok(col), Ok(row)) = (u16::try_from(x), usize::try_from(y)) else {
            return;
        };
        let cols = self.cols;
        let Some(target) = self.rows.get_mut(row) else {
            return;
        };
        let Some(old) = target.get(col).copied() else {
            return;
        };
        if old.is_wide_spacer() && col > 0 {
            blank(target, col - 1);
        }
        if old.is_wide() && col + 1 < cols {
            blank(target, col + 1);
        }
        cell.set_bg(old.bg());
        target[col] = cell;
        target.set_grapheme_extra(col, extra.unwrap_or(&[]));
    }

    fn cell_mut(&mut self, x: i32, y: i32) -> Option<&mut Cell> {
        let row = self.rows.get_mut(usize::try_from(y).ok()?)?;
        row.get_mut(u16::try_from(x).ok()?)
    }
}

/// Make the cell at `col` a space on the background it had.
fn blank(row: &mut Row, col: u16) {
    let bg = row[col].bg();
    let mut cell = Cell::default();
    cell.set_bg(bg);
    row[col] = cell;
    row.set_grapheme_extra(col, &[]);
}

/// The characters a border is drawn with.
struct BorderGlyphs {
    horizontal: char,
    vertical: char,
    top_left: char,
    top_right: char,
    bottom_left: char,
    bottom_right: char,
}

impl BorderGlyphs {
    fn of(border: BorderType) -> Option<Self> {
        let [horizontal, vertical, top_left, top_right, bottom_left, bottom_right] = match border {
            BorderType::None => return None,
            BorderType::Thin => ['─', '│', '┌', '┐', '└', '┘'],
            BorderType::Thick => ['━', '┃', '┏', '┓', '┗', '┛'],
            BorderType::Double => ['═', '║', '╔', '╗', '╚', '╝'],
            BorderType::Rounded => ['─', '│', '╭', '╮', '╰', '╯'],
            BorderType::Dashed => ['╌', '╎', '┌', '┐', '└', '┘'],
            BorderType::Ascii => ['-', '|', '+', '+', '+', '+'],
        };
        Some(Self {
            horizontal,
            vertical,
            top_left,
            top_right,
            bottom_left,
            bottom_right,
        })
    }
}

/// A blank cell in foreground `fg` (the default when `None`) with the
/// attributes of `style`.
fn template(fg: Option<&ProtoColor>, style: &Style) -> Cell {
    let mut cell = Cell::default();
    if let Some(fg) = fg {
        cell.set_fg(cell_color(fg));
    }
    for (on, flag) in [
        (style.bold, CellFlags::BOLD),
        (style.italic, CellFlags::ITALIC),
        (style.underline, CellFlags::UNDERLINE),
        (style.dim, CellFlags::DIM),
    ] {
        if on {
            cell.set_flag(flag);
        }
    }
    cell
}

/// A protocol colour as a cell colour. `Foreground` and `Background` are
/// the terminal's own, which a cell's default colours are.
fn cell_color(color: &ProtoColor) -> Color {
    match *color {
        ProtoColor::Named(NamedColor::Foreground | NamedColor::Background) => Color::Default,
        ProtoColor::Named(named) => Color::Named(named as u8),
        ProtoColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        ProtoColor::Indexed(index) => Color::Indexed(index),
    }
}

#[cfg(test)]
mod tests {
    use ultty_protocol::{
        BorderEdges, BorderStyle, ButtonWidget, Constraints, Dimension, Edges, InputWidget,
        Overflow, OverflowAxis, ProgressWidget, RichText, SpinnerWidget, TextContent,
        WidgetContent,
    };

    use super::*;
    use crate::scene::layout::LayoutEngine;

    const SURFACE: RegionId = RegionId::new(10);

    /// A scene with an empty surface `rows` high, laid out `cols` wide.
    struct Card {
        scene: SceneTree,
        cols: u16,
        rows: u16,
    }

    impl Card {
        fn new(cols: u16, rows: u16) -> Self {
            Self::with_root_style(cols, rows, Style::default())
        }

        fn with_root_style(cols: u16, rows: u16, style: Style) -> Self {
            let mut scene = SceneTree::new();
            let mut root = Region::new(SURFACE);
            root.style = style;
            scene.try_insert_surface(root, rows).unwrap();
            Self { scene, cols, rows }
        }

        /// Add region `id` under `parent`, `width x height` cells.
        fn add(
            &mut self,
            id: u32,
            parent: RegionId,
            (width, height): (Dimension, Dimension),
            style: Style,
            content: Content,
        ) -> RegionId {
            let mut region = Region::new(RegionId::new(id));
            region.parent = Some(parent);
            let none = Dimension::Cells(0.0);
            region.constraints = Constraints {
                width,
                height,
                flex_shrink: 0.0,
                margin: Edges {
                    top: none,
                    right: none,
                    bottom: none,
                    left: none,
                },
                ..Constraints::default()
            };
            region.style = style;
            region.content = content;
            self.scene.insert(region);
            RegionId::new(id)
        }

        fn rows(&mut self) -> Vec<Row> {
            LayoutEngine::new()
                .compute_scene_layout(&mut self.scene, f32::from(self.cols), 24.0)
                .unwrap();
            rasterise_surface(&self.scene, SURFACE, self.cols, self.rows, |_| (0.0, 0.0))
        }

        fn text(&mut self) -> Vec<String> {
            self.rows()
                .iter()
                .map(|row| row.copy_text(0, row.width()))
                .collect()
        }
    }

    fn cells(width: f32, height: f32) -> (Dimension, Dimension) {
        (Dimension::Cells(width), Dimension::Cells(height))
    }

    fn text(text: &str) -> Content {
        Content::Text(TextContent::new(text))
    }

    fn widget(widget: WidgetContent) -> Content {
        Content::Widget(widget)
    }

    /// Region text breaks exactly where the renderer's `wrap_line` breaks
    /// it, one piece per row.
    #[test]
    fn text_breaks_where_the_renderer_breaks_it() {
        let prose = "the quick brown fox jumps over a lazy dog\nand then\n\nsleeps";
        let mut card = Card::new(14, 8);
        card.add(1, SURFACE, cells(12.0, 8.0), Style::default(), text(prose));
        let expected: Vec<String> = prose
            .split('\n')
            .flat_map(|line| wrap_line(line, 12))
            .map(|piece| format!("{piece:<14}"))
            .chain(std::iter::repeat(" ".repeat(14)))
            .take(8)
            .collect();
        assert_eq!(card.text(), expected);
    }

    #[test]
    fn a_line_of_one_character_fills_the_width() {
        let mut card = Card::new(8, 2);
        card.add(
            1,
            SURFACE,
            cells(5.0, 2.0),
            Style::default(),
            text("-\nx y"),
        );
        assert_eq!(card.text(), ["-----   ", "x y     "]);
    }

    /// Spans take their colours and attributes, with the region's added;
    /// the region's background stays under the text.
    #[test]
    fn spans_and_styles_become_cell_colours_and_attributes() {
        let mut card = Card::new(10, 1);
        let rich = RichText::new()
            .span("ab", Style::new().bold().fg(ProtoColor::RED))
            .span("cd", Style::default());
        let region_style = Style {
            underline: true,
            bg: Some(ProtoColor::Rgb(1, 2, 3)),
            fg: Some(ProtoColor::Indexed(42)),
            ..Style::default()
        };
        card.add(
            1,
            SURFACE,
            cells(6.0, 1.0),
            region_style,
            Content::RichText(rich),
        );
        let rows = card.rows();
        let row = &rows[0];
        assert_eq!(row.copy_text(0, 9), "abcd      ");
        assert_eq!(row[0].fg(), Color::RED);
        assert!(row[0].has_flag(CellFlags::BOLD) && row[0].has_flag(CellFlags::UNDERLINE));
        assert_eq!(row[2].fg(), Color::Indexed(42));
        assert!(!row[2].has_flag(CellFlags::BOLD) && row[2].has_flag(CellFlags::UNDERLINE));
        for col in 0..6 {
            assert_eq!(row[col].bg(), Color::Rgb(1, 2, 3), "column {col}");
        }
        assert_eq!(row[6].bg(), Color::Default);

        let dim = Style {
            dim: true,
            italic: true,
            fg: Some(ProtoColor::Named(NamedColor::Foreground)),
            ..Style::default()
        };
        let mut card = Card::new(4, 1);
        card.add(1, SURFACE, cells(4.0, 1.0), dim, text("hi"));
        let row = &card.rows()[0];
        assert!(row[0].has_flag(CellFlags::DIM) && row[0].has_flag(CellFlags::ITALIC));
        assert_eq!(row[0].fg(), Color::Default);
    }

    /// Rich text runs on across spans and breaks by display width, where
    /// the renderer does.
    #[test]
    fn rich_text_flows_across_spans_and_lines() {
        let mut card = Card::new(6, 3);
        let rich = RichText::new().text("abcd").text("efg\nh");
        card.add(
            1,
            SURFACE,
            cells(5.0, 3.0),
            Style::default(),
            Content::RichText(rich),
        );
        assert_eq!(card.text(), ["abcde ", "fg    ", "h     "]);
    }

    /// The tree `ulttui` sends for the agent-transcript diff card, with its
    /// constraints as `ulttui` builds them (from `Constraints::default()`),
    /// in a surface one row taller than its content: the free row stays at
    /// the bottom, not between the title and the diff.
    #[test]
    fn a_short_column_leaves_its_free_rows_at_the_bottom() {
        use ultty_protocol::{Display, FlexDirection};
        let mut card = Card::new(20, 7);
        let mut put = |id: u32, parent: RegionId, constraints: Constraints, content: Content| {
            let mut region = Region::new(RegionId::new(id));
            region.parent = Some(parent);
            region.constraints = constraints;
            region.content = content;
            card.scene.insert(region);
            RegionId::new(id)
        };
        let column = Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            ..Constraints::default()
        };
        let line = |height: Dimension| Constraints {
            display: Display::Flex,
            width: Dimension::Percent(100.0),
            height,
            flex_shrink: 1.0,
            ..Constraints::default()
        };
        let card_root = put(
            1,
            SURFACE,
            Constraints {
                height: Dimension::Percent(100.0),
                ..column.clone()
            },
            Content::None,
        );
        put(
            2,
            card_root,
            line(Dimension::Cells(1.0)),
            text("[diff] title"),
        );
        let diff = put(3, card_root, column, Content::None);
        for n in 0..5 {
            put(
                4 + n,
                diff,
                line(Dimension::Cells(1.0)),
                text(&format!("line {n}")),
            );
        }
        let text: Vec<String> = card
            .text()
            .iter()
            .map(|r| r.trim_end().to_string())
            .collect();
        assert_eq!(
            text,
            [
                "[diff] title",
                "line 0",
                "line 1",
                "line 2",
                "line 3",
                "line 4",
                ""
            ]
        );
    }

    #[test]
    fn a_surface_root_background_fills_every_cell() {
        let style = Style {
            bg: Some(ProtoColor::Rgb(200, 40, 160)),
            ..Style::default()
        };
        let mut card = Card::with_root_style(5, 2, style);
        for row in card.rows() {
            assert!(row
                .cells()
                .all(|cell| cell.bg() == Color::Rgb(200, 40, 160)));
        }
        // Without one, the cells are the terminal's own.
        let mut card = Card::new(5, 2);
        assert!(card
            .rows()
            .iter()
            .all(|row| row.cells().all(|cell| *cell == Cell::default())));
    }

    #[test]
    fn borders_are_drawn_with_the_characters_of_their_style() {
        let boxes = [
            (BorderType::Thin, ["┌──┐", "│  │", "└──┘"]),
            (BorderType::Thick, ["┏━━┓", "┃  ┃", "┗━━┛"]),
            (BorderType::Double, ["╔══╗", "║  ║", "╚══╝"]),
            (BorderType::Rounded, ["╭──╮", "│  │", "╰──╯"]),
            (BorderType::Dashed, ["┌╌╌┐", "╎  ╎", "└╌╌┘"]),
            (BorderType::Ascii, ["+--+", "|  |", "+--+"]),
            (BorderType::None, ["    ", "    ", "    "]),
        ];
        for (kind, expected) in boxes {
            let mut card = Card::new(4, 3);
            let style = Style {
                border: BorderStyle {
                    style: kind,
                    edges: BorderEdges::ALL,
                    radius: 0,
                },
                border_color: Some(ProtoColor::GREEN),
                ..Style::default()
            };
            card.add(1, SURFACE, cells(4.0, 3.0), style, Content::None);
            let rows = card.rows();
            let text: Vec<String> = rows.iter().map(|row| row.copy_text(0, 3)).collect();
            assert_eq!(text, expected, "{kind:?}");
            if kind != BorderType::None {
                assert_eq!(rows[0][0].fg(), Color::GREEN, "{kind:?}");
            }
        }
    }

    /// Only the edges asked for: a rule over the top and down the left.
    #[test]
    fn a_border_draws_only_its_edges() {
        let mut card = Card::new(4, 3);
        let style = Style {
            border: BorderStyle {
                style: BorderType::Ascii,
                edges: BorderEdges {
                    top: true,
                    left: true,
                    right: false,
                    bottom: false,
                },
                radius: 0,
            },
            ..Style::default()
        };
        card.add(1, SURFACE, cells(4.0, 3.0), style, text(""));
        assert_eq!(card.text(), ["+---", "|   ", "|   "]);
    }

    /// Text stops at the clip rect of an ancestor that hides overflow and
    /// at the surface's own rows.
    #[test]
    fn content_is_clipped_to_its_clip_rect_and_the_surface() {
        let mut card = Card::new(8, 2);
        let clipper = card.add(1, SURFACE, cells(3.0, 1.0), Style::default(), Content::None);
        if let Some(region) = card.scene.get_mut(clipper) {
            region.constraints.overflow = Overflow {
                x: OverflowAxis::Hidden,
                y: OverflowAxis::Hidden,
            };
        }
        card.add(
            2,
            clipper,
            cells(6.0, 1.0),
            Style::default(),
            text("abcdef"),
        );
        card.add(
            3,
            SURFACE,
            cells(8.0, 4.0),
            Style::default(),
            text("1\n2\n3\n4 5"),
        );
        assert_eq!(card.text(), ["abc     ", "11111111"]);
    }

    /// A wide character cut by a clip edge is blanked, as the grid blanks
    /// one cut by a resize; graphemes keep their cell.
    #[test]
    fn wide_characters_at_a_clip_edge_are_blanked_and_clusters_kept() {
        let mut card = Card::new(6, 2);
        let clipper = card.add(1, SURFACE, cells(3.0, 1.0), Style::default(), Content::None);
        if let Some(region) = card.scene.get_mut(clipper) {
            region.constraints.overflow = Overflow {
                x: OverflowAxis::Hidden,
                y: OverflowAxis::Visible,
            };
        }
        card.add(2, clipper, cells(4.0, 1.0), Style::default(), text("ab你"));
        card.add(
            3,
            SURFACE,
            cells(6.0, 1.0),
            Style::default(),
            text("cafe\u{301}你"),
        );
        let rows = card.rows();
        assert_eq!(rows[0].copy_text(0, 5), "ab    ");
        assert!(!rows[0][2].is_wide(), "half a wide character is not drawn");
        assert_eq!(rows[1].copy_text(0, 5), "cafe\u{301}你");
        assert_eq!(rows[1].grapheme(3).as_deref(), Some("e\u{301}"));
        assert!(rows[1][4].is_wide() && rows[1][5].is_wide_spacer());
        // At the surface's own edge too.
        let mut card = Card::new(3, 1);
        card.add(1, SURFACE, cells(4.0, 1.0), Style::default(), text("ab你"));
        let row = &card.rows()[0];
        assert_eq!(row.copy_text(0, 2), "ab ");
        assert!(!row[2].is_wide());
    }

    /// Text over half of a wide character drawn before it blanks the other
    /// half, so no cell is left a lone half.
    #[test]
    fn overwriting_half_a_wide_character_blanks_the_other_half() {
        let mut card = Card::new(4, 1);
        card.add(1, SURFACE, cells(4.0, 1.0), Style::default(), text("你你"));
        let over = card.add(2, SURFACE, cells(1.0, 1.0), Style::default(), text("x"));
        if let Some(region) = card.scene.get_mut(over) {
            region.constraints.margin.left = Dimension::Cells(1.0);
            region.constraints.margin.top = Dimension::Cells(-1.0);
        }
        let row = &card.rows()[0];
        assert_eq!(row.copy_text(0, 3), " x你");
        assert!(!row[0].is_wide() && !row[1].is_wide_spacer());
    }

    #[test]
    fn widgets_show_their_label_or_value() {
        let mut card = Card::new(12, 6);
        let progress = |value: f32, label: Option<&str>| {
            widget(WidgetContent::Progress(ProgressWidget {
                value,
                label: label.map(String::from),
                animated: false,
                indeterminate: false,
            }))
        };
        let line = cells(12.0, 1.0);
        let plain = Style::default;
        card.add(1, SURFACE, line, plain(), progress(0.42, None));
        let spinner = SpinnerWidget {
            label: Some("wait".into()),
            ..SpinnerWidget::default()
        };
        card.add(
            2,
            SURFACE,
            line,
            plain(),
            widget(WidgetContent::Spinner(spinner)),
        );
        let password = InputWidget {
            value: "pw".into(),
            password: true,
            ..InputWidget::default()
        };
        card.add(
            3,
            SURFACE,
            line,
            plain(),
            widget(WidgetContent::Input(password)),
        );
        let empty = InputWidget {
            placeholder: "type".into(),
            ..InputWidget::default()
        };
        card.add(
            4,
            SURFACE,
            line,
            plain(),
            widget(WidgetContent::Input(empty)),
        );
        let button = WidgetContent::Button(ButtonWidget::new("OK"));
        card.add(5, SURFACE, cells(8.0, 1.0), plain(), widget(button));
        card.add(6, SURFACE, line, plain(), progress(0.5, Some("half")));
        let rows = card.rows();
        let text: Vec<String> = rows.iter().map(|row| row.text_trimmed()).collect();
        assert_eq!(text, ["42%", "  wait", "••", "type", "   OK", "half"]);
        assert_eq!(rows[3][0].fg(), Color::BRIGHT_BLACK);
        assert!(rows[4][3].has_flag(CellFlags::BOLD));
    }

    /// An input's caret is the reversed cell after its first `cursor`
    /// clusters; the text is the value, with nothing added.
    #[test]
    fn an_input_draws_its_caret_as_a_reversed_cell() {
        let line = cells(10.0, 1.0);
        let plain = Style::default;
        let input = |value: &str, cursor| {
            widget(WidgetContent::Input(InputWidget {
                value: value.into(),
                cursor,
                ..InputWidget::default()
            }))
        };
        let mut card = Card::new(10, 5);
        card.add(1, SURFACE, line, plain(), input("héllo", Some(2)));
        card.add(2, SURFACE, line, plain(), input("hi", Some(2)));
        card.add(3, SURFACE, line, plain(), input("你好", Some(1)));
        card.add(4, SURFACE, line, plain(), input("no caret", None));
        card.add(
            5,
            SURFACE,
            line,
            plain(),
            widget(WidgetContent::Input(InputWidget {
                placeholder: "type".into(),
                cursor: Some(1),
                ..InputWidget::default()
            })),
        );
        let rows = card.rows();
        let reversed = |row: usize| -> Vec<u16> {
            (0..10u16)
                .filter(|&col| rows[row][col].has_flag(CellFlags::REVERSE))
                .collect()
        };
        assert_eq!(rows[0].text_trimmed(), "héllo");
        assert_eq!(reversed(0), [2]);
        // Past the end: a blank cell.
        assert_eq!(rows[1].text_trimmed(), "hi");
        assert_eq!(reversed(1), [2]);
        // After a wide character: its two cells are the first, so col 2.
        assert_eq!(reversed(2), [2, 3]);
        assert_eq!(reversed(3), Vec::<u16>::new());
        // An empty input shows its placeholder, the caret counted into it.
        assert_eq!(rows[4].text_trimmed(), "type");
        assert_eq!(reversed(4), [1]);
    }

    /// An image is blank cells: what drew there before it is gone.
    #[test]
    fn an_image_is_blank_cells() {
        use ultty_protocol::{ImageScale, ImageWidget};
        let mut card = Card::new(6, 1);
        card.add(
            1,
            SURFACE,
            cells(6.0, 1.0),
            Style::default(),
            text("abcdef"),
        );
        let image = WidgetContent::Image(ImageWidget {
            data: vec![0; 4],
            width: 1,
            height: 1,
            scale: ImageScale::Fit,
        });
        let id = card.add(2, SURFACE, cells(2.0, 1.0), Style::default(), widget(image));
        if let Some(region) = card.scene.get_mut(id) {
            region.constraints.margin.left = Dimension::Cells(2.0);
            region.constraints.margin.top = Dimension::Cells(-1.0);
        }
        assert_eq!(card.text(), ["ab  ef"]);
    }

    /// Lay `card` out and rasterise it, failing if rasterising alone takes
    /// longer than `budget`. Budgets are seconds: they catch the hangs
    /// these tests guard against (tens of seconds and up before the fixes,
    /// microseconds to milliseconds after) without failing a debug build on
    /// a loaded machine.
    fn rows_within(card: &mut Card, budget: std::time::Duration) -> Vec<Row> {
        LayoutEngine::new()
            .compute_scene_layout(&mut card.scene, f32::from(card.cols), 24.0)
            .unwrap();
        let start = std::time::Instant::now();
        let rows = rasterise_surface(&card.scene, SURFACE, card.cols, card.rows, |_| (0.0, 0.0));
        let took = start.elapsed();
        assert!(took < budget, "rasterising took {took:?}");
        rows
    }

    /// Layout sizes are not bounded: a region a billion cells square, with
    /// a border, an image or a filled line, costs what its visible cells
    /// cost. Visiting its whole box took 0.8 s at 30 000 cells square.
    #[test]
    fn huge_regions_cost_only_their_visible_cells() {
        let budget = std::time::Duration::from_secs(2);
        let huge = cells(1.0e9, 1.0e9);

        let mut card = Card::new(6, 3);
        let border = Style {
            border: BorderStyle::thin(),
            ..Style::default()
        };
        card.add(1, SURFACE, huge, border, Content::None);
        let shown: Vec<String> = rows_within(&mut card, budget)
            .iter()
            .map(|row| row.copy_text(0, 5))
            .collect();
        assert_eq!(shown, ["┌─────", "│     ", "│     "]);

        let mut card = Card::new(6, 3);
        card.add(1, SURFACE, cells(6.0, 3.0), Style::default(), text("abc"));
        let image = WidgetContent::Image(ultty_protocol::ImageWidget {
            data: vec![0; 4],
            width: 1,
            height: 1,
            scale: ultty_protocol::ImageScale::Fit,
        });
        let id = card.add(2, SURFACE, huge, Style::default(), widget(image));
        if let Some(region) = card.scene.get_mut(id) {
            region.constraints.margin.top = Dimension::Cells(-3.0);
        }
        let rows = rows_within(&mut card, budget);
        assert!(rows.iter().all(|row| row.text_trimmed().is_empty()));

        let mut card = Card::new(6, 2);
        let log: String = (10..20_010).map(|n| format!("{n}\n")).collect();
        let id = card.add(1, SURFACE, cells(1.0e9, 3.0), Style::default(), text("-"));
        card.add(2, SURFACE, cells(6.0, 1.0e9), Style::default(), text(&log));
        if let Some(region) = card.scene.get_mut(id) {
            region.constraints.margin.bottom = Dimension::Cells(-2.0);
        }
        let shown: Vec<String> = rows_within(&mut card, budget)
            .iter()
            .map(|row| row.copy_text(0, 5))
            .collect();
        assert_eq!(shown, ["------", "10    "]);
    }

    /// Layout values past what fits in an `i32` are held to the most a
    /// surface could show, not overflowed (checked in debug builds).
    #[test]
    fn sizes_past_i32_do_not_overflow() {
        let mut card = Card::new(8, 2);
        let pad = Dimension::Cells(2.0);
        let style = Style {
            border: BorderStyle::thin(),
            ..Style::default()
        };
        let id = card.add(1, SURFACE, cells(3.0e9, 3.0e9), style, text("x\ny"));
        if let Some(region) = card.scene.get_mut(id) {
            region.constraints.padding = Edges {
                top: Dimension::Cells(1.0),
                right: pad,
                bottom: pad,
                left: pad,
            };
            region.constraints.margin.left = Dimension::Cells(-3.0e9);
        }
        let rich = RichText::new().text("a".repeat(70_000)).text("b");
        card.add(
            2,
            SURFACE,
            cells(3.0e9, 1.0),
            Style::default(),
            Content::RichText(rich),
        );
        card.rows();
    }

    /// A border takes no room in the layout, so text can sit on it; the
    /// border is drawn over it. Padding keeps the text clear of it.
    #[test]
    fn borders_are_drawn_over_content() {
        let border = Style {
            border: BorderStyle::thin(),
            ..Style::default()
        };
        let mut card = Card::new(7, 3);
        card.add(1, SURFACE, cells(7.0, 3.0), border.clone(), text("Hello"));
        card.add(
            2,
            RegionId::new(1),
            cells(7.0, 3.0),
            Style::default(),
            text("\nchild"),
        );
        assert_eq!(card.text(), ["┌─────┐", "│hild │", "└─────┘"]);

        let mut card = Card::new(7, 3);
        let id = card.add(1, SURFACE, cells(7.0, 3.0), border, text("Hello"));
        if let Some(region) = card.scene.get_mut(id) {
            let one = Dimension::Cells(1.0);
            region.constraints.padding = Edges {
                top: one,
                right: one,
                bottom: one,
                left: one,
            };
        }
        assert_eq!(card.text(), ["┌─────┐", "│Hello│", "└─────┘"]);
    }

    /// Text starting far left of its clip (a negative margin, or a region
    /// scrolled sideways) costs what it is and what of it shows, not how
    /// far left it starts: twenty regions over the same eight rows, each
    /// with eight lines starting 100 000 columns left of the surface.
    #[test]
    fn text_far_left_of_the_clip_costs_what_shows() {
        const LEFT: f32 = 100_000.0;
        let mut card = Card::new(14, 8);
        if let Some(root) = card.scene.get_mut(SURFACE) {
            root.constraints.display = ultty_protocol::Display::Flex;
            root.constraints.flex_direction = ultty_protocol::FlexDirection::Column;
        }
        let line = format!("{}visible {{}}", "a".repeat(LEFT as usize));
        for id in 101..=120u32 {
            let lines: Vec<String> = (0..8)
                .map(|row| line.replace("{}", &format!("{id}.{row}")))
                .collect();
            let region = card.add(
                id,
                SURFACE,
                cells(2.0 * LEFT, 8.0),
                Style::default(),
                text(&lines.join("\n")),
            );
            if let Some(region) = card.scene.get_mut(region) {
                region.constraints.margin.left = Dimension::Cells(-LEFT);
                if id > 101 {
                    region.constraints.margin.top = Dimension::Cells(-8.0);
                }
            }
        }
        let rows = rows_within(&mut card, std::time::Duration::from_secs(5));
        let shown: Vec<String> = rows.iter().map(|row| row.text_trimmed()).collect();
        let expected: Vec<String> = (0..8).map(|row| format!("visible 120.{row}")).collect();
        assert_eq!(shown, expected);
    }

    /// One-character fill lines cost what of them shows: 20 000 of them
    /// above the surface and a screenful on it, in a region a million
    /// columns wide that starts a million columns left of the surface.
    #[test]
    fn fill_lines_far_left_of_the_clip_cost_what_shows() {
        const LEFT: f32 = 1.0e6;
        let mut card = Card::new(8, 4);
        let lines = format!("{}yz\n-\n你\nz", "x\n".repeat(20_000));
        let id = card.add(
            1,
            SURFACE,
            cells(2.0 * LEFT + 8.0, 30_000.0),
            Style::default(),
            text(&lines),
        );
        if let Some(region) = card.scene.get_mut(id) {
            region.constraints.margin.left = Dimension::Cells(-LEFT);
            region.constraints.margin.top = Dimension::Cells(-20_000.0);
        }
        let rows = rows_within(&mut card, std::time::Duration::from_secs(2));
        let shown: Vec<String> = rows.iter().map(|row| row.copy_text(0, 7)).collect();
        // Plain text starts a million columns off; the fills reach in. The
        // wide fill takes two rows, as the renderer wraps it.
        assert_eq!(shown, ["        ", "--------", "你你你你", "你你你你"]);
        assert!(rows[2][0].is_wide() && rows[2][1].is_wide_spacer());
    }

    /// A fill line's visible part lines up with its characters: a wide
    /// fill starting at an odd column shows the halves cut by the clip as
    /// blanks, as the whole line drawn and clipped would.
    #[test]
    fn a_clipped_fill_keeps_its_columns() {
        for left in [0.0f32, 1.0, 3.0] {
            let mut card = Card::new(6, 2);
            let id = card.add(1, SURFACE, cells(12.0, 2.0), Style::default(), text("你"));
            if let Some(region) = card.scene.get_mut(id) {
                region.constraints.margin.left = Dimension::Cells(-left);
            }
            let rows = card.rows();
            let expected = if left == 0.0 { "你你你" } else { " 你你 " };
            assert_eq!(rows[0].copy_text(0, 5), expected, "{left} left");
        }
    }

    /// Text just left of its clip starts in the right column, wide
    /// characters and all, whether or not its start is skipped.
    #[test]
    fn text_left_of_the_clip_keeps_its_columns() {
        for left in [3usize, 2000] {
            let mut card = Card::new(6, 1);
            let line = format!("{}你b", "a".repeat(left));
            let id = card.add(1, SURFACE, cells(1.0e5, 1.0), Style::default(), text(&line));
            if let Some(region) = card.scene.get_mut(id) {
                region.constraints.margin.left = Dimension::Cells(-(left as f32 - 1.0));
            }
            let rows = card.rows();
            assert_eq!(rows[0].copy_text(0, 5), "a你b  ", "{left} columns left");
            assert!(rows[0][1].is_wide());
        }
    }
}
