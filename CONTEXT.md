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

## Jumping to Visible Characters

The user types one character they can see and then one or two further keys to move the cursor onto a chosen occurrence of it.

### Language

**Search character**:
The character the user types to choose which visible positions become jump targets.
_Avoid_: Query, pattern

**Jump target**:
A visible position the user can jump to.
_Avoid_: Candidate, match, hint

**Jump label**:
The short virtual text drawn over a jump target that the user types to jump there.
_Avoid_: Symbol, hint, mark

## Blank Lines Around Selections

### Language

**Blank line**:
A line containing no text other than spaces or tabs. The empty position after a file's final line ending is not a separate blank line.
_Avoid_: Newline (a line ending rather than an empty line)

**Adjacent blank line**:
A blank line immediately before the first line or after the last line touched by a selection.

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

## Staging Changes

The user moves individual changes of the open file into or out of Git's index and commits what the index holds, without leaving the editor.

### Language

**Change**:
A contiguous run of lines in the open file that differs from the committed version. It is what the diff gutter marks and what the change navigation commands move between.
_Avoid_: Hunk (the diff tool's and Git's word, not Helix's)

**Staging state**:
Where a change's lines sit relative to the index: staged, partly staged, or unstaged. Shown in the diff gutter.
_Avoid_: Stage status

**Stage**:
Move a change's unstaged lines into the index. A partly staged change becomes staged.

**Unstage**:
Move a change's staged lines out of the index. A partly staged change becomes unstaged.

**Discard**:
Revert a change's unstaged lines in the open file. A fully staged change is refused rather than reverted.
_Avoid_: Reset (overloaded by Git)

**Commit**:
Record everything the index holds, across the whole repository, under a one-line message.
