use super::{
    AnimationFrame, App, FileDiskStamp, ReviewEditorToolbarAction, ReviewEditorToolbarHit, ViewMode,
};
use crate::config::{
    MentionFileScope, MentionFinder, ReviewActionConfig, ReviewHookConfig, ReviewHookEvent,
    ReviewHookStdin,
};
use crate::toasts::ToastEvent;
use crossterm::event::KeyEvent;
use keymap::{parser::parse_seq, ToKeyMap};
use oyo_core::{ChangeKind, LineKind, ViewLine};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReviewTargetKind {
    PullRequest,
    File,
    Line,
    Hunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReviewSide {
    Old,
    New,
}

impl ReviewSide {
    pub(crate) fn as_str(self) -> &'static str {
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
pub(crate) struct ReviewAuthor {
    pub(crate) name: String,
    pub(crate) email: Option<String>,
    #[serde(
        default,
        rename = "type",
        alias = "authorType",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) author_type: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) usernames: BTreeMap<String, String>,
    #[serde(default, alias = "avatarUrl", skip_serializing_if = "Option::is_none")]
    pub(crate) avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewProviderComment {
    pub(crate) provider: String,
    pub(crate) remote: String,
    pub(crate) repo: String,
    pub(crate) pr_number: u64,
    pub(crate) comment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) author_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pr_title: Option<String>,
    #[serde(default = "review_provider_comment_api_kind_default")]
    pub(crate) api_kind: String,
    pub(crate) sync_state: String,
}

fn review_provider_comment_api_kind_default() -> String {
    "review".to_string()
}

