//! Entering and leaving the full-screen terminal mode, the cursor and pointer shapes and the
//! window's identity: `jtui::terminal`, with the cursor shape read from jed's settings.

pub use jtui::terminal::*;

/// The terminal cursor's shape for `settings` (`cursor_style`, `cursor_blinking`).
pub fn cursor_shape(settings: &crate::edit::Settings) -> CursorShape {
    use crate::edit::CursorStyle as Setting;
    CursorShape {
        style: match settings.cursor_style {
            Setting::Line => CursorStyle::Line,
            Setting::Block => CursorStyle::Block,
            Setting::Underline => CursorStyle::Underline,
        },
        blinking: settings.cursor_blinking,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{CursorStyle as Setting, Settings};

    #[test]
    fn cursor_shape_maps_every_style_and_blink() {
        for (setting, style) in [
            (Setting::Line, CursorStyle::Line),
            (Setting::Block, CursorStyle::Block),
            (Setting::Underline, CursorStyle::Underline),
        ] {
            for blinking in [false, true] {
                let settings = Settings {
                    cursor_style: setting,
                    cursor_blinking: blinking,
                    ..Settings::default()
                };
                assert_eq!(cursor_shape(&settings), CursorShape { style, blinking });
            }
        }
    }
}
