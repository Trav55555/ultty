//! Property-based roundtrip tests for protocol encode/decode.
//!
//! These tests verify that all message types can be encoded and decoded
//! without data loss: decode(encode(msg)) == msg

use proptest::prelude::*;

use ultty_protocol::{
    // Layout types
    AlignContent,
    AlignItems,
    AlignSelf,
    // Message types
    AppMessage,
    // Style types
    BorderEdges,
    BorderStyle,
    BorderType,
    // Content types
    CodeWidget,
    Color,
    ComputedRegionLayout,
    Constraints,
    Content,
    // Codec traits
    Decode,
    Dimension,
    Display,
    Edges,
    Encode,
    FlexDirection,
    FlexWrap,
    // Event types
    FocusEvent,
    FrameId,
    GridLine,
    GridPlacement,
    InputEvent,
    InputType,
    InputWidget,
    JustifyContent,
    Key,
    LayoutTrigger,
    MarkdownWidget,
    Modifiers,
    MouseButton,
    NamedColor,
    Overflow,
    OverflowAxis,
    ProgressWidget,
    // Core types
    RegionId,
    ResizeEvent,
    RichText,
    ScrollEvent,
    Size,
    Span,
    SpinnerWidget,
    Style,
    TerminalMessage,
    TextAlign,
    TextContent,
    TextOverflow,
    TextWrap,
    TrackSizingFunction,
    WidgetContent,
};

// ============================================================================
// Arbitrary Implementations for Basic Types
// ============================================================================

fn arb_region_id() -> impl Strategy<Value = RegionId> {
    any::<u32>().prop_map(RegionId)
}

fn arb_frame_id() -> impl Strategy<Value = FrameId> {
    any::<u32>().prop_map(FrameId)
}

// ============================================================================
// Style Types
// ============================================================================

fn arb_named_color() -> impl Strategy<Value = NamedColor> {
    prop_oneof![
        Just(NamedColor::Black),
        Just(NamedColor::Red),
        Just(NamedColor::Green),
        Just(NamedColor::Yellow),
        Just(NamedColor::Blue),
        Just(NamedColor::Magenta),
        Just(NamedColor::Cyan),
        Just(NamedColor::White),
        Just(NamedColor::BrightBlack),
        Just(NamedColor::BrightRed),
        Just(NamedColor::BrightGreen),
        Just(NamedColor::BrightYellow),
        Just(NamedColor::BrightBlue),
        Just(NamedColor::BrightMagenta),
        Just(NamedColor::BrightCyan),
        Just(NamedColor::BrightWhite),
        Just(NamedColor::Foreground),
        Just(NamedColor::Background),
    ]
}

fn arb_color() -> impl Strategy<Value = Color> {
    prop_oneof![
        arb_named_color().prop_map(Color::Named),
        (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| Color::Rgb(r, g, b)),
        any::<u8>().prop_map(Color::Indexed),
    ]
}

fn arb_border_type() -> impl Strategy<Value = BorderType> {
    prop_oneof![
        Just(BorderType::None),
        Just(BorderType::Thin),
        Just(BorderType::Thick),
        Just(BorderType::Double),
        Just(BorderType::Rounded),
        Just(BorderType::Dashed),
        Just(BorderType::Ascii),
    ]
}

fn arb_border_edges() -> impl Strategy<Value = BorderEdges> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(|(t, r, b, l)| {
        BorderEdges {
            top: t,
            right: r,
            bottom: b,
            left: l,
        }
    })
}

fn arb_border_style() -> impl Strategy<Value = BorderStyle> {
    (arb_border_type(), arb_border_edges(), 0u8..=2).prop_map(|(style, edges, radius)| {
        BorderStyle {
            style,
            edges,
            radius,
        }
    })
}

fn arb_style() -> impl Strategy<Value = Style> {
    (
        proptest::option::of(arb_color()),
        proptest::option::of(arb_color()),
        arb_border_style(),
        proptest::option::of(arb_color()),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        0.0f32..=1.0,
    )
        .prop_map(
            |(fg, bg, border, border_color, bold, italic, underline, dim, opacity)| Style {
                fg,
                bg,
                border,
                border_color,
                bold,
                italic,
                underline,
                dim,
                opacity,
            },
        )
}

// ============================================================================
// Layout Types
// ============================================================================

fn arb_display() -> impl Strategy<Value = Display> {
    prop_oneof![
        Just(Display::Block),
        Just(Display::Flex),
        Just(Display::Grid),
        Just(Display::None),
    ]
}

fn arb_flex_direction() -> impl Strategy<Value = FlexDirection> {
    prop_oneof![
        Just(FlexDirection::Row),
        Just(FlexDirection::RowReverse),
        Just(FlexDirection::Column),
        Just(FlexDirection::ColumnReverse),
    ]
}

