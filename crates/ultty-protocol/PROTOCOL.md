# Ultty Native Protocol

Binary wire protocol for communication between TUI applications and the Ultty terminal emulator via Unix sockets.

## Connection

Apps connect to a Unix socket created by the terminal at a well-known path. The handshake is:

1. App sends `ClientHello` with protocol version
2. Terminal responds with `ServerHello` (version, viewport dimensions)
3. Terminal sends `SessionReady` (assigned session ID)

## Wire Format

Every message uses an 8-byte header followed by a variable-length payload:

```
Offset  Size  Field
0       2     Magic: 0x55 0x54 ("UT")
2       2     Type: u16 little-endian (message type code)
4       4     Length: u32 little-endian (payload size in bytes)
8       N     Payload (binary-encoded fields)
```

**Encoding rules:**
- Integers: little-endian
- Floats: IEEE 754 little-endian
- Strings: `[u32 length][UTF-8 bytes]`
- Enums: `[u8 discriminant][variant data]`
- Booleans: `[u8]` (0 = false, 1 = true)

**Security limits:** max payload 16 MB, strings 1 MB, region count 10k, span count 10k.

## Message Types

### App -> Terminal

| Code   | Message           | Description |
|--------|-------------------|-------------|
| 0x0001 | ClientHello       | Protocol version negotiation |
| 0x0010 | CreateRegion      | Create a new region in the scene tree |
| 0x0012 | DeleteRegion      | Remove a region and its children |
| 0x0013 | ReparentRegion    | Move a region to a new parent |
| 0x0020 | SetContent        | Set region content (text, widget, etc.) |
| 0x0021 | SetStyle          | Update region visual style |
| 0x0022 | SetConstraints    | Update region layout constraints |
| 0x0030 | BeginFrame        | Start an atomic batch of updates |
| 0x0031 | EndFrame          | Commit the batch (triggers layout) |
| 0x0040 | Scroll            | Programmatic scroll of a region |
| 0x0041 | Focus             | Set keyboard focus to a region |
| 0x0042 | Blur              | Remove keyboard focus |
| 0x0050 | Subscribe         | Start receiving events for a region |
| 0x0051 | Unsubscribe       | Stop receiving events |
| 0x00FF | Disconnect        | Graceful disconnect |

### Terminal -> App

| Code   | Message          | Description |
|--------|------------------|-------------|
| 0x0001 | ServerHello      | Version, viewport dimensions |
| 0x0002 | SessionReady     | Assigned session ID |
| 0x0010 | LayoutComputed   | Computed positions for all regions |
| 0x0020 | Input            | Key, mouse, or text input event |
| 0x0022 | Resize           | Terminal viewport resized |
| 0x0023 | Focus            | Region focus gained/lost |
| 0x0024 | Scroll           | User-initiated scroll event |
| 0x00FF | Error            | Error message |

## Scene Tree

Apps build a UI by creating a tree of **regions**. Each region has:

- **RegionId**: unique `u32` identifier (`0` = ROOT, the viewport)
- **Parent**: which region contains this one
- **Constraints**: layout properties (CSS Flexbox/Grid via Taffy)
- **Style**: visual properties (colors, borders, text formatting)
- **Content**: what to display (text, rich text, or a widget)

The terminal computes layout using Taffy and sends back `LayoutComputed` with pixel positions for each region.

### Atomic Updates

Wrap multiple mutations in `BeginFrame`/`EndFrame` to batch them into a single layout computation:

```
BeginFrame(frame_id=1)
  CreateRegion(id=1, parent=ROOT, ...)
  SetContent(id=1, content=Text("Hello"))
  CreateRegion(id=2, parent=ROOT, ...)
  SetContent(id=2, content=Text("World"))
EndFrame(frame_id=1)
```

## Content Types

### Text
Plain text with configurable wrapping, alignment, and overflow.

### RichText
Styled text with inline spans. Each span has text content plus optional: foreground/background color, bold, italic, underline, dim.

### Widgets

| Widget     | Fields | Description |
|------------|--------|-------------|
| **Input**      | value, placeholder, multiline, password | Text input field |
| **Markdown**   | source | Rendered markdown content |
| **Code**       | source, language, line_numbers | Syntax-highlighted code block |
| **Progress**   | value (0.0-1.0), label, animated, indeterminate | Progress bar |
| **Spinner**    | label, style (Braille/Dots/Line/Bounce) | Loading animation |
| **Image**      | data (RGBA bytes), width, height, scale (Fit/Fill/Stretch/None) | Inline image |

## Layout System

Layout uses CSS Flexbox and Grid properties:

- **Display**: Block, Flex, Grid, None
- **Sizing**: width, height, min/max, aspect-ratio
- **Dimensions**: Auto, Cells(f32), Percent(f32), Fr(f32), Vw, Vh
- **Flex**: direction, wrap, grow, shrink, basis
- **Grid**: template-columns/rows, column/row placement
- **Alignment**: justify-content, align-items, align-self
- **Spacing**: margin, padding, gap (row/column)
- **Overflow**: per-axis (Visible, Hidden, Scroll, Auto)

## Event Subscription

Apps subscribe to events on specific regions using bitmask flags:

| Flag | Value | Events |
|------|-------|--------|
| EVENT_MOUSE  | 0x01 | MouseDown, MouseUp, MouseMove |
| EVENT_KEY    | 0x02 | KeyDown, KeyUp |
| EVENT_FOCUS  | 0x04 | Focus gained/lost |
| EVENT_SCROLL | 0x08 | User scroll (wheel, touchpad) |

Input events include region-local coordinates (`x`, `y` relative to the region's content area).

## Styling

- **Colors**: Named (ANSI 0-17), RGB(r, g, b), Indexed (256-color palette)
- **Text**: bold, italic, underline, dim, opacity (0.0-1.0)
- **Borders**: type (Thin/Thick/Double/Rounded/Dashed/ASCII), per-edge control, radius

## Example Flow

```
App                              Terminal
 |--- ClientHello(v=1) --------->|
 |<-- ServerHello(v=1, 80x24) --|
 |<-- SessionReady(sid=1) ------|
 |                                |
 |--- BeginFrame(1) ------------>|
 |--- CreateRegion(1, ROOT) ---->|
 |--- SetConstraints(1, ...) --->|
 |--- SetContent(1, Text) ------>|
 |--- Subscribe(1, KEY|MOUSE) -->|
 |--- EndFrame(1) -------------->|
 |                                |
 |<-- LayoutComputed(frame=1) --|
 |                                |
 |<-- Input(region=1, KeyDown) -|
 |--- SetContent(1, newtext) --->|
 |<-- LayoutComputed ------------|
```

## Version

Current protocol version: **1**

Versions are negotiated during the ClientHello/ServerHello handshake. Unknown message types are rejected with an Error response.
