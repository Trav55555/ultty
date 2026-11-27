//! Kitty graphics Unicode placeholders (`U+10EEEE`, ADR 0026 Slice 2).
//!
//! Translates placeholder cells in the visible grid into textured quads using
//! the official Kitty `rowcolumn-diacritics.txt` combining-mark table and the
//! three left-to-right diacritic inheritance rules.

use crate::grid::buffer::Buffer;
use crate::grid::cell::Color;
use crate::grid::graphics::store::VirtualPlacement;
use crate::grid::ImageId;

/// Private-use character marking a Kitty Unicode placeholder cell.
pub const PLACEHOLDER_CHAR: char = '\u{10EEEE}';

/// Combining diacritics encoding 0-based row, column, and high image-id byte
/// indices, in the exact order of Kitty's `gen/rowcolumn-diacritics.txt`.
pub const DIACRITICS: [char; 297] = [
    '\u{0305}',
    '\u{030D}',
    '\u{030E}',
    '\u{0310}',
    '\u{0312}',
    '\u{033D}',
    '\u{033E}',
    '\u{033F}',
    '\u{0346}',
    '\u{034A}',
    '\u{034B}',
    '\u{034C}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035B}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036A}',
    '\u{036B}',
    '\u{036C}',
    '\u{036D}',
    '\u{036E}',
    '\u{036F}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059C}',
    '\u{059D}',
    '\u{059E}',
    '\u{059F}',
    '\u{05A0}',
    '\u{05A1}',
    '\u{05A8}',
    '\u{05A9}',
    '\u{05AB}',
    '\u{05AC}',
    '\u{05AF}',
    '\u{05C4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065A}',
    '\u{065B}',
    '\u{065D}',
    '\u{065E}',
    '\u{06D6}',
    '\u{06D7}',
    '\u{06D8}',
    '\u{06D9}',
    '\u{06DA}',
    '\u{06DB}',
    '\u{06DC}',
    '\u{06DF}',
    '\u{06E0}',
    '\u{06E1}',
    '\u{06E2}',
    '\u{06E4}',
    '\u{06E7}',
    '\u{06E8}',
    '\u{06EB}',
    '\u{06EC}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073A}',
    '\u{073D}',
    '\u{073F}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074A}',
    '\u{07EB}',
    '\u{07EC}',
    '\u{07ED}',
    '\u{07EE}',
    '\u{07EF}',
    '\u{07F0}',
    '\u{07F1}',
    '\u{07F3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081B}',
    '\u{081C}',
    '\u{081D}',
    '\u{081E}',
    '\u{081F}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082A}',
    '\u{082B}',
    '\u{082C}',
    '\u{082D}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0F82}',
    '\u{0F83}',
    '\u{0F86}',
    '\u{0F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

/// Map a Kitty row/column combining diacritic to its 0-based index.
#[inline]
pub fn diacritic_index(c: char) -> Option<u16> {
    DIACRITICS.binary_search(&c).ok().map(|idx| idx as u16)
}

/// Map a 0-based index to its Kitty row/column combining diacritic.
#[inline]
pub fn index_to_diacritic(index: u16) -> Option<char> {
    DIACRITICS.get(usize::from(index)).copied()
}

/// A coalesced textured quad produced from visible `U+10EEEE` placeholder cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedPlaceholder {
    /// Internal image id (also the renderer's texture id).
    pub image: ImageId,
    /// Client image id (`i=`).
    pub client_id: u32,
    /// Placement id (`p=`).
    pub placement_id: u32,
    /// Stacking order relative to text (`z=`).
    pub z: i32,
    /// Drawn rectangle in visible viewport cells: `[col0, row0, col1, row1]`.
    pub screen: [f32; 4],
    /// Texture UV slice: `[u0, v0, u1, v1]`.
    pub uv: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PrevCell {
    fg: Color,
    underline: Color,
    img_row: u16,
    img_col: u16,
    msb: u8,
}

#[derive(Clone, Copy)]
struct RowRun {
    screen_row: u16,
    screen_col: u16,
    cols: u16,
    img_row: u16,
    img_col: u16,
    image: ImageId,
    client_id: u32,
    placement_id: u32,
}

#[derive(Clone, Copy)]
struct BlockRun {
    screen_row: u16,
    screen_col: u16,
    cols: u16,
    rows: u16,
    img_row: u16,
    img_col: u16,
    image: ImageId,
    client_id: u32,
    placement_id: u32,
}

fn color_to_id(color: Color) -> Option<u32> {
    match color {
        Color::Rgb(r, g, b) => Some((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)),
        Color::Indexed(index) | Color::Named(index) => Some(u32::from(index)),
        Color::Default => None,
    }
}

/// Resolve all visible `U+10EEEE` Unicode placeholder cells in `buffer` whose
/// virtual placement satisfies `include` into coalesced textured quads.
pub fn resolve_visible_placeholders(
    buffer: &Buffer,
    mut include: impl FnMut(&VirtualPlacement) -> bool,
) -> Vec<ResolvedPlaceholder> {
    let kitty = buffer.kitty_images();
    if !kitty.has_virtual_placements() {
        return Vec::new();
    }

    let dims = buffer.grid().dimensions();
    let mut row_runs: Vec<RowRun> = Vec::new();

    for screen_row in 0..dims.rows {
        let Some(row) = buffer.visible_row(screen_row) else {
            continue;
        };
        let mut prev: Option<PrevCell> = None;
        let max_col = row.width().min(dims.cols);

        for screen_col in 0..max_col {
            let Some(cell) = row.cell(screen_col) else {
                prev = None;
                continue;
            };
            if cell.char() != PLACEHOLDER_CHAR {
                prev = None;
                continue;
            }
            let fg = cell.fg();
            let Some(fg_id) = color_to_id(fg) else {
                prev = None;
                continue;
            };
            let underline = cell.underline_color();
            let requested_pid = color_to_id(underline).unwrap_or(0);
            let extra = row.grapheme_extra(screen_col).unwrap_or(&[]);
            let d0 = extra.first().copied().and_then(diacritic_index);
            let d1 = extra.get(1).copied().and_then(diacritic_index);
            let d2 = extra
                .get(2)
                .copied()
                .and_then(diacritic_index)
                .and_then(|v| u8::try_from(v).ok());

            let same_colors = |p: PrevCell| p.fg == fg && p.underline == underline;
            let (img_row, img_col, msb) = match (d0, d1, d2) {
                (None, _, _) => {
                    let Some(p) = prev.filter(|&p| same_colors(p)) else {
                        prev = None;
                        continue;
                    };
                    (p.img_row, p.img_col.saturating_add(1), p.msb)
                }
                (Some(r), None, _) => match prev.filter(|&p| same_colors(p) && p.img_row == r) {
                    Some(p) => (r, p.img_col.saturating_add(1), p.msb),
                    None => (r, 0, 0),
                },
                (Some(r), Some(c), None) => {
                    let inherited_msb = prev
                        .filter(|&p| {
                            same_colors(p) && p.img_row == r && p.img_col.saturating_add(1) == c
                        })
                        .map_or(0, |p| p.msb);
                    (r, c, inherited_msb)
                }
                (Some(r), Some(c), Some(m)) => (r, c, m),
            };

            prev = Some(PrevCell {
                fg,
                underline,
                img_row,
                img_col,
                msb,
            });

            let client_id = fg_id | (u32::from(msb) << 24);
            let Some((image, vp)) = kitty.virtual_placement_for(client_id, requested_pid) else {
                continue;
            };
            if !include(vp) || img_col >= vp.cols || img_row >= vp.rows {
                continue;
            }

            if let Some(last) = row_runs.last_mut() {
                if last.screen_row == screen_row
                    && last.screen_col.saturating_add(last.cols) == screen_col
                    && last.image == image.id
                    && last.client_id == vp.client_id
                    && last.placement_id == vp.placement_id
                    && last.img_row == img_row
                    && last.img_col.saturating_add(last.cols) == img_col
                {
                    last.cols = last.cols.saturating_add(1);
                    continue;
                }
            }
            row_runs.push(RowRun {
                screen_row,
                screen_col,
                cols: 1,
                img_row,
                img_col,
                image: image.id,
                client_id: vp.client_id,
                placement_id: vp.placement_id,
            });
        }
    }

    let mut blocks: Vec<BlockRun> = Vec::with_capacity(row_runs.len());
    for run in row_runs {
        if let Some(block) = blocks.iter_mut().rev().find(|block| {
            block.screen_col == run.screen_col
                && block.cols == run.cols
                && block.screen_row.saturating_add(block.rows) == run.screen_row
                && block.img_col == run.img_col
                && block.img_row.saturating_add(block.rows) == run.img_row
                && block.image == run.image
                && block.client_id == run.client_id
                && block.placement_id == run.placement_id
        }) {
            block.rows = block.rows.saturating_add(1);
        } else {
            blocks.push(BlockRun {
                screen_row: run.screen_row,
                screen_col: run.screen_col,
                cols: run.cols,
                rows: 1,
                img_row: run.img_row,
                img_col: run.img_col,
                image: run.image,
                client_id: run.client_id,
                placement_id: run.placement_id,
            });
        }
    }

    let mut quads = Vec::with_capacity(blocks.len());
    for block in blocks {
        let Some(image) = kitty.get(block.image) else {
            continue;
        };
        let Some(vp) = kitty.virtual_placement(block.image, block.placement_id) else {
            continue;
        };
        if image.width == 0 || image.height == 0 {
            continue;
        }
        let [dx, dy, dw, dh] = vp.dest;
        if !(dw > 0.0 && dh > 0.0) {
            continue;
        }
        let b_x0 = f32::from(block.img_col);
        let b_y0 = f32::from(block.img_row);
        let b_x1 = b_x0 + f32::from(block.cols);
        let b_y1 = b_y0 + f32::from(block.rows);

        let ix0 = dx.max(b_x0);
        let iy0 = dy.max(b_y0);
        let ix1 = (dx + dw).min(b_x1);
        let iy1 = (dy + dh).min(b_y1);
        if ix1 <= ix0 || iy1 <= iy0 {
            continue;
        }

        let draw_col0 = f32::from(block.screen_col) + (ix0 - b_x0);
        let draw_row0 = f32::from(block.screen_row) + (iy0 - b_y0);
        let draw_col1 = f32::from(block.screen_col) + (ix1 - b_x0);
        let draw_row1 = f32::from(block.screen_row) + (iy1 - b_y0);

        let [sx, sy, sw, sh] = vp.source.map(|v| v as f32);
        let tex_w = image.width as f32;
        let tex_h = image.height as f32;
        let u0 = (sx + sw * ((ix0 - dx) / dw)) / tex_w;
        let v0 = (sy + sh * ((iy0 - dy) / dh)) / tex_h;
        let u1 = (sx + sw * ((ix1 - dx) / dw)) / tex_w;
        let v1 = (sy + sh * ((iy1 - dy) / dh)) / tex_h;

        quads.push(ResolvedPlaceholder {
            image: block.image,
            client_id: vp.client_id,
            placement_id: vp.placement_id,
            z: vp.z,
            screen: [draw_col0, draw_row0, draw_col1, draw_row1],
            uv: [u0, v0, u1, v1],
        });
    }

    quads
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diacritics_table_is_strictly_sorted_and_roundtrips() {
        assert_eq!(DIACRITICS.len(), 297);
        assert_eq!(DIACRITICS[0], '\u{0305}');
        assert_eq!(DIACRITICS[1], '\u{030D}');
        assert_eq!(DIACRITICS[2], '\u{030E}');
        for pair in DIACRITICS.windows(2) {
            assert!(pair[0] < pair[1], "{pair:?} not sorted");
        }
        for (idx, &ch) in DIACRITICS.iter().enumerate() {
            assert_eq!(diacritic_index(ch), Some(idx as u16));
            assert_eq!(index_to_diacritic(idx as u16), Some(ch));
        }
        assert_eq!(diacritic_index('a'), None);
        assert_eq!(index_to_diacritic(297), None);
    }
}
