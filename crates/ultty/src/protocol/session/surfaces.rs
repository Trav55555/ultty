//! Where a session's inline surfaces stand (ADR 0020): waiting for their
//! anchor, or placed over rows of the terminal and shown or hidden there.
//!
//! The session only keeps the state and tells the app; pairing anchors with
//! surfaces is [`crate::protocol::pairing`]'s job.

use std::io;
use std::time::{Duration, Instant};

use tracing::debug;
use ultty_protocol::{RegionId, SurfaceEventKind};

use super::errors::NativeProtocolError;
use super::NativeSession;

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
}

impl SurfaceState {
    pub(super) fn new(created_at: Instant) -> Self {
        Self {
            created_at,
            placed: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Placed {
    /// The anchor tag on its rows.
    tag: u16,
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
    pub fn place_surface(&mut self, id: RegionId, tag: u16) -> io::Result<()> {
        let Some(state) = self.surface_states.get_mut(&id) else {
            return Ok(());
        };
        state.placed = Some(Placed {
            tag,
            shown: true,
            line: None,
        });
        debug!("Surface {:?} placed on anchor tag {}", id, tag);
        self.render_dirty = true;
        self.mark_shown_changed();
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
    /// and tell the app. Until surfaces are rasterised into their rows
    /// (ADR 0020 slice 4) freezing drops the scene and leaves the fallback
    /// text the app printed.
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
        self.mark_shown_changed();
        let kind = if shown {
            SurfaceEventKind::Shown
        } else {
            SurfaceEventKind::Hidden
        };
        self.send_surface_event(id, kind)
    }
}
