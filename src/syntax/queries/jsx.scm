; jed's JSX additions, layered after javascript.scm for the JSX and TSX grammars (the others lack
; these nodes).

; JSX: components (`<Foo>`) are types, attribute names light blue, tag brackets grey.
(jsx_opening_element (identifier) @type (#match? @type "^[A-Z]"))
(jsx_closing_element (identifier) @type (#match? @type "^[A-Z]"))
(jsx_self_closing_element (identifier) @type (#match? @type "^[A-Z]"))
(jsx_attribute (property_identifier) @tag.attribute)
(jsx_opening_element (["<" ">"]) @punctuation.tag)
(jsx_closing_element (["</" ">"]) @punctuation.tag)
(jsx_self_closing_element (["<" "/>"]) @punctuation.tag)
