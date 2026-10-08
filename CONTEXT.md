# Helix Fork

Personal features carried by this fork of Helix.

## Inline AI Editing

Helix can ask pi to transform a selected snippet and review the proposed replacement without leaving the editor.

### Language

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

## Reverse Outlining

A writer states what each passage of a prose document says in a summary, then reads the chapters and summaries in sequence to check the document's structure.

### Language

**Chapter**:
A part of a prose document introduced by a heading, at any heading level.
_Avoid_: Section, symbol

**Summary**:
A one-line comment marked with `Σ` that states what the passage below it says. It is not part of the prose.
_Avoid_: Topic sentence, gist, annotation

**Passage**:
A summary together with everything after it up to the next summary or heading. It may span several paragraphs, lists or other blocks.
_Avoid_: Paragraph

**Paragraph**:
A contiguous run of non-blank lines, excluding chapter and summary lines. A passage may contain several.

**Outline**:
The chapters and summaries of one document, in document order and nested by heading level.
_Avoid_: Table of contents, symbol list
