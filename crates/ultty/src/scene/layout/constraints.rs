//! Protocol constraint translation for the Taffy layout engine.

use taffy::prelude::*;
use taffy::Overflow;
use ultty_protocol::{
    AlignContent as ProtoAlignContent, AlignItems as ProtoAlignItems, AlignSelf as ProtoAlignSelf,
    Constraints, Dimension as ProtoDimension, Display as ProtoDisplay,
    FlexDirection as ProtoFlexDirection, FlexWrap as ProtoFlexWrap, GridLine as ProtoGridLine,
    JustifyContent as ProtoJustifyContent, OverflowAxis as ProtoOverflowAxis,
    TrackSizingFunction as ProtoTrackSizingFunction,
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
        justify_content: match c.justify_content {
            ProtoJustifyContent::Start => JustifyContent::START,
            ProtoJustifyContent::End => JustifyContent::END,
            ProtoJustifyContent::Center => JustifyContent::CENTER,
            ProtoJustifyContent::SpaceBetween => JustifyContent::SPACE_BETWEEN,
            ProtoJustifyContent::SpaceAround => JustifyContent::SPACE_AROUND,
            ProtoJustifyContent::SpaceEvenly => JustifyContent::SPACE_EVENLY,
        },
        align_items: match c.align_items {
            ProtoAlignItems::Start => AlignItems::START,
            ProtoAlignItems::End => AlignItems::END,
            ProtoAlignItems::Center => AlignItems::CENTER,
            ProtoAlignItems::Baseline => AlignItems::BASELINE,
            ProtoAlignItems::Stretch => AlignItems::STRETCH,
        },
        align_self: match c.align_self {
            ProtoAlignSelf::Auto => None,
            ProtoAlignSelf::Start => Some(AlignSelf::START),
            ProtoAlignSelf::End => Some(AlignSelf::END),
            ProtoAlignSelf::Center => Some(AlignSelf::CENTER),
            ProtoAlignSelf::Baseline => Some(AlignSelf::BASELINE),
            ProtoAlignSelf::Stretch => Some(AlignSelf::STRETCH),
        },
        align_content: match c.align_content {
            ProtoAlignContent::Start => AlignContent::START,
            ProtoAlignContent::End => AlignContent::END,
            ProtoAlignContent::Center => AlignContent::CENTER,
            ProtoAlignContent::SpaceBetween => AlignContent::SPACE_BETWEEN,
            ProtoAlignContent::SpaceAround => AlignContent::SPACE_AROUND,
            ProtoAlignContent::SpaceEvenly => AlignContent::SPACE_EVENLY,
            ProtoAlignContent::Stretch => AlignContent::STRETCH,
        },
        grid_column: Line {
            start: grid_line(&c.grid_column.start),
            end: grid_line(&c.grid_column.end),
        },
        grid_row: Line {
            start: grid_line(&c.grid_row.start),
            end: grid_line(&c.grid_row.end),
        },
        grid_template_columns: grid_tracks(&c.grid_template_columns),
        grid_template_rows: grid_tracks(&c.grid_template_rows),
        flex_grow: c.flex_grow,
        flex_shrink: c.flex_shrink,
        flex_basis: dimension_to_taffy(&c.flex_basis),
        size: Size {
            width: dimension_to_taffy(&c.width),
            height: dimension_to_taffy(&c.height),
        },
        min_size: Size {
            width: dimension_to_size_limit(&c.min_width),
            height: dimension_to_size_limit(&c.min_height),
        },
        max_size: Size {
            width: dimension_to_size_limit(&c.max_width),
            height: dimension_to_size_limit(&c.max_height),
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
        ProtoDimension::Auto => Dimension::auto(),
        ProtoDimension::Cells(v) => Dimension::length(*v),
        ProtoDimension::Percent(v) => Dimension::percent(*v / 100.0),
        ProtoDimension::Fr(_) => Dimension::auto(),
        ProtoDimension::Vw(v) => Dimension::percent(*v / 100.0),
        ProtoDimension::Vh(v) => Dimension::percent(*v / 100.0),
        ProtoDimension::MinContent => Dimension::auto(),
        ProtoDimension::MaxContent => Dimension::auto(),
    }
}

/// A min or max size. taffy 0.6+ types these as `LengthPercentageAuto`;
/// the mapping is the same as [`dimension_to_taffy`] for sizes.
pub(super) fn dimension_to_size_limit(d: &ProtoDimension) -> LengthPercentageAuto {
    match d {
        ProtoDimension::Cells(v) => LengthPercentageAuto::length(*v),
        ProtoDimension::Percent(v) | ProtoDimension::Vw(v) | ProtoDimension::Vh(v) => {
            LengthPercentageAuto::percent(*v / 100.0)
        }
        ProtoDimension::Auto
        | ProtoDimension::Fr(_)
        | ProtoDimension::MinContent
        | ProtoDimension::MaxContent => LengthPercentageAuto::auto(),
    }
}

