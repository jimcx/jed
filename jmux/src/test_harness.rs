//! Test helpers shared by jmux's headless test files (`#[cfg(test)]`), as methods on `App`: the
//! one copy of what each harness used to define for itself. They send through the attached
//! client's `ClientMsg::Input`, the path a real client's keys take (`App::handle`).
//!
//! **The rule: run the command, don't press its key.** A test starts a feature with
//! `app.run(Command::..)`. It presses a Ctrl, Cmd or Alt key only when the test is about key
//! handling (the keys the shell keeps, a focused side bar or search field, the close question),
//! and then through `app.press_bound(Command::..)`, never a raw platform key. The guard in
//! `jed/src/command_driven_tests.rs` (`command_driven_tests_guard_*`) fails on a raw press.

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::app::{App, AppEvent};
use crate::command::Command;
use crate::protocol::ClientMsg;

impl App {
    /// Sends `input` as the attached client (panics when none is attached).
    pub fn send_input(&mut self, input: Input) {
        let conn = self.attached.expect("a client is attached");
        self.handle(AppEvent::Client {
            conn,
            msg: ClientMsg::Input(input),
        });
    }

    /// Presses a raw key. For plain keys; a Ctrl, Cmd or Alt key only in a test about that key
    /// (and on the guard's allowlist), else `run` or `press_bound`.
    pub fn press(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.send_input(Input::Key(KeyEvent::new(code, mods)));
    }

    /// Types `text` (a `\n` is Enter), one key each.
    pub fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            let code = if ch == '\n' { KeyCode::Enter } else { KeyCode::Char(ch) };
            self.press(code, KeyModifiers::NONE);
        }
    }

    /// A pointer event with no modifiers.
    pub fn send_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        self.send_input(Input::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }));
    }

    /// Runs `command` as the key path would, without a key: a shown notice goes (a key clears it), the side bar gives the keys back to
    /// the terminal for a terminal command (`take_keys_back`), then `execute`. Nothing else
    /// follows a command in `App::key`; the frame is drawn by the event loop, as for keys.
    pub fn run(&mut self, command: Command) {
        if self.notice.take().is_some() {
            self.needs_redraw = true;
        }
        let context = self.context();
        self.take_keys_back(context, command);
        self.execute(command);
    }

    /// Presses the key that `lookup` in the current context resolves to `command`: the palette
    /// key if it does, else another binding of it. Panics naming the command, the context and
    /// the keys tried when none resolves, so a removed context binding fails here with a reason.
    pub fn press_bound(&mut self, command: Command) {
        let context = self.context();
        let shown = self.keymap.binding_for(command);
        let mut combos: Vec<_> = self
            .keymap
            .bindings_with_source()
            .filter(|(_, _, c, _)| *c == command)
            .map(|(combo, ..)| combo)
            .collect();
        combos.sort_by_key(|c| (shown != Some(*c), c.label()));
        combos.dedup();
        for combo in &combos {
            let ev = KeyEvent::new(combo.code, combo.mods);
            if self.keymap.lookup(&ev, context) == Some(command) {
                self.send_input(Input::Key(ev));
                return;
            }
        }
        let tried: Vec<String> = combos.iter().map(|c| c.label()).collect();
        panic!("press_bound: no key resolves to {command:?} in context {context:?}; bindings tried: {tried:?}");
    }
}