fn arb_flex_wrap() -> impl Strategy<Value = FlexWrap> {
    prop_oneof![
        Just(FlexWrap::NoWrap),
        Just(FlexWrap::Wrap),
        Just(FlexWrap::WrapReverse),
    ]
}

fn arb_justify_content() -> impl Strategy<Value = JustifyContent> {
    prop_oneof![
        Just(JustifyContent::Start),
        Just(JustifyContent::End),
        Just(JustifyContent::Center),
        Just(JustifyContent::SpaceBetween),
        Just(JustifyContent::SpaceAround),
        Just(JustifyContent::SpaceEvenly),
    ]
}

fn arb_align_items() -> impl Strategy<Value = AlignItems> {
    prop_oneof![
        Just(AlignItems::Start),
        Just(AlignItems::End),
        Just(AlignItems::Center),
        Just(AlignItems::Baseline),
        Just(AlignItems::Stretch),
    ]
}

fn arb_align_self() -> impl Strategy<Value = AlignSelf> {
    prop_oneof![
        Just(AlignSelf::Auto),
        Just(AlignSelf::Start),
        Just(AlignSelf::End),
        Just(AlignSelf::Center),
        Just(AlignSelf::Baseline),
        Just(AlignSelf::Stretch),
    ]
}

fn arb_align_content() -> impl Strategy<Value = AlignContent> {
    prop_oneof![
        Just(AlignContent::Start),
        Just(AlignContent::End),
        Just(AlignContent::Center),
        Just(AlignContent::SpaceBetween),
        Just(AlignContent::SpaceAround),
        Just(AlignContent::SpaceEvenly),
        Just(AlignContent::Stretch),
    ]
}

fn arb_overflow_axis() -> impl Strategy<Value = OverflowAxis> {
    prop_oneof![
        Just(OverflowAxis::Visible),
        Just(OverflowAxis::Hidden),
        Just(OverflowAxis::Scroll),
        Just(OverflowAxis::Auto),
    ]
}

fn arb_overflow() -> impl Strategy<Value = Overflow> {
    (arb_overflow_axis(), arb_overflow_axis()).prop_map(|(x, y)| Overflow { x, y })
}

// Use finite floats to avoid NaN comparison issues
fn arb_finite_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        Just(0.0f32),
        Just(1.0f32),
        Just(-1.0f32),
        Just(f32::MIN_POSITIVE),
        Just(f32::MAX / 2.0),
        Just(f32::MIN / 2.0),
        (-1000.0f32..1000.0f32),
    ]
}

fn arb_dimension() -> impl Strategy<Value = Dimension> {
    prop_oneof![
        Just(Dimension::Auto),
        arb_finite_f32().prop_map(Dimension::Cells),
        arb_finite_f32().prop_map(Dimension::Percent),
        arb_finite_f32().prop_map(Dimension::Fr),
        arb_finite_f32().prop_map(Dimension::Vw),
        arb_finite_f32().prop_map(Dimension::Vh),
        Just(Dimension::MinContent),
        Just(Dimension::MaxContent),
    ]
}

fn arb_edges_dimension() -> impl Strategy<Value = Edges<Dimension>> {
    (
        arb_dimension(),
        arb_dimension(),
        arb_dimension(),
        arb_dimension(),
    )
        .prop_map(|(top, right, bottom, left)| Edges {
            top,
            right,
            bottom,
            left,
        })
}

fn arb_size_dimension() -> impl Strategy<Value = Size<Dimension>> {
    (arb_dimension(), arb_dimension()).prop_map(|(width, height)| Size { width, height })
}

fn arb_grid_line() -> impl Strategy<Value = GridLine> {
    prop_oneof![
        Just(GridLine::Auto),
        any::<i16>().prop_map(GridLine::Line),
        any::<u16>().prop_map(GridLine::Span),
    ]
}

fn arb_grid_placement() -> impl Strategy<Value = GridPlacement> {
    (arb_grid_line(), arb_grid_line()).prop_map(|(start, end)| GridPlacement { start, end })
}

fn arb_track_sizing_function() -> impl Strategy<Value = TrackSizingFunction> {
    // Limit recursion depth for Vec
    let leaf = prop_oneof![
        arb_dimension().prop_map(TrackSizingFunction::Single),
        (arb_dimension(), arb_dimension()).prop_map(|(a, b)| TrackSizingFunction::MinMax(a, b)),
    ];

    leaf.prop_flat_map(|base| {
        prop_oneof![
            Just(base.clone()),
            (
                any::<u16>(),
                proptest::collection::vec(arb_dimension(), 0..3)
            )
                .prop_map(|(count, dims)| TrackSizingFunction::Repeat(count, dims)),
        ]
    })
}

