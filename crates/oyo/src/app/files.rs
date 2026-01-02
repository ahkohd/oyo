use super::{AnimationPhase, App, ViewMode};

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
        if self.file_filter.is_empty() {
            return (0..self.multi_diff.files.len()).collect();
        }
        let query = self.file_filter.to_ascii_lowercase();
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

    /// Refresh current file from disk
    pub fn refresh_current_file(&mut self) {
        self.multi_diff.refresh_current_file();
        let idx = self.multi_diff.selected_index;
        if idx < self.syntax_caches.len() {
            self.syntax_caches[idx] = None;
        }
        self.scroll_offset = 0;
        self.horizontal_scroll = 0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
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
        }
    }

    /// Get the total number of lines in the current view
    #[allow(dead_code)]
    pub fn total_lines(&mut self) -> usize {
        let frame = self.animation_frame();
        self.multi_diff
            .current_navigator()
            .current_view_with_frame(frame)
            .len()
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
