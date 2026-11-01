; jed's query for tree-sitter-ini. The grammar wants `key = value` on every line and no `:` or
; bare `key value` lines, so the colours come from the line pass in highlight.rs (`ini_lines`),
; which paints every line it sees; this only gives the comment nodes a colour beyond 16 KB.

(comment) @comment
