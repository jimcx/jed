; JavaScript-only part of jed's additions: the TypeScript grammars name classes and parameters
; differently (see typescript.scm).

; Class names are types; `new Foo()` is already a constructor (upper-case identifier).
(class_declaration name: (identifier) @type)
(class name: (identifier) @type)

; Parameters.
(formal_parameters (identifier) @variable.parameter)
(formal_parameters (assignment_pattern left: (identifier) @variable.parameter))
(formal_parameters (rest_pattern (identifier) @variable.parameter))
(arrow_function parameter: (identifier) @variable.parameter)
