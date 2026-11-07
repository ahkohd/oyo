use super::types::SyntaxScopeCache;
use super::{display_metrics, AnimationPhase, App, ViewMode};
use crate::syntax::{SyntaxCache, SyntaxEngine, SyntaxSide};
use oyo_core::{LineKind, ViewLine};
use ratatui::text::Span;
use std::time::Instant;

#[derive(Clone)]
struct SyntaxIdentity {
    file_index: usize,
    snapshot: u64,
    revision: u64,
    file_name: String,
    old: std::sync::Arc<str>,
    new: std::sync::Arc<str>,
    engine: SyntaxEngine,
}

impl SyntaxIdentity {
    fn matches(&self, app: &App) -> bool {
        self.snapshot == app.multi_diff.content_snapshot()
            && self.revision == app.multi_diff.content_revision(self.file_index)
            && app
                .multi_diff
                .files
                .get(self.file_index)
                .is_some_and(|file| file.display_name == self.file_name)
            && app
                .multi_diff
                .file_contents_arc(self.file_index)
                .is_some_and(|(old, new)| {
                    std::sync::Arc::ptr_eq(&self.old, &old)
                        && std::sync::Arc::ptr_eq(&self.new, &new)
                })
            && app
                .syntax_engine
                .as_ref()
                .is_some_and(|engine| engine.shares_syntax_with(&self.engine))
    }

