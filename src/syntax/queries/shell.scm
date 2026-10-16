; jed's additions to tree-sitter-bash's highlights.scm, for VS Code Dark+'s colours.

[
  "if"
  "then"
  "else"
  "elif"
  "fi"
  "case"
  "esac"
  "for"
  "while"
  "until"
  "do"
  "done"
  "in"
  "select"
] @keyword.control

[
  "declare"
  "local"
  "readonly"
  "typeset"
] @keyword

(number) @number
(ansi_c_string) @string
(translated_string) @string

(simple_expansion "$" @variable)
(expansion
  "${" @variable
  "}" @variable)
(special_variable_name) @variable

((command_name (word) @keyword.control)
 (#match? @keyword.control "^(return|break|continue)$"))
