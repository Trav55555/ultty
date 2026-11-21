# Ultty protocol v1 conformance vectors

The vectors cover the binary frame format, hello and session messages, the
two message namespaces, text and legacy widgets, and three rejection cases.
Each row gives a direction, a complete frame in hex or a deliberately invalid
frame, and the expected message or error. Use the selected direction even
when the bytes are identical in both namespaces.

For a valid vector, decode from the hex and compare each field in
`expected`. Encode that message and compare the full frame byte for byte.
The `text` vector uses word wrapping, left alignment, and clip overflow.
Read `session_id` as an unsigned 64-bit integer from its decimal string.
For a rejection vector, require the specified error category. Error strings
may differ between implementations.

These cases run in workspace CI through the Rust integration test.

```sh
cargo test -p ultty-protocol --test wire_conformance
```

The format is defined in [the wire specification](../../../../docs/PROTOCOL_SPEC.md).
