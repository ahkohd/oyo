use super::types::{
    BlameCacheKey, BlamePrefetchKey, BlamePrefetchRange, BlameRequest, BlameResponse, BlameStepHint,
};
use super::App;
use crate::blame::{
    blame_line, blame_range, format_blame_github_text, format_blame_hint_text, load_git_user_name,
    BlameInfo,
};
use crate::color;
use crate::config::BlameMode;
use oyo_core::multi::BlameSource;
use oyo_core::{LineKind, ViewLine};
use ratatui::style::Color;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use time::OffsetDateTime;

impl App {
    pub(super) fn clear_blame_step_hint(&mut self) {
        self.blame_step_hint = None;
    }

    pub(super) fn clear_blame_hunk_hint(&mut self) {
        self.blame_hunk_hint = None;
    }

    pub(crate) fn ensure_blame_user_name(&mut self) {
        if self.blame_user_name.is_some() {
            return;
        }
        let root = match self.multi_diff.repo_root() {
            Some(root) => root,
            None => return,
        };
        self.blame_user_name = load_git_user_name(root);
    }

    fn active_view_line(&mut self) -> Option<ViewLine> {
        let frame = oyo_core::AnimationFrame::Idle;
        let view_lines = self.current_view_with_frame(frame);
        let mut fallback: Option<ViewLine> = None;
        for line in view_lines {
            if line.is_primary_active {
                return Some(line);
            }
            if fallback.is_none() && line.is_active_change {
                fallback = Some(line);
            }
        }
        fallback
    }

    fn blame_cache_key_for_line(&self, view_line: &ViewLine) -> Option<BlameCacheKey> {
        let (old_source, new_source) = self.multi_diff.blame_sources()?;
        let file = self.multi_diff.current_file()?;
        let old_path = file.old_path.as_ref().unwrap_or(&file.path);
        if view_line.new_line.is_none() {
            if let Some(old_line) = view_line.old_line {
                return Some(BlameCacheKey {
                    path: old_path.to_path_buf(),
                    line: old_line,
                    source: old_source,
                });
            }
        }
        let (line, path, source) = match view_line.kind {
            LineKind::Deleted | LineKind::PendingDelete => {
                (view_line.old_line?, old_path, old_source)
            }
            LineKind::Inserted | LineKind::PendingInsert => {
                (view_line.new_line?, &file.path, new_source)
            }
            LineKind::Modified | LineKind::PendingModify => (
                view_line.new_line.or(view_line.old_line)?,
                &file.path,
                new_source,
            ),
            LineKind::Context => {
                if view_line.has_changes {
                    if let Some(old_line) = view_line.old_line {
                        (old_line, old_path, old_source)
                    } else {
                        (view_line.new_line?, &file.path, new_source)
                    }
                } else {
                    (
                        view_line.new_line.or(view_line.old_line)?,
                        &file.path,
                        new_source,
                    )
                }
            }
        };
        Some(BlameCacheKey {
            path: path.to_path_buf(),
            line,
            source,
        })
    }

    fn blame_display_from_info(&mut self, info: &BlameInfo, now: i64) -> super::BlameDisplay {
        let group_key = if info.uncommitted {
            "uncommitted".to_string()
        } else {
            info.commit.clone()
        };
        let text = if info.uncommitted {
            "Uncommitted".to_string()
        } else {
            self.format_blame_github_info(info, now)
        };
        super::BlameDisplay {
            group_key,
            text,
            author_time: info.author_time,
            uncommitted: info.uncommitted,
        }
    }

    fn blame_uncommitted_display(&self) -> super::BlameDisplay {
        super::BlameDisplay {
            group_key: "uncommitted".to_string(),
            text: "Uncommitted".to_string(),
            author_time: None,
            uncommitted: true,
        }
    }

