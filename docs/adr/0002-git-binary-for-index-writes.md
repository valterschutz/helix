---
status: accepted
---

# Write to the index and history through the `git` binary

Staging, unstaging and committing from the editor need to change the Git index and create commits. Helix reads repositories in-process with gix (diff base, HEAD name, status, and now the index content for staging states), so the obvious move was to write through gix too. Instead, the writes run the `git` executable: `git apply --cached --unidiff-zero` with a zero-context patch rendered from the in-process diff, `git add` / `git reset` for whole files, and `git commit --message`. Reads remain in gix.

## Considered options

- **Write through gix.** Rejected: committing correctly means honouring `commit.gpgsign`, `user.signingkey`, `core.hooksPath` and the pre-commit, prepare-commit-msg, commit-msg and post-commit hooks, identity resolution from `GIT_AUTHOR_*` and includes, and `commit.template`. gix does not run hooks or sign, and reimplementing that surface here would drift from Git and from what the user gets on the command line. Index edits are smaller but would still duplicate Git's `apply` semantics for partial hunks.
- **Shell out for reads too.** Rejected: upstream's gix reads are fast, run on every file open and reload, respect the workspace-trust gate, and already exist. Nothing is gained by replacing them.
- **Delegate to an external tool such as lazygit or the hunk fork.** Rejected: the point of the feature is staging the change under the cursor without leaving the buffer; a separate TUI already exists for the review-shaped workflow.

## Consequences

- `git` must be on `PATH`; the commands fail with Git's own message when it is not. Nothing else in Helix needs it.
- Hooks run exactly as on the command line. A hook that prompts gets no terminal (`GIT_TERMINAL_PROMPT=0`, stdin closed) and fails instead of hanging the editor. `git commit` runs on a blocking task so a slow hook does not freeze the UI; `apply`, `add` and `reset` run synchronously because they finish in milliseconds.
- The staging state shown in the gutter is computed in-process from HEAD, index and buffer, while the patch that stages a change is applied by Git. The two agree because the patch is rendered from the same index content the classification used, with zero context, so Git applies it at exact line offsets. One known gap: the index content is read through the worktree filter pipeline (as the diff base is), while `git apply --cached` matches the raw index blob, so a file whose filters change its bytes (`core.autocrlf`, a clean/smudge driver) can fail to stage with Git's "patch does not apply" message. Unfiltered files, the common case on this setup, are unaffected.
- Only Git is supported. The `DiffProviderRegistry` abstraction stays read-only; the write functions are `helix_vcs::git_cli`, outside the `git` feature because they need no gix. A build without the feature still compiles; its commands fail with Git's message at run time, and show no staging state since the index is never read.
- Three reads also go through the binary because they belong to the write workflow rather than to the gutter: `rev-parse --show-toplevel` for the repository root the patch paths are relative to, `diff --cached --quiet` to refuse an empty commit, and `log -1` for the new HEAD to report. They run synchronously like the index edits; each is a few milliseconds.
