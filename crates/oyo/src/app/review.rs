use super::{AnimationFrame, App, ViewMode};
use crate::config::{MentionFileScope, MentionFinder};
use oyo_core::{ChangeKind, LineKind, ViewLine};
use serde::{Deserialize, Serialize};
use std::collections::{hash_map::DefaultHasher, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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

#[derive(Debug, Clone)]
pub(crate) struct ReviewEditorRender {
    pub(crate) title: String,
    pub(crate) anchor_label: String,
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
            let side_label = side.as_str();
            (
                format!("line  {}:{}:{}", current_file, side_label, line_no),
                format!("@{}:{}:{}", current_file, side_label, line_no),
            )
        }
        None => (
            format!("line  {}:{}", current_file, line_no),
            format!("@{}:{}", current_file, line_no),
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

    pub fn clear_review_preview_boxes(&mut self) {
        self.review_preview_boxes.clear();
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
        });
    }

    pub fn handle_review_preview_click(&mut self, column: u16, row: u16) -> bool {
        if !self.review_mode || self.review_editor.is_some() {
            return false;
        }

        let anchor_key = self.review_preview_boxes.iter().rev().find_map(|hit| {
            let end_x = hit.x.saturating_add(hit.width);
            let end_y = hit.y.saturating_add(hit.height);
            (column >= hit.x && column < end_x && row >= hit.y && row < end_y)
                .then_some(hit.anchor_key.clone())
        });

        let Some(anchor_key) = anchor_key else {
            return false;
        };

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
            ReviewTargetKind::Line => {
                let side = anchor.side.unwrap_or(ReviewSide::New);
                let target_line = match side {
                    ReviewSide::Old => anchor.old_range.map(|r| r.start),
                    ReviewSide::New => anchor.new_range.map(|r| r.start),
                };
                let Some(target_line) = target_line else {
                    return anchor.display_idx_hint.map(|idx| (idx, idx));
                };

                visible
                    .iter()
                    .find_map(|(idx, line)| {
                        let line_no = match side {
                            ReviewSide::Old => line.old_line,
                            ReviewSide::New => line.new_line,
                        };
                        (line_no == Some(target_line)).then_some(*idx)
                    })
                    .map(|idx| (idx, idx))
            }
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

        let title = " Comment ".to_string();

        let anchor_label = match editor.anchor.kind {
            ReviewTargetKind::Line => {
                let side = editor.anchor.side.map(|s| s.as_str()).unwrap_or("new");
                let range = match editor.anchor.side {
                    Some(ReviewSide::Old) => editor.anchor.old_range,
                    Some(ReviewSide::New) | None => editor.anchor.new_range,
                };
                let range = format_opt_range_display(range);
                format!("{} {}:{}", editor.anchor.file_path, side, range)
            }
            ReviewTargetKind::Hunk => format!(
                "{} old:{} new:{}",
                editor.anchor.file_path,
                format_opt_range_display(editor.anchor.old_range),
                format_opt_range_display(editor.anchor.new_range)
            ),
        };

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
            anchor_label,
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

        let mut overlays = Vec::new();
        for comment in self
            .review_comments
            .iter()
            .filter(|comment| comment.anchor.file_path == file_path)
        {
            let display_idx = match comment.anchor.kind {
                ReviewTargetKind::Line => {
                    let side = comment.anchor.side.unwrap_or(ReviewSide::New);
                    let target_line = match side {
                        ReviewSide::Old => comment.anchor.old_range.map(|r| r.start),
                        ReviewSide::New => comment.anchor.new_range.map(|r| r.start),
                    };
                    let Some(target_line) = target_line else {
                        continue;
                    };
                    visible.iter().find_map(|(idx, line)| {
                        line.hunk_index?;
                        let line_no = match side {
                            ReviewSide::Old => line.old_line,
                            ReviewSide::New => line.new_line,
                        };
                        (line_no == Some(target_line)).then_some(*idx)
                    })
                }
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
                anchor_key: comment.anchor.anchor_key.clone(),
                prefer_right,
                is_hunk: matches!(comment.anchor.kind, ReviewTargetKind::Hunk),
            });
        }

        overlays.sort_by(|a, b| a.display_idx.cmp(&b.display_idx));
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
        self.touch_review_state();
        self.persist_review_session();
    }

    pub fn review_save_editor(&mut self) {
        self.review_mention_picker = None;
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
            }
            self.touch_review_state();
            self.persist_review_session();
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
    }

    pub fn submit_review_and_quit(&mut self) {
        if self.review_editor.is_some() {
            self.review_save_editor();
        }
        self.review_mention_picker = None;
        let output = self.format_review_output();
        self.review_submission_output = Some(output);
        self.touch_review_state();
        self.persist_review_session();
        self.should_quit = true;
    }

    pub fn take_review_submission_output(&mut self) -> Option<String> {
        self.review_submission_output.take()
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
            true
        } else {
            false
        }
    }

    fn open_review_editor(&mut self, anchor: ReviewAnchor) {
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
                    None,
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
                        let mention = format!("@{}:new:{}", current_file, line_no);
                        if ref_count < MAX_REF_ITEMS && seen.insert(mention.clone()) {
                            items.push(ReviewMentionItem {
                                label: format!("line  {}:new:{}", current_file, line_no),
                                insert_text: mention,
                            });
                            ref_count += 1;
                        }
                    }
                    if let Some(line_no) = line.old_line {
                        let mention = format!("@{}:old:{}", current_file, line_no);
                        if ref_count < MAX_REF_ITEMS && seen.insert(mention.clone()) {
                            items.push(ReviewMentionItem {
                                label: format!("line  {}:old:{}", current_file, line_no),
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
                let mention = format!("@{}:new:{}", current_file, line_no);
                if ref_count < MAX_REF_ITEMS
                    && seen.insert(mention.clone())
                    && matches_query(&mention)
                {
                    items.push(ReviewMentionItem {
                        label: format!("line  {}:new:{}", current_file, line_no),
                        insert_text: mention,
                    });
                    ref_count += 1;
                }
            }
            if let Some(line_no) = line.old_line {
                let mention = format!("@{}:old:{}", current_file, line_no);
                if ref_count < MAX_REF_ITEMS
                    && seen.insert(mention.clone())
                    && matches_query(&mention)
                {
                    items.push(ReviewMentionItem {
                        label: format!("line  {}:old:{}", current_file, line_no),
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
        let mut side = match line.kind {
            LineKind::Deleted | LineKind::PendingDelete => ReviewSide::Old,
            LineKind::Inserted | LineKind::PendingInsert => ReviewSide::New,
            _ => {
                if line.new_line.is_some() {
                    ReviewSide::New
                } else {
                    ReviewSide::Old
                }
            }
        };

        if side == ReviewSide::Old && line.old_line.is_none() && line.new_line.is_some() {
            side = ReviewSide::New;
        }
        if side == ReviewSide::New && line.new_line.is_none() && line.old_line.is_some() {
            side = ReviewSide::Old;
        }

        let old_range = line.old_line.map(|n| ReviewRange { start: n, end: n });
        let new_range = line.new_line.map(|n| ReviewRange { start: n, end: n });
        let line_no = match side {
            ReviewSide::Old => old_range.map(|r| r.start),
            ReviewSide::New => new_range.map(|r| r.start),
        }?;

        let anchor_key = format!("line|{}|{}|{}", file_path, side.as_str(), line_no);

        Some(ReviewAnchor {
            file_index,
            file_path,
            kind: ReviewTargetKind::Line,
            side: Some(side),
            old_range,
            new_range,
            hunk_id: line.hunk_index,
            display_idx_hint: Some(*display_idx),
            anchor_key,
        })
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

        match anchor.kind {
            ReviewTargetKind::Line => {
                let side = anchor.side.unwrap_or(ReviewSide::New);
                let range = match side {
                    ReviewSide::Old => anchor.old_range,
                    ReviewSide::New => anchor.new_range,
                };
                lines.push(format!("Side: {}", side.as_str()));
                lines.push(format!("Line: {}", format_opt_range_display(range)));
            }
            ReviewTargetKind::Hunk => {
                lines.push(format!(
                    "Old: {}",
                    format_opt_range_display(anchor.old_range)
                ));
                lines.push(format!(
                    "New: {}",
                    format_opt_range_display(anchor.new_range)
                ));
            }
        }

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
    use super::preserve_ref_trailing_space;

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
}
