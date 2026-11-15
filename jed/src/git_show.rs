//! `git show --stat --format=medium` as lines: the model's rendering (no ratatui, no theme —
//! `git_show::render` returns plain text and a kind per segment, which the commit tab colours
//! like git).
//!
//! The result is what git prints at `--stat-width=<width>`, line by line (without ANSI colours):
//! the commit id, the `Merge:` line for a merge, `Author:` and `Date:` (the author date, in the
//! commit's own offset), a blank line, the message indented four spaces (blank lines keep the
//! indent, leading blank lines dropped, the trailing ones trimmed), then — when the commit has
//! changes — a blank line and the stat: ` name | count +++---` with git's graph scaling
//! (`diff.c`'s `show_stats`), `Bin <old> -> <new> bytes` for binaries, git's `pprint_rename`
//! brace form for renames, and git's ` N files changed, X insertions(+), Y deletions(-)`.
//!
//! Deviations: a name is not C-quoted (only names with control characters or `"`/`\` differ);
//! the truncation of a too-long name follows git's columns-then-bytes arithmetic only for valid
//! UTF-8 around a `/` (which is all the tests compare).

use unicode_width::UnicodeWidthStr;

use crate::git::show::{Content, File, Show};

/// What a segment of a line is, for colouring like git's `log` colours (the commit tab's theme
/// keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Anything git prints in the default colour.
    Plain,
    /// The commit's id (yellow).
    Commit,
    /// The `Merge:` line.
    Merge,
    /// The author's line.
    Author,
    /// The date's line.
    Date,
    /// A message line (its four-space indent included).
    Message,
    /// A stat line's ` name | ` part.
    StatName,
    /// A stat line's change count, or `Bin`.
    StatCount,
    /// The `+` graph (green).
    StatAdd,
    /// The `-` graph (red), or a binary's old size.
    StatDel,
    /// The ` N files changed, …` line.
    Summary,
}

/// A piece of a line in one kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub kind: Kind,
}

impl Segment {
    fn new(text: impl Into<String>, kind: Kind) -> Segment {
        Segment {
            text: text.into(),
            kind,
        }
    }
}

/// One printed line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub segments: Vec<Segment>,
    /// The index into [`Show::files`] of the stat line this is, if it is one (the commit tab
    /// opens that file's change; `None` for the header, message, blank and summary lines).
    pub file: Option<usize>,
}

impl Line {
    fn new(text: impl Into<String>, kind: Kind) -> Line {
        Line {
            segments: vec![Segment::new(text, kind)],
            file: None,
        }
    }

    /// The line's text (joining its segments).
    pub fn text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }

    /// The first segment's kind (`Plain` for an empty line).
    #[cfg_attr(not(test), allow(dead_code))] // its tests read the kinds
    pub fn kind(&self) -> Kind {
        self.segments.first().map_or(Kind::Plain, |s| s.kind)
    }
}

/// `show` as `git show --stat --format=medium --stat-width=<width>` prints it: each line without
/// its line break.
pub fn render(show: &Show, width: usize) -> Vec<Line> {
    let mut lines = vec![Line {
        segments: vec![
            Segment::new("commit ", Kind::Plain),
            Segment::new(show.id.clone(), Kind::Commit),
        ],
        file: None,
    }];
    if show.parents.len() > 1 {
        lines.push(Line::new(format!("Merge: {}", show.parents.join(" ")), Kind::Merge));
    }
    lines.push(Line::new(
        format!("Author: {} <{}>", show.author, show.email),
        Kind::Author,
    ));
    lines.push(Line::new(
        format!("Date:   {}", date(show.time, show.offset)),
        Kind::Date,
    ));
    let message = message_lines(&show.message);
    if !message.is_empty() {
        lines.push(Line::default());
        lines.extend(message.into_iter().map(|l| Line::new(l, Kind::Message)));
    }
    if !show.files.is_empty() {
        lines.push(Line::default());
        lines.extend(stat_lines(show, width));
    }
    lines
}

