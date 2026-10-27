//! How jed was started (`main`'s `-s` / `--single`), and the one answer to "is this command or
//! side view available": `execute`, the palette, the activity bar and its clicks all ask here.

use crate::command::Command;
use crate::layout::SidePanel;

/// The notice a command that isn't available in single file mode shows.
pub const UNAVAILABLE: &str = "Not available in single file mode";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// A folder (or a file's workspace): everything.
    #[default]
    Workspace,
    /// `jed -s <file>`: one file and nothing per-workspace — no side panel, bottom panel, quick
    /// open, workspace search, language servers, tasks, git or file history; no session, recent
    /// entry or trust dialog; only the file watched.
    SingleFile,
}

impl Mode {
    pub fn is_single(self) -> bool {
        self == Mode::SingleFile
    }

    /// Whether `cmd` can run in this mode.
    pub fn allows(self, cmd: Command) -> bool {
        match self {
            Mode::Workspace => true,
            Mode::SingleFile => !matches!(
                cmd,
                // Side views and the bottom panel.
                Command::ToggleSidebar
                    | Command::FocusExplorer
                    | Command::Explorer(_)
                    | Command::FindInFiles
                    | Command::Search(_)
                    | Command::FocusScm
                    | Command::Scm(_)
                    | Command::GitHistory(_)
                    | Command::ShowTools
                    | Command::Tools(_)
                    | Command::TogglePanel
                    | Command::ToggleProblems
                    | Command::Problems(_)
                    // Per-workspace things.
                    | Command::QuickOpen
                    | Command::ManageWorkspaceTrust
                    | Command::OpenRecentWorkspace
                    | Command::RunTask
                    | Command::NewShell
                    | Command::MoveTerminalToEditor
                    | Command::MoveTerminalToArea
                    | Command::InstallShellIntegration
                    | Command::KillTask
                    | Command::CloseTask
                    | Command::Task(_)
                    | Command::Shell(_)
                    | Command::ToggleToolTrace
                    | Command::RestartServer
                    | Command::NewUntitledFile
                    | Command::SaveAs
                    | Command::ShowFileHistory
                    | Command::RestoreFileHistory
                    // Git.
                    | Command::StageHunk
                    | Command::UnstageHunk
                    | Command::StageFile
                    | Command::UnstageFile
                    | Command::StageAll
                    | Command::UnstageAll
                    | Command::OpenChange
                    | Command::ToggleBlame
            ),
        }
    }

    /// Whether the side view `panel` can be shown (its activity bar icon is live).
    pub fn allows_view(self, panel: SidePanel) -> bool {
        self.allows(view_command(panel))
    }
}

/// The command that shows `panel`.
fn view_command(panel: SidePanel) -> Command {
    match panel {
        SidePanel::Explorer => Command::FocusExplorer,
        SidePanel::Search => Command::FindInFiles,
        SidePanel::SourceControl => Command::FocusScm,
        SidePanel::Tools => Command::ShowTools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_file_mode_keeps_editing_and_drops_views() {
        assert!(Command::all().into_iter().all(|c| Mode::Workspace.allows(c)));
        let single = Mode::SingleFile;
        for c in [
            Command::Save,
            Command::CloseTab,
            Command::Quit,
            Command::ShowCommands,
            Command::GoToLine,
            Command::OpenFind,
            Command::SplitRight,
            Command::AddNextMatch,
            Command::CompareWithSaved,
            Command::ReloadFromDisk,
            Command::ChangeLanguageMode,
            Command::SaveAll,
        ] {
            assert!(single.allows(c), "{}", c.name());
        }
        for c in [
            Command::ToggleSidebar,
            Command::QuickOpen,
            Command::FindInFiles,
            Command::TogglePanel,
            Command::RunTask,
            Command::NewShell,
            Command::MoveTerminalToEditor,
            Command::MoveTerminalToArea,
            Command::InstallShellIntegration,
            Command::Shell(crate::command::ShellCommand::ScrollUp),
            Command::NewUntitledFile,
            Command::SaveAs,
        ] {
            assert!(!single.allows(c), "{}", c.name());
        }
        assert!(SidePanel::ALL.into_iter().all(|p| !single.allows_view(p)));
        assert!(SidePanel::ALL.into_iter().all(|p| Mode::Workspace.allows_view(p)));
    }
}
