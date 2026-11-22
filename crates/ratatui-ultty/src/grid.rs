//! Minimal CSS-grid track computation for placing arbitrary ratatui `Rect`s
//! at exact cell coordinates inside an Ultty full-scene layout.

use ratatui::layout::Rect;
use ultty_protocol::{
    Constraints, Dimension, Display, GridLine, GridPlacement, TrackSizingFunction,
};

/// Computed grid track layout for a viewport and a set of overlay rectangles.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OverlayGrid {
    pub(crate) cols: Vec<u16>,
    pub(crate) rows: Vec<u16>,
    pub(crate) col_tracks: Vec<TrackSizingFunction>,
    pub(crate) row_tracks: Vec<TrackSizingFunction>,
}

impl OverlayGrid {
    /// Build minimal column and row tracks covering `viewport` and every edge
    /// of `overlays`. Each unique boundary creates one cell-sized track so the
    /// grid uses at most `2 * overlays.len() + 1` tracks on each axis.
    pub(crate) fn build(viewport: Rect, overlays: &[Rect]) -> Self {
        let mut x_stops = Vec::with_capacity(overlays.len() * 2 + 2);
        let mut y_stops = Vec::with_capacity(overlays.len() * 2 + 2);
        x_stops.push(0);
        x_stops.push(viewport.width.max(1));
        y_stops.push(0);
        y_stops.push(viewport.height.max(1));

        for rect in overlays {
            let x0 = rect.x.min(viewport.width);
            let x1 = rect.right().min(viewport.width);
            let y0 = rect.y.min(viewport.height);
            let y1 = rect.bottom().min(viewport.height);
            x_stops.push(x0);
            x_stops.push(x1);
            y_stops.push(y0);
            y_stops.push(y1);
        }

        x_stops.sort_unstable();
        x_stops.dedup();
        y_stops.sort_unstable();
        y_stops.dedup();

        let col_tracks = tracks_from_stops(&x_stops);
        let row_tracks = tracks_from_stops(&y_stops);

        Self {
            cols: x_stops,
            rows: y_stops,
            col_tracks,
            row_tracks,
        }
    }

    /// Root container constraints spanning the entire viewport.
    pub(crate) fn root_constraints(&self) -> Constraints {
        Constraints {
            display: Display::Grid,
            width: Dimension::Percent(100.0),
            height: Dimension::Percent(100.0),
            grid_template_columns: self.col_tracks.clone(),
            grid_template_rows: self.row_tracks.clone(),
            ..Constraints::default()
        }
    }

    /// Placement covering every track of the grid (used for the ratatui backdrop).
    pub(crate) fn full_placement(&self) -> (GridPlacement, GridPlacement) {
        let col_end = (self.col_tracks.len() + 1) as i16;
        let row_end = (self.row_tracks.len() + 1) as i16;
        (
            GridPlacement {
                start: GridLine::Line(1),
                end: GridLine::Line(col_end),
            },
            GridPlacement {
                start: GridLine::Line(1),
                end: GridLine::Line(row_end),
            },
        )
    }

    /// Exact grid placement for an overlay `rect` clamped to the viewport.
    pub(crate) fn place(&self, rect: Rect) -> Option<(GridPlacement, GridPlacement)> {
        let max_x = *self.cols.last()?;
        let max_y = *self.rows.last()?;
        let x0 = rect.x.min(max_x);
        let x1 = rect.right().min(max_x);
        let y0 = rect.y.min(max_y);
        let y1 = rect.bottom().min(max_y);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let c0 = self.cols.binary_search(&x0).ok()? as i16 + 1;
        let c1 = self.cols.binary_search(&x1).ok()? as i16 + 1;
        let r0 = self.rows.binary_search(&y0).ok()? as i16 + 1;
        let r1 = self.rows.binary_search(&y1).ok()? as i16 + 1;
        Some((
            GridPlacement {
                start: GridLine::Line(c0),
                end: GridLine::Line(c1),
            },
            GridPlacement {
                start: GridLine::Line(r0),
                end: GridLine::Line(r1),
            },
        ))
    }
}

fn tracks_from_stops(stops: &[u16]) -> Vec<TrackSizingFunction> {
    stops
        .windows(2)
        .map(|pair| {
            let size = f32::from(pair[1].saturating_sub(pair[0]));
            TrackSizingFunction::Single(Dimension::Cells(size))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlay_grid_minimal_tracks_and_placement() {
        let viewport = Rect::new(0, 0, 120, 40);
        let log_area = Rect::new(20, 3, 100, 34);
        let grid = OverlayGrid::build(viewport, &[log_area]);

        assert_eq!(grid.cols, vec![0, 20, 120]);
        assert_eq!(grid.rows, vec![0, 3, 37, 40]);
        assert_eq!(grid.col_tracks.len(), 2);
        assert_eq!(grid.row_tracks.len(), 3);

        let (col, row) = grid.place(log_area).expect("valid placement");
        assert_eq!(col.start, GridLine::Line(2));
        assert_eq!(col.end, GridLine::Line(3));
        assert_eq!(row.start, GridLine::Line(2));
        assert_eq!(row.end, GridLine::Line(3));
    }

    #[test]
    fn test_overlay_grid_zero_area_returns_none() {
        let viewport = Rect::new(0, 0, 80, 24);
        let grid = OverlayGrid::build(viewport, &[Rect::new(10, 5, 0, 10)]);
        assert!(grid.place(Rect::new(10, 5, 0, 10)).is_none());
    }
}