fn arb_constraints() -> impl Strategy<Value = Constraints> {
    // Split into groups of <=10 to stay within proptest's tuple Strategy limit
    let flex_props = (
        arb_display(),
        arb_flex_direction(),
        arb_flex_wrap(),
        arb_justify_content(),
        arb_align_items(),
        arb_align_self(),
        arb_align_content(),
        arb_finite_f32(), // flex_grow
        arb_finite_f32(), // flex_shrink
        arb_dimension(),  // flex_basis
    );

    let size_props = (
        arb_dimension(),                        // width
        arb_dimension(),                        // height
        arb_dimension(),                        // min_width
        arb_dimension(),                        // min_height
        arb_dimension(),                        // max_width
        arb_dimension(),                        // max_height
        proptest::option::of(arb_finite_f32()), // aspect_ratio
    );

    let spacing_props = (
        arb_edges_dimension(), // margin
        arb_edges_dimension(), // padding
        arb_size_dimension(),  // gap
        arb_overflow(),
    );

    let grid_props = (
        proptest::collection::vec(arb_track_sizing_function(), 0..3), // grid_template_columns
        proptest::collection::vec(arb_track_sizing_function(), 0..3), // grid_template_rows
        arb_grid_placement(),                                         // grid_column
        arb_grid_placement(),                                         // grid_row
    );

    // Combine groups using nested tuples (4 groups, each <=10 elements)
    (flex_props, size_props, spacing_props, grid_props).prop_map(
        |(
            (
                display,
                flex_direction,
                flex_wrap,
                justify_content,
                align_items,
                align_self,
                align_content,
                flex_grow,
                flex_shrink,
                flex_basis,
            ),
            (width, height, min_width, min_height, max_width, max_height, aspect_ratio),
            (margin, padding, gap, overflow),
            (grid_template_columns, grid_template_rows, grid_column, grid_row),
        )| {
            Constraints {
                display,
                flex_direction,
                flex_wrap,
                justify_content,
                align_items,
                align_self,
                align_content,
                flex_grow,
                flex_shrink,
                flex_basis,
                width,
                height,
                min_width,
                min_height,
                max_width,
                max_height,
                aspect_ratio,
                margin,
                padding,
                gap,
                overflow,
                grid_template_columns,
                grid_template_rows,
                grid_column,
                grid_row,
            }
        },
    )
}

// ============================================================================
// Content Types
// ============================================================================

fn arb_text_wrap() -> impl Strategy<Value = TextWrap> {
    prop_oneof![
        Just(TextWrap::None),
        Just(TextWrap::Char),
        Just(TextWrap::Word),
    ]
}

fn arb_text_align() -> impl Strategy<Value = TextAlign> {
    prop_oneof![
        Just(TextAlign::Left),
        Just(TextAlign::Center),
        Just(TextAlign::Right),
    ]
}

fn arb_text_overflow() -> impl Strategy<Value = TextOverflow> {
    prop_oneof![
        Just(TextOverflow::Clip),
        Just(TextOverflow::Ellipsis),
        Just(TextOverflow::Fade),
    ]
}

fn arb_text_content() -> impl Strategy<Value = TextContent> {
    (".*", arb_text_wrap(), arb_text_align(), arb_text_overflow()).prop_map(
        |(text, wrap, align, overflow)| TextContent {
            text,
            wrap,
            align,
            overflow,
        },
    )
}

fn arb_span() -> impl Strategy<Value = Span> {
    (".*", arb_style()).prop_map(|(text, style)| Span { text, style })
}

fn arb_rich_text() -> impl Strategy<Value = RichText> {
    proptest::collection::vec(arb_span(), 0..5).prop_map(|spans| RichText { spans })
}

fn arb_input_widget() -> impl Strategy<Value = InputWidget> {
    (".*", ".*", any::<bool>(), any::<bool>()).prop_map(
        |(value, placeholder, multiline, password)| InputWidget {
            value,
            placeholder,
            multiline,
            password,
        },
    )
}

fn arb_markdown_widget() -> impl Strategy<Value = MarkdownWidget> {
    ".*".prop_map(|source| MarkdownWidget { source })
}

fn arb_code_widget() -> impl Strategy<Value = CodeWidget> {
    (".*", ".*", any::<bool>()).prop_map(|(source, language, line_numbers)| CodeWidget {
        source,
        language,
        line_numbers,
    })
}

fn arb_progress_widget() -> impl Strategy<Value = ProgressWidget> {
    (0.0f32..=1.0, proptest::option::of(".*"), proptest::bool::ANY, proptest::bool::ANY)
        .prop_map(|(value, label, animated, indeterminate)| ProgressWidget {
            value,
            label,
            animated,
            indeterminate,
        })
}

