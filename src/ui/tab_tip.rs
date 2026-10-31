//! The tab tooltip (`app.tab_tip.shown`): one line, the file's path, under its tab (the hover
//! popup's colours, `editorHoverWidget.*`), moved left to stay inside the screen and cut at its
//! edge.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::layout::GroupHit;

pub fn render(f: &mut Frame, app: &App, groups: &[GroupHit]) {
    let Some(shown) = &app.tab_tip.shown else { return };
    let Some(g) = groups.iter().find(|g| g.id == shown.group) else {
        return;
    };
    let Some(tab) = g.tabs.iter().find(|t| t.index == shown.index) else {
        return;
    };
    let screen = f.area();
    let y = g.tab_bar.y + 1;
    if y >= screen.bottom() || screen.width < 3 {
        return;
    }
    let width = (shown.text.width() as u16 + 2).min(screen.width);
    let x = tab.x0.min(screen.right().saturating_sub(width)).max(screen.x);
    let rect = Rect { x, y, width, height: 1 };
    let theme = &app.theme;
    let style = theme
        .get("editorHoverWidget.background")
        .patch(theme.get("editorHoverWidget.foreground"));
    let out = f.buffer_mut();
    out.set_style(rect, style);
    let line = format!(" {} ", shown.text);
    out.set_stringn(
        rect.x,
        y,
        format!("{line:<w$}", w = width as usize),
        width as usize,
        style,
    );
}
