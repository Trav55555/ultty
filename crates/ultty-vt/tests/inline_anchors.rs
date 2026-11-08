//! Inline surface anchors (ADR 0020): an app writes
//! `ESC _ u <session> ; <surface> ; <rows> ESC \` and then its fallback
//! text; the terminal reserves and tags those rows on the main screen and
//! reports what becomes of them.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{
    Anchor, AnchorEvent, AnchorRefusal, Dimensions, Terminal, TerminalHandler, MAX_ANCHORS,
};

fn feed(term: &mut Terminal, bytes: &[u8]) {
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(term);
    parser.feed(bytes, |action| handler.handle(action));
    term.check_integrity()
        .expect("terminal state stays consistent");
}

/// A terminal with inline anchors turned on, as the embedder does once it
/// pairs them with sessions.
fn term(cols: u16, rows: u16) -> Terminal {
    let mut term = Terminal::new(Dimensions::new(cols, rows));
    term.set_inline_anchors(true);
    term
}

fn anchor_bytes(session_id: u64, surface_id: u32, rows: u16) -> Vec<u8> {
    format!("\x1b_u{session_id};{surface_id};{rows}\x1b\\").into_bytes()
}

fn anchor(surface_id: u32, rows: u16) -> Anchor {
    Anchor {
        session_id: 7,
        surface_id,
        rows,
    }
}

/// Place surface `surface_id`'s anchor at the cursor and return its tag.
fn place(term: &mut Terminal, surface_id: u32, rows: u16) -> u16 {
    feed(term, &anchor_bytes(7, surface_id, rows));
    match term.take_anchor_events().as_slice() {
        [AnchorEvent::Placed { tag, anchor: a }] => {
            assert_eq!(*a, anchor(surface_id, rows));
            *tag
        }
        other => panic!("expected one Placed event, got {other:?}"),
    }
}

/// The tags (0 for none) on every line, scrollback first.
fn tags(term: &Terminal) -> Vec<u16> {
    let buffer = term.buffer();
    (0..buffer.total_lines())
        .map(|line| buffer.row_at(line).unwrap().anchor().tag())
        .collect()
}

/// The text of every line, scrollback first, trailing blanks trimmed.
fn lines(term: &Terminal) -> Vec<String> {
    let buffer = term.buffer();
    (0..buffer.total_lines())
        .map(|line| buffer.row_at(line).unwrap().text_trimmed())
        .collect()
}

fn cursor(term: &Terminal) -> (u16, u16) {
    let position = term.grid().cursor().position;
    (position.row, position.col)
}

fn lost(tag: u16, surface_id: u32, rows: u16) -> AnchorEvent {
    AnchorEvent::Lost {
        tag,
        anchor: anchor(surface_id, rows),
    }
}

#[test]
fn malformed_anchors_reserve_nothing() {
    let mut term = term(20, 5);
    feed(
        &mut term,
        b"x\x1b_u1;2;0\x1b\\\x1b_u1;2;129\x1b\\\x1b_u1;2\x1b\\y",
    );
    let mut oversized = b"\x1b_u1;2;3".to_vec();
    oversized.extend(std::iter::repeat_n(b'0', 100));
    oversized.extend_from_slice(b"\x1b\\");
    feed(&mut term, &oversized);
    assert_eq!(term.take_anchor_events(), vec![]);
    assert_eq!(cursor(&term), (0, 2));
    assert!(tags(&term).iter().all(|&t| t == 0));
    assert_eq!(term.grid().row(0).unwrap().text_trimmed(), "xy");
}

