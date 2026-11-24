# Ultty Native Protocol v1 Wire Conformance Suite

`v1.json` is the language-independent binary wire conformance corpus for
Ultty Native Protocol v1 (see [docs/PROTOCOL_SPEC.md](../../../../docs/PROTOCOL_SPEC.md),
frozen at tag `protocol-v1`).

Existing vectors in `v1.json` are immutable once frozen; new additive vectors
for v1 coverage may be appended without altering existing hex encodings.

## Vector Format

Each entry in `v1.json` is a JSON object:

- `name`: human-readable test case description.
- `direction`: `"app"` (App → Terminal) or `"terminal"` (Terminal → App).
- `hex`: lowercase hexadecimal wire bytes for a single framed message
  (`magic[2] | type_u16_le | payload_len_u32_le | payload`).
- `expected`: either a decoded message expectation (`type != "reject"`) or a
  normative decode rejection (`{"type": "reject", "error": "<class>"}`).

Conforming codecs must:

1. Decode every non-reject vector into the expected message fields, consuming all bytes.
2. Re-encode every non-reject vector back to the exact same `hex` byte sequence.
3. Reject every `"reject"` vector with the matching error class:
   `unexpected_eof`, `invalid_magic`, `invalid_utf8`, `unknown_message_type`,
   `invalid_float`, or `invalid_data`.

## Running the Reference Implementations

Both the Rust and TypeScript implementations validate against `v1.json`:

```bash
# Rust reference codec (ultty-protocol)
cargo test -p ultty-protocol --test integration wire_conformance

# TypeScript client codec (@ultty/client, Node >= 22.6 or Bun)
scripts/ci-ts-conformance.sh
```
