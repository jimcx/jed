//! Language detection for open buffers and their background parses.

use crate::buffer::{Buffer, BufferId};
use crate::logs::Tool;
use crate::palette::{self, Picker, Row};
use crate::syntax::plugin::LIBRARY;
use crate::syntax::plugin::PluginId;
use crate::syntax::{self, Associations, Language, Parsed};

use super::{App, AppEvent, Level};

impl App {
    /// The language of `buf`: the one picked for it ("Change Language Mode"), else
    /// `.workspace` associations, then its name, then its shebang.
    pub(super) fn detect_language(&self, buf: &Buffer) -> Language {
        if let Some(picked) = buf.language_override {
            return picked;
        }
        if !buf.has_file() {
            return Language::PlainText;
        }
        let rope = &buf.rope;
        let head = rope.slice(..rope.floor_char_boundary(rope.len().min(256))).to_string();
        let first_line = head.lines().next().unwrap_or("");
        syntax::detect(&buf.path, &self.workspace.root, first_line, &self.associations)
    }

    /// "Change Language Mode": a picker of every language for the active buffer.
    pub(super) fn change_language_mode(&mut self) {
        if self.active_buffer().is_none() {
            return;
        }
        let Some(buffer) = self.group().active_tab().map(|t| t.buffer) else {
            return;
        };
        self.open_palette(Picker::Language { buffer }, "");
    }

    /// The language picker's rows and hint for `query`.
    pub(super) fn language_rows(&self, query: &str) -> (Vec<Row>, String) {
        let Some(Picker::Language { buffer }) = self.palette.as_ref().map(|p| p.picker) else {
            return (Vec::new(), String::new());
        };
        let Some(buf) = self.buffers.get(buffer) else {
            return (Vec::new(), String::new());
        };
        let rows = palette::language_rows(query, &Language::all(), buf.syntax.language());
        let hint = if rows.is_empty() {
            "No matching language".into()
        } else {
            "Select Language Mode".into()
        };
        (rows, hint)
    }

    /// A language picked for `buffer`: it keeps it (over detection) until it closes.
    pub(super) fn language_picked(&mut self, buffer: BufferId, lang: Language) {
        let Some(buf) = self.buffers.get_mut(buffer) else {
            return;
        };
        buf.language_override = Some(lang);
        buf.syntax.set_language(lang);
        self.servers_dirty = true;
        tracing::debug!(buffer, language = lang.id(), "language picked");
    }

    /// Re-detects every open buffer's language (associations changed, files renamed).
    pub(super) fn redetect_languages(&mut self) {
        let langs: Vec<(BufferId, Language)> = self
            .buffers
            .iter()
            .map(|(id, b)| (id, self.detect_language(b)))
            .collect();
        for (id, lang) in langs {
            if let Some(buf) = self.buffers.get_mut(id) {
                buf.syntax.set_language(lang);
            }
        }
        self.servers_dirty = true;
    }

    /// Compiles `.workspace`'s `[files] associations`, reporting entries it can't use.
    pub(super) fn load_associations(&mut self) {
        let (associations, errors) = Associations::new(&self.workspace.config.files.associations);
        self.associations = associations;
        if let Some(first) = errors.first() {
            self.notify(
                Level::Error,
                format!("{}: associations: {first}", crate::workspace::FILE_NAME),
            );
        }
    }