#[test]
fn reservation_at_the_bottom_scrolls_in_and_returns_to_its_first_row() {
    let mut term = term(20, 5);
    feed(&mut term, b"one\r\ntwo\r\nthree\r\nfour\r\nprompt$ ");
    feed(&mut term, b"\r\n");
    // The cursor is on the bottom row, column 0, with one line in
    // scrollback; four rows scroll in three more.
    assert_eq!(term.buffer().history_len(), 1);
    let tag = place(&mut term, 1, 4);
    assert_eq!(tag, 1);
    assert_eq!(term.buffer().history_len(), 4);
    assert_eq!(cursor(&term), (1, 0));
    assert_eq!(tags(&term), vec![0, 0, 0, 0, 0, 1, 1, 1, 1]);
    assert!(term.grid().row(1).unwrap().anchor().is_first());
    assert!(!term.grid().row(2).unwrap().anchor().is_first());
    assert_eq!(term.buffer().anchor_line(tag), Some(5));

    // The fallback text lands on the reserved rows and output continues
    // after them.
    feed(&mut term, b"+--+\r\n|hi|\r\n|  |\r\n+--+\r\n$ ");
    assert_eq!(
        lines(&term)[4..],
        ["prompt$", "+--+", "|hi|", "|  |", "+--+", "$"].map(String::from)
    );
    assert_eq!(tags(&term)[4..], [0, 1, 1, 1, 1, 0]);
}

#[test]
fn reservation_mid_screen_tags_rows_below_without_scrolling() {
    let mut term = term(20, 6);
    feed(&mut term, b"top\r\n");
    let tag = place(&mut term, 9, 3);
    assert_eq!(term.buffer().history_len(), 0);
    assert_eq!(cursor(&term), (1, 0));
    assert_eq!(tags(&term), vec![0, tag, tag, tag, 0, 0]);
    assert_eq!(term.buffer().anchor(tag), Some(anchor(9, 3)));

    // A one-row anchor tags the cursor's row and leaves the cursor.
    feed(&mut term, b"\x1b[6;4H");
    let one = place(&mut term, 10, 1);
    assert_eq!(cursor(&term), (5, 3));
    assert_eq!(tags(&term), vec![0, tag, tag, tag, 0, one]);
}

#[test]
fn tags_survive_scrolling_into_scrollback_and_back() {
    let mut term = term(20, 4);
    let tag = place(&mut term, 1, 2);
    feed(&mut term, b"card 1\r\ncard 2\r\n");
    for n in 0..10 {
        feed(&mut term, format!("line {n}\r\n").as_bytes());
    }
    let buffer = term.buffer();
    assert_eq!(buffer.history_len(), 9);
    assert_eq!(buffer.anchor_line(tag), Some(0));
    assert_eq!(tags(&term)[..3], [tag, tag, 0]);
    assert_eq!(term.take_anchor_events(), vec![]);

    // A taller window pulls the card back from scrollback onto the grid.
    term.resize(Dimensions::new(20, 13));
    assert_eq!(term.buffer().history_len(), 0);
    assert_eq!(term.grid().row(0).unwrap().text_trimmed(), "card 1");
    assert_eq!(term.grid().row(0).unwrap().anchor().tag(), tag);
    assert!(term.grid().row(0).unwrap().anchor().is_first());
    assert_eq!(term.grid().row(1).unwrap().anchor().tag(), tag);
    assert_eq!(term.buffer().anchor_line(tag), Some(0));

    // And a shorter one pushes it back without losing it.
    term.resize(Dimensions::new(20, 3));
    assert_eq!(term.buffer().anchor_line(tag), Some(0));
    assert_eq!(term.take_anchor_events(), vec![]);
    term.check_integrity().unwrap();
}

#[test]
fn a_blank_reservation_is_content_when_the_window_shrinks() {
    let mut term = term(20, 6);
    feed(&mut term, b"$ ");
    // Cursor above its own reservation's blank rows: they are not dropped
    // as blank space below the cursor.
    let tag = place(&mut term, 1, 3);
    term.resize(Dimensions::new(20, 2));
    assert_eq!(term.take_anchor_events(), vec![]);
    assert_eq!(tags(&term).iter().filter(|&&t| t == tag).count(), 3);
}

