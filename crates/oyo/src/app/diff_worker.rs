use super::types::{DiffRequest, DiffResponse};
use super::App;
use oyo_core::MultiFileDiff;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

impl App {
    pub(crate) fn mark_user_input(&mut self) {
        self.diff_last_input = Instant::now();
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
                    file_index: req.file_index,
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

    pub(crate) fn queue_diff_for_file(&mut self, idx: usize) {
        if !self.diff_defer {
            return;
        }
        if !matches!(
            self.multi_diff.diff_status(idx),
            oyo_core::multi::DiffStatus::Deferred
        ) {
            return;
        }
        if self.diff_inflight == Some(idx) || self.diff_queue.contains(&idx) {
            return;
        }
        self.diff_queue.push_back(idx);
        self.start_next_diff_job();
    }

    pub(crate) fn queue_current_file_diff(&mut self) {
        let idx = self.multi_diff.selected_index;
        self.queue_diff_for_file(idx);
    }

    fn start_next_diff_job(&mut self) {
        if self.diff_inflight.is_some() {
            return;
        }
        let Some(idx) = self.diff_queue.pop_front() else {
            return;
        };
        self.ensure_diff_worker();
        let (old, new) = match self.multi_diff.file_contents_arc(idx) {
            Some((old, new)) => (old, new),
            None => return,
        };
        if let Some(tx) = self.diff_worker_tx.as_ref() {
            self.multi_diff.mark_diff_computing(idx);
            self.diff_inflight = Some(idx);
            let request = DiffRequest {
                file_index: idx,
                old,
                new,
            };
            if tx.send(request).is_err() {
                self.diff_inflight = None;
            }
        }
    }

    pub(crate) fn poll_diff_responses(&mut self) {
        let Some(rx) = self.diff_worker_rx.as_mut() else {
            return;
        };
        let mut responses = Vec::new();
        while let Ok(resp) = rx.try_recv() {
            responses.push(resp);
        }
        for resp in responses {
            if self.diff_inflight == Some(resp.file_index) {
                self.diff_inflight = None;
            }
            match resp.diff {
                Ok(diff) => {
                    self.multi_diff.apply_diff_result(resp.file_index, diff);
                    if resp.file_index == self.multi_diff.selected_index {
                        self.multi_diff.ensure_full_navigator(resp.file_index);
                    }
                    if resp.file_index == self.multi_diff.selected_index {
                        self.reset_current_max_line_width();
                        if !self.files_visited[resp.file_index]
                            || !self.no_step_visited[resp.file_index]
                        {
                            self.finish_file_enter();
                        }
                    }
                }
                Err(_) => {
                    self.multi_diff.mark_diff_failed(resp.file_index);
                }
            }
        }
        self.start_next_diff_job();
    }

    pub(crate) fn maybe_queue_idle_diff(&mut self) {
        if !self.diff_defer || self.diff_inflight.is_some() {
            return;
        }
        if !self.diff_queue.is_empty() {
            return;
        }
        let idle = self.diff_last_input.elapsed().as_millis();
        if idle < self.diff_idle_ms as u128 {
            return;
        }
        for idx in 0..self.multi_diff.file_count() {
            if matches!(
                self.multi_diff.diff_status(idx),
                oyo_core::multi::DiffStatus::Deferred
            ) {
                self.queue_diff_for_file(idx);
                break;
            }
        }
    }
}
