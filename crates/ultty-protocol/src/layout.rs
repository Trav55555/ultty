//! Layout constraint types.
//!
//! These types mirror Taffy's layout model and are serialized over the wire.

/// Display mode for a region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Display {
    /// Block-level, full width.
    #[default]
    Block = 0,
    /// Flexbox container.
    Flex = 1,
    /// CSS Grid container.
    Grid = 2,
    /// Not rendered.
    None = 3,
}

/// Flex direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum FlexDirection {
    /// Horizontal, left-to-right.
    Row = 0,
    /// Horizontal, right-to-left.
    RowReverse = 1,
    /// Vertical, top-to-bottom.
    #[default]
    Column = 2,
    /// Vertical, bottom-to-top.
    ColumnReverse = 3,
}

/// Flex wrap behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum FlexWrap {
    /// No wrapping.
    #[default]
    NoWrap = 0,
    /// Wrap to new lines.
    Wrap = 1,
    /// Wrap in reverse.
    WrapReverse = 2,
}

/// Main axis alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum JustifyContent {
    /// Pack items at the start.
    #[default]
    Start = 0,
    /// Pack items at the end.
    End = 1,
    /// Center items.
    Center = 2,
    /// Distribute items with equal space between them.
    SpaceBetween = 3,
    /// Distribute items with equal space around them.
    SpaceAround = 4,
    /// Distribute items with equal space between and around them.
    SpaceEvenly = 5,
}

/// Cross axis alignment for items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum AlignItems {
    /// Align items to the start of the cross axis.
    Start = 0,
    /// Align items to the end of the cross axis.
    End = 1,
    /// Center items on the cross axis.
    Center = 2,
    /// Align items along their text baseline.
    Baseline = 3,
    /// Stretch items to fill the cross axis.
    #[default]
    Stretch = 4,
}

/// Cross axis alignment for self.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum AlignSelf {
    /// Inherit from parent's `align_items`.
    #[default]
    Auto = 0,
    /// Align self to the start of the cross axis.
    Start = 1,
    /// Align self to the end of the cross axis.
    End = 2,
    /// Center self on the cross axis.
    Center = 3,
    /// Align self along the text baseline.
    Baseline = 4,
    /// Stretch self to fill the cross axis.
    Stretch = 5,
}

/// Cross axis alignment for multiple lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum AlignContent {
    /// Pack lines at the start of the cross axis.
    Start = 0,
    /// Pack lines at the end of the cross axis.
    End = 1,
    /// Center lines on the cross axis.
    Center = 2,
    /// Distribute lines with equal space between them.
    SpaceBetween = 3,
    /// Distribute lines with equal space around them.
    SpaceAround = 4,
    /// Distribute lines with equal space between and around them.
    SpaceEvenly = 5,
    /// Stretch lines to fill the cross axis.
    #[default]
    Stretch = 6,
}

/// Dimension value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Dimension {
    /// Automatic sizing.
    #[default]
    Auto,
    /// Absolute size in terminal cells.
    Cells(f32),
    /// Percentage of parent.
    Percent(f32),
    /// Flex fraction.
    Fr(f32),
    /// Viewport width percentage.
    Vw(f32),
    /// Viewport height percentage.
    Vh(f32),
    /// Minimum content size.
    MinContent,
    /// Maximum content size.
    MaxContent,
}

impl Dimension {
    /// Create a cells dimension.
    pub const fn cells(n: f32) -> Self {
        Dimension::Cells(n)
    }

    /// Create a percentage dimension.
    pub const fn pct(n: f32) -> Self {
        Dimension::Percent(n)
    }

    /// Create a flex fraction.
    pub const fn fr(n: f32) -> Self {
        Dimension::Fr(n)
    }
}

/// Overflow behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overflow {
    /// Horizontal overflow behavior.
    pub x: OverflowAxis,
    /// Vertical overflow behavior.
    pub y: OverflowAxis,
}

/// Overflow on a single axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum OverflowAxis {
    /// Content is not clipped and may render outside the region.
    #[default]
    Visible = 0,
    /// Content is clipped at the region boundary.
    Hidden = 1,
    /// Always show a scrollbar.
    Scroll = 2,
    /// Show a scrollbar only when content overflows.
    Auto = 3,
}

/// Grid line specification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GridLine {
    /// Automatic grid line placement.
    #[default]
    Auto,
    /// Explicit grid line number (1-indexed, negative counts from end).
    Line(i16),
    /// Span across the given number of tracks.
    Span(u16),
}

/// Grid placement (start and end lines).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GridPlacement {
    /// Start grid line.
    pub start: GridLine,
    /// End grid line.
    pub end: GridLine,
}

/// Track sizing function for grid.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackSizingFunction {
    /// A single fixed track size.
    Single(Dimension),
    /// A track with minimum and maximum size bounds.
    MinMax(Dimension, Dimension),
    /// Repeat a pattern of track sizes a given number of times.
    Repeat(u16, Vec<Dimension>),
}

/// Size with width and height.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size<T> {
    /// Width component.
    pub width: T,
    /// Height component.
    pub height: T,
}

