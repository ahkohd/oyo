//! Application state and logic

use oyo_core::{AnimationFrame, MultiFileDiff, StepDirection, StepState};
use std::time::{Duration, Instant};

/// Animation phase for smooth transitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationPhase {
    /// No animation happening
    Idle,
    /// Fading out the old content
    FadeOut,
    /// Fading in the new content
    FadeIn,
}

/// View mode for displaying diffs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Single pane showing both old and new with markers
    #[default]
    SinglePane,
    /// Side-by-side view with old on left, new on right
    SideBySide,
    /// Evolution view - shows file morphing, deletions just disappear
    Evolution,
}

impl ViewMode {
    /// Cycle to the next view mode
    pub fn next(self) -> Self {
        match self {
            ViewMode::SinglePane => ViewMode::SideBySide,
            ViewMode::SideBySide => ViewMode::Evolution,
            ViewMode::Evolution => ViewMode::SinglePane,
        }
    }

}

/// The main application state
pub struct App {
    /// Multi-file diff manager
    pub multi_diff: MultiFileDiff,
    /// Current view mode
    pub view_mode: ViewMode,
    /// Animation speed in milliseconds
    pub animation_speed: u64,
    /// Whether autoplay is enabled
    pub autoplay: bool,
    /// Current scroll offset
    pub scroll_offset: usize,
    /// Per-file scroll offsets (to restore when switching files)
    scroll_offsets: Vec<usize>,
    /// Tracks which files have been visited (for auto-step on first visit)
    files_visited: Vec<bool>,
    /// Whether to quit
    pub should_quit: bool,
    /// Current animation phase
    pub animation_phase: AnimationPhase,
    /// Animation progress (0.0 to 1.0)
    pub animation_progress: f32,
    /// Last animation tick time
    last_animation_tick: Instant,
    /// Last autoplay tick time
    last_autoplay_tick: Instant,
    /// Whether the file list is focused (for multi-file mode)
    pub file_list_focused: bool,
    /// Whether the file panel is visible (for multi-file mode)
    pub file_panel_visible: bool,
    /// File list scroll offset
    pub file_list_scroll: usize,
    /// Whether animations are enabled (false = instant transitions)
    pub animation_enabled: bool,
    /// Zen mode - hide UI chrome (top bar, progress bar, help bar)
    pub zen_mode: bool,
    /// Flag to scroll to active change on next render (after stepping)
    pub needs_scroll_to_active: bool,
    /// Whether to show the help popover
    pub show_help: bool,
    /// Git branch name (if in a git repo)
    pub git_branch: Option<String>,
    /// Auto-center on active change after stepping (like vim's zz)
    pub auto_center: bool,
    /// Animation duration in milliseconds (how long fade effects take)
    pub animation_duration: u64,
    /// Pending count for vim-style commands (e.g., 10j = scroll down 10 lines)
    pub pending_count: Option<usize>,
    /// Horizontal scroll offset (for long lines)
    pub horizontal_scroll: usize,
    /// Per-file horizontal scroll offsets
    horizontal_scrolls: Vec<usize>,
    /// Line wrap mode (when true, horizontal scroll is ignored)
    pub line_wrap: bool,
    /// Show scrollbar
    pub scrollbar_visible: bool,
    /// Whether user has manually toggled the file panel (overrides auto-hide)
    pub file_panel_manually_set: bool,
    /// Whether to show the file path popup (Ctrl+G)
    pub show_path_popup: bool,
    /// Whether the file panel is currently auto-hidden due to narrow viewport
    pub file_panel_auto_hidden: bool,
    /// Auto-step to first change when entering a file at step 0
    pub auto_step_on_enter: bool,
    /// Manual center was requested (zz), enables overscroll until manual scroll
    pub centered_once: bool,
    /// Marker for primary active line (left pane / single pane)
    pub primary_marker: String,
    /// Marker for right pane primary line
    pub primary_marker_right: String,
    /// Marker for hunk extent lines (left pane / single pane)
    pub extent_marker: String,
    /// Marker for right pane extent lines
    pub extent_marker_right: String,
    /// Clear active change after next render (for one-frame extent marker display)
    pub clear_active_on_next_render: bool,
}

/// Pure helper: determine if overscroll should be allowed
fn allow_overscroll_state(auto_center: bool, needs_scroll_to_active: bool, centered_once: bool) -> bool {
    (auto_center && needs_scroll_to_active) || centered_once
}

