//! Application state and logic

use crate::blame::BlameInfo;
use crate::config::{
    BlameMode, DiffExtentMarkerMode, DiffExtentMarkerScope, DiffForegroundMode, DiffHighlightMode,
    FileCountMode, FoldContextMode, HunkWrapMode, ModifiedStepMode, ResolvedTheme, StepWrapMode,
    SyntaxMode,
};
use crate::syntax::{SyntaxCache, SyntaxEngine};
use crate::time_format::TimeFormatter;
use oyo_core::{
    multi::DiffStatus, AnimationFrame, LineKind, MultiFileDiff, StepDirection, StepState, ViewLine,
};
use ratatui::style::Color;
use regex::Regex;
use rustc_hash::FxHashMap;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod blame;
mod diff_worker;
mod file_panel;
mod files;
mod navigation;
mod palette;
mod playback;
mod search;
mod syntax;
mod types;
mod utils;

pub(crate) use types::{
    AnimationPhase, BlameDisplay, BlameRenderCache, BlameRenderKey, PeekMode, PeekScope, PeekState,
    UnifiedRenderKey, UnifiedRenderModel, ViewMode, DIFF_VIEW_MIN_WIDTH, FILE_PANEL_MIN_WIDTH,
};
use types::{
    BlameCacheKey, BlamePrefetchKey, BlamePrefetchRange, BlameRequest, BlameResponse,
    BlameStepHint, DiffRequest, DiffResponse, HunkBounds, HunkEdge, HunkEdgeHint, HunkStart,
    NoStepState, StepEdge, StepEdgeHint, SyntaxScopeCache,
};
use utils::{allow_overscroll_state, max_scroll};
pub(crate) use utils::{display_metrics, is_conflict_marker, is_fold_line};