#[test]
fn insert_and_delete_line_move_tags_with_their_rows() {
    let mut term = term(20, 8);
    feed(&mut term, b"a\r\nb\r\n");
    let tag = place(&mut term, 1, 2);
    feed(&mut term, b"card\r\ncard\r\n");
    assert_eq!(term.buffer().anchor_line(tag), Some(2));

    // IL above the card pushes it down.
    feed(&mut term, b"\x1b[1H\x1b[2L");
    assert_eq!(tags(&term), vec![0, 0, 0, 0, tag, tag, 0, 0]);
    assert_eq!(term.buffer().anchor_line(tag), Some(4));
    // DL above it pulls it back up.
    feed(&mut term, b"\x1b[1H\x1b[3M");
    assert_eq!(tags(&term), vec![0, tag, tag, 0, 0, 0, 0, 0]);
    assert_eq!(term.buffer().anchor_line(tag), Some(1));
    assert_eq!(term.take_anchor_events(), vec![]);

    // DL on the card's first row deletes it: the surface is lost and its
    // other row no longer carries the tag.
    feed(&mut term, b"\x1b[2H\x1b[1M");
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 2)]);
    assert!(tags(&term).iter().all(|&t| t == 0));
    assert_eq!(term.buffer().anchor(tag), None);
}

#[test]
fn rows_pushed_out_of_the_screen_by_insert_line_lose_their_anchor() {
    let mut term = term(20, 4);
    feed(&mut term, b"\x1b[3H");
    let tag = place(&mut term, 1, 2);
    feed(&mut term, b"\x1b[1H\x1b[2L");
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 2)]);
}

#[test]
fn scroll_regions_move_tags_and_discard_them_off_the_top() {
    let mut term = term(20, 8);
    // Region rows 3 to 6 (1-based); cursor at its bottom.
    feed(&mut term, b"\x1b[3;6r\x1b[6H");
    let tag = place(&mut term, 1, 2);
    // One line scrolled in inside the region; the rows above it stay.
    assert_eq!(tags(&term), vec![0, 0, 0, 0, tag, tag, 0, 0]);
    assert_eq!(cursor(&term), (4, 0));
    assert_eq!(term.buffer().history_len(), 0);

    // Scrolling the region moves the card up within it...
    feed(&mut term, b"\x1b[6H\n");
    assert_eq!(term.buffer().anchor_line(tag), Some(3));
    assert_eq!(term.take_anchor_events(), vec![]);
    // ...and lines leaving a region below the top of the screen are
    // discarded, taking the anchor with them.
    feed(&mut term, b"\n\n");
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 2)]);
    assert!(tags(&term).iter().all(|&t| t == 0));
}

#[test]
fn reservations_that_cannot_stay_on_screen_are_refused() {
    let mut term = term(20, 8);
    let refused = |rows, reason| AnchorEvent::Refused {
        anchor: anchor(1, rows),
        reason,
    };

    // Taller than the screen.
    feed(&mut term, &anchor_bytes(7, 1, 9));
    assert_eq!(
        term.take_anchor_events(),
        vec![refused(9, AnchorRefusal::NoRoom)]
    );
    // Taller than the scroll region.
    feed(&mut term, b"\x1b[3;6r\x1b[4H");
    feed(&mut term, &anchor_bytes(7, 1, 5));
    // The cursor outside the scroll region.
    feed(&mut term, b"\x1b[8H");
    feed(&mut term, &anchor_bytes(7, 1, 1));
    assert_eq!(
        term.take_anchor_events(),
        vec![
            refused(5, AnchorRefusal::NoRoom),
            refused(1, AnchorRefusal::NoRoom)
        ]
    );
    assert_eq!(cursor(&term), (7, 0));
    assert!(tags(&term).iter().all(|&t| t == 0));
}

#[test]
fn erase_display_drops_tags_and_reports_the_loss() {
    let mut term = term(20, 5);
    let tag = place(&mut term, 1, 2);
    feed(&mut term, b"\x1b[2J");
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 2)]);
    assert!(tags(&term).iter().all(|&t| t == 0));
}

#[test]
fn erase_saved_lines_drops_tags_in_scrollback() {
    let mut term = term(20, 3);
    let in_history = place(&mut term, 1, 1);
    feed(&mut term, b"\r\n\r\n\r\n\r\n");
    let on_screen = place(&mut term, 2, 1);
    assert!(term.buffer().history_len() > 0);

    feed(&mut term, b"\x1b[3J");
    assert_eq!(term.take_anchor_events(), vec![lost(in_history, 1, 1)]);
    // The anchor on the screen is untouched, and found where it is.
    assert_eq!(term.buffer().anchor_line(on_screen), Some(2));
    assert_eq!(tags(&term), vec![0, 0, on_screen]);
}