    fn same(&self, other: &Self) -> bool {
        self.file_index == other.file_index
            && self.snapshot == other.snapshot
            && self.revision == other.revision
            && self.file_name == other.file_name
            && std::sync::Arc::ptr_eq(&self.old, &other.old)
            && std::sync::Arc::ptr_eq(&self.new, &other.new)
            && self.engine.shares_syntax_with(&other.engine)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SyntaxLine {
    side: SyntaxSide,
    index: usize,
}

type RenderedSyntax = Vec<(SyntaxLine, Option<Vec<Span<'static>>>)>;

#[derive(Clone)]
pub(super) struct SyntaxFileCache {
    identity: SyntaxIdentity,
    cache: Option<SyntaxCache>,
    ready: RenderedSyntax,
    epoch: u64,
}

impl SyntaxFileCache {
    fn cached_rendered_spans(&self, side: SyntaxSide, index: usize) -> Option<Vec<Span<'static>>> {
        self.ready
            .iter()
            .find(|(line, _)| *line == SyntaxLine { side, index })
            .and_then(|(_, spans)| spans.clone())
            .or_else(|| self.cache.as_ref()?.cached_rendered_spans(side, index))
    }

    fn covers(&self, lines: &[SyntaxLine]) -> bool {
        lines
            .iter()
            .all(|line| self.ready.iter().any(|(ready, _)| ready == line))
    }
}

struct SyntaxJob {
    identity: SyntaxIdentity,
    lines: Vec<SyntaxLine>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

struct SyntaxResponse {
    job: SyntaxJob,
    ready: RenderedSyntax,
}

struct WorkerSyntaxCache {
    identity: SyntaxIdentity,
    cache: SyntaxCache,
}

impl SyntaxJob {
    fn run(self, retained: &mut Option<WorkerSyntaxCache>) -> SyntaxResponse {
        let mut ready = Vec::new();
        if !self.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            if !retained
                .as_ref()
                .is_some_and(|slot| slot.identity.same(&self.identity))
            {
                *retained = Some(WorkerSyntaxCache {
                    identity: self.identity.clone(),
                    cache: SyntaxCache::new(
                        &self.identity.engine,
                        &self.identity.old,
                        &self.identity.new,
                        &self.identity.file_name,
                        true,
                    ),
                });
            }
            let cache = &mut retained.as_mut().unwrap().cache;
            for line in &self.lines {
                if self.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let spans =
                    cache.rendered_spans_with_cancel(line.side, line.index, Some(&self.cancelled));
                ready.push((*line, spans));
            }
        }
        SyntaxResponse { job: self, ready }
    }
}

enum SyntaxWork {
    Parse(SyntaxJob),
    Clear,
}

struct SyntaxFlight {
    identity: SyntaxIdentity,
    lines: Vec<SyntaxLine>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

pub(super) struct SyntaxWorker {
    tx: std::sync::mpsc::SyncSender<SyntaxWork>,
    rx: std::sync::mpsc::Receiver<SyntaxResponse>,
    flight: Option<SyntaxFlight>,
    retained: Option<SyntaxIdentity>,
}

impl SyntaxWorker {
    fn new(
        parse: impl Fn(SyntaxJob, &mut Option<WorkerSyntaxCache>) -> SyntaxResponse + Send + 'static,
    ) -> Self {
        let (tx, requests) = std::sync::mpsc::sync_channel::<SyntaxWork>(1);
        let (responses, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut retained = None;
            while let Ok(work) = requests.recv() {
                match work {
                    SyntaxWork::Parse(job) => {
                        if responses.send(parse(job, &mut retained)).is_err() {
                            break;
                        }
                    }
                    SyntaxWork::Clear => retained = None,
                }
            }
        });
        Self {
            tx,
            rx,
            flight: None,
            retained: None,
        }
    }
}

impl Drop for SyntaxWorker {
    fn drop(&mut self) {
        if let Some(flight) = &self.flight {
            flight
                .cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[derive(Clone)]
struct FoldScopeRequest {
    file_index: usize,
    snapshot: u64,
    revision: u64,
    file_name: String,
    content: std::sync::Arc<str>,
    engine: SyntaxEngine,
}

impl FoldScopeRequest {
    fn matches(&self, app: &App) -> bool {
        self.snapshot == app.multi_diff.content_snapshot()
            && self.revision == app.multi_diff.content_revision(self.file_index)
            && app
                .multi_diff
                .files
                .get(self.file_index)
                .is_some_and(|file| file.display_name == self.file_name)
            && app
                .multi_diff
                .file_contents_arc(self.file_index)
                .is_some_and(|(_, new)| std::sync::Arc::ptr_eq(&self.content, &new))
            && app
                .syntax_engine
                .as_ref()
                .is_some_and(|engine| engine.shares_syntax_with(&self.engine))
    }
}

#[derive(Clone)]
pub(super) struct FoldScopeCache {
    request: FoldScopeRequest,
    pub(super) ranges: Vec<crate::syntax::EnclosingScopeRange>,
}

pub(super) struct FoldScopeWorker {
    tx: std::sync::mpsc::SyncSender<FoldScopeRequest>,
    rx: std::sync::mpsc::Receiver<FoldScopeCache>,
    busy: bool,
}

impl FoldScopeWorker {
    fn new(
        parse: impl Fn(&FoldScopeRequest) -> Vec<crate::syntax::EnclosingScopeRange> + Send + 'static,
    ) -> Self {
        let (tx, requests) = std::sync::mpsc::sync_channel::<FoldScopeRequest>(1);
        let (responses, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            while let Ok(request) = requests.recv() {
                let ranges = parse(&request);
                if responses.send(FoldScopeCache { request, ranges }).is_err() {
                    break;
                }
            }
        });
        Self {
            tx,
            rx,
            busy: false,
        }
    }
}

impl App {
    pub fn syntax_enabled(&self) -> bool {
        if self.multi_diff.file_count() == 0 || self.multi_diff.current_file_is_binary() {
            return false;
        }
        match self.syntax_mode {
            crate::config::SyntaxMode::On => true,
            crate::config::SyntaxMode::Off => false,
        }
    }

    pub(crate) fn set_syntax_theme(&mut self, theme: String) {
        if self.syntax_theme == theme {
            return;
        }
        self.syntax_theme = theme;
        self.syntax_engine = None;
        self.syntax_engine_rx = None;
        self.syntax_caches = vec![None; self.multi_diff.file_count()];
        self.fold_scope_caches = vec![None; self.multi_diff.file_count()];
        self.clear_fold_context_caches();
        self.unified_render_cache = None;
        self.start_syntax_engine_load();
    }

    pub(crate) fn start_syntax_engine_load(&mut self) {
        if matches!(self.syntax_mode, crate::config::SyntaxMode::Off)
            || self.syntax_engine.is_some()
            || self.syntax_engine_rx.is_some()
        {
            return;
        }
        let theme = self.syntax_theme.clone();
        let light = self.theme_is_light;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(SyntaxEngine::new(&theme, light));
        });
        self.syntax_engine_rx = Some(rx);
    }

    fn ensure_syntax_engine(&mut self) -> bool {
        if self.syntax_engine.is_some() {
            return true;
        }
        if let Some(rx) = self.syntax_engine_rx.as_ref() {
            match rx.try_recv() {
                Ok(engine) => {
                    self.syntax_engine = Some(engine);
                    self.syntax_engine_rx = None;
                    return true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    #[cfg(not(test))]
                    return false;
                    #[cfg(test)]
                    match self.syntax_engine_rx.as_ref().unwrap().recv() {
                        Ok(engine) => {
                            self.syntax_engine = Some(engine);
                            self.syntax_engine_rx = None;
                            return true;
                        }
                        Err(_) => self.syntax_engine_rx = None,
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.syntax_engine_rx = None;
                }
            }
        }
        self.syntax_engine = Some(SyntaxEngine::new(&self.syntax_theme, self.theme_is_light));
        true
    }

    pub(crate) fn preview_source_spans(
        &mut self,
        file_name: &str,
        text: &str,
    ) -> Option<Vec<Vec<Span<'static>>>> {
        if matches!(self.syntax_mode, crate::config::SyntaxMode::Off) {
            return None;
        }
        if !self.ensure_syntax_engine() {
            return None;
        }
        let remote = self.is_remote_session();
        let engine = self.syntax_engine.as_ref()?;
        Some(
            engine
                .highlight(text, file_name)
                .into_iter()
                .map(|line| {
                    line.into_iter()
                        .map(|span| {
                            Span::styled(
                                if remote {
                                    oyo_core::patch::terminal_text(&span.text)
                                } else {
                                    span.text
                                },
                                span.style,
                            )
                        })
                        .collect()
                })
                .collect(),
        )
    }

    /// Syntax-highlight the body of a fenced code block, selecting the grammar
    /// from the fence's language token. Returns one span vector per code line,
    /// or `None` when highlighting is disabled or no language was given.
    pub(crate) fn highlight_code_block(
        &mut self,
        lang: Option<&str>,
        code: &str,
    ) -> Option<Vec<Vec<Span<'static>>>> {
        if matches!(self.syntax_mode, crate::config::SyntaxMode::Off) {
            return None;
        }
        let lang = lang?;
        if !self.ensure_syntax_engine() {
            return None;
        }
        let engine = self.syntax_engine.as_ref()?;
        Some(
            engine
                .highlight_by_token(code, lang)
                .into_iter()
                .map(|line| {
                    line.into_iter()
                        .map(|span| Span::styled(span.text, span.style))
                        .collect()
                })
                .collect(),
        )
    }

    pub(crate) fn syntax_cache_epoch(&self) -> u64 {
        if !self.syntax_enabled() {
            return 0;
        }
        self.syntax_caches
            .get(self.multi_diff.selected_index)
            .and_then(|cache| cache.as_ref())
            .map(|cache| {
                cache
                    .epoch
                    .wrapping_add(1)
                    .wrapping_add(cache.cache.as_ref().map(SyntaxCache::epoch).unwrap_or(0))
            })
            .unwrap_or(0)
    }

    pub(crate) fn syntax_warmup_pending(&self) -> bool {
        self.syntax_worker
            .as_ref()
            .is_some_and(|worker| worker.flight.is_some())
            || (self.syntax_enabled()
                && self.view_mode != ViewMode::Preview
                && self
                    .syntax_warmup_target
                    .is_some_and(|target| target.file_index == self.multi_diff.selected_index)
                && !self.syntax_requested_lines.is_empty()
                && !self
                    .syntax_caches
                    .get(self.multi_diff.selected_index)
                    .and_then(Option::as_ref)
                    .is_some_and(|cache| {
                        cache.identity.matches(self) && cache.covers(&self.syntax_requested_lines)
                    }))
    }

    pub fn cached_syntax_spans_for_line(
        &mut self,
        side: SyntaxSide,
        line_num: Option<usize>,
    ) -> Option<Vec<Span<'static>>> {
        if !self.syntax_enabled() {
            return None;
        }
        let index = line_num?.checked_sub(1)?;
        self.record_syntax_warmup_line(side, index + 1);
        let line = SyntaxLine { side, index };
        if !self.syntax_frame_lines.contains(&line) {
            self.syntax_frame_lines.push(line);
        }
        let remote = self.is_remote_session();
        let mut spans = self
            .ensure_syntax_file_cache()?
            .cached_rendered_spans(side, index)?;
        if remote {
            for span in &mut spans {
                span.content = oyo_core::patch::terminal_text(&span.content).into();
            }
        }
        Some(spans)
    }