/// `git log`'s default date (`DATE_NORMAL`): `Sun Oct 18 12:54:46 2026 +1100`, the commit's own
/// offset.
fn date(secs: i64, offset_minutes: i32) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let local = secs + i64::from(offset_minutes) * 60;
    let days = local.div_euclid(86_400);
    let rest = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday (index 4 with Sunday first).
    let weekday = (days + 4).rem_euclid(7) as usize;
    let offset = offset_minutes.abs();
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    format!(
        "{} {} {} {:02}:{:02}:{:02} {year} {sign}{:02}{:02}",
        WEEKDAYS[weekday],
        MONTHS[(month - 1) as usize],
        day,
        rest / 3600,
        rest / 60 % 60,
        rest % 60,
        offset / 60,
        offset % 60
    )
}

/// Howard Hinnant's `civil_from_days`: (year, month 1–12, day) of a day count from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The message as git's medium format prints it: leading blank lines dropped, each line after
/// four spaces, a blank line as the four spaces, trailing whitespace trimmed.
fn message_lines(message: &str) -> Vec<String> {
    fn blank(c: char) -> bool {
        matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r')
    }
    let mut out: Vec<String> = Vec::new();
    let mut first = true;
    for raw in message.split('\n') {
        let line = raw.trim_end_matches(blank);
        if line.is_empty() {
            if first {
                continue;
            }
            out.push("    ".to_string());
        } else {
            first = false;
            out.push(format!("    {line}"));
        }
    }
    while out.last().is_some_and(|l| l.chars().all(blank)) {
        out.pop();
    }
    out
}

/// The stat section: one line per file, then the summary.
fn stat_lines(show: &Show, width: usize) -> Vec<Line> {
    let names: Vec<String> = show.files.iter().map(print_name).collect();
    let max_len = names.iter().map(|n| width_of(n)).max().unwrap_or(0);
    let mut max_change = 0usize;
    let mut bin_width = 0usize;
    let mut number_width = 0usize;
    for file in &show.files {
        match file.content {
            Content::Text { added, deleted } => max_change = max_change.max(added + deleted),
            Content::Binary(sizes) => {
                // "Bin XXX -> YYY bytes" (0 for a side that has no sizes).
                let (old, new) = sizes.unwrap_or((0, 0));
                bin_width = bin_width.max(14 + digits(old) + digits(new));
                number_width = 3;
            }
        }
    }
    number_width = number_width.max(digits(max_change as u64));
    let mut width = width;
    if width < 16 + 6 + number_width {
        width = 16 + 6 + number_width;
    }
    // The graph is as wide as wanted, ignoring the available width, then shrunk.
    let mut graph_width = if max_change + 4 > bin_width {
        max_change
    } else {
        bin_width - 4
    };
    let mut name_width = max_len;
    if name_width + number_width + 6 + graph_width > width {
        let cap = (width * 3 / 8) as isize - number_width as isize - 6;
        if (graph_width as isize) > cap {
            graph_width = cap.max(6) as usize;
        }
        if name_width > width.saturating_sub(number_width + 6 + graph_width) {
            name_width = width - number_width - 6 - graph_width;
        } else {
            graph_width = width - number_width - 6 - name_width;
        }
    }
    let mut lines = Vec::with_capacity(show.files.len() + 1);
    for (k, (file, print)) in show.files.iter().zip(&names).enumerate() {
        let (prefix, name) = truncate(print, name_width);
        let padding = name_width.saturating_sub(width_of(&prefix) + width_of(&name));
        let mut head = String::with_capacity(1 + prefix.len() + name.len() + padding + 3);
        head.push(' ');
        head.push_str(&prefix);
        head.push_str(&name);
        head.extend(std::iter::repeat_n(' ', padding));
        head.push_str(" | ");
        let mut segments = vec![Segment::new(head, Kind::StatName)];
        match file.content {
            Content::Binary(sizes) => {
                segments.push(Segment::new(format!("{:>number_width$}", "Bin"), Kind::StatCount));
                if let Some((old, new)) = sizes {
                    segments.push(Segment::new(format!(" {old}"), Kind::StatDel));
                    segments.push(Segment::new(" -> ", Kind::Plain));
                    segments.push(Segment::new(new.to_string(), Kind::StatAdd));
                    segments.push(Segment::new(" bytes", Kind::Plain));
                }
            }
            Content::Text { added, deleted } => {
                let total = added + deleted;
                let mut count = format!("{total:>number_width$}");
                if total > 0 {
                    count.push(' ');
                }
                segments.push(Segment::new(count, Kind::StatCount));
                let (add, del) = scale(added, deleted, graph_width, max_change);
                if add > 0 {
                    segments.push(Segment::new("+".repeat(add), Kind::StatAdd));
                }
                if del > 0 {
                    segments.push(Segment::new("-".repeat(del), Kind::StatDel));
                }
            }
        }
        lines.push(Line {
            segments,
            file: Some(k),
        });
    }
    let mut adds = 0;
    let mut dels = 0;
    for file in &show.files {
        if let Content::Text { added, deleted } = file.content {
            adds += added;
            dels += deleted;
        }
    }
    let files = show.files.len();
    let mut summary = if files == 1 {
        " 1 file changed".to_string()
    } else {
        format!(" {files} files changed")
    };
    if adds > 0 || dels == 0 {
        summary.push_str(&format!(", {adds} insertion{}", if adds == 1 { "(+)" } else { "s(+)" }));
    }
    if dels > 0 || adds == 0 {
        summary.push_str(&format!(", {dels} deletion{}", if dels == 1 { "(-)" } else { "s(-)" }));
    }
    lines.push(Line::new(summary, Kind::Summary));
    lines
}

