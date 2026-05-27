# Ultimate Terminal Protocol Specification

**Version**: 0.1.0-draft  
**Status**: Draft  
**Last Updated**: 2026-01-30

---

## 1. Overview

The Ultimate Terminal Protocol (UTP) is a binary protocol for communication between TUI applications and the Ultimate Terminal emulator. It enables:

- **Declarative layout**: Apps describe constraints, terminal computes positions
- **Retained mode rendering**: Terminal owns the scene graph, apps send diffs
- **Atomic updates**: Frames are buffered and swapped atomically
- **Native widgets**: Terminal renders complex widgets (scroll, input, markdown)

### 1.1 Design Goals

| Goal | Rationale |
|------|-----------|
| **Sub-millisecond layout** | Resize must be instant |
| **Minimal wire overhead** | Binary, not text |
| **Zero-copy where possible** | Shared memory for large content |
| **Backward compatible** | Version negotiation, feature flags |
| **Debuggable** | Text mode for development |

### 1.2 Transport

| Mode | Transport | Use Case |
|------|-----------|----------|
| **Native** | Unix domain socket | Primary, lowest latency |
| **Shared Memory** | `memfd` + socket signaling | Large content (>4KB) |
| **Embedded** | stdin/stdout framing | Fallback, sandboxed environments |

---

## 2. Connection Lifecycle

```
┌─────────┐                              ┌──────────┐
│   App   │                              │ Terminal │
└────┬────┘                              └────┬─────┘
     │                                        │
     │  ──── Connect (Unix Socket) ────────►  │
     │                                        │
     │  ──── ClientHello ──────────────────►  │
     │       { version, requested_features }  │
     │                                        │
     │  ◄──── ServerHello ─────────────────  │
     │        { version, capabilities }       │
     │                                        │
     │  ◄──── SessionReady ────────────────  │
     │        { session_id, root_region }     │
     │                                        │
     │  ════ Bidirectional Messages ════════  │
     │                                        │
     │  ──── Disconnect ───────────────────►  │
     │                                        │
```

### 2.1 Capability Negotiation

```rust
enum Capability : u16 {
    LAYOUT_FLEXBOX      = 0x0001,  // Flexbox layout (Taffy)
    LAYOUT_GRID         = 0x0002,  // CSS Grid layout
    WIDGET_SCROLL       = 0x0010,  // Native scroll views
    WIDGET_INPUT        = 0x0020,  // Native text input
    WIDGET_MARKDOWN     = 0x0040,  // Markdown rendering
    WIDGET_SYNTAX       = 0x0080,  // Syntax highlighting
    RENDER_TRUECOLOR    = 0x0100,  // 24-bit color
    RENDER_UNICODE      = 0x0200,  // Full Unicode + emoji
    RENDER_SIXEL        = 0x0400,  // Sixel graphics
    SHARED_MEMORY       = 0x1000,  // Shared memory transport
    ANIMATIONS          = 0x2000,  // Terminal-side animations
}
```

---

## 3. Message Format

### 3.1 Frame Structure

All messages are framed:

```
┌─────────────────────────────────────────────────────────────┐
│  0       1       2       3       4       5       6     ... │
├─────────────────────────────────────────────────────────────┤
│  Magic (2)  │  Type (2)     │  Length (4) │  Payload (N)   │
│   0x55 0x54 │  Little-end   │           │                  │
└─────────────────────────────────────────────────────────────┘

Magic:   0x55 0x54 ("UT")
Type:    u16, message type
Length:  u32, payload length (not including header)
Payload: Variable, type-specific
```

### 3.2 Message Types

#### App → Terminal