    fn should_force_uncommitted_blame(&self, view_line: &ViewLine) -> bool {
        let (_, new_source) = match self.multi_diff.blame_sources() {
            Some(sources) => sources,
            None => return false,
        };
        if !matches!(new_source, BlameSource::Worktree | BlameSource::Index) {
            return false;
        }
        matches!(
            view_line.kind,
            LineKind::Inserted
                | LineKind::PendingInsert
                | LineKind::Deleted
                | LineKind::PendingDelete
                | LineKind::Modified
                | LineKind::PendingModify
        )
    }

    fn update_blame_time_range(&mut self, key: &BlameCacheKey, info: &BlameInfo) {
        let Some(ts) = info.author_time else {
            return;
        };
        let range_key = BlamePrefetchKey {
            path: key.path.clone(),
            source: key.source.clone(),
        };
        let entry = self.blame_time_ranges.entry(range_key).or_insert((ts, ts));
        if ts < entry.0 {
            entry.0 = ts;
        }
        if ts > entry.1 {
            entry.1 = ts;
        }
    }

    fn blame_info_for_line(&mut self, view_line: &ViewLine, allow_sync: bool) -> Option<BlameInfo> {
        let repo_root = self.multi_diff.repo_root()?;
        let key = self.blame_cache_key_for_line(view_line)?;
        let BlameCacheKey { path, line, source } = key.clone();
        let info = if let Some(info) = self.blame_cache.get(&key) {
            info.clone()
        } else {
            if !allow_sync {
                return None;
            }
            let info = blame_line(repo_root, &path, line, &source)?;
            self.update_blame_time_range(&key, &info);
            self.blame_cache.insert(key, info.clone());
            info
        };
        Some(info)
    }

    fn set_blame_step_hint(&mut self) {
        if let Some(line) = self.active_view_line() {
            if let Some(text) = self.blame_text_for_line(&line) {
                self.blame_step_hint = Some(BlameStepHint {
                    change_id: line.change_id,
                    text,
                });
            }
        }
    }

    pub(super) fn set_blame_hunk_hint(&mut self) {
        if !self.blame_hunk_hint_enabled {
            return;
        }
        if let Some(line) = self.active_view_line() {
            if let Some(text) = self.blame_text_for_line(&line) {
                self.blame_hunk_hint = Some(text);
            }
        }
    }

    fn blame_text_for_line(&mut self, view_line: &ViewLine) -> Option<String> {
        if !self.blame_enabled {
            return None;
        }
        if self.should_force_uncommitted_blame(view_line) {
            return Some("Uncommitted".to_string());
        }
        let info = self.blame_info_for_line(view_line, true)?;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        Some(self.format_blame_hint_info(&info, now))
    }

    pub(super) fn refresh_blame_toggle_hint(&mut self) {
        if !self.blame_enabled {
            return;
        }
        if matches!(self.blame_mode, BlameMode::Toggle) && self.blame_toggle {
            self.clear_blame_step_hint();
            self.set_blame_step_hint();
        }
    }

    pub(crate) fn blame_step_hint_for_change(&self, change_id: usize) -> Option<&str> {
        if !self.blame_enabled {
            return None;
        }
        let hint = self.blame_step_hint.as_ref()?;
        if hint.change_id == change_id {
            Some(hint.text.as_str())
        } else {
            None
        }
    }

    pub(crate) fn blame_hunk_hint_text(&self) -> Option<&str> {
        if !self.blame_enabled || !self.blame_hunk_hint_enabled {
            return None;
        }
        self.blame_hunk_hint.as_deref()
    }

    pub(crate) fn blame_display_for_view_line(
        &mut self,
        view_line: &ViewLine,
        now: i64,
    ) -> Option<super::BlameDisplay> {
        if !self.blame_enabled {
            return None;
        }
        if self.should_force_uncommitted_blame(view_line) {
            let display = self.blame_uncommitted_display();
            if let Some(key) = self.blame_cache_key_for_line(view_line) {
                self.blame_display_cache.insert(key, display.clone());
            }
            return Some(display);
        }
        let key = self.blame_cache_key_for_line(view_line)?;
        if let Some(info) = self.blame_info_for_line(view_line, false) {
            let display = self.blame_display_from_info(&info, now);
            self.update_blame_time_range(&key, &info);
            self.blame_display_cache.insert(key, display.clone());
            return Some(display);
        }
        self.blame_display_cache.get(&key).cloned()
    }