/// Pure helper: compute max scroll offset
fn max_scroll(total_lines: usize, viewport_height: usize, allow_overscroll: bool) -> usize {
    if allow_overscroll {
        // Allow last line to be centered: enough space for half viewport below
        total_lines.saturating_sub(1).saturating_sub(viewport_height / 2)
    } else {
        total_lines.saturating_sub(viewport_height)
    }
}

impl App {
    pub fn new(
        multi_diff: MultiFileDiff,
        view_mode: ViewMode,
        animation_speed: u64,
        autoplay: bool,
        git_branch: Option<String>,
    ) -> Self {
        let file_count = multi_diff.file_count();
        Self {
            multi_diff,
            view_mode,
            animation_speed,
            autoplay,
            scroll_offset: 0,
            scroll_offsets: vec![0; file_count],
            files_visited: vec![false; file_count],
            should_quit: false,
            animation_phase: AnimationPhase::Idle,
            animation_progress: 1.0,
            last_animation_tick: Instant::now(),
            last_autoplay_tick: Instant::now(),
            file_list_focused: false,
            file_panel_visible: true,
            file_list_scroll: 0,
            animation_enabled: false,
            zen_mode: false,
            needs_scroll_to_active: true, // Scroll to first change on startup
            show_help: false,
            git_branch,
            auto_center: true,
            animation_duration: 150,
            pending_count: None,
            horizontal_scroll: 0,
            horizontal_scrolls: vec![0; file_count],
            line_wrap: false,
            scrollbar_visible: false,
            file_panel_manually_set: false,
            show_path_popup: false,
            file_panel_auto_hidden: false,
            auto_step_on_enter: false,
            centered_once: false,
            primary_marker: "▶".to_string(),
            primary_marker_right: "◀".to_string(),
            extent_marker: "▌".to_string(),
            extent_marker_right: "▐".to_string(),
            clear_active_on_next_render: false,
        }
    }

    /// Add a digit to the pending count (vim-style command counts)
    pub fn push_count_digit(&mut self, digit: u8) {
        let current = self.pending_count.unwrap_or(0);
        // Prevent overflow, cap at reasonable max
        let new_count = current.saturating_mul(10).saturating_add(digit as usize);
        self.pending_count = Some(new_count.min(9999));
    }

    /// Get the pending count (defaults to 1) and reset it
    pub fn take_count(&mut self) -> usize {
        self.pending_count.take().unwrap_or(1)
    }

