//! Protocol constraint translation for the Taffy layout engine.

use taffy::prelude::*;
use taffy::Overflow;
use ultty_protocol::{
    AlignContent as ProtoAlignContent, AlignItems as ProtoAlignItems, AlignSelf as ProtoAlignSelf,
    Constraints, Dimension as ProtoDimension, Display as ProtoDisplay,
    FlexDirection as ProtoFlexDirection, FlexWrap as ProtoFlexWrap,
    JustifyContent as ProtoJustifyContent, OverflowAxis as ProtoOverflowAxis,
};

pub(super) fn constraints_to_style(c: &Constraints) -> Style {
    Style {
        display: match c.display {
            ProtoDisplay::Block => Display::Block,
            ProtoDisplay::Flex => Display::Flex,
            ProtoDisplay::Grid => Display::Grid,
            ProtoDisplay::None => Display::None,
        },
        flex_direction: match c.flex_direction {
            ProtoFlexDirection::Row => FlexDirection::Row,
            ProtoFlexDirection::RowReverse => FlexDirection::RowReverse,
            ProtoFlexDirection::Column => FlexDirection::Column,
            ProtoFlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
        },
        flex_wrap: match c.flex_wrap {
            ProtoFlexWrap::NoWrap => FlexWrap::NoWrap,
            ProtoFlexWrap::Wrap => FlexWrap::Wrap,
            ProtoFlexWrap::WrapReverse => FlexWrap::WrapReverse,
        },
        justify_content: Some(match c.justify_content {
            ProtoJustifyContent::Start => JustifyContent::Start,
            ProtoJustifyContent::End => JustifyContent::End,
            ProtoJustifyContent::Center => JustifyContent::Center,
            ProtoJustifyContent::SpaceBetween => JustifyContent::SpaceBetween,
            ProtoJustifyContent::SpaceAround => JustifyContent::SpaceAround,
            ProtoJustifyContent::SpaceEvenly => JustifyContent::SpaceEvenly,
        }),
        align_items: Some(match c.align_items {
            ProtoAlignItems::Start => AlignItems::Start,
            ProtoAlignItems::End => AlignItems::End,
            ProtoAlignItems::Center => AlignItems::Center,
            ProtoAlignItems::Baseline => AlignItems::Baseline,
            ProtoAlignItems::Stretch => AlignItems::Stretch,
        }),
        align_self: match c.align_self {
            ProtoAlignSelf::Auto => None,
            ProtoAlignSelf::Start => Some(AlignSelf::Start),
            ProtoAlignSelf::End => Some(AlignSelf::End),
            ProtoAlignSelf::Center => Some(AlignSelf::Center),
            ProtoAlignSelf::Baseline => Some(AlignSelf::Baseline),
            ProtoAlignSelf::Stretch => Some(AlignSelf::Stretch),
        },
        align_content: Some(match c.align_content {
            ProtoAlignContent::Start => AlignContent::Start,
            ProtoAlignContent::End => AlignContent::End,
            ProtoAlignContent::Center => AlignContent::Center,
            ProtoAlignContent::SpaceBetween => AlignContent::SpaceBetween,
            ProtoAlignContent::SpaceAround => AlignContent::SpaceAround,
            ProtoAlignContent::SpaceEvenly => AlignContent::SpaceEvenly,
            ProtoAlignContent::Stretch => AlignContent::Stretch,
        }),
        flex_grow: c.flex_grow,
        flex_shrink: c.flex_shrink,
        flex_basis: dimension_to_taffy(&c.flex_basis),
        size: Size {
            width: dimension_to_taffy(&c.width),
            height: dimension_to_taffy(&c.height),
        },
        min_size: Size {
            width: dimension_to_taffy(&c.min_width),
            height: dimension_to_taffy(&c.min_height),
        },
        max_size: Size {
            width: dimension_to_taffy(&c.max_width),
            height: dimension_to_taffy(&c.max_height),
        },
        aspect_ratio: c.aspect_ratio,
        margin: Rect {
            left: dimension_to_length_auto(&c.margin.left),
            right: dimension_to_length_auto(&c.margin.right),
            top: dimension_to_length_auto(&c.margin.top),
            bottom: dimension_to_length_auto(&c.margin.bottom),
        },
        padding: Rect {
            left: dimension_to_length_pct(&c.padding.left),
            right: dimension_to_length_pct(&c.padding.right),
            top: dimension_to_length_pct(&c.padding.top),
            bottom: dimension_to_length_pct(&c.padding.bottom),
        },
        gap: Size {
            width: dimension_to_length_pct(&c.gap.width),
            height: dimension_to_length_pct(&c.gap.height),
        },
        overflow: taffy::Point {
            x: match c.overflow.x {
                ProtoOverflowAxis::Visible => Overflow::Visible,
                ProtoOverflowAxis::Hidden => Overflow::Hidden,
                ProtoOverflowAxis::Scroll => Overflow::Scroll,
                ProtoOverflowAxis::Auto => Overflow::Scroll,
            },
            y: match c.overflow.y {
                ProtoOverflowAxis::Visible => Overflow::Visible,
                ProtoOverflowAxis::Hidden => Overflow::Hidden,
                ProtoOverflowAxis::Scroll => Overflow::Scroll,
                ProtoOverflowAxis::Auto => Overflow::Scroll,
            },
        },
        ..Default::default()
    }
}

pub(super) fn dimension_to_taffy(d: &ProtoDimension) -> Dimension {
    match d {
        ProtoDimension::Auto => Dimension::Auto,
        ProtoDimension::Cells(v) => Dimension::Length(*v),
        ProtoDimension::Percent(v) => Dimension::Percent(*v / 100.0),
        ProtoDimension::Fr(_) => Dimension::Auto,
        ProtoDimension::Vw(v) => Dimension::Percent(*v / 100.0),
        ProtoDimension::Vh(v) => Dimension::Percent(*v / 100.0),
        ProtoDimension::MinContent => Dimension::Auto,
        ProtoDimension::MaxContent => Dimension::Auto,
    }
}

pub(super) fn dimension_to_length_auto(d: &ProtoDimension) -> LengthPercentageAuto {
    match d {
        ProtoDimension::Auto => LengthPercentageAuto::Auto,
        ProtoDimension::Cells(v) => LengthPercentageAuto::Length(*v),
        ProtoDimension::Percent(v) => LengthPercentageAuto::Percent(*v / 100.0),
        _ => LengthPercentageAuto::Auto,
    }
}

pub(super) fn dimension_to_length_pct(d: &ProtoDimension) -> LengthPercentage {
    match d {
        ProtoDimension::Cells(v) => LengthPercentage::Length(*v),
        ProtoDimension::Percent(v) => LengthPercentage::Percent(*v / 100.0),
        _ => LengthPercentage::Length(0.0),
    }
}
