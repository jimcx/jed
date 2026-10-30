//! Named colour keys. Widgets never hard-code colours; they look up a key here.

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style};

pub struct Theme {
    styles: HashMap<&'static str, Style>,
    /// Task terminals' colours (`terminal_cell`).
    terminal: TerminalColors,
}

/// The 16 ANSI colours (`terminal.ansi*`), the default foreground and background.
struct TerminalColors {
    ansi: [Color; 16],
    fg: Color,
    bg: Color,
    truecolor: bool,
}

/// The `terminal.ansi*` keys, in ANSI order (0–7, then the bright 8–15).
pub const ANSI_KEYS: [&str; 16] = [
    "terminal.ansiBlack",
    "terminal.ansiRed",
    "terminal.ansiGreen",
    "terminal.ansiYellow",
    "terminal.ansiBlue",
    "terminal.ansiMagenta",
    "terminal.ansiCyan",
    "terminal.ansiWhite",
    "terminal.ansiBrightBlack",
    "terminal.ansiBrightRed",
    "terminal.ansiBrightGreen",
    "terminal.ansiBrightYellow",
    "terminal.ansiBrightBlue",
    "terminal.ansiBrightMagenta",
    "terminal.ansiBrightCyan",
    "terminal.ansiBrightWhite",
];

/// Ends what `Theme::ansi` started.
pub const ANSI_RESET: &str = "\x1b[0m";

impl Theme {
    pub fn get(&self, key: &str) -> Style {
        self.styles.get(key).copied().unwrap_or_else(|| {
            tracing::warn!(key, "missing theme key");
            Style::default()
        })
    }

    /// `key`'s style, `None` (and no warning) when the theme doesn't define it.
    pub fn find(&self, key: &str) -> Option<Style> {
        self.styles.get(key).copied()
    }

    /// `style` dimmed for code the server says is compiled out: its foreground (the editor's when
    /// it has none) blended halfway into the editor background, so the syntax colours stay
    /// recognisable. Without an RGB colour to blend (one of the terminal's 16) it takes
    /// `semantic.inactive`'s foreground.
    pub fn dim(&self, style: Style) -> Style {
        let editor = self.get("editor");
        let blended = style.fg.or(editor.fg).zip(editor.bg).and_then(|(fg, bg)| {
            let (f, b) = (rgb_of(fg)?, rgb_of(bg)?);
            let mix = |f: u8, b: u8| ((f as u16 + b as u16) / 2) as u8;
            let hex = (mix(f.0, b.0) as u32) << 16 | (mix(f.1, b.1) as u32) << 8 | mix(f.2, b.2) as u32;
            Some(rgb(hex, matches!(fg, Color::Rgb(..))))
        });
        match blended {
            Some(fg) => style.fg(fg),
            None => style.patch(self.get("semantic.inactive")),
        }
    }