#[test]
fn full_reset_drops_every_anchor() {
    let mut term = term(20, 4);
    let in_history = place(&mut term, 1, 2);
    feed(&mut term, b"\r\n\r\n\r\n\r\n");
    let on_screen = place(&mut term, 2, 1);
    feed(&mut term, b"\x1bc");
    assert_eq!(
        term.take_anchor_events(),
        vec![lost(in_history, 1, 2), lost(on_screen, 2, 1)]
    );
    assert!(tags(&term).iter().all(|&t| t == 0));
}

#[test]
fn full_reset_from_the_alternate_screen_drops_main_screen_anchors() {
    let mut term = term(20, 4);
    let tag = place(&mut term, 1, 2);
    feed(&mut term, b"\x1b[?1049h\x1bc");
    assert!(!term.is_alternate_screen());
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 2)]);
    assert!(tags(&term).iter().all(|&t| t == 0));
}

#[test]
fn erasing_a_line_drops_its_tag_and_its_first_line_loses_the_anchor() {
    let mut term = term(20, 6);
    let tag = place(&mut term, 1, 3);
    feed(&mut term, b"card 1\r\ncard 2\r\ncard 3");

    // Erasing part of a line, as apps do while printing, keeps its tag.
    feed(
        &mut term,
        b"\x1b[1;3H\x1b[K\x1b[3;1H\x1b[1K\x1b[2;1H\x1b[5X",
    );
    assert_eq!(tags(&term)[..3], [tag, tag, tag]);
    assert_eq!(term.take_anchor_events(), vec![]);

    // Erasing the first line loses the anchor and frees every row.
    feed(&mut term, b"\x1b[1;1H\x1b[2K");
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 3)]);
    assert!(tags(&term).iter().all(|&t| t == 0));
}

#[test]
fn erasing_any_reserved_line_loses_the_anchor() {
    let mut term = term(20, 6);
    let tag = place(&mut term, 1, 3);
    feed(&mut term, b"card 1\r\ncard 2\r\ncard 3");
    feed(&mut term, b"\x1b[3;1H\x1b[2K");
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 3)]);
    assert!(tags(&term).iter().all(|&t| t == 0));
}

#[test]
fn eviction_from_scrollback_loses_the_anchor() {
    let mut term = term(20, 3);
    term.set_scrollback_limit(4);
    let tag = place(&mut term, 1, 2);
    feed(&mut term, b"card\r\ncard\r\n");
    for n in 0..4 {
        feed(&mut term, format!("{n}\r\n").as_bytes());
    }
    // Scrollback is full and the card's first row is its oldest line.
    assert_eq!(term.buffer().anchor_line(tag), Some(0));
    assert_eq!(term.take_anchor_events(), vec![]);

    feed(&mut term, b"one more\r\n");
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 2)]);
    // The card's second row is still in scrollback, without the tag.
    assert_eq!(lines(&term)[0], "card");
    assert!(tags(&term).iter().all(|&t| t == 0));
}

#[test]
fn lowering_the_scrollback_limit_loses_evicted_anchors() {
    let mut term = term(20, 3);
    let tag = place(&mut term, 1, 1);
    feed(&mut term, b"\r\n\r\n\r\n\r\n\r\n");
    term.set_scrollback_limit(1);
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 1)]);
}

#[test]
fn the_alternate_screen_tags_nothing_and_main_anchors_wait_for_it() {
    let mut term = term(20, 5);
    let tag = place(&mut term, 1, 2);

    feed(&mut term, b"\x1b[?1049h\x1b[3H");
    feed(&mut term, &anchor_bytes(7, 2, 2));
    assert_eq!(
        term.take_anchor_events(),
        vec![AnchorEvent::Refused {
            anchor: anchor(2, 2),
            reason: AnchorRefusal::AlternateScreen,
        }]
    );
    assert_eq!(cursor(&term), (2, 0));
    assert!(term.grid().rows().all(|row| row.anchor().is_none()));
    // A full-screen app clearing its screen does not touch the main one.
    feed(&mut term, b"\x1b[2J");

    feed(&mut term, b"\x1b[?1049l");
    assert_eq!(term.take_anchor_events(), vec![]);
    assert_eq!(term.buffer().anchor_line(tag), Some(0));
    assert_eq!(tags(&term)[..2], [tag, tag]);
}