    pub fn syntax_spans_for_line(
        &mut self,
        side: SyntaxSide,
        line_num: Option<usize>,
    ) -> Option<Vec<Span<'static>>> {
        if !self.syntax_enabled() {
            return None;
        }
        let line_num = line_num?;
        if line_num == 0 {
            return None;
        }
        let cache = self.ensure_syntax_cache()?;
        let mut spans = cache.rendered_spans(side, line_num - 1)?;
        if self.is_remote_session() {
            for span in &mut spans {
                span.content = oyo_core::patch::terminal_text(&span.content).into();
            }
        }
        Some(spans)
    }

    pub(crate) fn review_grep_syntax_spans(
        &mut self,
        file_index: usize,
        side: super::review::ReviewSide,
        line_number: usize,
        text: &str,
    ) -> Option<Vec<Span<'static>>> {
        if matches!(self.syntax_mode, crate::config::SyntaxMode::Off) || line_number == 0 {
            return None;
        }
        let diff_revision = self.diff_revision();
        let identity_matches = self
            .review_grep
            .syntax_identity
            .as_ref()
            .is_some_and(|identity| {
                identity.diff_revision == diff_revision
                    && identity.ui_theme.as_ref() == self.ui_theme_name.as_ref()
                    && identity.syntax_theme == self.syntax_theme
                    && identity.light == self.theme_is_light
            });
        if !identity_matches {
            self.review_grep.syntax_identity = Some(super::grep::ReviewGrepSyntaxIdentity {
                diff_revision,
                ui_theme: self.ui_theme_name.clone(),
                syntax_theme: self.syntax_theme.clone(),
                light: self.theme_is_light,
            });
            self.review_grep.syntax_spans.clear();
        }
        let side_key = match side {
            super::review::ReviewSide::Old => 0,
            super::review::ReviewSide::New => 1,
        };
        let key = (file_index, side_key, line_number);
        if let Some(spans) = self.review_grep.syntax_spans.get(&key) {
            return spans.clone();
        }
        let file = self.multi_diff.files.get(file_index)?;
        let file_name = file.display_name.clone();
        if file.binary {
            self.review_grep.syntax_spans.insert(key, None);
            #[cfg(test)]
            {
                self.review_grep.syntax_cache_misses += 1;
            }
            return None;
        }
        if !self.ensure_syntax_engine() {
            return None;
        }
        if !self.syntax_engine.as_ref()?.has_syntax_for_file(&file_name) {
            self.review_grep.syntax_spans.insert(key, None);
            #[cfg(test)]
            {
                self.review_grep.syntax_cache_misses += 1;
            }
            return None;
        }
        let syntax_side = match side {
            super::review::ReviewSide::Old => SyntaxSide::Old,
            super::review::ReviewSide::New => SyntaxSide::New,
        };
        let spans = self
            .syntax_caches
            .get(file_index)
            .and_then(Option::as_ref)
            .and_then(|cache| cache.cached_rendered_spans(syntax_side, line_number - 1))
            .or_else(|| {
                self.syntax_engine
                    .as_ref()?
                    .highlight(text, &file_name)
                    .into_iter()
                    .next()
                    .map(|line| {
                        line.into_iter()
                            .map(|span| Span::styled(span.text, span.style))
                            .collect()
                    })
            })?;
        #[cfg(test)]
        {
            self.review_grep.syntax_cache_misses += 1;
        }
        self.review_grep
            .syntax_spans
            .insert(key, Some(spans.clone()));
        Some(spans)
    }

    #[cfg(test)]
    pub(crate) fn review_grep_syntax_cache_misses(&self) -> usize {
        self.review_grep.syntax_cache_misses
    }

    #[cfg(test)]
    pub(crate) fn review_grep_syntax_cache_len(&self) -> usize {
        self.review_grep.syntax_spans.len()
    }

    pub(crate) fn maybe_warm_syntax_cache(&mut self) -> bool {
        let response = match self
            .syntax_worker
            .as_ref()
            .map(|worker| worker.rx.try_recv())
        {
            Some(Ok(response)) => Some(response),
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.syntax_worker = None;
                None
            }
            _ => None,
        };
        let mut changed = false;
        if let Some(response) = response {
            if let Some(worker) = self.syntax_worker.as_mut() {
                worker.flight = None;
                worker.retained = Some(response.job.identity.clone());
            }
            let identity = &response.job.identity;
            let valid = identity.matches(self);
            if valid {
                if let Some(slot) = self
                    .syntax_caches
                    .get_mut(identity.file_index)
                    .and_then(Option::as_mut)
                    .filter(|slot| slot.identity.same(identity))
                {
                    if !response
                        .job
                        .cancelled
                        .load(std::sync::atomic::Ordering::Relaxed)
                        && response.job.lines == self.syntax_requested_lines
                        && identity.file_index == self.multi_diff.selected_index
                        && response.ready.len() == response.job.lines.len()
                    {
                        slot.ready = response.ready;
                        slot.epoch = slot.epoch.wrapping_add(1);
                        changed = true;
                    }
                }
            }
        }
        let enabled = self.syntax_enabled() && self.view_mode != ViewMode::Preview;
        if let Some(flight) = self
            .syntax_worker
            .as_ref()
            .and_then(|worker| worker.flight.as_ref())
        {
            if !enabled
                || flight.identity.file_index != self.multi_diff.selected_index
                || !flight.identity.matches(self)
                || flight.lines != self.syntax_requested_lines
            {
                flight
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return changed;
        }
        let clear_retained = self
            .syntax_worker
            .as_ref()
            .and_then(|worker| worker.retained.as_ref())
            .is_some_and(|identity| {
                !enabled
                    || identity.file_index != self.multi_diff.selected_index
                    || !identity.matches(self)
            });
        if clear_retained {
            if let Some(worker) = self.syntax_worker.as_mut() {
                if worker.tx.try_send(SyntaxWork::Clear).is_ok() {
                    worker.retained = None;
                }
            }
        }
        if changed {
            self.unified_render_cache = None;
        }
        if self.view_mode == ViewMode::Preview {
            return self.warm_preview_syntax_cache() || changed;
        }
        if !enabled
            || self.syntax_requested_lines.is_empty()
            || !self
                .syntax_warmup_target
                .is_some_and(|target| target.file_index == self.multi_diff.selected_index)
        {
            return changed;
        }
        let lines = self.syntax_requested_lines.clone();
        let Some(slot) = self.ensure_syntax_file_cache() else {
            return changed;
        };
        if slot.covers(&lines) {
            return changed;
        }
        let job = SyntaxJob {
            identity: slot.identity.clone(),
            lines,
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let worker = self
            .syntax_worker
            .get_or_insert_with(|| SyntaxWorker::new(SyntaxJob::run));
        let flight = SyntaxFlight {
            identity: job.identity.clone(),
            lines: job.lines.clone(),
            cancelled: job.cancelled.clone(),
        };
        match worker.tx.try_send(SyntaxWork::Parse(job)) {
            Ok(()) => worker.flight = Some(flight),
            Err(std::sync::mpsc::TrySendError::Full(_)) => {}
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => self.syntax_worker = None,
        }
        if changed {
            self.unified_render_cache = None;
        }
        changed
    }

    fn ensure_syntax_file_cache(&mut self) -> Option<&mut SyntaxFileCache> {
        if !self.syntax_enabled() {
            return None;
        }
        let index = self.multi_diff.selected_index;
        if index >= self.multi_diff.file_count() {
            return None;
        }
        if self.syntax_engine.is_none() {
            self.poll_scope_syntax_engine();
            if self.syntax_engine.is_none() {
                self.start_syntax_engine_load();
                return None;
            }
        }
        if self.syntax_caches.len() != self.multi_diff.file_count() {
            self.syntax_caches = vec![None; self.multi_diff.file_count()];
        }
        if !self.syntax_caches[index]
            .as_ref()
            .is_some_and(|cache| cache.identity.matches(self))
        {
            let (old, new) = self.multi_diff.file_contents_arc(index)?;
            let identity = SyntaxIdentity {
                file_index: index,
                snapshot: self.multi_diff.content_snapshot(),
                revision: self.multi_diff.content_revision(index),
                file_name: self.current_file_path(),
                old,
                new,
                engine: self.syntax_engine.as_ref()?.clone(),
            };
            self.syntax_caches[index] = Some(SyntaxFileCache {
                identity,
                cache: None,
                ready: Vec::new(),
                epoch: 0,
            });
        }
        self.syntax_caches[index].as_mut()
    }

    fn warm_preview_syntax_cache(&mut self) -> bool {
        if !self.syntax_enabled() {
            return false;
        }
        if self.animation_phase != AnimationPhase::Idle || self.snap_frame.is_some() {
            return false;
        }
        let idle = self.diff_last_input.elapsed().as_millis();
        let idle_threshold = self.diff_idle_ms as u128;
        let debounce = self.syntax_warmup_debounce_ms;
        let active_lines = self.syntax_warmup_active_lines;
        let pending_lines = self.syntax_warmup_pending_lines;
        let idle_lines = self.syntax_warmup_idle_lines;

        let now = Instant::now();
        let target_ready = self
            .syntax_warmup_target_at
            .map(|at| now.duration_since(at).as_millis() >= debounce as u128)
            .unwrap_or(true);
        if self.syntax_warmup_target.is_some() && !target_ready {
            return false;
        }
        let target = if target_ready {
            self.syntax_warmup_target
        } else {
            None
        };
        let target_applied = self.syntax_warmup_target_applied;
        let current_index = self.multi_diff.selected_index;

        let (apply_target, clear_target, warmed) = {
            let Some(cache) = self.ensure_syntax_cache() else {
                return false;
            };

            let mut apply_target = None;
            let mut clear_target = false;
            if let Some(target) = target {
                if target.file_index == current_index {
                    if target_applied != Some(target) {
                        cache.set_warmup_targets(target.old, target.new);
                        apply_target = Some(target);
                    }
                } else {
                    clear_target = true;
                }
            }

            let warm_pending = cache.warm_pending();
            if idle < idle_threshold && !warm_pending {
                return false;
            }
            let budget = if idle >= idle_threshold {
                idle_lines
            } else if warm_pending {
                pending_lines
            } else {
                active_lines
            };
            let warmed = cache.warm_checkpoints(budget) > 0;
            (apply_target, clear_target, warmed)
        };

        let mut changed = warmed;
        if clear_target {
            self.syntax_warmup_target = None;
            self.syntax_warmup_target_applied = None;
            changed = true;
        }
        if let Some(target) = apply_target {
            self.syntax_warmup_target_applied = Some(target);
            changed = true;
        }
        if target_ready && self.syntax_warmup_target.is_some() {
            self.syntax_warmup_target_at = None;
            changed = true;
        }
        changed
    }

    pub(crate) fn begin_syntax_warmup_frame(&mut self) {
        self.syntax_warmup_frame_old = None;
        self.syntax_warmup_frame_new = None;
        self.syntax_frame_lines.clear();
    }

    pub(crate) fn record_syntax_warmup_line(&mut self, side: SyntaxSide, line_num: usize) {
        if line_num == 0 {
            return;
        }
        let line_idx = line_num.saturating_sub(1);
        match side {
            SyntaxSide::Old => update_warmup_range(&mut self.syntax_warmup_frame_old, line_idx),
            SyntaxSide::New => update_warmup_range(&mut self.syntax_warmup_frame_new, line_idx),
        }
    }

    pub(crate) fn commit_syntax_warmup_frame(&mut self) {
        self.syntax_requested_lines = self.syntax_frame_lines.clone();
        if !self.syntax_enabled() {
            return;
        }
        let old = self.syntax_warmup_frame_old;
        let new = self.syntax_warmup_frame_new;
        if old.is_none() && new.is_none() {
            return;
        }
        let target = super::SyntaxWarmupTarget {
            file_index: self.multi_diff.selected_index,
            old,
            new,
        };
        if self.syntax_warmup_target != Some(target) {
            self.syntax_warmup_target = Some(target);
            self.syntax_warmup_target_at = Some(Instant::now());
        }
    }

    pub fn syntax_scope_target(&mut self, view: &[ViewLine]) -> Option<(usize, String)> {
        if !self.show_syntax_scopes {
            return None;
        }
        let step_direction = self.multi_diff.current_step_direction();
        let (display_len, _) = display_metrics(
            view,
            self.view_mode,
            self.animation_phase,
            self.scroll_offset,
            step_direction,
            self.split_align_lines,
        );
        if display_len == 0 {
            return None;
        }
        let viewport_height = self.last_viewport_height.max(1);
        let target_idx = self.scroll_offset.saturating_add(viewport_height / 2);
        let display_idx = target_idx.min(display_len.saturating_sub(1));

        let (side, line_num) = self.syntax_line_for_display(view, display_idx)?;
        let file_index = self.multi_diff.selected_index;
        if let Some(cache) = &self.syntax_scope_cache {
            if cache.file_index == file_index && cache.side == side && cache.line_num == line_num {
                return Some((display_idx, cache.label.clone()));
            }
        }
        if !self.ensure_syntax_engine() {
            return None;
        }
        let file_name = self.current_file_path();
        let nav = self.multi_diff.current_navigator();
        let content = match side {
            SyntaxSide::Old => nav.old_content()?,
            SyntaxSide::New => nav.new_content()?,
        };
        let engine = self.syntax_engine.as_ref()?;
        let scopes = engine.scopes_for_line(content, &file_name, line_num - 1);
        let label = if scopes.is_empty() {
            "scopes: (none)".to_string()
        } else {
            format!("scopes: {}", scopes.join(" | "))
        };
        self.syntax_scope_cache = Some(SyntaxScopeCache {
            file_index,
            side,
            line_num,
            label: label.clone(),
        });
        Some((display_idx, label))
    }

    pub(crate) fn rebuild_current_syntax_cache_after_reload(&mut self) {
        let idx = self.multi_diff.selected_index;
        if idx >= self.multi_diff.file_count() {
            return;
        }
        if self.syntax_caches.len() != self.multi_diff.file_count() {
            self.syntax_caches = (0..self.multi_diff.file_count()).map(|_| None).collect();
        } else {
            self.syntax_caches[idx] = None;
        }
        if self.fold_scope_caches.len() != self.multi_diff.file_count() {
            self.fold_scope_caches = vec![None; self.multi_diff.file_count()];
        } else {
            self.fold_scope_caches[idx] = None;
        }
        self.syntax_scope_cache = None;
        self.syntax_warmup_target = None;
        self.syntax_warmup_target_applied = None;
        self.syntax_warmup_target_at = None;
        self.syntax_warmup_frame_old = None;
        self.syntax_warmup_frame_new = None;

        if self.view_mode == ViewMode::Preview {
            let budget = self
                .syntax_warmup_idle_lines
                .max(self.last_viewport_height.saturating_mul(4))
                .max(1);
            if let Some(cache) = self.ensure_syntax_cache() {
                cache.warm_checkpoints(budget);
            }
        } else {
            self.ensure_syntax_file_cache();
        }
    }

    fn poll_scope_syntax_engine(&mut self) -> bool {
        if self.syntax_engine.is_some() {
            return false;
        }
        let Some(rx) = self.syntax_engine_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(engine) => {
                self.syntax_engine = Some(engine);
                self.syntax_engine_rx = None;
                self.clear_fold_context_caches();
                true
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.syntax_engine_rx = None;
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
        }
    }

    pub(crate) fn poll_fold_scope_results(&mut self) -> bool {
        let engine_ready = self.poll_scope_syntax_engine();
        let Some(worker) = self.fold_scope_worker.as_mut() else {
            return engine_ready;
        };
        let result = match worker.rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return engine_ready,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.fold_scope_worker = None;
                return true;
            }
        };
        worker.busy = false;
        if result.request.matches(self) {
            let index = result.request.file_index;
            if let Some(cache) = self.fold_scope_caches.get_mut(index) {
                *cache = Some(result);
            }
        }
        self.clear_fold_context_caches();
        true
    }

    pub(crate) fn ensure_current_fold_scope_cache(&mut self) {
        let idx = self.multi_diff.selected_index;
        if idx >= self.multi_diff.file_count() {
            return;
        }
        if self.fold_scope_caches.len() != self.multi_diff.file_count() {
            self.fold_scope_caches = vec![None; self.multi_diff.file_count()];
        }
        self.poll_fold_scope_results();
        if self.fold_scope_caches[idx]
            .as_ref()
            .is_some_and(|cache| cache.request.matches(self))
        {
            return;
        }
        self.fold_scope_caches[idx] = None;
        let Some(file_name) = self
            .multi_diff
            .current_file()
            .map(|file| file.display_name.clone())
        else {
            return;
        };
        let Some((_, content)) = self.multi_diff.file_contents_arc(idx) else {
            return;
        };
        if self.syntax_engine.is_none() {
            if self.syntax_engine_rx.is_none() {
                let theme = self.syntax_theme.clone();
                let light = self.theme_is_light;
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(SyntaxEngine::new(&theme, light));
                });
                self.syntax_engine_rx = Some(rx);
            }
            return;
        }
        let worker = self.fold_scope_worker.get_or_insert_with(|| {
            FoldScopeWorker::new(|request| {
                request
                    .engine
                    .enclosing_scope_ranges(&request.content, &request.file_name)
            })
        });
        if worker.busy {
            return;
        }
        let request = FoldScopeRequest {
            file_index: idx,
            snapshot: self.multi_diff.content_snapshot(),
            revision: self.multi_diff.content_revision(idx),
            file_name,
            content,
            engine: self.syntax_engine.as_ref().unwrap().clone(),
        };
        if worker.tx.try_send(request).is_ok() {
            worker.busy = true;
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_for_current_fold_scopes(&mut self) {
        let index = self.multi_diff.selected_index;
        let until = Instant::now() + std::time::Duration::from_secs(30);
        loop {
            self.ensure_current_fold_scope_cache();
            if self.fold_scope_caches[index].is_some() {
                break;
            }
            assert!(Instant::now() < until, "scope parsing did not complete");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    pub(crate) fn ensure_syntax_cache(&mut self) -> Option<&mut SyntaxCache> {
        if !self.syntax_enabled() {
            return None;
        }
        if !self.ensure_syntax_engine() {
            return None;
        }
        let busy = self
            .syntax_worker
            .as_ref()
            .is_some_and(|worker| worker.flight.is_some());
        let slot = self.ensure_syntax_file_cache()?;
        if slot.cache.is_none() {
            if busy {
                return None;
            }
            slot.cache = Some(SyntaxCache::new(
                &slot.identity.engine,
                &slot.identity.old,
                &slot.identity.new,
                &slot.identity.file_name,
                true,
            ));
        }
        slot.cache.as_mut()
    }

    fn syntax_line_for_display(
        &self,
        view: &[ViewLine],
        display_idx: usize,
    ) -> Option<(SyntaxSide, usize)> {
        match self.view_mode {
            ViewMode::UnifiedPane | ViewMode::Blame => view.get(display_idx).and_then(|line| {
                line.new_line.or(line.old_line).map(|line_num| {
                    let side = if line.new_line.is_some() {
                        SyntaxSide::New
                    } else {
                        SyntaxSide::Old
                    };
                    (side, line_num)
                })
            }),
            ViewMode::Evolution => {
                let mut display_count = 0usize;
                for line in view {
                    let visible = match line.kind {
                        LineKind::Deleted => false,
                        LineKind::PendingDelete => {
                            line.is_active && self.animation_phase != AnimationPhase::Idle
                        }
                        _ => true,
                    };
                    if visible {
                        if display_count == display_idx {
                            let line_num = line.new_line.or(line.old_line)?;
                            let side = if line.new_line.is_some() {
                                SyntaxSide::New
                            } else {
                                SyntaxSide::Old
                            };
                            return Some((side, line_num));
                        }
                        display_count += 1;
                    }
                }
                None
            }
            ViewMode::Split => {
                let align_lines = self.split_align_lines;
                let mut old_count = 0usize;
                let mut new_count = 0usize;
                let mut old_line = None;
                let mut new_line = None;

                for line in view {
                    let old_present = line.old_line.is_some();
                    let new_present = line.new_line.is_some()
                        && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete);
                    if old_present || (align_lines && new_present) {
                        if old_present && old_count == display_idx {
                            old_line = line.old_line;
                        }
                        old_count += 1;
                    }
                    if new_present || (align_lines && old_present) {
                        if new_present && new_count == display_idx {
                            new_line = line.new_line;
                        }
                        new_count += 1;
                    }
                    if new_line.is_some() || (old_count > display_idx && new_count > display_idx) {
                        break;
                    }
                }

                if let Some(line_num) = new_line {
                    Some((SyntaxSide::New, line_num))
                } else {
                    old_line.map(|line_num| (SyntaxSide::Old, line_num))
                }
            }
            ViewMode::Preview => None,
        }
    }
}

fn update_warmup_range(range: &mut Option<super::WarmupRange>, line_idx: usize) {
    match range {
        Some(existing) => {
            existing.start = existing.start.min(line_idx);
            existing.end = existing.end.max(line_idx);
        }
        None => {
            *range = Some(super::WarmupRange {
                start: line_idx,
                end: line_idx,
            });
        }
    }
}

#[cfg(test)]
mod fold_scope_tests {
    use super::*;
    use oyo_core::{AnimationFrame, MultiFileDiff};
    use std::sync::mpsc;
    use std::time::Duration;

    fn app() -> App {
        let old = format!(
            "fn old_scope() {{\n{}\n}}\n",
            "    let value = 1;\n".repeat(50)
        );
        let new = old.replacen("value = 1", "value = 2", 1);
        let mut app = App::new(
            MultiFileDiff::from_file_pairs(
                (0..3)
                    .map(|i| (format!("file{i}.rs").into(), old.clone(), new.clone()))
                    .collect(),
            ),
            ViewMode::UnifiedPane,
            0,
            false,
            None,
        );
        app.syntax_engine = Some(SyntaxEngine::new("ansi", false));
        app.stepping = false;
        app.multi_diff.current_navigator().goto_end();
        app
    }

    fn finish(app: &mut App) {
        let until = Instant::now() + Duration::from_secs(5);
        while app
            .fold_scope_worker
            .as_ref()
            .is_some_and(|worker| worker.busy)
            && Instant::now() < until
        {
            app.poll_fold_scope_results();
            std::thread::yield_now();
        }
        assert!(!app.fold_scope_worker.as_ref().unwrap().busy);
    }

    #[test]
    fn gated_scope_parse_leaves_navigation_responsive_and_only_scans_latest_selection() {
        let mut app = app();
        let (started, starts) = mpsc::channel();
        let (release, permits) = mpsc::channel();
        let ui = std::thread::current().id();
        app.fold_scope_worker = Some(FoldScopeWorker::new(move |request| {
            assert_ne!(std::thread::current().id(), ui);
            started.send(request.file_index).unwrap();
            permits.recv_timeout(Duration::from_secs(5)).unwrap();
            request
                .engine
                .enclosing_scope_ranges(&request.content, &request.file_name)
        }));
        let _ = app.current_view_with_frame(AnimationFrame::Idle);
        assert_eq!(starts.recv_timeout(Duration::from_secs(5)).unwrap(), 0);
        app.select_file(1);
        let _ = app.current_view_with_frame(AnimationFrame::Idle);
        app.select_file(2);
        let _ = app.current_view_with_frame(AnimationFrame::Idle);
        assert_eq!(app.multi_diff.selected_index, 2);
        assert!(starts.try_recv().is_err());
        release.send(()).unwrap();
        finish(&mut app);
        let _ = app.current_view_with_frame(AnimationFrame::Idle);
        assert_eq!(starts.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
        app.scroll_offset = 3;
        app.needs_scroll_to_active = false;
        app.centered_once = true;
        app.search_query = "retained search".to_owned();
        release.send(()).unwrap();
        finish(&mut app);
        assert_eq!(app.scroll_offset, 3);
        assert!(!app.needs_scroll_to_active);
        assert!(app.centered_once);
        assert_eq!(app.search_query, "retained search");
        let _ = app.current_view_with_frame(AnimationFrame::Idle);
        assert!(app.fold_scope_caches[2].is_some());
        assert!(starts.try_recv().is_err());
    }

    #[test]
    fn scope_completion_rejects_replaced_contents_and_reloaded_engine() {
        for reload_engine in [false, true] {
            let mut app = app();
            let (started, starts) = mpsc::channel();
            let (release, permits) = mpsc::channel();
            app.fold_scope_worker = Some(FoldScopeWorker::new(move |request| {
                started.send(()).unwrap();
                permits.recv_timeout(Duration::from_secs(5)).unwrap();
                request
                    .engine
                    .enclosing_scope_ranges(&request.content, &request.file_name)
            }));
            let _ = app.current_view_with_frame(AnimationFrame::Idle);
            starts.recv_timeout(Duration::from_secs(5)).unwrap();
            if reload_engine {
                app.set_syntax_theme("github".to_owned());
            } else {
                app.multi_diff = MultiFileDiff::from_file_pairs(vec![(
                    "file0.rs".into(),
                    "old".into(),
                    "new".into(),
                )]);
            }
            release.send(()).unwrap();
            finish(&mut app);
            assert!(app.fold_scope_caches.iter().all(Option::is_none));
        }
    }
}

#[cfg(test)]
mod syntax_worker_tests {
    use super::*;
    use oyo_core::MultiFileDiff;
    use std::sync::mpsc;
    use std::time::Duration;

    fn app() -> App {
        let mut app = App::new(
            MultiFileDiff::from_file_pairs(
                (0..3)
                    .map(|i| {
                        (
                            format!("file{i}.rs").into(),
                            "let before = 1;\n".into(),
                            "let after = 2;\n".into(),
                        )
                    })
                    .collect(),
            ),
            ViewMode::UnifiedPane,
            0,
            false,
            None,
        );
        app.syntax_engine = Some(SyntaxEngine::new("ansi", false));
        app
    }

    fn demand(app: &mut App, line: usize) {
        app.begin_syntax_warmup_frame();
        app.cached_syntax_spans_for_line(SyntaxSide::New, Some(line));
        app.commit_syntax_warmup_frame();
        app.maybe_warm_syntax_cache();
    }

    fn until(app: &mut App, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(app) && Instant::now() < deadline {
            app.maybe_warm_syntax_cache();
            std::thread::yield_now();
        }
        assert!(done(app));
    }

    #[test]
    fn gated_highlighting_keeps_navigation_responsive_and_coalesces_selected_work() {
        let mut app = app();
        let (started, starts) = mpsc::channel();
        let (release, permits) = mpsc::channel();
        let ui = std::thread::current().id();
        app.syntax_worker = Some(SyntaxWorker::new(move |job, retained| {
            assert_ne!(std::thread::current().id(), ui);
            started.send(job.identity.file_index).unwrap();
            permits.recv_timeout(Duration::from_secs(10)).unwrap();
            job.run(retained)
        }));
        demand(&mut app, 1);
        assert_eq!(starts.recv_timeout(Duration::from_secs(10)).unwrap(), 0);
        app.select_file(1);
        demand(&mut app, 1);
        app.select_file(2);
        demand(&mut app, 1);
        assert_eq!(app.multi_diff.selected_index, 2);
        assert!(starts.try_recv().is_err());
        release.send(()).unwrap();
        until(&mut app, |app| {
            app.syntax_worker
                .as_ref()
                .unwrap()
                .flight
                .as_ref()
                .is_some_and(|flight| flight.identity.file_index == 2)
        });
        assert_eq!(starts.recv_timeout(Duration::from_secs(10)).unwrap(), 2);
        app.scroll_offset = 3;
        app.search_query = "retained search".into();
        release.send(()).unwrap();
        until(&mut app, |app| {
            app.syntax_caches[2]
                .as_ref()
                .unwrap()
                .covers(&app.syntax_requested_lines)
        });
        let spans = app
            .cached_syntax_spans_for_line(SyntaxSide::New, Some(1))
            .unwrap();
        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "let after = 2;"
        );
        assert_eq!(app.scroll_offset, 3);
        assert_eq!(app.search_query, "retained search");
        assert!(starts.try_recv().is_err());
    }

    #[test]
    fn highlighted_viewport_keeps_ready_colors_during_new_demand() {
        let mut app = app();
        demand(&mut app, 1);
        until(&mut app, |app| {
            app.syntax_caches[0]
                .as_ref()
                .unwrap()
                .covers(&app.syntax_requested_lines)
        });
        let before = app
            .cached_syntax_spans_for_line(SyntaxSide::New, Some(1))
            .unwrap();
        let (started, starts) = mpsc::channel();
        let (release, permits) = mpsc::channel();
        app.syntax_worker = Some(SyntaxWorker::new(move |job, retained| {
            started.send(()).unwrap();
            permits.recv_timeout(Duration::from_secs(10)).unwrap();
            job.run(retained)
        }));
        demand(&mut app, 2);
        starts.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            app.cached_syntax_spans_for_line(SyntaxSide::New, Some(1))
                .unwrap(),
            before
        );
        release.send(()).unwrap();
        until(&mut app, |app| {
            app.syntax_caches[0]
                .as_ref()
                .unwrap()
                .covers(&app.syntax_requested_lines)
        });
    }

    #[test]
    fn worker_rejects_replaced_content_and_reloaded_theme_results() {
        for theme_change in [false, true] {
            let mut app = app();
            let replacement_engine = theme_change.then(|| SyntaxEngine::new("github", true));
            let (started, starts) = mpsc::channel();
            let (release, permits) = mpsc::channel();
            app.syntax_worker = Some(SyntaxWorker::new(move |job, retained| {
                started.send(()).unwrap();
                permits.recv_timeout(Duration::from_secs(10)).unwrap();
                job.run(retained)
            }));
            demand(&mut app, 1);
            starts.recv_timeout(Duration::from_secs(10)).unwrap();
            if theme_change {
                app.syntax_engine = replacement_engine;
            } else {
                app.multi_diff = MultiFileDiff::from_file_pairs(vec![(
                    "file0.rs".into(),
                    "old\n".into(),
                    "fresh content\n".into(),
                )]);
            }
            app.syntax_caches = vec![None; app.multi_diff.file_count()];
            demand(&mut app, 1);
            release.send(()).unwrap();
            until(&mut app, |app| {
                app.syntax_worker
                    .as_ref()
                    .unwrap()
                    .flight
                    .as_ref()
                    .is_some_and(|flight| {
                        !flight.cancelled.load(std::sync::atomic::Ordering::Relaxed)
                    })
            });
            starts.recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(app.syntax_caches[0].as_ref().unwrap().ready.is_empty());
            release.send(()).unwrap();
            until(&mut app, |app| {
                app.syntax_caches[0]
                    .as_ref()
                    .unwrap()
                    .covers(&app.syntax_requested_lines)
            });
            let spans = app
                .cached_syntax_spans_for_line(SyntaxSide::New, Some(1))
                .unwrap();
            let (_, text) = app.multi_diff.file_contents_arc(0).unwrap();
            let expected = app
                .syntax_engine
                .as_ref()
                .unwrap()
                .highlight(&text, "file0.rs");
            let expected = expected[0]
                .iter()
                .map(|span| Span::styled(span.text.clone(), span.style))
                .collect::<Vec<_>>();
            assert_eq!(spans, expected);
        }
    }

    #[test]
    fn cold_engine_delivery_highlights_the_latest_selection_through_ticks() {
        let _guard = crate::test_utils::DiffSettingsGuard::default();
        let mut app = App::new(
            MultiFileDiff::from_file_pairs(
                (0..3)
                    .map(|index| {
                        (
                            format!("cold{index}.rs").into(),
                            "let previous = 0;\n".to_owned(),
                            format!("let selected{index} = {};\n", index + 1),
                        )
                    })
                    .collect(),
            ),
            ViewMode::UnifiedPane,
            0,
            false,
            None,
        );
        assert!(app.syntax_engine.is_none());
        let (engines, engine_rx) = mpsc::channel();
        let (built, construction) = mpsc::channel();
        let (release, permits) = mpsc::channel();
        let (published, publication) = mpsc::channel();
        app.syntax_engine_rx = Some(engine_rx);
        let theme = app.syntax_theme.clone();
        let light = app.theme_is_light;
        let loader = std::thread::spawn(move || {
            let engine = SyntaxEngine::new(&theme, light);
            built.send(()).unwrap();
            permits.recv().unwrap();
            engines.send(engine.clone()).unwrap();
            published.send(()).unwrap();
            engine
        });
        demand(&mut app, 1);
        app.select_file(2);
        demand(&mut app, 1);
        loop {
            match construction.try_recv() {
                Ok(()) => break,
                Err(mpsc::TryRecvError::Empty) => {
                    app.tick();
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(mpsc::TryRecvError::Disconnected) => panic!("engine construction disconnected"),
            }
        }
        assert_eq!(app.multi_diff.selected_index, 2);
        assert!(app.syntax_engine.is_none());
        assert!(app
            .cached_syntax_spans_for_line(SyntaxSide::New, Some(1))
            .is_none());
        release.send(()).unwrap();
        publication.recv_timeout(Duration::from_secs(10)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let actual = loop {
            app.tick();
            if let Some(spans) = app.cached_syntax_spans_for_line(SyntaxSide::New, Some(1)) {
                break spans;
            }
            assert!(
                Instant::now() < deadline,
                "published engine did not produce selected highlights"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        let reference = loader.join().unwrap();
        let expected = reference.highlight("let selected2 = 3;\n", "cold2.rs");
        let expected = expected[0]
            .iter()
            .map(|span| Span::styled(span.text.clone(), span.style))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert_eq!(
            actual
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "let selected2 = 3;"
        );
        assert!(app.syntax_caches[..2].iter().all(Option::is_none));
    }

    #[test]
    fn deep_context_highlighting_progresses_with_zero_optional_warmup() {
        let text = format!(
            "/*\n{}*/\nlet value = 1;\n",
            "comment context\n".repeat(6_500)
        );
        let mut app = App::new(
            MultiFileDiff::from_file_pairs(vec![("file.rs".into(), text.clone(), text.clone())]),
            ViewMode::UnifiedPane,
            0,
            false,
            None,
        );
        app.syntax_engine = Some(SyntaxEngine::new("ansi", false));
        app.syntax_warmup_active_lines = 0;
        app.syntax_warmup_pending_lines = 0;
        app.syntax_warmup_idle_lines = 0;
        demand(&mut app, 6_200);
        until(&mut app, |app| {
            app.syntax_caches[0]
                .as_ref()
                .unwrap()
                .covers(&app.syntax_requested_lines)
        });
        let spans = app
            .cached_syntax_spans_for_line(SyntaxSide::New, Some(6_200))
            .unwrap();
        let expected = app
            .syntax_engine
            .as_ref()
            .unwrap()
            .highlight(&text, "file.rs");
        let expected = expected[6_199]
            .iter()
            .map(|span| Span::styled(span.text.clone(), span.style))
            .collect::<Vec<_>>();
        assert_eq!(spans, expected);
    }

    #[test]
    fn disconnected_worker_recovers_and_drop_cancels_outstanding_work() {
        let mut app = app();
        app.syntax_worker = Some(SyntaxWorker::new(|_, _| {
            panic!("synthetic worker disconnection")
        }));
        demand(&mut app, 1);
        until(&mut app, |app| {
            app.syntax_caches[0]
                .as_ref()
                .unwrap()
                .covers(&app.syntax_requested_lines)
        });
        assert!(app
            .cached_syntax_spans_for_line(SyntaxSide::New, Some(1))
            .is_some());
        let (started, starts) = mpsc::channel();
        let (release, permits) = mpsc::channel();
        let (stopped, stops) = mpsc::channel();
        struct Stopped(mpsc::Sender<()>);
        impl Drop for Stopped {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let stopped = Stopped(stopped);
        app.syntax_worker = Some(SyntaxWorker::new(move |job, retained| {
            let _ = &stopped;
            started.send(()).unwrap();
            permits.recv_timeout(Duration::from_secs(10)).unwrap();
            job.run(retained)
        }));
        demand(&mut app, 2);
        starts.recv_timeout(Duration::from_secs(10)).unwrap();
        let flag = app
            .syntax_worker
            .as_ref()
            .unwrap()
            .flight
            .as_ref()
            .unwrap()
            .cancelled
            .clone();
        drop(app);
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
        release.send(()).unwrap();
        stops.recv_timeout(Duration::from_secs(10)).unwrap();
    }

    #[test]
    fn disabled_reload_does_not_construct_highlighting_state() {
        for mode in [
            ViewMode::Preview,
            ViewMode::UnifiedPane,
            ViewMode::Split,
            ViewMode::Evolution,
        ] {
            let mut app = app();
            app.view_mode = mode;
            app.syntax_mode = crate::config::SyntaxMode::Off;
            app.syntax_engine = None;
            app.syntax_engine_rx = None;
            app.rebuild_current_syntax_cache_after_reload();
            assert!(app.syntax_engine.is_none());
            assert!(app.syntax_engine_rx.is_none());
            assert!(app.syntax_caches.iter().all(Option::is_none));
            assert!(app.syntax_worker.is_none());
        }
    }

    #[test]
    fn idle_worker_releases_evicted_source_without_another_parse_request() {
        let mut app = app();
        demand(&mut app, 1);
        until(&mut app, |app| {
            app.syntax_caches[0]
                .as_ref()
                .unwrap()
                .covers(&app.syntax_requested_lines)
        });
        let weak = std::sync::Arc::downgrade(&app.multi_diff.file_contents_arc(0).unwrap().1);
        app.multi_diff = MultiFileDiff::from_file_pairs(vec![(
            "other.rs".into(),
            "old\n".into(),
            "new\n".into(),
        )]);
        app.syntax_caches = vec![None];
        app.syntax_requested_lines.clear();
        app.maybe_warm_syntax_cache();
        until(&mut app, |_| weak.upgrade().is_none());
        assert!(app.syntax_worker.as_ref().unwrap().retained.is_none());
        assert!(app.syntax_worker.as_ref().unwrap().flight.is_none());
    }
}