/// A file's print name: the path, or a rename's `pprint_rename` brace form.
fn print_name(file: &File) -> String {
    match &file.old_path {
        Some(old) => rename(old, &file.path),
        None => file.path.clone(),
    }
}

/// git's `pprint_rename`: the common prefix and suffix at slash boundaries, as
/// `pfx{old => new}sfx`, `{old => new}sfx`, `pfx{old => new}` or `old => new`.
fn rename(a: &str, b: &str) -> String {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let mut pfx = 0;
    let mut i = 0;
    while i < ab.len() && i < bb.len() && ab[i] == bb[i] {
        if ab[i] == b'/' {
            pfx = i + 1;
        }
        i += 1;
    }
    let adjust = usize::from(pfx > 0);
    let mut old = ab.len() as isize;
    let mut new = bb.len() as isize;
    let mut sfx = 0;
    loop {
        let at = |s: &[u8], i: isize| {
            if i < 0 {
                0
            } else {
                s.get(i as usize).copied().unwrap_or(0)
            }
        };
        if (pfx as isize - adjust as isize) > old || (pfx as isize - adjust as isize) > new {
            break;
        }
        if at(ab, old) != at(bb, new) {
            break;
        }
        if at(ab, old) == b'/' {
            sfx = ab.len() - old as usize;
        }
        old -= 1;
        new -= 1;
    }
    let a_mid = ab.len().saturating_sub(pfx + sfx);
    let b_mid = bb.len().saturating_sub(pfx + sfx);
    let mut out = String::new();
    if pfx + sfx > 0 {
        out.push_str(&a[..pfx]);
        out.push('{');
    }
    out.push_str(&a[pfx..pfx + a_mid]);
    out.push_str(" => ");
    out.push_str(&b[pfx..pfx + b_mid]);
    if pfx + sfx > 0 {
        out.push('}');
        out.push_str(&a[ab.len() - sfx..]);
    }
    out
}

/// A name cut to `name_width` columns the way git does it: a `...` prefix and the rest from the
/// next `/` after the cut.
fn truncate(name: &str, name_width: usize) -> (String, String) {
    let width = width_of(name);
    if name_width >= width {
        return (String::new(), name.to_string());
    }
    let len = name_width.saturating_sub(3);
    let mut at = (width - len).min(name.len());
    while at > 0 && !name.is_char_boundary(at) {
        at -= 1;
    }
    let tail = &name[at..];
    let shown = match tail.find('/') {
        Some(i) => &tail[i..],
        None => tail,
    };
    ("...".to_string(), shown.to_string())
}

