//! First frames drawn on the CPU while the GPU starts (ADR 0030).
//!
//! Bringing up the GPU takes most of startup on some drivers (about 240 ms
//! on an NVIDIA T4). [`CpuFrame`] draws the focused terminal's grid into a
//! pixel buffer meanwhile: cell backgrounds, glyphs rasterised by swash from
//! the same fonts the GPU renderer will use, and a block cursor. It draws
//! nothing else (no tab bar text, panes, images, decorations or selection);
//! it only has to hold the window until the GPU renderer takes over.
//!
//! Pixels are softbuffer's: `0xAARRGGBB` in a `u32`, sRGB. Alpha is always
//! opaque, so a window with an alpha visual (X11 depth 32) shows the frame
//! rather than nothing.

use cosmic_text::{Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent};
use foldhash::{HashMap, HashMapExt};

use super::text::font_attrs;
use super::{ColorScheme, Fonts, Rgba};
use crate::config::ColorConfig;
use crate::grid::{CellFlags, CursorShape, Terminal};

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

    /// Draw `terminal` into `pixels`, a `width`-pixel-wide buffer, with the
    /// grid starting `top` pixels down (below the tab bar). Returns whether
    /// any cell had something to show: the first frame that does is the
    /// one that shows the shell's prompt.
    pub fn draw(&mut self, pixels: &mut [u32], width: u32, top: f32, terminal: &Terminal) -> bool {
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

    fn draw_glyph(&mut self, canvas: &mut Canvas<'_>, c: char, x: f32, baseline: f32, fg: Rgba) {
        if !self.glyphs.contains_key(&c) {
            let glyph = self.rasterize(c);
            self.glyphs.insert(c, glyph);
        }
        let Some(Some(glyph)) = self.glyphs.get(&c) else {
            return;
        };
        let origin_x = x.round() as i64 + i64::from(glyph.left);
        let origin_y = baseline.round() as i64 - i64::from(glyph.top);
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
    use crate::renderer::{ColorScheme, Fonts};

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
        let drawn = frame.draw(&mut pixels, width as u32, 10.0, terminal);
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
}
