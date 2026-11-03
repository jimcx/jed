# Bundled dictionaries

Used by `src/spellcheck.rs` (`include_str!`). Lower case, one word per line, `#` lines are comments.

- `en_AU.txt` — a **tiny placeholder** (about 250 common words), the fallback when the user has
  not installed a dictionary. It is not the real Australian English list: the real one is not
  bundled (licence notices would have to ship with it), the user installs it, see
  [docs/spell-check.md](../../docs/spell-check.md). `Dictionary::is_placeholder()` tells.
- `code.txt` — programming terms accepted in every language (jed's own list).
- `lang/<name>.txt` — keywords and standard library names for one language, loaded only when a
  buffer in that language is checked (`spellcheck::language_terms`; jed's own lists).

The installable list comes from the SCOWL `en_AU` Hunspell dictionary (LibreOffice
`dictionaries`, `en/`), expanded by `scripts/gen-dictionary`; its licence terms are in that
download's `README_en_AU.txt`.
