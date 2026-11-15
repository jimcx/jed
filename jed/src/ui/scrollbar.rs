//! Scroll bar drawing (`jtui::scrollbar_draw`); `look` reads the app's drag and hover state.

use crate::app::App;
use crate::layout::ScrollTarget;

pub use jtui::scrollbar_draw::{Look, Mark, render, render_marks, render_row, take_column, take_row};

/// How `target`'s thumb looks: dragged, under the pointer, or neither.
pub fn look(app: &App, target: ScrollTarget) -> Look {
    if app.scrollbar_dragged() == Some(target) {
        Look::Active
    } else if app.ui.scrollbar_hover == Some(target) {
        Look::Hover
    } else {
        Look::Normal
    }
}
