//! Stage, unstage, discard and commit changes of the current file.
//!
//! A change is a run of lines where the document differs from the committed version, as the
//! diff gutter shows it. Staging and unstaging move the selected changes' lines through the
//! Git index with the `git` binary; discarding reverts their unstaged lines in the buffer;
//! committing records the whole index. See `CONTEXT.md` ("Staging Changes") for the terms.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _};
use helix_core::{Rope, Tendril, Transaction};
use helix_vcs::{git_cli, unified_patch, Hunk};
use helix_view::{current, current_ref, Editor};

use helix_core::command_line::Args;

use crate::compositor;
use crate::job::Callback;
use crate::ui::{self, PromptEvent};

/// The repository root and the current document's path relative to it.
fn repo_and_relative_path(editor: &Editor) -> anyhow::Result<(PathBuf, PathBuf, String)> {
    let (_, doc) = current_ref!(editor);
    let path = doc.path().context("Buffer has no file")?.to_path_buf();
    let root = git_cli::repo_root(&path).map_err(|_| anyhow!("Not in a Git repository"))?;
    let canonical = helix_stdx::path::canonicalize(&path);
    let relative = canonical
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?;
    Ok((root, path, relative))
}

/// The changes of the current document that intersect any selection.
fn selected_hunks(editor: &Editor) -> Vec<Hunk> {
    let (view, doc) = current_ref!(editor);
    let Some(handle) = doc.diff_handle() else {
        return Vec::new();
    };
    let diff = handle.load();
    let text = doc.text().slice(..);
    diff.hunks_intersecting_line_ranges(doc.selection(view.id).line_ranges(text))
        .cloned()
        .collect()
}

/// Deduplicate and order hunks so a patch or transaction can be built from them.
fn sorted_unique(mut hunks: Vec<Hunk>) -> Vec<Hunk> {
    hunks.sort_by_key(|h| (h.before.start, h.after.start));
    hunks.dedup();
    hunks
}

fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// Stage or unstage the changes under the selections, then refresh every buffer of the repo.
fn move_selected_changes(cx: &mut compositor::Context, stage: bool) -> anyhow::Result<()> {
    let verb = if stage { "Staged" } else { "Unstaged" };
    let (root, path, relative) = repo_and_relative_path(cx.editor)?;
    let (_, doc) = current_ref!(cx.editor);
    if doc.is_modified() {
        bail!(
            "Buffer has unsaved changes; save it before {}",
            if stage { "staging" } else { "unstaging" }
        );
    }

    let Some(handle) = doc.diff_handle() else {
        // Not in HEAD, so there are no changes to pick from: the whole file is the change.
        if stage {
            git_cli::add_path(&root, &path)?;
        } else {
            git_cli::reset_path(&root, &path)?;
        }
        cx.editor.set_status(format!("{verb} {relative}"));
        cx.editor.refresh_vcs(&root);
        return Ok(());
    };

    let diff = handle.load();
    let selected = selected_hunks(cx.editor);
    if selected.is_empty() {
        bail!("There are no changes under any selection");
    }
    let Some(index) = diff.index() else {
        // In HEAD but not in the index: Git tracks that as a whole-file state.
        drop(diff);
        if stage {
            git_cli::add_path(&root, &path)?;
        } else {
            git_cli::reset_path(&root, &path)?;
        }
        cx.editor.set_status(format!("{verb} {relative}"));
        cx.editor.refresh_vcs(&root);
        return Ok(());
    };

    let mut moved = 0;
    let mut hunks = Vec::new();
    for hunk in &selected {
        let touching = if stage {
            diff.unstaged_hunks_touching(hunk)
        } else {
            diff.staged_hunks_touching(hunk)
        };
        if !touching.is_empty() {
            moved += 1;
            hunks.extend(touching);
        }
    }
    if hunks.is_empty() {
        bail!(
            "The selected changes are already {}",
            if stage { "staged" } else { "unstaged" }
        );
    }
    let hunks = sorted_unique(hunks);
    if stage {
        let patch = unified_patch(&relative, index, diff.doc(), &hunks);
        drop(diff);
        git_cli::stage_patch(&root, &patch)?;
    } else {
        let patch = unified_patch(&relative, diff.diff_base(), index, &hunks);
        drop(diff);
        git_cli::unstage_patch(&root, &patch)?;
    }
    cx.editor
        .set_status(format!("{verb} {}", plural(moved, "change")));
    cx.editor.refresh_vcs(&root);
    Ok(())
}

