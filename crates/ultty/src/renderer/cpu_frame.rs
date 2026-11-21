//! First frames drawn on the CPU while the GPU starts (ADR 0030).
//!
//! Bringing up the GPU takes most of startup on some drivers (about 240 ms
//! on an NVIDIA T4). [`CpuFrame`] draws the focused terminal's grid and the
//! tab bar / window chrome into a pixel buffer meanwhile: cell backgrounds,
//! glyphs rasterised by swash from the same fonts the GPU renderer will use,
//! the cursor, and the CSD title band and tab row matching `tabbar`. It draws
//! nothing else (no split panes, images, decorations or selection); it only
//! has to hold the window cleanly until the GPU renderer takes over.
//!
//! Pixels are softbuffer's: `0xAARRGGBB` in a `u32`, sRGB. Alpha is always
//! opaque, so a window with an alpha visual (X11 depth 32) shows the frame
//! rather than nothing.

use cosmic_text::{Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent};
use foldhash::{HashMap, HashMapExt};
use ultty_protocol::control::PaneStatus;

use super::tabbar;
use super::text::font_attrs;
use super::{ColorScheme, Fonts, Rgba};
use crate::config::ColorConfig;
use crate::grid::{CellFlags, CursorShape, Terminal};
use crate::tabs::{
    TabBarState, TitleButton, STATUS_MARKER_SIZE, TAB_CLOSE_BUTTON_SIZE, TAB_PADDING,
};

/// Whether `terminal` shows anything yet: a frame of it shows the shell's
/// first output (its prompt, usually).
pub fn shows_output(terminal: &Terminal) -> bool {
    let grid = terminal.buffer().grid();
    (0..grid.dimensions().rows)
        .filter_map(|row| grid.row(row))
        .any(|row| (0..row.width()).any(|col| row.cell(col).is_some_and(|cell| !cell.is_blank())))
}

/// A glyph rasterised for the CPU frames.
struct Glyph {
    /// Offset of the image from the pen position: right, and up from the
    /// baseline.
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    /// One coverage byte per pixel, or four (RGBA) for a colour glyph.
    data: Vec<u8>,
    color: bool,
}

/// Draws a terminal's grid into a CPU pixel buffer; see the module docs.
pub struct CpuFrame {
    font_system: FontSystem,
    swash: SwashCache,
    metrics: Metrics,
    family: String,
    cell: (f32, f32),
    baseline: f32,
    colors: ColorScheme,
    /// Rasterised glyphs by character; `None` for one with no image (a
    /// space, or nothing in the fonts).
    glyphs: HashMap<char, Option<Glyph>>,
}

impl CpuFrame {
    /// Draw with the fonts `fonts` measured and the colours of `colors`.
    /// The font database is copied (its face list; no font is read again),
    /// so `fonts` can go on to the GPU renderer.
    pub fn new(fonts: &Fonts, colors: &ColorConfig) -> Self {
        Self {
            font_system: fonts.font_system_copy(),
            swash: SwashCache::new(),
            metrics: fonts.metrics(),
            family: fonts.family().to_owned(),
            cell: fonts.cell_dimensions(),
            baseline: fonts.baseline(),
            colors: ColorScheme::from_config(colors),
            glyphs: HashMap::with_capacity(128),
        }
    }

