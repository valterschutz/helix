//! Writes to the Git index and history, done through the `git` binary.
//!
//! Reads stay in gix, but applying patches to the index and committing run `git` itself so
//! hooks, commit signing, identity and the rest of the user's configuration apply exactly as
//! they would on the command line. See `docs/adr/0002-git-binary-for-index-writes.md`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

/// Run `git` in `root` with `args`, feeding `stdin`, and return stdout on success. Failure
/// carries the first non-empty line Git printed, which is what the editor shows.
fn git(root: &Path, args: &[&str], stdin: Option<&str>) -> Result<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("failed to run git")?;
    if let Some(input) = stdin {
        use std::io::Write;
        let mut pipe = child.stdin.take().context("git has no stdin")?;
        pipe.write_all(input.as_bytes())?;
        drop(pipe);
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let line = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("git failed");
        bail!("{line}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The top-level working tree directory of the repository containing `file`.
pub fn repo_root(file: &Path) -> Result<PathBuf> {
    let dir = file.parent().context("file has no parent directory")?;
    let out = git(dir, &["rev-parse", "--show-toplevel"], None)?;
    Ok(PathBuf::from(out.trim_end()))
}

/// Apply a zero-context patch to the index.
pub fn stage_patch(root: &Path, patch: &str) -> Result<()> {
    git(
        root,
        &["apply", "--cached", "--unidiff-zero", "-"],
        Some(patch),
    )
    .map(drop)
}

/// Reverse-apply a zero-context patch to the index.
pub fn unstage_patch(root: &Path, patch: &str) -> Result<()> {
    git(
        root,
        &["apply", "--cached", "--reverse", "--unidiff-zero", "-"],
        Some(patch),
    )
    .map(drop)
}

/// Stage a whole file, which is what Git can express for a file not yet committed.
pub fn add_path(root: &Path, file: &Path) -> Result<()> {
    git(root, &["add", "--", &file.to_string_lossy()], None).map(drop)
}

/// Take a whole file's staged changes out of the index.
pub fn reset_path(root: &Path, file: &Path) -> Result<()> {
    git(
        root,
        &["reset", "--quiet", "--", &file.to_string_lossy()],
        None,
    )
    .map(drop)
}

/// Whether the index differs from HEAD anywhere in the repository.
pub fn has_staged_changes(root: &Path) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--cached", "--quiet"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to run git")?;
    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => bail!("git diff --cached failed"),
    }
}

/// Commit the index with `message` and return the new HEAD as `<short hash> <subject>`.
pub fn commit(root: &Path, message: &str) -> Result<String> {
    git(root, &["commit", "--message", message], None)?;
    let head = git(root, &["log", "-1", "--format=%h %s"], None)?;
    Ok(head.trim_end().to_string())
}
