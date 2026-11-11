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

### 1.2 Transports

Frames (§3) travel over one of two transports. The frame byte stream is the
same on both.

**Socket.** The terminal binds one listening socket per tab
(`ultty-<pid>-tab<id>.sock` in the user's runtime directory) and passes that
tab's path to the tab's shell as `ULTTY_SOCKET`, with the tab's PTY slave
path as `ULTTY_TTY`; an app connects to the path in its own environment.
`ulttui` connects only when its stdin, stdout, or stderr is the `ULTTY_TTY`
terminal, so a copy of the variables inherited into a multiplexer pane or a
recording PTY is ignored (`ULTTY_FORCE_NATIVE=1` overrides this). The socket
reaches only programs on the same machine. A session belongs to the tab whose
socket accepted it: input events and clipboard reads are delivered only while
that tab is visible, and closing the tab unlinks its socket and disconnects
its sessions. Keyboard input (`KeyDown`, `KeyUp`, `TextInput`) also goes only
to a session whose connecting process (its peer pid) is in the foreground
process group of the focused pane's PTY, so a background job that subscribes
to keys sees nothing the user types. In-band sessions talk over the PTY
itself and qualify while they last.

**In-band** ([ADR 0018](adr/0018-in-band-native-transport.md)). Frames travel
in the terminal byte stream, so native mode works wherever that stream goes:
over ssh, under sudo, in containers, and through recorders such as `script`.
`ulttui` uses the socket when it can and probes for in-band otherwise.

### 1.3 In-band framing

The frame byte stream is cut into chunks of at most 65,536 bytes. Each chunk
is sent as one APC string:

```text
ESC _ U <base64url of the chunk, no padding> ESC \
```

- **Directions.** Apps write strings to their terminal (stdout). The
  terminal writes them to the app's input (stdin).
- **Reassembly.** A frame may span strings and a string may hold several
  frames. Receivers concatenate decoded chunks and read frames by their
  length headers.
- **Decoding.** Receivers accept padded or unpadded base64url. They may
  discard strings longer than 131,072 base64 bytes, and a string that is not
  valid base64url ends the session.
- **Other bytes.** Bytes outside `ESC _ U ... ESC \` strings are not
  protocol data. Apps ignore them on stdin. Terminals process them as
  ordinary output.

**Probe.** An app first asks the terminal's name: XTVERSION, then DA1
(`ESC [ > q ESC [ c`). It goes on only if the reply before DA1 is Ultty's,
`DCS > | ultty <version> ST`. The reply comes from whatever draws the
screen, so a multiplexer answers for itself and its pane never sees the APC
string, even when it cannot be recognised from the environment (a tmux pane
reached over ssh).

The app then writes `ClientHello` as an in-band string and then DA1. A
terminal that supports in-band answers with a frame, normally
`ServerHello`, before its DA1 reply. A terminal must send its reply to a
query that follows an in-band string only after its answer to that string.
Ultty applies this per read from the PTY, so it may also delay replies to
queries made just before the string; apps must wait for the name reply
before sending the hello. An app that sees the DA1 reply first, or nothing
within a timeout (`ulttui`: 1 s for both steps), uses ANSI.

- Apps keep their tty in raw mode, without echo, for the whole session.
  Otherwise the line discipline holds the terminal's strings and echoes
  them back.
- Apps must not wrap the probe for multiplexer passthrough. A terminal
  outside a multiplexer cannot know where a pane is.
- `ulttui` does not probe at all under tmux or screen when it can tell:
  when `TMUX` or `STY` is set, or `TERM` starts with `tmux` or `screen`. Both
  take APC strings as title or status text, which the name check above
  guards against when they cannot be told apart. It also skips the probe on `TERM=dumb` and on the Linux
  console, which prints unknown APC strings, and when `ULTTY_IN_BAND=0`.

**Sessions.** A terminal opens an in-band session only when a well-formed
`ClientHello` starts the data, so stray strings, such as a file being
displayed, cannot open one. Ultty allows one in-band session per tab, and
only on a tab with a single pane.

**Confirmation.** A hello alone does not show that a live app is reading
the terminal's answers: a displayed file or a remote host's output can hold
one. So the terminal answers `ClientHello` (`ServerHello`, `SessionReady`)
at once, then treats the session as unconfirmed until in-band data arrives
in a later read than the hello, after the answer was written. An app must
send its next message (any message: `ulttui` sends `Subscribe`) within 2 s
of `SessionReady`, and must not send anything after its hello until it has
read the answer. While a session is unconfirmed, Ultty does not draw its
scene, sends it no input, and holds every other reply; on confirmation the
held replies are sent. Data that arrives in the same read as the hello is
applied but does not confirm.

A session ends when any of these happen:

- the app sends `Disconnect`;
- the stream is malformed;
- it is not confirmed within 2 s of the hello answer;
- the app has been silent for 10 s;
- the terminal sees the app is gone: Ultty checks the PTY's foreground
  process group and the processes it held at hello.

The terminal never sends `Ping` in-band: if the app has died, the string
would reach the shell as typed input. Instead an idle app sends an
unsolicited `Pong` at least every 2 s.

**Trust.** Anything that can write to the terminal can send in-band data,
including a remote host. In-band sessions therefore cannot read the
clipboard: `ClipboardGet` is answered with `ClipboardContent` with no text.

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
| `CAP_INLINE_SURFACES` | `0x0100` |
| `CAP_SEMANTICS` | `0x0200` |
| `CAP_INPUT_CURSOR` | `0x0400` |

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
| `MAX_SEMANTICS_STRING_LEN` | 4 KiB per `Semantics` string |
| `MAX_SEMANTICS_TOTAL_BYTES` | 8 KiB across one `Semantics`' strings |
| Surface rows | `MIN_SURFACE_ROWS` 1 to `MAX_SURFACE_ROWS` 128 |
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
| `SetRegionMetadata` | `0x0014` | `CreateSurface` | `0x0015` |
| `CloseSurface` | `0x0016` | `SetContent` | `0x0020` |
| `SetStyle` | `0x0021` | `SetConstraints` | `0x0022` |
| `AppendContent` | `0x0023` | `StreamStart` | `0x0024` |
| `StreamEnd` | `0x0025` | `SetSemantics` | `0x0026` |
| `BeginFrame` | `0x0030` | `EndFrame` | `0x0031` |
| `Scroll` | `0x0040` | `Focus` | `0x0041` |
| `Blur` | `0x0042` | `Subscribe` | `0x0050` |
| `Unsubscribe` | `0x0051` | `ClipboardGet` | `0x0060` |
| `ClipboardSet` | `0x0061` | `Pong` | `0x0070` |
| `Disconnect` | `0x00FF` | | |

#### Terminal → App

| Type | Code | Type | Code |
|------|------|------|------|
| `ServerHello` | `0x0001` | `SessionReady` | `0x0002` |
| `LayoutComputed` | `0x0010` | `Input` | `0x0020` |
| `Resize` | `0x0022` | `FocusEvent` | `0x0023` |
| `ScrollEvent` | `0x0024` | `ClipboardContent` | `0x0025` |
| `SurfaceEvent` | `0x0026` | `Ping` | `0x0070` |
| `Error` | `0x00FF` | | |

Code `0x0026` is `SetSemantics` from the app and `SurfaceEvent` from the terminal.

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

Positions are absolute cells from the top left of the region's root: the viewport for regions under `ROOT`, the surface for regions in an inline surface (section 10.6). `LayoutComputed` lists the `ROOT` tree first, then each surface's tree in the order the surfaces were created.

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

**Text limits.** Terminals bound the text a session holds, since
`AppendContent` adds up even though each message is size-limited. Ultty's
limits:

- **Per region: 4 MiB, as a ring.** When `AppendContent` takes a region's
  text (`Text`, `RichText` spans, or `Markdown` source) past 4 MiB, the
  oldest text is dropped until about 3 MiB remain, cutting after a line
  break when one is near. Streaming apps such as logs and chat keep working
  and keep their newest output; an app that needs its whole history must
  keep it itself or split it across regions.
- **Per session: 64 MiB.** A `SetContent` or `AppendContent` that would take
  the text of all the session's regions past 64 MiB is refused: the region
  keeps its content and the app gets `Error` 400. Deleting regions or
  replacing their content frees room.

### 8.3 Rich Text Content

`RichText` is a `u32` span count followed by spans. Each span is a string followed by a full `Style`.

### 8.4 Widget Content

| Tag | Widget | Fields in encoding order |
|-----|--------|--------------------------|
| 0 | Input | value string, placeholder string, flags u8 (`multiline=1`, `password=2`) |
| 1 | Markdown (deprecated) | source string |
| 2 | Code (deprecated) | source string, language string, line_numbers u8 |
| 3 | Progress | value f32, label Option<string>, animated u8, indeterminate u8 |
| 4 | Spinner | label Option<string>, style u8 (`Braille=0`, `Dots=1`, `Line=2`, `Bounce=3`) |
| 5 | Image | width u32, height u32, scale u8, data_len u32, bytes |
| 6 | Button | label string, style u8 (`Default=0`, `Primary=1`, `Danger=2`, `Success=3`) |
| 7 | Input with caret (`CAP_INPUT_CURSOR`) | the fields of tag 0, then cursor u32 |

An input's caret is `InputWidget::cursor`: the number of grapheme clusters
before it in the text the input shows, which is `value`, or `placeholder`
while `value` is empty. An input with a caret is encoded as tag 7, one
without as tag 0, so a terminal that did not negotiate `CAP_INPUT_CURSOR`
(and rejects tag 7 as an invalid widget type) is never sent one: apps send
a caret only when the capability was granted. The terminal draws the caret
in the input and, for assistive technology, exposes it as the text
selection of the input's text, which is `value` alone. A password's caret is drawn but never exposed. A multiline input has no caret.

Image scale is `Fit=0`, `Fill=1`, `Stretch=2`, `None=3`.

`Markdown` and `Code` are deprecated ([ADR 0018](adr/0018-in-band-native-transport.md)).
Terminals still decode them, but draw the source verbatim, one line per row,
without parsing markdown or highlighting. Apps should send `RichText` and do
their own styling; `ulttui`'s `MarkdownView` already does. Tags 1 and 2 will
be removed in a later protocol version.

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
| `SetSemantics` | id u32, `Semantics` (section 10.5) | `CAP_SEMANTICS` |
| `CreateSurface` | id u32, rows u16, full `Constraints`, full `Style` (section 10.6) | `CAP_INLINE_SURFACES` |
| `CloseSurface` | id u32 | `CAP_INLINE_SURFACES` |

Metadata is opaque to the terminal; tools read it back (`ulttyctl regions`). By convention the key `id` holds the widget id of the region's widget, which `ulttui` sets for every region it creates for a widget with an id. `Focus` and `Blur` name the region with keyboard focus for those tools too; they do not change where key events go unless the focused region subscribed to `EVENT_KEY`.

`Scroll` is `id: u32`, `x: f32`, `y: f32`, `animate: u8`; `Focus` is `id: u32`; `Blur` is empty. None of these is capability-gated: `CAP_WIDGET_SCROLL` only advertises that the terminal handles scroll offsets and animation, and `CAP_LAYOUT_FLEXBOX` that it runs the flexbox layout described in section 5.

### 10.5 Semantics

`SetSemantics` says what a region is, for screen readers and other assistive technology ([ADR 0021](adr/0021-accessibility-semantics.md)). It replaces the region's earlier semantics as a whole, like `SetStyle`, and is batched by frames like any other region message. Without `CAP_SEMANTICS` it is `Error` 400; for an unknown region it is `Error` 404. `DeleteRegion` drops a region's semantics with it.

The payload after `id: u32`, in encoding order:

| Field | Encoding | Meaning |
|-------|----------|---------|
| `role` | `u8` | Role table below; `0` (`Auto`) derives the role from content |
| `label` | `Option<string>` | Accessible name |
| `description` | `Option<string>` | Longer help text |
| `live` | `u8` | `Off=0`, `Polite=1`, `Assertive=2` |
| `flags` | `u8` | `busy=1`, `hidden=2`, `read_only=4`, `disabled=8`, `atomic=16`, `modal=32`, `selected=64` |
| `toggled` | `u8` | `Unset=0`, `False=1`, `True=2`, `Mixed=3` |
| `expanded` | `u8` | `Unset=0`, `Collapsed=1`, `Expanded=2` |
| `value` | `Option<(f32 now, f32 min, f32 max)>` | Numeric value and range |
| `value_text` | `Option<string>` | Spoken instead of the number when present |
| `position` | `Option<(u32 index, u32 size)>` | 1-based position in a set, and the set's size |
| `level` | `u8` | Heading or tree level; `0` for none |
| `labelled_by` | `Option<u32>` | Region whose text names this one |
| `described_by` | `Option<u32>` | Region whose text describes this one |
| `controls` | `Option<u32>` | Region this one controls |

An unknown `role`, `live`, `toggled` or `expanded` value, a flag bit outside `0x7F`, a NaN or infinite `value` float, a `label`, `description` or `value_text` longer than 4 KiB (`MAX_SEMANTICS_STRING_LEN`), and more than 8 KiB across those three (`MAX_SEMANTICS_TOTAL_BYTES`) are decode errors (`Error` 400). Accessible names are short, and the limits bound what semantics can add to a session's memory.

Out-of-range numbers are accepted and clamped when the tree is exposed: a `position` index is clamped to `1..=size`, a `value` with `min > max` is treated as having no known range, and otherwise `now` is clamped into `[min, max]`. Relationship targets are not checked: apps create regions in any order inside a frame, and a target that does not exist is ignored when the tree is exposed.

`busy` marks work in progress; `hidden` leaves the region and its subtree out of the accessibility tree (decorative dividers, spacers); `atomic` asks for the whole region to be announced when part of it changes; `modal` marks a `Dialog` that blocks the rest of the app; `selected` marks the chosen item of a set (the active `Tab`, the selected `ListItem`, `TreeItem` or table row). A `Tab`, `ListItem` or `TreeItem` without it is exposed as selectable but not selected; other roles expose no selection state unless it is set.

Roles form a closed set. Each maps to an AccessKit role when the terminal exposes the tree:

| Value | Role | AccessKit role |
|-------|------|----------------|
| 0 | `Auto` | derived, see below |
| 1 | `Group` | `Group` |
| 2 | `Text` | `Label` (one line) or `Paragraph` |
| 3 | `Heading` | `Heading`, with `level` |
| 4 | `Log` | `Log` |
| 5 | `Status` | `Status` |
| 6 | `Alert` | `Alert` |
| 7 | `Dialog` | `Dialog`, with `modal` |
| 8 | `Input` | `TextInput`, `MultilineTextInput` or `PasswordInput` by the `Input` widget's flags |
| 9 | `Button` | `Button` |
| 10 | `Checkbox` | `CheckBox`, with `toggled` |
| 11 | `Switch` | `Switch`, with `toggled` |
| 12 | `Progress` | `ProgressIndicator`, with `value` |
| 13 | `List` | `List` |
| 14 | `ListItem` | `ListItem`, with `position` and `selected` |
| 15 | `TabList` | `TabList` |
| 16 | `Tab` | `Tab`, with `position` and `selected` |
| 17 | `Tree` | `Tree` |
| 18 | `TreeItem` | `TreeItem`, with `expanded`, `level` and `selected` |
| 19 | `Image` | `Image`; the label is the alt text |
| 20 | `Link` | `Link` |

Adding a role is additive; removing one is not.

**Defaults.** A region that never received `SetSemantics`, or whose role is `Auto`, takes the role its content implies. Whatever the role, the state in the third column fills the fields the app left empty:

| Content | Role | Derived state |
|---------|------|---------------|
| `None` | `Group` | |
| `Text`, `RichText`, `Markdown`, `Code` | `Text`, or `Log` once the region has received `AppendContent` | |
| `Input` | `Input` | `value_text` is the value (not for password inputs), `description` the placeholder |
| `Button` | `Button` | `label` is the button's label |
| `Progress` | `Progress` | `value` is the progress in `0..1`, or `busy` when indeterminate; `label` is the bar's label |
| `Spinner` | `Status` | `busy`; `label` is the spinner's label |
| `Image` | `Image` | unlabelled |

Fields the app did set always win, except that `busy` has no unset value: a spinner or an indeterminate progress bar is busy whatever the app sent. A region with no label takes its text as its name when the text is one line (a trailing newline is ignored), whatever its role; an `Input`'s text is its value, never its name. `live` defaults to `Off` and is never inferred: a wrongly live region floods speech.

`ulttyctl regions` reports each region's effective `role`, `label` and `live`; the JSON form leaves out `label` when there is none and `live` when it is `off`.

### 10.6 Inline Surfaces

An inline surface is a region tree anchored to rows of the main screen, `rows` high and the pane's width wide, that scrolls with the text and into scrollback, and is frozen into plain styled text when closed ([ADR 0020](adr/0020-inline-surfaces.md)). A session can hold surfaces and a full-scene `ROOT` tree at once.

| Message | Direction | Code | Payload in encoding order |
|---------|-----------|------|---------------------------|
| `CreateSurface` | app → terminal | `0x0015` | `id: u32`, `rows: u16`, full `Constraints`, full `Style` |
| `CloseSurface` | app → terminal | `0x0016` | `id: u32` |
| `SurfaceEvent` | terminal → app | `0x0026` | `id: u32`, `kind: u8` |

`id` is never `0` (the root) and `rows` is 1 to 128; anything else is a decode error (`Error` 400). `SurfaceEvent` kinds are `Placed=0`, `Closed=1`, `Evicted=2`, `Hidden=3` and `Shown=4`; an unknown kind is a decode error.

**Creating.** `CreateSurface` creates region `id` as a second root of the session's scene, laid out on its own at `cols × rows` cells, where `cols` is the viewport width. Its `width`, `height` and `margin` constraints (and the minimum and maximum sizes) are ignored; its other constraints and its style apply as for any region. Children attach with ordinary `CreateRegion { parent: id }`, and `SetConstraints`, `SetStyle`, `SetContent`, `AppendContent`, streams and metadata work on surface regions unchanged. A surface's regions, its root included, count toward the session's region count (`MAX_REGION_COUNT`, section 3) and text limits (section 8.2), and nesting depth is counted from the surface root. A session holds at most 16 live surfaces (`MAX_SURFACES`): one more is `Error` 400, as is an `id` already in use. The surface's height is fixed for its life. On a viewport resize every surface is laid out again at the new width.

**Closing.** `CloseSurface` deletes the surface root and its subtree and answers `SurfaceEvent { kind: Closed }`. `DeleteRegion` on a surface root does the same. `CloseSurface` on a region that is not a surface root is `Error` 400, and on an unknown region `Error` 404. A surface root cannot be moved with `ReparentRegion` (400); regions inside a surface can be moved to and from other trees.

Both app messages without `CAP_INLINE_SURFACES` are `Error` 400. Like other region messages they are batched by frames and applied at `EndFrame`, and a frame that creates or closes a surface gets a `LayoutComputed` reply.

**Anchoring.** The place of a surface in the text is set by an APC string the app writes to its stdout, on both transports: `ESC _ u <session_id> ; <surface_id> ; <rows> ESC \` (decimal ASCII, at most 64 bytes, at column 0). The terminal reserves `rows` rows there, and the app then prints exactly `rows` lines of fallback text, which is what a terminal that drops the string shows.

**Placing.** A surface is placed, and the app gets `SurfaceEvent { kind: Placed }`, when its anchor and its `CreateSurface` have both arrived, in either order. The anchor pairs only with a session of the tab whose PTY carried it, and only while that PTY is the tab's one pane; an anchor from a pane of a split tab is released at once. Then:

- If the anchor's `rows` and the `CreateSurface`'s disagree, the surface is dropped with `Error` 400 and its rows keep the fallback text.
- A `CreateSurface` whose anchor has not arrived 2 s later is dropped with `Error` 400. An anchor no surface claims within 2 s is released: its rows keep the fallback text and nothing more pairs with it.
- An anchor the terminal refused (on the alternate screen, no room in the scroll region, or 1024 anchors live) drops its surface with `Error` 400 if the surface is waiting for it; a surface created after the refusal waits out its 2 s. The terminal reports at most 64 refused anchors between two pairing passes; past that a refusal is not reported, and a surface waiting for that anchor waits out its 2 s instead of getting an immediate 400.
- A second anchor for a surface already placed is ignored and released, so another program in the tab cannot move or claim it.

**Drawing.** A placed surface is drawn over its rows, at the pane's left edge and the row its first line shows on, clipped to its own rows and to the pane's grid, with the grid text drawn under it. The surface root's background covers the fallback text; a root without `bg` is filled with the terminal's background. It moves with its rows as output scrolls them and as the viewport scrolls through scrollback. A surface with no row on the screen is not drawn; nor are surfaces while the alternate screen shows, while the tab is hidden, or in a tab with more than one pane. `Hidden` and `Shown` tell the app when a surface stops and starts being drawn for any of these reasons, once per change.

**Rasterising.** A placed surface's rows hold its scene as styled cells, so copy, search, `ulttyctl text` and `last-command` read what is drawn, in order with the text around it. Whenever its scene changed (a frame, a scroll, an animation step, a new viewport width), the next pass of the UI loop writes it into its rows, on the screen or in scrollback; a pass without a change writes nothing. The rows are written while the surface is hidden too, but not while the alternate screen shows: they are written when it goes. The cells are the region tree drawn as the GPU draws it and clipped the same way: backgrounds as cell background (a root without `bg` gives the default background), borders as box-drawing characters for their `BorderType` (`Ascii` gives `+`, `-` and `|`), text broken into lines where the renderer breaks it, `RichText` spans and region styles as foreground colour, bold, italic, underline and dim, widgets as their label or value, and images as blank cells. Borders take no room in the layout, so they are drawn last, over any content on their cells: a bordered region needs padding to keep its text clear of them. Only the visible cells of a region are visited, however large its layout. A row whose cells did not change is not written; writing one drops a selection that covers it, as streaming output does, and leaves other selections, scrolled back or not, alone. After a resize the rows are clipped or padded, not re-wrapped, and the surface is laid out and written again at the new width.

**Evicting and freezing.** When a placed surface's anchor row leaves scrollback or is erased (`ED 2`, `ED 3`, `RIS`, erasing a reserved row), the app gets `Evicted` and the surface is frozen. A surface the app closes is frozen with `Closed`, and when a session ends (disconnect, idle timeout, app gone) its surfaces are frozen without an event. Freezing writes the surface's last frame into its rows if a pass has not yet (a frame that changes a surface and closes it, or a session that ends right after a frame), then drops its region tree and takes the anchor tag off its rows, which keep that text as plain styled text: a frozen surface costs what its rows of text cost. An evicted surface's rows are gone or erased, so they keep only what earlier passes wrote. A surface never placed leaves the fallback text the app printed.

**Trust.** A live surface's rows are the app's to write, wherever they are: after they scroll into history the app can still change what they hold, and so what is copied or searched there, until the surface is closed, evicted or its session ends. Rows outside its reservation, and rows after it freezes, it cannot touch. A user who copies from a live card copies what the app last drew.

While `ROOT` has children the session is full-scene and its surfaces are hidden under it (they are not reported `Hidden`); with none, the grid is drawn and the surfaces over it.

A region in a surface gets no key events: `Subscribe` with `EVENT_KEY` on a surface root or a region under one is `Error` 400. Key and text input go to a session only while it is full-scene; an inline-only app reads keys from its tty.

Ultty offers `CAP_INLINE_SURFACES` to every session. `ulttyctl regions` lists each surface with its rows, its state (`unplaced` until its anchor came, `placed` while drawn, `hidden` while placed but not drawn) and, once placed, the line of its first row counted from the oldest line the terminal holds, then its region tree.

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
| 0.1.0-draft | 2026-11-10 | `CAP_INPUT_CURSOR` (`0x0400`) and the input widget's caret, widget tag 7, section 8.4 |
| 0.1.0-draft | 2026-11-08 | `CreateSurface` (`0x0015`), `CloseSurface` (`0x0016`), `SurfaceEvent` (terminal `0x0026`) and `CAP_INLINE_SURFACES` (`0x0100`), section 10.6 (ADR 0020) |
| 0.1.0-draft | 2026-11-08 | `SetSemantics` (`0x0026`) and `CAP_SEMANTICS` (`0x0200`), section 10.5 (ADR 0021) |
| 0.1.0-draft | 2026-09-22 | Regenerated wire-format sections from `codec.rs`; removed shared-memory/zero-copy claims |
| 0.1.0-draft | 2026-01-30 | Initial draft |

---

## 15. Open Questions

1. Which additional properties should transitions support?
2. Should app-defined widgets be added in a future version?
3. Should terminal tabs or splits be represented?
