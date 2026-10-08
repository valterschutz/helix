; A heading's text is its nodes after the marker, apart from a trailing label. Each level has a
; pattern for headings without a trailing label, one for headings with one, and one for empty
; headings.

((heading "=" . (_)* @name . (_) @name @_last .) @chapter.1
 (#not-match? @_last "^<[^\\s<>]+>$"))
(heading "=" . (_)* @name . (label) .) @chapter.1
(heading "=" .) @chapter.1

((heading "==" . (_)* @name . (_) @name @_last .) @chapter.2
 (#not-match? @_last "^<[^\\s<>]+>$"))
(heading "==" . (_)* @name . (label) .) @chapter.2
(heading "==" .) @chapter.2

((heading "===" . (_)* @name . (_) @name @_last .) @chapter.3
 (#not-match? @_last "^<[^\\s<>]+>$"))
(heading "===" . (_)* @name . (label) .) @chapter.3
(heading "===" .) @chapter.3

((heading "====" . (_)* @name . (_) @name @_last .) @chapter.4
 (#not-match? @_last "^<[^\\s<>]+>$"))
(heading "====" . (_)* @name . (label) .) @chapter.4
(heading "====" .) @chapter.4

((heading "=====" . (_)* @name . (_) @name @_last .) @chapter.5
 (#not-match? @_last "^<[^\\s<>]+>$"))
(heading "=====" . (_)* @name . (label) .) @chapter.5
(heading "=====" .) @chapter.5

((heading "======" . (_)* @name . (_) @name @_last .) @chapter.6
 (#not-match? @_last "^<[^\\s<>]+>$"))
(heading "======" . (_)* @name . (label) .) @chapter.6
(heading "======" .) @chapter.6

(comment) @comment
