; Markdown definition query. Both heading forms are captured as whole
; heading nodes; the builder digs out the heading_content field so the
; recorded name carries no ATX marker or setext underline. Hashes
; inside fenced code blocks parse as code_fence_content and can never
; match these patterns.

(atx_heading) @definition.heading

(setext_heading) @definition.heading
