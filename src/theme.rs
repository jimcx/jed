//! Named colour keys. Widgets never hard-code colours; they look up a key here. The type and the
//! shared widgets' entries are `jtui::theme`'s; this adds the editor, syntax and feature entries.

use ratatui::style::{Modifier, Style};

pub use jtui::theme::{ANSI_RESET, terminal_has_truecolor};
use jtui::theme::{rgb, squiggle};

pub struct Theme {
    base: jtui::theme::Theme,
    /// `syntax::KEYS`' styles in order, resolved once when the theme is built (indexed by a
    /// highlight's index); `get` would hash and look up every key every frame.
    token_styles: Vec<Style>,
}

/// How many backgrounds `Theme::status_bar_auto` picks from: 30 hues × 4 saturation / lightness pairs.
pub const STATUS_AUTO_SLOTS: u64 = 120;

impl std::ops::Deref for Theme {
    type Target = jtui::theme::Theme;

    fn deref(&self) -> &jtui::theme::Theme {
        &self.base
    }
}

impl Theme {
    /// The style of each `syntax::KEYS` entry, in order.
    pub fn token_styles(&self) -> &[Style] {
        &self.token_styles
    }

    /// A Markdown preview span's style: its key's style, plus the span's bold / italic /
    /// strikethrough flags, on the code block background when `block`.
    pub fn markdown_style(&self, s: &crate::markdown::SpanStyle) -> Style {
        let mut st = if s.block {
            self.get("markdown.codeblock")
        } else {
            Style::default()
        };
        st = st.patch(self.get(s.key));
        if s.bold {
            st = st.add_modifier(Modifier::BOLD);
        }
        if s.italic {
            st = st.add_modifier(Modifier::ITALIC);
        }
        if s.strike {
            st = st.add_modifier(Modifier::CROSSED_OUT);
        }
        st
    }

    /// `[window] status_bar = "auto"`: white text on one of `STATUS_AUTO_SLOTS` backgrounds, `slot`
    /// (any number; taken modulo) picking a hue (30, equally spaced; consecutive slots step the hue)
    /// and one of four saturation / lightness pairs at the dark, saturated end, near the fixed colours.
    /// Computed, not themeable: no theme key, only the terminal's colour depth is used.
    pub fn status_bar_auto(&self, slot: u64) -> Style {
        const HUES: u64 = 30;
        // (saturation, lightness): lightness stays low so the white text reads on every hue.
        const SL: [(f32, f32); 4] = [(1.0, 0.34), (0.9, 0.30), (0.8, 0.27), (0.7, 0.24)];
        let slot = slot % STATUS_AUTO_SLOTS;
        let hue = (slot % HUES) as f32 * 360.0 / HUES as f32;
        let (s, l) = SL[(slot / HUES) as usize];
        let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
        let (r, g, b) = match (hue / 60.0) as u32 {
            0 => (chroma, x, 0.0),
            1 => (x, chroma, 0.0),
            2 => (0.0, chroma, x),
            3 => (0.0, x, chroma),
            4 => (x, 0.0, chroma),
            _ => (chroma, 0.0, x),
        };
        let m = l - chroma / 2.0;
        let byte = |v: f32| ((v + m) * 255.0).round() as u32;
        let hex = byte(r) << 16 | byte(g) << 8 | byte(b);
        let truecolor = self.truecolor();
        Style::new().fg(rgb(0xFFFFFF, truecolor)).bg(rgb(hex, truecolor))
    }

    /// The built-in dark theme, modelled on VS Code Dark+: `jtui`'s widget entries plus `own_entries`.
    pub fn dark_plus(truecolor: bool) -> Self {
        let base = jtui::theme::Theme::new(
            jtui::theme::widget_entries(truecolor)
                .into_iter()
                .chain(own_entries(truecolor)),
            truecolor,
        );
        let token_styles = crate::syntax::KEYS
            .iter()
            .map(|k| {
                base.find(k).unwrap_or_else(|| {
                    tracing::warn!(key = k, "missing theme key");
                    Style::default()
                })
            })
            .collect();
        Self { base, token_styles }
    }
}

