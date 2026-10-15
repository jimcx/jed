; red's additions to tree-sitter-c's highlights.scm, for VS Code Dark+'s colours. Read after the
; grammar's queries: for the same node a later pattern wins.

[
  "if"
  "else"
  "for"
  "while"
  "do"
  "switch"
  "case"
  "default"
  "break"
  "continue"
  "return"
  "goto"
] @keyword.control

[
  "#define"
  "#elif"
  "#else"
  "#endif"
  "#if"
  "#ifdef"
  "#ifndef"
  "#include"
] @keyword.control
(preproc_directive) @keyword.control

(primitive_type) @type.builtin
(sized_type_specifier) @type.builtin

(char_literal) @string
(escape_sequence) @escape

[
  (true)
  (false)
  (null)
] @constant.builtin

(parameter_declaration
  declarator: (identifier) @variable.parameter)
(parameter_declaration
  declarator: (pointer_declarator
    declarator: (identifier) @variable.parameter))
