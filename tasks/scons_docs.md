---
id: scons_docs
name: 'SCons: a real SCons for `site_scons` in the docs'
summary: "helpers in `site_scons/` that `from SCons.Script import …` / `import SCons.Util` only resolve when ty sees the `SCons` package. Document both ways, checked against real ty and SCons in a scratch project as the rest of the section was: `scons` installed into the `.venv` ty uses (`.venv/bin/python -m pip install scons`), or SCons's install folder on `extra-paths` (`/usr/lib/python3/dist-packages` on Debian / Ubuntu, `/usr/lib/scons` on some distros, `$(python3 -c 'import SCons, os; print(os.path.dirname(os.path.dirname(SCons.__file__)))')` to find it). Say why the stub isn't replaced by `from SCons.Script import *` (SCons's run-time globals `Glob` / `Default` / `Export` / `Import` are missing and `env.Program` comes out as `Never`, as tried with ty 0.0.90). A row in the troubleshooting table for `Cannot resolve imported module 'SCons…'`. Docs only"
status: open
priority: 0
dependencies: []
---

