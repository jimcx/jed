; jed's additions to tree-sitter-yaml's highlights.scm, for VS Code Dark+'s colours: mapping keys
; are tags (blue), as YAML's TextMate grammar scopes them.

(block_mapping_pair
  key: (flow_node
    [
      (double_quote_scalar)
      (single_quote_scalar)
    ] @tag))

(block_mapping_pair
  key: (flow_node
    (plain_scalar
      (string_scalar) @tag)))

(flow_mapping
  (_
    key: (flow_node
      [
        (double_quote_scalar)
        (single_quote_scalar)
      ] @tag)))

(flow_mapping
  (_
    key: (flow_node
      (plain_scalar
        (string_scalar) @tag))))

(tag) @keyword

[
  (anchor_name)
  (alias_name)
] @type

(escape_sequence) @escape