    /// Draw `terminal` (and `tab_bar` in `0..top` when present and visible)
    /// into `pixels`, a `width`-pixel-wide buffer, with the grid starting
    /// `top` pixels down (below the tab bar). Returns whether any grid cell
    /// had something to show: the first frame that does is the one that shows
    /// the shell's prompt.
    pub fn draw(
        &mut self,
        pixels: &mut [u32],
        width: u32,
        top: f32,
        terminal: &Terminal,
        tab_bar: Option<&TabBarState>,
    ) -> bool {
        let background = pack(self.colors.background);
        pixels.fill(background);
        let width = width as usize;
        if width == 0 {
            return false;
        }
        let height = pixels.len() / width;
        let mut canvas = Canvas {
            pixels,
            width,
            height,
        };
        if top > 0.0 {
            if let Some(tab_bar) = tab_bar.filter(|bar| bar.visible && bar.height() > 0.0) {
                self.draw_tab_bar(&mut canvas, tab_bar);
            }
        }
        let (cell_width, cell_height) = self.cell;
        let grid = terminal.buffer().grid();
        let cursor = grid.cursor();
        let cursor_cell = (cursor.visible && !terminal.buffer().is_scrolled()).then_some((
            cursor.position.col,
            cursor.position.row,
            cursor.shape,
        ));
        let dims = grid.dimensions();
        let mut drawn = false;
        for row in 0..dims.rows {
            let Some(view) = grid.row(row) else {
                continue;
            };
            let y = top + f32::from(row) * cell_height;
            for col in 0..view.width() {
                let Some(cell) = view.cell(col) else {
                    continue;
                };
                if cell.is_wide_spacer() {
                    continue;
                }
                let x = f32::from(col) * cell_width;
                let span = if cell.is_wide() { 2.0 } else { 1.0 };
                let reverse = cell.has_flag(CellFlags::REVERSE);
                let mut fg = self
                    .colors
                    .resolve(if reverse { cell.bg() } else { cell.fg() }, !reverse);
                let bg = self
                    .colors
                    .resolve(if reverse { cell.fg() } else { cell.bg() }, reverse);
                if bg != self.colors.background {
                    canvas.fill(x, y, cell_width * span, cell_height, pack(bg));
                }
                if cursor_cell == Some((col, row, CursorShape::Block)) {
                    canvas.fill(
                        x,
                        y,
                        cell_width * span,
                        cell_height,
                        pack(self.colors.cursor),
                    );
                    fg = self.colors.background;
                }
                let c = cell.char();
                if c == ' ' || cell.has_flag(CellFlags::HIDDEN) {
                    drawn |= !cell.is_blank();
                    continue;
                }
                drawn = true;
                self.draw_glyph(&mut canvas, c, x, y + self.baseline, fg);
            }
        }
        if let Some((col, row, shape)) = cursor_cell {
            let x = f32::from(col) * cell_width;
            let y = top + f32::from(row) * cell_height;
            let color = pack(self.colors.cursor);
            match shape {
                CursorShape::Block => {}
                CursorShape::Underline => {
                    canvas.fill(x, y + cell_height - 2.0, cell_width, 2.0, color);
                }
                CursorShape::Bar => canvas.fill(x, y, 2.0, cell_height, color),
            }
        }
        drawn
    }

