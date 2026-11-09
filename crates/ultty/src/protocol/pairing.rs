//! Pairing inline surfaces with the anchors in a terminal's output
//! (ADR 0020, spec §10.6).
//!
//! An app's `CreateSurface` comes over its session and its anchor through
//! the PTY, in either order. A surface is placed once both have arrived
//! and their `rows` agree; whichever half waits more than
//! [`SURFACE_PAIRING_TIMEOUT`] for the other is dropped, leaving the
//! fallback text the app printed. An anchor pairs only with a session of
//! the tab whose PTY carried it, and only while that PTY is the tab's one
//! pane, so another program cannot claim or move an app's surface.

use std::io;
use std::time::Instant;

use tracing::warn;
use ultty_protocol::RegionId;

use super::session::{NativeSession, SurfacePlacement, SURFACE_PAIRING_TIMEOUT};
use crate::grid::{Anchor, AnchorEvent, Buffer, Terminal};

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

/// The pairing state of one terminal's anchors.
#[derive(Debug, Default)]
pub struct AnchorPairing {
    /// Anchors waiting for their surface, oldest first.
    pending: Vec<PendingAnchor>,
    /// Anchors a surface is placed over.
    claimed: Vec<Claimed>,
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
    /// none stays placed on a tag nothing holds.
    pub fn abandon(&mut self, sessions: &mut [&mut NativeSession]) {
        self.pending.clear();
        for claimed in std::mem::take(&mut self.claimed) {
            self.evict(sessions, claimed);
        }
    }

    fn evict(&self, sessions: &mut [&mut NativeSession], claimed: Claimed) {
        if let Some(session) = find(sessions, claimed.session_id) {
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
    /// halves have both arrived, evict those whose rows are gone, release
    /// anchors nobody claims and tell each placed surface's app whether it
    /// is on the screen.
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
    ) {
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
        // where it stands: its rows keep their text and lose the tag.
        self.claimed.retain(|claimed| {
            let live = find(sessions, claimed.session_id)
                .is_some_and(|session| is_placed_on(session, claimed.surface, claimed.tag));
            if !live {
                terminal.release_anchor(claimed.tag);
            }
            live
        });

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
                    let placed = session.place_surface(id, pending.tag);
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
        AppMessage, Constraints, FrameId, Style, SurfaceEventKind, TerminalMessage,
        CAP_INLINE_SURFACES, PROTOCOL_VERSION,
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
            let (session, peer) = session(SESSION);
            Self {
                term: term(20, 6),
                session,
                _peer: peer,
                pairing: AnchorPairing::default(),
                now: Instant::now(),
            }
        }

        fn update(&mut self) {
            self.update_with(true, true);
        }

        fn update_with(&mut self, claimable: bool, on_screen: bool) {
            self.pairing.update(
                &mut self.term,
                &mut [&mut self.session],
                claimable,
                on_screen,
                self.now,
            );
            self.session.expire_unplaced_surfaces(self.now);
        }

        fn sent(&mut self) -> (Vec<(u32, SurfaceEventKind)>, Vec<u16>) {
            sent(&mut self.session)
        }

        fn placement(&self, id: u32) -> Option<SurfacePlacement> {
            self.session.surface_placement(RegionId::new(id))
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
        feed(&mut rig.term, b"card");
        rig.update();
        rig.sent();

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
