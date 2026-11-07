//! Underline and strikethrough geometry.
//!
//! Decorations are drawn as solid quads through the text pipeline, so they
//! sit on top of backgrounds and scale with the font like glyphs do. Every
//! shape is laid out per cell and repeats exactly across cells (the curly
//! wave has a period of one cell, dots and dashes divide the cell evenly),
//! so a run of decorated cells reads as one continuous line.

use crate::grid::UnderlineStyle;

/// Vertical metrics, in pixels from the cell top, used to place decorations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct DecorationMetrics {
    pub baseline: f32,
    pub x_height: f32,
    /// Line thickness; derived from the font size, so it follows the
    /// window's scale factor.
    pub thickness: f32,
}

/// `(x, y, width, height)` in pixels.
pub(super) type Rect = (f32, f32, f32, f32);

/// A quad with a coverage value per corner, in the order top-left,
/// top-right, bottom-right, bottom-left. Coverage multiplies the line
/// color's alpha and is interpolated across the quad, which is how the
/// curly underline gets anti-aliased edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Quad {
    pub corners: [[f32; 2]; 4],
    pub coverage: [f32; 4],
}

impl Quad {
    fn rect((x, y, width, height): Rect) -> Self {
        Self {
            corners: [
                [x, y],
                [x + width, y],
                [x + width, y + height],
                [x, y + height],
            ],
            coverage: [1.0; 4],
        }
    }
}

/// Append the quads of a cell's underline (`cell` is the cell's rectangle).
///
/// Straight lines start one pixel below the baseline; every shape stays
/// inside the cell so it never bleeds into the next row.
pub(super) fn underline_quads(
    style: UnderlineStyle,
    cell: Rect,
    metrics: DecorationMetrics,
    out: &mut Vec<Quad>,
) {
    let (cell_x, cell_y, width, height) = cell;
    let t = metrics.thickness;
    let lowest = cell_y + height - t;
    let first = (cell_y + metrics.baseline + 1.0).round().min(lowest);

    match style {
        UnderlineStyle::None => {}
        UnderlineStyle::Single => out.push(Quad::rect((cell_x, first, width, t))),
        UnderlineStyle::Double => {
            let second = (first + 2.0 * t).min(lowest);
            let first = second - 2.0 * t;
            out.push(Quad::rect((cell_x, first, width, t)));
            out.push(Quad::rect((cell_x, second, width, t)));
        }
        UnderlineStyle::Dotted => {
            // Square dots one line thick, as many as fit with an equal gap,
            // centred in equal slots so the spacing holds across cells.
            let count = (width / (2.0 * t)).floor().max(1.0);
            let pitch = width / count;
            for i in 0..count as usize {
                let x = (cell_x + i as f32 * pitch + (pitch - t) * 0.5).round();
                out.push(Quad::rect((x, first, t, t)));
            }
        }
        UnderlineStyle::Dashed => {
            // One dash per cell, centred: dashes and gaps are each half a
            // cell wide along the whole run.
            let dash = (width * 0.5).round().max(1.0);
            let x = (cell_x + (width - dash) * 0.5).round();
            out.push(Quad::rect((x, first, dash, t)));
        }
        UnderlineStyle::Curly => curly_quads(cell, first, t, out),
    }
}

/// The strikethrough quad, through the middle of the x-height.
pub(super) fn strikethrough_quad(cell: Rect, metrics: DecorationMetrics) -> Quad {
    let (cell_x, cell_y, width, height) = cell;
    let t = metrics.thickness;
    let lowest = cell_y + height - t;
    let y = (cell_y + metrics.baseline - metrics.x_height * 0.5 - t * 0.5).round();
    Quad::rect((cell_x, y.clamp(cell_y, lowest), width, t))
}

