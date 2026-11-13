//! Selection of visible native transcript rows, isolated from the VT grid.

use ultty::scene::selection::{NativeSelection, SelectionRow, MAX_CAPTURE_BYTES, MAX_CAPTURE_ROWS};
use ultty::scene::SearchMatch;
use ultty::terminal_manager::TerminalId;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{native_panes::native_mouse_target, App};

#[derive(Default)]
pub(super) struct NativeSelectionState {
    selection: NativeSelection,
    pane: Option<TerminalId>,
    session: Option<u64>,
    active: bool,
    invalid: bool,
    moved: bool,
    ctrl: bool,
    anchor: Option<(usize, u64)>,
    snapshot: Option<String>,
    pointer_origin: (f64, f64),
}

struct Hit {
    pane: TerminalId,
    session: u64,
    row: SelectionRow,
    column: usize,
    rows: Option<Vec<SelectionRow>>,
}

impl App {
    fn mark_native_selection_dirty(&mut self) {
        let Some(id) = self.native_selection.session else {
            return;
        };
        if let Some(server) = self.runtime.protocol.as_mut() {
            if let Some(session) = server
                .sessions_mut()
                .find(|session| session.session_id() == id)
            {
                session.mark_render_dirty();
            }
        }
    }

    pub(super) fn native_transcript_selection_active(&self) -> bool {
        self.native_selection.active
    }

    fn native_transcript_hit(&self, captured: Option<TerminalId>) -> Option<Hit> {
        let (width, height) = self.renderer.terminal_area();
        let cell = self.renderer.cell_dimensions();
        if cell.0 <= 0.0 || cell.1 <= 0.0 {
            return None;
        }
        let (pane, x, y) = native_mouse_target(
            &self.runtime.manager.active_pane_rects(width, height),
            captured,
            (
                self.mouse_pos.x as f32,
                (self.mouse_pos.y - self.tab_bar.content_top() as f64) as f32,
            ),
            cell,
        )?;
        let server = self.runtime.protocol.as_ref()?;
        server
            .pane_or_unbound_sessions(pane)
            .filter(|session| session.is_full_scene())
            .find_map(|session| {
                let (id, local_x, _) = session.hit_test(x, y);
                let region = session.scene().get(id)?;
                let row = SelectionRow::from_region(region)?;
                let inset = (region.computed.content_x - region.computed.x).max(0.0);
                let column = (local_x - inset).max(0.0) as usize;
                let mut rows = Vec::new();
                let mut bytes = 0usize;
                let mut bounded = true;
                for region in session.scene().iter_tree(session.scene().root()) {
                    let Some(size) = SelectionRow::rendered_len(region) else {
                        continue;
                    };
                    let Some(total) = bytes.checked_add(size) else {
                        bounded = false;
                        break;
                    };
                    if total > MAX_CAPTURE_BYTES || rows.len() >= MAX_CAPTURE_ROWS {
                        bounded = false;
                        break;
                    }
                    if let Some(row) = SelectionRow::from_region(region) {
                        bytes = total;
                        rows.push(row);
                    }
                }
                let rows = bounded.then_some(rows);
                Some(Hit {
                    pane,
                    session: session.session_id(),
                    row,
                    column,
                    rows,
                })
            })
    }

    pub(super) fn native_transcript_at_pointer(&self) -> bool {
        self.native_transcript_hit(None).is_some()
    }

    pub(super) fn begin_native_transcript_selection(&mut self) -> bool {
        let Some(hit) = self.native_transcript_hit(None) else {
            return false;
        };
        let Some(position) = hit.row.position(hit.column) else {
            return false;
        };
        let mut selection = NativeSelection::default();
        let mut valid = selection.begin(hit.row.clone(), hit.column);
        if let Some(rows) = hit.rows {
            for row in rows {
                if !selection.observe(row) {
                    valid = false;
                    break;
                }
            }
        } else {
            valid = false;
        }
        self.native_selection = NativeSelectionState {
            selection,
            pane: Some(hit.pane),
            session: Some(hit.session),
            active: true,
            invalid: !valid,
            moved: false,
            ctrl: self.modifiers.control_key(),
            anchor: Some((position, hit.row.revision)),
            snapshot: None,
            pointer_origin: (self.mouse_pos.x, self.mouse_pos.y),
        };
        self.mark_native_selection_dirty();
        self.needs_redraw = true;
        true
    }

