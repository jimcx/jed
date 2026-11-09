; jed's additions to tree-sitter-typescript's highlights.scm, layered on tree-sitter-javascript's
; and queries/javascript.scm (for the same node a later pattern wins), for VS Code Dark+'s colours.

; Type names (type parameters, `type_parameter name:`, are `type_identifier`s too) and built-in types.
(type_identifier) @type
(predefined_type) @type.builtin

; Keywords the crate's query misses (`module`, `asserts`, `is`, `infer`, `unique`, `accessor`, `global`).
[
  "module"
  "global"
  "infer"
  "is"
  "asserts"
  "accessor"
  "static"
  "async"
  "extends"
  "new"
  "typeof"
  "instanceof"
  "in"
  "delete"
] @keyword

; Module-level `import type` / `export type`, and `as` / `satisfies` casts, are control in Dark+.
(as_expression "as" @keyword.control)
(satisfies_expression "satisfies" @keyword.control)
(import_statement "type" @keyword.control)
(export_statement "type" @keyword.control)

; Class names (`type_identifier`, unlike JavaScript's `identifier`) and parameters.
(class_declaration name: (type_identifier) @type)
(class name: (type_identifier) @type)
(required_parameter pattern: (identifier) @variable.parameter)
(optional_parameter pattern: (identifier) @variable.parameter)
(required_parameter (rest_pattern (identifier) @variable.parameter))
(arrow_function parameter: (identifier) @variable.parameter)

; Interface / alias / enum / namespace names are types.
(interface_declaration name: (type_identifier) @type)
(type_alias_declaration name: (type_identifier) @type)
(enum_declaration name: (identifier) @type)
(internal_module name: (identifier) @type)
(abstract_class_declaration name: (type_identifier) @type)

; `?` and `!` (optional / non-null) and type operators.
(optional_parameter "?" @operator)
(non_null_expression "!" @operator)
(union_type "|" @operator)
(intersection_type "&" @operator)