fn review_comment_can_edit_default() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewComment {
    pub(crate) id: u64,
    pub(crate) anchor: ReviewAnchor,
    pub(crate) body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) author: Option<ReviewAuthor>,
    #[serde(default = "review_comment_can_edit_default")]
    pub(crate) can_edit: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<ReviewProviderComment>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewEditorState {
    pub(crate) anchor: ReviewAnchor,
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewTargetMetadata {
    pub(crate) label: String,
    pub(crate) vcs: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jj_change_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jj_commit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) git_base_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) git_head_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) git_base_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) git_head_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pr_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pr_repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bookmarks: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewPaths {
    pub(crate) review_dir: Option<PathBuf>,
    pub(crate) db_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PublicReviewComments {
    version: u32,
    comments: Vec<PublicReviewComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PublicReviewComment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    old_range: Option<ReviewRange>,
    #[serde(default, rename = "oldRange", skip_serializing_if = "Option::is_none")]
    old_range_camel: Option<ReviewRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_range: Option<ReviewRange>,
    #[serde(default, rename = "newRange", skip_serializing_if = "Option::is_none")]
    new_range_camel: Option<ReviewRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hunk_id: Option<usize>,
    #[serde(default, rename = "hunkId", skip_serializing_if = "Option::is_none")]
    hunk_id_camel: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<ReviewAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    can_edit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<ReviewProviderComment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<u64>,
    body: String,
}

#[derive(Debug, Serialize)]
struct ReviewExport<'a> {
    version: u32,
    event: &'static str,
    repo_root: String,
    review_db: Option<String>,
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
    author: Option<&'a ReviewAuthor>,
    created_at: u64,
    updated_at: u64,
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
    pub(crate) avatar_url: Option<String>,
    pub(crate) avatar_seed: String,
    pub(crate) anchor_key: String,
    pub(crate) edit_label: Option<String>,
    pub(crate) delete_label: Option<String>,
    pub(crate) prefer_right: bool,
    pub(crate) is_hunk: bool,
    pub(crate) can_edit: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewPreviewBox {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) anchor_key: String,
    pub(crate) edit: bool,
    pub(crate) delete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewDeleteTarget {
    All,
    DiscardSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewDeleteConfirmation {
    pub(crate) target: ReviewDeleteTarget,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) confirm_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewDeleteConfirmationAction {
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReviewDeleteConfirmationHit {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) action: ReviewDeleteConfirmationAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewSyncAction {
    Sync,
    Pull,
    Push,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewSyncRequest {
    pub(crate) action: ReviewSyncAction,
    pub(crate) remote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewRemoteOption {
    pub(crate) name: String,
    pub(crate) label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewRemotePickerState {
    pub(crate) action: ReviewSyncAction,
    pub(crate) remotes: Vec<ReviewRemoteOption>,
    pub(crate) selected: usize,
    pub(crate) query: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReviewRemotePickerHit {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReviewSidebarOverflowHit {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) action: ReviewSyncAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrCommentHitAction {
    Open(u64),
    Edit(u64),
    Reply(u64),
    Delete(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrCommentHit {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) action: PrCommentHitAction,
}

fn review_index_action_label(prefix: &str, idx: usize) -> String {
    if idx < 26 {
        format!("{prefix}{}", (b'a' + idx as u8) as char)
    } else {
        format!("{prefix}{}", idx + 1)
    }
}

fn pr_comment_action_label(prefix: &str, ids: &[u64], id: u64) -> Option<String> {
    ids.iter()
        .position(|comment_id| *comment_id == id)
        .map(|idx| review_index_action_label(prefix, idx))
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

#[cfg(not(test))]
fn default_review_base_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("oyo")
        .join("reviews")
}

fn review_db_stamp(path: &Path) -> FileDiskStamp {
    let Ok(metadata) = fs::metadata(path) else {
        return FileDiskStamp::default();
    };
    FileDiskStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
        exists: true,
    }
}

#[cfg(test)]
fn default_review_base_dir() -> PathBuf {
    std::env::temp_dir().join("oyo").join("reviews")
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
        ReviewTargetKind::PullRequest => "pr".to_string(),
        ReviewTargetKind::File => "file".to_string(),
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

fn same_review_author(author: &ReviewAuthor, current_author: &ReviewAuthor) -> bool {
    if author.email.is_some() && author.email == current_author.email {
        return true;
    }
    if author
        .usernames
        .iter()
        .any(|(provider, username)| current_author.usernames.get(provider) == Some(username))
    {
        return true;
    }
    !author.name.trim().is_empty() && author.name == current_author.name
}

fn review_author_avatar_seed(author: Option<&ReviewAuthor>) -> String {
    let Some(author) = author else {
        return "unknown".to_string();
    };
    author
        .usernames
        .values()
        .next()
        .cloned()
        .or_else(|| author.email.clone())
        .or_else(|| (!author.name.trim().is_empty()).then(|| author.name.clone()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn review_author_type_label(author: &ReviewAuthor) -> Option<&str> {
    author
        .author_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "human")
}

fn review_author_label(
    author: Option<&ReviewAuthor>,
    current_author: Option<&ReviewAuthor>,
) -> String {
    if let (Some(author), Some(current_author)) = (author, current_author) {
        if same_review_author(author, current_author) {
            return "You".to_string();
        }
    }
    let Some(author) = author else {
        return "Unknown".to_string();
    };
    let label = if !author.name.trim().is_empty() {
        author.name.clone()
    } else {
        author
            .usernames
            .values()
            .next()
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string())
    };
    if let Some(author_type) = review_author_type_label(author) {
        format!("{label} ({author_type})")
    } else {
        label
    }
}

fn review_time_label(timestamp: u64) -> String {
    let now = now_ts();
    let elapsed = now.saturating_sub(timestamp);
    if elapsed < 60 {
        return "a moment ago".to_string();
    }
    if elapsed < 60 * 60 {
        return format!("{}m ago", elapsed / 60);
    }
    if elapsed < 24 * 60 * 60 {
        return format!("{}h ago", elapsed / 60 / 60);
    }
    let Ok(date) = OffsetDateTime::from_unix_timestamp(timestamp as i64) else {
        return "unknown date".to_string();
    };
    let Ok(now_date) = OffsetDateTime::from_unix_timestamp(now as i64) else {
        return "unknown date".to_string();
    };
    let month = match u8::from(date.month()) {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    };
    if date.year() == now_date.year() {
        format!(
            "{} {month} {:02}:{:02}",
            date.day(),
            date.hour(),
            date.minute()
        )
    } else {
        format!("{} {month} {}", date.day(), date.year())
    }
}

fn review_comment_subject(comment: &ReviewComment) -> String {
    match comment.anchor.kind {
        ReviewTargetKind::PullRequest => comment
            .provider
            .as_ref()
            .and_then(|provider| provider.pr_title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "Pull request".to_string()),
        ReviewTargetKind::File => comment.anchor.file_path.clone(),
        ReviewTargetKind::Line | ReviewTargetKind::Hunk => format!(
            "{} {}",
            comment.anchor.file_path,
            review_anchor_location_label(&comment.anchor)
        ),
    }
}

fn review_comment_title(comment: &ReviewComment, current_author: Option<&ReviewAuthor>) -> String {
    format!(
        "{}, {} • {}",
        review_author_label(comment.author.as_ref(), current_author),
        review_time_label(comment.updated_at),
        review_comment_subject(comment)
    )
}

fn file_review_anchor(file_index: usize, file_path: String) -> Option<ReviewAnchor> {
    if file_path.is_empty() {
        return None;
    }
    let anchor_key = format!("file|{file_path}");
    Some(ReviewAnchor {
        file_index,
        file_path,
        kind: ReviewTargetKind::File,
        side: None,
        old_range: None,
        new_range: None,
        hunk_id: None,
        display_idx_hint: None,
        anchor_key,
    })
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

fn logical_line_bounds(text: &str, row: usize) -> (usize, usize) {
    let starts = line_starts(text);
    if starts.is_empty() {
        return (0, 0);
    }
    let row = row.min(starts.len().saturating_sub(1));
    let start = starts[row];
    let end = if row + 1 < starts.len() {
        starts[row + 1].saturating_sub(1)
    } else {
        text.len()
    };
    (start, end)
}

fn cursor_for_row_col(text: &str, row: usize, col: usize) -> usize {
    let (start, line_end) = logical_line_bounds(text, row);
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

fn visual_pos_for_cursor(line: &str, cursor: usize, wrap_width: usize) -> (usize, usize) {
    if wrap_width == 0 || line.is_empty() {
        return (0, 0);
    }
    let mut row = 0usize;
    let mut col = 0usize;
    for (idx, ch) in line.char_indices() {
        if idx >= cursor {
            break;
        }
        let width = ch.width().unwrap_or(1).max(1);
        if col > 0 && col.saturating_add(width) > wrap_width {
            row = row.saturating_add(1);
            col = 0;
        }
        col = col.saturating_add(width);
        if col >= wrap_width {
            row = row.saturating_add(1);
            col = 0;
        }
    }
    (row, col)
}

fn visual_row_count(line: &str, wrap_width: usize) -> usize {
    if wrap_width == 0 || line.is_empty() {
        return 1;
    }
    let (row, col) = visual_pos_for_cursor(line, line.len(), wrap_width);
    row.saturating_add(usize::from(col > 0 || row == 0))
}

fn cursor_for_visual_pos(
    line: &str,
    line_start: usize,
    row: usize,
    col: usize,
    wrap_width: usize,
) -> usize {
    if wrap_width == 0 || line.is_empty() {
        return line_start;
    }
    let mut visual_row = 0usize;
    let mut visual_col = 0usize;
    for (idx, ch) in line.char_indices() {
        if visual_row > row || (visual_row == row && visual_col >= col) {
            return line_start.saturating_add(idx);
        }
        let width = ch.width().unwrap_or(1).max(1);
        if visual_col > 0 && visual_col.saturating_add(width) > wrap_width {
            visual_row = visual_row.saturating_add(1);
            visual_col = 0;
            if visual_row > row || (visual_row == row && visual_col >= col) {
                return line_start.saturating_add(idx);
            }
        }
        visual_col = visual_col.saturating_add(width);
        if visual_col >= wrap_width {
            visual_row = visual_row.saturating_add(1);
            visual_col = 0;
        }
    }
    line_start.saturating_add(line.len())
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
            self.review_dir_path = None;
            self.review_db_path = None;
            self.review_db_stamp = FileDiskStamp::default();
        }
    }

    pub(crate) fn set_review_filter_to_current_diff(&mut self, enabled: bool) {
        self.review_filter_to_current_diff = enabled;
    }

    pub fn set_review_base_dir(&mut self, path: Option<PathBuf>) {
        self.review_base_dir_override = path;
    }

    pub fn set_review_workspace_root(&mut self, path: Option<PathBuf>) {
        self.review_workspace_root_override = path;
    }

    pub(crate) fn set_review_target_metadata(&mut self, metadata: Option<ReviewTargetMetadata>) {
        self.review_target_metadata = metadata;
    }

    pub fn set_review_author(&mut self, author: Option<ReviewAuthor>) {
        self.review_author = author;
    }

    fn fill_review_author_details(author: &mut ReviewAuthor, current: &ReviewAuthor) -> bool {
        let mut changed = false;
        for (provider, username) in &current.usernames {
            if !author.usernames.contains_key(provider) {
                author.usernames.insert(provider.clone(), username.clone());
                changed = true;
            }
        }
        if author.avatar_url.is_none() && current.avatar_url.is_some() {
            author.avatar_url = current.avatar_url.clone();
            changed = true;
        }
        changed
    }

    fn fill_current_author_comments(&mut self) -> bool {
        let Some(current) = self.review_author.clone() else {
            return false;
        };
        let mut changed = false;
        for comment in &mut self.review_comments {
            let Some(author) = comment.author.as_mut() else {
                continue;
            };
            if same_review_author(author, &current) {
                changed |= Self::fill_review_author_details(author, &current);
            }
        }
        changed
    }

    pub(crate) fn set_review_author_provider_avatar(
        &mut self,
        provider: &str,
        username: &str,
        avatar_url: Option<String>,
    ) {
        let Some(author) = self.review_author.as_mut() else {
            return;
        };
        author
            .usernames
            .insert(provider.to_string(), username.to_string());
        if avatar_url.is_some() {
            author.avatar_url = avatar_url;
        }
        if self.fill_current_author_comments() {
            self.touch_review_state();
            self.persist_review_session();
        }
    }

    pub(crate) fn review_paths(&self) -> ReviewPaths {
        ReviewPaths {
            review_dir: self.review_dir_path.clone(),
            db_file: self.review_db_path.clone(),
        }
    }

    pub fn review_markdown(&self) -> String {
        self.format_review_output()
    }

    pub fn review_comments_json(&self) -> String {
        self.public_review_comments_json()
    }

    pub fn review_workspace_root(&self) -> Option<&str> {
        self.review_repo_root.as_deref()
    }

    pub fn review_diff_fingerprint(&self) -> &str {
        &self.review_diff_fingerprint
    }

    pub fn review_revision(&self) -> u64 {
        self.review_revision
    }

    pub fn review_comment_count(&self) -> usize {
        self.review_comments
            .iter()
            .filter(|comment| !comment.deleted)
            .count()
    }

    pub(crate) fn review_comment_count_for_file(&self, file_index: usize) -> usize {
        self.review_comments
            .iter()
            .filter(|comment| !comment.deleted && comment.anchor.file_index == file_index)
            .count()
    }

    pub(crate) fn file_review_comments_supported(&self) -> bool {
        self.review_mode && (self.current_file_is_binary() || self.current_file_is_image())
    }

    pub(crate) fn set_review_file_comment_hit(&mut self, hit: Option<(u16, u16, u16, u16)>) {
        self.review_file_comment_hit = hit;
    }

    pub(crate) fn handle_review_file_comment_click(&mut self, column: u16, row: u16) -> bool {
        let hit = self
            .review_file_comment_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            });
        if !hit {
            return false;
        }
        self.start_file_comment()
    }

    pub(crate) fn filtered_review_comment_indices(&self) -> Vec<usize> {
        self.review_comment_indices_for_query(&self.file_filter)
    }

    pub(crate) fn review_comment_indices_for_query(&self, query: &str) -> Vec<usize> {
        let query = query.trim().to_ascii_lowercase();
        self.review_comments
            .iter()
            .enumerate()
            .filter_map(|(idx, comment)| {
                if comment.deleted {
                    return None;
                }
                if query.is_empty() {
                    return Some(idx);
                }
                let location = review_anchor_location_label(&comment.anchor);
                let author = comment
                    .author
                    .as_ref()
                    .map(|author| {
                        format!(
                            "{} {}",
                            author.name,
                            author.email.as_deref().unwrap_or_default()
                        )
                    })
                    .unwrap_or_default();
                let haystack = format!(
                    "{} {} {} {}",
                    comment.anchor.file_path, location, comment.body, author
                )
                .to_ascii_lowercase();
                haystack.contains(&query).then_some(idx)
            })
            .collect()
    }

    pub(crate) fn review_comment_is_active(&self, index: usize) -> bool {
        let Some(comment) = self
            .review_comments
            .get(index)
            .filter(|comment| !comment.deleted)
        else {
            return false;
        };
        if self.active_review_comment_id == Some(comment.id) {
            return true;
        }
        self.review_editor
            .as_ref()
            .is_some_and(|editor| comment.anchor.anchor_key == editor.anchor.anchor_key)
    }

    pub(crate) fn review_comment_sidebar_sort_key(&self, index: usize) -> Option<(u64, u64)> {
        self.review_comments
            .get(index)
            .filter(|comment| !comment.deleted)
            .map(|comment| (comment.updated_at, comment.id))
    }

    pub(crate) fn review_comment_sidebar_bucket(&self, index: usize) -> Option<String> {
        let comment = self
            .review_comments
            .get(index)
            .filter(|comment| !comment.deleted)?;
        let now = now_ts();
        let elapsed = now.saturating_sub(comment.updated_at);
        if elapsed < 60 * 60 {
            return Some("today".to_string());
        }
        if elapsed < 24 * 60 * 60 {
            return Some(format!("{}hr ago", elapsed / 60 / 60));
        }
        let Ok(date) = OffsetDateTime::from_unix_timestamp(comment.updated_at as i64) else {
            return Some("unknown date".to_string());
        };
        let Ok(now_date) = OffsetDateTime::from_unix_timestamp(now as i64) else {
            return Some("unknown date".to_string());
        };
        let month = match u8::from(date.month()) {
            1 => "Jan.",
            2 => "Feb.",
            3 => "Mar.",
            4 => "Apr.",
            5 => "May",
            6 => "Jun.",
            7 => "Jul.",
            8 => "Aug.",
            9 => "Sep.",
            10 => "Oct.",
            11 => "Nov.",
            _ => "Dec.",
        };
        Some(if date.year() == now_date.year() {
            format!("{month} {}", date.day())
        } else {
            format!("{month} {} {}", date.day(), date.year())
        })
    }

    pub(crate) fn review_status_comment_rows(&self) -> Vec<(u64, String, String, String)> {
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| !comment.deleted)
            .collect::<Vec<_>>();
        comments.sort_by(|a, b| {
            a.anchor
                .file_path
                .cmp(&b.anchor.file_path)
                .then_with(|| {
                    let rank = |kind| match kind {
                        ReviewTargetKind::PullRequest => 0,
                        ReviewTargetKind::File => 1,
                        ReviewTargetKind::Line => 2,
                        ReviewTargetKind::Hunk => 3,
                    };
                    rank(a.anchor.kind).cmp(&rank(b.anchor.kind))
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
            .into_iter()
            .map(|comment| {
                let subject = match comment.anchor.kind {
                    ReviewTargetKind::PullRequest => "pull request".to_string(),
                    ReviewTargetKind::File | ReviewTargetKind::Line | ReviewTargetKind::Hunk => {
                        comment.anchor.file_path.clone()
                    }
                };
                let location = review_anchor_location_label(&comment.anchor);
                let first_line = comment.body.lines().next().unwrap_or_default().trim();
                let mut preview = if first_line.is_empty() {
                    "(empty)".to_string()
                } else {
                    first_line.to_string()
                };
                if comment.body.contains('\n') && first_line.len() < comment.body.trim().len() {
                    preview.push_str(" ...");
                }
                (comment.id, subject, location, preview)
            })
            .collect()
    }

    pub(crate) fn review_comment_sidebar_item(
        &self,
        index: usize,
    ) -> Option<(usize, String, String, String)> {
        let comment = self
            .review_comments
            .get(index)
            .filter(|comment| !comment.deleted)?;
        let title = match comment.anchor.kind {
            ReviewTargetKind::PullRequest => review_comment_subject(comment),
            ReviewTargetKind::File | ReviewTargetKind::Line | ReviewTargetKind::Hunk => {
                comment.anchor.file_path.clone()
            }
        };
        let location = match comment.anchor.kind {
            ReviewTargetKind::PullRequest => String::new(),
            ReviewTargetKind::File => "file".to_string(),
            ReviewTargetKind::Line | ReviewTargetKind::Hunk => {
                review_anchor_location_label(&comment.anchor)
            }
        };
        let first_line = comment.body.lines().next().unwrap_or_default().trim();
        let mut preview = if first_line.is_empty() {
            "(empty)".to_string()
        } else {
            first_line.to_string()
        };
        if comment.body.contains('\n') && first_line.len() < comment.body.trim().len() {
            preview.push_str(" ...");
        }
        Some((comment.anchor.file_index, title, location, preview))
    }

    pub fn open_review_comment(&mut self, index: usize) -> bool {
        let Some(comment) = self
            .review_comments
            .get(index)
            .filter(|comment| !comment.deleted)
            .cloned()
        else {
            return false;
        };
        self.active_review_comment_id = Some(comment.id);
        self.flash_review_preview(comment.anchor.anchor_key.clone());
        if self.review_editor_active() {
            self.review_cancel_editor();
        }
        if comment.anchor.kind == ReviewTargetKind::PullRequest {
            self.open_pr_comments_in_current_tab(Some(comment.id));
            return true;
        }
        self.select_file(comment.anchor.file_index);
        self.scroll_to_review_anchor(&comment.anchor);
        true
    }

    fn scroll_to_review_anchor(&mut self, anchor: &ReviewAnchor) {
        let Some((start, _)) = self.review_anchor_display_span(anchor) else {
            return;
        };
        let viewport_height = self.last_viewport_height.max(1);
        if self.auto_center {
            self.scroll_offset = start.saturating_sub(viewport_height / 2);
            self.centered_once = true;
        } else {
            self.scroll_offset = start;
            self.centered_once = false;
        }
        self.needs_scroll_to_active = false;
        self.multi_diff.current_navigator().set_hunk_scope(false);
    }

    pub(crate) fn active_pr_comments_view(&self) -> bool {
        self.active_topbar_content() == Some(super::TopbarTabContent::PrComments)
    }

    pub(crate) fn pull_request_comment_ids(&self) -> Vec<u64> {
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| {
                !comment.deleted && comment.anchor.kind == ReviewTargetKind::PullRequest
            })
            .collect::<Vec<_>>();
        comments.sort_by_key(|comment| (comment.created_at, comment.id));
        comments.into_iter().map(|comment| comment.id).collect()
    }

    pub(crate) fn pull_request_comment_overlays(&self) -> Vec<(u64, usize, ReviewCommentOverlay)> {
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| {
                !comment.deleted && comment.anchor.kind == ReviewTargetKind::PullRequest
            })
            .collect::<Vec<_>>();
        comments.sort_by_key(|comment| (comment.created_at, comment.id));
        comments
            .into_iter()
            .enumerate()
            .map(|(idx, comment)| {
                (
                    comment.id,
                    idx + 1,
                    ReviewCommentOverlay {
                        display_idx: 0,
                        preview: comment.body.lines().next().unwrap_or_default().to_string(),
                        body: comment.body.clone(),
                        title: review_comment_title(comment, self.review_author.as_ref()),
                        avatar_url: comment
                            .author
                            .as_ref()
                            .and_then(|author| author.avatar_url.clone()),
                        avatar_seed: review_author_avatar_seed(comment.author.as_ref()),
                        anchor_key: comment.anchor.anchor_key.clone(),
                        edit_label: None,
                        delete_label: None,
                        prefer_right: true,
                        is_hunk: false,
                        can_edit: comment.can_edit,
                    },
                )
            })
            .collect()
    }

    pub(crate) fn pull_request_reply_label(&self, id: u64) -> Option<String> {
        pr_comment_action_label("r", &self.pull_request_comment_ids(), id)
    }

    fn pull_request_editable_comment_ids(&self) -> Vec<u64> {
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| {
                !comment.deleted
                    && comment.can_edit
                    && comment.anchor.kind == ReviewTargetKind::PullRequest
            })
            .collect::<Vec<_>>();
        comments.sort_by_key(|comment| (comment.created_at, comment.id));
        comments.into_iter().map(|comment| comment.id).collect()
    }

    pub(crate) fn pull_request_edit_label(&self, id: u64) -> Option<String> {
        pr_comment_action_label("i", &self.pull_request_editable_comment_ids(), id)
    }

    pub(crate) fn pull_request_delete_label(&self, id: u64) -> Option<String> {
        pr_comment_action_label("x", &self.pull_request_editable_comment_ids(), id)
    }

    pub(crate) fn reply_to_pull_request_comment_letter(&mut self, letter: char) -> bool {
        let letter = letter.to_ascii_lowercase();
        if !letter.is_ascii_lowercase() {
            return false;
        }
        let idx = (letter as u8 - b'a') as usize;
        let Some(id) = self.pull_request_comment_ids().get(idx).copied() else {
            return false;
        };
        self.start_pull_request_reply(id)
    }

    pub(crate) fn reply_to_pull_request_comment_number(&mut self, number: usize) -> bool {
        let Some(id) = self
            .pull_request_comment_ids()
            .get(number.saturating_sub(1))
            .copied()
        else {
            return false;
        };
        self.start_pull_request_reply(id)
    }

    pub(crate) fn edit_review_comment_letter(&mut self, letter: char) -> bool {
        let letter = letter.to_ascii_lowercase();
        if !letter.is_ascii_lowercase() {
            return false;
        }
        self.edit_review_comment_index((letter as u8 - b'a') as usize)
    }

    pub(crate) fn edit_review_comment_number(&mut self, number: usize) -> bool {
        if number == 0 {
            return false;
        }
        self.edit_review_comment_index(number - 1)
    }

    fn editable_review_comment_anchor_at_index(&mut self, idx: usize) -> Option<String> {
        if self.view_mode == ViewMode::Preview {
            return self
                .review_file_comment_overlay()
                .filter(|overlay| overlay.can_edit)
                .map(|overlay| overlay.anchor_key);
        }
        self.review_comment_overlays_for_current_file()
            .into_iter()
            .filter(|overlay| overlay.can_edit)
            .nth(idx)
            .map(|overlay| overlay.anchor_key)
    }

    fn edit_review_comment_index(&mut self, idx: usize) -> bool {
        if self.active_pr_comments_view() {
            let Some(id) = self.pull_request_editable_comment_ids().get(idx).copied() else {
                return false;
            };
            return self.edit_pull_request_comment(id);
        }

        let anchor_key = self.editable_review_comment_anchor_at_index(idx);
        let Some(anchor_key) = anchor_key else {
            return false;
        };
        let anchor = self
            .review_comments
            .iter()
            .find(|comment| {
                !comment.deleted && comment.can_edit && comment.anchor.anchor_key == anchor_key
            })
            .map(|comment| comment.anchor.clone());
        if let Some(anchor) = anchor {
            self.open_review_editor(anchor);
            true
        } else {
            false
        }
    }

    pub(crate) fn delete_review_comment_letter(&mut self, letter: char) -> bool {
        let letter = letter.to_ascii_lowercase();
        if !letter.is_ascii_lowercase() {
            return false;
        }
        self.delete_review_comment_index((letter as u8 - b'a') as usize)
    }

    pub(crate) fn delete_review_comment_number(&mut self, number: usize) -> bool {
        if number == 0 {
            return false;
        }
        self.delete_review_comment_index(number - 1)
    }

    fn delete_review_comment_index(&mut self, idx: usize) -> bool {
        if self.active_pr_comments_view() {
            let Some(id) = self.pull_request_editable_comment_ids().get(idx).copied() else {
                return false;
            };
            return self.request_delete_comment_by_id(id);
        }
        let Some(anchor_key) = self.editable_review_comment_anchor_at_index(idx) else {
            return false;
        };
        self.request_delete_comment_by_anchor(anchor_key)
    }

    pub(crate) fn review_comment_body_for_id(&self, id: u64) -> Option<String> {
        self.review_comments
            .iter()
            .find(|comment| comment.id == id && !comment.deleted)
            .map(|comment| comment.body.clone())
    }

    pub(crate) fn pull_request_title(&self) -> String {
        self.review_comments
            .iter()
            .find_map(|comment| {
                comment
                    .provider
                    .as_ref()
                    .and_then(|provider| provider.pr_title.clone())
            })
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "Pull request".to_string())
    }

    pub(crate) fn pull_request_comment_target_available(&self) -> bool {
        self.review_target_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.pr_number.is_some())
            || self.review_comments.iter().any(|comment| {
                !comment.deleted
                    && comment.provider.as_ref().is_some_and(|provider| {
                        provider.pr_number > 0 && !provider.repo.trim().is_empty()
                    })
            })
    }

    fn pull_request_anchor(&self, anchor_key: String) -> ReviewAnchor {
        ReviewAnchor {
            file_index: 0,
            file_path: self.pull_request_title(),
            kind: ReviewTargetKind::PullRequest,
            side: None,
            old_range: None,
            new_range: None,
            hunk_id: None,
            display_idx_hint: Some(0),
            anchor_key,
        }
    }

    pub(crate) fn start_pull_request_comment(&mut self) -> bool {
        if !self.pull_request_comment_target_available() {
            return false;
        }
        let key = format!("pr|new|{}", self.review_next_comment_id);
        self.open_review_editor(self.pull_request_anchor(key));
        true
    }

    pub(crate) fn start_pull_request_reply(&mut self, id: u64) -> bool {
        if !self.pull_request_comment_target_available() {
            return false;
        }
        let Some(body) = self.review_comment_body_for_id(id) else {
            return false;
        };
        let quote = body
            .lines()
            .map(|line| format!("> {line}"))
            .chain(std::iter::once(">".to_string()))
            .chain(std::iter::once(String::new()))
            .collect::<Vec<_>>()
            .join("\n");
        let key = format!("pr|reply|{}|{}", id, self.review_next_comment_id);
        self.review_editor = Some(ReviewEditorState {
            anchor: self.pull_request_anchor(key),
            text: quote,
            cursor: 0,
        });
        if let Some(editor) = self.review_editor.as_mut() {
            editor.cursor = editor.text.len();
        }
        true
    }

    pub(crate) fn edit_pull_request_comment(&mut self, id: u64) -> bool {
        let Some(comment) = self
            .review_comments
            .iter()
            .find(|comment| comment.id == id && !comment.deleted && comment.can_edit)
            .cloned()
        else {
            return false;
        };
        self.open_review_editor(comment.anchor);
        true
    }

    pub(crate) fn set_pr_comment_hits(&mut self, hits: Vec<PrCommentHit>) {
        self.pr_comment_hits = hits;
    }

    pub(crate) fn set_pr_comment_add_hit(&mut self, hit: Option<(u16, u16, u16, u16)>) {
        self.pr_comment_add_hit = hit;
    }

    pub(crate) fn handle_pr_comment_view_click(&mut self, column: u16, row: u16) -> bool {
        if !self.active_pr_comments_view() || self.review_editor.is_some() {
            return false;
        }
        if let Some(action) = self.pr_comment_hits.iter().rev().find_map(|hit| {
            let in_box = column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height);
            in_box.then_some(hit.action)
        }) {
            return match action {
                PrCommentHitAction::Open(id) | PrCommentHitAction::Edit(id) => {
                    self.edit_pull_request_comment(id)
                }
                PrCommentHitAction::Reply(id) => self.start_pull_request_reply(id),
                PrCommentHitAction::Delete(id) => self.request_delete_comment_by_id(id),
            };
        }
        if self
            .pr_comment_add_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.start_pull_request_comment();
            return true;
        }
        false
    }

    pub fn take_review_hook_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.review_hook_warnings)
    }

    pub(crate) fn review_delete_confirmation_active(&self) -> bool {
        self.review_delete_confirmation.is_some()
    }

    pub(crate) fn review_delete_confirmation_render(&self) -> Option<ReviewDeleteConfirmation> {
        self.review_delete_confirmation.clone()
    }

    pub(crate) fn set_review_delete_confirmation_hits(
        &mut self,
        hits: Vec<ReviewDeleteConfirmationHit>,
    ) {
        self.review_delete_confirmation_hits = hits;
    }

    pub(crate) fn cancel_review_delete_confirmation(&mut self) {
        self.review_delete_confirmation = None;
        self.review_delete_confirmation_hits.clear();
        self.review_delete_confirmation_hover = None;
    }

    pub(crate) fn update_review_delete_confirmation_hover(
        &mut self,
        column: u16,
        row: u16,
    ) -> bool {
        let hover = self.review_delete_confirmation_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.action)
        });
        if self.review_delete_confirmation_hover == hover {
            return false;
        }
        self.review_delete_confirmation_hover = hover;
        true
    }

    pub(crate) fn handle_review_delete_confirmation_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            crossterm::event::KeyCode::Char('d') | crossterm::event::KeyCode::Char('D') => {
                self.confirm_review_delete();
                true
            }
            crossterm::event::KeyCode::Esc => {
                self.cancel_review_delete_confirmation();
                true
            }
            _ => true,
        }
    }

    pub(crate) fn handle_review_delete_confirmation_click(
        &mut self,
        column: u16,
        row: u16,
    ) -> bool {
        let Some(action) = self.review_delete_confirmation_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.action)
        }) else {
            return true;
        };
        match action {
            ReviewDeleteConfirmationAction::Confirm => self.confirm_review_delete(),
            ReviewDeleteConfirmationAction::Cancel => self.cancel_review_delete_confirmation(),
        }
        true
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

    pub(crate) fn set_review_editor_wrap_width(&mut self, width: usize) {
        self.review_editor_wrap_width = width;
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
        if !overlay.can_edit {
            return format!("{} • read only", overlay.preview);
        }
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

    pub(crate) fn review_preview_flash_key(&self) -> Option<String> {
        self.review_preview_flash
            .as_ref()
            .and_then(|(key, until)| (Instant::now() < *until).then(|| key.clone()))
    }

    pub(crate) fn review_preview_flash_active(&self, anchor_key: &str) -> bool {
        self.review_preview_flash
            .as_ref()
            .is_some_and(|(key, until)| key == anchor_key && Instant::now() < *until)
    }

    fn flash_review_preview(&mut self, anchor_key: String) {
        self.review_preview_flash = Some((anchor_key, Instant::now() + Duration::from_millis(650)));
    }

    fn request_delete_comment_by_anchor(&mut self, anchor_key: String) -> bool {
        self.remove_comment_for_anchor_key(&anchor_key)
    }

    fn request_delete_comment_by_id(&mut self, id: u64) -> bool {
        self.remove_review_comment_from_cli(id)
    }

    pub fn confirm_review_delete(&mut self) {
        let Some(confirmation) = self.review_delete_confirmation.take() else {
            return;
        };
        self.review_delete_confirmation_hits.clear();
        self.review_delete_confirmation_hover = None;
        match confirmation.target {
            ReviewDeleteTarget::All => {
                self.clear_all_review_comments_now();
            }
            ReviewDeleteTarget::DiscardSession => {
                self.discard_review_session_changes_now();
            }
        }
    }

    pub fn remove_hovered_review_comment(&mut self) -> bool {
        let Some(anchor_key) = self.review_preview_hover.clone() else {
            return false;
        };
        self.review_preview_hover = None;
        self.request_delete_comment_by_anchor(anchor_key)
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
            edit: false,
            delete: false,
        });
    }

    pub fn add_review_preview_edit_box(
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
            edit: true,
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
            edit: false,
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
        let note_boxes = self
            .review_preview_boxes
            .iter()
            .filter(|hit| !hit.delete && !hit.edit);
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
        let Some((x, y, width, height)) = self.diff_view_area else {
            return false;
        };
        if column < x
            || column >= x.saturating_add(width)
            || row < y
            || row >= y.saturating_add(height)
        {
            return false;
        }

        let hit = self.review_preview_boxes.iter().rev().find_map(|hit| {
            let end_x = hit.x.saturating_add(hit.width);
            let end_y = hit.y.saturating_add(hit.height);
            (column >= hit.x && column < end_x && row >= hit.y && row < end_y)
                .then(|| (hit.anchor_key.clone(), hit.edit, hit.delete))
        });

        let Some((anchor_key, _edit, delete)) = hit else {
            return false;
        };
        if delete {
            return self.request_delete_comment_by_anchor(anchor_key);
        }

        let anchor = self
            .review_comments
            .iter()
            .find(|c| !c.deleted && c.can_edit && c.anchor.anchor_key == anchor_key)
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
            ReviewTargetKind::PullRequest | ReviewTargetKind::File => None,
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
            ReviewTargetKind::PullRequest | ReviewTargetKind::File => true,
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
            .filter(|comment| {
                !comment.deleted
                    && comment.anchor.kind != ReviewTargetKind::PullRequest
                    && comment.anchor.file_path == file_path
            })
            .filter(|comment| active_anchor_key != Some(comment.anchor.anchor_key.as_str()))
        {
            let display_idx = match comment.anchor.kind {
                ReviewTargetKind::PullRequest => Some(0),
                ReviewTargetKind::File => None,
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
                ReviewTargetKind::PullRequest | ReviewTargetKind::File => true,
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
                title: review_comment_title(comment, self.review_author.as_ref()),
                avatar_url: comment
                    .author
                    .as_ref()
                    .and_then(|author| author.avatar_url.clone()),
                avatar_seed: review_author_avatar_seed(comment.author.as_ref()),
                anchor_key: comment.anchor.anchor_key.clone(),
                edit_label: None,
                delete_label: None,
                prefer_right,
                is_hunk: matches!(comment.anchor.kind, ReviewTargetKind::Hunk),
                can_edit: comment.can_edit,
            });
        }

        overlays.sort_by_key(|overlay| overlay.display_idx);
        let mut action_idx = 0;
        for overlay in &mut overlays {
            if overlay.can_edit {
                overlay.edit_label = Some(review_index_action_label("i", action_idx));
                overlay.delete_label = Some(review_index_action_label("x", action_idx));
                action_idx += 1;
            }
        }
        overlays
    }

    pub fn review_file_comment_overlay(&self) -> Option<ReviewCommentOverlay> {
        if !self.file_review_comments_supported() {
            return None;
        }
        let anchor = self.resolve_file_review_anchor()?;
        let active_anchor_key = self
            .review_editor
            .as_ref()
            .map(|editor| editor.anchor.anchor_key.as_str());
        let comment = self
            .review_comments
            .iter()
            .find(|comment| !comment.deleted && comment.anchor.anchor_key == anchor.anchor_key)?;
        if active_anchor_key == Some(comment.anchor.anchor_key.as_str()) {
            return None;
        }

        let first_line = comment.body.lines().next().unwrap_or_default().trim();
        let (mut preview, was_truncated) = truncate_preview_chars(first_line, 50);
        let multiline = comment.body.contains('\n');
        if multiline && !was_truncated {
            preview.push_str(" …");
        }
        if preview.is_empty() {
            preview = "(empty)".to_string();
        }

        Some(ReviewCommentOverlay {
            display_idx: 0,
            preview,
            body: comment.body.clone(),
            title: review_comment_title(comment, self.review_author.as_ref()),
            avatar_url: comment
                .author
                .as_ref()
                .and_then(|author| author.avatar_url.clone()),
            avatar_seed: review_author_avatar_seed(comment.author.as_ref()),
            anchor_key: comment.anchor.anchor_key.clone(),
            edit_label: comment.can_edit.then(|| review_index_action_label("i", 0)),
            delete_label: comment.can_edit.then(|| review_index_action_label("x", 0)),
            prefer_right: true,
            is_hunk: false,
            can_edit: comment.can_edit,
        })
    }

    pub fn enable_review_mode(&mut self) {
        self.configure_review_mode(true);
    }

    pub fn load_review_mode(&mut self) {
        self.configure_review_mode(false);
    }

    fn configure_review_mode(&mut self, create_missing: bool) {
        self.review_mode = true;
        self.touch_review_state();

        let repo_root = self
            .review_workspace_root_override
            .clone()
            .or_else(|| self.multi_diff.repo_root().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        self.review_repo_root = Some(repo_root.to_string_lossy().to_string());
        self.invalidate_review_repo_file_cache();

        let diff_fingerprint = self.compute_review_diff_fingerprint();
        self.review_diff_fingerprint = diff_fingerprint.clone();
        self.review_session_created_at = now_ts();

        if !self.review_persist_enabled {
            self.review_dir_path = None;
            self.review_db_path = None;
            self.review_db_stamp = FileDiskStamp::default();
            self.review_comments.clear();
            self.review_session_baseline.clear();
            self.review_editor = None;
            self.review_mention_picker = None;
            self.review_next_comment_id = 1;
            self.touch_review_state();
            return;
        }

        let repo_key = hash_hex(&repo_root.to_string_lossy());
        let base = self
            .review_base_dir_override
            .clone()
            .unwrap_or_else(default_review_base_dir);
        let dir = base.join(repo_key);
        let db_path = dir.join("review.db");
        self.review_dir_path = Some(dir);
        self.review_db_path = Some(db_path.clone());
        self.review_db_stamp = review_db_stamp(&db_path);
        self.last_review_db_check = Instant::now();

        if self.load_review_state(&db_path) {
            self.repair_review_comment_file_indexes();
            self.repair_review_editor_file_index();
            self.refresh_review_mention_picker();
            self.review_next_comment_id = self
                .review_comments
                .iter()
                .map(|c| c.id)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            if self.fill_current_author_comments() {
                self.persist_review_session();
            }
            self.review_session_baseline = self.review_comments.clone();
            self.touch_review_state();
            return;
        }

        self.review_comments.clear();
        self.review_session_baseline.clear();
        self.review_editor = None;
        self.review_mention_picker = None;
        self.review_next_comment_id = 1;
        self.touch_review_state();
        if create_missing {
            self.persist_review_session();
        }
    }

    pub fn start_file_comment(&mut self) -> bool {
        if !self.file_review_comments_supported() {
            return false;
        }
        let Some(anchor) = self.resolve_file_review_anchor() else {
            return false;
        };
        self.open_review_editor(anchor);
        true
    }

    pub fn start_line_comment(&mut self) {
        if !self.review_mode {
            return;
        }
        if self.active_pr_comments_view() {
            self.start_pull_request_comment();
            return;
        }
        if self.file_review_comments_supported() {
            let _ = self.start_file_comment();
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
        if self.file_review_comments_supported() {
            let Some(anchor) = self.resolve_file_review_anchor() else {
                return false;
            };
            return self.request_delete_comment_by_anchor(anchor.anchor_key);
        }
        let Some(anchor) = self.resolve_line_review_anchor() else {
            return false;
        };
        self.request_delete_comment_by_anchor(anchor.anchor_key)
    }

    pub fn remove_hunk_comment_at_cursor(&mut self) -> bool {
        if !self.review_mode {
            return false;
        }
        let Some(anchor) = self.resolve_hunk_review_anchor() else {
            return false;
        };
        self.request_delete_comment_by_anchor(anchor.anchor_key)
    }

    pub fn clear_all_review_comments(&mut self) -> bool {
        if !self.review_mode || self.review_comment_count() == 0 {
            return false;
        }
        let count = self.review_comment_count();
        self.review_delete_confirmation = Some(ReviewDeleteConfirmation {
            target: ReviewDeleteTarget::All,
            title: "Delete all comments?".to_string(),
            body: format!("This deletes {count} comments. This cannot be undone."),
            confirm_label: "d delete all".to_string(),
        });
        true
    }

    fn clear_all_review_comments_now(&mut self) -> bool {
        if !self.review_mode || self.review_comments.is_empty() {
            return false;
        }
        self.review_comments.clear();
        self.active_review_comment_id = None;
        self.review_editor = None;
        self.review_mention_picker = None;
        self.review_next_comment_id = 1;
        self.touch_review_state();
        self.persist_review_session();
        self.run_review_hooks(ReviewHookEvent::CommentsCleared, None);
        self.notify(ToastEvent::CommentsCleared);
        true
    }

    pub(crate) fn review_session_has_changes(&self) -> bool {
        self.review_comments != self.review_session_baseline || self.review_editor.is_some()
    }

    fn discard_review_session_summary(&self) -> String {
        let mut new_comments = 0usize;
        let mut edited_comments = 0usize;
        let mut deleted_comments = 0usize;

        let baseline = self
            .review_session_baseline
            .iter()
            .map(|comment| (comment.id, comment))
            .collect::<BTreeMap<_, _>>();
        let current = self
            .review_comments
            .iter()
            .map(|comment| (comment.id, comment))
            .collect::<BTreeMap<_, _>>();

        for comment in self
            .review_comments
            .iter()
            .filter(|comment| !comment.deleted)
        {
            match baseline.get(&comment.id) {
                None => new_comments += 1,
                Some(old) if *old != comment => edited_comments += 1,
                _ => {}
            }
        }
        for comment in self
            .review_session_baseline
            .iter()
            .filter(|comment| !comment.deleted)
        {
            if current
                .get(&comment.id)
                .is_none_or(|current| current.deleted)
            {
                deleted_comments += 1;
            }
        }

        let editor_changed = self.review_editor.as_ref().is_some_and(|editor| {
            self.review_comments
                .iter()
                .find(|comment| {
                    !comment.deleted && comment.anchor.anchor_key == editor.anchor.anchor_key
                })
                .map(|comment| comment.body.as_str())
                .unwrap_or_default()
                != editor.text.as_str()
        });

        let comment_word = |count| if count == 1 { "comment" } else { "comments" };
        let mut lines = vec!["This will discard:".to_string()];
        if new_comments > 0 {
            lines.push(format!(
                "• {new_comments} new {}",
                comment_word(new_comments)
            ));
        }
        if edited_comments > 0 {
            lines.push(format!(
                "• {edited_comments} edited {}",
                comment_word(edited_comments)
            ));
        }
        if deleted_comments > 0 {
            lines.push(format!(
                "• {deleted_comments} deleted {}",
                comment_word(deleted_comments)
            ));
        }
        if editor_changed {
            lines.push("• open editor draft".to_string());
        }
        if lines.len() == 1 {
            lines.push("• open editor state".to_string());
        }
        lines.join("\n")
    }

    pub(crate) fn request_discard_review_session_changes(&mut self) -> bool {
        if !self.review_session_has_changes() {
            return false;
        }
        self.review_delete_confirmation = Some(ReviewDeleteConfirmation {
            target: ReviewDeleteTarget::DiscardSession,
            title: "Discard review changes?".to_string(),
            body: self.discard_review_session_summary(),
            confirm_label: "d discard".to_string(),
        });
        true
    }

    fn discard_review_session_changes_now(&mut self) -> bool {
        if !self.review_session_has_changes() {
            return false;
        }
        self.review_comments = self.review_session_baseline.clone();
        self.active_review_comment_id = None;
        self.review_editor = None;
        self.review_mention_picker = None;
        self.review_next_comment_id = self
            .review_comments
            .iter()
            .map(|comment| comment.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.touch_review_state();
        self.persist_review_session();
        self.notify(ToastEvent::SelectionActionStarted(
            "Review changes discarded".to_string(),
        ));
        true
    }

    pub(crate) fn request_review_sync_action(
        &mut self,
        action: ReviewSyncAction,
        remote: Option<String>,
    ) {
        self.comments_sidebar_overflow_open = false;
        self.comments_sidebar_overflow_menu_hover = None;
        self.review_sync_requested = Some(ReviewSyncRequest { action, remote });
    }

    pub(crate) fn run_comments_sidebar_sync(&mut self) {
        self.request_review_sync_action(ReviewSyncAction::Sync, None);
    }

    pub(crate) fn take_review_sync_requested(&mut self) -> Option<ReviewSyncRequest> {
        self.review_sync_requested.take()
    }

    pub(crate) fn set_review_sync_status(&mut self, status: Option<ReviewSyncAction>) {
        self.review_sync_status = status;
    }

    pub(crate) fn review_sync_status(&self) -> Option<ReviewSyncAction> {
        self.review_sync_status
    }

    pub(crate) fn mark_review_session_clean(&mut self) {
        self.review_session_baseline = self.review_comments.clone();
    }

    pub(crate) fn toggle_comments_sidebar_overflow(&mut self) {
        self.comments_sidebar_overflow_open = !self.comments_sidebar_overflow_open;
    }

    pub(crate) fn set_comments_sidebar_overflow_hits(
        &mut self,
        hits: Vec<ReviewSidebarOverflowHit>,
    ) {
        self.comments_sidebar_overflow_hits = hits;
    }

    pub(crate) fn handle_comments_sidebar_overflow_click(&mut self, column: u16, row: u16) -> bool {
        if self
            .comments_sidebar_overflow_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.toggle_comments_sidebar_overflow();
            return true;
        }
        if !self.comments_sidebar_overflow_open {
            return false;
        }
        let action = self.comments_sidebar_overflow_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.action)
        });
        if let Some(action) = action {
            self.request_review_sync_action(action, None);
        } else {
            self.comments_sidebar_overflow_open = false;
        }
        true
    }

    pub(crate) fn open_review_remote_picker(
        &mut self,
        action: ReviewSyncAction,
        remotes: Vec<ReviewRemoteOption>,
    ) {
        self.review_remote_picker = Some(ReviewRemotePickerState {
            action,
            remotes,
            selected: 0,
            query: String::new(),
        });
        self.file_filter_cursor_visible = true;
        self.file_filter_cursor_last_blink = std::time::Instant::now();
        self.review_remote_picker_hover = None;
    }

    pub(crate) fn review_remote_picker_active(&self) -> bool {
        self.review_remote_picker.is_some()
    }

    pub(crate) fn review_remote_picker_render(&self) -> Option<&ReviewRemotePickerState> {
        self.review_remote_picker.as_ref()
    }

    pub(crate) fn set_review_remote_picker_hits(&mut self, hits: Vec<ReviewRemotePickerHit>) {
        self.review_remote_picker_hits = hits;
    }

    pub(crate) fn cancel_review_remote_picker(&mut self) {
        self.review_remote_picker = None;
        self.review_remote_picker_hits.clear();
        self.review_remote_picker_hover = None;
    }

    fn review_remote_matches(remote: &ReviewRemoteOption, query: &str) -> bool {
        let query = query.trim().to_ascii_lowercase();
        query.is_empty()
            || remote.name.to_ascii_lowercase().contains(&query)
            || remote.label.to_ascii_lowercase().contains(&query)
    }

    fn review_remote_picker_filtered_indices(&self) -> Vec<usize> {
        let Some(picker) = self.review_remote_picker.as_ref() else {
            return Vec::new();
        };
        picker
            .remotes
            .iter()
            .enumerate()
            .filter_map(|(idx, remote)| {
                Self::review_remote_matches(remote, &picker.query).then_some(idx)
            })
            .collect()
    }

    fn reset_review_remote_picker_cursor(&mut self) {
        self.file_filter_cursor_visible = true;
        self.file_filter_cursor_last_blink = std::time::Instant::now();
    }

    fn select_first_matching_review_remote(&mut self) {
        let Some(first) = self
            .review_remote_picker_filtered_indices()
            .first()
            .copied()
        else {
            return;
        };
        if let Some(picker) = self.review_remote_picker.as_mut() {
            picker.selected = first;
        }
    }

    pub(crate) fn move_review_remote_picker(&mut self, delta: isize) {
        let indices = self.review_remote_picker_filtered_indices();
        if indices.is_empty() {
            return;
        }
        let Some(picker) = self.review_remote_picker.as_mut() else {
            return;
        };
        let current = indices
            .iter()
            .position(|idx| *idx == picker.selected)
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(indices.len() as isize) as usize;
        picker.selected = indices[next];
    }

    pub(crate) fn choose_review_remote_picker(&mut self, index: Option<usize>) -> bool {
        let Some(picker) = self.review_remote_picker.as_ref() else {
            return false;
        };
        let idx = index.unwrap_or(picker.selected);
        let Some(remote) = picker.remotes.get(idx).cloned() else {
            return false;
        };
        if !Self::review_remote_matches(&remote, &picker.query) {
            return false;
        }
        let action = picker.action;
        self.review_remote_picker = None;
        self.review_remote_picker_hits.clear();
        self.review_remote_picker_hover = None;
        self.request_review_sync_action(action, Some(remote.name));
        true
    }

    pub(crate) fn update_review_remote_picker_hover(&mut self, column: u16, row: u16) -> bool {
        let hover = self.review_remote_picker_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.index)
        });
        if self.review_remote_picker_hover == hover {
            return false;
        }
        if let (Some(idx), Some(picker)) = (hover, self.review_remote_picker.as_mut()) {
            picker.selected = idx.min(picker.remotes.len().saturating_sub(1));
        }
        self.review_remote_picker_hover = hover;
        true
    }

    pub(crate) fn handle_review_remote_picker_click(&mut self, column: u16, row: u16) -> bool {
        let selected = self.review_remote_picker_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.index)
        });
        if let Some(index) = selected {
            self.choose_review_remote_picker(Some(index));
        } else {
            self.cancel_review_remote_picker();
        }
        true
    }

    pub(crate) fn handle_review_remote_picker_key(&mut self, key: KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};

        match key.code {
            KeyCode::Esc => self.cancel_review_remote_picker(),
            KeyCode::Enter => {
                self.choose_review_remote_picker(None);
            }
            KeyCode::Up => self.move_review_remote_picker(-1),
            KeyCode::Down => self.move_review_remote_picker(1),
            KeyCode::Backspace => {
                let Some(picker) = self.review_remote_picker.as_mut() else {
                    return true;
                };
                if picker.query.is_empty() {
                    self.cancel_review_remote_picker();
                } else {
                    picker.query.pop();
                    self.select_first_matching_review_remote();
                    self.reset_review_remote_picker_cursor();
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(picker) = self.review_remote_picker.as_mut() {
                    picker.query.clear();
                    picker.selected = 0;
                }
                self.reset_review_remote_picker_cursor();
            }
            KeyCode::Char(ch)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                if let Some(picker) = self.review_remote_picker.as_mut() {
                    picker.query.push(ch);
                }
                self.select_first_matching_review_remote();
                self.reset_review_remote_picker_cursor();
            }
            _ => {}
        }
        true
    }

    pub(crate) fn handle_comments_sidebar_action_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers != crossterm::event::KeyModifiers::NONE {
            return false;
        }
        match key.code {
            crossterm::event::KeyCode::Char('s') => {
                self.run_comments_sidebar_sync();
                true
            }
            crossterm::event::KeyCode::Char('d') => self.request_discard_review_session_changes(),
            _ => false,
        }
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
        let wrap_width = self.review_editor_wrap_width;
        if wrap_width > 0 {
            let (line_start, line_end) = logical_line_bounds(&editor.text, row);
            let line = &editor.text[line_start..line_end];
            let (visual_row, visual_col) =
                visual_pos_for_cursor(line, editor.cursor.saturating_sub(line_start), wrap_width);
            if visual_row > 0 {
                editor.cursor =
                    cursor_for_visual_pos(line, line_start, visual_row - 1, visual_col, wrap_width);
                self.refresh_review_mention_picker();
                return;
            }
        }
        if row == 0 {
            return;
        }
        if wrap_width > 0 {
            let (line_start, line_end) = logical_line_bounds(&editor.text, row - 1);
            let line = &editor.text[line_start..line_end];
            let last_visual_row = visual_row_count(line, wrap_width).saturating_sub(1);
            editor.cursor =
                cursor_for_visual_pos(line, line_start, last_visual_row, col, wrap_width);
        } else {
            editor.cursor = cursor_for_row_col(&editor.text, row - 1, col);
        }
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
        let wrap_width = self.review_editor_wrap_width;
        if wrap_width > 0 {
            let (line_start, line_end) = logical_line_bounds(&editor.text, row);
            let line = &editor.text[line_start..line_end];
            let (visual_row, visual_col) =
                visual_pos_for_cursor(line, editor.cursor.saturating_sub(line_start), wrap_width);
            if visual_row + 1 < visual_row_count(line, wrap_width) {
                editor.cursor =
                    cursor_for_visual_pos(line, line_start, visual_row + 1, visual_col, wrap_width);
                self.refresh_review_mention_picker();
                return;
            }
        }
        if row + 1 >= starts.len() {
            return;
        }
        if wrap_width > 0 {
            let (line_start, line_end) = logical_line_bounds(&editor.text, row + 1);
            let line = &editor.text[line_start..line_end];
            editor.cursor = cursor_for_visual_pos(line, line_start, 0, col, wrap_width);
        } else {
            editor.cursor = cursor_for_row_col(&editor.text, row + 1, col);
        }
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

        let existing_idx = self.review_comments.iter().position(|c| {
            !c.deleted && c.can_edit && c.anchor.anchor_key == editor.anchor.anchor_key
        });

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
            let author = self.review_author.clone();
            if let Some(existing) = self.review_comments.get_mut(idx) {
                existing.body = body;
                existing.anchor = editor.anchor;
                if existing.author.is_none() {
                    existing.author = author;
                }
                if let Some(provider) = existing.provider.as_mut() {
                    provider.sync_state = "dirty".to_string();
                }
                existing.updated_at = now;
            }
        } else {
            let id = self.review_next_comment_id;
            self.review_next_comment_id = self.review_next_comment_id.saturating_add(1);
            self.review_comments.push(ReviewComment {
                id,
                anchor: editor.anchor,
                body,
                author: self.review_author.clone(),
                can_edit: true,
                deleted: false,
                provider: None,
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
        self.touch_review_state();
        self.persist_review_session();
        self.notify(ToastEvent::ReviewSubmitted);
        self.should_quit = true;
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
        if let Some(path) = self.review_db_path.as_ref() {
            command.env("OYO_REVIEW_DB", path);
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
            .filter(|comment| !comment.deleted)
            .map(|comment| ReviewExportComment {
                id: comment.id,
                file: &comment.anchor.file_path,
                kind: match comment.anchor.kind {
                    ReviewTargetKind::PullRequest => "pr",
                    ReviewTargetKind::File => "file",
                    ReviewTargetKind::Line => "line",
                    ReviewTargetKind::Hunk => "hunk",
                },
                side: comment.anchor.side.map(ReviewSide::as_str),
                old_range: comment.anchor.old_range,
                new_range: comment.anchor.new_range,
                author: comment.author.as_ref(),
                created_at: comment.created_at,
                updated_at: comment.updated_at,
                body: &comment.body,
            })
            .collect();
        let export = ReviewExport {
            version: 1,
            event: review_event_name(event),
            repo_root: self.review_repo_root.clone().unwrap_or_default(),
            review_db: self
                .review_db_path
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

    fn mark_or_remove_review_comment(&mut self, idx: usize) {
        if self.active_review_comment_id == Some(self.review_comments[idx].id) {
            self.active_review_comment_id = None;
        }
        if self.review_comments[idx].provider.is_some() {
            self.review_comments[idx].deleted = true;
            self.review_comments[idx].updated_at = now_ts();
            if let Some(provider) = self.review_comments[idx].provider.as_mut() {
                provider.sync_state = "deleted".to_string();
            }
        } else {
            self.review_comments.remove(idx);
        }
    }

    fn remove_comment_for_anchor_key(&mut self, anchor_key: &str) -> bool {
        if let Some(idx) = self
            .review_comments
            .iter()
            .position(|c| !c.deleted && c.can_edit && c.anchor.anchor_key == anchor_key)
        {
            self.mark_or_remove_review_comment(idx);
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
        let existing = self
            .review_comments
            .iter()
            .find(|c| !c.deleted && c.anchor.anchor_key == anchor.anchor_key);
        self.active_review_comment_id = existing.map(|comment| comment.id);
        let text = existing.map(|c| c.body.clone()).unwrap_or_default();
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
        self.stop_theme_picker();
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

    fn resolve_file_review_anchor(&self) -> Option<ReviewAnchor> {
        file_review_anchor(self.multi_diff.selected_index, self.current_file_path())
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

    fn review_db(path: &Path) -> rusqlite::Result<Connection> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reviews (
                diff_fingerprint TEXT PRIMARY KEY,
                repo_root TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                editor_json TEXT,
                target_json TEXT
            );
            CREATE TABLE IF NOT EXISTS comments (
                diff_fingerprint TEXT NOT NULL,
                id INTEGER NOT NULL,
                comment_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (diff_fingerprint, id)
            );",
        )?;
        Ok(conn)
    }

    fn persist_review_session(&mut self) {
        if !self.review_mode || !self.review_persist_enabled {
            return;
        }
        let Some(path) = self.review_db_path.clone() else {
            return;
        };
        let Ok(mut conn) = Self::review_db(&path) else {
            return;
        };
        let Ok(tx) = conn.transaction() else {
            return;
        };
        let now = now_ts();
        let editor_json = self
            .review_editor
            .as_ref()
            .and_then(|editor| serde_json::to_string(editor).ok());
        let target_json = self
            .review_target_metadata
            .as_ref()
            .and_then(|target| serde_json::to_string(target).ok());
        if tx
            .execute(
                "INSERT INTO reviews (
                    diff_fingerprint, repo_root, created_at, updated_at, editor_json, target_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(diff_fingerprint) DO UPDATE SET
                    repo_root = excluded.repo_root,
                    updated_at = excluded.updated_at,
                    editor_json = excluded.editor_json,
                    target_json = excluded.target_json",
                params![
                    self.review_diff_fingerprint,
                    self.review_repo_root.clone().unwrap_or_default(),
                    self.review_session_created_at as i64,
                    now as i64,
                    editor_json,
                    target_json,
                ],
            )
            .is_err()
        {
            return;
        }
        if tx
            .execute(
                "DELETE FROM comments WHERE diff_fingerprint = ?1",
                params![self.review_diff_fingerprint],
            )
            .is_err()
        {
            return;
        }
        for comment in &self.review_comments {
            let Ok(comment_json) = serde_json::to_string(comment) else {
                return;
            };
            if tx
                .execute(
                    "INSERT INTO comments (diff_fingerprint, id, comment_json, updated_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        self.review_diff_fingerprint,
                        comment.id as i64,
                        comment_json,
                        comment.updated_at as i64,
                    ],
                )
                .is_err()
            {
                return;
            }
        }
        if tx.commit().is_ok() {
            self.review_db_stamp = review_db_stamp(&path);
            self.last_review_db_check = Instant::now();
        }
    }

    pub(crate) fn maybe_watch_reload_review_state(&mut self) -> bool {
        if !self.review_mode || !self.review_persist_enabled || self.review_editor.is_some() {
            return false;
        }
        let now = Instant::now();
        if now.duration_since(self.last_review_db_check) < Duration::from_secs(1) {
            return false;
        }
        self.last_review_db_check = now;
        let Some(path) = self.review_db_path.clone() else {
            return false;
        };
        let stamp = review_db_stamp(&path);
        if stamp == self.review_db_stamp {
            return false;
        }
        if !self.load_review_state(&path) {
            self.review_db_stamp = stamp;
            return false;
        }
        self.repair_review_comment_file_indexes();
        self.repair_review_editor_file_index();
        self.refresh_review_mention_picker();
        self.review_next_comment_id = self
            .review_comments
            .iter()
            .map(|comment| comment.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.review_session_baseline = self.review_comments.clone();
        self.touch_review_state();
        true
    }

    pub(crate) fn load_review_by_fingerprint(&mut self, fingerprint: &str) -> bool {
        self.review_diff_fingerprint = fingerprint.to_string();
        let Some(path) = self.review_db_path.clone() else {
            return false;
        };
        self.load_review_state(&path)
    }

    pub(crate) fn load_review_snapshot_into_current_target(&mut self, fingerprint: &str) -> bool {
        self.load_review_snapshots_into_current_target(&[fingerprint.to_string()])
    }

    pub(crate) fn load_review_snapshots_into_current_target(
        &mut self,
        fingerprints: &[String],
    ) -> bool {
        let current_fingerprint = self.review_diff_fingerprint.clone();
        let current_metadata = self.review_target_metadata.clone();
        let mut comments = Vec::new();
        let mut seen = BTreeSet::new();
        for fingerprint in fingerprints {
            if !self.load_review_by_fingerprint(fingerprint) {
                continue;
            }
            for comment in self
                .review_comments
                .iter()
                .filter(|comment| !comment.deleted)
            {
                let key = (
                    comment.anchor.anchor_key.clone(),
                    comment.body.clone(),
                    serde_json::to_string(&comment.provider).unwrap_or_default(),
                );
                if seen.insert(key) {
                    comments.push(comment.clone());
                }
            }
        }
        self.review_diff_fingerprint = current_fingerprint;
        self.review_target_metadata = current_metadata;
        if comments.is_empty() {
            return false;
        }
        for (index, comment) in comments.iter_mut().enumerate() {
            comment.id = index.saturating_add(1) as u64;
        }
        self.review_comments = comments;
        self.review_editor = None;
        self.repair_review_comment_file_indexes();
        self.repair_review_editor_file_index();
        self.refresh_review_mention_picker();
        self.review_next_comment_id = self
            .review_comments
            .iter()
            .map(|c| c.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.review_session_baseline = self.review_comments.clone();
        self.touch_review_state();
        true
    }

    fn load_review_state(&mut self, path: &Path) -> bool {
        if !path.exists() {
            return false;
        }
        let Ok(conn) = Self::review_db(path) else {
            return false;
        };
        let row = conn
            .query_row(
                "SELECT created_at, editor_json, target_json FROM reviews WHERE diff_fingerprint = ?1",
                params![self.review_diff_fingerprint],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional();
        let Ok(Some((created_at, editor_json, target_json))) = row else {
            return false;
        };
        let mut stmt = match conn
            .prepare("SELECT comment_json FROM comments WHERE diff_fingerprint = ?1 ORDER BY id")
        {
            Ok(stmt) => stmt,
            Err(_) => return false,
        };
        let rows = match stmt.query_map(params![self.review_diff_fingerprint], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(rows) => rows,
            Err(_) => return false,
        };
        let mut comments = Vec::new();
        for row in rows {
            let Ok(data) = row else {
                return false;
            };
            let Ok(comment) = serde_json::from_str::<ReviewComment>(&data) else {
                return false;
            };
            comments.push(comment);
        }
        self.review_session_created_at = created_at.max(0) as u64;
        self.review_editor = editor_json.and_then(|json| serde_json::from_str(&json).ok());
        self.review_target_metadata = target_json.and_then(|json| serde_json::from_str(&json).ok());
        self.review_comments = comments;
        if self.review_filter_to_current_diff {
            self.filter_review_comments_to_current_diff();
        }
        self.review_db_stamp = review_db_stamp(path);
        self.last_review_db_check = Instant::now();
        true
    }

    fn public_review_comments_json(&self) -> String {
        let comments = self
            .review_comments
            .iter()
            .filter(|comment| !comment.deleted)
            .map(|comment| PublicReviewComment {
                id: Some(comment.id),
                file: comment.anchor.file_path.clone(),
                kind: Some(
                    match comment.anchor.kind {
                        ReviewTargetKind::PullRequest => "pr",
                        ReviewTargetKind::File => "file",
                        ReviewTargetKind::Line => "line",
                        ReviewTargetKind::Hunk => "hunk",
                    }
                    .to_string(),
                ),
                side: comment.anchor.side.map(|side| side.as_str().to_string()),
                old_range: comment.anchor.old_range,
                old_range_camel: None,
                new_range: comment.anchor.new_range,
                new_range_camel: None,
                hunk_id: comment.anchor.hunk_id,
                hunk_id_camel: None,
                author: comment.author.clone(),
                can_edit: Some(comment.can_edit),
                provider: comment.provider.clone(),
                created_at: Some(comment.created_at),
                updated_at: Some(comment.updated_at),
                body: comment.body.clone(),
            })
            .collect();
        let file = PublicReviewComments {
            version: 1,
            comments,
        };
        serde_json::to_string_pretty(&file).unwrap_or_else(|_| "{}".to_string())
    }

    pub(crate) fn parse_review_comments_json_for_sync(
        &self,
        data: &str,
    ) -> Result<Vec<ReviewComment>, String> {
        self.parse_public_review_comments(data)
    }

    fn parse_public_review_comments(&self, data: &str) -> Result<Vec<ReviewComment>, String> {
        let file: PublicReviewComments = serde_json::from_str(data).map_err(|e| e.to_string())?;
        if file.version != 1 {
            return Err("Unsupported comments JSON version".to_string());
        }
        let mut comments = Vec::new();
        for item in file.comments {
            comments.push(self.public_comment_to_review_comment(item)?);
        }
        Ok(comments)
    }

    fn public_comment_to_review_comment(
        &self,
        item: PublicReviewComment,
    ) -> Result<ReviewComment, String> {
        let kind = match item.kind.as_deref().unwrap_or("line") {
            "review" | "pr" | "pull_request" => ReviewTargetKind::PullRequest,
            "file" => ReviewTargetKind::File,
            "line" => ReviewTargetKind::Line,
            "hunk" => ReviewTargetKind::Hunk,
            other => return Err(format!("Unsupported comment kind: {other}")),
        };
        let file_index = if kind == ReviewTargetKind::PullRequest {
            0
        } else {
            self.multi_diff
                .files
                .iter()
                .position(|file| {
                    file.display_name == item.file || file.path == Path::new(&item.file)
                })
                .ok_or_else(|| format!("No changed file matches {}", item.file))?
        };
        let side = match item.side.as_deref() {
            Some("old") => Some(ReviewSide::Old),
            Some("new") => Some(ReviewSide::New),
            Some(other) => return Err(format!("Unsupported comment side: {other}")),
            None => None,
        };
        let old_range = item.old_range.or(item.old_range_camel);
        let new_range = item.new_range.or(item.new_range_camel);
        let hunk_id = item.hunk_id.or(item.hunk_id_camel);
        let mut anchor_key = match kind {
            ReviewTargetKind::PullRequest => "pr".to_string(),
            ReviewTargetKind::File => format!("file|{}", item.file),
            ReviewTargetKind::Line => match side {
                Some(side) => {
                    let line_no = match side {
                        ReviewSide::Old => old_range.map(|range| range.start),
                        ReviewSide::New => new_range.map(|range| range.start),
                    }
                    .ok_or_else(|| "Line comments need a matching range".to_string())?;
                    format!("line|{}|{}|{}", item.file, side.as_str(), line_no)
                }
                None => format!(
                    "line|{}|both|{}|{}",
                    item.file,
                    format_opt_range(old_range),
                    format_opt_range(new_range)
                ),
            },
            ReviewTargetKind::Hunk => format!(
                "hunk|{}|{}|{}",
                item.file,
                format_opt_range(old_range),
                format_opt_range(new_range)
            ),
        };
        if let Some(provider) = item.provider.as_ref() {
            anchor_key = format!(
                "{}|provider|{}|{}|{}",
                anchor_key, provider.provider, provider.repo, provider.comment_id
            );
        }
        let now = now_ts();
        Ok(ReviewComment {
            id: item.id.unwrap_or(0),
            anchor: ReviewAnchor {
                file_index,
                file_path: item.file,
                kind,
                side,
                old_range,
                new_range,
                hunk_id,
                display_idx_hint: None,
                anchor_key,
            },
            body: item.body,
            author: item.author.or_else(|| self.review_author.clone()),
            can_edit: item.can_edit.unwrap_or(true),
            deleted: false,
            provider: item.provider,
            created_at: item.created_at.unwrap_or(now),
            updated_at: item.updated_at.or(item.created_at).unwrap_or(now),
        })
    }

    pub fn add_review_comment_from_cli(
        &mut self,
        file: &str,
        kind: ReviewTargetKind,
        side: Option<ReviewSide>,
        old_range: Option<ReviewRange>,
        new_range: Option<ReviewRange>,
        body: String,
    ) -> Result<u64, String> {
        let comment = self.public_comment_to_review_comment(PublicReviewComment {
            id: None,
            file: file.to_string(),
            kind: Some(
                match kind {
                    ReviewTargetKind::PullRequest => "pr",
                    ReviewTargetKind::File => "file",
                    ReviewTargetKind::Line => "line",
                    ReviewTargetKind::Hunk => "hunk",
                }
                .to_string(),
            ),
            side: side.map(|side| side.as_str().to_string()),
            old_range,
            old_range_camel: None,
            new_range,
            new_range_camel: None,
            hunk_id: None,
            hunk_id_camel: None,
            author: None,
            can_edit: None,
            provider: None,
            created_at: None,
            updated_at: None,
            body,
        })?;
        let id = self.review_next_comment_id;
        self.review_next_comment_id = self.review_next_comment_id.saturating_add(1);
        let now = now_ts();
        let mut comment = comment;
        comment.id = id;
        comment.created_at = now;
        comment.updated_at = now;
        self.review_comments.push(comment);
        self.touch_review_state();
        self.persist_review_session();
        Ok(id)
    }

    pub fn edit_review_comment_from_cli(&mut self, id: u64, body: String) -> bool {
        let Some(comment) = self
            .review_comments
            .iter_mut()
            .find(|comment| comment.id == id && !comment.deleted && comment.can_edit)
        else {
            return false;
        };
        comment.body = body;
        if let Some(provider) = comment.provider.as_mut() {
            provider.sync_state = "dirty".to_string();
        }
        comment.updated_at = now_ts();
        self.touch_review_state();
        self.persist_review_session();
        true
    }

    pub fn remove_review_comment_from_cli(&mut self, id: u64) -> bool {
        let Some(idx) = self
            .review_comments
            .iter()
            .position(|comment| comment.id == id && !comment.deleted && comment.can_edit)
        else {
            return false;
        };
        self.mark_or_remove_review_comment(idx);
        self.touch_review_state();
        self.persist_review_session();
        true
    }

    pub(crate) fn review_comments_for_sync(&self) -> Vec<ReviewComment> {
        self.review_comments.clone()
    }

    pub(crate) fn mark_review_comment_synced(
        &mut self,
        id: u64,
        provider: ReviewProviderComment,
    ) -> bool {
        let Some(idx) = self
            .review_comments
            .iter()
            .position(|comment| comment.id == id)
        else {
            return false;
        };
        if self.review_comments[idx].deleted {
            self.review_comments.remove(idx);
        } else {
            self.review_comments[idx].provider = Some(provider);
            self.review_comments[idx].can_edit = true;
        }
        self.touch_review_state();
        self.persist_review_session();
        true
    }

    pub(crate) fn upsert_provider_review_comment(&mut self, mut comment: ReviewComment) -> u64 {
        let provider_comment_id = comment.provider.as_ref().map(|provider| {
            (
                provider.provider.clone(),
                provider.repo.clone(),
                provider.pr_number,
                provider.comment_id.clone(),
            )
        });
        if let Some(provider_comment_id) = provider_comment_id {
            if let Some(existing) = self.review_comments.iter_mut().find(|existing| {
                existing.provider.as_ref().map(|provider| {
                    (
                        provider.provider.clone(),
                        provider.repo.clone(),
                        provider.pr_number,
                        provider.comment_id.clone(),
                    )
                }) == Some(provider_comment_id.clone())
            }) {
                let id = existing.id;
                let has_local_change = existing.deleted
                    || existing.provider.as_ref().is_some_and(|provider| {
                        matches!(provider.sync_state.as_str(), "dirty" | "deleted")
                    });
                if has_local_change {
                    return id;
                }
                comment.id = id;
                *existing = comment;
                self.touch_review_state();
                self.persist_review_session();
                return id;
            }
        }
        let id = self.review_next_comment_id;
        self.review_next_comment_id = self.review_next_comment_id.saturating_add(1);
        comment.id = id;
        self.review_comments.push(comment);
        self.touch_review_state();
        self.persist_review_session();
        id
    }

    pub fn apply_review_comments_from_cli(&mut self, data: &str) -> Result<Vec<u64>, String> {
        let mut comments = self.parse_public_review_comments(data)?;
        let mut ids = Vec::new();
        for mut comment in comments.drain(..) {
            if comment.id == 0 {
                comment.id = self.review_next_comment_id;
                self.review_next_comment_id = self.review_next_comment_id.saturating_add(1);
            }
            ids.push(comment.id);
            if let Some(existing) = self
                .review_comments
                .iter_mut()
                .find(|existing| existing.id == comment.id)
            {
                *existing = comment;
            } else {
                self.review_comments.push(comment);
            }
        }
        self.touch_review_state();
        self.persist_review_session();
        Ok(ids)
    }

    pub fn abandon_review_from_cli(&mut self) -> bool {
        let removed = self
            .review_db_path
            .as_ref()
            .and_then(|path| Self::review_db(path).ok())
            .map(|conn| {
                let comments = conn
                    .execute(
                        "DELETE FROM comments WHERE diff_fingerprint = ?1",
                        params![self.review_diff_fingerprint],
                    )
                    .unwrap_or(0);
                let reviews = conn
                    .execute(
                        "DELETE FROM reviews WHERE diff_fingerprint = ?1",
                        params![self.review_diff_fingerprint],
                    )
                    .unwrap_or(0);
                comments > 0 || reviews > 0
            })
            .unwrap_or(false);
        self.review_comments.clear();
        self.review_editor = None;
        self.review_mention_picker = None;
        self.review_next_comment_id = 1;
        self.touch_review_state();
        removed
    }

    fn review_file_index_for_path(&self, path: &str) -> Option<usize> {
        self.multi_diff
            .files
            .iter()
            .position(|file| file.display_name == path)
    }

    fn current_diff_file_indexes(&self) -> std::collections::HashMap<String, usize> {
        self.multi_diff
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.display_name.clone(), index))
            .collect()
    }

    fn filter_review_comments_to_current_diff(&mut self) {
        let paths = self.current_diff_file_indexes();
        self.review_comments.retain(|comment| {
            comment.deleted
                || (comment.anchor.kind != ReviewTargetKind::PullRequest
                    && paths.contains_key(&comment.anchor.file_path))
        });
    }

    fn repair_review_comment_file_indexes(&mut self) {
        let paths = self.current_diff_file_indexes();
        for comment in &mut self.review_comments {
            if let Some(index) = paths.get(&comment.anchor.file_path) {
                comment.anchor.file_index = *index;
            }
        }
    }

    fn repair_review_editor_file_index(&mut self) {
        let Some(path) = self
            .review_editor
            .as_ref()
            .map(|editor| editor.anchor.file_path.clone())
        else {
            return;
        };
        let Some(index) = self.review_file_index_for_path(&path) else {
            return;
        };
        if let Some(editor) = self.review_editor.as_mut() {
            editor.anchor.file_index = index;
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
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| !comment.deleted)
            .cloned()
            .collect::<Vec<_>>();
        comments.sort_by(|a, b| {
            a.anchor
                .file_path
                .cmp(&b.anchor.file_path)
                .then_with(|| {
                    let rank = |kind| match kind {
                        ReviewTargetKind::PullRequest => 0,
                        ReviewTargetKind::File => 1,
                        ReviewTargetKind::Line => 2,
                        ReviewTargetKind::Hunk => 3,
                    };
                    rank(a.anchor.kind).cmp(&rank(b.anchor.kind))
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
        if let Some(author) = &comment.author {
            let mut author_label = match &author.email {
                Some(email) if !email.trim().is_empty() => format!("{} <{}>", author.name, email),
                _ => author.name.clone(),
            };
            if let Some(author_type) = review_author_type_label(author) {
                author_label.push_str(&format!(" ({author_type})"));
            }
            lines.push(format!("Author: {author_label}"));
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
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app
    }

    fn provider_link(state: &str) -> ReviewProviderComment {
        ReviewProviderComment {
            provider: "github".to_string(),
            remote: "origin".to_string(),
            repo: "owner/repo".to_string(),
            pr_number: 1,
            comment_id: "10".to_string(),
            thread_id: None,
            author_username: Some("reviewer".to_string()),
            pr_title: Some("PR".to_string()),
            api_kind: "review".to_string(),
            sync_state: state.to_string(),
        }
    }

    fn line_comment() -> ReviewComment {
        ReviewComment {
            id: 1,
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
            author: None,
            can_edit: true,
            deleted: false,
            provider: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn pull_preserves_dirty_local_provider_comment() {
        let mut app = test_app();
        let mut local = line_comment();
        local.body = "local edit".to_string();
        local.provider = Some(provider_link("dirty"));
        app.review_comments.push(local);

        let mut remote = line_comment();
        remote.body = "remote old body".to_string();
        remote.provider = Some(provider_link("clean"));

        let id = app.upsert_provider_review_comment(remote);

        assert_eq!(id, 1);
        assert_eq!(app.review_comments[0].body, "local edit");
        assert_eq!(
            app.review_comments[0]
                .provider
                .as_ref()
                .map(|provider| provider.sync_state.as_str()),
            Some("dirty")
        );
    }

    #[test]
    fn pull_preserves_deleted_local_provider_comment() {
        let mut app = test_app();
        let mut local = line_comment();
        local.deleted = true;
        local.provider = Some(provider_link("deleted"));
        app.review_comments.push(local);

        let mut remote = line_comment();
        remote.body = "remote old body".to_string();
        remote.deleted = false;
        remote.provider = Some(provider_link("clean"));

        let id = app.upsert_provider_review_comment(remote);

        assert_eq!(id, 1);
        assert!(app.review_comments[0].deleted);
        assert_eq!(
            app.review_comments[0]
                .provider
                .as_ref()
                .map(|provider| provider.sync_state.as_str()),
            Some("deleted")
        );
    }

    #[test]
    fn review_edit_letter_opens_visible_comment() {
        let mut app = test_app();
        app.review_comments.push(line_comment());

        assert!(app.edit_review_comment_letter('a'));
        assert_eq!(app.active_review_comment_id, Some(1));
        assert!(app.review_editor_active());
    }

    #[test]
    fn review_editor_up_down_follow_wrapped_lines() {
        let mut app = test_app();
        app.start_line_comment();
        for ch in "abcdefg".chars() {
            app.review_insert_char(ch);
        }
        app.set_review_editor_wrap_width(3);

        app.review_move_up();
        let editor = app.review_editor_render().unwrap();
        assert_eq!(editor.cursor_col, 4);

        app.review_move_down();
        let editor = app.review_editor_render().unwrap();
        assert_eq!(editor.cursor_col, 7);
    }

    #[test]
    fn review_delete_letter_removes_visible_comment() {
        let mut app = test_app();
        app.review_comments.push(line_comment());

        assert!(app.delete_review_comment_letter('a'));
        assert_eq!(app.review_comment_count(), 0);
    }

    #[test]
    fn comment_sidebar_opens_location_not_editor() {
        let mut app = test_app();
        app.review_comments.push(line_comment());
        app.start_line_comment();

        assert!(app.review_editor_active());
        assert!(app.open_review_comment(0));
        assert!(!app.review_editor_active());
    }

    #[test]
    fn comment_sidebar_click_flashes_review_card() {
        let mut app = test_app();
        let comment = line_comment();
        let key = comment.anchor.anchor_key.clone();
        app.review_comments.push(comment);

        assert!(app.open_review_comment(0));
        assert!(app.review_preview_flash_active(&key));
    }

    #[test]
    fn stale_preview_boxes_do_not_handle_sidebar_clicks() {
        let mut app = test_app();
        let comment = line_comment();
        let key = comment.anchor.anchor_key.clone();
        app.review_comments.push(comment);
        app.diff_view_area = Some((10, 0, 30, 5));
        app.add_review_preview_box(0, 0, 8, 1, key);

        assert!(!app.handle_review_preview_click(1, 0));
        assert!(!app.review_editor_active());
    }

    #[test]
    fn comment_sidebar_groups_recent_comments_by_hour() {
        let mut app = test_app();
        let mut comment = line_comment();
        comment.updated_at = now_ts().saturating_sub(5 * 60 * 60 + 30);
        app.review_comments.push(comment);

        assert_eq!(
            app.review_comment_sidebar_bucket(0).as_deref(),
            Some("5hr ago")
        );
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
            author: Some(ReviewAuthor {
                name: "Reviewer".to_string(),
                email: Some("reviewer@example.com".to_string()),
                author_type: None,
                usernames: BTreeMap::new(),
                avatar_url: None,
            }),
            can_edit: true,
            deleted: false,
            provider: None,
            created_at: 1,
            updated_at: 1,
        });

        let json = app.review_export_json(ReviewHookEvent::ReviewReady, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["event"], "review_ready");
        assert_eq!(value["diff"]["branch"], "branch");
        assert_eq!(value["review"]["comments"][0]["file"], "new.txt");
        assert_eq!(value["review"]["comments"][0]["body"], "please fix");
        assert_eq!(value["review"]["comments"][0]["author"]["name"], "Reviewer");
    }

    #[test]
    fn binary_file_comments_use_file_anchor() {
        let diff = MultiFileDiff::from_file_pair_bytes(
            std::path::PathBuf::from("old.bin"),
            vec![0, 1],
            vec![0, 2],
        );
        let mut app = App::new(diff, ViewMode::Preview, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();

        assert!(app.start_file_comment());
        app.review_insert_char('x');
        app.review_save_editor();

        assert_eq!(app.review_comments.len(), 1);
        assert_eq!(app.review_comments[0].anchor.kind, ReviewTargetKind::File);
        assert_eq!(app.review_comments[0].anchor.anchor_key, "file|old.bin");

        let json = app.review_export_json(ReviewHookEvent::ReviewReady, None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["review"]["comments"][0]["kind"], "file");
    }

    #[test]
    fn review_files_are_written_and_loaded() {
        let base = temp_path("files");
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_base_dir(Some(base.clone()));
        app.set_review_author(Some(ReviewAuthor {
            name: "Reviewer".to_string(),
            email: Some("reviewer@example.com".to_string()),
            author_type: None,
            usernames: BTreeMap::new(),
            avatar_url: None,
        }));
        app.enable_review_mode();
        let id = app
            .add_review_comment_from_cli(
                "new.txt",
                ReviewTargetKind::Line,
                Some(ReviewSide::New),
                None,
                Some(ReviewRange { start: 1, end: 1 }),
                "check this".to_string(),
            )
            .unwrap();
        assert_eq!(id, 1);
        let paths = app.review_paths();
        assert!(paths.db_file.unwrap().exists());
        let public_comments: serde_json::Value =
            serde_json::from_str(&app.review_comments_json()).unwrap();
        assert_eq!(public_comments["comments"][0]["author"]["name"], "Reviewer");

        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut loaded = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        loaded.set_review_base_dir(Some(base.clone()));
        loaded.load_review_mode();
        assert_eq!(loaded.review_comment_count(), 1);
        assert!(loaded.review_markdown().contains("check this"));

        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut external = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        external.set_review_base_dir(Some(base.clone()));
        external.load_review_mode();
        external
            .add_review_comment_from_cli(
                "new.txt",
                ReviewTargetKind::Line,
                Some(ReviewSide::New),
                None,
                Some(ReviewRange { start: 1, end: 1 }),
                "external note".to_string(),
            )
            .unwrap();

        loaded.last_review_db_check = Instant::now() - Duration::from_secs(2);
        assert!(loaded.maybe_watch_reload_review_state());
        assert_eq!(loaded.review_comment_count(), 2);
        assert!(loaded.review_markdown().contains("external note"));

        assert!(loaded.abandon_review_from_cli());
        assert_eq!(loaded.review_comment_count(), 0);

        let _ = std::fs::remove_dir_all(base);
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