| Type | Code | Description |
|------|------|-------------|
| `ClientHello` | 0x0001 | Version and feature negotiation |
| `CreateRegion` | 0x0010 | Create new region in scene graph |
| `UpdateRegion` | 0x0011 | Update existing region |
| `DeleteRegion` | 0x0012 | Remove region from scene graph |
| `SetContent` | 0x0020 | Set region content (text, widget) |
| `SetStyle` | 0x0021 | Update region styling |
| `SetConstraints` | 0x0022 | Update layout constraints |
| `BeginFrame` | 0x0030 | Start atomic update batch |
| `EndFrame` | 0x0031 | Commit atomic update batch |
| `Scroll` | 0x0040 | Programmatic scroll |
| `Focus` | 0x0041 | Set keyboard focus |
| `Subscribe` | 0x0050 | Subscribe to events |
| `Unsubscribe` | 0x0051 | Unsubscribe from events |

#### Terminal → App

| Type | Code | Description |
|------|------|-------------|
| `ServerHello` | 0x0001 | Version and capabilities |
| `SessionReady` | 0x0002 | Session established |
| `LayoutComputed` | 0x0010 | Layout results after resize/change |
| `RegionEvent` | 0x0020 | Event in a region (click, key, etc.) |
| `InputEvent` | 0x0021 | Keyboard/mouse input |
| `ResizeEvent` | 0x0022 | Terminal resized |
| `FocusEvent` | 0x0023 | Focus changed |
| `ScrollEvent` | 0x0024 | User scrolled |
| `Error` | 0x00FF | Error response |

---

## 4. Region Model

### 4.1 Region Structure

A **Region** is the fundamental unit of the scene graph:

```rust
struct Region {
    // Identity
    id: RegionId,           // u32, app-assigned, unique per session
    parent: Option<RegionId>,
    children: Vec<RegionId>,
    
    // Layout (input to Taffy)
    constraints: LayoutConstraints,
    
    // Layout (output from Taffy)
    computed: ComputedLayout,  // Read-only, set by terminal
    
    // Content
    content: Content,
    
    // Presentation
    style: Style,
    
    // Behavior
    flags: RegionFlags,
}

// Region IDs
type RegionId = u32;
const ROOT_REGION: RegionId = 0;  // Always exists, represents terminal viewport
```

### 4.2 Region Hierarchy

```
ROOT (id=0, terminal viewport)
├── HEADER (id=1)
│   └── TITLE_TEXT (id=2)
├── MAIN (id=3)
│   ├── SIDEBAR (id=4)
│   │   └── FILE_TREE (id=5)
│   └── CONTENT (id=6)
│       ├── MESSAGES (id=7, scrollable)
│       │   ├── MSG_1 (id=8)
│       │   ├── MSG_2 (id=9)
│       │   └── MSG_3 (id=10)
│       └── INPUT (id=11)
└── FOOTER (id=12)
```

---

## 5. Layout Constraints

### 5.1 Constraint Structure

```rust
struct LayoutConstraints {
    // Display mode
    display: Display,
    position: Position,
    
    // Flexbox (when display = Flex)
    flex_direction: FlexDirection,
    flex_wrap: FlexWrap,
    align_items: AlignItems,
    align_self: AlignSelf,
    align_content: AlignContent,
    justify_content: JustifyContent,
    flex_grow: f32,
    flex_shrink: f32,
    flex_basis: Dimension,
    
    // Grid (when display = Grid)
    grid_template_rows: Vec<TrackSizingFunction>,
    grid_template_columns: Vec<TrackSizingFunction>,
    grid_row: GridPlacement,
    grid_column: GridPlacement,
    gap: Size<Dimension>,
    
    // Size constraints
    size: Size<Dimension>,
    min_size: Size<Dimension>,
    max_size: Size<Dimension>,
    aspect_ratio: Option<f32>,
    
    // Spacing
    margin: Edges<Dimension>,
    padding: Edges<Dimension>,
    
    // Overflow
    overflow: Overflow,
}
```

### 5.2 Dimension Type

