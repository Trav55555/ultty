//! SGR (Select Graphic Rendition) sequence handling.

use super::AnsiParser;
use super::{Action, Attribute, Color, NamedColor};

impl AnsiParser {
    /// Dispatch SGR (Select Graphic Rendition) sequence.
    ///
    /// Parameters come in groups: a parameter and the `:` sub-parameters
    /// after it (`4:3`, `38:2::255:0:0`). The semicolon forms of the
    /// extended colors (`38;5;n`, `38;2;r;g;b`) span several groups instead.
    /// Groups we do not understand are skipped whole, so a sub-parameter is
    /// never mistaken for an attribute of its own.
    pub(super) fn dispatch_sgr<F>(&self, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.params.is_empty() || self.params == [0] {
            callback(Action::ResetAttributes);
            return;
        }

        let params = &self.params[..];
        let mut i = 0;
        while i < params.len() {
            let mut end = i + 1;
            while end < params.len() && self.subparams & (1 << end) != 0 {
                end += 1;
            }
            if end - i > 1 {
                if let Some(attr) = Self::sgr_with_subparams(&params[i..end]) {
                    callback(Action::SetAttribute(attr));
                }
                i = end;
                continue;
            }

            let p = params[i];
            let attr = match p {
                0 => {
                    callback(Action::ResetAttributes);
                    None
                }
                1 => Some(Attribute::Bold),
                2 => Some(Attribute::Dim),
                3 => Some(Attribute::Italic),
                4 => Some(Attribute::Underline),
                5 => Some(Attribute::Blink),
                7 => Some(Attribute::Reverse),
                8 => Some(Attribute::Hidden),
                9 => Some(Attribute::Strikethrough),
                21 => Some(Attribute::DoubleUnderline),
                22 => Some(Attribute::NoBold),
                23 => Some(Attribute::NoItalic),
                24 => Some(Attribute::NoUnderline),
                25 => Some(Attribute::NoBlink),
                27 => Some(Attribute::NoReverse),
                28 => Some(Attribute::NoHidden),
                29 => Some(Attribute::NoStrikethrough),
                53 => Some(Attribute::Overline),
                55 => Some(Attribute::NoOverline),

                30..=37 => Some(Attribute::ForegroundColor(Color::Named(
                    Self::named_color_from_index((p - 30) as u8),
                ))),
                39 => Some(Attribute::ForegroundColor(Color::Default)),
                40..=47 => Some(Attribute::BackgroundColor(Color::Named(
                    Self::named_color_from_index((p - 40) as u8),
                ))),
                49 => Some(Attribute::BackgroundColor(Color::Default)),
                59 => Some(Attribute::UnderlineColor(Color::Default)),
                90..=97 => Some(Attribute::ForegroundColor(Color::Named(
                    Self::bright_color_from_index((p - 90) as u8),
                ))),
                100..=107 => Some(Attribute::BackgroundColor(Color::Named(
                    Self::bright_color_from_index((p - 100) as u8),
                ))),

                // Extended colors, semicolon form: 38;5;n or 38;2;r;g;b.
                // The color's parameters are consumed even when they are
                // incomplete, so they never act as attributes.
                38 | 48 | 58 => {
                    let (color, used) = Self::extended_color(&params[i + 1..]);
                    i += used;
                    color.map(|color| Self::color_attribute(p, color))
                }

                _ => None,
            };
            if let Some(attr) = attr {
                callback(Action::SetAttribute(attr));
            }
            i += 1;
        }
    }

