//! Application state and logic

use crate::color;
use crate::config::{
    DiffExtentMarkerMode, DiffExtentMarkerScope, DiffForegroundMode, DiffHighlightMode,
    FileCountMode, HunkWrapMode, ModifiedStepMode, ResolvedTheme, StepWrapMode, SyntaxMode,
};
use crate::syntax::{SyntaxCache, SyntaxEngine, SyntaxSide};
use oyo_core::{
    AnimationFrame, Change, ChangeKind, LineKind, MultiFileDiff, StepDirection, StepState, ViewLine,
};
use ratatui::style::Color;
use ratatui::text::Span;
use regex::{Regex, RegexBuilder};
use std::time::{Duration, Instant};
use std::{
    io::Write,
    process::{Command, Stdio},
};

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

pub(crate) const FILE_PANEL_MIN_WIDTH: u16 = 24;
pub(crate) const DIFF_VIEW_MIN_WIDTH: u16 = 50;

#[derive(Clone, Copy, Debug)]
struct NoStepState {
    current_hunk: usize,
    cursor_change: Option<usize>,
    last_nav_was_hunk: bool,
}

#[derive(Clone, Copy, Debug)]
enum StepEdge {
    Start,
    End,
}

#[derive(Clone, Copy, Debug)]
struct StepEdgeHint {
    change_id: Option<usize>,
    edge: StepEdge,
    until: Instant,
}

#[derive(Clone, Copy, Debug)]
enum HunkEdge {
    First,
    Last,
}

#[derive(Clone, Copy, Debug)]
struct HunkEdgeHint {
    edge: HunkEdge,
    until: Instant,
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
    /// True when autoplay is running in reverse
    pub autoplay_reverse: bool,
    /// Current scroll offset
    pub scroll_offset: usize,
    /// Per-file scroll offsets when stepping
    scroll_offsets_step: Vec<usize>,
    /// Per-file scroll offsets when not stepping
    scroll_offsets_no_step: Vec<usize>,
    /// Tracks if a file has a saved no-step scroll position
    no_step_visited: Vec<bool>,
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
    /// File panel width (in columns)
    pub file_panel_width: u16,
    /// File panel full area (x, y, width, height)
    pub file_panel_rect: Option<(u16, u16, u16, u16)>,
    /// True when dragging the file panel separator
    pub file_panel_resizing: bool,
    /// File list scroll offset
    pub file_list_scroll: usize,
    /// File list view area (x, y, width, height)
    pub file_list_area: Option<(u16, u16, u16, u16)>,
    /// File list row mapping for mouse selection
    pub file_list_rows: Vec<Option<usize>>,
    /// File list filter input area (x, y, width, height)
    pub file_filter_area: Option<(u16, u16, u16, u16)>,
    /// When to show per-file +/- counts in the file panel
    pub file_count_mode: FileCountMode,
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
    /// Current scroll offset for help popover
    pub help_scroll: usize,
    /// Max scroll for help popover (computed during render)
    pub help_max_scroll: usize,
    /// Git branch name (if in a git repo)
    pub git_branch: Option<String>,
    /// Auto-center on active change after stepping (like vim's zz)
    pub auto_center: bool,
    /// Show top bar in diff view
    pub topbar: bool,
    /// Animation duration in milliseconds (how long fade effects take)
    pub animation_duration: u64,
    /// Pending count for vim-style commands (e.g., 10j = scroll down 10 lines)
    pub pending_count: Option<usize>,
    /// Pending "g" prefix for vim-style commands (e.g., gg)
    pub pending_g_prefix: bool,
    /// Horizontal scroll offset (for long lines)
    pub horizontal_scroll: usize,
    /// Per-file horizontal scroll offsets when stepping
    horizontal_scrolls_step: Vec<usize>,
    /// Per-file horizontal scroll offsets when not stepping
    horizontal_scrolls_no_step: Vec<usize>,
    /// Cached max line width per file (stepping)
    max_line_widths_step: Vec<usize>,
    /// Cached max line width per file (no-step)
    max_line_widths_no_step: Vec<usize>,
    /// Line wrap mode (when true, horizontal scroll is ignored)
    pub line_wrap: bool,
    /// Cached wrapped display length (for line wrap centering)
    last_wrap_display_len: Option<usize>,
    /// Cached wrapped active display index (for line wrap centering)
    last_wrap_active_idx: Option<usize>,
    /// Show scrollbar
    pub scrollbar_visible: bool,
    /// Show strikethrough on deleted text
    pub strikethrough_deletions: bool,
    /// Show +/- sign column in the gutter (single/evolution)
    pub gutter_signs: bool,
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
    /// Auto-jump to first hunk when entering a file in no-step mode
    pub no_step_auto_jump_on_enter: bool,
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
    /// Whether the UI theme is in light mode
    pub theme_is_light: bool,
    /// Whether stepping is enabled (false = no-step diff view)
    pub stepping: bool,
    /// Wrap hunk navigation across ends (h/l at edges wrap to first/last hunk)
    pub hunk_wrap: HunkWrapMode,
    /// Wrap stepping across files (j at end goes to next file, k at start goes to previous file)
    pub step_wrap: StepWrapMode,
    /// Diff background (full-line) toggle
    pub diff_bg: bool,
    /// Diff foreground rendering mode
    pub diff_fg: DiffForegroundMode,
    /// Inline diff highlight mode
    pub diff_highlight: DiffHighlightMode,
    /// Diff extent marker color mode
    pub diff_extent_marker: DiffExtentMarkerMode,
    /// Diff extent marker scope
    pub diff_extent_marker_scope: DiffExtentMarkerScope,
    /// Single-pane modified line render mode while stepping
    pub single_modified_step_mode: ModifiedStepMode,
    /// Keep split panes vertically aligned by inserting blank rows
    pub split_align_lines: bool,
    /// Fill character for aligned blank rows in split view
    pub split_align_fill: String,
    /// Syntax scope in evolution view
    pub evo_syntax: crate::config::EvoSyntaxMode,
    /// Syntax highlighting mode
    pub syntax_mode: SyntaxMode,
    /// Syntax theme selection
    pub syntax_theme: String,
    /// Syntax highlighter (lazy initialized)
    syntax_engine: Option<SyntaxEngine>,
    /// Per-file syntax cache (old/new spans)
    syntax_caches: Vec<Option<SyntaxCache>>,
    /// Show syntax scope debug label in the status bar
    show_syntax_scopes: bool,
    /// Cached syntax scope label for the active line
    syntax_scope_cache: Option<SyntaxScopeCache>,
    /// Peek old/new state (stepping-only)
    peek_state: Option<PeekState>,
    /// Saved peek state for stepping mode (when toggled off)
    step_peek_state: Option<PeekState>,
    /// Saved step state per file (to restore after toggling off)
    step_state_snapshots: Vec<Option<StepState>>,
    /// Saved no-step cursor/marker state per file
    no_step_state_snapshots: Vec<Option<NoStepState>>,
    /// View mode to restore when stepping is enabled
    step_view_mode: ViewMode,
    /// Search query (diff pane)
    search_query: String,
    /// True when search input is active
    search_active: bool,
    /// Last matched display index for search navigation
    search_last_target: Option<usize>,
    /// Pending scroll to a search target
    needs_scroll_to_search: bool,
    /// Target display index for search scrolling
    search_target: Option<usize>,
    /// Cached search regex (case-insensitive)
    search_regex: Option<Regex>,
    /// Goto query (":" command)
    goto_query: String,
    /// True when goto input is active
    goto_active: bool,
    /// Snap animation frame when animations are disabled
    snap_frame: Option<AnimationFrame>,
    /// Start time of the current snap frame
    snap_frame_started_at: Option<Instant>,
    /// Remaining steps for limited autoplay (replay)
    autoplay_remaining: Option<usize>,
    /// Edge-of-steps hint (shown briefly after trying to step past ends)
    step_edge_hint: Option<StepEdgeHint>,
    /// Edge-of-hunks hint (shown briefly after trying to go past ends)
    hunk_edge_hint: Option<HunkEdgeHint>,
    /// Last known viewport height for the diff area
    pub last_viewport_height: usize,
}

const SNAP_PHASE_MS: u64 = 50;
const STEP_EDGE_HINT_MS: u64 = 700;

/// Pure helper: determine if overscroll should be allowed
fn allow_overscroll_state(
    auto_center: bool,
    needs_scroll_to_active: bool,
    centered_once: bool,
) -> bool {
    (auto_center && needs_scroll_to_active) || centered_once
}