/// Complete layout constraints for a region.
///
/// The default has no margins (`Cells(0)` on every edge), as in CSS. An
/// `Auto` margin takes free space in its container, so it has to be asked
/// for (to centre a region, say).
#[derive(Debug, Clone, PartialEq)]
pub struct Constraints {
    /// Display mode (block, flex, grid, or none).
    pub display: Display,

    /// Flex layout direction.
    pub flex_direction: FlexDirection,
    /// Flex wrapping behavior.
    pub flex_wrap: FlexWrap,
    /// Main axis alignment.
    pub justify_content: JustifyContent,
    /// Cross axis alignment for children.
    pub align_items: AlignItems,
    /// Cross axis alignment override for this item.
    pub align_self: AlignSelf,
    /// Cross axis alignment for wrapped lines.
    pub align_content: AlignContent,
    /// Flex grow factor.
    pub flex_grow: f32,
    /// Flex shrink factor.
    pub flex_shrink: f32,
    /// Initial main axis size before growing/shrinking.
    pub flex_basis: Dimension,

    /// Preferred width.
    pub width: Dimension,
    /// Preferred height.
    pub height: Dimension,
    /// Minimum width constraint.
    pub min_width: Dimension,
    /// Minimum height constraint.
    pub min_height: Dimension,
    /// Maximum width constraint.
    pub max_width: Dimension,
    /// Maximum height constraint.
    pub max_height: Dimension,
    /// Width-to-height aspect ratio.
    pub aspect_ratio: Option<f32>,

    /// Outer margin on each edge.
    pub margin: Edges<Dimension>,
    /// Inner padding on each edge.
    pub padding: Edges<Dimension>,
    /// Gap between children (column gap, row gap).
    pub gap: Size<Dimension>,

    /// Overflow behavior for content that exceeds bounds.
    pub overflow: Overflow,

    /// Grid column track definitions (only used when display is `Grid`).
    pub grid_template_columns: Vec<TrackSizingFunction>,
    /// Grid row track definitions (only used when display is `Grid`).
    pub grid_template_rows: Vec<TrackSizingFunction>,
    /// Grid column placement for this item.
    pub grid_column: GridPlacement,
    /// Grid row placement for this item.
    pub grid_row: GridPlacement,
}

impl Default for Constraints {
    fn default() -> Self {
        Self {
            display: Display::default(),
            flex_direction: FlexDirection::default(),
            flex_wrap: FlexWrap::default(),
            justify_content: JustifyContent::default(),
            align_items: AlignItems::default(),
            align_self: AlignSelf::default(),
            align_content: AlignContent::default(),
            flex_grow: 0.0,
            flex_shrink: 0.0,
            flex_basis: Dimension::default(),
            width: Dimension::default(),
            height: Dimension::default(),
            min_width: Dimension::default(),
            min_height: Dimension::default(),
            max_width: Dimension::default(),
            max_height: Dimension::default(),
            aspect_ratio: None,
            margin: Edges::all(Dimension::Cells(0.0)),
            padding: Edges::default(),
            gap: Size::default(),
            overflow: Overflow::default(),
            grid_template_columns: Vec::new(),
            grid_template_rows: Vec::new(),
            grid_column: GridPlacement::default(),
            grid_row: GridPlacement::default(),
        }
    }
}

impl Constraints {
    /// Create new constraints with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set display mode.
    pub fn display(mut self, display: Display) -> Self {
        self.display = display;
        self
    }

    /// Set flex direction.
    pub fn flex_direction(mut self, dir: FlexDirection) -> Self {
        self.flex_direction = dir;
        self
    }

    /// Set flex grow.
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.flex_grow = grow;
        self
    }

    /// Set width.
    pub fn width(mut self, width: Dimension) -> Self {
        self.width = width;
        self
    }

    /// Set height.
    pub fn height(mut self, height: Dimension) -> Self {
        self.height = height;
        self
    }

    /// Set padding on all sides.
    pub fn padding_all(mut self, padding: Dimension) -> Self {
        self.padding = Edges::all(padding);
        self
    }

    /// Set overflow behavior.
    pub fn overflow(mut self, overflow: Overflow) -> Self {
        self.overflow = overflow;
        self
    }

    /// Set overflow to scroll on Y axis.
    pub fn overflow_y_scroll(mut self) -> Self {
        self.overflow.y = OverflowAxis::Scroll;
        self
    }
}

/// Edge values (top, right, bottom, left).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges<T> {
    /// Top edge value.
    pub top: T,
    /// Right edge value.
    pub right: T,
    /// Bottom edge value.
    pub bottom: T,
    /// Left edge value.
    pub left: T,
}

impl<T: Clone> Edges<T> {
    /// Same value on all edges.
    pub fn all(value: T) -> Self {
        Self {
            top: value.clone(),
            right: value.clone(),
            bottom: value.clone(),
            left: value,
        }
    }

    /// Symmetric vertical and horizontal.
    pub fn symmetric(vertical: T, horizontal: T) -> Self {
        Self {
            top: vertical.clone(),
            bottom: vertical,
            left: horizontal.clone(),
            right: horizontal,
        }
    }
}

impl<T> Edges<T> {
    /// Create with individual values.
    pub fn new(top: T, right: T, bottom: T, left: T) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }
}
