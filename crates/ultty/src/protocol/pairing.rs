//! Pairing inline surfaces with the anchors in a terminal's output
//! (ADR 0020, spec §10.6).
//!
//! An app's `CreateSurface` comes over its session and its anchor through
//! the PTY, in either order. A surface is placed once both have arrived
//! and their `rows` agree; whichever half waits more than
//! [`SURFACE_PAIRING_TIMEOUT`] for the other is dropped, leaving the
//! fallback text the app printed. An anchor pairs only with a session of
//! pane whose PTY carried it, so another pane cannot claim or move an
//! app's surface.

use std::io;
use std::time::Instant;

use tracing::warn;
use ultty_protocol::RegionId;

use super::session::{NativeSession, SurfacePlacement, SURFACE_PAIRING_TIMEOUT};
use crate::grid::{Anchor, AnchorEvent, Buffer, Row, Terminal};

/// The row of the screen, 0 at the top, that `line` (as counted by
/// [`Buffer::row_at`]) shows on with the buffer's current scrollback
/// position. Negative above the screen, past its last row below it.
pub fn visual_row(buffer: &Buffer, line: usize) -> i64 {
    let top = buffer.history_len().saturating_sub(buffer.scroll_offset());
    line as i64 - top as i64
}

/// Whether rows `first .. first + rows` of the screen (see [`visual_row`])
/// show any row of a buffer `screen_rows` high.
pub fn rows_on_screen(first: i64, rows: u16, screen_rows: u16) -> bool {
    first < i64::from(screen_rows) && first + i64::from(rows) > 0
}

/// A placed surface with at least one row on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceOnScreen {
    pub session_id: u64,
    pub id: RegionId,
    pub rows: u16,
    /// The screen row its first row shows on (see [`visual_row`]);
    /// negative when it starts above the screen.
    pub row: i64,
}

/// The placed surfaces of `sessions` that show on `terminal`'s screen, in
/// drawing order: none while the alternate screen shows.
pub fn surfaces_on_screen<'a, S: crate::scene::view::NativeRenderSource + ?Sized + 'a>(
    terminal: &Terminal,
    sessions: impl IntoIterator<Item = &'a S>,
) -> Vec<SurfaceOnScreen> {
    if terminal.is_alternate_screen() {
        return Vec::new();
    }
    let buffer = terminal.main_buffer();
    let screen_rows = buffer.dimensions().rows;
    let mut visible = Vec::new();
    for session in sessions {
        session.placed_surfaces(&mut |surface| {
            let Some(line) = buffer.anchor_line(surface.tag) else {
                return;
            };
            let row = visual_row(buffer, line);
            if rows_on_screen(row, surface.rows, screen_rows) {
                visible.push(SurfaceOnScreen {
                    session_id: session.session_id(),
                    id: surface.id,
                    rows: surface.rows,
                    row,
                });
            }
        });
    }
    visible
}

/// An anchor that no surface has claimed yet.
#[derive(Debug, Clone, Copy)]
struct PendingAnchor {
    tag: u16,
    /// Which placement on that tag it is (see `Buffer::anchor_serial`).
    serial: u64,
    anchor: Anchor,
    seen_at: Instant,
}

/// A surface placed over one of the terminal's anchors.
#[derive(Debug, Clone, Copy)]
struct Claimed {
    tag: u16,
    /// Which placement on that tag the surface is drawn over.
    serial: u64,
    rows: u16,
    session_id: u64,
    surface: RegionId,
}

/// The last rows of a surface placed here whose session ended, until the
/// next update writes them.
#[derive(Debug)]
struct Frozen {
    tag: u16,
    serial: u64,
    rows: Vec<Row>,
}

/// The pairing state of one terminal's anchors.
#[derive(Debug, Default)]
pub struct AnchorPairing {
    /// Anchors waiting for their surface, oldest first.
    pending: Vec<PendingAnchor>,
    /// Anchors a surface is placed over.
    claimed: Vec<Claimed>,
    /// Rows of surfaces whose session ended since the last update.
    frozen: Vec<Frozen>,
}

/// Write `rows`, a surface's cells, over the rows tagged `tag` that hold
/// it, from its first row on. Rows past the reservation, or that lost the
/// tag, are left alone. Returns whether any row changed.
fn write_rows(buffer: &mut Buffer, tag: u16, rows: &[Row]) -> bool {
    let Some(first) = buffer.anchor_line(tag) else {
        return false;
    };
    let mut changed = false;
    for (offset, row) in rows.iter().enumerate() {
        let line = first + offset;
        if buffer
            .anchor_at(line)
            .is_some_and(|anchor| anchor.tag() == tag)
        {
            changed |= buffer.replace_row(line, row);
        }
    }
    changed
}

