# Syntax plugins

jed has built-in syntax highlighting for a fixed list of languages. A **syntax plugin** adds another one without rebuilding jed: a tree-sitter grammar's sources plus a small manifest, in `~/.config/jed/languages/<id>/`. jed compiles the grammar with your C compiler the first time it starts (in the background; files open uncoloured until it finishes) and loads the result as a shared library.

Why this works: a tree-sitter grammar's whole interface is one C function, `const TSLanguage *tree_sitter_<name>(void)`, and the tables it returns carry an ABI version that jed checks (it accepts 13 to 15). Neovim and Helix load grammars the same way.

You need a C compiler (`cc`, or `$CC`; a grammar with a `scanner.cc` also needs `c++` / `$CXX`): Xcode's command line tools on macOS, `build-essential` or `gcc` on Linux.

## 1. Walk-through: Lua

```sh
jed --new-language lua
```

creates `~/.config/jed/languages/lua/` with a commented `language.toml`, an empty `src/` and an empty `queries/`, and prints the next steps. Get the grammar (any tree-sitter grammar repository with a generated `src/parser.c` works; this one is [tree-sitter-grammars/tree-sitter-lua](https://github.com/tree-sitter-grammars/tree-sitter-lua)):

```sh
git clone https://github.com/tree-sitter-grammars/tree-sitter-lua /tmp/tree-sitter-lua
cp -r /tmp/tree-sitter-lua/src/{parser.c,scanner.c,tree_sitter} ~/.config/jed/languages/lua/src/
cp /tmp/tree-sitter-lua/queries/highlights.scm ~/.config/jed/languages/lua/queries/
```

(`scanner.c` only if the grammar has one; `src/tree_sitter/` holds the three headers `parser.c` includes. There is no tree-sitter CLI or `grammar.js` step: jed compiles the generated C.)

Edit `~/.config/jed/languages/lua/language.toml`: the generated file already has `id`, `name` and `symbol = "tree_sitter_lua"`; set what Lua needs.

```toml
line_comment = "--"
brackets = ["()", "[]", "{}"]
pairs = [
    { open = "{", close = "}" },
    { open = "[", close = "]" },
    { open = "(", close = ")" },
    { open = "\"", close = "\"", not_in = ["string"] },
    { open = "'", close = "'", not_in = ["string", "comment"] },
]
[files]
extensions = ["lua"]
```

Check it:

```sh
jed --check-language lua --sample hello.lua
```

This reads the manifest, builds the grammar (the compiler command and its output are shown), loads the library, compiles the queries, lists captures that no theme colour exists for, and prints the sample file coloured in your terminal. It ends with `OK` and exits 0, or with `FAILED` and exits 1 after `✗` lines saying what is wrong ([troubleshooting](#7-troubleshooting)). It leaves the built library in place, so the next jed start finds it ready.

Now start jed and open a `.lua` file: the status bar says "Lua", it is coloured when the background build finishes (the first start only; the next start loads the library at once), Cmd+/ (Ctrl+/ on Linux) comments with `--`, and quotes close as `pairs` says. A `.workspace` association `"*.rockspec" = "lua"` opens rockspecs as Lua too. For a language server, see [Language servers](#6-language-servers).

## 2. `language.toml`

Everything but `id` is optional. A manifest is read leniently: an unknown key or a bad value is reported (at start in the status bar; `--check-language` shows the line) and ignored, the rest still applies. The generated file has every key with a comment; the vendored template `examples/languages/cmake/` is jed's own CMake support written as a plugin (a test keeps it equal to the built-in).

| Key | Meaning | VS Code's equivalent |
|---|---|---|
| `id` | The language id: letters, digits, `-`, `_`, `.`. `.workspace` associations and `[lsp.<id>]` call it this, and it is the LSP `languageId`. The directory must have the same name. It can't be a built-in's id (`rust`, `cmake`, …) | `id` |
| `name` | The status bar and language picker name. Default: the id | |
| `symbol` | The C function `parser.c` exports. Default `tree_sitter_<id>` (`-` and `.` become `_`). Check `grep -n "TS_PUBLIC const TSLanguage" src/parser.c` | |
| `line_comment` | What Comment Line inserts. Leave out for block-comment-only languages | `comments.lineComment` |
| `brackets` | `["()", "[]"]`: pairs for matching and auto-indent, single characters | `brackets` |
| `pairs` | `{ open, close, not_in }` tables that close themselves when the opener is typed. `not_in` lists `"string"` and / or `"comment"`: where the opener does not auto-close (needs the grammar). Default `{}` `[]` `()` | `autoClosingPairs` |
| `surround` | Pairs typing an opener wraps around a selection with. Default `{}` `[]` `()` and quotes | `surroundingPairs` |
| `insert_spaces` | `false` to indent with tabs whatever the user's setting says (Makefile-style). Default: the user's setting | `[language]` `editor.insertSpaces` |
| `tree_tokens` | Whether the grammar's leaves are the language's tokens, so a bracket counts only as a token. `true` for nearly every grammar | |
| `comment_kinds`, `string_kinds`, `code_kinds` | Node kinds that are comments, strings, or code inside a string (an interpolation). A kind whose name contains `comment` or `string` is recognised anyway. `--check-language` lists what jed sees | |
| `[files] extensions` | Extensions without the dot, case-insensitive; `tar.gz`-style works | `extensions` |
| `[files] filenames` | Exact file names | `filenames` |
| `[files] shebangs` | Program names in a `#!` line (`#!/usr/bin/env foo`), a version suffix allowed (`foo3`) | `firstLine` |

Auto-close, wrapping and indent rules that use `not_in` or the kinds need the grammar's tree, so they behave as if nothing is a string or comment until the grammar is loaded.

### Which language a file gets

In this order, first match wins: a `.workspace` `[files] associations` entry; built-in exact file names (`Makefile`, `CMakeLists.txt`, `Dockerfile`, `Cargo.lock`, …); plugin `filenames`; plugin `extensions`; built-in extensions; then shebangs (built-in programs first, then plugins').

So a plugin extension beats a built-in one (a plugin claiming `h` wins over C++), but a plugin can never take a built-in exact name: the copied CMake template, with its `filenames = ["CMakeLists.txt"]`, still opens `CMakeLists.txt` as the built-in CMake. To try the template on such a file, drop its `[files]` claims, give it another id (`cmake2`), and associate the file in `.workspace`: `"CMakeLists.txt" = "cmake2"`. Its highlighting then equals the built-in's.

## 3. How captures become colours

`queries/highlights.scm` is a normal tree-sitter highlight query. jed has no per-plugin theme: a capture name maps to one of its theme keys, so any theme that colours the built-in languages colours yours.

- The capture's dotted name is looked up whole, then its longest dotted prefix (`@function.method.call` finds `function.method`, then `function`).
- At each step the name is tried in the key list, then in the alias list (Neovim's older and newer names).
- A name with no match, and names starting with `_`, paint nothing: the node keeps the colour an earlier pattern gave it, or none. `--check-language` lists such captures and which keys are close.

Theme keys: `comment`, `constant`, `constant.builtin`, `constructor`, `escape`, `function`, `keyword`, `keyword.control`, `label`, `markup.heading`, `markup.link`, `markup.list`, `markup.quote`, `markup.raw`, `number`, `operator`, `property`, `punctuation`, `punctuation.special`, `punctuation.tag`, `string`, `tag`, `tag.attribute`, `type`, `type.builtin`, `variable`, `variable.builtin`, `variable.parameter`.

Aliases: `boolean` is `constant.builtin`; `string.special.key` is `property`; `string.escape` is `escape`; `keyword.conditional`, `keyword.repeat`, `keyword.return`, `conditional`, `repeat`, `include` and `exception` are `keyword.control`; `text.title` is `markup.heading`; `text.literal` is `markup.raw`; `text.uri` and `text.reference` are `markup.link`.

Rename captures in a copy of the query to get other colours; most upstream queries already use these names.

### `jed.scm`: your additions

`queries/jed.scm`, if present, is appended after `highlights.scm`. For the same node a later pattern wins, so put corrections there (recolour control keywords as `@keyword.control`, mark parameters) without editing the upstream file, which you can then update by copying.

## 4. Building

On the first start (and whenever the sources change) jed runs, in the plugin directory, one background build at a time after startup:

```
cc -shared -fPIC -O2 -I src src/parser.c src/scanner.c -o parser.so
```

`$CC` replaces `cc`; with a `scanner.cc` the files are compiled to objects (`cc -c`, `$CXX -c`) and linked with `$CXX` / `c++`. On macOS `-dynamiclib` replaces `-shared` and the library is `parser.dylib`. The library is written to a temporary file and renamed, so a running jed's copy stays valid.

`build.toml` stamps a SHA-256 of the `.c`, `.cc` and `.h` files in `src/` and `src/tree_sitter/`, the compilers (with their `--version`), the OS and the architecture. Nothing is rebuilt while it matches and the library exists; change `scanner.c`, switch compilers or copy the directory to another platform and the next start rebuilds. A failed build keeps the old library and stamp.

Instead of sources you can put a prebuilt `parser.so` (`parser.dylib` on macOS) in the plugin directory; with no `src/parser.c` it is loaded as is.

Each grammar has a row in the **Tools** view with its state (building, ready, failed) and a log: the command line, the compiler's output and `grammar loaded`, or the error. A failure is reported once in the status bar, the language stays uncoloured, and jed keeps running. **Languages: Rebuild Syntax Plugins** (command palette, unbound) rebuilds every plugin now; a grammar already in use this session is rebuilt on disk and used from the next start.

## 5. ABI versions

jed (tree-sitter 0.27) loads grammars of ABI 13, 14 and 15. The number is `#define LANGUAGE_VERSION` near the top of `src/parser.c`. An older one is rejected (`grammar ABI version 12 is not supported (jed accepts 13 to 15); regenerate it with a newer tree-sitter`): regenerate `parser.c` from `grammar.js` with a current `tree-sitter generate`, or take a newer release of the grammar.

## 6. Language servers

A plugin's id works in `[lsp.<id>]`, in `~/.config/jed/settings.toml` (always trusted) or `.workspace` (asks once, like any command from a workspace):

```toml
[lsp.lua]
command = "lua-language-server"
args = []
```

Precedence: a trusted `.workspace` entry, then `settings.toml`, then the built-ins. The id is also the LSP `languageId`.

## 7. Trust and safety

Only your own `~/.config/jed/languages/` is read. A `.workspace` can associate files with a plugin's id but can never add, build or point at a plugin: a `[languages]` table there is an unknown key and the whole `.workspace` is rejected with an error, and nothing under a workspace is ever compiled or loaded.

A plugin is native code running inside jed. A library is never unloaded, and a grammar that crashes crashes jed (crash backups restore your unsaved text). Only install grammars you trust.

## 8. Troubleshooting

`jed --check-language <id>` prints these; the app reports the same text as `Syntax plugin <id>: …` in the status bar and in the grammar's Tools log.

| Message | Cause and fix |
|---|---|
| `<file>: unknown key \`x\`` / `\`x\` must be …` | A manifest typo. The line number is shown. Other keys still apply |
| `\`<id>\` is a built-in language's id` | Choose another id: a plugin can't replace a built-in language |
| `language \`<id>\` is defined twice` | Two directories claim one id (a directory name must equal `id`) |
| `\`id\` is "x" but the directory is "y"` | Rename the directory or the `id` |
| `no src/parser.c and no prebuilt parser.so in <dir>` (the app: `<dir>: no src/parser.c and no parser.so`) | The grammar's files are not copied in |
| `build failed: cannot run \`cc\`: … (set $CC / $CXX to a compiler)` | No C compiler on `PATH`. Install one or set `$CC` |
| `build failed: \`cc\` failed (exit status: 1): <first error>` | A compile error; the whole output is in the log. Often a missing header: copy `src/tree_sitter/*.h` too |
| `<lib>: no symbol \`tree_sitter_x\`` | `symbol` is wrong; find the exported name with `grep "TS_PUBLIC const TSLanguage" src/parser.c` |
| `grammar ABI version N is not supported (jed accepts 13 to 15)` | See [ABI versions](#5-abi-versions) |
| `<file>: highlight query, line L column C: <message>` | The query names a node or field the grammar doesn't have (a query written for another version of the grammar), or has a syntax error. `L` is within the named file (`highlights.scm` or `jed.scm`) |
| `@name maps to no theme key and paints nothing` | A note, not an error: rename the capture, see [captures](#3-how-captures-become-colours) |
| `build failed: cannot read the sources: …` / `cannot install <lib>: …` / `cannot create …` / `built, but cannot write build.toml: …` | The plugin directory isn't readable or writable |
| `<lib>: <the system loader's error>` | The library can't be opened: a prebuilt one for another platform or architecture, or a dependency missing. Delete it and copy in the sources |
| `Syntax plugins are already building` | **Rebuild** was run while a build is in progress |

If a file doesn't pick the language: `--check-language` doesn't test detection; check `extensions` (no dot), and remember built-in exact names win, and that a `.workspace` association with an unknown id is reported.