#[test]
fn reflow_clips_or_pads_tagged_rows_and_rewraps_their_neighbours() {
    let mut term = term(20, 8);
    feed(&mut term, b"0123456789abcdefghijKL\r\n");
    let tag = place(&mut term, 1, 2);
    feed(
        &mut term,
        b"+-card-----------+\r\n|wide card line 12|\r\nABCDEFGHIJKLMNOPQRSTUVW\r\n",
    );
    assert_eq!(
        lines(&term)[..6],
        [
            "0123456789abcdefghij",
            "KL",
            "+-card-----------+",
            "|wide card line 12|",
            "ABCDEFGHIJKLMNOPQRST",
            "UVW"
        ]
        .map(String::from)
    );

    // Narrower: the untagged lines rewrap, each tagged row stays one row,
    // clipped, with its tag.
    term.resize(Dimensions::new(10, 8));
    let narrow: Vec<(String, u16)> = lines(&term).into_iter().zip(tags(&term)).collect();
    let want = [
        ("0123456789", 0),
        ("abcdefghij", 0),
        ("KL", 0),
        ("+-card----", tag),
        ("|wide card", tag),
        ("ABCDEFGHIJ", 0),
        ("KLMNOPQRST", 0),
        ("UVW", 0),
    ];
    assert_eq!(
        narrow[..want.len()],
        want.map(|(text, tag)| (text.to_string(), tag))
    );
    assert!(term.grid().rows().any(|row| row.anchor().is_first()));
    assert_eq!(term.take_anchor_events(), vec![]);

    // Wider: the neighbours join up again; the clipped rows are padded and
    // keep their tag.
    term.resize(Dimensions::new(30, 8));
    let wide: Vec<(String, u16)> = lines(&term).into_iter().zip(tags(&term)).collect();
    let want = [
        ("0123456789abcdefghijKL", 0),
        ("+-card----", tag),
        ("|wide card", tag),
        ("ABCDEFGHIJKLMNOPQRSTUVW", 0),
    ];
    assert_eq!(
        wide[..want.len()],
        want.map(|(text, tag)| (text.to_string(), tag))
    );
    let first = term.buffer().anchor_line(tag).unwrap();
    assert_eq!(term.buffer().row_at(first).unwrap().width(), 30);
    assert_eq!(term.take_anchor_events(), vec![]);
}

#[test]
fn a_tagged_row_ends_the_line_that_wrapped_into_it() {
    let mut term = term(10, 4);
    let tag = place(&mut term, 1, 2);
    // Fallback text one character too long wraps into the second row.
    feed(&mut term, b"0123456789X\r\n");
    assert!(term.grid().row(0).unwrap().is_wrapped());
    term.resize(Dimensions::new(20, 4));
    assert_eq!(lines(&term)[..2], ["0123456789", "X"].map(String::from));
    assert_eq!(tags(&term)[..2], [tag, tag]);
    assert!(!term.grid().row(0).unwrap().is_wrapped());
}

#[test]
fn an_untagged_line_that_wrapped_into_a_reservation_ends_before_it() {
    let mut term = term(10, 5);
    // A prompt wraps onto the row the anchor then reserves.
    feed(&mut term, b"0123456789X\r");
    let tag = place(&mut term, 1, 1);
    feed(&mut term, b"card\r\nafter\r\n");
    assert!(term.grid().row(0).unwrap().is_wrapped());
    term.resize(Dimensions::new(20, 5));
    assert_eq!(
        lines(&term)[..3],
        ["0123456789", "card", "after"].map(String::from)
    );
    assert_eq!(tags(&term)[..3], [0, tag, 0]);
}

#[test]
fn reflow_keeps_the_cursor_on_its_tagged_row() {
    let mut term = term(20, 4);
    let tag = place(&mut term, 1, 2);
    feed(&mut term, b"abcdefghijklmno");
    term.resize(Dimensions::new(8, 4));
    let line = term.buffer().anchor_line(tag).unwrap();
    assert_eq!(cursor(&term), (line as u16, 7));
    term.resize(Dimensions::new(20, 4));
    assert_eq!(cursor(&term), (line as u16, 7));
}