type UnifiedHunkCacheKey = (usize, ViewMode, FoldContextMode, bool, usize, usize, usize);
type SplitHunkCacheKey = (usize, FoldContextMode, bool, bool, usize, usize, usize);
type UnifiedHunkStartsCache = Option<(UnifiedHunkCacheKey, Vec<Option<HunkStart>>)>;
type UnifiedHunkBoundsCache = Option<(UnifiedHunkCacheKey, Vec<Option<HunkBounds>>)>;
type SplitHunkStartsCache = Option<(
    SplitHunkCacheKey,
    (Vec<Option<HunkStart>>, Vec<Option<HunkStart>>),
)>;
type SplitHunkBoundsCache = Option<(
    SplitHunkCacheKey,
    (Vec<Option<HunkBounds>>, Vec<Option<HunkBounds>>),
)>;

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
    /// Whether to open the commit picker dashboard
    pub open_dashboard: bool,
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
    /// Defer heavy view rebuild by one frame (for large-file jumps)
    view_build_defer: bool,
    /// True while a deferred view rebuild is pending
    view_build_pending: bool,
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
    /// Collapse long unchanged (context) blocks
    pub fold_context: FoldContextMode,
    /// Default fold context mode (restored when toggling)
    fold_context_default: FoldContextMode,
    /// Cached wrapped display length (for line wrap centering)
    last_wrap_display_len: Option<usize>,
    /// Cached wrapped active display index (for line wrap centering)
    last_wrap_active_idx: Option<usize>,
    /// Show scrollbar
    pub scrollbar_visible: bool,
    /// Show strikethrough on deleted text
    pub strikethrough_deletions: bool,
    /// Show +/- sign column in the gutter (unified/evolution)
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
    /// Marker for primary active line (left pane / unified pane)
    pub primary_marker: String,
    /// Marker for right pane primary line
    pub primary_marker_right: String,
    /// Marker for hunk extent lines (left pane / unified pane)
    pub extent_marker: String,
    /// Marker for right pane extent lines
    pub extent_marker_right: String,
    /// Clear active change after next render (for one-frame animation styling)
    pub clear_active_on_next_render: bool,
    /// Resolved theme (colors, gradients)
    pub theme: ResolvedTheme,
    /// Time formatting rules
    pub time_format: TimeFormatter,
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
    /// Show extent markers on unchanged context lines within a hunk
    pub diff_extent_marker_context: bool,
    /// Blame display enabled
    pub blame_enabled: bool,
    /// Blame display mode
    pub blame_mode: BlameMode,
    /// Show blame hint when jumping to a hunk
    pub blame_hunk_hint_enabled: bool,
    /// True when blame toggle is active
    blame_toggle: bool,
    /// Cached git user name for blame display
    blame_user_name: Option<String>,
    /// Cached blame entries
    blame_cache: FxHashMap<BlameCacheKey, BlameInfo>,
    /// Cached blame display text (used as fallback while loading)
    blame_display_cache: FxHashMap<BlameCacheKey, BlameDisplay>,
    /// Cached blame bar colors (used as fallback while loading)
    blame_bar_cache: FxHashMap<BlameCacheKey, Color>,
    /// Cached blame time ranges (min/max) per file/source
    blame_time_ranges: FxHashMap<BlamePrefetchKey, (i64, i64)>,
    /// Cached unified hunk starts for no-step mode
    hunk_starts_unified_cache: UnifiedHunkStartsCache,
    /// Cached unified hunk bounds for no-step mode
    hunk_bounds_unified_cache: UnifiedHunkBoundsCache,
    /// Cached split hunk starts for no-step mode
    hunk_starts_split_cache: SplitHunkStartsCache,
    /// Cached split hunk bounds for no-step mode
    hunk_bounds_split_cache: SplitHunkBoundsCache,
    /// Cached blame prefetch windows
    blame_prefetch: FxHashMap<BlamePrefetchKey, BlamePrefetchRange>,
    /// Cached blame render layout (for scroll performance)
    pub(crate) blame_render_cache: Option<BlameRenderCache>,
    /// Revision for blame cache updates
    pub(crate) blame_cache_revision: u64,
    /// Blame prefetch requests currently in flight
    blame_pending: FxHashMap<BlamePrefetchKey, BlamePrefetchRange>,
    /// Throttle blame prefetch to avoid repeated git calls
    blame_prefetch_at: Option<Instant>,
    blame_worker_tx: Option<mpsc::Sender<BlameRequest>>,
    blame_worker_rx: Option<mpsc::Receiver<BlameResponse>>,
    /// Defer diff computation for large files
    pub diff_defer: bool,
    /// Idle time (ms) before background diff computation
    pub diff_idle_ms: u64,
    /// Last user interaction timestamp (for idle detection)
    diff_last_input: Instant,
    /// Pending diff jobs (file indices)
    diff_queue: VecDeque<usize>,
    /// Currently computing diff (file index)
    diff_inflight: Option<usize>,
    /// Worker thread for diff computation
    diff_worker_tx: Option<mpsc::Sender<DiffRequest>>,
    diff_worker_rx: Option<mpsc::Receiver<DiffResponse>>,
    /// Extra display rows after each line (blame wrapping).
    pub(crate) blame_extra_rows: Option<Vec<usize>>,
    /// One-shot blame hint for the active change
    blame_step_hint: Option<BlameStepHint>,
    /// Blame hint shown when jumping to a hunk
    blame_hunk_hint: Option<String>,
    /// Single-pane modified line render mode while stepping
    pub unified_modified_step_mode: ModifiedStepMode,
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
    /// Syntax warmup budget while actively navigating (lines per tick)
    pub syntax_warmup_active_lines: usize,
    /// Syntax warmup budget when waiting on a pending checkpoint (lines per tick)
    pub syntax_warmup_pending_lines: usize,
    /// Syntax warmup budget when idle (lines per tick)
    pub syntax_warmup_idle_lines: usize,
    /// Syntax warmup debounce window (ms)
    pub syntax_warmup_debounce_ms: u64,
    /// Warmup range (old side) accumulated for the current frame
    syntax_warmup_frame_old: Option<WarmupRange>,
    /// Warmup range (new side) accumulated for the current frame
    syntax_warmup_frame_new: Option<WarmupRange>,
    /// Latest warmup target from the viewport
    syntax_warmup_target: Option<SyntaxWarmupTarget>,
    /// Last warmup target applied to the cache
    syntax_warmup_target_applied: Option<SyntaxWarmupTarget>,
    /// Timestamp of the last warmup target update
    syntax_warmup_target_at: Option<Instant>,
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
    /// Command palette query
    command_palette_query: String,
    /// True when command palette is active
    command_palette_active: bool,
    /// Selected command palette entry
    command_palette_selection: usize,
    /// Command palette list area (x, y, width, height)
    command_palette_list_area: Option<(u16, u16, u16, u16)>,
    /// Command palette list start index
    command_palette_list_start: usize,
    /// Command palette visible list count
    command_palette_list_count: usize,
    /// Command palette list item height (rows per item)
    command_palette_item_height: u16,
    /// Quick file search query
    file_search_query: String,
    /// True when quick file search is active
    file_search_active: bool,
    /// Selected quick file search entry
    file_search_selection: usize,
    /// Quick file search list area (x, y, width, height)
    file_search_list_area: Option<(u16, u16, u16, u16)>,
    /// Quick file search list start index
    file_search_list_start: usize,
    /// Quick file search visible list count
    file_search_list_count: usize,
    /// Quick file search list item height (rows per item)
    file_search_item_height: u16,
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
    /// Cached view lines for the current state/frame
    view_cache: Option<ViewCache>,
    /// Cached render model for unified view
    pub(crate) unified_render_cache: Option<UnifiedRenderModel>,
    /// Windowed render start (for large-file partial rendering)
    view_window_start: usize,
    /// Total logical length for windowed render (for scrollbars/metrics)
    view_window_total_len: Option<usize>,
}