    /// The ANSI escape that starts `key`'s style on a terminal (truecolor, or 256 colours as the
    /// theme was built), for text printed outside the UI (`--check-language --sample`); end it
    /// with `ANSI_RESET`.
    pub fn ansi(&self, key: &str) -> String {
        let style = self.get(key);
        let mut codes = Vec::new();
        for (modifier, code) in [
            (Modifier::BOLD, "1"),
            (Modifier::ITALIC, "3"),
            (Modifier::UNDERLINED, "4"),
        ] {
            if style.add_modifier.contains(modifier) {
                codes.push(code.to_string());
            }
        }
        match style.fg {
            Some(Color::Rgb(r, g, b)) => codes.push(format!("38;2;{r};{g};{b}")),
            Some(Color::Indexed(i)) => codes.push(format!("38;5;{i}")),
            _ => {}
        }
        if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        }
    }

    /// Maps a task terminal's cell as `tui-term` drew it from the `vt100` screen (`Reset` for
    /// the defaults, `Indexed` for palette colours, `Rgb`) to the theme: the defaults to
    /// `terminal.foreground` / `.background`, ANSI 0–15 to `terminal.ansi*` (0–7 drawn bright
    /// when bold, VS Code's `drawBoldTextInBrightColors`), 16–255 kept, RGB to the nearest
    /// 256-colour index without truecolor.
    pub fn terminal_cell(&self, cell: &mut ratatui::buffer::Cell) {
        let t = &self.terminal;
        let bold = cell.modifier.contains(Modifier::BOLD);
        let map = |c: Color, default: Color, fg: bool| match c {
            Color::Reset => default,
            Color::Indexed(i) if i < 8 && fg && bold => t.ansi[i as usize + 8],
            Color::Indexed(i) if i < 16 => t.ansi[i as usize],
            Color::Rgb(r, g, b) if !t.truecolor => Color::Indexed(to_ansi256(r, g, b)),
            c => c,
        };
        cell.fg = map(cell.fg, t.fg, true);
        cell.bg = map(cell.bg, t.bg, false);
    }

    /// The built-in dark theme, modelled on VS Code Dark+.
    pub fn dark_plus(truecolor: bool) -> Self {
        let c = |hex: u32| rgb(hex, truecolor);
        let fg_bg = |fg: u32, bg: u32| Style::new().fg(c(fg)).bg(c(bg));
        let styles = HashMap::from([
            ("editor", fg_bg(0xD4D4D4, 0x1E1E1E)),
            ("editor.lineHighlight", Style::new().bg(c(0x2A2D2E))),
            ("editor.selection", Style::new().bg(c(0x264F78))),
            // Secondary cursors: `cursor_style = "block"` reverses the cell, `line` / `underline`
            // underline it in Dark+'s `editorCursor.foreground`.
            ("editor.cursor.secondary", Style::new().add_modifier(Modifier::REVERSED)),
            ("editor.cursor.secondaryUnderline", squiggle(c(0xAEAFAD))),
            // Dark Modern's current match; Dark+'s other matches (#EA5C0055) blended over the editor.
            ("editor.findMatch", Style::new().bg(c(0x9E6A03))),
            ("editor.findMatchHighlight", Style::new().bg(c(0x613214))),
            // Dark+'s `editorBracketMatch` is a faint green fill with a grey border; a terminal cell
            // has no border, so a stronger fill.
            ("editor.bracketMatch", Style::new().bg(c(0x2F4F3A))),
            // Semantic tokens (the language server's, over tree-sitter's colours; only these differ
            // from them): Dark+'s enum member / readonly colours; parameters in the variable blue
            // (clangd marks every use, tree-sitter only some), fields in a light grey (Visual
            // Studio's dark C++); constructors and namespaces in the type colour; macros in Dark+'s
            // macro purple. `inactive` is the fallback foreground for code the server compiled out
            // (clangd's `#if 0`) when `Theme::dim` can't blend the cell's own colour.
            ("semantic.constructor", Style::new().fg(c(0x4EC9B0))),
            ("semantic.macro", Style::new().fg(c(0xBD63C5))),
            ("semantic.parameter", Style::new().fg(c(0x9CDCFE))),
            ("semantic.property", Style::new().fg(c(0xDADADA))),
            ("semantic.enumMember", Style::new().fg(c(0x4FC1FF))),
            ("semantic.namespace", Style::new().fg(c(0x4EC9B0))),
            ("semantic.typeParameter", Style::new().fg(c(0x4EC9B0))),
            ("semantic.readonly", Style::new().fg(c(0x4FC1FF))),
            ("semantic.deprecated", Style::new().add_modifier(Modifier::CROSSED_OUT)),
            ("semantic.inactive", Style::new().fg(c(0x5A5A5A))),
            ("editor.whitespace.control", Style::new().fg(c(0xF48771))),
            // Rendered spaces and tabs (`·` / `→`): Dark+'s #E3E4E229 blended over the editor.
            ("editorWhitespace.foreground", Style::new().fg(c(0x3E3E3D))),
            // Inline blame after the cursor line (VS Code's `git.blame.editorDecorationForeground`,
            // Dark+'s `editorCodeLens.foreground`).
            ("git.blame.editorDecorationForeground", Style::new().fg(c(0x999999))),
            // The editors' scroll bar thumb (its track is the editor's background): Dark+'s
            // translucent sliders blended over the editor — #79797966, hover #646464B3, while
            // dragged #BFBFBF66.
            ("scrollbarSlider.background", Style::new().bg(c(0x424242))),
            ("scrollbarSlider.hoverBackground", Style::new().bg(c(0x4F4F4F))),
            ("scrollbarSlider.activeBackground", Style::new().bg(c(0x5E5E5E))),
            // Diagnostics' squiggles (Dark+'s colours): a terminal has no undercurl through
            // ratatui, so an underline in the colour (SGR 58; terminals without it underline in
            // the text's colour). Dark+'s hint (#EEEEEEB3) blended over the editor.
            ("editorError.foreground", squiggle(c(0xF14C4C))),
            ("editorWarning.foreground", squiggle(c(0xCCA700))),
            ("editorInfo.foreground", squiggle(c(0x3794FF))),
            ("editorHint.foreground", squiggle(c(0xB0B0B0))),
            ("gutter", fg_bg(0x858585, 0x1E1E1E)),
            // Hex tabs: offsets dim as line numbers, the `.` stand-ins (and their bytes' hex)
            // dimmed a little, the ASCII column's `|`s.
            ("hexEditor.offset", Style::new().fg(c(0x858585))),
            ("hexEditor.nonPrintable", Style::new().fg(c(0x8C8C8C))),
            ("hexEditor.border", Style::new().fg(c(0x5A5A5A))),
            ("editorRuler.foreground", Style::new().fg(c(0x5A5A5A))),
            ("gutter.active", Style::new().fg(c(0xC6C6C6))),
            // Quick diff's gutter markers (Dark+'s `editorGutter.*Background`; a bar or a line in
            // the colour, so the foreground here). Deleted is Dark+'s error foreground.
            ("editorGutter.addedBackground", Style::new().fg(c(0x487E02))),
            ("editorGutter.modifiedBackground", Style::new().fg(c(0x1B81A8))),
            ("editorGutter.deletedBackground", Style::new().fg(c(0xF14C4C))),
            // The scroll bar's marks for staged changes: the same three, darkened (×0.6), so a
            // staged line reads quieter than one only in the working tree.
            ("editorGutter.stagedAddedBackground", Style::new().fg(c(0x2B4B01))),
            ("editorGutter.stagedModifiedBackground", Style::new().fg(c(0x104D65))),
            ("editorGutter.stagedDeletedBackground", Style::new().fg(c(0x912E2E))),
            // The gutter's peek: removed / added lines (Dark+'s `diffEditor.*LineBackground`,
            // #ff000033 and #9bb95533, blended over the peek's background), its buttons.
            ("diffEditor.removedLineBackground", Style::new().bg(c(0x511E1E))),
            ("diffEditor.insertedLineBackground", Style::new().bg(c(0x3D432F))),
            // The diff view: the changed part of a changed line (Dark+'s `*TextBackground`,
            // #ff000033 / #9ccc2c33, blended over the line's), the other side of a lone line
            // (`diffEditor.diagonalFill`, #cccccc33 over the editor), the header row.
            ("diffEditor.removedTextBackground", Style::new().bg(c(0x741818))),
            ("diffEditor.insertedTextBackground", Style::new().bg(c(0x505F2F))),
            ("diffEditor.diagonalFill", Style::new().fg(c(0x404040))),
            ("diffEditor.header", fg_bg(0xBBBBBB, 0x252526)),
            // Merge conflicts' blocks (VS Code's `merge.*Background`; the header colours are
            // Dark+'s #3675aa55 / #3c806b55 / #3a3a3a55 blended over the editor, the content
            // ones opaque). The marker labels are `git.blame.editorDecorationForeground`.
            ("merge.currentHeaderBackground", Style::new().bg(c(0x263B4D))),
            ("merge.currentContentBackground", Style::new().bg(c(0x2E4B6F))),
            ("merge.incomingHeaderBackground", Style::new().bg(c(0x283F38))),
            ("merge.incomingContentBackground", Style::new().bg(c(0x29594E))),
            ("merge.commonHeaderBackground", Style::new().bg(c(0x272727))),
            ("merge.commonContentBackground", Style::new().bg(c(0x3D3D3D))),
            // A commit tab (`git show`'s text): git's own log colours — the commit id yellow,
            // the stat graph's `+` green and `-` red; everything else the editor's colour.
            ("gitShow.commit", Style::new().fg(c(0xE5E510))),
            ("gitShow.add", Style::new().fg(c(0x81B88B))),
            ("gitShow.del", Style::new().fg(c(0xF14C4C))),
            ("quickDiff.title", Style::new().fg(c(0xBBBBBB))),
            ("quickDiff.action", fg_bg(0xFFFFFF, 0x0E639C)),
            // The diff view's Revert button (red: Dark+'s `statusBarItem.errorBackground`).
            ("diffEditor.revertAction", fg_bg(0xFFFFFF, 0xC72E0F)),
            // The status bar's backgrounds (`[window] status_bar`): a matching family, white
            // text, Dark+'s blue kept as `statusBar.blue`. The warning / error / restricted
            // cells keep their own keys.
            ("statusBar.red", fg_bg(0xFFFFFF, 0xC72E0F)),
            ("statusBar.orange", fg_bg(0xFFFFFF, 0xB35C00)),
            ("statusBar.yellow", fg_bg(0xFFFFFF, 0x8A6D00)),
            ("statusBar.green", fg_bg(0xFFFFFF, 0x16825D)),
            ("statusBar.cyan", fg_bg(0xFFFFFF, 0x0E7A8C)),
            ("statusBar.blue", fg_bg(0xFFFFFF, 0x007ACC)),
            ("statusBar.purple", fg_bg(0xFFFFFF, 0x68217A)),
            ("statusBar.magenta", fg_bg(0xFFFFFF, 0xB51963)),
            ("statusBar.grey", fg_bg(0xFFFFFF, 0x5A5A5A)),
            ("statusBar.warning", fg_bg(0x1E1E1E, 0xCCA700)),
            ("statusBar.error", fg_bg(0xFFFFFF, 0xC72E0F)),
            ("dialog", fg_bg(0xCCCCCC, 0x252526)),
            ("dialog.border", Style::new().fg(c(0x454545))),
            ("dialog.key", Style::new().fg(c(0x3794FF)).add_modifier(Modifier::BOLD)),
            ("dialog.input", fg_bg(0xCCCCCC, 0x3C3C3C)),
            ("dialog.error", Style::new().fg(c(0xF48771))),
            // The keyboard cheat sheet's section headings (keys are `dialog.key`, titles `dialog`).
            (
                "cheatSheet.heading",
                Style::new().fg(c(0xE7E7E7)).add_modifier(Modifier::BOLD),
            ),
            ("activityBar", fg_bg(0x858585, 0x333333)),
            ("activityBar.active", Style::new().fg(c(0xFFFFFF))),
            ("activityBar.disabled", Style::new().fg(c(0x4E4E4E))),
            ("sideBar", fg_bg(0xCCCCCC, 0x252526)),
            ("sideBar.title", Style::new().fg(c(0xBBBBBB))),
            (
                "sideBar.header",
                Style::new().fg(c(0xCCCCCC)).add_modifier(Modifier::BOLD),
            ),
            ("sideBar.border", Style::new().fg(c(0x3C3C3C))),
            ("sideBar.dim", Style::new().fg(c(0x858585))),
            // The Explorer's "Git History" rows: the author, the relative date, the short id.
            ("gitHistory.author", Style::new().fg(c(0xBBBBBB))),
            ("gitHistory.date", Style::new().fg(c(0x858585))),
            ("gitHistory.hash", Style::new().fg(c(0x858585))),
            ("list.active", fg_bg(0xFFFFFF, 0x04395E)),
            // Explorer entries by git status (Dark+'s `gitDecoration.*`; a folder takes its most
            // important file's).
            // Gitignored entries: dimmer than the normal entry text; git colours win.
            ("explorer.ignored", Style::new().fg(c(0x8C8C8C))),
            ("gitDecoration.addedResourceForeground", Style::new().fg(c(0x81B88B))),
            ("gitDecoration.modifiedResourceForeground", Style::new().fg(c(0xE2C08D))),
            ("gitDecoration.deletedResourceForeground", Style::new().fg(c(0xC74E39))),
            ("gitDecoration.renamedResourceForeground", Style::new().fg(c(0x73C991))),
            (
                "gitDecoration.untrackedResourceForeground",
                Style::new().fg(c(0x73C991)),
            ),
            (
                "gitDecoration.conflictingResourceForeground",
                Style::new().fg(c(0xE4676B)),
            ),
            ("list.inactive", Style::new().bg(c(0x37373D))),
            // The Source Control view: section counts (Dark+'s `badge.*`), the rows' stage /
            // unstage buttons, a deleted file's name struck through (as VS Code).
            ("scm.count", fg_bg(0xFFFFFF, 0x4D4D4D)),
            ("scm.action", Style::new().fg(c(0xFFFFFF)).add_modifier(Modifier::BOLD)),
            ("scm.deleted", Style::new().add_modifier(Modifier::CROSSED_OUT)),
            // The Tools view: a language server's state mark, its log's error badge
            // (`activityErrorBadge`).
            ("tools.starting", Style::new().fg(c(0xCCA700))),
            ("tools.running", Style::new().fg(c(0x89D185))),
            ("tools.failed", Style::new().fg(c(0xF14C4C))),
            ("tools.exited", Style::new().fg(c(0x858585))),
            ("tools.errorBadge", fg_bg(0xFFFFFF, 0xF14C4C)),
            ("tabBar", Style::new().bg(c(0x252526))),
            ("tab.active", fg_bg(0xFFFFFF, 0x1E1E1E)),
            ("tab.inactive", fg_bg(0x969696, 0x2D2D2D)),
            ("tab.unfocusedActive", fg_bg(0x8F8F8F, 0x1E1E1E)),
            // The tab bar's `‹` / `›` while its tabs overflow it (Dark+'s icon foreground, and
            // dimmed when there's nothing more that way).
            ("tab.scrollArrow", fg_bg(0xC5C5C5, 0x252526)),
            ("tab.scrollArrow.disabled", fg_bg(0x5A5A5A, 0x252526)),
            // A group's number in its tab bar's top-right corner (two or more groups).
            ("tab.groupNumber", fg_bg(0x6E6E6E, 0x252526)),
            // A shell tab's state glyph: `○` at the prompt, `●` while a command runs, and a red
            // `○` when the last command failed.
            ("tab.shellRunning", Style::new().fg(c(0x3794FF))),
            ("tab.shellIdle", Style::new().fg(c(0x858585))),
            ("tab.shellError", Style::new().fg(c(0xF48771))),
            // Where a dragged tab would land on a tab bar (VS Code's `tab.dropBorder`).
            ("tab.dropMarker", fg_bg(0xFFFFFF, 0x007ACC)),
            // The zone a dragged tab would drop in over an editor (VS Code's
            // `editorGroup.dropBackground`): the whole editor, or the half a split takes.
            ("editor.dropBackground", fg_bg(0xFFFFFF, 0x0A4A7A)),
            ("editorGroup.border", fg_bg(0x444444, 0x1E1E1E)),
            // The splash screen in an editor group with no tabs (a quiet logo, like VS Code's
            // watermark).
            ("splash.logo", Style::new().fg(c(0x3B6EA5))),
            ("splash.subtitle", Style::new().fg(c(0x858585))),
            ("tab.deleted", Style::new().add_modifier(Modifier::CROSSED_OUT)),
            // The changed-on-disk banner over an editor (VS Code's `banner.*`; its buttons are
            // Dark+'s `button.*`, the icon its warning colour).
            ("banner", fg_bg(0xFFFFFF, 0x04395E)),
            ("banner.icon", Style::new().fg(c(0xCCA700))),
            ("banner.action", fg_bg(0xFFFFFF, 0x0E639C)),
            ("statusBar.restricted", fg_bg(0x1E1E1E, 0xCCA700)),
            (
                "palette.highlight",
                Style::new().fg(c(0x2AAAFF)).add_modifier(Modifier::BOLD),
            ),
            ("palette.detail", Style::new().fg(c(0x8C8C8C))),
            ("find.option", Style::new().fg(c(0x8C8C8C))),
            ("find.option.active", fg_bg(0xFFFFFF, 0x1F6FB0)),
            ("find.option.disabled", Style::new().fg(c(0x4D4D4D))),
            // Search view: Dark+'s find highlight (#EA5C0055) blended over the side bar.
            ("search.match", Style::new().bg(c(0x663719))),
            (
                "search.fileName",
                Style::new().fg(c(0xE7E7E7)).add_modifier(Modifier::BOLD),
            ),
            ("search.count", fg_bg(0xFFFFFF, 0x4D4D4D)),
            // Its rows' buttons (the `×` dismiss) and file rows' chevrons.
            (
                "search.action",
                Style::new().fg(c(0xFFFFFF)).add_modifier(Modifier::BOLD),
            ),
            ("search.chevron", Style::new().fg(c(0xC5C5C5))),
            // A result line's replace preview (VS Code's: the match struck through on
            // `diffEditor.removedTextBackground`, its replacement on `insertedTextBackground`).
            (
                "search.replace.removed",
                Style::new().bg(c(0x741818)).add_modifier(Modifier::CROSSED_OUT),
            ),
            ("search.replace.inserted", Style::new().bg(c(0x505F2F))),
            ("search.error", Style::new().fg(c(0xF48771))),
            ("search.warning", Style::new().fg(c(0xCCA700))),
            // The bottom panel (Dark+: panel background as the editor's, a #80808059 border).
            ("panel", fg_bg(0xCCCCCC, 0x1E1E1E)),
            ("panel.border", Style::new().fg(c(0x4B4B4B))),
            ("panel.dim", Style::new().fg(c(0x8C8C8C))),
            (
                "panelTitle.active",
                Style::new().fg(c(0xE7E7E7)).add_modifier(Modifier::UNDERLINED),
            ),
            ("panelTitle.inactive", Style::new().fg(c(0x969696))),
            ("panel.badge", fg_bg(0xFFFFFF, 0x4D4D4D)),
            (
                "panel.fileName",
                Style::new().fg(c(0xE7E7E7)).add_modifier(Modifier::BOLD),
            ),
            // Task terminals (Dark+'s integrated terminal: its foreground, the panel's background,
            // the ANSI palette), and their tabs' state marks.
            ("terminal.foreground", Style::new().fg(c(0xCCCCCC))),
            ("terminal.background", Style::new().bg(c(0x1E1E1E))),
            ("terminal.ansiBlack", Style::new().fg(c(0x000000))),
            ("terminal.ansiRed", Style::new().fg(c(0xCD3131))),
            ("terminal.ansiGreen", Style::new().fg(c(0x0DBC79))),
            ("terminal.ansiYellow", Style::new().fg(c(0xE5E510))),
            ("terminal.ansiBlue", Style::new().fg(c(0x2472C8))),
            ("terminal.ansiMagenta", Style::new().fg(c(0xBC3FBC))),
            ("terminal.ansiCyan", Style::new().fg(c(0x11A8CD))),
            ("terminal.ansiWhite", Style::new().fg(c(0xE5E5E5))),
            ("terminal.ansiBrightBlack", Style::new().fg(c(0x666666))),
            ("terminal.ansiBrightRed", Style::new().fg(c(0xF14C4C))),
            ("terminal.ansiBrightGreen", Style::new().fg(c(0x23D18B))),
            ("terminal.ansiBrightYellow", Style::new().fg(c(0xF5F543))),
            ("terminal.ansiBrightBlue", Style::new().fg(c(0x3B8EEA))),
            ("terminal.ansiBrightMagenta", Style::new().fg(c(0xD670D6))),
            ("terminal.ansiBrightCyan", Style::new().fg(c(0x29B8DB))),
            ("terminal.ansiBrightWhite", Style::new().fg(c(0xE5E5E5))),
            ("task.running", Style::new().fg(c(0x3794FF))),
            ("task.succeeded", Style::new().fg(c(0x89D185))),
            ("task.failed", Style::new().fg(c(0xF14C4C))),
            ("problemsErrorIcon.foreground", Style::new().fg(c(0xF14C4C))),
            ("problemsWarningIcon.foreground", Style::new().fg(c(0xCCA700))),
            ("problemsInfoIcon.foreground", Style::new().fg(c(0x3794FF))),
            ("problemsHintIcon.foreground", Style::new().fg(c(0xB0B0B0))),
            // The hover popup (Dark+), its headings bold, code (inline and blocks) in Dark+'s
            // `textPreformat.foreground`.
            ("editorHoverWidget.background", Style::new().bg(c(0x252526))),
            ("editorHoverWidget.foreground", Style::new().fg(c(0xCCCCCC))),
            ("editorHoverWidget.border", Style::new().fg(c(0x454545))),
            ("editorHoverWidget.heading", Style::new().add_modifier(Modifier::BOLD)),
            // A diagnostic's `source(code)` in the hover, dimmed (VS Code's descriptionForeground).
            ("editorHoverWidget.source", Style::new().fg(c(0x9D9D9D))),
            ("textPreformat.foreground", Style::new().fg(c(0xD7BA7D))),
            // The completion list (Dark+'s suggest widget); the detail is Dark+'s
            // `descriptionForeground` (#CCCCCCB3) blended over the widget.
            ("editorSuggestWidget.background", Style::new().bg(c(0x252526))),
            ("editorSuggestWidget.foreground", Style::new().fg(c(0xD4D4D4))),
            ("editorSuggestWidget.border", Style::new().fg(c(0x454545))),
            ("editorSuggestWidget.selectedBackground", fg_bg(0xFFFFFF, 0x04395E)),
            (
                "editorSuggestWidget.highlightForeground",
                Style::new().fg(c(0x2AAAFF)).add_modifier(Modifier::BOLD),
            ),
            ("editorSuggestWidget.detail", Style::new().fg(c(0x9D9D9D))),
            // The code action list (Dark+'s `editorActionList.*`, `disabledForeground`), the
            // preferred action's mark, the hover's "Quick Fix…" row (`textLink.foreground`), the
            // gutter's lightbulb (`editorLightBulb.foreground`; the emoji keeps its own colour).
            ("editorActionList.background", Style::new().bg(c(0x252526))),
            ("editorActionList.foreground", Style::new().fg(c(0xCCCCCC))),
            ("editorActionList.border", Style::new().fg(c(0x454545))),
            ("editorActionList.focusBackground", fg_bg(0xFFFFFF, 0x04395E)),
            ("editorActionList.disabledForeground", Style::new().fg(c(0x7F7F7F))),
            ("editorActionList.preferred", Style::new().fg(c(0xFFCC00))),
            ("textLink.foreground", Style::new().fg(c(0x3794FF))),
            ("editorLightBulb.foreground", Style::new().fg(c(0xFFCC00))),
            // Completion kind icons: Dark+'s `symbolIcon.*Foreground` groups.
            ("symbolIcon.functionForeground", Style::new().fg(c(0xB180D7))),
            ("symbolIcon.variableForeground", Style::new().fg(c(0x75BEFF))),
            ("symbolIcon.classForeground", Style::new().fg(c(0xEE9D28))),
            ("symbolIcon.foreground", Style::new().fg(c(0xCCCCCC))),
            // Syntax (the capture keys in `syntax::KEYS`): VS Code Dark+'s token colours.
            ("comment", Style::new().fg(c(0x6A9955))),
            ("constant", Style::new().fg(c(0x4FC1FF))),
            ("constant.builtin", Style::new().fg(c(0x569CD6))),
            ("constructor", Style::new().fg(c(0x4EC9B0))),
            // The git commit message editor's line pass: the diff below the scissors line (the
            // diff view's colours as foregrounds: git's own green / red, the header's grey),
            // the subject's overflow (the warning yellow) and text on line 2 (the error red).
            ("diff.inserted", Style::new().fg(c(0x89D185))),
            ("diff.deleted", Style::new().fg(c(0xF14C4C))),
            ("diff.header", Style::new().fg(c(0xBBBBBB)).add_modifier(Modifier::BOLD)),
            ("diff.hunk", Style::new().fg(c(0x4EC9B0))),
            ("overflow", Style::new().fg(c(0xCCA700))),
            ("invalid", Style::new().fg(c(0xF44747))),
            ("escape", Style::new().fg(c(0xD7BA7D))),
            ("function", Style::new().fg(c(0xDCDCAA))),
            ("keyword", Style::new().fg(c(0x569CD6))),
            ("keyword.control", Style::new().fg(c(0xC586C0))),
            ("label", Style::new().fg(c(0xC8C8C8))),
            // Markdown (Dark+'s `markup.heading`, `markup.inline.raw`, `markup.underline`, the
            // list and quote punctuation).
            (
                "markup.heading",
                Style::new().fg(c(0x569CD6)).add_modifier(Modifier::BOLD),
            ),
            (
                "markup.link",
                Style::new().fg(c(0xD4D4D4)).add_modifier(Modifier::UNDERLINED),
            ),
            ("markup.list", Style::new().fg(c(0x6796E6))),
            ("markup.quote", Style::new().fg(c(0x6A9955))),
            ("markup.raw", Style::new().fg(c(0xCE9178))),
            ("number", Style::new().fg(c(0xB5CEA8))),
            ("operator", Style::new().fg(c(0xD4D4D4))),
            ("property", Style::new().fg(c(0x9CDCFE))),
            ("punctuation", Style::new().fg(c(0xD4D4D4))),
            ("punctuation.special", Style::new().fg(c(0x569CD6))),
            // XML (Dark+'s `punctuation.definition.tag`, `entity.name.tag`, attribute names); YAML
            // keys are tags too.
            ("punctuation.tag", Style::new().fg(c(0x808080))),
            ("string", Style::new().fg(c(0xCE9178))),
            ("tag", Style::new().fg(c(0x569CD6))),
            ("tag.attribute", Style::new().fg(c(0x9CDCFE))),
            ("type", Style::new().fg(c(0x4EC9B0))),
            ("type.builtin", Style::new().fg(c(0x569CD6))),
            ("variable", Style::new().fg(c(0x9CDCFE))),
            ("variable.builtin", Style::new().fg(c(0x569CD6))),
            ("variable.parameter", Style::new().fg(c(0x9CDCFE))),
        ]);
        let fg = |key: &str| styles.get(key).and_then(|s| s.fg).unwrap_or(Color::Reset);
        let terminal = TerminalColors {
            ansi: ANSI_KEYS.map(fg),
            fg: fg("terminal.foreground"),
            bg: styles
                .get("terminal.background")
                .and_then(|s| s.bg)
                .unwrap_or(Color::Reset),
            truecolor,
        };
        Self { styles, terminal }
    }
}