#[test]
fn reflow_blanks_a_wide_character_cut_by_the_clip() {
    let mut term = term(10, 4);
    let tag = place(&mut term, 1, 1);
    feed(&mut term, "abc中de".as_bytes());
    term.resize(Dimensions::new(4, 4));
    let line = term.buffer().anchor_line(tag).unwrap();
    assert_eq!(term.buffer().row_at(line).unwrap().text_trimmed(), "abc");
    term.check_integrity().unwrap();
}

#[test]
fn reflow_drops_anchors_pushed_past_the_scrollback_limit() {
    let mut term = term(20, 3);
    term.set_scrollback_limit(3);
    let tag = place(&mut term, 1, 1);
    feed(&mut term, b"card\r\n");
    for _ in 0..3 {
        feed(&mut term, b"long line long line \r\n");
    }
    assert_eq!(term.buffer().anchor_line(tag), Some(0));
    // Halving the width doubles the rows after the card, which pushes it
    // out of scrollback.
    term.resize(Dimensions::new(10, 3));
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 1)]);
}

#[test]
fn a_full_table_refuses_anchors_until_tags_are_freed() {
    let mut term = term(20, 3);
    let mut wire = Vec::new();
    for surface in 0..MAX_ANCHORS as u32 {
        wire.extend(anchor_bytes(7, surface, 1));
        wire.extend_from_slice(b"\r\n");
    }
    feed(&mut term, &wire);
    let events = term.take_anchor_events();
    assert_eq!(events.len(), MAX_ANCHORS);
    assert!(events.iter().enumerate().all(|(i, e)| matches!(
        e,
        AnchorEvent::Placed { tag, .. } if usize::from(*tag) == i + 1
    )));
    assert_eq!(term.buffer().anchors().count(), MAX_ANCHORS);

    feed(&mut term, &anchor_bytes(7, 5000, 1));
    assert_eq!(
        term.take_anchor_events(),
        vec![AnchorEvent::Refused {
            anchor: anchor(5000, 1),
            reason: AnchorRefusal::TableFull,
        }]
    );

    // Clearing scrollback loses all but the last two (still on screen), and
    // their tags are reused.
    feed(&mut term, b"\x1b[3J");
    let gone = term.take_anchor_events();
    assert_eq!(gone.len(), MAX_ANCHORS - 2);
    assert_eq!(place(&mut term, 5000, 1), 1);
}

#[test]
fn releasing_an_anchor_frees_its_tag_and_rows() {
    let mut term = term(20, 5);
    let tag = place(&mut term, 1, 3);
    assert_eq!(term.buffer_mut().release_anchor(tag), Some(anchor(1, 3)));
    assert!(tags(&term).iter().all(|&t| t == 0));
    assert_eq!(term.buffer_mut().release_anchor(tag), None);
    assert_eq!(term.take_anchor_events(), vec![]);
    term.check_integrity().unwrap();
}

#[test]
fn a_new_anchor_over_an_old_first_row_replaces_it() {
    let mut term = term(20, 5);
    let old = place(&mut term, 1, 2);
    feed(&mut term, b"\x1b[1H");
    feed(&mut term, &anchor_bytes(7, 2, 1));
    assert_eq!(
        term.take_anchor_events(),
        vec![
            AnchorEvent::Placed {
                tag: 2,
                anchor: anchor(2, 1)
            },
            lost(old, 1, 2)
        ]
    );
    assert_eq!(tags(&term), vec![2, 0, 0, 0, 0]);
}

#[test]
fn anchors_are_ignored_until_turned_on() {
    let mut term = Terminal::new(Dimensions::new(20, 5));
    assert!(!term.inline_anchors());
    feed(&mut term, b"$ ");
    feed(&mut term, &anchor_bytes(7, 1, 3));
    feed(&mut term, b"card");
    assert_eq!(term.take_anchor_events(), vec![]);
    assert!(tags(&term).iter().all(|&t| t == 0));
    assert_eq!(term.grid().row(0).unwrap().text_trimmed(), "$ card");
    assert_eq!(cursor(&term), (0, 6));

    term.set_inline_anchors(true);
    feed(&mut term, b"\r\n");
    let tag = place(&mut term, 1, 3);
    assert_eq!(tags(&term), vec![0, tag, tag, tag, 0]);
}

