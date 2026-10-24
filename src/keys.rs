//! The keyboard shortcuts view's model ("Preferences: Keyboard Shortcuts"): every binding with
//! its command's title, context and source. `app/keys.rs` opens and drives it; keys-a opens an
//! empty view, keys-b fills in the list and its search.

/// The open view. Empty for now (keys-b gives it its rows, search and selection).
#[derive(Debug)]
pub struct KeysView;
