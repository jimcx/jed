; red's additions to tree-sitter-make's highlights.scm, for VS Code Dark+'s colours: targets are
; functions (yellow), variables light blue, values and recipes plain.

(variable_assignment
  name: (word) @variable)
; Values plain, as Dark+ (`punctuation` has the editor's colour; the grammar makes them strings).
(variable_assignment
  value: (text) @punctuation)
(variable_reference
  (word) @variable)
(automatic_variable) @variable

(targets (word) @function)

[
  "ifeq"
  "ifneq"
  "ifdef"
  "ifndef"
  "else"
  "endif"
  "include"
  "sinclude"
  "-include"
  "foreach"
] @keyword.control

; Built-in functions (`$(wildcard …)`, `$(info …)`): Dark+'s `support.function`.
(function_call function: _ @function)
(shell_function "shell" @function)
