; jed's additions to tree-sitter-javascript's highlights.scm (and highlights-jsx.scm for JSX), for
; VS Code Dark+'s colours; the crate's queries come first (for the same node a later pattern wins).
; Kept to nodes and tokens the TypeScript grammars share (JSX is in jsx.scm), so queries/typescript.scm
; can be layered on this one.

; Control flow, modules and `await` / `yield` / `of` are keyword.control (purple) in Dark+;
; declarations and `async` are plain keywords (blue).
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
  "throw"
  "try"
  "catch"
  "finally"
  "import"
  "export"
  "from"
  "as"
  "await"
  "yield"
  "of"
  "with"
  "debugger"
] @keyword.control

; `?` and `:` of a ternary are operators, not punctuation.
(ternary_expression [ "?" ":" ] @operator)

; Decorators (`@dec`, `@dec(args)`, `@a.b`): Dark+ colours them as functions.
(decorator "@" @function)
(decorator (identifier) @function)
(decorator (call_expression function: (identifier) @function))
(decorator (member_expression property: (property_identifier) @function))

; A shebang line.
(hash_bang_line) @comment

; A regex literal is a string throughout (the crate paints its `/` delimiters as operators).
(regex "/" @string)
