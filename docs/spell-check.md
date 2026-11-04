# Dictionaries

The checker uses Hunspell dictionaries (read by the `spellbook` crate). Australian English
(`en_AU`) is built into jed with its licence (`assets/dictionaries/en_AU/`, zstd-packed into the
binary by `build.rs`), so it works out of the box. The first load also writes it out, with its
licence and `NOTICES.md`, to `~/.local/share/jed/` (`$XDG_DATA_HOME/jed`) when `.bundle-stamp` there
doesn't name this build's bundle (so a new jed version refreshes them; a matching stamp costs one
small read). To use another language, set `spell_check_language = "de_DE"` (any Hunspell name;
default `"en_AU"`) and install its files:

    scripts/fetch-dictionary de_DE

That downloads `de_DE.aff`, `de_DE.dic` and the dictionary's licence/readme files from the
LibreOffice `dictionaries` repository into `<config dir>/dictionaries/de_DE/` (the config dir is
where `settings.toml` lives). Keep the licence files with the dictionary. Run from a jed checkout;
needs python3 and curl. `scripts/fetch-dictionary --bundled <lang>` refreshes the bundled
`assets/dictionaries/<lang>/` instead. Changing the setting loads the new dictionary at once; if
its files are missing jed uses `en_AU` and says so once.

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
| `spell_check_language` | `"en_AU"` | the dictionary: a Hunspell name; `en_AU` is built in, others via `scripts/fetch-dictionary` |
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
