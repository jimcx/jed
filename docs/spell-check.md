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

# What you see

Misspelled words are underlined in their own colour (`editorSpell.foreground`), just the misspelled
piece (`recieve` in `recieveMessage`). Hovering shows `Unknown word "recieve"`. They are hints:
a real error or warning on the same line wins the status bar. Checking runs a moment (300 ms) after
you stop typing and only re-reads the lines you touched; a file is checked in full when opened and
when a setting or a dictionary file changes, in the background. The word you are typing is not
underlined until the caret leaves it or you type a separator. Misspellings are listed in the
Problems panel (under `Spelling`) only with `spell_check_problems = true`. Diff, hex and shell tabs,
log tabs and files over `syntax_max_bytes` are not checked.

# Commands

All in the palette ("Spelling: ..."), none bound to a key. The word is the misspelling under each
selection, else the word at the caret.

- Go to Next / Previous Problem: the caret to the next / previous misspelling, wrapping.
- Add Word to User Dictionary / to Workspace Dictionary: appends to `<config dir>/dictionary.txt` /
  `<workspace>/.jed/dictionary.txt`; the underline goes in every open file. With several selections
  each distinct word (the first 256) is added.
- Ignore Word in This File: accepted until the file is closed, not saved.
- Toggle Spell Check: off / on for this file, until it is closed.
- Open User Dictionary: opens the file, creating it empty when missing.

They refuse (a notice) in a read-only file and in diff, hex and shell tabs (Open User Dictionary
works anywhere). Ctrl+. (Cmd+. on macOS) or the lightbulb on a misspelling lists `Replace with
"receive"` (up to 8, in the word's case; inside an identifier only the piece changes), then
`Add "recieve" to user dictionary`, `... to workspace dictionary` and `Ignore in this file`, after
any language server fixes. A replace changes every selection that holds the same misspelling, as one
undo step.
