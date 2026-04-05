use super::{AnimationPhase, App, FileDiskStamp, ViewMode};
use std::time::{Duration, Instant};

impl App {
    // File navigation methods
    pub fn next_file(&mut self) {
        if !self.file_filter.is_empty() {
            let indices = self.filtered_file_indices();
            if indices.is_empty() {
                return;
            }
            let current = self.multi_diff.selected_index;
            let pos = indices.iter().position(|&i| i == current);
            let next_index = match pos {
                Some(p) if p + 1 < indices.len() => indices[p + 1],
                None => indices[0],
                _ => return,
            };
            self.select_file(next_index);
            return;
        }

        let current = self.multi_diff.selected_index;
        let next_index = current.saturating_add(1);
        if next_index < self.multi_diff.file_count() {
            self.select_file(next_index);
        }
    }

    pub fn prev_file(&mut self) {
        if !self.file_filter.is_empty() {
            let indices = self.filtered_file_indices();
            if indices.is_empty() {
                return;
            }
            let current = self.multi_diff.selected_index;
            let pos = indices.iter().position(|&i| i == current);
            let prev_index = match pos {
                Some(p) if p > 0 => indices[p - 1],
                None => indices[indices.len().saturating_sub(1)],
                _ => return,
            };
            self.select_file(prev_index);
            return;
        }

        let current = self.multi_diff.selected_index;
        if current > 0 {
            self.select_file(current - 1);
        }
    }

    pub(super) fn next_file_wrapped(&mut self) -> bool {
        if !self.file_filter.is_empty() {
            let indices = self.filtered_file_indices();
            if indices.is_empty() {
                return false;
            }
            let current = self.multi_diff.selected_index;
            let pos = indices.iter().position(|&i| i == current).unwrap_or(0);
            let next_index = if pos + 1 < indices.len() {
                indices[pos + 1]
            } else {
                indices[0]
            };
            if next_index == current {
                return false;
            }
            self.select_file(next_index);
            return true;
        }

        let count = self.multi_diff.file_count();
        if count == 0 {
            return false;
        }
        let current = self.multi_diff.selected_index;
        let next_index = if current + 1 < count { current + 1 } else { 0 };
        if next_index == current {
            return false;
        }
        self.select_file(next_index);
        true
    }

    pub(super) fn prev_file_wrapped(&mut self) -> bool {
        if !self.file_filter.is_empty() {
            let indices = self.filtered_file_indices();
            if indices.is_empty() {
                return false;
            }
            let current = self.multi_diff.selected_index;
            let pos = indices.iter().position(|&i| i == current).unwrap_or(0);
            let prev_index = if pos > 0 {
                indices[pos - 1]
            } else {
                indices[indices.len().saturating_sub(1)]
            };
            if prev_index == current {
                return false;
            }
            self.select_file(prev_index);
            return true;
        }

        let count = self.multi_diff.file_count();
        if count == 0 {
            return false;
        }
        let current = self.multi_diff.selected_index;
        if current == 0 {
            self.select_file(count - 1);
            return count > 1;
        }
        self.select_file(current - 1);
        true
    }

    pub fn select_file(&mut self, index: usize) {
        let old_index = self.multi_diff.selected_index;
        self.clear_step_edge_hint();
        self.clear_hunk_edge_hint();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        if !self.stepping {
            self.save_no_step_state_snapshot(old_index);
        }
        self.save_scroll_position_for(old_index);
        self.multi_diff.select_file(index);
        self.restore_scroll_position_for(self.multi_diff.selected_index);
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.view_build_defer = false;
        self.view_build_pending = false;
        self.reset_search_for_file_switch();
        self.centered_once = false;
        self.update_file_list_scroll();
        self.handle_file_enter();
    }

    pub fn start_file_filter(&mut self) {
        self.file_filter_active = true;
        self.file_filter.clear();
        self.file_list_scroll = 0;
        self.ensure_selection_matches_filter();
        self.update_file_list_scroll();
    }

    pub fn stop_file_filter(&mut self) {
        self.file_filter_active = false;
    }

    pub fn push_file_filter_char(&mut self, ch: char) {
        self.file_filter.push(ch);
        self.on_filter_changed();
    }

    pub fn pop_file_filter_char(&mut self) {
        self.file_filter.pop();
        self.on_filter_changed();
    }

    pub fn clear_file_filter(&mut self) {
        self.file_filter.clear();
        self.on_filter_changed();
    }

    /// Check if current file would be blank at step 0 (new file: empty old, non-empty new)
    fn is_blank_at_step0(&self) -> bool {
        self.multi_diff.current_old_is_empty() && !self.multi_diff.current_new_is_empty()
    }

    /// Handle entering a file (marks visited, optionally auto-steps to first change)
    /// Called on initial file and when switching files.
    pub fn handle_file_enter(&mut self) {
        self.queue_current_file_diff();
        if self.stepping && !self.current_file_diff_ready() {
            return;
        }
        self.finish_file_enter();
    }

