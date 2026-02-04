//! SGR (Select Graphic Rendition) sequence handling.

use super::{Action, Attribute, Color, NamedColor};
use super::AnsiParser;

impl AnsiParser {
    /// Dispatch SGR (Select Graphic Rendition) sequence.
    pub(super) fn dispatch_sgr<F>(&self, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.params.is_empty() || self.params == [0] {
            callback(Action::ResetAttributes);
            return;
        }

        let mut i = 0;
        while i < self.params.len() {
            let p = self.params[i];
            match p {
                0 => callback(Action::ResetAttributes),
                1 => callback(Action::SetAttribute(Attribute::Bold)),
                2 => callback(Action::SetAttribute(Attribute::Dim)),
                3 => callback(Action::SetAttribute(Attribute::Italic)),
                4 => callback(Action::SetAttribute(Attribute::Underline)),
                5 => callback(Action::SetAttribute(Attribute::Blink)),
                7 => callback(Action::SetAttribute(Attribute::Reverse)),
                8 => callback(Action::SetAttribute(Attribute::Hidden)),
                9 => callback(Action::SetAttribute(Attribute::Strikethrough)),
                21 => callback(Action::SetAttribute(Attribute::DoubleUnderline)),
                22 => callback(Action::SetAttribute(Attribute::NoBold)),
                23 => callback(Action::SetAttribute(Attribute::NoItalic)),
                24 => callback(Action::SetAttribute(Attribute::NoUnderline)),
                25 => callback(Action::SetAttribute(Attribute::NoBlink)),
                27 => callback(Action::SetAttribute(Attribute::NoReverse)),
                28 => callback(Action::SetAttribute(Attribute::NoHidden)),
                29 => callback(Action::SetAttribute(Attribute::NoStrikethrough)),
                53 => callback(Action::SetAttribute(Attribute::Overline)),
                55 => callback(Action::SetAttribute(Attribute::NoOverline)),

                // Foreground colors
                30..=37 => {
                    let color = Self::named_color_from_index((p - 30) as u8);
                    callback(Action::SetAttribute(Attribute::ForegroundColor(
                        Color::Named(color),
                    )));
                }
                38 => {
                    // Extended foreground color
                    if i + 2 < self.params.len() && self.params[i + 1] == 5 {
                        // 256-color: ESC[38;5;Nm
                        let idx = self.params[i + 2] as u8;
                        callback(Action::SetAttribute(Attribute::ForegroundColor(
                            Color::Indexed(idx),
                        )));
                        i += 2;
                    } else if i + 4 < self.params.len() && self.params[i + 1] == 2 {
                        // True color: ESC[38;2;R;G;Bm
                        let r = self.params[i + 2] as u8;
                        let g = self.params[i + 3] as u8;
                        let b = self.params[i + 4] as u8;
                        callback(Action::SetAttribute(Attribute::ForegroundColor(
                            Color::Rgb { r, g, b },
                        )));
                        i += 4;
                    }
                }
                39 => callback(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Default,
                ))),

                // Background colors
                40..=47 => {
                    let color = Self::named_color_from_index((p - 40) as u8);
                    callback(Action::SetAttribute(Attribute::BackgroundColor(
                        Color::Named(color),
                    )));
                }
                48 => {
                    // Extended background color
                    if i + 2 < self.params.len() && self.params[i + 1] == 5 {
                        let idx = self.params[i + 2] as u8;
                        callback(Action::SetAttribute(Attribute::BackgroundColor(
                            Color::Indexed(idx),
                        )));
                        i += 2;
                    } else if i + 4 < self.params.len() && self.params[i + 1] == 2 {
                        let r = self.params[i + 2] as u8;
                        let g = self.params[i + 3] as u8;
                        let b = self.params[i + 4] as u8;
                        callback(Action::SetAttribute(Attribute::BackgroundColor(
                            Color::Rgb { r, g, b },
                        )));
                        i += 4;
                    }
                }
                49 => callback(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Default,
                ))),

                // Bright foreground colors
                90..=97 => {
                    let color = Self::bright_color_from_index((p - 90) as u8);
                    callback(Action::SetAttribute(Attribute::ForegroundColor(
                        Color::Named(color),
                    )));
                }

                // Bright background colors
                100..=107 => {
                    let color = Self::bright_color_from_index((p - 100) as u8);
                    callback(Action::SetAttribute(Attribute::BackgroundColor(
                        Color::Named(color),
                    )));
                }

                _ => {}
            }
            i += 1;
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
                Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Red)
                )),
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
                Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Green)
                )),
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
                Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Blue)
                )),
                Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::White)
                )),
            ]
        );
    }

    #[test]
    fn sgr_bold_with_rgb_foreground() {
        assert_eq!(
            parse(b"\x1b[1;38;2;255;0;0m"),
            vec![
                Action::SetAttribute(Attribute::Bold),
                Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Rgb { r: 255, g: 0, b: 0 }
                )),
            ]
        );
    }

    #[test]
    fn sgr_256_fg_and_256_bg() {
        assert_eq!(
            parse(b"\x1b[38;5;82;48;5;232m"),
            vec![
                Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Indexed(82)
                )),
                Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Indexed(232)
                )),
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
        // CSI 38;5 m - missing the color index
        // params = [38, 5]: i=0 for 38 doesn't have enough following params
        // to form 38;5;N, so 38 is skipped. Then i=1 processes param 5 as Blink.
        assert_eq!(
            parse(b"\x1b[38;5m"),
            vec![Action::SetAttribute(Attribute::Blink)]
        );
    }

    #[test]
    fn sgr_38_2_missing_components() {
        // CSI 38;2;100;150 m - missing blue component
        // params = [38, 2, 100, 150]: 38 at i=0 doesn't have i+4 < len,
        // so it's skipped. Remaining params are processed individually:
        // 2 -> Dim, 100 -> BrightBlack background, 150 -> unknown (skipped)
        assert_eq!(
            parse(b"\x1b[38;2;100;150m"),
            vec![
                Action::SetAttribute(Attribute::Dim),
                Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::BrightBlack)
                )),
            ]
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
}
