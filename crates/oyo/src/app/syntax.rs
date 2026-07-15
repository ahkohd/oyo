use super::types::SyntaxScopeCache;
use super::{display_metrics, AnimationPhase, App, ViewMode};
use crate::syntax::{SyntaxCache, SyntaxEngine, SyntaxSide};
use oyo_core::{LineKind, ViewLine};
use ratatui::text::Span;
use std::time::Instant;

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
        let engine = self.syntax_engine.as_ref()?;
        Some(
            engine
                .highlight(text, file_name)
                .into_iter()
                .map(|line| {
                    line.into_iter()
                        .map(|span| Span::styled(span.text, span.style))
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
            .map(|cache| cache.epoch().wrapping_add(1))
            .unwrap_or(0)
    }

    pub(crate) fn syntax_warmup_pending(&self) -> bool {
        if !self.syntax_enabled() {
            return false;
        }
        self.syntax_caches
            .get(self.multi_diff.selected_index)
            .and_then(|cache| cache.as_ref())
            .map(|cache| cache.warm_pending())
            .unwrap_or(false)
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
        cache.rendered_spans(side, line_num - 1)
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
            SyntaxSide::Old => nav.old_content(),
            SyntaxSide::New => nav.new_content(),
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

        let budget = self
            .syntax_warmup_idle_lines
            .max(self.last_viewport_height.saturating_mul(4))
            .max(1);
        if let Some(cache) = self.ensure_syntax_cache() {
            cache.warm_checkpoints(budget);
        }
    }

    pub(crate) fn ensure_current_fold_scope_cache(&mut self) {
        let idx = self.multi_diff.selected_index;
        if idx >= self.multi_diff.file_count() {
            return;
        }
        if self.fold_scope_caches.len() != self.multi_diff.file_count() {
            self.fold_scope_caches = vec![None; self.multi_diff.file_count()];
        }
        if self.fold_scope_caches[idx].is_some() {
            return;
        }

        let Some(file_name) = self
            .multi_diff
            .current_file()
            .map(|file| file.display_name.clone())
        else {
            return;
        };
        let Some((_, new_content)) = self.multi_diff.file_contents_arc(idx) else {
            return;
        };
        if !self.ensure_syntax_engine() {
            return;
        }
        let ranges = self
            .syntax_engine
            .as_ref()
            .map(|engine| engine.enclosing_scope_ranges(new_content.as_ref(), &file_name))
            .unwrap_or_default();
        self.fold_scope_caches[idx] = Some(ranges);
    }

    pub(crate) fn ensure_syntax_cache(&mut self) -> Option<&mut SyntaxCache> {
        if !self.syntax_enabled() {
            return None;
        }
        let idx = self.multi_diff.selected_index;
        if idx >= self.multi_diff.file_count() {
            return None;
        }
        if idx >= self.syntax_caches.len() {
            self.syntax_caches = vec![None; self.multi_diff.file_count()];
        }
        if self.syntax_caches.get(idx)?.is_none() {
            let file_name = self.current_file_path();
            let (old_content, new_content) = self.multi_diff.file_contents_arc(idx)?;
            let force_lazy = true;
            if !self.ensure_syntax_engine() {
                return None;
            }
            let engine = self.syntax_engine.as_ref()?;
            self.syntax_caches[idx] = Some(SyntaxCache::new(
                engine,
                old_content.as_ref(),
                new_content.as_ref(),
                &file_name,
                force_lazy,
            ));
        }
        self.syntax_caches[idx].as_mut()
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
