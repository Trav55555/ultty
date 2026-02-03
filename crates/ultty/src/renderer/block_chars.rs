//! Procedural block character rendering.
//!
//! Returns fractional rectangles within a cell for Unicode block drawing characters.
//! These are rendered as solid-color quads instead of glyphs for pixel-perfect alignment.

const UL: (f32, f32, f32, f32) = (0.0, 0.0, 0.5, 0.5);
const UR: (f32, f32, f32, f32) = (0.5, 0.0, 0.5, 0.5);
const LL: (f32, f32, f32, f32) = (0.0, 0.5, 0.5, 0.5);
const LR: (f32, f32, f32, f32) = (0.5, 0.5, 0.5, 0.5);

/// A fractional rectangle within a cell: `(x, y, width, height)` in 0.0..1.0.
pub type Rect4 = (f32, f32, f32, f32);

/// Returns fractional (x, y, w, h) rects within the cell. Up to 3 rects for multi-quadrant chars.
pub fn block_char_rects(ch: char) -> Option<([Rect4; 3], usize)> {
    let zero: Rect4 = (0.0, 0.0, 0.0, 0.0);
    let mut v = [zero; 3];
    let mut n = 0usize;
    macro_rules! push { ($r:expr) => { v[n] = $r; n += 1; } }
    match ch {
        '\u{2588}' => { push!((0.0, 0.0, 1.0, 1.0)); }      // █ full
        '\u{2580}' => { push!((0.0, 0.0, 1.0, 0.5)); }      // ▀ upper half
        '\u{2584}' => { push!((0.0, 0.5, 1.0, 0.5)); }      // ▄ lower half
        '\u{258C}' => { push!((0.0, 0.0, 0.5, 1.0)); }      // ▌ left half
        '\u{2590}' => { push!((0.5, 0.0, 0.5, 1.0)); }      // ▐ right half
        '\u{2581}' => { push!((0.0, 7.0/8.0, 1.0, 1.0/8.0)); }
        '\u{2582}' => { push!((0.0, 6.0/8.0, 1.0, 2.0/8.0)); }
        '\u{2583}' => { push!((0.0, 5.0/8.0, 1.0, 3.0/8.0)); }
        '\u{2585}' => { push!((0.0, 3.0/8.0, 1.0, 5.0/8.0)); }
        '\u{2586}' => { push!((0.0, 2.0/8.0, 1.0, 6.0/8.0)); }
        '\u{2587}' => { push!((0.0, 1.0/8.0, 1.0, 7.0/8.0)); }
        '\u{2589}' => { push!((0.0, 0.0, 7.0/8.0, 1.0)); }
        '\u{258A}' => { push!((0.0, 0.0, 6.0/8.0, 1.0)); }
        '\u{258B}' => { push!((0.0, 0.0, 5.0/8.0, 1.0)); }
        '\u{258D}' => { push!((0.0, 0.0, 3.0/8.0, 1.0)); }
        '\u{258E}' => { push!((0.0, 0.0, 2.0/8.0, 1.0)); }
        '\u{258F}' => { push!((0.0, 0.0, 1.0/8.0, 1.0)); }
        '\u{2594}' => { push!((0.0, 0.0, 1.0, 1.0/8.0)); }
        '\u{2595}' => { push!((7.0/8.0, 0.0, 1.0/8.0, 1.0)); }
        // Single quadrants
        '\u{2596}' => { push!(LL); }                          // ▖
        '\u{2597}' => { push!(LR); }                          // ▗
        '\u{2598}' => { push!(UL); }                          // ▘
        '\u{259D}' => { push!(UR); }                          // ▝
        // Multi-quadrant composites
        '\u{2599}' => { push!(UL); push!(LL); push!(LR); }   // ▙
        '\u{259A}' => { push!(UL); push!(LR); }              // ▚
        '\u{259B}' => { push!(UL); push!(UR); push!(LL); }   // ▛
        '\u{259C}' => { push!(UL); push!(UR); push!(LR); }   // ▜
        '\u{259E}' => { push!(UR); push!(LL); }              // ▞
        '\u{259F}' => { push!(UR); push!(LL); push!(LR); }   // ▟
        // Shade characters — fall through to font rendering
        '\u{2591}'..='\u{2593}' => { return None; }
        _ => { return None; }
    }
    Some((v, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: asserts that a character maps to exactly `expected_count` rectangles.
    fn assert_rect_count(ch: char, expected_count: usize) {
        let (rects, count) = block_char_rects(ch)
            .unwrap_or_else(|| panic!("expected Some for U+{:04X}", ch as u32));
        assert_eq!(
            count, expected_count,
            "U+{:04X}: expected {} rects, got {}",
            ch as u32, expected_count, count
        );
        // Verify unused slots are zeroed
        for (i, rect) in rects.iter().enumerate().skip(count) {
            assert_eq!(*rect, (0.0, 0.0, 0.0, 0.0), "unused slot {i} should be zero");
        }
    }

    /// Helper: asserts that a character maps to exactly one rect with the given values.
    fn assert_single_rect(ch: char, expected: Rect4) {
        let (rects, count) = block_char_rects(ch).unwrap();
        assert_eq!(count, 1, "expected 1 rect for U+{:04X}", ch as u32);
        let (x, y, w, h) = rects[0];
        let (ex, ey, ew, eh) = expected;
        assert!(
            (x - ex).abs() < 1e-5 && (y - ey).abs() < 1e-5
                && (w - ew).abs() < 1e-5 && (h - eh).abs() < 1e-5,
            "U+{:04X}: expected ({ex}, {ey}, {ew}, {eh}), got ({x}, {y}, {w}, {h})",
            ch as u32,
        );
    }

    #[test]
    fn full_block() {
        assert_single_rect('\u{2588}', (0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn upper_half_block() {
        assert_single_rect('\u{2580}', (0.0, 0.0, 1.0, 0.5));
    }

    #[test]
    fn lower_half_block() {
        assert_single_rect('\u{2584}', (0.0, 0.5, 1.0, 0.5));
    }

    #[test]
    fn left_half_block() {
        assert_single_rect('\u{258C}', (0.0, 0.0, 0.5, 1.0));
    }

    #[test]
    fn right_half_block() {
        assert_single_rect('\u{2590}', (0.5, 0.0, 0.5, 1.0));
    }

    #[test]
    fn fractional_lower_blocks() {
        // 1/8 lower block
        assert_single_rect('\u{2581}', (0.0, 7.0 / 8.0, 1.0, 1.0 / 8.0));
        // 2/8 lower block
        assert_single_rect('\u{2582}', (0.0, 6.0 / 8.0, 1.0, 2.0 / 8.0));
        // 3/8 lower block
        assert_single_rect('\u{2583}', (0.0, 5.0 / 8.0, 1.0, 3.0 / 8.0));
        // 5/8 lower block
        assert_single_rect('\u{2585}', (0.0, 3.0 / 8.0, 1.0, 5.0 / 8.0));
        // 6/8 lower block
        assert_single_rect('\u{2586}', (0.0, 2.0 / 8.0, 1.0, 6.0 / 8.0));
        // 7/8 lower block
        assert_single_rect('\u{2587}', (0.0, 1.0 / 8.0, 1.0, 7.0 / 8.0));
    }

    #[test]
    fn fractional_left_blocks() {
        // 7/8 left block
        assert_single_rect('\u{2589}', (0.0, 0.0, 7.0 / 8.0, 1.0));
        // 6/8 left block
        assert_single_rect('\u{258A}', (0.0, 0.0, 6.0 / 8.0, 1.0));
        // 5/8 left block
        assert_single_rect('\u{258B}', (0.0, 0.0, 5.0 / 8.0, 1.0));
        // 3/8 left block
        assert_single_rect('\u{258D}', (0.0, 0.0, 3.0 / 8.0, 1.0));
        // 2/8 left block
        assert_single_rect('\u{258E}', (0.0, 0.0, 2.0 / 8.0, 1.0));
        // 1/8 left block
        assert_single_rect('\u{258F}', (0.0, 0.0, 1.0 / 8.0, 1.0));
    }

    #[test]
    fn upper_and_right_eighth_blocks() {
        // Upper 1/8 block
        assert_single_rect('\u{2594}', (0.0, 0.0, 1.0, 1.0 / 8.0));
        // Right 1/8 block
        assert_single_rect('\u{2595}', (7.0 / 8.0, 0.0, 1.0 / 8.0, 1.0));
    }

    #[test]
    fn single_quadrants() {
        // Lower-left
        assert_single_rect('\u{2596}', (0.0, 0.5, 0.5, 0.5));
        // Lower-right
        assert_single_rect('\u{2597}', (0.5, 0.5, 0.5, 0.5));
        // Upper-left
        assert_single_rect('\u{2598}', (0.0, 0.0, 0.5, 0.5));
        // Upper-right
        assert_single_rect('\u{259D}', (0.5, 0.0, 0.5, 0.5));
    }

    #[test]
    fn multi_quadrant_two_rects() {
        // UL + LR (diagonal)
        assert_rect_count('\u{259A}', 2);
        // UR + LL (anti-diagonal)
        assert_rect_count('\u{259E}', 2);
    }

    #[test]
    fn multi_quadrant_three_rects() {
        // UL + LL + LR
        assert_rect_count('\u{2599}', 3);
        // UL + UR + LL
        assert_rect_count('\u{259B}', 3);
        // UL + UR + LR
        assert_rect_count('\u{259C}', 3);
        // UR + LL + LR
        assert_rect_count('\u{259F}', 3);
    }

    #[test]
    fn shade_characters_return_none() {
        // Light shade
        assert!(block_char_rects('\u{2591}').is_none());
        // Medium shade
        assert!(block_char_rects('\u{2592}').is_none());
        // Dark shade
        assert!(block_char_rects('\u{2593}').is_none());
    }

    #[test]
    fn non_block_characters_return_none() {
        assert!(block_char_rects('A').is_none());
        assert!(block_char_rects(' ').is_none());
        assert!(block_char_rects('\0').is_none());
        assert!(block_char_rects('█').is_some()); // sanity: full block IS recognized
    }

    #[test]
    fn all_rects_within_unit_square() {
        // Every returned rect should have coordinates in [0, 1]
        let chars: Vec<char> = ('\u{2580}'..='\u{259F}').collect();
        for ch in chars {
            if let Some((rects, count)) = block_char_rects(ch) {
                for (i, &(x, y, w, h)) in rects.iter().enumerate().take(count) {
                    assert!(
                        x >= 0.0 && y >= 0.0 && w >= 0.0 && h >= 0.0,
                        "U+{:04X} rect {i} has negative value: ({x}, {y}, {w}, {h})",
                        ch as u32,
                    );
                    assert!(
                        x + w <= 1.0 + 1e-5 && y + h <= 1.0 + 1e-5,
                        "U+{:04X} rect {i} exceeds unit square: ({x}, {y}, {w}, {h})",
                        ch as u32,
                    );
                }
            }
        }
    }
}
