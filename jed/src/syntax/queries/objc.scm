; jed's additions to tree-sitter-objc's highlights.scm (after C's, C's additions): for VS Code
; Dark+'s colours. tree-sitter-objc captures every identifier a property (its
; `#has-ancestor?` predicate isn't evaluated), so identifiers start as variables again and the
; specific patterns follow.

(identifier) @variable
(field_identifier) @property
; C's upper-case constants (the reset above undid them), and Objective-C's literals.
((identifier) @constant
  (#match? @constant "^[A-Z][A-Z\\d_]*$"))
((identifier) @constant.builtin
  (#any-of? @constant.builtin "YES" "NO" "nil" "Nil"))

[
  "@try"
  "@catch"
  "@finally"
  "@throw"
  "@synchronized"
  "@autoreleasepool"
  "@available"
] @keyword.control
(preproc_include "#import" @keyword.control)
"#import" @keyword.control

[
  "@interface"
  "@implementation"
  "@protocol"
  "@property"
  "@synthesize"
  "@dynamic"
  "@selector"
  "@end"
  "@optional"
  "@required"
  "@compatibility_alias"
  "@defs"
  "@encode"
  "@public"
  "@private"
  "@protected"
  "@package"
] @keyword
(module_import "@import" @keyword.control)
(visibility_specification) @keyword
(protocol_qualifier) @keyword
(class_declaration "@" @keyword "class" @keyword)

(method_definition ["+" "-"] @operator)
(method_declaration ["+" "-"] @operator)

((identifier) @variable.builtin
  (#any-of? @variable.builtin "self" "super"))

; Types and protocols
(class_declaration (identifier) @type)
(class_interface "@interface" . (identifier) @type superclass: _? @type category: _? @type)
(class_implementation "@implementation" . (identifier) @type superclass: _? @type category: _? @type)
(protocol_declaration "@protocol" . (identifier) @type)
(protocol_forward_declaration (identifier) @type)
(protocol_reference_list (identifier) @type)
(parameterized_arguments (type_name (type_identifier) @type))
(typedefed_specifier) @type
["BOOL" "IMP" "SEL" "Class" "id"] @type.builtin
(type_qualifier) @keyword

; Methods and messages
(method_definition (identifier) @function)
(method_declaration (identifier) @function)
(method_identifier (identifier)? @function ":" @function (identifier)? @function)
(message_expression method: (identifier) @function)
(selector_expression (identifier) @function)

; Parameters and properties
(method_parameter (identifier) @variable.parameter)
(method_parameter ":" @punctuation)
(property_attribute (identifier) @keyword)
(property_implementation "@synthesize" (identifier) @property)

; Literals
(string_literal "@" @string)
(at_expression "@" @number)
(at_expression (identifier) @constant.builtin)
(array_literal "@" @punctuation.special)