/// One period of a cosine wave per cell, lowest at both cell edges, so
/// neighbouring cells join without a seam.
///
/// The wave's centre line is sampled in short segments. Each segment is a
/// band whose vertical extent grows with the slope (so the stroke keeps its
/// thickness on the steep parts), with a one-pixel ramp to zero coverage on
/// both sides for anti-aliasing. The ramps hold half the coverage of a
/// pixel each, so the stroke's total weight still equals its thickness.
fn curly_quads(cell: Rect, first: f32, t: f32, out: &mut Vec<Quad>) {
    let (cell_x, cell_y, width, height) = cell;
    let half = t * 0.5;
    // The stroke's outer edge (half-thickness plus the half-pixel ramp)
    // touches the cell bottom at the troughs.
    let bottom = cell_y + height - half - 0.5;
    let room = bottom - (first + half);
    let amplitude = room.clamp(1.5 * t + 1.0, 3.0 * t + 1.0);
    let top = bottom - amplitude;

    let segments = (width * 0.5).round().clamp(4.0, 24.0) as usize;
    let sample = |i: usize| {
        let u = i as f32 / segments as f32;
        let phase = std::f32::consts::TAU * u;
        let x = cell_x + u * width;
        let y = top + amplitude * 0.5 * (1.0 + phase.cos());
        let slope = -amplitude * std::f32::consts::PI / width * phase.sin();
        let vertical_half = half * (1.0 + slope * slope).sqrt();
        let core = (vertical_half - 0.5).max(0.0);
        // Edges from top to bottom: outer, core, core, outer.
        (x, [y - core - 1.0, y - core, y + core, y + core + 1.0])
    };

    let mut left = sample(0);
    for i in 1..=segments {
        let right = sample(i);
        let (x0, l) = left;
        let (x1, r) = right;
        let band = |a: usize, b: usize, coverage_top: f32, coverage_bottom: f32| Quad {
            corners: [[x0, l[a]], [x1, r[a]], [x1, r[b]], [x0, l[b]]],
            coverage: [coverage_top, coverage_top, coverage_bottom, coverage_bottom],
        };
        out.push(band(0, 1, 0.0, 1.0));
        if l[2] > l[1] || r[2] > r[1] {
            out.push(band(1, 2, 1.0, 1.0));
        }
        out.push(band(2, 3, 1.0, 0.0));
        left = right;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const METRICS: DecorationMetrics = DecorationMetrics {
        baseline: 14.0,
        x_height: 8.0,
        thickness: 1.0,
    };
    const CELL: Rect = (0.0, 0.0, 10.0, 20.0);

    fn quads(style: UnderlineStyle, cell: Rect, metrics: DecorationMetrics) -> Vec<Quad> {
        let mut out = Vec::new();
        underline_quads(style, cell, metrics, &mut out);
        out
    }

    /// `(min_x, min_y, max_x, max_y)` over all corners.
    fn bounds(quads: &[Quad]) -> (f32, f32, f32, f32) {
        quads.iter().flat_map(|q| q.corners).fold(
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
            |(x0, y0, x1, y1), [x, y]| (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        )
    }

    #[test]
    fn single_underline_sits_one_pixel_below_baseline() {
        assert_eq!(
            quads(UnderlineStyle::Single, (0.0, 10.0, 8.0, 18.0), METRICS),
            vec![Quad::rect((0.0, 25.0, 8.0, 1.0))]
        );
    }

    #[test]
    fn no_underline_emits_nothing() {
        assert!(quads(UnderlineStyle::None, CELL, METRICS).is_empty());
    }

    #[test]
    fn strikethrough_crosses_mid_x_height() {
        let quad = strikethrough_quad((0.0, 10.0, 8.0, 18.0), METRICS);
        assert_eq!(quad, Quad::rect((0.0, 20.0, 8.0, 1.0)));
    }

    #[test]
    fn double_underline_stays_inside_the_cell() {
        let metrics = DecorationMetrics {
            baseline: 16.0,
            x_height: 8.0,
            thickness: 2.0,
        };
        let lines = quads(UnderlineStyle::Double, (0.0, 0.0, 8.0, 18.0), metrics);
        assert_eq!(lines.len(), 2);
        let (_, top, _, bottom) = bounds(&lines);
        assert!(top >= 0.0 && bottom <= 18.0, "{top}..{bottom}");
        let first = lines[0].corners[0][1];
        let second = lines[1].corners[0][1];
        assert_eq!(
            second - first,
            4.0,
            "a line-thickness gap between the lines"
        );
    }

    #[test]
    fn dotted_dots_are_evenly_spaced_across_cell_boundaries() {
        let mut out = Vec::new();
        for col in 0..3 {
            let cell = (col as f32 * 10.0, 0.0, 10.0, 20.0);
            underline_quads(UnderlineStyle::Dotted, cell, METRICS, &mut out);
        }
        let xs: Vec<f32> = out.iter().map(|q| q.corners[0][0]).collect();
        assert_eq!(xs.len(), 15);
        assert!(
            xs.windows(2).all(|w| w[1] - w[0] == 2.0),
            "dots must keep one pitch, also between cells: {xs:?}"
        );
    }

    #[test]
    fn dashes_and_gaps_alternate_evenly_across_cells() {
        let mut out = Vec::new();
        for col in 0..2 {
            let cell = (col as f32 * 10.0, 0.0, 10.0, 20.0);
            underline_quads(UnderlineStyle::Dashed, cell, METRICS, &mut out);
        }
        let spans: Vec<(f32, f32)> = out
            .iter()
            .map(|q| (q.corners[0][0], q.corners[1][0]))
            .collect();
        // 5 px dashes with 5 px gaps, the gap at the boundary split evenly.
        assert_eq!(spans, vec![(3.0, 8.0), (13.0, 18.0)]);
    }

    #[test]
    fn curly_wave_meets_its_neighbour_without_a_seam() {
        let left = quads(UnderlineStyle::Curly, (0.0, 0.0, 10.0, 20.0), METRICS);
        let right = quads(UnderlineStyle::Curly, (10.0, 0.0, 10.0, 20.0), METRICS);
        // The left cell's last column of edges is the right cell's first.
        let last_edges: Vec<[f32; 2]> = left[left.len() - 3..]
            .iter()
            .flat_map(|q| [q.corners[1], q.corners[2]])
            .collect();
        let first_edges: Vec<[f32; 2]> = right[..3]
            .iter()
            .flat_map(|q| [q.corners[0], q.corners[3]])
            .collect();
        for (a, b) in last_edges.iter().zip(&first_edges) {
            assert_eq!(a[0], b[0]);
            assert!((a[1] - b[1]).abs() < 1e-4, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn curly_wave_rises_and_falls_inside_the_cell() {
        let wave = quads(UnderlineStyle::Curly, CELL, METRICS);
        let (min_x, top, max_x, bottom) = bounds(&wave);
        assert_eq!((min_x, max_x), (0.0, 10.0));
        assert!(bottom <= 20.0, "stays above the next row: {bottom}");
        assert!(top >= METRICS.baseline - 1.0, "stays off the glyphs: {top}");
        // A wave, not a flat line: the centre at mid-cell is above the edges.
        let mid = wave.len() / 2;
        let centre = |q: &Quad| (q.corners[0][1] + q.corners[3][1]) * 0.5;
        assert!(centre(&wave[mid]) < centre(&wave[0]) - 1.0);
    }

    #[test]
    fn curly_wave_scales_with_line_thickness() {
        let hidpi = DecorationMetrics {
            baseline: 28.0,
            x_height: 16.0,
            thickness: 2.0,
        };
        let thin = bounds(&quads(UnderlineStyle::Curly, CELL, METRICS));
        let thick = bounds(&quads(UnderlineStyle::Curly, (0.0, 0.0, 20.0, 40.0), hidpi));
        assert!(thick.3 - thick.1 > thin.3 - thin.1);
        assert!(thick.3 <= 40.0);
    }
}
