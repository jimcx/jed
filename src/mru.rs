//! Most-recently-used commands and files, the model (no app state: `App::mru` holds the two
//! lists, `app/palette.rs` records commands, `app/files.rs` files). The command palette and quick
//! open list them first while their input is empty, as VS Code's "recently used"; in memory only,
//! so a restart forgets them.

use std::path::PathBuf;

use crate::command::Command;

/// Both lists keep this many items.
pub const MAX: usize = 20;

/// The last commands picked from the palette and the last files opened, most recent first.
#[derive(Debug, Default)]
pub struct Mru {
    pub commands: Vec<Command>,
    /// Workspace-relative when inside the root, else absolute.
    pub files: Vec<PathBuf>,
}

/// Puts `item` first, dropping an earlier equal one and anything beyond [`MAX`].
pub fn record<T: PartialEq>(list: &mut Vec<T>, item: T) {
    list.retain(|x| *x != item);
    list.insert(0, item);
    list.truncate(MAX);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::command::Motion;

    #[test]
    fn record_puts_the_newest_first() {
        let mut list = Vec::new();
        record(&mut list, Command::Save);
        record(&mut list, Command::ToggleSidebar);
        assert_eq!(list, [Command::ToggleSidebar, Command::Save]);
    }

    #[test]
    fn record_moves_an_earlier_copy_to_the_front_without_duplicating_it() {
        let a = Command::Move {
            motion: Motion::Left,
            select: false,
        };
        let b = Command::Move {
            motion: Motion::Right,
            select: false,
        };
        let mut list = Vec::new();
        for item in [a, b, a] {
            record(&mut list, item);
        }
        assert_eq!(list, [a, b], "a moved first, still one copy");
    }

    #[test]
    fn record_caps_at_max() {
        let mut files: Vec<PathBuf> = Vec::new();
        for i in 0..MAX + 5 {
            record(&mut files, Path::new(&format!("f{i}.txt")).to_path_buf());
        }
        assert_eq!(files.len(), MAX);
        assert_eq!(files[0], Path::new("f24.txt"));
        assert_eq!(files[MAX - 1], Path::new("f5.txt"));
        // A file the cap dropped comes back at the front; the list still fits.
        record(&mut files, Path::new("f0.txt").to_path_buf());
        assert_eq!(files.len(), MAX);
        assert_eq!(files[0], Path::new("f0.txt"));
        assert!(
            !files.contains(&Path::new("f5.txt").to_path_buf()),
            "the oldest one went"
        );
    }
}
