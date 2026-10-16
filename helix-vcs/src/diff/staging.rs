//! Where a change's lines sit relative to the index.
//!
//! The gutter diff compares the committed version (the diff base) with the document, so one
//! change can hold lines on both sides of the index. The worker also diffs the base against the
//! index and the index against the document; a change touched only by the first is staged, only
//! by the second is unstaged, and by both is partly staged. Base→index changes are compared on
//! the base side and index→document changes on the document side, the sides each of those diffs
//! shares with the displayed one.

use std::ops::Range;

use helix_core::Rope;
use imara_diff::Hunk;

use super::line_cache::InternedRopeLines;
use super::ALGORITHM;

/// Where one change's lines sit relative to the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagingState {
    /// Every line of the change is in the index.
    Staged,
    /// Some of the change's lines are in the index and some are only in the document.
    PartlyStaged,
    /// None of the change's lines are in the index.
    Unstaged,
}

/// Whether two line ranges on the same side of a diff touch.
///
/// A range with lines owns its lines and the boundaries around them, so an insertion directly
/// above or below it belongs to it. An empty range (a pure insertion or removal on this side)
/// is the boundary at its start and touches only ranges that reach that boundary.
pub(super) fn touches(a: &Range<u32>, b: &Range<u32>) -> bool {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => a.start == b.start,
        (true, false) => b.start <= a.start && a.start <= b.end,
        (false, true) => a.start <= b.start && b.start <= a.end,
        (false, false) => a.start < b.end && b.start < a.end,
    }
}

/// Classify one base→document change given the base→index and index→document changes.
pub(super) fn classify(hunk: &Hunk, staged: &[Hunk], unstaged: &[Hunk]) -> StagingState {
    let has_staged = staged.iter().any(|s| touches(&s.before, &hunk.before));
    let has_unstaged = unstaged.iter().any(|u| touches(&u.after, &hunk.after));
    match (has_staged, has_unstaged) {
        (true, true) => StagingState::PartlyStaged,
        (true, false) => StagingState::Staged,
        (false, _) => StagingState::Unstaged,
    }
}

/// Line-diff two ropes with the gutter's algorithm. Returns no changes for texts too large to
/// diff, like the gutter does.
pub(super) fn diff_ropes(before: Rope, after: Rope) -> Vec<Hunk> {
    let interner = InternedRopeLines::new(before, after);
    let Some(input) = interner.interned_lines() else {
        return Vec::new();
    };
    let mut diff = imara_diff::Diff::default();
    diff.compute_with(
        ALGORITHM,
        &input.before,
        &input.after,
        input.interner.num_tokens(),
    );
    diff.hunks().collect()
}
