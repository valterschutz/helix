# Inline AI Editing

Helix can ask pi to transform a selected snippet and review the proposed replacement without leaving the editor.

## Language

**AI edit conversation**:
The interaction that begins when a selected snippet opens an inline prompt and ends when the proposal is applied or the interface is closed.
_Avoid_: Pi session, chat session

**AI edit preference**:
The model and thinking level retained specifically for future AI edit conversations, independently of pi's normal interactive default.
_Avoid_: Pi default

**Scoped model**:
A model included in pi's configured `enabledModels` cycle. Model switching in an AI edit conversation moves only among these models.
_Avoid_: Enabled model, available model

**Proposal**:
The complete replacement snippet returned by pi for review before it is applied.
_Avoid_: Patch, diff

**Reference**:
A workspace file named with a leading `@` in a request so that pi reads it while producing the proposal.
_Avoid_: Mention, attachment, context file

**Command**:
A pi skill or prompt template named with a leading `/` in a request, expanded by pi before the model sees the request.
_Avoid_: Slash command, skill call

# Reverse Outlining

Helix can show a prose document's structure as its headings plus a one-line summary of each passage, so the writer can check the argument without reading the whole text.

## Language

**Chapter**:
A heading at any level, such as a Markdown `#` or setext heading.
_Avoid_: Section

**Summary**:
A one-line comment whose text starts with `Σ` (U+03A3), written in the document language's comment syntax, such as `<!-- Σ … -->` in Markdown. By convention it sits directly above the first line of the passage it summarises. Other comments are not summaries.
_Avoid_: Topic sentence

**Passage**:
A summary plus everything after it up to the next summary or chapter.
_Avoid_: Paragraph

**Paragraph**:
A contiguous run of non-blank lines, not counting chapter and summary lines. A passage may contain several. Add-summary works on the paragraph under the cursor.

**Outline**:
The document's chapters and summaries in document order, with each chapter indented by heading level and each summary one step under its chapter.
