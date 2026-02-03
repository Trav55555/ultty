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
                22 => callback(Action::SetAttribute(Attribute::NoBold)),
                23 => callback(Action::SetAttribute(Attribute::NoItalic)),
                24 => callback(Action::SetAttribute(Attribute::NoUnderline)),
                25 => callback(Action::SetAttribute(Attribute::NoBlink)),
                27 => callback(Action::SetAttribute(Attribute::NoReverse)),
                28 => callback(Action::SetAttribute(Attribute::NoHidden)),
                29 => callback(Action::SetAttribute(Attribute::NoStrikethrough)),

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