    /// Draw the CSD title band and optional tab row matching `tabbar`.
    fn draw_tab_bar(&mut self, canvas: &mut Canvas<'_>, tab_bar: &TabBarState) {
        let bar_width = canvas.width as f32;
        let height = tab_bar.height();
        let title_bar_height = tab_bar.title_bar_height();
        let tab_row_height = tab_bar.tab_row_height();
        let px = |logical: f32| tab_bar.scaled(logical);
        let line = px(1.0);
        let show_tabs = tab_bar.shows_tab_row();
        let (cell_width, cell_height) = self.cell;

        let bg = self.colors.background;
        let fg = self.colors.foreground;
        let title_bg = tabbar::lighten(bg, 0.012);
        let tab_row_bg = tabbar::lighten(bg, 0.035);
        let rail_line = tabbar::lighten(bg, 0.075);
        let active_tab_bg = tabbar::lighten(bg, 0.058);
        let inactive_tab_bg = tabbar::lighten(bg, 0.02);
        let inactive_hover_bg = tabbar::lighten(bg, 0.055);
        let accent_color = tabbar::dim(self.colors.cursor, 0.92);
        let active_edge = tabbar::dim(fg, 0.18);
        let control_bg = tabbar::lighten(bg, 0.085);
        let control_hover_bg = tabbar::lighten(bg, 0.19);
        let control_border = tabbar::lighten(bg, 0.17);
        let control_hover_border = tabbar::lighten(bg, 0.26);
        let destructive_hover_bg = Rgba::new(0.55, 0.16, 0.14, 1.0);
        let destructive_hover_border = Rgba::new(0.78, 0.25, 0.23, 1.0);

        canvas.fill(0.0, 0.0, bar_width, title_bar_height, pack(title_bg));
        if show_tabs {
            canvas.fill(
                0.0,
                title_bar_height,
                bar_width,
                tab_row_height,
                pack(tab_row_bg),
            );
        }
        canvas.fill(0.0, height - line, bar_width, line, pack(rail_line));

        let tab_text_y = title_bar_height + (tab_row_height - cell_height) / 2.0;
        if show_tabs {
            for (tab_idx, tab) in tab_bar.tabs.iter().enumerate() {
                let tab_bg = if tab.is_active {
                    active_tab_bg
                } else if tab.is_hovered {
                    inactive_hover_bg
                } else {
                    inactive_tab_bg
                };
                let margin_x = line;
                let tab_y = title_bar_height + if tab.is_active { line } else { px(3.0) };
                let tab_height = if tab.is_active {
                    height - tab_y
                } else {
                    height - tab_y - line
                };
                canvas.fill(
                    tab.x + margin_x,
                    tab_y,
                    tab.width - margin_x * 2.0,
                    tab_height,
                    pack(tab_bg),
                );
                if tab.is_active {
                    canvas.fill(
                        tab.x + margin_x,
                        tab_y,
                        tab.width - margin_x * 2.0,
                        px(3.0),
                        pack(accent_color),
                    );
                    canvas.fill(tab.x + margin_x, tab_y, line, tab_height, pack(active_edge));
                    canvas.fill(
                        tab.x + tab.width - margin_x - line,
                        tab_y,
                        line,
                        tab_height,
                        pack(active_edge),
                    );
                }
                if let Some(status) = &tab.status {
                    let marker_box = (
                        tab.x + px(TAB_PADDING),
                        tab_text_y,
                        tab_bar.scaled(STATUS_MARKER_SIZE),
                        cell_height,
                    );
                    draw_status_marker(canvas, tab_bar, marker_box, &self.colors, status);
                    if let Some(progress) = status.progress.filter(|progress| *progress > 0.0) {
                        let track = tab.width - margin_x * 2.0;
                        canvas.fill(
                            tab.x + margin_x,
                            height - line - px(2.0),
                            (track * progress).round(),
                            px(2.0),
                            pack(tabbar::status_color(&self.colors, status)),
                        );
                    }
                }
                if tab_bar.hovered_close_button == Some(tab_idx) {
                    let (cx, cy, target_size, _) = tab.close_hover_bounds();
                    canvas.fill(cx, cy, target_size, target_size, pack(destructive_hover_bg));
                }

                let text_color = if tab.is_active {
                    fg
                } else {
                    tabbar::dim(fg, 0.62)
                };
                let max_text_width = tab.max_text_width();
                let title = tabbar::truncate_tab_title(&tab.title, max_text_width, cell_width);
                let text_x = tabbar::centered_tab_title_x(tab, title.as_ref(), cell_width);
                self.draw_label(
                    canvas,
                    title.as_ref(),
                    text_x,
                    tab_text_y,
                    max_text_width,
                    text_color,
                );

                let is_close_hovered = tab_bar.hovered_close_button == Some(tab_idx);
                let close_center_x = tab.close_button_x()
                    + tab_bar.scaled(TAB_CLOSE_BUTTON_SIZE) / 2.0
                    - cell_width / 2.0;
                let close_color = if is_close_hovered {
                    tabbar::lighten(fg, 0.08)
                } else {
                    tabbar::dim(fg, 0.36)
                };
                self.draw_glyph(
                    canvas,
                    '×',
                    close_center_x,
                    tab_text_y + cell_height,
                    close_color,
                );
            }
        }

        for (button, slot_x) in tab_bar.title_buttons() {
            let button_rect = tabbar::chrome_button_rect(tab_bar, slot_x);
            let is_hovered = tab_bar.hovered_title_button == Some(button)
                || (button == TitleButton::Menu && tab_bar.menu_open);
            let destructive = button == TitleButton::Close;
            let (button_bg, button_border) = match (is_hovered, destructive) {
                (true, true) => (destructive_hover_bg, destructive_hover_border),
                (true, false) => (control_hover_bg, control_hover_border),
                (false, _) => (control_bg, control_border),
            };
            draw_chrome_button(canvas, tab_bar, button_rect, button_bg, button_border);
            let symbol_color = if destructive {
                tabbar::close_symbol_color(fg, is_hovered)
            } else {
                tabbar::chrome_symbol_color(fg, accent_color, is_hovered)
            };
            match button {
                TitleButton::Menu => draw_menu_icon(canvas, tab_bar, button_rect, symbol_color),
                TitleButton::SplitRight => {
                    draw_split_icon(canvas, tab_bar, button_rect, true, symbol_color);
                }
                TitleButton::SplitDown => {
                    draw_split_icon(canvas, tab_bar, button_rect, false, symbol_color);
                }
                TitleButton::NewTab => {
                    self.draw_centered_glyph(canvas, '+', button_rect, symbol_color);
                }
                TitleButton::Minimize => {
                    self.draw_centered_glyph(canvas, '−', button_rect, symbol_color);
                }
                TitleButton::Maximize => {
                    self.draw_centered_glyph(canvas, '▢', button_rect, symbol_color);
                }
                TitleButton::Close => {
                    self.draw_centered_glyph(canvas, '×', button_rect, symbol_color);
                }
            }
        }

        let separator_height = tab_bar.chrome_button_size() - px(6.0);
        canvas.fill(
            tab_bar.title_group_separator_x(),
            (title_bar_height - separator_height) / 2.0,
            line,
            separator_height,
            pack(control_border),
        );

        let title_text_y = (title_bar_height - cell_height) / 2.0;
        let green = self.colors.named[2];
        let aqua = self.colors.named[6];
        let x0 = tabbar::title_logo_x(tab_bar);
        self.draw_glyph(canvas, '-', x0, title_text_y + cell_height, green);
        for i in 1..=3 {
            self.draw_glyph(
                canvas,
                '>',
                x0 + i as f32 * cell_width,
                title_text_y + cell_height,
                aqua,
            );
        }
        let letters = tab_bar.title_letters.min(5);
        if letters > 0 {
            self.draw_label(
                canvas,
                &"ultty"[..letters],
                x0 + 5.0 * cell_width,
                title_text_y,
                letters as f32 * cell_width + cell_width,
                fg,
            );
        }
        let cursor_column = if letters == 0 { 4 } else { 5 + letters };
        self.draw_glyph(
            canvas,
            '_',
            x0 + cursor_column as f32 * cell_width,
            title_text_y + cell_height,
            green,
        );

        let band = tabbar::title_band_layout(tab_bar, cell_width);
        if !band.path.is_empty() {
            let path_width = band.path.chars().count() as f32 * cell_width + cell_width;
            self.draw_label(
                canvas,
                &band.path,
                band.path_x,
                title_text_y,
                path_width,
                fg,
            );
        }
        if !band.suffix.is_empty() {
            let suffix_x = band.path_x + band.path.chars().count() as f32 * cell_width;
            let suffix_width = band.suffix.chars().count() as f32 * cell_width + cell_width;
            self.draw_label(
                canvas,
                &band.suffix,
                suffix_x,
                title_text_y,
                suffix_width,
                aqua,
            );
        }
        if let (Some(status), Some((marker_x, label))) = (&tab_bar.title_status, &band.status) {
            let marker_box = (*marker_x, title_text_y, cell_width, cell_height);
            draw_status_marker(canvas, tab_bar, marker_box, &self.colors, status);
            if !label.is_empty() {
                let label_width = label.chars().count() as f32 * cell_width + cell_width;
                self.draw_label(
                    canvas,
                    label,
                    marker_x + 2.0 * cell_width,
                    title_text_y,
                    label_width,
                    tabbar::dim(fg, 0.75),
                );
            }
        }

        if show_tabs {
            if tab_bar.new_tab_button_hovered {
                canvas.fill(
                    tab_bar.new_tab_button_x + px(2.0),
                    title_bar_height + px(3.0),
                    tab_bar.new_tab_button_width() - px(4.0),
                    tab_row_height - px(4.0),
                    pack(inactive_hover_bg),
                );
            }
            let plus_color =
                tabbar::chrome_symbol_color(fg, accent_color, tab_bar.new_tab_button_hovered);
            self.draw_centered_glyph(
                canvas,
                '+',
                (
                    tab_bar.new_tab_button_x,
                    title_bar_height,
                    tab_bar.new_tab_button_width(),
                    tab_row_height,
                ),
                plus_color,
            );
        }
    }

