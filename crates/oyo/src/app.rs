//! Application state and logic

use crate::config::{ResolvedTheme, SyntaxMode};
use crate::syntax::{SyntaxCache, SyntaxEngine, SyntaxSide};
use oyo_core::{AnimationFrame, LineKind, MultiFileDiff, StepDirection, StepState, ViewLine};
use ratatui::text::Span;
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
    /// Split view with old on left, new on right
    Split,
    /// Evolution view - shows file morphing, deletions just disappear
    Evolution,
}

impl ViewMode {
    /// Cycle to the next view mode
    pub fn next(self) -> Self {
        match self {
            ViewMode::SinglePane => ViewMode::Split,
            ViewMode::Split => ViewMode::Evolution,
            ViewMode::Evolution => ViewMode::SinglePane,
        }
    }

}

#[derive(Clone, Copy, Debug)]
struct HunkStart {
    idx: usize,
    change_id: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct HunkBounds {
    start: HunkStart,
    end: HunkStart,
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
    /// File list filter text
    pub file_filter: String,
    /// True when filter input is active
    pub file_filter_active: bool,
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
    /// Pending "g" prefix for vim-style commands (e.g., gg)
    pub pending_g_prefix: bool,
    /// Horizontal scroll offset (for long lines)
    pub horizontal_scroll: usize,
    /// Per-file horizontal scroll offsets
    horizontal_scrolls: Vec<usize>,
    /// Line wrap mode (when true, horizontal scroll is ignored)
    pub line_wrap: bool,
    /// Show scrollbar
    pub scrollbar_visible: bool,
    /// Show strikethrough on deleted text
    pub strikethrough_deletions: bool,
    /// Whether user has manually toggled the file panel (overrides auto-hide)
    pub file_panel_manually_set: bool,
    /// Whether to show the file path popup (Ctrl+G)
    pub show_path_popup: bool,
    /// Whether the file panel is currently auto-hidden due to narrow viewport
    pub file_panel_auto_hidden: bool,
    /// Auto-step to first change when entering a file at step 0
    pub auto_step_on_enter: bool,
    /// Auto-step when file would be blank at step 0 (new files)
    pub auto_step_blank_files: bool,
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
    /// Clear active change after next render (for one-frame animation styling)
    pub clear_active_on_next_render: bool,
    /// Resolved theme (colors, gradients)
    pub theme: ResolvedTheme,
    /// Whether stepping is enabled (false = no-step diff view)
    pub stepping: bool,
    /// Syntax highlighting mode
    pub syntax_mode: SyntaxMode,
    /// Syntax highlighter (lazy initialized)
    syntax_engine: Option<SyntaxEngine>,
    /// Per-file syntax cache (old/new spans)
    syntax_caches: Vec<Option<SyntaxCache>>,
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
            file_filter: String::new(),
            file_filter_active: false,
            animation_enabled: false,
            zen_mode: false,
            needs_scroll_to_active: true, // Scroll to first change on startup
            show_help: false,
            git_branch,
            auto_center: true,
            animation_duration: 150,
            pending_count: None,
            pending_g_prefix: false,
            horizontal_scroll: 0,
            horizontal_scrolls: vec![0; file_count],
            line_wrap: false,
            scrollbar_visible: false,
            strikethrough_deletions: false,
            file_panel_manually_set: false,
            show_path_popup: false,
            file_panel_auto_hidden: false,
            auto_step_on_enter: false,
            auto_step_blank_files: true,
            centered_once: false,
            primary_marker: "▶".to_string(),
            primary_marker_right: "◀".to_string(),
            extent_marker: "▌".to_string(),
            extent_marker_right: "▐".to_string(),
            clear_active_on_next_render: false,
            theme: ResolvedTheme::default(),
            stepping: true,
            syntax_mode: SyntaxMode::Auto,
            syntax_engine: None,
            syntax_caches: vec![None; file_count],
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

    pub fn toggle_syntax(&mut self) {
        self.syntax_mode = match self.syntax_mode {
            SyntaxMode::Auto => SyntaxMode::On,
            SyntaxMode::On => SyntaxMode::Off,
            SyntaxMode::Off => SyntaxMode::Auto,
        };
        if matches!(self.syntax_mode, SyntaxMode::Off) {
            self.syntax_engine = None;
            self.syntax_caches = vec![None; self.multi_diff.file_count()];
        }
    }

    pub fn state(&mut self) -> StepState {
        self.multi_diff.current_navigator().state().clone()
    }

    pub fn syntax_enabled(&self) -> bool {
        match self.syntax_mode {
            SyntaxMode::On => true,
            SyntaxMode::Off => false,
            SyntaxMode::Auto => !self.stepping,
        }
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
        let spans = cache.spans(side, line_num - 1)?;
        Some(
            spans
                .iter()
                .map(|span| Span::styled(span.text.clone(), span.style))
                .collect(),
        )
    }

    fn ensure_syntax_cache(&mut self) -> Option<&SyntaxCache> {
        if !self.syntax_enabled() {
            return None;
        }
        let idx = self.multi_diff.selected_index;
        if idx >= self.syntax_caches.len() {
            self.syntax_caches = vec![None; self.multi_diff.file_count()];
        }
        if self.syntax_caches[idx].is_none() {
            let file_name = self.current_file_path();
            let (old_content, new_content) = {
                let nav = self.multi_diff.current_navigator();
                (nav.old_content().to_string(), nav.new_content().to_string())
            };
            if self.syntax_engine.is_none() {
                self.syntax_engine = Some(SyntaxEngine::new(&self.theme));
            }
            let engine = self.syntax_engine.as_ref()?;
            self.syntax_caches[idx] = Some(SyntaxCache::new(
                engine,
                &old_content,
                &new_content,
                &file_name,
            ));
        }
        self.syntax_caches[idx].as_ref()
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

    /// Compute hunk starts for single/evolution view (display index + change id).
    fn compute_hunk_starts_single(&mut self) -> Vec<Option<HunkStart>> {
        let view = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(AnimationFrame::Idle);
        let (_, total_hunks) = self.hunk_info();

        let mut hunk_starts = vec![None; total_hunks];
        let mut display_idx = 0;

        for line in &view {
            let is_visible = match self.view_mode {
                ViewMode::Evolution => {
                    !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                }
                _ => true,
            };
            if !is_visible {
                continue;
            }
            if let Some(hidx) = line.hunk_index {
                if hidx < total_hunks && hunk_starts[hidx].is_none() {
                    hunk_starts[hidx] = Some(HunkStart {
                        idx: display_idx,
                        change_id: Some(line.change_id),
                    });
                }
            }
            display_idx += 1;
        }
        hunk_starts
    }

    /// Compute hunk bounds for single/evolution view (display start/end + change id).
    fn compute_hunk_bounds_single(&mut self) -> Vec<Option<HunkBounds>> {
        let view = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(AnimationFrame::Idle);
        let (_, total_hunks) = self.hunk_info();

        let mut bounds: Vec<Option<HunkBounds>> = vec![None; total_hunks];
        let mut display_idx = 0;

        for line in &view {
            let is_visible = match self.view_mode {
                ViewMode::Evolution => {
                    !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                }
                _ => true,
            };
            if !is_visible {
                continue;
            }
            if let Some(hidx) = line.hunk_index {
                if hidx < total_hunks {
                    let start = HunkStart {
                        idx: display_idx,
                        change_id: Some(line.change_id),
                    };
                    if let Some(existing) = bounds[hidx] {
                        bounds[hidx] = Some(HunkBounds {
                            start: existing.start,
                            end: start,
                        });
                    } else {
                        bounds[hidx] = Some(HunkBounds {
                            start,
                            end: start,
                        });
                    }
                }
            }
            display_idx += 1;
        }
        bounds
    }

    /// Compute hunk starts for split view (per-pane display index + change id).
    fn compute_hunk_starts_split(&mut self) -> (Vec<Option<HunkStart>>, Vec<Option<HunkStart>>) {
        let view = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(AnimationFrame::Idle);
        let (_, total_hunks) = self.hunk_info();

        let mut old_starts = vec![None; total_hunks];
        let mut new_starts = vec![None; total_hunks];
        let mut old_idx = 0usize;
        let mut new_idx = 0usize;

        for line in &view {
            if line.old_line.is_some() {
                if let Some(hidx) = line.hunk_index {
                    if hidx < total_hunks && old_starts[hidx].is_none() {
                        old_starts[hidx] = Some(HunkStart {
                            idx: old_idx,
                            change_id: Some(line.change_id),
                        });
                    }
                }
                old_idx += 1;
            }
            if line.new_line.is_some() {
                if let Some(hidx) = line.hunk_index {
                    if hidx < total_hunks && new_starts[hidx].is_none() {
                        new_starts[hidx] = Some(HunkStart {
                            idx: new_idx,
                            change_id: Some(line.change_id),
                        });
                    }
                }
                new_idx += 1;
            }
        }

        (old_starts, new_starts)
    }

    /// Compute hunk bounds for split view (per-pane display start/end + change id).
    fn compute_hunk_bounds_split(
        &mut self,
    ) -> (Vec<Option<HunkBounds>>, Vec<Option<HunkBounds>>) {
        let view = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(AnimationFrame::Idle);
        let (_, total_hunks) = self.hunk_info();

        let mut old_bounds: Vec<Option<HunkBounds>> = vec![None; total_hunks];
        let mut new_bounds: Vec<Option<HunkBounds>> = vec![None; total_hunks];
        let mut old_idx = 0usize;
        let mut new_idx = 0usize;

        for line in &view {
            if line.old_line.is_some() {
                if let Some(hidx) = line.hunk_index {
                    if hidx < total_hunks {
                        let start = HunkStart {
                            idx: old_idx,
                            change_id: Some(line.change_id),
                        };
                        if let Some(existing) = old_bounds[hidx] {
                            old_bounds[hidx] = Some(HunkBounds {
                                start: existing.start,
                                end: start,
                            });
                        } else {
                            old_bounds[hidx] = Some(HunkBounds {
                                start,
                                end: start,
                            });
                        }
                    }
                }
                old_idx += 1;
            }
            if line.new_line.is_some() {
                if let Some(hidx) = line.hunk_index {
                    if hidx < total_hunks {
                        let start = HunkStart {
                            idx: new_idx,
                            change_id: Some(line.change_id),
                        };
                        if let Some(existing) = new_bounds[hidx] {
                            new_bounds[hidx] = Some(HunkBounds {
                                start: existing.start,
                                end: start,
                            });
                        } else {
                            new_bounds[hidx] = Some(HunkBounds {
                                start,
                                end: start,
                            });
                        }
                    }
                }
                new_idx += 1;
            }
        }

        (old_bounds, new_bounds)
    }

    fn pick_split_start(&self, old: Option<HunkStart>, new: Option<HunkStart>) -> Option<HunkStart> {
        match (old, new) {
            (Some(o), Some(n)) => {
                let old_dist = (o.idx as isize - self.scroll_offset as isize).abs();
                let new_dist = (n.idx as isize - self.scroll_offset as isize).abs();
                if old_dist < new_dist {
                    Some(o)
                } else if new_dist < old_dist {
                    Some(n)
                } else {
                    Some(n)
                }
            }
            (Some(o), None) => Some(o),
            (None, Some(n)) => Some(n),
            (None, None) => None,
        }
    }

    fn pick_split_bounds(
        &self,
        old: Option<HunkBounds>,
        new: Option<HunkBounds>,
    ) -> Option<HunkBounds> {
        match (old, new) {
            (Some(o), Some(n)) => {
                let old_dist = (o.start.idx as isize - self.scroll_offset as isize).abs();
                let new_dist = (n.start.idx as isize - self.scroll_offset as isize).abs();
                if old_dist < new_dist {
                    Some(o)
                } else if new_dist < old_dist {
                    Some(n)
                } else {
                    Some(n)
                }
            }
            (Some(o), None) => Some(o),
            (None, Some(n)) => Some(n),
            (None, None) => None,
        }
    }

    fn next_hunk_from_starts(
        &self,
        starts: &[Option<HunkStart>],
        inclusive: bool,
    ) -> Option<(usize, HunkStart)> {
        let current_hunk_idx = starts
            .iter()
            .enumerate()
            .filter_map(|(idx, start)| start.map(|s| (idx, s.idx)))
            .filter(|&(_, start)| {
                if inclusive {
                    start <= self.scroll_offset
                } else {
                    start < self.scroll_offset
                }
            })
            .map(|(idx, _)| idx)
            .last();

        let mut target_idx = match current_hunk_idx {
            Some(curr) => curr + 1,
            None => 0,
        };

        while target_idx < starts.len() {
            if let Some(start) = starts[target_idx] {
                return Some((target_idx, start));
            }
            target_idx += 1;
        }
        None
    }

    fn next_hunk_from_index(
        &self,
        starts: &[Option<HunkStart>],
        current_hunk: usize,
    ) -> Option<(usize, HunkStart)> {
        let mut target_idx = current_hunk.saturating_add(1);
        while target_idx < starts.len() {
            if let Some(start) = starts[target_idx] {
                return Some((target_idx, start));
            }
            target_idx += 1;
        }
        None
    }

    fn single_hunk_fallback(&self, starts: &[Option<HunkStart>]) -> Option<(usize, HunkStart)> {
        let mut only: Option<(usize, HunkStart)> = None;
        for (idx, start) in starts.iter().enumerate() {
            if let Some(start) = start {
                if only.is_some() {
                    return None;
                }
                only = Some((idx, *start));
            }
        }
        only
    }

    fn prev_hunk_from_starts(
        &self,
        starts: &[Option<HunkStart>],
    ) -> Option<(usize, HunkStart)> {
        starts
            .iter()
            .enumerate()
            .filter_map(|(idx, start)| start.map(|s| (idx, s)))
            .filter(|&(_, start)| start.idx < self.scroll_offset)
            .last()
    }

    fn prev_hunk_from_index(
        &self,
        starts: &[Option<HunkStart>],
        current_hunk: usize,
    ) -> Option<(usize, HunkStart)> {
        if starts.is_empty() {
            return None;
        }
        let mut idx = current_hunk.min(starts.len() - 1);
        while idx > 0 {
            idx -= 1;
            if let Some(start) = starts[idx] {
                return Some((idx, start));
            }
        }
        None
    }

    fn current_hunk_from_bounds(&self, bounds: &[Option<HunkBounds>]) -> Option<usize> {
        bounds
            .iter()
            .enumerate()
            .filter_map(|(idx, bound)| bound.map(|b| (idx, b.start.idx)))
            .filter(|&(_, start)| start <= self.scroll_offset)
            .map(|(idx, _)| idx)
            .last()
    }

    fn first_available_hunk(bounds: &[Option<HunkBounds>]) -> Option<(usize, HunkBounds)> {
        bounds
            .iter()
            .enumerate()
            .find_map(|(idx, bound)| bound.map(|b| (idx, b)))
    }

    fn set_cursor_for_current_scroll(&mut self) {
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_starts, new_starts) = self.compute_hunk_starts_split();
                let effective: Vec<Option<HunkStart>> = old_starts
                    .into_iter()
                    .zip(new_starts.into_iter())
                    .map(|(old, new)| self.pick_split_start(old, new))
                    .collect();
                effective
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, start)| start.map(|s| (idx, s)))
                    .filter(|&(_, start)| start.idx <= self.scroll_offset)
                    .last()
            }
            _ => {
                let hunk_starts = self.compute_hunk_starts_single();
                hunk_starts
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, start)| start.map(|s| (idx, s)))
                    .filter(|&(_, start)| start.idx <= self.scroll_offset)
                    .last()
            }
        };

        if let Some((hidx, start)) = target {
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, start.change_id);
        } else {
            self.multi_diff.current_navigator().clear_cursor_change();
        }
    }

    /// Scroll to the next hunk (no-step mode)
    pub fn next_hunk_scroll(&mut self) {
        let auto_center = self.auto_center;
        let (current_hunk, cursor_set) = {
            let state = self.multi_diff.current_navigator().state();
            (state.current_hunk, state.cursor_change.is_some())
        };
        let in_hunk_scope = self.multi_diff.current_navigator().state().last_nav_was_hunk;
        let use_cursor = auto_center && cursor_set && in_hunk_scope;
        let inclusive = in_hunk_scope;
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_starts, new_starts) = self.compute_hunk_starts_split();
                let effective: Vec<Option<HunkStart>> = old_starts
                    .into_iter()
                    .zip(new_starts.into_iter())
                    .map(|(old, new)| self.pick_split_start(old, new))
                    .collect();
                let mut target = if use_cursor && current_hunk < effective.len() {
                    self.next_hunk_from_index(&effective, current_hunk)
                } else {
                    self.next_hunk_from_starts(&effective, inclusive)
                };
                if target.is_none() {
                    target = self.single_hunk_fallback(&effective);
                }
                target
            }
            _ => {
                let hunk_starts = self.compute_hunk_starts_single();
                let mut target = if use_cursor && current_hunk < hunk_starts.len() {
                    self.next_hunk_from_index(&hunk_starts, current_hunk)
                } else {
                    self.next_hunk_from_starts(&hunk_starts, inclusive)
                };
                if target.is_none() {
                    target = self.single_hunk_fallback(&hunk_starts);
                }
                target
            }
        };

        if let Some((hidx, start)) = target {
            self.scroll_offset = start.idx;
            self.centered_once = false;
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, start.change_id);
            self.multi_diff.current_navigator().set_hunk_scope(true);
            if self.auto_center {
                self.needs_scroll_to_active = true;
            }
        }
    }

    /// Scroll to the previous hunk (no-step mode)
    pub fn prev_hunk_scroll(&mut self) {
        let auto_center = self.auto_center;
        let (current_hunk, cursor_set) = {
            let state = self.multi_diff.current_navigator().state();
            (state.current_hunk, state.cursor_change.is_some())
        };
        let in_hunk_scope = self.multi_diff.current_navigator().state().last_nav_was_hunk;
        let use_cursor = auto_center && cursor_set && in_hunk_scope;
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_starts, new_starts) = self.compute_hunk_starts_split();
                let effective: Vec<Option<HunkStart>> = old_starts
                    .into_iter()
                    .zip(new_starts.into_iter())
                    .map(|(old, new)| self.pick_split_start(old, new))
                    .collect();
                let mut target = if use_cursor && current_hunk < effective.len() {
                    self.prev_hunk_from_index(&effective, current_hunk)
                } else {
                    self.prev_hunk_from_starts(&effective)
                };
                if target.is_none() {
                    target = self.single_hunk_fallback(&effective);
                }
                target
            }
            _ => {
                let hunk_starts = self.compute_hunk_starts_single();
                let mut target = if use_cursor && current_hunk < hunk_starts.len() {
                    self.prev_hunk_from_index(&hunk_starts, current_hunk)
                } else {
                    self.prev_hunk_from_starts(&hunk_starts)
                };
                if target.is_none() {
                    target = self.single_hunk_fallback(&hunk_starts);
                }
                target
            }
        };

        if let Some((hidx, start)) = target {
            self.scroll_offset = start.idx;
            self.centered_once = false;
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, start.change_id);
            self.multi_diff.current_navigator().set_hunk_scope(true);
            if self.auto_center {
                self.needs_scroll_to_active = true;
            }
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

    /// Jump to first change of current hunk
    pub fn goto_hunk_start(&mut self) {
        if self.multi_diff.current_navigator().goto_hunk_start() {
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
        }
    }

    /// Jump to the start of the current hunk (no-step mode)
    pub fn goto_hunk_start_scroll(&mut self) {
        let (current_hunk, in_hunk_scope) = {
            let state = self.multi_diff.current_navigator().state();
            (
                state.current_hunk,
                state.last_nav_was_hunk && state.cursor_change.is_some(),
            )
        };
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_bounds, new_bounds) = self.compute_hunk_bounds_split();
                let effective: Vec<Option<HunkBounds>> = old_bounds
                    .into_iter()
                    .zip(new_bounds.into_iter())
                    .map(|(old, new)| self.pick_split_bounds(old, new))
                    .collect();
                let hidx = if in_hunk_scope {
                    Some(current_hunk)
                } else {
                    self.current_hunk_from_bounds(&effective)
                };
                hidx.and_then(|idx| effective.get(idx).copied().flatten().map(|b| (idx, b)))
                    .or_else(|| Self::first_available_hunk(&effective))
            }
            _ => {
                let bounds = self.compute_hunk_bounds_single();
                let hidx = if in_hunk_scope {
                    Some(current_hunk)
                } else {
                    self.current_hunk_from_bounds(&bounds)
                };
                hidx.and_then(|idx| bounds.get(idx).copied().flatten().map(|b| (idx, b)))
                    .or_else(|| Self::first_available_hunk(&bounds))
            }
        };

        if let Some((hidx, bound)) = target {
            self.scroll_offset = bound.start.idx;
            self.centered_once = false;
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, bound.start.change_id);
            self.multi_diff.current_navigator().set_hunk_scope(true);
            if self.auto_center {
                self.needs_scroll_to_active = true;
            }
        }
    }

    /// Jump to last change of current hunk
    pub fn goto_hunk_end(&mut self) {
        if self.multi_diff.current_navigator().goto_hunk_end() {
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
        }
    }

    /// Jump to the end of the current hunk (no-step mode)
    pub fn goto_hunk_end_scroll(&mut self) {
        let (current_hunk, in_hunk_scope) = {
            let state = self.multi_diff.current_navigator().state();
            (
                state.current_hunk,
                state.last_nav_was_hunk && state.cursor_change.is_some(),
            )
        };
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_bounds, new_bounds) = self.compute_hunk_bounds_split();
                let effective: Vec<Option<HunkBounds>> = old_bounds
                    .into_iter()
                    .zip(new_bounds.into_iter())
                    .map(|(old, new)| self.pick_split_bounds(old, new))
                    .collect();
                let hidx = if in_hunk_scope {
                    Some(current_hunk)
                } else {
                    self.current_hunk_from_bounds(&effective)
                };
                hidx.and_then(|idx| effective.get(idx).copied().flatten().map(|b| (idx, b)))
                    .or_else(|| Self::first_available_hunk(&effective))
            }
            _ => {
                let bounds = self.compute_hunk_bounds_single();
                let hidx = if in_hunk_scope {
                    Some(current_hunk)
                } else {
                    self.current_hunk_from_bounds(&bounds)
                };
                hidx.and_then(|idx| bounds.get(idx).copied().flatten().map(|b| (idx, b)))
                    .or_else(|| Self::first_available_hunk(&bounds))
            }
        };

        if let Some((hidx, bound)) = target {
            self.scroll_offset = bound.end.idx;
            self.centered_once = false;
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, bound.end.change_id);
            self.multi_diff.current_navigator().set_hunk_scope(true);
            if self.auto_center {
                self.needs_scroll_to_active = true;
            }
        }
    }

    /// Enter no-step mode without changing scroll position.
    pub fn enter_no_step_mode(&mut self) {
        // Evolution mode requires stepping, so switch to Single view
        if self.view_mode == ViewMode::Evolution {
            self.view_mode = ViewMode::SinglePane;
        }

        let old_scroll = self.scroll_offset;
        self.multi_diff.current_navigator().goto_end();
        self.multi_diff.current_navigator().clear_active_change();
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.scroll_offset = old_scroll;
        self.needs_scroll_to_active = false;
        self.set_cursor_for_current_scroll();
    }

    pub fn toggle_stepping(&mut self) {
        self.stepping = !self.stepping;
        if !self.stepping {
            self.enter_no_step_mode();
        } else {
            // Turning ON stepping
            // Reset to clean slate (start)
            self.goto_start();
        }
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
        if !self.stepping {
            self.scroll_offset = 0;
            self.centered_once = false;
            self.needs_scroll_to_active = false;
            self.multi_diff.current_navigator().clear_cursor_change();
            self.multi_diff.current_navigator().set_hunk_scope(false);
            return;
        }
        self.multi_diff.current_navigator().goto_start();
        self.scroll_offset = 0;
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
    }

    pub fn goto_end(&mut self) {
        if !self.stepping {
            self.scroll_offset = usize::MAX;
            self.centered_once = false;
            self.needs_scroll_to_active = false;
            self.multi_diff.current_navigator().clear_cursor_change();
            self.multi_diff.current_navigator().set_hunk_scope(false);
            return;
        }
        self.multi_diff.current_navigator().goto_end();
        self.scroll_offset = usize::MAX; // Will be clamped to bottom
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        // Don't set needs_scroll_to_active - we want to stay at bottom
    }

    pub fn goto_first_step(&mut self) {
        self.multi_diff.current_navigator().goto(1);
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
        if !self.stepping {
            // In no-step mode, skip Evolution view as it requires stepping
            self.view_mode = match self.view_mode {
                ViewMode::SinglePane => ViewMode::Split,
                ViewMode::Split => ViewMode::SinglePane,
                _ => ViewMode::SinglePane,
            };
        } else {
            self.view_mode = self.view_mode.next();
        }
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

    pub fn toggle_strikethrough_deletions(&mut self) {
        self.strikethrough_deletions = !self.strikethrough_deletions;
    }

    pub fn increase_speed(&mut self) {
        self.animation_speed = (self.animation_speed + 50).min(2000);
    }

    pub fn decrease_speed(&mut self) {
        self.animation_speed = self.animation_speed.saturating_sub(50).max(50);
    }

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

        // Only process on first visit to this file
        if self.files_visited[idx] {
            return;
        }

        // Mark as visited
        self.files_visited[idx] = true;

        // If in no-step mode, ensure full content is shown immediately
        if !self.stepping {
            self.multi_diff.current_navigator().goto_end();
            self.multi_diff.current_navigator().clear_active_change();
            self.animation_phase = AnimationPhase::Idle;
            self.animation_progress = 1.0;
            self.set_cursor_for_current_scroll();
            // Don't mess with scroll_offset here; it might have been restored by next_file/prev_file
            return;
        }

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
        let selected_pos = indices
            .iter()
            .position(|&i| i == selected)
            .unwrap_or(0);
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
        let step_direction = self.multi_diff.current_step_direction();

        let (display_len, display_idx) = display_metrics(
            &view,
            self.view_mode,
            self.animation_phase,
            self.scroll_offset,
            step_direction,
        );

        if let Some(idx) = display_idx {
            let margin = 3.min(viewport_height / 4);

            // Check if active line is above viewport
            if idx < self.scroll_offset.saturating_add(margin) {
                self.scroll_offset = idx.saturating_sub(margin);
            }
            // Check if active line is below viewport
            else if idx >= self.scroll_offset.saturating_add(viewport_height.saturating_sub(margin)) {
                self.scroll_offset = idx.saturating_sub(viewport_height.saturating_sub(margin + 1));
            }
        } else if display_len > 0 {
            // No active line (step 0); snap to top so "first step" is visible.
            self.scroll_offset = 0;
        }
    }

    /// Center the viewport on the active change (like Vim's zz)
    pub fn center_on_active(&mut self, viewport_height: usize) {
        let frame = self.animation_frame();
        let view = self.multi_diff.current_navigator().current_view_with_frame(frame);
        let step_direction = self.multi_diff.current_step_direction();

        let (display_len, display_idx) = display_metrics(
            &view,
            self.view_mode,
            self.animation_phase,
            self.scroll_offset,
            step_direction,
        );

        if let Some(idx) = display_idx {
            let half_viewport = viewport_height / 2;
            self.scroll_offset = idx.saturating_sub(half_viewport);
        } else if display_len > 0 {
            // No active line (step 0); default to top of file.
            self.scroll_offset = 0;
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
        if self.stepping && self.autoplay && self.animation_phase == AnimationPhase::Idle {
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
            self.scroll_offsets = vec![0; file_count];
            self.horizontal_scrolls = vec![0; file_count];
            self.files_visited = vec![false; file_count];
            self.syntax_caches = vec![None; file_count];
            self.scroll_offset = 0;
            self.horizontal_scroll = 0;
            self.needs_scroll_to_active = true;
            self.centered_once = false;
            self.handle_file_enter();
        }
    }
}

