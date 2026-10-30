# Configuring sourcekit-lsp

jed starts [sourcekit-lsp](https://github.com/swiftlang/sourcekit-lsp) for Swift files with no configuration: it runs `sourcekit-lsp` (no arguments) in the workspace root. It gives diagnostics, completion, hover, go to definition, references, rename, formatting and code actions. Everything below is optional.

## 1. Installing sourcekit-lsp

It ships with the Swift toolchain; there is nothing to install separately.

- **macOS:** install Xcode or the command line tools (`xcode-select --install`). `sourcekit-lsp` is usually not on `PATH`: when it isn't, jed asks `xcrun --find sourcekit-lsp` and runs what that returns.
- **Linux:** install a toolchain from [swift.org](https://www.swift.org/install/linux/) or with [swiftly](https://www.swift.org/install/), and put its `usr/bin` on `PATH`.

If it isn't found, jed says "Language server sourcekit-lsp not found"; if it starts and then fails, the message ends with its last stderr line. The Tools view has its log (including which path xcrun gave). To use another binary, set it in `.workspace` (used once you trust the workspace) or in `~/.config/jed/settings.toml` (always):

```toml
[lsp.swift]
command = "/opt/swift/usr/bin/sourcekit-lsp"
args = []
```

## 2. Packages and single files

sourcekit-lsp reads the build settings under the workspace root: `Package.swift` (SwiftPM), `compile_commands.json`, or `buildServer.json` (Xcode projects through a build server such as xcode-build-server). With one of them it knows every file of each target, so go to definition, references and rename work across files, after it has indexed the package (the status bar shows its progress). SwiftPM runs a build in `.build/` to produce the index; the first one can take a while.

Without any of them (a lone `.swift` file) it still reports that file's errors and offers completion, hover and go to definition inside the file and the standard library, but it knows nothing about other files: no cross-file results.

The workspace root must be where `Package.swift` is. Open the package's folder, not a parent of it.