fn arb_spinner_widget() -> impl Strategy<Value = SpinnerWidget> {
    (proptest::option::of(".*"), 0u8..=3).prop_map(|(label, style_val)| SpinnerWidget {
        label,
        style: match style_val {
            0 => ultty_protocol::SpinnerStyle::Braille,
            1 => ultty_protocol::SpinnerStyle::Dots,
            2 => ultty_protocol::SpinnerStyle::Line,
            _ => ultty_protocol::SpinnerStyle::Bounce,
        },
    })
}

fn arb_image_widget() -> impl Strategy<Value = ultty_protocol::ImageWidget> {
    (1u32..=64, 1u32..=64, 0u8..=3).prop_map(|(w, h, scale_val)| {
        ultty_protocol::ImageWidget {
            data: vec![0u8; (w * h * 4) as usize],
            width: w,
            height: h,
            scale: match scale_val {
                0 => ultty_protocol::ImageScale::Fit,
                1 => ultty_protocol::ImageScale::Fill,
                2 => ultty_protocol::ImageScale::Stretch,
                _ => ultty_protocol::ImageScale::None,
            },
        }
    })
}

fn arb_widget_content() -> impl Strategy<Value = WidgetContent> {
    prop_oneof![
        arb_input_widget().prop_map(WidgetContent::Input),
        arb_markdown_widget().prop_map(WidgetContent::Markdown),
        arb_code_widget().prop_map(WidgetContent::Code),
        arb_progress_widget().prop_map(WidgetContent::Progress),
        arb_spinner_widget().prop_map(WidgetContent::Spinner),
        arb_image_widget().prop_map(WidgetContent::Image),
    ]
}

fn arb_content() -> impl Strategy<Value = Content> {
    prop_oneof![
        Just(Content::None),
        arb_text_content().prop_map(Content::Text),
        arb_rich_text().prop_map(Content::RichText),
        arb_widget_content().prop_map(Content::Widget),
    ]
}

// ============================================================================
// Event Types
// ============================================================================

fn arb_mouse_button() -> impl Strategy<Value = MouseButton> {
    prop_oneof![
        Just(MouseButton::Left),
        Just(MouseButton::Middle),
        Just(MouseButton::Right),
        Just(MouseButton::Button4),
        Just(MouseButton::Button5),
    ]
}

fn arb_key() -> impl Strategy<Value = Key> {
    prop_oneof![
        any::<char>().prop_map(Key::Char),
        (1u8..=12).prop_map(Key::Function),
        Just(Key::Escape),
        Just(Key::Enter),
        Just(Key::Tab),
        Just(Key::Backspace),
        Just(Key::Delete),
        Just(Key::Insert),
        Just(Key::Home),
        Just(Key::End),
        Just(Key::PageUp),
        Just(Key::PageDown),
        Just(Key::Up),
        Just(Key::Down),
        Just(Key::Left),
        Just(Key::Right),
    ]
}

fn arb_modifiers() -> impl Strategy<Value = Modifiers> {
    prop_oneof![
        Just(Modifiers::NONE),
        Just(Modifiers::SHIFT),
        Just(Modifiers::CTRL),
        Just(Modifiers::ALT),
        Just(Modifiers::META),
        Just(Modifiers::SHIFT.union(Modifiers::CTRL)),
        Just(Modifiers::CTRL.union(Modifiers::ALT)),
        Just(Modifiers::SHIFT.union(Modifiers::ALT)),
        Just(
            Modifiers::SHIFT
                .union(Modifiers::CTRL)
                .union(Modifiers::ALT)
        ),
    ]
}

fn arb_input_type() -> impl Strategy<Value = InputType> {
    prop_oneof![
        arb_mouse_button().prop_map(|button| InputType::MouseDown { button }),
        arb_mouse_button().prop_map(|button| InputType::MouseUp { button }),
        Just(InputType::MouseMove),
        (arb_finite_f32(), arb_finite_f32())
            .prop_map(|(delta_x, delta_y)| InputType::MouseScroll { delta_x, delta_y }),
        (arb_key(), arb_modifiers())
            .prop_map(|(key, modifiers)| InputType::KeyDown { key, modifiers }),
        (arb_key(), arb_modifiers())
            .prop_map(|(key, modifiers)| InputType::KeyUp { key, modifiers }),
        ".*".prop_map(|text| InputType::TextInput { text }),
    ]
}

fn arb_input_event() -> impl Strategy<Value = InputEvent> {
    (
        arb_region_id(),
        arb_input_type(),
        arb_finite_f32(),
        arb_finite_f32(),
    )
        .prop_map(|(region_id, event_type, x, y)| InputEvent {
            region_id,
            event_type,
            x,
            y,
        })
}

