use super::types::{DiffJobKey, DiffRequest, DiffResponse};
use super::App;
use oyo_core::MultiFileDiff;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

impl App {
    pub(crate) fn mark_user_input(&mut self) {
        self.release_remote_scroll();
        self.diff_last_input = Instant::now();
        self.diff_refresh_restore_end = None;
        self.scroll_to_render_end = false;
    }

    fn ensure_diff_worker(&mut self) {
        if self.diff_worker_tx.is_some() {
            return;
        }
        let (req_tx, req_rx) = mpsc::channel::<DiffRequest>();
        let (resp_tx, resp_rx) = mpsc::channel::<DiffResponse>();
        thread::spawn(move || {
            while let Ok(req) = req_rx.recv() {
                let diff = MultiFileDiff::compute_diff(req.old.as_ref(), req.new.as_ref());
                let response = DiffResponse {
                    key: req.key,
                    old: req.old,
                    new: req.new,
                    diff: Ok(diff),
                };
                if resp_tx.send(response).is_err() {
                    break;
                }
            }
        });
        self.diff_worker_tx = Some(req_tx);
        self.diff_worker_rx = Some(resp_rx);
    }

    fn diff_job_key(&self, index: usize) -> DiffJobKey {
        DiffJobKey {
            snapshot: self.multi_diff.content_snapshot(),
            revision: self.multi_diff.content_revision(index),
            file_index: index,
        }
    }

    pub(crate) fn queue_diff_for_file(&mut self, idx: usize) -> bool {
        let buffered = self.multi_diff.diff_status(idx) == oyo_core::multi::DiffStatus::Buffered;
        if (!self.diff_defer && !buffered) || (buffered && idx != self.multi_diff.selected_index) {
            return false;
        }
        if !matches!(
            self.multi_diff.diff_status(idx),
            oyo_core::multi::DiffStatus::Deferred | oyo_core::multi::DiffStatus::Buffered
        ) {
            return false;
        }
        if self.diff_inflight == Some(self.diff_job_key(idx)) || self.diff_queue.contains(&idx) {
            return false;
        }
        self.diff_queue.push_back(idx);
        let _ = self.start_next_diff_job();
        true
    }

    pub(crate) fn queue_current_file_diff(&mut self) -> bool {
        let idx = self.multi_diff.selected_index;
        self.queue_diff_for_file(idx)
    }

    pub(crate) fn queue_refreshed_current_file_diff(&mut self) -> bool {
        let idx = self.multi_diff.selected_index;
        if self.queue_diff_for_file(idx) {
            return true;
        }
        if !matches!(
            self.multi_diff.diff_status(idx),
            oyo_core::multi::DiffStatus::Deferred
        ) {
            self.diff_queue.retain(|queued| *queued != idx);
            return false;
        }
        if self.diff_queue.contains(&idx) {
            return true;
        }
        if self.diff_inflight == Some(self.diff_job_key(idx)) {
            self.diff_queue.push_back(idx);
            return true;
        }
        false
    }

    fn start_next_diff_job(&mut self) -> bool {
        if self.diff_inflight.is_some() {
            return false;
        }
        if let Some(position) = self
            .diff_queue
            .iter()
            .position(|i| *i == self.multi_diff.selected_index)
        {
            self.diff_queue.swap(0, position);
        }
        let Some(idx) = self.diff_queue.pop_front() else {
            return false;
        };
        self.ensure_diff_worker();
        let (old, new) = match self.multi_diff.file_contents_arc(idx) {
            Some((old, new)) => (old, new),
            None => return false,
        };
        if let Some(tx) = self.diff_worker_tx.as_ref() {
            self.multi_diff.mark_diff_computing(idx);
            let key = self.diff_job_key(idx);
            self.diff_inflight = Some(key);
            let request = DiffRequest { key, old, new };
            if tx.send(request).is_err() {
                self.diff_inflight = None;
            }
            return true;
        }
        false
    }

    pub(crate) fn poll_diff_responses(&mut self) -> bool {
        let Some(rx) = self.diff_worker_rx.as_mut() else {
            return false;
        };
        let mut responses = Vec::new();
        while let Ok(resp) = rx.try_recv() {
            responses.push(resp);
        }
        let changed = !responses.is_empty();
        for resp in responses {
            if self.diff_inflight == Some(resp.key) {
                self.diff_inflight = None;
            }
            let stale = resp.key.snapshot != self.multi_diff.content_snapshot()
                || resp.key.revision != self.multi_diff.content_revision(resp.key.file_index)
                || self
                    .multi_diff
                    .file_contents_arc(resp.key.file_index)
                    .is_none_or(|(old, new)| old != resp.old || new != resp.new);
            if stale || self.diff_queue.contains(&resp.key.file_index) {
                continue;
            }
            let restore_refresh_end = self.diff_refresh_restore_end == Some(resp.key.file_index);
            if self.diff_refresh_restore_end == Some(resp.key.file_index) {
                self.diff_refresh_restore_end = None;
            }
            match resp.diff {
                Ok(diff) => {
                    let promotion = if self.multi_diff.has_patch_display(resp.key.file_index) {
                        self.capture_file_promotion(resp.key.file_index)
                    } else {
                        None
                    };
                    self.multi_diff.apply_diff_result(resp.key.file_index, diff);
                    self.mark_diff_changed();
                    if resp.key.file_index == self.multi_diff.selected_index {
                        self.multi_diff.ensure_full_navigator(resp.key.file_index);
                        if !self.stepping {
                            self.ensure_step_state_snapshot(resp.key.file_index);
                        }
                        if !self.stepping || restore_refresh_end {
                            self.multi_diff.current_navigator().goto_end();
                            if !self.stepping {
                                self.multi_diff.current_navigator().clear_active_change();
                            }
                        }
                        self.invalidate_fold_context_view();
                        self.reset_current_max_line_width();
                        if (restore_refresh_end && !self.stepping)
                            || !self.files_visited[resp.key.file_index]
                            || !self.no_step_visited[resp.key.file_index]
                        {
                            self.finish_file_enter();
                        }
                    }
                    if let Some(position) = promotion {
                        self.restore_file_promotion(position);
                    }
                }
                Err(_) => {
                    self.multi_diff.mark_diff_failed(resp.key.file_index);
                }
            }
        }
        self.restore_evicted_content_position();
        let started = self.start_next_diff_job();
        changed || started
    }

    pub(crate) fn maybe_queue_idle_diff(&mut self) -> bool {
        if !self.diff_defer || self.diff_inflight.is_some() {
            return false;
        }
        if !self.diff_queue.is_empty() {
            return false;
        }
        let idle = self.diff_last_input.elapsed().as_millis();
        if idle < self.diff_idle_ms as u128 {
            return false;
        }
        for idx in 0..self.multi_diff.file_count() {
            if matches!(
                self.multi_diff.diff_status(idx),
                oyo_core::multi::DiffStatus::Deferred
            ) {
                return self.queue_diff_for_file(idx);
            }
        }
        false
    }
}
