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
    #[default]
    Start = 0,
    End = 1,
    Center = 2,
    SpaceBetween = 3,
    SpaceAround = 4,
    SpaceEvenly = 5,
}

/// Cross axis alignment for items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum AlignItems {
    Start = 0,
    End = 1,
    Center = 2,
    Baseline = 3,
    #[default]
    Stretch = 4,
}

/// Cross axis alignment for self.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum AlignSelf {
    #[default]
    Auto = 0,
    Start = 1,
    End = 2,
    Center = 3,
    Baseline = 4,
    Stretch = 5,
}

/// Cross axis alignment for multiple lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum AlignContent {
    Start = 0,
    End = 1,
    Center = 2,
    SpaceBetween = 3,
    SpaceAround = 4,
    SpaceEvenly = 5,
    #[default]
    Stretch = 6,
}

/// Dimension value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dimension {
    /// Automatic sizing.
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

impl Default for Dimension {
    fn default() -> Self {
        Dimension::Auto
    }
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
    pub x: OverflowAxis,
    pub y: OverflowAxis,
}

/// Overflow on a single axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum OverflowAxis {
    #[default]
    Visible = 0,
    Hidden = 1,
    Scroll = 2,
    Auto = 3,
}

/// Grid line specification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GridLine {
    #[default]
    Auto,
    Line(i16),
    Span(u16),
}

/// Grid placement (start and end lines).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GridPlacement {
    pub start: GridLine,
    pub end: GridLine,
}

/// Track sizing function for grid.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackSizingFunction {
    Single(Dimension),
    MinMax(Dimension, Dimension),
    Repeat(u16, Vec<Dimension>),
}

/// Size with width and height.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size<T> {
    pub width: T,
    pub height: T,
}

/// Complete layout constraints for a region.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Constraints {
    // Display
    pub display: Display,

    // Flexbox
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub align_self: AlignSelf,
    pub align_content: AlignContent,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Dimension,

    // Size
    pub width: Dimension,
    pub height: Dimension,
    pub min_width: Dimension,
    pub min_height: Dimension,
    pub max_width: Dimension,
    pub max_height: Dimension,
    pub aspect_ratio: Option<f32>,

    // Spacing
    pub margin: Edges<Dimension>,
    pub padding: Edges<Dimension>,
    pub gap: Size<Dimension>,

    // Overflow
    pub overflow: Overflow,

    // Grid (only used when display = Grid)
    pub grid_template_columns: Vec<TrackSizingFunction>,
    pub grid_template_rows: Vec<TrackSizingFunction>,
    pub grid_column: GridPlacement,
    pub grid_row: GridPlacement,
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
    pub top: T,
    pub right: T,
    pub bottom: T,
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
