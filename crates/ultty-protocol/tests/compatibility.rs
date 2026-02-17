use ultty_protocol::{
    negotiate_capabilities, negotiate_protocol_version, CAP_ALL_KNOWN, CAP_APPEND_CONTENT,
    CAP_LAYOUT_FLEXBOX, CAP_REGION_METADATA, CAP_STREAM_LIFECYCLE, CAP_WIDGET_SCROLL,
    MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};

#[test]
fn negotiate_protocol_version_same_version() {
    let version = negotiate_protocol_version(PROTOCOL_VERSION, PROTOCOL_VERSION);
    assert_eq!(version, Some(PROTOCOL_VERSION));
}

#[test]
fn negotiate_protocol_version_future_client_degrades_to_server() {
    let version = negotiate_protocol_version(PROTOCOL_VERSION + 1, PROTOCOL_VERSION);
    assert_eq!(version, Some(PROTOCOL_VERSION));
}

#[test]
fn negotiate_protocol_version_rejects_too_old() {
    let too_old = MIN_PROTOCOL_VERSION.saturating_sub(1);
    let version = negotiate_protocol_version(too_old, PROTOCOL_VERSION);
    assert_eq!(version, None);
}

#[test]
fn negotiate_protocol_version_matrix() {
    let too_old = MIN_PROTOCOL_VERSION.saturating_sub(1);
    let cases = [
        (PROTOCOL_VERSION, PROTOCOL_VERSION, Some(PROTOCOL_VERSION)),
        (
            PROTOCOL_VERSION + 3,
            PROTOCOL_VERSION,
            Some(PROTOCOL_VERSION),
        ),
        (
            PROTOCOL_VERSION,
            PROTOCOL_VERSION + 3,
            Some(PROTOCOL_VERSION),
        ),
        (
            MIN_PROTOCOL_VERSION,
            PROTOCOL_VERSION,
            Some(MIN_PROTOCOL_VERSION),
        ),
        (
            PROTOCOL_VERSION,
            MIN_PROTOCOL_VERSION,
            Some(MIN_PROTOCOL_VERSION),
        ),
        (too_old, PROTOCOL_VERSION, None),
        (PROTOCOL_VERSION, too_old, None),
    ];

    for (client, server, expected) in cases {
        assert_eq!(
            negotiate_protocol_version(client, server),
            expected,
            "client={client}, server={server}"
        );
    }
}

#[test]
fn negotiate_capabilities_intersects_masks() {
    let requested = CAP_LAYOUT_FLEXBOX | CAP_APPEND_CONTENT | CAP_STREAM_LIFECYCLE;
    let supported = CAP_LAYOUT_FLEXBOX | CAP_WIDGET_SCROLL | CAP_APPEND_CONTENT;
    let negotiated = negotiate_capabilities(requested, supported);
    assert_eq!(negotiated, CAP_LAYOUT_FLEXBOX | CAP_APPEND_CONTENT);
}

#[test]
fn negotiate_capabilities_drops_unknown_bits() {
    let unknown = 0x8000_0000;
    let requested = CAP_ALL_KNOWN | unknown;
    let negotiated = negotiate_capabilities(requested, CAP_ALL_KNOWN);
    assert_eq!(negotiated, CAP_ALL_KNOWN);
}

#[test]
fn known_capabilities_include_optional_protocol_features() {
    assert_ne!(CAP_ALL_KNOWN & CAP_APPEND_CONTENT, 0);
    assert_ne!(CAP_ALL_KNOWN & CAP_STREAM_LIFECYCLE, 0);
    assert_ne!(CAP_ALL_KNOWN & CAP_REGION_METADATA, 0);
}
