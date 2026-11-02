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
