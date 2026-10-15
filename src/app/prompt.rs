//! Modal prompts and their keys.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, FileOp, Level};
use crate::buffer::BufferId;
use crate::explorer;
use crate::field::{Outcome, TextField};
use crate::layout::GroupId;
use crate::workspace::trust::Trust;

pub enum Prompt {
    SaveBeforeClose { group: GroupId, buffer: BufferId },
    SaveAllBeforeQuit { buffers: Vec<BufferId> },
    Trust { commands: Vec<String> },
    Input(Input),
    ConfirmDelete { path: PathBuf, is_dir: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    NewFile { dir: PathBuf },
    NewFolder { dir: PathBuf },
    Rename { from: PathBuf },
}

/// A name prompt: a text field plus what the name is for.
pub struct Input {
    pub kind: InputKind,
    pub field: TextField,
    pub error: Option<&'static str>,
}

impl Input {
    pub fn new(kind: InputKind, text: String) -> Self {
        Self {
            kind,
            field: TextField::new(text),
            error: None,
        }
    }

    pub fn insert(&mut self, s: &str) {
        self.field.insert(s);
        self.error = None;
    }

    fn key(&mut self, key: KeyEvent) {
        if self.field.key(key) == Outcome::Edited {
            self.error = None;
        }
    }
}

impl App {
    pub(super) fn ask_trust(&mut self) {
        if self.prompt.is_some() {
            self.trust_pending = true;
            return;
        }
        self.trust_pending = false;
        self.prompt = Some(Prompt::Trust {
            commands: self.workspace.config.describe_commands(),
        });
    }

    fn close_prompt(&mut self) {
        self.prompt = None;
        if self.trust_pending {
            self.ask_trust();
        }
    }

    pub(super) fn handle_prompt_key(&mut self, key: KeyEvent) {
        let Some(prompt) = self.prompt.take() else { return };
        let ch = match key.code {
            KeyCode::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        let enter = key.code == KeyCode::Enter;
        let esc = key.code == KeyCode::Esc;
        match prompt {
            Prompt::SaveBeforeClose { group, buffer } => match ch {
                _ if enter || ch == Some('s') => {
                    self.close_prompt();
                    self.after_save = Some(super::AfterSave::Close(group, buffer));
                    self.save(buffer);
                }
                Some('d') => {
                    self.close_prompt();
                    if let Some(i) = self.groups.get(group).and_then(|g| g.find(buffer)) {
                        self.discard_tab(group, i);
                    }
                }
                _ if esc || ch == Some('c') => self.close_prompt(),
                _ => self.prompt = Some(Prompt::SaveBeforeClose { group, buffer }),
            },
            Prompt::SaveAllBeforeQuit { buffers } => match ch {
                _ if enter || ch == Some('s') => {
                    self.close_prompt();
                    self.save_all_then_quit(buffers);
                }
                Some('d') => self.should_quit = true,
                _ if esc || ch == Some('c') => self.close_prompt(),
                _ => self.prompt = Some(Prompt::SaveAllBeforeQuit { buffers }),
            },
            Prompt::Trust { commands } => match ch {
                Some('t') => {
                    self.close_prompt();
                    self.decide_trust(true);
                }
                _ if esc || ch == Some('r') => {
                    self.close_prompt();
                    self.decide_trust(false);
                }
                _ => self.prompt = Some(Prompt::Trust { commands }),
            },
            Prompt::ConfirmDelete { path, is_dir } => match ch {
                _ if enter || ch == Some('t') => {
                    self.close_prompt();
                    self.run_file_op(FileOp::Delete(path));
                }
                _ if esc || ch == Some('c') => self.close_prompt(),
                _ => self.prompt = Some(Prompt::ConfirmDelete { path, is_dir }),
            },
            Prompt::Input(mut input) => {
                if esc {
                    self.close_prompt();
                } else if enter {
                    self.submit_input(input);
                } else {
                    input.key(key);
                    self.prompt = Some(Prompt::Input(input));
                }
            }
        }
    }

    fn submit_input(&mut self, mut input: Input) {
        let name = input.field.text.trim().to_string();
        let nested = !matches!(input.kind, InputKind::Rename { .. });
        if let Err(e) = explorer::validate_name(&name, nested) {
            input.error = Some(e);
            self.prompt = Some(Prompt::Input(input));
            return;
        }
        self.close_prompt();
        let op = match input.kind {
            InputKind::NewFile { dir } => FileOp::NewFile(dir.join(&name)),
            InputKind::NewFolder { dir } => FileOp::NewFolder(dir.join(&name)),
            InputKind::Rename { from } => {
                let to = from.with_file_name(&name);
                if to == from {
                    return;
                }
                FileOp::Rename { from, to }
            }
        };
        self.run_file_op(op);
    }

    fn decide_trust(&mut self, trusted: bool) {
        self.trust = if trusted { Trust::Trusted } else { Trust::Restricted };
        self.servers_dirty = true;
        self.trust_store
            .record(&self.workspace.root, &self.workspace.config, trusted);
        let store = self.trust_store.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = store.write() {
                tracing::error!("saving trust decision: {e}");
            }
        });
        let text = if trusted {
            "Workspace trusted"
        } else {
            "Restricted mode: workspace commands won't run"
        };
        self.notify(Level::Info, text.into());
    }
}
