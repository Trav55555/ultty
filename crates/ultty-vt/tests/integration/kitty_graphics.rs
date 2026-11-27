//! Kitty graphics protocol conformance (ADR 0026, slice 1): commands in as
//! terminal output, replies and screen state out, one test per spec rule.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

fn feed(term: &mut Terminal, bytes: &[u8]) {
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(term);
    parser.feed(bytes, |action| handler.handle(action));
}

/// Everything the terminal answered to `bytes`, concatenated, with the
/// escape bytes shown as `<` and `>` for readable assertions.
fn replies(term: &mut Terminal, bytes: &[u8]) -> String {
    feed(term, bytes);
    let out = String::from_utf8(term.take_responses().concat()).expect("replies are UTF-8");
    out.replace("\x1b_G", "<").replace("\x1b\\", ">")
}

/// A graphics command with `control` keys and a base64 `payload`.
fn apc(control: &str, payload: &str) -> Vec<u8> {
    format!("\x1b_G{control};{payload}\x1b\\").into_bytes()
}

fn term() -> Terminal {
    Terminal::new(Dimensions::new(20, 5))
}

#[test]
fn file_temp_file_and_shared_memory_media_are_refused_with_ebadf() {
    // The payload names a file that exists; it must not be opened.
    let path = "L2V0Yy9wYXNzd2Q="; // "/etc/passwd"
    for medium in ["f", "t", "s"] {
        let mut term = term();
        let got = replies(&mut term, &apc(&format!("a=T,i=3,f=100,t={medium}"), path));
        assert_eq!(
            got, "<i=3;EBADF:Only direct transmission is supported>",
            "t={medium}"
        );
        assert_eq!(
            term.grid().row(0).unwrap().text_trimmed(),
            "",
            "nothing printed"
        );
    }
}

#[test]
fn every_failure_to_read_a_medium_gets_one_identical_reply() {
    // Whether the path exists, is a directory or is missing, the reply is
    // the same, so it reveals nothing about the file system.
    let mut term = term();
    let paths = ["L2V0Yy9wYXNzd2Q=", "L3RtcA==", "L25vL3N1Y2gvZmlsZQ=="];
    let got: Vec<String> = paths
        .iter()
        .map(|path| replies(&mut term, &apc("i=1,t=f", path)))
        .collect();
    assert!(got.iter().all(|reply| reply == &got[0]), "{got:?}");
}

#[test]
fn unsupported_features_are_refused_with_einval() {
    let cases = [
        (
            "i=1,f=8,s=1,v=1",
            "EINVAL:Indexed color images are not supported",
        ),
        (
            "i=1,f=16,s=1,v=1",
            "EINVAL:Indexed color images are not supported",
        ),
        (
            "i=1,a=p,P=2",
            "EINVAL:Relative placements are not supported",
        ),
        (
            "i=1,a=p,Q=2",
            "EINVAL:Relative placements are not supported",
        ),
        (
            "i=1,a=p,H=1",
            "EINVAL:Relative placements are not supported",
        ),
        (
            "i=1,a=T,V=-1",
            "EINVAL:Relative placements are not supported",
        ),
        ("i=1,a=f", "EINVAL:Animation is not supported"),
        ("i=1,a=a", "EINVAL:Animation is not supported"),
        ("i=1,a=c", "EINVAL:Animation is not supported"),
        (
            "i=1,a=q,e=T,U=1",
            "EINVAL:Feature queries are not supported",
        ),
        (
            "i=1,I=2",
            "EINVAL:Must not specify both image id and image number",
        ),
        ("i=1,s=x", "EINVAL:Malformed control data"),
        ("i=1,o=b", "EINVAL:Unknown image compression"),
        ("i=1,f=7", "EINVAL:Unknown image format"),
    ];
    for (control, message) in cases {
        let mut term = term();
        let got = replies(&mut term, &apc(control, "AAAA"));
        let ids = if control.contains("I=2") {
            "i=1,I=2"
        } else {
            "i=1"
        };
        assert_eq!(got, format!("<{ids};{message}>"), "{control}");
    }
}

#[test]
fn refusals_honour_the_quiet_modes() {
    let mut term = term();
    assert_eq!(
        replies(&mut term, &apc("i=1,t=f,q=1", "AAAA")),
        "<i=1;EBADF:Only direct transmission is supported>"
    );
    assert_eq!(replies(&mut term, &apc("i=1,t=f,q=2", "AAAA")), "");
}

#[test]
fn a_refusal_without_an_id_is_silent() {
    let mut term = term();
    assert_eq!(replies(&mut term, &apc("a=T,t=f,f=100", "AAAA")), "");
}

#[test]
fn replies_never_echo_request_bytes() {
    // A reply carries the parsed ids and fixed text only, never the payload
    // or a key the client sent.
    let mut term = term();
    let got = replies(&mut term, &apc("i=0007,t=f,k=evil", "ZXZpbCB0ZXh0"));
    assert_eq!(got, "<i=7;EBADF:Only direct transmission is supported>");
}

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

fn b64(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

fn zlib(bytes: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(bytes, 6)
}

/// A `width` x `height` RGBA PNG of one colour.
fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let pixels: Vec<u8> = (0..width * height).flat_map(|_| rgba).collect();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    out
}

/// The client ids and pixel sizes of the active screen's Kitty images.
fn stored(term: &Terminal) -> Vec<(u32, u32, u32)> {
    term.buffer()
        .kitty_images()
        .iter()
        .map(|image| (image.client_id, image.width, image.height))
        .collect()
}

