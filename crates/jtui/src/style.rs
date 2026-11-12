//! What the shared widgets need of a colour theme: a style by key.

use ratatui::style::Style;

/// A key to `Style` map (VS Code colour keys such as `scrollbarSlider.background`). Missing keys
/// give the default style.
pub trait Styles {
    fn get(&self, key: &str) -> Style;
}