fn arb_resize_event() -> impl Strategy<Value = ResizeEvent> {
    (any::<u16>(), any::<u16>()).prop_map(|(width, height)| ResizeEvent { width, height })
}

fn arb_focus_event() -> impl Strategy<Value = FocusEvent> {
    (arb_region_id(), any::<bool>())
        .prop_map(|(region_id, focused)| FocusEvent { region_id, focused })
}

fn arb_scroll_event() -> impl Strategy<Value = ScrollEvent> {
    (arb_region_id(), arb_finite_f32(), arb_finite_f32()).prop_map(
        |(region_id, scroll_x, scroll_y)| ScrollEvent {
            region_id,
            scroll_x,
            scroll_y,
        },
    )
}

fn arb_layout_trigger() -> impl Strategy<Value = LayoutTrigger> {
    prop_oneof![
        Just(LayoutTrigger::Resize),
        Just(LayoutTrigger::ConstraintChange),
        Just(LayoutTrigger::ContentChange),
        Just(LayoutTrigger::TreeChange),
    ]
}

fn arb_computed_region_layout() -> impl Strategy<Value = ComputedRegionLayout> {
    (
        arb_region_id(),
        arb_finite_f32(),
        arb_finite_f32(),
        arb_finite_f32(),
        arb_finite_f32(),
        arb_finite_f32(),
        arb_finite_f32(),
        arb_finite_f32(),
        arb_finite_f32(),
        any::<bool>(),
    )
        .prop_map(
            |(
                id,
                x,
                y,
                width,
                height,
                content_x,
                content_y,
                content_width,
                content_height,
                visible,
            )| {
                ComputedRegionLayout {
                    id,
                    x,
                    y,
                    width,
                    height,
                    content_x,
                    content_y,
                    content_width,
                    content_height,
                    visible,
                }
            },
        )
}

// ============================================================================
// Message Types
// ============================================================================

fn arb_app_message() -> impl Strategy<Value = AppMessage> {
    prop_oneof![
        // ClientHello
        (any::<u16>(), any::<u32>()).prop_map(|(version, requested_capabilities)| {
            AppMessage::ClientHello {
                version,
                requested_capabilities,
            }
        }),
        // CreateRegion
        (
            arb_region_id(),
            arb_region_id(),
            any::<i32>(),
            arb_constraints(),
            arb_style()
        )
            .prop_map(|(id, parent, insert_index, constraints, style)| {
                AppMessage::CreateRegion {
                    id,
                    parent,
                    insert_index,
                    constraints,
                    style,
                }
            }),
        // SetConstraints
        (arb_region_id(), arb_constraints())
            .prop_map(|(id, constraints)| AppMessage::SetConstraints { id, constraints }),
        // SetStyle
        (arb_region_id(), arb_style()).prop_map(|(id, style)| AppMessage::SetStyle { id, style }),
        // SetContent
        (arb_region_id(), arb_content())
            .prop_map(|(id, content)| AppMessage::SetContent { id, content }),
        // DeleteRegion
        arb_region_id().prop_map(|id| AppMessage::DeleteRegion { id }),
        // ReparentRegion
        (arb_region_id(), arb_region_id(), any::<i32>()).prop_map(
            |(id, new_parent, insert_index)| {
                AppMessage::ReparentRegion {
                    id,
                    new_parent,
                    insert_index,
                }
            }
        ),
        // BeginFrame
        arb_frame_id().prop_map(|frame_id| AppMessage::BeginFrame { frame_id }),
        // EndFrame
        arb_frame_id().prop_map(|frame_id| AppMessage::EndFrame { frame_id }),
        // Scroll
        (
            arb_region_id(),
            arb_finite_f32(),
            arb_finite_f32(),
            any::<bool>()
        )
            .prop_map(|(id, x, y, animate)| AppMessage::Scroll { id, x, y, animate }),
        // Focus
        arb_region_id().prop_map(|id| AppMessage::Focus { id }),
        // Blur
        Just(AppMessage::Blur),
        // Subscribe
        (arb_region_id(), any::<u32>())
            .prop_map(|(id, events)| AppMessage::Subscribe { id, events }),
        // Unsubscribe
        (arb_region_id(), any::<u32>())
            .prop_map(|(id, events)| AppMessage::Unsubscribe { id, events }),
        // Disconnect
        Just(AppMessage::Disconnect),
    ]
}

