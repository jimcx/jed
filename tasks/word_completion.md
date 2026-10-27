---
id: word_completion
name: Word-based completion
summary: "without a language server (plain text, YAML, TOML, Markdown, … or a server returning nothing), Ctrl+Space and quick suggestions offer the words in the open buffers (current buffer first, then others; VS Code's `editor.wordBasedSuggestions: matchingDocuments`), filtered with the same `completion::Filter`; collected off the UI thread per buffer and cached by revision; capped (e.g. 2 MB of text, 20k words); a setting to turn it off"
status: open
priority: 0
dependencies: []
---