    /// Brings every buffer's parse tree up to date: small edits are re-parsed here, first parses
    /// and big edits are started off-thread (→ `AppEvent::Parsed`). The event loop calls it once
    /// per turn, before drawing. Without a runtime (`App::new`, plain unit tests) it waits.
    pub(super) fn sync_syntax(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let max = self.settings.syntax_max_bytes;
        for (id, buf) in self.buffers.iter_mut() {
            if let Some(job) = buf.syntax.update(&buf.rope, max) {
                let tx = self.tx.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = tx.send(AppEvent::Parsed {
                        buffer: id,
                        parsed: job.run(),
                    });
                });
            }
        }
    }

    /// Builds (from `src/`, when stale) and loads the grammar of each of `self.syntax_plugins`,
    /// in one background task: one build at a time, each reported as `AppEvent::GrammarBuilding`
    /// then `AppEvent::Grammar` (an installed grammar answers `Ok` at once; a failure is reported
    /// to this app and tried again by the next load). `main` sets the list to the registry,
    /// which only holds plugins from the user's own config directory (never a `.workspace`'s).
    pub(super) fn load_grammars(&mut self) {
        self.grammar_job(false);
    }

    /// "Languages: Rebuild Syntax Plugins": the same job with every build forced. Not while one
    /// is running.
    pub(super) fn rebuild_grammars(&mut self) {
        use crate::tools_view::GrammarState;
        if self
            .grammars
            .iter()
            .any(|(_, s)| matches!(s, GrammarState::Waiting | GrammarState::Building))
        {
            self.notify(Level::Info, "Syntax plugins are already building".into());
        } else if self.syntax_plugins.is_empty() {
            self.notify(Level::Info, "No syntax plugins (see ~/.config/jed/languages)".into());
        } else {
            self.grammar_job(true);
        }
    }

    fn grammar_job(&mut self, force: bool) {
        use crate::syntax::build::{self, Outcome};
        use crate::tools_view::GrammarState;
        let plugins = self.syntax_plugins.clone();
        if plugins.is_empty() {
            return;
        }
        for p in &plugins {
            self.set_grammar_state(p.get().id, GrammarState::Waiting);
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let tools = build::Toolchain::from_env();
            for p in plugins {
                let _ = tx.send(AppEvent::GrammarBuilding(p));
                let plugin = p.get();
                let was_loaded = p.loaded();
                let built = plugin.buildable().then(|| build::build(&plugin.dir, &tools, force));
                let result = match &built {
                    Some(b) => match &b.outcome {
                        Outcome::Failed(e) => Err(e.clone()),
                        _ => p.load(),
                    },
                    None if plugin.loadable() => p.load(),
                    None => Err(format!("{}: no src/parser.c and no {}", plugin.dir.display(), LIBRARY)),
                };
                let replaced_in_use = was_loaded && built.as_ref().is_some_and(|b| b.outcome == Outcome::Built);
                let _ = tx.send(AppEvent::Grammar {
                    language: Language::Plugin(p),
                    build: built,
                    replaced_in_use,
                    result,
                });
            }
        });
    }

    fn set_grammar_state(&mut self, id: &str, state: crate::tools_view::GrammarState) {
        match self.grammars.iter_mut().find(|(i, _)| i == id) {
            Some((_, s)) => *s = state,
            None => self.grammars.push((id.to_string(), state)),
        }
        self.needs_redraw = true;
    }

    /// A build starts: its row says so and its log starts afresh.
    pub(super) fn grammar_building(&mut self, plugin: PluginId) {
        let id = plugin.get().id;
        self.set_grammar_state(id, crate::tools_view::GrammarState::Building);
        self.logs.clear(&Tool::Grammar(id.to_string()));
    }

    /// A plugin's build and load finished: its log gets the build's lines, its buffers that were
    /// uncoloured get parsed. A failure is reported once and the language stays without a grammar.
    pub(super) fn grammar_loaded(
        &mut self,
        language: Language,
        build: Option<crate::syntax::build::Build>,
        replaced_in_use: bool,
        result: Result<(), String>,
    ) {
        use crate::tools_view::GrammarState;
        let tool = Tool::Grammar(language.id().to_string());
        if let Some(b) = &build {
            for line in &b.log {
                self.logs.push(&tool, crate::logs::Level::Info, line);
            }
        }
        match result {
            Ok(()) => {
                self.logs.push(&tool, crate::logs::Level::Info, "grammar loaded");
                self.set_grammar_state(language.id(), GrammarState::Ready);
                for (_, buf) in self.buffers.iter_mut() {
                    if buf.syntax.language() == language {
                        buf.syntax.grammar_installed();
                    }
                }
                if replaced_in_use {
                    self.notify(
                        Level::Info,
                        format!(
                            "Syntax plugin {}: rebuilt; the new build is used from the next start",
                            language.id()
                        ),
                    );
                }
                self.needs_redraw = true;
            }
            Err(e) => {
                self.logs.push(&tool, crate::logs::Level::Error, &e);
                // A failed rebuild leaves a grammar this process already uses working.
                let in_use = matches!(language, Language::Plugin(p) if p.loaded());
                self.set_grammar_state(
                    language.id(),
                    if in_use {
                        GrammarState::Ready
                    } else {
                        GrammarState::Failed
                    },
                );
                self.notify(Level::Error, format!("Syntax plugin {}: {e}", language.id()));
            }
        }
    }

    pub(super) fn parsed(&mut self, buffer: BufferId, parsed: Parsed) {
        if let Some(buf) = self.buffers.get_mut(buffer) {
            buf.syntax.parsed(parsed);
        }
    }
}
