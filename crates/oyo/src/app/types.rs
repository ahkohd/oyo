use crate::blame::BlameInfo;
use crate::syntax::SyntaxSide;
use oyo_core::{multi::BlameSource, AnimationFrame};
use ratatui::style::Color;
use std::path::PathBuf;
use std::time::Instant;

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
    UnifiedPane,
    /// Split view with old on left, new on right
    Split,
    /// Evolution view - shows file morphing, deletions just disappear
    Evolution,
    /// Blame view - code with per-line blame gutter
    Blame,
}

impl ViewMode {
    /// Cycle to the next view mode
    pub fn next(self) -> Self {
        match self {
            ViewMode::UnifiedPane => ViewMode::Split,
            ViewMode::Split => ViewMode::Evolution,
            ViewMode::Evolution => ViewMode::Blame,
            ViewMode::Blame => ViewMode::UnifiedPane,
        }
    }

    /// Cycle to the previous view mode
    pub fn prev(self) -> Self {
        match self {
            ViewMode::UnifiedPane => ViewMode::Blame,
            ViewMode::Split => ViewMode::UnifiedPane,
            ViewMode::Evolution => ViewMode::Split,
            ViewMode::Blame => ViewMode::Evolution,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HunkStart {
    pub(crate) idx: usize,
    pub(crate) change_id: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HunkBounds {
    pub(crate) start: HunkStart,
    pub(crate) end: HunkStart,
}

pub(crate) const FILE_PANEL_MIN_WIDTH: u16 = 24;
pub(crate) const DIFF_VIEW_MIN_WIDTH: u16 = 50;

#[derive(Clone, Copy, Debug)]
pub(crate) struct NoStepState {
    pub(crate) current_hunk: usize,
    pub(crate) cursor_change: Option<usize>,
    pub(crate) last_nav_was_hunk: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum StepEdge {
    Start,
    End,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct StepEdgeHint {
    pub(crate) change_id: Option<usize>,
    pub(crate) edge: StepEdge,
    pub(crate) until: Instant,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum HunkEdge {
    First,
    Last,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HunkEdgeHint {
    pub(crate) edge: HunkEdge,
    pub(crate) until: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct BlameStepHint {
    pub(crate) change_id: usize,
    pub(crate) text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct BlameDisplay {
    pub(crate) group_key: String,
    pub(crate) text: String,
    pub(crate) author_time: Option<i64>,
    pub(crate) uncommitted: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct BlameCacheKey {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
    pub(crate) source: BlameSource,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct BlamePrefetchKey {
    pub(crate) path: PathBuf,
    pub(crate) source: BlameSource,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BlamePrefetchRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlameRenderKey {
    pub(crate) file_index: usize,
    pub(crate) current_step: usize,
    pub(crate) current_hunk: usize,
    pub(crate) hunk_preview_mode: bool,
    pub(crate) preview_from_backward: bool,
    pub(crate) stepping: bool,
    pub(crate) line_wrap: bool,
    pub(crate) wrap_width: usize,
    pub(crate) blame_width: u16,
    pub(crate) view_len: usize,
    pub(crate) animation_frame: AnimationFrame,
    pub(crate) cache_rev: u64,
    pub(crate) time_bucket: i64,
}

pub(crate) struct BlameRenderCache {
    pub(crate) key: BlameRenderKey,
    pub(crate) wrap_counts: Vec<usize>,
    pub(crate) extra_rows_after_line: Vec<usize>,
    pub(crate) extra_texts_after_line: Vec<Vec<String>>,
    pub(crate) display_texts: Vec<String>,
    pub(crate) bar_colors: Vec<Option<Color>>,
}

#[derive(Clone, Debug)]
pub(crate) struct BlameRequest {
    pub(crate) repo_root: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) source: BlameSource,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct BlameResponse {
    pub(crate) path: PathBuf,
    pub(crate) source: BlameSource,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) entries: Vec<(usize, BlameInfo)>,
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
pub(crate) struct SyntaxScopeCache {
    pub(crate) file_index: usize,
    pub(crate) side: SyntaxSide,
    pub(crate) line_num: usize,
    pub(crate) label: String,
}
