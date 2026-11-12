//! The scroll bar model, in `jtui::scrollbar`; `lit` is the tests' cell check (`cfg(test)` does not
//! cross crates).

pub use jtui::scrollbar::*;

/// Whether the drawn cell at (`x`, `y`) is part of a vertical thumb in `colour`: filled with it, or
/// a half block drawn in it.
#[cfg(test)]
pub fn lit(
    term: &ratatui::Terminal<ratatui::backend::TestBackend>,
    x: u16,
    y: u16,
    colour: ratatui::style::Color,
) -> bool {
    let c = &term.backend().buffer()[(x, y)];
    c.bg == colour || (c.fg == colour && matches!(c.symbol(), "▀" | "▄"))
}