impl AnchorPairing {
    /// Whether this terminal has anchors waiting or claimed, so it needs a
    /// pass even without new output.
    pub fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.claimed.is_empty()
    }

    /// When the oldest waiting anchor is due to be released, so the UI
    /// wakes for it; `None` when no anchor waits.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending
            .iter()
            .map(|pending| pending.seen_at + SURFACE_PAIRING_TIMEOUT)
            .min()
    }

    /// The terminal is gone: evict every surface placed on its anchors, so
    /// none stays placed on a tag nothing holds. Rows kept for it have
    /// nowhere to go.
    pub fn abandon(&mut self, sessions: &mut [&mut NativeSession]) {
        self.pending.clear();
        self.frozen.clear();
        for claimed in std::mem::take(&mut self.claimed) {
            self.evict(sessions, claimed);
        }
    }

    /// `session` is ending: keep the last rows of each of its surfaces
    /// placed here (from [`NativeSession::freeze_surfaces`]) for the next
    /// update to write before it releases their anchors.
    pub fn session_ending(&mut self, session: &mut NativeSession) {
        for claimed in &self.claimed {
            if claimed.session_id != session.session_id() {
                continue;
            }
            if let Some(rows) = session.take_frozen_surface(claimed.surface, claimed.serial) {
                self.frozen.push(Frozen {
                    tag: claimed.tag,
                    serial: claimed.serial,
                    rows,
                });
            }
        }
    }

    /// The rows of `claimed` are gone or erased: evict its surface if it is
    /// still placed, and drop the rows kept for it if it was just closed.
    fn evict(&self, sessions: &mut [&mut NativeSession], claimed: Claimed) {
        if let Some(session) = find(sessions, claimed.session_id) {
            session.take_frozen_surface(claimed.surface, claimed.serial);
            if is_placed_on(session, claimed.surface, claimed.tag) {
                let evicted = session.evict_surface(claimed.surface);
                report(claimed.session_id, evicted);
            }
        }
    }

    /// Events were lost (the terminal's queue overflowed), so make the
    /// pairing agree with the anchors the terminal holds: drop waiting
    /// anchors that are gone, evict surfaces whose tag is gone or now holds
    /// a later placement (even of an identical anchor), and treat live
    /// anchors nobody knows of as newly seen.
    fn reconcile(
        &mut self,
        terminal: &mut Terminal,
        sessions: &mut [&mut NativeSession],
        claimable: bool,
        now: Instant,
    ) {
        let buffer = terminal.main_buffer();
        let live: Vec<(u16, u64, Anchor)> = buffer
            .anchors()
            .filter_map(|(tag, anchor)| Some((tag, buffer.anchor_serial(tag)?, anchor)))
            .collect();
        let serial_of = |tag: u16| {
            live.iter()
                .find(|(live_tag, _, _)| *live_tag == tag)
                .map(|&(_, serial, _)| serial)
        };
        self.pending
            .retain(|pending| serial_of(pending.tag) == Some(pending.serial));
        let mut gone = Vec::new();
        self.claimed.retain(|claimed| {
            let intact = serial_of(claimed.tag) == Some(claimed.serial);
            if !intact {
                gone.push(*claimed);
            }
            intact
        });
        for claimed in gone {
            self.evict(sessions, claimed);
        }
        for &(tag, serial, anchor) in &live {
            let known = self.pending.iter().any(|pending| pending.tag == tag)
                || self.claimed.iter().any(|claimed| claimed.tag == tag);
            if known {
                continue;
            }
            if claimable {
                self.pending.push(PendingAnchor {
                    tag,
                    serial,
                    anchor,
                    seen_at: now,
                });
            } else {
                terminal.release_anchor(tag);
            }
        }
    }

    /// Bring the terminal's anchors and the tab's `sessions` up to date as
    /// of `now`: take the terminal's anchor events, place surfaces whose
    /// halves have both arrived, evict those whose rows are gone, write
    /// the cells of each surface whose scene changed into its rows (and of
    /// each closed one, before releasing its anchor), release anchors
    /// nobody claims and tell each placed surface's app whether it is on
    /// the screen. Returns whether any of the terminal's rows changed, so
    /// what a screen reader is shown can be looked at again.
    ///
    /// `claimable` says the terminal is its tab's only pane; otherwise new
    /// anchors are released at once. `on_screen` says it is drawn now (its
    /// tab is shown and has one pane).
    ///
    /// Surfaces that never get an anchor are dropped by
    /// [`NativeSession::expire_unplaced_surfaces`], called after every
    /// terminal of the tab had its pass.
    pub fn update(
        &mut self,
        terminal: &mut Terminal,
        sessions: &mut [&mut NativeSession],
        claimable: bool,
        on_screen: bool,
        now: Instant,
    ) -> bool {
        let mut wrote = false;
        // A tab split while an anchor waited: it can no longer be claimed.
        if !claimable {
            for pending in std::mem::take(&mut self.pending) {
                terminal.release_anchor(pending.tag);
            }
        }
        for event in terminal.take_anchor_events() {
            match event {
                AnchorEvent::Placed {
                    tag,
                    serial,
                    anchor,
                } if claimable => {
                    self.pending.push(PendingAnchor {
                        tag,
                        serial,
                        anchor,
                        seen_at: now,
                    });
                }
                AnchorEvent::Placed { tag, .. } => {
                    terminal.release_anchor(tag);
                }
                AnchorEvent::Refused { anchor, reason } => {
                    let id = RegionId::new(anchor.surface_id);
                    if let Some(session) = find(sessions, anchor.session_id).filter(|session| {
                        claimable
                            && matches!(
                                session.surface_placement(id),
                                Some(SurfacePlacement::Unplaced { .. })
                            )
                    }) {
                        session.refuse_surface(id, &format!("anchor refused: {reason:?}"));
                    }
                }
                AnchorEvent::Lost { tag, serial, .. } => {
                    // Match the serial too: after lost events, a tag may
                    // name a later placement, even of an identical anchor.
                    self.pending
                        .retain(|pending| (pending.tag, pending.serial) != (tag, serial));
                    if let Some(index) = self
                        .claimed
                        .iter()
                        .position(|c| (c.tag, c.serial) == (tag, serial))
                    {
                        let claimed = self.claimed.remove(index);
                        self.evict(sessions, claimed);
                    }
                }
            }
        }
        if terminal.take_anchor_overflow() {
            self.reconcile(terminal, sessions, claimable, now);
        }

        // A surface closed by its app, or whose session ended, freezes
        // where it stands: its rows get its last cells, if they were not
        // written yet, and lose the tag.
        let frozen = &mut self.frozen;
        self.claimed.retain(|claimed| {
            let session = find(sessions, claimed.session_id);
            let live = session
                .as_deref()
                .is_some_and(|session| is_placed_on(session, claimed.surface, claimed.tag));
            if !live {
                let closed = session.and_then(|session| {
                    session.take_frozen_surface(claimed.surface, claimed.serial)
                });
                let ended = || {
                    let index = frozen
                        .iter()
                        .position(|f| (f.tag, f.serial) == (claimed.tag, claimed.serial))?;
                    Some(frozen.swap_remove(index).rows)
                };
                if let Some(rows) = closed.or_else(ended) {
                    wrote |= write_rows(terminal.main_buffer_mut(), claimed.tag, &rows);
                }
                terminal.release_anchor(claimed.tag);
            }
            live
        });
        self.frozen.clear();

        self.pending.retain(|pending| {
            let fresh = now.saturating_duration_since(pending.seen_at) < SURFACE_PAIRING_TIMEOUT;
            if !fresh {
                terminal.release_anchor(pending.tag);
            }
            fresh
        });

        for pending in std::mem::take(&mut self.pending) {
            let id = RegionId::new(pending.anchor.surface_id);
            let Some(session) = find(sessions, pending.anchor.session_id) else {
                self.pending.push(pending);
                continue;
            };
            match session.surface_placement(id) {
                None => self.pending.push(pending),
                // A second anchor for a placed surface is ignored.
                Some(SurfacePlacement::Placed { .. }) => {
                    terminal.release_anchor(pending.tag);
                }
                Some(SurfacePlacement::Unplaced { rows, .. }) if rows != pending.anchor.rows => {
                    session.refuse_surface(
                        id,
                        &format!(
                            "its anchor reserved {} rows, CreateSurface asked for {rows}",
                            pending.anchor.rows
                        ),
                    );
                    terminal.release_anchor(pending.tag);
                }
                Some(SurfacePlacement::Unplaced { rows, .. }) => {
                    let placed = session.place_surface(id, pending.tag, pending.serial);
                    report(pending.anchor.session_id, placed);
                    self.claimed.push(Claimed {
                        tag: pending.tag,
                        serial: pending.serial,
                        rows,
                        session_id: pending.anchor.session_id,
                        surface: id,
                    });
                }
            }
        }

        // Surfaces whose scene changed are written into their rows, so
        // copy and search read what is drawn. Not while the alternate
        // screen hides them: they are written when it goes.
        if !terminal.is_alternate_screen() {
            let buffer = terminal.main_buffer_mut();
            let cols = buffer.dimensions().cols;
            for claimed in &self.claimed {
                let rows = find(sessions, claimed.session_id)
                    .and_then(|session| session.take_surface_rows(claimed.surface, cols));
                if let Some(rows) = rows {
                    wrote |= write_rows(buffer, claimed.tag, &rows);
                }
            }
        }

        let shown_at_all = on_screen && !terminal.is_alternate_screen();
        let buffer = terminal.main_buffer();
        let screen_rows = buffer.dimensions().rows;
        for claimed in &self.claimed {
            let line = buffer.anchor_line(claimed.tag);
            let shown = shown_at_all
                && line.is_some_and(|line| {
                    rows_on_screen(visual_row(buffer, line), claimed.rows, screen_rows)
                });
            if let Some(session) = find(sessions, claimed.session_id) {
                let viewed = session.set_surface_view(claimed.surface, line, shown);
                report(claimed.session_id, viewed);
            }
        }
        wrote
    }
}

