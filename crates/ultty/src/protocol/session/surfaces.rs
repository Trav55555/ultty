//! Where a session's inline surfaces stand (ADR 0020): waiting for their
//! anchor, or placed over rows of the terminal and shown or hidden there.
//!
//! The session keeps the state, tells the app, and rasterises a surface
//! whose scene changed into the cells its rows should hold; pairing anchors
//! with surfaces and writing those cells into the rows is
//! [`crate::protocol::pairing`]'s job.

use std::io;
use std::time::{Duration, Instant};

use tracing::{debug, warn};
use ultty_protocol::{RegionId, SurfaceEventKind};

use super::errors::NativeProtocolError;
use super::NativeSession;
use crate::grid::Row;
use crate::scene::raster;

/// How long a surface waits for its anchor, and an anchor for its surface,
/// before it is dropped.
pub const SURFACE_PAIRING_TIMEOUT: Duration = Duration::from_secs(2);

/// What the session knows of one surface beyond its scene.
#[derive(Debug, Clone, Copy)]
pub(super) struct SurfaceState {
    /// When `CreateSurface` was applied.
    created_at: Instant,
    /// Set once its anchor arrived.
    placed: Option<Placed>,
    /// Its scene changed since its rows were last written.
    changed: bool,
}

impl SurfaceState {
    pub(super) fn new(created_at: Instant) -> Self {
        Self {
            created_at,
            placed: None,
            changed: true,
        }
    }
}

/// The last rows of a placed surface that was closed, or whose session
/// ended, since they were last written: they are written over its anchor's
/// rows before the anchor is released.
#[derive(Debug)]
pub(super) struct FrozenSurface {
    id: RegionId,
    /// The placement it froze on (see `Buffer::anchor_serial`), so a
    /// surface placed later with the same id and tag never gets its rows.
    serial: u64,
    rows: Vec<Row>,
}

#[derive(Debug, Clone, Copy)]
struct Placed {
    /// The anchor tag on its rows.
    tag: u16,
    /// Which placement on that tag it is.
    serial: u64,
    /// Whether any of its rows is on the screen, as last told to the app.
    shown: bool,
    /// Its first row's line, counted from the oldest line the terminal
    /// holds, when last looked at.
    line: Option<usize>,
}

/// Where a surface stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfacePlacement {
    /// Its anchor has not arrived; it was created at `created_at`.
    Unplaced { rows: u16, created_at: Instant },
    /// Drawn over the rows tagged `tag`.
    Placed { rows: u16, tag: u16 },
}

/// A placed surface, for drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedSurface {
    pub id: RegionId,
    pub rows: u16,
    pub tag: u16,
}

impl NativeSession {
    /// Whether the session is full-scene: `ROOT` has children, so the grid
    /// and every surface are hidden under its tree.
    pub fn is_full_scene(&self) -> bool {
        self.scene
            .get(RegionId::ROOT)
            .is_some_and(|root| !root.children.is_empty())
    }

    /// Whether the session holds any surface, placed or not.
    pub fn has_surfaces(&self) -> bool {
        !self.scene.surfaces().is_empty()
    }

    /// Where surface `id` stands, if it is one.
    pub fn surface_placement(&self, id: RegionId) -> Option<SurfacePlacement> {
        let rows = self.scene.surface(id)?.rows;
        let state = self.surface_states.get(&id)?;
        Some(match state.placed {
            Some(placed) => SurfacePlacement::Placed {
                rows,
                tag: placed.tag,
            },
            None => SurfacePlacement::Unplaced {
                rows,
                created_at: state.created_at,
            },
        })
    }

