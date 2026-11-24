//! Selection of visible native transcript rows, isolated from the VT grid.

use ultty::scene::selection::{NativeSelection, SelectionRow, MAX_CAPTURE_BYTES, MAX_CAPTURE_ROWS};
use ultty::scene::SearchMatch;
use ultty::terminal_manager::TerminalId;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{native_panes::native_mouse_target, WindowCtx};

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
    link: Option<String>,
}

struct Hit {
    pane: TerminalId,
    session: u64,
    row: SelectionRow,
    column: usize,
    rows: Option<Vec<SelectionRow>>,
    link: Option<String>,
}

impl WindowCtx<'_> {
    fn mark_native_selection_dirty(&mut self) {
        let Some(id) = self.win.native_selection.session else {
            return;
        };
        if let Some(server) = self.app.runtime.protocol.as_mut() {
            if let Some(session) = server
                .sessions_mut()
                .find(|session| session.session_id() == id)
            {
                session.mark_render_dirty();
            }
        }
    }

    pub(super) fn native_transcript_selection_active(&self) -> bool {
        self.win.native_selection.active
    }

    fn native_transcript_hit(&self, captured: Option<TerminalId>) -> Option<Hit> {
        let (width, height) = self.win.renderer.terminal_area();
        let cell = self.win.renderer.cell_dimensions();
        if cell.0 <= 0.0 || cell.1 <= 0.0 {
            return None;
        }
        let active_tab = self.app.runtime.manager.window_active_tab(self.win.key)?.id;
        let (pane, x, y) = native_mouse_target(
            &self
                .app
                .runtime
                .manager
                .window_active_pane_rects(self.win.key, width, height),
            captured,
            (
                self.win.mouse_pos.x as f32,
                (self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64) as f32,
            ),
            cell,
        )?;
        let server = self.app.runtime.protocol.as_ref()?;
        server
            .pane_or_unbound_sessions_in_tab(active_tab, pane)
            .filter(|session| session.is_full_scene())
            .find_map(|session| {
                let (id, local_x, _) = session.hit_test(x, y);
                let region = session.scene().get(id)?;
                let row = SelectionRow::from_region(region)?;
                let inset = (region.computed.content_x - region.computed.x).max(0.0);
                let column = (local_x - inset).max(0.0) as usize;
                let offset = row.position(column)?.checked_sub(row.start)?;
                let link = region.metadata.iter().find_map(|(key, value)| {
                    if !key.starts_with("transcript-link-v1-") || value.len() > 4096 {
                        return None;
                    }
                    let mut fields = value.splitn(3, ':');
                    let start: usize = fields.next()?.parse().ok()?;
                    let end: usize = fields.next()?.parse().ok()?;
                    let target = fields.next()?;
                    if start > offset
                        || offset >= end
                        || end > row.text.len()
                        || !row.text.is_char_boundary(start)
                        || !row.text.is_char_boundary(end)
                    {
                        return None;
                    }
                    let delimited = format!("{target} ");
                    (ultty::url::url_at_text(&delimited, 0).as_deref() == Some(target))
                        .then(|| target.to_owned())
                });
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
                    link,
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
        self.win.native_selection = NativeSelectionState {
            selection,
            pane: Some(hit.pane),
            session: Some(hit.session),
            active: true,
            invalid: !valid,
            moved: false,
            ctrl: self.win.modifiers.control_key(),
            anchor: Some((position, hit.row.revision)),
            snapshot: None,
            pointer_origin: (self.win.mouse_pos.x, self.win.mouse_pos.y),
            link: hit.link,
        };
        self.mark_native_selection_dirty();
        self.win.needs_redraw = true;
        true
    }

    pub(super) fn update_native_transcript_selection(&mut self) {
        if self.win.native_selection.invalid {
            return;
        }
        let Some(pane) = self.win.native_selection.pane else {
            return;
        };
        let Some(hit) = self.native_transcript_hit(Some(pane)) else {
            self.invalidate_native_transcript_selection();
            return;
        };
        if Some(hit.session) != self.win.native_selection.session {
            self.invalidate_native_transcript_selection();
            return;
        }
        let position = hit.row.position(hit.column);
        let (x, y) = self.win.native_selection.pointer_origin;
        let distance = (self.win.mouse_pos.x - x).hypot(self.win.mouse_pos.y - y);
        if distance >= 3.0 || self.win.native_selection.anchor.map(|a| a.0) != position {
            self.win.native_selection.moved = true;
        }
        let Some(rows) = hit.rows else {
            self.invalidate_native_transcript_selection();
            return;
        };
        for row in rows {
            if !self.win.native_selection.selection.observe(row) {
                self.invalidate_native_transcript_selection();
                return;
            }
        }
        if !self
            .win
            .native_selection
            .selection
            .extend(hit.row, hit.column)
        {
            self.invalidate_native_transcript_selection();
            return;
        }
        self.mark_native_selection_dirty();
        self.win.needs_redraw = true;
    }

    pub(super) fn end_native_transcript_selection(&mut self) {
        self.update_native_transcript_selection();
        if !self.win.native_selection.active {
            return;
        }
        self.win.native_selection.active = false;
        if self.win.native_selection.invalid {
            self.mark_native_selection_dirty();
            self.win.needs_redraw = true;
            return;
        }
        if self.win.native_selection.ctrl && !self.win.native_selection.moved {
            if self.win.url_detector.is_enabled() {
                if let Some((position, _)) = self.win.native_selection.anchor {
                    if let Some(url) = self
                        .win
                        .native_selection
                        .link
                        .clone()
                        .or_else(|| self.win.native_selection.selection.url_at(position))
                    {
                        if let Err(error) = ultty::url::UrlDetector::open_url(&url) {
                            tracing::warn!("Failed to open URL: {error}");
                        }
                    }
                }
            }
            self.invalidate_native_transcript_selection();
            return;
        }
        self.win.native_selection.snapshot = self
            .win
            .native_selection
            .selection
            .text()
            .filter(|text| !text.is_empty());
        if self.win.native_selection.snapshot.is_none() {
            self.invalidate_native_transcript_selection();
        }
        self.mark_native_selection_dirty();
        self.win.needs_redraw = true;
    }

    fn invalidate_native_transcript_selection(&mut self) {
        self.win.native_selection.selection = NativeSelection::default();
        self.win.native_selection.snapshot = None;
        self.win.native_selection.invalid = true;
        self.mark_native_selection_dirty();
        self.win.needs_redraw = true;
    }

    pub(super) fn cancel_native_transcript_selection(&mut self) {
        self.mark_native_selection_dirty();
        self.win.native_selection = NativeSelectionState::default();
        self.win.needs_redraw = true;
    }

    pub(super) fn native_transcript_copy(&mut self) -> bool {
        if self.win.native_selection.pane.is_none() {
            return false;
        }
        if self
            .app
            .runtime
            .manager
            .window_focused_terminal_id(self.win.key)
            != self.win.native_selection.pane
        {
            self.cancel_native_transcript_selection();
            return false;
        }
        if self.win.native_selection.invalid {
            return true;
        }
        let active_tab = self
            .app
            .runtime
            .manager
            .window_active_tab(self.win.key)
            .map(|tab| tab.id);
        let owner_present = active_tab.is_some_and(|tab| {
            self.app.runtime.protocol.as_ref().is_some_and(|server| {
                server
                    .pane_or_unbound_sessions_in_tab(tab, self.win.native_selection.pane.unwrap())
                    .any(|session| Some(session.session_id()) == self.win.native_selection.session)
            })
        });
        if !owner_present {
            self.invalidate_native_transcript_selection();
            return true;
        }
        if let Some(text) = self.win.native_selection.snapshot.clone() {
            self.app.copy_to_clipboard(&text);
        }
        true
    }

    fn native_transcript_revision_matches(&self) -> bool {
        let Some(pane) = self.win.native_selection.pane else {
            return false;
        };
        if self
            .app
            .runtime
            .manager
            .window_focused_terminal_id(self.win.key)
            != Some(pane)
        {
            return false;
        }
        let Some(active_tab) = self
            .app
            .runtime
            .manager
            .window_active_tab(self.win.key)
            .map(|tab| tab.id)
        else {
            return false;
        };
        let Some(server) = self.app.runtime.protocol.as_ref() else {
            return false;
        };
        server
            .pane_or_unbound_sessions_in_tab(active_tab, pane)
            .any(|session| {
                Some(session.session_id()) == self.win.native_selection.session
                    && session
                        .scene()
                        .iter_tree(session.scene().root())
                        .filter_map(SelectionRow::from_region)
                        .any(|row| {
                            Some(row.revision) == self.win.native_selection.selection.revision()
                        })
            })
    }

    pub(super) fn native_transcript_highlights(&self, pane: TerminalId) -> Vec<SearchMatch> {
        if self.win.native_selection.pane != Some(pane)
            || !self.native_transcript_revision_matches()
        {
            return Vec::new();
        }
        let Some(range) = self.win.native_selection.selection.range() else {
            return Vec::new();
        };
        let Some(active_tab) = self
            .app
            .runtime
            .manager
            .window_active_tab(self.win.key)
            .map(|tab| tab.id)
        else {
            return Vec::new();
        };
        let Some(server) = self.app.runtime.protocol.as_ref() else {
            return Vec::new();
        };
        let Some(session) = server
            .pane_or_unbound_sessions_in_tab(active_tab, pane)
            .find(|session| Some(session.session_id()) == self.win.native_selection.session)
        else {
            return Vec::new();
        };
        session
            .scene()
            .iter_tree(session.scene().root())
            .filter_map(|region| {
                let row = SelectionRow::from_region(region)?;
                if Some(row.revision) != self.win.native_selection.selection.revision() {
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