    fn ensure_glyph(&mut self, c: char) -> Option<&Glyph> {
        if !self.glyphs.contains_key(&c) {
            let glyph = self.rasterize(c);
            self.glyphs.insert(c, glyph);
        }
        self.glyphs.get(&c).and_then(Option::as_ref)
    }

    fn draw_label(
        &mut self,
        canvas: &mut Canvas<'_>,
        text: &str,
        x: f32,
        y: f32,
        max_width: f32,
        fg: Rgba,
    ) {
        let (cell_width, cell_height) = self.cell;
        let baseline = y + cell_height;
        let mut cursor_x = x;
        let max_x = x + max_width;
        for c in text.chars() {
            if cursor_x + cell_width > max_x + 0.5 {
                break;
            }
            if c != ' ' {
                self.draw_glyph(canvas, c, cursor_x, baseline, fg);
            }
            cursor_x += cell_width;
        }
    }

    fn draw_centered_glyph(
        &mut self,
        canvas: &mut Canvas<'_>,
        c: char,
        (x, y, width, height): (f32, f32, f32, f32),
        fg: Rgba,
    ) {
        let Some(glyph) = self.ensure_glyph(c) else {
            return;
        };
        let origin_x = (x + (width - glyph.width as f32) / 2.0).round() as i64;
        let origin_y = (y + (height - glyph.height as f32) / 2.0).round() as i64;
        blit_glyph(canvas, glyph, origin_x, origin_y, fg);
    }

    fn draw_glyph(&mut self, canvas: &mut Canvas<'_>, c: char, x: f32, baseline: f32, fg: Rgba) {
        let Some(glyph) = self.ensure_glyph(c) else {
            return;
        };
        let origin_x = x.round() as i64 + i64::from(glyph.left);
        let origin_y = baseline.round() as i64 - i64::from(glyph.top);
        blit_glyph(canvas, glyph, origin_x, origin_y, fg);
    }

