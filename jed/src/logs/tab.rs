//! A log tab's model (no app state; `app/tools.rs` drives it): which tool its buffer shows and
//! how far it has read, and each read turned into one [`ChangeSet`] on the buffer's text — the
//! new lines appended at the end, the lines the ring dropped meanwhile (`missed`, or older than
//! its `first`) deleted from the start, everything after a clear (`reset`) replaced. The text is
//! one line per log line (`14:03:09.123 INFO  text`, the time UTC), each ending in `\n`, so the
//! buffer's last line is the empty one after them: a cursor there follows the tail.

use ropey::Rope;

use super::{Cursor, Line, Read, Tool};
use crate::buffer::change::ChangeSet;
use crate::buffer::pos::{self, ByteIdx};

/// What a log buffer shows (`Buffer::log`).
#[derive(Debug, Clone)]
pub struct LogTab {
    pub tool: Tool,
    /// Where the next read starts.
    cursor: Cursor,
    /// The sequence number of the text's first line (when it has any).
    first: u64,
    /// Lines in the text.
    lines: usize,
}

impl LogTab {
    pub fn new(tool: Tool) -> Self {
        Self {
            tool,
            cursor: Cursor::default(),
            first: 0,
            lines: 0,
        }
    }

    /// Where the next read starts.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The change to `rope` (this tab's text so far) that `read` makes: lines the ring no longer
    /// has deleted from the start, the new ones appended; empty when nothing changed.
    pub fn update(&mut self, rope: &Rope, read: Read) -> ChangeSet {
        let drop = if read.reset {
            self.lines
        } else {
            (read.first.saturating_sub(self.first) as usize).min(self.lines)
        };
        self.cursor = read.cursor;
        self.lines -= drop;
        self.first += drop as u64;
        if self.lines == 0 {
            self.first = read.cursor.next - read.lines.len() as u64;
        }
        self.lines += read.lines.len();
        let mut text = String::new();
        for line in &read.lines {
            format_line(line, &mut text);
        }
        let mut changes = ChangeSet::with_capacity(2, text.len());
        let len = ByteIdx(rope.len());
        if drop > 0 {
            changes.push(ByteIdx(0), pos::line_start(rope, drop), "");
        }
        changes.push(len, len, &text);
        changes
    }
}

/// `line` as a log tab shows it, with its `\n`: `14:03:09.123 INFO  text` (UTC). A CR in the
/// text (a line break to the editor) shows as `␍`.
pub fn format_line(line: &Line, out: &mut String) {
    use std::fmt::Write;
    let ms = line.time % 86_400_000;
    let (h, m, s) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60);
    let _ = write!(out, "{h:02}:{m:02}:{s:02}.{:03} {:<5} ", ms % 1000, line.level.label());
    if line.text.contains('\r') {
        out.push_str(&line.text.replace('\r', "␍"));
    } else {
        out.push_str(&line.text);
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::{Level, Ring};

    fn line(text: &str) -> Line {
        Line {
            time: 3_600_000 + 61_234,
            level: Level::Info,
            text: text.into(),
        }
    }

    fn texts(rope: &Rope) -> Vec<String> {
        rope.to_string()
            .lines()
            .map(|l| l.split_once("INFO  ").map_or(l, |(_, t)| t).to_string())
            .collect()
    }

    /// Reads `ring` into `tab` / `rope` as the app does, returning the change set's size.
    fn sync(tab: &mut LogTab, rope: &mut Rope, ring: &Ring) -> usize {
        let changes = tab.update(rope, ring.read(tab.cursor()));
        let n = changes.len();
        changes.apply(rope);
        n
    }

    #[test]
    fn lines_are_formatted_with_time_and_level() {
        let mut out = String::new();
        format_line(&line("hello"), &mut out);
        format_line(
            &Line {
                time: 0,
                level: Level::Error,
                text: "a\rb".into(),
            },
            &mut out,
        );
        assert_eq!(out, "01:01:01.234 INFO  hello\n00:00:00.000 ERROR a␍b\n");
    }

    /// New lines are appended in one change; nothing new is no change.
    #[test]
    fn appends_new_lines() {
        let mut ring = Ring::with_caps(100, 1 << 20);
        let (mut tab, mut rope) = (LogTab::new(Tool::Jed), Rope::new());
        assert_eq!(sync(&mut tab, &mut rope, &ring), 0);
        ring.push(line("a"));
        ring.push(line("b"));
        assert_eq!(sync(&mut tab, &mut rope, &ring), 1);
        ring.push(line("c"));
        assert_eq!(sync(&mut tab, &mut rope, &ring), 1);
        assert_eq!(texts(&rope), ["a", "b", "c"]);
        assert_eq!(sync(&mut tab, &mut rope, &ring), 0);
    }

    /// Lines the ring drops leave the text too, in the same change set as the new ones, so the
    /// text holds what the ring does.
    #[test]
    fn dropped_lines_leave_the_text() {
        let mut ring = Ring::with_caps(3, 1 << 20);
        let (mut tab, mut rope) = (LogTab::new(Tool::Jed), Rope::new());
        for t in ["a", "b", "c"] {
            ring.push(line(t));
        }
        sync(&mut tab, &mut rope, &ring);
        ring.push(line("d"));
        assert_eq!(sync(&mut tab, &mut rope, &ring), 2, "delete + append");
        assert_eq!(texts(&rope), ["b", "c", "d"]);
        // More dropped than read (`missed`): everything old goes.
        for t in ["e", "f", "g", "h", "i"] {
            ring.push(line(t));
        }
        sync(&mut tab, &mut rope, &ring);
        assert_eq!(texts(&rope), ["g", "h", "i"]);
        // Dropped with nothing new (a byte cap would): still trimmed.
        ring.push(line("j"));
        sync(&mut tab, &mut rope, &ring);
        assert_eq!(texts(&rope), ["h", "i", "j"]);
    }

    /// A clear empties the text; lines after it start it again.
    #[test]
    fn a_clear_resets_the_text() {
        let mut ring = Ring::with_caps(10, 1 << 20);
        let (mut tab, mut rope) = (LogTab::new(Tool::Jed), Rope::new());
        ring.push(line("a"));
        sync(&mut tab, &mut rope, &ring);
        ring.clear();
        sync(&mut tab, &mut rope, &ring);
        assert_eq!(rope.to_string(), "");
        ring.push(line("b"));
        ring.clear();
        ring.push(line("c"));
        sync(&mut tab, &mut rope, &ring);
        assert_eq!(texts(&rope), ["c"]);
        ring.push(line("d"));
        sync(&mut tab, &mut rope, &ring);
        assert_eq!(texts(&rope), ["c", "d"]);
    }
}
