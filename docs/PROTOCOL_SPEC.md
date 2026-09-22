# Ultimate Terminal Protocol Specification

**Version**: 0.1.0-draft  
**Status**: Draft  
**Last Updated**: 2026-09-22

`crates/ultty-protocol/src/codec.rs` is normative. Where this document and the code disagree, the code wins; a drift test in that crate checks the message-type codes and capability bits listed here.

---

## 1. Overview

The Ultimate Terminal Protocol (UTP) is a binary protocol between TUI applications and the Ultimate Terminal emulator. It provides declarative layout, retained rendering, atomic updates, and native content.

### 1.1 Design Goals

| Goal | Rationale |
|------|-----------|
| Sub-millisecond layout | Resize must be instant |
| Minimal wire overhead | Binary, not text |
| Backward compatible | Version and capability negotiation |
| Debuggable | Explicit framing and typed payloads |

### 1.2 Transport

The only transport is a Unix domain socket. The terminal binds one listening
socket per tab (`ultty-<pid>-tab<id>.sock` in the user's runtime directory) and
passes that tab's path to the tab's shell as `ULTTY_SOCKET`; an app connects to
the path in its own environment. A session belongs to the tab whose socket
accepted it: input events and clipboard reads are delivered only while that tab
is visible, and closing the tab unlinks its socket and disconnects its sessions.

---

## 2. Connection Lifecycle

```
App                                          Terminal
 │--- Connect (Unix domain socket) ----------->│
 │--- ClientHello(version, capabilities) ----->│
 │<-- ServerHello(version, capabilities) ------│
 │<-- SessionReady(session, root, size) --------│
 │<========== Bidirectional messages =========>│
 │--- Disconnect ----------------------------->│
```

### 2.1 Capability Negotiation

`ClientHello` is `version: u16`, `requested_capabilities: u32`.

| Constant | Value |
|----------|-------|
| `PROTOCOL_VERSION` | 1 |
| `MIN_PROTOCOL_VERSION` | 1 |
| `CAP_LAYOUT_FLEXBOX` | `0x0001` |
| `CAP_WIDGET_SCROLL` | `0x0010` |
| `CAP_APPEND_CONTENT` | `0x0020` |
| `CAP_STREAM_LIFECYCLE` | `0x0040` |
| `CAP_REGION_METADATA` | `0x0080` |

`CAP_ALL_KNOWN` is the OR of those bits. Capabilities are negotiated as `requested & supported`; the version is `min(client, server)`. A version below `MIN_PROTOCOL_VERSION` is rejected with `Error` 426.

| Response | Payload in encoding order |
|----------|---------------------------|
| `ServerHello` | `version: u16`, `capabilities: u32` |
| `SessionReady` | `session_id: u64`, `root_region: u32`, `width: u16`, `height: u16` |

---

## 3. Message Format

Integers are little-endian. Floats are IEEE-754 little-endian `f32`; NaN and infinity are rejected on decode. Strings are `u32` byte length plus UTF-8 bytes. `Option<T>` is a `u8` tag (`0`/`1`) plus `T` when present. Booleans are `u8`; enums are a `u8` discriminant plus variant data.

| Decode limit | Value |
|--------------|-------|
| `MAX_PAYLOAD_LEN` | 16 MiB frame cap; governs everything |
| `MAX_STRING_LEN` | 1 MiB |
| `MAX_REGION_COUNT` | 10,000 |
| `MAX_TRACK_COUNT` | 1,000 |
| `MAX_SPAN_COUNT` | 10,000 |
| `MAX_METADATA_PAIRS` | 1,024 |
| `MAX_METADATA_TOTAL_BYTES` | 1 MiB |
| `MAX_IMAGE_DATA_LEN` | 16 MiB, equal to the payload cap |
| Image dimensions | width and height each at most 16,384; `width * height * 4 <= data_len` |

If decoding consumes fewer bytes than declared, the remainder is skipped. Consuming more is an error.

### 3.1 Frame Structure

```
┌───────────┬────────────┬──────────────┬─────────────┐
│ Magic (2) │ Type (2)   │ Length (4)   │ Payload (N) │
│ 55 54     │ u16 LE     │ u32 LE       │             │
└───────────┴────────────┴──────────────┴─────────────┘
```

Length excludes the eight-byte header.

### 3.2 Message Types

App and terminal codes are separate namespaces.

#### App → Terminal

| Type | Code | Type | Code |
|------|------|------|------|
| `ClientHello` | `0x0001` | `CreateRegion` | `0x0010` |
| `DeleteRegion` | `0x0012` | `ReparentRegion` | `0x0013` |
| `SetRegionMetadata` | `0x0014` | `SetContent` | `0x0020` |
| `SetStyle` | `0x0021` | `SetConstraints` | `0x0022` |
| `AppendContent` | `0x0023` | `StreamStart` | `0x0024` |
| `StreamEnd` | `0x0025` | `BeginFrame` | `0x0030` |
| `EndFrame` | `0x0031` | `Scroll` | `0x0040` |
| `Focus` | `0x0041` | `Blur` | `0x0042` |
| `Subscribe` | `0x0050` | `Unsubscribe` | `0x0051` |
| `ClipboardGet` | `0x0060` | `ClipboardSet` | `0x0061` |
| `Pong` | `0x0070` | `Disconnect` | `0x00FF` |

#### Terminal → App

| Type | Code | Type | Code |
|------|------|------|------|
| `ServerHello` | `0x0001` | `SessionReady` | `0x0002` |
| `LayoutComputed` | `0x0010` | `Input` | `0x0020` |
| `Resize` | `0x0022` | `FocusEvent` | `0x0023` |
| `ScrollEvent` | `0x0024` | `ClipboardContent` | `0x0025` |
| `Ping` | `0x0070` | `Error` | `0x00FF` |

---

## 4. Region Model

### 4.1 Region Structure

A region is an app-assigned `RegionId(u32)` with a parent, ordered children, full `Constraints`, `Content`, and `Style`. Region zero is the terminal-owned root viewport; computed layout is terminal-owned.

### 4.2 Region Hierarchy

```
ROOT (0)
├── HEADER (1)
├── MAIN (2)
│   ├── SIDEBAR (3)
│   └── CONTENT (4)
└── FOOTER (5)
```

---

## 5. Layout Constraints

### 5.1 Constraint Structure

The full structure and exact encoding order are:

```rust
struct Constraints {
    display: Display,
    flex_direction: FlexDirection,
    flex_wrap: FlexWrap,
    justify_content: JustifyContent,
    align_items: AlignItems,
    align_self: AlignSelf,
    align_content: AlignContent,
    flex_grow: f32,
    flex_shrink: f32,
    flex_basis: Dimension,
    width: Dimension,
    height: Dimension,
    min_width: Dimension,
    min_height: Dimension,
    max_width: Dimension,
    max_height: Dimension,
    aspect_ratio: Option<f32>,
    margin: Edges<Dimension>,       // top, right, bottom, left
    padding: Edges<Dimension>,      // top, right, bottom, left
    gap: Size<Dimension>,           // width, height
    overflow: Overflow,             // x, y
    grid_template_columns: Vec<TrackSizingFunction>,
    grid_template_rows: Vec<TrackSizingFunction>,
    grid_column: GridPlacement,
    grid_row: GridPlacement,
}
```

Track vectors encode a `u32` count then items. A placement encodes `start` then `end`.

### 5.2 Dimension Type

`Dimension` values are `Auto=0`, `Cells(f32)=1`, `Percent(f32)=2`, `Fr(f32)=3`, `Vw(f32)=4`, `Vh(f32)=5`, `MinContent=6`, `MaxContent=7`.

### 5.3 Display Modes

`Display`: `Block=0`, `Flex=1`, `Grid=2`, `None=3`.

### 5.4 Flexbox Properties

| Type | Discriminants |
|------|---------------|
| `FlexDirection` | `Row=0`, `RowReverse=1`, `Column=2`, `ColumnReverse=3` |
| `FlexWrap` | `NoWrap=0`, `Wrap=1`, `WrapReverse=2` |
| `JustifyContent` | `Start=0`, `End=1`, `Center=2`, `SpaceBetween=3`, `SpaceAround=4`, `SpaceEvenly=5` |
| `AlignItems` | `Start=0`, `End=1`, `Center=2`, `Baseline=3`, `Stretch=4` |
| `AlignSelf` | `Auto=0`, `Start=1`, `End=2`, `Center=3`, `Baseline=4`, `Stretch=5` |
| `AlignContent` | `Start=0`, `End=1`, `Center=2`, `SpaceBetween=3`, `SpaceAround=4`, `SpaceEvenly=5`, `Stretch=6` |

### 5.5 Grid Properties

`GridLine`: `0 Auto`, `1 Line(i16)`, `2 Span(u16)`.

`TrackSizingFunction`: `0 Single(Dimension)`, `1 MinMax(Dimension, Dimension)`, `2 Repeat(count: u16, len: u32, dimensions)`. The repeat dimensions are `len` consecutive `Dimension` values.

### 5.6 Spacing (Edges)

`Edges<Dimension>` encodes top, right, bottom, left. `Size<Dimension>` encodes width, height.

### 5.7 Overflow

`Overflow` encodes X then Y. `OverflowAxis`: `Visible=0`, `Hidden=1`, `Scroll=2`, `Auto=3`.

---

## 6. Computed Layout

`ComputedRegionLayout` is `id: u32`, then eight `f32` values in this order: `x`, `y`, `width`, `height`, `content_x`, `content_y`, `content_width`, `content_height`; it ends with `visible: u8`.

---

## 7. Wire Format

### 7.1 CreateRegion Message

```
┌──────────────────────────────────────────────────┐
│ CreateRegion (0x0010)                            │
├──────────────────────────────────────────────────┤
│ id: u32                                          │
│ parent: u32                                      │
│ insert_index: i32 (-1 means append)              │
│ constraints: full Constraints                    │
│ style: full Style                                │
└──────────────────────────────────────────────────┘
```

### 7.2 Constraints Encoding

All fields are present and encode in section 5.1 order. Enum fields are `u8`; vectors carry `u32` counts; compound types retain their documented member order.

### 7.3 Dimension Encoding

A `Dimension` is a `u8` tag followed immediately by an `f32` LE value only for tags 1 through 5. Tags 0, 6, and 7 have no value: `0 Auto`, `1 Cells`, `2 Percent`, `3 Fr`, `4 Vw`, `5 Vh`, `6 MinContent`, `7 MaxContent`.

### 7.4 SetConstraints Message

`SetConstraints` is `id: u32` followed by the full `Constraints` structure. It is a full replacement; there is no partial-update encoding.

### 7.5 LayoutComputed Event

```
┌──────────────────────────────────────────────────┐
│ frame_id: Option<FrameId u32> (tag then value)  │
│ trigger: u8                                      │
│ region_count: u32                                │
│ regions: region_count × ComputedRegionLayout     │
└──────────────────────────────────────────────────┘
```

`LayoutTrigger`: `Resize=0`, `ConstraintChange=1`, `ContentChange=2`, `TreeChange=3`. Each region uses the exact layout in section 6.

---

## 8. Content Types

### 8.1 Content Enum

`Content` tag: `0 None`, `1 Text`, `2 RichText`, `3 Widget`.

### 8.2 Text Content

`TextContent`: string, then `wrap: u8` (`None=0`, `Char=1`, `Word=2`), `align: u8` (`Left=0`, `Center=1`, `Right=2`), `overflow: u8` (`Clip=0`, `Ellipsis=1`, `Fade=2`).

### 8.3 Rich Text Content

`RichText` is a `u32` span count followed by spans. Each span is a string followed by a full `Style`.

### 8.4 Widget Content

| Tag | Widget | Fields in encoding order |
|-----|--------|--------------------------|
| 0 | Input | value string, placeholder string, flags u8 (`multiline=1`, `password=2`) |
| 1 | Markdown | source string |
| 2 | Code | source string, language string, line_numbers u8 |
| 3 | Progress | value f32, label Option<string>, animated u8, indeterminate u8 |
| 4 | Spinner | label Option<string>, style u8 (`Braille=0`, `Dots=1`, `Line=2`, `Bounce=3`) |
| 5 | Image | width u32, height u32, scale u8, data_len u32, bytes |
| 6 | Button | label string, style u8 (`Default=0`, `Primary=1`, `Danger=2`, `Success=3`) |

Image scale is `Fit=0`, `Fill=1`, `Stretch=2`, `None=3`.

---

## 9. Style

`Style` encodes `fg: Option<Color>`, `bg: Option<Color>`, `border: BorderStyle`, `border_color: Option<Color>`, flags `u8`, opacity `f32`, then `Transition` if its flag is set. Flag bits are bold `1`, italic `2`, underline `4`, dim `8`, transition present `16`.

`BorderStyle` is type `u8`, edges `u8`, radius `u8`. Types are `None=0`, `Thin=1`, `Thick=2`, `Double=3`, `Rounded=4`, `Dashed=5`, `Ascii=6`. Edge bits are top `1`, right `2`, bottom `4`, left `8`.

`Color`: tag `0 Named(u8 0..17)`, `1 Rgb(u8,u8,u8)`, `2 Indexed(u8)`. Named values 0..15 are ANSI colors; 16 is foreground and 17 background.

`Transition` is `duration_ms: u32`, `easing: u8`, `properties: u32`. Easing is `Linear=0`, `EaseIn=1`, `EaseOut=2`, `EaseInOut=3`; unknown values are errors. Properties are `TRANSITION_OPACITY=0x01`, `TRANSITION_BG=0x02`, `TRANSITION_FG=0x04`, `TRANSITION_BORDER_COLOR=0x08`.

---

## 10. Events

### 10.1 Event Subscription

`Subscribe`/`Unsubscribe` are `region_id: u32`, `events: u32`. Bits: `EVENT_MOUSE=0x01`, `EVENT_KEY=0x02`, `EVENT_FOCUS=0x04`, `EVENT_SCROLL=0x08`.

### 10.2 Input Events

`InputEvent`: `region_id: u32`, event type, `x: f32`, `y: f32`, `modifiers: u8`.

| Tag | Input type payload |
|-----|--------------------|
| 0 | `MouseDown(button: u8)` |
| 1 | `MouseUp(button: u8)` |
| 2 | `MouseMove` |
| 3 | `MouseScroll(f32, f32)` |
| 4 | `KeyDown(Key, Modifiers u8)` |
| 5 | `KeyUp(Key, Modifiers u8)` |
| 6 | `TextInput(string)` |

Mouse buttons are Left 0, Middle 1, Right 2, Button4 3, Button5 4. Key tags are `0 Char(u32 codepoint)`, `1 Function(u8)`, `2 Escape`, `3 Enter`, `4 Tab`, `5 Backspace`, `6 Delete`, `7 Insert`, `8 Home`, `9 End`, `10 PageUp`, `11 PageDown`, `12 Up`, `13 Down`, `14 Left`, `15 Right`. Modifier bits are `SHIFT=0x01`, `CTRL=0x02`, `ALT=0x04`, `META=0x08`.

### 10.3 Resize Event

`ResizeEvent`: `width: u16`, `height: u16`. `FocusEvent`: `region_id: u32`, `focused: u8`. `ScrollEvent`: `region_id: u32`, `scroll_x: f32`, `scroll_y: f32`.

### 10.4 Session, Clipboard, and Extensions

The terminal sends empty `Ping` after 30 seconds idle. It closes after 10 seconds without `Pong`, or after 120 seconds idle. `Pong` is empty.

`ClipboardGet` is empty. `ClipboardSet` is a string. `ClipboardContent` is `Option<string>`.

`Error` is `code: u16`, message string. Codes: 400 bad request, 404 not found, 426 unsupported version, 500 internal.

| Message | Payload | Capability gate |
|---------|---------|-----------------|
| `AppendContent` | id u32, text string | `CAP_APPEND_CONTENT` |
| `StreamStart` | id u32, stream_id u32 | `CAP_STREAM_LIFECYCLE` |
| `StreamEnd` | id u32, stream_id u32 | `CAP_STREAM_LIFECYCLE` |
| `SetRegionMetadata` | id u32, pair count u32, key/value strings | `CAP_REGION_METADATA` |

`Scroll` is `id: u32`, `x: f32`, `y: f32`, `animate: u8`; `Focus` is `id: u32`; `Blur` is empty. None of these is capability-gated: `CAP_WIDGET_SCROLL` only advertises that the terminal handles scroll offsets and animation, and `CAP_LAYOUT_FLEXBOX` that it runs the flexbox layout described in section 5.

---

## 11. Atomic Updates

### 11.1 Frame Batching

```
App                                     Terminal
 │--- BeginFrame(id) --------------------->│
 │--- mutations -------------------------->│ (buffered)
 │--- EndFrame(id) ----------------------->│
 │<-- LayoutComputed(frame=id) ------------│
```

### 11.2 Frame Timing

`BeginFrame` and `EndFrame` carry only `frame_id: u32`. The terminal replies with `LayoutComputed` for every frame that changed the scene; content-only frames use `ContentChange`. A viewport resize produces `LayoutComputed` with `Resize`. A mismatched `EndFrame` discards the pending frame and returns `Error` 400.

---

## 12. Examples

### 12.1 Simple Layout

Create a full-size column container, fixed header/footer regions, and a growing content region. Each create carries complete constraints and style.

### 12.2 Responsive Sidebar

Use a row container, a sidebar with width bounds, and a main region with `flex_grow`. Constraint changes replace the full structure.

### 12.3 Grid Dashboard

Supply row and column track vectors and place children with start/end `GridLine` values.

---

## 13. Appendix: Comparison with ANSI

| Operation | ANSI | UTP |
|-----------|------|-----|
| Position | Cursor escapes | Computed layout |
| Boxes | Character sequences | Region border |
| Resize | App recomputes | Terminal recomputes and reports |
| Atomic update | Terminal extension | Frames |
| Color | SGR sequence | Typed color |

---

## 14. Version History

| Version | Date | Changes |
|---------|------|---------|
| 0.1.0-draft | 2026-09-22 | Regenerated wire-format sections from `codec.rs`; removed shared-memory/zero-copy claims |
| 0.1.0-draft | 2026-01-30 | Initial draft |

---

## 15. Open Questions

1. Which additional properties should transitions support?
2. How should accessibility semantics be exposed?
3. Should app-defined widgets be added in a future version?
4. Should terminal tabs or splits be represented?