    pub(super) fn update_native_transcript_selection(&mut self) {
        if self.native_selection.invalid {
            return;
        }
        let Some(pane) = self.native_selection.pane else {
            return;
        };
        let Some(hit) = self.native_transcript_hit(Some(pane)) else {
            self.invalidate_native_transcript_selection();
            return;
        };
        if Some(hit.session) != self.native_selection.session {
            self.invalidate_native_transcript_selection();
            return;
        }
        let position = hit.row.position(hit.column);
        let (x, y) = self.native_selection.pointer_origin;
        let distance = (self.mouse_pos.x - x).hypot(self.mouse_pos.y - y);
        if distance >= 3.0 || self.native_selection.anchor.map(|a| a.0) != position {
            self.native_selection.moved = true;
        }
        let Some(rows) = hit.rows else {
            self.invalidate_native_transcript_selection();
            return;
        };
        for row in rows {
            if !self.native_selection.selection.observe(row) {
                self.invalidate_native_transcript_selection();
                return;
            }
        }
        if !self.native_selection.selection.extend(hit.row, hit.column) {
            self.invalidate_native_transcript_selection();
            return;
        }
        self.mark_native_selection_dirty();
        self.needs_redraw = true;
    }

    pub(super) fn end_native_transcript_selection(&mut self) {
        self.update_native_transcript_selection();
        if !self.native_selection.active {
            return;
        }
        self.native_selection.active = false;
        if self.native_selection.invalid {
            self.mark_native_selection_dirty();
            self.needs_redraw = true;
            return;
        }
        if self.native_selection.ctrl && !self.native_selection.moved {
            if self.url_detector.is_enabled() {
                if let Some((position, _)) = self.native_selection.anchor {
                    if let Some(url) = self.native_selection.selection.url_at(position) {
                        if let Err(error) = ultty::url::UrlDetector::open_url(&url) {
                            tracing::warn!("Failed to open URL: {error}");
                        }
                    }
                }
            }
            self.invalidate_native_transcript_selection();
            return;
        }
        self.native_selection.snapshot = self
            .native_selection
            .selection
            .text()
            .filter(|text| !text.is_empty());
        if self.native_selection.snapshot.is_none() {
            self.invalidate_native_transcript_selection();
        }
        self.mark_native_selection_dirty();
        self.needs_redraw = true;
    }

    fn invalidate_native_transcript_selection(&mut self) {
        self.native_selection.selection = NativeSelection::default();
        self.native_selection.snapshot = None;
        self.native_selection.invalid = true;
        self.mark_native_selection_dirty();
        self.needs_redraw = true;
    }

    pub(super) fn cancel_native_transcript_selection(&mut self) {
        self.mark_native_selection_dirty();
        self.native_selection = NativeSelectionState::default();
        self.needs_redraw = true;
    }

    pub(super) fn native_transcript_copy(&mut self) -> bool {
        if self.native_selection.pane.is_none() {
            return false;
        }
        if self.runtime.manager.focused_terminal_id() != self.native_selection.pane {
            self.cancel_native_transcript_selection();
            return false;
        }
        if self.native_selection.invalid {
            return true;
        }
        if let Some(text) = self.native_selection.snapshot.clone() {
            self.copy_to_clipboard(&text);
        }
        true
    }

    fn native_transcript_revision_matches(&self) -> bool {
        let Some(pane) = self.native_selection.pane else {
            return false;
        };
        if self.runtime.manager.focused_terminal_id() != Some(pane) {
            return false;
        }
        let Some(server) = self.runtime.protocol.as_ref() else {
            return false;
        };
        server.pane_or_unbound_sessions(pane).any(|session| {
            Some(session.session_id()) == self.native_selection.session
                && session
                    .scene()
                    .iter_tree(session.scene().root())
                    .filter_map(SelectionRow::from_region)
                    .any(|row| Some(row.revision) == self.native_selection.selection.revision())
        })
    }

    pub(super) fn native_transcript_highlights(&self, pane: TerminalId) -> Vec<SearchMatch> {
        if self.native_selection.pane != Some(pane) || !self.native_transcript_revision_matches() {
            return Vec::new();
        }
        let Some(range) = self.native_selection.selection.range() else {
            return Vec::new();
        };
        let Some(server) = self.runtime.protocol.as_ref() else {
            return Vec::new();
        };
        let Some(session) = server
            .pane_or_unbound_sessions(pane)
            .find(|session| Some(session.session_id()) == self.native_selection.session)
        else {
            return Vec::new();
        };
        session
            .scene()
            .iter_tree(session.scene().root())
            .filter_map(|region| {
                let row = SelectionRow::from_region(region)?;
                if Some(row.revision) != self.native_selection.selection.revision() {
                    return None;
                }
                let start = range
                    .start
                    .max(row.start)
                    .saturating_sub(row.start)
                    .min(row.text.len());
                let end = range.end.saturating_sub(row.start).min(row.text.len());
                if start >= end
                    || !row.text.is_char_boundary(start)
                    || !row.text.is_char_boundary(end)
                {
                    return None;
                }
                Some(SearchMatch {
                    region_id: region.id,
                    span_index: None,
                    char_offset: row.text[..start]
                        .graphemes(true)
                        .map(|g| g.width().max(1))
                        .sum(),
                    length: row.text[start..end]
                        .graphemes(true)
                        .map(|g| g.width().max(1))
                        .sum(),
                })
            })
            .collect()
    }
}