pub(super) fn stage_diff_change(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    move_selected_changes(cx, true)
}

pub(super) fn unstage_diff_change(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    move_selected_changes(cx, false)
}

/// Revert the unstaged lines of the changes under the selections in the buffer. A fully staged
/// change is skipped rather than reverted. Without index content (no staging information) the
/// whole change is reverted to the diff base, as upstream does.
pub(super) fn reset_diff_change(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let scrolloff = cx.editor.config().scrolloff;
    let selected = selected_hunks(cx.editor);
    let (view, doc) = current!(cx.editor);
    let Some(handle) = doc.diff_handle() else {
        bail!("Diff is not available in the current buffer")
    };
    if selected.is_empty() {
        bail!("There are no changes under any selection");
    }

    let diff = handle.load();
    let doc_text = doc.text().slice(..);
    // Each reverted hunk is replaced by the lines of `source` it came from.
    let (source, hunks, skipped): (&Rope, Vec<Hunk>, usize) = match diff.index() {
        Some(index) => {
            let mut hunks = Vec::new();
            let mut skipped = 0;
            for hunk in &selected {
                let unstaged = diff.unstaged_hunks_touching(hunk);
                if unstaged.is_empty() {
                    skipped += 1;
                } else {
                    hunks.extend(unstaged);
                }
            }
            (index, sorted_unique(hunks), skipped)
        }
        None => (diff.diff_base(), selected.clone(), 0),
    };
    let discarded = selected.len() - skipped;
    if hunks.is_empty() {
        bail!(
            "{} fully staged; unstage first to discard",
            if skipped == 1 {
                "The selected change is"
            } else {
                "The selected changes are"
            }
        );
    }

    let transaction = Transaction::change(
        doc.text(),
        hunks.iter().map(|hunk| {
            let start = source.line_to_char(hunk.before.start as usize);
            let end = source.line_to_char(hunk.before.end as usize);
            let text: Tendril = source.slice(start..end).chunks().collect();
            (
                doc_text.line_to_char(hunk.after.start as usize),
                doc_text.line_to_char(hunk.after.end as usize),
                (!text.is_empty()).then_some(text),
            )
        }),
    );

    drop(diff); // make borrow check happy
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    view.ensure_cursor_in_view(doc, scrolloff);
    let mut status = format!("Discarded {}", plural(discarded, "change"));
    if skipped > 0 {
        status.push_str(&format!(
            ", skipped {} fully staged {}",
            skipped,
            if skipped == 1 { "change" } else { "changes" }
        ));
    }
    cx.editor.set_status(status);
    Ok(())
}

/// Commit the index. With arguments they are the message; otherwise a one-line prompt asks
/// for it, pre-filled with the message of the last failed commit.
pub(super) fn commit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let (root, _, _) = repo_and_relative_path(cx.editor)?;
    if !git_cli::has_staged_changes(&root)? {
        bail!("Nothing staged");
    }

    if !args.is_empty() {
        let message = args.join(" ");
        run_commit(cx, root, message);
        return Ok(());
    }

    let initial = cx.editor.pending_commit_message.take().unwrap_or_default();
    cx.jobs.callback(async move {
        let call: Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut compositor::Compositor| {
                let prompt = ui::Prompt::new(
                    "commit:".into(),
                    Some('C'),
                    ui::completers::none,
                    move |cx: &mut compositor::Context, input: &str, event: PromptEvent| {
                        if event != PromptEvent::Validate || input.trim().is_empty() {
                            return;
                        }
                        run_commit(cx, root.clone(), input.to_string());
                    },
                )
                .with_line(initial, editor);
                compositor.push(Box::new(prompt));
            },
        ));
        Ok(call)
    });
    Ok(())
}

/// Run `git commit` off the UI thread (hooks may take a while), then report the new HEAD or
/// keep the message for the next attempt.
fn run_commit(cx: &mut compositor::Context, root: PathBuf, message: String) {
    cx.jobs.callback(async move {
        let result = {
            let root = root.clone();
            let message = message.clone();
            tokio::task::spawn_blocking(move || git_cli::commit(&root, &message)).await?
        };
        let call: Callback = Callback::Editor(Box::new(move |editor: &mut Editor| match result {
            Ok(head) => {
                editor.set_status(format!("Committed {head}"));
                editor.refresh_vcs(Path::new(&root));
            }
            Err(err) => {
                editor.pending_commit_message = Some(message);
                editor.set_error(format!("Commit failed: {err}"));
            }
        }));
        Ok(call)
    });
}
