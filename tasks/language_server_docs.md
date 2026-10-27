---
id: language_server_docs
name: Language server setup docs
summary: docs for installing and configuring language servers
status: open
priority: 0
dependencies: []
---

Human-facing `docs/language-servers.md` (folding in `docs/clangd.md` and `docs/ty.md`, which exist: ty's install, `.venv`, `ty.toml` and SCons are in the latter) (linked from a README / `docs/` index): which servers jed starts by itself (`clangd` for C / C++, `ty server` for Python, `rust-analyzer`) and how to install each on macOS (Homebrew, `uv tool install ty`, `rustup component add rust-analyzer`) and Linux (apt / dnf, uv, rustup); making them findable (`PATH`, a project `.venv`); clangd's `compile_commands.json` (CMake `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, `bear -- make`, SCons) and `.clangd`; ty's project config (`pyproject.toml` / `ty.toml`, the venv) and the SCons support jed adds; adding any other server through `.workspace` `[lsp.<language id>]` (command, args, `initialization_options`, with worked examples, e.g. a YAML or TOML server) and why workspace trust asks first; what works (the v1 features) and the keys for each; troubleshooting (the status bar / Problems, `--log-level` and "Log: Open" with the `lsp::wire` / `lsp::stderr` targets, a server that fails is not retried until restart); kept current when servers or config keys change (add a line to CLAUDE.md's conventions)
