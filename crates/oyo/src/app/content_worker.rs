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
        if remote {
            let queue = Arc::new(Mutex::new(VecDeque::from(requests)));
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
        });
        true
    }

    pub(crate) fn content_loading_count(&self) -> usize {
        self.content_loading.len()
    }

    pub(crate) fn poll_content_responses(&mut self) -> bool {
        let Some(rx) = self.content_worker_rx.as_ref() else {
            return false;
        };
        rx.selected
            .store(self.multi_diff.selected_index, Ordering::Release);
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
            return false;
        }
        for response in responses {
            if response.generation != self.content_generation
                || self.content_loading.get(&response.file_index) != Some(&response.identity)
            {
                continue;
            }
            self.content_loading.remove(&response.file_index);
            if !self
                .multi_diff
                .apply_prepared_content(response.file_index, response.content)
            {
                continue;
            }
            if response.file_index == self.multi_diff.selected_index {
                self.multi_diff.ensure_full_navigator(response.file_index);
                if self.multi_diff.remote_snapshot.is_some()
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
            self.mark_diff_changed();
        }
        if self.multi_diff.remote_snapshot.is_some() {
            self.repair_review_comments_after_diff_refresh();
        }
        if self.content_loading.is_empty() {
            self.content_worker_rx = None;
        }
        true
    }
}