const SNAP_PHASE_MS: u64 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ViewCacheKey {
    file_index: usize,
    view_mode: ViewMode,
    frame: AnimationFrame,
    current_step: usize,
    active_change: Option<usize>,
    cursor_change: Option<usize>,
    animating_hunk: Option<usize>,
    step_direction: StepDirection,
    current_hunk: usize,
    last_nav_was_hunk: bool,
    hunk_preview_mode: bool,
    preview_from_backward: bool,
    show_hunk_extent_while_stepping: bool,
    placeholder_view: bool,
    fold_context: FoldContextMode,
    viewport_height: usize,
    windowed: bool,
    window_start: usize,
}

struct ViewCache {
    key: ViewCacheKey,
    lines: std::sync::Arc<Vec<ViewLine>>,
    window_start: usize,
    window_total_len: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct ViewWindow {
    start: usize,
    end: usize,
    total_len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WarmupRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SyntaxWarmupTarget {
    file_index: usize,
    old: Option<WarmupRange>,
    new: Option<WarmupRange>,
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
            open_dashboard: false,
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
            view_build_defer: false,
            view_build_pending: false,
            horizontal_scroll: 0,
            horizontal_scrolls_step: vec![0; file_count],
            horizontal_scrolls_no_step: vec![0; file_count],
            max_line_widths_step: vec![0; file_count],
            max_line_widths_no_step: vec![0; file_count],
            line_wrap: false,
            fold_context: FoldContextMode::Off,
            fold_context_default: FoldContextMode::Off,
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
            time_format: TimeFormatter::default(),
            theme_is_light: false,
            stepping: true,
            hunk_wrap: HunkWrapMode::None,
            step_wrap: StepWrapMode::None,
            diff_bg: false,
            diff_fg: DiffForegroundMode::Theme,
            diff_highlight: DiffHighlightMode::Text,
            diff_extent_marker: DiffExtentMarkerMode::Neutral,
            diff_extent_marker_scope: DiffExtentMarkerScope::Progress,
            diff_extent_marker_context: false,
            blame_enabled: false,
            blame_mode: BlameMode::OneShot,
            blame_hunk_hint_enabled: true,
            blame_toggle: false,
            blame_user_name: None,
            blame_cache: FxHashMap::default(),
            blame_display_cache: FxHashMap::default(),
            blame_bar_cache: FxHashMap::default(),
            blame_time_ranges: FxHashMap::default(),
            hunk_starts_unified_cache: None,
            hunk_bounds_unified_cache: None,
            hunk_starts_split_cache: None,
            hunk_bounds_split_cache: None,
            blame_prefetch: FxHashMap::default(),
            blame_render_cache: None,
            blame_cache_revision: 0,
            blame_pending: FxHashMap::default(),
            blame_prefetch_at: None,
            blame_worker_tx: None,
            blame_worker_rx: None,
            diff_defer: true,
            diff_idle_ms: 250,
            diff_last_input: Instant::now(),
            diff_queue: VecDeque::new(),
            diff_inflight: None,
            diff_worker_tx: None,
            diff_worker_rx: None,
            blame_extra_rows: None,
            blame_step_hint: None,
            blame_hunk_hint: None,
            unified_modified_step_mode: ModifiedStepMode::Mixed,
            split_align_lines: false,
            split_align_fill: "╱".to_string(),
            evo_syntax: crate::config::EvoSyntaxMode::Context,
            syntax_mode: SyntaxMode::On,
            syntax_theme: "ansi".to_string(),
            syntax_warmup_active_lines: 100,
            syntax_warmup_pending_lines: 300,
            syntax_warmup_idle_lines: 1_000,
            syntax_warmup_debounce_ms: 80,
            syntax_warmup_frame_old: None,
            syntax_warmup_frame_new: None,
            syntax_warmup_target: None,
            syntax_warmup_target_applied: None,
            syntax_warmup_target_at: None,
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
            command_palette_query: String::new(),
            command_palette_active: false,
            command_palette_selection: 0,
            command_palette_list_area: None,
            command_palette_list_start: 0,
            command_palette_list_count: 0,
            command_palette_item_height: 1,
            file_search_query: String::new(),
            file_search_active: false,
            file_search_selection: 0,
            file_search_list_area: None,
            file_search_list_start: 0,
            file_search_list_count: 0,
            file_search_item_height: 1,
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
            view_cache: None,
            unified_render_cache: None,
            view_window_start: 0,
            view_window_total_len: None,
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

    pub fn help_scroll_up(&mut self) {
        self.help_scroll = self.help_scroll.saturating_sub(1);
    }

    pub fn help_scroll_down(&mut self) {
        self.help_scroll = (self.help_scroll + 1).min(self.help_max_scroll);
    }

    pub fn toggle_path_popup(&mut self) {
        self.show_path_popup = !self.show_path_popup;
    }

    pub fn toggle_zen(&mut self) {
        self.zen_mode = !self.zen_mode;
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
        self.needs_scroll_to_active = true;
        self.centered_once = false;
    }

    pub fn toggle_fold_context(&mut self) {
        if self.fold_context.is_enabled() {
            self.fold_context = FoldContextMode::Off;
        } else if self.fold_context_default.is_enabled() {
            self.fold_context = self.fold_context_default;
        } else {
            self.fold_context = FoldContextMode::On;
        }
        self.last_wrap_display_len = None;
        self.last_wrap_active_idx = None;
        self.needs_scroll_to_active = true;
        self.centered_once = false;
        self.blame_render_cache = None;
    }

    pub fn set_fold_context_mode(&mut self, mode: FoldContextMode) {
        self.fold_context = mode;
        self.fold_context_default = mode;
    }

    pub fn toggle_strikethrough_deletions(&mut self) {
        self.strikethrough_deletions = !self.strikethrough_deletions;
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

    fn start_animation(&mut self) {
        self.animation_phase = AnimationPhase::FadeOut;
        self.animation_progress = 0.0;
        self.last_animation_tick = Instant::now();
    }

    pub(crate) fn animation_frame(&self) -> AnimationFrame {
        if let Some(frame) = self.snap_frame {
            return frame;
        }
        // Force FadeOut for one-frame render when animation disabled,
        // so backward insert-only changes produce ViewLines for extent markers.
        if self.clear_active_on_next_render {
            return AnimationFrame::FadeOut;
        }
        match self.animation_phase {
            AnimationPhase::FadeOut => AnimationFrame::FadeOut,
            AnimationPhase::FadeIn => AnimationFrame::FadeIn,
            AnimationPhase::Idle => AnimationFrame::Idle,
        }
    }

    pub(crate) fn current_file_diff_ready(&self) -> bool {
        matches!(
            self.multi_diff.current_file_diff_status(),
            DiffStatus::Ready
        )
    }

    fn view_cache_key(
        &mut self,
        frame: AnimationFrame,
        windowed: bool,
        window_start: usize,
    ) -> ViewCacheKey {
        let idx = self.multi_diff.selected_index;
        let state = self.multi_diff.current_navigator().state();
        ViewCacheKey {
            file_index: idx,
            view_mode: self.view_mode,
            frame,
            current_step: state.current_step,
            active_change: state.active_change,
            cursor_change: state.cursor_change,
            animating_hunk: state.animating_hunk,
            step_direction: state.step_direction,
            current_hunk: state.current_hunk,
            last_nav_was_hunk: state.last_nav_was_hunk,
            hunk_preview_mode: state.hunk_preview_mode,
            preview_from_backward: state.preview_from_backward,
            show_hunk_extent_while_stepping: state.show_hunk_extent_while_stepping,
            placeholder_view: self.multi_diff.current_navigator_is_placeholder(),
            fold_context: self.fold_context,
            viewport_height: self.last_viewport_height,
            windowed,
            window_start,
        }
    }

    pub(crate) fn render_scroll_offset(&self) -> usize {
        self.scroll_offset.saturating_sub(self.view_window_start)
    }

    pub(crate) fn peek_state(&self) -> Option<PeekState> {
        self.peek_state
    }

    pub(crate) fn view_window_start(&self) -> usize {
        self.view_window_start
    }

    pub(crate) fn view_windowed(&self) -> bool {
        self.view_window_total_len.is_some()
    }

    pub(crate) fn render_total_lines(&self, view_len: usize) -> usize {
        self.view_window_total_len.unwrap_or(view_len)
    }

    pub(crate) fn view_build_pending(&self) -> bool {
        self.view_build_pending
    }

    pub(crate) fn defer_view_build_for_jump(&mut self) {
        if !self.stepping {
            return;
        }
        if !self.multi_diff.current_file_is_large() {
            return;
        }
        if !self.current_file_diff_ready() {
            return;
        }
        if self.view_cache.is_none() {
            return;
        }
        self.view_build_defer = true;
    }

    fn cancel_view_build_defer(&mut self) {
        self.view_build_defer = false;
        self.view_build_pending = false;
    }

    fn compute_view_window(&mut self) -> Option<ViewWindow> {
        if self.line_wrap {
            return None;
        }
        if !self.multi_diff.current_file_is_large() {
            return None;
        }
        let allow_split = self.view_mode == ViewMode::Split
            && self.split_align_lines
            && !self.fold_context.is_enabled();
        if !matches!(self.view_mode, ViewMode::UnifiedPane | ViewMode::Blame) && !allow_split {
            return None;
        }

        let allow_overscroll = self.allow_overscroll();
        let nav = self.multi_diff.current_navigator();
        let total_len = nav.diff().changes.len();
        if total_len == 0 {
            return None;
        }
        let viewport_height = self.last_viewport_height.max(1);
        let max_scroll = max_scroll(total_len, viewport_height, allow_overscroll);
        if self.scroll_offset > max_scroll {
            self.scroll_offset = max_scroll;
        }

        let span = self.last_viewport_height.max(20).saturating_mul(4).max(200);
        let margin = span / 4;

        if self.stepping {
            let state = nav.state();
            let change_id = state
                .active_change
                .or(state.applied_changes.last().copied())
                .or_else(|| nav.diff().significant_changes.first().copied())?;
            let idx = nav.change_index_for(change_id)?;
            let mut start = idx.saturating_sub(span);
            let mut end = (idx + span).min(total_len.saturating_sub(1));
            let scroll = self.scroll_offset.min(total_len.saturating_sub(1));
            if !self.needs_scroll_to_active {
                let inside_window =
                    scroll >= start.saturating_add(margin) && scroll <= end.saturating_sub(margin);
                if !inside_window {
                    start = scroll.saturating_sub(margin);
                    end = (start + span).min(total_len.saturating_sub(1));
                }
            }
            return Some(ViewWindow {
                start,
                end,
                total_len,
            });
        }

        let scroll = self.scroll_offset.min(total_len.saturating_sub(1));
        if total_len <= span {
            return Some(ViewWindow {
                start: 0,
                end: total_len.saturating_sub(1),
                total_len,
            });
        }
        let mut start = scroll.saturating_sub(margin);
        let mut end = (start + span).min(total_len.saturating_sub(1));
        if self.view_window_total_len.is_some() {
            let current_start = self.view_window_start;
            let current_end = (current_start + span).min(total_len.saturating_sub(1));
            let inside_window = scroll >= current_start.saturating_add(margin)
                && scroll <= current_end.saturating_sub(margin);
            if inside_window {
                start = current_start;
                end = current_end;
            }
        }
        Some(ViewWindow {
            start,
            end,
            total_len,
        })
    }

    pub(crate) fn current_view_with_frame(
        &mut self,
        frame: AnimationFrame,
    ) -> std::sync::Arc<Vec<ViewLine>> {
        let window = self.compute_view_window();
        let windowed = window.is_some();
        let window_start = window.map(|w| w.start).unwrap_or(0);
        let mut window_start_override = None;
        let mut window_total_override = None;
        let key = self.view_cache_key(frame, windowed, window_start);
        if let Some(cache) = &self.view_cache {
            if cache.key == key {
                self.view_window_start = cache.window_start;
                self.view_window_total_len = cache.window_total_len;
                self.view_build_pending = false;
                return cache.lines.clone();
            }
        }
        if self.view_build_defer {
            self.view_build_defer = false;
            if let Some(cache) = &self.view_cache {
                if cache.key.file_index == self.multi_diff.selected_index {
                    self.view_window_start = cache.window_start;
                    self.view_window_total_len = cache.window_total_len;
                    self.view_build_pending = true;
                    return cache.lines.clone();
                }
            }
            self.view_build_pending = false;
        } else {
            self.view_build_pending = false;
        }
        let mut view = if let Some(window) = window {
            let nav = self.multi_diff.current_navigator();
            let view = nav.current_view_for_change_range(frame, window.start, window.end);
            if self.view_mode == ViewMode::Evolution {
                let display_start = nav
                    .evolution_display_index_for_change_index(window.start)
                    .unwrap_or(0);
                window_start_override = Some(display_start);
                window_total_override = Some(nav.evolution_visible_len());
            }
            view
        } else if self.stepping && self.multi_diff.current_file_is_large() {
            let nav = self.multi_diff.current_navigator();
            let state = nav.state();
            let change_id = state
                .active_change
                .or(state.applied_changes.last().copied())
                .or_else(|| nav.diff().significant_changes.first().copied());
            if let Some(change_id) = change_id {
                let radius = self.last_viewport_height.max(20).saturating_mul(4).max(200);
                if self.view_mode == ViewMode::Evolution {
                    if let Some(display_idx) = nav.evolution_display_index_or_nearest(change_id) {
                        if let Some((start, end)) =
                            nav.evolution_change_range_for_display(display_idx, radius)
                        {
                            let start_display = display_idx.saturating_sub(radius);
                            window_start_override = Some(start_display);
                            window_total_override = Some(nav.evolution_visible_len());
                            nav.current_view_for_change_range(frame, start, end)
                        } else if let Some(anchor) =
                            nav.evolution_nearest_visible_change_id_dynamic(change_id, radius)
                        {
                            nav.current_view_for_change_window(frame, anchor, radius)
                        } else {
                            nav.current_view_for_change_window(frame, change_id, radius)
                        }
                    } else if let Some(anchor) =
                        nav.evolution_nearest_visible_change_id_dynamic(change_id, radius)
                    {
                        nav.current_view_for_change_window(frame, anchor, radius)
                    } else {
                        nav.current_view_for_change_window(frame, change_id, radius)
                    }
                } else {
                    nav.current_view_for_change_window(frame, change_id, radius)
                }
            } else {
                nav.current_view_with_frame(frame)
            }
        } else {
            self.multi_diff
                .current_navigator()
                .current_view_with_frame(frame)
        };
        if self.view_mode == ViewMode::Evolution && self.multi_diff.current_file_is_large() {
            let nav = self.multi_diff.current_navigator();
            let hunk_preview = nav.state().hunk_preview_mode;
            let has_visible = view.iter().any(|line| match line.kind {
                LineKind::Deleted => false,
                LineKind::PendingDelete => {
                    if hunk_preview {
                        false
                    } else {
                        frame != AnimationFrame::Idle && line.is_active_change
                    }
                }
                _ => true,
            });
            if !has_visible {
                let state = nav.state();
                let change_id = nav
                    .state()
                    .active_change
                    .or(state.applied_changes.last().copied())
                    .or_else(|| nav.diff().significant_changes.first().copied());
                if let Some(change_id) = change_id {
                    let radius = self.last_viewport_height.max(20).saturating_mul(4).max(200);
                    if let Some(anchor) =
                        nav.evolution_nearest_visible_change_id_dynamic(change_id, radius)
                    {
                        if let Some(display_idx) = nav.evolution_display_index_for_change(anchor) {
                            window_start_override = Some(display_idx.saturating_sub(radius));
                            window_total_override = Some(nav.evolution_visible_len());
                        }
                        view = nav.current_view_for_change_window(frame, anchor, radius);
                    }
                }
            }
        }
        let view = utils::fold_context_view(view, self.fold_context);
        let lines = std::sync::Arc::new(view);
        let applied_start = window_start_override.unwrap_or(window_start);
        let applied_total = window_total_override.or(window.map(|w| w.total_len));
        self.view_window_start = applied_start;
        self.view_window_total_len = applied_total;
        self.view_cache = Some(ViewCache {
            key,
            lines: lines.clone(),
            window_start: applied_start,
            window_total_len: self.view_window_total_len,
        });
        lines
    }

    pub(crate) fn is_backward_animation(&self) -> bool {
        if self.snap_frame.is_some() {
            return self.multi_diff.current_step_direction() == StepDirection::Backward;
        }
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

    /// Ensure active change is visible if needed (called from views after stepping)
    pub fn ensure_active_visible_if_needed(&mut self, viewport_height: usize) {
        if !self.current_file_diff_ready() {
            return;
        }
        if self.handle_search_scroll_if_needed(viewport_height) {
            return;
        }
        if !self.needs_scroll_to_active {
            return;
        }
        if self.auto_center && self.snap_frame.is_some() {
            return;
        }
        let frame = self.animation_frame();
        let view = self.current_view_with_frame(frame);
        if self.view_build_pending {
            return;
        }

        let step_direction = self.multi_diff.current_step_direction();
        let auto_center = self.auto_center;
        // If auto_center is enabled, always center on active change
        if auto_center {
            self.center_on_active(viewport_height);
            self.needs_scroll_to_active = false;
            return;
        }

        let scroll_offset = self.render_scroll_offset();
        let view_start = self.view_window_start;
        let (display_len, display_idx) = display_metrics(
            &view,
            self.view_mode,
            self.animation_phase,
            scroll_offset,
            step_direction,
            self.split_align_lines,
        );

        if let Some(idx) = display_idx {
            let margin = 3.min(viewport_height / 4);

            // Check if active line is above viewport
            if idx < scroll_offset.saturating_add(margin) {
                self.scroll_offset = view_start.saturating_add(idx.saturating_sub(margin));
            }
            // Check if active line is below viewport
            else if idx >= scroll_offset.saturating_add(viewport_height.saturating_sub(margin)) {
                self.scroll_offset = view_start
                    .saturating_add(idx.saturating_sub(viewport_height.saturating_sub(margin + 1)));
            }
        } else if display_len > 0 {
            let state = self.multi_diff.current_navigator().state();
            if self.view_mode == ViewMode::Evolution && self.stepping && state.current_step > 0 {
                return;
            }
            // No active line (step 0); snap to top so "first step" is visible.
            self.scroll_offset = 0;
        }
        self.needs_scroll_to_active = false;
    }

    pub fn ensure_active_visible_if_needed_wrapped(
        &mut self,
        viewport_height: usize,
        display_len: usize,
        display_idx: Option<usize>,
    ) {
        if !self.current_file_diff_ready() {
            return;
        }
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

        if self.multi_diff.current_file_is_large() && self.view_mode != ViewMode::Split {
            let nav = self.multi_diff.current_navigator();
            let state = nav.state();
            let primary_change = if state.cursor_change.is_some()
                && state.active_change.is_none()
                && state.step_direction == StepDirection::None
            {
                state.cursor_change
            } else if state.step_direction == StepDirection::Backward {
                state
                    .applied_changes
                    .last()
                    .copied()
                    .or(state.active_change)
            } else {
                state.active_change
            };
            let change_id = primary_change
                .or(state.applied_changes.last().copied())
                .or_else(|| nav.diff().significant_changes.first().copied());
            if let Some(change_id) = change_id {
                if self.view_mode == ViewMode::Evolution {
                    if let Some(idx) = nav.evolution_display_index_or_nearest(change_id) {
                        let display_len = nav.evolution_visible_len();
                        self.center_with_display_idx(viewport_height, display_len, Some(idx));
                        return;
                    }
                } else if let Some(idx) = nav.change_index_for(change_id) {
                    let display_len = nav.diff().changes.len();
                    self.center_with_display_idx(viewport_height, display_len, Some(idx));
                    return;
                }
            }
        }

        let frame = self.animation_frame();
        let view = self.current_view_with_frame(frame);
        let step_direction = self.multi_diff.current_step_direction();

        let (display_len, display_idx) = display_metrics(
            &view,
            self.view_mode,
            self.animation_phase,
            self.render_scroll_offset(),
            step_direction,
            self.split_align_lines,
        );
        let total_len = self.render_total_lines(display_len);
        let global_idx = display_idx.map(|idx| idx.saturating_add(self.view_window_start));
        self.center_with_display_idx(viewport_height, total_len, global_idx);
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

        self.poll_diff_responses();
        self.maybe_queue_idle_diff();

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

        self.maybe_warm_syntax_cache();
    }
}

#[cfg(test)]
mod tests;
