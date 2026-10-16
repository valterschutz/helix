# Autocomplete

The editor offers suggestions that can replace the text being entered. Other fork terminology is recorded in [CONTEXT.md](CONTEXT.md).

## Language

**Visible suggestion**:
An autocomplete suggestion currently shown in the menu. Its position is counted from the top of the visible menu, not from the beginning of all matching suggestions.
_Avoid_: Absolute rank

**Accept**:
Insert a suggestion and dismiss its menu, rather than merely highlight or preview it.
_Avoid_: Select (ambiguous between highlighting and accepting)
