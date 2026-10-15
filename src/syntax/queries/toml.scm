; red's highlight query for tree-sitter-toml (instead of the grammar's, which captures each whole
; `pair` as a property): keys are properties, table names types, as in Even Better TOML.

(comment) @comment

(pair (bare_key) @property)
(pair (quoted_key) @property)
(pair (dotted_key (bare_key) @property))
(pair (dotted_key (quoted_key) @property))

(table (bare_key) @type)
(table (dotted_key (bare_key) @type))
(table (quoted_key) @type)
(table_array_element (bare_key) @type)
(table_array_element (dotted_key (bare_key) @type))
(table_array_element (quoted_key) @type)

(string) @string
(escape_sequence) @escape

[
  (integer)
  (float)
] @number

(boolean) @constant.builtin

[
  (offset_date_time)
  (local_date_time)
  (local_date)
  (local_time)
] @string

[
  "."
  ","
] @punctuation

"=" @operator

[
  "["
  "]"
  "[["
  "]]"
  "{"
  "}"
] @punctuation