```rust
enum Dimension {
    // Automatic sizing (content-based)
    Auto,
    
    // Absolute units (in terminal cells)
    Cells(f32),           // e.g., 3.0 = 3 character cells
    
    // Relative units
    Percent(f32),         // e.g., 50.0 = 50% of parent
    
    // Flex units (for grid tracks)
    Fr(f32),              // e.g., 1.0 = 1 fraction of remaining space
    
    // Viewport units
    Vw(f32),              // % of terminal width
    Vh(f32),              // % of terminal height
    Vmin(f32),            // % of smaller dimension
    Vmax(f32),            // % of larger dimension
    
    // Content-based
    MinContent,           // Minimum content size
    MaxContent,           // Maximum content size
    FitContent(f32),      // min(max-content, max(min-content, <length>))
}

// Shorthand
impl Dimension {
    fn cells(n: u16) -> Self { Dimension::Cells(n as f32) }
    fn pct(n: f32) -> Self { Dimension::Percent(n) }
    fn fr(n: f32) -> Self { Dimension::Fr(n) }
}
```

### 5.3 Display Modes

```rust
enum Display {
    // Normal flow
    Block,              // Full width, stacked vertically
    Inline,             // Inline with text flow
    
    // Flexbox
    Flex,               // Flexbox container
    
    // Grid
    Grid,               // CSS Grid container
    
    // Special
    None,               // Not rendered, no space
    Contents,           // Children promoted to parent
}
```

### 5.4 Flexbox Properties

```rust
enum FlexDirection {
    Row,                // Horizontal, left-to-right
    RowReverse,         // Horizontal, right-to-left
    Column,             // Vertical, top-to-bottom (default for TUIs)
    ColumnReverse,      // Vertical, bottom-to-top
}

enum FlexWrap {
    NoWrap,             // Single line (default)
    Wrap,               // Wrap to multiple lines
    WrapReverse,        // Wrap in reverse direction
}

enum JustifyContent {
    Start,              // Pack at start
    End,                // Pack at end
    Center,             // Center items
    SpaceBetween,       // Even spacing, no edge space
    SpaceAround,        // Even spacing, half edge space
    SpaceEvenly,        // Even spacing including edges
}

enum AlignItems {
    Start,              // Align to cross-start
    End,                // Align to cross-end
    Center,             // Center on cross axis
    Baseline,           // Align baselines
    Stretch,            // Stretch to fill (default)
}

enum AlignSelf {
    Auto,               // Inherit from parent align-items
    Start,
    End,
    Center,
    Baseline,
    Stretch,
}

enum AlignContent {
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
    Stretch,            // Default
}
```

### 5.5 Grid Properties

```rust
struct GridPlacement {
    start: GridLine,
    end: GridLine,
}

enum GridLine {
    Auto,                           // Automatic placement
    Line(i16),                      // Specific line number (1-indexed, negative from end)
    Span(u16),                      // Span N tracks
    Name(String),                   // Named grid line
}

enum TrackSizingFunction {
    Single(Dimension),              // Fixed or flexible size
    MinMax(Dimension, Dimension),   // minmax(min, max)
    FitContent(Dimension),          // fit-content(limit)
    Repeat(RepeatCount, Vec<TrackSizingFunction>), // repeat()
}

enum RepeatCount {
    Count(u16),                     // Repeat exactly N times
    AutoFill,                       // As many as fit
    AutoFit,                        // As many as fit, collapse empty
}
```

### 5.6 Spacing (Edges)

```rust
struct Edges<T> {
    top: T,
    right: T,
    bottom: T,
    left: T,
}

impl<T: Clone> Edges<T> {
    fn all(value: T) -> Self { ... }
    fn symmetric(vertical: T, horizontal: T) -> Self { ... }
    fn new(top: T, right: T, bottom: T, left: T) -> Self { ... }
}

// Shorthand for common patterns
padding: Edges::all(Dimension::cells(1))           // padding: 1
padding: Edges::symmetric(cells(1), cells(2))      // padding: 1 2
margin: Edges::new(cells(0), auto(), cells(0), auto())  // margin: 0 auto (centering)
```

### 5.7 Overflow

```rust
struct Overflow {
    x: OverflowAxis,
    y: OverflowAxis,
}

enum OverflowAxis {
    Visible,            // Content can overflow bounds (default)
    Hidden,             // Clip content at bounds
    Scroll,             // Scrollable (terminal manages scroll)
    Auto,               // Scroll if needed
}
```

