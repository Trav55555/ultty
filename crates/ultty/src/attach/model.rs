//! Complete replacement snapshots for attachment. Capture locks every PTY in
//! stable pane order before copying any screen; native state belongs to the
//! calling owner thread. No parser bytes, pending frames or credentials cross
//! this boundary. A publication sequence identifies a snapshot, not a PTY event.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use ultty_protocol::control::{OwnerRef, PaneStatus};
use ultty_vt::grid::snapshot::TerminalSnapshot;

use super::native::NativeSnapshot;
use crate::runtime::SessionRuntime;

/// Snapshot schema version, independent of the native app protocol.
pub const SNAPSHOT_VERSION: u16 = 1;
/// Most panes a complete attachment can retain.
pub const MAX_ATTACH_PANES: usize = 128;
/// Most bytes in an encoded complete replacement, enforced by the writer too.
pub const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// A consistent view of one live owner. Publishing never migrates processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Snapshot schema version.
    pub version: u16,
    /// Process lifetime identity; clients must never silently replace it.
    pub owner: OwnerRef,
    /// Strictly increasing publication sequence for this owner.
    pub sequence: u64,
    /// Canonical viewport columns and rows, controlled by one attachment.
    pub viewport: [u16; 2],
    /// Controller's canonical pixel width and height per cell.
    pub cell_pixels: [u16; 2],
    /// Selected tab, or none when the host has no processes.
    pub active_tab: Option<u64>,
    /// Tabs in display order.
    pub tabs: Vec<TabSnapshot>,
    /// Every pane, including hidden tabs, with its committed native sessions.
    pub panes: Vec<PaneSnapshot>,
}

/// Tab layout metadata. Rectangles are canonical cell coordinates, not pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabSnapshot {
    /// Stable owner-scoped tab identity.
    pub id: u64,
    /// Display title.
    pub title: String,
    /// Focused pane identity.
    pub focused: u64,
    /// Panes in layout order; zoomed-out siblings remain in this list.
    pub panes: Vec<u64>,
}

/// One process and its rendering state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneSnapshot {
    /// Stable owner-scoped pane identity.
    pub id: u64,
    /// Owning tab.
    pub tab: u64,
    /// Actual child process identity, unchanged by viewer detach/reattach.
    pub pid: u32,
    /// Authoritative terminal observation generation at the snapshot cut.
    pub revision: u64,
    /// Display title.
    pub title: String,
    /// Chrome status projection, separately from process and connection state.
    pub status: Option<PaneStatus>,
    /// Rectangle [x,y,width,height] within its tab, or none while zoom-hidden.
    pub rect: Option<[f32; 4]>,
    /// Both VT screens, history and display/input modes.
    pub terminal: TerminalSnapshot,
    /// Committed native trees; application links and pending frames stay local.
    pub native: Vec<NativeSnapshot>,
}

