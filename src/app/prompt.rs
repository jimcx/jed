//! Modal prompts and their keys.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, AppEvent, FileOp, Level};
use crate::buffer::BufferId;
use crate::buffer::pos::ByteIdx;
use crate::clipboard::Copied;
use crate::explorer;
use crate::field::{Outcome, TextField};
use crate::layout::GroupId;
use crate::workspace::trust::Trust;

pub enum Prompt {
    SaveBeforeClose {
        group: GroupId,
        buffer: BufferId,
    },
    SaveAllBeforeQuit {
        buffers: Vec<BufferId>,
    },
    Trust {
        commands: Vec<String>,
    },
    Input(Input),
    ConfirmDelete {
        path: PathBuf,
        is_dir: bool,
    },
    /// Staging a change from a dirty buffer: save it first (staging reads the file on disk).
    SaveBeforeStage {
        buffer: BufferId,
        line: usize,
        /// The whole file (`git.stageFile`), not the change at `line`.
        file: bool,
    },
    /// Stage All (the Source Control view) with dirty buffers among the files: save them first.
    SaveBeforeStageAll {
        buffers: Vec<BufferId>,
    },
    /// Crash backups found at start (`app.backups.offered`): Restore / Discard / Later (Esc).
    RestoreBackups,
    /// A Save As onto an existing file: Overwrite / Cancel.
    ConfirmOverwrite {
        buffer: BufferId,
        path: PathBuf,
    },
    /// The Search view's Replace All over more than one file: Replace / Cancel.
    ConfirmReplace {
        occurrences: usize,
        files: usize,
        /// The replace text.
        with: String,
    },
    /// Discarding changes (the index's text back, untracked files to the Trash): Discard / Cancel.
    ConfirmDiscard {
        files: Vec<PathBuf>,
        /// Some files are open (left unsaved), some closed (can't be undone); `trash` untracked.
        open: bool,
        closed: bool,
        trash: usize,
    },
    /// Staging a conflicted file whose open buffer still has conflict markers: Stage anyway
    /// (`git add`, marking it resolved) / Cancel.
    ConfirmStageConflict {
        path: PathBuf,
    },
    /// "Terminal: Install Shell Integration": the snippet to source, the shell's init file and the
    /// line that sources it; C copies the line, A appends it, Esc closes.
    InstallShellIntegration {
        snippet: PathBuf,
        init: PathBuf,
        line: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    NewFile {
        dir: PathBuf,
    },
    NewFolder {
        dir: PathBuf,
    },
    Rename {
        from: PathBuf,
    },
    /// Save As (and Save on an untitled buffer): the path for `buffer`, relative to the root or
    /// absolute.
    SaveAs {
        buffer: BufferId,
    },
    /// Rename symbol (F2 in the editor): the new name for `old`, at `at` in `buffer` (shown in
    /// `group`) as it was at `revision`.
    RenameSymbol {
        group: GroupId,
        buffer: BufferId,
        at: ByteIdx,
        revision: u64,
        old: String,
    },
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
        } else if self.backup_offer_pending() {
            self.offer_backups();
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
            Prompt::SaveBeforeStage { buffer, line, file } => match ch {
                _ if enter || ch == Some('s') => {
                    self.close_prompt();
                    self.save_then_stage(super::quick_diff::StageAt { buffer, line, file });
                }
                _ if esc || ch == Some('c') => self.close_prompt(),
                _ => self.prompt = Some(Prompt::SaveBeforeStage { buffer, line, file }),
            },
            Prompt::SaveBeforeStageAll { buffers } => match ch {
                _ if enter || ch == Some('s') => {
                    self.close_prompt();
                    self.save_then_stage_all(buffers);
                }
                _ if esc || ch == Some('c') => self.close_prompt(),
                _ => self.prompt = Some(Prompt::SaveBeforeStageAll { buffers }),
            },
            Prompt::RestoreBackups => match ch {
                _ if enter || ch == Some('r') => {
                    self.close_prompt();
                    self.restore_backups();
                }
                Some('d') => {
                    self.close_prompt();
                    self.discard_backups();
                }
                _ if esc || ch == Some('l') => {
                    self.close_prompt();
                    self.keep_backups();
                }
                _ => self.prompt = Some(Prompt::RestoreBackups),
            },
            Prompt::ConfirmReplace {
                occurrences,
                files,
                with,
            } => match ch {
                _ if enter || ch == Some('r') => {
                    self.close_prompt();
                    self.replace_now(super::replace::Scope::All);
                }
                _ if esc || ch == Some('c') => self.close_prompt(),
                _ => {
                    self.prompt = Some(Prompt::ConfirmReplace {
                        occurrences,
                        files,
                        with,
                    })
                }
            },
            Prompt::ConfirmOverwrite { buffer, path } => match ch {
                _ if enter || ch == Some('o') => {
                    self.close_prompt();
                    self.save_as_to(buffer, path, true);
                }
                _ if esc || ch == Some('c') => {
                    self.close_prompt();
                    self.cancel_save_as();
                }
                _ => self.prompt = Some(Prompt::ConfirmOverwrite { buffer, path }),
            },
            Prompt::ConfirmDelete { path, is_dir } => match ch {
                _ if enter || ch == Some('t') => {
                    self.close_prompt();
                    self.run_file_op(FileOp::Delete(path));
                }
                _ if esc || ch == Some('c') => self.close_prompt(),
                _ => self.prompt = Some(Prompt::ConfirmDelete { path, is_dir }),
            },
            Prompt::ConfirmDiscard {
                files,
                open,
                closed,
                trash,
            } => match ch {
                _ if enter || ch == Some('d') => {
                    self.close_prompt();
                    self.discard_now(files);
                }
                _ if esc || ch == Some('c') => self.close_prompt(),
                _ => {
                    self.prompt = Some(Prompt::ConfirmDiscard {
                        files,
                        open,
                        closed,
                        trash,
                    })
                }
            },
            Prompt::ConfirmStageConflict { path } => match ch {
                _ if enter || ch == Some('s') => {
                    self.close_prompt();
                    self.stage_path_now(path, false);
                }
                _ if esc || ch == Some('c') => self.close_prompt(),
                _ => self.prompt = Some(Prompt::ConfirmStageConflict { path }),
            },
            Prompt::InstallShellIntegration { snippet, init, line } => match ch {
                Some('c') => {
                    self.clipboard.write(Copied {
                        text: line.clone(),
                        ..Copied::default()
                    });
                    self.notify(Level::Info, "Copied the shell integration line".into());
                    self.prompt = Some(Prompt::InstallShellIntegration { snippet, init, line });
                }
                Some('a') => {
                    self.close_prompt();
                    let tx = self.tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let result = crate::shell_integration::append_line(&init, &line).map(|()| init);
                        let _ = tx.send(AppEvent::ShellIntegrationInstalled(result));
                    });
                }
                _ if esc => self.close_prompt(),
                _ => self.prompt = Some(Prompt::InstallShellIntegration { snippet, init, line }),
            },
            Prompt::Input(mut input) => {
                if esc {
                    self.close_prompt();
                    if matches!(input.kind, InputKind::SaveAs { .. }) {
                        self.cancel_save_as();
                    }
                } else if enter {
                    self.submit_input(input);
                } else {
                    if self.field_key(&mut input.field, None, key) == Outcome::Edited {
                        input.error = None;
                    }
                    self.prompt = Some(Prompt::Input(input));
                }
            }
        }
    }

    fn submit_input(&mut self, mut input: Input) {
        if let InputKind::RenameSymbol {
            group,
            buffer,
            at,
            revision,
            old,
        } = input.kind
        {
            self.close_prompt();
            let new = input.field.text;
            // Nothing typed, or the same name: nothing to do (VS Code).
            if !new.trim().is_empty() && new != old {
                self.request_rename(group, buffer, at, revision, old, new);
            }
            return;
        }
        if let InputKind::SaveAs { buffer } = input.kind {
            match self.save_as_target(buffer, input.field.text.trim()) {
                Ok(path) => {
                    self.close_prompt();
                    self.save_as_to(buffer, path, false);
                }
                Err(e) => {
                    input.error = Some(e);
                    self.prompt = Some(Prompt::Input(input));
                }
            }
            return;
        }
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
            InputKind::RenameSymbol { .. } | InputKind::SaveAs { .. } => return,
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
