//! Test helper: where the repository (the workspace root) is.

use std::path::Path;

/// The repository root, the parent of jed's package directory (`CARGO_MANIFEST_DIR` is `jed/`):
/// where `.venv`, `docs/`, `rust-toolchain.toml` and `claude-plugin/` live. Jed's own files
/// (`src/`, `examples/`) stay under `CARGO_MANIFEST_DIR`.
pub(crate) fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("jed/ has a parent")
}

#[cfg(test)]
mod tests {
    use super::repo_root;

    #[test]
    fn repo_root_is_the_workspace_root_not_jeds_package() {
        let root = repo_root();
        assert!(root.join("rust-toolchain.toml").is_file(), "{}", root.display());
        assert!(root.join("jed/Cargo.toml").is_file());
        assert!(root.join("docs/forges.md").is_file());
    }
}
