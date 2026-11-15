//! The editor's semantic token styles: `Styles` is built once per frame from the theme and answers
//! "what does this `(kind, mods)` look like" in O(1), `None` when the theme has nothing for it (the
//! tree-sitter colour stays). Theme keys: `semantic.<type>` (`semantic.macro`, `.parameter`,
//! `.property`, `.enumMember`, `.namespace`, `.typeParameter`), `semantic.constructor` for clangd's
//! constructors / destructors (type `class` with the `constructorOrDestructor` modifier),
//! `semantic.readonly` and `semantic.deprecated` patched over the type's, and `Theme::dim`
//! for regions the server marked inactive (`ui::editor` dims their foreground).

use ratatui::style::Style;

use crate::semantic::{Kind, mods};
use crate::theme::Theme;

pub struct Styles {
    by_kind: [Option<Style>; Kind::NAMES.len()],
    constructor: Option<Style>,
    readonly: Option<Style>,
    deprecated: Option<Style>,
}

impl Styles {
    pub fn new(theme: &Theme) -> Self {
        let key = |name: &str| theme.find(&format!("semantic.{name}"));
        Self {
            by_kind: std::array::from_fn(|i| key(Kind::NAMES[i])),
            constructor: key("constructor"),
            readonly: key("readonly"),
            deprecated: key("deprecated"),
        }
    }

    /// The style to patch over the tree-sitter colour, if the theme has one for this token.
    pub fn get(&self, kind: Kind, modifiers: u32) -> Option<Style> {
        let mut style = if kind == Kind::Class && modifiers & mods::CONSTRUCTOR_OR_DESTRUCTOR != 0 {
            self.constructor
        } else {
            None
        };
        if style.is_none() {
            style = self.by_kind.get(kind as usize).copied().flatten();
        }
        for (bit, extra) in [(mods::READONLY, self.readonly), (mods::DEPRECATED, self.deprecated)] {
            if modifiers & bit != 0
                && let Some(extra) = extra
            {
                style = Some(style.map_or(extra, |s| s.patch(extra)));
            }
        }
        style
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_follow_the_theme_keys() {
        let theme = Theme::dark_plus(true);
        let st = Styles::new(&theme);
        let ctor = theme.get("semantic.constructor");
        assert_eq!(st.get(Kind::Class, mods::CONSTRUCTOR_OR_DESTRUCTOR), Some(ctor));
        assert_eq!(st.get(Kind::Class, 0), None, "a plain class keeps tree-sitter's colour");
        assert_eq!(st.get(Kind::Function, 0), None);
        assert_eq!(st.get(Kind::Variable, 0), None);
        assert_eq!(st.get(Kind::Macro, 0), Some(theme.get("semantic.macro")));
        assert_eq!(st.get(Kind::Parameter, 0), Some(theme.get("semantic.parameter")));
        assert_eq!(
            st.get(Kind::Variable, mods::READONLY),
            Some(theme.get("semantic.readonly"))
        );
        // Readonly over a parameter: the readonly colour wins, the rest is kept.
        let both = st.get(Kind::Parameter, mods::READONLY).unwrap();
        assert_eq!(both.fg, theme.get("semantic.readonly").fg);
        let dep = st.get(Kind::Function, mods::DEPRECATED).unwrap();
        assert!(dep.add_modifier.contains(ratatui::style::Modifier::CROSSED_OUT));
        assert_eq!(st.get(Kind::Unknown, 0), None);
    }
}
