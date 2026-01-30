//! Content types for region contents.

use crate::style::Style;

/// Region content.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Content {
    /// Empty region (container only).
    #[default]
    None,
    /// Plain text.
    Text(TextContent),
    /// Styled text with inline formatting.
    RichText(RichText),
    /// Native widget.
    Widget(WidgetContent),
}

/// Plain text content.
#[derive(Debug, Clone, PartialEq)]
pub struct TextContent {
    /// The text.
    pub text: String,
    /// Wrap mode.
    pub wrap: TextWrap,
    /// Alignment.
    pub align: TextAlign,
    /// Overflow handling.
    pub overflow: TextOverflow,
}

impl TextContent {
    /// Create new text content.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            wrap: TextWrap::Word,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }
    }

    /// Set wrap mode.
    pub fn wrap(mut self, wrap: TextWrap) -> Self {
        self.wrap = wrap;
        self
    }

    /// Set alignment.
    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }
}

/// Text wrap mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum TextWrap {
    /// No wrapping.
    None = 0,
    /// Wrap at character boundary.
    Char = 1,
    /// Wrap at word boundary.
    #[default]
    Word = 2,
}

/// Text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum TextAlign {
    #[default]
    Left = 0,
    Center = 1,
    Right = 2,
}

/// Text overflow handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum TextOverflow {
    /// Hard clip at boundary.
    #[default]
    Clip = 0,
    /// Add ellipsis.
    Ellipsis = 1,
    /// Fade out (gradient).
    Fade = 2,
}

/// Rich text with multiple styled spans.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RichText {
    /// Styled spans.
    pub spans: Vec<Span>,
}

impl RichText {
    /// Create empty rich text.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a span.
    pub fn span(mut self, text: impl Into<String>, style: Style) -> Self {
        self.spans.push(Span {
            text: text.into(),
            style,
        });
        self
    }

    /// Add plain text span.
    pub fn text(self, text: impl Into<String>) -> Self {
        self.span(text, Style::default())
    }

    /// Add bold text span.
    pub fn bold(self, text: impl Into<String>) -> Self {
        self.span(text, Style::new().bold())
    }
}

/// A styled text span.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// The text.
    pub text: String,
    /// Style for this span.
    pub style: Style,
}

/// Native widget content.
#[derive(Debug, Clone, PartialEq)]
pub enum WidgetContent {
    /// Text input field.
    Input(InputWidget),
    /// Markdown rendering.
    Markdown(MarkdownWidget),
    /// Syntax-highlighted code.
    Code(CodeWidget),
    /// Progress bar.
    Progress(ProgressWidget),
    /// Loading spinner.
    Spinner(SpinnerWidget),
}

/// Text input widget.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InputWidget {
    /// Current value.
    pub value: String,
    /// Placeholder text.
    pub placeholder: String,
    /// Is multiline.
    pub multiline: bool,
    /// Is password (masked).
    pub password: bool,
}

/// Markdown widget.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownWidget {
    /// Markdown source.
    pub source: String,
}

/// Syntax-highlighted code widget.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeWidget {
    /// Source code.
    pub source: String,
    /// Language for highlighting.
    pub language: String,
    /// Show line numbers.
    pub line_numbers: bool,
}

/// Progress bar widget.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressWidget {
    /// Progress value (0.0 - 1.0).
    pub value: f32,
    /// Optional label.
    pub label: Option<String>,
}

/// Spinner widget.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpinnerWidget {
    /// Optional label.
    pub label: Option<String>,
}