    pub(crate) fn finish_file_enter(&mut self) {
        let idx = self.multi_diff.selected_index;

        if !self.stepping {
            if !self.files_visited[idx] {
                self.files_visited[idx] = true;
            }
            // If in no-step mode, ensure full content is shown immediately
            self.ensure_step_state_snapshot(idx);
            self.multi_diff.current_navigator().goto_end();
            self.multi_diff.current_navigator().clear_active_change();
            self.animation_phase = AnimationPhase::Idle;
            self.animation_progress = 1.0;
            if !self.restore_no_step_state_snapshot(idx) {
                if self.no_step_auto_jump_on_enter && !self.no_step_visited[idx] {
                    self.goto_hunk_index_scroll(0);
                } else {
                    self.set_cursor_for_current_scroll();
                    self.multi_diff.current_navigator().set_hunk_scope(false);
                }
            }
            self.no_step_visited[idx] = true;
            // Don't mess with scroll_offset here; it might have been restored by next_file/prev_file
            return;
        }

        // Only process on first visit to this file
        if self.files_visited[idx] {
            return;
        }

        let is_large = self.multi_diff.file_is_large(idx);
        if is_large {
            self.files_visited[idx] = true;
            return;
        }

        // Mark as visited
        self.files_visited[idx] = true;

        let state = self.multi_diff.current_navigator().state();
        let at_step_0 = state.current_step == 0;
        let has_steps = state.total_steps > 1;
        if !at_step_0 || !has_steps {
            return;
        }

        // Auto-step for blank files (new files) regardless of view mode
        if self.auto_step_blank_files && self.is_blank_at_step0() {
            self.next_step();
            return;
        }

        // Regular auto-step on enter (not for Evolution mode)
        if self.auto_step_on_enter && self.view_mode != ViewMode::Evolution {
            self.next_step();
        }
    }

    pub fn is_multi_file(&self) -> bool {
        self.multi_diff.is_multi_file()
    }

    fn update_file_list_scroll(&mut self) {
        let indices = self.filtered_file_indices();
        if indices.is_empty() {
            self.file_list_scroll = 0;
            return;
        }

        // Keep selected file visible in the file list
        let selected = self.multi_diff.selected_index;
        let selected_pos = indices.iter().position(|&i| i == selected).unwrap_or(0);
        if selected_pos < self.file_list_scroll {
            self.file_list_scroll = selected_pos;
        }
        // Assume roughly 20 visible files
        let visible_files = 20;
        if selected_pos >= self.file_list_scroll + visible_files {
            self.file_list_scroll = selected_pos.saturating_sub(visible_files - 1);
        }
    }

    fn on_filter_changed(&mut self) {
        self.file_list_scroll = 0;
        self.ensure_selection_matches_filter();
        self.update_file_list_scroll();
    }

    fn ensure_selection_matches_filter(&mut self) {
        if self.file_filter.is_empty() {
            return;
        }
        let indices = self.filtered_file_indices();
        if indices.is_empty() {
            return;
        }
        if !indices.contains(&self.multi_diff.selected_index) {
            self.select_file(indices[0]);
        }
    }

    pub fn filtered_file_indices(&self) -> Vec<usize> {
        self.file_indices_for_query(&self.file_filter)
    }

