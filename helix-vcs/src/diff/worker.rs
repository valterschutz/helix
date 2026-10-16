use std::sync::Arc;

use helix_core::{Rope, RopeSlice};
use imara_diff::{IndentHeuristic, IndentLevel, InternedInput};
use parking_lot::RwLock;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Notify;
use tokio::time::{timeout, timeout_at, Duration};

use crate::diff::{
    staging, DiffInner, Event, RenderLock, ALGORITHM, DIFF_DEBOUNCE_TIME_ASYNC,
    DIFF_DEBOUNCE_TIME_SYNC,
};
use imara_diff::Hunk;

use super::line_cache::InternedRopeLines;

#[cfg(test)]
mod test;

pub(super) struct DiffWorker {
    pub channel: UnboundedReceiver<Event>,
    pub diff: Arc<RwLock<DiffInner>>,
    pub diff_finished_notify: Arc<Notify>,
    pub diff_alloc: imara_diff::Diff,
    /// The index content, when the file is in the index.
    pub index: Option<Rope>,
    /// Changes from the diff base to `index`; empty without an index.
    pub staged: Vec<Hunk>,
    /// Changes from `index` to the document; empty without an index.
    pub unstaged: Vec<Hunk>,
}

/// A new base and its index content, accumulated from base events.
struct BaseUpdate {
    diff_base: Rope,
    index: Option<Rope>,
}

impl DiffWorker {
    async fn accumulate_events(&mut self, event: Event) -> (Option<Rope>, Option<BaseUpdate>) {
        let mut accumulator = EventAccumulator::new();
        accumulator.handle_event(event).await;
        accumulator
            .accumulate_debounced_events(&mut self.channel, self.diff_finished_notify.clone())
            .await;
        (accumulator.doc, accumulator.diff_base)
    }

    pub async fn run(mut self, diff_base: Rope, index: Option<Rope>, doc: Rope) {
        let mut interner = InternedRopeLines::new(diff_base, doc);
        if let Some(lines) = interner.interned_lines() {
            self.perform_diff(lines);
        }
        self.index = index;
        self.perform_staging_diffs(interner.diff_base(), true, interner.doc());
        self.apply_hunks(interner.diff_base(), interner.doc());
        while let Some(event) = self.channel.recv().await {
            let (doc, diff_base) = self.accumulate_events(event).await;

            let process_accumulated_events = || {
                let base_changed = diff_base.is_some();
                if let Some(BaseUpdate { diff_base, index }) = diff_base {
                    self.index = index;
                    interner.update_diff_base(diff_base, doc)
                } else {
                    interner.update_doc(doc.unwrap())
                }

                if let Some(lines) = interner.interned_lines() {
                    self.perform_diff(lines)
                }
                self.perform_staging_diffs(interner.diff_base(), base_changed, interner.doc());
            };

            // Calculating diffs is computationally expensive and should
            // not run inside an async function to avoid blocking other futures.
            // Note: tokio::task::block_in_place does not work during tests
            #[cfg(test)]
            process_accumulated_events();
            #[cfg(not(test))]
            tokio::task::block_in_place(process_accumulated_events);

            self.apply_hunks(interner.diff_base(), interner.doc());
        }
    }

    /// update the hunks (used by the gutter) by replacing it with `self.new_hunks`.
    /// `self.new_hunks` is always empty after this function runs.
    /// To improve performance this function tries to reuse the allocation of the old diff previously stored in `self.line_diffs`
    fn apply_hunks(&mut self, diff_base: Rope, doc: Rope) {
        let mut diff = self.diff.write();
        diff.diff_base = diff_base;
        diff.doc = doc;
        diff.hunks.clear();
        diff.hunks.extend(self.diff_alloc.hunks());
        diff.index = self.index.clone();
        diff.staged.clone_from(&self.staged);
        diff.unstaged.clone_from(&self.unstaged);
        drop(diff);
        self.diff_finished_notify.notify_waiters();
    }

