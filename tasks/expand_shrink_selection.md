---
id: expand_shrink_selection
name: Expand / shrink selection
summary: grow / shrink each selection to enclosing syntax nodes
status: open
priority: 0
dependencies: []
---

(Shift+Alt+Right / Left on Linux, Ctrl+Shift+Cmd+Right / Left on macOS, as VS Code): grow each selection to the enclosing tree-sitter node (word first, then string contents, the string, the bracket contents, the brackets, the statement, the block, …), shrink back through the same history (kept per view, cleared on other selection changes); without a tree: word, line, all; at every selection