#[test]
fn rgba_rgb_and_png_are_transmitted_and_stored() {
    let mut term = term();
    let got = replies(&mut term, &apc("i=1,f=32,s=1,v=1", &b64(&[1, 2, 3, 4])));
    assert_eq!(got, "<i=1;OK>");
    let got = replies(
        &mut term,
        &apc("i=2,f=24,s=2,v=1", &b64(&[1, 2, 3, 4, 5, 6])),
    );
    assert_eq!(got, "<i=2;OK>");
    let got = replies(&mut term, &apc("i=3,f=100", &b64(&png(3, 2, [9; 4]))));
    assert_eq!(got, "<i=3;OK>");
    assert_eq!(stored(&term), [(1, 1, 1), (2, 2, 1), (3, 3, 2)]);
    let rgb = term.buffer().kitty_images().by_client(2).unwrap();
    assert_eq!(&rgb.rgba[..], [1, 2, 3, 255, 4, 5, 6, 255]);
}

#[test]
fn compressed_data_is_inflated() {
    let mut term = term();
    let pixels = [7u8; 16];
    let got = replies(&mut term, &apc("i=1,s=2,v=2,o=z", &b64(&zlib(&pixels))));
    assert_eq!(got, "<i=1;OK>");
    let image = png(4, 4, [1, 2, 3, 255]);
    let control = format!("i=2,f=100,o=z,S={}", image.len());
    assert_eq!(
        replies(&mut term, &apc(&control, &b64(&zlib(&image)))),
        "<i=2;OK>"
    );
}

#[test]
fn a_decompression_bomb_is_refused_and_nothing_kept() {
    let mut term = term();
    let bomb = zlib(&vec![0; 16 * 1024 * 1024]);
    let got = replies(&mut term, &apc("i=1,s=1,v=1,o=z", &b64(&bomb)));
    assert_eq!(got, "<i=1;EINVAL:Failed to inflate image data>");
    assert!(stored(&term).is_empty());
}

#[test]
fn bad_data_gets_kittys_error_codes() {
    let cases = [
        (
            "i=1,s=2,v=2",
            b64(&[0; 4]),
            "ENODATA:Insufficient image data",
        ),
        (
            "i=1,s=1,v=1",
            b64(&[0; 8]),
            "EINVAL:Image dimensions do not match data size",
        ),
        (
            "i=1,s=0,v=1",
            b64(&[]),
            "EINVAL:Zero width/height not allowed",
        ),
        ("i=1,s=16385,v=1", b64(&[0; 4]), "EINVAL:Image too large"),
        (
            "i=1,f=100",
            b64(b"not a png"),
            "EBADPNG:Failed to decode PNG data",
        ),
        (
            "i=1,s=1,v=1",
            "!!!!".to_string(),
            "EINVAL:Invalid base64 payload",
        ),
    ];
    for (control, payload, message) in cases {
        let mut term = term();
        assert_eq!(
            replies(&mut term, &apc(control, &payload)),
            format!("<i=1;{message}>"),
            "{control}"
        );
        assert!(stored(&term).is_empty());
    }
}

#[test]
fn a_query_validates_and_stores_nothing() {
    let mut term = term();
    assert_eq!(
        replies(&mut term, &apc("i=31,s=1,v=1,a=q,t=d,f=24", "AAAA")),
        "<i=31;OK>"
    );
    assert!(stored(&term).is_empty());
    let got = replies(&mut term, &apc("i=32,a=q,f=100", &b64(b"junk")));
    assert_eq!(got, "<i=32;EBADPNG:Failed to decode PNG data>");
    // A query does not replace an image with the same id.
    replies(&mut term, &apc("i=5,s=1,v=1", "AAAAAA=="));
    replies(&mut term, &apc("i=5,a=q,s=2,v=1", &b64(&[0; 8])));
    assert_eq!(stored(&term), [(5, 1, 1)]);
}

#[test]
fn the_query_then_da1_probe_sees_the_reply_first() {
    let mut term = term();
    let got = replies(
        &mut term,
        b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c",
    );
    assert!(got.starts_with("<i=31;OK>\x1b[?"), "{got:?}");
}

#[test]
fn quiet_modes_suppress_ok_and_errors() {
    let mut term = term();
    assert_eq!(replies(&mut term, &apc("i=1,s=1,v=1,q=1", "AAAAAA==")), "");
    assert_eq!(
        replies(&mut term, &apc("i=2,s=1,v=1,q=1", "AA==")),
        "<i=2;ENODATA:Insufficient image data>"
    );
    assert_eq!(replies(&mut term, &apc("i=3,s=1,v=1,q=2", "AA==")), "");
    assert_eq!(stored(&term), [(1, 1, 1)]);
}

#[test]
fn an_image_number_gets_a_fresh_id_and_reply() {
    let mut term = term();
    let first = replies(&mut term, &apc("I=13,s=1,v=1", "AAAAAA=="));
    let second = replies(&mut term, &apc("I=13,s=1,v=1", "AAAAAA=="));
    assert_eq!(first, "<i=1,I=13;OK>");
    assert_eq!(second, "<i=2,I=13;OK>");
    assert_eq!(stored(&term).len(), 2, "numbers are not unique; both kept");
}

#[test]
fn retransmitting_an_id_replaces_the_image() {
    let mut term = term();
    replies(&mut term, &apc("i=4,s=1,v=1", "AAAAAA=="));
    replies(&mut term, &apc("i=4,s=2,v=1", &b64(&[0; 8])));
    assert_eq!(stored(&term), [(4, 2, 1)]);
}