/// Dark+'s entries for everything but the keys `jtui::theme::widget_entries` holds.
fn own_entries(truecolor: bool) -> Vec<(&'static str, Style)> {
    let c = |hex: u32| rgb(hex, truecolor);
    let fg_bg = |fg: u32, bg: u32| Style::new().fg(c(fg)).bg(c(bg));
    vec![
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
        ("editor.selectionHighlight", Style::new().bg(c(0x343A40))),
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
        // The commit hash that starts the label (over the label's colour).
        ("git.blame.hash", Style::new().fg(c(0x6e6e6e))),
        // Diagnostics' squiggles (Dark+'s colours): a terminal has no undercurl through
        // ratatui, so an underline in the colour (SGR 58; terminals without it underline in
        // the text's colour). Dark+'s hint (#EEEEEEB3) blended over the editor.
        ("editorError.foreground", squiggle(c(0xF14C4C))),
        ("editorWarning.foreground", squiggle(c(0xCCA700))),
        ("editorInfo.foreground", squiggle(c(0x3794FF))),
        ("editorHint.foreground", squiggle(c(0xB0B0B0))),
        // Spelling marks (`spell_check`): teal, apart from the error / warning / info colours.
        ("editorSpell.foreground", squiggle(c(0x4EC9B0))),
        ("gutter", fg_bg(0x858585, 0x1E1E1E)),
        // Hex tabs: offsets dim as line numbers, the `.` stand-ins (and their bytes' hex)
        // dimmed a little, the ASCII column's `|`s.
        ("hexEditor.offset", Style::new().fg(c(0x858585))),
        ("hexEditor.nonPrintable", Style::new().fg(c(0x8C8C8C))),
        ("hexEditor.border", Style::new().fg(c(0x5A5A5A))),
        ("hexEditor.pending", Style::new().fg(c(0x6A6A6A))),
        // The data inspector: its header, labels (dim), values, the row(s) whose bytes are
        // outlined, and those bytes (`inspected`: a box of colour, underlined so it shows
        // over the selection).
        (
            "hexEditor.inspector.header",
            Style::new().fg(c(0xBBBBBB)).add_modifier(Modifier::BOLD),
        ),
        ("hexEditor.inspector.label", Style::new().fg(c(0x858585))),
        ("hexEditor.inspector.value", Style::new().fg(c(0xD4D4D4))),
        ("hexEditor.inspector.active", fg_bg(0xFFFFFF, 0x04395E)),
        (
            "hexEditor.inspected",
            Style::new()
                .fg(c(0xFFFFFF))
                .bg(c(0x0B5A8E))
                .add_modifier(Modifier::UNDERLINED),
        ),
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
        // The title bar (`[window] title_bar`): indigo, a hue none of the status bar's colours use.
        ("titleBar", fg_bg(0xFFFFFF, 0x35359A)),
        ("titleBar.button", fg_bg(0xFFFFFF, 0x5454C4)),
        // The same while the terminal window has lost the focus.
        ("titleBar.inactive", fg_bg(0xB0B0B0, 0x3A3A3A)),
        ("titleBar.inactive.button", fg_bg(0xB0B0B0, 0x4A4A4A)),
        ("statusBar.warning", fg_bg(0x1E1E1E, 0xCCA700)),
        ("statusBar.error", fg_bg(0xFFFFFF, 0xC72E0F)),
        ("dialog", fg_bg(0xCCCCCC, 0x252526)),
        ("dialog.border", Style::new().fg(c(0x454545))),
        ("dialog.key", Style::new().fg(c(0x3794FF)).add_modifier(Modifier::BOLD)),
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
        ("sideBar.dim", Style::new().fg(c(0x858585))),
        // The Explorer's "Git History" rows: the author, the relative date, the short id.
        ("fileHistory.date", Style::new().fg(c(0x858585))),
        ("fileHistory.size", Style::new().fg(c(0xBBBBBB))),
        ("gitHistory.author", Style::new().fg(c(0xBBBBBB))),
        ("gitHistory.date", Style::new().fg(c(0x858585))),
        ("gitHistory.hash", Style::new().fg(c(0x858585))),
        ("list.active", fg_bg(0xFFFFFF, 0x04395E)),
        // Explorer entries by git status (Dark+'s `gitDecoration.*`; a folder takes its most
        // important file's).
        // Gitignored entries: dimmer than the normal entry text; git colours win.
        ("list.dropBackground", fg_bg(0xFFFFFF, 0x383B3D)),
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
        // The Code Review panel: file status letters, +/- counts, the viewed mark, an error.
        ("review.added", Style::new().fg(c(0x81B88B))),
        ("review.modified", Style::new().fg(c(0xE2C08D))),
        ("review.deleted", Style::new().fg(c(0xC74E39))),
        ("review.renamed", Style::new().fg(c(0x73C991))),
        ("review.viewed", Style::new().fg(c(0x89D185))),
        ("review.unresolved", Style::new().fg(c(0xE2C08D))),
        ("review.resolved", Style::new().fg(c(0x89D185))),
        ("review.error", Style::new().fg(c(0xF14C4C))),
        ("review.approved", Style::new().fg(c(0x89D185))),
        ("review.changesRequested", Style::new().fg(c(0xE2C08D))),
        // Thread blocks in a review diff: the frame, the header row, a resolved thread's header
        // and the comment text.
        ("review.thread.border", fg_bg(0x6A9FB5, 0x1E2A33)),
        ("review.thread.header", fg_bg(0x9CDCFE, 0x1E2A33)),
        ("review.thread.resolved", fg_bg(0x808080, 0x1E2A33)),
        ("review.thread.text", fg_bg(0xD4D4D4, 0x1E2A33)),
        // Draft comments (a dimmed frame, text like a thread's), the comment box being written
        // (a bright frame, its buttons) and a held-back draft in the PENDING list.
        ("review.draft.border", fg_bg(0x7A6A3E, 0x1E2A33)),
        ("review.draft.header", fg_bg(0xE2C08D, 0x1E2A33)),
        ("review.composer.border", fg_bg(0x4FC1FF, 0x1E2A33)),
        ("review.composer.button", fg_bg(0xFFFFFF, 0x0E639C)),
        ("review.thread.button", fg_bg(0xFFFFFF, 0x3A3D41)),
        ("review.pending.held", Style::new().fg(c(0xCCA700))),
        ("tools.starting", Style::new().fg(c(0xCCA700))),
        ("tools.running", Style::new().fg(c(0x89D185))),
        ("tools.failed", Style::new().fg(c(0xF14C4C))),
        ("tools.exited", Style::new().fg(c(0x858585))),
        ("tools.errorBadge", fg_bg(0xFFFFFF, 0xF14C4C)),
        // The zone a dragged tab would drop in over an editor (VS Code's
        // `editorGroup.dropBackground`): the whole editor, or the half a split takes.
        ("editor.dropBackground", fg_bg(0xFFFFFF, 0x0A4A7A)),
        // The splash screen in an editor group with no tabs (a quiet logo, like VS Code's
        // watermark).
        ("splash.logo", Style::new().fg(c(0x3B6EA5))),
        ("splash.subtitle", Style::new().fg(c(0x858585))),
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
        // Markdown preview (`markdown.rs` spans carry these keys): headings get a colour ramp,
        // H1-H4 bold, H5 italic, H6 dim italic.
        ("markdown.h1", Style::new().fg(c(0xFFFFFF)).add_modifier(Modifier::BOLD)),
        ("markdown.h2", Style::new().fg(c(0x9CDCFE)).add_modifier(Modifier::BOLD)),
        ("markdown.h3", Style::new().fg(c(0x569CD6)).add_modifier(Modifier::BOLD)),
        ("markdown.h4", Style::new().fg(c(0x7F9FBF)).add_modifier(Modifier::BOLD)),
        (
            "markdown.h5",
            Style::new().fg(c(0x569CD6)).add_modifier(Modifier::ITALIC),
        ),
        (
            "markdown.h6",
            Style::new().fg(c(0x808080)).add_modifier(Modifier::ITALIC),
        ),
        ("markdown.text", Style::new().fg(c(0xD4D4D4))),
        ("markdown.code", fg_bg(0xCE9178, 0x2D2D2D)),
        // Fenced / indented code blocks: this background sits under every `block` span.
        ("markdown.codeblock", fg_bg(0xD4D4D4, 0x262626)),
        (
            "markdown.link",
            Style::new().fg(c(0x3794FF)).add_modifier(Modifier::UNDERLINED),
        ),
        ("markdown.url", Style::new().fg(c(0x808080))),
        ("markdown.muted", Style::new().fg(c(0x808080))),
        ("markdown.quote", Style::new().fg(c(0x6A9955))),
        ("markdown.rule", Style::new().fg(c(0x6E6E6E))),
        ("markdown.bullet", Style::new().fg(c(0x6796E6))),
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
    ]
}

