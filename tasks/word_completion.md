---
id: word_completion
name: Word-based completion
summary: offer words from open buffers when no server suggests
status: done
priority: 0
dependencies: []
---

Without a language server (plain text, YAML, TOML, Markdown, … or a server returning nothing), Ctrl+Space and quick suggestions offer the words in the open buffers (current buffer first, then others; VS Code's `editor.wordBasedSuggestions: matchingDocuments`), filtered with the same `completion::Filter`; collected off the UI thread per buffer and cached by revision; capped (e.g. 2 MB of text, 20k words); a setting to turn it off

- ✅ Done.
  - Model in `word_completion.rs` (`collect`, `WordCache` keyed by buffer and revision); the fallback runs in `app/completion.rs` (`word_completion`, `show_words`, `words_collected`) when no server can answer, the server lacks completion, or its answer shows nothing; results arrive as `ViewEvent::Words` (generation-checked). Setting `word_based_suggestions` (default true).
  - Other buffers count when their `syntax.language()` matches; config-file buffers are never sources. `Item::word`; item sort keys widened to 7 digits.
  - Existing tests that expected no words without a server set `word_based_suggestions = false`.
  - Deferred: no background pre-warm of other buffers' words (collected on demand); `collect` may emit a truncated last word at the 2 MB cap.
  - Still to check by hand: type a prefix of an earlier word in a plain text / Markdown file (list after ~10 ms); two same-language tabs, Ctrl+Space offers the other tab's words after the current's; setting false restores the old behaviour.