#[test]
fn chunks_are_reassembled_and_answered_once() {
    let mut term = term();
    let data = b64(&[1, 2, 3, 4, 5, 6, 7, 8]); // 12 base64 bytes
    let mut got = replies(&mut term, &apc("i=9,s=2,v=1,m=1", &data[..4]));
    got += &replies(&mut term, &apc("m=1", &data[4..8]));
    got += &replies(&mut term, &apc("m=0", &data[8..]));
    assert_eq!(got, "<i=9;OK>");
    let image = term.buffer().kitty_images().by_client(9).unwrap();
    assert_eq!(&image.rgba[..], [1, 2, 3, 4, 5, 6, 7, 8]);
    assert!(term.buffer().kitty_images().upload.is_none());
}

#[test]
fn text_between_chunks_prints_and_does_not_corrupt_the_upload() {
    let mut term = term();
    let data = b64(&[1, 2, 3, 4, 5, 6, 7, 8]);
    feed(&mut term, &apc("i=9,s=2,v=1,m=1", &data[..8]));
    feed(&mut term, b"hi");
    feed(&mut term, &apc("m=0", &data[8..]));
    assert_eq!(term.grid().row(0).unwrap().text_trimmed(), "hi");
    assert_eq!(stored(&term), [(9, 2, 1)]);
}

#[test]
fn a_chunk_that_is_not_whole_base64_groups_aborts_the_upload() {
    let mut term = term();
    let got = replies(&mut term, &apc("i=9,s=2,v=1,m=1", "AAAAAA"));
    assert_eq!(got, "<i=9;EINVAL:Invalid base64 payload>");
    assert_eq!(replies(&mut term, &apc("m=0", "AAAA")), "");
    assert!(stored(&term).is_empty());
}

#[test]
fn the_last_chunks_quiet_mode_wins() {
    let mut term = term();
    feed(&mut term, &apc("i=9,s=1,v=1,m=1", "AAAA"));
    assert_eq!(replies(&mut term, &apc("m=0,q=1", "AA==")), "");
    assert_eq!(stored(&term).len(), 1);
}

#[test]
fn a_delete_aborts_an_upload() {
    let mut term = term();
    feed(&mut term, &apc("i=9,s=1,v=1,m=1", "AAAA"));
    feed(&mut term, b"\x1b_Ga=d\x1b\\");
    assert!(term.buffer().kitty_images().upload.is_none());
    assert_eq!(replies(&mut term, &apc("m=0", "AA==")), "");
    assert!(stored(&term).is_empty());
}

#[test]
fn a_new_image_header_abandons_an_unfinished_upload() {
    // A client that died mid-upload must not swallow the next image.
    let mut term = term();
    feed(&mut term, &apc("i=9,s=1,v=1,m=1", "AAAA"));
    let got = replies(&mut term, &apc("i=10,s=1,v=1", "AAAAAA=="));
    assert_eq!(got, "<i=10;OK>");
    assert_eq!(stored(&term), [(10, 1, 1)]);
}

#[test]
fn an_upload_past_the_transmit_cap_is_aborted_with_efbig() {
    let mut term = term();
    // 48 KiB of data per 64 KiB chunk: the cap of 32 MiB passes in the
    // 683rd chunk.
    let chunk = "A".repeat(64 * 1024 - 32);
    let mut got = replies(&mut term, &apc("i=1,f=100,m=1", &chunk));
    for _ in 0..700 {
        got += &replies(&mut term, &apc("m=1", &chunk));
        if !got.is_empty() {
            break;
        }
    }
    assert_eq!(got, "<i=1;EFBIG:Too much data>");
    assert!(
        term.buffer().kitty_images().upload.is_none(),
        "buffer freed"
    );
    assert_eq!(
        term.grid().row(0).unwrap().text_trimmed(),
        "",
        "nothing printed"
    );
}

#[test]
fn images_past_the_quota_evict_the_oldest_unplaced_first() {
    let mut term = term();
    // Room for two 16-byte images.
    term.set_image_storage_limit(40);
    for id in 1..=3 {
        replies(&mut term, &apc(&format!("i={id},s=2,v=2"), &b64(&[0; 16])));
    }
    assert_eq!(stored(&term), [(2, 2, 2), (3, 2, 2)]);
    term.check_integrity().unwrap();
}

#[test]
fn transient_images_are_evicted_before_older_ones() {
    let mut term = term();
    term.set_image_storage_limit(40);
    replies(&mut term, &apc("i=1,s=2,v=2", &b64(&[0; 16])));
    replies(&mut term, &apc("i=2,s=2,v=2,N=1", &b64(&[0; 16])));
    replies(&mut term, &apc("i=3,s=2,v=2", &b64(&[0; 16])));
    assert_eq!(stored(&term), [(1, 2, 2), (3, 2, 2)]);
}

#[test]
fn an_image_larger_than_the_quota_is_refused() {
    let mut term = term();
    term.set_image_storage_limit(8);
    let got = replies(&mut term, &apc("i=1,s=2,v=2", &b64(&[0; 16])));
    assert_eq!(got, "<i=1;ENOSPC:Image does not fit in the storage quota>");
    assert!(stored(&term).is_empty());
}

