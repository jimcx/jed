---
id: back_forward
name: Back / forward navigation
summary: `nav.back` / `nav.forward` on Ctrl+- / Ctrl+Shift+- (Ctrl+Alt+- on Linux), VS Code's Go Back / Go Forward
status: done
priority: 0
dependencies: []
---

  - One history for the window (VS Code's default `navigationScope`): entries are (group, file, primary selection, scroll), up to 50; recorded before jumps — go to definition / references / the Locations list, quick open (files and `:line`), go to line, opening a Search view or Problems result, F8 / Shift+F8, Cmd+Home / End, opening or switching to another file's tab — and when the primary cursor moves 10+ lines from the last entry in one step; going back first records where you are so forward returns; a new jump after going back drops the forward entries
  - Back to a closed tab reopens its file (off the UI thread, through `opening`); a file gone from disk is skipped; positions map through edits (kept as byte positions mapped like other views', or re-clamped when used); a diff tab counts as its file
  - `navigation.rs` (model: `History` of `Entry { group, path, selection, scroll }`, ≤ 50, consecutive duplicates collapsed, forward entries dropped by a new jump, mapped through edits) and `app/navigation.rs`: recorded in one place — `nav_begin` / `nav_end` around every input event, background result and `test_execute` — when the active group or file changed, a jump command moved the primary without an edit (`open_file`: quick open, Search / Problems / Locations results, F8, definition; go to line; Cmd+Home / End), or (input only) the primary moved 10+ lines in one step without an edit; session restore records nothing. Going to an entry prefers a text tab of its file in its group (else the active group), else a new view of the loaded buffer, else reopens it through `opening` (`Goto::Place`); a file gone from disk or failing to load is skipped and navigation goes on. Keys: global `nav.back` / `nav.forward`; `KeyCombo::from_event` turns `_` with modifiers into `-` + Shift (kitty's Ctrl+Shift+-)
  - The 10-line rule is per step (from where the cursor was before the event), not from the last entry
  - Deferred: an explorer rename doesn't update entries' paths (the renamed file's entries are skipped as gone); entries recorded in the same event as a background edit (rename, format) are only clamped; legacy (non-kitty) terminals send Ctrl+- as Ctrl+7, so Go Back doesn't fire there; mouse buttons (next item)
  - Still to check by hand: Ctrl+- / Ctrl+Shift+- (macOS) and Ctrl+Alt+- / Ctrl+Shift+- (Linux) arriving in kitty, Ghostty, WezTerm, iTerm2, Alacritty and tmux; going back across splits and to a closed tab's file in a real session
