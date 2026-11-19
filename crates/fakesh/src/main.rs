//! `fakesh`: a shell with fixed behaviour for jed's and jmux's terminal tests
//! (`jterm::testing::fake_shell`). Real shells differ in what the tests don't care about (the
//! prompt, whether bracketed paste is on before the first prompt, what Ctrl+C echoes); this one
//! does the same everywhere, root or not.
//!
//! It turns bracketed paste on (`ESC[?2004h`), prompts with `$ `, and reads lines in the tty's
//! cooked mode (echo, Backspace, Ctrl+U, Ctrl+D at an empty line as end of input are the kernel's).
//! The commands are `shell::Shell::command`'s match arms: only what the tests type. It never
//! forks. SIGINT drops the line (or ends `sleep` / `read`) and draws a fresh prompt; the paste
//! markers of a line are removed and `pasted` says whether the previous line had them.

mod expand;
mod input;
mod printf;
mod shell;

use expand::Vars;
use shell::{Flow, Line, Shell};
use std::io::Write;

const PROMPT: &str = "$ ";

fn main() {
    input::install_sigint_handler();
    let mut out = std::io::stdout();
    let mut sh = Shell::new(Vars::from_env(), Box::new(input::Stdin::default()));
    // `fakesh -c LINE` (how `tasks.shell` runs a task's command): run the line, exit with `$?`.
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, line, ..] = args.as_slice()
        && flag == "-c"
    {
        let flow = sh.run_line(line, &mut out);
        let _ = out.flush();
        std::process::exit(match flow {
            Flow::Exit(code) => code,
            Flow::Continue => sh.vars.status,
        });
    }
    let _ = write!(out, "\x1b[?2004h");
    loop {
        let _ = write!(out, "{PROMPT}");
        let _ = out.flush();
        // The shell owns the input (`read` uses it too), so the prompt loop asks it for lines.
        let line = sh.next_line();
        let flow = match line {
            Line::Eof => Flow::Exit(0),
            Line::Interrupted => Flow::Continue,
            Line::Text(text) => sh.run_line(&text, &mut out),
        };
        if input::take_interrupt() {
            let _ = writeln!(out);
            sh.vars.status = 130;
        }
        if let Flow::Exit(code) = flow {
            let _ = out.flush();
            std::process::exit(code);
        }
    }
}
