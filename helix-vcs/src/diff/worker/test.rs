use helix_core::Rope;
use tokio::task::JoinHandle;

use crate::diff::{DiffHandle, Hunk};

impl DiffHandle {
    fn new_test(diff_base: &str, doc: &str) -> (DiffHandle, JoinHandle<()>) {
        DiffHandle::new_with_handle(Rope::from_str(diff_base), None, Rope::from_str(doc))
    }
    async fn into_diff(self, handle: JoinHandle<()>) -> Vec<Hunk> {
        let diff = self.diff;
        // dropping the channel terminates the task
        drop(self.channel);
        handle.await.unwrap();
        let diff = diff.read();
        Vec::clone(&diff.hunks)
    }
}

#[tokio::test]
async fn append_line() {
    let (differ, handle) = DiffHandle::new_test("foo\n", "foo\nbar\n");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 1..1,
            after: 1..2
        }]
    )
}

#[tokio::test]
async fn prepend_line() {
    let (differ, handle) = DiffHandle::new_test("foo\n", "bar\nfoo\n");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 0..0,
            after: 0..1
        }]
    )
}

#[tokio::test]
async fn modify() {
    let (differ, handle) = DiffHandle::new_test("foo\nbar\n", "foo bar\nbar\n");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 0..1,
            after: 0..1
        }]
    )
}

#[tokio::test]
async fn delete_line() {
    let (differ, handle) = DiffHandle::new_test("foo\nfoo bar\nbar\n", "foo\nbar\n");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 1..2,
            after: 1..1
        }]
    )
}

#[tokio::test]
async fn delete_line_and_modify() {
    let (differ, handle) = DiffHandle::new_test("foo\nbar\ntest\nfoo", "foo\ntest\nfoo bar");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[
            Hunk {
                before: 1..2,
                after: 1..1
            },
            Hunk {
                before: 3..4,
                after: 2..3
            },
        ]
    )
}

#[tokio::test]
async fn add_use() {
    let (differ, handle) = DiffHandle::new_test(
        "use ropey::Rope;\nuse tokio::task::JoinHandle;\n",
        "use ropey::Rope;\nuse ropey::RopeSlice;\nuse tokio::task::JoinHandle;\n",
    );
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 1..1,
            after: 1..2
        },]
    )
}

#[tokio::test]
async fn update_document() {
    let (differ, handle) = DiffHandle::new_test("foo\nbar\ntest\nfoo", "foo\nbar\ntest\nfoo");
    differ.update_document(Rope::from_str("foo\ntest\nfoo bar"), false);
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[
            Hunk {
                before: 1..2,
                after: 1..1
            },
            Hunk {
                before: 3..4,
                after: 2..3
            },
        ]
    )
}

#[tokio::test]
async fn update_base() {
    let (differ, handle) = DiffHandle::new_test("foo\ntest\nfoo bar", "foo\ntest\nfoo bar");
    differ.update_diff_base(Rope::from_str("foo\nbar\ntest\nfoo"), None);
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[
            Hunk {
                before: 1..2,
                after: 1..1
            },
            Hunk {
                before: 3..4,
                after: 2..3
            },
        ]
    )
}

mod staging {
    use helix_core::Rope;
    use tokio::task::JoinHandle;

    use crate::diff::{Diff, DiffHandle, StagingState};

    /// Diff `doc` against `diff_base`, with `index` as the index content, and return each
    /// change's staging state once the worker has finished.
    async fn staging_states(
        diff_base: &str,
        index: Option<&str>,
        doc: &str,
    ) -> Vec<Option<StagingState>> {
        let (differ, handle): (DiffHandle, JoinHandle<()>) = DiffHandle::new_with_handle(
            Rope::from_str(diff_base),
            index.map(Rope::from_str),
            Rope::from_str(doc),
        );
        let diff = differ.diff.clone();
        drop(differ.channel);
        handle.await.unwrap();
        let diff = Diff {
            diff: diff.read(),
            inverted: false,
        };
        (0..diff.len()).map(|n| diff.staging_state(n)).collect()
    }

    #[tokio::test]
    async fn without_index_content_no_change_has_a_staging_state() {
        assert_eq!(staging_states("a\nb\n", None, "a\nB\n").await, vec![None]);
    }

    #[tokio::test]
    async fn change_in_index_is_staged_change_only_in_doc_is_unstaged() {
        // `b -> B` is in the index; `d` is appended in the document only.
        assert_eq!(
            staging_states("a\nb\nc\n", Some("a\nB\nc\n"), "a\nB\nc\nd\n").await,
            vec![Some(StagingState::Staged), Some(StagingState::Unstaged)]
        );
    }

    #[tokio::test]
    async fn change_touched_on_both_sides_of_the_index_is_partly_staged() {
        // `b -> B` is staged, then `B` is edited again to `X` in the document.
        assert_eq!(
            staging_states("a\nb\nc\n", Some("a\nB\nc\n"), "a\nX\nc\n").await,
            vec![Some(StagingState::PartlyStaged)]
        );
    }

    #[tokio::test]
    async fn index_equal_to_base_leaves_every_change_unstaged() {
        assert_eq!(
            staging_states("a\nb\n", Some("a\nb\n"), "a\nB\nc\n").await,
            vec![Some(StagingState::Unstaged)]
        );
    }

    #[tokio::test]
    async fn insertion_next_to_a_staged_change_counts_as_touching_it() {
        // The whole file was rewritten and staged; the document then inserts a line at the top.
        assert_eq!(
            staging_states("a\n", Some("b\n"), "x\nb\n").await,
            vec![Some(StagingState::PartlyStaged)]
        );
    }
}
