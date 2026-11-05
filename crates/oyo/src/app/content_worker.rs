use super::{App, ContentResponse};
use oyo_core::multi::{ContentSource, PendingFileContent};
use oyo_core::MultiFileDiff;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

pub(super) struct ContentSession {
    snapshot: u64,
    context: crate::remote::loading::ReadContext,
    evicted: HashMap<usize, super::remote_position::FilePromotion>,
    restore_input: Option<(usize, std::time::Instant)>,
    reviewed_fingerprints: Mutex<HashMap<usize, String>>,
}

impl Drop for ContentSession {
    fn drop(&mut self) {
        self.context.cancel();
    }
}

struct ContentJob {
    request: PendingFileContent,
    background: bool,
}

type ActiveJob = Arc<Mutex<Option<(usize, crate::remote::loading::ReadContext)>>>;

pub(super) struct ContentWorker {
    rx: mpsc::Receiver<ContentResponse>,
    background_rx: mpsc::Receiver<ContentResponse>,
    context: crate::remote::loading::ReadContext,
    selected: Arc<AtomicUsize>,
    foreground: ActiveJob,
    background: ActiveJob,
    queue: Option<Arc<Mutex<VecDeque<ContentJob>>>>,
}

impl ContentWorker {
    fn try_recv(&self) -> Result<ContentResponse, mpsc::TryRecvError> {
        self.rx
            .try_recv()
            .or_else(|_| self.background_rx.try_recv())
    }

    fn select(&self, index: usize) -> Vec<PendingFileContent> {
        use crate::remote::loading::ReadPriority;
        self.selected.store(index, Ordering::Release);
        if let Some((active, context)) = self
            .foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            if *active != index {
                context.cancel();
            }
        }
        if let Some((active, context)) = self
            .background
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            context.set_priority(if *active == index {
                ReadPriority::Selected
            } else {
                ReadPriority::Prefetch
            });
        }
        let mut cancelled = Vec::new();
        if let Some(queue) = &self.queue {
            queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|job| {
                    let keep = job.background || job.request.file_index == index;
                    if !keep {
                        cancelled.push(job.request.clone());
                    }
                    keep
                });
        }
        cancelled
    }
}

impl Drop for ContentWorker {
    fn drop(&mut self) {
        self.context.cancel();
    }
}
use std::thread;

const MAX_CONTENT_BYTES: u64 = 32 * 1024 * 1024;