    /// An SGR parameter with `:` sub-parameters: `group[0]` is the
    /// parameter, the rest its sub-parameters.
    fn sgr_with_subparams(group: &[u16]) -> Option<Attribute> {
        match group {
            [4, style, ..] => match style {
                0 => Some(Attribute::NoUnderline),
                1 => Some(Attribute::Underline),
                2 => Some(Attribute::DoubleUnderline),
                3 => Some(Attribute::CurlyUnderline),
                4 => Some(Attribute::DottedUnderline),
                5 => Some(Attribute::DashedUnderline),
                _ => None,
            },
            [p @ (38 | 48 | 58), rest @ ..] => {
                let color = match rest {
                    [5, n, ..] => Color::Indexed(channel(*n)),
                    // 38:2:<colorspace>:r:g:b, the T.416 form.
                    [2, _, r, g, b, ..] => rgb(*r, *g, *b),
                    // 38:2:r:g:b, the common form without a colorspace.
                    [2, r, g, b] => rgb(*r, *g, *b),
                    _ => return None,
                };
                Some(Self::color_attribute(*p, color))
            }
            _ => None,
        }
    }

    /// The color named by the semicolon form after 38, 48 or 58, and how
    /// many parameters it used.
    fn extended_color(rest: &[u16]) -> (Option<Color>, usize) {
        match rest {
            [5, n, ..] => (Some(Color::Indexed(channel(*n))), 2),
            [2, r, g, b, ..] => (Some(rgb(*r, *g, *b)), 4),
            // Incomplete: swallow what is there.
            [5] | [2, ..] => (None, rest.len()),
            _ => (None, 0),
        }
    }

    /// The attribute SGR `p` (38, 48 or 58) sets to `color`.
    fn color_attribute(p: u16, color: Color) -> Attribute {
        match p {
            38 => Attribute::ForegroundColor(color),
            48 => Attribute::BackgroundColor(color),
            _ => Attribute::UnderlineColor(color),
        }
    }

    /// Convert color index (0-7) to NamedColor.
    pub(super) fn named_color_from_index(idx: u8) -> NamedColor {
        match idx {
            0 => NamedColor::Black,
            1 => NamedColor::Red,
            2 => NamedColor::Green,
            3 => NamedColor::Yellow,
            4 => NamedColor::Blue,
            5 => NamedColor::Magenta,
            6 => NamedColor::Cyan,
            7 => NamedColor::White,
            _ => NamedColor::White,
        }
    }

    /// Convert bright color index (0-7) to bright NamedColor.
    pub(super) fn bright_color_from_index(idx: u8) -> NamedColor {
        match idx {
            0 => NamedColor::BrightBlack,
            1 => NamedColor::BrightRed,
            2 => NamedColor::BrightGreen,
            3 => NamedColor::BrightYellow,
            4 => NamedColor::BrightBlue,
            5 => NamedColor::BrightMagenta,
            6 => NamedColor::BrightCyan,
            7 => NamedColor::BrightWhite,
            _ => NamedColor::BrightWhite,
        }
    }
}

/// An SGR color component, saturated to a byte.
fn channel(value: u16) -> u8 {
    value.min(u8::MAX as u16) as u8
}

