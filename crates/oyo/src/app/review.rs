use super::{AnimationFrame, App, ReviewEditorToolbarAction, ReviewEditorToolbarHit, ViewMode};
use crate::config::{
    MentionFileScope, MentionFinder, ReviewActionConfig, ReviewHookConfig, ReviewHookEvent,
    ReviewHookStdin,
};
use crate::toasts::ToastEvent;
use crossterm::event::KeyEvent;
use keymap::{parser::parse_seq, ToKeyMap};
use oyo_core::{ChangeKind, LineKind, ViewLine};
use serde::{Deserialize, Serialize};
use std::collections::{hash_map::DefaultHasher, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReviewTargetKind {
    Line,
    Hunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReviewSide {
    Old,
    New,
}

impl ReviewSide {
    fn as_str(self) -> &'static str {
        match self {
            ReviewSide::Old => "old",
            ReviewSide::New => "new",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewAnchor {
    pub(crate) file_index: usize,
    pub(crate) file_path: String,
    pub(crate) kind: ReviewTargetKind,
    pub(crate) side: Option<ReviewSide>,
    pub(crate) old_range: Option<ReviewRange>,
    pub(crate) new_range: Option<ReviewRange>,
    pub(crate) hunk_id: Option<usize>,
    pub(crate) display_idx_hint: Option<usize>,
    pub(crate) anchor_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewComment {
    pub(crate) id: u64,
    pub(crate) anchor: ReviewAnchor,
    pub(crate) body: String,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewEditorState {
    pub(crate) anchor: ReviewAnchor,
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReviewSession {
    version: u32,
    repo_root: String,
    diff_fingerprint: String,
    created_at: u64,
    updated_at: u64,
    comments: Vec<ReviewComment>,
    editor: Option<ReviewEditorState>,
}

#[derive(Debug, Serialize)]
struct ReviewExport<'a> {
    version: u32,
    event: &'static str,
    repo_root: String,
    session_file: Option<String>,
    diff_fingerprint: String,
    diff: ReviewExportDiff,
    review: ReviewExportBody<'a>,
}

#[derive(Debug, Serialize)]
struct ReviewExportDiff {
    branch: Option<String>,
    range: Option<(String, String)>,
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ReviewExportBody<'a> {
    text: &'a str,
    comments: Vec<ReviewExportComment<'a>>,
}

#[derive(Debug, Serialize)]
struct ReviewExportComment<'a> {
    id: u64,
    file: &'a str,
    kind: &'static str,
    side: Option<&'static str>,
    old_range: Option<ReviewRange>,
    new_range: Option<ReviewRange>,
    body: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewEditorRender {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
    pub(crate) display_idx_hint: Option<usize>,
    pub(crate) anchor_display_span: Option<(usize, usize)>,
    pub(crate) anchor_is_hunk: bool,
    pub(crate) prefer_right: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewCommentOverlay {
    pub(crate) display_idx: usize,
    pub(crate) preview: String,
    pub(crate) body: String,
    pub(crate) title: String,
    pub(crate) anchor_key: String,
    pub(crate) prefer_right: bool,
    pub(crate) is_hunk: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewPreviewBox {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) anchor_key: String,
    pub(crate) delete: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewMentionItem {
    pub(crate) label: String,
    pub(crate) insert_text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewMentionPickerState {
    pub(crate) start: usize,
    pub(crate) query: String,
    pub(crate) items: Vec<ReviewMentionItem>,
    pub(crate) selected: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewMentionRender {
    pub(crate) query: String,
    pub(crate) items: Vec<String>,
    pub(crate) selected: usize,
    pub(crate) scroll_start: usize,
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn review_event_name(event: ReviewHookEvent) -> &'static str {
    event.as_str()
}

fn hook_label(config: &ReviewHookConfig) -> String {
    if config.id.trim().is_empty() {
        config.command.clone()
    } else {
        config.id.clone()
    }
}

fn action_label(config: &ReviewActionConfig) -> String {
    if config.label.trim().is_empty() {
        config.id.clone()
    } else {
        config.label.clone()
    }
}

fn action_shown(config: &ReviewActionConfig, place: &str) -> bool {
    config.show.is_empty() || config.show.iter().any(|item| item == place)
}

fn key_matches_config(key: KeyEvent, binding: &str) -> bool {
    let Ok(seq) = parse_seq(binding) else {
        return false;
    };
    if seq.len() != 1 {
        return false;
    }
    key.to_keymap().ok().as_ref() == seq.first()
}

fn hash_hex(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn format_opt_range(range: Option<ReviewRange>) -> String {
    match range {
        Some(range) => format!("{}-{}", range.start, range.end),
        None => "-".to_string(),
    }
}

fn format_opt_range_display(range: Option<ReviewRange>) -> String {
    match range {
        Some(range) if range.start == range.end => range.start.to_string(),
        Some(range) => format!("{}-{}", range.start, range.end),
        None => "-".to_string(),
    }
}

fn review_side_label(side: ReviewSide, range: Option<ReviewRange>) -> String {
    let prefix = match side {
        ReviewSide::Old => "L",
        ReviewSide::New => "R",
    };
    format!("{prefix}{}", format_opt_range_display(range))
}

fn review_both_sides_label(
    old_range: Option<ReviewRange>,
    new_range: Option<ReviewRange>,
) -> String {
    match (old_range, new_range) {
        (Some(old), Some(new)) => format!(
            "{}/{}",
            review_side_label(ReviewSide::Old, Some(old)),
            review_side_label(ReviewSide::New, Some(new))
        ),
        (Some(old), None) => review_side_label(ReviewSide::Old, Some(old)),
        (None, Some(new)) => review_side_label(ReviewSide::New, Some(new)),
        (None, None) => "-".to_string(),
    }
}

fn review_anchor_location_label(anchor: &ReviewAnchor) -> String {
    match anchor.kind {
        ReviewTargetKind::Line => match anchor.side {
            Some(ReviewSide::Old) => review_side_label(ReviewSide::Old, anchor.old_range),
            Some(ReviewSide::New) => review_side_label(ReviewSide::New, anchor.new_range),
            None => review_both_sides_label(anchor.old_range, anchor.new_range),
        },
        ReviewTargetKind::Hunk => review_both_sides_label(anchor.old_range, anchor.new_range),
    }
}

fn range_contains_line(range: Option<ReviewRange>, line: Option<usize>) -> bool {
    match (range, line) {
        (Some(range), Some(line)) => line >= range.start && line <= range.end,
        _ => false,
    }
}

fn line_anchor_matches(anchor: &ReviewAnchor, line: &ViewLine) -> bool {
    match anchor.side {
        Some(ReviewSide::Old) => range_contains_line(anchor.old_range, line.old_line),
        Some(ReviewSide::New) => range_contains_line(anchor.new_range, line.new_line),
        None => {
            range_contains_line(anchor.old_range, line.old_line)
                || range_contains_line(anchor.new_range, line.new_line)
        }
    }
}

fn review_comment_title(anchor: &ReviewAnchor) -> String {
    format!(
        "Comment • {} {}",
        anchor.file_path,
        review_anchor_location_label(anchor)
    )
}

fn truncate_middle_chars(text: &str, max_chars: usize) -> String {
    let len = text.chars().count();
    if len <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let keep = max_chars.saturating_sub(3);
    let head = keep / 2;
    let tail = keep.saturating_sub(head);
    let start = text.chars().take(head).collect::<String>();
    let end = text
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{start}...{end}")
}

fn wrap_note_line(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut chunk = String::new();
    for ch in line.chars() {
        chunk.push(ch);
        if chunk.chars().count() == width {
            out.push(std::mem::take(&mut chunk));
        }
    }
    if !chunk.is_empty() || out.is_empty() {
        out.push(chunk);
    }
    out
}

fn line_review_anchor_from_view_line(
    file_index: usize,
    file_path: String,
    display_idx: usize,
    line: &ViewLine,
) -> Option<ReviewAnchor> {
    line_review_anchor_from_view_line_with_side(file_index, file_path, display_idx, line, None)
}

fn line_review_anchor_from_view_line_with_side(
    file_index: usize,
    file_path: String,
    display_idx: usize,
    line: &ViewLine,
    preferred_side: Option<ReviewSide>,
) -> Option<ReviewAnchor> {
    let mut side = preferred_side.or(match line.kind {
        LineKind::Deleted | LineKind::PendingDelete => Some(ReviewSide::Old),
        LineKind::Inserted | LineKind::PendingInsert => Some(ReviewSide::New),
        LineKind::Context if line.old_line.is_some() && line.new_line.is_some() => {
            Some(ReviewSide::New)
        }
        _ => {
            if line.new_line.is_some() {
                Some(ReviewSide::New)
            } else {
                Some(ReviewSide::Old)
            }
        }
    });

    if side == Some(ReviewSide::Old) && line.old_line.is_none() && line.new_line.is_some() {
        side = Some(ReviewSide::New);
    }
    if side == Some(ReviewSide::New) && line.new_line.is_none() && line.old_line.is_some() {
        side = Some(ReviewSide::Old);
    }

    let old_range = line.old_line.map(|n| ReviewRange { start: n, end: n });
    let new_range = line.new_line.map(|n| ReviewRange { start: n, end: n });
    let line_no = match side {
        Some(ReviewSide::Old) => old_range.map(|r| r.start),
        Some(ReviewSide::New) => new_range.map(|r| r.start),
        None => old_range.or(new_range).map(|r| r.start),
    }?;

    let anchor_key = match side {
        Some(side) => format!("line|{}|{}|{}", file_path, side.as_str(), line_no),
        None => format!(
            "line|{}|both|{}|{}",
            file_path,
            format_opt_range(old_range),
            format_opt_range(new_range)
        ),
    };

    Some(ReviewAnchor {
        file_index,
        file_path,
        kind: ReviewTargetKind::Line,
        side,
        old_range,
        new_range,
        hunk_id: line.hunk_index,
        display_idx_hint: Some(display_idx),
        anchor_key,
    })
}

fn truncate_preview_chars(text: &str, max_chars: usize) -> (String, bool) {
    if max_chars == 0 {
        return (String::new(), !text.is_empty());
    }

    let total = text.chars().count();
    if total <= max_chars {
        return (text.to_string(), false);
    }

    let keep = max_chars.saturating_sub(1);
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= keep {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    (out, true)
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn cursor_row_col(text: &str, cursor: usize) -> (usize, usize) {
    let cursor = cursor.min(text.len());
    let starts = line_starts(text);
    let mut row = 0usize;
    for (idx, start) in starts.iter().enumerate() {
        if *start > cursor {
            break;
        }
        row = idx;
    }
    let line_start = starts.get(row).copied().unwrap_or(0);
    let line = &text[line_start..cursor];
    let col = line.chars().count();
    (row, col)
}

fn cursor_for_row_col(text: &str, row: usize, col: usize) -> usize {
    let starts = line_starts(text);
    if starts.is_empty() {
        return 0;
    }
    let row = row.min(starts.len().saturating_sub(1));
    let start = starts[row];
    let line_end = if row + 1 < starts.len() {
        starts[row + 1].saturating_sub(1)
    } else {
        text.len()
    };
    let line = &text[start..line_end];
    let mut idx = start;
    for (chars, ch) in line.chars().enumerate() {
        if chars >= col {
            break;
        }
        idx += ch.len_utf8();
    }
    idx
}

fn prev_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut iter = text[cursor..].char_indices();
    let _ = iter.next();
    iter.next()
        .map(|(delta, _)| cursor + delta)
        .unwrap_or(text.len())
}

fn mention_query_at_cursor(text: &str, cursor: usize) -> Option<(usize, String)> {
    let cursor = cursor.min(text.len());
    let before = &text[..cursor];
    let at = before.rfind('@')?;
    let token = &before[at + 1..];
    let valid_char =
        |ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':');
    if token.chars().any(|ch| !valid_char(ch)) {
        return None;
    }
    Some((at, token.to_string()))
}

fn is_numeric_query(query: &str) -> bool {
    !query.is_empty() && query.chars().all(|ch| ch.is_ascii_digit())
}

fn preserve_ref_trailing_space(text: &str) -> bool {
    let without_spaces = text.trim_end_matches(' ');
    if without_spaces.len() == text.len() {
        return false;
    }

    if without_spaces
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        return true;
    }

    let token_start = without_spaces
        .char_indices()
        .rev()
        .find_map(|(idx, ch)| ch.is_whitespace().then_some(idx + ch.len_utf8()))
        .unwrap_or(0);
    let tail = &without_spaces[token_start..];
    if !tail.starts_with('@') {
        return false;
    }

    let token = &tail[1..];
    if token.is_empty() {
        return false;
    }

    let valid_char =
        |ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':');
    token.chars().all(valid_char)
}

fn merge_changed_and_repo_paths(changed_paths: &[String], repo_paths: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();

    for path in changed_paths {
        if seen.insert(path.clone()) {
            out.push(path.clone());
        }
    }

    for path in repo_paths {
        if seen.insert(path.clone()) {
            out.push(path.clone());
        }
    }

    out
}

fn push_numeric_line_mention_item(
    items: &mut Vec<ReviewMentionItem>,
    current_file: &str,
    query: &str,
    side: Option<ReviewSide>,
    line_no: usize,
    limit: usize,
) {
    if items.len() >= limit {
        return;
    }
    let line_text = line_no.to_string();
    if !line_text.starts_with(query) {
        return;
    }

    let (label, insert_text) = match side {
        Some(side) => {
            let location = review_side_label(
                side,
                Some(ReviewRange {
                    start: line_no,
                    end: line_no,
                }),
            );
            (
                format!("line  {current_file} {location}"),
                format!("@{current_file}:{location}"),
            )
        }
        None => (
            format!("line  {current_file} {line_no}"),
            format!("@{current_file}:{line_no}"),
        ),
    };

    items.push(ReviewMentionItem { label, insert_text });
}

fn nearest_hunk_line_index(visible: &[(usize, ViewLine)], focus_pos: usize) -> Option<usize> {
    if visible.is_empty() {
        return None;
    }
    let focus_pos = focus_pos.min(visible.len().saturating_sub(1));
    if visible[focus_pos].1.hunk_index.is_some() {
        return Some(focus_pos);
    }

    for dist in 1..visible.len() {
        let right = focus_pos.saturating_add(dist);
        if right < visible.len() && visible[right].1.hunk_index.is_some() {
            return Some(right);
        }
        let left = focus_pos.saturating_sub(dist);
        if left < visible.len() && visible[left].1.hunk_index.is_some() {
            return Some(left);
        }
    }

    None
}

impl App {
    pub fn review_mode(&self) -> bool {
        self.review_mode
    }

    pub fn set_review_persist_enabled(&mut self, enabled: bool) {
        self.review_persist_enabled = enabled;
        if !enabled {
            self.review_session_path = None;
        }
    }

    pub fn set_review_clear_session_on_start(&mut self, enabled: bool) {
        self.review_clear_session_on_start = enabled;
    }

    pub fn review_revision(&self) -> u64 {
        self.review_revision
    }

    pub fn review_comment_count(&self) -> usize {
        self.review_comments.len()
    }

    pub(crate) fn review_comment_count_for_file(&self, file_index: usize) -> usize {
        self.review_comments
            .iter()
            .filter(|comment| comment.anchor.file_index == file_index)
            .count()
    }

    pub(crate) fn filtered_review_comment_indices(&self) -> Vec<usize> {
        let query = self.file_filter.trim().to_ascii_lowercase();
        self.review_comments
            .iter()
            .enumerate()
            .filter_map(|(idx, comment)| {
                if query.is_empty() {
                    return Some(idx);
                }
                let location = review_anchor_location_label(&comment.anchor);
                let haystack =
                    format!("{} {} {}", comment.anchor.file_path, location, comment.body)
                        .to_ascii_lowercase();
                haystack.contains(&query).then_some(idx)
            })
            .collect()
    }

    pub(crate) fn review_comment_is_active(&self, index: usize) -> bool {
        let Some(editor) = self.review_editor.as_ref() else {
            return false;
        };
        self.review_comments
            .get(index)
            .is_some_and(|comment| comment.anchor.anchor_key == editor.anchor.anchor_key)
    }

    pub(crate) fn review_comment_sidebar_item(
        &self,
        index: usize,
    ) -> Option<(usize, String, String, String)> {
        let comment = self.review_comments.get(index)?;
        let first_line = comment.body.lines().next().unwrap_or_default().trim();
        let preview = if first_line.is_empty() {
            "(empty)".to_string()
        } else {
            first_line.to_string()
        };
        Some((
            comment.anchor.file_index,
            comment.anchor.file_path.clone(),
            review_anchor_location_label(&comment.anchor),
            preview,
        ))
    }

    pub fn open_review_comment(&mut self, index: usize) -> bool {
        let Some(anchor) = self
            .review_comments
            .get(index)
            .map(|comment| comment.anchor.clone())
        else {
            return false;
        };
        self.select_file(anchor.file_index);
        self.open_review_editor(anchor);
        true
    }

    pub fn take_review_hook_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.review_hook_warnings)
    }

    pub(crate) fn review_action_entries_for_editor(&self) -> Vec<(usize, String, String)> {
        self.review_actions
            .iter()
            .enumerate()
            .filter(|(_, action)| action_shown(action, "review_editor"))
            .filter_map(|(idx, action)| {
                let label = action_label(action);
                (!label.trim().is_empty())
                    .then(|| (idx, action.key.clone().unwrap_or_default(), label))
            })
            .collect()
    }

    pub(crate) fn set_review_editor_toolbar_hits(
        &mut self,
        rect: Option<(u16, u16, u16, u16)>,
        hits: Vec<ReviewEditorToolbarHit>,
    ) {
        if hits.is_empty() {
            self.review_editor_toolbar_hover = None;
        }
        self.review_editor_toolbar_rect = rect;
        self.review_editor_toolbar_hits = hits;
    }

    pub(crate) fn clear_review_editor_toolbar(&mut self) {
        self.review_editor_toolbar_hits.clear();
        self.review_editor_toolbar_rect = None;
        self.review_editor_toolbar_scroll = 0;
        self.review_editor_toolbar_hover = None;
    }

    pub(crate) fn mouse_over_review_editor_toolbar(&self, column: u16, row: u16) -> bool {
        self.review_editor_toolbar_rect
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
    }

    pub(crate) fn scroll_review_editor_toolbar(&mut self, delta: isize) -> bool {
        let action_count = self
            .review_action_entries_for_editor()
            .len()
            .saturating_add(3);
        let max_scroll = action_count.saturating_sub(1);
        let old = self.review_editor_toolbar_scroll.min(max_scroll);
        let next = if delta.is_negative() {
            old.saturating_sub(delta.unsigned_abs())
        } else {
            old.saturating_add(delta as usize).min(max_scroll)
        };
        self.review_editor_toolbar_scroll = next;
        old != next
    }

    pub(crate) fn handle_review_editor_toolbar_click(&mut self, column: u16, row: u16) -> bool {
        if !self.mouse_over_review_editor_toolbar(column, row) {
            return false;
        }
        let Some(action) = self.review_editor_toolbar_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.action)
        }) else {
            return true;
        };
        match action {
            ReviewEditorToolbarAction::Save => self.review_save_editor(),
            ReviewEditorToolbarAction::Cancel => self.review_cancel_editor(),
            ReviewEditorToolbarAction::Mention => self.review_insert_char('@'),
            ReviewEditorToolbarAction::ScrollLeft => {
                self.scroll_review_editor_toolbar(-1);
            }
            ReviewEditorToolbarAction::ScrollRight => {
                self.scroll_review_editor_toolbar(1);
            }
            ReviewEditorToolbarAction::Custom(idx) => self.run_review_action(idx),
        }
        true
    }

    pub fn handle_review_action_key(&mut self, key: KeyEvent) -> bool {
        let idx = self.review_actions.iter().position(|action| {
            action_shown(action, "review_editor")
                && action
                    .key
                    .as_deref()
                    .is_some_and(|binding| key_matches_config(key, binding))
        });
        if let Some(idx) = idx {
            self.run_review_action(idx);
            true
        } else {
            false
        }
    }

    pub fn run_review_action(&mut self, idx: usize) {
        let Some(action) = self.review_actions.get(idx).cloned() else {
            return;
        };
        if action.save_editor && self.review_editor.is_some() {
            self.review_save_editor();
        }
        self.run_review_action_command(&action);
    }

    pub fn review_editor_active(&self) -> bool {
        self.review_editor.is_some()
    }

    pub fn review_mention_picker_active(&self) -> bool {
        self.review_mention_picker.is_some()
    }

    pub fn review_mention_render(&self) -> Option<ReviewMentionRender> {
        let picker = self.review_mention_picker.as_ref()?;
        let visible_cap = 5usize;
        let len = picker.items.len();
        let max_start = len.saturating_sub(visible_cap);
        let mut scroll_start = picker
            .selected
            .saturating_sub(visible_cap.saturating_sub(1));
        if picker.selected >= visible_cap / 2 {
            scroll_start = picker.selected.saturating_sub(visible_cap / 2);
        }
        scroll_start = scroll_start.min(max_start);

        Some(ReviewMentionRender {
            query: picker.query.clone(),
            items: picker.items.iter().map(|item| item.label.clone()).collect(),
            selected: picker.selected,
            scroll_start,
        })
    }

    pub fn review_preview_hint_text(&self, overlay: &ReviewCommentOverlay) -> String {
        let update_key = if overlay.is_hunk { "M" } else { "m" };
        let delete_key = if overlay.is_hunk { "X" } else { "x" };
        format!(
            "{} • {} to update, {} to remove",
            overlay.preview, update_key, delete_key
        )
    }

    pub(crate) fn review_preview_note_lines(
        &self,
        overlay: &ReviewCommentOverlay,
        visible_width: usize,
    ) -> Vec<String> {
        let max_width = visible_width.max(12);
        let content_width = max_width.saturating_sub(4).max(1);
        let body_lines = if overlay.body.is_empty() {
            vec!["(empty)".to_string()]
        } else {
            overlay
                .body
                .lines()
                .flat_map(|line| wrap_note_line(line, content_width))
                .collect::<Vec<_>>()
        };
        let title = truncate_middle_chars(&overlay.title, max_width.saturating_sub(4));
        let rule = "─".repeat(max_width.saturating_sub(title.chars().count().saturating_add(4)));
        let bottom_rule =
            "─".repeat(max_width.saturating_sub("x delete".chars().count().saturating_add(4)));
        let mut lines = Vec::with_capacity(body_lines.len().saturating_add(2));
        lines.push(format!("╭ {title} {rule}╮"));
        for line in body_lines {
            let padding = " ".repeat(content_width.saturating_sub(line.chars().count()));
            lines.push(format!("│ {line}{padding} │"));
        }
        lines.push(format!("╰ x delete {bottom_rule}╯"));
        lines
    }

    pub fn clear_review_preview_boxes(&mut self) {
        self.review_preview_boxes.clear();
    }

    pub fn remove_hovered_review_comment(&mut self) -> bool {
        let Some(anchor_key) = self.review_preview_hover.clone() else {
            return false;
        };
        self.review_preview_hover = None;
        self.remove_comment_for_anchor_key(&anchor_key)
    }

    pub fn add_review_preview_box(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        anchor_key: String,
    ) {
        self.review_preview_boxes.push(ReviewPreviewBox {
            x,
            y,
            width,
            height,
            anchor_key,
            delete: false,
        });
    }

    pub fn add_review_preview_delete_box(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        anchor_key: String,
    ) {
        self.review_preview_boxes.push(ReviewPreviewBox {
            x,
            y,
            width,
            height,
            anchor_key,
            delete: true,
        });
    }

    pub(crate) fn clear_review_line_add_hit(&mut self) {
        self.review_line_add_hit = None;
    }

    pub(crate) fn review_line_add_button_x(&self) -> Option<u16> {
        let (x, _, width, _) = self.diff_view_area?;
        let right = x
            .saturating_add(width)
            .saturating_sub(u16::from(self.scrollbar_visible));
        Some(right.saturating_sub(3))
    }

    pub(crate) fn review_display_idx_for_screen_row(&self, row: u16) -> Option<usize> {
        let (_, y, _, height) = self.diff_view_area?;
        if row < y || row >= y.saturating_add(height) {
            return None;
        }
        let note_boxes = self.review_preview_boxes.iter().filter(|hit| !hit.delete);
        if note_boxes.clone().any(|hit| {
            let end = hit.y.saturating_add(hit.height);
            row >= hit.y && row < end
        }) {
            return None;
        }
        let mut ranges = note_boxes
            .filter_map(|hit| {
                let end = hit.y.saturating_add(hit.height);
                (row > hit.y).then_some((hit.y, row.min(end)))
            })
            .collect::<Vec<_>>();
        ranges.sort_unstable();
        let mut reserved_rows = 0usize;
        let mut merged_end = 0u16;
        for (start, end) in ranges {
            if end <= merged_end {
                continue;
            }
            let start = start.max(merged_end);
            reserved_rows = reserved_rows.saturating_add(end.saturating_sub(start) as usize);
            merged_end = end;
        }
        Some(
            self.render_scroll_offset()
                .saturating_add((row.saturating_sub(y) as usize).saturating_sub(reserved_rows)),
        )
    }

    pub(crate) fn review_line_add_hover_at(&self, column: u16, row: u16) -> (Option<u16>, bool) {
        if !self.review_mode
            || self.view_mode == ViewMode::Preview
            || self.current_file_is_binary()
            || self.review_editor.is_some()
            || self.selection_toolbar_visible()
        {
            return (None, false);
        }
        let Some((x, y, width, height)) = self.diff_view_area else {
            return (None, false);
        };
        if column < x
            || column >= x.saturating_add(width)
            || row < y
            || row >= y.saturating_add(height)
        {
            return (None, false);
        }
        let local_row = row.saturating_sub(y) as usize;
        if self
            .diff_selection_cells
            .get(local_row)
            .is_none_or(|cells| cells.iter().all(|cell| cell.trim().is_empty()))
        {
            return (None, false);
        }
        if self.review_display_idx_for_screen_row(row).is_none() {
            return (None, false);
        }
        let Some(hit_x) = self.review_line_add_button_x() else {
            return (Some(row), false);
        };
        let hover = column >= hit_x && column < hit_x.saturating_add(3);
        (Some(row), hover)
    }

    pub fn handle_review_line_add_click(&mut self, column: u16, row: u16) -> bool {
        if !self.review_mode || self.review_editor.is_some() {
            return false;
        }
        let Some(hit) = self.review_line_add_hit else {
            return false;
        };
        if column < hit.x
            || column >= hit.x.saturating_add(hit.width)
            || row < hit.y
            || row >= hit.y.saturating_add(hit.height)
        {
            return false;
        }
        self.start_line_comment_at_screen_row(hit.row)
    }

    pub fn handle_review_preview_click(&mut self, column: u16, row: u16) -> bool {
        if !self.review_mode || self.review_editor.is_some() {
            return false;
        }

        let hit = self.review_preview_boxes.iter().rev().find_map(|hit| {
            let end_x = hit.x.saturating_add(hit.width);
            let end_y = hit.y.saturating_add(hit.height);
            (column >= hit.x && column < end_x && row >= hit.y && row < end_y)
                .then(|| (hit.anchor_key.clone(), hit.delete))
        });

        let Some((anchor_key, delete)) = hit else {
            return false;
        };
        if delete {
            return self.remove_comment_for_anchor_key(&anchor_key);
        }

        let anchor = self
            .review_comments
            .iter()
            .find(|c| c.anchor.anchor_key == anchor_key)
            .map(|c| c.anchor.clone());

        if let Some(anchor) = anchor {
            self.open_review_editor(anchor);
            true
        } else {
            false
        }
    }

    fn review_anchor_display_span(&mut self, anchor: &ReviewAnchor) -> Option<(usize, usize)> {
        let visible = self.review_visible_lines_with_idx();
        if visible.is_empty() {
            return anchor.display_idx_hint.map(|idx| (idx, idx));
        }

        let span = match anchor.kind {
            ReviewTargetKind::Line => visible
                .iter()
                .find_map(|(idx, line)| line_anchor_matches(anchor, line).then_some(*idx))
                .map(|idx| (idx, idx)),
            ReviewTargetKind::Hunk => {
                let mut start: Option<usize> = None;
                let mut end: Option<usize> = None;

                let in_range = |line: &ViewLine| {
                    let old_match = match (anchor.old_range, line.old_line) {
                        (Some(range), Some(line_no)) => {
                            line_no >= range.start && line_no <= range.end
                        }
                        _ => false,
                    };
                    let new_match = match (anchor.new_range, line.new_line) {
                        (Some(range), Some(line_no)) => {
                            line_no >= range.start && line_no <= range.end
                        }
                        _ => false,
                    };
                    old_match || new_match
                };

                for (idx, line) in &visible {
                    let matches = if let Some(hunk_id) = anchor.hunk_id {
                        line.hunk_index == Some(hunk_id) || in_range(line)
                    } else {
                        in_range(line)
                    };

                    if matches {
                        start = Some(start.map_or(*idx, |v| v.min(*idx)));
                        end = Some(end.map_or(*idx, |v| v.max(*idx)));
                    }
                }

                match (start, end) {
                    (Some(start), Some(end)) => Some((start, end)),
                    _ => None,
                }
            }
        };

        span.or_else(|| anchor.display_idx_hint.map(|idx| (idx, idx)))
    }

    pub fn review_editor_render(&mut self) -> Option<ReviewEditorRender> {
        let editor = self.review_editor.as_ref()?.clone();
        let (cursor_row, cursor_col) = cursor_row_col(&editor.text, editor.cursor);
        let mut lines: Vec<String> = if editor.text.is_empty() {
            vec![String::new()]
        } else {
            editor.text.split('\n').map(ToString::to_string).collect()
        };
        if lines.is_empty() {
            lines.push(String::new());
        }

        let anchor_label = format!(
            "{} {}",
            editor.anchor.file_path,
            review_anchor_location_label(&editor.anchor)
        );
        let title = format!(" Comment • {anchor_label} ");

        let anchor_display_span = self.review_anchor_display_span(&editor.anchor);

        let prefer_right = match editor.anchor.kind {
            ReviewTargetKind::Line => !matches!(editor.anchor.side, Some(ReviewSide::Old)),
            ReviewTargetKind::Hunk => !matches!(
                (editor.anchor.old_range, editor.anchor.new_range),
                (Some(_), None)
            ),
        };

        Some(ReviewEditorRender {
            title,
            lines,
            cursor_row,
            cursor_col,
            display_idx_hint: editor.anchor.display_idx_hint,
            anchor_display_span,
            anchor_is_hunk: matches!(editor.anchor.kind, ReviewTargetKind::Hunk),
            prefer_right,
        })
    }

    pub fn review_comment_overlays_for_current_file(&mut self) -> Vec<ReviewCommentOverlay> {
        if !self.review_mode {
            return Vec::new();
        }

        let file_path = self.current_file_path();
        if file_path.is_empty() {
            return Vec::new();
        }

        let visible = self.review_visible_lines_with_idx();
        if visible.is_empty() {
            return Vec::new();
        }

        let active_anchor_key = self
            .review_editor
            .as_ref()
            .map(|editor| editor.anchor.anchor_key.as_str());
        let mut overlays = Vec::new();
        for comment in self
            .review_comments
            .iter()
            .filter(|comment| comment.anchor.file_path == file_path)
            .filter(|comment| active_anchor_key != Some(comment.anchor.anchor_key.as_str()))
        {
            let display_idx = match comment.anchor.kind {
                ReviewTargetKind::Line => visible.iter().find_map(|(idx, line)| {
                    line_anchor_matches(&comment.anchor, line).then_some(*idx)
                }),
                ReviewTargetKind::Hunk => {
                    if let Some(hunk_id) = comment.anchor.hunk_id {
                        visible.iter().find_map(|(idx, line)| {
                            (line.hunk_index == Some(hunk_id)).then_some(*idx)
                        })
                    } else {
                        let old_range = comment.anchor.old_range;
                        let new_range = comment.anchor.new_range;
                        visible.iter().find_map(|(idx, line)| {
                            let old_match = match (old_range, line.old_line) {
                                (Some(range), Some(line_no)) => {
                                    line_no >= range.start && line_no <= range.end
                                }
                                _ => false,
                            };
                            let new_match = match (new_range, line.new_line) {
                                (Some(range), Some(line_no)) => {
                                    line_no >= range.start && line_no <= range.end
                                }
                                _ => false,
                            };
                            (old_match || new_match).then_some(*idx)
                        })
                    }
                }
            };

            let Some(display_idx) = display_idx else {
                continue;
            };

            let first_line = comment.body.lines().next().unwrap_or_default().trim();
            let (mut preview, was_truncated) = truncate_preview_chars(first_line, 50);
            let multiline = comment.body.contains('\n');
            if multiline && !was_truncated {
                preview.push_str(" …");
            }
            if preview.is_empty() {
                preview = "(empty)".to_string();
            }

            let prefer_right = match comment.anchor.kind {
                ReviewTargetKind::Line => !matches!(comment.anchor.side, Some(ReviewSide::Old)),
                ReviewTargetKind::Hunk => !matches!(
                    (comment.anchor.old_range, comment.anchor.new_range),
                    (Some(_), None)
                ),
            };

            overlays.push(ReviewCommentOverlay {
                display_idx,
                preview,
                body: comment.body.clone(),
                title: review_comment_title(&comment.anchor),
                anchor_key: comment.anchor.anchor_key.clone(),
                prefer_right,
                is_hunk: matches!(comment.anchor.kind, ReviewTargetKind::Hunk),
            });
        }

        overlays.sort_by_key(|overlay| overlay.display_idx);
        overlays
    }

    pub fn enable_review_mode(&mut self) {
        self.review_mode = true;
        self.review_submission_output = None;
        self.touch_review_state();

        let repo_root = self
            .multi_diff
            .repo_root()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        self.review_repo_root = Some(repo_root.to_string_lossy().to_string());
        self.invalidate_review_repo_file_cache();

        let diff_fingerprint = self.compute_review_diff_fingerprint();
        self.review_diff_fingerprint = diff_fingerprint.clone();

        self.review_session_created_at = now_ts();

        if !self.review_persist_enabled {
            self.review_session_path = None;
            self.review_comments.clear();
            self.review_editor = None;
            self.review_mention_picker = None;
            self.review_next_comment_id = 1;
            self.touch_review_state();
            return;
        }

        let repo_key = hash_hex(&repo_root.to_string_lossy());
        let base = std::env::temp_dir()
            .join("oyo")
            .join("review")
            .join(repo_key);
        let path = base.join(format!("{}.json", diff_fingerprint));
        self.review_session_path = Some(path.clone());

        if self.review_clear_session_on_start {
            let _ = fs::remove_file(&path);
            self.review_comments.clear();
            self.review_editor = None;
            self.review_mention_picker = None;
            self.review_next_comment_id = 1;
            self.touch_review_state();
            self.persist_review_session();
            return;
        }

        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(session) = serde_json::from_str::<ReviewSession>(&data) {
                if session.version == 1 && session.diff_fingerprint == self.review_diff_fingerprint
                {
                    self.review_session_created_at = session.created_at;
                    self.review_comments = session.comments;
                    self.review_editor = session.editor;
                    self.review_next_comment_id = self
                        .review_comments
                        .iter()
                        .map(|c| c.id)
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1);
                    self.repair_review_editor_file_index();
                    self.refresh_review_mention_picker();
                    self.touch_review_state();
                    return;
                }
            }
        }

        self.review_comments.clear();
        self.review_editor = None;
        self.review_mention_picker = None;
        self.review_next_comment_id = 1;
        self.touch_review_state();
        self.persist_review_session();
    }

    pub fn start_line_comment(&mut self) {
        if !self.review_mode {
            return;
        }
        let Some(anchor) = self.resolve_line_review_anchor() else {
            return;
        };
        self.open_review_editor(anchor);
    }

    pub fn start_line_comment_at_screen_row(&mut self, row: u16) -> bool {
        self.start_line_comment_at_screen_row_on_side(row, None)
    }

    pub(crate) fn start_line_comment_at_screen_row_on_side(
        &mut self,
        row: u16,
        preferred_side: Option<ReviewSide>,
    ) -> bool {
        if !self.review_mode {
            return false;
        }
        let Some((_, y, _, height)) = self.diff_view_area else {
            return false;
        };
        if row < y || row >= y.saturating_add(height) {
            return false;
        }
        let Some(display_idx) = self.review_display_idx_for_screen_row(row) else {
            return false;
        };
        let Some(anchor) =
            self.resolve_line_review_anchor_at_display_idx_on_side(display_idx, preferred_side)
        else {
            return false;
        };
        self.open_review_editor(anchor);
        true
    }

    pub fn start_hunk_comment(&mut self) {
        if !self.review_mode {
            return;
        }
        let Some(anchor) = self.resolve_hunk_review_anchor() else {
            return;
        };
        self.open_review_editor(anchor);
    }

    pub fn remove_line_comment_at_cursor(&mut self) -> bool {
        if !self.review_mode {
            return false;
        }
        let Some(anchor) = self.resolve_line_review_anchor() else {
            return false;
        };
        self.remove_comment_for_anchor_key(&anchor.anchor_key)
    }

    pub fn remove_hunk_comment_at_cursor(&mut self) -> bool {
        if !self.review_mode {
            return false;
        }
        let Some(anchor) = self.resolve_hunk_review_anchor() else {
            return false;
        };
        self.remove_comment_for_anchor_key(&anchor.anchor_key)
    }

    pub fn clear_all_review_comments(&mut self) -> bool {
        if !self.review_mode || self.review_comments.is_empty() {
            return false;
        }
        self.review_comments.clear();
        self.review_editor = None;
        self.review_mention_picker = None;
        self.review_next_comment_id = 1;
        self.touch_review_state();
        self.persist_review_session();
        self.run_review_hooks(ReviewHookEvent::CommentsCleared, None);
        self.notify(ToastEvent::CommentsCleared);
        true
    }

    pub fn review_mention_move_selection(&mut self, delta: isize) {
        let Some(picker) = self.review_mention_picker.as_mut() else {
            return;
        };
        if picker.items.is_empty() {
            return;
        }
        let len = picker.items.len() as isize;
        let next = (picker.selected as isize + delta).rem_euclid(len) as usize;
        picker.selected = next;
    }

    pub fn review_accept_mention(&mut self) -> bool {
        let Some(picker) = self.review_mention_picker.clone() else {
            return false;
        };
        let Some(item) = picker.items.get(picker.selected).cloned() else {
            return false;
        };
        let Some(editor) = self.review_editor.as_mut() else {
            return false;
        };

        let start = picker.start.min(editor.text.len());
        let end = editor.cursor.min(editor.text.len());
        editor.text.replace_range(start..end, &item.insert_text);
        editor.cursor = start.saturating_add(item.insert_text.len());

        self.review_mention_picker = None;
        self.touch_review_state();
        self.persist_review_session();
        true
    }

    pub fn review_cancel_mention_picker(&mut self) -> bool {
        if self.review_mention_picker.is_some() {
            self.review_mention_picker = None;
            true
        } else {
            false
        }
    }

    pub fn review_insert_char(&mut self, ch: char) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        editor.text.insert(editor.cursor, ch);
        editor.cursor += ch.len_utf8();
        self.refresh_review_mention_picker();
        self.touch_review_state();
        self.persist_review_session();
    }

    pub fn review_insert_newline(&mut self) {
        self.review_insert_char('\n');
    }

    pub fn review_backspace(&mut self) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        if editor.cursor == 0 {
            return;
        }
        let prev = prev_char_boundary(&editor.text, editor.cursor);
        editor.text.replace_range(prev..editor.cursor, "");
        editor.cursor = prev;
        self.refresh_review_mention_picker();
        self.touch_review_state();
        self.persist_review_session();
    }

    pub fn review_delete(&mut self) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        if editor.cursor >= editor.text.len() {
            return;
        }
        let next = next_char_boundary(&editor.text, editor.cursor);
        editor.text.replace_range(editor.cursor..next, "");
        self.refresh_review_mention_picker();
        self.touch_review_state();
        self.persist_review_session();
    }

    pub fn review_move_left(&mut self) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        editor.cursor = prev_char_boundary(&editor.text, editor.cursor);
        self.refresh_review_mention_picker();
    }

    pub fn review_move_right(&mut self) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        editor.cursor = next_char_boundary(&editor.text, editor.cursor);
        self.refresh_review_mention_picker();
    }

    pub fn review_move_up(&mut self) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        let (row, col) = cursor_row_col(&editor.text, editor.cursor);
        if row == 0 {
            return;
        }
        editor.cursor = cursor_for_row_col(&editor.text, row - 1, col);
        self.refresh_review_mention_picker();
    }

    pub fn review_move_down(&mut self) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        let starts = line_starts(&editor.text);
        if starts.is_empty() {
            return;
        }
        let (row, col) = cursor_row_col(&editor.text, editor.cursor);
        if row + 1 >= starts.len() {
            return;
        }
        editor.cursor = cursor_for_row_col(&editor.text, row + 1, col);
        self.refresh_review_mention_picker();
    }

    pub fn review_move_home(&mut self) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        let (row, _) = cursor_row_col(&editor.text, editor.cursor);
        editor.cursor = cursor_for_row_col(&editor.text, row, 0);
        self.refresh_review_mention_picker();
    }

    pub fn review_move_end(&mut self) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        let (row, _) = cursor_row_col(&editor.text, editor.cursor);
        let starts = line_starts(&editor.text);
        let line_end = if row + 1 < starts.len() {
            starts[row + 1].saturating_sub(1)
        } else {
            editor.text.len()
        };
        editor.cursor = line_end;
        self.refresh_review_mention_picker();
    }

    pub fn review_clear_editor_text(&mut self) {
        let Some(editor) = self.review_editor.as_mut() else {
            return;
        };
        editor.text.clear();
        editor.cursor = 0;
        self.review_mention_picker = None;
        self.touch_review_state();
        self.persist_review_session();
    }

    pub fn review_cancel_editor(&mut self) {
        self.review_editor = None;
        self.review_mention_picker = None;
        self.clear_review_editor_toolbar();
        self.touch_review_state();
        self.persist_review_session();
    }

    pub fn review_save_editor(&mut self) {
        self.review_mention_picker = None;
        self.clear_review_editor_toolbar();
        let Some(editor) = self.review_editor.take() else {
            return;
        };

        let preserve_space = preserve_ref_trailing_space(&editor.text);
        let mut body = editor.text.trim_end().to_string();
        if preserve_space {
            body.push(' ');
        }

        let existing_idx = self
            .review_comments
            .iter()
            .position(|c| c.anchor.anchor_key == editor.anchor.anchor_key);

        if body.trim().is_empty() {
            if let Some(idx) = existing_idx {
                self.review_comments.remove(idx);
                self.touch_review_state();
                self.persist_review_session();
                self.run_review_hooks(ReviewHookEvent::CommentDeleted, None);
                self.notify(ToastEvent::CommentDeleted);
            } else {
                self.touch_review_state();
                self.persist_review_session();
            }
            return;
        }

        let now = now_ts();
        if let Some(idx) = existing_idx {
            if let Some(existing) = self.review_comments.get_mut(idx) {
                existing.body = body;
                existing.anchor = editor.anchor;
                existing.updated_at = now;
            }
        } else {
            let id = self.review_next_comment_id;
            self.review_next_comment_id = self.review_next_comment_id.saturating_add(1);
            self.review_comments.push(ReviewComment {
                id,
                anchor: editor.anchor,
                body,
                created_at: now,
                updated_at: now,
            });
        }

        self.touch_review_state();
        self.persist_review_session();
        self.run_review_hooks(ReviewHookEvent::CommentSaved, None);
        self.notify(ToastEvent::CommentSaved);
    }

    pub fn submit_review_and_quit(&mut self) {
        if self.review_editor.is_some() {
            self.review_save_editor();
        }
        self.review_mention_picker = None;
        let output = self.format_review_output();
        self.run_review_hooks(ReviewHookEvent::ReviewReady, Some(&output));
        self.review_submission_output = Some(output);
        self.touch_review_state();
        self.persist_review_session();
        self.notify(ToastEvent::ReviewSubmitted);
        self.should_quit = true;
    }

    pub fn take_review_submission_output(&mut self) -> Option<String> {
        self.review_submission_output.take()
    }

    fn run_review_hooks(&mut self, event: ReviewHookEvent, output: Option<&str>) {
        let hooks: Vec<_> = self
            .review_hooks
            .iter()
            .filter(|hook| hook.on == event)
            .cloned()
            .collect();
        for hook in hooks {
            self.run_review_hook_command(&hook, event, output);
        }
    }

    fn run_review_action_command(&mut self, action: &ReviewActionConfig) {
        let hook = ReviewHookConfig {
            id: action.id.clone(),
            on: action.on,
            command: action.command.clone(),
            args: action.args.clone(),
            stdin: action.stdin,
            blocking: action.blocking,
            timeout_ms: action.timeout_ms,
        };
        let output = self.format_review_output();
        self.run_review_hook_command(&hook, action.on, Some(&output));
    }

    fn run_review_hook_command(
        &mut self,
        hook: &ReviewHookConfig,
        event: ReviewHookEvent,
        output: Option<&str>,
    ) {
        if hook.command.trim().is_empty() {
            return;
        }
        let payload = self.review_export_json(event, output);
        let mut command = Command::new(&hook.command);
        command.args(&hook.args);
        if let Some(root) = self
            .review_repo_root
            .as_deref()
            .filter(|root| !root.is_empty())
        {
            command.current_dir(root);
            command.env("OYO_REPO_ROOT", root);
        }
        command.env("OYO_REVIEW_EVENT", review_event_name(event));
        command.env("OYO_DIFF_FINGERPRINT", &self.review_diff_fingerprint);
        if let Some(path) = self.review_session_path.as_ref() {
            command.env("OYO_SESSION_FILE", path);
        }
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        match hook.stdin {
            ReviewHookStdin::Json => {
                command.stdin(Stdio::piped());
            }
            ReviewHookStdin::None => {
                command.stdin(Stdio::null());
            }
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.review_hook_warnings.push(format!(
                    "Review hook '{}' failed to start: {error}",
                    hook_label(hook)
                ));
                return;
            }
        };

        if matches!(hook.stdin, ReviewHookStdin::Json) {
            if let Some(mut stdin) = child.stdin.take() {
                if let Err(error) = stdin.write_all(payload.as_bytes()) {
                    self.review_hook_warnings.push(format!(
                        "Review hook '{}' failed to receive JSON: {error}",
                        hook_label(hook)
                    ));
                }
            }
        }

        if !hook.blocking {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return;
        }

        let timeout = Duration::from_millis(hook.timeout_ms.max(1));
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        self.review_hook_warnings.push(format!(
                            "Review hook '{}' exited with status {status}",
                            hook_label(hook)
                        ));
                    }
                    return;
                }
                Ok(None) if started.elapsed() >= timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    self.review_hook_warnings.push(format!(
                        "Review hook '{}' timed out after {}ms",
                        hook_label(hook),
                        hook.timeout_ms
                    ));
                    return;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(error) => {
                    self.review_hook_warnings.push(format!(
                        "Review hook '{}' failed while waiting: {error}",
                        hook_label(hook)
                    ));
                    return;
                }
            }
        }
    }

    fn review_export_json(&self, event: ReviewHookEvent, output: Option<&str>) -> String {
        let output_owned;
        let output = match output {
            Some(output) => output,
            None => {
                output_owned = self.format_review_output();
                &output_owned
            }
        };
        let comments = self
            .review_comments
            .iter()
            .map(|comment| ReviewExportComment {
                id: comment.id,
                file: &comment.anchor.file_path,
                kind: match comment.anchor.kind {
                    ReviewTargetKind::Line => "line",
                    ReviewTargetKind::Hunk => "hunk",
                },
                side: comment.anchor.side.map(ReviewSide::as_str),
                old_range: comment.anchor.old_range,
                new_range: comment.anchor.new_range,
                body: &comment.body,
            })
            .collect();
        let export = ReviewExport {
            version: 1,
            event: review_event_name(event),
            repo_root: self.review_repo_root.clone().unwrap_or_default(),
            session_file: self
                .review_session_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            diff_fingerprint: self.review_diff_fingerprint.clone(),
            diff: ReviewExportDiff {
                branch: self.git_branch.clone(),
                range: self.multi_diff.git_range_display(),
                files: self
                    .multi_diff
                    .files
                    .iter()
                    .map(|file| file.display_name.clone())
                    .collect(),
            },
            review: ReviewExportBody {
                text: output,
                comments,
            },
        };
        serde_json::to_string_pretty(&export).unwrap_or_else(|_| "{}".to_string())
    }

    fn touch_review_state(&mut self) {
        self.review_revision = self.review_revision.saturating_add(1);
    }

    fn remove_comment_for_anchor_key(&mut self, anchor_key: &str) -> bool {
        if let Some(idx) = self
            .review_comments
            .iter()
            .position(|c| c.anchor.anchor_key == anchor_key)
        {
            self.review_comments.remove(idx);
            self.touch_review_state();
            self.persist_review_session();
            self.run_review_hooks(ReviewHookEvent::CommentDeleted, None);
            self.notify(ToastEvent::CommentDeleted);
            true
        } else {
            false
        }
    }

    fn open_review_editor(&mut self, anchor: ReviewAnchor) {
        self.clear_diff_selection();
        let text = self
            .review_comments
            .iter()
            .find(|c| c.anchor.anchor_key == anchor.anchor_key)
            .map(|c| c.body.clone())
            .unwrap_or_default();
        let cursor = text.len();
        self.review_editor = Some(ReviewEditorState {
            anchor,
            text,
            cursor,
        });
        self.refresh_review_mention_picker();
        self.touch_review_state();
        self.stop_command_palette();
        self.stop_file_search();
        self.stop_file_filter();
        self.clear_search();
        self.clear_goto();
        self.persist_review_session();
    }

    fn refresh_review_mention_picker(&mut self) {
        let (start, query) = {
            let Some(editor) = self.review_editor.as_ref() else {
                self.review_mention_picker = None;
                return;
            };
            let Some((start, query)) = mention_query_at_cursor(&editor.text, editor.cursor) else {
                self.review_mention_picker = None;
                return;
            };
            (start, query)
        };

        let items = self.review_mention_candidates(&query);
        if items.is_empty() {
            self.review_mention_picker = None;
            return;
        }

        let selected = self
            .review_mention_picker
            .as_ref()
            .and_then(|picker| {
                (picker.start == start && picker.query == query)
                    .then_some(picker.selected.min(items.len().saturating_sub(1)))
            })
            .unwrap_or(0);

        self.review_mention_picker = Some(ReviewMentionPickerState {
            start,
            query,
            items,
            selected,
        });
    }

    pub(crate) fn invalidate_review_repo_file_cache(&mut self) {
        self.review_repo_file_cache = None;
    }

    fn review_mention_fzf_available(&mut self) -> bool {
        if let Some(available) = self.review_mention_fzf_available {
            return available;
        }

        let available = Command::new("fzf")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);

        self.review_mention_fzf_available = Some(available);
        available
    }

    fn review_changed_file_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        let mut seen = BTreeSet::new();
        for file in &self.multi_diff.files {
            let path = file.display_name.clone();
            if seen.insert(path.clone()) {
                paths.push(path);
            }
        }
        paths
    }

    fn load_review_repo_file_paths(&self) -> Option<Vec<String>> {
        let repo_root = self.multi_diff.repo_root()?;
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let mut paths: Vec<String> = output
            .stdout
            .split(|b| *b == 0)
            .filter(|raw| !raw.is_empty())
            .map(|raw| String::from_utf8_lossy(raw).into_owned())
            .collect();
        paths.sort();
        paths.dedup();
        Some(paths)
    }

    fn review_repo_file_paths(&mut self) -> Vec<String> {
        if self.review_repo_file_cache.is_none() {
            self.review_repo_file_cache = self.load_review_repo_file_paths();
        }
        self.review_repo_file_cache.clone().unwrap_or_default()
    }

    fn review_mention_file_paths(&mut self) -> Vec<String> {
        let changed_paths = self.review_changed_file_paths();
        let mut paths = match self.review_mention_file_scope {
            MentionFileScope::Changed => changed_paths.clone(),
            MentionFileScope::Repo => {
                let repo_paths = self.review_repo_file_paths();
                if repo_paths.is_empty() {
                    changed_paths.clone()
                } else {
                    merge_changed_and_repo_paths(&changed_paths, &repo_paths)
                }
            }
        };

        let current_file = self.current_file_path();
        if !current_file.is_empty() {
            if let Some(pos) = paths.iter().position(|p| p == &current_file) {
                let current = paths.remove(pos);
                paths.insert(0, current);
            }
        }

        paths
    }

    fn review_current_file_line_counts(&self) -> (usize, usize) {
        let Some((old, new)) = self
            .multi_diff
            .file_contents(self.multi_diff.selected_index)
        else {
            return (0, 0);
        };

        let old_total = if old.is_empty() {
            0
        } else {
            old.lines().count()
        };
        let new_total = if new.is_empty() {
            0
        } else {
            new.lines().count()
        };
        (old_total, new_total)
    }

    fn review_changed_line_numbers(&mut self) -> (BTreeSet<usize>, BTreeSet<usize>) {
        let diff = self.multi_diff.current_navigator().diff();
        let mut old_changed = BTreeSet::new();
        let mut new_changed = BTreeSet::new();

        for change in &diff.changes {
            for span in &change.spans {
                if span.kind == ChangeKind::Equal {
                    continue;
                }
                if let Some(line_no) = span.old_line {
                    old_changed.insert(line_no);
                }
                if let Some(line_no) = span.new_line {
                    new_changed.insert(line_no);
                }
            }
        }

        (old_changed, new_changed)
    }

    fn review_numeric_line_mention_candidates(
        &mut self,
        current_file: &str,
        query: &str,
        limit: usize,
    ) -> Vec<ReviewMentionItem> {
        if current_file.is_empty() || limit == 0 {
            return Vec::new();
        }

        let (old_total, new_total) = self.review_current_file_line_counts();
        if old_total == 0 && new_total == 0 {
            return Vec::new();
        }

        let (old_changed, new_changed) = self.review_changed_line_numbers();
        let mut items: Vec<ReviewMentionItem> = Vec::new();

        for line_no in 1..=new_total {
            if new_changed.contains(&line_no) {
                push_numeric_line_mention_item(
                    &mut items,
                    current_file,
                    query,
                    Some(ReviewSide::New),
                    line_no,
                    limit,
                );
            }
        }
        for line_no in 1..=old_total {
            if old_changed.contains(&line_no) {
                push_numeric_line_mention_item(
                    &mut items,
                    current_file,
                    query,
                    Some(ReviewSide::Old),
                    line_no,
                    limit,
                );
            }
        }

        if items.len() >= limit {
            return items;
        }

        let common = old_total.min(new_total);
        for line_no in 1..=common {
            if !old_changed.contains(&line_no) && !new_changed.contains(&line_no) {
                push_numeric_line_mention_item(
                    &mut items,
                    current_file,
                    query,
                    Some(ReviewSide::New),
                    line_no,
                    limit,
                );
            }
        }

        if items.len() >= limit {
            return items;
        }

        for line_no in (common + 1)..=new_total {
            if !new_changed.contains(&line_no) {
                push_numeric_line_mention_item(
                    &mut items,
                    current_file,
                    query,
                    Some(ReviewSide::New),
                    line_no,
                    limit,
                );
            }
        }
        for line_no in (common + 1)..=old_total {
            if !old_changed.contains(&line_no) {
                push_numeric_line_mention_item(
                    &mut items,
                    current_file,
                    query,
                    Some(ReviewSide::Old),
                    line_no,
                    limit,
                );
            }
        }

        items
    }

    fn filter_review_file_paths_builtin(
        paths: &[String],
        query: &str,
        limit: usize,
    ) -> Vec<String> {
        if paths.is_empty() || limit == 0 {
            return Vec::new();
        }

        let query_lc = query.to_ascii_lowercase();
        if query_lc.is_empty() {
            return paths.iter().take(limit).cloned().collect();
        }

        let mut scored: Vec<(usize, usize, usize)> = Vec::new();
        for (idx, path) in paths.iter().enumerate() {
            let path_lc = path.to_ascii_lowercase();
            let Some(pos) = path_lc.find(&query_lc) else {
                continue;
            };
            let filename = path_lc.rsplit(['/', '\\']).next().unwrap_or(&path_lc);
            let tier = if filename.starts_with(&query_lc) {
                0
            } else if pos == 0 {
                1
            } else {
                2
            };
            scored.push((tier, pos, idx));
        }

        scored.sort_unstable();
        scored
            .into_iter()
            .take(limit)
            .map(|(_, _, idx)| paths[idx].clone())
            .collect()
    }

    fn filter_review_file_paths_with_fzf(
        &self,
        paths: &[String],
        query: &str,
        limit: usize,
    ) -> Option<Vec<String>> {
        if query.is_empty() || paths.is_empty() || limit == 0 {
            return None;
        }

        let mut child = Command::new("fzf")
            .arg("--filter")
            .arg(query)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        {
            let mut stdin = child.stdin.take()?;
            for path in paths {
                if writeln!(stdin, "{path}").is_err() {
                    return None;
                }
            }
        }

        let output = child.wait_with_output().ok()?;
        // fzf exits with status 1 when no match is found.
        if !output.status.success() && output.status.code() != Some(1) {
            return None;
        }

        let mut out: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect();
        if out.len() > limit {
            out.truncate(limit);
        }
        Some(out)
    }

    fn filter_review_file_paths(
        &mut self,
        paths: &[String],
        query: &str,
        limit: usize,
    ) -> Vec<String> {
        let builtin = || Self::filter_review_file_paths_builtin(paths, query, limit);

        match self.review_mention_finder {
            MentionFinder::Builtin => builtin(),
            MentionFinder::Fzf => self
                .filter_review_file_paths_with_fzf(paths, query, limit)
                .unwrap_or_else(builtin),
            MentionFinder::Auto => {
                if query.is_empty() || !self.review_mention_fzf_available() {
                    builtin()
                } else {
                    self.filter_review_file_paths_with_fzf(paths, query, limit)
                        .unwrap_or_else(builtin)
                }
            }
        }
    }

    fn review_mention_candidates(&mut self, query: &str) -> Vec<ReviewMentionItem> {
        const MAX_ITEMS: usize = 40;
        const MAX_REF_ITEMS: usize = 16;

        let query_lc = query.to_ascii_lowercase();
        let matches_query = |text: &str| {
            query_lc.is_empty() || text.to_ascii_lowercase().contains(query_lc.as_str())
        };

        let current_file = self.current_file_path();
        let mut items: Vec<ReviewMentionItem> = Vec::new();

        // Empty query ordering: changed files -> line refs -> repo files.
        if query.is_empty() {
            let changed_paths = self.review_changed_file_paths();
            for path in &changed_paths {
                if items.len() >= MAX_ITEMS {
                    break;
                }
                items.push(ReviewMentionItem {
                    label: format!("file  {path}"),
                    insert_text: format!("@{path}"),
                });
            }

            if items.len() < MAX_ITEMS && !current_file.is_empty() {
                let mut seen: BTreeSet<String> = BTreeSet::new();
                let mut ref_count = 0usize;
                for (_, line) in self.review_visible_lines_with_idx() {
                    if line.hunk_index.is_none() {
                        continue;
                    }

                    if let Some(line_no) = line.new_line {
                        let location = review_side_label(
                            ReviewSide::New,
                            Some(ReviewRange {
                                start: line_no,
                                end: line_no,
                            }),
                        );
                        let mention = format!("@{current_file}:{location}");
                        if ref_count < MAX_REF_ITEMS && seen.insert(mention.clone()) {
                            items.push(ReviewMentionItem {
                                label: format!("line  {current_file} {location}"),
                                insert_text: mention,
                            });
                            ref_count += 1;
                        }
                    }
                    if let Some(line_no) = line.old_line {
                        let location = review_side_label(
                            ReviewSide::Old,
                            Some(ReviewRange {
                                start: line_no,
                                end: line_no,
                            }),
                        );
                        let mention = format!("@{current_file}:{location}");
                        if ref_count < MAX_REF_ITEMS && seen.insert(mention.clone()) {
                            items.push(ReviewMentionItem {
                                label: format!("line  {current_file} {location}"),
                                insert_text: mention,
                            });
                            ref_count += 1;
                        }
                    }

                    if ref_count >= MAX_REF_ITEMS || items.len() >= MAX_ITEMS {
                        break;
                    }
                }
            }

            if items.len() < MAX_ITEMS && self.review_mention_file_scope == MentionFileScope::Repo {
                let changed_set: BTreeSet<String> = changed_paths.into_iter().collect();
                for path in self.review_repo_file_paths() {
                    if changed_set.contains(&path) {
                        continue;
                    }
                    if items.len() >= MAX_ITEMS {
                        break;
                    }
                    items.push(ReviewMentionItem {
                        label: format!("file  {path}"),
                        insert_text: format!("@{path}"),
                    });
                }
            }

            return items;
        }

        // Numeric query (`@123`): show only line refs, with changed lines first.
        if is_numeric_query(query) {
            return self.review_numeric_line_mention_candidates(&current_file, query, MAX_ITEMS);
        }

        // Non-empty non-numeric query: file mentions first (fzf in auto/fzf mode), then line refs.
        let file_paths = self.review_mention_file_paths();
        let file_paths = self.filter_review_file_paths(&file_paths, query, MAX_ITEMS);
        for path in file_paths {
            let insert_text = format!("@{path}");
            items.push(ReviewMentionItem {
                label: format!("file  {path}"),
                insert_text,
            });
        }

        if items.len() >= MAX_ITEMS || current_file.is_empty() {
            return items;
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut ref_count = 0usize;
        for (_, line) in self.review_visible_lines_with_idx() {
            if line.hunk_index.is_none() {
                continue;
            }

            if let Some(line_no) = line.new_line {
                let location = review_side_label(
                    ReviewSide::New,
                    Some(ReviewRange {
                        start: line_no,
                        end: line_no,
                    }),
                );
                let mention = format!("@{current_file}:{location}");
                if ref_count < MAX_REF_ITEMS
                    && seen.insert(mention.clone())
                    && matches_query(&mention)
                {
                    items.push(ReviewMentionItem {
                        label: format!("line  {current_file} {location}"),
                        insert_text: mention,
                    });
                    ref_count += 1;
                }
            }
            if let Some(line_no) = line.old_line {
                let location = review_side_label(
                    ReviewSide::Old,
                    Some(ReviewRange {
                        start: line_no,
                        end: line_no,
                    }),
                );
                let mention = format!("@{current_file}:{location}");
                if ref_count < MAX_REF_ITEMS
                    && seen.insert(mention.clone())
                    && matches_query(&mention)
                {
                    items.push(ReviewMentionItem {
                        label: format!("line  {current_file} {location}"),
                        insert_text: mention,
                    });
                    ref_count += 1;
                }
            }

            if ref_count >= MAX_REF_ITEMS || items.len() >= MAX_ITEMS {
                break;
            }
        }

        if items.len() > MAX_ITEMS {
            items.truncate(MAX_ITEMS);
        }
        items
    }

    fn resolve_line_review_anchor(&mut self) -> Option<ReviewAnchor> {
        let file_index = self.multi_diff.selected_index;
        let file_path = self.current_file_path();
        if file_path.is_empty() {
            return None;
        }

        let target_offset = if self.view_windowed() {
            self.render_scroll_offset()
        } else {
            self.scroll_offset
        };

        let visible = self.review_visible_lines_with_idx();
        if visible.is_empty() {
            return None;
        }

        let focus_display_idx = visible
            .iter()
            .find_map(|(idx, line)| line.is_primary_active.then_some(*idx))
            .or_else(|| {
                visible
                    .iter()
                    .find_map(|(idx, line)| line.is_active.then_some(*idx))
            })
            .unwrap_or(target_offset);

        let mut pos = visible.partition_point(|(idx, _)| *idx < focus_display_idx);
        if pos >= visible.len() {
            pos = visible.len().saturating_sub(1);
        }

        let chosen = nearest_hunk_line_index(&visible, pos)?;

        let (display_idx, line) = &visible[chosen];
        line_review_anchor_from_view_line(file_index, file_path, *display_idx, line)
    }

    fn resolve_line_review_anchor_at_display_idx_on_side(
        &mut self,
        display_idx: usize,
        preferred_side: Option<ReviewSide>,
    ) -> Option<ReviewAnchor> {
        let file_index = self.multi_diff.selected_index;
        let file_path = self.current_file_path();
        if file_path.is_empty() {
            return None;
        }
        let visible = self.review_visible_lines_with_idx();
        let (_, line) = visible.iter().find(|(idx, _)| *idx == display_idx)?;
        line_review_anchor_from_view_line_with_side(
            file_index,
            file_path,
            display_idx,
            line,
            preferred_side,
        )
    }

    fn resolve_hunk_review_anchor(&mut self) -> Option<ReviewAnchor> {
        let file_index = self.multi_diff.selected_index;
        let file_path = self.current_file_path();
        if file_path.is_empty() {
            return None;
        }

        let target_offset = if self.view_windowed() {
            self.render_scroll_offset()
        } else {
            self.scroll_offset
        };

        let visible = self.review_visible_lines_with_idx();
        if visible.is_empty() {
            return None;
        }

        let focus_display_idx = visible
            .iter()
            .find_map(|(idx, line)| line.is_primary_active.then_some(*idx))
            .or_else(|| {
                visible
                    .iter()
                    .find_map(|(idx, line)| line.is_active.then_some(*idx))
            })
            .unwrap_or(target_offset);

        let mut pos = visible.partition_point(|(idx, _)| *idx < focus_display_idx);
        if pos >= visible.len() {
            pos = visible.len().saturating_sub(1);
        }

        let chosen = nearest_hunk_line_index(&visible, pos)?;

        let hunk_idx = visible[chosen].1.hunk_index?;

        let mut old_start: Option<usize> = None;
        let mut old_end: Option<usize> = None;
        let mut new_start: Option<usize> = None;
        let mut new_end: Option<usize> = None;

        for (_, line) in visible.iter() {
            if line.hunk_index != Some(hunk_idx) {
                continue;
            }
            if let Some(old_line) = line.old_line {
                old_start = Some(old_start.map_or(old_line, |v| v.min(old_line)));
                old_end = Some(old_end.map_or(old_line, |v| v.max(old_line)));
            }
            if let Some(new_line) = line.new_line {
                new_start = Some(new_start.map_or(new_line, |v| v.min(new_line)));
                new_end = Some(new_end.map_or(new_line, |v| v.max(new_line)));
            }
        }

        let old_range = match (old_start, old_end) {
            (Some(start), Some(end)) => Some(ReviewRange { start, end }),
            _ => None,
        };
        let new_range = match (new_start, new_end) {
            (Some(start), Some(end)) => Some(ReviewRange { start, end }),
            _ => None,
        };

        let display_idx_hint = visible
            .iter()
            .find_map(|(idx, line)| (line.hunk_index == Some(hunk_idx)).then_some(*idx));

        let anchor_key = format!(
            "hunk|{}|{}|{}",
            file_path,
            format_opt_range(old_range),
            format_opt_range(new_range)
        );

        Some(ReviewAnchor {
            file_index,
            file_path,
            kind: ReviewTargetKind::Hunk,
            side: None,
            old_range,
            new_range,
            hunk_id: Some(hunk_idx),
            display_idx_hint,
            anchor_key,
        })
    }

    fn review_visible_lines_with_idx(&mut self) -> Vec<(usize, ViewLine)> {
        let view = self.current_view_with_frame(AnimationFrame::Idle);
        let mut out = Vec::new();
        let mut display_idx = 0usize;
        for line in view.iter() {
            let visible = match self.view_mode {
                ViewMode::Evolution => {
                    !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                }
                _ => true,
            };
            if !visible {
                continue;
            }
            out.push((display_idx, line.clone()));
            display_idx += 1;
        }
        out
    }

    fn persist_review_session(&mut self) {
        if !self.review_mode || !self.review_persist_enabled {
            return;
        }
        let Some(path) = self.review_session_path.as_ref() else {
            return;
        };

        let Some(parent) = path.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }

        let session = ReviewSession {
            version: 1,
            repo_root: self.review_repo_root.clone().unwrap_or_default(),
            diff_fingerprint: self.review_diff_fingerprint.clone(),
            created_at: self.review_session_created_at,
            updated_at: now_ts(),
            comments: self.review_comments.clone(),
            editor: self.review_editor.clone(),
        };

        if let Ok(serialized) = serde_json::to_string_pretty(&session) {
            let _ = fs::write(path, serialized);
        }
    }

    fn repair_review_editor_file_index(&mut self) {
        if let Some(editor) = self.review_editor.as_mut() {
            if let Some(idx) = self
                .multi_diff
                .files
                .iter()
                .position(|f| f.display_name == editor.anchor.file_path)
            {
                editor.anchor.file_index = idx;
            }
        }
    }

    fn compute_review_diff_fingerprint(&self) -> String {
        let mut hasher = DefaultHasher::new();
        if let Some(root) = self.multi_diff.repo_root() {
            root.to_string_lossy().hash(&mut hasher);
        }
        self.multi_diff.file_count().hash(&mut hasher);
        for file in &self.multi_diff.files {
            file.display_name.hash(&mut hasher);
            file.path.to_string_lossy().hash(&mut hasher);
            format!("{:?}", file.status).hash(&mut hasher);
            file.insertions.hash(&mut hasher);
            file.deletions.hash(&mut hasher);
        }
        if let Some((from, to)) = self.multi_diff.git_range_display() {
            from.hash(&mut hasher);
            to.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    fn format_review_output(&self) -> String {
        let mut comments = self.review_comments.clone();
        comments.sort_by(|a, b| {
            a.anchor
                .file_path
                .cmp(&b.anchor.file_path)
                .then_with(|| match (a.anchor.kind, b.anchor.kind) {
                    (ReviewTargetKind::Line, ReviewTargetKind::Hunk) => std::cmp::Ordering::Less,
                    (ReviewTargetKind::Hunk, ReviewTargetKind::Line) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                })
                .then_with(|| {
                    let a_line = a
                        .anchor
                        .new_range
                        .or(a.anchor.old_range)
                        .map(|r| r.start)
                        .unwrap_or(usize::MAX);
                    let b_line = b
                        .anchor
                        .new_range
                        .or(b.anchor.old_range)
                        .map(|r| r.start)
                        .unwrap_or(usize::MAX);
                    a_line.cmp(&b_line)
                })
        });

        comments
            .iter()
            .enumerate()
            .map(|(idx, comment)| Self::format_review_comment(comment, idx + 1))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn format_review_comment(comment: &ReviewComment, index: usize) -> String {
        let anchor = &comment.anchor;
        let mut lines = vec![
            format!("=== Comment {index} ==="),
            format!("File: {}", anchor.file_path),
        ];

        lines.push(format!(
            "Location: {}",
            review_anchor_location_label(anchor)
        ));

        lines.push("Body:".to_string());
        let body = comment.body.trim_end();
        if body.is_empty() {
            lines.push("  (empty)".to_string());
        } else {
            lines.extend(body.lines().map(|line| format!("  {line}")));
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ViewMode;
    use oyo_core::MultiFileDiff;
    use std::path::Path;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oyo-review-hook-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn test_app() -> App {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(
            diff,
            ViewMode::UnifiedPane,
            0,
            false,
            Some("branch".to_string()),
        );
        app.enable_review_mode();
        app
    }

    #[test]
    fn preserve_space_for_trailing_line_reference() {
        assert!(preserve_ref_trailing_space("see @foo/new.rs:new:1234 "));
        assert!(preserve_ref_trailing_space("1234 "));
    }

    #[test]
    fn preserve_space_for_trailing_file_reference() {
        assert!(preserve_ref_trailing_space("see @foo/new.rs "));
        assert!(preserve_ref_trailing_space("@foo/new.rs "));
        assert!(preserve_ref_trailing_space("see\n@foo/new.rs:old:abc "));
    }

    #[test]
    fn do_not_preserve_unrelated_trailing_space() {
        assert!(!preserve_ref_trailing_space("plain text "));
        assert!(!preserve_ref_trailing_space("@ "));
        assert!(!preserve_ref_trailing_space("no trailing space"));
        assert!(!preserve_ref_trailing_space("ends with tab\t"));
    }

    #[test]
    fn review_export_json_contains_comments() {
        let mut app = test_app();
        app.review_comments.push(ReviewComment {
            id: 7,
            anchor: ReviewAnchor {
                file_index: 0,
                file_path: "new.txt".to_string(),
                kind: ReviewTargetKind::Line,
                side: Some(ReviewSide::New),
                old_range: None,
                new_range: Some(ReviewRange { start: 1, end: 1 }),
                hunk_id: Some(0),
                display_idx_hint: Some(0),
                anchor_key: "line|new.txt|new|1".to_string(),
            },
            body: "please fix".to_string(),
            created_at: 1,
            updated_at: 1,
        });

        let json = app.review_export_json(ReviewHookEvent::ReviewReady, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["event"], "review_ready");
        assert_eq!(value["diff"]["branch"], "branch");
        assert_eq!(value["review"]["comments"][0]["file"], "new.txt");
        assert_eq!(value["review"]["comments"][0]["body"], "please fix");
    }

    #[test]
    fn review_action_key_runs_command_and_shows_label() {
        let root = temp_path("action");
        let hook_path = root.join("hook.sh");
        let out_path = root.join("action.json");
        write_file(
            &hook_path,
            &format!("#!/bin/sh\ncat > '{}'\n", out_path.display()),
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&hook_path, perms).unwrap();
        }

        let mut app = test_app();
        app.review_actions = vec![ReviewActionConfig {
            id: "capture-action".to_string(),
            label: "Capture".to_string(),
            key: Some("ctrl-g".to_string()),
            on: ReviewHookEvent::ReviewReady,
            command: hook_path.to_string_lossy().to_string(),
            args: Vec::new(),
            stdin: ReviewHookStdin::Json,
            blocking: true,
            timeout_ms: 5_000,
            save_editor: true,
            show: vec!["review_editor".to_string()],
        }];

        assert_eq!(
            app.review_action_entries_for_editor(),
            vec![(0, "ctrl-g".to_string(), "Capture".to_string())]
        );
        assert!(app.handle_review_action_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('g'),
            crossterm::event::KeyModifiers::CONTROL,
        )));

        let payload = std::fs::read_to_string(&out_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["event"], "review_ready");
        assert!(app.take_review_hook_warnings().is_empty());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn review_ready_hook_receives_json() {
        let root = temp_path("ready");
        let hook_path = root.join("hook.sh");
        let out_path = root.join("payload.json");
        write_file(
            &hook_path,
            &format!("#!/bin/sh\ncat > '{}'\n", out_path.display()),
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&hook_path, perms).unwrap();
        }

        let mut app = test_app();
        app.review_hooks = vec![ReviewHookConfig {
            id: "capture".to_string(),
            on: ReviewHookEvent::ReviewReady,
            command: hook_path.to_string_lossy().to_string(),
            args: Vec::new(),
            stdin: ReviewHookStdin::Json,
            blocking: true,
            timeout_ms: 5_000,
        }];
        app.submit_review_and_quit();

        let payload = std::fs::read_to_string(&out_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["event"], "review_ready");
        assert!(app.take_review_hook_warnings().is_empty());

        let _ = std::fs::remove_dir_all(root);
    }
}
