# Visible character jumps

Confirmed with the user on 2026-10-17, following a report that `_` (`goto_char`)
scrolls the original line out of view while jump labels appear.

- Apply the same contract to `goto_char` and `extend_to_char`.
- Preserve the exact viewport during activation, search-character input, label
  display, label-prefix input, no matches and cancellation.
- Only fully visible document graphemes are jump targets. Exclude offscreen
  wrapped continuations, horizontal clipping and partially visible wide
  characters or tabs; virtual text is not a document character.
- Visible targets inside the scroll margin remain reachable.
- A completed jump may scroll normally, including to satisfy scroll margins.
- Preserve existing smart-case matching, whitespace/newline searches, label
  ordering, automatic single-target jumps and selection-extension semantics.

## Validation

Agreed seam: the existing command-level integration harness sends actual
key events through Helix and observes viewport state, selections and rendered
labels. Compare all fields of the viewport, not just its document anchor.

Validation on the implementation checkout:

- All 259 command integration tests pass (22 character/word-jump tests).
- The workspace test/doc-test run has 10 unit-test failures caused by missing
  Tokio runtime contexts in `helix-event` and `helix-view`. A detached checkout
  of the unchanged `cb88ca470` baseline reproduces the exact same 10 failures.
  They are pre-existing and intentionally not changed by this fix.
- The original label-time viewport regression was observed failing before the
  fix (anchor `116` became `106`) and passing afterwards. Hidden wrapped
  targets and scroll-margin activation were also observed red before fixes.
- Spec review caught a right-edge label starting inside a wide grapheme. A
  terminal-buffer diff test reproduced the missing first letter before the
  fix and passes after clearing the straddling grapheme's leading cell.
- Standards review found no hard breaches, only optional duplicated fixture
  setup in two scroll-margin tests; the explicit independent fixtures are kept.

Review baseline agreed with the user: `4e0964582`. Implementation starts from
`cb88ca470` (the subsequently fetched fork master, already pinned by Home Manager).
Commits between those points belong to pre-existing work, not this fix.

## Rationale and scope

Text-replacement overlays can change line breaks and grapheme widths; suppressing
scroll alone would leave the view's contents reflowed. Draw jump labels after text
layout instead. Actual formatted screen coordinates also exclude hidden targets
that estimated document-line bounds used to include. Reuse that position mapping
for rendering rather than recomputing screen coordinates separately per target.

The existing shared word-jump renderer uses the same labels, so it also stops
changing document layout. Its target enumeration and selection behavior are not
redesigned. Labels remain configured by the existing theme scope; nearby labels
can still overlap, and labels wider than the whole viewport cannot be drawn.
