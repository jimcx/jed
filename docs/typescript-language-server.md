# Configuring typescript-language-server

jed starts [typescript-language-server](https://github.com/typescript-language-server/typescript-language-server) for JavaScript, JSX, TypeScript and TSX files with no configuration: it runs `typescript-language-server --stdio` in the workspace root, one server for all four languages (`languageId`s `javascript`, `javascriptreact`, `typescript`, `typescriptreact`). It wraps TypeScript's own `tsserver`, and in jed gives diagnostics, completion (with details resolved on selection), hover, go to definition, references, rename (also of the other files that use the name), formatting, code actions and quick fixes (including "Organize Imports") and semantic highlighting. Everything below is optional.

## 1. Installing it

```sh
npm i -g typescript@5 typescript-language-server
```

It needs Node.js and a TypeScript **5.x** (or 4.x) install: `tsserver.js` has to exist. TypeScript 7 (the native port, what a plain `npm i typescript` installs now) has no `tsserver.js`, and typescript-language-server 6 then stops with "Could not find a valid TypeScript installation" (jed shows it as the failed initialize message). Install `typescript@5` next to it, or give a project its own TypeScript (below).

A project that has both as dev dependencies (`npm i -D typescript typescript-language-server`) needs no global install, and jed prefers the project's:

- When `<workspace>/node_modules/.bin/typescript-language-server` exists, jed runs that instead of the one on `PATH` (the tool log says "Using the workspace's …").
- When `<workspace>/node_modules/typescript/lib/tsserver.js` exists, jed sends `initializationOptions.tsserver.path` pointing at that folder, so the language service is the project's TypeScript version, as in VS Code. (The server also finds a workspace's TypeScript itself; the explicit path makes it certain.) An `initializationOptions.tsserver.path` you set yourself is kept.

Only a bare `typescript-language-server` command gets this (the built-in's, or a `.workspace` / `settings.toml` `command` set to just that name); a path or another command runs as written. If the binary isn't found, jed says "Language server typescript-language-server not found"; if it starts and then fails, the message ends with its last stderr line. The Tools view has its log. To use another server or path, set it in `.workspace` (used once you trust the workspace) or in `~/.config/jed/settings.toml` (always); each language id has its own entry:

```toml
[lsp.typescript]
command = "/opt/node/bin/typescript-language-server"
args = ["--stdio"]
[lsp.typescript.init_options.tsserver]
path = "/opt/ts/node_modules/typescript/lib"
```

(Repeat for `javascript`, `javascriptreact` and `typescriptreact`: equal entries share one server.)

TypeScript 7 also ships its own native language server; this page doesn't cover it (set it up as a custom server, below, if you use it).

## 2. Projects and single files

The server's root is the workspace root. tsserver itself finds the nearest `tsconfig.json` / `jsconfig.json` above each open file, so a monorepo with a config per package works from the repository root (the plan's "nearest ancestor with a config as root" isn't needed). With a config, go to definition, references and rename see every file the config includes, once tsserver has loaded the project (answers can come back empty for a second or two while it loads).

A lone file with no `tsconfig.json` / `jsconfig.json` is an *inferred project*: tsserver checks it with default options (`strict` off, JavaScript not type-checked unless the file has `// @ts-check`), and follows only the imports it can resolve (relative paths and `node_modules` packages); there are no cross-file results for files nothing imports. Add a `jsconfig.json` (`{ "compilerOptions": { "checkJs": true } }`) to get checking in a JavaScript project.

## 3. Formatting and progress

Format Document sends jed's tab size and "insert spaces" for the buffer (its own detected / `.editorconfig` / settings values), not the server's defaults of 4 spaces. tsserver's own formatter settings (braces, semicolons) are its defaults. `window/logMessage` goes to the tool log (its "Initialized:" line names the server's version); `$/typescriptVersion` is only in the debug log and the protocol trace; project loading shows as progress in the status bar when the server reports it.

## 4. Alternatives

- **vtsls** (`npm i -g @vtsls/language-server`, command `vtsls --stdio`): wraps VS Code's own TypeScript extension, so its behaviour is the closest to VS Code's (more code actions, update imports on file rename, workspace TypeScript selection). Set it as above.
- **Deno** (`deno lsp`): for `deno.json` projects, with its own module resolution (URL imports, `npm:` / `jsr:` specifiers); set `command = "deno"`, `args = ["lsp"]` and `init_options = { enable = true }`.
- **biome lsp-proxy** / **eslint** (`vscode-eslint-language-server`): lint and format diagnostics next to tsserver's. jed runs one server per language, so these can't run alongside typescript-language-server yet; running one *instead* works (with its own limits: no completion or definition from biome / eslint). Multiple servers per language are not built.
