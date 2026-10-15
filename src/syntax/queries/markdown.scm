; red's additions to tree-sitter-markdown's (block grammar) highlights.scm, for VS Code Dark+'s
; colours. Inline text (emphasis, code spans, links in paragraphs) is one `inline` leaf here: the
; inline grammar isn't run, so it stays plain.

[
  (atx_heading)
  (setext_heading)
] @markup.heading

[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
] @markup.heading

[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
] @markup.list

[
  (task_list_marker_checked)
  (task_list_marker_unchecked)
] @keyword

(block_quote_marker) @markup.quote
((block_continuation) @markup.quote
 (#match? @markup.quote "^[ \t]*>"))

(fenced_code_block_delimiter) @punctuation
(info_string) @punctuation

(link_reference_definition (link_label) @markup.link)

(html_block) @markup.raw

[
  (minus_metadata)
  (plus_metadata)
] @comment