fn arb_terminal_message() -> impl Strategy<Value = TerminalMessage> {
    prop_oneof![
        // ServerHello
        (any::<u16>(), any::<u32>()).prop_map(|(version, capabilities)| {
            TerminalMessage::ServerHello {
                version,
                capabilities,
            }
        }),
        // SessionReady
        (any::<u64>(), arb_region_id(), any::<u16>(), any::<u16>()).prop_map(
            |(session_id, root_region, width, height)| TerminalMessage::SessionReady {
                session_id,
                root_region,
                width,
                height,
            }
        ),
        // LayoutComputed
        (
            proptest::option::of(arb_frame_id()),
            arb_layout_trigger(),
            proptest::collection::vec(arb_computed_region_layout(), 0..10)
        )
            .prop_map(
                |(frame_id, trigger, regions)| TerminalMessage::LayoutComputed {
                    frame_id,
                    trigger,
                    regions,
                }
            ),
        // Input
        arb_input_event().prop_map(TerminalMessage::Input),
        // Resize
        arb_resize_event().prop_map(TerminalMessage::Resize),
        // Focus
        arb_focus_event().prop_map(TerminalMessage::Focus),
        // Scroll
        arb_scroll_event().prop_map(TerminalMessage::Scroll),
        // Error
        (any::<u16>(), ".*").prop_map(|(code, message)| TerminalMessage::Error { code, message }),
    ]
}