---

## 6. Computed Layout

After Taffy processes constraints, the terminal computes:

```rust
struct ComputedLayout {
    // Position relative to parent's content area
    x: f32,             // In cells
    y: f32,             // In cells
    
    // Size
    width: f32,         // In cells
    height: f32,        // In cells
    
    // Content area (after padding)
    content_x: f32,
    content_y: f32,
    content_width: f32,
    content_height: f32,
    
    // Scroll state (if overflow: scroll)
    scroll_x: f32,
    scroll_y: f32,
    scroll_width: f32,   // Total scrollable width
    scroll_height: f32,  // Total scrollable height
    
    // Visibility
    visible: bool,       // False if display: none or clipped by ancestor
}
```

---

## 7. Wire Format

### 7.1 CreateRegion Message

```
┌─────────────────────────────────────────────────────────────┐
│  CreateRegion (0x0010)                                      │
├─────────────────────────────────────────────────────────────┤
│  region_id      : u32          │  App-assigned ID           │
│  parent_id      : u32          │  Parent region (0 = root)  │
│  insert_index   : i32          │  Position in parent (-1 = append) │
│  constraints    : Constraints  │  Layout constraints        │
│  style          : Style        │  Visual style              │
│  flags          : u32          │  Region flags              │
└─────────────────────────────────────────────────────────────┘
```

### 7.2 Constraints Encoding

```
┌─────────────────────────────────────────────────────────────┐
│  Constraints                                                │
├─────────────────────────────────────────────────────────────┤
│  flags          : u32          │  Which fields are set      │
│  display        : u8           │  Display mode              │
│  position       : u8           │  Position mode             │
│  flex_direction : u8           │  Flex direction            │
│  flex_wrap      : u8           │  Flex wrap                 │
│  justify_content: u8           │  Justify content           │
│  align_items    : u8           │  Align items               │
│  align_content  : u8           │  Align content             │
│  align_self     : u8           │  Align self                │
│  flex_grow      : f32          │  Flex grow factor          │
│  flex_shrink    : f32          │  Flex shrink factor        │
│  flex_basis     : Dimension    │  Flex basis                │
│  width          : Dimension    │  Width constraint          │
│  height         : Dimension    │  Height constraint         │
│  min_width      : Dimension    │  Minimum width             │
│  min_height     : Dimension    │  Minimum height            │
│  max_width      : Dimension    │  Maximum width             │
│  max_height     : Dimension    │  Maximum height            │
│  aspect_ratio   : f32          │  Aspect ratio (0 = none)   │
│  margin         : Edges        │  Margin                    │
│  padding        : Edges        │  Padding                   │
│  gap            : Size         │  Gap (row_gap, column_gap) │
│  overflow_x     : u8           │  X overflow                │
│  overflow_y     : u8           │  Y overflow                │
│  ... grid fields if display=Grid ...                        │
└─────────────────────────────────────────────────────────────┘
```

### 7.3 Dimension Encoding

```
┌─────────────────────────────────────────────────────────────┐
│  Dimension (4 bytes total)                                  │
├─────────────────────────────────────────────────────────────┤
│  type  : u8     │  Value type                               │
│  _pad  : u8     │  Padding                                  │
│  value : f16    │  Value (interpretation depends on type)   │
└─────────────────────────────────────────────────────────────┘

Type values:
  0x00 = Auto
  0x01 = Cells(value)
  0x02 = Percent(value)
  0x03 = Fr(value)
  0x04 = Vw(value)
  0x05 = Vh(value)
  0x06 = Vmin(value)
  0x07 = Vmax(value)
  0x08 = MinContent
  0x09 = MaxContent
  0x0A = FitContent(value)
```

### 7.4 SetConstraints Message (Partial Update)