#[test]
fn erasing_below_the_first_row_loses_the_anchor() {
    let mut term = term(20, 6);
    let tag = place(&mut term, 1, 3);
    feed(&mut term, b"card 1\r\ncard 2\r\ncard 3\r\n");
    // ED 0 on the first row keeps that row's tag (it erases only part of
    // it) but erases the rest of the reservation.
    feed(&mut term, b"\x1b[1;1H\x1b[J");
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 3)]);
    assert!(tags(&term).iter().all(|&t| t == 0));
}

#[test]
fn inserting_or_deleting_a_line_inside_a_reservation_loses_it() {
    for split in [&b"\x1b[2;1H\x1b[L"[..], b"\x1b[2;1H\x1b[M"] {
        let mut term = term(20, 6);
        let tag = place(&mut term, 1, 3);
        feed(&mut term, split);
        assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 3)]);
        assert!(tags(&term).iter().all(|&t| t == 0));
    }
}

#[test]
fn a_first_row_moved_on_the_grid_then_scrolled_off_is_still_found() {
    let mut term = term(20, 6);
    feed(&mut term, b"\x1b[2H");
    let tag = place(&mut term, 1, 2);
    // Insert line moves the card down; then, with no sweep in between,
    // output scrolls it into scrollback.
    feed(&mut term, b"\x1b[1H\x1b[2L\x1b[6H");
    for n in 0..20 {
        feed(&mut term, format!("{n}\r\n").as_bytes());
    }
    assert_eq!(term.take_anchor_events(), vec![]);
    let first = tags(&term).iter().position(|&t| t == tag).unwrap();
    assert!(first < term.buffer().history_len());
    assert_eq!(term.buffer().anchor_line(tag), Some(first));
}

#[test]
fn stale_tags_left_in_scrollback_are_dropped_by_reflow() {
    let mut term = term(20, 4);
    let tag = place(&mut term, 1, 2);
    feed(&mut term, b"card 1\r\ncard 2");
    // Insert line on the second row pulls the reservation apart; output
    // then scrolls all of it into scrollback before anything sweeps.
    feed(&mut term, b"\x1b[2;1H\x1b[L\x1b[4H");
    for n in 0..6 {
        feed(&mut term, format!("{n}\r\n").as_bytes());
    }
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 2)]);
    // The second row was two lines down, outside what the anchor reserved,
    // and keeps a stale tag that marks no first row.
    assert_eq!(tags(&term)[..3], [0, 0, tag]);
    term.resize(Dimensions::new(10, 4));
    assert!(tags(&term).iter().all(|&t| t == 0));
    assert!(lines(&term).contains(&"card 2".to_string()));
}

#[test]
fn a_reservation_pulled_back_from_scrollback_can_be_erased_again() {
    let mut term = term(20, 4);
    let tag = place(&mut term, 1, 2);
    feed(&mut term, b"card 1\r\ncard 2\r\n");
    for n in 0..6 {
        feed(&mut term, format!("{n}\r\n").as_bytes());
    }
    // Checked while all in scrollback...
    assert_eq!(term.take_anchor_events(), vec![]);
    // ...then back on the grid, where its second row is erased.
    term.resize(Dimensions::new(20, 12));
    assert_eq!(term.buffer().history_len(), 0);
    feed(&mut term, b"\x1b[2;1H\x1b[2K");
    assert_eq!(term.take_anchor_events(), vec![lost(tag, 1, 2)]);
}

#[test]
fn an_anchor_at_the_last_column_keeps_its_text_on_its_row() {
    let mut term = term(10, 4);
    feed(&mut term, b"0123456789");
    let tag = place(&mut term, 1, 1);
    feed(&mut term, b"X");
    assert_eq!(term.grid().row(0).unwrap().text_trimmed(), "012345678X");
    assert_eq!(tags(&term), vec![tag, 0, 0, 0]);
}
