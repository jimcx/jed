//! A ratatui `Backend` that writes crossterm's commands into a byte buffer instead of a tty.
//!
//! The size comes from the caller ([`StreamBackend::set_size`]), never from the terminal the process
//! runs on: a server draws for a remote client's screen through it, then sends [`take_output`]'s
//! bytes. Cursor queries answer with the last position set.
//!
//! [`take_output`]: StreamBackend::take_output

use std::io;
use std::sync::{Arc, Mutex};

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// The buffer the backend writes into, shared so the backend can drain it (the writer accessors of
/// `CrosstermBackend` are unstable).
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl io::Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct StreamBackend {
    inner: CrosstermBackend<Shared>,
    out: Shared,
    size: Size,
    cursor: Position,
}

impl StreamBackend {
    pub fn new(cols: u16, rows: u16) -> Self {
        let out = Shared::default();
        StreamBackend {
            inner: CrosstermBackend::new(out.clone()),
            out,
            size: Size {
                width: cols,
                height: rows,
            },
            cursor: Position::ORIGIN,
        }
    }

    /// The size `size()` reports from now on (a `Terminal` picks it up at its next `autoresize`).
    pub fn set_size(&mut self, cols: u16, rows: u16) {
        self.size = Size {
            width: cols,
            height: rows,
        };
    }

    /// Everything written since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut *self.out.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Bytes written and not yet taken.
    pub fn pending(&self) -> usize {
        self.out.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

impl Backend for StreamBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let p = position.into();
        self.cursor = p;
        self.inner.set_cursor_position(p)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn size(&self) -> io::Result<Size> {
        Ok(self.size)
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size { width: 0, height: 0 },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::widgets::Paragraph;

    /// The screen a `vt100` terminal of this size shows after `bytes`.
    fn screen(bytes: &[u8], rows: u16, cols: u16) -> String {
        let mut p = vt100::Parser::new(rows.try_into().unwrap(), cols.try_into().unwrap(), 0);
        p.process(bytes);
        p.screen().contents()
    }

    fn draw(t: &mut Terminal<StreamBackend>, text: &'static str) {
        t.draw(|f| f.render_widget(Paragraph::new(text), f.area())).unwrap();
    }

    #[test]
    fn draws_text_and_positions() {
        let mut t = Terminal::new(StreamBackend::new(20, 5)).unwrap();
        draw(&mut t, "hello\nworld");
        let out = t.backend_mut().take_output();
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("hello") && s.contains("world"));
        assert!(s.contains("\x1b[2;1H"), "{s:?}");
        assert_eq!(screen(&out, 5, 20), "hello\nworld");
    }

    #[test]
    fn second_identical_draw_emits_no_text() {
        let mut t = Terminal::new(StreamBackend::new(20, 5)).unwrap();
        draw(&mut t, "hello");
        assert!(t.backend().pending() > 0);
        t.backend_mut().take_output();
        draw(&mut t, "hello");
        let out = t.backend_mut().take_output();
        assert!(!String::from_utf8_lossy(&out).contains("hello"));
        draw(&mut t, "howdy");
        let out = t.backend_mut().take_output();
        assert!(String::from_utf8_lossy(&out).contains("owdy"));
    }

    #[test]
    fn size_follows_set_size_and_resize_redraws() {
        let mut t = Terminal::new(StreamBackend::new(20, 5)).unwrap();
        assert_eq!(t.size().unwrap(), Size { width: 20, height: 5 });
        draw(&mut t, "hi");
        let first = t.backend_mut().take_output();
        t.backend_mut().set_size(10, 3);
        t.autoresize().unwrap();
        assert_eq!(t.size().unwrap(), Size { width: 10, height: 3 });
        t.draw(|f| {
            assert_eq!((f.area().width, f.area().height), (10, 3));
            f.render_widget(Paragraph::new("hi"), f.area());
        })
        .unwrap();
        let second = t.backend_mut().take_output();
        // A client-side screen fed both sequences shows the text at the new size.
        let mut all = first;
        all.extend(second);
        assert_eq!(screen(&all, 3, 10), "hi");
        let ws = t.backend_mut().window_size().unwrap();
        assert_eq!(ws.columns_rows, Size { width: 10, height: 3 });
    }

    #[test]
    fn take_output_empties_and_cursor_is_remembered() {
        let mut t = Terminal::new(StreamBackend::new(20, 5)).unwrap();
        draw(&mut t, "x");
        assert!(!t.backend_mut().take_output().is_empty());
        assert!(t.backend_mut().take_output().is_empty());
        t.set_cursor_position((3, 2)).unwrap();
        assert_eq!(t.get_cursor_position().unwrap(), Position { x: 3, y: 2 });
        let out = t.backend_mut().take_output();
        assert!(String::from_utf8_lossy(&out).contains("\x1b[3;4H"));
    }

    #[test]
    fn cursor_show_hide_and_clear_reach_the_stream() {
        let mut t = Terminal::new(StreamBackend::new(10, 3)).unwrap();
        t.backend_mut().take_output();
        t.hide_cursor().unwrap();
        assert_eq!(t.backend_mut().take_output(), b"\x1b[?25l");
        t.show_cursor().unwrap();
        assert_eq!(t.backend_mut().take_output(), b"\x1b[?25h");
        draw(&mut t, "abc");
        let first = t.backend_mut().take_output();
        t.clear().unwrap();
        let cleared = t.backend_mut().take_output();
        assert!(!cleared.is_empty());
        let mut all = first;
        all.extend(cleared);
        assert_eq!(screen(&all, 3, 10), "");
    }

    #[test]
    fn size_never_comes_from_the_tty() {
        // A size no test runner's terminal has; both queries answer it, and a fresh backend's cursor is the origin.
        let mut b = StreamBackend::new(317, 91);
        assert_eq!(b.size().unwrap(), Size { width: 317, height: 91 });
        let ws = b.window_size().unwrap();
        assert_eq!(ws.columns_rows, Size { width: 317, height: 91 });
        assert_eq!(b.get_cursor_position().unwrap(), Position::ORIGIN);
    }
}
