//! The commands side bar's model: the session's command history as one list, fed from every
//! terminal's [`CommandRecord`]s (`App::run_event` calls [`History::update`]), with no drawing and
//! no app state.
//!
//! Order: running commands first (newest start first), then finished ones, newest finish first.
//! The list holds at most `cap` rows: the oldest finished ones go first, a running one never does.
//! A record that fell off its run's ring stays here. [`History::close`] marks a terminal's rows
//! `closed` (they stay, dimmed); a command still running then is taken as ended at that moment with
//! no exit status (glyph `·`). The filter is a case-insensitive substring of the command line.
//! The selection is kept by identity, so rows arriving above it don't move it; the scroll is the
//! index of the first visible row among [`History::visible`]. Row text (glyph, duration, age) is
//! pure functions of the record and the current time.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use jterm::run::{CommandRecord, RunId};

/// One command of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub run: RunId,
    pub record: CommandRecord,
    /// The terminal's tab title (without a state glyph), kept current while it is open.
    pub title: String,
    /// The terminal has gone: the row does nothing and is drawn dimmed.
    pub closed: bool,
}

/// A row's identity: its terminal and the record's id in that run.
pub type RowKey = (RunId, u64);

impl Row {
    pub fn key(&self) -> RowKey {
        (self.run, self.record.id)
    }

    pub fn running(&self) -> bool {
        self.record.finished.is_none()
    }

    /// The command line, or a stand-in when the shell's snippet didn't send it.
    pub fn label(&self) -> &str {
        self.record.command.as_deref().unwrap_or("(unknown command)")
    }

    /// `●` running, `✓` exit 0, `✗` another exit, `·` ended without a status (its terminal closed).
    pub fn glyph(&self) -> char {
        match (self.record.finished, self.record.exit) {
            (None, _) => '●',
            (Some(_), Some(0)) => '✓',
            (Some(_), Some(_)) => '✗',
            (Some(_), None) => '·',
        }
    }

    /// How long it ran, or has been running (`3.2s`, `1m 05s`, `2h 03m`).
    pub fn duration_text(&self, now: Instant) -> String {
        let end = self.record.finished.unwrap_or(now);
        duration_text(end.saturating_duration_since(self.record.started))
    }

    /// How long ago it ended (`12s ago`); `None` while it runs.
    pub fn age_text(&self, now: Instant) -> Option<String> {
        let finished = self.record.finished?;
        Some(age_text(now.saturating_duration_since(finished)))
    }
}

/// `3.2s` under a minute, `1m 05s` under an hour, else `2h 03m`.
pub fn duration_text(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        // Tenths, truncated: `59.99s` is `59.9s`, never `60.0s`.
        format!("{}.{}s", secs, d.subsec_millis() / 100)
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, secs % 3600 / 60)
    }
}

