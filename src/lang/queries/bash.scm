; Bash definition query. Both the `function foo()` and the `foo()`
; definition forms parse to the same function_definition node, so a
; single pattern covers them. Variable assignments are captured
; everywhere and filtered down to top level in the builder.

(function_definition
  name: (word) @definition.func)

(variable_assignment
  name: (variable_name) @definition.var)
