//! The commands: one match arm each in `Shell::command`. Output goes to a `Write` (the tty;
//! errors too, as with a real shell on a terminal), so the arms are unit-tested on a `Vec<u8>`.

use crate::expand::{self, Vars};
use crate::printf::printf;
use std::io::Write;

/// The bracketed paste markers a terminal wraps a paste in.
const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// What reading a line gave.
pub enum Line {
    Text(String),
    /// SIGINT arrived: whatever was typed is dropped.
    Interrupted,
    /// End of input (Ctrl+D at an empty line) or the terminal went away.
    Eof,
}

/// Where lines come from (stdin; a script in the tests).
pub trait LineSource {
    fn next_line(&mut self) -> Line;
}

/// Whether the shell goes on.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Exit(i32),
}

pub struct Shell {
    pub vars: Vars,
    input: Box<dyn LineSource>,
    /// Whether the previous line came as a bracketed paste (what `pasted` reports).
    pasted: bool,
}

impl Shell {
    pub fn new(vars: Vars, input: Box<dyn LineSource>) -> Shell {
        Shell {
            vars,
            input,
            pasted: false,
        }
    }

    /// The next line typed.
    pub fn next_line(&mut self) -> Line {
        self.input.next_line()
    }

    /// Runs a typed line: removes the paste markers (remembering them for `pasted`), then its
    /// `;`-separated commands in turn. An interrupt (`crate::input::take_interrupt`, left set for the caller) ends the line.
    pub fn run_line(&mut self, raw: &str, out: &mut impl Write) -> Flow {
        let (line, pasted) = strip_paste(raw);
        // While it runs, `self.pasted` is still the previous line's: `pasted` asks about that.
        let mut flow = Flow::Continue;
        for cmd in expand::split_commands(&line) {
            if crate::input::interrupted() {
                break;
            }
            flow = match expand::words(cmd, &self.vars) {
                Ok(words) if words.is_empty() => Flow::Continue,
                Ok(words) => self.command(&words, out),
                Err(e) => {
                    let _ = writeln!(out, "fakesh: {e}");
                    self.vars.status = 2;
                    Flow::Continue
                }
            };
            if flow != Flow::Continue {
                break;
            }
        }
        self.pasted = pasted;
        let _ = out.flush();
        flow
    }

