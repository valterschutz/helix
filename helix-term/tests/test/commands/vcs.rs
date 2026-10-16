//! Staging, discarding and committing changes from the editor.
//!
//! Each test builds a Git repository in a temporary directory with `file.txt` committed as
//! `a\nb\nc\n`, opens the file with the working tree and index set up for the case, and runs
//! the typable command under test.

use std::path::{Path, PathBuf};
use std::process::Command;

use helix_core::Position;
use tempfile::TempDir;

use super::*;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env_remove("GIT_DIR")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A repository whose `file.txt` is committed as `a\nb\nc\n`, with `worktree` on disk and the
/// index holding `index`. Commits inside it never sign or need a global identity.
fn repo(index: &str, worktree: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "test@helix.org"]);
    git(root, &["config", "user.name", "helix-test"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    let file = root.join("file.txt");
    std::fs::write(&file, "a\nb\nc\n").unwrap();
    git(root, &["add", "file.txt"]);
    git(root, &["commit", "-q", "-m", "initial"]);
    std::fs::write(&file, index).unwrap();
    git(root, &["add", "file.txt"]);
    std::fs::write(&file, worktree).unwrap();
    (dir, file)
}

fn index_content(repo: &Path) -> String {
    git(repo, &["show", ":file.txt"])
}

fn status_text(app: &Application) -> String {
    app.editor
        .get_status()
        .map(|(text, _)| text.to_string())
        .unwrap_or_default()
}

/// A key sequence and the check to run on the application once it has been handled.
type Step<'a> = (&'a str, &'a dyn Fn(&Application));

/// Open `file` with the cursor on `row`, then run each key sequence once the diff has been
/// computed, checking the application after each one.
async fn run(file: &Path, row: usize, sequences: Vec<Step<'_>>) -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_file(file, Some(Position::new(row, 0)))
        .build()?;
    run_event_loop_until_idle(&mut app).await;
    // The diff worker finishes after the file opens; give it a moment before acting on it.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let inputs = sequences
        .into_iter()
        .map(|(keys, check)| (Some(keys), Some(check)))
        .collect();
    test_key_sequences(&mut app, inputs, false).await
}

fn doc_text(app: &Application) -> String {
    let (_, doc) = helix_view::current_ref!(app.editor);
    doc.text().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_diff_change_moves_the_change_under_the_cursor_into_the_index() -> anyhow::Result<()>
{
    let (dir, file) = repo("a\nb\nc\n", "a\nB\nc\nd\n");
    run(
        &file,
        1,
        vec![(":stage-diff-change<ret>", &|app| {
            assert_eq!(status_text(app), "Staged 1 change");
        })],
    )
    .await?;
    assert_eq!(index_content(dir.path()), "a\nB\nc\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_diff_change_refuses_an_unsaved_buffer() -> anyhow::Result<()> {
    let (dir, file) = repo("a\nb\nc\n", "a\nB\nc\n");
    run(
        &file,
        1,
        vec![("i-<esc>:stage-diff-change<ret>", &|app| {
            let status = status_text(app);
            assert!(status.contains("save"), "status: {status}");
        })],
    )
    .await?;
    assert_eq!(index_content(dir.path()), "a\nb\nc\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_diff_change_adds_a_file_not_yet_in_the_repository() -> anyhow::Result<()> {
    let (dir, _) = repo("a\nb\nc\n", "a\nb\nc\n");
    let new = dir.path().join("new.txt");
    std::fs::write(&new, "new\n").unwrap();
    run(&new, 0, vec![(":stage-diff-change<ret>", &|_| {})]).await?;
    assert_eq!(git(dir.path(), &["show", ":new.txt"]), "new\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unstage_diff_change_takes_the_change_under_the_cursor_out_of_the_index(
) -> anyhow::Result<()> {
    let (dir, file) = repo("a\nB\nc\nd\n", "a\nB\nc\nd\n");
    run(
        &file,
        3,
        vec![(":unstage-diff-change<ret>", &|app| {
            assert_eq!(status_text(app), "Unstaged 1 change");
        })],
    )
    .await?;
    assert_eq!(index_content(dir.path()), "a\nB\nc\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reset_diff_change_discards_unstaged_lines_and_keeps_staged_ones() -> anyhow::Result<()> {
    let (_dir, file) = repo("a\nB\nc\n", "a\nB\nc\nd\n");
    run(
        &file,
        0,
        vec![("%:reset-diff-change<ret>", &|app| {
            assert_eq!(doc_text(app), "a\nB\nc\n");
            assert_eq!(
                status_text(app),
                "Discarded 1 change, skipped 1 fully staged change"
            );
        })],
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn reset_diff_change_refuses_a_fully_staged_change() -> anyhow::Result<()> {
    let (_dir, file) = repo("a\nB\nc\n", "a\nB\nc\n");
    run(
        &file,
        1,
        vec![(":reset-diff-change<ret>", &|app| {
            assert_eq!(doc_text(app), "a\nB\nc\n");
            let status = status_text(app);
            assert!(status.contains("fully staged"), "status: {status}");
        })],
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_records_the_index_with_the_prompted_message() -> anyhow::Result<()> {
    let (dir, file) = repo("a\nB\nc\n", "a\nB\nc\n");
    run(
        &file,
        0,
        vec![
            // The prompt opens once the command's job has run, so the message is a second step.
            (":commit<ret>", &|_| {}),
            ("Capitalize b<ret>", &|app| {
                let status = status_text(app);
                assert!(status.ends_with(" Capitalize b"), "status: {status}");
            }),
        ],
    )
    .await?;
    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%s"]),
        "Capitalize b\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_refuses_an_empty_index() -> anyhow::Result<()> {
    let (dir, file) = repo("a\nb\nc\n", "a\nB\nc\n");
    run(
        &file,
        0,
        vec![(":commit<ret>", &|app| {
            let status = status_text(app);
            assert!(status.contains("Nothing staged"), "status: {status}");
        })],
    )
    .await?;
    assert_eq!(git(dir.path(), &["log", "--format=%s"]), "initial\n");
    Ok(())
}