fn rgb(r: u16, g: u16, b: u16) -> Color {
    Color::Rgb {
        r: channel(r),
        g: channel(g),
        b: channel(b),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Action, AnsiParser, Attribute, Color, NamedColor};

    /// Feed raw bytes through the parser and collect emitted actions.
    fn parse(input: &[u8]) -> Vec<Action> {
        let mut parser = AnsiParser::new();
        let mut actions = Vec::new();
        parser.feed(input, |a| actions.push(a));
        actions
    }

    // ------------------------------------------------------------------
    // Basic attributes
    // ------------------------------------------------------------------

    #[test]
    fn sgr_bold() {
        assert_eq!(
            parse(b"\x1b[1m"),
            vec![Action::SetAttribute(Attribute::Bold)]
        );
    }

    #[test]
    fn sgr_dim() {
        assert_eq!(
            parse(b"\x1b[2m"),
            vec![Action::SetAttribute(Attribute::Dim)]
        );
    }

    #[test]
    fn sgr_italic() {
        assert_eq!(
            parse(b"\x1b[3m"),
            vec![Action::SetAttribute(Attribute::Italic)]
        );
    }

    #[test]
    fn sgr_underline() {
        assert_eq!(
            parse(b"\x1b[4m"),
            vec![Action::SetAttribute(Attribute::Underline)]
        );
    }

    #[test]
    fn sgr_blink() {
        assert_eq!(
            parse(b"\x1b[5m"),
            vec![Action::SetAttribute(Attribute::Blink)]
        );
    }

    #[test]
    fn sgr_reverse() {
        assert_eq!(
            parse(b"\x1b[7m"),
            vec![Action::SetAttribute(Attribute::Reverse)]
        );
    }

    #[test]
    fn sgr_hidden() {
        assert_eq!(
            parse(b"\x1b[8m"),
            vec![Action::SetAttribute(Attribute::Hidden)]
        );
    }

    #[test]
    fn sgr_strikethrough() {
        assert_eq!(
            parse(b"\x1b[9m"),
            vec![Action::SetAttribute(Attribute::Strikethrough)]
        );
    }

    // ------------------------------------------------------------------
    // Reset
    // ------------------------------------------------------------------

    #[test]
    fn sgr_reset_explicit_zero() {
        assert_eq!(parse(b"\x1b[0m"), vec![Action::ResetAttributes]);
    }

    #[test]
    fn sgr_reset_empty_params() {
        // CSI m with no params = reset
        assert_eq!(parse(b"\x1b[m"), vec![Action::ResetAttributes]);
    }

    // ------------------------------------------------------------------
    // Disable attributes
    // ------------------------------------------------------------------

    #[test]
    fn sgr_no_bold() {
        assert_eq!(
            parse(b"\x1b[22m"),
            vec![Action::SetAttribute(Attribute::NoBold)]
        );
    }

    #[test]
    fn sgr_no_italic() {
        assert_eq!(
            parse(b"\x1b[23m"),
            vec![Action::SetAttribute(Attribute::NoItalic)]
        );
    }

    #[test]
    fn sgr_no_underline() {
        assert_eq!(
            parse(b"\x1b[24m"),
            vec![Action::SetAttribute(Attribute::NoUnderline)]
        );
    }

    #[test]
    fn sgr_no_blink() {
        assert_eq!(
            parse(b"\x1b[25m"),
            vec![Action::SetAttribute(Attribute::NoBlink)]
        );
    }

    #[test]
    fn sgr_no_reverse() {
        assert_eq!(
            parse(b"\x1b[27m"),
            vec![Action::SetAttribute(Attribute::NoReverse)]
        );
    }

    #[test]
    fn sgr_no_hidden() {
        assert_eq!(
            parse(b"\x1b[28m"),
            vec![Action::SetAttribute(Attribute::NoHidden)]
        );
    }

    #[test]
    fn sgr_no_strikethrough() {
        assert_eq!(
            parse(b"\x1b[29m"),
            vec![Action::SetAttribute(Attribute::NoStrikethrough)]
        );
    }

    // ------------------------------------------------------------------
    // Named foreground colors (30-37)
    // ------------------------------------------------------------------

    #[test]
    fn sgr_all_named_foreground_colors() {
        let expected = [
            (30, NamedColor::Black),
            (31, NamedColor::Red),
            (32, NamedColor::Green),
            (33, NamedColor::Yellow),
            (34, NamedColor::Blue),
            (35, NamedColor::Magenta),
            (36, NamedColor::Cyan),
            (37, NamedColor::White),
        ];
        for (code, color) in expected {
            let input = format!("\x1b[{}m", code);
            assert_eq!(
                parse(input.as_bytes()),
                vec![Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(color)
                ))],
                "SGR {} should map to {:?}",
                code,
                color,
            );
        }
    }

    // ------------------------------------------------------------------
    // Named background colors (40-47)
    // ------------------------------------------------------------------

    #[test]
    fn sgr_all_named_background_colors() {
        let expected = [
            (40, NamedColor::Black),
            (41, NamedColor::Red),
            (42, NamedColor::Green),
            (43, NamedColor::Yellow),
            (44, NamedColor::Blue),
            (45, NamedColor::Magenta),
            (46, NamedColor::Cyan),
            (47, NamedColor::White),
        ];
        for (code, color) in expected {
            let input = format!("\x1b[{}m", code);
            assert_eq!(
                parse(input.as_bytes()),
                vec![Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(color)
                ))],
                "SGR {} should map to {:?}",
                code,
                color,
            );
        }
    }

    // ------------------------------------------------------------------
    // Default colors (39, 49)
    // ------------------------------------------------------------------

    #[test]
    fn sgr_default_foreground() {
        assert_eq!(
            parse(b"\x1b[39m"),
            vec![Action::SetAttribute(Attribute::ForegroundColor(
                Color::Default
            ))]
        );
    }

    #[test]
    fn sgr_default_background() {
        assert_eq!(
            parse(b"\x1b[49m"),
            vec![Action::SetAttribute(Attribute::BackgroundColor(
                Color::Default
            ))]
        );
    }

    // ------------------------------------------------------------------
    // 256-color (indexed) foreground and background
    // ------------------------------------------------------------------

    #[test]
    fn sgr_256_foreground_color() {
        assert_eq!(
            parse(b"\x1b[38;5;200m"),
            vec![Action::SetAttribute(Attribute::ForegroundColor(
                Color::Indexed(200)
            ))]
        );
    }

    #[test]
    fn sgr_256_foreground_color_zero() {
        assert_eq!(
            parse(b"\x1b[38;5;0m"),
            vec![Action::SetAttribute(Attribute::ForegroundColor(
                Color::Indexed(0)
            ))]
        );
    }

    #[test]
    fn sgr_256_foreground_color_255() {
        assert_eq!(
            parse(b"\x1b[38;5;255m"),
            vec![Action::SetAttribute(Attribute::ForegroundColor(
                Color::Indexed(255)
            ))]
        );
    }

    #[test]
    fn sgr_256_background_color() {
        assert_eq!(
            parse(b"\x1b[48;5;100m"),
            vec![Action::SetAttribute(Attribute::BackgroundColor(
                Color::Indexed(100)
            ))]
        );
    }

    // ------------------------------------------------------------------
    // RGB (true color) foreground and background
    // ------------------------------------------------------------------

    #[test]
    fn sgr_rgb_foreground() {
        assert_eq!(
            parse(b"\x1b[38;2;100;150;200m"),
            vec![Action::SetAttribute(Attribute::ForegroundColor(
                Color::Rgb {
                    r: 100,
                    g: 150,
                    b: 200
                }
            ))]
        );
    }

    #[test]
    fn sgr_rgb_foreground_black() {
        assert_eq!(
            parse(b"\x1b[38;2;0;0;0m"),
            vec![Action::SetAttribute(Attribute::ForegroundColor(
                Color::Rgb { r: 0, g: 0, b: 0 }
            ))]
        );
    }

    #[test]
    fn sgr_rgb_foreground_white() {
        assert_eq!(
            parse(b"\x1b[38;2;255;255;255m"),
            vec![Action::SetAttribute(Attribute::ForegroundColor(
                Color::Rgb {
                    r: 255,
                    g: 255,
                    b: 255
                }
            ))]
        );
    }

    #[test]
    fn sgr_rgb_background() {
        assert_eq!(
            parse(b"\x1b[48;2;10;20;30m"),
            vec![Action::SetAttribute(Attribute::BackgroundColor(
                Color::Rgb {
                    r: 10,
                    g: 20,
                    b: 30
                }
            ))]
        );
    }

    // ------------------------------------------------------------------
    // Bright foreground colors (90-97)
    // ------------------------------------------------------------------

    #[test]
    fn sgr_bright_foreground_colors() {
        let expected = [
            (90, NamedColor::BrightBlack),
            (91, NamedColor::BrightRed),
            (92, NamedColor::BrightGreen),
            (93, NamedColor::BrightYellow),
            (94, NamedColor::BrightBlue),
            (95, NamedColor::BrightMagenta),
            (96, NamedColor::BrightCyan),
            (97, NamedColor::BrightWhite),
        ];
        for (code, color) in expected {
            let input = format!("\x1b[{}m", code);
            assert_eq!(
                parse(input.as_bytes()),
                vec![Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(color)
                ))],
                "SGR {} should map to {:?}",
                code,
                color,
            );
        }
    }

    // ------------------------------------------------------------------
    // Bright background colors (100-107)
    // ------------------------------------------------------------------

    #[test]
    fn sgr_bright_background_colors() {
        let expected = [
            (100, NamedColor::BrightBlack),
            (101, NamedColor::BrightRed),
            (102, NamedColor::BrightGreen),
            (103, NamedColor::BrightYellow),
            (104, NamedColor::BrightBlue),
            (105, NamedColor::BrightMagenta),
            (106, NamedColor::BrightCyan),
            (107, NamedColor::BrightWhite),
        ];
        for (code, color) in expected {
            let input = format!("\x1b[{}m", code);
            assert_eq!(
                parse(input.as_bytes()),
                vec![Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(color)
                ))],
                "SGR {} should map to {:?}",
                code,
                color,
            );
        }
    }

    // ------------------------------------------------------------------
    // Combined attributes
    // ------------------------------------------------------------------

    #[test]
    fn sgr_bold_and_red() {
        assert_eq!(
            parse(b"\x1b[1;31m"),
            vec![
                Action::SetAttribute(Attribute::Bold),
                Action::SetAttribute(Attribute::ForegroundColor(Color::Named(NamedColor::Red))),
            ]
        );
    }

    #[test]
    fn sgr_reset_then_bold_green() {
        assert_eq!(
            parse(b"\x1b[0;1;32m"),
            vec![
                Action::ResetAttributes,
                Action::SetAttribute(Attribute::Bold),
                Action::SetAttribute(Attribute::ForegroundColor(Color::Named(NamedColor::Green))),
            ]
        );
    }

    #[test]
    fn sgr_italic_underline_blue_on_white() {
        assert_eq!(
            parse(b"\x1b[3;4;34;47m"),
            vec![
                Action::SetAttribute(Attribute::Italic),
                Action::SetAttribute(Attribute::Underline),
                Action::SetAttribute(Attribute::ForegroundColor(Color::Named(NamedColor::Blue))),
                Action::SetAttribute(Attribute::BackgroundColor(Color::Named(NamedColor::White))),
            ]
        );
    }

    #[test]
    fn sgr_bold_with_rgb_foreground() {
        assert_eq!(
            parse(b"\x1b[1;38;2;255;0;0m"),
            vec![
                Action::SetAttribute(Attribute::Bold),
                Action::SetAttribute(Attribute::ForegroundColor(Color::Rgb {
                    r: 255,
                    g: 0,
                    b: 0
                })),
            ]
        );
    }

    #[test]
    fn sgr_256_fg_and_256_bg() {
        assert_eq!(
            parse(b"\x1b[38;5;82;48;5;232m"),
            vec![
                Action::SetAttribute(Attribute::ForegroundColor(Color::Indexed(82))),
                Action::SetAttribute(Attribute::BackgroundColor(Color::Indexed(232))),
            ]
        );
    }

    // ------------------------------------------------------------------
    // Edge cases
    // ------------------------------------------------------------------

    #[test]
    fn sgr_unknown_code_ignored() {
        // Code 6 is not mapped to anything - should be silently ignored
        // Only bold (1) should be emitted
        assert_eq!(
            parse(b"\x1b[1;6m"),
            vec![Action::SetAttribute(Attribute::Bold)]
        );
    }

    #[test]
    fn sgr_38_without_enough_params() {
        // CSI 38 m - extended foreground with no sub-params
        // Should just skip the 38 (no following 5;N or 2;R;G;B)
        let actions = parse(b"\x1b[38m");
        assert!(actions.is_empty());
    }

    #[test]
    fn sgr_48_without_enough_params() {
        // CSI 48 m - extended background with no sub-params
        let actions = parse(b"\x1b[48m");
        assert!(actions.is_empty());
    }

    #[test]
    fn sgr_38_5_without_index() {
        // An incomplete color is swallowed; its 5 is not Blink.
        assert_eq!(parse(b"\x1b[38;5m"), vec![]);
    }

    #[test]
    fn sgr_38_2_missing_components() {
        // An incomplete RGB color is swallowed whole: its 2 is not Dim and
        // its 100 is not a background color.
        assert_eq!(parse(b"\x1b[38;2;100;150m"), vec![]);
        assert_eq!(
            parse(b"\x1b[1;38;2;100m"),
            vec![Action::SetAttribute(Attribute::Bold)]
        );
    }

    // ------------------------------------------------------------------
    // named_color_from_index / bright_color_from_index helpers
    // ------------------------------------------------------------------

    #[test]
    fn named_color_from_index_all() {
        use super::super::AnsiParser;
        assert_eq!(AnsiParser::named_color_from_index(0), NamedColor::Black);
        assert_eq!(AnsiParser::named_color_from_index(1), NamedColor::Red);
        assert_eq!(AnsiParser::named_color_from_index(2), NamedColor::Green);
        assert_eq!(AnsiParser::named_color_from_index(3), NamedColor::Yellow);
        assert_eq!(AnsiParser::named_color_from_index(4), NamedColor::Blue);
        assert_eq!(AnsiParser::named_color_from_index(5), NamedColor::Magenta);
        assert_eq!(AnsiParser::named_color_from_index(6), NamedColor::Cyan);
        assert_eq!(AnsiParser::named_color_from_index(7), NamedColor::White);
    }

    #[test]
    fn named_color_from_index_out_of_range() {
        use super::super::AnsiParser;
        assert_eq!(AnsiParser::named_color_from_index(8), NamedColor::White);
        assert_eq!(AnsiParser::named_color_from_index(255), NamedColor::White);
    }

    #[test]
    fn bright_color_from_index_all() {
        use super::super::AnsiParser;
        assert_eq!(
            AnsiParser::bright_color_from_index(0),
            NamedColor::BrightBlack
        );
        assert_eq!(
            AnsiParser::bright_color_from_index(1),
            NamedColor::BrightRed
        );
        assert_eq!(
            AnsiParser::bright_color_from_index(2),
            NamedColor::BrightGreen
        );
        assert_eq!(
            AnsiParser::bright_color_from_index(3),
            NamedColor::BrightYellow
        );
        assert_eq!(
            AnsiParser::bright_color_from_index(4),
            NamedColor::BrightBlue
        );
        assert_eq!(
            AnsiParser::bright_color_from_index(5),
            NamedColor::BrightMagenta
        );
        assert_eq!(
            AnsiParser::bright_color_from_index(6),
            NamedColor::BrightCyan
        );
        assert_eq!(
            AnsiParser::bright_color_from_index(7),
            NamedColor::BrightWhite
        );
    }

    #[test]
    fn bright_color_from_index_out_of_range() {
        use super::super::AnsiParser;
        assert_eq!(
            AnsiParser::bright_color_from_index(8),
            NamedColor::BrightWhite
        );
        assert_eq!(
            AnsiParser::bright_color_from_index(255),
            NamedColor::BrightWhite
        );
    }

    // ------------------------------------------------------------------
    // Sub-parameters (colon form), underline styles and underline color
    // ------------------------------------------------------------------

    fn attr(a: Attribute) -> Action {
        Action::SetAttribute(a)
    }

    #[test]
    fn sgr_underline_styles_with_colons() {
        assert_eq!(parse(b"\x1b[4:0m"), vec![attr(Attribute::NoUnderline)]);
        assert_eq!(parse(b"\x1b[4:1m"), vec![attr(Attribute::Underline)]);
        assert_eq!(parse(b"\x1b[4:2m"), vec![attr(Attribute::DoubleUnderline)]);
        assert_eq!(parse(b"\x1b[4:3m"), vec![attr(Attribute::CurlyUnderline)]);
        assert_eq!(parse(b"\x1b[4:4m"), vec![attr(Attribute::DottedUnderline)]);
        assert_eq!(parse(b"\x1b[4:5m"), vec![attr(Attribute::DashedUnderline)]);
        // An unknown style is ignored, not applied as SGR 4 or SGR 9.
        assert_eq!(parse(b"\x1b[4:9m"), vec![]);
    }

    #[test]
    fn sgr_sub_parameters_do_not_become_attributes() {
        assert_eq!(
            parse(b"\x1b[1;4:3;7m"),
            vec![
                attr(Attribute::Bold),
                attr(Attribute::CurlyUnderline),
                attr(Attribute::Reverse),
            ]
        );
        // Sub-parameters on an attribute that has none are skipped whole.
        assert_eq!(parse(b"\x1b[1:2;3m"), vec![attr(Attribute::Italic)]);
    }

    #[test]
    fn sgr_extended_colors_with_colons() {
        let red = Color::Rgb { r: 255, g: 0, b: 0 };
        assert_eq!(
            parse(b"\x1b[38:2::255:0:0m"),
            vec![attr(Attribute::ForegroundColor(red.clone()))]
        );
        assert_eq!(
            parse(b"\x1b[38:2:255:0:0m"),
            vec![attr(Attribute::ForegroundColor(red.clone()))]
        );
        assert_eq!(
            parse(b"\x1b[48:2:0:255:0:0m"),
            vec![attr(Attribute::BackgroundColor(red.clone()))]
        );
        assert_eq!(
            parse(b"\x1b[38:5:196m"),
            vec![attr(Attribute::ForegroundColor(Color::Indexed(196)))]
        );
        assert_eq!(
            parse(b"\x1b[58:2::255:0:0m"),
            vec![attr(Attribute::UnderlineColor(red))]
        );
        assert_eq!(
            parse(b"\x1b[58:5:3m"),
            vec![attr(Attribute::UnderlineColor(Color::Indexed(3)))]
        );
        // Unknown color space or truncated forms are ignored.
        assert_eq!(parse(b"\x1b[38:3:1:2:3m"), vec![]);
        assert_eq!(parse(b"\x1b[38:2:1m"), vec![]);
    }

    #[test]
    fn sgr_58_semicolon_forms_keep_the_other_attributes() {
        // The 2 and 0 inside the color must not act as Dim and Reset, so
        // bold survives.
        assert_eq!(
            parse(b"\x1b[1;58;2;255;0;0m"),
            vec![
                attr(Attribute::Bold),
                attr(Attribute::UnderlineColor(Color::Rgb { r: 255, g: 0, b: 0 })),
            ]
        );
        assert_eq!(
            parse(b"\x1b[58;5;9;3m"),
            vec![
                attr(Attribute::UnderlineColor(Color::Indexed(9))),
                attr(Attribute::Italic),
            ]
        );
        assert_eq!(
            parse(b"\x1b[59m"),
            vec![attr(Attribute::UnderlineColor(Color::Default))]
        );
    }

    #[test]
    fn sgr_color_components_saturate() {
        assert_eq!(
            parse(b"\x1b[38;2;300;0;999m"),
            vec![attr(Attribute::ForegroundColor(Color::Rgb {
                r: 255,
                g: 0,
                b: 255
            }))]
        );
    }
}