    /// Diff the base against the index (only when the base changed, since the index comes with
    /// it) and the index against the document, for staging states.
    fn perform_staging_diffs(&mut self, diff_base: Rope, base_changed: bool, doc: Rope) {
        let Some(index) = self.index.clone() else {
            self.staged.clear();
            self.unstaged.clear();
            return;
        };
        if base_changed {
            self.staged = staging::diff_ropes(diff_base, index.clone());
        }
        self.unstaged = staging::diff_ropes(index, doc);
    }

    fn perform_diff(&mut self, input: &InternedInput<RopeSlice>) {
        self.diff_alloc.compute_with(
            ALGORITHM,
            &input.before,
            &input.after,
            input.interner.num_tokens(),
        );
        self.diff_alloc.postprocess_with(
            &input.before,
            &input.after,
            IndentHeuristic::new(|token| {
                IndentLevel::for_ascii_line(input.interner[token].bytes(), 4)
            }),
        );
    }
}

struct EventAccumulator {
    diff_base: Option<BaseUpdate>,
    doc: Option<Rope>,
    render_lock: Option<RenderLock>,
}

impl EventAccumulator {
    fn new() -> EventAccumulator {
        EventAccumulator {
            diff_base: None,
            doc: None,
            render_lock: None,
        }
    }

    async fn handle_event(&mut self, event: Event) {
        if event.is_base {
            self.diff_base = Some(BaseUpdate {
                diff_base: event.text,
                index: event.index,
            });
        } else {
            self.doc = Some(event.text);
        }

        // always prefer the most synchronous requested render mode
        if let Some(render_lock) = event.render_lock {
            match &mut self.render_lock {
                Some(RenderLock { timeout, .. }) => {
                    // A timeout of `None` means that the render should
                    // always wait for the diff to complete (so no timeout)
                    // remove the existing timeout, otherwise keep the previous timeout
                    // because it will be shorter then the current timeout
                    if render_lock.timeout.is_none() {
                        timeout.take();
                    }
                }
                None => self.render_lock = Some(render_lock),
            }
        }
    }

    async fn accumulate_debounced_events(
        &mut self,
        channel: &mut UnboundedReceiver<Event>,
        diff_finished_notify: Arc<Notify>,
    ) {
        let async_debounce = Duration::from_millis(DIFF_DEBOUNCE_TIME_ASYNC);
        let sync_debounce = Duration::from_millis(DIFF_DEBOUNCE_TIME_SYNC);
        loop {
            // if we are not blocking rendering use a much longer timeout
            let debounce = if self.render_lock.is_none() {
                async_debounce
            } else {
                sync_debounce
            };

            if let Ok(Some(event)) = timeout(debounce, channel.recv()).await {
                self.handle_event(event).await;
            } else {
                break;
            }
        }

        // setup task to trigger the rendering
        match self.render_lock.take() {
            // diff is performed outside of the rendering loop
            // request a redraw after the diff is done
            None => {
                tokio::spawn(async move {
                    diff_finished_notify.notified().await;
                    helix_event::request_redraw();
                });
            }
            // diff is performed inside the rendering loop
            // block redraw until the diff is done or the timeout is expired
            Some(RenderLock {
                lock,
                timeout: Some(timeout),
            }) => {
                tokio::spawn(async move {
                    let res = {
                        // Acquire a lock on the redraw handle.
                        // The lock will block the rendering from occurring while held.
                        // The rendering waits for the diff if it doesn't time out
                        timeout_at(timeout, diff_finished_notify.notified()).await
                    };
                    // we either reached the timeout or the diff is finished, release the render lock
                    drop(lock);
                    if res.is_ok() {
                        // Diff finished in time we are done.
                        return;
                    }
                    // Diff failed to complete in time log the event
                    // and wait until the diff occurs to trigger an async redraw
                    log::info!("Diff computation timed out, update of diffs might appear delayed");
                    diff_finished_notify.notified().await;
                    helix_event::request_redraw()
                });
            }
            // a blocking diff is performed inside the rendering loop
            // block redraw until the diff is done
            Some(RenderLock {
                lock,
                timeout: None,
            }) => {
                tokio::spawn(async move {
                    diff_finished_notify.notified().await;
                    // diff is done release the lock
                    drop(lock)
                });
            }
        };
    }
}
