; red's additions to tree-sitter-xml's highlights.scm, for VS Code Dark+'s colours: tag names
; blue, attribute names light blue, tag brackets grey.

(STag (Name) @tag)
(ETag (Name) @tag)
(EmptyElemTag (Name) @tag)

(Attribute (Name) @tag.attribute)

[
  "<"
  ">"
  "</"
  "/>"
  "<?"
  "?>"
] @punctuation.tag

(Attribute "=" @punctuation)

(CDSect
  (CDStart) @punctuation.tag
  "]]>" @punctuation.tag)