    /// Shape `c` alone in the configured family (falling back as the GPU
    /// renderer's shaping does) and rasterise it with swash.
    fn rasterize(&mut self, c: char) -> Option<Glyph> {
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        let mut text = [0; 4];
        buffer.set_text(
            c.encode_utf8(&mut text),
            &font_attrs(&self.family),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        let run = buffer.layout_runs().next()?;
        let key = run.glyphs.first()?.physical((0.0, 0.0), 1.0).cache_key;
        let image = self.swash.get_image_uncached(&mut self.font_system, key)?;
        let color = match image.content {
            SwashContent::Mask => false,
            SwashContent::Color => true,
            // Not asked for; drawn as a coverage mask would be wrong.
            SwashContent::SubpixelMask => return None,
        };
        Some(Glyph {
            left: image.placement.left,
            top: image.placement.top,
            width: image.placement.width,
            height: image.placement.height,
            data: image.data,
            color,
        })
    }
}

/// The pixel buffer being drawn into.
struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
}

impl Canvas<'_> {
    /// Fill the rectangle at (`x`, `y`), `width` by `height` pixels, clipped
    /// to the buffer.
    fn fill(&mut self, x: f32, y: f32, width: f32, height: f32, color: u32) {
        let x0 = (x.round().max(0.0) as usize).min(self.width);
        let x1 = ((x + width).round().max(0.0) as usize).min(self.width);
        let y0 = (y.round().max(0.0) as usize).min(self.height);
        let y1 = ((y + height).round().max(0.0) as usize).min(self.height);
        for row in y0..y1 {
            self.pixels[row * self.width + x0..row * self.width + x1].fill(color);
        }
    }

    /// Fill a rounded rectangle clipped to the buffer.
    fn fill_rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: f32,
        color: u32,
    ) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let r = radius.max(0.0).min(width / 2.0).min(height / 2.0);
        if r <= 0.5 {
            self.fill(x, y, width, height, color);
            return;
        }
        let x0 = (x.round().max(0.0) as usize).min(self.width);
        let x1 = ((x + width).round().max(0.0) as usize).min(self.width);
        let y0 = (y.round().max(0.0) as usize).min(self.height);
        let y1 = ((y + height).round().max(0.0) as usize).min(self.height);
        let left_cx = x + r;
        let right_cx = x + width - r;
        let top_cy = y + r;
        let bottom_cy = y + height - r;
        let r2 = r * r;
        for py in y0..y1 {
            let cy = py as f32 + 0.5;
            let dy = if cy < top_cy {
                top_cy - cy
            } else if cy > bottom_cy {
                cy - bottom_cy
            } else {
                0.0
            };
            let row = py * self.width;
            if dy == 0.0 {
                self.pixels[row + x0..row + x1].fill(color);
                continue;
            }
            let dy2 = dy * dy;
            for px in x0..x1 {
                let cx = px as f32 + 0.5;
                let dx = if cx < left_cx {
                    left_cx - cx
                } else if cx > right_cx {
                    cx - right_cx
                } else {
                    0.0
                };
                if dx * dx + dy2 <= r2 {
                    self.pixels[row + px] = color;
                }
            }
        }
    }
}

/// Blit a rasterised glyph onto `canvas` with its top-left corner at
/// (`origin_x`, `origin_y`).
fn blit_glyph(canvas: &mut Canvas<'_>, glyph: &Glyph, origin_x: i64, origin_y: i64, fg: Rgba) {
    let fg = [fg.r, fg.g, fg.b].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u32);
    for gy in 0..glyph.height as i64 {
        let py = origin_y + gy;
        if py < 0 || py >= canvas.height as i64 {
            continue;
        }
        for gx in 0..glyph.width as i64 {
            let px = origin_x + gx;
            if px < 0 || px >= canvas.width as i64 {
                continue;
            }
            let at = (gy * glyph.width as i64 + gx) as usize;
            let (color, coverage) = if glyph.color {
                let rgba = &glyph.data[at * 4..at * 4 + 4];
                (
                    [rgba[0], rgba[1], rgba[2]].map(u32::from),
                    u32::from(rgba[3]),
                )
            } else {
                (fg, u32::from(glyph.data[at]))
            };
            if coverage == 0 {
                continue;
            }
            let index = py as usize * canvas.width + px as usize;
            canvas.pixels[index] = blend(canvas.pixels[index], color, coverage);
        }
    }
}

/// Draw a square status marker centered in `(x, y, width, height)`.
fn draw_status_marker(
    canvas: &mut Canvas<'_>,
    tab_bar: &TabBarState,
    (x, y, width, height): (f32, f32, f32, f32),
    colors: &ColorScheme,
    status: &PaneStatus,
) {
    let size = tab_bar.scaled(STATUS_MARKER_SIZE).round();
    canvas.fill(
        (x + (width - size) / 2.0).round(),
        (y + height * 0.56 - size / 2.0).round(),
        size,
        size,
        pack(tabbar::status_color(colors, status)),
    );
}