    pub(crate) fn blame_bar_color_for_view_line(
        &mut self,
        view_line: &ViewLine,
        display: Option<&super::BlameDisplay>,
    ) -> Option<Color> {
        if !self.blame_enabled {
            return None;
        }
        let key = self.blame_cache_key_for_line(view_line)?;
        let range_key = BlamePrefetchKey {
            path: key.path.clone(),
            source: key.source.clone(),
        };
        let range = self.blame_time_ranges.get(&range_key).copied();
        let computed = display.and_then(|display| {
            if display.uncommitted {
                return Some(color::ramp_color(self.theme.warning, 1.0));
            }
            let time = display.author_time?;
            let t = if let Some((min, max)) = range {
                if max > min {
                    (time - min) as f32 / (max - min) as f32
                } else {
                    1.0
                }
            } else {
                1.0
            };
            Some(color::ramp_color(self.theme.warning, t))
        });
        if let Some(color) = computed {
            self.blame_bar_cache.insert(key, color);
            return Some(color);
        }
        self.blame_bar_cache.get(&key).copied()
    }

    pub(crate) fn format_blame_github_info(&mut self, info: &BlameInfo, now: i64) -> String {
        self.ensure_blame_user_name();
        let time_text = self.time_format.format(info.author_time, now);
        format_blame_github_text(info, self.blame_user_name.as_deref(), &time_text)
    }

    fn format_blame_hint_info(&mut self, info: &BlameInfo, now: i64) -> String {
        self.ensure_blame_user_name();
        let time_text = self.time_format.format(info.author_time, now);
        format_blame_hint_text(info, self.blame_user_name.as_deref(), &time_text, 60)
    }

    fn ensure_blame_worker(&mut self) {
        if self.blame_worker_tx.is_some() {
            return;
        }
        let (req_tx, req_rx) = mpsc::channel::<BlameRequest>();
        let (resp_tx, resp_rx) = mpsc::channel::<BlameResponse>();
        thread::spawn(move || {
            while let Ok(req) = req_rx.recv() {
                let entries =
                    blame_range(&req.repo_root, &req.path, req.start, req.end, &req.source)
                        .unwrap_or_default();
                let _ = resp_tx.send(BlameResponse {
                    path: req.path,
                    source: req.source,
                    start: req.start,
                    end: req.end,
                    entries,
                });
            }
        });
        self.blame_worker_tx = Some(req_tx);
        self.blame_worker_rx = Some(resp_rx);
    }

    pub(crate) fn poll_blame_responses(&mut self) {
        let Some(rx) = self.blame_worker_rx.as_ref() else {
            return;
        };
        let mut updated = false;
        while let Ok(resp) = rx.try_recv() {
            let range_key = BlamePrefetchKey {
                path: resp.path.clone(),
                source: resp.source.clone(),
            };
            for (line_num, info) in resp.entries {
                let key = BlameCacheKey {
                    path: resp.path.clone(),
                    line: line_num,
                    source: resp.source.clone(),
                };
                if let Some(ts) = info.author_time {
                    let entry = self
                        .blame_time_ranges
                        .entry(range_key.clone())
                        .or_insert((ts, ts));
                    if ts < entry.0 {
                        entry.0 = ts;
                    }
                    if ts > entry.1 {
                        entry.1 = ts;
                    }
                }
                self.blame_cache.insert(key, info);
                updated = true;
            }
            self.blame_prefetch.insert(
                range_key.clone(),
                BlamePrefetchRange {
                    start: resp.start,
                    end: resp.end,
                },
            );
            self.blame_pending.remove(&range_key);
        }
        if updated {
            self.blame_cache_revision = self.blame_cache_revision.wrapping_add(1);
            self.blame_render_cache = None;
        }
    }