    fn command(&mut self, words: &[String], out: &mut impl Write) -> Flow {
        let (name, args) = (words[0].as_str(), &words[1..]);
        let status = match name {
            "echo" => {
                let (newline, args) = match args.split_first() {
                    Some((flag, rest)) if flag == "-n" => (false, rest),
                    _ => (true, args),
                };
                let _ = write!(out, "{}{}", args.join(" "), if newline { "\n" } else { "" });
                0
            }
            "printf" => match printf(args) {
                Ok(bytes) => {
                    let _ = out.write_all(&bytes);
                    0
                }
                Err(e) => fail(out, &e, 1),
            },
            "pwd" => match std::env::current_dir() {
                Ok(dir) => {
                    let _ = writeln!(out, "{}", dir.display());
                    0
                }
                Err(e) => fail(out, &format!("pwd: {e}"), 1),
            },
            "cd" => {
                let dir = args
                    .first()
                    .cloned()
                    .or_else(|| self.vars.get("HOME").map(String::from));
                match dir.as_deref().map(|d| (d, std::env::set_current_dir(d))) {
                    Some((_, Ok(()))) => {
                        if let Ok(cwd) = std::env::current_dir() {
                            self.vars.set("PWD", &cwd.to_string_lossy());
                        }
                        0
                    }
                    Some((d, Err(e))) => fail(out, &format!("cd: {d}: {}", os_message(&e)), 1),
                    None => fail(out, "cd: HOME not set", 1),
                }
            }
            "seq" => match args.iter().map(|a| a.parse::<i64>()).collect::<Result<Vec<_>, _>>() {
                Ok(n) if n.len() == 1 || n.len() == 2 => {
                    let (first, last) = if n.len() == 1 { (1, n[0]) } else { (n[0], n[1]) };
                    let text: String = (first..=last).map(|i| format!("{i}\n")).collect();
                    let _ = out.write_all(text.as_bytes());
                    0
                }
                _ => fail(out, "seq: usage: seq [first] last", 1),
            },
            "stty" if args == ["size"] => match window_size() {
                Some((rows, cols)) => {
                    let _ = writeln!(out, "{rows} {cols}");
                    0
                }
                None => fail(out, "stty: standard input: Inappropriate ioctl for device", 1),
            },
            "sleep" => match args.first().and_then(|a| a.parse::<f64>().ok()).filter(|s| *s >= 0.0) {
                Some(secs) => {
                    let _ = out.flush();
                    crate::input::sleep(secs);
                    if crate::input::interrupted() { 130 } else { 0 }
                }
                None => fail(out, "sleep: invalid time interval", 1),
            },
            "read" => {
                let _ = out.flush();
                match self.input.next_line() {
                    Line::Text(text) => {
                        let var = args.first().map_or("REPLY", String::as_str);
                        self.vars.set(var, &strip_paste(&text).0);
                        0
                    }
                    Line::Interrupted => 130,
                    Line::Eof => 1,
                }
            }
            "pasted" => {
                let _ = writeln!(out, "{}", if self.pasted { "yes" } else { "no" });
                0
            }
            "true" | ":" => 0,
            "false" => 1,
            "exit" => {
                let code = match args.first() {
                    None => self.vars.status,
                    Some(a) => a.parse::<i32>().unwrap_or(2),
                };
                return Flow::Exit(code & 0xff);
            }
            _ => match assignment(words) {
                Some((var, value)) => {
                    self.vars.set(var, value);
                    0
                }
                None => fail(out, &format!("{name}: not found"), 127),
            },
        };
        self.vars.status = status;
        Flow::Continue
    }
}

/// `NAME=value` as a whole command.
fn assignment(words: &[String]) -> Option<(&str, &str)> {
    let (var, value) = words.first()?.split_once('=')?;
    let valid = var.starts_with(|c: char| c.is_alphabetic() || c == '_')
        && var.chars().all(|c| c.is_alphanumeric() || c == '_');
    (valid && words.len() == 1).then_some((var, value))
}

fn fail(out: &mut impl Write, message: &str, status: i32) -> i32 {
    let _ = writeln!(out, "fakesh: {message}");
    status
}

/// An I/O error without Rust's " (os error N)" suffix.
fn os_message(e: &std::io::Error) -> String {
    let text = e.to_string();
    text.split(" (os error").next().unwrap_or(&text).to_string()
}

/// `line` without its bracketed paste markers, and whether it had any.
fn strip_paste(line: &str) -> (String, bool) {
    let pasted = line.contains(PASTE_START) || line.contains(PASTE_END);
    (line.replace(PASTE_START, "").replace(PASTE_END, ""), pasted)
}