/// Pure helper: compute max scroll offset
fn max_scroll(total_lines: usize, viewport_height: usize, allow_overscroll: bool) -> usize {
    if allow_overscroll {
        // Allow last line to be centered: enough space for half viewport below
        total_lines
            .saturating_sub(1)
            .saturating_sub(viewport_height / 2)
    } else {
        total_lines.saturating_sub(viewport_height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeekScope {
    Change,
    Hunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeekMode {
    Old,
    Modified,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeekState {
    pub scope: PeekScope,
    pub mode: PeekMode,
}

#[derive(Debug, Clone)]
struct SyntaxScopeCache {
    file_index: usize,
    side: SyntaxSide,
    line_num: usize,
    label: String,
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
            autoplay_reverse: false,
            scroll_offset: 0,
            scroll_offsets_step: vec![0; file_count],
            scroll_offsets_no_step: vec![0; file_count],
            no_step_visited: vec![false; file_count],
            files_visited: vec![false; file_count],
            should_quit: false,
            animation_phase: AnimationPhase::Idle,
            animation_progress: 1.0,
            last_animation_tick: Instant::now(),
            last_autoplay_tick: Instant::now(),
            file_list_focused: false,
            file_panel_visible: true,
            file_panel_width: 30,
            file_panel_rect: None,
            file_panel_resizing: false,
            file_list_scroll: 0,
            file_list_area: None,
            file_list_rows: Vec::new(),
            file_filter_area: None,
            file_count_mode: FileCountMode::Active,
            file_filter: String::new(),
            file_filter_active: false,
            animation_enabled: false,
            zen_mode: false,
            needs_scroll_to_active: true, // Scroll to first change on startup
            show_help: false,
            help_scroll: 0,
            help_max_scroll: 0,
            git_branch,
            auto_center: true,
            topbar: true,
            animation_duration: 150,
            pending_count: None,
            pending_g_prefix: false,
            horizontal_scroll: 0,
            horizontal_scrolls_step: vec![0; file_count],
            horizontal_scrolls_no_step: vec![0; file_count],
            max_line_widths_step: vec![0; file_count],
            max_line_widths_no_step: vec![0; file_count],
            line_wrap: false,
            last_wrap_display_len: None,
            last_wrap_active_idx: None,
            scrollbar_visible: false,
            strikethrough_deletions: false,
            gutter_signs: true,
            file_panel_manually_set: false,
            show_path_popup: false,
            file_panel_auto_hidden: false,
            auto_step_on_enter: true,
            auto_step_blank_files: true,
            no_step_auto_jump_on_enter: true,
            centered_once: false,
            primary_marker: "▶".to_string(),
            primary_marker_right: "◀".to_string(),
            extent_marker: "▌".to_string(),
            extent_marker_right: "▐".to_string(),
            clear_active_on_next_render: false,
            theme: ResolvedTheme::default(),
            theme_is_light: false,
            stepping: true,
            hunk_wrap: HunkWrapMode::None,
            step_wrap: StepWrapMode::None,
            diff_bg: false,
            diff_fg: DiffForegroundMode::Theme,
            diff_highlight: DiffHighlightMode::Text,
            diff_extent_marker: DiffExtentMarkerMode::Neutral,
            diff_extent_marker_scope: DiffExtentMarkerScope::Progress,
            single_modified_step_mode: ModifiedStepMode::Mixed,
            split_align_lines: false,
            split_align_fill: "╱".to_string(),
            evo_syntax: crate::config::EvoSyntaxMode::Context,
            syntax_mode: SyntaxMode::On,
            syntax_theme: "ansi".to_string(),
            syntax_engine: None,
            syntax_caches: vec![None; file_count],
            show_syntax_scopes: false,
            syntax_scope_cache: None,
            peek_state: None,
            step_peek_state: None,
            step_state_snapshots: vec![None; file_count],
            no_step_state_snapshots: vec![None; file_count],
            step_view_mode: view_mode,
            search_query: String::new(),
            search_active: false,
            search_last_target: None,
            needs_scroll_to_search: false,
            search_target: None,
            search_regex: None,
            goto_query: String::new(),
            goto_active: false,
            snap_frame: None,
            snap_frame_started_at: None,
            autoplay_remaining: None,
            step_edge_hint: None,
            hunk_edge_hint: None,
            last_viewport_height: 0,
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
        if self.show_help {
            self.help_scroll = 0;
        }
    }

    pub fn toggle_path_popup(&mut self) {
        self.show_path_popup = !self.show_path_popup;
    }

    pub fn toggle_syntax(&mut self) {
        self.syntax_mode = match self.syntax_mode {
            SyntaxMode::On => SyntaxMode::Off,
            SyntaxMode::Off => SyntaxMode::On,
        };
        if matches!(self.syntax_mode, SyntaxMode::Off) {
            self.syntax_engine = None;
            self.syntax_caches = vec![None; self.multi_diff.file_count()];
        }
    }

    pub fn toggle_evo_syntax(&mut self) {
        self.evo_syntax = match self.evo_syntax {
            crate::config::EvoSyntaxMode::Context => crate::config::EvoSyntaxMode::Full,
            crate::config::EvoSyntaxMode::Full => crate::config::EvoSyntaxMode::Context,
        };
    }

    pub fn toggle_peek_old_change(&mut self) {
        self.cycle_peek_change();
    }

    pub fn toggle_peek_old_hunk(&mut self) {
        self.toggle_peek_hunk();
    }

    fn clear_peek(&mut self) {
        self.peek_state = None;
    }

    fn cycle_peek_change(&mut self) {
        if !self.stepping {
            return;
        }
        let base = self.base_modified_view_mode();
        let current = match self.peek_state {
            Some(PeekState {
                scope: PeekScope::Change,
                mode,
            }) => mode,
            _ => base,
        };
        let next = match current {
            PeekMode::Modified => PeekMode::Old,
            PeekMode::Old => PeekMode::Mixed,
            PeekMode::Mixed => PeekMode::Modified,
        };
        if next == base {
            self.peek_state = None;
        } else {
            self.peek_state = Some(PeekState {
                scope: PeekScope::Change,
                mode: next,
            });
        }
    }

    fn toggle_peek_hunk(&mut self) {
        if !self.stepping {
            return;
        }
        let next = PeekState {
            scope: PeekScope::Hunk,
            mode: PeekMode::Old,
        };
        if self.peek_state == Some(next) {
            self.peek_state = None;
        } else {
            self.peek_state = Some(next);
        }
    }

    fn base_modified_view_mode(&self) -> PeekMode {
        if self.single_modified_step_mode == ModifiedStepMode::Modified {
            PeekMode::Modified
        } else {
            PeekMode::Mixed
        }
    }

    pub fn is_peek_override_for_line(&mut self, view_line: &ViewLine) -> bool {
        if !self.stepping {
            return false;
        }
        let Some(peek) = self.peek_state else {
            return false;
        };
        match peek.scope {
            PeekScope::Change => view_line.is_primary_active,
            PeekScope::Hunk => {
                let current_hunk = self.multi_diff.current_navigator().state().current_hunk;
                view_line.hunk_index == Some(current_hunk)
            }
        }
    }

    pub fn peek_mode_for_line(&mut self, view_line: &ViewLine) -> Option<PeekMode> {
        if !self.stepping {
            return None;
        }
        if let Some(peek) = self.peek_state {
            match peek.scope {
                PeekScope::Change => {
                    if view_line.is_primary_active {
                        return Some(peek.mode);
                    }
                }
                PeekScope::Hunk => {
                    let current_hunk = self.multi_diff.current_navigator().state().current_hunk;
                    if view_line.hunk_index == Some(current_hunk) {
                        return Some(PeekMode::Old);
                    }
                }
            }
            return None;
        }
        if view_line.is_primary_active
            && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
        {
            return Some(self.base_modified_view_mode());
        }
        None
    }

    pub fn start_search(&mut self) {
        self.search_active = true;
        self.search_query.clear();
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.search_regex = None;
    }

    pub fn stop_search(&mut self) {
        self.search_active = false;
    }

    pub fn clear_search(&mut self) {
        self.search_active = false;
        self.search_query.clear();
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.search_regex = None;
    }

    pub fn clear_search_text(&mut self) {
        self.search_query.clear();
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.search_regex = None;
    }

    pub fn start_goto(&mut self) {
        self.goto_active = true;
        self.goto_query.clear();
    }

    pub fn clear_goto(&mut self) {
        self.goto_active = false;
        self.goto_query.clear();
    }

    pub fn clear_goto_text(&mut self) {
        self.goto_query.clear();
    }

    pub fn push_goto_char(&mut self, ch: char) {
        self.goto_query.push(ch);
    }

    pub fn pop_goto_char(&mut self) {
        self.goto_query.pop();
    }

    pub fn goto_active(&self) -> bool {
        self.goto_active
    }

    pub fn goto_query(&self) -> &str {
        &self.goto_query
    }

    pub fn push_search_char(&mut self, ch: char) {
        self.search_query.push(ch);
        self.search_last_target = None;
        self.update_search_regex();
    }

    pub fn pop_search_char(&mut self) {
        self.search_query.pop();
        self.search_last_target = None;
        self.update_search_regex();
    }

    fn reset_search_for_file_switch(&mut self) {
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
    }

    pub fn search_active(&self) -> bool {
        self.search_active
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    fn update_search_regex(&mut self) {
        let query = self.search_query.trim();
        if query.is_empty() {
            self.search_regex = None;
            return;
        }
        let regex = RegexBuilder::new(query)
            .case_insensitive(true)
            .build()
            .or_else(|_| {
                RegexBuilder::new(&regex::escape(query))
                    .case_insensitive(true)
                    .build()
            })
            .ok();
        self.search_regex = regex;
    }

    pub fn search_target(&self) -> Option<usize> {
        self.search_target
    }

    pub fn search_next(&mut self) {
        let matches = self.collect_search_matches();
        if matches.is_empty() {
            return;
        }
        let start = self.search_last_target.unwrap_or(self.scroll_offset);
        let target = matches
            .iter()
            .copied()
            .find(|idx| *idx > start)
            .unwrap_or(matches[0]);
        self.search_last_target = Some(target);
        self.search_target = Some(target);
        self.needs_scroll_to_search = true;
    }

    pub fn search_prev(&mut self) {
        let matches = self.collect_search_matches();
        if matches.is_empty() {
            return;
        }
        let start = self.search_last_target.unwrap_or(self.scroll_offset);
        let target = matches
            .iter()
            .copied()
            .rev()
            .find(|idx| *idx < start)
            .unwrap_or(*matches.last().unwrap());
        self.search_last_target = Some(target);
        self.search_target = Some(target);
        self.needs_scroll_to_search = true;
    }

    pub fn apply_goto(&mut self) {
        let query = self.goto_query.trim();
        if query.is_empty() {
            return;
        }

        let mut chars = query.chars();
        let first = match chars.next() {
            Some(ch) => ch,
            None => return,
        };

        match first {
            'h' | 'H' => {
                let rest = chars
                    .as_str()
                    .trim_start_matches(|c: char| c == ':' || c.is_whitespace());
                if let Ok(num) = rest.parse::<usize>() {
                    self.goto_hunk_number(num);
                }
            }
            's' | 'S' => {
                let rest = chars
                    .as_str()
                    .trim_start_matches(|c: char| c == ':' || c.is_whitespace());
                if let Ok(num) = rest.parse::<usize>() {
                    self.goto_step_number(num);
                }
            }
            _ => {
                if query.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(num) = query.parse::<usize>() {
                        self.goto_line_number(num);
                    }
                }
            }
        }
    }

    pub fn highlight_search_spans(
        &self,
        spans: Vec<Span<'static>>,
        text: &str,
        is_active: bool,
    ) -> Vec<Span<'static>> {
        let Some(regex) = self.search_regex.as_ref() else {
            return spans;
        };
        let ranges = match_ranges(text, regex);
        if ranges.is_empty() {
            return spans;
        }
        let highlight_bg = if is_active {
            self.theme.accent
        } else {
            color::dim_color(self.theme.accent)
        };
        let highlight_fg = if is_active {
            self.search_highlight_fg(highlight_bg)
        } else {
            None
        };
        apply_highlight_spans(spans, &ranges, highlight_bg, highlight_fg)
    }

    pub fn yank_current_change(&mut self) {
        let frame = self.animation_frame();
        let view_lines = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(frame);
        let Some(line) = view_lines.iter().find(|line| line.is_primary_active) else {
            return;
        };
        if let Some(text) = self.text_for_yank(line) {
            copy_to_clipboard(&text);
        }
    }

    fn search_highlight_fg(&self, bg: Color) -> Option<Color> {
        let text = self.theme.text;
        let mut best_color = text;
        let mut best_ratio = color::contrast_ratio(bg, text).unwrap_or(0.0);
        if let Some(bg_color) = self.theme.background {
            let ratio = color::contrast_ratio(bg, bg_color).unwrap_or(0.0);
            if ratio > best_ratio {
                best_ratio = ratio;
                best_color = bg_color;
            }
        }
        if best_ratio == 0.0 {
            None
        } else {
            Some(best_color)
        }
    }

    pub fn yank_current_hunk(&mut self) {
        let frame = self.animation_frame();
        let view_lines = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(frame);
        let current_hunk = self.multi_diff.current_navigator().state().current_hunk;
        let mut lines: Vec<String> = Vec::new();
        for line in view_lines
            .iter()
            .filter(|line| line.hunk_index == Some(current_hunk))
        {
            if let Some(text) = self.text_for_yank(line) {
                lines.push(text);
            }
        }
        if lines.is_empty() {
            return;
        }
        copy_to_clipboard(&lines.join("\n"));
    }

    fn text_for_yank(&mut self, view_line: &ViewLine) -> Option<String> {
        if let Some(mode) = self.peek_mode_for_line(view_line) {
            match mode {
                PeekMode::Old => {
                    if let Some(text) = self.peek_text_for_line(view_line) {
                        return Some(text);
                    }
                }
                PeekMode::Modified => {
                    if let Some(text) = self.modified_only_text_for_line(view_line) {
                        return Some(text);
                    }
                }
                PeekMode::Mixed => {
                    let change = self
                        .multi_diff
                        .current_navigator()
                        .diff()
                        .changes
                        .get(view_line.change_id);
                    if let Some(change) = change {
                        let text = inline_text_for_change(change);
                        if !text.is_empty() {
                            return Some(text);
                        }
                    }
                }
            }
        }
        Some(view_line.content.clone())
    }

    fn peek_text_for_line(&mut self, view_line: &ViewLine) -> Option<String> {
        if !matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify) {
            return None;
        }
        let change = self
            .multi_diff
            .current_navigator()
            .diff()
            .changes
            .get(view_line.change_id)?;
        let text = old_text_for_change(change);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn modified_only_text_for_line(&mut self, view_line: &ViewLine) -> Option<String> {
        if !matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify) {
            return None;
        }
        let change = self
            .multi_diff
            .current_navigator()
            .diff()
            .changes
            .get(view_line.change_id)?;
        let text = modified_only_text_for_change(change);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn collect_search_matches(&mut self) -> Vec<usize> {
        let regex = match self.search_regex.as_ref() {
            Some(regex) => regex.clone(),
            None => return Vec::new(),
        };
        let frame = self.animation_frame();
        let view = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(frame);
        let mut matches = Vec::new();

        match self.view_mode {
            ViewMode::SinglePane => {
                for (display_idx, line) in view.iter().enumerate() {
                    let text = self.search_text_single(line);
                    if line_has_query(&text, &regex) {
                        matches.push(display_idx);
                    }
                }
            }
            ViewMode::Evolution => {
                let mut display_idx = 0usize;
                for line in &view {
                    let visible = match line.kind {
                        LineKind::Deleted => false,
                        LineKind::PendingDelete => {
                            line.is_active && self.animation_phase != AnimationPhase::Idle
                        }
                        _ => true,
                    };
                    if !visible {
                        continue;
                    }
                    let text = self.search_text_single(line);
                    if line_has_query(&text, &regex) {
                        matches.push(display_idx);
                    }
                    display_idx += 1;
                }
            }
            ViewMode::Split => {
                let mut old_idx = 0usize;
                let mut new_idx = 0usize;
                for line in &view {
                    if line.old_line.is_some() {
                        if let Some(text) = self.search_text_split_old(line) {
                            if line_has_query(&text, &regex) {
                                matches.push(old_idx);
                            }
                        }
                        old_idx += 1;
                    }
                    if line.new_line.is_some()
                        && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                    {
                        if let Some(text) = self.search_text_split_new(line) {
                            if line_has_query(&text, &regex) {
                                matches.push(new_idx);
                            }
                        }
                        new_idx += 1;
                    }
                }
            }
        }

        matches.sort_unstable();
        matches
    }

    fn search_text_single(&mut self, view_line: &ViewLine) -> String {
        if let Some(mode) = self.peek_mode_for_line(view_line) {
            match mode {
                PeekMode::Old => {
                    if let Some(text) = self.peek_text_for_line(view_line) {
                        return text;
                    }
                }
                PeekMode::Modified => {
                    if let Some(text) = self.modified_only_text_for_line(view_line) {
                        return text;
                    }
                }
                PeekMode::Mixed => {
                    if let Some(change) = self
                        .multi_diff
                        .current_navigator()
                        .diff()
                        .changes
                        .get(view_line.change_id)
                    {
                        let text = inline_text_for_change(change);
                        if !text.is_empty() {
                            return text;
                        }
                    }
                }
            }
        }
        if !self.stepping && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
        {
            if let Some(change) = self
                .multi_diff
                .current_navigator()
                .diff()
                .changes
                .get(view_line.change_id)
            {
                let text = inline_text_for_change(change);
                if !text.is_empty() {
                    return text;
                }
            }
        }
        view_line.content.clone()
    }

    fn search_text_split_old(&mut self, view_line: &ViewLine) -> Option<String> {
        view_line.old_line?;
        if matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify) {
            if let Some(change) = self
                .multi_diff
                .current_navigator()
                .diff()
                .changes
                .get(view_line.change_id)
            {
                let text = old_text_for_change(change);
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        Some(view_line.content.clone())
    }

    fn search_text_split_new(&mut self, view_line: &ViewLine) -> Option<String> {
        view_line.new_line?;
        Some(view_line.content.clone())
    }

    pub fn state(&mut self) -> StepState {
        self.multi_diff.current_navigator().state().clone()
    }

    pub fn syntax_enabled(&self) -> bool {
        match self.syntax_mode {
            SyntaxMode::On => true,
            SyntaxMode::Off => false,
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
        let file_name = self.current_file_path();
        let nav = self.multi_diff.current_navigator();
        let content = match side {
            SyntaxSide::Old => nav.old_content(),
            SyntaxSide::New => nav.new_content(),
        };
        if self.syntax_engine.is_none() {
            self.syntax_engine = Some(SyntaxEngine::new(&self.syntax_theme, self.theme_is_light));
        }
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
                self.syntax_engine =
                    Some(SyntaxEngine::new(&self.syntax_theme, self.theme_is_light));
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

    fn syntax_line_for_display(
        &self,
        view: &[ViewLine],
        display_idx: usize,
    ) -> Option<(SyntaxSide, usize)> {
        match self.view_mode {
            ViewMode::SinglePane => view.get(display_idx).and_then(|line| {
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
                let mut old_count = 0usize;
                let mut new_count = 0usize;
                let mut old_line = None;
                let mut new_line = None;

                for line in view {
                    if line.old_line.is_some() {
                        if old_count == display_idx {
                            old_line = line.old_line;
                        }
                        old_count += 1;
                    }
                    if line.new_line.is_some() {
                        if new_count == display_idx {
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
        }
    }

    pub fn next_step(&mut self) {
        self.step_forward();
    }

    pub fn prev_step(&mut self) {
        self.step_backward();
    }

    pub fn replay_step(&mut self) {
        if !self.stepping {
            return;
        }
        let count = self.take_count();
        if self.animation_phase != AnimationPhase::Idle || self.snap_frame.is_some() {
            return;
        }
        let current_step = self.multi_diff.current_navigator().state().current_step;
        if current_step == 0 {
            return;
        }
        let back_steps = count.min(current_step);
        if back_steps == 0 {
            return;
        }
        self.autoplay = false;
        self.autoplay_remaining = None;
        let target_step = current_step.saturating_sub(back_steps);
        self.clear_peek();
        self.snap_frame = None;
        self.snap_frame_started_at = None;
        self.clear_active_on_next_render = false;
        self.multi_diff.current_navigator().goto(target_step);
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
        self.autoplay = true;
        self.autoplay_reverse = false;
        self.autoplay_remaining = Some(back_steps);
        self.last_autoplay_tick = Instant::now();
    }

    fn clear_step_edge_hint(&mut self) {
        self.step_edge_hint = None;
    }

    fn clear_hunk_edge_hint(&mut self) {
        self.hunk_edge_hint = None;
    }

    pub(crate) fn step_edge_hint_for_change(&self, change_id: usize) -> Option<&'static str> {
        let hint = self.step_edge_hint?;
        if Instant::now() > hint.until {
            return None;
        }
        if hint.change_id == Some(change_id) {
            Some(match hint.edge {
                StepEdge::Start => "... start",
                StepEdge::End => "... end",
            })
        } else {
            None
        }
    }

    pub(crate) fn hunk_edge_hint_text(&self) -> Option<&'static str> {
        let hint = self.hunk_edge_hint?;
        if Instant::now() > hint.until {
            return None;
        }
        Some(match hint.edge {
            HunkEdge::First => "first hunk",
            HunkEdge::Last => "last hunk",
        })
    }

    pub(crate) fn last_step_hint_text(&mut self) -> Option<&'static str> {
        if !self.stepping {
            return None;
        }
        let state = self.multi_diff.current_navigator().state();
        if state.total_steps < 2 {
            return None;
        }
        let remaining = state
            .total_steps
            .saturating_sub(1)
            .saturating_sub(state.current_step);
        if remaining != 1 {
            return None;
        }
        Some("last step next")
    }

    fn trigger_hunk_edge_hint(&mut self, edge: HunkEdge) {
        self.hunk_edge_hint = Some(HunkEdgeHint {
            edge,
            until: Instant::now() + Duration::from_millis(STEP_EDGE_HINT_MS),
        });
    }

    pub(crate) fn hunk_hint_overflow(
        &mut self,
        hunk_idx: usize,
        viewport_height: usize,
    ) -> Option<(bool, bool)> {
        let bounds = match self.view_mode {
            ViewMode::Split => {
                let (old_bounds, new_bounds) = self.compute_hunk_bounds_split();
                let old = old_bounds.get(hunk_idx).copied().flatten();
                let new = new_bounds.get(hunk_idx).copied().flatten();
                self.pick_split_bounds(old, new)
            }
            _ => self
                .compute_hunk_bounds_single()
                .get(hunk_idx)
                .copied()
                .flatten(),
        }?;

        let visible_start = self.scroll_offset;
        let visible_end = self
            .scroll_offset
            .saturating_add(viewport_height.saturating_sub(1));
        let overflow_above = bounds.start.idx < visible_start;
        let overflow_below = bounds.end.idx > visible_end;
        Some((overflow_above, overflow_below))
    }

    fn trigger_step_edge_hint(&mut self, edge: StepEdge) {
        let state = self.multi_diff.current_navigator().state();
        let change_id = match edge {
            StepEdge::End => state
                .applied_changes
                .last()
                .copied()
                .or(state.active_change),
            StepEdge::Start => state
                .applied_changes
                .first()
                .copied()
                .or(state.active_change),
        };
        self.step_edge_hint = Some(StepEdgeHint {
            change_id,
            edge,
            until: Instant::now() + Duration::from_millis(STEP_EDGE_HINT_MS),
        });
    }

    fn step_forward(&mut self) -> bool {
        self.clear_peek();
        self.clear_hunk_edge_hint();
        self.snap_frame = None;
        self.snap_frame_started_at = None;
        if self.multi_diff.current_navigator().next() {
            self.clear_step_edge_hint();
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
            true
        } else {
            match self.step_wrap {
                StepWrapMode::File => {
                    if self.next_file_wrapped() {
                        self.goto_first_step();
                        return true;
                    }
                }
                StepWrapMode::Step => {
                    self.goto_first_step();
                    return true;
                }
                StepWrapMode::None => {}
            }
            self.trigger_step_edge_hint(StepEdge::End);
            false
        }
    }

    fn step_backward(&mut self) -> bool {
        self.clear_peek();
        self.clear_hunk_edge_hint();
        self.snap_frame = None;
        self.snap_frame_started_at = None;
        if !self.animation_enabled {
            self.snap_frame = Some(AnimationFrame::FadeOut);
            self.snap_frame_started_at = Some(Instant::now());
            self.clear_active_on_next_render = false;
        }
        if self.multi_diff.current_navigator().prev() {
            self.clear_step_edge_hint();
            if self.animation_enabled {
                self.start_animation();
            } else if self.snap_frame.is_none() {
                self.clear_active_on_next_render = true;
            }
            self.needs_scroll_to_active = true;
            true
        } else {
            match self.step_wrap {
                StepWrapMode::File => {
                    if self.prev_file_wrapped() {
                        self.goto_last_step();
                        return true;
                    }
                }
                StepWrapMode::Step => {
                    self.goto_last_step();
                    return true;
                }
                StepWrapMode::None => {}
            }
            self.trigger_step_edge_hint(StepEdge::Start);
            false
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
                        bounds[hidx] = Some(HunkBounds { start, end: start });
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
    fn compute_hunk_bounds_split(&mut self) -> (Vec<Option<HunkBounds>>, Vec<Option<HunkBounds>>) {
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
                            old_bounds[hidx] = Some(HunkBounds { start, end: start });
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
                            new_bounds[hidx] = Some(HunkBounds { start, end: start });
                        }
                    }
                }
                new_idx += 1;
            }
        }

        (old_bounds, new_bounds)
    }

    fn pick_split_start(
        &self,
        old: Option<HunkStart>,
        new: Option<HunkStart>,
    ) -> Option<HunkStart> {
        match (old, new) {
            (Some(o), Some(n)) => {
                let old_dist = (o.idx as isize - self.scroll_offset as isize).abs();
                let new_dist = (n.idx as isize - self.scroll_offset as isize).abs();
                if old_dist < new_dist {
                    Some(o)
                } else {
                    Some(n)
                }
            }
            (Some(o), None) => Some(o),
            (None, Some(n)) => Some(n),
            (None, None) => None,
        }
    }

    fn pick_split_index(&self, old: Option<usize>, new: Option<usize>) -> Option<usize> {
        match (old, new) {
            (Some(o), Some(n)) => {
                let old_dist = (o as isize - self.scroll_offset as isize).abs();
                let new_dist = (n as isize - self.scroll_offset as isize).abs();
                if old_dist < new_dist {
                    Some(o)
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

    fn first_hunk_from_starts(&self, starts: &[Option<HunkStart>]) -> Option<(usize, HunkStart)> {
        starts
            .iter()
            .enumerate()
            .find_map(|(idx, start)| start.map(|s| (idx, s)))
    }

    fn last_hunk_from_starts(&self, starts: &[Option<HunkStart>]) -> Option<(usize, HunkStart)> {
        starts
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, start)| start.map(|s| (idx, s)))
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

    fn prev_hunk_from_starts(&self, starts: &[Option<HunkStart>]) -> Option<(usize, HunkStart)> {
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
        bounds.iter().enumerate().rev().find_map(|(idx, bound)| {
            bound.and_then(|b| (b.start.idx <= self.scroll_offset).then_some(idx))
        })
    }

    fn first_available_hunk(bounds: &[Option<HunkBounds>]) -> Option<(usize, HunkBounds)> {
        bounds
            .iter()
            .enumerate()
            .find_map(|(idx, bound)| bound.map(|b| (idx, b)))
    }

    fn set_cursor_for_current_scroll(&mut self) {
        let view = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(AnimationFrame::Idle);
        let mut display_idx = 0usize;
        let mut cursor_line = None;

        for line in &view {
            let visible = match self.view_mode {
                ViewMode::Evolution => {
                    !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                }
                _ => true,
            };
            if !visible {
                continue;
            }
            if display_idx >= self.scroll_offset {
                cursor_line = Some(line);
                break;
            }
            display_idx += 1;
        }

        if let Some(line) = cursor_line {
            if let Some(hidx) = line.hunk_index {
                self.multi_diff
                    .current_navigator()
                    .set_cursor_hunk(hidx, Some(line.change_id));
            } else {
                self.multi_diff
                    .current_navigator()
                    .set_cursor_change(Some(line.change_id));
            }
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
        let in_hunk_scope = self
            .multi_diff
            .current_navigator()
            .state()
            .last_nav_was_hunk;
        let use_cursor = auto_center && cursor_set && in_hunk_scope;
        let inclusive = in_hunk_scope;
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_starts, new_starts) = self.compute_hunk_starts_split();
                let effective: Vec<Option<HunkStart>> = old_starts
                    .into_iter()
                    .zip(new_starts)
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
                if target.is_none() && matches!(self.hunk_wrap, HunkWrapMode::Hunk) {
                    target = self.first_hunk_from_starts(&effective);
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
                if target.is_none() && matches!(self.hunk_wrap, HunkWrapMode::Hunk) {
                    target = self.first_hunk_from_starts(&hunk_starts);
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
            self.clear_hunk_edge_hint();
        } else if matches!(self.hunk_wrap, HunkWrapMode::File) {
            if self.wrap_to_file_hunk(true, false) {
                self.clear_hunk_edge_hint();
            } else {
                self.trigger_hunk_edge_hint(HunkEdge::Last);
            }
        } else {
            self.trigger_hunk_edge_hint(HunkEdge::Last);
        }
    }

    /// Scroll to the previous hunk (no-step mode)
    pub fn prev_hunk_scroll(&mut self) {
        let auto_center = self.auto_center;
        let (current_hunk, cursor_set) = {
            let state = self.multi_diff.current_navigator().state();
            (state.current_hunk, state.cursor_change.is_some())
        };
        let in_hunk_scope = self
            .multi_diff
            .current_navigator()
            .state()
            .last_nav_was_hunk;
        let use_cursor = auto_center && cursor_set && in_hunk_scope;
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_starts, new_starts) = self.compute_hunk_starts_split();
                let effective: Vec<Option<HunkStart>> = old_starts
                    .into_iter()
                    .zip(new_starts)
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
                if target.is_none() && matches!(self.hunk_wrap, HunkWrapMode::Hunk) {
                    target = self.last_hunk_from_starts(&effective);
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
                if target.is_none() && matches!(self.hunk_wrap, HunkWrapMode::Hunk) {
                    target = self.last_hunk_from_starts(&hunk_starts);
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
            self.clear_hunk_edge_hint();
        } else if matches!(self.hunk_wrap, HunkWrapMode::File) {
            if self.wrap_to_file_hunk(false, false) {
                self.clear_hunk_edge_hint();
            } else {
                self.trigger_hunk_edge_hint(HunkEdge::First);
            }
        } else {
            self.trigger_hunk_edge_hint(HunkEdge::First);
        }
    }

    /// Move to the next hunk (group of related changes)
    pub fn next_hunk(&mut self) {
        self.clear_peek();
        if self.multi_diff.current_navigator().next_hunk() {
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
            self.clear_hunk_edge_hint();
        } else {
            match self.hunk_wrap {
                HunkWrapMode::Hunk => {
                    let total = self.multi_diff.current_navigator().state().total_hunks;
                    if total > 0 {
                        self.goto_hunk_index(0);
                        self.clear_hunk_edge_hint();
                    } else {
                        self.trigger_hunk_edge_hint(HunkEdge::Last);
                    }
                }
                HunkWrapMode::File => {
                    if self.wrap_to_file_hunk(true, true) {
                        self.clear_hunk_edge_hint();
                    } else {
                        self.trigger_hunk_edge_hint(HunkEdge::Last);
                    }
                }
                HunkWrapMode::None => {
                    self.trigger_hunk_edge_hint(HunkEdge::Last);
                }
            }
        }
    }

    /// Move to the previous hunk (group of related changes)
    pub fn prev_hunk(&mut self) {
        self.clear_peek();
        if self.multi_diff.current_navigator().prev_hunk() {
            if self.animation_enabled {
                self.start_animation();
            } else {
                self.clear_active_on_next_render = true;
            }
            self.needs_scroll_to_active = true;
            self.clear_hunk_edge_hint();
        } else {
            match self.hunk_wrap {
                HunkWrapMode::Hunk => {
                    let total = self.multi_diff.current_navigator().state().total_hunks;
                    if total > 0 {
                        self.goto_hunk_index(total.saturating_sub(1));
                        self.clear_hunk_edge_hint();
                    } else {
                        self.trigger_hunk_edge_hint(HunkEdge::First);
                    }
                }
                HunkWrapMode::File => {
                    if self.wrap_to_file_hunk(false, true) {
                        self.clear_hunk_edge_hint();
                    } else {
                        self.trigger_hunk_edge_hint(HunkEdge::First);
                    }
                }
                HunkWrapMode::None => {
                    self.trigger_hunk_edge_hint(HunkEdge::First);
                }
            }
        }
    }

    /// Get current hunk info (current hunk index, total hunks)
    pub fn hunk_info(&mut self) -> (usize, usize) {
        let state = self.multi_diff.current_navigator().state();
        (state.current_hunk + 1, state.total_hunks) // 1-indexed for display
    }

    pub fn hunk_step_info(&mut self) -> Option<(usize, usize)> {
        let nav = self.multi_diff.current_navigator();
        let state = nav.state();
        let hunk = nav.current_hunk()?;
        let total = hunk.change_ids.len();
        if total == 0 {
            return None;
        }
        let mut applied = 0usize;
        for id in &hunk.change_ids {
            if state.applied_changes.contains(id) {
                applied += 1;
            }
        }
        Some((applied, total))
    }

    pub fn pending_insert_only_in_current_hunk(&mut self) -> usize {
        let nav = self.multi_diff.current_navigator();
        let state = nav.state();
        let hunk = match nav.current_hunk() {
            Some(hunk) => hunk,
            None => return 0,
        };

        let cursor_id = state
            .cursor_change
            .or(state.active_change)
            .or_else(|| state.applied_changes.last().copied());
        let cursor_id = match cursor_id {
            Some(id) => id,
            None => return 0,
        };
        let cursor_idx = match hunk.change_ids.iter().position(|id| *id == cursor_id) {
            Some(idx) => idx,
            None => return 0,
        };
        let get_change = |id| nav.diff().changes.iter().find(|c| c.id == id);
        let is_insert_only = |change: &oyo_core::Change| {
            change
                .spans
                .iter()
                .all(|span| span.kind == ChangeKind::Insert)
        };
        let cursor_change = match get_change(cursor_id) {
            Some(change) => change,
            None => return 0,
        };
        if !is_insert_only(cursor_change) {
            return 0;
        }

        let mut pending = 0usize;
        for change_id in hunk.change_ids.iter().skip(cursor_idx + 1) {
            let change = match get_change(*change_id) {
                Some(change) => change,
                None => continue,
            };
            if !is_insert_only(change) {
                break;
            }
            if state.applied_changes.contains(change_id) {
                continue;
            }
            pending += 1;
        }

        pending
    }

    /// Jump to first change of current hunk
    pub fn goto_hunk_start(&mut self) {
        self.clear_peek();
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
                    .zip(new_bounds)
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
        self.clear_peek();
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
                    .zip(new_bounds)
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

        self.peek_state = None;
        self.multi_diff.current_navigator().goto_end();
        self.multi_diff.current_navigator().clear_active_change();
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.needs_scroll_to_active = false;
        let index = self.multi_diff.selected_index;
        if !self.restore_no_step_state_snapshot(index) {
            if self.no_step_auto_jump_on_enter && !self.no_step_visited[index] {
                self.goto_hunk_index_scroll(0);
            } else {
                self.set_cursor_for_current_scroll();
                self.multi_diff.current_navigator().set_hunk_scope(false);
            }
        }
        self.no_step_visited[index] = true;
    }

    pub fn toggle_stepping(&mut self) {
        let current_index = self.multi_diff.selected_index;
        if self.stepping {
            // Turning OFF stepping: snapshot state and scroll, then enter no-step.
            self.save_scroll_position_for(current_index);
            self.save_step_state_snapshot(current_index);
            self.step_peek_state = self.peek_state.take();
            self.step_view_mode = self.view_mode;
            self.stepping = false;
            self.clear_step_edge_hint();
            self.clear_hunk_edge_hint();
            if !self.no_step_visited[current_index] {
                self.scroll_offsets_no_step[current_index] = self.scroll_offset;
                self.horizontal_scrolls_no_step[current_index] = self.horizontal_scroll;
            }
            self.restore_scroll_position_for(current_index);
            self.enter_no_step_mode();
        } else {
            // Turning ON stepping: restore snapshot and scroll.
            self.save_no_step_state_snapshot(current_index);
            self.save_scroll_position_for(current_index);
            self.stepping = true;
            self.clear_step_edge_hint();
            self.clear_hunk_edge_hint();
            self.peek_state = self.step_peek_state.take();
            self.view_mode = self.step_view_mode;
            if !self.restore_step_state_snapshot(current_index) {
                self.goto_start();
            }
            self.restore_scroll_position_for(current_index);
            self.animation_phase = AnimationPhase::Idle;
            self.animation_progress = 1.0;
            self.needs_scroll_to_active = false;
        }
    }

    pub fn toggle_animation(&mut self) {
        self.animation_enabled = !self.animation_enabled;
    }

    pub fn toggle_zen(&mut self) {
        self.zen_mode = !self.zen_mode;
    }

    pub fn help_scroll_up(&mut self) {
        self.help_scroll = self.help_scroll.saturating_sub(1);
    }

    pub fn help_scroll_down(&mut self) {
        if self.help_scroll < self.help_max_scroll {
            self.help_scroll += 1;
        }
    }

    pub fn handle_file_list_click(&mut self, column: u16, row: u16) -> bool {
        if let Some((x, y, width, height)) = self.file_filter_area {
            let end_x = x.saturating_add(width);
            let end_y = y.saturating_add(height);
            if column >= x && column < end_x && row >= y && row < end_y {
                self.file_list_focused = true;
                self.start_file_filter();
                return true;
            }
        }

        let (x, y, width, height) = match self.file_list_area {
            Some(area) => area,
            None => {
                if self.file_list_focused {
                    self.file_list_focused = false;
                    self.file_filter_active = false;
                    return true;
                }
                return false;
            }
        };
        let end_x = x.saturating_add(width);
        let end_y = y.saturating_add(height);
        if column < x || column >= end_x || row < y || row >= end_y {
            if self.file_list_focused {
                self.file_list_focused = false;
                self.file_filter_active = false;
                return true;
            }
            return false;
        }

        let item_start = y.saturating_add(1);
        if row < item_start {
            self.file_list_focused = true;
            return true;
        }

        let row_idx = (row - item_start) as usize;
        if let Some(Some(file_idx)) = self.file_list_rows.get(row_idx) {
            self.file_list_focused = true;
            self.select_file(*file_idx);
            return true;
        }

        self.file_list_focused = true;
        true
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

    pub fn clamp_file_panel_width(&self, viewport_width: u16) -> u16 {
        let max_panel = viewport_width
            .saturating_sub(DIFF_VIEW_MIN_WIDTH)
            .max(FILE_PANEL_MIN_WIDTH);
        self.file_panel_width.clamp(FILE_PANEL_MIN_WIDTH, max_panel)
    }

    pub fn resize_file_panel(&mut self, delta: i16, viewport_width: u16) {
        let next = (self.file_panel_width as i16).saturating_add(delta);
        let next = next.max(FILE_PANEL_MIN_WIDTH as i16) as u16;
        self.file_panel_width = next;
        self.file_panel_width = self.clamp_file_panel_width(viewport_width);
        self.file_panel_manually_set = true;
    }

    pub fn start_file_panel_resize(&mut self, column: u16, row: u16) -> bool {
        let (x, y, width, height) = match self.file_panel_rect {
            Some(rect) => rect,
            None => return false,
        };
        let sep_x = x.saturating_add(width.saturating_sub(1));
        let end_y = y.saturating_add(height);
        if column == sep_x && row >= y && row < end_y {
            self.file_panel_resizing = true;
            self.file_panel_manually_set = true;
            return true;
        }
        false
    }

    pub fn drag_file_panel_resize(&mut self, column: u16, viewport_width: u16) -> bool {
        if !self.file_panel_resizing {
            return false;
        }
        if let Some((x, _, _, _)) = self.file_panel_rect {
            let width = column.saturating_sub(x).saturating_add(1);
            self.file_panel_width = width;
            self.file_panel_width = self.clamp_file_panel_width(viewport_width);
            return true;
        }
        false
    }

    pub fn end_file_panel_resize(&mut self) {
        self.file_panel_resizing = false;
    }

    /// Check if current animation is backward (un-applying a change)
    pub fn is_backward_animation(&self) -> bool {
        self.animation_phase != AnimationPhase::Idle
            && self.multi_diff.current_step_direction() == StepDirection::Backward
    }

    pub(crate) fn allow_virtual_lines(&self) -> bool {
        if self.snap_frame.is_some() {
            return false;
        }
        !self.is_backward_animation()
    }

    pub(crate) fn cursor_visible_in_wrap(&self, viewport_height: usize) -> bool {
        self.last_wrap_active_idx
            .map(|idx| {
                idx >= self.scroll_offset
                    && idx < self.scroll_offset.saturating_add(viewport_height)
            })
            .unwrap_or(false)
    }

    /// Convert CLI animation phase to core AnimationFrame for phase-aware rendering
    pub fn animation_frame(&self) -> AnimationFrame {
        if let Some(frame) = self.snap_frame {
            return frame;
        }
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
        self.clear_peek();
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
        self.clear_peek();
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
        self.clear_peek();
        self.multi_diff.current_navigator().goto(1);
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
    }

    pub fn goto_last_step(&mut self) {
        self.clear_peek();
        self.multi_diff.current_navigator().goto_end();
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
    }

    fn goto_step_number(&mut self, step_number: usize) {
        if !self.stepping {
            return;
        }
        let total_steps = self.multi_diff.current_navigator().state().total_steps;
        if total_steps == 0 {
            return;
        }
        self.clear_peek();
        let clamped = step_number.max(1).min(total_steps);
        let target_step = clamped.saturating_sub(1);
        self.multi_diff.current_navigator().goto(target_step);
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
    }

    fn goto_hunk_number(&mut self, hunk_number: usize) {
        let total_hunks = self.multi_diff.current_navigator().state().total_hunks;
        if total_hunks == 0 {
            return;
        }
        let clamped = hunk_number.max(1).min(total_hunks);
        let hunk_idx = clamped - 1;
        if self.stepping {
            self.goto_hunk_index(hunk_idx);
        } else {
            self.goto_hunk_index_scroll(hunk_idx);
        }
    }

    fn goto_hunk_index(&mut self, hunk_idx: usize) {
        self.clear_peek();
        self.multi_diff.current_navigator().goto_hunk(hunk_idx);
        if self.animation_enabled {
            self.start_animation();
        } else {
            self.clear_active_on_next_render = true;
        }
        self.needs_scroll_to_active = true;
    }

    pub fn goto_first_hunk_scroll(&mut self) {
        let total = self.multi_diff.current_navigator().state().total_hunks;
        if total == 0 {
            return;
        }
        self.goto_hunk_index_scroll(0);
    }

    pub fn goto_last_hunk_scroll(&mut self) {
        let total = self.multi_diff.current_navigator().state().total_hunks;
        if total == 0 {
            return;
        }
        self.goto_hunk_index_scroll(total.saturating_sub(1));
    }

    fn goto_hunk_index_scroll(&mut self, hunk_idx: usize) {
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_bounds, new_bounds) = self.compute_hunk_bounds_split();
                let old = old_bounds.get(hunk_idx).copied().flatten();
                let new = new_bounds.get(hunk_idx).copied().flatten();
                self.pick_split_bounds(old, new).map(|b| (hunk_idx, b))
            }
            _ => {
                let bounds = self.compute_hunk_bounds_single();
                bounds
                    .get(hunk_idx)
                    .copied()
                    .flatten()
                    .map(|b| (hunk_idx, b))
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

    fn goto_line_number(&mut self, line_number: usize) {
        self.clear_peek();
        let view = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(AnimationFrame::Idle);
        let target_idx = match self.view_mode {
            ViewMode::Split => {
                let mut old_idx = 0usize;
                let mut new_idx = 0usize;
                let mut old_last = None;
                let mut new_last = None;
                let mut old_max_line = 0usize;
                let mut new_max_line = 0usize;
                let mut old_match = None;
                let mut new_match = None;
                for line in &view {
                    if let Some(old_line) = line.old_line {
                        old_max_line = old_max_line.max(old_line);
                        if old_line == line_number {
                            old_match = Some(old_idx);
                        }
                        old_idx += 1;
                        old_last = Some(old_idx - 1);
                    }
                    if let Some(new_line) = line.new_line {
                        new_max_line = new_max_line.max(new_line);
                        if new_line == line_number {
                            new_match = Some(new_idx);
                        }
                        new_idx += 1;
                        new_last = Some(new_idx - 1);
                    }
                }
                if line_number == 0 {
                    let first_old = if old_idx > 0 { Some(0) } else { None };
                    let first_new = if new_idx > 0 { Some(0) } else { None };
                    self.pick_split_index(first_old, first_new)
                } else {
                    let max_line = old_max_line.max(new_max_line);
                    if max_line > 0 && line_number > max_line {
                        if old_max_line > new_max_line {
                            old_last
                        } else if new_max_line > old_max_line {
                            new_last
                        } else {
                            self.pick_split_index(old_last, new_last)
                        }
                    } else {
                        self.pick_split_index(old_match, new_match)
                    }
                }
            }
            ViewMode::Evolution => {
                let mut target = None;
                let mut last_idx = None;
                let mut max_line = 0usize;
                for (display_idx, line) in view
                    .iter()
                    .filter(|line| {
                        !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                    })
                    .enumerate()
                {
                    let line_num = line.new_line.or(line.old_line);
                    if let Some(num) = line_num {
                        max_line = max_line.max(num);
                    }
                    if line_num == Some(line_number) {
                        target = Some(display_idx);
                        break;
                    }
                    last_idx = Some(display_idx);
                }
                if line_number == 0 {
                    last_idx.map(|_| 0)
                } else if max_line > 0 && line_number > max_line {
                    last_idx
                } else {
                    target
                }
            }
            _ => {
                let mut target = None;
                let mut last_idx = None;
                let mut max_line = 0usize;
                for (display_idx, line) in view.iter().enumerate() {
                    let line_num = line.old_line.or(line.new_line);
                    if let Some(num) = line_num {
                        max_line = max_line.max(num);
                    }
                    if line_num == Some(line_number) {
                        target = Some(display_idx);
                        break;
                    }
                    last_idx = Some(display_idx);
                }
                if line_number == 0 {
                    last_idx.map(|_| 0)
                } else if max_line > 0 && line_number > max_line {
                    last_idx
                } else {
                    target
                }
            }
        };

        if let Some(idx) = target_idx {
            let viewport_height = self.last_viewport_height.max(1);
            if self.auto_center {
                let half_viewport = viewport_height / 2;
                self.scroll_offset = idx.saturating_sub(half_viewport);
                self.centered_once = true;
            } else {
                self.scroll_offset = idx;
                self.centered_once = false;
            }
            self.needs_scroll_to_active = false;
            self.multi_diff.current_navigator().set_hunk_scope(false);
            if !self.stepping {
                self.set_cursor_for_current_scroll();
            }
        }
    }

    pub fn toggle_autoplay(&mut self) {
        if self.autoplay && !self.autoplay_reverse {
            self.autoplay = false;
        } else {
            self.autoplay = true;
            self.autoplay_reverse = false;
        }
        self.last_autoplay_tick = Instant::now();
    }

    pub fn toggle_autoplay_reverse(&mut self) {
        if self.autoplay && self.autoplay_reverse {
            self.autoplay = false;
        } else {
            self.autoplay = true;
            self.autoplay_reverse = true;
            self.autoplay_remaining = None;
        }
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
    pub fn clamp_scroll(
        &mut self,
        total_lines: usize,
        viewport_height: usize,
        allow_overscroll: bool,
    ) {
        self.scroll_offset =
            self.scroll_offset
                .min(max_scroll(total_lines, viewport_height, allow_overscroll));
    }

    /// Whether overscroll is allowed (centering is about to happen or manual zz was used)
    pub fn allow_overscroll(&self) -> bool {
        allow_overscroll_state(
            self.auto_center,
            self.needs_scroll_to_active,
            self.centered_once,
        )
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
            let max_scroll = max_line_width.saturating_sub(viewport_width);
            self.horizontal_scroll = self.horizontal_scroll.min(max_scroll);
        }
    }

    pub fn clamp_horizontal_scroll_cached(&mut self, viewport_width: usize) {
        if self.line_wrap {
            return;
        }
        let max_line_width = self.current_max_line_width();
        if max_line_width == 0 {
            return;
        }
        let max_scroll = max_line_width.saturating_sub(viewport_width);
        self.horizontal_scroll = self.horizontal_scroll.min(max_scroll);
    }

    pub fn reset_current_max_line_width(&mut self) {
        let idx = self.multi_diff.selected_index;
        if self.stepping {
            if let Some(slot) = self.max_line_widths_step.get_mut(idx) {
                *slot = 0;
            }
        } else if let Some(slot) = self.max_line_widths_no_step.get_mut(idx) {
            *slot = 0;
        }
    }

    pub fn set_current_max_line_width(&mut self, max_line_width: usize) {
        let idx = self.multi_diff.selected_index;
        if self.stepping {
            if let Some(slot) = self.max_line_widths_step.get_mut(idx) {
                *slot = max_line_width;
            }
        } else if let Some(slot) = self.max_line_widths_no_step.get_mut(idx) {
            *slot = max_line_width;
        }
    }

    pub fn update_current_max_line_width(&mut self, max_line_width: usize) {
        let idx = self.multi_diff.selected_index;
        if self.stepping {
            if let Some(slot) = self.max_line_widths_step.get_mut(idx) {
                *slot = (*slot).max(max_line_width);
            }
        } else if let Some(slot) = self.max_line_widths_no_step.get_mut(idx) {
            *slot = (*slot).max(max_line_width);
        }
    }

    fn current_max_line_width(&self) -> usize {
        let idx = self.multi_diff.selected_index;
        if self.stepping {
            self.max_line_widths_step.get(idx).copied().unwrap_or(0)
        } else {
            self.max_line_widths_no_step.get(idx).copied().unwrap_or(0)
        }
    }

    pub fn toggle_line_wrap(&mut self) {
        self.line_wrap = !self.line_wrap;
        // Reset horizontal scroll when enabling wrap
        if self.line_wrap {
            self.horizontal_scroll = 0;
        }
        self.last_wrap_display_len = None;
        self.last_wrap_active_idx = None;
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

        let current = self.multi_diff.selected_index;
        let next_index = current.saturating_add(1);
        if next_index < self.multi_diff.file_count() {
            self.select_file(next_index);
        }
    }

    fn next_file_wrapped(&mut self) -> bool {
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

    fn prev_file_wrapped(&mut self) -> bool {
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

    fn wrap_to_file_hunk(&mut self, forward: bool, stepping: bool) -> bool {
        let indices = if !self.file_filter.is_empty() {
            self.filtered_file_indices()
        } else {
            (0..self.multi_diff.file_count()).collect()
        };
        if indices.is_empty() {
            return false;
        }
        let current = self.multi_diff.selected_index;
        let start_pos = indices.iter().position(|&i| i == current).unwrap_or(0);
        for offset in 1..=indices.len() {
            let pos = if forward {
                (start_pos + offset) % indices.len()
            } else {
                (start_pos + indices.len().saturating_sub(offset)) % indices.len()
            };
            let index = indices[pos];
            if index == current {
                break;
            }
            self.select_file(index);
            let total = self.multi_diff.current_navigator().state().total_hunks;
            if total == 0 {
                continue;
            }
            let target = if forward { 0 } else { total.saturating_sub(1) };
            if stepping {
                self.goto_hunk_index(target);
            } else {
                self.goto_hunk_index_scroll(target);
            }
            return true;
        }
        false
    }

    pub fn select_file(&mut self, index: usize) {
        let old_index = self.multi_diff.selected_index;
        self.clear_step_edge_hint();
        self.clear_hunk_edge_hint();
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

    fn active_scroll_buffers(&self) -> (&Vec<usize>, &Vec<usize>) {
        if self.stepping {
            (&self.scroll_offsets_step, &self.horizontal_scrolls_step)
        } else {
            (
                &self.scroll_offsets_no_step,
                &self.horizontal_scrolls_no_step,
            )
        }
    }

    fn active_scroll_buffers_mut(&mut self) -> (&mut Vec<usize>, &mut Vec<usize>) {
        if self.stepping {
            (
                &mut self.scroll_offsets_step,
                &mut self.horizontal_scrolls_step,
            )
        } else {
            (
                &mut self.scroll_offsets_no_step,
                &mut self.horizontal_scrolls_no_step,
            )
        }
    }

    fn save_scroll_position_for(&mut self, index: usize) {
        let scroll_offset = self.scroll_offset;
        let horizontal_scroll = self.horizontal_scroll;
        let (scrolls, horizontals) = self.active_scroll_buffers_mut();
        if let Some(slot) = scrolls.get_mut(index) {
            *slot = scroll_offset;
        }
        if let Some(slot) = horizontals.get_mut(index) {
            *slot = horizontal_scroll;
        }
    }

    fn restore_scroll_position_for(&mut self, index: usize) {
        let (scrolls, horizontals) = self.active_scroll_buffers();
        let scroll_value = scrolls.get(index).copied();
        let horizontal_value = horizontals.get(index).copied();
        if let Some(value) = scroll_value {
            self.scroll_offset = value;
        }
        if let Some(value) = horizontal_value {
            self.horizontal_scroll = value;
        }
    }

    fn save_step_state_snapshot(&mut self, index: usize) {
        let state = self.multi_diff.current_navigator().state().clone();
        if let Some(slot) = self.step_state_snapshots.get_mut(index) {
            *slot = Some(state);
        }
    }

    fn restore_step_state_snapshot(&mut self, index: usize) -> bool {
        let Some(snapshot) = self.step_state_snapshots.get(index).and_then(|s| s.clone()) else {
            return false;
        };
        self.multi_diff.current_navigator().set_state(snapshot)
    }

    fn ensure_step_state_snapshot(&mut self, index: usize) {
        let needs_snapshot = self
            .step_state_snapshots
            .get(index)
            .map(|slot| slot.is_none())
            .unwrap_or(false);
        if !needs_snapshot {
            return;
        }
        let state = self.multi_diff.current_navigator().state().clone();
        if let Some(slot) = self.step_state_snapshots.get_mut(index) {
            *slot = Some(state);
        }
    }

    fn save_no_step_state_snapshot(&mut self, index: usize) {
        if self.stepping {
            return;
        }
        let state = self.multi_diff.current_navigator().state();
        if let Some(slot) = self.no_step_state_snapshots.get_mut(index) {
            *slot = Some(NoStepState {
                current_hunk: state.current_hunk,
                cursor_change: state.cursor_change,
                last_nav_was_hunk: state.last_nav_was_hunk,
            });
        }
    }

    fn restore_no_step_state_snapshot(&mut self, index: usize) -> bool {
        let Some(snapshot) = self.no_step_state_snapshots.get(index).and_then(|s| *s) else {
            return false;
        };
        if snapshot.last_nav_was_hunk && snapshot.cursor_change.is_some() {
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(snapshot.current_hunk, snapshot.cursor_change);
            self.multi_diff
                .current_navigator()
                .set_hunk_scope(snapshot.last_nav_was_hunk);
        } else if self.no_step_auto_jump_on_enter {
            self.goto_hunk_index_scroll(0);
        } else {
            self.multi_diff.current_navigator().clear_cursor_change();
            self.multi_diff.current_navigator().set_hunk_scope(false);
        }
        true
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

    fn start_animation(&mut self) {
        self.animation_phase = AnimationPhase::FadeOut;
        self.animation_progress = 0.0;
        self.last_animation_tick = Instant::now();
    }

    /// Ensure active change is visible if needed (called from views after stepping)
    pub fn ensure_active_visible_if_needed(&mut self, viewport_height: usize) {
        if self.handle_search_scroll_if_needed(viewport_height) {
            return;
        }
        if !self.needs_scroll_to_active {
            return;
        }
        if self.auto_center && self.snap_frame.is_some() {
            return;
        }
        self.needs_scroll_to_active = false;

        let step_direction = self.multi_diff.current_step_direction();
        let auto_center = self.auto_center;
        // If auto_center is enabled, always center on active change
        if auto_center {
            self.center_on_active(viewport_height);
            return;
        }

        let frame = self.animation_frame();
        let view = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(frame);

        let (display_len, display_idx) = display_metrics(
            &view,
            self.view_mode,
            self.animation_phase,
            self.scroll_offset,
            step_direction,
            self.split_align_lines,
        );

        if let Some(idx) = display_idx {
            let margin = 3.min(viewport_height / 4);

            // Check if active line is above viewport
            if idx < self.scroll_offset.saturating_add(margin) {
                self.scroll_offset = idx.saturating_sub(margin);
            }
            // Check if active line is below viewport
            else if idx
                >= self
                    .scroll_offset
                    .saturating_add(viewport_height.saturating_sub(margin))
            {
                self.scroll_offset = idx.saturating_sub(viewport_height.saturating_sub(margin + 1));
            }
        } else if display_len > 0 {
            let state = self.multi_diff.current_navigator().state();
            if self.view_mode == ViewMode::Evolution && self.stepping && state.current_step > 0 {
                return;
            }
            // No active line (step 0); snap to top so "first step" is visible.
            self.scroll_offset = 0;
        }
    }

    pub fn handle_search_scroll_if_needed(&mut self, viewport_height: usize) -> bool {
        if !self.needs_scroll_to_search {
            return false;
        }
        self.needs_scroll_to_search = false;
        if let Some(idx) = self.search_target {
            if self.auto_center {
                let half_viewport = viewport_height / 2;
                self.scroll_offset = idx.saturating_sub(half_viewport);
                self.centered_once = true;
            } else {
                let margin = 3.min(viewport_height / 4);
                if idx < self.scroll_offset.saturating_add(margin) {
                    self.scroll_offset = idx.saturating_sub(margin);
                } else if idx
                    >= self
                        .scroll_offset
                        .saturating_add(viewport_height.saturating_sub(margin))
                {
                    self.scroll_offset =
                        idx.saturating_sub(viewport_height.saturating_sub(margin + 1));
                }
            }
        }
        true
    }

    pub fn ensure_active_visible_if_needed_wrapped(
        &mut self,
        viewport_height: usize,
        display_len: usize,
        display_idx: Option<usize>,
    ) {
        self.last_wrap_display_len = Some(display_len);
        self.last_wrap_active_idx = display_idx;

        if !self.needs_scroll_to_active {
            return;
        }
        if self.auto_center && self.snap_frame.is_some() {
            return;
        }
        self.needs_scroll_to_active = false;

        if self.auto_center {
            self.center_with_display_idx(viewport_height, display_len, display_idx);
            return;
        }

        if let Some(idx) = display_idx {
            let margin = 3.min(viewport_height / 4);

            if idx < self.scroll_offset.saturating_add(margin) {
                self.scroll_offset = idx.saturating_sub(margin);
            } else if idx
                >= self
                    .scroll_offset
                    .saturating_add(viewport_height.saturating_sub(margin))
            {
                self.scroll_offset = idx.saturating_sub(viewport_height.saturating_sub(margin + 1));
            }
        } else if display_len > 0 {
            let state = self.multi_diff.current_navigator().state();
            if self.view_mode == ViewMode::Evolution && self.stepping && state.current_step > 0 {
                return;
            }
            self.scroll_offset = 0;
        }
    }

    fn center_with_display_idx(
        &mut self,
        viewport_height: usize,
        display_len: usize,
        display_idx: Option<usize>,
    ) {
        if let Some(idx) = display_idx {
            let half_viewport = viewport_height / 2;
            self.scroll_offset = idx.saturating_sub(half_viewport);
        } else if display_len > 0 {
            let state = self.multi_diff.current_navigator().state();
            if self.view_mode == ViewMode::Evolution && self.stepping && state.current_step > 0 {
                return;
            }
            self.scroll_offset = 0;
        }

        self.centered_once = true;
        self.horizontal_scroll = 0;
    }

    /// Center the viewport on the active change (like Vim's zz)
    pub fn center_on_active(&mut self, viewport_height: usize) {
        if self.line_wrap {
            if let Some(display_len) = self.last_wrap_display_len {
                let display_idx = self.last_wrap_active_idx;
                self.center_with_display_idx(viewport_height, display_len, display_idx);
                return;
            }
        }

        let frame = self.animation_frame();
        let view = self
            .multi_diff
            .current_navigator()
            .current_view_with_frame(frame);
        let step_direction = self.multi_diff.current_step_direction();

        let (display_len, display_idx) = display_metrics(
            &view,
            self.view_mode,
            self.animation_phase,
            self.scroll_offset,
            step_direction,
            self.split_align_lines,
        );

        self.center_with_display_idx(viewport_height, display_len, display_idx);
    }

    /// Called every frame to update animations and autoplay
    pub fn tick(&mut self) {
        let now = Instant::now();

        if let Some(hint) = self.step_edge_hint {
            if now >= hint.until {
                self.step_edge_hint = None;
            }
        }
        if let Some(hint) = self.hunk_edge_hint {
            if now >= hint.until {
                self.hunk_edge_hint = None;
            }
        }

        if let Some(frame) = self.snap_frame {
            let started_at = self.snap_frame_started_at.get_or_insert(now);
            let phase_duration = Duration::from_millis(SNAP_PHASE_MS);
            if now.duration_since(*started_at) >= phase_duration {
                match frame {
                    AnimationFrame::FadeOut => {
                        self.snap_frame = Some(AnimationFrame::FadeIn);
                        self.snap_frame_started_at = Some(now);
                    }
                    AnimationFrame::FadeIn | AnimationFrame::Idle => {
                        self.snap_frame = None;
                        self.snap_frame_started_at = None;
                        let step_dir = self.multi_diff.current_navigator().state().step_direction;
                        if step_dir == StepDirection::Backward {
                            self.multi_diff.current_navigator().clear_active_change();
                        }
                    }
                }
            }
        }

        // Update animation
        if self.animation_phase != AnimationPhase::Idle {
            let elapsed = now.duration_since(self.last_animation_tick);
            let phase_duration = Duration::from_millis(self.animation_duration);

            self.animation_progress =
                (elapsed.as_secs_f32() / phase_duration.as_secs_f32()).min(1.0);

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
                let moved = if self.autoplay_reverse {
                    self.step_backward()
                } else {
                    self.step_forward()
                };
                if let Some(remaining) = self.autoplay_remaining.as_mut() {
                    if moved && *remaining > 0 {
                        *remaining = remaining.saturating_sub(1);
                    }
                    if !moved || *remaining == 0 {
                        self.autoplay_remaining = None;
                        self.autoplay = false;
                    }
                } else if !moved {
                    self.autoplay = false;
                }
                self.last_autoplay_tick = now;
            }
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
}

fn copy_to_clipboard(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        write_to_clipboard_cmd("pbcopy", &[], text)
    }
    #[cfg(target_os = "linux")]
    {
        if write_to_clipboard_cmd("wl-copy", &["--type", "text/plain"], text) {
            return true;
        }
        if write_to_clipboard_cmd("xclip", &["-selection", "clipboard"], text) {
            return true;
        }
        write_to_clipboard_cmd("xsel", &["--clipboard", "--input"], text)
    }
    #[cfg(target_os = "windows")]
    {
        write_to_clipboard_cmd("clip", &[], text)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

fn write_to_clipboard_cmd(cmd: &str, args: &[&str], text: &str) -> bool {
    let mut child = match Command::new(cmd).args(args).stdin(Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().is_ok()
}

fn old_text_for_change(change: &Change) -> String {
    let mut text = String::new();
    for span in &change.spans {
        match span.kind {
            ChangeKind::Equal => text.push_str(&span.text),
            ChangeKind::Delete | ChangeKind::Replace => text.push_str(&span.text),
            ChangeKind::Insert => {}
        }
    }
    text
}

fn inline_text_for_change(change: &Change) -> String {
    let mut text = String::new();
    for span in &change.spans {
        match span.kind {
            ChangeKind::Equal => text.push_str(&span.text),
            ChangeKind::Delete => text.push_str(&span.text),
            ChangeKind::Insert => text.push_str(&span.text),
            ChangeKind::Replace => {
                text.push_str(&span.text);
                text.push_str(&span.new_text.clone().unwrap_or_else(|| span.text.clone()));
            }
        }
    }
    text
}

fn modified_only_text_for_change(change: &Change) -> String {
    let mut text = String::new();
    for span in &change.spans {
        match span.kind {
            ChangeKind::Equal => text.push_str(&span.text),
            ChangeKind::Delete => {}
            ChangeKind::Insert => text.push_str(&span.text),
            ChangeKind::Replace => {
                text.push_str(&span.new_text.clone().unwrap_or_else(|| span.text.clone()));
            }
        }
    }
    text
}

fn line_has_query(text: &str, regex: &Regex) -> bool {
    regex.is_match(text)
}

fn match_ranges(text: &str, regex: &Regex) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    for mat in regex.find_iter(text) {
        ranges.push((mat.start(), mat.end()));
    }
    ranges
}

fn apply_highlight_spans(
    spans: Vec<Span<'static>>,
    ranges: &[(usize, usize)],
    bg: Color,
    fg: Option<Color>,
) -> Vec<Span<'static>> {
    if ranges.is_empty() {
        return spans;
    }
    let mut out: Vec<Span> = Vec::new();
    let mut range_idx = 0usize;
    let mut offset = 0usize;

    for span in spans {
        let text = span.content.as_ref();
        let span_len = text.len();
        let span_start = offset;
        let span_end = offset + span_len;

        if span_len == 0 {
            continue;
        }

        while range_idx < ranges.len() && ranges[range_idx].1 <= span_start {
            range_idx += 1;
        }

        let mut cursor = span_start;
        while range_idx < ranges.len() && ranges[range_idx].0 < span_end {
            let (r_start, r_end) = ranges[range_idx];
            let before_end = r_start.max(span_start);
            if before_end > cursor {
                let slice = &text[(cursor - span_start)..(before_end - span_start)];
                out.push(Span::styled(slice.to_string(), span.style));
            }
            let highlight_start = r_start.max(span_start);
            let highlight_end = r_end.min(span_end);
            if highlight_end > highlight_start {
                let slice = &text[(highlight_start - span_start)..(highlight_end - span_start)];
                let mut style = span.style.bg(bg);
                if let Some(fg) = fg {
                    style = style.fg(fg);
                }
                out.push(Span::styled(slice.to_string(), style));
            }
            cursor = highlight_end;
            if r_end <= span_end {
                range_idx += 1;
            } else {
                break;
            }
        }

        if cursor < span_end {
            let slice = &text[(cursor - span_start)..(span_end - span_start)];
            out.push(Span::styled(slice.to_string(), span.style));
        }

        offset += span_len;
    }

    out
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
    split_align_lines: bool,
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
        ViewMode::Split => {
            split_display_metrics(view, scroll_offset, step_direction, split_align_lines)
        }
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
    split_align_lines: bool,
) -> (usize, Option<usize>) {
    let mut old_count = 0usize;
    let mut new_count = 0usize;
    // Track primary separately from fallback (first non-primary active)
    let mut old_primary_idx: Option<usize> = None;
    let mut new_primary_idx: Option<usize> = None;
    let mut old_fallback_idx: Option<usize> = None;
    let mut new_fallback_idx: Option<usize> = None;

    for line in view {
        let old_present = line.old_line.is_some();
        let new_present = line.new_line.is_some()
            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete);
        if old_present || (split_align_lines && new_present) {
            if line.is_primary_active {
                old_primary_idx = Some(old_count);
            } else if line.is_active && old_fallback_idx.is_none() {
                old_fallback_idx = Some(old_count);
            }
            old_count += 1;
        }
        if new_present || (split_align_lines && old_present) {
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
        assert!(allow_overscroll_state(false, false, true)); // manual zz
        assert!(!allow_overscroll_state(false, true, false));
        assert!(!allow_overscroll_state(true, false, false)); // auto_center but not scrolling
        assert!(allow_overscroll_state(true, true, false)); // auto_center + about to scroll
        assert!(allow_overscroll_state(true, true, true)); // both
        assert!(allow_overscroll_state(true, false, true)); // centered_once wins
    }

    #[test]
    fn test_max_scroll_normal() {
        assert_eq!(max_scroll(100, 20, false), 80);
        assert_eq!(max_scroll(50, 10, false), 40);
        assert_eq!(max_scroll(20, 20, false), 0);
        assert_eq!(max_scroll(5, 20, false), 0); // short file
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
            is_active_change: is_active,
            is_primary_active,
            show_hunk_extent: false,
            change_id: 0,
            hunk_index: None,
            has_changes: kind != LineKind::Context,
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
            make_view_line(LineKind::Context, Some(1), Some(1), true, false), // active but not primary, both sides
            make_view_line(LineKind::Context, Some(2), Some(2), false, false),
            make_view_line(LineKind::Inserted, None, Some(3), true, true), // primary, new side only
        ];
        // scroll_offset=0: old side's active at idx 0 is closer than new side's primary at idx 2
        // But primary must dominate, so result should be new side's idx 2
        let (len, idx) = split_display_metrics(&view, 0, StepDirection::Forward, false);
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
        let (_, idx) = split_display_metrics(&view, 0, StepDirection::Forward, false);
        assert_eq!(idx, Some(2)); // new side wins on Forward

        let (_, idx) = split_display_metrics(&view, 0, StepDirection::Backward, false);
        assert_eq!(idx, Some(2)); // old side wins on Backward (same value here)

        // scroll_offset=10: both at dist=8, tie-break applies
        let (_, idx) = split_display_metrics(&view, 10, StepDirection::Forward, false);
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
        let (len, idx) = split_display_metrics(&view, 0, StepDirection::Forward, false);
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

    fn make_app_with_single_hunk_two_changes() -> App {
        let old = "one\ntwo\nthree\nfour".to_string();
        let new = "ONE\nTWO\nthree\nfour".to_string();
        let multi_diff = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            old,
            new,
        );
        App::new(multi_diff, ViewMode::SinglePane, 0, false, None)
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
        let current_hunk = state.current_hunk;

        app.goto_hunk_end_scroll();
        let end_state = app.multi_diff.current_navigator().state();
        assert_eq!(end_state.current_hunk, current_hunk);
        assert!(end_state.cursor_change.is_some());

        app.goto_hunk_start_scroll();
        let start_state = app.multi_diff.current_navigator().state();
        assert_eq!(start_state.current_hunk, current_hunk);
        assert!(start_state.cursor_change.is_some());
    }

    #[test]
    fn test_toggle_stepping_restores_no_step_cursor_scope() {
        let mut app = make_app_with_two_hunks();
        app.next_hunk_scroll();

        let before = app.multi_diff.current_navigator().state().clone();
        assert!(before.last_nav_was_hunk);
        assert!(before.cursor_change.is_some());

        app.toggle_stepping(); // go to stepping
        assert!(app.stepping);
        app.toggle_stepping(); // back to no-step

        let after = app.multi_diff.current_navigator().state();
        assert_eq!(after.current_hunk, before.current_hunk);
        assert_eq!(after.cursor_change, before.cursor_change);
        assert!(after.last_nav_was_hunk);
    }

    #[test]
    fn test_hunk_step_info_counts_applied_changes() {
        let mut app = make_app_with_single_hunk_two_changes();
        assert_eq!(app.hunk_step_info(), Some((0, 2)));

        app.next_step();
        assert_eq!(app.hunk_step_info(), Some((1, 2)));

        app.next_step();
        assert_eq!(app.hunk_step_info(), Some((2, 2)));
    }

    #[test]
    fn test_no_step_snapshot_restores_cursor_or_jumps() {
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
        app.no_step_auto_jump_on_enter = true;
        app.enter_no_step_mode();

        let idx = app.multi_diff.selected_index;
        app.save_no_step_state_snapshot(idx);
        app.multi_diff.current_navigator().clear_cursor_change();
        app.multi_diff.current_navigator().set_hunk_scope(false);

        assert!(app.restore_no_step_state_snapshot(idx));
        let cursor_id = app
            .multi_diff
            .current_navigator()
            .state()
            .cursor_change
            .expect("cursor change expected");
        assert!(cursor_id > 0);
    }

    #[test]
    fn test_no_step_cursor_stable_through_file_cycles() {
        let old_lines: Vec<String> = (1..=25).map(|i| format!("line{}", i)).collect();
        let mut new_lines = old_lines.clone();
        new_lines[1] = "line2-new".to_string();
        new_lines[19] = "line20-new".to_string();
        let old = old_lines.join("\n");
        let new = new_lines.join("\n");

        let multi = MultiFileDiff::from_file_pairs(vec![
            (std::path::PathBuf::from("a.txt"), old.clone(), new.clone()),
            (std::path::PathBuf::from("b.txt"), old.clone(), new.clone()),
            (std::path::PathBuf::from("c.txt"), old.clone(), new.clone()),
        ]);
        let mut app = App::new(multi, ViewMode::SinglePane, 0, false, None);
        app.stepping = false;
        app.no_step_auto_jump_on_enter = true;
        app.enter_no_step_mode();

        app.goto_hunk_start_scroll();
        let first_cursor = app.multi_diff.current_navigator().state().cursor_change;

        app.next_file();
        app.next_file();
        app.prev_file();
        app.prev_file();

        let cursor_after = app.multi_diff.current_navigator().state().cursor_change;

        assert_eq!(first_cursor, cursor_after);
    }
}
