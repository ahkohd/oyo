use super::{App, ContentResponse};
use oyo_core::multi::{ContentSource, PendingFileContent};
use oyo_core::MultiFileDiff;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

pub(super) struct ContentWorker {
    rx: mpsc::Receiver<ContentResponse>,
    context: crate::remote::loading::ReadContext,
    selected: Arc<AtomicUsize>,
    foreground: Arc<Mutex<Option<(usize, crate::remote::loading::ReadContext)>>>,
    queue: Option<Arc<Mutex<VecDeque<PendingFileContent>>>>,
}

impl ContentWorker {
    fn try_recv(&self) -> Result<ContentResponse, mpsc::TryRecvError> {
        self.rx.try_recv()
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
    requests: Vec<PendingFileContent>,
    tx: &mpsc::SyncSender<ContentResponse>,
) {
    if requests.iter().any(|r| {
        matches!(r.old, ContentSource::RemoteBlob { .. })
            || matches!(r.new, ContentSource::RemoteBlob { .. })
    }) {
        for request in requests {
            let content = match crate::remote::load_pair(&request) {
                Ok((old, new)) => MultiFileDiff::prepare_remote_file_content(old, new),
                Err(error) => {
                    MultiFileDiff::failed_file_content(&format!("Remote file unavailable: {error}"))
                }
            };
            if crate::remote::loading::cancelled() {
                break;
            }
            if tx
                .send(ContentResponse {
                    generation,
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
    load_requests(0, vec![request], &tx);
    rx.recv()
        .expect("content loader must return one response")
        .content
}

impl App {
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
                    .extend(requests);
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
        let (tx, rx) = mpsc::sync_channel(2);
        let selected = Arc::new(AtomicUsize::new(selected));
        let remote = self.multi_diff.remote_snapshot.is_some();
        let foreground = Arc::new(Mutex::new(None));
        let mut shared_queue = None;
        if remote {
            let queue = Arc::new(Mutex::new(VecDeque::from(requests)));
            shared_queue = Some(queue.clone());
            for worker_index in 0..6 {
                let queue = queue.clone();
                let selected = selected.clone();
                let context = context.clone();
                let foreground = foreground.clone();
                let tx = tx.clone();
                thread::spawn(move || loop {
                    if context.is_cancelled() {
                        break;
                    }
                    let request = {
                        let mut queue = queue.lock().unwrap_or_else(|e| e.into_inner());
                        let priority = selected.load(Ordering::Acquire);
                        let index = queue.iter().position(|r| {
                            if worker_index == 0 {
                                r.file_index == priority
                            } else {
                                r.file_index != priority
                            }
                        });
                        index.and_then(|index| queue.remove(index))
                    };
                    let Some(request) = request else {
                        thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    };
                    let job = context.child(worker_index == 0);
                    if worker_index == 0 {
                        *foreground.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some((request.file_index, job.clone()));
                    }
                    job.run(|| load_requests(generation, vec![request.clone()], &tx));
                    if worker_index == 0 {
                        foreground.lock().unwrap_or_else(|e| e.into_inner()).take();
                        if job.is_cancelled() && !context.is_cancelled() {
                            queue
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push_back(request);
                        }
                    }
                });
            }
        } else {
            let first = requests.remove(0);
            thread::spawn(move || {
                load_requests(generation, vec![first], &tx);
                if !requests.is_empty() {
                    load_requests(generation, requests, &tx);
                }
            });
        }
        self.content_worker_rx = Some(ContentWorker {
            rx,
            context,
            selected,
            foreground,
            queue: shared_queue,
        });
        true
    }

    pub(crate) fn content_loading_count(&self) -> usize {
        self.content_loading.len()
    }

    pub(crate) fn poll_content_responses(&mut self) -> bool {
        let streamed = self.poll_remote_stream();
        if let Some(worker) = self.content_worker_rx.as_ref() {
            worker
                .selected
                .store(self.multi_diff.selected_index, Ordering::Release);
        }
        let started = self.multi_diff.file_count() > 0
            && self.multi_diff.remote_snapshot.is_some()
            && self.ensure_remote_content(self.multi_diff.selected_index);
        let changed = streamed || started;
        let Some(rx) = self.content_worker_rx.as_ref() else {
            return changed;
        };
        if let Some((index, context)) = rx
            .foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            if *index != self.multi_diff.selected_index {
                context.cancel();
            }
        }
        let responses = (0..4).map_while(|_| rx.try_recv().ok()).collect::<Vec<_>>();
        if responses.is_empty() {
            return changed;
        }
        for response in responses {
            if response.generation != self.content_generation
                || self.content_loading.get(&response.file_index) != Some(&response.identity)
            {
                continue;
            }
            self.content_loading.remove(&response.file_index);
            let was_sparse = self.multi_diff.has_patch_display(response.file_index);
            let promotion = if was_sparse {
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
                    self.notify(crate::toasts::ToastEvent::SelectionActionFailed(error));
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
            self.mark_diff_changed();
        }
        if self.multi_diff.remote_snapshot.is_some() && self.remote_manifest_complete() {
            self.repair_review_comments_after_diff_refresh();
        }
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
