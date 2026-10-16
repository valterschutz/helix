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

/// Which way a change moves through the index.
#[derive(Clone, Copy)]
enum Direction {
    Stage,
    Unstage,
}

impl Direction {
    fn past_tense(self) -> &'static str {
        match self {
            Direction::Stage => "Staged",
            Direction::Unstage => "Unstaged",
        }
    }

    fn gerund(self) -> &'static str {
        match self {
            Direction::Stage => "staging",
            Direction::Unstage => "unstaging",
        }
    }

    fn adjective(self) -> &'static str {
        match self {
            Direction::Stage => "staged",
            Direction::Unstage => "unstaged",
        }
    }
}

/// The current document's file, placed in its repository.
struct RepoFile {
    /// The repository's top-level working tree directory.
    root: PathBuf,
    /// The file's path as the document has it.
    path: PathBuf,
    /// The file's path relative to `root`, as Git names it in patches.
    relative: String,
}

fn repo_file(editor: &Editor) -> anyhow::Result<RepoFile> {
    let (_, doc) = current_ref!(editor);
    let path = doc
        .path()
        .ok_or_else(|| anyhow!("Not in a Git repository"))?
        .to_path_buf();
    let root = git_cli::repo_root(&path).map_err(|_| anyhow!("Not in a Git repository"))?;
    // Git reports the root with symlinks resolved, so resolve them on the file too.
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let relative = canonical
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?;
    Ok(RepoFile {
        root,
        path,
        relative,
    })
}

/// The changes of the current document that intersect any selection.
fn selected_changes(editor: &Editor) -> Vec<Hunk> {
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

/// Deduplicate and order changes so a patch or transaction can be built from them.
fn sorted_unique(mut changes: Vec<Hunk>) -> Vec<Hunk> {
    changes.sort_by_key(|h| (h.before.start, h.after.start));
    changes.dedup();
    changes
}

fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// Stage or unstage a file whole: what Git can express for a file HEAD or the index does not
/// have, where there are no individual changes to pick from.
fn move_whole_file(
    cx: &mut compositor::Context,
    file: &RepoFile,
    direction: Direction,
) -> anyhow::Result<()> {
    match direction {
        Direction::Stage => git_cli::add_path(&file.root, &file.path)?,
        Direction::Unstage => git_cli::reset_path(&file.root, &file.path)?,
    }
    cx.editor
        .set_status(format!("{} {}", direction.past_tense(), file.relative));
    cx.editor.refresh_vcs(&file.root);
    Ok(())
}

/// Stage or unstage the changes under the selections, then refresh every buffer of the repo.
fn move_selected_changes(cx: &mut compositor::Context, direction: Direction) -> anyhow::Result<()> {
    let file = repo_file(cx.editor)?;
    let (_, doc) = current_ref!(cx.editor);
    if doc.is_modified() {
        bail!(
            "Buffer has unsaved changes; save it before {}",
            direction.gerund()
        );
    }

    if doc.diff_handle().is_none() {
        return move_whole_file(cx, &file, direction);
    }
    // Select before loading the diff: both take the diff's read lock, and a nested read would
    // block behind the worker's pending write.
    let selected = selected_changes(cx.editor);
    if selected.is_empty() {
        bail!("There are no changes under any selection");
    }
    let (_, doc) = current_ref!(cx.editor);
    let diff = doc.diff_handle().expect("checked above").load();
    let Some(index) = diff.index() else {
        drop(diff);
        return move_whole_file(cx, &file, direction);
    };

    let touching = |change: &Hunk| match direction {
        Direction::Stage => diff.unstaged_changes_touching(change),
        Direction::Unstage => diff.staged_changes_touching(change),
    };
    let changes = sorted_unique(selected.iter().flat_map(touching).collect());
    if changes.is_empty() {
        bail!("The selected changes are already {}", direction.adjective());
    }
    // One index-side change can span several displayed changes, so count every displayed
    // change the patch moves rather than only the selected ones.
    let moved = (0..diff.len())
        .filter(|&n| {
            touching(&diff.nth_hunk(n))
                .iter()
                .any(|c| changes.contains(c))
        })
        .count();
    match direction {
        Direction::Stage => {
            let patch = unified_patch(&file.relative, index, diff.doc(), &changes);
            drop(diff);
            git_cli::stage_patch(&file.root, &patch)?;
        }
        Direction::Unstage => {
            let patch = unified_patch(&file.relative, diff.diff_base(), index, &changes);
            drop(diff);
            git_cli::unstage_patch(&file.root, &patch)?;
        }
    }
    cx.editor.set_status(format!(
        "{} {}",
        direction.past_tense(),
        plural(moved, "change")
    ));
    cx.editor.refresh_vcs(&file.root);
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
    move_selected_changes(cx, Direction::Stage)
}

pub(super) fn unstage_diff_change(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    move_selected_changes(cx, Direction::Unstage)
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
    let selected = selected_changes(cx.editor);
    let (view, doc) = current!(cx.editor);
    let Some(handle) = doc.diff_handle() else {
        bail!("Diff is not available in the current buffer")
    };
    if selected.is_empty() {
        bail!("There are no changes under any selection");
    }

    let diff = handle.load();
    let doc_text = doc.text().slice(..);
    // Each reverted change is replaced by the lines of `source` it came from.
    let (source, changes, skipped): (&Rope, Vec<Hunk>, usize) = match diff.index() {
        Some(index) => {
            let mut changes = Vec::new();
            let mut skipped = 0;
            for change in &selected {
                let unstaged = diff.unstaged_changes_touching(change);
                if unstaged.is_empty() {
                    skipped += 1;
                } else {
                    changes.extend(unstaged);
                }
            }
            (index, sorted_unique(changes), skipped)
        }
        None => (diff.diff_base(), selected.clone(), 0),
    };
    let discarded = selected.len() - skipped;
    if changes.is_empty() {
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
        changes.iter().map(|change| {
            let start = source.line_to_char(change.before.start as usize);
            let end = source.line_to_char(change.before.end as usize);
            let text: Tendril = source.slice(start..end).chunks().collect();
            (
                doc_text.line_to_char(change.after.start as usize),
                doc_text.line_to_char(change.after.end as usize),
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
            ", skipped {}",
            plural(skipped, "fully staged change")
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
    let root = repo_file(cx.editor)?.root;
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
                        if event != PromptEvent::Validate {
                            return;
                        }
                        if input.trim().is_empty() {
                            cx.editor
                                .set_error("Commit message is empty; nothing committed");
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
