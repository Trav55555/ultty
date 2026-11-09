//! Inline surfaces (ADR 0020, spec §10.6).
//!
//! A surface is a region tree anchored to rows of the main screen, `rows`
//! high and the pane's width wide. Its root is a second root of the
//! session's scene, created with `CreateSurface` and closed with
//! `CloseSurface` (or `DeleteRegion` on the root).

/// Fewest rows a surface may have.
pub const MIN_SURFACE_ROWS: u16 = 1;

/// Most rows a surface may have. A `CreateSurface` outside
/// `MIN_SURFACE_ROWS..=MAX_SURFACE_ROWS` is a decode error.
pub const MAX_SURFACE_ROWS: u16 = 128;

/// Most live surfaces one session may hold; one more is refused with
/// `Error` 400.
pub const MAX_SURFACES: usize = 16;

/// What happened to a surface, in a `SurfaceEvent`.
///
/// The discriminant is the wire value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SurfaceEventKind {
    /// The surface's anchor and its `CreateSurface` have both arrived.
    Placed = 0,
    /// The app closed the surface (`CloseSurface`, or `DeleteRegion` on its
    /// root).
    Closed = 1,
    /// The anchor row left scrollback or was erased; the surface is frozen.
    Evicted = 2,
    /// The surface scrolled off the screen.
    Hidden = 3,
    /// The surface scrolled back onto the screen.
    Shown = 4,
}

impl SurfaceEventKind {
    /// Every kind, in wire order.
    pub const ALL: [SurfaceEventKind; 5] = [
        SurfaceEventKind::Placed,
        SurfaceEventKind::Closed,
        SurfaceEventKind::Evicted,
        SurfaceEventKind::Hidden,
        SurfaceEventKind::Shown,
    ];

    /// The kind with wire value `value`, if there is one.
    pub fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.get(usize::from(value)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_are_the_discriminants() {
        for (index, kind) in SurfaceEventKind::ALL.into_iter().enumerate() {
            assert_eq!(kind as u8 as usize, index);
            assert_eq!(SurfaceEventKind::from_u8(kind as u8), Some(kind));
        }
        assert_eq!(SurfaceEventKind::from_u8(5), None);
        assert_eq!(SurfaceEventKind::from_u8(u8::MAX), None);
    }
}
