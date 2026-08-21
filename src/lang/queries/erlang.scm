;; Erlang definition query.

;; The module attribute
(module_attribute name: (atom) @module)

;; Record declarations
(record_decl name: (atom) @class)

;; Preprocessor macro definitions (the builder resolves the name)
(pp_define lhs: (macro_lhs) @const)

;; Type aliases
(type_alias name: (type_name name: (atom) @type))

;; Function declarations (the builder collapses the clauses)
(fun_decl) @func