// ============================================================================
// Roundtrip Tests
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    #[ignore] // Requires larger stack: RUST_MIN_STACK=8388608 cargo test
    fn app_message_roundtrip(msg in arb_app_message()) {
        let encoded = msg.encode_to_bytes();
        let mut cursor = encoded.as_ref();
        let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
        prop_assert_eq!(msg, decoded, "roundtrip should preserve message");
    }

    /// Test that any TerminalMessage can roundtrip through encode/decode.
    #[test]
    fn terminal_message_roundtrip(msg in arb_terminal_message()) {
        let encoded = msg.encode_to_bytes();
        let mut cursor = encoded.as_ref();
        let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
        prop_assert_eq!(msg, decoded, "roundtrip should preserve message");
    }
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
fn empty_content_roundtrip() {
    let msg = AppMessage::SetContent {
        id: RegionId(1),
        content: Content::None,
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn empty_text_roundtrip() {
    let msg = AppMessage::SetContent {
        id: RegionId(1),
        content: Content::Text(TextContent {
            text: String::new(),
            wrap: TextWrap::Word,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn empty_rich_text_roundtrip() {
    let msg = AppMessage::SetContent {
        id: RegionId(1),
        content: Content::RichText(RichText { spans: vec![] }),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn large_text_roundtrip() {
    // Test with 64KB of text (near max reasonable message size)
    let large_text = "x".repeat(65536);
    let msg = AppMessage::SetContent {
        id: RegionId(1),
        content: Content::Text(TextContent {
            text: large_text,
            wrap: TextWrap::Word,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn max_region_id_roundtrip() {
    let msg = AppMessage::DeleteRegion {
        id: RegionId(u32::MAX),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn zero_region_id_roundtrip() {
    let msg = AppMessage::DeleteRegion { id: RegionId::ROOT };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn complex_constraints_roundtrip() {
    let msg = AppMessage::SetConstraints {
        id: RegionId(42),
        constraints: Constraints {
            display: Display::Grid,
            flex_direction: FlexDirection::RowReverse,
            flex_wrap: FlexWrap::WrapReverse,
            justify_content: JustifyContent::SpaceEvenly,
            align_items: AlignItems::Baseline,
            align_self: AlignSelf::Stretch,
            align_content: AlignContent::SpaceAround,
            flex_grow: 2.5,
            flex_shrink: 0.5,
            flex_basis: Dimension::Percent(50.0),
            width: Dimension::Cells(100.0),
            height: Dimension::Vh(80.0),
            min_width: Dimension::Cells(10.0),
            min_height: Dimension::Cells(5.0),
            max_width: Dimension::Percent(100.0),
            max_height: Dimension::MaxContent,
            aspect_ratio: Some(1.5),
            margin: Edges::new(
                Dimension::Cells(1.0),
                Dimension::Cells(2.0),
                Dimension::Cells(3.0),
                Dimension::Cells(4.0),
            ),
            padding: Edges::all(Dimension::Cells(1.0)),
            gap: Size {
                width: Dimension::Cells(2.0),
                height: Dimension::Cells(1.0),
            },
            overflow: Overflow {
                x: OverflowAxis::Hidden,
                y: OverflowAxis::Scroll,
            },
            grid_template_columns: vec![
                TrackSizingFunction::Single(Dimension::Fr(1.0)),
                TrackSizingFunction::MinMax(Dimension::Cells(100.0), Dimension::Fr(2.0)),
            ],
            grid_template_rows: vec![TrackSizingFunction::Repeat(
                3,
                vec![Dimension::Fr(1.0), Dimension::Cells(50.0)],
            )],
            grid_column: GridPlacement {
                start: GridLine::Line(1),
                end: GridLine::Span(2),
            },
            grid_row: GridPlacement {
                start: GridLine::Auto,
                end: GridLine::Line(-1),
            },
        },
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn all_style_options_roundtrip() {
    let msg = AppMessage::SetStyle {
        id: RegionId(1),
        style: Style {
            fg: Some(Color::Rgb(255, 128, 0)),
            bg: Some(Color::Named(NamedColor::BrightBlue)),
            border: BorderStyle {
                style: BorderType::Double,
                edges: BorderEdges::ALL,
                radius: 2,
            },
            border_color: Some(Color::Indexed(196)),
            bold: true,
            italic: true,
            underline: true,
            dim: true,
            opacity: 0.75,
        },
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn default_style_roundtrip() {
    let msg = AppMessage::SetStyle {
        id: RegionId(1),
        style: Style::default(),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn layout_computed_empty_regions_roundtrip() {
    let msg = TerminalMessage::LayoutComputed {
        frame_id: None,
        trigger: LayoutTrigger::Resize,
        regions: vec![],
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn layout_computed_many_regions_roundtrip() {
    let regions: Vec<ComputedRegionLayout> = (0..100)
        .map(|i| ComputedRegionLayout {
            id: RegionId(i),
            x: i as f32,
            y: i as f32 * 2.0,
            width: 100.0,
            height: 50.0,
            content_x: i as f32 + 1.0,
            content_y: i as f32 * 2.0 + 1.0,
            content_width: 98.0,
            content_height: 48.0,
            visible: i % 2 == 0,
        })
        .collect();

    let msg = TerminalMessage::LayoutComputed {
        frame_id: Some(FrameId(999)),
        trigger: LayoutTrigger::TreeChange,
        regions,
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn all_input_types_roundtrip() {
    let test_cases = vec![
        InputType::MouseDown {
            button: MouseButton::Left,
        },
        InputType::MouseUp {
            button: MouseButton::Right,
        },
        InputType::MouseMove,
        InputType::MouseScroll {
            delta_x: 1.5,
            delta_y: -2.5,
        },
        InputType::KeyDown {
            key: Key::Char('a'),
            modifiers: Modifiers::CTRL,
        },
        InputType::KeyUp {
            key: Key::Function(12),
            modifiers: Modifiers::SHIFT.union(Modifiers::ALT),
        },
        InputType::TextInput {
            text: "Hello, World!".into(),
        },
    ];

    for event_type in test_cases {
        let msg = TerminalMessage::Input(InputEvent {
            region_id: RegionId(1),
            event_type,
            x: 10.0,
            y: 20.0,
        });
        let encoded = msg.encode_to_bytes();
        let mut cursor = encoded.as_ref();
        let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
        assert_eq!(msg, decoded);
    }
}

#[test]
fn all_key_types_roundtrip() {
    let keys = vec![
        Key::Char('Z'),
        Key::Function(1),
        Key::Function(12),
        Key::Escape,
        Key::Enter,
        Key::Tab,
        Key::Backspace,
        Key::Delete,
        Key::Insert,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
    ];

    for key in keys {
        let msg = TerminalMessage::Input(InputEvent {
            region_id: RegionId(1),
            event_type: InputType::KeyDown {
                key,
                modifiers: Modifiers::NONE,
            },
            x: 0.0,
            y: 0.0,
        });
        let encoded = msg.encode_to_bytes();
        let mut cursor = encoded.as_ref();
        let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
        assert_eq!(msg, decoded);
    }
}

#[test]
fn unicode_text_roundtrip() {
    let unicode_samples = vec!["Hello, World!", "", "Rust ", "", "", "Mixed: Hello  "];

    for text in unicode_samples {
        let msg = AppMessage::SetContent {
            id: RegionId(1),
            content: Content::Text(TextContent {
                text: text.to_string(),
                wrap: TextWrap::Word,
                align: TextAlign::Left,
                overflow: TextOverflow::Clip,
            }),
        };
        let encoded = msg.encode_to_bytes();
        let mut cursor = encoded.as_ref();
        let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
        assert_eq!(msg, decoded);
    }
}

#[test]
fn all_widget_types_roundtrip() {
    let widgets = vec![
        WidgetContent::Input(InputWidget {
            value: "test".into(),
            placeholder: "Enter text...".into(),
            multiline: true,
            password: false,
        }),
        WidgetContent::Markdown(MarkdownWidget {
            source: "# Hello\n\n**Bold** and *italic*".into(),
        }),
        WidgetContent::Code(CodeWidget {
            source: "fn main() { println!(\"Hello\"); }".into(),
            language: "rust".into(),
            line_numbers: true,
        }),
        WidgetContent::Progress(ProgressWidget {
            value: 0.75,
            label: Some("Loading...".into()),
            animated: false,
            indeterminate: false,
        }),
        WidgetContent::Spinner(SpinnerWidget {
            label: Some("Processing".into()),
            style: ultty_protocol::SpinnerStyle::Braille,
        }),
    ];

    for widget in widgets {
        let msg = AppMessage::SetContent {
            id: RegionId(1),
            content: Content::Widget(widget),
        };
        let encoded = msg.encode_to_bytes();
        let mut cursor = encoded.as_ref();
        let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
        assert_eq!(msg, decoded);
    }
}

#[test]
fn disconnect_roundtrip() {
    // This should work even with the current placeholder implementation
    let msg = AppMessage::Disconnect;
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn blur_roundtrip() {
    let msg = AppMessage::Blur;
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

// ============================================================================
// Negative Tests (Malformed Input)
// ============================================================================

#[test]
fn decode_empty_buffer_fails() {
    let mut cursor: &[u8] = &[];
    let result = AppMessage::decode(&mut cursor);
    assert!(result.is_err());
}

#[test]
fn decode_truncated_header_fails() {
    let mut cursor: &[u8] = &[0x55, 0x54, 0x00]; // Magic + partial type
    let result = AppMessage::decode(&mut cursor);
    assert!(result.is_err());
}

#[test]
fn decode_invalid_magic_fails() {
    let mut cursor: &[u8] = &[0xFF, 0xFF, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00];
    let result = AppMessage::decode(&mut cursor);
    assert!(result.is_err());
}

// ============================================================================
// Specific Variant Tests (to ensure no variant is missed)
// ============================================================================

#[test]
fn app_message_client_hello_roundtrip() {
    let msg = AppMessage::ClientHello {
        version: 1,
        requested_capabilities: 0xFFFFFFFF,
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_create_region_roundtrip() {
    let msg = AppMessage::CreateRegion {
        id: RegionId(1),
        parent: RegionId::ROOT,
        insert_index: -1,
        constraints: Constraints::default(),
        style: Style::default(),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_set_constraints_roundtrip() {
    let msg = AppMessage::SetConstraints {
        id: RegionId(5),
        constraints: Constraints::default().flex_grow(1.0),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_set_style_roundtrip() {
    let msg = AppMessage::SetStyle {
        id: RegionId(5),
        style: Style::new().fg(Color::RED).bold(),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_set_content_roundtrip() {
    let msg = AppMessage::SetContent {
        id: RegionId(5),
        content: Content::Text(TextContent::new("Hello")),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_delete_region_roundtrip() {
    let msg = AppMessage::DeleteRegion { id: RegionId(10) };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_reparent_region_roundtrip() {
    let msg = AppMessage::ReparentRegion {
        id: RegionId(5),
        new_parent: RegionId(2),
        insert_index: 0,
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_begin_frame_roundtrip() {
    let msg = AppMessage::BeginFrame {
        frame_id: FrameId(123),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_end_frame_roundtrip() {
    let msg = AppMessage::EndFrame {
        frame_id: FrameId(123),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_scroll_roundtrip() {
    let msg = AppMessage::Scroll {
        id: RegionId(5),
        x: 10.5,
        y: 20.5,
        animate: true,
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_focus_roundtrip() {
    let msg = AppMessage::Focus { id: RegionId(5) };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_subscribe_roundtrip() {
    let msg = AppMessage::Subscribe {
        id: RegionId(5),
        events: 0b1111,
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn app_message_unsubscribe_roundtrip() {
    let msg = AppMessage::Unsubscribe {
        id: RegionId(5),
        events: 0b1111,
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = AppMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn terminal_message_server_hello_roundtrip() {
    let msg = TerminalMessage::ServerHello {
        version: 1,
        capabilities: 0xDEADBEEF,
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn terminal_message_session_ready_roundtrip() {
    let msg = TerminalMessage::SessionReady {
        session_id: 0x123456789ABCDEF0,
        root_region: RegionId::ROOT,
        width: 80,
        height: 24,
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn terminal_message_resize_roundtrip() {
    let msg = TerminalMessage::Resize(ResizeEvent {
        width: 120,
        height: 40,
    });
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn terminal_message_focus_roundtrip() {
    let msg = TerminalMessage::Focus(FocusEvent {
        region_id: RegionId(5),
        focused: true,
    });
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn terminal_message_scroll_roundtrip() {
    let msg = TerminalMessage::Scroll(ScrollEvent {
        region_id: RegionId(5),
        scroll_x: 0.0,
        scroll_y: 100.5,
    });
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}

#[test]
fn terminal_message_error_roundtrip() {
    let msg = TerminalMessage::Error {
        code: 404,
        message: "Region not found".to_string(),
    };
    let encoded = msg.encode_to_bytes();
    let mut cursor = encoded.as_ref();
    let decoded = TerminalMessage::decode(&mut cursor).expect("decode should succeed");
    assert_eq!(msg, decoded);
}
