; red's additions to tree-sitter-rust's highlights.scm, for VS Code Dark+'s colours. Read after the
; grammar's queries (and a first `(identifier) @variable`): for the same node a later pattern wins.

[
  "if"
  "else"
  "for"
  "while"
  "loop"
  "match"
  "return"
  "break"
  "continue"
  "yield"
  "await"
] @keyword.control

(primitive_type) @type

[
  (integer_literal)
  (float_literal)
] @number

(lifetime (identifier) @type)
(lifetime "'" @type)

; The grammar's all-caps pattern never matches (a stray quote in its regex).
((identifier) @constant
 (#match? @constant "^[A-Z][A-Z\\d_]+$"))

(macro_invocation
  macro: (scoped_identifier
    name: (identifier) @function.macro))
(macro_definition
  name: (identifier) @function.macro)

(closure_parameters (identifier) @variable.parameter)