    fn queue_blame_range(
        &mut self,
        repo_root: &std::path::Path,
        path: &std::path::Path,
        source: &BlameSource,
        start: usize,
        end: usize,
    ) {
        self.ensure_blame_worker();
        let key = BlamePrefetchKey {
            path: path.to_path_buf(),
            source: source.clone(),
        };
        if let Some(range) = self.blame_prefetch.get(&key) {
            if start >= range.start && end <= range.end {
                return;
            }
        }
        if let Some(range) = self.blame_pending.get(&key) {
            if start >= range.start && end <= range.end {
                return;
            }
        }
        if let Some(tx) = self.blame_worker_tx.as_ref() {
            let _ = tx.send(BlameRequest {
                repo_root: repo_root.to_path_buf(),
                path: path.to_path_buf(),
                source: source.clone(),
                start,
                end,
            });
            self.blame_pending
                .insert(key, BlamePrefetchRange { start, end });
        }
    }

    pub(crate) fn prefetch_blame_for_view(
        &mut self,
        view_lines: &[ViewLine],
        visible_indices: &[usize],
        visible_height: usize,
    ) {
        if !self.blame_enabled || visible_indices.is_empty() {
            return;
        }
        if self.animation_phase != super::AnimationPhase::Idle {
            return;
        }
        if let Some(last) = self.blame_prefetch_at {
            if last.elapsed() < Duration::from_millis(80) {
                return;
            }
        }
        let repo_root = match self.multi_diff.repo_root() {
            Some(root) => root.to_path_buf(),
            None => return,
        };
        let (old_source, new_source) = match self.multi_diff.blame_sources() {
            Some(sources) => sources,
            None => return,
        };
        let (file_path, old_path_buf) = match self.multi_diff.current_file() {
            Some(file) => (
                file.path.clone(),
                file.old_path.as_ref().unwrap_or(&file.path).to_path_buf(),
            ),
            None => return,
        };

        let mut missing_old: Vec<usize> = Vec::new();
        let mut missing_new: Vec<usize> = Vec::new();
        let mut first_missing_old: Option<usize> = None;
        let mut first_missing_new: Option<usize> = None;

        for idx in visible_indices {
            let line = &view_lines[*idx];
            match line.kind {
                LineKind::Deleted | LineKind::PendingDelete => {
                    if let Some(old_line) = line.old_line {
                        let key = BlameCacheKey {
                            path: old_path_buf.clone(),
                            line: old_line,
                            source: old_source.clone(),
                        };
                        if !self.blame_cache.contains_key(&key) {
                            missing_old.push(old_line);
                            if first_missing_old.is_none() {
                                first_missing_old = Some(old_line);
                            }
                        }
                    }
                }
                LineKind::Inserted | LineKind::PendingInsert => {
                    if let Some(new_line) = line.new_line {
                        let key = BlameCacheKey {
                            path: file_path.clone(),
                            line: new_line,
                            source: new_source.clone(),
                        };
                        if !self.blame_cache.contains_key(&key) {
                            missing_new.push(new_line);
                            if first_missing_new.is_none() {
                                first_missing_new = Some(new_line);
                            }
                        }
                    }
                }
                LineKind::Modified | LineKind::PendingModify | LineKind::Context => {
                    if line.new_line.is_none() {
                        if let Some(old_line) = line.old_line {
                            let key = BlameCacheKey {
                                path: old_path_buf.clone(),
                                line: old_line,
                                source: old_source.clone(),
                            };
                            if !self.blame_cache.contains_key(&key) {
                                missing_old.push(old_line);
                                if first_missing_old.is_none() {
                                    first_missing_old = Some(old_line);
                                }
                            }
                        }
                    } else if let Some(line_num) = line.new_line.or(line.old_line) {
                        let key = BlameCacheKey {
                            path: file_path.clone(),
                            line: line_num,
                            source: new_source.clone(),
                        };
                        if !self.blame_cache.contains_key(&key) {
                            missing_new.push(line_num);
                            if first_missing_new.is_none() {
                                first_missing_new = Some(line_num);
                            }
                        }
                    }
                }
            }
        }

        let missing_total = missing_old.len() + missing_new.len();
        if missing_total == 0 {
            return;
        }

        let mut margin = visible_height.saturating_mul(2).max(20);
        let mut max_range = visible_height.saturating_mul(6).max(120);
        let allow_full_fetch = matches!(self.view_mode, super::ViewMode::Blame);
        let large_missing = !allow_full_fetch && missing_total > visible_height.saturating_mul(2);

        if large_missing {
            margin = visible_height.saturating_sub(1).max(4);
            max_range = visible_height.saturating_mul(2).max(40);
            let mut focus_idx: Option<usize> = None;
            for idx in visible_indices {
                if view_lines[*idx].is_primary_active {
                    focus_idx = Some(*idx);
                    break;
                }
            }
            if focus_idx.is_none() {
                for idx in visible_indices {
                    if view_lines[*idx].is_active_change {
                        focus_idx = Some(*idx);
                        break;
                    }
                }
            }
            if let Some(idx) = focus_idx {
                let line = &view_lines[idx];
                let focus_old = line.old_line;
                let focus_new = line.new_line.or(line.old_line);
                if let Some(focus_old) = focus_old {
                    missing_old.retain(|line| *line == focus_old);
                } else {
                    missing_old.clear();
                }
                if let Some(focus_new) = focus_new {
                    missing_new.retain(|line| *line == focus_new);
                } else {
                    missing_new.clear();
                }
            }
            if missing_old.is_empty() && missing_new.is_empty() {
                if let Some(line) = first_missing_new {
                    missing_new.push(line);
                } else if let Some(line) = first_missing_old {
                    missing_old.push(line);
                }
            }
        }
        let mut did_prefetch = false;
        let mut schedule_range = |lines: &Vec<usize>, path: &PathBuf, source: &BlameSource| {
            if lines.is_empty() {
                return;
            }
            let min_line = *lines.iter().min().unwrap();
            let max_line_found = *lines.iter().max().unwrap();
            let start = min_line.saturating_sub(margin).max(1);
            let end = max_line_found.saturating_add(margin);
            let mut fetch_start = start;
            let mut fetch_end = end;
            let key = BlamePrefetchKey {
                path: path.clone(),
                source: source.clone(),
            };
            if let Some(prev) = self.blame_prefetch.get(&key) {
                if start >= prev.start && end <= prev.end {
                    return;
                }
                let union_start = start.min(prev.start);
                let union_end = end.max(prev.end);
                if union_end.saturating_sub(union_start) <= max_range {
                    fetch_start = union_start;
                    fetch_end = union_end;
                }
            }
            self.queue_blame_range(&repo_root, path, source, fetch_start, fetch_end);
            self.blame_prefetch.insert(
                BlamePrefetchKey {
                    path: path.clone(),
                    source: source.clone(),
                },
                BlamePrefetchRange {
                    start: fetch_start,
                    end: fetch_end,
                },
            );
            did_prefetch = true;
        };

        schedule_range(&missing_old, &old_path_buf, &old_source);
        schedule_range(&missing_new, &file_path, &new_source);

        if did_prefetch {
            self.blame_prefetch_at = Some(Instant::now());
        }
    }

    pub fn trigger_blame_hint(&mut self) {
        if !self.blame_enabled {
            return;
        }
        self.clear_blame_hunk_hint();
        match self.blame_mode {
            BlameMode::OneShot => {
                self.clear_blame_step_hint();
                self.set_blame_step_hint();
            }
            BlameMode::Toggle => {
                self.blame_toggle = !self.blame_toggle;
                self.clear_blame_step_hint();
                if self.blame_toggle {
                    self.set_blame_step_hint();
                }
            }
        }
    }
}