/// `12s ago`, `5m ago`, `3h ago`, `2d ago`.
pub fn age_text(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

#[derive(Debug)]
pub struct History {
    /// Every row, in list order (running first, then finished newest first).
    rows: Vec<Row>,
    /// The most rows kept (`command_history`).
    cap: usize,
    filter: String,
    selected: Option<RowKey>,
    /// The index among the visible rows of the first one shown.
    scroll: usize,
}

impl History {
    pub fn new(cap: usize) -> History {
        History {
            rows: Vec::new(),
            cap,
            filter: String::new(),
            selected: None,
            scroll: 0,
        }
    }

    /// Takes a run's current records (oldest first, `Run::commands`): new ones are added, known
    /// ones updated in place, and every row of the run gets `title`.
    pub fn update(&mut self, run: RunId, title: &str, records: Vec<CommandRecord>) {
        let mut index: HashMap<u64, usize> = HashMap::new();
        for (i, row) in self.rows.iter_mut().enumerate() {
            if row.run == run {
                if row.title != title {
                    row.title = title.to_string();
                }
                index.insert(row.record.id, i);
            }
        }
        let mut changed = false;
        for record in records {
            match index.get(&record.id) {
                Some(&i) => {
                    if self.rows[i].record != record {
                        self.rows[i].record = record;
                        changed = true;
                    }
                }
                None => {
                    self.rows.push(Row {
                        run,
                        record,
                        title: title.to_string(),
                        closed: false,
                    });
                    changed = true;
                }
            }
        }
        if changed {
            self.reorder();
        }
    }

    /// The run's terminal has closed: its rows stay, marked; one still running is ended now with
    /// no exit status.
    pub fn close(&mut self, run: RunId, now: Instant) {
        for row in self.rows.iter_mut().filter(|r| r.run == run) {
            row.closed = true;
            if row.record.finished.is_none() {
                row.record.finished = Some(now);
            }
        }
        self.reorder();
    }

    /// Sorts, applies the cap and keeps the selection and scroll in range.
    fn reorder(&mut self) {
        self.rows.sort_by(|a, b| {
            let (ra, rb) = (a.record.finished, b.record.finished);
            // Running first (newest start first), then newest finish first; ties by identity.
            ra.is_some()
                .cmp(&rb.is_some())
                .then_with(|| match (ra, rb) {
                    (Some(x), Some(y)) => y.cmp(&x),
                    _ => b.record.started.cmp(&a.record.started),
                })
                .then_with(|| b.key().cmp(&a.key()))
        });
        while self.rows.len() > self.cap
            && let Some(last) = self.rows.last()
            && !last.running()
        {
            self.rows.pop();
        }
        self.clamp();
    }

    /// Every row, in list order.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Sets the filter; the selection stays on its row when that still shows, else moves to the first.
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.clamp();
    }

    /// The rows the filter lets through, in list order.
    pub fn visible(&self) -> Vec<&Row> {
        if self.filter.is_empty() {
            return self.rows.iter().collect();
        }
        let needle = self.filter.to_lowercase();
        self.rows
            .iter()
            .filter(|r| {
                r.record
                    .command
                    .as_ref()
                    .is_some_and(|c| c.to_lowercase().contains(&needle))
            })
            .collect()
    }

    /// The selected row, when it shows.
    pub fn selected(&self) -> Option<&Row> {
        let key = self.selected?;
        self.visible().into_iter().find(|r| r.key() == key)
    }

    /// The selected row's place among the visible ones.
    pub fn selected_index(&self) -> Option<usize> {
        let key = self.selected?;
        self.visible().iter().position(|r| r.key() == key)
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Selects the `index`th visible row (clamped; none when nothing shows) and scrolls it into
    /// a view of `height` rows.
    pub fn select_index(&mut self, index: usize, height: usize) {
        let visible = self.visible();
        let n = visible.len();
        self.selected = visible.get(index.min(n.saturating_sub(1))).map(|r| r.key());
        self.scroll_into_view(height, n);
    }

    /// Moves the selection by `delta` rows (none selected: from before the first for a step
    /// down, from after the last for a step up), stopping at the ends.
    pub fn move_selection(&mut self, delta: isize, height: usize) {
        let n = self.visible().len();
        if n == 0 {
            return;
        }
        let from = self
            .selected_index()
            .map_or(if delta >= 0 { -1 } else { n as isize }, |i| i as isize);
        let to = (from + delta).clamp(0, n as isize - 1);
        self.select_index(to as usize, height);
    }

    pub fn select_first(&mut self, height: usize) {
        self.select_index(0, height);
    }

    pub fn select_last(&mut self, height: usize) {
        self.select_index(usize::MAX, height);
    }

    /// Scrolls the view by `delta` rows without moving the selection (the wheel).
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        let max = self.visible().len().saturating_sub(height);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    /// Scrolls so the selected row is inside a view of `height` rows.
    pub fn reveal_selected(&mut self, height: usize) {
        let n = self.visible().len();
        self.scroll_into_view(height, n);
    }

    fn scroll_into_view(&mut self, height: usize, n: usize) {
        if let Some(i) = self.selected_index() {
            if i < self.scroll {
                self.scroll = i;
            } else if height > 0 && i >= self.scroll + height {
                self.scroll = i + 1 - height;
            }
        }
        self.scroll = self.scroll.min(n.saturating_sub(height.max(1)));
    }

    /// After the rows or the filter changed: the selection falls back to the first visible row
    /// when its own is gone, the scroll stays in range.
    fn clamp(&mut self) {
        let visible = self.visible();
        let n = visible.len();
        if self.selected.is_some_and(|k| !visible.iter().any(|r| r.key() == k)) {
            self.selected = visible.first().map(|r| r.key());
        }
        self.scroll = self.scroll.min(n.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record that started `start` seconds after `base`, finished `end` seconds after it (None: running).
    fn rec(base: Instant, id: u64, cmd: &str, start: u64, end: Option<(u64, i32)>) -> CommandRecord {
        CommandRecord {
            id,
            command: Some(cmd.to_string()),
            cwd: None,
            started: base + Duration::from_secs(start),
            started_at: std::time::SystemTime::UNIX_EPOCH,
            finished: end.map(|(e, _)| base + Duration::from_secs(e)),
            exit: end.map(|(_, x)| x),
            start_row: 0,
            end_row: 0,
        }
    }

    fn labels(h: &History) -> Vec<String> {
        h.visible().iter().map(|r| r.label().to_string()).collect()
    }

    #[test]
    fn running_first_then_finished_newest_first_across_runs() {
        let b = Instant::now();
        let mut h = History::new(100);
        h.update(
            RunId(1),
            "sh",
            vec![
                rec(b, 1, "a", 0, Some((1, 0))),
                rec(b, 2, "run1", 10, None),
                rec(b, 3, "c", 5, Some((8, 0))),
            ],
        );
        h.update(
            RunId(2),
            "zsh",
            vec![rec(b, 1, "d", 6, Some((7, 1))), rec(b, 2, "run2", 12, None)],
        );
        assert_eq!(labels(&h), ["run2", "run1", "c", "d", "a"]);
    }

    #[test]
    fn update_merges_in_place_and_keeps_records_that_left_the_ring() {
        let b = Instant::now();
        let mut h = History::new(100);
        h.update(
            RunId(1),
            "sh",
            vec![rec(b, 1, "old", 0, Some((1, 0))), rec(b, 2, "sleep", 2, None)],
        );
        assert!(h.rows()[0].running());
        // The ring dropped `old`; `sleep` finished.
        h.update(RunId(1), "sh (2)", vec![rec(b, 2, "sleep", 2, Some((7, 0)))]);
        assert_eq!(h.rows().len(), 2);
        assert_eq!(labels(&h), ["sleep", "old"]);
        assert!(!h.rows()[0].running());
        assert!(h.rows().iter().all(|r| r.title == "sh (2)"));
    }

    #[test]
    fn the_cap_drops_the_oldest_finished_and_never_a_running_row() {
        let b = Instant::now();
        let mut h = History::new(3);
        let mut recs: Vec<_> = (1..=5)
            .map(|i| rec(b, i, &format!("c{i}"), i, Some((i + 1, 0))))
            .collect();
        recs.push(rec(b, 6, "live", 0, None));
        h.update(RunId(1), "sh", recs);
        assert_eq!(labels(&h), ["live", "c5", "c4"]);
        let mut small = History::new(1);
        small.update(RunId(1), "sh", vec![rec(b, 1, "x", 0, None), rec(b, 2, "y", 1, None)]);
        assert_eq!(small.rows().len(), 2, "running rows are never dropped");
    }

    #[test]
    fn closing_a_run_marks_its_rows_and_ends_a_running_one() {
        let b = Instant::now();
        let mut h = History::new(100);
        h.update(
            RunId(1),
            "sh",
            vec![rec(b, 1, "done", 0, Some((1, 0))), rec(b, 2, "hang", 2, None)],
        );
        h.update(RunId(2), "sh (2)", vec![rec(b, 1, "other", 3, Some((4, 0)))]);
        h.close(RunId(1), b + Duration::from_secs(9));
        let r = |l: &str| h.rows().iter().find(|r| r.label() == l).unwrap().clone();
        assert!(r("done").closed && r("hang").closed && !r("other").closed);
        assert!(!r("hang").running());
        assert_eq!(r("hang").glyph(), '·');
        assert_eq!(r("done").glyph(), '✓');
        assert_eq!(labels(&h), ["hang", "other", "done"]);
    }

    #[test]
    fn the_filter_is_a_case_insensitive_substring_of_the_command() {
        let b = Instant::now();
        let mut h = History::new(100);
        let mut unknown = rec(b, 3, "", 3, Some((4, 0)));
        unknown.command = None;
        h.update(
            RunId(1),
            "sh",
            vec![
                rec(b, 1, "cargo Build", 0, Some((1, 0))),
                rec(b, 2, "ls", 1, Some((2, 0))),
                unknown,
            ],
        );
        h.set_filter("BUILD");
        assert_eq!(labels(&h), ["cargo Build"]);
        h.set_filter("");
        assert_eq!(h.visible().len(), 3);
        h.set_filter("zzz");
        assert!(h.visible().is_empty() && h.selected().is_none());
    }

    #[test]
    fn the_selection_stays_on_its_row_when_rows_arrive_above_it() {
        let b = Instant::now();
        let mut h = History::new(100);
        h.update(
            RunId(1),
            "sh",
            vec![rec(b, 1, "a", 0, Some((1, 0))), rec(b, 2, "b", 2, Some((3, 0)))],
        );
        h.select_index(1, 10);
        assert_eq!(h.selected().unwrap().label(), "a");
        h.update(RunId(1), "sh", vec![rec(b, 3, "c", 4, Some((5, 0)))]);
        assert_eq!(h.selected().unwrap().label(), "a");
        assert_eq!(h.selected_index(), Some(2));
        // Its row filtered out: the first visible takes over.
        h.set_filter("c");
        assert_eq!(h.selected().unwrap().label(), "c");
    }

    #[test]
    fn moving_and_scrolling_clamp_and_keep_the_selection_in_view() {
        let b = Instant::now();
        let mut h = History::new(100);
        let recs = (1..=10).map(|i| rec(b, i, &format!("c{i}"), i, Some((i, 0)))).collect();
        h.update(RunId(1), "sh", recs);
        h.move_selection(1, 3);
        assert_eq!(h.selected_index(), Some(0));
        h.move_selection(-1, 3);
        assert_eq!(h.selected_index(), Some(0));
        h.move_selection(5, 3);
        assert_eq!((h.selected_index(), h.scroll()), (Some(5), 3));
        h.select_last(3);
        assert_eq!((h.selected_index(), h.scroll()), (Some(9), 7));
        h.select_first(3);
        assert_eq!((h.selected_index(), h.scroll()), (Some(0), 0));
        h.scroll_by(100, 3);
        assert_eq!(h.scroll(), 7);
        h.scroll_by(-100, 3);
        assert_eq!(h.scroll(), 0);
        // Fewer rows than the view: no scrolling.
        h.set_filter("c10");
        assert_eq!(h.scroll(), 0);
        // Nothing shows: moving does nothing (and doesn't panic).
        h.set_filter("zzz");
        h.move_selection(1, 3);
        h.move_selection(-1, 3);
        h.select_last(3);
        assert_eq!(h.selected_index(), None);
    }

    #[test]
    fn row_text_formats_durations_and_ages() {
        let s = Duration::from_secs;
        assert_eq!(duration_text(Duration::from_millis(3250)), "3.2s");
        assert_eq!(duration_text(Duration::from_millis(59_990)), "59.9s");
        assert_eq!(duration_text(s(65)), "1m 05s");
        assert_eq!(duration_text(s(2 * 3600 + 180)), "2h 03m");
        assert_eq!(age_text(s(12)), "12s ago");
        assert_eq!(age_text(s(300)), "5m ago");
        assert_eq!(age_text(s(3 * 3600)), "3h ago");
        assert_eq!(age_text(s(2 * 86_400 + 5)), "2d ago");
        let b = Instant::now();
        let mut h = History::new(10);
        h.update(
            RunId(1),
            "sh",
            vec![rec(b, 1, "x", 0, Some((5, 2))), rec(b, 2, "y", 6, None)],
        );
        let now = b + Duration::from_secs(20);
        let (y, x) = (&h.rows()[0], &h.rows()[1]);
        assert_eq!(
            (y.glyph(), y.duration_text(now), y.age_text(now)),
            ('●', "14.0s".into(), None)
        );
        assert_eq!(
            (x.glyph(), x.duration_text(now), x.age_text(now)),
            ('✗', "5.0s".into(), Some("15s ago".into()))
        );
    }
}
