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