pub(super) fn dimension_to_length_auto(d: &ProtoDimension) -> LengthPercentageAuto {
    match d {
        ProtoDimension::Auto => LengthPercentageAuto::auto(),
        ProtoDimension::Cells(v) => LengthPercentageAuto::length(*v),
        ProtoDimension::Percent(v) => LengthPercentageAuto::percent(*v / 100.0),
        _ => LengthPercentageAuto::auto(),
    }
}

pub(super) fn dimension_to_length_pct(d: &ProtoDimension) -> LengthPercentage {
    match d {
        ProtoDimension::Cells(v) => LengthPercentage::length(*v),
        ProtoDimension::Percent(v) => LengthPercentage::percent(*v / 100.0),
        _ => LengthPercentage::length(0.0),
    }
}

fn grid_line(line: &ProtoGridLine) -> GridPlacement {
    match line {
        ProtoGridLine::Auto => GridPlacement::Auto,
        ProtoGridLine::Line(0) => GridPlacement::Auto,
        ProtoGridLine::Line(n) => GridPlacement::Line((*n).into()),
        ProtoGridLine::Span(n) => GridPlacement::Span(*n),
    }
}

fn grid_track_min(d: &ProtoDimension) -> MinTrackSizingFunction {
    match d {
        ProtoDimension::Cells(v) => MinTrackSizingFunction::length(*v),
        ProtoDimension::Percent(v) | ProtoDimension::Vw(v) | ProtoDimension::Vh(v) => {
            MinTrackSizingFunction::percent(*v / 100.0)
        }
        ProtoDimension::MinContent => MinTrackSizingFunction::min_content(),
        ProtoDimension::MaxContent => MinTrackSizingFunction::max_content(),
        ProtoDimension::Auto | ProtoDimension::Fr(_) => MinTrackSizingFunction::auto(),
    }
}

fn grid_track_max(d: &ProtoDimension) -> MaxTrackSizingFunction {
    match d {
        ProtoDimension::Cells(v) => MaxTrackSizingFunction::length(*v),
        ProtoDimension::Percent(v) | ProtoDimension::Vw(v) | ProtoDimension::Vh(v) => {
            MaxTrackSizingFunction::percent(*v / 100.0)
        }
        ProtoDimension::MinContent => MaxTrackSizingFunction::min_content(),
        ProtoDimension::MaxContent => MaxTrackSizingFunction::max_content(),
        ProtoDimension::Fr(v) => MaxTrackSizingFunction::fr(*v),
        ProtoDimension::Auto => MaxTrackSizingFunction::auto(),
    }
}

fn grid_tracks(tracks: &[ProtoTrackSizingFunction]) -> Vec<GridTemplateComponent<String>> {
    let mut result = Vec::new();
    for track in tracks {
        match track {
            ProtoTrackSizingFunction::Single(d) => {
                result.push(GridTemplateComponent::Single(TrackSizingFunction {
                    min: grid_track_min(d),
                    max: grid_track_max(d),
                }))
            }
            ProtoTrackSizingFunction::MinMax(min, max) => {
                result.push(GridTemplateComponent::Single(TrackSizingFunction {
                    min: grid_track_min(min),
                    max: grid_track_max(max),
                }))
            }
            ProtoTrackSizingFunction::Repeat(count, pattern) => {
                for _ in 0..*count {
                    for dimension in pattern {
                        if result.len() >= 256 {
                            break;
                        }
                        result.push(GridTemplateComponent::Single(TrackSizingFunction {
                            min: grid_track_min(dimension),
                            max: grid_track_max(dimension),
                        }));
                    }
                    if result.len() >= 256 || pattern.is_empty() {
                        break;
                    }
                }
            }
        }
        if result.len() >= 256 {
            break;
        }
    }
    result
}

#[cfg(test)]
mod grid_tests {
    use super::*;
    use ultty_protocol::GridPlacement as ProtoGridPlacement;

    #[test]
    fn maps_grid_placement_and_tracks_for_overlays() {
        let constraints = Constraints {
            display: ProtoDisplay::Grid,
            grid_template_columns: vec![ProtoTrackSizingFunction::Single(ProtoDimension::Percent(
                100.0,
            ))],
            grid_template_rows: vec![ProtoTrackSizingFunction::Single(ProtoDimension::Cells(
                20.0,
            ))],
            grid_column: ProtoGridPlacement {
                start: ProtoGridLine::Line(1),
                end: ProtoGridLine::Line(2),
            },
            grid_row: ProtoGridPlacement {
                start: ProtoGridLine::Line(1),
                end: ProtoGridLine::Span(1),
            },
            ..Default::default()
        };
        let style = constraints_to_style(&constraints);
        assert_eq!(style.display, Display::Grid);
        assert_eq!(style.grid_column.start, GridPlacement::Line(1.into()));
        assert_eq!(style.grid_column.end, GridPlacement::Line(2.into()));
        assert_eq!(style.grid_row.end, GridPlacement::Span(1));
        assert_eq!(style.grid_template_columns.len(), 1);
        assert_eq!(style.grid_template_rows.len(), 1);
    }

    #[test]
    fn bounds_repeat_expansion() {
        let repeated = ProtoTrackSizingFunction::Repeat(u16::MAX, vec![ProtoDimension::Fr(1.0)]);
        assert!(grid_tracks(&[repeated]).len() <= 256);
    }
}
