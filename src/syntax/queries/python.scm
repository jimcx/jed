; jed's additions to tree-sitter-python's highlights.scm, for VS Code Dark+'s colours. Read after
; the grammar's queries: for the same node a later pattern wins.

[
  "if"
  "elif"
  "else"
  "for"
  "while"
  "break"
  "continue"
  "return"
  "try"
  "except"
  "finally"
  "raise"
  "with"
  "yield"
  "await"
  "pass"
  "import"
  "from"
  "match"
  "case"
] @keyword.control

[
  "and"
  "or"
  "not"
  "in"
  "is"
  "lambda"
] @keyword

(class_definition
  name: (identifier) @type)

(parameters (identifier) @variable.parameter)
(default_parameter name: (identifier) @variable.parameter)
(typed_parameter (identifier) @variable.parameter)
(typed_default_parameter name: (identifier) @variable.parameter)

((identifier) @variable.builtin
 (#match? @variable.builtin "^(self|cls)$"))
