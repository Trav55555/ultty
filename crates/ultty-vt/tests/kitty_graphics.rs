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

// ---------------------------------------------------------------------------
// Refusals (slice 1)
// ---------------------------------------------------------------------------

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
        (
            "i=1,a=p,U=1",
            "EINVAL:Unicode placeholders are not supported",
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

// ---------------------------------------------------------------------------
// Transmission, decoding and storage
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Chunk reassembly, for any chunking and any split of the byte stream
// ---------------------------------------------------------------------------

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
