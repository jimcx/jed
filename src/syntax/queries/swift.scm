; jed's additions to tree-sitter-swift's highlights.scm, for VS Code Dark+'s colours (the
; grammar's query comes first; for the same node a later pattern wins).

; Control flow and `import` are keyword.control in Dark+; declarations are plain keywords.
[
  "import"
  "do"
  "guard"
  "if"
  "switch"
  "for"
  "while"
  "repeat"
  "break"
  "continue"
  "return"
  "fallthrough"
  "in"
] @keyword.control
[
  (else)
  (try_operator)
  (throw_keyword)
  (catch_keyword)
  (default_keyword)
] @keyword.control
(switch_entry "case" @keyword.control)
(for_statement "in" @keyword.control)
(guard_statement "guard" @keyword.control)

; `?` and `:` of a ternary are operators, not keywords.
(ternary_expression [ "?" ":" ] @operator)

; Attributes (`@objc`, `@MainActor`, `@available(…)`): Dark+'s storage.modifier.attribute.
(attribute "@" @keyword)
(attribute (user_type (type_identifier) @keyword))

; Conditional compilation (`#if`, `#else`, `#endif`, `#available`, `#selector`) and `#file`.
(directive) @keyword.control
(special_literal) @constant

; Members read like properties.
(navigation_expression (navigation_suffix (simple_identifier) @property))
(class_body (property_declaration (pattern (simple_identifier) @property)))
(protocol_property_declaration (pattern (simple_identifier) @property))
(value_argument name: (value_argument_label (simple_identifier) @property))
(enum_entry name: (simple_identifier) @constant)

; Interpolation: `\(` and `)` keep the grammar's punctuation.special; doc comments are comments.
(multiline_comment) @comment
(comment) @comment

; A `#!` line is a comment (Dark+: comment.line.number-sign.shebang).
(shebang_line) @comment