impl jtui::style::Styles for Theme {
    fn get(&self, key: &str) -> Style {
        self.base.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    /// The token style table matches `get` for every key, and is built once (the same slice each call).
    #[test]
    fn token_styles_match_get_and_are_built_once() {
        let theme = Theme::dark_plus(true);
        let t = theme.token_styles();
        assert_eq!(t.len(), crate::syntax::KEYS.len());
        for (k, st) in crate::syntax::KEYS.iter().zip(t) {
            assert_eq!(theme.find(k), Some(*st), "{k}");
        }
        assert!(std::ptr::eq(t.as_ptr(), theme.token_styles().as_ptr()));
    }

    /// Every highlight key has a colour, distinct in 256-colour mode for the main token kinds.
    #[test]
    fn syntax_keys_are_in_the_theme() {
        for truecolor in [true, false] {
            let theme = Theme::dark_plus(truecolor);
            for key in crate::syntax::KEYS {
                assert!(theme.find(key).and_then(|s| s.fg).is_some(), "{key}");
            }
        }
        let theme = Theme::dark_plus(false);
        let fg = |k: &str| theme.get(k).fg.unwrap();
        assert_eq!(fg("keyword"), Color::Indexed(jtui::theme::to_ansi256(0x56, 0x9C, 0xD6)));
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

    /// `status_bar = "auto"` has 120 distinct, dark backgrounds with white text.
    #[test]
    fn status_bar_auto_has_120_distinct_dark_colours() {
        let theme = Theme::dark_plus(true);
        let mut seen = std::collections::HashSet::new();
        for slot in 0..STATUS_AUTO_SLOTS {
            let style = theme.status_bar_auto(slot);
            assert_eq!(style.fg, Some(Color::Rgb(0xFF, 0xFF, 0xFF)));
            let Some(Color::Rgb(r, g, b)) = style.bg else {
                panic!("slot {slot} is RGB")
            };
            assert!(r.max(g).max(b) < 0xB0, "slot {slot} stays dark");
            assert!(seen.insert((r, g, b)), "slot {slot} is distinct");
        }
        assert_eq!(
            theme.status_bar_auto(STATUS_AUTO_SLOTS),
            theme.status_bar_auto(0),
            "wraps"
        );
    }

    /// Under 256 colours `status_bar_auto` uses palette indices (the theme's colour depth).
    #[test]
    fn status_bar_auto_uses_the_palette_in_256_colours() {
        let theme = Theme::dark_plus(false);
        assert!(!theme.truecolor());
        for slot in 0..STATUS_AUTO_SLOTS {
            let style = theme.status_bar_auto(slot);
            assert!(matches!(style.bg, Some(Color::Indexed(_))), "slot {slot}");
            assert!(matches!(style.fg, Some(Color::Indexed(_))), "slot {slot}");
        }
    }

    #[test]
    fn squiggles_are_coloured_underlines() {
        for truecolor in [true, false] {
            let theme = Theme::dark_plus(truecolor);
            for s in crate::diagnostics::Severity::ALL {
                let style = theme.get(s.theme_key());
                assert!(style.add_modifier.contains(Modifier::UNDERLINED), "{s:?}");
                assert!(style.underline_color.is_some() && style.fg.is_none() && style.bg.is_none());
            }
        }
    }

    #[test]
    fn spell_check_squiggle_is_its_own_coloured_underline() {
        for truecolor in [true, false] {
            let theme = Theme::dark_plus(truecolor);
            let style = theme.get("editorSpell.foreground");
            assert!(style.add_modifier.contains(Modifier::UNDERLINED));
            assert!(style.underline_color.is_some() && style.fg.is_none() && style.bg.is_none());
            for s in crate::diagnostics::Severity::ALL {
                assert_ne!(style.underline_color, theme.get(s.theme_key()).underline_color, "{s:?}");
            }
        }
    }

    /// Jed's entries and the shared widgets' never share a key, and neither list repeats one.
    #[test]
    fn own_entries_do_not_repeat_the_widget_keys() {
        let widget: Vec<&str> = jtui::theme::widget_entries(true).iter().map(|(k, _)| *k).collect();
        let own = own_entries(true);
        let mut seen = std::collections::HashSet::new();
        for (k, _) in &own {
            assert!(!widget.contains(k), "{k} is a widget key");
            assert!(seen.insert(*k), "{k} is listed twice");
        }
    }

    /// Dark+ defines every key `jterm::draw` reads (a missing one logs `missing theme key`).
    #[test]
    fn dark_plus_defines_the_terminal_drawing_keys() {
        let theme = Theme::dark_plus(true);
        for key in jterm::draw::KEYS {
            assert!(theme.find(key).is_some(), "{key} is missing");
        }
    }
}
