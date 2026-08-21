; Dockerfile definition query. FROM lines yield the full image
; reference as written (tag or digest included) plus the optional
; build-stage alias. ARG and ENV capture one name per declared
; variable; multi-pair and legacy space-separated ENV lines both parse
; to env_pair nodes, so one pattern covers every form. RUN and CMD are
; deliberately not extracted since they define no addressable name.

(from_instruction
  (image_spec) @definition.image)

(from_instruction
  as: (image_alias) @definition.stage)

(arg_instruction
  name: (_) @definition.arg)

(env_pair
  name: (_) @definition.env)
