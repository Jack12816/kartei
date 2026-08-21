; Make definition query. Every word inside a rule's target list is
; captured separately, so multi-target rules yield one capture per
; target; special dot-targets and pattern targets are filtered in the
; builder. Variable assignments cover all operators (=, :=, ::=, ?=,
; +=) through one pattern since the grammar folds them into a single
; variable_assignment node.

(rule
  (targets
    (word) @definition.target))

(variable_assignment
  name: (word) @definition.var)
