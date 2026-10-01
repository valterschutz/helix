---
status: accepted
---

# Run a read-only pi RPC child per AI edit conversation

The AI edit conversation needs references (`@file`) and commands (`/skill:name`, `/template`) that behave as they do in pi. Rather than keep spawning one `pi --print` process per message and reimplementing pi's file reading, skill discovery, and command expansion inside Helix, each AI edit conversation starts one `pi --mode rpc` child when the popup opens and kills it when the popup closes. Pi owns the conversation history, reads references with its `read` tool, lists commands through `get_commands`, and expands a leading command itself. Helix only completes references and commands, rotates a trailing command to the front of the request, and parses the final assistant text for the proposal.

## Considered options

- **Keep `pi --print`, let Helix inline referenced files and walk skill directories.** Rejected: it duplicates pi's resource discovery rules (user, project, and package skills) and drifts as pi changes them, and inlined files cost tokens every turn.
- **Let pi's `trailing-skill` extension do the rotation.** Rejected: the extension handles skills only and would require loading all user extensions in the child.
- **Full tools so pi can edit files.** Rejected: the proposal contract (one selection in, one replacement out) is what keeps the feature reviewable. Pi never writes.

## Consequences

- The child runs with `--no-extensions` plus an explicit `-e` for the claude-bridge package, because that provider is extension-registered and the scoped models include it. No other extension has a job inside an edit run.
- Only one command per request is supported, leading or trailing, because pi's core expands exactly one leading command.
- A child crash loses the conversation; Helix does not replay history.
- The selected snippet, its file, and its language travel in the child's system prompt, written to a temp file passed as `--system-prompt`, not in the user message. Pi's prompt templates discard their arguments unless the template has a placeholder, so context in the user message would be lost on a `/template` request.
