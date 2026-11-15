# Bundled dictionaries

Used by `src/spellcheck.rs`; `build.rs` zstd-packs `en_AU/` into the binary (`src/assets.rs`), so a refreshed dictionary needs a rebuild.

- `en_AU/` — the Australian English Hunspell dictionary (`en_AU.aff`, `en_AU.dic`) with its
  licence file (`README_en_AU.txt`: the SCOWL, Ispell and WordNet notices; the folder's generic
  `license.txt` is GPL and belongs to another dictionary, so it is not shipped), from the LibreOffice
  `dictionaries` repository (SCOWL-derived). Refresh or add a language with
  `scripts/fetch-dictionary --bundled <lang>`; the licence file must ship with the dictionary.
- `code.txt` — programming terms accepted in every language (jed's own list; lower case, one word
  per line, `#` lines are comments).
- `lang/<name>.txt` — keywords and standard library names for one language, loaded only when a
  buffer in that language is checked (`spellcheck::language_terms`; jed's own lists).

Other languages are not bundled: the user fetches them into `<config dir>/dictionaries/<lang>/`
([docs/spell-check.md](../../docs/spell-check.md)).
