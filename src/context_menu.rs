//! The context menu's model, in `jtui::context_menu`, with jed's `Command` as the menu's command
//! type. `ui/context_menu.rs` draws it, `app/context_menu.rs` drives it.

use crate::command::Command;

pub use jtui::context_menu::place;

pub type MenuItem = jtui::context_menu::MenuItem<Command>;
pub type ContextMenu = jtui::context_menu::ContextMenu<Command>;