```
┌─────────────────────────────────────────────────────────────┐
│  SetConstraints (0x0022)                                    │
├─────────────────────────────────────────────────────────────┤
│  region_id      : u32          │  Target region             │
│  field_mask     : u64          │  Which fields to update    │
│  ... only fields indicated by mask ...                      │
└─────────────────────────────────────────────────────────────┘

Field mask bits:
  0x0001 = display
  0x0002 = flex_direction
  0x0004 = flex_wrap
  0x0008 = justify_content
  0x0010 = align_items
  0x0020 = flex_grow
  0x0040 = flex_shrink
  0x0080 = flex_basis
  0x0100 = width
  0x0200 = height
  0x0400 = min_width
  0x0800 = min_height
  ... etc
```

### 7.5 LayoutComputed Event

Terminal sends this after computing layout:

```
┌─────────────────────────────────────────────────────────────┐
│  LayoutComputed (0x0010)                                    │
├─────────────────────────────────────────────────────────────┤
│  trigger        : u8           │  What triggered recompute  │
│  region_count   : u32          │  Number of regions updated │
│  regions        : [RegionLayout] │  Updated region layouts  │
└─────────────────────────────────────────────────────────────┘

struct RegionLayout {
    region_id: u32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    content_x: f32,
    content_y: f32,
    content_width: f32,
    content_height: f32,
    visible: bool,
}

Trigger values:
  0x01 = Resize (terminal resized)
  0x02 = ConstraintChange (app changed constraints)
  0x03 = ContentChange (content size changed)
  0x04 = TreeChange (regions added/removed)
```

---

## 8. Content Types

### 8.1 Content Enum

```rust
enum Content {
    // Empty region (container only)
    None,
    
    // Plain text
    Text(TextContent),
    
    // Styled text with inline formatting
    RichText(RichTextContent),
    
    // Native widgets
    Widget(WidgetContent),
}
```

### 8.2 Text Content

```rust
struct TextContent {
    text: String,
    wrap: TextWrap,
    align: TextAlign,
    overflow: TextOverflow,
}

enum TextWrap {
    None,               // No wrapping, clip or scroll
    Char,               // Wrap at character boundary
    Word,               // Wrap at word boundary (default)
}

enum TextAlign {
    Left,
    Center,
    Right,
}

enum TextOverflow {
    Clip,               // Hard clip at boundary
    Ellipsis,           // Add "..." at end
    Fade,               // Gradient fade out
}
```

### 8.3 Rich Text Content

```rust
struct RichTextContent {
    spans: Vec<Span>,
}

struct Span {
    text: String,
    style: SpanStyle,
}

struct SpanStyle {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    dim: bool,
}
```

### 8.4 Widget Content

```rust
enum WidgetContent {
    // Text input field
    Input(InputWidget),
    
    // Markdown rendering
    Markdown(MarkdownWidget),
    
    // Syntax-highlighted code
    Code(CodeWidget),
    
    // Progress indicator
    Progress(ProgressWidget),
    
    // Spinner/loading
    Spinner(SpinnerWidget),
    
    // Table
    Table(TableWidget),
    
    // Tree view
    Tree(TreeWidget),
    
    // Custom (app provides render callback)
    Custom(CustomWidget),
}
```

---

## 9. Style

```rust
struct Style {
    // Colors
    fg: Option<Color>,
    bg: Option<Color>,
    
    // Border
    border: BorderStyle,
    border_color: Option<Color>,
    
    // Text
    bold: bool,
    italic: bool,
    underline: bool,
    
    // Effects
    opacity: f32,       // 0.0 - 1.0
    
    // Focus
    focus_style: Option<Box<Style>>,  // Style when focused
    hover_style: Option<Box<Style>>,  // Style when hovered
}

enum Color {
    Named(NamedColor),              // ANSI color (adapts to theme)
    Rgb(u8, u8, u8),               // True color
    Indexed(u8),                    // 256-color palette
}

enum NamedColor {
    Black, Red, Green, Yellow, Blue, Magenta, Cyan, White,
    BrightBlack, BrightRed, BrightGreen, BrightYellow,
    BrightBlue, BrightMagenta, BrightCyan, BrightWhite,
    Foreground,                     // Terminal's default fg
    Background,                     // Terminal's default bg
}

struct BorderStyle {
    style: BorderType,
    edges: BorderEdges,
    radius: u8,                     // Corner radius (0-2 cells)
}

enum BorderType {
    None,
    Thin,           // ─ │ ┌ ┐ └ ┘
    Thick,          // ━ ┃ ┏ ┓ ┗ ┛
    Double,         // ═ ║ ╔ ╗ ╚ ╝
    Rounded,        // ─ │ ╭ ╮ ╰ ╯
    Dashed,         // ╌ ╎
    Ascii,          // - | + +
}

struct BorderEdges {
    top: bool,
    right: bool,
    bottom: bool,
    left: bool,
}
```

