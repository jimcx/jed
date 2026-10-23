; jed's additions to tree-sitter-gitcommit's highlights, for Dark+'s colours (later patterns win).
; `BREAKING CHANGE:` is a keyword, not the grammar's `comment.error`.
(breaking_change
  (token) @keyword)

; A plain `#` comment ending in `:` (`# Conflicts:` in a MERGE_MSG) parses as a `title`: keep it a
; comment. git's own status titles (`Changes to be committed:`) are in `generated_comment`s.
(comment
  (title) @comment)
