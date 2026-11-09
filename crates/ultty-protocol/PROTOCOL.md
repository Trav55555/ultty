# Ultty Native Protocol

See `docs/PROTOCOL_SPEC.md` for the full specification; `src/codec.rs` is normative.

Binary protocol over a Unix domain socket between applications and Ultty.

## Connection

The app sends `ClientHello(version: u16, requested_capabilities: u32)`. The terminal responds with `ServerHello(version: u16, capabilities: u32)` and `SessionReady(session_id: u64, root_region: u32, width: u16, height: u16)`. Current and minimum version are 1; capabilities are `requested & supported`.

## Wire Format

Every message has an eight-byte header:

```
Offset  Size  Field
0       2     Magic 0x55 0x54 ("UT")
2       2     Type u16 LE
4       4     Payload length u32 LE
8       N     Payload
```

Integers and finite IEEE-754 `f32` values are little-endian. Strings are `u32` length plus UTF-8, options have a `u8` 0/1 tag, and booleans/enums use `u8`.

Limits: payload 16 MiB; string 1 MiB; regions 10,000; tracks 1,000; spans 10,000; metadata pairs 1,024 and metadata bytes 1 MiB; semantics strings 4 KiB each and 8 KiB together; surface rows 1 to 128; image data 16 MiB; image width/height 16,384 each, with sufficient RGBA data.

## Message Types

Codes are separate namespaces by direction.

### App -> Terminal

| Code | Message | Code | Message |
|------|---------|------|---------|
| `0x0001` | `ClientHello` | `0x0010` | `CreateRegion` |
| `0x0012` | `DeleteRegion` | `0x0013` | `ReparentRegion` |
| `0x0014` | `SetRegionMetadata` | `0x0015` | `CreateSurface` |
| `0x0016` | `CloseSurface` | `0x0020` | `SetContent` |
| `0x0021` | `SetStyle` | `0x0022` | `SetConstraints` |
| `0x0023` | `AppendContent` | `0x0024` | `StreamStart` |
| `0x0025` | `StreamEnd` | `0x0026` | `SetSemantics` |
| `0x0030` | `BeginFrame` | `0x0031` | `EndFrame` |
| `0x0040` | `Scroll` | `0x0041` | `Focus` |
| `0x0042` | `Blur` | `0x0050` | `Subscribe` |
| `0x0051` | `Unsubscribe` | `0x0060` | `ClipboardGet` |
| `0x0061` | `ClipboardSet` | `0x0070` | `Pong` |
| `0x00FF` | `Disconnect` | | |

### Terminal -> App

| Code | Message | Code | Message |
|------|---------|------|---------|
| `0x0001` | `ServerHello` | `0x0002` | `SessionReady` |
| `0x0010` | `LayoutComputed` | `0x0020` | `Input` |
| `0x0022` | `Resize` | `0x0023` | `FocusEvent` |
| `0x0024` | `ScrollEvent` | `0x0025` | `ClipboardContent` |
| `0x0026` | `SurfaceEvent` | `0x0070` | `Ping` |
| `0x00FF` | `Error` | | |

## Scene Tree

Region zero is the root viewport. Regions carry complete constraints, style, and content; `SetConstraints` replaces the full constraints value.

### Atomic Updates

`BeginFrame(frame_id: u32)` and matching `EndFrame(frame_id: u32)` batch mutations atomically.

## Content Types

Content is None, Text, RichText, or Widget. Widgets are Input, Markdown, Code, Progress, Spinner, Image, and Button.

## Accessibility Semantics

`SetSemantics` (gated by `CAP_SEMANTICS = 0x0200`) gives a region a role, label, liveness and state for screen readers; regions without it get roles derived from their content. See spec §10.5 and ADR 0021.

## Inline Surfaces

`CreateSurface` and `CloseSurface` (gated by `CAP_INLINE_SURFACES = 0x0100`) open and close region trees anchored to rows of the main screen, laid out at `cols × rows` independently of the root; the terminal answers with `SurfaceEvent`. See spec §10.6 and ADR 0020.

## Layout System

Constraints cover display, sizing, flexbox, grid, alignment, spacing, and overflow. Dimensions are Auto, Cells, Percent, Fr, Vw, Vh, MinContent, and MaxContent.

## Event Subscription

| Flag | Value |
|------|-------|
| `EVENT_MOUSE` | `0x01` |
| `EVENT_KEY` | `0x02` |
| `EVENT_FOCUS` | `0x04` |
| `EVENT_SCROLL` | `0x08` |

## Styling

Styles carry optional colors, border type/edges/radius, text flags, opacity, and an optional transition.

## Version

Current protocol version: **1**.
