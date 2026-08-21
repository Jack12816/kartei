;; C definition query.

;; Function definitions only; plain declarations (prototypes) carry no
;; body node and therefore never match this pattern
(function_definition) @func

;; Tagged struct/union/enum definitions; bodyless specifiers are mere
;; type references and stay unmatched
(struct_specifier
  name: (type_identifier) @class
  body: (field_declaration_list))
(union_specifier
  name: (type_identifier) @class
  body: (field_declaration_list))
(enum_specifier
  name: (type_identifier) @class
  body: (enumerator_list))

;; Typedef statements (the builder resolves the declared names)
(type_definition) @typedef

;; Object-like and function-like preprocessor macros
(preproc_def name: (identifier) @const)
(preproc_function_def name: (identifier) @const)
