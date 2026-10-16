; jed's additions to tree-sitter-json's highlights.scm, for VS Code Dark+'s colours: object keys
; are properties (light blue), not strings (the grammar's `(string) @string` comes after its key
; pattern and wins).

(pair
  key: (string) @property)
