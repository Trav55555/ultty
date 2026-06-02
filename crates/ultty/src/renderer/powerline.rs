//! Procedural rendering helpers for Powerline cell separators.
//!
//! Full Powerline separators are intended to fill a terminal cell edge-to-edge.
//! Rendering them as font glyphs can expose font side bearings and vertical
//! metrics, so the GPU renderer draws them as solid triangles instead.

/// Direction for full Powerline triangle separators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerlineTriangle {
    /// U+E0B0, a filled triangle pointing right.
    Right,
    /// U+E0B2, a filled triangle pointing left.
    Left,
}

/// Return the procedural triangle direction for a Powerline separator.
pub fn powerline_triangle(ch: char) -> Option<PowerlineTriangle> {
    match ch {
        '\u{E0B0}' => Some(PowerlineTriangle::Right),
        '\u{E0B2}' => Some(PowerlineTriangle::Left),
        _ => None,
    }
}

/// Check whether a character is rendered procedurally as a Powerline triangle.
pub fn is_powerline_triangle(ch: char) -> bool {
    powerline_triangle(ch).is_some()
}

/// Return fractional triangle points within a cell.
pub fn triangle_points(direction: PowerlineTriangle) -> [(f32, f32); 3] {
    match direction {
        PowerlineTriangle::Right => [(0.0, 0.0), (0.0, 1.0), (1.0, 0.5)],
        PowerlineTriangle::Left => [(1.0, 0.0), (1.0, 1.0), (0.0, 0.5)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_full_powerline_triangles() {
        assert_eq!(
            powerline_triangle('\u{E0B0}'),
            Some(PowerlineTriangle::Right)
        );
        assert_eq!(
            powerline_triangle('\u{E0B2}'),
            Some(PowerlineTriangle::Left)
        );
        assert_eq!(powerline_triangle('\u{E0B1}'), None);
        assert_eq!(powerline_triangle('A'), None);
    }

    #[test]
    fn triangle_points_fill_cell_edges() {
        assert_eq!(
            triangle_points(PowerlineTriangle::Right),
            [(0.0, 0.0), (0.0, 1.0), (1.0, 0.5)]
        );
        assert_eq!(
            triangle_points(PowerlineTriangle::Left),
            [(1.0, 0.0), (1.0, 1.0), (0.0, 0.5)]
        );
    }
}
