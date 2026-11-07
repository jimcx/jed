//! Seeded random-input fuzzing through the real event path: keys, text, pastes, mouse, resizes
//! and commands against a tempdir workspace. Checks: no panic, selections in bounds on char
//! boundaries, each step + draw under a time budget.
//!
//! `#[ignore]`d: random commands write user files (spell dictionary, `keybindings.toml`, …) under
//! `paths::config_dir()`, so it refuses to run unless `JED_FUZZ_SANDBOX=1` and `HOME` and the
//! `XDG_*_HOME`s point at a scratch directory. Run (after `cargo test --no-run`) with the test
//! binary, e.g.
//! `S=$(mktemp -d); HOME=$S XDG_CONFIG_HOME=$S/c XDG_STATE_HOME=$S/s XDG_DATA_HOME=$S/d
//! JED_FUZZ_SANDBOX=1 JED_FUZZ_SEEDS=400 JED_FUZZ_STEPS=600 target/debug/deps/jed-* fuzz_random --ignored`.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind as ME};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const SAMPLES: &[&str] = &[
    "fn main() {\n    println!(\"héllo 日本語 👨‍👩‍👧\");\n}\n",
    "",
    "\n\n\n",
    "a\tb\tc\r\nline2\r\nline3",
    "{[(\"'`<>)]}\n",
    "# Title\n\n- item\n- item\n\n```rs\nlet x = 1;\n```\n",
    "\u{0}\u{7}\u{1b}[31mred\u{200b}\u{feff}e\u{301}\u{301}\u{301}\n",
];
const WORDS: &[&str] = &[
    "foo", "(", "\"", "{", "\n", "\t", "  ", "日本", "👍🏽", "e\u{301}", "\r\n", "//", "x", " ", "[", "<",
];
const KEYS: &[K] = &[
    K::Left,
    K::Right,
    K::Up,
    K::Down,
    K::Home,
    K::End,
    K::PageUp,
    K::PageDown,
    K::Enter,
    K::Tab,
    K::BackTab,
    K::Backspace,
    K::Delete,
    K::Esc,
    K::Insert,
];

fn random_event(r: &mut Rng, app: &mut App, cmds: &[Command], w: u16, h: u16, log: &mut Vec<String>) {
    let mods = [
        M::NONE,
        M::SHIFT,
        M::CONTROL,
        M::ALT,
        M::CONTROL | M::SHIFT,
        M::ALT | M::SHIFT,
        M::SUPER,
    ][r.below(7)];
    let pick = r.below(100);
    let rr = r.0;
    log.push(format!("pick {pick} rng {rr}"));
    match pick {
        0..=24 => app.press(KEYS[r.below(KEYS.len())], if r.below(2) == 0 { M::NONE } else { mods }),
        25..=44 => app.type_text(WORDS[r.below(WORDS.len())]),
        45..=54 => {
            let c = (b'a' + r.below(26) as u8) as char;
            app.press(K::Char(c), mods)
        }
        55..=57 => app.press(K::F(1 + r.below(12) as u8), mods),
        58..=62 => app.handle_test_event(Event::Paste(SAMPLES[r.below(SAMPLES.len())].to_string())),
        63..=82 => {
            let (x, y) = (r.below(w as usize) as u16, r.below(h as usize) as u16);
            let b = [MouseButton::Left, MouseButton::Right, MouseButton::Middle][r.below(3)];
            let kind = match r.below(6) {
                0 => ME::Down(b),
                1 => ME::Up(b),
                2 | 3 => ME::Drag(b),
                4 => ME::ScrollDown,
                _ => ME::ScrollUp,
            };
            log.push(format!("mouse {kind:?} {x},{y}"));
            app.mouse_with(kind, x, y, if r.below(4) == 0 { mods } else { M::NONE })
        }
        _ => {
            let c = cmds[r.below(cmds.len())];
            log.push(format!("{c:?}"));
            app.test_execute(c)
        }
    }
}