#[test]
fn a_zero_storage_limit_turns_both_protocols_off() {
    let mut term = term();
    term.set_image_storage_limit(0);
    assert_eq!(
        replies(&mut term, &apc("i=31,s=1,v=1,a=q,f=24", "AAAA")),
        ""
    );
    assert_eq!(replies(&mut term, &apc("i=1,t=f", "AAAA")), "");
    feed(&mut term, b"\x1bPq#0;2;100;0;0~~\x1b\\");
    assert!(term.images().is_empty());
    assert!(stored(&term).is_empty());
}

#[test]
fn each_screen_keeps_its_own_kitty_images_and_the_alternate_starts_bare() {
    let mut term = term();
    replies(&mut term, &apc("i=1,s=1,v=1", "AAAAAA=="));
    feed(&mut term, b"\x1b[?1049h");
    assert!(stored(&term).is_empty());
    replies(&mut term, &apc("i=2,s=1,v=1", "AAAAAA=="));
    feed(&mut term, b"\x1b[?1049l");
    assert_eq!(stored(&term), [(1, 1, 1)]);
    feed(&mut term, b"\x1b[?1049h");
    assert!(
        stored(&term).is_empty(),
        "the alternate screen's images went on leaving"
    );
    feed(&mut term, b"\x1b[?1049l\x1bc");
    assert!(stored(&term).is_empty(), "RIS clears them");
}

#[test]
fn sixel_and_kitty_images_share_one_quota() {
    let mut term = term();
    // A 1x6 sixel holds 24 bytes; room for one sixel and one 16-byte image.
    term.set_image_storage_limit(40);
    feed(&mut term, b"\x1bPq#0;2;100;0;0~\x1b\\");
    assert_eq!(term.images().len(), 1);
    replies(&mut term, &apc("i=1,s=2,v=2", &b64(&[0; 16])));
    assert_eq!((term.images().len(), stored(&term).len()), (1, 1));
    // A second sixel evicts the unplaced Kitty image first.
    feed(&mut term, b"\x1bPq#0;2;100;0;0~\x1b\\");
    assert_eq!((term.images().len(), stored(&term).len()), (1, 0));
    term.check_integrity().unwrap();
}

mod chunking {
    use super::*;
    use proptest::prelude::*;

    /// The upload of `pixels` (RGBA, `width` wide, one row) as commands:
    /// the base64 text cut at `cuts` (in 4-byte groups), with `text`
    /// printed between chunks.
    fn upload(width: u32, pixels: &[u8], cuts: &[usize], text: &str) -> Vec<u8> {
        let data = b64(pixels);
        let groups = data.len() / 4;
        let mut bounds: Vec<usize> = cuts.iter().map(|c| (c % groups.max(1)) * 4).collect();
        bounds.push(0);
        bounds.push(data.len());
        bounds.sort_unstable();
        bounds.dedup();
        let mut out = Vec::new();
        let chunks = bounds.len() - 1;
        for (n, pair) in bounds.windows(2).enumerate() {
            let more = u8::from(n + 1 < chunks);
            let control = if n == 0 {
                format!("i=7,s={width},v=1,m={more}")
            } else {
                format!("m={more}")
            };
            out.extend(apc(&control, &data[pair[0]..pair[1]]));
            if more == 1 {
                out.extend_from_slice(text.as_bytes());
            }
        }
        out
    }

    proptest! {
        #[test]
        fn any_chunking_and_feed_split_stores_the_same_image(
            width in 1u32..40,
            seed in any::<u8>(),
            cuts in proptest::collection::vec(0usize..64, 0..6),
            splits in proptest::collection::vec(1usize..64, 1..8),
            text in "[a-z]{0,3}",
        ) {
            let pixels: Vec<u8> = (0..width * 4).map(|i| (i as u8).wrapping_mul(31) ^ seed).collect();
            let stream = upload(width, &pixels, &cuts, &text);
            let mut term = term();
            {
                let mut parser = AnsiParser::new();
                let mut handler = TerminalHandler::new(&mut term);
                let mut rest = &stream[..];
                let mut cycle = splits.iter().cycle();
                while !rest.is_empty() {
                    let n = (*cycle.next().unwrap()).min(rest.len());
                    parser.feed(&rest[..n], |action| handler.handle(action));
                    rest = &rest[n..];
                }
            }
            let replies = String::from_utf8(term.take_responses().concat()).unwrap();
            prop_assert_eq!(replies, "\x1b_Gi=7;OK\x1b\\");
            let image = term.buffer().kitty_images().by_client(7).expect("stored");
            prop_assert_eq!(&image.rgba[..], &pixels[..]);
            prop_assert!(term.buffer().kitty_images().upload.is_none());
            term.check_integrity().unwrap();
        }

        #[test]
        fn a_delete_anywhere_mid_upload_aborts_it(
            width in 2u32..40,
            at in 1usize..8,
        ) {
            let pixels = vec![5u8; width as usize * 4];
            let cuts: Vec<usize> = (1..10).collect();
            let stream = upload(width, &pixels, &cuts, "");
            // Cut the stream after the `at`th command and delete there.
            let ends: Vec<usize> = stream
                .windows(2)
                .enumerate()
                .filter(|(_, w)| w == b"\x1b\\")
                .map(|(i, _)| i + 2)
                .collect();
            prop_assume!(at < ends.len());
            let mut term = term();
            feed(&mut term, &stream[..ends[at - 1]]);
            feed(&mut term, b"\x1b_Ga=d,d=a\x1b\\");
            feed(&mut term, &stream[ends[at - 1]..]);
            prop_assert!(term.buffer().kitty_images().is_empty());
            prop_assert!(term.take_responses().is_empty());
            prop_assert!(term.grid().row(0).unwrap().text_trimmed().is_empty());
        }
    }
}