/// Compute display metrics for scroll calculations.
/// Returns (display_len, display_idx_of_active).
/// display_idx is the row index of the primary active line (fallback to any active)
/// in the filtered/displayed line stream for the current view mode.
pub fn display_metrics(
    view: &[ViewLine],
    view_mode: ViewMode,
    animation_phase: AnimationPhase,
    scroll_offset: usize,
    step_direction: StepDirection,
) -> (usize, Option<usize>) {
    match view_mode {
        ViewMode::SinglePane => {
            let idx = view
                .iter()
                .position(|l| l.is_primary_active)
                .or_else(|| view.iter().position(|l| l.is_active));
            (view.len(), idx)
        }
        ViewMode::Evolution => evolution_display_metrics(view, animation_phase),
        ViewMode::Split => split_display_metrics(view, scroll_offset, step_direction),
    }
}

/// Evolution view skips Deleted lines and idle PendingDelete lines.
fn evolution_display_metrics(
    view: &[ViewLine],
    animation_phase: AnimationPhase,
) -> (usize, Option<usize>) {
    let mut display_len = 0usize;
    let mut primary_idx: Option<usize> = None;
    let mut any_active_idx: Option<usize> = None;

    for line in view {
        let visible = match line.kind {
            LineKind::Deleted => false,
            LineKind::PendingDelete => {
                // Show during animation if active, hide when idle
                line.is_active && animation_phase != AnimationPhase::Idle
            }
            _ => true,
        };

        if visible {
            if line.is_primary_active && primary_idx.is_none() {
                primary_idx = Some(display_len);
            }
            if line.is_active && any_active_idx.is_none() {
                any_active_idx = Some(display_len);
            }
            display_len += 1;
        }
    }

    (display_len, primary_idx.or(any_active_idx))
}