---

## 10. Events

### 10.1 Event Subscription

Apps subscribe to events per-region:

```rust
struct Subscribe {
    region_id: RegionId,
    events: EventMask,
}

bitflags EventMask : u32 {
    MOUSE_CLICK     = 0x0001,
    MOUSE_MOVE      = 0x0002,
    MOUSE_SCROLL    = 0x0004,
    KEY_PRESS       = 0x0010,
    KEY_RELEASE     = 0x0020,
    FOCUS_IN        = 0x0100,
    FOCUS_OUT       = 0x0200,
    RESIZE          = 0x0400,
    SCROLL          = 0x0800,
    LAYOUT          = 0x1000,
}
```

### 10.2 Input Events

```rust
struct InputEvent {
    region_id: RegionId,        // Region that received input
    event_type: InputType,
    // Position relative to region's content area
    x: f32,
    y: f32,
}

enum InputType {
    MouseDown { button: MouseButton },
    MouseUp { button: MouseButton },
    MouseMove,
    MouseScroll { delta_x: f32, delta_y: f32 },
    KeyDown { key: Key, modifiers: Modifiers },
    KeyUp { key: Key, modifiers: Modifiers },
    TextInput { text: String },  // For IME composition
}

enum MouseButton {
    Left, Middle, Right,
    Button4, Button5,
}

bitflags Modifiers : u8 {
    SHIFT = 0x01,
    CTRL  = 0x02,
    ALT   = 0x04,
    META  = 0x08,  // Cmd on macOS, Win on Windows
}
```

### 10.3 Resize Event

```rust
struct ResizeEvent {
    width: u16,         // New terminal width in cells
    height: u16,        // New terminal height in cells
    // Layout will be recomputed automatically
    // LayoutComputed event follows with new positions
}
```

---

## 11. Atomic Updates

### 11.1 Frame Batching

To prevent tearing, batch updates:

```
App                                          Terminal
 │                                               │
 │  ─────── BeginFrame ────────────────────────► │
 │                                               │ (start buffering)
 │  ─────── CreateRegion(id=5) ────────────────► │
 │  ─────── SetContent(id=5, text="...") ──────► │
 │  ─────── SetConstraints(id=3, height=10) ───► │
 │  ─────── DeleteRegion(id=7) ────────────────► │
 │                                               │
 │  ─────── EndFrame ──────────────────────────► │
 │                                               │ (compute layout)
 │                                               │ (render to buffer)
 │                                               │ (atomic swap)
 │  ◄────── LayoutComputed ─────────────────────│
 │                                               │
```

### 11.2 Frame Timing

```rust
struct BeginFrame {
    frame_id: u32,      // App-assigned, echoed in LayoutComputed
    deadline: Option<u64>,  // Timestamp (µs) by which frame must complete
}

struct EndFrame {
    frame_id: u32,
}
```

---

## 12. Examples

### 12.1 Simple Layout

Creating a header/content/footer layout:

