# Installing the spell checker's dictionary

jed bundles only a tiny placeholder word list. For real checking, install the Australian English
list yourself:

1. Download `en_AU.dic`, `en_AU.aff` and `README_en_AU.txt` from the `en/` folder of the
   LibreOffice `dictionaries` repository (SCOWL-derived; its licence terms are in
   `README_en_AU.txt`, keep it with the files).
2. Expand them to a plain word list (one lower-case word per line):
   `scripts/gen-dictionary en_AU.dic en_AU.aff > en_AU.txt` (python3; run from a jed checkout).
3. Put the result at `<config dir>/dictionaries/en_AU.txt` (the config dir is where
   `settings.toml` lives, e.g. `~/.config/jed/`).

Restart jed. Without that file the checker falls back to the placeholder list and says so once.

# Your own words

Two plain-text files, one word per line, `#` comments (a whole line or after the word), blank lines
ignored; matching is case-insensitive (an entry `Colour` also accepts `colour`); a line that isn't a
word (spaces, digits, over 64 characters) is skipped and logged:

- `<config dir>/dictionary.txt`: every workspace.
- `<workspace>/.jed/dictionary.txt`: that project's terms. Writing it also adds `!dictionary.txt` to
  `.jed/.gitignore` (which otherwise ignores everything in `.jed/`), so it can be committed.

Edits to them apply as soon as they are saved. For a few words, `spell_check_ignore_words = [...]` in
`[settings]` works too.

# Settings

All under `[settings]` in `settings.toml` or `.workspace` (flat keys, like the others):

| key | default | |
|-----|---------|---|
| `spell_check_scope` | `"all"` | `all` (comments, strings, identifiers), `comments_and_strings` or `off` |
| `spell_check_min_length` | `3` | shorter words are not checked |
| `spell_check_problems` | `false` | list misspellings in the Problems panel |
| `spell_check_language` | `"en_AU"` | the dictionary; only `en_AU` ships |
| `spell_check_ignore_words` | `[]` | extra accepted words |
| `spell_check_off_languages` | `[]` | language ids not checked, e.g. `["log"]` |

Files over `syntax_max_bytes` are not checked. Identifiers are found through each language's
highlight query (names it colours as variable, function, type, property, ...), so they are checked
wherever the language is highlighted; Markdown prose is always checked and its code only with `all`.
The marks use the theme key `editorSpell.foreground`.
