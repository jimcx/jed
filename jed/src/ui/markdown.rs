//! A Markdown preview tab (`markdown_tab::MarkdownTab`): fitted to the area first (re-rendered
//! when the width changed), then only the visible rows drawn from the `markdown::Doc`: each span
//! in `Theme::markdown_style` over the `editor` base, code lines filling the whole row on the
//! `markdown.codeblock` background, scrolled sideways by `left` when a table is wider than the
//! view, the selection as `editor.selection` and the caret as the terminal cursor (focused group
//! only).

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthChar;

use crate::markdown::Kind;
use crate::markdown_tab::MarkdownTab;
use crate::theme::Theme;

/// Draws `tab` in `area`; returns the rows' rect (`GroupHit::text`, for the mouse).
pub fn render(f: &mut Frame, area: Rect, tab: &mut MarkdownTab, theme: &Theme, focused: bool) -> Rect {
    tab.fit(area.width as usize, area.height as usize);
    let base = theme.get("editor");
    let selection = theme.get("editor.selection");
    let code_bg = base.patch(theme.get("markdown.codeblock"));
    let out = f.buffer_mut();
    out.set_style(area, base);
    let right = tab.left + area.width as usize;
    for screen in 0..area.height as usize {
        let row = tab.top + screen;
        let Some(line) = tab.doc.lines.get(row) else { break };
        let y = area.y + screen as u16;
        if line.kind == Kind::Code {
            out.set_style(Rect::new(area.x, y, area.width, 1), code_bg);
        }
        // Columns scrolled out on the left are skipped, a wide character cut by an edge too.
        // A zero-width character (a combining mark) joins the cell drawn before it.
        let mut col = 0usize;
        let mut last = None;
        for span in &line.spans {
            let style = base.patch(theme.markdown_style(&span.style));
            for ch in span.text.chars() {
                let w = ch.width().unwrap_or(0);
                if w > 0 && col >= tab.left && col + w <= right {
                    let x = area.x + (col - tab.left) as u16;
                    let mut buf = [0u8; 4];
                    out.set_string(x, y, ch.encode_utf8(&mut buf), style);
                    last = Some(x);
                } else if w > 0 {
                    last = None;
                } else if let Some(x) = last
                    && !ch.is_control()
                {
                    let cell = &mut out[(x, y)];
                    let joined = format!("{}{ch}", cell.symbol());
                    cell.set_symbol(&joined);
                }
                col += w;
            }
        }
        if let Some(sel) = tab.selected_cols(row) {
            let (from, to) = (sel.start.max(tab.left), sel.end.min(right));
            if from < to {
                let rect = Rect::new(area.x + (from - tab.left) as u16, y, (to - from) as u16, 1);
                out.set_style(rect, selection);
            }
        }
    }
    let (l, c) = tab.cursor;
    if focused && (tab.top..tab.top + area.height as usize).contains(&l) && (tab.left..right).contains(&c) {
        f.set_cursor_position(Position::new(
            area.x + (c - tab.left) as u16,
            area.y + (l - tab.top) as u16,
        ));
    }
    area
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::command::Motion;

    fn draw(tab: &mut MarkdownTab, area: Rect, focused: bool) -> Terminal<TestBackend> {
        let theme = Theme::dark_plus(true);
        let mut term = Terminal::new(TestBackend::new(30, 8)).unwrap();
        term.draw(|f| {
            render(f, area, tab, &theme, focused);
        })
        .unwrap();
        term
    }

    fn row(term: &Terminal<TestBackend>, y: u16) -> Vec<String> {
        let buf = term.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()
    }

    fn tab(text: &str) -> MarkdownTab {
        MarkdownTab::new(1, None, "x.md".into(), text.into(), 0)
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        let mut t = tab("# Title\n\n| a | b |\n|---|---|\n| \u{4e2d} | 2 |\n\n```rust\nfn x() {}\n```\n");
        t.select_all();
        for w in 0..4 {
            for h in 0..4 {
                draw(&mut t, Rect::new(1, 1, w, h), true);
                t.scroll_cols(5);
                t.scroll_by(3);
                t.motion(Motion::DocEnd, true);
                t.click(5, 5, true);
            }
        }
    }

    #[test]
    fn rows_past_the_end_are_blank_and_wide_chars_clip_whole() {
        let mut t = tab("a\u{4e2d}b\n");
        let term = draw(&mut t, Rect::new(0, 0, 30, 8), true);
        // (The wide character's second cell holds a blank.)
        assert_eq!(row(&term, 0)[..4].concat(), "a\u{4e2d} b");
        assert!((1..8).all(|y| row(&term, y).iter().all(|s| s == " ")));
        // A line wider than a 2-column view (laid out at 2 first, then widened by hand).
        draw(&mut t, Rect::new(0, 0, 2, 8), true);
        t.doc.lines.truncate(1);
        t.doc.lines[0].spans.truncate(1);
        t.doc.lines[0].spans[0].text = format!("a\u{4e2d}b{}", "x".repeat(40));
        // Cut by the right edge: not drawn at all.
        let term = draw(&mut t, Rect::new(0, 0, 2, 8), true);
        assert_eq!(row(&term, 0)[..3].concat(), "a  ");
        // Starting at the left edge: drawn whole.
        t.left = 1;
        let term = draw(&mut t, Rect::new(0, 0, 2, 8), true);
        assert_eq!(t.left, 1);
        assert_eq!(row(&term, 0)[0], "\u{4e2d}");
        // Cut by the left edge: its second half stays blank.
        t.left = 2;
        let term = draw(&mut t, Rect::new(0, 0, 2, 8), true);
        assert_eq!(row(&term, 0)[..2].concat(), " b", "half a wide char is never drawn");
        t.left = 3;
        let term = draw(&mut t, Rect::new(0, 0, 2, 8), true);
        assert_eq!(row(&term, 0)[..2].concat(), "bx");
    }

    #[test]
    fn combining_marks_join_their_cell() {
        let mut t = tab("e\u{301}x\n");
        let term = draw(&mut t, Rect::new(0, 0, 30, 2), true);
        assert_eq!(row(&term, 0)[0], "e\u{301}");
        assert_eq!(row(&term, 0)[1], "x");
    }

    #[test]
    fn selection_is_highlighted_past_line_ends() {
        let mut t = tab("ab\n\ncd\n");
        t.motion(Motion::Right, true);
        let sel = Theme::dark_plus(true).get("editor.selection").bg.unwrap();
        let term = draw(&mut t, Rect::new(0, 0, 30, 4), true);
        let buf = term.backend().buffer();
        assert_eq!(buf[(0, 0)].bg, sel);
        assert_ne!(buf[(1, 0)].bg, sel);
        let mut term = term;
        assert_eq!(term.get_cursor_position().unwrap(), Position::new(1, 0));
        t.select_all();
        let term = draw(&mut t, Rect::new(0, 0, 30, 4), false);
        let buf = term.backend().buffer();
        // "ab" and the line break after it, the blank line's break, "cd" up to the caret.
        assert!((0..3).all(|x| buf[(x, 0)].bg == sel));
        assert_ne!(buf[(3, 0)].bg, sel);
        assert_eq!(buf[(0, 1)].bg, sel);
        assert_eq!(buf[(1, 2)].bg, sel);
        assert_ne!(buf[(2, 2)].bg, sel);
    }
}