    pub(super) fn file_indices_for_query(&self, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return (0..self.multi_diff.files.len()).collect();
        }
        let query = query.to_ascii_lowercase();
        self.multi_diff
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.display_name.to_ascii_lowercase().contains(&query))
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Get current file path for display
    pub fn current_file_path(&self) -> String {
        self.multi_diff
            .current_file()
            .map(|f| f.display_name.clone())
            .unwrap_or_default()
    }

    fn disk_stamp_for_index(&self, idx: usize) -> FileDiskStamp {
        let Some(file) = self.multi_diff.files.get(idx) else {
            return FileDiskStamp::default();
        };

        let full_path = if let Some(repo_root) = self.multi_diff.repo_root() {
            repo_root.join(&file.path)
        } else {
            file.path.clone()
        };

        match std::fs::metadata(&full_path) {
            Ok(meta) => FileDiskStamp {
                modified: meta.modified().ok(),
                len: meta.len(),
                exists: true,
            },
            Err(_) => FileDiskStamp::default(),
        }
    }

    pub(crate) fn rebuild_file_disk_baseline(&mut self) {
        let file_count = self.multi_diff.file_count();
        self.file_disk_baseline = (0..file_count)
            .map(|idx| self.disk_stamp_for_index(idx))
            .collect();
        self.file_disk_changed = vec![false; file_count];
    }

    fn refresh_file_disk_baseline_for(&mut self, idx: usize) {
        if self.file_disk_baseline.len() != self.multi_diff.file_count() {
            self.rebuild_file_disk_baseline();
            return;
        }
        let stamp = self.disk_stamp_for_index(idx);
        if let Some(slot) = self.file_disk_baseline.get_mut(idx) {
            *slot = stamp;
        }
    }

    fn recompute_file_change_state(&mut self) {
        let file_count = self.multi_diff.file_count();
        if self.file_disk_baseline.len() != file_count {
            self.rebuild_file_disk_baseline();
        }
        if self.file_disk_changed.len() != file_count {
            self.file_disk_changed = vec![false; file_count];
        }

        let mut any_changed = false;
        for idx in 0..file_count {
            let changed = self.disk_stamp_for_index(idx) != self.file_disk_baseline[idx];
            if let Some(slot) = self.file_disk_changed.get_mut(idx) {
                *slot = changed;
            }
            any_changed |= changed;
        }
        self.files_changed_on_disk = any_changed;
    }

    pub(crate) fn file_changed_on_disk(&self, idx: usize) -> bool {
        self.file_disk_changed.get(idx).copied().unwrap_or(false)
    }

    /// Check if tracked files changed on disk since the last refresh baseline.
    pub fn maybe_check_file_changes(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_fs_check) < Duration::from_secs(1) {
            return;
        }
        self.last_fs_check = now;
        self.recompute_file_change_state();
    }

    /// Refresh current file from disk
    pub fn refresh_current_file(&mut self) {
        // Preserve no-step hunk scope/cursor context when possible.
        let preserve_no_step_hunk = if !self.stepping {
            let nav = self.multi_diff.current_navigator();
            let state = nav.state();
            if state.last_nav_was_hunk {
                let cursor_rank = nav
                    .diff()
                    .hunks
                    .get(state.current_hunk)
                    .and_then(|hunk| {
                        state
                            .cursor_change
                            .and_then(|cursor| hunk.change_ids.iter().position(|id| *id == cursor))
                    })
                    .unwrap_or(0);
                Some((state.current_hunk, cursor_rank))
            } else {
                None
            }
        } else {
            None
        };

        self.multi_diff.refresh_current_file();

        // The navigator is rebuilt at step 0 after refresh; jump to the end
        // so all changes remain visible.
        {
            let nav = self.multi_diff.current_navigator();
            nav.goto_end();
            if !self.stepping {
                // Keep no-step state semantics after refresh.
                nav.clear_active_change();
            }
        }

        if !self.stepping {
            let restored_hunk_scope = if let Some((prev_hunk, prev_cursor_rank)) =
                preserve_no_step_hunk
            {
                let nav = self.multi_diff.current_navigator();
                let total_hunks = nav.state().total_hunks;
                if total_hunks > 0 {
                    let hunk_idx = prev_hunk.min(total_hunks.saturating_sub(1));
                    let cursor_change = nav.diff().hunks.get(hunk_idx).and_then(|hunk| {
                        if hunk.change_ids.is_empty() {
                            None
                        } else {
                            let idx = prev_cursor_rank.min(hunk.change_ids.len().saturating_sub(1));
                            hunk.change_ids.get(idx).copied()
                        }
                    });
                    nav.set_cursor_hunk(hunk_idx, cursor_change);
                    nav.set_hunk_scope(true);
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if !restored_hunk_scope {
                self.set_cursor_for_current_scroll();
                self.multi_diff.current_navigator().set_hunk_scope(false);
            }
        }

        let idx = self.multi_diff.selected_index;
        if idx < self.syntax_caches.len() {
            self.syntax_caches[idx] = None;
        }
        self.ensure_syntax_cache();

        self.refresh_file_disk_baseline_for(idx);
        self.recompute_file_change_state();
    }

    /// Refresh all files from git (re-scan for uncommitted changes)
    pub fn refresh_all_files(&mut self) {
        if self.multi_diff.refresh_all_from_git() {
            // Reset scroll states for all files
            let file_count = self.multi_diff.file_count();
            self.scroll_offsets_step = vec![0; file_count];
            self.scroll_offsets_no_step = vec![0; file_count];
            self.horizontal_scrolls_step = vec![0; file_count];
            self.horizontal_scrolls_no_step = vec![0; file_count];
            self.max_line_widths_step = vec![0; file_count];
            self.max_line_widths_no_step = vec![0; file_count];
            self.no_step_visited = vec![false; file_count];
            self.files_visited = vec![false; file_count];
            self.syntax_caches = vec![None; file_count];
            self.step_state_snapshots = vec![None; file_count];
            self.no_step_state_snapshots = vec![None; file_count];
            self.scroll_offset = 0;
            self.horizontal_scroll = 0;
            self.needs_scroll_to_active = true;
            self.centered_once = false;
            self.handle_file_enter();

            self.rebuild_file_disk_baseline();
            self.files_changed_on_disk = false;
        }
    }

    /// Get the total number of lines in the current view
    #[allow(dead_code)]
    pub fn total_lines(&mut self) -> usize {
        let frame = self.animation_frame();
        self.current_view_with_frame(frame).len()
    }

    /// Get statistics about the current file's diff
    pub fn stats(&mut self) -> (usize, usize) {
        if self.current_file_is_binary() {
            return (0, 0);
        }
        let diff = self.multi_diff.current_navigator().diff();
        (diff.insertions, diff.deletions)
    }

    pub fn current_file_is_binary(&self) -> bool {
        self.multi_diff.current_file_is_binary()
    }
}