impl SessionSnapshot {
    /// Copy a consistent cut of all pane screens and committed native scenes.
    /// Call on the runtime owner thread, between mutation passes. All terminal
    /// locks are dropped before encoding or writing to any viewer.
    pub fn capture(
        runtime: &SessionRuntime,
        owner: OwnerRef,
        sequence: u64,
        viewport: [u16; 2],
        cell_pixels: [u16; 2],
    ) -> Result<Self, String> {
        let manager = runtime.manager();
        let managed: Vec<_> = manager
            .tabs()
            .iter()
            .flat_map(|tab| &tab.terminals)
            .map(|id| {
                manager
                    .terminal(*id)
                    .ok_or("layout references missing pane")
            })
            .collect::<Result<_, _>>()?;
        if managed.len() > MAX_ATTACH_PANES {
            return Err("owner exceeds attachment pane limit".into());
        }
        // Fixed tab/layout order, no duplicate pane locks. Manager invariants
        // prevent duplicates; validate explicitly before taking any locks.
        let mut ids = HashSet::new();
        if managed.iter().any(|pane| !ids.insert(pane.id)) {
            return Err("duplicate owner pane identity".into());
        }
        let guards: Vec<_> = managed
            .iter()
            .map(|pane| pane.event_loop.terminal())
            .collect();
        let tabs = manager
            .tabs()
            .iter()
            .map(|tab| TabSnapshot {
                id: tab.id.raw() as u64,
                title: tab.title.clone(),
                focused: tab.focused_terminal.raw() as u64,
                panes: tab.terminals.iter().map(|id| id.raw() as u64).collect(),
            })
            .collect();
        let mut budget = SnapshotBudget(MAX_SNAPSHOT_BYTES);
        budget.account(&tabs)?;
        let mut panes = Vec::with_capacity(managed.len());
        for (managed, terminal) in managed.iter().zip(&guards) {
            let tab = manager
                .tabs()
                .iter()
                .find(|tab| tab.id == managed.tab_id)
                .ok_or("pane has no tab")?;
            let rect = tab
                .visible_rects((0.0, 0.0, f32::from(viewport[0]), f32::from(viewport[1])))
                .into_iter()
                .find(|(id, _)| *id == managed.id)
                .map(|(_, (x, y, w, h))| [x, y, w, h]);
            let mut native = Vec::new();
            if let Some(server) = runtime.protocol() {
                for session in server.all_pane_sessions(managed.id) {
                    if native.len() == MAX_ATTACH_PANES {
                        return Err("too many native sessions to attach".into());
                    }
                    let captured = NativeSnapshot::capture(session)?;
                    budget.account(&captured)?;
                    native.push(captured);
                }
            }
            let revision = terminal.snapshot_generation();
            let terminal = TerminalSnapshot::capture(terminal)?;
            budget.account(&terminal)?;
            panes.push(PaneSnapshot {
                id: managed.id.raw() as u64,
                tab: managed.tab_id.raw() as u64,
                pid: managed.event_loop.child_pid(),
                revision,
                title: managed.title.clone(),
                status: managed.shown_status().cloned(),
                rect,
                terminal,
                native,
            });
        }
        let snapshot = Self {
            version: SNAPSHOT_VERSION,
            owner,
            sequence,
            viewport,
            cell_pixels,
            active_tab: manager.active_tab().map(|tab| tab.id.raw() as u64),
            tabs,
            panes,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Validate identity, layout and size metadata before hydrating any models.
    /// Each terminal and native snapshot separately validates its semantic data.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != SNAPSHOT_VERSION
            || self.viewport.contains(&0)
            || self.viewport[0] > 4096
            || self.viewport[1] > 4096
            || self
                .cell_pixels
                .iter()
                .any(|value| !(1..=512).contains(value))
            || u32::from(self.viewport[0]) * u32::from(self.cell_pixels[0]) > u32::from(u16::MAX)
            || u32::from(self.viewport[1]) * u32::from(self.cell_pixels[1]) > u32::from(u16::MAX)
            || self.panes.len() > MAX_ATTACH_PANES
            || self.tabs.len() > MAX_ATTACH_PANES
            || self.owner.epoch.len() != 64
            || !self.owner.epoch.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("invalid attachment snapshot header".into());
        }
        let mut tabs = HashSet::new();
        let mut panes = HashSet::new();
        let mut listed = HashSet::new();
        for tab in &self.tabs {
            if !tabs.insert(tab.id)
                || tab.title.len() > 65536
                || tab.panes.is_empty()
                || !tab.panes.contains(&tab.focused)
            {
                return Err("invalid snapshot tab identity/focus".into());
            }
            for pane in &tab.panes {
                if !listed.insert(*pane)
                    || !self.panes.iter().any(|p| p.id == *pane && p.tab == tab.id)
                {
                    return Err("invalid snapshot pane membership".into());
                }
            }
        }
        let mut sessions = HashSet::new();
        let mut remaining_cells = crate::grid::snapshot::MAX_VIEW_CELLS;
        for pane in &self.panes {
            // Blank trailing cells are compact on the wire. Bound expansion
            // across the entire replacement, not independently per pane.
            for buffer in [&pane.terminal.main, &pane.terminal.alternate] {
                let cells = buffer
                    .rows
                    .len()
                    .checked_mul(usize::from(buffer.cols))
                    .ok_or("snapshot cell count overflow")?;
                remaining_cells = remaining_cells
                    .checked_sub(cells)
                    .ok_or("snapshot exceeds aggregate cell limit")?;
            }
            if !panes.insert(pane.id)
                || !tabs.contains(&pane.tab)
                || !listed.contains(&pane.id)
                || pane.title.len() > 65536
            {
                return Err("invalid snapshot pane identity".into());
            }
            if let Some([x, y, w, h]) = pane.rect {
                if ![x, y, w, h].iter().all(|n| n.is_finite())
                    || x < 0.0
                    || y < 0.0
                    || w <= 0.0
                    || h <= 0.0
                    || x + w > f32::from(self.viewport[0]) + 0.01
                    || y + h > f32::from(self.viewport[1]) + 0.01
                {
                    return Err("invalid snapshot pane rectangle".into());
                }
            }
            for native in &pane.native {
                if !sessions.insert(native.session_id) {
                    return Err("duplicate snapshot native session".into());
                }
            }
        }
        if self.active_tab.is_some_and(|id| !tabs.contains(&id))
            || self.active_tab.is_none() != self.tabs.is_empty()
        {
            return Err("invalid snapshot active tab".into());
        }
        Ok(())
    }
}

/// Count encoded state incrementally while capturing so multiple large panes
/// cannot accumulate an unbounded DTO before the transport notices its limit.
struct SnapshotBudget(usize);

impl SnapshotBudget {
    fn account(&mut self, value: &impl Serialize) -> Result<(), String> {
        serde_json::to_writer(self, value).map_err(|error| error.to_string())
    }
}

impl std::io::Write for SnapshotBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("snapshot exceeds attachment byte limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