fn read_file(path: &Path) -> Option<Vec<u8>> {
    let metadata = path.metadata().ok()?;
    if metadata.len() > MAX_CONTENT_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

fn read_jj_file(repo_root: &Path, revision: &str, path: &Path) -> Option<Vec<u8>> {
    let output = Command::new("jj")
        .current_dir(repo_root)
        .arg("-R")
        .arg(repo_root)
        .arg("--no-pager")
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .args(["file", "show", "-r", revision])
        .arg(path)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn load_direct(source: &ContentSource) -> Option<Option<Vec<u8>>> {
    match source {
        ContentSource::Empty => Some(Some(Vec::new())),
        ContentSource::File(path) => Some(read_file(path)),
        ContentSource::JjFile {
            repo_root,
            revision,
            path,
        } => Some(read_jj_file(repo_root, revision, path)),
        ContentSource::GitObject { .. } | ContentSource::RemoteBlob { .. } => None,
    }
}

fn load_requests(
    generation: u64,
    snapshot: u64,
    background: bool,
    buffered: bool,
    requests: Vec<PendingFileContent>,
    tx: &mpsc::SyncSender<ContentResponse>,
) {
    if requests.iter().any(|r| {
        matches!(r.old, ContentSource::RemoteBlob { .. })
            || matches!(r.new, ContentSource::RemoteBlob { .. })
    }) {
        for request in requests {
            let (content, failure) = match crate::remote::load_pair(&request) {
                Ok((old, new)) if !crate::remote::loading::cancelled() => (
                    if !buffered {
                        MultiFileDiff::prepare_remote_file_content(old, new)
                    } else {
                        MultiFileDiff::prepare_buffered_remote_content(old, new)
                    },
                    None,
                ),
                result => {
                    let failure = if crate::remote::loading::cancelled() {
                        crate::remote::shared::ReadFailure::cancelled()
                    } else {
                        crate::remote::shared::ReadFailure::from_error(result.unwrap_err())
                    };
                    (
                        MultiFileDiff::failed_file_content(&format!(
                            "Remote file unavailable: {failure}"
                        )),
                        Some(failure),
                    )
                }
            };
            if tx
                .send(ContentResponse {
                    generation,
                    snapshot,
                    background,
                    failure,
                    file_index: request.file_index,
                    identity: request,
                    content,
                })
                .is_err()
            {
                break;
            }
        }
        return;
    }
    let mut bytes = vec![[None, None]; requests.len()];
    let mut git_groups: HashMap<PathBuf, Vec<(usize, usize, String)>> = HashMap::new();
    for (request_idx, request) in requests.iter().enumerate() {
        for (side_idx, source) in [&request.old, &request.new].into_iter().enumerate() {
            if let Some(value) = load_direct(source) {
                bytes[request_idx][side_idx] = Some(value);
            } else if let ContentSource::GitObject { repo_root, spec } = source {
                git_groups.entry(repo_root.clone()).or_default().push((
                    request_idx,
                    side_idx,
                    spec.clone(),
                ));
            }
        }
    }

    for (repo_root, group) in git_groups {
        let specs = group
            .iter()
            .map(|(_, _, spec)| spec.clone())
            .collect::<Vec<_>>();
        let objects = oyo_core::git::get_objects_batch(&repo_root, &specs, MAX_CONTENT_BYTES)
            .unwrap_or_else(|_| vec![None; specs.len()]);
        for ((request_idx, side_idx, _), object) in group.into_iter().zip(objects) {
            bytes[request_idx][side_idx] = Some(object);
        }
    }

    for (request, mut sides) in requests.into_iter().zip(bytes) {
        let old = sides[0].take().flatten();
        let new = sides[1].take().flatten();
        let response = ContentResponse {
            generation,
            snapshot,
            background,
            failure: None,
            file_index: request.file_index,
            identity: request.clone(),
            content: MultiFileDiff::prepare_file_content(old, new),
        };
        if tx.send(response).is_err() {
            break;
        }
    }
}

pub(crate) fn load_content_request_sync(
    request: PendingFileContent,
) -> oyo_core::multi::PreparedFileContent {
    let (tx, rx) = mpsc::sync_channel(2);
    load_requests(0, 0, false, false, vec![request], &tx);
    rx.recv()
        .expect("content loader must return one response")
        .content
}

impl App {
    pub(super) fn ensure_content_session(
        &mut self,
        context: Option<crate::remote::loading::ReadContext>,
    ) {
        let snapshot = self.multi_diff.content_snapshot();
        if self
            .content_session
            .as_ref()
            .is_none_or(|session| session.snapshot != snapshot)
        {
            self.content_session = Some(ContentSession {
                snapshot,
                context: context.unwrap_or_default(),
                evicted: HashMap::new(),
                restore_input: None,
                reviewed_fingerprints: Mutex::new(HashMap::new()),
            });
        }
    }

    pub(super) fn cache_remote_review_fingerprint(&self, index: usize, fingerprint: String) {
        if !self.remote_manifest_complete()
            || self.review_live_diff().file_contents(index).is_none()
        {
            return;
        }
        let session = self
            .outdated_diff_view
            .as_ref()
            .and_then(|view| view.worker_state.content_session.as_ref())
            .or(self.content_session.as_ref());
        if let Some(session) =
            session.filter(|session| session.snapshot == self.review_live_diff().content_snapshot())
        {
            session
                .reviewed_fingerprints
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(index, fingerprint);
        }
    }

    pub(super) fn retained_remote_review_fingerprint(&self, index: usize) -> Option<String> {
        let session = self
            .outdated_diff_view
            .as_ref()
            .and_then(|view| view.worker_state.content_session.as_ref())
            .or(self.content_session.as_ref())?;
        if session.snapshot != self.review_live_diff().content_snapshot() {
            return None;
        }
        session
            .reviewed_fingerprints
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&index)
            .cloned()
    }

    pub(super) fn prioritise_selected_content(&mut self) {
        if let Some(worker) = self.content_worker_rx.as_ref() {
            for request in worker.select(self.multi_diff.selected_index) {
                self.content_loading.remove(&request.file_index);
                self.multi_diff.cancel_content_request(request.file_index);
            }
        }
    }

    pub(super) fn selected_content_context(&mut self) -> crate::remote::loading::ReadContext {
        self.ensure_content_session(None);
        self.content_session.as_ref().unwrap().context.child(true)
    }

    pub(super) fn restore_evicted_content_position(&mut self) {
        let index = self.multi_diff.selected_index;
        let Some(session) = self.content_session.as_mut() else {
            return;
        };
        if !session.evicted.contains_key(&index) {
            session.restore_input = None;
            return;
        }
        match session.restore_input {
            Some((file, input)) if file == index && input != self.diff_last_input => {
                session.evicted.remove(&index);
                session.restore_input = None;
                return;
            }
            Some((file, _)) if file == index => {}
            _ => session.restore_input = Some((index, self.diff_last_input)),
        }
        if self.multi_diff.file_is_sparse(index)
            || self.multi_diff.diff_status(index) != oyo_core::multi::DiffStatus::Ready
        {
            return;
        }
        self.multi_diff.ensure_full_navigator(index);
        if self.multi_diff.navigator_for(index).is_none() {
            return;
        }
        let saved = self
            .content_session
            .as_mut()
            .and_then(|session| session.evicted.remove(&index));
        if let Some(saved) = saved {
            self.restore_file_promotion(saved);
        }
    }

    pub(super) fn enforce_remote_retention(&mut self) {
        for index in self.multi_diff.remote_retention_victims() {
            let position = self.capture_file_promotion(index);
            if !self.multi_diff.evict_remote_content(index) {
                continue;
            }
            self.diff_queue.retain(|i| *i != index);
            if let Some(session) = self.content_session.as_mut() {
                if let Some(position) = position {
                    session.evicted.insert(index, position);
                }
            }
            if let Some(cache) = self.syntax_caches.get_mut(index) {
                *cache = None;
            }
            if let Some(cache) = self.fold_scope_caches.get_mut(index) {
                *cache = None;
            }
            if let Some(state) = self.step_state_snapshots.get_mut(index) {
                *state = None;
            }
            if let Some(state) = self.no_step_state_snapshots.get_mut(index) {
                *state = None;
            }
            for tab in self
                .topbar_tabs
                .iter_mut()
                .filter(|tab| tab.content == super::TopbarTabContent::File(index))
            {
                tab.navigator_state = None;
                tab.preview_rendered = false;
                self.structured_previews.remove(&tab.id);
                self.csv_previews.remove(&tab.id);
            }
            self.max_line_widths_step[index] = 0;
            self.max_line_widths_no_step[index] = 0;
        }
    }

    fn start_remote_prefetch(&mut self) -> bool {
        let selected = self.multi_diff.selected_index;
        if self.content_loading.keys().any(|index| *index != selected) {
            return false;
        }
        let Some(index) = self.multi_diff.next_remote_prefetch() else {
            return false;
        };
        if !self.multi_diff.request_full_content(index) {
            return false;
        }
        self.start_content_loading()
    }

    pub(crate) fn start_content_loading(&mut self) -> bool {
        self.start_content_loading_with_context(crate::remote::loading::ReadContext::default())
    }

    pub(crate) fn start_content_loading_with_context(
        &mut self,
        context: crate::remote::loading::ReadContext,
    ) -> bool {
        let mut requests = self.multi_diff.take_pending_content();
        if requests.is_empty() {
            return false;
        }
        let remote = self.multi_diff.remote_snapshot.is_some();
        let buffered = self.multi_diff.remote_prefetch_enabled();
        if remote {
            self.ensure_content_session(Some(context));
        }
        if let Some(worker) = self.content_worker_rx.as_ref() {
            if let Some(queue) = &worker.queue {
                worker
                    .selected
                    .store(self.multi_diff.selected_index, Ordering::Release);
                for request in &requests {
                    self.content_loading
                        .insert(request.file_index, request.clone());
                }
                queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend(requests.into_iter().map(|request| ContentJob {
                        background: request.file_index != self.multi_diff.selected_index,
                        request,
                    }));
                return true;
            }
        }
        self.content_generation = self.content_generation.wrapping_add(1);
        let generation = self.content_generation;
        self.content_loading.clear();
        for request in &requests {
            self.content_loading
                .insert(request.file_index, request.clone());
        }
        let selected = self.multi_diff.selected_index;
        requests.sort_by_key(|request| usize::from(request.file_index != selected));
        let (tx, rx) = mpsc::sync_channel(if buffered { 1 } else { 2 });
        let (background_tx, background_rx) = mpsc::sync_channel(1);
        let selected = Arc::new(AtomicUsize::new(selected));
        let context = if remote {
            self.content_session.as_ref().unwrap().context.child(false)
        } else {
            crate::remote::loading::ReadContext::default()
        };
        let snapshot = self.multi_diff.content_snapshot();
        let foreground = Arc::new(Mutex::new(None));
        let background = Arc::new(Mutex::new(None));
        let mut shared_queue = None;
        if remote {
            let queue = Arc::new(Mutex::new(
                requests
                    .into_iter()
                    .map(|request| ContentJob {
                        background: request.file_index != selected.load(Ordering::Acquire),
                        request,
                    })
                    .collect::<VecDeque<_>>(),
            ));
            shared_queue = Some(queue.clone());
            for worker_index in 0..if buffered { 2 } else { 6 } {
                let queue = queue.clone();
                let selected = selected.clone();
                let context = context.clone();
                let foreground = foreground.clone();
                let background = background.clone();
                let tx = if worker_index == 0 || !buffered {
                    tx.clone()
                } else {
                    background_tx.clone()
                };
                thread::spawn(move || loop {
                    if context.is_cancelled() {
                        break;
                    }
                    if buffered
                        && worker_index != 0
                        && background
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .is_some()
                    {
                        thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    let request = {
                        let mut queue = queue.lock().unwrap_or_else(|e| e.into_inner());
                        let priority = selected.load(Ordering::Acquire);
                        let index = queue.iter().position(|job| {
                            if worker_index == 0 {
                                job.request.file_index == priority
                            } else {
                                job.request.file_index != priority
                            }
                        });
                        index
                            .and_then(|index| queue.remove(index))
                            .map(|job| job.request)
                    };
                    let Some(request) = request else {
                        thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    };
                    let active = if worker_index == 0 {
                        &foreground
                    } else {
                        &background
                    };
                    let job_context = if worker_index == 0 {
                        context.child(true)
                    } else if buffered {
                        context.prefetch_child()
                    } else {
                        context.child(false)
                    };
                    if worker_index == 0 || buffered {
                        *active.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some((request.file_index, job_context.clone()));
                    }
                    job_context.run(|| {
                        load_requests(
                            generation,
                            snapshot,
                            buffered && worker_index != 0,
                            buffered,
                            vec![request],
                            &tx,
                        )
                    });
                    if worker_index == 0 || !buffered {
                        active.lock().unwrap_or_else(|e| e.into_inner()).take();
                    }
                });
            }
        } else {
            let first = requests.remove(0);
            thread::spawn(move || {
                load_requests(generation, snapshot, false, false, vec![first], &tx);
                if !requests.is_empty() {
                    load_requests(generation, snapshot, false, false, requests, &tx);
                }
            });
        }
        self.content_worker_rx = Some(ContentWorker {
            rx,
            background_rx,
            context,
            selected,
            foreground,
            background,
            queue: shared_queue,
        });
        true
    }

    pub(crate) fn content_loading_count(&self) -> usize {
        self.content_loading.len()
    }

    pub(crate) fn poll_content_responses(&mut self) -> bool {
        let streamed = self.poll_remote_stream();
        self.prioritise_selected_content();
        self.restore_evicted_content_position();
        self.enforce_remote_retention();
        let started = self.multi_diff.file_count() > 0
            && self.multi_diff.remote_snapshot.is_some()
            && self.ensure_remote_content(self.multi_diff.selected_index);
        let prefetched = self.start_remote_prefetch();
        let changed = streamed || started || prefetched;
        let Some(rx) = self.content_worker_rx.as_ref() else {
            return changed;
        };
        let responses = (0..4).map_while(|_| rx.try_recv().ok()).collect::<Vec<_>>();
        if responses.is_empty() {
            return changed;
        }
        for response in responses {
            if response.background {
                if let Some(worker) = &self.content_worker_rx {
                    worker
                        .background
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take();
                }
            }
            if response.generation != self.content_generation
                || response.snapshot != self.multi_diff.content_snapshot()
                || self.content_loading.get(&response.file_index) != Some(&response.identity)
            {
                continue;
            }
            self.content_loading.remove(&response.file_index);
            if let Some(failure) = &response.failure {
                use crate::remote::shared::FailureKind;
                if failure.kind == FailureKind::Cancelled {
                    self.multi_diff.cancel_content_request(response.file_index);
                    continue;
                }
                if matches!(failure.kind, FailureKind::Blocked | FailureKind::Other)
                    && self.multi_diff.suspend_remote_prefetch()
                    && response.background
                {
                    self.notify(crate::toasts::ToastEvent::SelectionActionFailed(format!(
                        "Background loading paused: {failure}"
                    )));
                }
            }
            let was_sparse = self.multi_diff.has_patch_display(response.file_index);
            let promotion =
                if was_sparse && self.multi_diff.navigator_for(response.file_index).is_some() {
                    self.capture_file_promotion(response.file_index)
                } else {
                    None
                };
            if !self
                .multi_diff
                .apply_prepared_content(response.file_index, response.content)
            {
                continue;
            }
            if self.multi_diff.file_is_sparse(response.file_index) {
                if let Some(error) = self
                    .multi_diff
                    .hydration_failure(response.file_index)
                    .map(str::to_owned)
                {
                    if !response.background || response.file_index == self.multi_diff.selected_index
                    {
                        self.notify(crate::toasts::ToastEvent::SelectionActionFailed(error));
                    }
                }
                continue;
            }
            if response.file_index == self.multi_diff.selected_index {
                self.multi_diff.ensure_full_navigator(response.file_index);
                if !was_sparse
                    && self.multi_diff.remote_snapshot.is_some()
                    && !self.stepping
                    && self.multi_diff.diff_status(response.file_index)
                        == oyo_core::multi::DiffStatus::Ready
                {
                    self.finish_file_enter();
                }
                self.clear_fold_context_caches();
                self.reset_current_max_line_width();
                if let Some(cache) = self.syntax_caches.get_mut(response.file_index) {
                    *cache = None;
                }
                if let Some(cache) = self.fold_scope_caches.get_mut(response.file_index) {
                    *cache = None;
                }
                self.syntax_scope_cache = None;
                let _ = self.queue_current_file_diff();
            }
            if let Some(position) = promotion {
                self.restore_file_promotion(position);
            }
            self.enforce_remote_retention();
            self.mark_diff_changed();
        }
        self.restore_evicted_content_position();
        if self.multi_diff.remote_snapshot.is_some() && self.remote_manifest_complete() {
            self.repair_review_comments_after_diff_refresh();
        }
        self.start_remote_prefetch();
        if self.content_loading.is_empty() {
            self.content_worker_rx = None;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ViewMode;
    use oyo_core::patch::PatchRecord;
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn sparse_files() -> (App, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "oyo-auto-content-{}-{:?}",
            std::process::id(),
            Instant::now()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let old = (1..=100).map(|n| format!("line {n}\n")).collect::<String>();
        let new = old
            .replace("line 40\n", "changed 40\n")
            .replace("line 80\n", "changed 80\n");
        std::fs::write(root.join("old"), &old).unwrap();
        std::fs::write(root.join("new"), &new).unwrap();
        let mut diff = MultiFileDiff::from_pending_files(None, Vec::new(), true);
        for file in ["f.rs", "g.rs"] {
            let patch=format!("diff --git a/{file} b/{file}\nindex 1111111..2222222 100644\n--- a/{file}\n+++ b/{file}\n@@ -40 +40 @@\n-line 40\n+changed 40\n@@ -80 +80 @@\n-line 80\n+changed 80\n");
            diff.append_patch(
                PatchRecord::parse(patch.into_bytes()).unwrap(),
                ContentSource::File(root.join("old")),
                ContentSource::File(root.join("new")),
            );
        }
        diff.remote_snapshot = Some(json!({"acquisition":"patch-v1","streamState":"receiving"}));
        (App::new(diff, ViewMode::UnifiedPane, 0, false, None), root)
    }

    fn finish_selected(app: &mut App) {
        let until = Instant::now() + Duration::from_secs(2);
        while app.multi_diff.file_is_sparse(app.multi_diff.selected_index) && Instant::now() < until
        {
            app.poll_content_responses();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!app.multi_diff.file_is_sparse(app.multi_diff.selected_index));
    }

    #[test]
    fn selected_patch_hydrates_without_expanding_context_or_loading_other_files() {
        let (mut app, root) = sparse_files();
        assert!(app.fold_context.is_enabled());
        assert!(app.multi_diff.file_contents(0).is_none());
        finish_selected(&mut app);
        assert!(app.fold_context.is_enabled());
        assert!(
            app.current_view_with_frame(oyo_core::AnimationFrame::Idle)
                .len()
                < 50
        );
        assert!(app
            .multi_diff
            .file_contents(0)
            .unwrap()
            .1
            .contains("changed 80"));
        assert!(app.multi_diff.file_is_sparse(1));
        assert_eq!(app.content_loading_count(), 0);
        app.multi_diff.select_file(1);
        finish_selected(&mut app);
        assert!(app.fold_context.is_enabled());
        assert!(app
            .multi_diff
            .file_contents(1)
            .unwrap()
            .1
            .contains("changed 40"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn entering_a_buffered_file_keeps_no_step_at_the_new_text() {
        let (mut app, root) = sparse_files();
        app.multi_diff.request_full_content(1);
        app.multi_diff.take_pending_content_for(1);
        let old = std::fs::read(root.join("old")).unwrap();
        let new = std::fs::read(root.join("new")).unwrap();
        assert!(app
            .multi_diff
            .apply_prepared_content(1, MultiFileDiff::prepare_buffered_remote_content(old, new)));
        assert!(app.multi_diff.navigator_for(1).is_none());
        app.diff_defer = false;
        app.stepping = false;
        app.multi_diff.select_file(1);
        app.handle_file_enter();
        app.syntax_mode = crate::config::SyntaxMode::On;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(155, 45)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while app.multi_diff.diff_status(1) != oyo_core::multi::DiffStatus::Ready
            && Instant::now() < until
        {
            app.poll_diff_responses();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            app.multi_diff.diff_status(1),
            oyo_core::multi::DiffStatus::Ready
        );
        assert!(app.multi_diff.current_navigator().state().is_at_end());
        let view = app.current_view_with_frame(oyo_core::AnimationFrame::Idle);
        assert!(view.iter().any(|line| line.content.contains("changed 40")));
        assert!(app.fold_context.is_enabled());
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let text = (0..45)
            .map(|y| {
                (0..155)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("changed 40"), "{text}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn evicted_logical_position_is_restored_in_the_full_diff_completion_frame() {
        for user_moved in [false, true] {
            let (mut app, root) = sparse_files();
            app.stepping = false;
            app.ensure_content_session(None);
            let old = std::fs::read(root.join("old")).unwrap();
            let new = std::fs::read(root.join("new")).unwrap();
            app.multi_diff.request_full_content(1);
            app.multi_diff.take_pending_content_for(1);
            app.multi_diff.apply_prepared_content(
                1,
                MultiFileDiff::prepare_remote_file_content(old.clone(), new.clone()),
            );
            app.multi_diff.select_file(1);
            app.handle_file_enter();
            let nav = app.multi_diff.current_navigator();
            let id = nav
                .diff()
                .changes
                .iter()
                .find(|c| c.spans.iter().any(|s| s.old_line == Some(80)))
                .unwrap()
                .id;
            let mut state = nav.state().clone();
            state.cursor_change = Some(id);
            nav.set_state(state);
            app.save_no_step_state_snapshot(1);
            let saved = app.capture_file_promotion(1).unwrap();
            app.multi_diff.select_file(0);
            assert!(app.multi_diff.evict_remote_content(1));
            app.step_state_snapshots[1] = None;
            app.no_step_state_snapshots[1] = None;
            app.content_session
                .as_mut()
                .unwrap()
                .evicted
                .insert(1, saved);
            app.multi_diff.select_file(1);
            app.handle_file_enter();
            app.multi_diff.request_full_content(1);
            app.multi_diff.take_pending_content_for(1);
            app.multi_diff.apply_prepared_content(
                1,
                MultiFileDiff::prepare_buffered_remote_content(old, new),
            );
            app.handle_file_enter();
            if user_moved {
                app.mark_user_input();
                let nav = app.multi_diff.current_navigator();
                let id = nav
                    .diff()
                    .changes
                    .iter()
                    .find(|change| change.spans.iter().any(|span| span.old_line == Some(40)))
                    .unwrap()
                    .id;
                let mut state = nav.state().clone();
                state.cursor_change = Some(id);
                nav.set_state(state);
            }
            let until = Instant::now() + Duration::from_secs(2);
            while app.multi_diff.diff_status(1) != oyo_core::multi::DiffStatus::Ready
                && Instant::now() < until
            {
                app.poll_diff_responses();
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(
                app.multi_diff.diff_status(1),
                oyo_core::multi::DiffStatus::Ready
            );
            let nav = app.multi_diff.current_navigator();
            let id = nav.state().cursor_change.unwrap();
            let line = nav
                .diff()
                .changes
                .iter()
                .find(|c| c.id == id)
                .unwrap()
                .spans
                .iter()
                .find_map(|s| s.old_line);
            assert_eq!(line, Some(if user_moved { 40 } else { 80 }));
            assert!(!app
                .content_session
                .as_ref()
                .unwrap()
                .evicted
                .contains_key(&1));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn a_new_snapshot_queues_its_selected_diff_behind_the_previous_physical_job() {
        use super::super::types::{DiffJobKey, DiffRequest, DiffResponse};
        let (mut app, root) = sparse_files();
        app.diff_defer = false;
        let old_key = DiffJobKey {
            snapshot: app.multi_diff.content_snapshot(),
            revision: 0,
            file_index: 0,
        };
        let (request_tx, request_rx) = mpsc::channel::<DiffRequest>();
        let (response_tx, response_rx) = mpsc::channel();
        app.diff_worker_tx = Some(request_tx);
        app.diff_worker_rx = Some(response_rx);
        app.diff_inflight = Some(old_key);
        let (replacement, second_root) = sparse_files();
        app.multi_diff = replacement.multi_diff;
        let old = std::fs::read(root.join("old")).unwrap();
        let new = std::fs::read(root.join("new")).unwrap();
        app.multi_diff.request_full_content(0);
        app.multi_diff.take_pending_content_for(0);
        app.multi_diff
            .apply_prepared_content(0, MultiFileDiff::prepare_buffered_remote_content(old, new));
        assert!(app.queue_current_file_diff());
        assert_eq!(app.diff_inflight, Some(old_key));
        response_tx
            .send(DiffResponse {
                key: old_key,
                old: Arc::from("old"),
                new: Arc::from("new"),
                diff: Ok(MultiFileDiff::compute_diff("old", "new")),
            })
            .unwrap();
        app.poll_diff_responses();
        let request = request_rx.try_recv().unwrap();
        assert_ne!(request.key.snapshot, old_key.snapshot);
        assert_eq!(app.diff_inflight, Some(request.key));
        response_tx
            .send(DiffResponse {
                key: request.key,
                old: request.old.clone(),
                new: request.new.clone(),
                diff: Ok(MultiFileDiff::compute_diff(&request.old, &request.new)),
            })
            .unwrap();
        app.poll_diff_responses();
        assert_eq!(
            app.multi_diff.current_file_diff_status(),
            oyo_core::multi::DiffStatus::Ready
        );
        assert!(app
            .multi_diff
            .file_contents(0)
            .unwrap()
            .1
            .contains("changed 80"));
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(second_root).unwrap();
    }

    #[test]
    fn automatic_failure_is_stable_until_an_explicit_retry() {
        let (mut app, root) = sparse_files();
        assert!(app.multi_diff.request_full_content(0));
        app.multi_diff.take_pending_content();
        assert!(app
            .multi_diff
            .apply_prepared_content(0, MultiFileDiff::failed_file_content("fixture failure")));
        for _ in 0..20 {
            app.poll_content_responses();
        }
        assert!(app.multi_diff.file_is_sparse(0));
        assert!(app.multi_diff.hydration_failure(0).is_some());
        assert_eq!(app.content_loading_count(), 0);
        assert!(app.content_worker_rx.is_none());
        assert!(app.request_remote_content(0));
        finish_selected(&mut app);
        assert!(app.fold_context.is_enabled());
        std::fs::remove_dir_all(root).unwrap();
    }
}