fn find<'s>(
    sessions: &'s mut [&mut NativeSession],
    session_id: u64,
) -> Option<&'s mut NativeSession> {
    sessions
        .iter_mut()
        .find(|session| session.session_id() == session_id)
        .map(|session| &mut **session)
}

fn is_placed_on(session: &NativeSession, surface: RegionId, tag: u16) -> bool {
    matches!(
        session.surface_placement(surface),
        Some(SurfacePlacement::Placed { tag: placed, .. }) if placed == tag
    )
}

/// Log a session that could not queue a surface event; the server drops
/// it when its socket fails.
fn report(session_id: u64, result: io::Result<()>) {
    if let Err(e) = result {
        warn!("Session {session_id}: cannot queue a surface event: {e}");
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use ultty_protocol::{
        AppMessage, Constraints, Content, FrameId, Style, SurfaceEventKind, TerminalMessage,
        TextContent, CAP_INLINE_SURFACES, PROTOCOL_VERSION,
    };
    use ultty_vt::ansi::AnsiParser;
    use ultty_vt::grid::{Dimensions, TerminalHandler, MAX_ANCHORS};

    use super::*;
    use crate::terminal_manager::TabId;

    const SESSION: u64 = 7;

    fn term(cols: u16, rows: u16) -> Terminal {
        let mut term = Terminal::new(Dimensions::new(cols, rows));
        term.set_inline_anchors(true);
        term
    }

    fn feed(term: &mut Terminal, bytes: &[u8]) {
        let mut parser = AnsiParser::new();
        let mut handler = TerminalHandler::new(term);
        parser.feed(bytes, |action| handler.handle(action));
    }

    fn anchor(term: &mut Terminal, session_id: u64, surface: u32, rows: u16) {
        feed(
            term,
            format!("\x1b_u{session_id};{surface};{rows}\x1b\\").as_bytes(),
        );
    }

    /// A session that negotiated inline surfaces. The peer end is kept so
    /// writes do not fail.
    fn session(session_id: u64) -> (NativeSession, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut session = NativeSession::new(ours, session_id, TabId::new(0)).unwrap();
        session.set_inline_surfaces(true);
        session
            .handle_message_for_test(AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: CAP_INLINE_SURFACES,
            })
            .unwrap();
        session.take_sent_messages();
        (session, theirs)
    }

    fn create_surface(session: &mut NativeSession, id: u32, rows: u16) {
        for msg in [
            AppMessage::BeginFrame {
                frame_id: FrameId(id),
            },
            AppMessage::CreateSurface {
                id: RegionId::new(id),
                rows,
                constraints: Constraints::default(),
                style: Style::default(),
            },
            AppMessage::EndFrame {
                frame_id: FrameId(id),
            },
        ] {
            session.handle_message_for_test(msg).unwrap();
        }
    }

    /// Surface events and error codes the session sent since last asked.
    fn sent(session: &mut NativeSession) -> (Vec<(u32, SurfaceEventKind)>, Vec<u16>) {
        let mut events = Vec::new();
        let mut errors = Vec::new();
        for msg in session.take_sent_messages() {
            match msg {
                TerminalMessage::SurfaceEvent { id, kind } => events.push((id.0, kind)),
                TerminalMessage::Error { code, .. } => errors.push(code),
                _ => {}
            }
        }
        (events, errors)
    }

    fn tags(term: &Terminal) -> Vec<u16> {
        let buffer = term.main_buffer();
        (0..buffer.total_lines())
            .map(|line| buffer.row_at(line).unwrap().anchor().tag())
            .collect()
    }

    struct Rig {
        term: Terminal,
        session: NativeSession,
        _peer: UnixStream,
        pairing: AnchorPairing,
        now: Instant,
    }

    impl Rig {
        fn new() -> Self {
            let (mut session, peer) = session(SESSION);
            session.set_viewport(20.0, 6.0);
            Self {
                term: term(20, 6),
                session,
                _peer: peer,
                pairing: AnchorPairing::default(),
                now: Instant::now(),
            }
        }

        /// Apply `ops` as one frame.
        fn frame(&mut self, ops: Vec<AppMessage>) {
            let frame_id = FrameId(9000);
            self.session
                .handle_message_for_test(AppMessage::BeginFrame { frame_id })
                .unwrap();
            for op in ops {
                self.session.handle_message_for_test(op).unwrap();
            }
            self.session
                .handle_message_for_test(AppMessage::EndFrame { frame_id })
                .unwrap();
        }

        /// Set region `id`'s content to plain `text`.
        fn set_text(&mut self, id: u32, text: &str) {
            self.frame(vec![set_text(id, text)]);
        }

        /// The terminal's text, scrollback and screen, as `ulttyctl text
        /// --scrollback` and copy read it.
        fn text(&self) -> String {
            let buffer = self.term.main_buffer();
            buffer.text_of_rows(0, buffer.total_lines(), 0)
        }

        /// A pass; whether it changed any of the terminal's rows.
        fn update(&mut self) -> bool {
            self.update_with(true, true)
        }

        fn update_with(&mut self, claimable: bool, on_screen: bool) -> bool {
            let wrote = self.pairing.update(
                &mut self.term,
                &mut [&mut self.session],
                claimable,
                on_screen,
                self.now,
            );
            self.session.expire_unplaced_surfaces(self.now);
            wrote
        }

        fn sent(&mut self) -> (Vec<(u32, SurfaceEventKind)>, Vec<u16>) {
            sent(&mut self.session)
        }

        fn placement(&self, id: u32) -> Option<SurfacePlacement> {
            self.session.surface_placement(RegionId::new(id))
        }
    }

    fn set_text(id: u32, text: &str) -> AppMessage {
        AppMessage::SetContent {
            id: RegionId::new(id),
            content: Content::Text(TextContent::new(text)),
        }
    }

    fn placed(id: u32) -> (Vec<(u32, SurfaceEventKind)>, Vec<u16>) {
        (vec![(id, SurfaceEventKind::Placed)], vec![])
    }

    const NOTHING: (Vec<(u32, SurfaceEventKind)>, Vec<u16>) = (vec![], vec![]);

    #[test]
    fn a_surface_is_placed_when_its_anchor_comes_after_it() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 2);
        rig.update();
        assert!(matches!(
            rig.placement(5),
            Some(SurfacePlacement::Unplaced { rows: 2, .. })
        ));
        assert_eq!(rig.sent(), NOTHING);

        anchor(&mut rig.term, SESSION, 5, 2);
        rig.update();
        assert_eq!(rig.sent(), placed(5));
        assert_eq!(
            rig.placement(5),
            Some(SurfacePlacement::Placed { rows: 2, tag: 1 })
        );
        assert_eq!(rig.session.describe().surfaces[0].state, "placed");
        assert_eq!(rig.session.describe().surfaces[0].anchor_line, Some(0));
    }

    #[test]
    fn a_surface_is_placed_when_its_anchor_comes_first() {
        let mut rig = Rig::new();
        anchor(&mut rig.term, SESSION, 5, 2);
        rig.update();
        assert_eq!(rig.sent(), NOTHING);
        assert!(!rig.pairing.is_idle(), "the anchor waits for its surface");

        rig.now += Duration::from_millis(1500);
        create_surface(&mut rig.session, 5, 2);
        rig.update();
        assert_eq!(rig.sent(), placed(5));
        assert_eq!(tags(&rig.term)[..2], [1, 1]);
    }

    #[test]
    fn an_anchor_pairs_only_with_its_own_session() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 2);
        anchor(&mut rig.term, SESSION + 1, 5, 2);
        rig.update();
        assert_eq!(rig.sent(), NOTHING);
        assert!(matches!(
            rig.placement(5),
            Some(SurfacePlacement::Unplaced { .. })
        ));
    }

    #[test]
    fn anchors_of_a_pane_that_is_not_alone_are_released() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 2);
        anchor(&mut rig.term, SESSION, 5, 2);
        rig.update_with(false, false);
        assert_eq!(rig.sent(), NOTHING);
        assert!(rig.pairing.is_idle());
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
    }

    #[test]
    fn rows_that_disagree_close_the_surface_with_400_and_free_the_rows() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 3);
        anchor(&mut rig.term, SESSION, 5, 2);
        feed(&mut rig.term, b"fallback 1\r\nfallback 2");
        rig.update();
        assert_eq!(rig.sent(), (vec![], vec![400]));
        assert_eq!(rig.placement(5), None);
        assert!(rig.session.scene().get(RegionId::new(5)).is_none());
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
        assert_eq!(
            rig.term.main_buffer().row_at(1).unwrap().text_trimmed(),
            "fallback 2",
            "the fallback text stays"
        );
        assert!(rig.pairing.is_idle());
    }

    #[test]
    fn a_surface_without_an_anchor_is_dropped_with_400_after_two_seconds() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 2);
        let Some(SurfacePlacement::Unplaced { created_at, .. }) = rig.placement(5) else {
            panic!("surface 5 waits for its anchor");
        };
        rig.now = created_at + SURFACE_PAIRING_TIMEOUT - Duration::from_millis(1);
        rig.update();
        assert_eq!(rig.sent(), NOTHING);

        rig.now += Duration::from_millis(1);
        rig.update();
        assert_eq!(rig.sent(), (vec![], vec![400]));
        assert!(!rig.session.has_surfaces());

        // An anchor arriving after that claims nothing.
        anchor(&mut rig.term, SESSION, 5, 2);
        rig.update();
        assert_eq!(rig.sent(), NOTHING);
    }

    #[test]
    fn an_anchor_nobody_claims_is_released_after_two_seconds() {
        let mut rig = Rig::new();
        anchor(&mut rig.term, SESSION, 5, 2);
        rig.update();
        rig.now += SURFACE_PAIRING_TIMEOUT - Duration::from_millis(1);
        rig.update();
        assert_eq!(tags(&rig.term)[..2], [1, 1]);

        rig.now += Duration::from_millis(1);
        rig.update();
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
        assert!(rig.pairing.is_idle());

        // The surface coming late waits for an anchor of its own.
        create_surface(&mut rig.session, 5, 2);
        rig.update();
        assert_eq!(rig.sent(), NOTHING);
    }

    #[test]
    fn a_second_anchor_for_a_placed_surface_is_ignored() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 1);
        anchor(&mut rig.term, SESSION, 5, 1);
        feed(&mut rig.term, b"card\r\n");
        rig.update();
        assert_eq!(rig.sent(), placed(5));

        // Another program in the tab tries to move the card.
        feed(&mut rig.term, b"\r\n\r\n");
        anchor(&mut rig.term, SESSION, 5, 1);
        rig.update();
        assert_eq!(rig.sent(), NOTHING);
        assert_eq!(
            rig.placement(5),
            Some(SurfacePlacement::Placed { rows: 1, tag: 1 })
        );
        assert_eq!(tags(&rig.term), [1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_refused_anchor_drops_its_waiting_surface_with_400() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 2);
        feed(&mut rig.term, b"\x1b[?1049h");
        anchor(&mut rig.term, SESSION, 5, 2);
        rig.update();
        assert_eq!(rig.sent(), (vec![], vec![400]));
        assert_eq!(rig.placement(5), None);
    }

    #[test]
    fn a_lost_anchor_evicts_its_surface() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 2);
        anchor(&mut rig.term, SESSION, 5, 2);
        rig.update();
        assert_eq!(rig.sent(), placed(5));

        feed(&mut rig.term, b"\x1b[2J");
        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Evicted)], vec![]));
        assert_eq!(rig.placement(5), None);
        assert!(rig.session.scene().get(RegionId::new(5)).is_none());
        assert!(rig.pairing.is_idle());
    }

    #[test]
    fn closing_a_placed_surface_releases_its_rows_and_keeps_their_text() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 1);
        anchor(&mut rig.term, SESSION, 5, 1);
        feed(&mut rig.term, b"fallback");
        rig.update();
        rig.sent();
        rig.set_text(5, "card");
        rig.update();

        rig.session
            .handle_message_for_test(AppMessage::CloseSurface {
                id: RegionId::new(5),
            })
            .unwrap();
        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Closed)], vec![]));
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
        assert_eq!(
            rig.term.main_buffer().row_at(0).unwrap().text_trimmed(),
            "card"
        );
        assert!(rig.pairing.is_idle());
        assert_eq!(rig.term.take_anchor_events(), vec![]);
    }

    #[test]
    fn surfaces_of_an_ended_session_are_released() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 1);
        anchor(&mut rig.term, SESSION, 5, 1);
        rig.update();
        assert_eq!(rig.sent(), placed(5));

        rig.pairing
            .update(&mut rig.term, &mut [], true, true, rig.now);
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
        assert!(rig.pairing.is_idle());
    }

    /// `Hidden` and `Shown` follow the anchor's rows off and back onto the
    /// screen, once per change however many passes run.
    #[test]
    fn hidden_and_shown_follow_the_rows_across_the_screen_edge() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 2);
        anchor(&mut rig.term, SESSION, 5, 2);
        feed(&mut rig.term, b"card 1\r\ncard 2\r\n");
        rig.update();
        assert_eq!(rig.sent(), placed(5));

        // Four more lines scroll the card's first row off, but its second
        // is still on the screen.
        feed(&mut rig.term, b"1\r\n2\r\n3\r\n4\r\n");
        rig.update();
        assert_eq!(rig.sent(), NOTHING);
        assert_eq!(rig.session.describe().surfaces[0].state, "placed");

        feed(&mut rig.term, b"5\r\n");
        rig.update();
        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Hidden)], vec![]));
        assert_eq!(rig.session.describe().surfaces[0].state, "hidden");
        assert_eq!(rig.session.describe().surfaces[0].anchor_line, Some(0));

        // Scrolling back to it shows it.
        rig.term.buffer_mut().scroll_up(1);
        rig.update();
        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Shown)], vec![]));

        // A hidden tab, or the alternate screen, hides it too.
        rig.update_with(true, false);
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Hidden)], vec![]));
        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Shown)], vec![]));
        feed(&mut rig.term, b"\x1b[?1049h");
        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Hidden)], vec![]));
    }

    /// A tab split while an anchor waits: the anchor can no longer be
    /// claimed, so it is released at once rather than paired later.
    #[test]
    fn a_split_releases_waiting_anchors() {
        let mut rig = Rig::new();
        anchor(&mut rig.term, SESSION, 5, 2);
        rig.update();
        assert!(!rig.pairing.is_idle());

        rig.update_with(false, false);
        assert!(rig.pairing.is_idle());
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
        create_surface(&mut rig.session, 5, 2);
        rig.update();
        assert_eq!(rig.sent(), NOTHING);
    }

    /// The UI wakes when the oldest waiting anchor is due, and not at all
    /// when none waits.
    #[test]
    fn the_next_deadline_is_the_oldest_waiting_anchors_release() {
        let mut rig = Rig::new();
        assert_eq!(rig.pairing.next_deadline(), None);
        anchor(&mut rig.term, SESSION, 5, 1);
        rig.update();
        let first = rig.now;
        rig.now += Duration::from_millis(500);
        feed(&mut rig.term, b"\r\n");
        anchor(&mut rig.term, SESSION, 6, 1);
        rig.update();
        assert_eq!(
            rig.pairing.next_deadline(),
            Some(first + SURFACE_PAIRING_TIMEOUT)
        );

        // Placed anchors set no deadline.
        create_surface(&mut rig.session, 5, 1);
        create_surface(&mut rig.session, 6, 1);
        rig.update();
        assert_eq!(rig.pairing.next_deadline(), None);
    }

    /// A terminal that goes away takes its anchors with it: the surfaces
    /// placed on them are evicted, not left placed on dead tags.
    #[test]
    fn an_abandoned_terminal_evicts_its_surfaces() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 1);
        anchor(&mut rig.term, SESSION, 5, 1);
        rig.update();
        assert_eq!(rig.sent(), placed(5));

        rig.pairing.abandon(&mut [&mut rig.session]);
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Evicted)], vec![]));
        assert_eq!(rig.placement(5), None);
        assert!(rig.pairing.is_idle());
        assert_eq!(rig.session.placed_surfaces().count(), 0);
    }

    /// 1024 anchors fill the table, then 4096 more are refused in the same
    /// batch: every placed anchor is still seen, so none is held forever.
    #[test]
    fn a_flood_of_refusals_does_not_leak_tags() {
        let mut rig = Rig::new();
        let mut wire = Vec::new();
        for surface in 0..(MAX_ANCHORS + 4096) as u32 {
            wire.extend(format!("\x1b_u99;{surface};1\x1b\\\r\n").into_bytes());
        }
        feed(&mut rig.term, &wire);
        rig.update();
        assert_eq!(rig.term.main_buffer().anchors().count(), MAX_ANCHORS);

        rig.now += SURFACE_PAIRING_TIMEOUT;
        rig.update();
        assert_eq!(rig.term.main_buffer().anchors().count(), 0);
        assert!(rig.pairing.is_idle());
    }

    /// More placements and losses than the terminal's queue holds, with a
    /// card placed before them: the pairing reconciles with the anchors the
    /// terminal holds. The card, whose anchor went with the flood, is
    /// evicted; every anchor still live waits and then is released.
    #[test]
    fn an_overflowing_event_queue_is_reconciled_with_the_live_anchors() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 1);
        anchor(&mut rig.term, SESSION, 5, 1);
        feed(&mut rig.term, b"card\r\n");
        rig.update();
        assert_eq!(rig.sent(), placed(5));

        let mut wire = Vec::new();
        for round in 0..5u32 {
            wire.extend_from_slice(b"\x1b[3J\x1b[2J\x1b[H");
            for surface in 0..MAX_ANCHORS as u32 {
                let id = 10_000 + round * 2_000 + surface;
                wire.extend(format!("\x1b_u99;{id};1\x1b\\\r\n").into_bytes());
            }
        }
        feed(&mut rig.term, &wire);
        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Evicted)], vec![]));
        let live = rig.term.main_buffer().anchors().count();
        assert!(live > 0);
        assert_eq!(
            rig.pairing.pending.len(),
            live,
            "every live anchor is known"
        );

        rig.now += SURFACE_PAIRING_TIMEOUT;
        rig.update();
        assert_eq!(rig.term.main_buffer().anchors().count(), 0);
        assert!(rig.pairing.is_idle());
    }

    /// Another program in the tab clears the card's rows, overflows the
    /// event queue and prints the identical anchor again, which reuses the
    /// card's tag: the card is evicted, not moved onto the new rows.
    #[test]
    fn an_identical_anchor_on_a_reused_tag_does_not_move_a_placed_surface() {
        let mut rig = Rig::new();
        create_surface(&mut rig.session, 5, 1);
        anchor(&mut rig.term, SESSION, 5, 1);
        feed(&mut rig.term, b"card\r\n");
        rig.update();
        assert_eq!(rig.sent(), placed(5));

        let mut wire = b"\x1b[3J\x1b[2J\x1b[H".to_vec();
        for round in 0..5u32 {
            for surface in 0..MAX_ANCHORS as u32 {
                let id = 10_000 + round * 2_000 + surface;
                wire.extend(format!("\x1b_u99;{id};1\x1b\\\r\n").into_bytes());
            }
            wire.extend_from_slice(b"\x1b[3J\x1b[2J\x1b[H");
        }
        // The forged anchor lands on tag 1, the card's old tag.
        wire.extend(format!("\x1b_u{SESSION};5;1\x1b\\").into_bytes());
        feed(&mut rig.term, &wire);
        assert_eq!(
            rig.term.main_buffer().anchor(1),
            Some(Anchor {
                session_id: SESSION,
                surface_id: 5,
                rows: 1
            })
        );

        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Evicted)], vec![]));
        assert_eq!(rig.placement(5), None);
        assert_eq!(rig.session.placed_surfaces().count(), 0);
    }

    /// What [`transcript`] leaves in the terminal, as text.
    const TRANSCRIPT: &str = "prose 1\ncard line 1\ncard line 2\nprose 2";

    /// Prose, a two-row card (surface 5) with its fallback text, prose:
    /// the shape of an agent's transcript. The card is placed and written.
    fn transcript(rig: &mut Rig) {
        feed(&mut rig.term, b"prose 1\r\n");
        create_surface(&mut rig.session, 5, 2);
        anchor(&mut rig.term, SESSION, 5, 2);
        feed(&mut rig.term, b"fallback 1\r\nfallback 2\r\nprose 2\r\n");
        rig.set_text(5, "card line 1\ncard line 2");
        rig.update();
        assert_eq!(rig.sent(), placed(5));
    }

    fn close_surface(rig: &mut Rig, id: u32) {
        rig.frame(vec![AppMessage::CloseSurface {
            id: RegionId::new(id),
        }]);
    }

    /// `text --scrollback`, copy and search read the card's rows: its text
    /// in order with the shell's, the same once the card is closed.
    #[test]
    fn a_card_reads_in_order_with_the_shell_output_live_and_closed() {
        let mut rig = Rig::new();
        transcript(&mut rig);
        assert_eq!(rig.text(), TRANSCRIPT);
        assert!(tags(&rig.term)[1..3].iter().all(|&tag| tag != 0));

        close_surface(&mut rig, 5);
        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Closed)], vec![]));
        assert_eq!(rig.text(), TRANSCRIPT);
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
        assert!(rig.pairing.is_idle());
    }

    /// A frame that changes the card and closes it in one go, before any
    /// pass ran: the rows get its last cells all the same.
    #[test]
    fn the_last_frame_before_close_is_written() {
        let mut rig = Rig::new();
        transcript(&mut rig);
        rig.frame(vec![
            set_text(5, "final 1\nfinal 2"),
            AppMessage::CloseSurface {
                id: RegionId::new(5),
            },
        ]);
        rig.update();
        assert_eq!(rig.text(), "prose 1\nfinal 1\nfinal 2\nprose 2");
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
    }

    /// A session that ends after a frame no pass saw: the card freezes
    /// with that frame's cells, as the server ends a session.
    #[test]
    fn a_session_that_ends_leaves_its_last_frame() {
        let mut rig = Rig::new();
        transcript(&mut rig);
        rig.set_text(5, "last words");
        rig.session.freeze_surfaces();
        rig.pairing.session_ending(&mut rig.session);
        rig.pairing
            .update(&mut rig.term, &mut [], true, true, rig.now);
        assert_eq!(rig.text(), "prose 1\nlast words\n\nprose 2");
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
        assert!(rig.pairing.is_idle());
    }

    /// Rows are written for frames that changed the card, never for
    /// passes without one.
    #[test]
    fn passes_without_a_change_do_not_write_the_rows() {
        let mut rig = Rig::new();
        transcript(&mut rig);
        let mut scribble = crate::grid::Row::new(20);
        scribble[0].set_char('#');
        rig.term.main_buffer_mut().replace_row(1, &scribble);
        assert!(!rig.update(), "a pass without a change writes nothing");
        feed(&mut rig.term, b"more output\r\n");
        assert!(!rig.update());
        assert!(rig.text().starts_with("prose 1\n#\n"));

        // Metadata does not change what is drawn; content does.
        rig.frame(vec![AppMessage::SetRegionMetadata {
            id: RegionId::new(5),
            metadata: vec![("k".into(), "v".into())],
        }]);
        assert!(!rig.update());
        assert!(rig.text().starts_with("prose 1\n#\n"));
        rig.set_text(5, "card line 1\ncard line 2");
        assert!(rig.update(), "writing the card is a change to the screen");
        assert!(rig.text().starts_with(TRANSCRIPT));
        rig.set_text(5, "card line 1\ncard line 2");
        assert!(!rig.update(), "the same cells again change nothing");
    }

    /// The card goes on changing in scrollback, keeps its place as older
    /// lines are evicted, and freezes there.
    #[test]
    fn a_card_in_scrollback_is_rewritten_and_outlives_older_lines() {
        let mut rig = Rig::new();
        rig.term.set_scrollback_limit(4);
        transcript(&mut rig);
        feed(&mut rig.term, b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7");
        assert_eq!(rig.term.main_buffer().history_len(), 4);
        assert_eq!(rig.term.main_buffer().anchor_line(1), Some(0));
        let after = "card line 2\nprose 2\n1\n2\n3\n4\n5\n6\n7";
        assert_eq!(rig.text(), format!("card line 1\n{after}"));

        rig.set_text(5, "moved on\ncard line 2");
        rig.update();
        assert_eq!(rig.sent(), (vec![(5, SurfaceEventKind::Hidden)], vec![]));
        assert_eq!(rig.text(), format!("moved on\n{after}"));
        close_surface(&mut rig, 5);
        rig.update();
        assert_eq!(rig.text(), format!("moved on\n{after}"));
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
    }

    /// A narrower window clips the card's rows instead of wrapping them,
    /// and the card is laid out and written again at the new width, as
    /// on widening.
    #[test]
    fn a_resize_lays_the_card_out_again_in_its_own_rows() {
        let mut rig = Rig::new();
        transcript(&mut rig);
        rig.set_text(5, "card line 1\n-");
        rig.update();
        let resize = |rig: &mut Rig, cols: u16| {
            rig.term.resize(Dimensions::new(cols, 6));
            rig.session.set_viewport(f32::from(cols), 6.0);
            rig.session.relayout_for_resize().unwrap();
            rig.update();
        };

        resize(&mut rig, 12);
        assert_eq!(
            rig.text(),
            format!("prose 1\ncard line 1\n{}\nprose 2", "-".repeat(12))
        );
        let buffer = rig.term.main_buffer();
        let first = buffer.anchor_line(1).unwrap();
        for line in first..first + 2 {
            let row = buffer.row_at(line).unwrap();
            assert!(!row.is_wrapped());
            assert_eq!(row.width(), 12);
        }

        resize(&mut rig, 30);
        assert_eq!(
            rig.text(),
            format!("prose 1\ncard line 1\n{}\nprose 2", "-".repeat(30))
        );
        close_surface(&mut rig, 5);
        rig.update();
        assert_eq!(
            rig.text(),
            format!("prose 1\ncard line 1\n{}\nprose 2", "-".repeat(30))
        );
    }

    /// Selecting from the prose above the card to the prose below copies
    /// all three in order.
    #[test]
    fn a_selection_over_prose_card_and_prose_copies_them_in_order() {
        use crate::grid::{SelectionMode, SelectionPoint};
        let mut rig = Rig::new();
        transcript(&mut rig);
        rig.term
            .start_selection(SelectionPoint::new(0, 0), SelectionMode::Line);
        rig.term.update_selection(SelectionPoint::new(0, 3));
        assert_eq!(rig.term.selected_text().as_deref(), Some(TRANSCRIPT));
    }

    /// While the alternate screen shows, the card's rows wait; they are
    /// written once it is gone.
    #[test]
    fn the_alternate_screen_holds_writes_until_it_goes() {
        let mut rig = Rig::new();
        transcript(&mut rig);
        feed(&mut rig.term, b"\x1b[?1049h");
        rig.set_text(5, "while away");
        rig.update();
        assert_eq!(rig.text(), TRANSCRIPT);
        feed(&mut rig.term, b"\x1b[?1049l");
        rig.update();
        assert_eq!(rig.text(), "prose 1\nwhile away\n\nprose 2");
    }

    /// An app that replaces a card in one frame (final content, close,
    /// create the same id again) keeps the old card's last text in its
    /// rows; the new one waits for its own anchor.
    #[test]
    fn replacing_a_card_in_one_frame_keeps_the_old_cards_last_text() {
        let mut rig = Rig::new();
        transcript(&mut rig);
        rig.frame(vec![
            set_text(5, "final 1\nfinal 2"),
            AppMessage::CloseSurface {
                id: RegionId::new(5),
            },
            AppMessage::CreateSurface {
                id: RegionId::new(5),
                rows: 1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
        ]);
        rig.update();
        assert_eq!(rig.text(), "prose 1\nfinal 1\nfinal 2\nprose 2");
        assert!(tags(&rig.term).iter().all(|&tag| tag == 0));
        assert!(matches!(
            rig.placement(5),
            Some(SurfacePlacement::Unplaced { rows: 1, .. })
        ));
    }

    /// A card changed and closed in one frame whose rows are then erased
    /// in the same pass leaves nothing behind: a later card with the same
    /// id on the same tag, closed unchanged, keeps its own text.
    #[test]
    fn rows_kept_for_an_erased_card_never_reach_a_later_one() {
        let mut rig = Rig::new();
        transcript(&mut rig);
        rig.frame(vec![
            set_text(5, "old card"),
            AppMessage::CloseSurface {
                id: RegionId::new(5),
            },
        ]);
        feed(&mut rig.term, b"\x1b[2J\x1b[H");
        rig.update();
        assert_eq!(rig.session.frozen_surface_count(), 0);
        assert!(rig.pairing.is_idle());

        create_surface(&mut rig.session, 5, 1);
        anchor(&mut rig.term, SESSION, 5, 1);
        feed(&mut rig.term, b"fallback\r\n");
        rig.set_text(5, "new card");
        rig.update();
        assert_eq!(
            rig.placement(5),
            Some(SurfacePlacement::Placed { rows: 1, tag: 1 }),
            "the same tag is reused"
        );
        close_surface(&mut rig, 5);
        rig.update();
        assert_eq!(rig.text(), "new card");
        assert_eq!(rig.session.frozen_surface_count(), 0);
    }

    #[test]
    fn visual_rows_count_from_the_top_of_the_screen() {
        let mut term = term(20, 4);
        feed(&mut term, b"1\r\n2\r\n3\r\n4\r\n5\r\n6");
        let buffer = term.main_buffer();
        assert_eq!(buffer.history_len(), 2);
        assert_eq!(visual_row(buffer, 0), -2);
        assert_eq!(visual_row(buffer, 2), 0);
        term.buffer_mut().scroll_up(1);
        assert_eq!(visual_row(term.main_buffer(), 0), -1);

        assert!(!rows_on_screen(-2, 2, 4));
        assert!(rows_on_screen(-1, 2, 4));
        assert!(rows_on_screen(3, 2, 4));
        assert!(!rows_on_screen(4, 1, 4));
    }
}