fn check(app: &App) {
    for g in app.groups.ids() {
        let Some(group) = app.groups.get(g) else { continue };
        for t in &group.tabs {
            let Some(tt) = t.text() else { continue };
            let Some(buf) = app.buffers.get(tt.buffer) else {
                continue;
            };
            let len = buf.rope.len();
            for s in tt.view.selections.iter() {
                for p in [s.anchor.0, s.head.0] {
                    assert!(
                        p <= len,
                        "selection {p} past len {len}: group {g:?} active={} path={:?} sels={}",
                        group.tabs.iter().position(|x| std::ptr::eq(x, t)).unwrap() == group.active,
                        buf.path,
                        tt.view.selections.len()
                    );
                    assert!(
                        buf.rope.is_char_boundary(p),
                        "selection {p} not on a char boundary: {:?} drag={}",
                        buf.rope.to_string().chars().take(60).collect::<String>(),
                        app.dragging_tab()
                    );
                }
            }
        }
    }
}

fn run(seed: u64, steps: usize, cmds: &[Command]) -> Option<(usize, Duration, String)> {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    let mut r = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let mut body = SAMPLES[r.below(SAMPLES.len())].to_string();
    if r.below(4) == 0 {
        body = "word ".repeat(2000) + &body;
    }
    fs::write(root.join("a.rs"), body).unwrap();
    fs::write(root.join("b.md"), "# b\nhello world\nhello again\n").unwrap();
    let Ok(buffer) = Buffer::open(&root.join("a.rs")) else {
        return None;
    };
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(root.join("trust.toml"));
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    app.config_dir = root.join("config");
    app.test_start();
    let (mut w, mut h) = (120u16, 40u16);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut log: Vec<String> = Vec::new();
    let mut worst = (0usize, Duration::ZERO);
    for i in 0..steps {
        if app.quitting() {
            return None;
        }
        let t = Instant::now();
        let res = catch_unwind(AssertUnwindSafe(|| {
            random_event(&mut r, &mut app, cmds, w, h, &mut log);
            if r.below(3) == 0 {
                let a = term.size().unwrap();
                (w, h) = (a.width, a.height);
                term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
            }
            check(&app);
        }));
        let d = t.elapsed();
        if d > worst.1 {
            worst = (i, d);
        }
        if let Err(e) = res {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or(e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            return Some((
                i,
                d,
                format!(
                    "PANIC {msg}\n  recent: {:?}",
                    log[log.len().saturating_sub(40)..]
                        .iter()
                        .filter(|l| !l.starts_with("pick"))
                        .collect::<Vec<_>>()
                ),
            ));
        }
        if i % 7 == 0 && r.below(10) == 0 {
            let (nw, nh) = (20 + r.below(150) as u16, 5 + r.below(50) as u16);
            term = Terminal::new(TestBackend::new(nw, nh)).unwrap();
            (w, h) = (nw, nh);
            app.handle_test_event(Event::Resize(nw, nh));
        }
    }
    if worst.1 > Duration::from_millis(300) {
        return Some((worst.0, worst.1, "SLOW".into()));
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "writes user files: needs a sandboxed HOME / XDG dirs (see the module docs)"]
async fn fuzz_random_input() {
    assert_eq!(
        std::env::var("JED_FUZZ_SANDBOX").as_deref(),
        Ok("1"),
        "set JED_FUZZ_SANDBOX=1"
    );
    let home = crate::paths::home();
    for dir in [
        crate::paths::config_dir(),
        crate::paths::state_dir(),
        crate::paths::data_dir(),
    ] {
        assert!(
            dir.starts_with(&home) && !dir.starts_with(home.join(".config")) && !dir.starts_with(home.join(".local")),
            "{dir:?} isn't in a scratch HOME"
        );
    }
    assert!(!home.join(".config/jed").exists(), "HOME looks like a real home");
    let n = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (seeds, steps, first) = (
        n("JED_FUZZ_SEEDS", 20),
        n("JED_FUZZ_STEPS", 400),
        n("JED_FUZZ_FIRST", 1),
    );
    let skip = [
        "Quit", "Shell", "Terminal", "RunTask", "Restart", "Install", "Reload", "Update", "Release", "Push", "Pull",
        "Fetch", "Clone", "Sign",
    ];
    let cmds: Vec<Command> = Command::all()
        .into_iter()
        .filter(|c| {
            let s = format!("{c:?}");
            !skip.iter().any(|k| s.contains(k))
        })
        .collect();
    let mut bad = Vec::new();
    for seed in first..first + seeds {
        if let Some((step, d, msg)) = run(seed as u64, steps, &cmds) {
            eprintln!("seed {seed} step {step} ({d:?}): {msg}");
            bad.push(seed);
        }
    }
    assert!(bad.is_empty(), "failing seeds: {bad:?}");
}
