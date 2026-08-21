; Ruby definition query, based on the official tree-sitter-ruby
; tags.scm, extended with plain constant assignments (which the
; official query misses) and attr_* accessor symbol captures.

(class
  name: [(constant) (scope_resolution)] @definition.class)

(module
  name: [(constant) (scope_resolution)] @definition.module)

(method
  name: (_) @definition.method)

(singleton_method
  name: (_) @definition.smethod)

(assignment
  left: (constant) @definition.const)

((call
  method: (identifier) @_accessor
  arguments: (argument_list (simple_symbol) @definition.attr))
  (#any-of? @_accessor "attr_accessor" "attr_reader" "attr_writer"))