/// Split view shows old_line on left pane, new_line on right pane.
/// display_len = max(old_count, new_count).
/// For active index: primary always dominates; among candidates, minimize jump from
/// scroll_offset with tie-break by step direction.
fn split_display_metrics(
    view: &[ViewLine],
    scroll_offset: usize,
    step_direction: StepDirection,
) -> (usize, Option<usize>) {
    let mut old_count = 0usize;
    let mut new_count = 0usize;
    // Track primary separately from fallback (first non-primary active)
    let mut old_primary_idx: Option<usize> = None;
    let mut new_primary_idx: Option<usize> = None;
    let mut old_fallback_idx: Option<usize> = None;
    let mut new_fallback_idx: Option<usize> = None;

    for line in view {
        if line.old_line.is_some() {
            if line.is_primary_active {
                old_primary_idx = Some(old_count);
            } else if line.is_active && old_fallback_idx.is_none() {
                old_fallback_idx = Some(old_count);
            }
            old_count += 1;
        }
        if line.new_line.is_some() {
            if line.is_primary_active {
                new_primary_idx = Some(new_count);
            } else if line.is_active && new_fallback_idx.is_none() {
                new_fallback_idx = Some(new_count);
            }
            new_count += 1;
        }
    }

    let display_len = old_count.max(new_count);

    // Prefer primary over fallback: only use fallback when no primary exists on either side
    let (old_idx, new_idx) = if old_primary_idx.is_some() || new_primary_idx.is_some() {
        (old_primary_idx, new_primary_idx)
    } else {
        (old_fallback_idx, new_fallback_idx)
    };

    // Pick index that minimizes jump from current scroll_offset
    let active_idx = match (old_idx, new_idx) {
        (Some(old), Some(new)) => {
            let old_dist = (old as isize - scroll_offset as isize).abs();
            let new_dist = (new as isize - scroll_offset as isize).abs();
            if old_dist < new_dist {
                Some(old)
            } else if new_dist < old_dist {
                Some(new)
            } else {
                // Tie-break by step direction (default to new side)
                match step_direction {
                    StepDirection::Forward | StepDirection::None => Some(new),
                    StepDirection::Backward => Some(old),
                }
            }
        }
        (Some(old), None) => Some(old),
        (None, Some(new)) => Some(new),
        (None, None) => None,
    };

    (display_len, active_idx)
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

    fn make_view_line(
        kind: LineKind,
        old_line: Option<usize>,
        new_line: Option<usize>,
        is_active: bool,
        is_primary_active: bool,
    ) -> ViewLine {
        ViewLine {
            content: String::new(),
            spans: vec![],
            kind,
            old_line,
            new_line,
            is_active,
            is_primary_active,
            show_hunk_extent: false,
            change_id: 0,
            hunk_index: None,
        }
    }

    #[test]
    fn test_evolution_metrics_skips_deleted() {
        let view = vec![
            make_view_line(LineKind::Context, Some(1), Some(1), false, false),
            make_view_line(LineKind::Deleted, Some(2), None, false, false),
            make_view_line(LineKind::Deleted, Some(3), None, false, false),
            make_view_line(LineKind::Context, Some(4), Some(2), true, true),
        ];
        // Deleted lines skipped: display_len = 2 (context + context)
        // Primary active at raw index 3, but display index 1 (after skipping 2 deleted)
        let (len, idx) = evolution_display_metrics(&view, AnimationPhase::Idle);
        assert_eq!(len, 2);
        assert_eq!(idx, Some(1));
    }

    #[test]
    fn test_evolution_metrics_pending_delete_visibility() {
        // PendingDelete: visible only when is_active && animation_phase != Idle
        let view = vec![
            make_view_line(LineKind::Context, Some(1), Some(1), false, false),
            make_view_line(LineKind::PendingDelete, Some(2), None, true, true),
            make_view_line(LineKind::Context, Some(3), Some(2), false, false),
        ];

        // Idle: PendingDelete hidden even if active
        let (len, idx) = evolution_display_metrics(&view, AnimationPhase::Idle);
        assert_eq!(len, 2);
        assert_eq!(idx, None); // primary was on the hidden line

        // FadeOut: PendingDelete visible when active
        let (len, idx) = evolution_display_metrics(&view, AnimationPhase::FadeOut);
        assert_eq!(len, 3);
        assert_eq!(idx, Some(1));

        // FadeIn: also visible
        let (len, idx) = evolution_display_metrics(&view, AnimationPhase::FadeIn);
        assert_eq!(len, 3);
        assert_eq!(idx, Some(1));
    }

    #[test]
    fn test_split_metrics_primary_dominates() {
        // Insert-only change: primary exists only on new side (new_line.is_some())
        // Old side has a non-primary active line closer to scroll_offset
        let view = vec![
            make_view_line(LineKind::Context, Some(1), Some(1), true, false),  // active but not primary, both sides
            make_view_line(LineKind::Context, Some(2), Some(2), false, false),
            make_view_line(LineKind::Inserted, None, Some(3), true, true),     // primary, new side only
        ];
        // scroll_offset=0: old side's active at idx 0 is closer than new side's primary at idx 2
        // But primary must dominate, so result should be new side's idx 2
        let (len, idx) = split_display_metrics(&view, 0, StepDirection::Forward);
        assert_eq!(len, 3); // max(2 old, 3 new)
        assert_eq!(idx, Some(2)); // new_primary_idx, not old_fallback_idx
    }

    #[test]
    fn test_split_metrics_minimize_jump() {
        // Both sides have primary active (e.g., modified line with old+new)
        let view = vec![
            make_view_line(LineKind::Context, Some(1), Some(1), false, false),
            make_view_line(LineKind::Context, Some(2), Some(2), false, false),
            make_view_line(LineKind::Modified, Some(3), Some(3), true, true),
            make_view_line(LineKind::Context, Some(4), Some(4), false, false),
        ];
        // Both old and new primary at idx 2
        // scroll_offset=0: both equally close (dist=2), tie-break by direction
        let (_, idx) = split_display_metrics(&view, 0, StepDirection::Forward);
        assert_eq!(idx, Some(2)); // new side wins on Forward

        let (_, idx) = split_display_metrics(&view, 0, StepDirection::Backward);
        assert_eq!(idx, Some(2)); // old side wins on Backward (same value here)

        // scroll_offset=10: both at dist=8, tie-break applies
        let (_, idx) = split_display_metrics(&view, 10, StepDirection::Forward);
        assert_eq!(idx, Some(2));
    }

    #[test]
    fn test_split_metrics_fallback_when_no_primary() {
        // No primary active, should fall back to first active on each side
        let view = vec![
            make_view_line(LineKind::Context, Some(1), Some(1), false, false),
            make_view_line(LineKind::Context, Some(2), Some(2), true, false), // active, not primary
            make_view_line(LineKind::Context, Some(3), Some(3), false, false),
        ];
        let (len, idx) = split_display_metrics(&view, 0, StepDirection::Forward);
        assert_eq!(len, 3);
        assert_eq!(idx, Some(1)); // fallback to first active
    }

    fn make_app_with_two_hunks() -> App {
        let old_lines: Vec<String> = (1..=25).map(|i| format!("line{}", i)).collect();
        let mut new_lines = old_lines.clone();
        new_lines[1] = "line2-new".to_string();
        new_lines[19] = "line20-new".to_string();
        let old = old_lines.join("\n");
        let new = new_lines.join("\n");

        let multi_diff = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            old,
            new,
        );
        let mut app = App::new(multi_diff, ViewMode::SinglePane, 0, false, None);
        app.stepping = false;
        app.enter_no_step_mode();
        app
    }

    fn make_app_with_single_hunk() -> App {
        let old = "one\ntwo\nthree".to_string();
        let new = "one\nTWO\nthree".to_string();
        let multi_diff = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            old,
            new,
        );
        let mut app = App::new(multi_diff, ViewMode::SinglePane, 0, false, None);
        app.stepping = false;
        app.enter_no_step_mode();
        app
    }

    #[test]
    fn test_no_step_prev_hunk_from_bottom_advances() {
        let mut app = make_app_with_two_hunks();
        let total_hunks = app.multi_diff.current_navigator().state().total_hunks;
        assert_eq!(total_hunks, 2);

        app.goto_end(); // no-step mode: scroll-only, no cursor
        app.prev_hunk_scroll();
        {
            let state = app.multi_diff.current_navigator().state();
            assert!(state.cursor_change.is_some());
            assert!(state.last_nav_was_hunk);
        }

        app.prev_hunk_scroll();
        let state = app.multi_diff.current_navigator().state();
        assert_eq!(state.current_hunk, 0);
    }

    #[test]
    fn test_no_step_next_hunk_after_goto_start() {
        let mut app = make_app_with_two_hunks();
        app.goto_start(); // no-step mode: clears cursor + scope

        app.next_hunk_scroll();
        let state = app.multi_diff.current_navigator().state();
        assert_eq!(state.current_hunk, 0);
        assert!(state.cursor_change.is_some());
        assert!(state.last_nav_was_hunk);
    }

    #[test]
    fn test_single_hunk_jump_sets_cursor() {
        let mut app = make_app_with_single_hunk();
        app.next_hunk_scroll();
        let state = app.multi_diff.current_navigator().state();
        assert_eq!(state.total_hunks, 1);
        assert_eq!(state.current_hunk, 0);
        assert!(state.cursor_change.is_some());
        assert!(state.last_nav_was_hunk);
    }

    #[test]
    fn test_goto_start_clears_hunk_scope_in_no_step() {
        let mut app = make_app_with_two_hunks();
        app.next_hunk_scroll();
        app.goto_start();

        let state = app.multi_diff.current_navigator().state();
        assert!(!state.last_nav_was_hunk);
        assert!(state.cursor_change.is_none());
    }

    #[test]
    fn test_goto_end_clears_hunk_scope_in_no_step() {
        let mut app = make_app_with_two_hunks();
        app.next_hunk_scroll();
        app.goto_end();

        let state = app.multi_diff.current_navigator().state();
        assert!(!state.last_nav_was_hunk);
        assert!(state.cursor_change.is_none());
    }

    #[test]
    fn test_no_step_b_e_jump_within_hunk() {
        let mut app = make_app_with_two_hunks();
        app.next_hunk_scroll();

        let state = app.multi_diff.current_navigator().state();
        assert_eq!(state.current_hunk, 0);

        app.goto_hunk_end_scroll();
        let end_state = app.multi_diff.current_navigator().state();
        assert_eq!(end_state.current_hunk, 0);
        assert!(end_state.cursor_change.is_some());

        app.goto_hunk_start_scroll();
        let start_state = app.multi_diff.current_navigator().state();
        assert_eq!(start_state.current_hunk, 0);
        assert!(start_state.cursor_change.is_some());
    }
}