    /// Reset pending count without using it
    pub fn reset_count(&mut self) {
        self.pending_count = None;
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    pub fn toggle_path_popup(&mut self) {
        self.show_path_popup = !self.show_path_popup;
    }

    pub fn state(&mut self) -> StepState {
        self.multi_diff.current_navigator().state().clone()
    }

    pub fn next_step(&mut self) {
        if self.multi_diff.current_navigator().next() {
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
        }
    }

    pub fn prev_step(&mut self) {
        if self.multi_diff.current_navigator().prev() {
            if self.animation_enabled {
                self.start_animation();
            } else {
                self.clear_active_on_next_render = true;
            }
            self.needs_scroll_to_active = true;
        }
    }

    /// Move to the next hunk (group of related changes)
    pub fn next_hunk(&mut self) {
        if self.multi_diff.current_navigator().next_hunk() {
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
        }
    }

    /// Move to the previous hunk (group of related changes)
    pub fn prev_hunk(&mut self) {
        if self.multi_diff.current_navigator().prev_hunk() {
            if self.animation_enabled {
                self.start_animation();
            } else {
                self.clear_active_on_next_render = true;
            }
            self.needs_scroll_to_active = true;
        }
    }

    /// Get current hunk info (current hunk index, total hunks)
    pub fn hunk_info(&mut self) -> (usize, usize) {
        let state = self.multi_diff.current_navigator().state();
        (state.current_hunk + 1, state.total_hunks) // 1-indexed for display
    }

    pub fn toggle_animation(&mut self) {
        self.animation_enabled = !self.animation_enabled;
    }

    pub fn toggle_zen(&mut self) {
        self.zen_mode = !self.zen_mode;
    }

    pub fn toggle_file_panel(&mut self) {
        if self.file_panel_manually_set {
            // Already manually controlled, just toggle
            self.file_panel_visible = !self.file_panel_visible;
        } else {
            // First manual toggle
            self.file_panel_manually_set = true;
            if self.file_panel_auto_hidden {
                // Panel was auto-hidden, show it
                self.file_panel_visible = true;
            } else {
                // Panel was visible, hide it
                self.file_panel_visible = false;
            }
        }
        if !self.file_panel_visible {
            self.file_list_focused = false;
        }
    }

    /// Check if current animation is backward (un-applying a change)
    pub fn is_backward_animation(&self) -> bool {
        self.animation_phase != AnimationPhase::Idle
            && self.multi_diff.current_step_direction() == StepDirection::Backward
    }

    /// Convert CLI animation phase to core AnimationFrame for phase-aware rendering
    pub fn animation_frame(&self) -> AnimationFrame {
        // Force FadeOut for one-frame render when animation disabled,
        // so backward insert-only changes produce ViewLines for extent markers
        if self.clear_active_on_next_render {
            return AnimationFrame::FadeOut;
        }
        match self.animation_phase {
            AnimationPhase::Idle => AnimationFrame::Idle,
            AnimationPhase::FadeOut => AnimationFrame::FadeOut,
            AnimationPhase::FadeIn => AnimationFrame::FadeIn,
        }
    }

    pub fn goto_start(&mut self) {
        self.multi_diff.current_navigator().goto_start();
        self.scroll_offset = 0;
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
    }

    pub fn goto_end(&mut self) {
        self.multi_diff.current_navigator().goto_end();
        self.scroll_offset = usize::MAX; // Will be clamped to bottom
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        // Don't set needs_scroll_to_active - we want to stay at bottom
    }

    pub fn goto_first_step(&mut self) {
        self.multi_diff.current_navigator().goto_start();
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
    }

    pub fn goto_last_step(&mut self) {
        self.multi_diff.current_navigator().goto_end();
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
    }

    pub fn toggle_autoplay(&mut self) {
        self.autoplay = !self.autoplay;
        self.last_autoplay_tick = Instant::now();
    }

    pub fn toggle_view_mode(&mut self) {
        self.view_mode = self.view_mode.next();
    }

    pub fn scroll_up(&mut self) {
        self.centered_once = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.centered_once = false;
        self.scroll_offset += 1;
    }

    pub fn scroll_half_page_up(&mut self, viewport_height: usize) {
        self.centered_once = false;
        let half = viewport_height / 2;
        self.scroll_offset = self.scroll_offset.saturating_sub(half);
    }

    /// Clamp scroll offset so we don't scroll past content
    /// When allow_overscroll is true, permits enough scroll for the last line to be centered
    pub fn clamp_scroll(&mut self, total_lines: usize, viewport_height: usize, allow_overscroll: bool) {
        self.scroll_offset = self.scroll_offset.min(max_scroll(total_lines, viewport_height, allow_overscroll));
    }

    /// Whether overscroll is allowed (centering is about to happen or manual zz was used)
    pub fn allow_overscroll(&self) -> bool {
        allow_overscroll_state(self.auto_center, self.needs_scroll_to_active, self.centered_once)
    }

    pub fn scroll_half_page_down(&mut self, viewport_height: usize) {
        self.centered_once = false;
        let half = viewport_height / 2;
        self.scroll_offset += half;
    }

    pub fn scroll_left(&mut self) {
        if !self.line_wrap {
            self.horizontal_scroll = self.horizontal_scroll.saturating_sub(4);
        }
    }

    pub fn scroll_right(&mut self) {
        if !self.line_wrap {
            self.horizontal_scroll += 4;
        }
    }

    /// Go to start of line (horizontal scroll = 0), like vim's 0
    pub fn scroll_to_line_start(&mut self) {
        self.horizontal_scroll = 0;
    }

    /// Go to end of line (max horizontal scroll), like vim's $
    pub fn scroll_to_line_end(&mut self) {
        if !self.line_wrap {
            // Set to max, will be clamped during render
            self.horizontal_scroll = usize::MAX / 2;
        }
    }

    /// Clamp horizontal scroll so we don't scroll too far right
    pub fn clamp_horizontal_scroll(&mut self, max_line_width: usize, viewport_width: usize) {
        if !self.line_wrap {
            let max_scroll = max_line_width.saturating_sub(viewport_width / 2);
            self.horizontal_scroll = self.horizontal_scroll.min(max_scroll);
        }
    }

    pub fn toggle_line_wrap(&mut self) {
        self.line_wrap = !self.line_wrap;
        // Reset horizontal scroll when enabling wrap
        if self.line_wrap {
            self.horizontal_scroll = 0;
        }
    }

    pub fn increase_speed(&mut self) {
        self.animation_speed = (self.animation_speed + 50).min(2000);
    }

    pub fn decrease_speed(&mut self) {
        self.animation_speed = self.animation_speed.saturating_sub(50).max(50);
    }

    // File navigation methods
    pub fn next_file(&mut self) {
        // Save current scroll positions
        let old_index = self.multi_diff.selected_index;
        if self.multi_diff.next_file() {
            self.scroll_offsets[old_index] = self.scroll_offset;
            self.horizontal_scrolls[old_index] = self.horizontal_scroll;
            // Restore scroll positions for new file
            self.scroll_offset = self.scroll_offsets[self.multi_diff.selected_index];
            self.horizontal_scroll = self.horizontal_scrolls[self.multi_diff.selected_index];
            self.animation_phase = AnimationPhase::Idle;
            self.animation_progress = 1.0;
            self.update_file_list_scroll();
            self.centered_once = false;
            self.handle_file_enter();
        }
    }

    pub fn prev_file(&mut self) {
        // Save current scroll positions
        let old_index = self.multi_diff.selected_index;
        if self.multi_diff.prev_file() {
            self.scroll_offsets[old_index] = self.scroll_offset;
            self.horizontal_scrolls[old_index] = self.horizontal_scroll;
            // Restore scroll positions for new file
            self.scroll_offset = self.scroll_offsets[self.multi_diff.selected_index];
            self.horizontal_scroll = self.horizontal_scrolls[self.multi_diff.selected_index];
            self.animation_phase = AnimationPhase::Idle;
            self.animation_progress = 1.0;
            self.update_file_list_scroll();
            self.centered_once = false;
            self.handle_file_enter();
        }
    }

    #[allow(dead_code)]
    pub fn select_file(&mut self, index: usize) {
        let old_index = self.multi_diff.selected_index;
        self.scroll_offsets[old_index] = self.scroll_offset;
        self.horizontal_scrolls[old_index] = self.horizontal_scroll;
        self.multi_diff.select_file(index);
        self.scroll_offset = self.scroll_offsets[self.multi_diff.selected_index];
        self.horizontal_scroll = self.horizontal_scrolls[self.multi_diff.selected_index];
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.handle_file_enter();
    }

    /// Handle entering a file (marks visited, optionally auto-steps to first change)
    /// Called on initial file and when switching files.
    pub fn handle_file_enter(&mut self) {
        let idx = self.multi_diff.selected_index;

        // Only process on first visit to this file
        if self.files_visited[idx] {
            return;
        }

        // Mark as visited
        self.files_visited[idx] = true;

        // Only auto-step if enabled in config
        if !self.auto_step_on_enter {
            return;
        }

        // Don't auto-step in Evolution mode
        if self.view_mode != ViewMode::Evolution {
            let state = self.multi_diff.current_navigator().state();
            if state.current_step == 0 && state.total_steps > 0 {
                self.next_step();
            }
        }
    }

    pub fn is_multi_file(&self) -> bool {
        self.multi_diff.is_multi_file()
    }

    fn update_file_list_scroll(&mut self) {
        // Keep selected file visible in the file list
        let selected = self.multi_diff.selected_index;
        if selected < self.file_list_scroll {
            self.file_list_scroll = selected;
        }
        // Assume roughly 20 visible files
        let visible_files = 20;
        if selected >= self.file_list_scroll + visible_files {
            self.file_list_scroll = selected.saturating_sub(visible_files - 1);
        }
    }

    fn start_animation(&mut self) {
        self.animation_phase = AnimationPhase::FadeOut;
        self.animation_progress = 0.0;
        self.last_animation_tick = Instant::now();
    }

    /// Ensure active change is visible if needed (called from views after stepping)
    pub fn ensure_active_visible_if_needed(&mut self, viewport_height: usize) {
        if !self.needs_scroll_to_active {
            return;
        }
        self.needs_scroll_to_active = false;

        // If auto_center is enabled, always center on active change
        if self.auto_center {
            self.center_on_active(viewport_height);
            return;
        }

        let frame = self.animation_frame();
        let view = self.multi_diff.current_navigator().current_view_with_frame(frame);

        // Prefer cursor (primary) over any active line for scroll target
        let idx = view
            .iter()
            .position(|line| line.is_primary_active)
            .or_else(|| view.iter().position(|line| line.is_active));

        if let Some(idx) = idx {
            let margin = 3.min(viewport_height / 4);

            // Check if active line is above viewport
            if idx < self.scroll_offset.saturating_add(margin) {
                self.scroll_offset = idx.saturating_sub(margin);
            }
            // Check if active line is below viewport
            else if idx >= self.scroll_offset.saturating_add(viewport_height.saturating_sub(margin)) {
                self.scroll_offset = idx.saturating_sub(viewport_height.saturating_sub(margin + 1));
            }
        }
    }

    /// Center the viewport on the active change (like Vim's zz)
    pub fn center_on_active(&mut self, viewport_height: usize) {
        let frame = self.animation_frame();
        let view = self.multi_diff.current_navigator().current_view_with_frame(frame);

        // Prefer cursor (primary) over any active line for centering
        let idx = view
            .iter()
            .position(|line| line.is_primary_active)
            .or_else(|| view.iter().position(|line| line.is_active));

        if let Some(idx) = idx {
            let half_viewport = viewport_height / 2;
            self.scroll_offset = idx.saturating_sub(half_viewport);
        }

        // Enable overscroll so centering works at bottom edge
        self.centered_once = true;

        // Also reset horizontal scroll
        self.horizontal_scroll = 0;
    }

    /// Called every frame to update animations and autoplay
    pub fn tick(&mut self) {
        let now = Instant::now();

        // Update animation
        if self.animation_phase != AnimationPhase::Idle {
            let elapsed = now.duration_since(self.last_animation_tick);
            let phase_duration = Duration::from_millis(self.animation_duration);

            self.animation_progress = (elapsed.as_secs_f32() / phase_duration.as_secs_f32()).min(1.0);

            if self.animation_progress >= 1.0 {
                match self.animation_phase {
                    AnimationPhase::FadeOut => {
                        self.animation_phase = AnimationPhase::FadeIn;
                        self.animation_progress = 0.0;
                        self.last_animation_tick = now;
                    }
                    AnimationPhase::FadeIn => {
                        self.animation_phase = AnimationPhase::Idle;
                        self.animation_progress = 1.0;

                        // If this was a backward animation, clear the active change
                        // so un-applied insertions properly disappear
                        let step_dir = self.multi_diff.current_navigator().state().step_direction;
                        if step_dir == StepDirection::Backward {
                            self.multi_diff.current_navigator().clear_active_change();
                        }
                    }
                    AnimationPhase::Idle => {}
                }
            }
        }

        // Handle autoplay
        if self.autoplay && self.animation_phase == AnimationPhase::Idle {
            let autoplay_interval = Duration::from_millis(self.animation_speed * 2);
            if now.duration_since(self.last_autoplay_tick) >= autoplay_interval {
                if !self.multi_diff.current_navigator().next() {
                    // Reached the end, stop autoplay
                    self.autoplay = false;
                } else {
                    if self.animation_enabled {
                        self.start_animation();
                    }
                    self.needs_scroll_to_active = true;
                }
                self.last_autoplay_tick = now;
            }
        }
    }

    /// Get the total number of lines in the current view
    #[allow(dead_code)]
    pub fn total_lines(&mut self) -> usize {
        let frame = self.animation_frame();
        self.multi_diff.current_navigator().current_view_with_frame(frame).len()
    }

    /// Get statistics about the current file's diff
    pub fn stats(&mut self) -> (usize, usize) {
        let diff = self.multi_diff.current_navigator().diff();
        (diff.insertions, diff.deletions)
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
            self.scroll_offsets = vec![0; file_count];
            self.horizontal_scrolls = vec![0; file_count];
            self.files_visited = vec![false; file_count];
            self.scroll_offset = 0;
            self.horizontal_scroll = 0;
            self.needs_scroll_to_active = true;
            self.centered_once = false;
            self.handle_file_enter();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allow_overscroll_state() {
        // (auto_center, needs_scroll_to_active, centered_once) -> expected
        assert!(!allow_overscroll_state(false, false, false));
        assert!(allow_overscroll_state(false, false, true));   // manual zz
        assert!(!allow_overscroll_state(false, true, false));
        assert!(!allow_overscroll_state(true, false, false));  // auto_center but not scrolling
        assert!(allow_overscroll_state(true, true, false));    // auto_center + about to scroll
        assert!(allow_overscroll_state(true, true, true));     // both
        assert!(allow_overscroll_state(true, false, true));    // centered_once wins
    }

    #[test]
    fn test_max_scroll_normal() {
        assert_eq!(max_scroll(100, 20, false), 80);
        assert_eq!(max_scroll(50, 10, false), 40);
        assert_eq!(max_scroll(20, 20, false), 0);
        assert_eq!(max_scroll(5, 20, false), 0);  // short file
    }

    #[test]
    fn test_max_scroll_overscroll() {
        // 100 - 1 - 10 = 89 (last line at center)
        assert_eq!(max_scroll(100, 20, true), 89);
        // 50 - 1 - 5 = 44
        assert_eq!(max_scroll(50, 10, true), 44);
        // short file: saturating_sub chain -> 0
        assert_eq!(max_scroll(5, 20, true), 0);
        assert_eq!(max_scroll(1, 20, true), 0);
    }
}