/// A 20 x 5 terminal with 10 x 20 pixel cells.
fn sized() -> Terminal {
    let mut term = term();
    term.set_cell_pixel_size(10, 20);
    term
}

/// Each Kitty placement as (client id, placement id, screen row, col,
/// cols, rows, z), sorted.
fn placements(term: &Terminal) -> Vec<(u32, u32, isize, u16, u16, u16, i32)> {
    let buffer = term.buffer();
    let mut out: Vec<_> = term
        .images()
        .images()
        .filter_map(|image| {
            let kitty = image.kitty.as_ref()?;
            Some((
                kitty.client_id,
                kitty.placement_id,
                buffer.screen_row_of_line(image.line),
                image.start_col,
                image.cell_width,
                image.cell_height,
                image.z,
            ))
        })
        .collect();
    out.sort_unstable();
    out
}

/// An opaque `width` x `height` RGBA image, as base64.
fn pixels(width: u32, height: u32) -> String {
    b64(&vec![200; (width * height * 4) as usize])
}

fn cursor(term: &Terminal) -> (u16, u16) {
    let position = term.grid().cursor().position;
    (position.row, position.col)
}

#[test]
fn transmit_then_place_and_the_cursor_moves_past_the_image() {
    let mut term = sized();
    assert_eq!(
        replies(&mut term, &apc("i=1,s=25,v=30", &pixels(25, 30))),
        "<i=1;OK>"
    );
    assert!(placements(&term).is_empty(), "a=t only stores");
    feed(&mut term, b"\x1b[2;3H");
    assert_eq!(replies(&mut term, &apc("a=p,i=1,p=4", "")), "<i=1,p=4;OK>");
    assert_eq!(placements(&term), [(1, 4, 1, 2, 3, 2, 0)]);
    // Right by its 3 columns, down by its 2 rows less one.
    assert_eq!(cursor(&term), (2, 5));
}

#[test]
fn transmit_and_place_in_one_command() {
    let mut term = sized();
    assert_eq!(
        replies(&mut term, &apc("a=T,i=2,s=10,v=20,z=-3", &pixels(10, 20))),
        "<i=2;OK>"
    );
    assert_eq!(placements(&term), [(2, 0, 0, 0, 1, 1, -3)]);
    assert_eq!(cursor(&term), (0, 1));
}

#[test]
fn a_chunked_transmit_and_place_shows_at_the_cursor_of_the_last_chunk() {
    let mut term = sized();
    let data = pixels(10, 20);
    feed(&mut term, &apc("a=T,i=2,s=10,v=20,m=1", &data[..400]));
    feed(&mut term, b"\x1b[3;4H");
    feed(&mut term, &apc("m=0", &data[400..]));
    assert_eq!(placements(&term), [(2, 0, 2, 3, 1, 1, 0)]);
}

#[test]
fn c_1_leaves_the_cursor_alone() {
    let mut term = sized();
    feed(&mut term, &apc("a=T,i=1,s=30,v=40,C=1", &pixels(30, 40)));
    assert_eq!(cursor(&term), (0, 0));
}

#[test]
fn placing_past_the_bottom_scrolls_the_screen() {
    let mut term = sized();
    feed(&mut term, b"\x1b[5;1H");
    feed(&mut term, &apc("a=T,i=1,s=10,v=60", &pixels(10, 60)));
    // Three rows tall from the last row: the screen scrolled two lines and
    // the image scrolled with it.
    assert_eq!(placements(&term), [(1, 0, 2, 0, 1, 3, 0)]);
    assert_eq!(cursor(&term), (4, 1));
}

