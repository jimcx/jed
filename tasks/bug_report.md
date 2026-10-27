---
id: bug_report
name: Bug reporter
summary: "\"Help: Report Bug\" (`help.reportBug`, palette, unbound) asks for a brief description of what happened (an `Input` prompt, multi-line not needed), then writes one self-contained Markdown file, `<state dir>/bug-reports/<UTC time>.md`, and shows its path in a message (also copied to the clipboard). Contents, each a section: the description; jed's version and git commit (built in at compile time), OS, terminal (`$TERM`, `$TERM_PROGRAM`, kitty keyboard protocol on or not, size, truecolor); the workspace (root, trust, `.workspace` settings, the layered settings, `keybindings.toml` overrides and load errors); the layout (groups and split tree, each group's tabs — path relative to the root, language, size, line count, dirty / deleted / read-only / disk conflict, EOL, encoding, selection count and primary's position, scroll — the active group and tab, focus, side / bottom panel and their views, open prompt / palette / find widget); language servers (per server: command, state, encoding, pending requests, diagnostics counts), git (branch label, entry counts), tasks (name, status), background work in flight (saves, parses, searches, backups); the last messages shown and the last ~50 commands run (a ring buffer of `Command::name()`s with times; no typed text); and the tail of the current log file (`newest_log`, last ~500 lines). Never file contents or typed text (privacy: the file is meant to be attached to a report); paths outside the workspace shown with `~`. Gathered on the UI thread as a cheap snapshot (O(groups + tabs + buffers), no rope scans), written in `spawn_blocking`. The panic hook could later write the same report (description = the panic message)"
status: open
priority: 0
dependencies: []
---

  - Needs: prompts, `App::notify`, the clipboard, `logging::newest_log`, the state dir, `build.rs`'s `JED_VERSION`
  - Done when: Report Bug with a description writes the file and shows / copies its path; every section above is there for a workspace with two groups, a running fake server, a git repo and a task; no file contents or typed text appear in it (a test types a marker string and searches the report for it)
  - Subtasks (one agent and one commit each):
    - bug_report-a: the command, the prompt, the file written off-thread and its path shown and copied; the description, version / commit (`build.rs`), OS, terminal, workspace and settings sections, and the log tail
    - bug_report-b: the app state sections: layout, language servers, git, tasks, background work, the last messages and the command ring buffer; the privacy test, gaps and hand checks; marks the task ✅
