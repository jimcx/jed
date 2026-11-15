; jed's highlight query for tree-sitter-cmake (instead of the grammar's, which relies on Neovim's
; `#lua-match?`, unknown to tree-sitter: its all-caps pattern would colour every argument).

[
  (line_comment)
  (bracket_comment)
] @comment

[
  (quoted_argument)
  (bracket_argument)
] @string

(escape_sequence) @escape

(normal_command (identifier) @function)

[
  (function)
  (endfunction)
  (macro)
  (endmacro)
  (block)
  (endblock)
] @keyword

[
  (if)
  (elseif)
  (else)
  (endif)
  (foreach)
  (endforeach)
  (while)
  (endwhile)
] @keyword.control

((normal_command (identifier) @keyword.control)
 (#match? @keyword.control "^(?i)(break|continue|return)$"))

(function_command
  (argument_list . (argument) @function))
(macro_command
  (argument_list . (argument) @function))

; Keyword arguments (PUBLIC, REQUIRED, VERSION, …).
((argument (unquoted_argument) @constant)
 (#match? @constant "^[A-Z][A-Z0-9_]*$"))

((argument (unquoted_argument) @constant.builtin)
 (#match? @constant.builtin "^(?i)(on|off|true|false|yes|no|y|n)$"))

([
  (normal_var)
  (env_var)
  (cache_var)
] @variable)
[
  "ENV"
  "CACHE"
] @variable
