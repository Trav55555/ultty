//! Pixel and terminal grid geometry shared by the runtime and renderer.

/// Largest grid size in cells on either axis, whatever the window says.
pub const MAX_GRID_DIMENSION: u16 = 4096;

/// Compute grid columns/rows from pixel space and cell metrics, clamped to
/// `1..=MAX_GRID_DIMENSION`. Non-finite or non-positive inputs yield 1.
pub fn grid_dimensions_for(
    width_px: f32,
    height_px: f32,
    cell_width: f32,
    cell_height: f32,
) -> (u16, u16) {
    fn axis(px: f32, cell: f32) -> u16 {
        if !px.is_finite() || !cell.is_finite() || cell <= 0.0 || px <= 0.0 {
            return 1;
        }
        let cells = (px / cell).floor();
        if !cells.is_finite() || cells < 1.0 {
            1
        } else if cells >= MAX_GRID_DIMENSION as f32 {
            MAX_GRID_DIMENSION
        } else {
            cells as u16
        }
    }
    (axis(width_px, cell_width), axis(height_px, cell_height))
}