/// An underline in `color`, the text's own colour kept.
fn squiggle(color: Color) -> Style {
    Style::new().underline_color(color).add_modifier(Modifier::UNDERLINED)
}

/// Whether the terminal advertises 24-bit colour.
pub fn terminal_has_truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit")
}

/// A colour's red, green and blue: exact for `Rgb`, xterm's for the 256-colour cube and grey
/// ramp; `None` for the terminal's 16 palette colours and the default.
fn rgb_of(color: Color) -> Option<(u8, u8, u8)> {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(i @ 16..=231) => {
            let i = (i - 16) as usize;
            Some((LEVELS[i / 36], LEVELS[i / 6 % 6], LEVELS[i % 6]))
        }
        Color::Indexed(i @ 232..=255) => {
            let v = 8 + 10 * (i - 232);
            Some((v, v, v))
        }
        _ => None,
    }
}

fn rgb(hex: u32, truecolor: bool) -> Color {
    let (r, g, b) = ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    if truecolor {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(to_ansi256(r, g, b))
    }
}

/// Nearest xterm-256 colour: the 6×6×6 cube or the grey ramp, whichever is closer.
fn to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |v: u8| (0..6).min_by_key(|&i| (LEVELS[i] as i32 - v as i32).abs()).unwrap();
    let (ri, gi, bi) = (nearest(r), nearest(g), nearest(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);

    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gi_ramp = ((avg.saturating_sub(8)) / 10).min(23) as u8;
    let grey = 8 + gi_ramp * 10;

    let dist = |(a, b2, c2): (u8, u8, u8)| {
        let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
        d(a, r) + d(b2, g) + d(c2, b)
    };
    if dist((grey, grey, grey)) < dist(cube) {
        232 + gi_ramp
    } else {
        16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `dim` blends the foreground halfway into the editor background (keeping its hue), in
    /// truecolor and 256 colours; a palette colour falls back to `semantic.inactive`.
    #[test]
    fn dim_blends_the_foreground_into_the_background() {
        let theme = Theme::dark_plus(true);
        // Keyword blue 0x569CD6 over 0x1E1E1E.
        let dim = theme.dim(Style::new().fg(Color::Rgb(0x56, 0x9C, 0xD6)));
        assert_eq!(dim.fg, Some(Color::Rgb(0x3A, 0x5D, 0x7A)));
        // No foreground: the editor's own text colour (0xD4D4D4), dimmed.
        assert_eq!(theme.dim(Style::new()).fg, Some(Color::Rgb(0x79, 0x79, 0x79)));
        let low = Theme::dark_plus(false);
        let blue = low.get("keyword").fg.unwrap();
        let dimmed = low.dim(Style::new().fg(blue)).fg.unwrap();
        assert!(matches!(dimmed, Color::Indexed(_)) && dimmed != blue);
        assert_eq!(
            theme.dim(Style::new().fg(Color::Red)).fg,
            theme.get("semantic.inactive").fg
        );
    }

    /// Every highlight key has a colour, distinct in 256-colour mode for the main token kinds.
    #[test]
    fn syntax_keys_are_in_the_theme() {
        for truecolor in [true, false] {
            let theme = Theme::dark_plus(truecolor);
            for key in crate::syntax::KEYS {
                assert!(theme.styles.get(key).and_then(|s| s.fg).is_some(), "{key}");
            }
        }
        let theme = Theme::dark_plus(false);
        let fg = |k: &str| theme.get(k).fg.unwrap();
        assert_eq!(fg("keyword"), Color::Indexed(to_ansi256(0x56, 0x9C, 0xD6)));
        let kinds = [
            "keyword",
            "keyword.control",
            "string",
            "comment",
            "number",
            "type",
            "function",
            "variable",
        ];
        for (i, a) in kinds.iter().enumerate() {
            for b in &kinds[i + 1..] {
                assert_ne!(fg(a), fg(b), "{a} and {b}");
            }
        }
    }

    /// The scroll bar's staged marks reuse the gutter colours, darkened: each staged key is a
    /// darker sibling of its unstaged one.
    #[test]
    fn staged_gutter_colours_darken_the_unstaged() {
        let theme = Theme::dark_plus(true);
        for (unstaged, staged) in [
            ("editorGutter.addedBackground", "editorGutter.stagedAddedBackground"),
            (
                "editorGutter.modifiedBackground",
                "editorGutter.stagedModifiedBackground",
            ),
            ("editorGutter.deletedBackground", "editorGutter.stagedDeletedBackground"),
        ] {
            let (Some(Color::Rgb(ar, ag, ab)), Some(Color::Rgb(br, bg, bb))) =
                (theme.get(unstaged).fg, theme.get(staged).fg)
            else {
                panic!("{unstaged} and {staged} should be RGB colours");
            };
            assert!(br < ar && bg < ag && bb < ab, "{staged} darker than {unstaged}");
        }
    }

    /// `[window] status_bar` has a key per colour, white text on distinct backgrounds, with
    /// Dark+'s blue kept.
    #[test]
    fn status_bar_colours_are_in_the_theme() {
        let theme = Theme::dark_plus(true);
        let mut backgrounds = Vec::new();
        for colour in crate::workspace::config::StatusBarColor::ALL {
            let style = theme.get(colour.theme_key());
            assert_eq!(style.fg, Some(Color::Rgb(0xFF, 0xFF, 0xFF)), "{}", colour.name());
            let Some(bg) = style.bg else {
                panic!("{} has a background", colour.name());
            };
            assert!(!backgrounds.contains(&bg), "{} is distinct", colour.name());
            backgrounds.push(bg);
        }
        assert_eq!(
            theme.get("statusBar.blue").bg,
            Some(Color::Rgb(0x00, 0x7A, 0xCC)),
            "the existing Dark+ blue"
        );
    }

    #[test]
    fn squiggles_are_coloured_underlines() {
        for truecolor in [true, false] {
            let theme = Theme::dark_plus(truecolor);
            for s in crate::diagnostics::Severity::ALL {
                let style = theme.styles[s.theme_key()];
                assert!(style.add_modifier.contains(Modifier::UNDERLINED), "{s:?}");
                assert!(style.underline_color.is_some() && style.fg.is_none() && style.bg.is_none());
            }
        }
    }

    #[test]
    fn terminal_cells_take_the_theme_colours() {
        use ratatui::buffer::Cell;
        for truecolor in [true, false] {
            let theme = Theme::dark_plus(truecolor);
            let mapped = |fg: Color, bg: Color, bold: bool| {
                let mut cell = Cell::default();
                cell.fg = fg;
                cell.bg = bg;
                if bold {
                    cell.modifier = Modifier::BOLD;
                }
                theme.terminal_cell(&mut cell);
                (cell.fg, cell.bg)
            };
            let key_fg = |k: &str| theme.get(k).fg.unwrap();
            assert_eq!(
                mapped(Color::Reset, Color::Reset, false),
                (
                    key_fg("terminal.foreground"),
                    theme.get("terminal.background").bg.unwrap()
                )
            );
            let (fg, bg) = mapped(Color::Indexed(1), Color::Indexed(4), false);
            assert_eq!((fg, bg), (key_fg("terminal.ansiRed"), key_fg("terminal.ansiBlue")));
            assert_eq!(
                mapped(Color::Indexed(1), Color::Reset, true).0,
                key_fg("terminal.ansiBrightRed")
            );
            assert_eq!(
                mapped(Color::Indexed(9), Color::Reset, false).0,
                key_fg("terminal.ansiBrightRed")
            );
            assert_eq!(mapped(Color::Indexed(208), Color::Reset, false).0, Color::Indexed(208));
            let rgb = mapped(Color::Rgb(0, 0x7A, 0xCC), Color::Reset, false).0;
            if truecolor {
                assert_eq!(rgb, Color::Rgb(0, 0x7A, 0xCC));
                assert_eq!(key_fg("terminal.ansiRed"), Color::Rgb(0xCD, 0x31, 0x31));
            } else {
                assert_eq!(rgb, Color::Indexed(32));
                assert!(matches!(key_fg("terminal.ansiRed"), Color::Indexed(_)));
            }
        }
    }

    #[test]
    fn ansi256_mapping() {
        assert_eq!(to_ansi256(0, 0, 0), 16);
        assert_eq!(to_ansi256(255, 255, 255), 231);
        assert_eq!(to_ansi256(0x1E, 0x1E, 0x1E), 234);
        assert_eq!(to_ansi256(0, 0x7A, 0xCC), 32);
    }
}
