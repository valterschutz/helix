use std::{fs::File, io::Write, path::Path, process::Command};

use tempfile::TempDir;

use crate::git;

fn exec_git_cmd(args: &str, git_dir: &Path) {
    let res = Command::new("git")
        .arg("-C")
        .arg(git_dir) // execute the git command in this directory
        .args(args.split_whitespace())
        .env_remove("GIT_DIR")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("GIT_TERMINAL_PROMPT", "false")
        .env("GIT_AUTHOR_DATE", "2000-01-01 00:00:00 +0000")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_NAME", "author")
        .env("GIT_COMMITTER_DATE", "2000-01-02 00:00:00 +0000")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_NAME", "committer")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .output()
        .unwrap_or_else(|_| panic!("`git {args}` failed"));
    if !res.status.success() {
        println!("{}", String::from_utf8_lossy(&res.stdout));
        eprintln!("{}", String::from_utf8_lossy(&res.stderr));
        panic!("`git {args}` failed (see output above)")
    }
}

fn create_commit(repo: &Path, add_modified: bool) {
    if add_modified {
        exec_git_cmd("add -A", repo);
    }
    exec_git_cmd("commit -m message", repo);
}

fn empty_git_repo() -> TempDir {
    let tmp = tempfile::tempdir().expect("create temp dir for git testing");
    exec_git_cmd("init", tmp.path());
    exec_git_cmd("config user.email test@helix.org", tmp.path());
    exec_git_cmd("config user.name helix-test", tmp.path());
    tmp
}

#[test]
fn missing_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    File::create(&file).unwrap().write_all(b"foo").unwrap();

    assert!(git::get_diff_base(&file, true).is_err());
}

#[test]
fn unmodified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    assert_eq!(
        git::get_diff_base(&file, true).unwrap(),
        Vec::from(contents)
    );
}

#[test]
fn modified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    File::create(&file).unwrap().write_all(b"bar").unwrap();

    assert_eq!(
        git::get_diff_base(&file, true).unwrap(),
        Vec::from(contents)
    );
}

/// Test that `get_file_head` does not return content for a directory.
/// This is important to correctly cover cases where a directory is removed and replaced by a file.
/// If the contents of the directory object were returned a diff between a path and the directory children would be produced.
#[test]
fn directory() {
    let temp_git = empty_git_repo();
    let dir = temp_git.path().join("file.txt");
    std::fs::create_dir(&dir).expect("");
    let file = dir.join("file.txt");
    let contents = b"foo".as_slice();
    File::create(file).unwrap().write_all(contents).unwrap();

    create_commit(temp_git.path(), true);

    std::fs::remove_dir_all(&dir).unwrap();
    File::create(&dir).unwrap().write_all(b"bar").unwrap();
    assert!(git::get_diff_base(&dir, true).is_err());
}

/// Test that `get_diff_base` resolves symlinks so that the same diff base is
/// used as the target file.
///
/// This is important to correctly cover cases where a symlink is removed and
/// replaced by a file. If the contents of the symlink object were returned
/// a diff between a literal file path and the actual file content would be
/// produced (bad ui).
#[cfg(any(unix, windows))]
#[test]
fn symlink() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    let file_link = temp_git.path().join("file_link.txt");

    symlink("file.txt", &file_link).unwrap();
    create_commit(temp_git.path(), true);

    assert_eq!(git::get_diff_base(&file_link, true).unwrap(), contents);
    assert_eq!(git::get_diff_base(&file, true).unwrap(), contents);
}

/// Test that `get_diff_base` returns content when the file is a symlink to
/// another file that is in a git repo, but the symlink itself is not.
#[cfg(any(unix, windows))]
#[test]
fn symlink_to_git_repo() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let temp_git = empty_git_repo();

    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    create_commit(temp_git.path(), true);

    let file_link = temp_dir.path().join("file_link.txt");
    symlink(&file, &file_link).unwrap();

    assert_eq!(git::get_diff_base(&file_link, true).unwrap(), contents);
    assert_eq!(git::get_diff_base(&file, true).unwrap(), contents);
}

mod staging {
    //! Reading the index and moving changes through it with the `git` binary.

    use std::{fs::File, io::Write};

    use helix_core::Rope;
    use imara_diff::Hunk;

    use super::{create_commit, empty_git_repo, exec_git_cmd};
    use crate::git::{self, cli};
    use crate::unified_patch;

    fn write(path: &std::path::Path, contents: &str) {
        File::create(path)
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
    }

    /// A repository where `file.txt` is committed as `a\nb\nc\n`, with the working tree holding
    /// `worktree`, and `commit.gpgsign` off so commits never wait for a signer.
    fn repo_with_file(worktree: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let repo = empty_git_repo();
        exec_git_cmd("config commit.gpgsign false", repo.path());
        let file = repo.path().join("file.txt");
        write(&file, "a\nb\nc\n");
        create_commit(repo.path(), true);
        write(&file, worktree);
        (repo, file)
    }