/// The tty's `(rows, cols)`.
fn window_size() -> Option<(u16, u16)> {
    // SAFETY: TIOCGWINSZ fills the `winsize` we pass.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut size) } == 0;
    ok.then_some((size.ws_row, size.ws_col))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Script(Vec<&'static str>);
    impl LineSource for Script {
        fn next_line(&mut self) -> Line {
            if self.0.is_empty() {
                Line::Eof
            } else {
                Line::Text(self.0.remove(0).into())
            }
        }
    }

    fn shell(input: Vec<&'static str>) -> Shell {
        let mut vars = Vars::default();
        vars.pid = 7;
        vars.set("HOME", "/");
        Shell::new(vars, Box::new(Script(input)))
    }

    /// What `lines` print, and the last status.
    fn run(lines: &[&str]) -> (String, i32) {
        let mut sh = shell(vec![]);
        let mut out = Vec::new();
        for l in lines {
            assert_eq!(sh.run_line(l, &mut out), Flow::Continue, "{l}");
        }
        (String::from_utf8(out).unwrap(), sh.vars.status)
    }

    #[test]
    fn echo_printf_and_semicolons() {
        assert_eq!(run(&["echo a   'b  c'; echo -n x; echo"]).0, "a b  c\nx\n");
        assert_eq!(run(&["printf 'A %s B\\n' 1; printf '\\033[0m'"]).0, "A 1 B\n\x1b[0m");
        assert_eq!(run(&["echo $((6*7)) TAG$((100+2))"]).0, "42 TAG102\n");
        assert_eq!(run(&["X=5; echo \"$X-${Y-none}-${X+set}\""]).0, "5-none-set\n");
        assert_eq!(run(&["", "  ;; "]), (String::new(), 0));
    }

    #[test]
    fn seq_counts_up() {
        assert_eq!(run(&["seq 3"]).0, "1\n2\n3\n");
        assert_eq!(run(&["seq 2 4"]).0, "2\n3\n4\n");
        assert_eq!(run(&["seq x"]).1, 1);
    }

    #[test]
    fn an_unknown_command_is_not_found_with_status_127() {
        assert_eq!(
            run(&["nope arg; echo $?"]),
            ("fakesh: nope: not found\n127\n".into(), 0)
        );
        assert_eq!(run(&["false; echo $?; true; echo $?"]).0, "1\n0\n");
    }

    #[test]
    fn errors_in_expansion_are_reported() {
        let (out, status) = run(&["echo \"open"]);
        assert_eq!(out, "fakesh: unterminated quote\n");
        assert_eq!(status, 2);
        assert_eq!(run(&["printf '%d' x"]).1, 1);
    }

    #[test]
    fn cd_and_pwd_follow_the_directory() {
        // The working directory is the process's: this test is the only one that changes it, and
        // it changes it back.
        let before = std::env::current_dir().unwrap();
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let (out, _) = run(&[&format!("cd {}; pwd; echo $PWD", dir.display())]);
        assert_eq!(out, format!("{0}\n{0}\n", dir.display()));
        let (out, status) = run(&["cd /no/such/dir"]);
        assert_eq!(out, "fakesh: cd: /no/such/dir: No such file or directory\n");
        assert_eq!(status, 1);
        std::env::set_current_dir(before).unwrap();
    }

    #[test]
    fn exit_takes_a_status_or_the_last_one() {
        let mut sh = shell(vec![]);
        let mut out = Vec::new();
        assert_eq!(sh.run_line("echo leaving; exit 3; echo not", &mut out), Flow::Exit(3));
        assert_eq!(out, b"leaving\n");
        assert_eq!(sh.run_line("exit", &mut out), Flow::Exit(0));
        assert_eq!(sh.run_line("nope; exit", &mut out), Flow::Exit(127));
        assert_eq!(sh.run_line("exit 258", &mut out), Flow::Exit(2));
    }

    #[test]
    fn pasted_reports_whether_the_previous_line_was_bracketed() {
        let mut sh = shell(vec![]);
        let mut out = Vec::new();
        sh.run_line("pasted", &mut out);
        sh.run_line(&format!("{PASTE_START}echo hi{PASTE_END}"), &mut out);
        sh.run_line("pasted", &mut out);
        sh.run_line("pasted", &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "no\nhi\nyes\nno\n");
    }

    #[test]
    fn read_takes_the_next_line_into_a_variable() {
        let mut sh = shell(vec!["typed", "second"]);
        let mut out = Vec::new();
        sh.run_line("read x; echo got-$x; read; echo $REPLY", &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "got-typed\nsecond\n");
    }

    #[test]
    fn pid_and_assignment_variables() {
        assert_eq!(run(&["echo $$ $?"]).0, "7 0\n");
        assert_eq!(run(&["A=1 B=2; echo x"]).0, "fakesh: A=1: not found\nx\n");
    }
}