    /// The placed surfaces, in creation order.
    pub fn placed_surfaces(&self) -> impl Iterator<Item = PlacedSurface> + '_ {
        self.scene.surfaces().iter().filter_map(|surface| {
            let placed = self.surface_states.get(&surface.id)?.placed?;
            Some(PlacedSurface {
                id: surface.id,
                rows: surface.rows,
                tag: placed.tag,
            })
        })
    }

    /// `ulttyctl regions`'s state of surface `id` and its first row's line.
    pub(super) fn surface_report(&self, id: RegionId) -> (&'static str, Option<u64>) {
        match self.surface_states.get(&id).and_then(|state| state.placed) {
            None => ("unplaced", None),
            Some(placed) => (
                if placed.shown { "placed" } else { "hidden" },
                placed.line.map(|line| line as u64),
            ),
        }
    }

    /// Place unplaced surface `id` over the rows tagged `tag` and tell the
    /// app. It counts as shown until [`NativeSession::set_surface_view`]
    /// says otherwise.
    pub fn place_surface(&mut self, id: RegionId, tag: u16, serial: u64) -> io::Result<()> {
        let Some(state) = self.surface_states.get_mut(&id) else {
            return Ok(());
        };
        state.placed = Some(Placed {
            tag,
            serial,
            shown: true,
            line: None,
        });
        // Its rows hold the fallback text until it is first written.
        state.changed = true;
        debug!("Surface {:?} placed on anchor tag {}", id, tag);
        self.render_dirty = true;
        self.send_surface_event(id, SurfaceEventKind::Placed)
    }

    /// Drop surface `id` with `Error` 400 saying why: its anchor was
    /// refused, its rows disagree, or its anchor never came.
    pub fn refuse_surface(&mut self, id: RegionId, reason: &str) {
        if self.remove_tree(id) {
            self.queue_protocol_error(NativeProtocolError::bad_request(format!(
                "surface {}: {}",
                id.0, reason
            )));
        }
    }

    /// Freeze surface `id`, whose anchor row left scrollback or was erased,
    /// and tell the app. Its rows are gone or cleared, so they keep what
    /// was last written to them, if anything.
    pub fn evict_surface(&mut self, id: RegionId) -> io::Result<()> {
        if self.remove_tree(id) {
            self.send_surface_event(id, SurfaceEventKind::Evicted)?;
        }
        Ok(())
    }

    /// Drop every surface whose anchor has not come within
    /// [`SURFACE_PAIRING_TIMEOUT`] of its creation, as of `now`.
    pub fn expire_unplaced_surfaces(&mut self, now: Instant) {
        let expired: Vec<RegionId> = self
            .surface_states
            .iter()
            .filter(|(_, state)| {
                state.placed.is_none()
                    && now.saturating_duration_since(state.created_at) >= SURFACE_PAIRING_TIMEOUT
            })
            .map(|(&id, _)| id)
            .collect();
        for id in expired {
            self.refuse_surface(id, "no anchor arrived within 2 s");
        }
    }

    /// Record where placed surface `id`'s first row is (`line`) and whether
    /// any of its rows is on the screen; the app hears `Hidden` or `Shown`
    /// only when that changes.
    pub fn set_surface_view(
        &mut self,
        id: RegionId,
        line: Option<usize>,
        shown: bool,
    ) -> io::Result<()> {
        let Some(placed) = self
            .surface_states
            .get_mut(&id)
            .and_then(|state| state.placed.as_mut())
        else {
            return Ok(());
        };
        placed.line = line;
        if placed.shown == shown {
            return Ok(());
        }
        placed.shown = shown;
        let kind = if shown {
            SurfaceEventKind::Shown
        } else {
            SurfaceEventKind::Hidden
        };
        self.send_surface_event(id, kind)
    }

    /// Note that the scene of the surface region `id` is in (if any)
    /// changed, so its rows are written again.
    pub(super) fn note_surface_change(&mut self, id: RegionId) {
        if let Some(state) = self
            .scene
            .surface_of(id)
            .and_then(|root| self.surface_states.get_mut(&root))
        {
            state.changed = true;
        }
    }

    /// Note that every surface changed, as a new width lays them all out
    /// again.
    pub(super) fn note_all_surfaces_changed(&mut self) {
        for state in self.surface_states.values_mut() {
            state.changed = true;
        }
    }

    /// The cells placed surface `id`'s rows should hold, `cols` wide, if
    /// its scene changed since it was last asked; `None` when it did not,
    /// so an unchanged frame costs nothing.
    pub fn take_surface_rows(&mut self, id: RegionId, cols: u16) -> Option<Vec<Row>> {
        let state = self.surface_states.get_mut(&id)?;
        if state.placed.is_none() || !state.changed {
            return None;
        }
        state.changed = false;
        self.rasterise_surface(id, cols)
    }

    /// Surface `id`'s scene as `cols` cells by its rows, laid out first
    /// if a change is waiting for layout.
    fn rasterise_surface(&mut self, id: RegionId, cols: u16) -> Option<Vec<Row>> {
        let rows = self.scene.surface(id)?.rows;
        if self.scene_dirty {
            // Laid out quietly: the frame or resize that dirtied the scene
            // still sends its LayoutComputed.
            if let Err(e) = self.layout.compute_scene_layout(
                &mut self.scene,
                self.viewport_width,
                self.viewport_height,
            ) {
                warn!("Session {} layout error: {}", self.session_id, e);
            }
        }
        Some(raster::rasterise_surface(
            &self.scene,
            id,
            cols,
            rows,
            |region| self.scroll_offset(region),
        ))
    }

    /// Keep the last rows of placed surface `id`, about to be closed or
    /// to end with its session, if they were not written yet, for
    /// [`NativeSession::take_frozen_surface`].
    pub(super) fn freeze_surface(&mut self, id: RegionId) {
        let Some(state) = self.surface_states.get_mut(&id) else {
            return;
        };
        let Some(placed) = state.placed.filter(|_| state.changed) else {
            return;
        };
        state.changed = false;
        let cols = self.viewport_width.clamp(1.0, f32::from(u16::MAX)) as u16;
        if let Some(rows) = self.rasterise_surface(id, cols) {
            self.frozen_surfaces.push(FrozenSurface {
                id,
                serial: placed.serial,
                rows,
            });
        }
    }

    /// Freeze every placed surface where it stands: the session is ending.
    pub fn freeze_surfaces(&mut self) {
        let ids: Vec<RegionId> = self.scene.surfaces().iter().map(|s| s.id).collect();
        for id in ids {
            self.freeze_surface(id);
        }
    }

    /// The last rows of surface `id`, placed as anchor placement
    /// `serial`, kept when it was closed or its session ended; `None` if
    /// its rows already hold them. Taking them is also how they are
    /// dropped when their rows are gone.
    pub fn take_frozen_surface(&mut self, id: RegionId, serial: u64) -> Option<Vec<Row>> {
        let index = self
            .frozen_surfaces
            .iter()
            .position(|frozen| frozen.id == id && frozen.serial == serial)?;
        Some(self.frozen_surfaces.swap_remove(index).rows)
    }

    /// Rows kept for closed surfaces and not yet taken.
    #[cfg(test)]
    pub(crate) fn frozen_surface_count(&self) -> usize {
        self.frozen_surfaces.len()
    }
}
