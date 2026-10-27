---
id: block_comment_bracket
name: Block comment and bracket jumps
summary: "\"Toggle Block Comment\" (`edit.blockComment`, Shift+Alt+A on both platforms, VS Code's) with each language's block tokens in `LanguageConfig` (`/* */` C / C++ / Rust / JSONC / CSS-like, `<!-- -->` XML / Markdown, `\"\"\"` Python as VS Code; none: nothing): per selection, remove when the selection (trimmed of surrounding whitespace) starts and ends with the tokens, else wrap it (`/* ` … ` */`); an empty cursor inserts `/*  */` with the cursor in the middle; selections keep their text; one `EditKind::Other` step, every selection. \"Go to Bracket\" (`editor.jumpToBracket`, Cmd+Shift+\\\\ on macOS, Ctrl+Shift+\\\\ on Linux): each cursor to its bracket's partner (`brackets::match_at`), else to the closing bracket of the innermost pair around it (`brackets::Openers`); \"Select to Bracket\" (unbound): the selection grows to cover the enclosing pair, brackets included. Both at every cursor (the 61k-selection budget). Cmd bindings update `docs/terminals/`"
status: open
priority: 0
dependencies: []
---

