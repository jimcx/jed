//! Test helpers for jed's and jmux's real-shell tests (the `testing` feature, turned on by their
//! dev-dependencies). `/bin/sh` prompts with `$ `, or `# ` when the tests run as root (as in CI
//! containers); bash as `sh` shows `sh-5.3$ ` / `sh-5.3# `.

/// The characters a shell's prompt ends with.
const PROMPT_END: [char; 2] = ['$', '#'];

/// Whether `text` (a screen's contents or a row) shows a prompt anywhere.
pub fn has_prompt(text: &str) -> bool {
    text.contains(PROMPT_END)
}

/// Whether `row` ends with a prompt (trailing spaces ignored): the shell waits for input there.
pub fn ends_with_prompt(row: &str) -> bool {
    row.trim_end().ends_with(PROMPT_END)
}

/// Whether any of `lines` (a screen's rows, or its contents' `lines()`) ends with a prompt: the
/// shell has drawn one and waits for input. Use it to wait for a shell to start.
pub fn shows_prompt<S: AsRef<str>>(lines: impl IntoIterator<Item = S>) -> bool {
    lines.into_iter().any(|l| ends_with_prompt(l.as_ref()))
}

/// What follows `row`'s prompt (from its first `$` or `#`), leading spaces trimmed; `None`
/// without one.
pub fn after_prompt(row: &str) -> Option<&str> {
    row.split_once(PROMPT_END).map(|(_, rest)| rest.trim_start())
}

/// Whether `row` is the line `want`: alone, or after a prompt (a command typed before the shell
/// drew its prompt has the tty echo it first, and its output lands after the prompt).
pub fn line_is(row: &str, want: &str) -> bool {
    let row = row.trim();
    row == want || after_prompt(row) == Some(want)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_and_root_prompts_both_count() {
        for prompt in ["$ ", "# ", "sh-5.3$ ", "sh-5.3# "] {
            assert!(has_prompt(&format!("1\n{prompt}")));
            assert!(ends_with_prompt(prompt));
            assert!(shows_prompt(format!("1\n{prompt}\n").lines()));
            assert_eq!(after_prompt(&format!("{prompt}seq 1 200")), Some("seq 1 200"));
        }
        assert!(!has_prompt("199\n200"));
        assert!(!ends_with_prompt("$ echo"));
        assert!(!shows_prompt(["$ echo", "# 1"]));
        assert_eq!(after_prompt("25"), None);
        for row in ["mine", "  mine  ", "$ mine", "# mine", "sh-5.3# mine"] {
            assert!(line_is(row, "mine"), "{row:?}");
        }
        assert!(!line_is("$ echo mine", "mine"));
        assert!(!line_is("mine2", "mine"));
    }
}
