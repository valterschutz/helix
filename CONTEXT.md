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
