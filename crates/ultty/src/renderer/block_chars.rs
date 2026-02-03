//! Procedural block character rendering.
//!
//! Returns fractional rectangles within a cell for Unicode block drawing characters.
//! These are rendered as solid-color quads instead of glyphs for pixel-perfect alignment.

const UL: (f32, f32, f32, f32) = (0.0, 0.0, 0.5, 0.5);
const UR: (f32, f32, f32, f32) = (0.5, 0.0, 0.5, 0.5);
const LL: (f32, f32, f32, f32) = (0.0, 0.5, 0.5, 0.5);
const LR: (f32, f32, f32, f32) = (0.5, 0.5, 0.5, 0.5);

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