/// Draw a rounded title-band chrome button with border and fill.
fn draw_chrome_button(
    canvas: &mut Canvas<'_>,
    tab_bar: &TabBarState,
    rect: (f32, f32, f32, f32),
    fill: Rgba,
    border: Rgba,
) {
    let (x, y, width, height) = rect;
    let radius = tab_bar.scaled(tabbar::CHROME_BUTTON_RADIUS);
    let border_width = tab_bar.scaled(tabbar::CHROME_BUTTON_BORDER);
    canvas.fill_rounded_rect(x, y, width, height, radius, pack(border));
    let (inner_x, inner_y, inner_w, inner_h) = tabbar::chrome_button_inner_rect(rect, border_width);
    canvas.fill_rounded_rect(
        inner_x,
        inner_y,
        inner_w,
        inner_h,
        radius - border_width,
        pack(fill),
    );
}

/// Draw the three-bar menu icon inside `rect`.
fn draw_menu_icon(
    canvas: &mut Canvas<'_>,
    tab_bar: &TabBarState,
    (x, y, w, h): (f32, f32, f32, f32),
    color: Rgba,
) {
    let (icon_w, stroke, gap) = (
        tab_bar.scaled(14.0),
        tab_bar.scaled(2.0),
        tab_bar.scaled(3.0),
    );
    let left = (x + (w - icon_w) / 2.0).round();
    let top = (y + (h - (3.0 * stroke + 2.0 * gap)) / 2.0).round();
    let packed = pack(color);
    for i in 0..3 {
        let bar_y = top + i as f32 * (stroke + gap);
        canvas.fill(left, bar_y, icon_w, stroke, packed);
    }
}

/// Draw the split-pane outline icon inside `rect`.
fn draw_split_icon(
    canvas: &mut Canvas<'_>,
    tab_bar: &TabBarState,
    (x, y, w, h): (f32, f32, f32, f32),
    side_by_side: bool,
    color: Rgba,
) {
    let (icon_w, icon_h, stroke) = (
        tab_bar.scaled(14.0),
        tab_bar.scaled(12.0),
        tab_bar.scaled(1.5),
    );
    let left = (x + (w - icon_w) / 2.0).round();
    let top = (y + (h - icon_h) / 2.0).round();
    let packed = pack(color);
    for (rx, ry, rw, rh) in [
        (left, top, icon_w, stroke),
        (left, top + icon_h - stroke, icon_w, stroke),
        (left, top, stroke, icon_h),
        (left + icon_w - stroke, top, stroke, icon_h),
    ] {
        canvas.fill(rx, ry, rw, rh, packed);
    }
    if side_by_side {
        let mid = (left + (icon_w - stroke) / 2.0).round();
        canvas.fill(mid, top, stroke, icon_h, packed);
    } else {
        let mid = (top + (icon_h - stroke) / 2.0).round();
        canvas.fill(left, mid, icon_w, stroke, packed);
    }
}

/// An opaque softbuffer pixel.
fn pack(color: Rgba) -> u32 {
    let [r, g, b] = [color.r, color.g, color.b].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u32);
    0xff00_0000 | r << 16 | g << 8 | b
}

/// `color` over `pixel` at `coverage` (0 to 255), in sRGB.
fn blend(pixel: u32, color: [u32; 3], coverage: u32) -> u32 {
    let under = [(pixel >> 16) & 0xff, (pixel >> 8) & 0xff, pixel & 0xff];
    let [r, g, b] =
        std::array::from_fn(|i| (color[i] * coverage + under[i] * (255 - coverage) + 127) / 255);
    0xff00_0000 | r << 16 | g << 8 | b
}

#[cfg(test)]
mod tests {
    use super::{pack, CpuFrame};
    use crate::config::{ColorConfig, FontConfig};
    use crate::grid::{Dimensions, Terminal, TerminalHandler};
    use crate::protocol::ansi::AnsiParser;
    use crate::renderer::{tabbar, ColorScheme, Fonts};
    use crate::tabs::{TabBarState, TabState};
    use crate::terminal_manager::TabId;

    fn terminal_showing(bytes: &[u8]) -> Terminal {
        let mut terminal = Terminal::new(Dimensions::new(20, 4));
        let mut parser = AnsiParser::new();
        let mut handler = TerminalHandler::new(&mut terminal);
        parser.feed(bytes, |action| handler.handle(action));
        terminal
    }

