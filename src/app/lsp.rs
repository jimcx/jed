//! Language servers from the app's side: starting the ones open files need, their events, and
//! stopping them on quit. The servers themselves live in `crate::lsp`.

use super::App;
use crate::syntax::Language;

impl App {
    /// Starts (or stops) servers to match the open files' languages, `.workspace` `[lsp.*]` and
    /// trust. Runs each event-loop turn, but only does anything after `servers_dirty` is set (a
    /// buffer added, languages re-detected, `.workspace` reloaded, trust decided) and once
    /// `start` has run.
    pub(super) fn sync_servers(&mut self) {
        if !self.servers_dirty || !self.lsp.started() {
            return;
        }
        self.servers_dirty = false;
        let mut languages: Vec<Language> = Vec::new();
        for (_, buf) in self.buffers.iter() {
            let l = buf.syntax.language();
            if !languages.contains(&l) {
                languages.push(l);
            }
        }
        self.lsp
            .sync(&self.workspace.root, &languages, &self.workspace.config, self.trust);
    }

    pub(super) fn lsp_event(&mut self, event: crate::lsp::Event) {
        if let Some(notice) = self.lsp.handle(event) {
            self.notify(notice.level, notice.text);
        }
    }

    /// Asks every server to exit and waits for them (bounded: see `lsp::Manager::shutdown`).
    /// `run` calls it on quit; tests call it after `Quit`.
    pub async fn shutdown_servers(&mut self) {
        self.lsp.shutdown().await;
    }
}
