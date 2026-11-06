//! Suggestions in review comments (no app state): the fenced blocks a reviewer writes to propose
//! new text for the lines a comment is on, parsed into normalised line ranges.
//!
//! - GitHub and Gitea (which has no native support: the fence is just text, and jed applies it):
//!   a ```` ```suggestion ```` block replaces exactly the comment's anchored lines.
//! - GitLab: ```` ```suggestion:-N+M ```` replaces lines `end - N ..= end + M`, `end` being the
//!   anchor's last line (N and M at most [`MAX_SPAN`]).
//!
//! Fences may be longer than three backticks, the info string is case-insensitive, CRLF bodies
//! work, an empty block deletes the lines, several blocks come out in order, and an unterminated
//! fence is ignored. [`fences`] is also what the thread block's drawing uses to style the code.

use crate::forge::{Anchor, Suggestion};

/// Largest N or M of a GitLab `suggestion:-N+M`.
pub const MAX_SPAN: u32 = 100;
/// Suggestion blocks read from one comment, and lines in one block.
const MAX_BLOCKS: usize = 20;
const MAX_LINES: usize = 2000;

/// A suggestion fence found in a comment body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence<'a> {
    /// The 0-based body lines of the opening and the closing fence.
    pub open: usize,
    pub close: usize,
    /// `Some((N, M))` for GitLab's `suggestion:-N+M`.
    pub span: Option<(u32, u32)>,
    /// The lines between the fences, without line endings.
    pub lines: Vec<&'a str>,
}

/// `(backticks, info)` of an opening fence line.
fn fence_start(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let n = t.bytes().take_while(|&b| b == b'`').count();
    let info = t[n..].trim();
    (n >= 3 && !info.contains('`')).then_some((n, info))
}

fn is_close(line: &str, n: usize) -> bool {
    let t = line.trim_start_matches(' ');
    let ticks = t.bytes().take_while(|&b| b == b'`').count();
    line.len() - t.len() <= 3 && ticks >= n && t[ticks..].trim().is_empty()
}

/// `suggestion` / `suggestion:-N+M` (any case): `Some(None)` and `Some(Some((N, M)))`; `None` when
/// the info string is anything else.
fn suggestion_info(info: &str) -> Option<Option<(u32, u32)>> {
    let lower = info.to_ascii_lowercase();
    let rest = lower.strip_prefix("suggestion")?;
    if rest.is_empty() {
        return Some(None);
    }
    let (n, m) = rest.strip_prefix(":-")?.split_once('+')?;
    let number =
        |s: &str| (!s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok())?;
    Some(Some((number(n)?, number(m)?)))
}

/// The suggestion fences of `body`, in order. Other fenced code is skipped whole (a suggestion
/// shown inside it is only an example).
pub fn fences(body: &str) -> Vec<Fence<'_>> {
    let mut out = Vec::new();
    if !body.contains("```") {
        return out;
    }
    let lines: Vec<&str> = body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut i = 0;
    while i < lines.len() {
        let Some((n, info)) = fence_start(lines[i]) else {
            i += 1;
            continue;
        };
        let Some(len) = lines[i + 1..].iter().position(|l| is_close(l, n)) else {
            break; // unterminated
        };
        let close = i + 1 + len;
        if let Some(span) = suggestion_info(info)
            && len <= MAX_LINES
            && out.len() < MAX_BLOCKS
        {
            out.push(Fence {
                open: i,
                close,
                span,
                lines: lines[i + 1..close].to_vec(),
            });
        }
        i = close + 1;
    }
    out
}

/// The suggestions in `body`, a comment on `anchor`. A GitLab range reaching before line 1 or
/// beyond [`MAX_SPAN`] is dropped. `replacement` has `\n` after every line (empty: delete).
pub fn parse(body: &str, anchor: &Anchor) -> Vec<Suggestion> {
    let last = anchor.end_line.max(anchor.start_line);
    fences(body)
        .into_iter()
        .filter_map(|f| {
            let (start_line, end_line) = match f.span {
                None => (anchor.start_line.max(1), last),
                Some((n, m)) if n <= MAX_SPAN && m <= MAX_SPAN => {
                    (last.checked_sub(n).filter(|&s| s >= 1)?, last.saturating_add(m))
                }
                Some(_) => return None,
            };
            let mut replacement = String::new();
            for l in &f.lines {
                replacement.push_str(l);
                replacement.push('\n');
            }
            Some(Suggestion {
                start_line,
                end_line,
                replacement,
            })
        })
        .collect()
}

