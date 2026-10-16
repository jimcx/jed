; jed's additions to tree-sitter-cpp's highlights.scm (after C's, C's additions and C++'s).

[
  "try"
  "catch"
  "throw"
  "co_return"
  "co_yield"
  "co_await"
] @keyword.control

(auto) @type.builtin
(this) @variable.builtin
(null "nullptr" @constant.builtin)
(namespace_identifier) @type

(parameter_declaration
  declarator: (reference_declarator
    (identifier) @variable.parameter))