/// git's `scale_linear`: 0 for nothing, else at least one column of `width`.
fn scale_linear(it: usize, width: usize, max_change: usize) -> usize {
    if it == 0 { 0 } else { 1 + it * (width - 1) / max_change }
}

/// The graph's `+` and `-` counts: the real ones when the graph has room for them, else scaled
/// as git scales them (the total first, then the smaller side).
fn scale(added: usize, deleted: usize, graph_width: usize, max_change: usize) -> (usize, usize) {
    if graph_width > max_change {
        return (added, deleted);
    }
    let mut total = scale_linear(added + deleted, graph_width, max_change);
    if total < 2 && added > 0 && deleted > 0 {
        total = 2;
    }
    if added < deleted {
        let add = scale_linear(added, graph_width, max_change);
        (add, total.saturating_sub(add))
    } else {
        let del = scale_linear(deleted, graph_width, max_change);
        (total.saturating_sub(del), del)
    }
}

fn digits(n: u64) -> usize {
    let mut width = 1;
    let mut n = n;
    while n >= 10 {
        n /= 10;
        width += 1;
    }
    width
}

fn width_of(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::super::git::test_repo::{git, init};
    use super::super::git::{Repo, show};
    use super::*;

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        init(&root);
        (dir, root)
    }

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, bytes).unwrap();
    }

    fn commit(root: &Path, message: &str) -> String {
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", message]);
        git(root, &["rev-parse", "HEAD"]).trim().to_string()
    }

    fn date_commit(root: &Path, date: &str, message: &str) -> String {
        git(root, &["add", "-A"]);
        let out = Command::new("git")
            .args(["commit", "-q", "-m", message])
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        git(root, &["rev-parse", "HEAD"]).trim().to_string()
    }

    fn ours(root: &Path, id: &str, width: usize) -> String {
        let show = show::read(&Repo::discover(root).unwrap(), id).unwrap();
        let mut out = String::new();
        for line in render(&show, width) {
            out.push_str(&line.text());
            out.push('\n');
        }
        out
    }

    fn cli(root: &Path, id: &str, width: usize) -> String {
        let width = format!("--stat-width={width}");
        git(root, &["show", "--stat", "--format=medium", &width, id])
    }

    fn assert_same(root: &Path, id: &str) {
        for width in [200, 120, 80, 40, 25, 10] {
            assert_eq!(ours(root, id, width), cli(root, id, width), "{id} at {width}");
        }
    }

    #[test]
    fn a_root_commit_with_text_and_binary() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\nb\nc\n");
        write(&root, "b.txt", b"one\ntwo\n");
        write(&root, "bin.dat", b"\x00\x01\x02binary");
        let id = commit(&root, "first commit");
        assert_same(&root, &id);
    }

    /// Several files with unequal counts and a long enough name to be cut at narrow widths.
    #[test]
    fn a_modification_scales_its_graph() {
        let (_d, root) = tmp();
        write(&root, "src/some/rather/long/path/name.txt", b"a\n");
        write(&root, "b.txt", b"1\n2\n3\n4\n5\n6\n7\n8\n");
        write(&root, "c.txt", b"x\ny\n");
        commit(&root, "root");
        write(&root, "src/some/rather/long/path/name.txt", b"a\nb\nc\nd\ne\nf\n");
        // Both additions and deletions (the smaller side scales), and enough lines for a
        // two-digit count (`number_width`).
        write(&root, "b.txt", b"1\n2\nnew a\nnew b\nnew c\n7\n8\n");
        write(&root, "many.txt", b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n");
        std::fs::remove_file(root.join("c.txt")).unwrap();
        let id = commit(&root, "modify");
        assert_same(&root, &id);
    }

    #[test]
    fn renames_use_the_brace_form() {
        let (_d, root) = tmp();
        write(&root, "src/a/old_name.txt", b"one\ntwo\n");
        write(&root, "src/mode.sh", b"hello\n");
        write(&root, "src/keep/one/file.txt", b"kept\n");
        write(&root, "src/rewrite.txt", b"alpha\nbeta\ngamma\n");
        commit(&root, "root");
        std::fs::rename(root.join("src/a/old_name.txt"), root.join("src/a/new_name.txt")).unwrap();
        std::fs::rename(root.join("src/mode.sh"), root.join("src/a/renamed_mode.sh")).unwrap();
        std::fs::create_dir_all(root.join("src/keep/two")).unwrap();
        std::fs::rename(root.join("src/keep/one/file.txt"), root.join("src/keep/two/file.txt")).unwrap();
        // Renamed but rewritten: too unlike for git's 50% rename detection (a deletion and an
        // addition).
        std::fs::rename(root.join("src/rewrite.txt"), root.join("src/rewritten.txt")).unwrap();
        std::fs::write(
            root.join("src/rewritten.txt"),
            b"one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
        )
        .unwrap();
        // A rename that also changes the text (its count and graph).
        std::fs::write(root.join("src/keep/two/file.txt"), b"kept\nmore\n").unwrap();
        let id = commit(&root, "renames");
        // A rename with both a common prefix and a common suffix (`src/keep/{one => two}/file.txt`).
        assert_same(&root, &id);
    }

    #[test]
    fn binaries_and_a_binary_mode_change() {
        let (_d, root) = tmp();
        write(&root, "bin.dat", b"\x00\x01\x02\x03binary");
        write(&root, "gone.bin", b"\x00\x01");
        write(&root, "mode.bin", b"\x00mode");
        commit(&root, "root");
        write(&root, "bin.dat", b"\x00\x01\x02\x04binary");
        std::fs::remove_file(root.join("gone.bin")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = root.join("mode.bin");
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        let id = commit(&root, "binary changes");
        assert_same(&root, &id);
    }

    #[test]
    fn a_mode_only_change_and_an_empty_commit() {
        let (_d, root) = tmp();
        write(&root, "mode.sh", b"hello\n");
        commit(&root, "root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = root.join("mode.sh");
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        let id = commit(&root, "mode only");
        assert_same(&root, &id);
        git(&root, &["commit", "-q", "--allow-empty", "-m", "empty"]);
        let empty = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        assert_same(&root, &empty);
        // No message either: the header alone.
        git(
            &root,
            &["commit", "-q", "--allow-empty", "--allow-empty-message", "-m", ""],
        );
        let bare = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        assert_same(&root, &bare);
    }

    #[test]
    fn a_merge_is_shown_against_its_first_parent() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\n");
        write(&root, "f.txt", b"f\n");
        commit(&root, "root");
        git(&root, &["checkout", "-q", "-b", "side"]);
        write(&root, "a.txt", b"a\nside\n");
        commit(&root, "side");
        git(&root, &["checkout", "-q", "main"]);
        write(&root, "f.txt", b"f\nmain\n");
        commit(&root, "main");
        git(&root, &["merge", "-q", "--no-ff", "side", "-m", "merge side"]);
        let id = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        assert_same(&root, &id);
        let show = show::read(&Repo::discover(&root).unwrap(), &id).unwrap();
        let lines = render(&show, 80);
        assert_eq!(lines[0].kind(), Kind::Plain);
        assert_eq!(lines[1].kind(), Kind::Merge, "the Merge: line");
        assert_eq!(lines[2].kind(), Kind::Author);
    }

    #[test]
    fn a_multi_paragraph_message_and_a_non_utc_date() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\n");
        let id = date_commit(
            &root,
            "2026-10-17T14:03:09+0530",
            "subject line\n\nfirst paragraph line one\nfirst paragraph line two\n\nsecond paragraph\n\ntrailing",
        );
        assert_same(&root, &id);
        // A date in another year and offset, and a day of the month below ten.
        write(&root, "b.txt", b"b\n");
        let id = date_commit(&root, "2020-01-02T23:59:59-0730", "second");
        assert_same(&root, &id);
    }

    #[test]
    fn an_empty_message_still_prints_the_stat() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\n");
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "--allow-empty-message", "-m", ""]);
        let id = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        assert_same(&root, &id);
    }

    /// The segments: the commit id is a `Commit` segment, additions green (`StatAdd`),
    /// deletions red (`StatDel`), a binary's old size red and its new size green.
    #[test]
    fn lines_carry_their_kinds() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"one\ntwo\n");
        write(&root, "bin.dat", b"\x00\x01");
        commit(&root, "root");
        write(&root, "a.txt", b"one\ntwo\nthree\nfour\n");
        write(&root, "bin.dat", b"\x00\x01\x02");
        let id = commit(&root, "change\n\nbody");
        let show = show::read(&Repo::discover(&root).unwrap(), &id).unwrap();
        let lines = render(&show, 80);
        assert_eq!(lines[0].kind(), Kind::Plain, "“commit ” then the id");
        assert!(lines[0].segments.iter().any(|s| s.kind == Kind::Commit && s.text == id));
        assert_eq!(lines[1].kind(), Kind::Author);
        assert_eq!(lines[2].kind(), Kind::Date);
        assert_eq!(lines[3].text(), "");
        assert!(
            lines
                .iter()
                .any(|l| l.kind() == Kind::Message && l.text() == "    change")
        );
        assert!(lines.iter().any(|l| l.kind() == Kind::Message && l.text() == "    "));
        let stat = lines
            .iter()
            .find(|l| {
                l.segments
                    .iter()
                    .any(|s| s.kind == Kind::StatName && s.text.contains("a.txt"))
            })
            .unwrap();
        assert!(stat.segments.iter().any(|s| s.kind == Kind::StatAdd && s.text == "++"));
        let binary = lines
            .iter()
            .find(|l| {
                l.segments
                    .iter()
                    .any(|s| s.kind == Kind::StatName && s.text.contains("bin.dat"))
            })
            .unwrap();
        assert_eq!(
            binary
                .segments
                .iter()
                .map(|s| (s.kind, s.text.as_str()))
                .collect::<Vec<_>>(),
            [
                (Kind::StatName, binary.segments[0].text.as_str()),
                (Kind::StatCount, "Bin"),
                (Kind::StatDel, " 2"),
                (Kind::Plain, " -> "),
                (Kind::StatAdd, "3"),
                (Kind::Plain, " bytes"),
            ]
        );
        assert_eq!(lines.last().unwrap().kind(), Kind::Summary);
        assert!(
            lines
                .last()
                .unwrap()
                .text()
                .ends_with("2 files changed, 2 insertions(+)"),
            "{}",
            lines.last().unwrap().text()
        );
    }

    /// Each stat line names its file's index in `Show::files`; the other lines name none.
    #[test]
    fn stat_lines_name_their_file() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\n");
        write(&root, "b.txt", b"b\n");
        commit(&root, "root");
        write(&root, "a.txt", b"a\nmore\n");
        write(&root, "b.txt", b"b\nmore\n");
        let id = commit(&root, "both");
        let show = show::read(&Repo::discover(&root).unwrap(), &id).unwrap();
        let lines = render(&show, 80);
        assert_eq!(lines[0].file, None, "the commit line");
        let stat: Vec<usize> = lines.iter().filter_map(|l| l.file).collect();
        assert_eq!(stat, (0..show.files.len()).collect::<Vec<_>>());
        let names: Vec<&str> = lines
            .iter()
            .filter(|l| l.file.is_some())
            .map(|l| show.files[l.file.unwrap()].path.as_str())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt"]);
        assert_eq!(lines.last().unwrap().file, None, "the summary");
    }

    /// The rename forms: prefix and suffix, prefix alone, suffix alone, and neither.
    #[test]
    fn rename_forms_match_gits() {
        assert_eq!(
            rename("src/a/old_name.txt", "src/a/new_name.txt"),
            "src/a/{old_name.txt => new_name.txt}"
        );
        assert_eq!(
            rename("src/mode.sh", "src/a/renamed_mode.sh"),
            "src/{mode.sh => a/renamed_mode.sh}"
        );
        assert_eq!(rename("a/foo.txt", "b/foo.txt"), "{a => b}/foo.txt");
        assert_eq!(rename("one.txt", "two.txt"), "one.txt => two.txt");
        assert_eq!(rename("same.txt", "same.txt"), "same.txt => same.txt");
    }
}