    #[test]
    fn index_content_is_what_was_added_not_the_working_tree() {
        let (repo, file) = repo_with_file("a\nB\nc\n");
        exec_git_cmd("add file.txt", repo.path());
        write(&file, "a\nX\nc\n");
        assert_eq!(
            git::get_index_content(&file, true).unwrap(),
            Some(b"a\nB\nc\n".to_vec())
        );
    }

    #[test]
    fn untracked_file_has_no_index_content() {
        let (repo, _) = repo_with_file("a\nb\nc\n");
        let other = repo.path().join("new.txt");
        write(&other, "new\n");
        assert_eq!(git::get_index_content(&other, true).unwrap(), None);
    }

    #[test]
    fn repo_root_is_the_working_tree_top_level() {
        let (repo, file) = repo_with_file("a\nb\nc\n");
        assert_eq!(
            cli::repo_root(&file).unwrap(),
            std::fs::canonicalize(repo.path()).unwrap()
        );
    }

    #[test]
    fn staging_a_patch_moves_only_its_change_into_the_index() {
        let (repo, file) = repo_with_file("a\nB\nc\nd\n");
        let root = cli::repo_root(&file).unwrap();
        // The index (still `a\nb\nc\n`) to the working tree has two changes; stage `b -> B`.
        let patch = unified_patch(
            "file.txt",
            &Rope::from_str("a\nb\nc\n"),
            &Rope::from_str("a\nB\nc\nd\n"),
            &[Hunk {
                before: 1..2,
                after: 1..2,
            }],
        );
        cli::stage_patch(&root, &patch).unwrap();
        assert_eq!(
            git::get_index_content(&file, true).unwrap(),
            Some(b"a\nB\nc\n".to_vec())
        );
        drop(repo);
    }

    #[test]
    fn unstaging_a_patch_removes_only_its_change_from_the_index() {
        let (repo, file) = repo_with_file("a\nB\nc\nd\n");
        exec_git_cmd("add file.txt", repo.path());
        let root = cli::repo_root(&file).unwrap();
        // HEAD to the index has two changes; unstage the appended `d`.
        let patch = unified_patch(
            "file.txt",
            &Rope::from_str("a\nb\nc\n"),
            &Rope::from_str("a\nB\nc\nd\n"),
            &[Hunk {
                before: 3..3,
                after: 3..4,
            }],
        );
        cli::unstage_patch(&root, &patch).unwrap();
        assert_eq!(
            git::get_index_content(&file, true).unwrap(),
            Some(b"a\nB\nc\n".to_vec())
        );
    }

    #[test]
    fn a_patch_for_a_file_without_final_newline_applies() {
        let (repo, file) = repo_with_file("a\nb\nc");
        let root = cli::repo_root(&file).unwrap();
        let patch = unified_patch(
            "file.txt",
            &Rope::from_str("a\nb\nc\n"),
            &Rope::from_str("a\nb\nc"),
            &[Hunk {
                before: 2..4,
                after: 2..3,
            }],
        );
        cli::stage_patch(&root, &patch).unwrap();
        assert_eq!(
            git::get_index_content(&file, true).unwrap(),
            Some(b"a\nb\nc".to_vec())
        );
        drop(repo);
    }

    #[test]
    fn commit_records_the_index_and_reports_the_new_head() {
        let (repo, file) = repo_with_file("a\nB\nc\n");
        let root = cli::repo_root(&file).unwrap();
        assert!(!cli::has_staged_changes(&root).unwrap());
        cli::add_path(&root, &file).unwrap();
        assert!(cli::has_staged_changes(&root).unwrap());

        let head = cli::commit(&root, "Capitalize b").unwrap();
        assert!(head.ends_with(" Capitalize b"), "{head}");
        assert!(!cli::has_staged_changes(&root).unwrap());
        assert_eq!(
            git::get_diff_base(&file, true).unwrap(),
            b"a\nB\nc\n".to_vec()
        );
        drop(repo);
    }

    #[test]
    fn commit_failure_is_reported_as_an_error() {
        let (_repo, file) = repo_with_file("a\nb\nc\n");
        let root = cli::repo_root(&file).unwrap();
        assert!(cli::commit(&root, "nothing staged").is_err());
    }

    #[test]
    fn reset_path_takes_a_new_file_out_of_the_index() {
        let (repo, _) = repo_with_file("a\nb\nc\n");
        let other = repo.path().join("new.txt");
        write(&other, "new\n");
        let root = cli::repo_root(&other).unwrap();
        cli::add_path(&root, &other).unwrap();
        assert!(cli::has_staged_changes(&root).unwrap());
        cli::reset_path(&root, &other).unwrap();
        assert!(!cli::has_staged_changes(&root).unwrap());
    }
}