#[test]
fn the_same_image_and_placement_id_replaces_the_placement() {
    let mut term = sized();
    feed(&mut term, &apc("i=1,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, &apc("a=p,i=1,p=7", ""));
    feed(&mut term, b"\x1b[3;3H");
    feed(&mut term, &apc("a=p,i=1,p=7", ""));
    assert_eq!(placements(&term), [(1, 7, 2, 2, 1, 1, 0)]);
    // Without a placement id, each put adds a placement.
    feed(&mut term, &apc("a=p,i=1", ""));
    feed(&mut term, &apc("a=p,i=1", ""));
    assert_eq!(placements(&term).len(), 3);
}

#[test]
fn placing_a_missing_image_is_enoent() {
    let mut term = sized();
    assert_eq!(
        replies(&mut term, &apc("a=p,i=9,p=2", "")),
        "<i=9,p=2;ENOENT:Image not found>"
    );
    assert_eq!(
        replies(&mut term, &apc("a=p,I=9", "")),
        "<I=9;ENOENT:Image not found>"
    );
}

#[test]
fn a_number_places_the_newest_image_with_it() {
    let mut term = sized();
    feed(&mut term, &apc("I=5,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, &apc("I=5,s=20,v=20", &pixels(20, 20)));
    term.take_responses();
    assert_eq!(replies(&mut term, &apc("a=p,I=5", "")), "<i=2,I=5;OK>");
    assert_eq!(placements(&term), [(2, 0, 0, 0, 2, 1, 0)]);
}

#[test]
fn retransmitting_an_id_deletes_its_placements() {
    let mut term = sized();
    feed(&mut term, &apc("a=T,i=1,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, &apc("i=1,s=10,v=20", &pixels(10, 20)));
    assert!(placements(&term).is_empty());
}

#[test]
fn an_anonymous_image_lives_as_long_as_its_placements() {
    let mut term = sized();
    feed(&mut term, &apc("a=T,s=10,v=20", &pixels(10, 20)));
    assert_eq!(placements(&term).len(), 1);
    feed(&mut term, b"\x1b_Ga=d\x1b\\");
    assert!(placements(&term).is_empty());
    assert!(stored(&term).is_empty());
}

/// Image 1 placed at row 0 col 0 (z 0, p=1), image 2 at row 2 col 5 (z 5,
/// p=2), each one cell, plus unplaced image 3.
fn scene() -> Terminal {
    let mut term = sized();
    feed(&mut term, &apc("i=1,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, &apc("i=2,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, &apc("i=3,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, &apc("a=p,i=1,p=1,C=1", ""));
    feed(&mut term, b"\x1b[3;6H");
    feed(&mut term, &apc("a=p,i=2,p=2,z=5,C=1", ""));
    term
}

/// The client ids of the placements left after `delete`, and of the
/// images left.
fn after_delete(delete: &str) -> (Vec<u32>, Vec<u32>) {
    let mut term = scene();
    term.take_responses();
    assert_eq!(
        replies(&mut term, format!("\x1b_Ga=d,{delete}\x1b\\").as_bytes()),
        "",
        "deletes never reply"
    );
    term.check_integrity().unwrap();
    (
        placements(&term).iter().map(|p| p.0).collect(),
        stored(&term).iter().map(|s| s.0).collect(),
    )
}

#[test]
fn every_delete_value_removes_what_the_spec_says() {
    let cases: [(&str, &[u32], &[u32]); 26] = [
        ("d=a", &[], &[1, 2, 3]),
        ("d=A", &[], &[3]),
        ("q=0", &[], &[1, 2, 3]),
        ("d=i,i=1", &[2], &[1, 2, 3]),
        ("d=I,i=1", &[2], &[2, 3]),
        ("d=I,i=3", &[1, 2], &[1, 2]),
        ("d=i,i=2,p=9", &[1, 2], &[1, 2, 3]),
        ("d=I,i=2,p=2", &[1], &[1, 3]),
        ("d=n,I=7", &[1, 2], &[1, 2, 3]),
        ("d=c", &[1], &[1, 2, 3]),
        ("d=C", &[1], &[1, 3]),
        ("d=p,x=6,y=3", &[1], &[1, 2, 3]),
        ("d=P,x=6,y=3", &[1], &[1, 3]),
        ("d=q,x=6,y=3,z=4", &[1, 2], &[1, 2, 3]),
        ("d=Q,x=6,y=3,z=5", &[1], &[1, 3]),
        ("d=r,x=1,y=2", &[], &[1, 2, 3]),
        ("d=R,x=2,y=3", &[1], &[1]),
        ("d=x,x=6", &[1], &[1, 2, 3]),
        ("d=X,x=1", &[2], &[2, 3]),
        ("d=y,y=3", &[1], &[1, 2, 3]),
        ("d=Y,y=1", &[2], &[2, 3]),
        ("d=z,z=5", &[1], &[1, 2, 3]),
        ("d=Z,z=0", &[2], &[2, 3]),
        ("d=f,i=1", &[1, 2], &[1, 2, 3]),
        ("d=F,i=1", &[1, 2], &[1, 2, 3]),
        ("d=?", &[1, 2], &[1, 2, 3]),
    ];
    for (delete, left, kept) in cases {
        assert_eq!(
            after_delete(delete),
            (left.to_vec(), kept.to_vec()),
            "{delete}"
        );
    }
}

#[test]
fn delete_by_number_takes_the_newest_image_with_it() {
    let mut term = sized();
    feed(&mut term, &apc("a=T,I=7,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, &apc("a=T,I=7,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, b"\x1b_Ga=d,d=N,I=7\x1b\\");
    assert_eq!(stored(&term).iter().map(|s| s.0).collect::<Vec<_>>(), [1]);
}

#[test]
fn deletes_leave_sixel_images_alone() {
    let mut term = sized();
    feed(&mut term, b"\x1bPq#0;2;100;0;0~~\x1b\\");
    feed(&mut term, b"\x1b_Ga=d,d=A\x1b\\");
    assert_eq!(term.images().len(), 1);
}

#[test]
fn placements_scroll_with_text_into_scrollback_and_are_dropped_on_eviction() {
    let mut term = sized();
    term.set_scrollback_limit(3);
    feed(&mut term, &apc("a=T,i=1,s=10,v=20,C=1", &pixels(10, 20)));
    feed(&mut term, b"\n\n");
    feed(&mut term, b"\x1b[5;1H\n\n");
    assert_eq!(placements(&term), [(1, 0, -2, 0, 1, 1, 0)]);
    feed(&mut term, b"\n\n");
    assert!(placements(&term).is_empty(), "evicted with its line");
    assert_eq!(stored(&term).len(), 1, "the image itself stays");
}

#[test]
fn erases_and_text_leave_kitty_placements_alone() {
    let mut term = sized();
    feed(&mut term, &apc("a=T,i=1,s=30,v=20,C=1", &pixels(30, 20)));
    feed(&mut term, b"abc\x1b[K\x1b[2K\x1b[1J\x1b[0J\x1b[3X\x1b[2P");
    assert_eq!(placements(&term), [(1, 0, 0, 0, 3, 1, 0)]);
}

#[test]
fn ed_2_clears_screen_placements_and_ed_3_scrollback_ones() {
    let mut term = sized();
    feed(&mut term, &apc("a=T,i=1,s=10,v=20,C=1", &pixels(10, 20)));
    feed(&mut term, b"\x1b[5;1H\n\n\n\n\n");
    feed(&mut term, &apc("a=T,i=2,s=10,v=20,C=1", &pixels(10, 20)));
    assert_eq!(placements(&term).len(), 2);
    feed(&mut term, b"\x1b[2J");
    assert_eq!(
        placements(&term).iter().map(|p| p.0).collect::<Vec<_>>(),
        [1]
    );
    feed(&mut term, b"\x1b[3J");
    assert!(placements(&term).is_empty());
    assert_eq!(stored(&term).len(), 2, "images with ids stay stored");
}

#[test]
fn the_alternate_screen_starts_bare_and_main_placements_survive_it() {
    let mut term = sized();
    feed(&mut term, &apc("a=T,i=1,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, b"\x1b[?1049h");
    assert!(placements(&term).is_empty());
    feed(&mut term, &apc("a=T,i=1,s=10,v=20", &pixels(10, 20)));
    assert_eq!(placements(&term).len(), 1);
    feed(&mut term, b"\x1b[?1049l");
    assert_eq!(placements(&term).len(), 1);
    feed(&mut term, b"\x1b[?1049h");
    assert!(placements(&term).is_empty());
}

#[test]
fn ris_clears_everything() {
    let mut term = sized();
    feed(&mut term, &apc("a=T,i=1,s=10,v=20", &pixels(10, 20)));
    feed(&mut term, b"\x1bc");
    assert!(placements(&term).is_empty());
    assert!(stored(&term).is_empty());
}

#[test]
fn a_region_scroll_moves_placements_inside_it() {
    let mut term = sized();
    feed(&mut term, b"\x1b[3;1H");
    feed(&mut term, &apc("a=T,i=1,s=10,v=20,C=1", &pixels(10, 20)));
    feed(&mut term, b"\x1b[2;4r\x1b[4;1H\n");
    assert_eq!(placements(&term), [(1, 0, 1, 0, 1, 1, 0)]);
    feed(&mut term, b"\n");
    assert!(placements(&term).is_empty(), "pushed out of the region");
}

/// As for sixel images (slice 0), reflow moves a placement to the row its
/// top-left cell's line went to, and keeps its column.
#[test]
fn reflow_keeps_a_placement_on_its_line() {
    let mut term = sized();
    feed(&mut term, b"aaaaaaaaaaaaaaaaaaaabbbb");
    feed(&mut term, &apc("a=T,i=1,s=10,v=20,C=1", &pixels(10, 20)));
    // On the second row; widened to 40 the two rows join into the first.
    assert_eq!(placements(&term), [(1, 0, 1, 4, 1, 1, 0)]);
    term.resize(Dimensions::new(40, 5));
    assert_eq!(placements(&term), [(1, 0, 0, 4, 1, 1, 0)]);
    term.check_integrity().unwrap();
}

#[test]
fn kitty_placements_and_sixel_images_coexist() {
    let mut term = sized();
    feed(&mut term, b"\x1bPq#0;2;100;0;0~~\x1b\\");
    feed(&mut term, &apc("a=T,i=1,s=10,v=20", &pixels(10, 20)));
    assert_eq!((term.images().len(), placements(&term).len()), (2, 1));
    // Text over the sixel cuts it; the Kitty placement is untouched.
    feed(&mut term, b"\x1b[1;1HXXXX");
    assert_eq!(placements(&term).len(), 1);
    term.check_integrity().unwrap();
}

#[test]
fn placements_past_the_limit_evict_the_oldest() {
    let mut term = sized();
    feed(&mut term, &apc("i=1,s=1,v=1", &pixels(1, 1)));
    for _ in 0..ultty_vt::grid::MAX_PLACEMENTS + 3 {
        feed(&mut term, &apc("a=p,i=1,C=1,q=2", ""));
    }
    assert!(term.images().len() <= ultty_vt::grid::MAX_PLACEMENTS);
    term.check_integrity().unwrap();
}

#[test]
fn ed_2_keeps_sixel_images_in_scrollback_too() {
    let mut term = sized();
    feed(&mut term, b"\x1bPq#0;2;100;0;0~~\x1b\\");
    feed(&mut term, b"\x1b[5;1H\n\n\n\n\n\n");
    feed(&mut term, b"\x1bPq#0;2;100;0;0~~\x1b\\");
    assert_eq!(term.images().len(), 2);
    feed(&mut term, b"\x1b[2J");
    assert_eq!(
        term.images().len(),
        1,
        "the scrollback image stays for ED 3"
    );
    feed(&mut term, b"\x1b[3J");
    assert!(term.images().is_empty());
}

#[test]
fn virtual_placement_and_placeholder_inheritance_resolve_and_coalesce() {
    use ultty_vt::grid::graphics::{
        index_to_diacritic, resolve_visible_placeholders, PLACEHOLDER_CHAR,
    };

    let mut term = sized();
    assert_eq!(
        replies(
            &mut term,
            &apc("a=T,U=1,i=42,c=3,r=2,s=30,v=40", &pixels(30, 40)),
        ),
        "<i=42;OK>"
    );
    assert!(
        term.images().is_empty(),
        "virtual placement is invisible as a direct placement"
    );
    assert_eq!(
        term.grid().cursor().position,
        ultty_vt::grid::CursorPosition::new(0, 0),
        "virtual placement does not move cursor"
    );
    assert_eq!(term.buffer().kitty_images().virtual_placements_len(), 1);

    let d0 = index_to_diacritic(0).unwrap();
    let d1 = index_to_diacritic(1).unwrap();
    let row0 = format!(
        "\x1b[38;5;42m{PLACEHOLDER_CHAR}{d0}{PLACEHOLDER_CHAR}{PLACEHOLDER_CHAR}\x1b[39m\r\n"
    );
    let row1 =
        format!("\x1b[38;5;42m{PLACEHOLDER_CHAR}{d1}{PLACEHOLDER_CHAR}{PLACEHOLDER_CHAR}\x1b[39m");
    feed(&mut term, row0.as_bytes());
    feed(&mut term, row1.as_bytes());

    let quads = resolve_visible_placeholders(term.buffer(), |_| true);
    assert_eq!(
        quads.len(),
        1,
        "full 3x2 placeholder grid coalesces to one quad"
    );
    let q = quads[0];
    assert_eq!((q.client_id, q.placement_id), (42, 0));
    assert_eq!(q.screen, [0.0, 0.0, 3.0, 2.0]);
    assert_eq!(q.uv, [0.0, 0.0, 1.0, 1.0]);

    assert_eq!(term.grid().row(0).unwrap().copy_text(0, 2), "");
    assert_eq!(term.grid().row(1).unwrap().copy_text(0, 2), "");
    term.check_integrity().unwrap();
}

#[test]
fn placeholder_msb_diacritic_underline_placement_id_and_cropping() {
    use ultty_vt::grid::graphics::{
        index_to_diacritic, resolve_visible_placeholders, PLACEHOLDER_CHAR,
    };

    let mut term = sized();
    let high_id: u32 = 42 + (2 << 24);
    assert_eq!(
        replies(
            &mut term,
            &apc(&format!("a=t,i={high_id},s=20,v=40"), &pixels(20, 40),),
        ),
        format!("<i={high_id};OK>")
    );
    assert_eq!(
        replies(
            &mut term,
            &apc(&format!("a=p,U=1,i={high_id},p=7,c=2,r=2"), ""),
        ),
        format!("<i={high_id},p=7;OK>")
    );

    let d0 = index_to_diacritic(0).unwrap();
    let d1 = index_to_diacritic(1).unwrap();
    let d2 = index_to_diacritic(2).unwrap();
    let line = format!(
        "A\x1b[38;5;42;58;5;7m{PLACEHOLDER_CHAR}{d1}{d0}{d2}{PLACEHOLDER_CHAR}{d1}{d1}\x1b[39;59mB"
    );
    feed(&mut term, line.as_bytes());

    let quads = resolve_visible_placeholders(term.buffer(), |_| true);
    assert_eq!(quads.len(), 1);
    let q = quads[0];
    assert_eq!((q.client_id, q.placement_id), (high_id, 7));
    assert_eq!(q.screen, [1.0, 0.0, 3.0, 1.0]);
    assert_eq!(q.uv, [0.0, 0.5, 1.0, 1.0]);
    assert_eq!(term.grid().row(0).unwrap().copy_text(0, 3), "AB");
    term.check_integrity().unwrap();
}

#[test]
fn virtual_placement_deletion_follows_spec_rules() {
    let mut term = sized();
    assert_eq!(
        replies(
            &mut term,
            &apc("a=T,U=1,i=10,p=1,c=2,r=2,s=10,v=20", &pixels(10, 20))
        ),
        "<i=10,p=1;OK>"
    );
    assert_eq!(
        replies(&mut term, &apc("a=p,U=1,i=10,p=2,c=2,r=2", "")),
        "<i=10,p=2;OK>"
    );

    for d in [
        "a", "A", "c", "C", "p", "P", "q", "Q", "x", "X", "y", "Y", "z", "Z",
    ] {
        feed(&mut term, &apc(&format!("a=d,d={d},x=1,y=1,z=0"), ""));
        assert_eq!(
            term.buffer().kitty_images().virtual_placements_len(),
            2,
            "d={d} must not delete virtual placements"
        );
        assert!(
            term.buffer().kitty_images().by_client(10).is_some(),
            "d={d} must not free image with virtual placements"
        );
    }

    feed(&mut term, b"\x1b[2J");
    assert_eq!(
        term.buffer().kitty_images().virtual_placements_len(),
        2,
        "ED 2 must not delete virtual placements"
    );

    feed(&mut term, &apc("a=d,d=i,i=10,p=1", ""));
    assert_eq!(term.buffer().kitty_images().virtual_placements_len(), 1);
    assert!(term.buffer().kitty_images().by_client(10).is_some());

    feed(&mut term, &apc("a=d,d=I,i=10", ""));
    assert_eq!(term.buffer().kitty_images().virtual_placements_len(), 0);
    assert!(term.buffer().kitty_images().by_client(10).is_none());

    assert_eq!(
        replies(
            &mut term,
            &apc("a=T,U=1,i=20,c=2,r=2,s=10,v=20", &pixels(10, 20))
        ),
        "<i=20;OK>"
    );
    feed(&mut term, &apc("a=d,d=R,x=15,y=25", ""));
    assert_eq!(term.buffer().kitty_images().virtual_placements_len(), 0);
    assert!(term.buffer().kitty_images().by_client(20).is_none());

    assert_eq!(
        replies(
            &mut term,
            &apc("a=T,U=1,i=30,c=2,r=2,s=10,v=20", &pixels(10, 20))
        ),
        "<i=30;OK>"
    );
    feed(&mut term, b"\x1bc");
    assert_eq!(term.buffer().kitty_images().virtual_placements_len(), 0);
    term.check_integrity().unwrap();
}