/// A comment's starting text for "Insert Suggestion": a fence round `lines`, and the byte the
/// caret goes to (the start of the first line inside).
pub fn template(lines: &[String]) -> (String, usize) {
    let longest = lines
        .iter()
        .map(|l| l.bytes().take_while(|&b| b == b'`').count())
        .max()
        .unwrap_or(0);
    let ticks = "`".repeat(3.max(longest + 1));
    let head = format!("{ticks}suggestion\n");
    let caret = head.len();
    let mut text = head;
    for l in lines {
        text.push_str(l);
        text.push('\n');
    }
    text.push_str(&ticks);
    (text, caret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::Side;

    fn anchor(start: u32, end: u32) -> Anchor {
        Anchor {
            path: "a.rs".into(),
            side: Side::Right,
            start_line: start,
            end_line: end,
            commit: "c".into(),
        }
    }

    fn one(body: &str, a: &Anchor) -> Suggestion {
        let mut v = parse(body, a);
        assert_eq!(v.len(), 1, "{body:?}");
        v.remove(0)
    }

    #[test]
    fn a_plain_fence_replaces_the_anchored_lines() {
        let s = one(
            "Use this:\n```suggestion\nlet a = 1;\nlet b = 2;\n```\nthanks",
            &anchor(4, 6),
        );
        assert_eq!((s.start_line, s.end_line), (4, 6));
        assert_eq!(s.replacement, "let a = 1;\nlet b = 2;\n");
    }

    #[test]
    fn gitlab_ranges_are_relative_to_the_last_line() {
        let s = one("```suggestion:-2+1\nx\n```", &anchor(10, 10));
        assert_eq!((s.start_line, s.end_line), (8, 11));
        let s = one("```suggestion:-0+0\nx\n```", &anchor(10, 12));
        assert_eq!((s.start_line, s.end_line), (12, 12));
    }

    #[test]
    fn gitlab_ranges_are_bounds_checked() {
        let a = anchor(3, 3);
        assert!(parse("```suggestion:-3+0\nx\n```", &a).is_empty()); // before line 1
        assert_eq!(parse("```suggestion:-2+0\nx\n```", &a).len(), 1); // line 1 is fine
        assert!(parse("```suggestion:-101+0\nx\n```", &anchor(500, 500)).is_empty());
        assert!(parse("```suggestion:-0+101\nx\n```", &a).is_empty());
        assert!(parse("```suggestion:-9999999999+0\nx\n```", &a).is_empty());
        assert!(parse("```suggestion:+1-1\nx\n```", &a).is_empty());
        assert!(parse("```suggestion:-1\nx\n```", &a).is_empty());
        assert!(parse("```suggestion:-a+1\nx\n```", &a).is_empty());
    }

    #[test]
    fn fences_may_be_longer_and_any_case() {
        let s = one("````SuggestION\nfoo\n```\nbar\n````", &anchor(1, 1));
        assert_eq!(s.replacement, "foo\n```\nbar\n");
        assert_eq!(parse("  ```suggestion  \nx\n  ```  ", &anchor(1, 1)).len(), 1);
        assert!(parse("    ```suggestion\nx\n    ```", &anchor(1, 1)).is_empty());
    }

    #[test]
    fn crlf_bodies_lose_the_carriage_returns() {
        let s = one("```suggestion\r\na\r\nb\r\n```\r\n", &anchor(1, 2));
        assert_eq!(s.replacement, "a\nb\n");
    }

    #[test]
    fn an_empty_block_deletes_and_a_blank_line_is_kept() {
        assert_eq!(one("```suggestion\n```", &anchor(2, 3)).replacement, "");
        assert_eq!(one("```suggestion\n\n```", &anchor(2, 3)).replacement, "\n");
    }

    #[test]
    fn several_blocks_come_out_in_order_and_other_code_is_skipped() {
        let body = "```rust\n```suggestion\nno\n```\n```suggestion\none\n```\ntext\n```suggestion:-1+0\ntwo\n```";
        let v = parse(body, &anchor(5, 5));
        let texts: Vec<_> = v.iter().map(|s| s.replacement.as_str()).collect();
        assert_eq!(texts, ["one\n", "two\n"]);
        assert_eq!(v[1].start_line, 4);
    }

    #[test]
    fn unterminated_fences_and_other_text_give_nothing() {
        let a = anchor(1, 1);
        assert!(parse("```suggestion\nx", &a).is_empty());
        assert!(parse("``suggestion\nx\n``", &a).is_empty());
        assert!(parse("```suggestions\nx\n```", &a).is_empty());
        assert!(parse("```suggestion extra\nx\n```", &a).is_empty());
        assert!(parse("", &a).is_empty());
        assert!(parse("```", &a).is_empty());
        assert!(parse("```suggestion\nx\n```suggestion\ny", &a).is_empty());
    }

    #[test]
    fn sizes_are_capped() {
        let big = format!("```suggestion\n{}```", "x\n".repeat(MAX_LINES + 1));
        assert!(parse(&big, &anchor(1, 1)).is_empty());
        let many = "```suggestion\nx\n```\n".repeat(MAX_BLOCKS + 5);
        assert_eq!(parse(&many, &anchor(1, 1)).len(), MAX_BLOCKS);
    }

    #[test]
    fn fences_report_their_lines() {
        let f = fences("a\n```suggestion\nx\ny\n```\nb");
        assert_eq!((f[0].open, f[0].close, f[0].lines.clone()), (1, 4, vec!["x", "y"]));
    }

    #[test]
    fn the_template_wraps_the_lines_and_puts_the_caret_inside() {
        let (t, caret) = template(&["a".into(), "b".into()]);
        assert_eq!(t, "```suggestion\na\nb\n```");
        assert_eq!(&t[caret..], "a\nb\n```");
        let (t, _) = template(&["```x".into()]);
        assert!(t.starts_with("````suggestion\n") && t.ends_with("\n````"));
        let s = one(&t, &anchor(1, 1));
        assert_eq!(s.replacement, "```x\n");
    }

    #[test]
    fn arbitrary_text_never_panics() {
        for body in [
            "`",
            "```\r",
            "```suggestion:-",
            "```suggestion:-1+",
            "\u{0}```suggestion\n```\n```",
            "```é\n```",
        ] {
            let _ = parse(body, &anchor(1, 1));
        }
    }
}