```rust
// Create regions
send(CreateRegion {
    region_id: 1,
    parent_id: ROOT_REGION,
    constraints: Constraints {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        size: Size { width: Percent(100.0), height: Percent(100.0) },
        ..default()
    },
    ..default()
});

// Header (fixed height)
send(CreateRegion {
    region_id: 2,
    parent_id: 1,
    constraints: Constraints {
        height: Cells(3),
        flex_shrink: 0.0,  // Don't shrink
        ..default()
    },
    ..default()
});

// Content (fills remaining space)
send(CreateRegion {
    region_id: 3,
    parent_id: 1,
    constraints: Constraints {
        flex_grow: 1.0,
        overflow_y: OverflowAxis::Scroll,
        ..default()
    },
    ..default()
});

// Footer (fixed height)
send(CreateRegion {
    region_id: 4,
    parent_id: 1,
    constraints: Constraints {
        height: Cells(1),
        flex_shrink: 0.0,
        ..default()
    },
    ..default()
});
```

### 12.2 Responsive Sidebar

```rust
// Main container
send(CreateRegion {
    region_id: 10,
    parent_id: ROOT_REGION,
    constraints: Constraints {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        size: Size { width: Percent(100.0), height: Percent(100.0) },
        ..default()
    },
    ..default()
});

// Sidebar
send(CreateRegion {
    region_id: 11,
    parent_id: 10,
    constraints: Constraints {
        width: Percent(25.0),
        min_width: Cells(20),
        max_width: Cells(60),
        ..default()
    },
    style: Style {
        border: BorderStyle {
            style: BorderType::Thin,
            edges: BorderEdges { right: true, ..default() },
            ..default()
        },
        ..default()
    },
    ..default()
});

// Content
send(CreateRegion {
    region_id: 12,
    parent_id: 10,
    constraints: Constraints {
        flex_grow: 1.0,
        ..default()
    },
    ..default()
});
```

### 12.3 Grid Dashboard

```rust
send(CreateRegion {
    region_id: 20,
    parent_id: ROOT_REGION,
    constraints: Constraints {
        display: Display::Grid,
        grid_template_columns: vec![Fr(1.0), Fr(2.0), Fr(1.0)],
        grid_template_rows: vec![Cells(3.0), Fr(1.0), Cells(1.0)],
        gap: Size { width: Cells(1.0), height: Cells(1.0) },
        ..default()
    },
    ..default()
});

// Header spans all columns
send(CreateRegion {
    region_id: 21,
    parent_id: 20,
    constraints: Constraints {
        grid_column: GridPlacement { start: Line(1), end: Line(4) },
        grid_row: GridPlacement { start: Line(1), end: Line(2) },
        ..default()
    },
    ..default()
});

// CPU panel
send(CreateRegion {
    region_id: 22,
    parent_id: 20,
    constraints: Constraints {
        grid_column: GridPlacement { start: Line(1), end: Line(2) },
        grid_row: GridPlacement { start: Line(2), end: Line(3) },
        ..default()
    },
    ..default()
});
```

---

## 13. Appendix: Comparison with ANSI

| Operation | ANSI Approach | UTP Approach |
|-----------|---------------|--------------|
| Set cursor position | `\x1b[{row};{col}H` | Implicit from layout |
| Draw box | 100+ bytes of line-drawing chars | `CreateRegion` with border style |
| Resize handling | App queries size, recomputes, redraws | Terminal recomputes, notifies app |
| Scroll | `\x1b[{n}S` (unreliable) | `overflow: scroll` on region |
| Atomic update | `\x1b[?2026h` (DEC private, spotty support) | `BeginFrame`/`EndFrame` |
| Color | `\x1b[38;2;{r};{g};{b}m` (12+ bytes) | `Color::Rgb(r,g,b)` (3 bytes) |

---

## 14. Version History

| Version | Date | Changes |
|---------|------|---------|
| 0.1.0-draft | 2026-01-30 | Initial draft |

---

## 15. Open Questions

1. **Shared memory protocol**: How to handle large content (>4KB)?
2. **Animation primitives**: Should terminal support transitions?
3. **Accessibility**: How to expose semantic structure to screen readers?
4. **Custom widgets**: Callback mechanism for app-rendered content?
5. **Multi-window**: Support for terminal tabs/splits?