    /// Draw `terminal` 20 cells wide and 4 high, under a 10 px tab bar.
    fn draw(terminal: &Terminal) -> (Vec<u32>, usize, (f32, f32), bool) {
        let colors = ColorConfig::default();
        let fonts = Fonts::load(&FontConfig::default());
        let cell = fonts.cell_dimensions();
        let mut frame = CpuFrame::new(&fonts, &colors);
        let width = (cell.0 * 20.0).ceil() as usize;
        let height = (10.0 + cell.1 * 4.0).ceil() as usize;
        let mut pixels = vec![0; width * height];
        let drawn = frame.draw(&mut pixels, width as u32, 10.0, terminal, None);
        (pixels, width, cell, drawn)
    }

    /// Pixels of cell (`col`, `row`) that are not the background.
    fn inked(pixels: &[u32], width: usize, cell: (f32, f32), col: usize, row: usize) -> usize {
        let background = pack(ColorScheme::from_config(&ColorConfig::default()).background);
        let (x0, y0) = (
            (col as f32 * cell.0) as usize,
            (10.0 + row as f32 * cell.1) as usize,
        );
        let (x1, y1) = (x0 + cell.0 as usize, y0 + cell.1 as usize);
        (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| y * width + x))
            .filter(|&at| pixels[at] != background)
            .count()
    }

    /// Hides the cursor (`DECTCEM` off) so only text inks the cells.
    #[test]
    fn a_prompt_is_drawn_in_its_cells_and_nowhere_else() {
        let terminal = terminal_showing(b"\x1b[?25l$ ls\r\nok");
        let (pixels, width, cell, drawn) = draw(&terminal);

        assert!(drawn, "the grid has text");
        for (col, row) in [(0, 0), (2, 0), (3, 0), (0, 1), (1, 1)] {
            assert!(
                inked(&pixels, width, cell, col, row) > 0,
                "cell {col},{row} drew a glyph"
            );
        }
        for (col, row) in [(1, 0), (5, 0), (2, 1), (0, 2), (19, 3)] {
            assert_eq!(
                inked(&pixels, width, cell, col, row),
                0,
                "cell {col},{row} is blank"
            );
        }
        assert!(
            pixels.iter().all(|pixel| pixel >> 24 == 0xff),
            "every pixel is opaque, for windows with an alpha visual"
        );
    }

    #[test]
    fn an_empty_grid_shows_only_the_background_and_cursor() {
        let terminal = terminal_showing(b"");
        let (pixels, width, cell, drawn) = draw(&terminal);

        assert!(!drawn, "nothing from the shell yet");
        let cursor = pack(ColorScheme::from_config(&ColorConfig::default()).cursor);
        let cursor_pixels = pixels.iter().filter(|&&pixel| pixel == cursor).count();
        let cell_area = (cell.0.round() * cell.1.round()) as usize;
        assert_eq!(cursor_pixels, cell_area, "a block cursor fills cell 0,0");
        assert_eq!(inked(&pixels, width, cell, 1, 0), 0);
    }

    fn test_tab_bar(width: f32, titles: &[&str], active: usize) -> TabBarState {
        let mut bar = TabBarState::new();
        bar.total_width = width;
        bar.visible = true;
        bar.active_index = active;
        let usable = (width - bar.new_tab_button_width()).max(0.0);
        let tab_width = (usable / titles.len().max(1) as f32).max(bar.min_tab_width());
        bar.tabs = titles
            .iter()
            .enumerate()
            .map(|(index, title)| TabState {
                id: TabId::new(index),
                title: (*title).to_string(),
                x: index as f32 * tab_width,
                width: tab_width,
                is_active: index == active,
                is_hovered: false,
                scale: 1.0,
                status: None,
            })
            .collect();
        bar.new_tab_button_x = (titles.len() as f32 * tab_width)
            .min(width - bar.new_tab_button_width())
            .max(0.0);
        bar
    }

    #[test]
    fn single_tab_draws_title_band_chrome_in_top_region() {
        let colors = ColorConfig::default();
        let scheme = ColorScheme::from_config(&colors);
        let fonts = Fonts::load(&FontConfig::default());
        let cell = fonts.cell_dimensions();
        let mut frame = CpuFrame::new(&fonts, &colors);
        let terminal = terminal_showing(b"\x1b[?25l$ ok");

        let width = 800usize;
        let tab_bar = test_tab_bar(width as f32, &["shell"], 0);
        let top = tab_bar.content_top();
        let height = (top + cell.1 * 4.0).ceil() as usize;
        let mut pixels = vec![0; width * height];

        let drawn = frame.draw(&mut pixels, width as u32, top, &terminal, Some(&tab_bar));
        assert!(drawn, "grid has prompt text");

        let bg = pack(scheme.background);
        let title_bg = pack(tabbar::lighten(scheme.background, 0.012));
        let rail_line = pack(tabbar::lighten(scheme.background, 0.075));
        let bar_h = tab_bar.height() as usize;

        assert!(
            pixels[..bar_h * width].contains(&title_bg),
            "title band fills 0..bar_height with title_bg"
        );
        let rail_row = (bar_h - 1) * width;
        assert!(
            pixels[rail_row..rail_row + width].contains(&rail_line),
            "bottom rail divider is drawn at height - 1"
        );
        let non_title_bg = pixels[..bar_h * width]
            .iter()
            .filter(|&&p| p != title_bg && p != rail_line)
            .count();
        assert!(
            non_title_bg > 0,
            "logo and chrome buttons are drawn inside the title band"
        );
        let padding_start = bar_h * width;
        let padding_end = top as usize * width;
        assert!(
            pixels[padding_start..padding_end].iter().all(|&p| p == bg),
            "terminal top padding stays in the terminal background"
        );
    }

    #[test]
    fn multi_tab_bar_draws_active_tab_pill_and_inks_active_title() {
        let colors = ColorConfig::default();
        let scheme = ColorScheme::from_config(&colors);
        let fonts = Fonts::load(&FontConfig::default());
        let cell = fonts.cell_dimensions();
        let mut frame = CpuFrame::new(&fonts, &colors);
        let terminal = terminal_showing(b"");

        let width = 800usize;
        let tab_bar = test_tab_bar(width as f32, &["alpha", "beta"], 0);
        let top = tab_bar.content_top();
        let height = (top + cell.1 * 4.0).ceil() as usize;
        let mut pixels = vec![0; width * height];

        let drawn = frame.draw(&mut pixels, width as u32, top, &terminal, Some(&tab_bar));
        assert!(
            !drawn,
            "tab bar chrome does not count as shell output from the grid"
        );

        let active_tab_bg = pack(tabbar::lighten(scheme.background, 0.058));
        let inactive_tab_bg = pack(tabbar::lighten(scheme.background, 0.02));
        let accent = pack(tabbar::dim(scheme.cursor, 0.92));

        let active_tab = &tab_bar.tabs[0];
        let inactive_tab = &tab_bar.tabs[1];
        let row_y0 = tab_bar.title_bar_height() as usize;
        let row_y1 = tab_bar.height() as usize;

        let active_region: Vec<u32> = (row_y0..row_y1)
            .flat_map(|y| {
                let x0 = active_tab.x as usize;
                let x1 = (active_tab.x + active_tab.width) as usize;
                pixels[y * width + x0..y * width + x1].iter().copied()
            })
            .collect();
        assert!(
            active_region.contains(&active_tab_bg),
            "active tab pill background is drawn"
        );
        assert!(
            active_region.contains(&accent),
            "active tab top accent bar is drawn"
        );

        let label_x0 = active_tab.label_x() as usize;
        let label_x1 = (active_tab.label_x() + active_tab.max_text_width()) as usize;
        let text_y0 = (tab_bar.title_bar_height() + 4.0) as usize;
        let inked_title_pixels = (text_y0..row_y1)
            .flat_map(|y| pixels[y * width + label_x0..y * width + label_x1].iter())
            .filter(|&&p| p != active_tab_bg)
            .count();
        assert!(
            inked_title_pixels > 0,
            "active tab title glyphs are inked inside the label area"
        );

        let inactive_region: Vec<u32> = (row_y0..row_y1)
            .flat_map(|y| {
                let x0 = inactive_tab.x as usize;
                let x1 = (inactive_tab.x + inactive_tab.width) as usize;
                pixels[y * width + x0..y * width + x1].iter().copied()
            })
            .collect();
        assert!(
            inactive_region.contains(&inactive_tab_bg),
            "inactive tab background is drawn"
        );
    }

    #[test]
    fn zero_top_skips_tab_bar_drawing() {
        let colors = ColorConfig::default();
        let scheme = ColorScheme::from_config(&colors);
        let fonts = Fonts::load(&FontConfig::default());
        let cell = fonts.cell_dimensions();
        let mut frame = CpuFrame::new(&fonts, &colors);
        let terminal = terminal_showing(b"\x1b[?25l");

        let width = 400usize;
        let height = (cell.1 * 4.0).ceil() as usize;
        let tab_bar = test_tab_bar(width as f32, &["shell"], 0);
        let mut pixels = vec![0; width * height];

        let drawn = frame.draw(&mut pixels, width as u32, 0.0, &terminal, Some(&tab_bar));
        assert!(!drawn);
        assert!(
            pixels.iter().all(|&p| p == pack(scheme.background)),
            "no tab bar pixels are drawn when top == 0"
        );
    }
}
