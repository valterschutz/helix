(atx_heading (atx_h1_marker) (inline)? @name) @chapter.1
(atx_heading (atx_h2_marker) (inline)? @name) @chapter.2
(atx_heading (atx_h3_marker) (inline)? @name) @chapter.3
(atx_heading (atx_h4_marker) (inline)? @name) @chapter.4
(atx_heading (atx_h5_marker) (inline)? @name) @chapter.5
(atx_heading (atx_h6_marker) (inline)? @name) @chapter.6

(setext_heading (paragraph) @name (setext_h1_underline)) @chapter.1
(setext_heading (paragraph) @name (setext_h2_underline)) @chapter.2

(html_block) @comment
