use super::{
    AnimationFrame, App, FileDiskStamp, ReviewCommentContextMenu, ReviewCommentContextMenuAction,
    ReviewEditorToolbarAction, ReviewEditorToolbarHit, ViewMode,
};
use crate::app::utils::copy_to_clipboard;
use crate::config::{
    MentionFileScope, MentionFinder, ReviewActionConfig, ReviewHookConfig, ReviewHookEvent,
    ReviewHookStdin,
};
use crate::toasts::ToastEvent;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use crossterm::event::KeyEvent;
use keymap::{parser::parse_seq, ToKeyMap};
use oyo_core::{ChangeKind, LineKind, ViewLine};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use std::collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
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

pub(crate) enum OutdatedReconstructionState {
    Pending,
    Ready(Box<oyo_core::MultiFileDiff>),
    Failed,
}

pub(crate) struct OutdatedReconstructionRequest {
    pub(crate) comment: ReviewComment,
    pub(crate) repo_root: PathBuf,
}

pub(crate) struct OutdatedReconstructionResponse {
    pub(crate) comment_id: u64,
    pub(crate) diff: Option<oyo_core::MultiFileDiff>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewAnchorSnapshotTarget {
    pub(crate) vcs: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jj_change_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jj_commit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) git_base_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) git_head_commit: Option<String>,
}

const CAPTURED_FILE_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapturedFile {
    pub(crate) data: String,
    pub(crate) orig_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewAnchorSnapshot {
    pub(crate) side: String,
    pub(crate) line_number: usize,
    pub(crate) line_text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) context_before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) context_after: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<ReviewAnchorSnapshotTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) old_file: Option<CapturedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) new_file: Option<CapturedFile>,
}

fn capture_file(content: &str) -> Option<CapturedFile> {
    if content.len() > CAPTURED_FILE_MAX_BYTES {
        return None;
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(content.as_bytes()).ok()?;
    let compressed = encoder.finish().ok()?;
    Some(CapturedFile {
        data: BASE64.encode(compressed),
        orig_len: content.len(),
    })
}

fn decode_captured_file(captured: &CapturedFile) -> Option<String> {
    if captured.orig_len > CAPTURED_FILE_MAX_BYTES {
        return None;
    }
    let compressed = BASE64.decode(&captured.data).ok()?;
    let decoder = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut bytes = Vec::with_capacity(captured.orig_len);
    decoder
        .take((CAPTURED_FILE_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() != captured.orig_len {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) snapshot: Option<ReviewAnchorSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewAuthor {
    pub(crate) name: String,
    pub(crate) email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) author_type: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) usernames: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewProviderComment {
    pub(crate) provider: String,
    pub(crate) remote: String,
    pub(crate) repo: String,
    pub(crate) pr_number: u64,
    pub(crate) comment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) in_reply_to_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) thread_resolved: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) resolved_dirty: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) author_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pr_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pr_url: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ReviewThreadKey {
    Provider(String, String, u64, String),
    Local(u64),
}

fn provider_thread_key(provider: &ReviewProviderComment) -> Option<ReviewThreadKey> {
    if provider.api_kind != "review" {
        return None;
    }
    Some(ReviewThreadKey::Provider(
        provider.provider.clone(),
        provider.repo.clone(),
        provider.pr_number,
        provider.thread_id.clone()?,
    ))
}

fn local_thread_root_id(comment: &ReviewComment, comments: &[ReviewComment]) -> u64 {
    let mut root_id = comment.in_reply_to.unwrap_or(comment.id);
    for _ in 0..comments.len() {
        let Some(parent) = comments.iter().find(|candidate| candidate.id == root_id) else {
            break;
        };
        let Some(parent_id) = parent.in_reply_to else {
            return parent.id;
        };
        root_id = parent_id;
    }
    root_id
}

fn review_thread_key(
    comment: &ReviewComment,
    comments: &[ReviewComment],
) -> Option<ReviewThreadKey> {
    if let Some(provider) = comment.provider.as_ref() {
        if let Some(key) = provider_thread_key(provider) {
            return Some(key);
        }
        return comments
            .iter()
            .any(|candidate| {
                candidate.in_reply_to.is_some()
                    && local_thread_root_id(candidate, comments) == comment.id
            })
            .then_some(ReviewThreadKey::Local(comment.id));
    }

    let root_id = local_thread_root_id(comment, comments);
    comments
        .iter()
        .find(|candidate| candidate.id == root_id)
        .and_then(|root| root.provider.as_ref())
        .and_then(provider_thread_key)
        .or(Some(ReviewThreadKey::Local(root_id)))
}

fn review_comment_is_reply(comment: &ReviewComment) -> bool {
    comment.in_reply_to.is_some()
        || comment
            .provider
            .as_ref()
            .and_then(|provider| provider.in_reply_to_id.as_ref())
            .is_some()
}

fn review_comments_share_origin(left: &ReviewComment, right: &ReviewComment) -> bool {
    let same_author = match (&left.author, &right.author) {
        (Some(left), Some(right)) => {
            same_review_author(left, right) || same_review_author(right, left)
        }
        (None, None) => true,
        _ => false,
    };
    left.created_at == right.created_at
        && same_author
        && left.body == right.body
        && left.anchor.kind == right.anchor.kind
        && left.anchor.file_path == right.anchor.file_path
        && left.anchor.side == right.anchor.side
}

fn review_comment_can_reply(comment: &ReviewComment) -> bool {
    !comment.deleted
        && !comment.outdated
        && matches!(
            comment.anchor.kind,
            ReviewTargetKind::Line | ReviewTargetKind::Hunk
        )
        && comment.provider.as_ref().is_none_or(|provider| {
            matches!(provider.provider.as_str(), "github" | "gitlab" | "forgejo")
                && provider.api_kind == "review"
                && provider.thread_id.is_some()
                && (!provider.comment_id.is_empty() || provider.in_reply_to_id.is_some())
                && provider.sync_state != "deleted"
        })
}

fn is_false(value: &bool) -> bool {
    !*value
}

const REVIEW_ANCHOR_CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewComment {
    pub(crate) id: u64,
    pub(crate) anchor: ReviewAnchor,
    pub(crate) body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) author: Option<ReviewAuthor>,
    #[serde(default = "review_comment_can_edit_default")]
    pub(crate) can_edit: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) resolved: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) outdated: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) reanchored: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<ReviewProviderComment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) in_reply_to: Option<u64>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewEditorState {
    pub(crate) anchor: ReviewAnchor,
    pub(crate) text: String,
    pub(crate) cursor: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply: Option<ReviewReplyDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewReplyDraft {
    pub(crate) provider: Option<ReviewProviderComment>,
    pub(crate) in_reply_to: Option<u64>,
    pub(crate) resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewPullRequestTarget {
    pub(crate) provider: String,
    pub(crate) remote: String,
    pub(crate) repo: String,
    pub(crate) number: u64,
    pub(crate) title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewTargetMetadata {
    pub(crate) label: String,
    pub(crate) vcs: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jj_change_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jj_change_ids: Option<Vec<String>>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewGcEntry {
    pub(crate) review_key: String,
    pub(crate) id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewGcResult {
    pub(crate) reaped: Vec<ReviewGcEntry>,
    pub(crate) estimated_bytes: u64,
    pub(crate) bytes_before: u64,
    pub(crate) bytes_after: u64,
    pub(crate) held_pending_sync: usize,
    pub(crate) held_within_grace: usize,
    pub(crate) dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewGcDecision {
    Reap,
    Keep,
    KeepPendingSync,
    KeepWithinGrace,
}

fn review_gc_decision(
    json: &str,
    row_updated_at: i64,
    now: u64,
    grace_seconds: u64,
    prune_now: bool,
) -> ReviewGcDecision {
    let Ok(comment) = serde_json::from_str::<ReviewComment>(json) else {
        return ReviewGcDecision::Keep;
    };
    if !comment.deleted || row_updated_at < 0 {
        return ReviewGcDecision::Keep;
    }
    if comment
        .provider
        .as_ref()
        .is_some_and(|provider| provider.sync_state == "deleted")
    {
        return ReviewGcDecision::KeepPendingSync;
    }
    if prune_now {
        return ReviewGcDecision::Reap;
    }
    let updated_at = (row_updated_at as u64).max(comment.updated_at);
    if now.saturating_sub(updated_at) >= grace_seconds && updated_at <= now {
        ReviewGcDecision::Reap
    } else {
        ReviewGcDecision::KeepWithinGrace
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicReviewComments {
    version: u32,
    comments: Vec<PublicReviewComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicReviewComment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    change_type: Option<String>,
    file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    old_range: Option<ReviewRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_range: Option<ReviewRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hunk_id: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anchor_snapshot: Option<ReviewAnchorSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<ReviewAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    can_edit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<ReviewProviderComment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    in_reply_to: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<u64>,
    #[serde(default)]
    resolved: bool,
    #[serde(default)]
    outdated: bool,
    #[serde(default)]
    reanchored: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    deleted: bool,
    body: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReviewCommentFilter {
    pub(crate) unresolved: bool,
    pub(crate) outdated: Option<bool>,
    pub(crate) author: Option<String>,
    pub(crate) author_type: Option<String>,
    pub(crate) since: Option<u64>,
    pub(crate) ids: Vec<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewStatusComment {
    pub(crate) id: u64,
    pub(crate) subject: String,
    pub(crate) location: String,
    pub(crate) preview: String,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
    pub(crate) resolved: bool,
    pub(crate) outdated: bool,
    pub(crate) deleted: bool,
}

impl ReviewCommentFilter {
    fn matches(&self, comment: &ReviewComment) -> bool {
        if !self.ids.is_empty() && !self.ids.contains(&comment.id) {
            return false;
        }
        if self.unresolved && comment.resolved {
            return false;
        }
        if self.unresolved && self.outdated != Some(true) && comment.outdated {
            return false;
        }
        if self
            .outdated
            .is_some_and(|outdated| comment.outdated != outdated)
        {
            return false;
        }
        if self.since.is_some_and(|since| comment.updated_at < since) {
            return false;
        }
        if let Some(expected) = self.author.as_deref().map(normalize_filter_value) {
            let Some(author) = comment.author.as_ref() else {
                return false;
            };
            let matches_name = normalize_filter_value(&author.name) == expected;
            let matches_email = author
                .email
                .as_deref()
                .map(normalize_filter_value)
                .is_some_and(|value| value == expected);
            let matches_username = author
                .usernames
                .values()
                .any(|value| normalize_filter_value(value) == expected);
            if !(matches_name || matches_email || matches_username) {
                return false;
            }
        }
        if let Some(expected) = self.author_type.as_deref().map(normalize_filter_value) {
            let actual = comment
                .author
                .as_ref()
                .and_then(|author| author.author_type.as_deref())
                .map(normalize_filter_value);
            if actual.as_deref() != Some(expected.as_str()) {
                return false;
            }
        }
        true
    }
}

fn normalize_filter_value(value: &str) -> String {
    value.trim().trim_start_matches('@').to_ascii_lowercase()
}

fn include_review_comment(comment: &ReviewComment, filter: &ReviewCommentFilter) -> bool {
    (filter.since.is_some() || !comment.deleted) && filter.matches(comment)
}

fn review_comment_change_type(comment: &ReviewComment, since: Option<u64>) -> Option<String> {
    since?;
    Some(
        if comment.deleted {
            "removed"
        } else if comment.created_at == comment.updated_at {
            "added"
        } else {
            "updated"
        }
        .to_string(),
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
struct ReviewExportDiff {
    branch: Option<String>,
    range: Option<(String, String)>,
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReviewExportBody<'a> {
    text: &'a str,
    comments: Vec<ReviewExportComment<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReviewExportComment<'a> {
    id: u64,
    file: &'a str,
    kind: &'static str,
    side: Option<&'static str>,
    old_range: Option<ReviewRange>,
    new_range: Option<ReviewRange>,
    author: Option<&'a ReviewAuthor>,
    resolved: bool,
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
    pub(crate) id: u64,
    pub(crate) display_idx: usize,
    pub(crate) preview: String,
    pub(crate) body: String,
    pub(crate) title: String,
    pub(crate) avatar_url: Option<String>,
    pub(crate) avatar_seed: String,
    pub(crate) anchor_key: String,
    pub(crate) edit_label: Option<String>,
    pub(crate) reply_label: Option<String>,
    pub(crate) resolve_label: Option<String>,
    pub(crate) delete_label: Option<String>,
    pub(crate) overflow_label: Option<String>,
    pub(crate) thread_continues: bool,
    pub(crate) prefer_right: bool,
    pub(crate) is_hunk: bool,
    pub(crate) can_edit: bool,
    pub(crate) resolved: bool,
    pub(crate) outdated: bool,
    pub(crate) syntax_path: Option<String>,
    pub(crate) snapshot_code: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct OutdatedCommentOverlay {
    pub(crate) id: u64,
    pub(crate) overlay: ReviewCommentOverlay,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewPreviewBox {
    pub(crate) comment_id: Option<u64>,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) anchor_key: String,
    pub(crate) edit: bool,
    pub(crate) reply: bool,
    pub(crate) resolve: bool,
    pub(crate) delete: bool,
    pub(crate) overflow: bool,
    pub(crate) passive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewDeleteTarget {
    All,
    DiscardSession,
    Comment { id: u64, reply_count: usize },
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

pub(crate) fn review_index_action_label(prefix: &str, idx: usize) -> String {
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

fn bumped_ts(previous: u64) -> u64 {
    now_ts().max(previous.saturating_add(1))
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

fn review_anchor_start_location_label(anchor: &ReviewAnchor) -> Option<String> {
    match anchor.kind {
        ReviewTargetKind::File | ReviewTargetKind::PullRequest => None,
        ReviewTargetKind::Line => Some(review_anchor_location_label(anchor)),
        ReviewTargetKind::Hunk => anchor
            .new_range
            .map(|range| {
                review_side_label(
                    ReviewSide::New,
                    Some(ReviewRange {
                        start: range.start,
                        end: range.start,
                    }),
                )
            })
            .or_else(|| {
                anchor.old_range.map(|range| {
                    review_side_label(
                        ReviewSide::Old,
                        Some(ReviewRange {
                            start: range.start,
                            end: range.start,
                        }),
                    )
                })
            }),
    }
}

fn collapse_home_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.to_string_lossy());
        }
    }
    path.to_string_lossy().to_string()
}

fn quote_markdown_body(body: &str) -> String {
    body.lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn provider_comment_url(provider: &ReviewProviderComment) -> Option<String> {
    provider.pr_url.clone().filter(|url| !url.trim().is_empty())
}

fn line_with_context(
    content: &str,
    line_number: usize,
) -> Option<(String, Vec<String>, Vec<String>)> {
    let idx = line_number.checked_sub(1)?;
    let lines = content.lines().collect::<Vec<_>>();
    let line_text = lines.get(idx)?.to_string();
    let before_start = idx.saturating_sub(REVIEW_ANCHOR_CONTEXT_LINES);
    let after_end = (idx + 1 + REVIEW_ANCHOR_CONTEXT_LINES).min(lines.len());
    let context_before = lines[before_start..idx]
        .iter()
        .map(|line| (*line).to_string())
        .collect();
    let context_after = lines[idx + 1..after_end]
        .iter()
        .map(|line| (*line).to_string())
        .collect();
    Some((line_text, context_before, context_after))
}

fn review_anchor_snapshot_position(anchor: &ReviewAnchor) -> Option<(ReviewSide, usize)> {
    match anchor.side {
        Some(ReviewSide::Old) => anchor.old_range.map(|range| (ReviewSide::Old, range.start)),
        Some(ReviewSide::New) => anchor.new_range.map(|range| (ReviewSide::New, range.start)),
        None => anchor
            .new_range
            .map(|range| (ReviewSide::New, range.start))
            .or_else(|| anchor.old_range.map(|range| (ReviewSide::Old, range.start))),
    }
}

fn review_anchor_snapshot_target(
    metadata: Option<&ReviewTargetMetadata>,
) -> Option<ReviewAnchorSnapshotTarget> {
    let metadata = metadata?;
    if metadata.jj_change_id.is_none()
        && metadata.jj_commit_id.is_none()
        && metadata.git_base_commit.is_none()
        && metadata.git_head_commit.is_none()
    {
        return None;
    }
    Some(ReviewAnchorSnapshotTarget {
        vcs: metadata.vcs.clone(),
        jj_change_id: metadata.jj_change_id.clone(),
        jj_commit_id: metadata.jj_commit_id.clone(),
        git_base_commit: metadata.git_base_commit.clone(),
        git_head_commit: metadata.git_head_commit.clone(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReviewAnchorCandidate {
    line_number: usize,
    score: usize,
}

fn snapshot_review_side(snapshot: &ReviewAnchorSnapshot) -> Option<ReviewSide> {
    match snapshot.side.as_str() {
        "old" => Some(ReviewSide::Old),
        "new" => Some(ReviewSide::New),
        _ => None,
    }
}

fn anchor_line_number(anchor: &ReviewAnchor, side: ReviewSide) -> Option<usize> {
    match side {
        ReviewSide::Old => anchor.old_range,
        ReviewSide::New => anchor.new_range,
    }
    .map(|range| range.start)
}

fn anchor_text_matches(left: &str, right: &str) -> bool {
    left.trim_end() == right.trim_end()
}

fn snapshot_context_score(lines: &[&str], idx: usize, snapshot: &ReviewAnchorSnapshot) -> usize {
    let mut score = 0;
    for (offset, expected) in snapshot.context_before.iter().rev().enumerate() {
        let Some(line_idx) = idx.checked_sub(offset + 1) else {
            continue;
        };
        if lines
            .get(line_idx)
            .is_some_and(|actual| anchor_text_matches(actual, expected))
        {
            score += 1;
        }
    }
    for (offset, expected) in snapshot.context_after.iter().enumerate() {
        if lines
            .get(idx + offset + 1)
            .is_some_and(|actual| anchor_text_matches(actual, expected))
        {
            score += 1;
        }
    }
    score
}

fn best_snapshot_line_match(
    content: &str,
    snapshot: &ReviewAnchorSnapshot,
    preferred_line: Option<usize>,
) -> Option<ReviewAnchorCandidate> {
    let lines = content.lines().collect::<Vec<_>>();
    let target = snapshot.line_text.trim_end();
    let mut best: Option<ReviewAnchorCandidate> = None;
    for (idx, line) in lines.iter().enumerate() {
        if !anchor_text_matches(line, target) {
            continue;
        }
        let candidate = ReviewAnchorCandidate {
            line_number: idx + 1,
            score: snapshot_context_score(&lines, idx, snapshot),
        };
        let replace = best.is_none_or(|current| {
            candidate.score > current.score
                || (candidate.score == current.score
                    && Some(candidate.line_number) == preferred_line
                    && Some(current.line_number) != preferred_line)
        });
        if replace {
            best = Some(candidate);
        }
    }
    best
}

fn shifted_range(range: Option<ReviewRange>, new_start: usize) -> ReviewRange {
    let len = range
        .map(|range| range.end.saturating_sub(range.start))
        .unwrap_or(0);
    ReviewRange {
        start: new_start,
        end: new_start.saturating_add(len),
    }
}

fn provider_anchor_suffix(provider: Option<&ReviewProviderComment>) -> String {
    provider
        .map(|provider| {
            format!(
                "|provider|{}|{}|{}",
                provider.provider, provider.repo, provider.comment_id
            )
        })
        .unwrap_or_default()
}

fn rebuild_review_anchor_key(
    anchor: &ReviewAnchor,
    provider: Option<&ReviewProviderComment>,
) -> String {
    let base = match anchor.kind {
        ReviewTargetKind::PullRequest => "pr".to_string(),
        ReviewTargetKind::File => format!("file|{}", anchor.file_path),
        ReviewTargetKind::Line => match anchor.side {
            Some(side) => {
                let line_no = anchor_line_number(anchor, side).unwrap_or(0);
                format!("line|{}|{}|{}", anchor.file_path, side.as_str(), line_no)
            }
            None => format!(
                "line|{}|both|{}|{}",
                anchor.file_path,
                format_opt_range(anchor.old_range),
                format_opt_range(anchor.new_range)
            ),
        },
        ReviewTargetKind::Hunk => format!(
            "hunk|{}|{}|{}",
            anchor.file_path,
            format_opt_range(anchor.old_range),
            format_opt_range(anchor.new_range)
        ),
    };
    format!("{}{}", base, provider_anchor_suffix(provider))
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

fn review_comment_is_inline_visible(
    comment: &ReviewComment,
    file_path: &str,
    reconstructed_comment_id: Option<u64>,
    comments: &[ReviewComment],
) -> bool {
    let reconstructed_thread = reconstructed_comment_id
        .is_some_and(|id| comment.id == id || local_thread_root_id(comment, comments) == id);
    !comment.deleted
        && reconstructed_comment_id.is_none_or(|_| reconstructed_thread)
        && (!comment.outdated || reconstructed_thread)
        && comment.anchor.kind != ReviewTargetKind::PullRequest
        && comment.anchor.file_path == file_path
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

fn review_comment_kind_rank(kind: ReviewTargetKind) -> u8 {
    match kind {
        ReviewTargetKind::PullRequest => 0,
        ReviewTargetKind::File => 1,
        ReviewTargetKind::Line => 2,
        ReviewTargetKind::Hunk => 3,
    }
}

fn review_comment_document_cmp(left: &ReviewComment, right: &ReviewComment) -> std::cmp::Ordering {
    let line = |comment: &ReviewComment| {
        comment
            .anchor
            .new_range
            .or(comment.anchor.old_range)
            .map(|range| range.start)
            .unwrap_or(usize::MAX)
    };
    left.anchor
        .file_path
        .cmp(&right.anchor.file_path)
        .then_with(|| {
            review_comment_kind_rank(left.anchor.kind)
                .cmp(&review_comment_kind_rank(right.anchor.kind))
        })
        .then_with(|| line(left).cmp(&line(right)))
        .then_with(|| left.id.cmp(&right.id))
}

fn review_comment_subject(comment: &ReviewComment) -> String {
    match comment.anchor.kind {
        ReviewTargetKind::PullRequest => comment
            .provider
            .as_ref()
            .and_then(|provider| provider.pr_title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| {
                comment
                    .provider
                    .as_ref()
                    .and_then(|provider| crate::ReviewProviderKind::from_id(&provider.provider))
                    .map(crate::ReviewProviderKind::long_review_noun_title)
                    .unwrap_or("Pull request")
                    .to_string()
            }),
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
        snapshot: None,
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
        snapshot: None,
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
        if self.review_target_metadata != metadata {
            self.review_target_metadata = metadata;
            self.persist_review_session();
        }
    }

    pub(crate) fn review_target_metadata(&self) -> Option<&ReviewTargetMetadata> {
        self.review_target_metadata.as_ref()
    }

    pub(crate) fn update_jj_review_target_ids(&mut self, change_id: String, commit_id: String) {
        let Some(mut metadata) = self.review_target_metadata.clone() else {
            return;
        };
        if metadata.vcs != "jj"
            || (metadata.jj_change_id.as_deref() == Some(change_id.as_str())
                && metadata.jj_commit_id.as_deref() == Some(commit_id.as_str()))
        {
            return;
        }
        metadata.jj_change_id = Some(change_id);
        metadata.jj_commit_id = Some(commit_id);
        self.set_review_target_metadata(Some(metadata));
    }

    pub(crate) fn refresh_git_review_target_commits(&mut self, repo_root: &Path) {
        let Some(mut metadata) = self.review_target_metadata.clone() else {
            return;
        };
        if metadata.vcs != "git" {
            return;
        }
        if let Some(base) = metadata.git_base_ref.as_deref() {
            if let Some(commit) = crate::git_commit(repo_root, base) {
                metadata.git_base_commit = Some(commit);
            }
        }
        if let Some(head) = metadata.git_head_ref.as_deref() {
            if let Some(commit) = crate::git_commit(repo_root, head) {
                metadata.git_head_commit = Some(commit);
            }
        }
        self.set_review_target_metadata(Some(metadata));
    }

    fn fill_review_anchor_snapshot(&self, anchor: &mut ReviewAnchor) {
        if anchor.snapshot.is_none() {
            anchor.snapshot = self.review_anchor_snapshot(anchor);
        }
    }

    fn review_anchor_snapshot(&self, anchor: &ReviewAnchor) -> Option<ReviewAnchorSnapshot> {
        let (side, line_number) = review_anchor_snapshot_position(anchor)?;
        let (old_content, new_content) = self.multi_diff.file_contents(anchor.file_index)?;
        let content = match side {
            ReviewSide::Old => old_content,
            ReviewSide::New => new_content,
        };
        let (line_text, context_before, context_after) = line_with_context(content, line_number)?;
        Some(ReviewAnchorSnapshot {
            side: side.as_str().to_string(),
            line_number,
            line_text,
            context_before,
            context_after,
            target: review_anchor_snapshot_target(self.review_target_metadata.as_ref()),
            old_file: capture_file(old_content),
            new_file: capture_file(new_content),
        })
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

    #[cfg(test)]
    pub(crate) fn review_markdown_filtered(&self, filter: &ReviewCommentFilter) -> String {
        self.format_review_output_filtered(filter)
    }

    pub(crate) fn review_markdown_filtered_colored(
        &self,
        filter: &ReviewCommentFilter,
        color: bool,
    ) -> String {
        self.format_review_output_filtered_colored(filter, color)
    }

    pub fn review_comments_json(&self) -> String {
        self.public_review_comments_json()
    }

    pub(crate) fn review_comments_json_filtered(&self, filter: &ReviewCommentFilter) -> String {
        self.public_review_comments_json_filtered(filter)
    }

    pub fn review_workspace_root(&self) -> Option<&str> {
        self.review_repo_root.as_deref()
    }

    pub fn review_diff_fingerprint(&self) -> &str {
        &self.review_diff_fingerprint
    }

    pub fn review_storage_key(&self) -> &str {
        self.review_db_key()
    }

    pub(crate) fn control_target_label(&self) -> String {
        self.review_target_metadata
            .as_ref()
            .map(|metadata| metadata.label.clone())
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| "current target".to_string())
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

    pub(crate) fn review_has_stored_comments(&self) -> bool {
        !self.review_comments.is_empty()
    }

    pub(crate) fn review_comment_count_for_file(&self, file_index: usize) -> usize {
        self.review_comments
            .iter()
            .filter(|comment| !comment.deleted && comment.anchor.file_index == file_index)
            .count()
    }

    pub(crate) fn missing_review_filter_id(&self, filter: &ReviewCommentFilter) -> Option<u64> {
        filter.ids.iter().copied().find(|id| {
            !self
                .review_comments
                .iter()
                .any(|comment| comment.id == *id && include_review_comment(comment, filter))
        })
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

    pub(crate) fn review_status_comment_rows_filtered(
        &self,
        filter: &ReviewCommentFilter,
    ) -> Vec<ReviewStatusComment> {
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| include_review_comment(comment, filter))
            .collect::<Vec<_>>();
        comments.sort_by(|left, right| review_comment_document_cmp(left, right));
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
                if comment.deleted {
                    preview = format!("[removed] {preview}");
                } else if comment.outdated {
                    preview = format!("[outdated] {preview}");
                } else if comment.resolved {
                    preview = format!("[resolved] {preview}");
                }
                ReviewStatusComment {
                    id: comment.id,
                    subject,
                    location,
                    preview,
                    created_at: comment.created_at,
                    updated_at: comment.updated_at,
                    resolved: comment.resolved,
                    outdated: comment.outdated,
                    deleted: comment.deleted,
                }
            })
            .collect()
    }

    pub(crate) fn review_comment_sidebar_item(
        &self,
        index: usize,
    ) -> Option<(usize, String, String, String, bool, bool)> {
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
        if comment.outdated {
            preview = format!("Outdated: {preview}");
        }
        Some((
            comment.anchor.file_index,
            title,
            location,
            preview,
            comment.outdated,
            comment.resolved,
        ))
    }

    pub(crate) fn focus_next_review_comment(&mut self) -> bool {
        self.focus_review_comment(true)
    }

    pub(crate) fn focus_prev_review_comment(&mut self) -> bool {
        self.focus_review_comment(false)
    }

    fn focus_review_comment(&mut self, forward: bool) -> bool {
        if !self.review_mode {
            return false;
        }
        let mut order = self
            .review_comments
            .iter()
            .enumerate()
            .filter(|(_, comment)| !comment.deleted)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        order.sort_by(|left, right| {
            review_comment_document_cmp(&self.review_comments[*left], &self.review_comments[*right])
        });
        if order.is_empty() {
            return false;
        }
        let current = self.active_review_comment_id.and_then(|id| {
            order
                .iter()
                .position(|index| self.review_comments[*index].id == id)
        });
        let position = match (current, forward) {
            (Some(position), true) => position.saturating_add(1) % order.len(),
            (Some(0), false) | (None, false) => order.len().saturating_sub(1),
            (Some(position), false) => position.saturating_sub(1),
            (None, true) => 0,
        };
        self.open_review_comment(order[position])
    }

    fn reconstructed_diff_places_anchor(
        comment: &ReviewComment,
        diff: &oyo_core::MultiFileDiff,
    ) -> bool {
        if diff.file_count() != 1 {
            return false;
        }
        let Some(snapshot) = comment.anchor.snapshot.as_ref() else {
            return false;
        };
        let Some(side) = snapshot_review_side(snapshot) else {
            return false;
        };
        let Some((old_content, new_content)) = diff.file_contents(0) else {
            return false;
        };
        let content = match side {
            ReviewSide::Old => old_content,
            ReviewSide::New => new_content,
        };
        let current_line = anchor_line_number(&comment.anchor, side).or(Some(snapshot.line_number));
        best_snapshot_line_match(content, snapshot, current_line).is_some()
    }

    fn reconstruct_captured_outdated_comment_diff(
        comment: &ReviewComment,
    ) -> Option<oyo_core::MultiFileDiff> {
        let snapshot = comment.anchor.snapshot.as_ref()?;
        let old_content = decode_captured_file(snapshot.old_file.as_ref()?)?;
        let new_content = decode_captured_file(snapshot.new_file.as_ref()?)?;
        let path = PathBuf::from(&comment.anchor.file_path);
        let diff =
            oyo_core::MultiFileDiff::from_file_pair(path.clone(), path, old_content, new_content);
        Self::reconstructed_diff_places_anchor(comment, &diff).then_some(diff)
    }

    fn reconstruct_outdated_comment_diff(
        repo_root: &Path,
        comment: &ReviewComment,
    ) -> Option<oyo_core::MultiFileDiff> {
        let target = comment.anchor.snapshot.as_ref()?.target.as_ref()?;
        let repo_root = repo_root.to_path_buf();
        let path = PathBuf::from(&comment.anchor.file_path);
        match target.vcs.as_str() {
            "jj" => {
                let mut seen = FxHashSet::default();
                for revision in [
                    target.jj_commit_id.as_deref(),
                    target.jj_change_id.as_deref(),
                ]
                .into_iter()
                .flatten()
                .map(str::to_string)
                .chain(
                    target
                        .jj_change_id
                        .as_deref()
                        .into_iter()
                        .flat_map(|change_id| {
                            crate::jj_evolog_commit_ids(&repo_root, change_id, 30)
                        }),
                ) {
                    if !seen.insert(revision.clone()) {
                        continue;
                    }
                    let Ok(diff) = crate::build_jj_diff(&repo_root, &revision, Some(&path)) else {
                        continue;
                    };
                    if Self::reconstructed_diff_places_anchor(comment, &diff) {
                        return Some(diff);
                    }
                }
                None
            }
            "git" => {
                let base = target.git_base_commit.as_deref()?;
                let head = target.git_head_commit.as_deref()?;
                let changes = oyo_core::git::get_changes_between(&repo_root, base, head)
                    .ok()?
                    .into_iter()
                    .filter(|file| {
                        file.path == path || file.old_path.as_deref() == Some(path.as_path())
                    })
                    .collect::<Vec<_>>();
                if changes.is_empty() {
                    return None;
                }
                let diff = oyo_core::MultiFileDiff::from_git_range(
                    repo_root,
                    changes,
                    base.to_string(),
                    head.to_string(),
                )
                .ok()?;
                Self::reconstructed_diff_places_anchor(comment, &diff).then_some(diff)
            }
            _ => None,
        }
    }

    fn ensure_outdated_reconstruction_worker(&mut self) {
        if self.outdated_reconstruction_tx.is_some() {
            return;
        }
        let (request_tx, request_rx) = std::sync::mpsc::channel::<OutdatedReconstructionRequest>();
        let (response_tx, response_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let comment_id = request.comment.id;
                let diff =
                    App::reconstruct_outdated_comment_diff(&request.repo_root, &request.comment);
                if response_tx
                    .send(OutdatedReconstructionResponse { comment_id, diff })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.outdated_reconstruction_tx = Some(request_tx);
        self.outdated_reconstruction_rx = Some(response_rx);
    }

    pub(crate) fn enqueue_outdated_reconstruction(&mut self, comment_id: u64) -> bool {
        let comment_id = self
            .review_thread_root_comment_id(comment_id)
            .unwrap_or(comment_id);
        if self.outdated_reconstruction_cache.contains_key(&comment_id) {
            return false;
        }
        let Some(comment) = self
            .review_comments
            .iter()
            .find(|comment| comment.id == comment_id && comment.outdated && !comment.deleted)
            .cloned()
        else {
            return false;
        };
        if let Some(diff) = Self::reconstruct_captured_outdated_comment_diff(&comment) {
            self.outdated_reconstruction_cache.insert(
                comment_id,
                OutdatedReconstructionState::Ready(Box::new(diff)),
            );
            return true;
        }
        let Some(repo_root) = self.multi_diff.repo_root().map(Path::to_path_buf) else {
            self.outdated_reconstruction_cache
                .insert(comment_id, OutdatedReconstructionState::Failed);
            return false;
        };
        self.ensure_outdated_reconstruction_worker();
        let request = OutdatedReconstructionRequest { comment, repo_root };
        if self
            .outdated_reconstruction_tx
            .as_ref()
            .is_some_and(|tx| tx.send(request).is_ok())
        {
            self.outdated_reconstruction_cache
                .insert(comment_id, OutdatedReconstructionState::Pending);
            true
        } else {
            self.outdated_reconstruction_cache
                .insert(comment_id, OutdatedReconstructionState::Failed);
            false
        }
    }

    pub(crate) fn preload_all_outdated_reconstructions(&mut self) {
        for id in self.outdated_comment_ids() {
            self.enqueue_outdated_reconstruction(id);
        }
    }

    pub(crate) fn maybe_preload_hovered_outdated_reconstruction(&mut self) {
        if let Some(id) = self.review_preview_hover_id {
            self.enqueue_outdated_reconstruction(id);
        }
    }

    pub(crate) fn maybe_preload_idle_outdated_reconstruction(&mut self) -> bool {
        if self.last_outdated_reconstruction_idle_enqueue.elapsed() < Duration::from_millis(500) {
            return false;
        }
        let next = self
            .outdated_comment_ids()
            .into_iter()
            .find(|id| !self.outdated_reconstruction_cache.contains_key(id));
        let Some(id) = next else {
            return false;
        };
        self.last_outdated_reconstruction_idle_enqueue = Instant::now();
        self.enqueue_outdated_reconstruction(id)
    }

    pub(crate) fn poll_outdated_reconstruction_responses(&mut self) -> bool {
        let mut responses = Vec::new();
        if let Some(rx) = self.outdated_reconstruction_rx.as_ref() {
            while let Ok(response) = rx.try_recv() {
                responses.push(response);
            }
        }
        let mut dirty = false;
        for response in responses {
            if !matches!(
                self.outdated_reconstruction_cache.get(&response.comment_id),
                Some(OutdatedReconstructionState::Pending)
            ) {
                continue;
            }
            let state = response
                .diff
                .map(|diff| OutdatedReconstructionState::Ready(Box::new(diff)))
                .unwrap_or(OutdatedReconstructionState::Failed);
            self.outdated_reconstruction_cache
                .insert(response.comment_id, state);
            dirty = true;
            if self
                .pending_outdated_reconstruction
                .as_ref()
                .is_some_and(|(id, _)| *id == response.comment_id)
            {
                self.pending_outdated_reconstruction = None;
                let comment = self
                    .review_comments
                    .iter()
                    .find(|comment| comment.id == response.comment_id)
                    .cloned();
                match (
                    comment,
                    self.outdated_reconstruction_cache
                        .remove(&response.comment_id),
                ) {
                    (Some(comment), Some(OutdatedReconstructionState::Ready(diff))) => {
                        self.show_reconstructed_outdated_diff(&comment, *diff);
                    }
                    (Some(comment), _) => {
                        self.outdated_reconstruction_cache
                            .insert(response.comment_id, OutdatedReconstructionState::Failed);
                        self.open_outdated_comments_in_current_tab(Some(comment.id));
                    }
                    _ => {}
                }
            }
        }
        dirty
    }

    pub(crate) fn clear_outdated_reconstruction_cache(&mut self) {
        self.outdated_reconstruction_cache.clear();
        self.pending_outdated_reconstruction = None;
        if let Some(view) = self.outdated_diff_view.as_mut() {
            view.cache_on_restore = false;
        }
    }

    fn show_reconstructed_outdated_diff(
        &mut self,
        comment: &ReviewComment,
        mut reconstructed: oyo_core::MultiFileDiff,
    ) {
        reconstructed.select_file(0);
        let active_tab_id = self.active_topbar_tab;
        let active_tab_content = active_tab_id.and_then(|id| {
            self.topbar_tabs
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| tab.content)
        });
        let live_backup = std::mem::replace(&mut self.multi_diff, reconstructed);
        self.outdated_diff_view = Some(super::OutdatedDiffView {
            comment_id: comment.id,
            file_path: comment.anchor.file_path.clone(),
            live_backup,
            active_tab_id,
            active_tab_content,
            cache_on_restore: true,
        });
        if let Some(id) = active_tab_id {
            if let Some(tab) = self.topbar_tabs.iter_mut().find(|tab| tab.id == id) {
                tab.content = super::TopbarTabContent::File(0);
                tab.navigator_state = None;
            }
        }
        self.reset_after_file_list_refresh(false);
        self.file_list_focused = false;
        self.stop_file_filter();
        self.scroll_to_review_anchor(&comment.anchor);
    }

    pub(crate) fn restore_live_diff_after_outdated_view(&mut self) -> bool {
        let cancelled_pending = self.pending_outdated_reconstruction.take().is_some();
        let Some(view) = self.outdated_diff_view.take() else {
            return cancelled_pending;
        };
        let reconstructed = std::mem::replace(&mut self.multi_diff, view.live_backup);
        if view.cache_on_restore {
            self.outdated_reconstruction_cache.insert(
                view.comment_id,
                OutdatedReconstructionState::Ready(Box::new(reconstructed)),
            );
        }
        if let Some(id) = view.active_tab_id {
            if let Some(tab) = self.topbar_tabs.iter_mut().find(|tab| tab.id == id) {
                if let Some(content) = view.active_tab_content {
                    tab.content = content;
                }
                tab.navigator_state = None;
            }
        }
        self.reset_after_file_list_refresh(false);
        self.start_content_loading();
        true
    }

    pub(crate) fn outdated_live_files(&self) -> Option<&[oyo_core::multi::FileEntry]> {
        self.outdated_diff_view
            .as_ref()
            .map(|view| view.live_backup.files.as_slice())
    }

    pub(crate) fn outdated_live_selected_index(&self) -> Option<usize> {
        self.outdated_diff_view
            .as_ref()
            .map(|view| view.live_backup.selected_index)
    }

    pub(crate) fn outdated_diff_title(&self) -> Option<String> {
        self.outdated_diff_view
            .as_ref()
            .map(|view| {
                debug_assert_eq!(self.active_review_comment_id, Some(view.comment_id));
                format!("Outdated: {}", view.file_path)
            })
            .or_else(|| {
                self.pending_outdated_reconstruction
                    .as_ref()
                    .map(|(_, file)| format!("Outdated: {file}"))
            })
    }

    pub(crate) fn outdated_reconstruction_pending(&self) -> bool {
        self.pending_outdated_reconstruction.is_some()
    }

    pub fn open_review_comment(&mut self, index: usize) -> bool {
        let Some(selected) = self
            .review_comments
            .get(index)
            .filter(|comment| !comment.deleted)
            .cloned()
        else {
            return false;
        };
        let comment = self
            .review_thread_root_comment_id(selected.id)
            .filter(|root_id| {
                selected.outdated || {
                    self.review_comments
                        .iter()
                        .any(|comment| comment.id == *root_id && comment.outdated)
                }
            })
            .and_then(|root_id| {
                self.review_comments
                    .iter()
                    .find(|comment| comment.id == root_id && !comment.deleted)
                    .cloned()
            })
            .unwrap_or(selected);
        let history_origin = self.view_history_origin();
        let was_replaying = self.view_history_replaying;
        self.view_history_replaying = true;
        self.active_review_comment_id = Some(comment.id);
        self.flash_review_preview(comment.id, comment.anchor.anchor_key.clone());
        if self.review_editor_active() {
            self.review_cancel_editor();
        }
        if comment.outdated {
            self.restore_live_diff_after_outdated_view();
            if let Some(diff) = Self::reconstruct_captured_outdated_comment_diff(&comment) {
                self.show_reconstructed_outdated_diff(&comment, diff);
            } else {
                match self.outdated_reconstruction_cache.remove(&comment.id) {
                    Some(OutdatedReconstructionState::Ready(diff)) => {
                        self.show_reconstructed_outdated_diff(&comment, *diff);
                    }
                    Some(OutdatedReconstructionState::Failed) => {
                        self.outdated_reconstruction_cache
                            .insert(comment.id, OutdatedReconstructionState::Failed);
                        self.open_outdated_comments_in_current_tab(Some(comment.id));
                    }
                    Some(OutdatedReconstructionState::Pending) => {
                        self.outdated_reconstruction_cache
                            .insert(comment.id, OutdatedReconstructionState::Pending);
                        self.pending_outdated_reconstruction =
                            Some((comment.id, comment.anchor.file_path.clone()));
                    }
                    None => {
                        self.enqueue_outdated_reconstruction(comment.id);
                        if matches!(
                            self.outdated_reconstruction_cache.get(&comment.id),
                            Some(OutdatedReconstructionState::Failed)
                        ) {
                            self.open_outdated_comments_in_current_tab(Some(comment.id));
                        } else {
                            self.pending_outdated_reconstruction =
                                Some((comment.id, comment.anchor.file_path.clone()));
                        }
                    }
                }
            }
        } else if comment.anchor.kind == ReviewTargetKind::PullRequest {
            self.open_pr_comments_in_current_tab(Some(comment.id));
        } else {
            self.select_file(comment.anchor.file_index);
            self.scroll_to_review_anchor(&comment.anchor);
        }
        self.view_history_replaying = was_replaying;
        self.record_view_landing(
            history_origin,
            super::ViewHistoryRecipe::Comment {
                comment_id: comment.id,
            },
        );
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

    pub(crate) fn active_outdated_comments_view(&self) -> bool {
        self.active_topbar_content() == Some(super::TopbarTabContent::OutdatedComments)
    }

    pub(crate) fn outdated_comment_ids(&self) -> Vec<u64> {
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| {
                comment.outdated && !comment.deleted && !review_comment_is_reply(comment)
            })
            .collect::<Vec<_>>();
        comments.sort_by_key(|comment| (comment.created_at, comment.id));
        comments.into_iter().map(|comment| comment.id).collect()
    }

    pub(crate) fn outdated_comment_overlays(&self) -> Vec<OutdatedCommentOverlay> {
        self.outdated_comment_ids()
            .into_iter()
            .enumerate()
            .flat_map(|(idx, id)| {
                let Some(comment) = self.review_comments.iter().find(|comment| comment.id == id)
                else {
                    return Vec::new();
                };
                let line = comment
                    .anchor
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.line_number)
                    .or_else(|| comment.anchor.new_range.map(|range| range.start))
                    .or_else(|| comment.anchor.old_range.map(|range| range.start));
                let location = line
                    .map(|line| format!("{}:{line}", comment.anchor.file_path))
                    .unwrap_or_else(|| comment.anchor.file_path.clone());
                let snapshot = comment.anchor.snapshot.as_ref().map(|snapshot| {
                    let mut lines = snapshot
                        .context_before
                        .iter()
                        .map(|line| format!("  {line}"))
                        .collect::<Vec<_>>();
                    lines.push(format!("→ {}", snapshot.line_text));
                    lines.extend(
                        snapshot
                            .context_after
                            .iter()
                            .map(|line| format!("  {line}")),
                    );
                    lines.join("\n")
                });
                let snapshot = snapshot.filter(|snapshot| !snapshot.is_empty());
                let mut body = comment.body.clone();
                body.push_str(&format!("\n\nSnapshot `{location}`\n"));
                if let Some(snapshot) = snapshot.as_deref() {
                    body.push_str("````text\n");
                    body.push_str(snapshot);
                    body.push_str("\n````");
                } else {
                    body.push_str("(unavailable)");
                }

                let mut replies = self
                    .review_comments
                    .iter()
                    .filter(|reply| {
                        !reply.deleted
                            && reply.outdated
                            && review_comment_is_reply(reply)
                            && self.review_thread_root_comment_id(reply.id) == Some(comment.id)
                    })
                    .collect::<Vec<_>>();
                replies.sort_by_key(|reply| (reply.created_at, reply.id));
                let mut overlays = vec![OutdatedCommentOverlay {
                    id,
                    overlay: ReviewCommentOverlay {
                        id: comment.id,
                        display_idx: 0,
                        preview: comment.body.lines().next().unwrap_or_default().to_string(),
                        body,
                        title: format!(
                            "{}  Outdated",
                            review_comment_title(comment, self.review_author.as_ref())
                        ),
                        avatar_url: comment
                            .author
                            .as_ref()
                            .and_then(|author| author.avatar_url.clone()),
                        avatar_seed: review_author_avatar_seed(comment.author.as_ref()),
                        anchor_key: comment.anchor.anchor_key.clone(),
                        edit_label: comment
                            .can_edit
                            .then(|| review_index_action_label("i", idx)),
                        reply_label: None,
                        resolve_label: Some(review_index_action_label("v", idx)),
                        delete_label: comment
                            .can_edit
                            .then(|| review_index_action_label("x", idx)),
                        overflow_label: None,
                        thread_continues: !replies.is_empty(),
                        prefer_right: true,
                        is_hunk: comment.anchor.kind == ReviewTargetKind::Hunk,
                        can_edit: comment.can_edit,
                        resolved: comment.resolved,
                        outdated: true,
                        syntax_path: Some(comment.anchor.file_path.clone()),
                        snapshot_code: snapshot,
                    },
                }];
                let reply_count = replies.len();
                overlays.extend(replies.into_iter().enumerate().map(|(reply_idx, reply)| {
                    OutdatedCommentOverlay {
                        id: reply.id,
                        overlay: ReviewCommentOverlay {
                            id: reply.id,
                            display_idx: 0,
                            preview: reply.body.lines().next().unwrap_or_default().to_string(),
                            body: reply.body.clone(),
                            title: review_comment_title(reply, self.review_author.as_ref()),
                            avatar_url: reply
                                .author
                                .as_ref()
                                .and_then(|author| author.avatar_url.clone()),
                            avatar_seed: review_author_avatar_seed(reply.author.as_ref()),
                            anchor_key: comment.anchor.anchor_key.clone(),
                            edit_label: None,
                            reply_label: None,
                            resolve_label: None,
                            delete_label: None,
                            overflow_label: None,
                            thread_continues: reply_idx + 1 < reply_count,
                            prefer_right: true,
                            is_hunk: comment.anchor.kind == ReviewTargetKind::Hunk,
                            can_edit: reply.can_edit,
                            resolved: reply.resolved,
                            outdated: true,
                            syntax_path: Some(comment.anchor.file_path.clone()),
                            snapshot_code: None,
                        },
                    }
                }));
                overlays
            })
            .collect()
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
                        id: comment.id,
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
                        reply_label: None,
                        resolve_label: None,
                        delete_label: None,
                        overflow_label: None,
                        thread_continues: false,
                        prefer_right: true,
                        is_hunk: false,
                        can_edit: comment.can_edit,
                        resolved: comment.resolved,
                        outdated: false,
                        syntax_path: None,
                        snapshot_code: None,
                    },
                )
            })
            .collect()
    }

    fn pull_request_replyable_comment_ids(&self) -> Vec<u64> {
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| {
                !comment.deleted
                    && comment.anchor.kind == ReviewTargetKind::PullRequest
                    && comment
                        .provider
                        .as_ref()
                        .is_none_or(|provider| provider.api_kind != "review_submission")
            })
            .collect::<Vec<_>>();
        comments.sort_by_key(|comment| (comment.created_at, comment.id));
        comments.into_iter().map(|comment| comment.id).collect()
    }

    pub(crate) fn pull_request_comment_can_reply(&self, id: u64) -> bool {
        self.pull_request_replyable_comment_ids().contains(&id)
    }

    pub(crate) fn pull_request_reply_label(&self, id: u64) -> Option<String> {
        pr_comment_action_label("r", &self.pull_request_replyable_comment_ids(), id)
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
        let Some(id) = self.pull_request_replyable_comment_ids().get(idx).copied() else {
            return false;
        };
        self.start_pull_request_reply(id)
    }

    pub(crate) fn reply_to_pull_request_comment_number(&mut self, number: usize) -> bool {
        let Some(id) = self
            .pull_request_replyable_comment_ids()
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

    fn review_comment_id_at_index(&mut self, idx: usize) -> Option<u64> {
        if self.active_outdated_comments_view() {
            return self.outdated_comment_ids().get(idx).copied();
        }
        if self.view_mode == ViewMode::Preview {
            return (idx == 0)
                .then(|| self.review_file_comment_overlay().map(|overlay| overlay.id))?;
        }
        self.review_comment_overlays_for_current_file()
            .get(idx)
            .map(|overlay| overlay.id)
    }

    fn editable_review_comment_id_at_index(&mut self, idx: usize) -> Option<u64> {
        let id = self.review_comment_id_at_index(idx)?;
        self.review_comments
            .iter()
            .any(|comment| comment.id == id && !comment.deleted && comment.can_edit)
            .then_some(id)
    }

    fn edit_review_comment_index(&mut self, idx: usize) -> bool {
        if self.active_pr_comments_view() {
            let Some(id) = self.pull_request_editable_comment_ids().get(idx).copied() else {
                return false;
            };
            return self.edit_pull_request_comment(id);
        }
        let Some(id) = self.editable_review_comment_id_at_index(idx) else {
            return false;
        };
        self.open_review_editor_for_id(id)
    }

    pub(crate) fn inline_review_actions_available(&mut self) -> bool {
        if self.active_pr_comments_view() {
            return false;
        }
        if self.active_outdated_comments_view() {
            return !self.outdated_comment_overlays().is_empty();
        }
        if self.view_mode == ViewMode::Preview {
            return self.review_file_comment_overlay().is_some();
        }
        !self.review_comment_overlays_for_current_file().is_empty()
    }

    pub(crate) fn inline_review_reply_available(&mut self) -> bool {
        !self.active_pr_comments_view()
            && !self.active_outdated_comments_view()
            && self.view_mode != ViewMode::Preview
            && self
                .review_comment_overlays_for_current_file()
                .iter()
                .any(|overlay| overlay.reply_label.is_some())
    }

    pub(crate) fn reply_to_review_comment_letter(&mut self, letter: char) -> bool {
        let letter = letter.to_ascii_lowercase();
        if !letter.is_ascii_lowercase() {
            return false;
        }
        self.reply_to_review_comment_index((letter as u8 - b'a') as usize)
    }

    pub(crate) fn reply_to_review_comment_number(&mut self, number: usize) -> bool {
        if number == 0 {
            return false;
        }
        self.reply_to_review_comment_index(number - 1)
    }

    fn reply_to_review_comment_index(&mut self, idx: usize) -> bool {
        let Some(id) = self
            .review_comment_overlays_for_current_file()
            .get(idx)
            .filter(|overlay| overlay.reply_label.is_some())
            .map(|overlay| overlay.id)
        else {
            return false;
        };
        self.start_review_comment_reply(id)
    }

    pub(crate) fn resolve_review_comment_letter(&mut self, letter: char) -> bool {
        let letter = letter.to_ascii_lowercase();
        if !letter.is_ascii_lowercase() {
            return false;
        }
        self.toggle_review_comment_resolved_index((letter as u8 - b'a') as usize)
    }

    pub(crate) fn resolve_review_comment_number(&mut self, number: usize) -> bool {
        if number == 0 {
            return false;
        }
        self.toggle_review_comment_resolved_index(number - 1)
    }

    fn toggle_review_comment_resolved_index(&mut self, idx: usize) -> bool {
        let Some(id) = self.review_comment_id_at_index(idx) else {
            return false;
        };
        let Some(comment_idx) = self
            .review_comments
            .iter()
            .position(|comment| comment.id == id && !comment.deleted)
        else {
            return false;
        };
        if review_comment_is_reply(&self.review_comments[comment_idx]) {
            return false;
        }
        let resolved = !self.review_comments[comment_idx].resolved;
        self.set_review_comment_resolved_at(comment_idx, resolved)
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
        let Some(id) = self.editable_review_comment_id_at_index(idx) else {
            return false;
        };
        self.request_delete_comment_by_id(id)
    }

    pub(crate) fn review_comment_body_for_id(&self, id: u64) -> Option<String> {
        self.review_comments
            .iter()
            .find(|comment| comment.id == id && !comment.deleted)
            .map(|comment| comment.body.clone())
    }

    pub(crate) fn review_provider_kind(&self) -> crate::ReviewProviderKind {
        self.review_target_metadata
            .as_ref()
            .and_then(|metadata| metadata.pr_provider.as_deref())
            .or_else(|| {
                self.review_pull_request_target
                    .as_ref()
                    .map(|target| target.provider.as_str())
            })
            .or_else(|| {
                self.review_comments.iter().find_map(|comment| {
                    comment
                        .provider
                        .as_ref()
                        .map(|provider| provider.provider.as_str())
                })
            })
            .and_then(crate::ReviewProviderKind::from_id)
            .unwrap_or(crate::ReviewProviderKind::GitHub)
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
            .or_else(|| {
                self.review_pull_request_target
                    .as_ref()
                    .map(|target| target.title.clone())
                    .filter(|title| !title.trim().is_empty())
            })
            .unwrap_or_else(|| {
                let noun = self.review_provider_kind().long_review_noun();
                let mut chars = noun.chars();
                chars
                    .next()
                    .map(|first| {
                        format!(
                            "{}{}",
                            first.to_ascii_uppercase(),
                            chars.collect::<String>()
                        )
                    })
                    .unwrap_or_default()
            })
    }

    pub(crate) fn set_review_pull_request_target(
        &mut self,
        target: Option<ReviewPullRequestTarget>,
    ) {
        self.review_pull_request_target = target;
    }

    pub(crate) fn review_pull_request_lookup_needed(&self) -> bool {
        self.review_mode && !self.pull_request_comment_target_available()
    }

    pub(crate) fn review_pull_request_lookup_target(
        &self,
        selected_remote: Option<&str>,
    ) -> Option<String> {
        self.review_pull_request_target
            .as_ref()
            .filter(|target| selected_remote.is_none_or(|remote| target.remote.as_str() == remote))
            .map(|target| target.number.to_string())
            .or_else(|| {
                if selected_remote.is_some() {
                    return None;
                }
                self.review_target_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.pr_number.map(|number| number.to_string()))
            })
            .or_else(|| {
                self.review_target_metadata.as_ref().and_then(|metadata| {
                    if metadata.vcs != "jj" {
                        return metadata
                            .branch
                            .clone()
                            .or_else(|| metadata.git_head_ref.clone());
                    }
                    if metadata.label.contains("..") {
                        return Some(metadata.label.clone());
                    }
                    let bookmarks = metadata
                        .bookmarks
                        .as_deref()?
                        .split_whitespace()
                        .collect::<Vec<_>>();
                    (bookmarks.len() == 1).then(|| bookmarks[0].to_string())
                })
            })
            .filter(|target| crate::usable_sync_pr_target(target).is_some())
    }

    pub(crate) fn pull_request_comment_target_available(&self) -> bool {
        self.review_target_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.pr_number.is_some())
            || self
                .review_pull_request_target
                .as_ref()
                .is_some_and(|target| {
                    target.number > 0
                        && !target.provider.trim().is_empty()
                        && !target.remote.trim().is_empty()
                        && !target.repo.trim().is_empty()
                })
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
            snapshot: None,
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
        self.active_review_comment_id = None;
        self.review_editor = Some(ReviewEditorState {
            anchor: self.pull_request_anchor(key),
            text: quote,
            cursor: 0,
            reply: None,
        });
        if let Some(editor) = self.review_editor.as_mut() {
            editor.cursor = editor.text.len();
        }
        true
    }

    fn review_reply_draft(
        &self,
        parent_id: u64,
    ) -> Result<(ReviewAnchor, ReviewReplyDraft), String> {
        let parent = self
            .review_comments
            .iter()
            .find(|comment| comment.id == parent_id && !comment.deleted)
            .ok_or_else(|| format!("No comment matches id {parent_id}."))?;
        if parent.deleted
            || parent.outdated
            || !matches!(
                parent.anchor.kind,
                ReviewTargetKind::Line | ReviewTargetKind::Hunk
            )
        {
            return Err(format!("Comment {parent_id} is not an inline comment."));
        }
        let (provider, in_reply_to) = if let Some(parent_provider) = parent.provider.as_ref() {
            if !review_comment_can_reply(parent) {
                return Err(format!("Comment {parent_id} cannot accept replies."));
            }
            let remote_parent_id = if parent_provider.comment_id.is_empty() {
                parent_provider.in_reply_to_id.clone().unwrap()
            } else {
                parent_provider.comment_id.clone()
            };
            let mut provider = parent_provider.clone();
            provider.in_reply_to_id = Some(remote_parent_id);
            provider.comment_id.clear();
            provider.sync_state = "dirty".to_string();
            provider.resolved_dirty = provider
                .thread_resolved
                .is_some_and(|remote| remote != parent.resolved);
            provider.author_username = self
                .review_author
                .as_ref()
                .and_then(|author| author.usernames.get(&provider.provider).cloned());
            (Some(provider), None)
        } else {
            (None, Some(parent_id))
        };
        let mut anchor = parent.anchor.clone();
        anchor.anchor_key = format!("reply|{parent_id}|{}", self.review_next_comment_id);
        Ok((
            anchor,
            ReviewReplyDraft {
                provider,
                in_reply_to,
                resolved: parent.resolved,
            },
        ))
    }

    pub(crate) fn start_review_comment_reply(&mut self, parent_id: u64) -> bool {
        let Ok((anchor, reply)) = self.review_reply_draft(parent_id) else {
            return false;
        };
        self.clear_diff_selection();
        self.active_review_comment_id = None;
        self.review_editor = Some(ReviewEditorState {
            anchor,
            text: String::new(),
            cursor: 0,
            reply: Some(reply),
        });
        true
    }

    pub fn add_review_reply_from_cli(
        &mut self,
        parent_id: u64,
        body: String,
    ) -> Result<u64, String> {
        if body.trim().is_empty() {
            return Err("Comment body cannot be empty.".to_string());
        }
        let (anchor, reply) = self.review_reply_draft(parent_id)?;
        let id = self.review_next_comment_id;
        self.review_next_comment_id = self.review_next_comment_id.saturating_add(1);
        let now = now_ts();
        self.review_comments.push(ReviewComment {
            id,
            anchor,
            body,
            author: self.review_author.clone(),
            can_edit: true,
            resolved: reply.resolved,
            outdated: false,
            reanchored: false,
            deleted: false,
            provider: reply.provider,
            in_reply_to: reply.in_reply_to,
            created_at: now,
            updated_at: now,
        });
        self.touch_review_state();
        self.persist_review_session();
        self.run_review_hooks(ReviewHookEvent::CommentSaved, None);
        self.notify(ToastEvent::CommentSaved);
        Ok(id)
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
        self.open_review_editor_for_id(comment.id)
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
            crossterm::event::KeyCode::Enter
                if matches!(
                    self.review_delete_confirmation
                        .as_ref()
                        .map(|item| &item.target),
                    Some(ReviewDeleteTarget::Comment { .. })
                ) =>
            {
                self.confirm_review_delete();
                true
            }
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
        let mut actions = Vec::new();
        if overlay.can_edit {
            actions.push(format!(
                "{} to update",
                if overlay.is_hunk { "M" } else { "m" }
            ));
        }
        if let Some(key) = overlay.reply_label.as_deref() {
            actions.push(format!("{key} to reply"));
        }
        if let Some(key) = overlay.resolve_label.as_deref() {
            actions.push(format!(
                "{key} to {}",
                if overlay.resolved {
                    "unresolve"
                } else {
                    "resolve"
                }
            ));
        }
        if overlay.can_edit {
            actions.push(format!(
                "{} to remove",
                if overlay.is_hunk { "X" } else { "x" }
            ));
        }
        format!(
            "{} • {}",
            overlay.preview,
            if actions.is_empty() {
                "read only".to_string()
            } else {
                actions.join(", ")
            }
        )
    }

    pub fn clear_review_preview_boxes(&mut self) {
        self.review_preview_boxes.clear();
    }

    pub(crate) fn review_preview_flash_key(&self) -> Option<u64> {
        self.review_preview_flash
            .as_ref()
            .and_then(|(id, _, until)| (Instant::now() < *until).then_some(*id))
    }

    pub(crate) fn review_preview_flash_active(&self, comment_id: u64, anchor_key: &str) -> bool {
        self.review_preview_flash
            .as_ref()
            .is_some_and(|(id, key, until)| {
                (*id == comment_id || (*id == 0 && key == anchor_key)) && Instant::now() < *until
            })
    }

    fn flash_review_preview(&mut self, comment_id: u64, anchor_key: String) {
        self.review_preview_flash = Some((
            comment_id,
            anchor_key,
            Instant::now() + Duration::from_millis(650),
        ));
    }

    fn set_review_comment_resolved_at(&mut self, idx: usize, resolved: bool) -> bool {
        let thread_key = self
            .review_comments
            .get(idx)
            .and_then(|comment| review_thread_key(comment, &self.review_comments));
        let comment_id = self.review_comments[idx].id;
        let thread_member_ids = thread_key.as_ref().map(|key| {
            self.review_comments
                .iter()
                .filter(|comment| {
                    review_thread_key(comment, &self.review_comments).as_ref() == Some(key)
                })
                .map(|comment| comment.id)
                .collect::<FxHashSet<_>>()
        });
        let mut changed = false;
        for comment in &mut self.review_comments {
            if comment.deleted
                || thread_member_ids
                    .as_ref()
                    .is_some_and(|ids| !ids.contains(&comment.id))
                || (thread_member_ids.is_none() && comment.id != comment_id)
                || comment.resolved == resolved
            {
                continue;
            }
            comment.resolved = resolved;
            if let Some(provider) = comment.provider.as_mut() {
                provider.resolved_dirty = provider
                    .thread_resolved
                    .is_none_or(|remote| remote != resolved);
            }
            comment.updated_at = bumped_ts(comment.updated_at);
            changed = true;
        }
        if changed {
            self.touch_review_state();
            self.persist_review_session();
        }
        true
    }

    fn toggle_review_comment_resolved_by_anchor(&mut self, anchor_key: String) -> bool {
        let Some(idx) = self
            .review_comments
            .iter()
            .position(|comment| !comment.deleted && comment.anchor.anchor_key == anchor_key)
        else {
            return false;
        };
        let resolved = !self.review_comments[idx].resolved;
        self.set_review_comment_resolved_at(idx, resolved)
    }

    fn request_delete_comment_by_anchor(&mut self, anchor_key: String) -> bool {
        self.remove_comment_for_anchor_key(&anchor_key)
    }

    fn request_delete_comment_by_id(&mut self, id: u64) -> bool {
        let Some(reply_count) = self.review_comment_delete_reply_count(id) else {
            return false;
        };
        if reply_count > 0 {
            let replies = if reply_count == 1 { "reply" } else { "replies" };
            self.review_delete_confirmation = Some(ReviewDeleteConfirmation {
                target: ReviewDeleteTarget::Comment { id, reply_count },
                title: "Delete comment".to_string(),
                body: format!("Delete this comment and its {reply_count} {replies}?"),
                confirm_label: "enter delete".to_string(),
            });
            self.review_delete_confirmation_hits.clear();
            self.review_delete_confirmation_hover = None;
            return true;
        }
        self.delete_review_comment_with_feedback(id)
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
            ReviewDeleteTarget::Comment { id, .. } => {
                self.delete_review_comment_with_feedback(id);
            }
        }
    }

    pub fn remove_hovered_review_comment(&mut self) -> bool {
        let id = self.review_preview_hover_id.take();
        let anchor_key = self.review_preview_hover.take();
        if let Some(id) = id {
            self.request_delete_comment_by_id(id)
        } else {
            anchor_key.is_some_and(|key| self.request_delete_comment_by_anchor(key))
        }
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
            comment_id: None,
            x,
            y,
            width,
            height,
            anchor_key,
            edit: false,
            reply: false,
            resolve: false,
            delete: false,
            overflow: false,
            passive: false,
        });
    }

    pub fn add_review_comment_preview_box(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        comment_id: u64,
        anchor_key: String,
    ) {
        self.review_preview_boxes.push(ReviewPreviewBox {
            comment_id: Some(comment_id),
            x,
            y,
            width,
            height,
            anchor_key,
            edit: false,
            reply: false,
            resolve: false,
            delete: false,
            overflow: false,
            passive: false,
        });
    }

    pub fn add_review_preview_passive_box(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        comment_id: u64,
        anchor_key: String,
    ) {
        self.review_preview_boxes.push(ReviewPreviewBox {
            comment_id: Some(comment_id),
            x,
            y,
            width,
            height,
            anchor_key,
            edit: false,
            reply: false,
            resolve: false,
            delete: false,
            overflow: false,
            passive: true,
        });
    }

    pub fn add_review_preview_edit_box(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        comment_id: u64,
        anchor_key: String,
    ) {
        self.review_preview_boxes.push(ReviewPreviewBox {
            comment_id: Some(comment_id),
            x,
            y,
            width,
            height,
            anchor_key,
            edit: true,
            reply: false,
            resolve: false,
            delete: false,
            overflow: false,
            passive: false,
        });
    }

    pub fn add_review_preview_reply_box(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        comment_id: u64,
        anchor_key: String,
    ) {
        self.review_preview_boxes.push(ReviewPreviewBox {
            comment_id: Some(comment_id),
            x,
            y,
            width,
            height,
            anchor_key,
            edit: false,
            reply: true,
            resolve: false,
            delete: false,
            overflow: false,
            passive: false,
        });
    }

    pub fn add_review_preview_resolve_box(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        comment_id: u64,
        anchor_key: String,
    ) {
        self.review_preview_boxes.push(ReviewPreviewBox {
            comment_id: Some(comment_id),
            x,
            y,
            width,
            height,
            anchor_key,
            edit: false,
            reply: false,
            resolve: true,
            delete: false,
            overflow: false,
            passive: false,
        });
    }

    pub fn add_review_preview_delete_box(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        comment_id: u64,
        anchor_key: String,
    ) {
        self.review_preview_boxes.push(ReviewPreviewBox {
            comment_id: Some(comment_id),
            x,
            y,
            width,
            height,
            anchor_key,
            edit: false,
            reply: false,
            resolve: false,
            delete: true,
            overflow: false,
            passive: false,
        });
    }

    pub fn add_review_preview_overflow_box(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        comment_id: u64,
        anchor_key: String,
    ) {
        self.review_preview_boxes.push(ReviewPreviewBox {
            comment_id: Some(comment_id),
            x,
            y,
            width,
            height,
            anchor_key,
            edit: false,
            reply: false,
            resolve: false,
            delete: false,
            overflow: true,
            passive: false,
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
            .filter(|hit| !hit.delete && !hit.edit && !hit.reply && !hit.resolve && !hit.overflow);
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

    pub(crate) fn diff_line_hover_row_at(&self, column: u16, row: u16) -> Option<u16> {
        if self.view_mode == ViewMode::Preview || self.current_file_is_binary() {
            return None;
        }
        let (x, y, width, height) = self.diff_view_area?;
        if column < x
            || column >= x.saturating_add(width)
            || row < y
            || row >= y.saturating_add(height)
        {
            return None;
        }
        let local_row = row.saturating_sub(y) as usize;
        if self
            .diff_selection_cells
            .get(local_row)
            .is_none_or(|cells| cells.iter().all(|cell| cell.trim().is_empty()))
        {
            return None;
        }
        self.review_display_idx_for_screen_row(row).map(|_| row)
    }

    pub(crate) fn remember_diff_line_hover(&mut self, column: u16, row: u16) {
        let Some(row) = self.diff_line_hover_row_at(column, row) else {
            return;
        };
        let side = self.review_side_at_screen_column(column);
        if let Some(anchor) = self.review_anchor_at_screen_row_on_side(row, side) {
            self.last_hovered_review_anchor = Some((self.diff_revision(), anchor));
        }
    }

    pub(crate) fn review_line_add_hover_at(&self, column: u16, row: u16) -> (Option<u16>, bool) {
        if !self.review_mode || self.review_editor.is_some() || self.selection_toolbar_visible() {
            return (None, false);
        }
        let Some(row) = self.diff_line_hover_row_at(column, row) else {
            return (None, false);
        };
        let Some(hit_x) = self.review_line_add_button_x() else {
            return (Some(row), false);
        };
        let hover = column >= hit_x && column < hit_x.saturating_add(3);
        (Some(row), hover)
    }

    pub(crate) fn clear_review_unified_line_rows(&mut self) {
        self.review_unified_line_rows.clear();
    }

    pub(crate) fn add_review_unified_line_row(&mut self, row: usize, display_idx: usize) {
        self.review_unified_line_rows.push((row, display_idx));
    }

    pub(crate) fn crop_review_unified_line_rows(&mut self, skipped: usize) {
        self.review_unified_line_rows = self
            .review_unified_line_rows
            .drain(..)
            .filter_map(|(row, display_idx)| row.checked_sub(skipped).map(|row| (row, display_idx)))
            .collect();
    }

    pub(crate) fn clear_review_split_line_rows(&mut self) {
        self.review_split_line_rows.clear();
    }

    pub(crate) fn add_review_split_line_row(
        &mut self,
        row: u16,
        side: ReviewSide,
        display_idx: usize,
    ) {
        self.review_split_line_rows.push((row, side, display_idx));
    }

    pub(crate) fn review_side_at_screen_column(&self, column: u16) -> Option<ReviewSide> {
        if self.view_mode != ViewMode::Split {
            return None;
        }
        let (x, _, _, _) = self.diff_view_area?;
        let column = column.saturating_sub(x);
        let ranges = self.diff_selection_content_ranges();
        let [left, right] = ranges.as_slice() else {
            return None;
        };
        if column >= right.0 && column < right.1 {
            Some(ReviewSide::New)
        } else if column >= left.0 && column < left.1 {
            Some(ReviewSide::Old)
        } else {
            None
        }
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
        self.start_line_comment_at_screen_row_on_side(hit.row, self.review_line_add_side)
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
            (column >= hit.x && column < end_x && row >= hit.y && row < end_y).then(|| {
                (
                    hit.comment_id,
                    hit.anchor_key.clone(),
                    hit.edit,
                    hit.reply,
                    hit.resolve,
                    hit.delete,
                    hit.overflow,
                    hit.passive,
                )
            })
        });

        let Some((comment_id, anchor_key, _edit, reply, resolve, delete, overflow, passive)) = hit
        else {
            return false;
        };
        if passive {
            return true;
        }
        if overflow {
            return comment_id
                .is_some_and(|id| self.open_review_comment_context_menu_for_id(id, column, row));
        }
        if delete {
            return comment_id.is_some_and(|id| self.request_delete_comment_by_id(id));
        }
        if reply {
            return comment_id.is_some_and(|id| self.start_review_comment_reply(id));
        }
        if resolve {
            return self.toggle_review_comment_resolved_by_anchor(anchor_key);
        }

        comment_id.is_some_and(|id| self.open_review_editor_for_id(id))
    }

    fn open_review_comment_context_menu_for_id(
        &mut self,
        comment_id: u64,
        column: u16,
        row: u16,
    ) -> bool {
        if !self
            .review_comments
            .iter()
            .any(|comment| !comment.deleted && comment.id == comment_id)
        {
            return false;
        }
        self.close_file_context_menu();
        self.close_status_mode_menu();
        self.review_comment_context_menu = Some(ReviewCommentContextMenu {
            comment_id,
            x: column,
            y: row,
        });
        self.review_comment_context_menu_hover =
            self.review_comment_context_menu_actions().first().copied();
        true
    }

    #[cfg(test)]
    pub fn open_review_comment_context_menu_for_anchor(
        &mut self,
        anchor_key: String,
        column: u16,
        row: u16,
    ) -> bool {
        let Some(comment_id) = self
            .review_comments
            .iter()
            .find(|comment| !comment.deleted && comment.anchor.anchor_key == anchor_key)
            .map(|comment| comment.id)
        else {
            return false;
        };
        self.open_review_comment_context_menu_for_id(comment_id, column, row)
    }

    pub fn open_review_comment_context_menu_letter(&mut self, letter: char) -> bool {
        let index = (letter as u8).saturating_sub(b'a') as usize;
        self.open_review_comment_context_menu_index(index)
    }

    pub fn open_review_comment_context_menu_number(&mut self, number: usize) -> bool {
        self.open_review_comment_context_menu_index(number.saturating_sub(1))
    }

    fn open_review_comment_context_menu_index(&mut self, idx: usize) -> bool {
        let Some(overlay) = self
            .review_comment_overlays_for_current_file()
            .get(idx)
            .cloned()
        else {
            return false;
        };
        let (x, y) = self
            .review_preview_boxes
            .iter()
            .find(|hit| hit.overflow && hit.anchor_key == overlay.anchor_key)
            .map(|hit| (hit.x, hit.y))
            .unwrap_or((0, 0));
        self.open_review_comment_context_menu_for_id(overlay.id, x, y)
    }

    pub fn close_review_comment_context_menu(&mut self) -> bool {
        let was_open = self.review_comment_context_menu.take().is_some();
        self.review_comment_context_menu_hits.clear();
        self.review_comment_context_menu_hover = None;
        was_open
    }

    pub(crate) fn review_comment_context_menu_actions(
        &self,
    ) -> Vec<ReviewCommentContextMenuAction> {
        let Some(comment) = self.review_comment_context_menu_comment() else {
            return Vec::new();
        };
        let mut actions = vec![
            ReviewCommentContextMenuAction::Body,
            ReviewCommentContextMenuAction::Id,
            ReviewCommentContextMenuAction::FileLine,
        ];
        if comment
            .provider
            .as_ref()
            .and_then(provider_comment_url)
            .is_some()
        {
            actions.push(ReviewCommentContextMenuAction::Url);
        }
        actions.push(ReviewCommentContextMenuAction::MarkdownQuote);
        actions
    }

    pub(crate) fn review_comment_context_menu_comment(&self) -> Option<&ReviewComment> {
        let comment_id = self.review_comment_context_menu?.comment_id;
        self.review_comments
            .iter()
            .find(|comment| !comment.deleted && comment.id == comment_id)
    }

    pub(crate) fn review_comment_context_menu_label(
        &self,
        action: ReviewCommentContextMenuAction,
    ) -> String {
        match action {
            ReviewCommentContextMenuAction::Body => "Copy body".to_string(),
            ReviewCommentContextMenuAction::Id => self
                .review_comment_context_menu_comment()
                .map(|comment| format!("Copy id (#{})", comment.id))
                .unwrap_or_else(|| "Copy id".to_string()),
            ReviewCommentContextMenuAction::FileLine => self
                .review_comment_context_menu_comment()
                .map(|comment| {
                    format!(
                        "Copy location ({})",
                        self.review_comment_path_line_label(comment)
                    )
                })
                .unwrap_or_else(|| "Copy location".to_string()),
            ReviewCommentContextMenuAction::Url => {
                format!(
                    "Copy {} URL",
                    self.review_provider_kind().short_review_noun()
                )
            }
            ReviewCommentContextMenuAction::MarkdownQuote => "Copy as blockquote".to_string(),
        }
    }

    pub(crate) fn review_comment_context_menu_action_at(
        &self,
        column: u16,
        row: u16,
    ) -> Option<ReviewCommentContextMenuAction> {
        self.review_comment_context_menu_hits
            .iter()
            .find(|hit| {
                column >= hit.x
                    && column < hit.x.saturating_add(hit.width)
                    && row >= hit.y
                    && row < hit.y.saturating_add(hit.height)
            })
            .map(|hit| hit.action)
    }

    pub fn update_review_comment_context_menu_hover(&mut self, column: u16, row: u16) -> bool {
        let hover = self.review_comment_context_menu_action_at(column, row);
        if self.review_comment_context_menu_hover == hover {
            return false;
        }
        self.review_comment_context_menu_hover = hover;
        true
    }

    pub(crate) fn move_review_comment_context_menu_active(&mut self, forward: bool) -> bool {
        let actions = self.review_comment_context_menu_actions();
        if actions.is_empty() {
            return false;
        }
        let position = self
            .review_comment_context_menu_hover
            .and_then(|active| actions.iter().position(|action| *action == active));
        let next = if forward {
            position.map_or(0, |index| (index + 1) % actions.len())
        } else {
            position.map_or(actions.len() - 1, |index| {
                index.checked_sub(1).unwrap_or(actions.len() - 1)
            })
        };
        self.review_comment_context_menu_hover = Some(actions[next]);
        true
    }

    pub(crate) fn activate_review_comment_context_menu(&mut self) -> bool {
        let Some(action) = self
            .review_comment_context_menu_hover
            .or_else(|| self.review_comment_context_menu_actions().first().copied())
        else {
            return false;
        };
        self.run_review_comment_context_menu_action(action);
        self.close_review_comment_context_menu();
        true
    }

    pub fn handle_review_comment_context_menu_click(&mut self, column: u16, row: u16) -> bool {
        if self.review_comment_context_menu.is_none() {
            return false;
        }
        if let Some(action) = self.review_comment_context_menu_action_at(column, row) {
            self.run_review_comment_context_menu_action(action);
            self.close_review_comment_context_menu();
            return true;
        }
        self.close_review_comment_context_menu();
        true
    }

    fn run_review_comment_context_menu_action(&mut self, action: ReviewCommentContextMenuAction) {
        let Some(comment) = self.review_comment_context_menu_comment().cloned() else {
            return;
        };
        let text = match action {
            ReviewCommentContextMenuAction::Body => comment.body.clone(),
            ReviewCommentContextMenuAction::Id => format!("#{}", comment.id),
            ReviewCommentContextMenuAction::FileLine => {
                self.review_comment_path_line_label(&comment)
            }
            ReviewCommentContextMenuAction::Url => comment
                .provider
                .as_ref()
                .and_then(provider_comment_url)
                .unwrap_or_default(),
            ReviewCommentContextMenuAction::MarkdownQuote => quote_markdown_body(&comment.body),
        };
        if text.is_empty() {
            self.notify(ToastEvent::CopyFailed);
        } else if copy_to_clipboard(&text) {
            self.notify(ToastEvent::SelectionActionStarted(
                "Copied comment detail".to_string(),
            ));
        } else {
            self.notify(ToastEvent::CopyFailed);
        }
    }

    fn current_cursor_review_anchor(&mut self) -> Option<ReviewAnchor> {
        let file_index = self.multi_diff.selected_index;
        let file_path = self.current_file_path();
        if file_path.is_empty() {
            return None;
        }
        let visible = self.review_visible_lines_with_idx();
        let (display_idx, line) = visible
            .iter()
            .find(|(_, line)| line.is_primary_active)
            .or_else(|| visible.iter().find(|(_, line)| line.is_active))?;
        line_review_anchor_from_view_line(file_index, file_path, *display_idx, line)
    }

    fn hovered_or_cursor_review_anchor(&mut self) -> Option<ReviewAnchor> {
        let current_path = self.current_file_path();
        if let Some((revision, anchor)) = &self.last_hovered_review_anchor {
            if *revision == self.diff_revision()
                && anchor.file_index == self.multi_diff.selected_index
                && anchor.file_path == current_path
            {
                return Some(anchor.clone());
            }
        }
        self.current_cursor_review_anchor()
    }

    fn relative_anchor_position_label(anchor: &ReviewAnchor) -> Option<String> {
        let position = review_anchor_start_location_label(anchor)?;
        Some(format!("{}:{position}", anchor.file_path))
    }

    pub(crate) fn current_file_relative_position_label(&mut self) -> Option<String> {
        let anchor = self.hovered_or_cursor_review_anchor()?;
        Self::relative_anchor_position_label(&anchor)
    }

    pub(crate) fn current_file_cursor_position_label(&mut self) -> Option<String> {
        let anchor = self.current_cursor_review_anchor()?;
        Self::relative_anchor_position_label(&anchor)
    }

    pub(crate) fn current_file_absolute_path_label(&self) -> Option<String> {
        let path = self.current_file_path();
        (!path.is_empty()).then(|| self.review_absolute_path_label(Path::new(&path)))
    }

    pub(crate) fn current_file_absolute_position_label(&mut self) -> Option<String> {
        let anchor = self.hovered_or_cursor_review_anchor()?;
        Some(self.review_anchor_path_line_label(&anchor))
    }

    pub(crate) fn current_file_absolute_cursor_position_label(&mut self) -> Option<String> {
        let anchor = self.current_cursor_review_anchor()?;
        Some(self.review_anchor_path_line_label(&anchor))
    }

    fn review_comment_path_line_label(&self, comment: &ReviewComment) -> String {
        self.review_anchor_path_line_label(&comment.anchor)
    }

    fn review_absolute_path_label(&self, path: &Path) -> String {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.review_workspace_root_override
                .as_deref()
                .or_else(|| self.review_repo_root.as_deref().map(Path::new))
                .or_else(|| self.multi_diff.repo_root())
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
                .join(path)
        };
        collapse_home_path(&abs)
    }

    fn review_anchor_path_line_label(&self, anchor: &ReviewAnchor) -> String {
        let label = self.review_absolute_path_label(Path::new(&anchor.file_path));
        match review_anchor_start_location_label(anchor) {
            Some(location) => format!("{label}:{location}"),
            None => label,
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
        let action = if editor.reply.is_some() {
            "Reply"
        } else {
            "Comment"
        };
        let title = format!(" {action} • {anchor_label} ");

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

    pub(crate) fn review_fold_anchor_revision(&self) -> u64 {
        if !self.review_mode {
            return 0;
        }
        let file_path = self.current_file_path();
        let reconstructed_comment_id = self.outdated_diff_view.as_ref().map(|view| view.comment_id);
        let anchors = self
            .review_comments
            .iter()
            .filter(|comment| {
                review_comment_is_inline_visible(
                    comment,
                    &file_path,
                    reconstructed_comment_id,
                    &self.review_comments,
                ) && comment.anchor.kind == ReviewTargetKind::Line
            })
            .map(|comment| comment.anchor.anchor_key.as_str())
            .collect::<BTreeSet<_>>();
        let mut hasher = DefaultHasher::new();
        anchors.hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn review_fold_anchor_change_ids(&self, view: &[ViewLine]) -> FxHashSet<usize> {
        if !self.review_mode {
            return FxHashSet::default();
        }
        let file_path = self.current_file_path();
        let reconstructed_comment_id = self.outdated_diff_view.as_ref().map(|view| view.comment_id);
        self.review_comments
            .iter()
            .filter(|comment| {
                review_comment_is_inline_visible(
                    comment,
                    &file_path,
                    reconstructed_comment_id,
                    &self.review_comments,
                ) && comment.anchor.kind == ReviewTargetKind::Line
            })
            .filter_map(|comment| {
                view.iter()
                    .find(|line| line_anchor_matches(&comment.anchor, line))
                    .map(|line| line.change_id)
            })
            .collect()
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

        let reconstructed_comment_id = self.outdated_diff_view.as_ref().map(|view| view.comment_id);
        let editing_comment_id = self
            .review_editor
            .as_ref()
            .filter(|editor| editor.reply.is_none())
            .and(self.active_review_comment_id);
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| {
                review_comment_is_inline_visible(
                    comment,
                    &file_path,
                    reconstructed_comment_id,
                    &self.review_comments,
                )
            })
            .filter(|comment| editing_comment_id != Some(comment.id))
            .collect::<Vec<_>>();
        let mut thread_order = BTreeMap::new();
        for comment in &comments {
            if let Some(key) = review_thread_key(comment, &self.review_comments) {
                let next = thread_order.len();
                thread_order.entry(key).or_insert(next);
            }
        }
        comments.sort_by_key(|comment| {
            (
                review_thread_key(comment, &self.review_comments)
                    .and_then(|key| thread_order.get(&key).copied())
                    .unwrap_or(usize::MAX),
                review_comment_is_reply(comment),
                comment.created_at,
                comment.id,
            )
        });

        let mut overlays = Vec::new();
        for (position, comment) in comments.iter().enumerate() {
            let thread_key = review_thread_key(comment, &self.review_comments);
            let display_anchor = if review_comment_is_reply(comment) {
                thread_key
                    .as_ref()
                    .and_then(|key| {
                        self.review_comments.iter().find(|candidate| {
                            !candidate.deleted
                                && !review_comment_is_reply(candidate)
                                && review_thread_key(candidate, &self.review_comments).as_ref()
                                    == Some(key)
                        })
                    })
                    .map(|root| &root.anchor)
                    .unwrap_or(&comment.anchor)
            } else {
                &comment.anchor
            };
            let display_idx = match display_anchor.kind {
                ReviewTargetKind::PullRequest => Some(0),
                ReviewTargetKind::File => None,
                ReviewTargetKind::Line => visible.iter().find_map(|(idx, line)| {
                    line_anchor_matches(display_anchor, line).then_some(*idx)
                }),
                ReviewTargetKind::Hunk => {
                    if let Some(hunk_id) = display_anchor.hunk_id {
                        visible.iter().find_map(|(idx, line)| {
                            (line.hunk_index == Some(hunk_id)).then_some(*idx)
                        })
                    } else {
                        let old_range = display_anchor.old_range;
                        let new_range = display_anchor.new_range;
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

            let prefer_right = match display_anchor.kind {
                ReviewTargetKind::PullRequest | ReviewTargetKind::File => true,
                ReviewTargetKind::Line => !matches!(display_anchor.side, Some(ReviewSide::Old)),
                ReviewTargetKind::Hunk => !matches!(
                    (display_anchor.old_range, display_anchor.new_range),
                    (Some(_), None)
                ),
            };

            let thread_continues = thread_key.as_ref().is_some_and(|key| {
                comments.get(position + 1).is_some_and(|next| {
                    review_thread_key(next, &self.review_comments).as_ref() == Some(key)
                })
            });
            let title = review_comment_title(comment, self.review_author.as_ref());
            overlays.push(ReviewCommentOverlay {
                id: comment.id,
                display_idx,
                preview,
                body: comment.body.clone(),
                title,
                avatar_url: comment
                    .author
                    .as_ref()
                    .and_then(|author| author.avatar_url.clone()),
                avatar_seed: review_author_avatar_seed(comment.author.as_ref()),
                anchor_key: comment.anchor.anchor_key.clone(),
                edit_label: None,
                reply_label: review_comment_can_reply(comment).then(String::new),
                resolve_label: (!review_comment_is_reply(comment)).then(String::new),
                delete_label: None,
                overflow_label: Some(String::new()),
                thread_continues,
                prefer_right,
                is_hunk: matches!(display_anchor.kind, ReviewTargetKind::Hunk),
                can_edit: comment.can_edit,
                resolved: comment.resolved,
                outdated: false,
                syntax_path: None,
                snapshot_code: None,
            });
        }

        overlays.sort_by_key(|overlay| overlay.display_idx);
        for (action_idx, overlay) in overlays.iter_mut().enumerate() {
            if overlay.reply_label.is_some() {
                overlay.reply_label = Some(review_index_action_label("r", action_idx));
            }
            if overlay.resolve_label.is_some() {
                overlay.resolve_label = Some(review_index_action_label("v", action_idx));
            }
            overlay.overflow_label = Some(review_index_action_label("o", action_idx));
            if overlay.can_edit {
                overlay.edit_label = Some(review_index_action_label("i", action_idx));
                overlay.delete_label = Some(review_index_action_label("x", action_idx));
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
        let comment = self.review_comments.iter().find(|comment| {
            !comment.deleted && !comment.outdated && comment.anchor.anchor_key == anchor.anchor_key
        })?;
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
            id: comment.id,
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
            reply_label: None,
            resolve_label: Some(review_index_action_label("v", 0)),
            delete_label: comment.can_edit.then(|| review_index_action_label("x", 0)),
            overflow_label: Some(review_index_action_label("o", 0)),
            thread_continues: false,
            prefer_right: true,
            is_hunk: false,
            can_edit: comment.can_edit,
            resolved: comment.resolved,
            outdated: false,
            syntax_path: None,
            snapshot_code: None,
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
        self.review_pull_request_target = None;
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
        self.review_storage_key = self.compute_review_storage_key(&repo_root, &diff_fingerprint);
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
            self.review_session_baseline = self.review_comments.clone();
            if self.fill_current_author_comments() {
                self.persist_review_session();
            }
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
        if self.active_outdated_comments_view() {
            return;
        }
        if self.file_review_comments_supported() {
            let _ = self.start_file_comment();
            return;
        }
        if let Some(row) = self.review_line_add_row {
            if self.start_line_comment_at_screen_row_on_side(row, self.review_line_add_side) {
                return;
            }
        }
        let Some(anchor) = self.resolve_line_review_anchor() else {
            return;
        };
        self.open_review_editor(anchor);
    }

    pub(crate) fn review_anchor_at_screen_row_on_side(
        &mut self,
        row: u16,
        preferred_side: Option<ReviewSide>,
    ) -> Option<ReviewAnchor> {
        let (_, y, _, height) = self.diff_view_area?;
        if row < y || row >= y.saturating_add(height) {
            return None;
        }
        let display_idx = if self.view_mode == ViewMode::Split {
            preferred_side.and_then(|side| {
                self.review_split_line_rows.iter().find_map(
                    |(target_row, target_side, display_idx)| {
                        (*target_row == row && *target_side == side).then_some(*display_idx)
                    },
                )
            })
        } else {
            let local_row = row.saturating_sub(y) as usize;
            self.review_unified_line_rows
                .iter()
                .find_map(|(target_row, display_idx)| {
                    (*target_row == local_row).then_some(*display_idx)
                })
        }
        .or_else(|| self.review_display_idx_for_screen_row(row));
        let display_idx = display_idx?;
        self.resolve_line_review_anchor_at_display_idx_on_side(display_idx, preferred_side)
    }

    pub(crate) fn start_line_comment_at_screen_row_on_side(
        &mut self,
        row: u16,
        preferred_side: Option<ReviewSide>,
    ) -> bool {
        if !self.review_mode {
            return false;
        }
        let Some(anchor) = self.review_anchor_at_screen_row_on_side(row, preferred_side) else {
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

        let existing_idx = if editor.reply.is_some() {
            None
        } else {
            self.active_review_comment_id
                .and_then(|id| {
                    self.review_comments.iter().position(|comment| {
                        comment.id == id && !comment.deleted && comment.can_edit
                    })
                })
                .or_else(|| {
                    self.review_comments.iter().position(|comment| {
                        !comment.deleted
                            && comment.can_edit
                            && comment.anchor.anchor_key == editor.anchor.anchor_key
                    })
                })
        };

        if body.trim().is_empty() {
            if let Some(id) = existing_idx.map(|idx| self.review_comments[idx].id) {
                self.request_delete_comment_by_id(id);
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
                existing.updated_at = bumped_ts(existing.updated_at);
            }
        } else {
            let id = self.review_next_comment_id;
            self.review_next_comment_id = self.review_next_comment_id.saturating_add(1);
            let (provider, in_reply_to, resolved) = editor
                .reply
                .map(|reply| (reply.provider, reply.in_reply_to, reply.resolved))
                .unwrap_or((None, None, false));
            self.review_comments.push(ReviewComment {
                id,
                anchor: editor.anchor,
                body,
                author: self.review_author.clone(),
                can_edit: true,
                resolved,
                outdated: false,
                reanchored: false,
                deleted: false,
                provider,
                in_reply_to,
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
        if self.review_delete_confirmation_active() {
            return;
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
                resolved: comment.resolved,
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
        self.clear_outdated_reconstruction_cache();
    }

    fn mark_or_remove_review_comment(&mut self, idx: usize) {
        if self.active_review_comment_id == Some(self.review_comments[idx].id) {
            self.active_review_comment_id = None;
        }
        let pending_reply = self.review_comments[idx]
            .provider
            .as_ref()
            .is_some_and(|provider| {
                provider.comment_id.is_empty() && provider.in_reply_to_id.is_some()
            });
        if pending_reply {
            self.review_comments.remove(idx);
            return;
        }
        // Keep tombstones for concurrent persistence and removed-comment history.
        self.review_comments[idx].deleted = true;
        self.review_comments[idx].updated_at = bumped_ts(self.review_comments[idx].updated_at);
        if let Some(provider) = self.review_comments[idx].provider.as_mut() {
            provider.sync_state = "deleted".to_string();
        }
    }

    fn deletable_review_reply_ids(&self, root_id: u64) -> Option<Vec<u64>> {
        let root = self
            .review_comments
            .iter()
            .find(|comment| comment.id == root_id && !comment.deleted && comment.can_edit)?;
        if review_comment_is_reply(root) {
            return Some(Vec::new());
        }

        let root_provider_scope = root.provider.as_ref().map(|provider| {
            (
                provider.provider.clone(),
                provider.repo.clone(),
                provider.pr_number,
            )
        });
        let mut parent_ids = FxHashSet::from_iter([root.id]);
        let mut parent_remote_ids = FxHashSet::default();
        if let Some(comment_id) = root
            .provider
            .as_ref()
            .map(|provider| provider.comment_id.as_str())
            .filter(|comment_id| !comment_id.is_empty())
        {
            parent_remote_ids.insert(comment_id.to_string());
        }
        let mut seen = parent_ids.clone();
        let mut deletable = Vec::new();
        loop {
            let mut found = false;
            for comment in &self.review_comments {
                if comment.deleted || seen.contains(&comment.id) {
                    continue;
                }
                let is_child = comment
                    .in_reply_to
                    .is_some_and(|parent_id| parent_ids.contains(&parent_id))
                    || comment.provider.as_ref().is_some_and(|provider| {
                        root_provider_scope.as_ref().is_some_and(
                            |(root_provider, root_repo, root_pr)| {
                                provider.provider == root_provider.as_str()
                                    && provider.repo == root_repo.as_str()
                                    && provider.pr_number == *root_pr
                            },
                        ) && provider
                            .in_reply_to_id
                            .as_ref()
                            .is_some_and(|parent_id| parent_remote_ids.contains(parent_id))
                    });
                if !is_child {
                    continue;
                }
                found = true;
                seen.insert(comment.id);
                parent_ids.insert(comment.id);
                if let Some(comment_id) = comment
                    .provider
                    .as_ref()
                    .map(|provider| provider.comment_id.as_str())
                    .filter(|comment_id| !comment_id.is_empty())
                {
                    parent_remote_ids.insert(comment_id.to_string());
                }
                if comment.can_edit {
                    deletable.push(comment.id);
                }
            }
            if !found {
                break;
            }
        }
        Some(deletable)
    }

    fn remove_review_comment_cascade(&mut self, id: u64) -> Option<usize> {
        let reply_ids = self.deletable_review_reply_ids(id)?;
        for delete_id in std::iter::once(id).chain(reply_ids.iter().copied()) {
            if let Some(idx) = self
                .review_comments
                .iter()
                .position(|comment| comment.id == delete_id && !comment.deleted)
            {
                self.mark_or_remove_review_comment(idx);
            }
        }
        self.touch_review_state();
        self.persist_review_session();
        Some(reply_ids.len())
    }

    fn delete_review_comment_with_feedback(&mut self, id: u64) -> bool {
        if self.remove_review_comment_cascade(id).is_none() {
            return false;
        }
        self.run_review_hooks(ReviewHookEvent::CommentDeleted, None);
        self.notify(ToastEvent::CommentDeleted);
        true
    }

    fn remove_comment_for_anchor_key(&mut self, anchor_key: &str) -> bool {
        let Some(id) = self
            .review_comments
            .iter()
            .find(|comment| {
                !comment.deleted && comment.can_edit && comment.anchor.anchor_key == anchor_key
            })
            .map(|comment| comment.id)
        else {
            return false;
        };
        self.request_delete_comment_by_id(id)
    }

    fn open_review_editor(&mut self, anchor: ReviewAnchor) {
        let existing = self
            .review_comments
            .iter()
            .find(|comment| !comment.deleted && comment.anchor.anchor_key == anchor.anchor_key);
        let id = existing.map(|comment| comment.id);
        let text = existing
            .map(|comment| comment.body.clone())
            .unwrap_or_default();
        self.open_review_editor_state(anchor, id, text);
    }

    fn open_review_editor_for_id(&mut self, id: u64) -> bool {
        let Some((anchor, text)) = self
            .review_comments
            .iter()
            .find(|comment| comment.id == id && !comment.deleted && comment.can_edit)
            .map(|comment| (comment.anchor.clone(), comment.body.clone()))
        else {
            return false;
        };
        self.open_review_editor_state(anchor, Some(id), text);
        true
    }

    fn open_review_editor_state(&mut self, anchor: ReviewAnchor, id: Option<u64>, text: String) {
        self.clear_diff_selection();
        self.active_review_comment_id = id;
        let cursor = text.len();
        self.review_editor = Some(ReviewEditorState {
            anchor,
            text,
            cursor,
            reply: None,
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
        let mut anchor =
            line_review_anchor_from_view_line(file_index, file_path, *display_idx, line)?;
        self.fill_review_anchor_snapshot(&mut anchor);
        Some(anchor)
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
        let mut anchor = line_review_anchor_from_view_line_with_side(
            file_index,
            file_path,
            display_idx,
            line,
            preferred_side,
        )?;
        self.fill_review_anchor_snapshot(&mut anchor);
        Some(anchor)
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

        let mut anchor = ReviewAnchor {
            file_index,
            file_path,
            kind: ReviewTargetKind::Hunk,
            side: None,
            old_range,
            new_range,
            hunk_id: Some(hunk_idx),
            display_idx_hint,
            anchor_key,
            snapshot: None,
        };
        self.fill_review_anchor_snapshot(&mut anchor);
        Some(anchor)
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

    fn scan_review_gc(
        conn: &Connection,
        review_key: Option<&str>,
        now: u64,
        grace_seconds: u64,
        prune_now: bool,
        dry_run: bool,
        bytes_before: u64,
    ) -> rusqlite::Result<ReviewGcResult> {
        let mut statement = conn.prepare(
            "SELECT review_key, id, comment_json, updated_at FROM comments ORDER BY review_key, id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut result = ReviewGcResult {
            reaped: Vec::new(),
            estimated_bytes: 0,
            bytes_before,
            bytes_after: bytes_before,
            held_pending_sync: 0,
            held_within_grace: 0,
            dry_run,
        };
        for row in rows {
            let (row_review_key, id, json, updated_at) = row?;
            if review_key.is_some_and(|key| key != row_review_key) || id < 0 {
                continue;
            }
            match review_gc_decision(&json, updated_at, now, grace_seconds, prune_now) {
                ReviewGcDecision::Reap => {
                    result.reaped.push(ReviewGcEntry {
                        review_key: row_review_key,
                        id: id as u64,
                    });
                    result.estimated_bytes = result
                        .estimated_bytes
                        .saturating_add(json.len().try_into().unwrap_or(u64::MAX));
                }
                ReviewGcDecision::KeepPendingSync => result.held_pending_sync += 1,
                ReviewGcDecision::KeepWithinGrace => result.held_within_grace += 1,
                ReviewGcDecision::Keep => {}
            }
        }
        Ok(result)
    }

    pub(crate) fn gc_review_tombstones(
        &self,
        all: bool,
        grace_days: u64,
        prune_now: bool,
        dry_run: bool,
    ) -> Result<ReviewGcResult, String> {
        let path = self
            .review_db_path
            .as_ref()
            .ok_or_else(|| "Review database is unavailable.".to_string())?;
        let bytes_before = fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let mut conn = Self::review_db(path).map_err(|error| error.to_string())?;
        let now = now_ts();
        let grace_seconds = grace_days.saturating_mul(24 * 60 * 60);
        let review_key = (!all).then(|| self.review_db_key());
        if dry_run {
            return Self::scan_review_gc(
                &conn,
                review_key,
                now,
                grace_seconds,
                prune_now,
                true,
                bytes_before,
            )
            .map_err(|error| error.to_string());
        }

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let mut result = Self::scan_review_gc(
            &tx,
            review_key,
            now,
            grace_seconds,
            prune_now,
            false,
            bytes_before,
        )
        .map_err(|error| error.to_string())?;
        for entry in &result.reaped {
            tx.execute(
                "DELETE FROM comments WHERE review_key = ?1 AND id = ?2",
                params![&entry.review_key, entry.id as i64],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        conn.execute_batch("VACUUM;")
            .map_err(|error| error.to_string())?;
        result.bytes_after = fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        Ok(result)
    }

    fn review_db(path: &Path) -> rusqlite::Result<Connection> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reviews (
                review_key TEXT PRIMARY KEY,
                repo_root TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                editor_json TEXT,
                target_json TEXT
            );
            CREATE TABLE IF NOT EXISTS comments (
                review_key TEXT NOT NULL,
                id INTEGER NOT NULL,
                comment_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (review_key, id)
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
        let Ok(tx) = conn.transaction_with_behavior(TransactionBehavior::Immediate) else {
            return;
        };
        let now = now_ts();
        let review_key = self.review_db_key().to_string();
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
                    review_key, repo_root, created_at, updated_at, editor_json, target_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(review_key) DO UPDATE SET
                    repo_root = excluded.repo_root,
                    updated_at = excluded.updated_at,
                    editor_json = excluded.editor_json,
                    target_json = excluded.target_json",
                params![
                    &review_key,
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
        let baseline = self
            .review_session_baseline
            .iter()
            .cloned()
            .map(|comment| (comment.id, comment))
            .collect::<BTreeMap<_, _>>();
        let current_ids = self
            .review_comments
            .iter()
            .map(|comment| comment.id)
            .collect::<BTreeSet<_>>();
        let removed_ids = baseline
            .keys()
            .copied()
            .filter(|id| !current_ids.contains(id))
            .collect::<Vec<_>>();
        let mut changed_indices = self
            .review_comments
            .iter()
            .enumerate()
            .filter(|(_, comment)| baseline.get(&comment.id).is_none_or(|old| old != *comment))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        let db_tombstones = {
            let Ok(mut statement) =
                tx.prepare("SELECT id, comment_json FROM comments WHERE review_key = ?1")
            else {
                return;
            };
            let Ok(rows) = statement.query_map(params![&review_key], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            }) else {
                return;
            };
            let mut tombstones = BTreeMap::new();
            for row in rows {
                let Ok((id, json)) = row else {
                    return;
                };
                let Ok(comment) = serde_json::from_str::<ReviewComment>(&json) else {
                    continue;
                };
                if id >= 0 && comment.deleted {
                    tombstones.insert(id as u64, comment);
                }
            }
            tombstones
        };
        let mut reconciled_tombstones = Vec::new();
        let mut reconciled_indices = BTreeSet::new();
        for (index, comment) in self.review_comments.iter_mut().enumerate() {
            if comment.deleted || !baseline.contains_key(&comment.id) {
                continue;
            }
            let Some(tombstone) = db_tombstones.get(&comment.id) else {
                continue;
            };
            *comment = tombstone.clone();
            reconciled_indices.insert(index);
            reconciled_tombstones.push(tombstone.clone());
        }
        changed_indices.retain(|index| !reconciled_indices.contains(index));

        let db_max_id = tx
            .query_row(
                "SELECT COALESCE(MAX(id), 0) FROM comments WHERE review_key = ?1",
                params![&review_key],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            .max(0) as u64;
        let mut used_ids = current_ids;
        let mut next_id = db_max_id
            .max(used_ids.iter().next_back().copied().unwrap_or(0))
            .saturating_add(1)
            .max(1);

        let mut id_remap = BTreeMap::new();
        for index in &changed_indices {
            let id = self.review_comments[*index].id;
            if baseline.contains_key(&id) {
                continue;
            }
            let disk_comment = match tx
                .query_row(
                    "SELECT comment_json FROM comments WHERE review_key = ?1 AND id = ?2",
                    params![&review_key, id as i64],
                    |row| row.get::<_, String>(0),
                )
                .optional()
            {
                Ok(Some(json)) => match serde_json::from_str::<ReviewComment>(&json) {
                    Ok(comment) => Some(comment),
                    Err(_) => return,
                },
                Ok(None) => None,
                Err(_) => return,
            };
            let Some(disk_comment) = disk_comment else {
                continue;
            };
            if review_comments_share_origin(&self.review_comments[*index], &disk_comment) {
                if disk_comment.deleted {
                    self.review_comments[*index] = disk_comment.clone();
                    reconciled_tombstones.push(disk_comment);
                }
                continue;
            }
            used_ids.remove(&id);
            while used_ids.contains(&next_id) {
                next_id = next_id.saturating_add(1);
            }
            self.review_comments[*index].id = next_id;
            id_remap.insert(id, next_id);
            used_ids.insert(next_id);
            next_id = next_id.saturating_add(1);
        }
        if !id_remap.is_empty() {
            let remapped_ids = id_remap.values().copied().collect::<BTreeSet<_>>();
            for index in 0..self.review_comments.len() {
                let mut parent_remapped = false;
                if let Some(parent_id) = self.review_comments[index].in_reply_to {
                    if let Some(new_parent_id) = id_remap.get(&parent_id).copied() {
                        self.review_comments[index].in_reply_to = Some(new_parent_id);
                        parent_remapped = true;
                    }
                }
                let own_id_remapped = remapped_ids.contains(&self.review_comments[index].id);
                if review_comment_is_reply(&self.review_comments[index])
                    && (parent_remapped || own_id_remapped)
                {
                    if let Some(parent_id) = self.review_comments[index].in_reply_to {
                        self.review_comments[index].anchor.anchor_key =
                            format!("reply|{parent_id}|{}", self.review_comments[index].id);
                    } else {
                        let parent_id = self.review_comments[index]
                            .provider
                            .as_ref()
                            .and_then(|provider| provider.in_reply_to_id.as_deref())
                            .map(str::to_owned)
                            .or_else(|| {
                                self.review_comments[index]
                                    .anchor
                                    .anchor_key
                                    .strip_prefix("reply|")
                                    .and_then(|key| key.rsplit_once('|'))
                                    .map(|(parent_id, _)| parent_id.to_string())
                            });
                        if let Some(parent_id) = parent_id {
                            self.review_comments[index].anchor.anchor_key =
                                format!("reply|{parent_id}|{}", self.review_comments[index].id);
                        }
                    }
                }
                if (parent_remapped || own_id_remapped) && !changed_indices.contains(&index) {
                    changed_indices.push(index);
                }
            }
        }

        let mut tombstoned_ids = db_tombstones
            .keys()
            .copied()
            .chain(
                self.review_comments
                    .iter()
                    .filter(|comment| comment.deleted)
                    .map(|comment| comment.id),
            )
            .collect::<BTreeSet<_>>();
        let mut tombstoned_remote_ids = db_tombstones
            .values()
            .chain(
                self.review_comments
                    .iter()
                    .filter(|comment| comment.deleted),
            )
            .filter_map(|comment| {
                let provider = comment.provider.as_ref()?;
                (!provider.comment_id.is_empty()).then(|| {
                    (
                        provider.provider.clone(),
                        provider.repo.clone(),
                        provider.pr_number,
                        provider.comment_id.clone(),
                    )
                })
            })
            .collect::<BTreeSet<_>>();
        loop {
            let mut found = false;
            for (index, comment) in self.review_comments.iter_mut().enumerate() {
                if comment.deleted || !comment.can_edit {
                    continue;
                }
                let parent_deleted = comment
                    .in_reply_to
                    .is_some_and(|parent_id| tombstoned_ids.contains(&parent_id))
                    || comment.provider.as_ref().is_some_and(|provider| {
                        provider.in_reply_to_id.as_ref().is_some_and(|parent_id| {
                            tombstoned_remote_ids.contains(&(
                                provider.provider.clone(),
                                provider.repo.clone(),
                                provider.pr_number,
                                parent_id.clone(),
                            ))
                        })
                    });
                if !parent_deleted {
                    continue;
                }
                comment.deleted = true;
                comment.updated_at = bumped_ts(comment.updated_at);
                if let Some(provider) = comment.provider.as_mut() {
                    provider.sync_state = "deleted".to_string();
                    if !provider.comment_id.is_empty() {
                        tombstoned_remote_ids.insert((
                            provider.provider.clone(),
                            provider.repo.clone(),
                            provider.pr_number,
                            provider.comment_id.clone(),
                        ));
                    }
                }
                tombstoned_ids.insert(comment.id);
                if !changed_indices.contains(&index) {
                    changed_indices.push(index);
                }
                reconciled_tombstones.push(comment.clone());
                found = true;
            }
            if !found {
                break;
            }
        }

        for id in removed_ids {
            if db_tombstones.contains_key(&id) {
                continue;
            }
            if tx
                .execute(
                    "DELETE FROM comments WHERE review_key = ?1 AND id = ?2",
                    params![&review_key, id as i64],
                )
                .is_err()
            {
                return;
            }
        }
        for index in changed_indices {
            let comment = &self.review_comments[index];
            let Ok(comment_json) = serde_json::to_string(comment) else {
                return;
            };
            if tx
                .execute(
                    "INSERT INTO comments (review_key, id, comment_json, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(review_key, id) DO UPDATE SET
                        comment_json = excluded.comment_json,
                        updated_at = excluded.updated_at",
                    params![
                        &review_key,
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
            if !reconciled_tombstones.is_empty() {
                self.touch_review_state();
            }
            self.review_session_baseline = self.review_comments.clone();
            self.review_next_comment_id = self
                .review_comments
                .iter()
                .map(|comment| comment.id)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
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
        let previous_comment_ids = self
            .review_comments
            .iter()
            .map(|comment| comment.id)
            .collect::<FxHashSet<_>>();
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
        if self.file_panel_mode != super::FilePanelMode::Comments
            && previous_comment_ids
                != self
                    .review_comments
                    .iter()
                    .map(|comment| comment.id)
                    .collect::<FxHashSet<_>>()
        {
            self.comments_tab_unseen = true;
        }
        self.touch_review_state();
        true
    }

    pub(crate) fn load_review_by_fingerprint(&mut self, fingerprint: &str) -> bool {
        self.review_storage_key = fingerprint.to_string();
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
        let current_storage_key = self.review_storage_key.clone();
        let current_metadata = self.review_target_metadata.clone();
        let mut comments = Vec::new();
        let mut seen = BTreeMap::new();
        let mut next_id = 1u64;
        for fingerprint in fingerprints {
            if !self.load_review_by_fingerprint(fingerprint) {
                continue;
            }
            let snapshot = self
                .review_comments
                .iter()
                .filter(|comment| !comment.deleted)
                .cloned()
                .collect::<Vec<_>>();
            let mut remap = BTreeMap::new();
            let mut reply_links = Vec::new();
            for mut comment in snapshot {
                let old_id = comment.id;
                let old_parent = comment.in_reply_to;
                let key = (
                    comment.anchor.anchor_key.clone(),
                    comment.body.clone(),
                    serde_json::to_string(&comment.provider).unwrap_or_default(),
                );
                let id = if let Some(id) = seen.get(&key).copied() {
                    id
                } else {
                    let id = next_id;
                    next_id = next_id.saturating_add(1);
                    comment.id = id;
                    seen.insert(key, id);
                    comments.push(comment);
                    id
                };
                remap.insert(old_id, id);
                if let Some(parent_id) = old_parent {
                    reply_links.push((id, parent_id));
                }
            }
            for (id, old_parent_id) in reply_links {
                if let Some(comment) = comments.iter_mut().find(|comment| comment.id == id) {
                    comment.in_reply_to = remap.get(&old_parent_id).copied();
                }
            }
        }
        self.review_diff_fingerprint = current_fingerprint;
        self.review_storage_key = current_storage_key;
        self.review_target_metadata = current_metadata;
        if comments.is_empty() {
            return false;
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
        let review_key = self.review_db_key().to_string();
        let row = conn
            .query_row(
                "SELECT created_at, editor_json, target_json FROM reviews WHERE review_key = ?1",
                params![&review_key],
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
            .prepare("SELECT comment_json FROM comments WHERE review_key = ?1 ORDER BY id")
        {
            Ok(stmt) => stmt,
            Err(_) => return false,
        };
        let rows = match stmt.query_map(params![&review_key], |row| row.get::<_, String>(0)) {
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
        self.public_review_comments_json_filtered(&ReviewCommentFilter::default())
    }

    fn public_review_comments_json_filtered(&self, filter: &ReviewCommentFilter) -> String {
        let comments = self
            .review_comments
            .iter()
            .filter(|comment| include_review_comment(comment, filter))
            .map(|comment| PublicReviewComment {
                id: Some(comment.id),
                change_type: review_comment_change_type(comment, filter.since),
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
                new_range: comment.anchor.new_range,
                hunk_id: comment.anchor.hunk_id,
                anchor_snapshot: comment.anchor.snapshot.clone(),
                author: comment.author.clone(),
                can_edit: Some(comment.can_edit),
                resolved: comment.resolved,
                outdated: comment.outdated,
                reanchored: comment.reanchored,
                deleted: comment.deleted,
                provider: comment.provider.clone(),
                in_reply_to: comment.in_reply_to,
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
            "review" | "pr" | "pullRequest" => ReviewTargetKind::PullRequest,
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
        let old_range = item.old_range;
        let new_range = item.new_range;
        let hunk_id = item.hunk_id;
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
        } else if let Some(parent_id) = item.in_reply_to {
            anchor_key = format!("reply|{parent_id}|{}", item.id.unwrap_or(0));
        }
        let now = now_ts();
        let mut anchor = ReviewAnchor {
            file_index,
            file_path: item.file,
            kind,
            side,
            old_range,
            new_range,
            hunk_id,
            display_idx_hint: None,
            anchor_key,
            snapshot: item.anchor_snapshot,
        };
        self.fill_review_anchor_snapshot(&mut anchor);
        Ok(ReviewComment {
            id: item.id.unwrap_or(0),
            anchor,
            body: item.body,
            author: item.author.or_else(|| self.review_author.clone()),
            can_edit: item.can_edit.unwrap_or(true),
            resolved: item.resolved,
            outdated: item.outdated,
            reanchored: item.reanchored,
            deleted: item.deleted,
            provider: item.provider,
            in_reply_to: item.in_reply_to,
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
            change_type: None,
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
            new_range,
            hunk_id: None,
            anchor_snapshot: None,
            author: None,
            can_edit: None,
            resolved: false,
            outdated: false,
            reanchored: false,
            deleted: false,
            provider: None,
            in_reply_to: None,
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
        let index = self.review_comments.len();
        self.review_comments.push(comment);
        self.touch_review_state();
        self.persist_review_session();
        Ok(self
            .review_comments
            .get(index)
            .map(|comment| comment.id)
            .unwrap_or(id))
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
        comment.updated_at = bumped_ts(comment.updated_at);
        self.touch_review_state();
        self.persist_review_session();
        true
    }

    pub fn review_comment_delete_ids(&self, id: u64) -> Option<Vec<u64>> {
        self.deletable_review_reply_ids(id)
            .map(|reply_ids| std::iter::once(id).chain(reply_ids).collect())
    }

    pub fn review_comment_delete_reply_count(&self, id: u64) -> Option<usize> {
        self.review_comment_delete_ids(id)
            .map(|ids| ids.len().saturating_sub(1))
    }

    pub fn remove_review_comment_from_cli(&mut self, id: u64) -> bool {
        self.remove_review_comment_cascade(id).is_some()
    }

    pub fn review_comment_is_reply_id(&self, id: u64) -> bool {
        self.review_comments
            .iter()
            .find(|comment| comment.id == id && !comment.deleted)
            .is_some_and(review_comment_is_reply)
    }

    pub fn set_review_comment_resolved_from_cli(&mut self, id: u64, resolved: bool) -> bool {
        let Some(idx) = self.review_comments.iter().position(|comment| {
            comment.id == id && !comment.deleted && !review_comment_is_reply(comment)
        }) else {
            return false;
        };
        self.set_review_comment_resolved_at(idx, resolved)
    }

    pub(crate) fn review_comments_for_sync(&mut self) -> Vec<ReviewComment> {
        let mut comments = self.review_comments.clone();
        let previous_file = self.multi_diff.selected_index;
        for comment in &mut comments {
            if comment.anchor.kind != ReviewTargetKind::Line {
                continue;
            }
            let target = match comment.anchor.side {
                Some(ReviewSide::Old) => comment
                    .anchor
                    .old_range
                    .map(|range| (ReviewSide::Old, range.end)),
                Some(ReviewSide::New) => comment
                    .anchor
                    .new_range
                    .map(|range| (ReviewSide::New, range.end)),
                None => comment
                    .anchor
                    .new_range
                    .map(|range| (ReviewSide::New, range.end))
                    .or_else(|| {
                        comment
                            .anchor
                            .old_range
                            .map(|range| (ReviewSide::Old, range.end))
                    }),
            };
            let Some((side, line)) = target else {
                continue;
            };
            let strip_other_side = |comment: &mut ReviewComment| match side {
                ReviewSide::Old => comment.anchor.new_range = None,
                ReviewSide::New => comment.anchor.old_range = None,
            };
            let Some(file_index) = self
                .multi_diff
                .files
                .iter()
                .position(|file| file.path == Path::new(&comment.anchor.file_path))
            else {
                strip_other_side(comment);
                continue;
            };
            let status = self.multi_diff.diff_status(file_index);
            if status == oyo_core::multi::DiffStatus::Loading {
                continue;
            }
            if status != oyo_core::multi::DiffStatus::Ready {
                strip_other_side(comment);
                continue;
            }
            self.multi_diff.select_file(file_index);
            if self.multi_diff.current_navigator_is_placeholder() {
                strip_other_side(comment);
                continue;
            }
            let mapping = self
                .multi_diff
                .current_navigator()
                .diff()
                .changes
                .iter()
                .find_map(|change| {
                    let matching_span = change.spans.iter().find(|span| match side {
                        ReviewSide::Old => span.old_line == Some(line),
                        ReviewSide::New => span.new_line == Some(line),
                    })?;
                    if change.has_changes() {
                        return Some(None);
                    }
                    Some(Some((matching_span.old_line?, matching_span.new_line?)))
                });
            match mapping {
                Some(Some((old_line, new_line))) => {
                    comment.anchor.old_range = Some(ReviewRange {
                        start: old_line,
                        end: old_line,
                    });
                    comment.anchor.new_range = Some(ReviewRange {
                        start: new_line,
                        end: new_line,
                    });
                }
                _ => strip_other_side(comment),
            }
        }
        self.multi_diff.select_file(previous_file);
        comments
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

    pub(crate) fn mark_review_thread_synced(
        &mut self,
        provider_id: &str,
        repo: &str,
        pr_number: u64,
        thread_id: &str,
        resolved: bool,
        comment_states: &[(u64, bool)],
    ) -> Vec<u64> {
        let mut changed = Vec::new();
        for comment in &mut self.review_comments {
            let matches = comment.provider.as_ref().is_some_and(|provider| {
                provider.provider == provider_id
                    && provider.repo == repo
                    && provider.pr_number == pr_number
                    && provider.api_kind == "review"
                    && provider.thread_id.as_deref() == Some(thread_id)
            });
            if !matches {
                continue;
            }
            if comment_states
                .iter()
                .find(|(id, _)| *id == comment.id)
                .is_some_and(|(_, snapshot)| *snapshot == comment.resolved)
            {
                comment.resolved = resolved;
            }
            if let Some(provider) = comment.provider.as_mut() {
                provider.thread_resolved = Some(resolved);
                provider.resolved_dirty = comment.resolved != resolved;
            }
            changed.push(comment.id);
        }
        if !changed.is_empty() {
            self.touch_review_state();
            self.persist_review_session();
        }
        changed
    }

    pub(crate) fn canonicalize_review_provider_repo(
        &mut self,
        provider_id: &str,
        pr_number: u64,
        repo: &str,
    ) -> bool {
        let mut changed = false;
        for comment in &mut self.review_comments {
            let Some(provider) = comment.provider.as_mut() else {
                continue;
            };
            if provider.provider == provider_id
                && provider.pr_number == pr_number
                && provider.repo != repo
            {
                provider.repo = repo.to_string();
                changed = true;
            }
        }
        if changed {
            self.touch_review_state();
            self.persist_review_session();
        }
        changed
    }

    pub(crate) fn upsert_provider_review_comment(&mut self, mut comment: ReviewComment) -> u64 {
        let incoming_thread = comment.provider.as_ref().and_then(|provider| {
            Some((
                provider.provider.clone(),
                provider.repo.clone(),
                provider.pr_number,
                provider.thread_id.clone()?,
                provider.thread_resolved?,
            ))
        });
        if let Some((provider_id, repo, pr_number, thread_id, remote_resolved)) = incoming_thread {
            for existing in &mut self.review_comments {
                let Some(provider) = existing.provider.as_mut() else {
                    continue;
                };
                if !provider.comment_id.is_empty()
                    || provider.in_reply_to_id.is_none()
                    || provider.provider != provider_id
                    || provider.repo != repo
                    || provider.pr_number != pr_number
                    || provider.thread_id.as_deref() != Some(thread_id.as_str())
                {
                    continue;
                }
                if !provider.resolved_dirty {
                    existing.resolved = remote_resolved;
                }
                provider.thread_resolved = Some(remote_resolved);
                provider.resolved_dirty = existing.resolved != remote_resolved;
            }
        }

        let provider_comment_id = comment.provider.as_ref().map(|provider| {
            (
                provider.provider.clone(),
                provider.repo.clone(),
                provider.pr_number,
                provider.api_kind.clone(),
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
                        provider.api_kind.clone(),
                        provider.comment_id.clone(),
                    )
                }) == Some(provider_comment_id.clone())
            }) {
                let id = existing.id;
                let local_resolution_change = existing.provider.as_ref().is_some_and(|provider| {
                    provider.resolved_dirty
                        || provider
                            .thread_resolved
                            .is_some_and(|resolved| resolved != existing.resolved)
                });
                let local_content_change = existing.deleted
                    || existing.provider.as_ref().is_some_and(|provider| {
                        matches!(provider.sync_state.as_str(), "dirty" | "deleted")
                    });
                if local_content_change {
                    if let Some(incoming) = comment.provider.as_ref().filter(|provider| {
                        provider.api_kind == "review" && provider.thread_resolved.is_some()
                    }) {
                        if !local_resolution_change {
                            existing.resolved = comment.resolved;
                        }
                        if let Some(provider) = existing.provider.as_mut() {
                            provider.thread_id.clone_from(&incoming.thread_id);
                            provider.thread_resolved = incoming.thread_resolved;
                            provider.resolved_dirty = incoming
                                .thread_resolved
                                .is_some_and(|remote| remote != existing.resolved);
                        }
                    }
                    self.touch_review_state();
                    self.persist_review_session();
                    return id;
                }
                comment.id = id;
                if local_resolution_change {
                    comment.resolved = existing.resolved;
                    if let Some(provider) = comment.provider.as_mut() {
                        provider.resolved_dirty = provider
                            .thread_resolved
                            .is_none_or(|remote| remote != existing.resolved);
                    }
                }
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
        let review_key = self.review_db_key().to_string();
        let removed = self
            .review_db_path
            .as_ref()
            .and_then(|path| Self::review_db(path).ok())
            .map(|conn| {
                let comments = conn
                    .execute(
                        "DELETE FROM comments WHERE review_key = ?1",
                        params![&review_key],
                    )
                    .unwrap_or(0);
                let reviews = conn
                    .execute(
                        "DELETE FROM reviews WHERE review_key = ?1",
                        params![&review_key],
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
        let mut paths = std::collections::HashMap::new();
        for (index, file) in self.multi_diff.files.iter().enumerate() {
            paths.insert(file.display_name.clone(), index);
            paths.insert(file.path.to_string_lossy().to_string(), index);
        }
        paths
    }

    fn filter_review_comments_to_current_diff(&mut self) {
        let paths = self.current_diff_file_indexes();
        self.review_comments.retain(|comment| {
            comment.deleted
                || comment.anchor.snapshot.is_some()
                || (comment.anchor.kind != ReviewTargetKind::PullRequest
                    && paths.contains_key(&comment.anchor.file_path))
        });
    }

    fn display_line_for_anchor_position(
        &mut self,
        file_index: usize,
        side: ReviewSide,
        line_number: usize,
    ) -> Option<(usize, Option<usize>)> {
        let previous_index = self.multi_diff.selected_index;
        self.multi_diff.selected_index = file_index;
        let visible = self.review_visible_lines_with_idx();
        self.multi_diff.selected_index = previous_index;
        visible.into_iter().find_map(|(idx, line)| {
            let line_matches = match side {
                ReviewSide::Old => line.old_line == Some(line_number),
                ReviewSide::New => line.new_line == Some(line_number),
            };
            line_matches.then_some((idx, line.hunk_index))
        })
    }

    fn reconcile_review_comment_anchor(
        &mut self,
        comment_idx: usize,
        file_index: Option<usize>,
    ) -> bool {
        if review_comment_is_reply(&self.review_comments[comment_idx]) {
            return false;
        }
        let Some(snapshot) = self.review_comments[comment_idx].anchor.snapshot.clone() else {
            return false;
        };
        if !matches!(
            self.review_comments[comment_idx].anchor.kind,
            ReviewTargetKind::Line | ReviewTargetKind::Hunk
        ) || self.review_comments[comment_idx].deleted
        {
            return false;
        }
        let Some(file_index) = file_index else {
            if self.review_comments[comment_idx].resolved
                || self.review_comments[comment_idx].outdated
            {
                return false;
            }
            self.review_comments[comment_idx].outdated = true;
            return true;
        };
        let Some(side) = snapshot_review_side(&snapshot) else {
            return false;
        };
        let Some((old_content, new_content)) = self.multi_diff.file_contents(file_index) else {
            return false;
        };
        let content = match side {
            ReviewSide::Old => old_content,
            ReviewSide::New => new_content,
        };
        let current_line = anchor_line_number(&self.review_comments[comment_idx].anchor, side)
            .or(Some(snapshot.line_number));
        let Some(candidate) = best_snapshot_line_match(content, &snapshot, current_line) else {
            if self.review_comments[comment_idx].outdated {
                return false;
            }
            self.review_comments[comment_idx].outdated = true;
            return true;
        };

        let display =
            self.display_line_for_anchor_position(file_index, side, candidate.line_number);
        let provider = self.review_comments[comment_idx].provider.clone();
        let moved;
        let mut changed = false;
        {
            let comment = &mut self.review_comments[comment_idx];
            if comment.outdated {
                comment.outdated = false;
                changed = true;
            }
            let anchor = &mut comment.anchor;
            let old_line = anchor_line_number(anchor, side);
            moved = old_line != Some(candidate.line_number);
            if anchor.file_index != file_index {
                anchor.file_index = file_index;
                changed = true;
            }
            if moved {
                match side {
                    ReviewSide::Old => {
                        anchor.old_range =
                            Some(shifted_range(anchor.old_range, candidate.line_number));
                    }
                    ReviewSide::New => {
                        anchor.new_range =
                            Some(shifted_range(anchor.new_range, candidate.line_number));
                    }
                }
                changed = true;
            }
            if let Some((display_idx, hunk_id)) = display {
                if anchor.display_idx_hint != Some(display_idx) {
                    anchor.display_idx_hint = Some(display_idx);
                    changed = true;
                }
                if anchor.hunk_id != hunk_id {
                    anchor.hunk_id = hunk_id;
                    changed = true;
                }
            }
            let new_key = rebuild_review_anchor_key(anchor, provider.as_ref());
            if anchor.anchor_key != new_key {
                anchor.anchor_key = new_key;
                changed = true;
            }
        }
        if moved && !self.review_comments[comment_idx].reanchored {
            self.review_comments[comment_idx].reanchored = true;
            changed = true;
        }
        changed
    }

    pub(crate) fn repair_review_comments_after_diff_refresh(&mut self) -> bool {
        if !self.review_mode || !self.repair_review_comment_file_indexes() {
            return false;
        }
        self.persist_review_session();
        self.touch_review_state();
        true
    }

    fn review_thread_root_comment_id(&self, comment_id: u64) -> Option<u64> {
        let comment = self
            .review_comments
            .iter()
            .find(|comment| comment.id == comment_id)?;
        let root_id = local_thread_root_id(comment, &self.review_comments);
        if root_id != comment_id {
            return Some(root_id);
        }
        let key = review_thread_key(comment, &self.review_comments)?;
        self.review_comments
            .iter()
            .find(|candidate| {
                !candidate.deleted
                    && !review_comment_is_reply(candidate)
                    && review_thread_key(candidate, &self.review_comments).as_ref() == Some(&key)
            })
            .map(|root| root.id)
            .or(Some(comment_id))
    }

    fn propagate_review_thread_root_anchors(&mut self) -> bool {
        let updates = self
            .review_comments
            .iter()
            .enumerate()
            .filter(|(_, comment)| !comment.deleted && review_comment_is_reply(comment))
            .filter_map(|(index, comment)| {
                let root_id = self.review_thread_root_comment_id(comment.id)?;
                let root = self
                    .review_comments
                    .iter()
                    .find(|candidate| !candidate.deleted && candidate.id == root_id)?;
                Some((index, root.anchor.clone(), root.outdated, root.reanchored))
            })
            .collect::<Vec<_>>();
        let mut changed = false;
        for (index, root_anchor, outdated, reanchored) in updates {
            let reply = &mut self.review_comments[index];
            let anchor = &mut reply.anchor;
            let needs_update = anchor.file_index != root_anchor.file_index
                || anchor.file_path != root_anchor.file_path
                || anchor.kind != root_anchor.kind
                || anchor.side != root_anchor.side
                || anchor.old_range != root_anchor.old_range
                || anchor.new_range != root_anchor.new_range
                || anchor.hunk_id != root_anchor.hunk_id
                || anchor.display_idx_hint != root_anchor.display_idx_hint
                || anchor.anchor_key != root_anchor.anchor_key
                || reply.outdated != outdated
                || reply.reanchored != reanchored;
            if !needs_update {
                continue;
            }
            anchor.file_index = root_anchor.file_index;
            anchor.file_path = root_anchor.file_path;
            anchor.kind = root_anchor.kind;
            anchor.side = root_anchor.side;
            anchor.old_range = root_anchor.old_range;
            anchor.new_range = root_anchor.new_range;
            anchor.hunk_id = root_anchor.hunk_id;
            anchor.display_idx_hint = root_anchor.display_idx_hint;
            anchor.anchor_key = root_anchor.anchor_key;
            reply.outdated = outdated;
            reply.reanchored = reanchored;
            changed = true;
        }
        changed
    }

    fn repair_review_comment_file_indexes(&mut self) -> bool {
        let paths = self.current_diff_file_indexes();
        let mut changed = false;
        for idx in 0..self.review_comments.len() {
            let path = self.review_comments[idx].anchor.file_path.clone();
            let file_index = paths.get(&path).copied();
            if let Some(index) = file_index {
                if self.review_comments[idx].anchor.file_index != index {
                    self.review_comments[idx].anchor.file_index = index;
                    changed = true;
                }
            }
            changed |= self.reconcile_review_comment_anchor(idx, file_index);
        }
        changed | self.propagate_review_thread_root_anchors()
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

    fn review_db_key(&self) -> &str {
        if self.review_storage_key.is_empty() {
            &self.review_diff_fingerprint
        } else {
            &self.review_storage_key
        }
    }

    fn compute_review_storage_key(&self, repo_root: &Path, diff_fingerprint: &str) -> String {
        Self::review_storage_key_for_metadata(
            &repo_root.to_string_lossy(),
            diff_fingerprint,
            self.review_target_metadata.as_ref(),
        )
    }

    fn review_storage_key_for_metadata(
        repo_root: &str,
        diff_fingerprint: &str,
        metadata: Option<&ReviewTargetMetadata>,
    ) -> String {
        let root_key = hash_hex(repo_root);
        let Some(metadata) = metadata else {
            return format!("diff:{diff_fingerprint}");
        };
        if let (Some(provider), Some(repo), Some(number)) = (
            metadata.pr_provider.as_ref(),
            metadata.pr_repo.as_ref(),
            metadata.pr_number,
        ) {
            return format!("pr:{root_key}:{}:{}:{number}", provider, hash_hex(repo));
        }
        match metadata.vcs.as_str() {
            "jj" => {
                if !metadata.label.contains("..") {
                    if let Some(change_id) = metadata
                        .jj_change_id
                        .as_ref()
                        .filter(|value| !value.trim().is_empty())
                    {
                        return format!("jj:change:{root_key}:{change_id}");
                    }
                }
                let mut change_ids = metadata.jj_change_ids.clone().unwrap_or_default();
                change_ids.retain(|value| !value.trim().is_empty());
                change_ids.sort();
                change_ids.dedup();
                let spec = if change_ids.is_empty() {
                    metadata.label.clone()
                } else {
                    serde_json::to_string(&change_ids).unwrap_or_else(|_| metadata.label.clone())
                };
                format!("jj:target:{root_key}:{}", hash_hex(&spec))
            }
            "git" => match metadata.label.as_str() {
                "@" => format!("git:worktree:{root_key}"),
                "staged" => format!("git:staged:{root_key}"),
                label
                    if metadata.git_base_ref.as_deref() == Some("HEAD")
                        && metadata.git_head_ref.is_none()
                        && metadata.git_head_commit.is_none() =>
                {
                    format!("git:file:{root_key}:{}", hash_hex(label))
                }
                _ => {
                    if let Some(branch) = metadata.branch.as_ref().filter(|value| {
                        !value.trim().is_empty()
                            && metadata.git_head_ref.as_deref() == Some(value.as_str())
                    }) {
                        return format!("git:branch:{root_key}:{}", hash_hex(branch));
                    }
                    let spec =
                        serde_json::to_string(metadata).unwrap_or_else(|_| metadata.label.clone());
                    format!("git:target:{root_key}:{}", hash_hex(&spec))
                }
            },
            _ => format!("diff:{diff_fingerprint}"),
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
        self.format_review_output_filtered(&ReviewCommentFilter::default())
    }

    fn format_review_output_filtered(&self, filter: &ReviewCommentFilter) -> String {
        self.format_review_output_filtered_colored(filter, false)
    }

    fn format_review_output_filtered_colored(
        &self,
        filter: &ReviewCommentFilter,
        color: bool,
    ) -> String {
        let mut comments = self
            .review_comments
            .iter()
            .filter(|comment| include_review_comment(comment, filter))
            .cloned()
            .collect::<Vec<_>>();
        comments.sort_by(review_comment_document_cmp);

        comments
            .iter()
            .map(|comment| Self::format_review_comment(comment, color))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn format_review_comment(comment: &ReviewComment, color: bool) -> String {
        let anchor = &comment.anchor;
        let id = crate::review_cli_paint(color, "1;38;5;8", &format!("#{}", comment.id));
        let file = crate::review_cli_paint(color, "2", &anchor.file_path);
        let location = crate::review_cli_paint(color, "2", &review_anchor_location_label(anchor));
        let base_status = if comment.deleted {
            crate::review_cli_paint(color, "2", "removed")
        } else if comment.resolved {
            crate::review_cli_paint(color, "32", "resolved")
        } else {
            crate::review_cli_paint(color, "33", "unresolved")
        };
        let status = if comment.outdated && !comment.deleted {
            format!(
                "{} {}",
                base_status,
                crate::review_cli_paint(color, "2;33", "(outdated)")
            )
        } else {
            base_status
        };
        let mut lines = vec![format!("ID: {id}"), format!("File: {file}")];

        lines.push(format!("Location: {location}"));
        lines.push(format!("Status: {status}"));
        if let Some(author) = &comment.author {
            let author_label = match &author.email {
                Some(email) if !email.trim().is_empty() => format!("{} <{}>", author.name, email),
                _ => author.name.clone(),
            };
            let mut line = format!(
                "{} {author_label}",
                crate::review_cli_paint(color, "2", "Author:")
            );
            if let Some(author_type) = review_author_type_label(author) {
                line.push(' ');
                line.push_str(&crate::review_cli_paint(
                    color,
                    "35",
                    &format!("({author_type})"),
                ));
            }
            lines.push(line);
        }

        lines.push(crate::review_cli_paint(color, "2", "Body:"));
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
    use std::path::{Path, PathBuf};

    #[test]
    fn sync_keeps_modified_lines_on_the_declared_side() {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("app.py"),
            PathBuf::from("app.py"),
            "before\n".to_string(),
            "after\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.add_review_comment_from_cli(
            "app.py",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 1, end: 1 }),
            "modified".to_string(),
        )
        .unwrap();
        app.review_comments[0].anchor.old_range = Some(ReviewRange { start: 1, end: 1 });

        let comments = app.review_comments_for_sync();
        assert_eq!(comments[0].anchor.old_range, None);
        assert_eq!(
            comments[0].anchor.new_range,
            Some(ReviewRange { start: 1, end: 1 })
        );
    }

    #[test]
    fn sync_does_not_infer_context_from_non_ready_diff() {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("app.py"),
            PathBuf::from("app.py"),
            "base\n".to_string(),
            "base\nadded\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.add_review_comment_from_cli(
            "app.py",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 2, end: 2 }),
            "added".to_string(),
        )
        .unwrap();
        app.review_comments[0].anchor.old_range = Some(ReviewRange { start: 2, end: 2 });
        app.multi_diff.mark_diff_computing(0);

        let comments = app.review_comments_for_sync();
        assert_eq!(comments[0].anchor.old_range, None);
        assert_eq!(
            comments[0].anchor.new_range,
            Some(ReviewRange { start: 2, end: 2 })
        );
    }

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

    fn persistent_test_app(base: &Path) -> App {
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
        app.set_review_base_dir(Some(base.to_path_buf()));
        app
    }

    fn add_line_comment(app: &mut App, body: &str) -> u64 {
        app.add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 1, end: 1 }),
            body.to_string(),
        )
        .unwrap()
    }

    fn test_metadata(label: &str, vcs: &str) -> ReviewTargetMetadata {
        ReviewTargetMetadata {
            label: label.to_string(),
            vcs: vcs.to_string(),
            jj_change_id: None,
            jj_change_ids: None,
            jj_commit_id: None,
            git_base_ref: None,
            git_head_ref: None,
            git_base_commit: None,
            git_head_commit: None,
            branch: None,
            pr_provider: None,
            pr_repo: None,
            pr_number: None,
            author: None,
            timestamp: None,
            bookmarks: None,
        }
    }

    #[test]
    fn review_storage_key_is_stable_for_jj_change() {
        let mut metadata = test_metadata("@", "jj");
        metadata.jj_change_id = Some("abc123".to_string());
        metadata.jj_commit_id = Some("oldcommit".to_string());
        let old = App::review_storage_key_for_metadata("/tmp/repo", "oldfp", Some(&metadata));
        metadata.jj_commit_id = Some("newcommit".to_string());
        let new = App::review_storage_key_for_metadata("/tmp/repo", "newfp", Some(&metadata));
        assert_eq!(old, new);
        assert!(old.contains("abc123"));
    }

    #[test]
    fn review_storage_key_is_stable_for_equivalent_jj_ranges_and_amends() {
        let mut metadata = test_metadata("main..feature", "jj");
        metadata.jj_change_id = Some("head1234".to_string());
        metadata.jj_change_ids = Some(vec!["change-b".to_string(), "change-a".to_string()]);
        metadata.jj_commit_id = Some("old-commit".to_string());
        metadata.timestamp = Some("old-time".to_string());
        metadata.bookmarks = Some("feature".to_string());
        let old = App::review_storage_key_for_metadata("/tmp/repo", "oldfp", Some(&metadata));

        metadata.label = "trunk()..feature".to_string();
        metadata.jj_change_ids = Some(vec!["change-a".to_string(), "change-b".to_string()]);
        metadata.jj_commit_id = Some("new-commit".to_string());
        metadata.timestamp = Some("new-time".to_string());
        metadata.bookmarks = Some("feature other".to_string());
        let amended = App::review_storage_key_for_metadata("/tmp/repo", "newfp", Some(&metadata));
        assert_eq!(old, amended);
        assert!(old.starts_with("jj:target:"));

        metadata.jj_change_ids = Some(vec!["change-a".to_string(), "change-c".to_string()]);
        let different = App::review_storage_key_for_metadata("/tmp/repo", "newfp", Some(&metadata));
        assert_ne!(old, different);
    }

    #[test]
    fn review_storage_key_jj_range_fallback_ignores_moving_metadata() {
        let mut metadata = test_metadata("main..feature", "jj");
        metadata.jj_commit_id = Some("old-commit".to_string());
        let old = App::review_storage_key_for_metadata("/tmp/repo", "oldfp", Some(&metadata));
        metadata.jj_commit_id = Some("new-commit".to_string());
        let amended = App::review_storage_key_for_metadata("/tmp/repo", "newfp", Some(&metadata));
        assert_eq!(old, amended);
    }

    #[test]
    fn review_storage_key_stays_on_branch_when_pull_adds_pr_metadata() {
        let mut app = test_app();
        app.set_review_workspace_root(Some(PathBuf::from("/tmp/repo")));
        app.set_review_persist_enabled(false);
        let mut branch = test_metadata("main...feature", "git");
        branch.git_base_ref = Some("main".to_string());
        branch.git_head_ref = Some("feature".to_string());
        branch.git_base_commit = Some("base".to_string());
        branch.git_head_commit = Some("head".to_string());
        branch.branch = Some("feature".to_string());
        app.set_review_target_metadata(Some(branch.clone()));
        app.enable_review_mode();
        let key = app.review_storage_key().to_string();

        branch.label = "github#7".to_string();
        branch.pr_provider = Some("github".to_string());
        branch.pr_repo = Some("owner/repo".to_string());
        branch.pr_number = Some(7);
        app.set_review_target_metadata(Some(branch));

        assert_eq!(app.review_storage_key(), key);
        assert!(key.starts_with("git:branch:"));
    }

    #[test]
    fn pr_target_metadata_persists_without_comments() {
        let root = temp_path("pr-target-metadata");
        let mut branch = test_metadata("main...feature", "git");
        branch.git_base_ref = Some("main".to_string());
        branch.git_head_ref = Some("feature".to_string());
        branch.branch = Some("feature".to_string());
        let mut app = persistent_test_app(&root);
        app.set_review_target_metadata(Some(branch.clone()));
        app.enable_review_mode();

        branch.label = "github#7".to_string();
        branch.pr_provider = Some("github".to_string());
        branch.pr_repo = Some("owner/repo".to_string());
        branch.pr_number = Some(7);
        app.set_review_target_metadata(Some(branch.clone()));
        drop(app);

        let mut loaded = persistent_test_app(&root);
        let mut local = branch.clone();
        local.label = "main...feature".to_string();
        local.pr_provider = None;
        local.pr_repo = None;
        local.pr_number = None;
        loaded.set_review_target_metadata(Some(local));
        loaded.load_review_mode();

        assert_eq!(loaded.review_target_metadata(), Some(&branch));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn review_storage_key_is_stable_for_git_worktree() {
        let mut metadata = test_metadata("@", "git");
        metadata.git_base_commit = Some("oldhead".to_string());
        let old = App::review_storage_key_for_metadata("/tmp/repo", "oldfp", Some(&metadata));
        metadata.git_base_commit = Some("newhead".to_string());
        let new = App::review_storage_key_for_metadata("/tmp/repo", "newfp", Some(&metadata));
        assert_eq!(old, new);
        assert!(old.starts_with("git:worktree:"));
    }

    fn provider_link(state: &str) -> ReviewProviderComment {
        ReviewProviderComment {
            provider: "github".to_string(),
            remote: "origin".to_string(),
            repo: "owner/repo".to_string(),
            pr_number: 1,
            comment_id: "10".to_string(),
            in_reply_to_id: None,
            thread_id: None,
            thread_resolved: None,
            resolved_dirty: false,
            author_username: Some("reviewer".to_string()),
            pr_title: Some("PR".to_string()),
            pr_url: Some("https://example.com/owner/repo/pulls/1".to_string()),
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
                snapshot: None,
            },
            body: "please fix".to_string(),
            author: None,
            can_edit: true,
            resolved: false,
            outdated: false,
            reanchored: false,
            deleted: false,
            provider: None,
            in_reply_to: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn review_gc_predicate_keeps_live_recent_and_pending_rows() {
        let day = 24 * 60 * 60;
        let now = 20 * day;
        let grace = 14 * day;
        let mut comment = line_comment();
        comment.deleted = true;
        comment.updated_at = now - grace;
        let json = serde_json::to_string(&comment).unwrap();
        assert_eq!(
            review_gc_decision(&json, comment.updated_at as i64, now, grace, false),
            ReviewGcDecision::Reap
        );

        comment.updated_at += 1;
        let json = serde_json::to_string(&comment).unwrap();
        assert_eq!(
            review_gc_decision(&json, comment.updated_at as i64, now, grace, false),
            ReviewGcDecision::KeepWithinGrace
        );

        comment.provider = Some(provider_link("deleted"));
        let json = serde_json::to_string(&comment).unwrap();
        assert_eq!(
            review_gc_decision(&json, comment.updated_at as i64, now, grace, true),
            ReviewGcDecision::KeepPendingSync
        );

        comment.deleted = false;
        comment.provider = None;
        let json = serde_json::to_string(&comment).unwrap();
        assert_eq!(
            review_gc_decision(&json, 1, now, grace, true),
            ReviewGcDecision::Keep
        );
    }

    #[test]
    fn review_gc_dry_run_reaps_once_and_vacuums_without_touching_live_rows() {
        let root = temp_path("review-gc");
        let mut app = persistent_test_app(&root);
        app.enable_review_mode();
        let deleted_id = add_line_comment(&mut app, &"x".repeat(300_000));
        let live_id = add_line_comment(&mut app, "keep me");
        assert!(app.remove_review_comment_from_cli(deleted_id));
        let path = app.review_paths().db_file.unwrap();
        let bytes_with_tombstone = fs::metadata(&path).unwrap().len();

        let dry_run = app.gc_review_tombstones(false, 14, true, true).unwrap();
        assert_eq!(dry_run.reaped.len(), 1);
        assert_eq!(dry_run.reaped[0].id, deleted_id);
        assert_eq!(dry_run.bytes_before, dry_run.bytes_after);
        let conn = App::review_db(&path).unwrap();
        let count = conn
            .query_row(
                "SELECT COUNT(*) FROM comments WHERE review_key = ?1",
                params![app.review_db_key()],
                |row| row.get::<_, usize>(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        drop(conn);

        let reaped = app.gc_review_tombstones(false, 14, true, false).unwrap();
        assert_eq!(reaped.reaped.len(), 1);
        assert!(reaped.bytes_after < bytes_with_tombstone);
        let conn = App::review_db(&path).unwrap();
        let ids = conn
            .prepare("SELECT id FROM comments WHERE review_key = ?1 ORDER BY id")
            .unwrap()
            .query_map(params![app.review_db_key()], |row| row.get::<_, u64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(ids, vec![live_id]);
        drop(conn);

        let again = app.gc_review_tombstones(false, 14, true, false).unwrap();
        assert!(again.reaped.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    fn colliding_thread_app() -> App {
        let mut app = test_app();
        let root = line_comment();
        let mut reply = root.clone();
        reply.id = 2;
        reply.body = "reply".to_string();
        reply.in_reply_to = Some(1);
        reply.created_at = 2;
        app.review_comments = vec![root, reply];
        app.review_next_comment_id = 3;
        app.goto_last_step();
        app
    }

    #[test]
    fn visible_context_comment_anchors_folds_but_outdated_comment_does_not() {
        let content = (1..=60)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let diff = MultiFileDiff::from_file_pair(
            "fold.txt".into(),
            "fold.txt".into(),
            content.clone(),
            content,
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();

        let mut comment = line_comment();
        comment.anchor.file_path = "fold.txt".to_string();
        comment.anchor.new_range = Some(ReviewRange { start: 31, end: 31 });
        comment.anchor.hunk_id = None;
        comment.anchor.anchor_key = "line|fold.txt|new|31".to_string();
        comment.resolved = true;
        app.review_comments.push(comment);

        let folded = app.current_view_with_frame(AnimationFrame::Idle);
        let anchor_idx = folded
            .iter()
            .position(|line| line.new_line == Some(31))
            .unwrap();
        assert_eq!(folded[anchor_idx - 3].new_line, Some(28));
        assert_eq!(folded[anchor_idx + 3].new_line, Some(34));
        assert!(app
            .review_comment_overlays_for_current_file()
            .iter()
            .any(|overlay| overlay.display_idx == anchor_idx && overlay.resolved));

        app.review_comments[0].outdated = true;
        let folded = app.current_view_with_frame(AnimationFrame::Idle);
        assert!(folded.iter().all(|line| line.new_line != Some(31)));
        assert!(app.review_comment_overlays_for_current_file().is_empty());
    }

    #[test]
    fn review_output_uses_real_ids_and_filters_resolved_comments() {
        let mut app = test_app();
        let mut comment = line_comment();
        comment.id = 5;
        comment.resolved = true;
        app.review_comments.push(comment);

        let output = app.review_markdown();

        assert!(output.contains("ID: #5"));
        assert!(output.contains("Status: resolved"));
        assert!(app
            .review_markdown_filtered(&ReviewCommentFilter {
                unresolved: true,
                ..ReviewCommentFilter::default()
            })
            .is_empty());
    }

    #[test]
    fn new_review_comments_capture_anchor_snapshot() {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "one\ntwo\nold\nfour\nfive\nsix\n".to_string(),
            "one\ntwo\nnew\nfour\nfive\nsix\n".to_string(),
        );
        let mut app = App::new(
            diff,
            ViewMode::UnifiedPane,
            0,
            false,
            Some("branch".to_string()),
        );
        let mut metadata = test_metadata("@", "jj");
        metadata.jj_change_id = Some("change1".to_string());
        metadata.jj_commit_id = Some("commit1".to_string());
        app.set_review_target_metadata(Some(metadata));
        app.set_review_persist_enabled(false);
        app.enable_review_mode();

        app.add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 3, end: 3 }),
            "please fix".to_string(),
        )
        .unwrap();

        let snapshot = app.review_comments[0].anchor.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.line_text, "new");
        assert_eq!(snapshot.context_before, vec!["one", "two"]);
        assert_eq!(snapshot.context_after, vec!["four", "five", "six"]);
        assert_eq!(
            snapshot.target.as_ref().unwrap().jj_change_id.as_deref(),
            Some("change1")
        );
        let json: serde_json::Value = serde_json::from_str(&app.review_comments_json()).unwrap();
        assert_eq!(json["comments"][0]["anchorSnapshot"]["lineText"], "new");
    }

    fn test_app_with_new_content(new_content: &str) -> App {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "one\ntwo\nold\nfour\nfive\nsix\n".to_string(),
            new_content.to_string(),
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

    #[test]
    fn missing_diff_file_does_not_mark_resolved_comment_outdated() {
        let mut app = test_app_with_new_content("one\ntwo\ntarget\nfour\nfive\nsix\n");
        app.add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 3, end: 3 }),
            "please fix".to_string(),
        )
        .unwrap();
        app.review_comments[0].resolved = true;

        assert!(!app.reconcile_review_comment_anchor(0, None));
        assert!(app.review_comments[0].resolved);
        assert!(!app.review_comments[0].outdated);
    }

    #[test]
    fn missing_diff_file_marks_unresolved_comment_outdated_and_opens_fallback() {
        let mut app = test_app_with_new_content("one\ntwo\ntarget\nfour\nfive\nsix\n");
        app.add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 3, end: 3 }),
            "please fix".to_string(),
        )
        .unwrap();
        app.review_comments[0].anchor.file_index = 99;

        assert!(app.reconcile_review_comment_anchor(0, None));
        assert!(app.review_comments[0].outdated);
        let snapshot = app.review_comments[0].anchor.snapshot.as_mut().unwrap();
        snapshot.old_file = None;
        snapshot.new_file = None;
        assert!(app.open_review_comment(0));
        assert!(app.active_outdated_comments_view());
        assert_eq!(app.outdated_comment_focus, Some(app.review_comments[0].id));
    }

    #[test]
    fn anchor_drift_reanchors_shifted_comment_idempotently() {
        let mut app = test_app_with_new_content("one\ntwo\ntarget\nfour\nfive\nsix\n");
        app.add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 3, end: 3 }),
            "please fix".to_string(),
        )
        .unwrap();
        let comment = app.review_comments[0].clone();

        let mut shifted =
            test_app_with_new_content("inserted\none\ntwo\ntarget\nfour\nfive\nsix\n");
        shifted.review_comments = vec![comment];
        assert!(shifted.repair_review_comment_file_indexes());
        let first = shifted.review_comments[0].clone();
        assert!(!first.outdated);
        assert!(first.reanchored);
        assert_eq!(
            first.anchor.new_range,
            Some(ReviewRange { start: 4, end: 4 })
        );
        assert_eq!(first.anchor.anchor_key, "line|new.txt|new|4");
        assert_eq!(first.anchor.snapshot.as_ref().unwrap().line_number, 3);

        assert!(!shifted.repair_review_comment_file_indexes());
        assert_eq!(
            shifted.review_comments[0].anchor.new_range,
            first.anchor.new_range
        );
        assert_eq!(
            shifted.review_comments[0].anchor.anchor_key,
            first.anchor.anchor_key
        );
    }

    #[test]
    fn anchor_drift_marks_changed_line_outdated_and_hides_overlay() {
        let mut app = test_app_with_new_content("one\ntwo\ntarget\nfour\nfive\nsix\n");
        app.add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 3, end: 3 }),
            "please fix".to_string(),
        )
        .unwrap();
        let comment = app.review_comments[0].clone();

        let mut changed = test_app_with_new_content("one\ntwo\nchanged\nfour\nfive\nsix\n");
        changed.review_comments = vec![comment];
        assert!(changed.repair_review_comment_file_indexes());
        assert!(changed.review_comments[0].outdated);
        assert_eq!(
            changed.review_comments[0]
                .anchor
                .snapshot
                .as_ref()
                .unwrap()
                .line_text,
            "target"
        );
        assert!(changed
            .review_comment_overlays_for_current_file()
            .is_empty());
        let json: serde_json::Value =
            serde_json::from_str(&changed.review_comments_json()).unwrap();
        assert_eq!(json["comments"][0]["outdated"], true);
    }

    #[test]
    fn outdated_root_propagates_to_replies_and_renders_as_one_thread() {
        let mut source = test_app_with_new_content("one\ntwo\ntarget\nfour\nfive\nsix\n");
        let root_id = source
            .add_review_comment_from_cli(
                "new.txt",
                ReviewTargetKind::Line,
                Some(ReviewSide::New),
                None,
                Some(ReviewRange { start: 3, end: 3 }),
                "root".to_string(),
            )
            .unwrap();
        source
            .add_review_reply_from_cli(root_id, "first reply".to_string())
            .unwrap();
        source
            .add_review_reply_from_cli(root_id, "second reply".to_string())
            .unwrap();
        assert!(source.repair_review_comment_file_indexes());
        assert!(source
            .review_comments
            .iter()
            .all(|comment| !comment.outdated));

        let mut changed = test_app_with_new_content("one\ntwo\nchanged\nfour\nfive\nsix\n");
        changed.review_comments = source.review_comments.clone();
        assert!(changed.repair_review_comment_file_indexes());
        assert!(changed
            .review_comments
            .iter()
            .all(|comment| comment.outdated));
        assert_eq!(changed.outdated_comment_ids(), vec![root_id]);
        let overlays = changed.outdated_comment_overlays();
        assert_eq!(
            overlays
                .iter()
                .map(|overlay| overlay.id)
                .collect::<Vec<_>>(),
            vec![root_id, root_id + 1, root_id + 2]
        );
        assert!(overlays[0].overlay.thread_continues);
        assert!(overlays[1].overlay.thread_continues);
        assert!(!overlays[2].overlay.thread_continues);
        assert_eq!(overlays[1].overlay.body, "first reply");
        assert_eq!(overlays[2].overlay.body, "second reply");

        assert!(changed.open_review_comment(1));
        assert_eq!(changed.active_review_comment_id, Some(root_id));
        assert!(changed.outdated_diff_view.is_some() || changed.active_outdated_comments_view());
        if changed.outdated_diff_view.is_some() {
            assert_eq!(
                changed
                    .review_comment_overlays_for_current_file()
                    .iter()
                    .map(|overlay| overlay.id)
                    .collect::<Vec<_>>(),
                vec![root_id, root_id + 1, root_id + 2]
            );
        }

        let mut restored =
            test_app_with_new_content("inserted\none\ntwo\ntarget\nfour\nfive\nsix\n");
        restored.review_comments = changed.review_comments;
        assert!(restored.repair_review_comment_file_indexes());
        assert!(restored
            .review_comments
            .iter()
            .all(|comment| !comment.outdated && comment.reanchored));
        for reply in &restored.review_comments[1..] {
            assert_eq!(
                reply.anchor.file_index,
                restored.review_comments[0].anchor.file_index
            );
            assert_eq!(
                reply.anchor.new_range,
                Some(ReviewRange { start: 4, end: 4 })
            );
            assert_eq!(reply.anchor.anchor_key, "line|new.txt|new|4");
        }
    }

    #[test]
    fn review_comment_context_menu_actions_and_path_line() {
        let mut app = test_app();
        let root = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/home/var"))
            .join("repo");
        app.review_repo_root = Some(root.to_string_lossy().to_string());
        let mut comment = line_comment();
        comment.id = 5;
        comment.anchor.file_path = "src/lib.rs".to_string();
        comment.anchor.new_range = Some(ReviewRange { start: 2, end: 2 });
        comment.anchor.anchor_key = "line|src/lib.rs|new|2".to_string();
        app.review_comments.push(comment.clone());
        assert!(app.open_review_comment_context_menu_for_anchor(
            comment.anchor.anchor_key.clone(),
            3,
            4,
        ));

        let actions = app.review_comment_context_menu_actions();
        assert!(actions.contains(&ReviewCommentContextMenuAction::Body));
        assert!(actions.contains(&ReviewCommentContextMenuAction::Id));
        assert!(actions.contains(&ReviewCommentContextMenuAction::FileLine));
        assert!(!actions.contains(&ReviewCommentContextMenuAction::Url));
        let location = format!("{}/src/lib.rs:R2", collapse_home_path(&root));
        assert_eq!(app.review_comment_path_line_label(&comment), location);
        assert_eq!(
            app.review_comment_context_menu_label(ReviewCommentContextMenuAction::FileLine),
            format!("Copy location ({location})")
        );

        let mut provider_comment = comment;
        provider_comment.id = 6;
        let mut provider = provider_link("clean");
        provider.provider = "example".to_string();
        provider.pr_url = Some("https://example.com/reviews/1".to_string());
        assert_eq!(
            provider_comment_url(&provider).as_deref(),
            Some("https://example.com/reviews/1")
        );
        provider_comment.provider = Some(provider);
        app.review_comments.push(provider_comment.clone());
        app.review_comment_context_menu = Some(ReviewCommentContextMenu {
            comment_id: 6,
            x: 0,
            y: 0,
        });
        assert!(app
            .review_comment_context_menu_actions()
            .contains(&ReviewCommentContextMenuAction::Url));
    }

    #[test]
    fn review_comment_cli_output_can_be_colored() {
        let mut app = test_app();
        let mut comment = line_comment();
        comment.id = 5;
        comment.author = Some(ReviewAuthor {
            name: "Agent".to_string(),
            email: None,
            author_type: Some("agent".to_string()),
            usernames: BTreeMap::new(),
            avatar_url: None,
        });
        app.review_comments.push(comment);

        let output = app.review_markdown_filtered_colored(&ReviewCommentFilter::default(), true);

        assert!(output.starts_with("ID: \u{1b}[1;38;5;8m#5\u{1b}[0m"));
        assert!(output.contains("Status: \u{1b}[33munresolved\u{1b}[0m"));
        assert!(output.contains("File: \u{1b}[2mnew.txt\u{1b}[0m"));
        assert!(output.contains("\u{1b}[2mAuthor:\u{1b}[0m Agent \u{1b}[35m(agent)\u{1b}[0m"));
        assert!(output.contains("\u{1b}[2mBody:\u{1b}[0m\n  please fix"));
    }

    #[test]
    fn captured_file_round_trips_and_respects_cap() {
        let content = "hello\n".repeat(1_000);
        let captured = capture_file(&content).unwrap();
        assert!(captured.data.len() < content.len());
        assert_eq!(
            decode_captured_file(&captured).as_deref(),
            Some(content.as_str())
        );
        assert!(capture_file(&"x".repeat(CAPTURED_FILE_MAX_BYTES + 1)).is_none());
        assert!(decode_captured_file(&CapturedFile {
            data: "not base64".to_string(),
            orig_len: 10,
        })
        .is_none());
    }

    #[test]
    fn captured_snapshot_round_trips_and_old_json_defaults_to_none() {
        let captured = capture_file("old\n").unwrap();
        let snapshot = ReviewAnchorSnapshot {
            side: "new".to_string(),
            line_number: 1,
            line_text: "new".to_string(),
            context_before: Vec::new(),
            context_after: Vec::new(),
            target: None,
            old_file: Some(captured.clone()),
            new_file: Some(capture_file("new\n").unwrap()),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let round_trip: ReviewAnchorSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(round_trip.old_file, Some(captured));
        let old: ReviewAnchorSnapshot = serde_json::from_str(
            r#"{"side":"new","lineNumber":1,"lineText":"new","contextBefore":[],"contextAfter":[]}"#,
        )
        .unwrap();
        assert!(old.old_file.is_none());
        assert!(old.new_file.is_none());
    }

    #[test]
    fn outdated_comment_reconstructs_from_capture_without_repository() {
        let mut app = test_app_with_new_content("one\ntwo\ncaptured\nfour\nfive\nsix\n");
        app.add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 3, end: 3 }),
            "captured note".to_string(),
        )
        .unwrap();
        app.review_comments[0].outdated = true;
        app.multi_diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "one\ntwo\nold\nfour\nfive\nsix\n".to_string(),
            "one\ntwo\nold\nfour\nfive\nsix\n".to_string(),
        );

        let tab_count = app.topbar_tabs.len();
        assert!(app.open_review_comment(0));
        assert_eq!(app.topbar_tabs.len(), tab_count);
        assert!(!app.outdated_reconstruction_pending());
        assert_eq!(
            app.outdated_diff_title().as_deref(),
            Some("Outdated: new.txt")
        );
        assert_eq!(
            app.multi_diff.file_contents(0).map(|(_, new)| new),
            Some("one\ntwo\ncaptured\nfour\nfive\nsix\n")
        );
        assert_eq!(app.view_history.len(), 2);
        assert!(matches!(
            app.current_view_history_recipe(),
            Some(crate::app::ViewHistoryRecipe::Comment { comment_id })
                if comment_id == app.review_comments[0].id
        ));
        assert!(app.navigate_view_back());
        assert!(app.outdated_diff_title().is_none());
        assert!(app.navigate_view_forward());
        assert_eq!(app.topbar_tabs.len(), tab_count);
        assert_eq!(
            app.outdated_diff_title().as_deref(),
            Some("Outdated: new.txt")
        );
        app.select_file(0);
        assert!(app.outdated_diff_title().is_none());
        assert!(app.navigate_view_back());
        assert_eq!(
            app.outdated_diff_title().as_deref(),
            Some("Outdated: new.txt")
        );
    }

    #[test]
    fn corrupt_capture_falls_back_without_repository() {
        let mut app = test_app_with_new_content("one\ntwo\ntarget\nfour\nfive\nsix\n");
        app.add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 3, end: 3 }),
            "note".to_string(),
        )
        .unwrap();
        app.review_comments[0].outdated = true;
        app.review_comments[0]
            .anchor
            .snapshot
            .as_mut()
            .unwrap()
            .new_file = Some(CapturedFile {
            data: "broken".to_string(),
            orig_len: 6,
        });

        assert!(app.open_review_comment(0));
        assert!(app.active_outdated_comments_view());
    }

    #[test]
    fn outdated_comment_navigation_opens_focused_snapshot_view() {
        let mut app = test_app();
        let mut comment = line_comment();
        comment.id = 7;
        comment.outdated = true;
        comment.anchor.file_path = "src/lib.rs".to_string();
        comment.anchor.snapshot = Some(ReviewAnchorSnapshot {
            side: "new".to_string(),
            line_number: 42,
            line_text: "let answer = 41;".to_string(),
            context_before: vec!["fn answer() {".to_string()],
            context_after: vec!["}".to_string()],
            target: None,
            old_file: None,
            new_file: None,
        });
        app.review_comments.push(comment);

        assert!(app.open_review_comment(0));
        assert!(app.active_outdated_comments_view());
        assert_eq!(app.outdated_comment_focus, Some(7));
        let overlays = app.outdated_comment_overlays();
        assert_eq!(overlays.len(), 1);
        assert!(overlays[0].overlay.title.contains("Outdated"));
        assert!(overlays[0].overlay.body.contains("src/lib.rs:42"));
        assert!(overlays[0].overlay.body.contains("let answer = 41;"));
        assert!(app.resolve_review_comment_number(1));
        assert!(app.review_comments[0].resolved);
    }

    #[test]
    fn outdated_comments_are_filterable_and_not_unresolved_tasks() {
        let mut app = test_app();
        let mut outdated = line_comment();
        outdated.id = 5;
        outdated.outdated = true;
        let mut live = line_comment();
        live.id = 7;
        live.body = "live".to_string();
        app.review_comments.push(outdated);
        app.review_comments.push(live);

        let all = app.review_markdown_filtered(&ReviewCommentFilter::default());
        assert!(all.contains("ID: #5"));
        assert!(all.contains("Status: unresolved (outdated)"));
        assert!(all.contains("ID: #7"));

        let unresolved = ReviewCommentFilter {
            unresolved: true,
            ..ReviewCommentFilter::default()
        };
        let unresolved_output = app.review_markdown_filtered(&unresolved);
        assert!(!unresolved_output.contains("ID: #5"));
        assert!(unresolved_output.contains("ID: #7"));
        assert_eq!(
            app.review_status_comment_rows_filtered(&unresolved).len(),
            1
        );

        let only_outdated = ReviewCommentFilter {
            outdated: Some(true),
            ..ReviewCommentFilter::default()
        };
        let outdated_output = app.review_markdown_filtered(&only_outdated);
        assert!(outdated_output.contains("ID: #5"));
        assert!(!outdated_output.contains("ID: #7"));

        let unresolved_outdated = ReviewCommentFilter {
            unresolved: true,
            outdated: Some(true),
            ..ReviewCommentFilter::default()
        };
        let intersection = app.review_markdown_filtered_colored(&unresolved_outdated, true);
        assert!(intersection.contains("ID: \u{1b}[1;38;5;8m#5\u{1b}[0m"));
        assert!(intersection.contains("\u{1b}[2;33m(outdated)\u{1b}[0m"));
        assert!(!intersection.contains("#7"));

        let no_outdated = ReviewCommentFilter {
            outdated: Some(false),
            ..ReviewCommentFilter::default()
        };
        assert!(!app
            .review_markdown_filtered(&no_outdated)
            .contains("ID: #5"));
    }

    #[test]
    fn review_id_filter_returns_one_comment_or_missing_id() {
        let mut app = test_app();
        let mut first = line_comment();
        first.id = 5;
        first.body = "first".to_string();
        let mut second = line_comment();
        second.id = 7;
        second.body = "second".to_string();
        app.review_comments.push(first);
        app.review_comments.push(second);

        let filter = ReviewCommentFilter {
            ids: vec![7],
            ..ReviewCommentFilter::default()
        };
        let output = app.review_markdown_filtered(&filter);
        assert!(!output.contains("ID: #5"));
        assert!(output.contains("ID: #7"));

        let json: serde_json::Value =
            serde_json::from_str(&app.review_comments_json_filtered(&filter)).unwrap();
        assert_eq!(json["comments"].as_array().unwrap().len(), 1);
        assert_eq!(json["comments"][0]["id"], 7);
        assert_eq!(app.missing_review_filter_id(&filter), None);

        let missing = ReviewCommentFilter {
            ids: vec![9],
            ..ReviewCommentFilter::default()
        };
        assert_eq!(app.missing_review_filter_id(&missing), Some(9));
    }

    #[test]
    fn local_comment_timestamps_set_and_bump() {
        let mut app = test_app();
        let id = app
            .add_review_comment_from_cli(
                "new.txt",
                ReviewTargetKind::Line,
                Some(ReviewSide::New),
                None,
                Some(ReviewRange { start: 1, end: 1 }),
                "first".to_string(),
            )
            .unwrap();
        let created = app.review_comments[0].created_at;
        let updated = app.review_comments[0].updated_at;
        assert!(created > 0);
        assert_eq!(created, updated);

        assert!(app.edit_review_comment_from_cli(id, "second".to_string()));
        let edited = app.review_comments[0].updated_at;
        assert!(edited > updated);

        assert!(app.set_review_comment_resolved_from_cli(id, true));
        let resolved = app.review_comments[0].updated_at;
        assert!(resolved > edited);

        assert!(app.set_review_comment_resolved_from_cli(id, false));
        assert!(app.review_comments[0].updated_at > resolved);
    }

    #[test]
    fn since_filter_reports_updated_and_removed_comments() {
        let mut app = test_app();
        let id = app
            .add_review_comment_from_cli(
                "new.txt",
                ReviewTargetKind::Line,
                Some(ReviewSide::New),
                None,
                Some(ReviewRange { start: 1, end: 1 }),
                "first".to_string(),
            )
            .unwrap();
        let since = app.review_comments[0].updated_at;
        assert!(app.edit_review_comment_from_cli(id, "second".to_string()));

        let value: serde_json::Value =
            serde_json::from_str(&app.review_comments_json_filtered(&ReviewCommentFilter {
                since: Some(since),
                ..ReviewCommentFilter::default()
            }))
            .unwrap();
        assert_eq!(value["comments"][0]["id"], id);
        assert_eq!(value["comments"][0]["changeType"], "updated");
        assert_eq!(value["comments"][0]["deleted"], serde_json::Value::Null);

        assert!(app.remove_review_comment_from_cli(id));
        let value: serde_json::Value =
            serde_json::from_str(&app.review_comments_json_filtered(&ReviewCommentFilter {
                since: Some(since),
                ..ReviewCommentFilter::default()
            }))
            .unwrap();
        assert_eq!(value["comments"][0]["changeType"], "removed");
        assert_eq!(value["comments"][0]["deleted"], true);
    }

    #[test]
    fn resolving_review_comment_updates_its_thread_only() {
        let mut app = test_app();
        let mut first = line_comment();
        first.id = 1;
        first.provider = Some(provider_link("clean"));
        first.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        first.provider.as_mut().unwrap().thread_resolved = Some(false);

        let mut reply = first.clone();
        reply.id = 2;
        reply.anchor.anchor_key = "reply".to_string();
        reply.provider.as_mut().unwrap().comment_id = "11".to_string();
        reply.provider.as_mut().unwrap().in_reply_to_id = Some("10".to_string());

        let mut issue = line_comment();
        issue.id = 3;
        issue.provider = Some(provider_link("clean"));
        issue.provider.as_mut().unwrap().api_kind = "issue".to_string();
        issue.provider.as_mut().unwrap().thread_id = None;

        let mut local = line_comment();
        local.id = 4;
        app.review_comments = vec![first, reply, issue, local];

        assert!(app.review_comment_is_reply_id(2));
        assert!(!app.set_review_comment_resolved_from_cli(2, true));
        assert!(!app.review_comments[0].resolved);
        assert!(!app.review_comments[1].resolved);

        assert!(app.set_review_comment_resolved_from_cli(1, true));
        assert!(app.review_comments[0].resolved);
        assert!(app.review_comments[1].resolved);
        assert!(!app.review_comments[2].resolved);
        assert!(!app.review_comments[3].resolved);

        assert!(app.set_review_comment_resolved_from_cli(4, true));
        assert!(app.review_comments[3].resolved);

        assert_eq!(
            app.mark_review_thread_synced(
                "github",
                "owner/repo",
                1,
                "thread-1",
                true,
                &[(1, true), (2, true)],
            ),
            vec![1, 2]
        );
        let provider = app.review_comments[0].provider.as_ref().unwrap();
        assert_eq!(provider.thread_resolved, Some(true));
        assert!(!provider.resolved_dirty);
    }

    #[test]
    fn pull_adds_missing_thread_link_without_clobbering_local_resolve() {
        let mut app = test_app();
        let mut existing = line_comment();
        existing.id = 1;
        existing.resolved = true;
        existing.provider = Some(provider_link("clean"));
        existing.provider.as_mut().unwrap().resolved_dirty = true;
        app.review_comments.push(existing);

        let mut incoming = line_comment();
        incoming.provider = Some(provider_link("clean"));
        incoming.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        incoming.provider.as_mut().unwrap().thread_resolved = Some(false);

        assert_eq!(app.upsert_provider_review_comment(incoming), 1);
        assert!(app.review_comments[0].resolved);
        let provider = app.review_comments[0].provider.as_ref().unwrap();
        assert_eq!(provider.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(provider.thread_resolved, Some(false));
        assert!(provider.resolved_dirty);
    }

    #[test]
    fn pull_clears_resolution_dirty_when_remote_catches_up() {
        let mut app = test_app();
        let mut existing = line_comment();
        existing.id = 1;
        existing.resolved = true;
        existing.provider = Some(provider_link("clean"));
        existing.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        existing.provider.as_mut().unwrap().thread_resolved = Some(false);
        existing.provider.as_mut().unwrap().resolved_dirty = true;
        app.review_comments.push(existing);

        let mut incoming = line_comment();
        incoming.resolved = true;
        incoming.provider = Some(provider_link("clean"));
        incoming.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        incoming.provider.as_mut().unwrap().thread_resolved = Some(true);

        assert_eq!(app.upsert_provider_review_comment(incoming), 1);
        assert!(app.review_comments[0].resolved);
        assert!(
            !app.review_comments[0]
                .provider
                .as_ref()
                .unwrap()
                .resolved_dirty
        );
    }

    #[test]
    fn pull_preserves_local_issue_comment_resolution() {
        let mut app = test_app();
        let mut existing = line_comment();
        existing.id = 1;
        existing.resolved = true;
        existing.provider = Some(provider_link("clean"));
        existing.provider.as_mut().unwrap().api_kind = "issue".to_string();
        existing.provider.as_mut().unwrap().resolved_dirty = true;
        app.review_comments.push(existing);

        let mut incoming = line_comment();
        incoming.body = "remote body".to_string();
        incoming.provider = Some(provider_link("clean"));
        incoming.provider.as_mut().unwrap().api_kind = "issue".to_string();

        assert_eq!(app.upsert_provider_review_comment(incoming), 1);
        assert!(app.review_comments[0].resolved);
        assert_eq!(app.review_comments[0].body, "remote body");
        assert!(
            app.review_comments[0]
                .provider
                .as_ref()
                .unwrap()
                .resolved_dirty
        );
    }

    #[test]
    fn push_completion_preserves_newer_local_thread_toggle() {
        let mut app = test_app();
        let mut comment = line_comment();
        comment.id = 1;
        comment.provider = Some(provider_link("clean"));
        comment.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        comment.provider.as_mut().unwrap().thread_resolved = Some(false);
        app.review_comments.push(comment);

        assert_eq!(
            app.mark_review_thread_synced(
                "github",
                "owner/repo",
                1,
                "thread-1",
                true,
                &[(1, true)],
            ),
            vec![1]
        );
        assert!(!app.review_comments[0].resolved);
        let provider = app.review_comments[0].provider.as_ref().unwrap();
        assert_eq!(provider.thread_resolved, Some(true));
        assert!(provider.resolved_dirty);
    }

    #[test]
    fn push_completion_normalizes_unchanged_thread_members() {
        let mut app = test_app();
        let mut first = line_comment();
        first.id = 1;
        first.resolved = true;
        first.provider = Some(provider_link("clean"));
        first.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        first.provider.as_mut().unwrap().thread_resolved = Some(false);
        let mut second = first.clone();
        second.id = 2;
        second.resolved = false;
        second.provider.as_mut().unwrap().comment_id = "11".to_string();
        app.review_comments = vec![first, second];

        assert_eq!(
            app.mark_review_thread_synced(
                "github",
                "owner/repo",
                1,
                "thread-1",
                true,
                &[(1, true), (2, false)],
            ),
            vec![1, 2]
        );
        assert!(app.review_comments.iter().all(|comment| comment.resolved));
        assert!(app
            .review_comments
            .iter()
            .all(|comment| { !comment.provider.as_ref().unwrap().resolved_dirty }));
    }

    #[test]
    fn colliding_reply_card_actions_target_the_reply_id() {
        let mut letter_delete = colliding_thread_app();
        assert!(letter_delete.delete_review_comment_letter('b'));
        assert!(!letter_delete.review_comments[0].deleted);
        assert!(letter_delete.review_comments[1].deleted);
        assert!(!letter_delete.review_delete_confirmation_active());

        let mut click_delete = colliding_thread_app();
        click_delete.diff_view_area = Some((0, 0, 80, 20));
        click_delete.add_review_preview_delete_box(2, 2, 8, 1, 2, "line|new.txt|new|1".to_string());
        assert!(click_delete.handle_review_preview_click(3, 2));
        assert!(!click_delete.review_comments[0].deleted);
        assert!(click_delete.review_comments[1].deleted);

        let mut letter_edit = colliding_thread_app();
        assert!(letter_edit.edit_review_comment_letter('b'));
        assert_eq!(letter_edit.active_review_comment_id, Some(2));
        letter_edit.review_clear_editor_text();
        for ch in "edited reply".chars() {
            letter_edit.review_insert_char(ch);
        }
        letter_edit.review_save_editor();
        assert_eq!(letter_edit.review_comments[0].body, "please fix");
        assert_eq!(letter_edit.review_comments[1].body, "edited reply");

        let mut click_edit = colliding_thread_app();
        click_edit.diff_view_area = Some((0, 0, 80, 20));
        click_edit.add_review_preview_edit_box(2, 2, 8, 1, 2, "line|new.txt|new|1".to_string());
        assert!(click_edit.handle_review_preview_click(3, 2));
        assert_eq!(click_edit.active_review_comment_id, Some(2));

        let mut root_delete = colliding_thread_app();
        assert!(root_delete.delete_review_comment_letter('a'));
        assert_eq!(
            root_delete
                .review_delete_confirmation_render()
                .unwrap()
                .body,
            "Delete this comment and its 1 reply?"
        );

        let mut root_edit = colliding_thread_app();
        assert!(root_edit.edit_review_comment_letter('a'));
        assert_eq!(
            root_edit
                .review_comment_overlays_for_current_file()
                .iter()
                .map(|overlay| overlay.id)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn reply_reconcile_skips_snapshot_matching_then_inherits_root_position() {
        let mut app = colliding_thread_app();
        app.review_comments[1].anchor.anchor_key = "reply|1|2".to_string();
        app.review_comments[1].anchor.new_range = Some(ReviewRange {
            start: 999,
            end: 999,
        });
        app.review_comments[1].anchor.snapshot = Some(ReviewAnchorSnapshot {
            side: "new".to_string(),
            line_number: 1,
            line_text: "new".to_string(),
            context_before: Vec::new(),
            context_after: Vec::new(),
            target: None,
            old_file: None,
            new_file: None,
        });

        assert!(!app.reconcile_review_comment_anchor(1, Some(0)));
        assert_eq!(app.review_comments[1].anchor.anchor_key, "reply|1|2");
        assert!(app.repair_review_comment_file_indexes());
        assert_eq!(
            app.review_comments[1].anchor.anchor_key,
            app.review_comments[0].anchor.anchor_key
        );
        assert_eq!(
            app.review_comments[1].anchor.new_range,
            app.review_comments[0].anchor.new_range
        );
        let overlays = app.review_comment_overlays_for_current_file();
        assert_eq!(overlays.len(), 2);
        assert_eq!(overlays[0].display_idx, overlays[1].display_idx);
    }

    #[test]
    fn cli_reply_creates_a_nested_review_thread_child() {
        let mut app = test_app();
        let mut parent = line_comment();
        let mut provider = provider_link("clean");
        provider.thread_id = Some("thread-1".to_string());
        provider.thread_resolved = Some(false);
        parent.provider = Some(provider);
        parent.can_edit = false;
        app.review_comments.push(parent);
        app.review_next_comment_id = 2;
        app.set_review_author(Some(ReviewAuthor {
            name: "Agent".to_string(),
            email: None,
            author_type: Some("agent".to_string()),
            usernames: BTreeMap::from([("github".to_string(), "agent".to_string())]),
            avatar_url: None,
        }));

        let id = app
            .add_review_reply_from_cli(1, "Thanks, fixed.".to_string())
            .unwrap();
        let reply = app
            .review_comments
            .iter()
            .find(|comment| comment.id == id)
            .unwrap();
        let provider = reply.provider.as_ref().unwrap();
        assert_eq!(reply.anchor.file_path, "new.txt");
        assert_eq!(
            reply.anchor.new_range,
            Some(ReviewRange { start: 1, end: 1 })
        );
        assert_eq!(
            reply.author.as_ref().unwrap().author_type.as_deref(),
            Some("agent")
        );
        assert_eq!(provider.comment_id, "");
        assert_eq!(provider.in_reply_to_id.as_deref(), Some("10"));
        assert_eq!(provider.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(provider.api_kind, "review");
        assert_eq!(provider.sync_state, "dirty");
        assert_eq!(reply.in_reply_to, None);

        app.goto_last_step();
        let overlays = app.review_comment_overlays_for_current_file();
        assert_eq!(overlays.len(), 2);
        assert!(!overlays[0].anchor_key.starts_with("reply|"));
        assert!(overlays[1].anchor_key.starts_with("reply|"));
        assert_eq!(overlays[0].reply_label.as_deref(), Some("ra"));
        assert_eq!(overlays[0].resolve_label.as_deref(), Some("va"));
        assert_eq!(overlays[1].reply_label.as_deref(), Some("rb"));
        assert_eq!(overlays[1].resolve_label, None);
        assert!(!app.resolve_review_comment_letter('b'));
    }

    #[test]
    fn cli_reply_accepts_gitlab_thread_and_uses_gitlab_username() {
        let mut app = test_app();
        let mut parent = line_comment();
        let mut provider = provider_link("clean");
        provider.provider = "gitlab".to_string();
        provider.thread_id = Some("discussion-1".to_string());
        provider.thread_resolved = Some(false);
        parent.provider = Some(provider);
        parent.can_edit = false;
        app.review_comments.push(parent);
        app.review_next_comment_id = 2;
        app.set_review_author(Some(ReviewAuthor {
            name: "Agent".to_string(),
            email: None,
            author_type: Some("agent".to_string()),
            usernames: BTreeMap::from([
                ("github".to_string(), "wrong".to_string()),
                ("gitlab".to_string(), "agent-gl".to_string()),
            ]),
            avatar_url: None,
        }));

        let id = app
            .add_review_reply_from_cli(1, "Thanks, fixed.".to_string())
            .unwrap();
        let provider = app
            .review_comments
            .iter()
            .find(|comment| comment.id == id)
            .and_then(|comment| comment.provider.as_ref())
            .unwrap();

        assert_eq!(provider.provider, "gitlab");
        assert_eq!(provider.thread_id.as_deref(), Some("discussion-1"));
        assert_eq!(provider.in_reply_to_id.as_deref(), Some("10"));
        assert_eq!(provider.author_username.as_deref(), Some("agent-gl"));
    }

    #[test]
    fn cli_reply_accepts_forgejo_thread_and_uses_forgejo_username() {
        let mut app = test_app();
        let mut parent = line_comment();
        let mut provider = provider_link("clean");
        provider.provider = "forgejo".to_string();
        provider.comment_id = "20".to_string();
        provider.thread_id = Some("review:10:20".to_string());
        parent.provider = Some(provider);
        parent.can_edit = false;
        app.review_comments.push(parent);
        app.review_next_comment_id = 2;
        app.set_review_author(Some(ReviewAuthor {
            name: "Agent".to_string(),
            email: None,
            author_type: Some("agent".to_string()),
            usernames: BTreeMap::from([("forgejo".to_string(), "agent-fj".to_string())]),
            avatar_url: None,
        }));

        let id = app
            .add_review_reply_from_cli(1, "Thanks, fixed.".to_string())
            .unwrap();
        let provider = app
            .review_comments
            .iter()
            .find(|comment| comment.id == id)
            .and_then(|comment| comment.provider.as_ref())
            .unwrap();

        assert_eq!(provider.provider, "forgejo");
        assert_eq!(provider.thread_id.as_deref(), Some("review:10:20"));
        assert_eq!(provider.in_reply_to_id.as_deref(), Some("20"));
        assert_eq!(provider.author_username.as_deref(), Some("agent-fj"));
    }

    #[test]
    fn cli_gitlab_reply_does_not_use_github_username_fallback() {
        let mut app = test_app();
        let mut parent = line_comment();
        let mut provider = provider_link("clean");
        provider.provider = "gitlab".to_string();
        provider.thread_id = Some("discussion-1".to_string());
        provider.thread_resolved = Some(false);
        parent.provider = Some(provider);
        app.review_comments.push(parent);
        app.review_next_comment_id = 2;
        app.set_review_author(Some(ReviewAuthor {
            name: "Agent".to_string(),
            email: None,
            author_type: Some("agent".to_string()),
            usernames: BTreeMap::from([("github".to_string(), "agent-gh".to_string())]),
            avatar_url: None,
        }));

        let id = app
            .add_review_reply_from_cli(1, "Thanks, fixed.".to_string())
            .unwrap();
        let provider = app
            .review_comments
            .iter()
            .find(|comment| comment.id == id)
            .and_then(|comment| comment.provider.as_ref())
            .unwrap();

        assert_eq!(provider.author_username, None);
    }

    #[test]
    fn tui_reply_editor_saves_the_same_thread_metadata() {
        let mut app = test_app();
        let mut parent = line_comment();
        let mut provider = provider_link("clean");
        provider.thread_id = Some("thread-1".to_string());
        provider.thread_resolved = Some(false);
        parent.provider = Some(provider);
        parent.resolved = true;
        let anchor_key = parent.anchor.anchor_key.clone();
        app.review_comments.push(parent);
        app.review_next_comment_id = 2;
        app.goto_last_step();
        app.diff_view_area = Some((0, 0, 80, 20));
        app.active_review_comment_id = Some(1);
        app.add_review_preview_reply_box(2, 2, 8, 1, 1, anchor_key);

        assert!(app.handle_review_preview_click(3, 2));
        assert_eq!(app.active_review_comment_id, None);
        assert!(app.review_editor.as_ref().unwrap().reply.is_some());
        app.review_cancel_editor();
        assert!(app.reply_to_review_comment_letter('a'));
        assert!(app.review_editor.as_ref().unwrap().reply.is_some());
        for ch in "Reply".chars() {
            app.review_insert_char(ch);
        }
        app.review_save_editor();

        let reply = app
            .review_comments
            .iter()
            .find(|comment| comment.id == 2)
            .unwrap();
        assert!(reply.resolved);
        assert!(reply.provider.as_ref().unwrap().resolved_dirty);
        assert_eq!(
            reply
                .provider
                .as_ref()
                .and_then(|provider| provider.in_reply_to_id.as_deref()),
            Some("10")
        );
    }

    #[test]
    fn local_replies_form_a_nested_provider_free_tree() {
        let mut app = test_app();
        app.review_comments.push(line_comment());
        app.review_next_comment_id = 2;

        let child_id = app
            .add_review_reply_from_cli(1, "Local child".to_string())
            .unwrap();
        let grandchild_id = app
            .add_review_reply_from_cli(child_id, "Local grandchild".to_string())
            .unwrap();

        let child = app
            .review_comments
            .iter()
            .find(|comment| comment.id == child_id)
            .unwrap();
        assert_eq!(child.in_reply_to, Some(1));
        assert!(child.provider.is_none());
        let grandchild = app
            .review_comments
            .iter()
            .find(|comment| comment.id == grandchild_id)
            .unwrap();
        assert_eq!(grandchild.in_reply_to, Some(child_id));
        assert!(grandchild.provider.is_none());

        app.goto_last_step();
        let overlays = app.review_comment_overlays_for_current_file();
        assert_eq!(
            overlays
                .iter()
                .map(|overlay| overlay.anchor_key.starts_with("reply|"))
                .collect::<Vec<_>>(),
            vec![false, true, true]
        );
        assert_eq!(
            overlays
                .iter()
                .map(|overlay| overlay.thread_continues)
                .collect::<Vec<_>>(),
            vec![true, true, false]
        );
        assert_eq!(overlays[0].reply_label.as_deref(), Some("ra"));
        assert_eq!(overlays[0].resolve_label.as_deref(), Some("va"));
        assert_eq!(overlays[1].reply_label.as_deref(), Some("rb"));
        assert_eq!(overlays[1].resolve_label, None);
        assert!(overlays[1].anchor_key.starts_with("reply|"));

        let mut pushed_parent = provider_link("clean");
        pushed_parent.thread_id = None;
        pushed_parent.thread_resolved = None;
        assert!(app.mark_review_comment_synced(1, pushed_parent));
        let overlays = app.review_comment_overlays_for_current_file();
        assert!(!overlays[0].anchor_key.starts_with("reply|"));
        assert!(overlays[1].anchor_key.starts_with("reply|"));
        assert!(overlays[0].thread_continues);
    }

    #[test]
    fn pending_provider_reply_can_accept_another_reply() {
        let mut app = test_app();
        let mut parent = line_comment();
        let mut provider = provider_link("clean");
        provider.thread_id = Some("thread-1".to_string());
        provider.thread_resolved = Some(false);
        parent.provider = Some(provider);
        parent.can_edit = false;
        app.review_comments.push(parent);
        app.review_next_comment_id = 2;
        let pending_id = app
            .add_review_reply_from_cli(1, "First reply".to_string())
            .unwrap();

        let next_id = app
            .add_review_reply_from_cli(pending_id, "Second reply".to_string())
            .unwrap();
        let next = app
            .review_comments
            .iter()
            .find(|comment| comment.id == next_id)
            .unwrap();
        assert_eq!(
            next.provider.as_ref().unwrap().in_reply_to_id.as_deref(),
            Some("10")
        );
    }

    #[test]
    fn tui_local_reply_saves_a_provider_free_child() {
        let mut app = test_app();
        app.review_comments.push(line_comment());
        app.review_next_comment_id = 2;

        assert!(app.start_review_comment_reply(1));
        for ch in "Local reply".chars() {
            app.review_insert_char(ch);
        }
        app.review_save_editor();

        let reply = app
            .review_comments
            .iter()
            .find(|comment| comment.id == 2)
            .unwrap();
        assert_eq!(reply.in_reply_to, Some(1));
        assert!(reply.provider.is_none());
    }

    #[test]
    fn snapshot_load_remaps_local_reply_parent_ids() {
        let base = temp_path("snapshot-local-reply-remap");
        let mut snapshot = persistent_test_app(&base);
        snapshot.enable_review_mode();
        snapshot.review_storage_key = "snapshot".to_string();
        let mut parent = line_comment();
        parent.id = 2;
        let mut child = line_comment();
        child.id = 3;
        child.anchor.anchor_key = "reply|2|3".to_string();
        child.in_reply_to = Some(2);
        snapshot.review_comments = vec![parent, child];
        snapshot.review_next_comment_id = 4;
        snapshot.persist_review_session();

        let mut loaded = persistent_test_app(&base);
        loaded.enable_review_mode();
        assert!(loaded.load_review_snapshot_into_current_target("snapshot"));
        assert_eq!(loaded.review_comments.len(), 2);
        assert_eq!(loaded.review_comments[0].id, 1);
        assert_eq!(loaded.review_comments[1].id, 2);
        assert_eq!(loaded.review_comments[1].in_reply_to, Some(1));

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn inline_actions_ignore_comments_from_other_files() {
        let mut app = test_app();
        let mut comment = line_comment();
        comment.anchor.file_path = "other.txt".to_string();
        comment.anchor.anchor_key = "line|other.txt|new|1".to_string();
        app.review_comments.push(comment);

        assert_eq!(app.review_comment_count(), 1);
        assert!(!app.inline_review_actions_available());
    }

    #[test]
    fn pending_reply_delete_removes_it_before_push() {
        let mut app = test_app();
        let mut parent = line_comment();
        let mut provider = provider_link("clean");
        provider.thread_id = Some("thread-1".to_string());
        provider.thread_resolved = Some(false);
        parent.provider = Some(provider);
        parent.can_edit = false;
        app.review_comments.push(parent);
        app.review_next_comment_id = 2;
        let reply_id = app
            .add_review_reply_from_cli(1, "Never send".to_string())
            .unwrap();

        assert!(app.remove_review_comment_from_cli(reply_id));
        assert!(app
            .review_comments_for_sync()
            .iter()
            .all(|comment| comment.id != reply_id));
    }

    #[test]
    fn pending_reply_refreshes_remote_thread_state_without_losing_local_toggle() {
        let mut app = test_app();
        let mut parent = line_comment();
        let mut provider = provider_link("clean");
        provider.thread_id = Some("thread-1".to_string());
        provider.thread_resolved = Some(false);
        parent.provider = Some(provider);
        parent.can_edit = false;
        app.review_comments.push(parent.clone());
        app.review_next_comment_id = 2;
        let reply_id = app
            .add_review_reply_from_cli(1, "Reply".to_string())
            .unwrap();

        let mut incoming = parent;
        incoming.resolved = true;
        incoming.provider.as_mut().unwrap().thread_resolved = Some(true);
        app.upsert_provider_review_comment(incoming);
        let reply = app
            .review_comments
            .iter()
            .find(|comment| comment.id == reply_id)
            .unwrap();
        assert!(reply.resolved);
        assert_eq!(reply.provider.as_ref().unwrap().thread_resolved, Some(true));
        assert!(!reply.provider.as_ref().unwrap().resolved_dirty);

        let reply = app
            .review_comments
            .iter_mut()
            .find(|comment| comment.id == reply_id)
            .unwrap();
        reply.resolved = false;
        reply.provider.as_mut().unwrap().resolved_dirty = true;
        let mut incoming = app.review_comments[0].clone();
        incoming.resolved = true;
        incoming.provider.as_mut().unwrap().thread_resolved = Some(true);
        app.upsert_provider_review_comment(incoming);
        let reply = app
            .review_comments
            .iter()
            .find(|comment| comment.id == reply_id)
            .unwrap();
        assert!(!reply.resolved);
        assert!(reply.provider.as_ref().unwrap().resolved_dirty);
    }

    #[test]
    fn pushed_reply_round_trips_without_a_duplicate_local_card() {
        let mut app = test_app();
        let mut parent = line_comment();
        let mut provider = provider_link("clean");
        provider.thread_id = Some("thread-1".to_string());
        provider.thread_resolved = Some(false);
        parent.provider = Some(provider);
        parent.can_edit = false;
        app.review_comments.push(parent);
        app.review_next_comment_id = 2;
        let reply_id = app
            .add_review_reply_from_cli(1, "Reply".to_string())
            .unwrap();
        let mut clean = provider_link("clean");
        clean.comment_id = "99".to_string();
        clean.in_reply_to_id = Some("10".to_string());
        clean.thread_id = Some("thread-1".to_string());
        clean.thread_resolved = Some(false);
        assert!(app.mark_review_comment_synced(reply_id, clean));

        let mut incoming = app
            .review_comments
            .iter()
            .find(|comment| comment.id == reply_id)
            .unwrap()
            .clone();
        incoming.id = 0;
        incoming.body = "Reply from GitHub".to_string();
        let pulled_id = app.upsert_provider_review_comment(incoming);

        assert_eq!(pulled_id, reply_id);
        assert_eq!(app.review_comments.len(), 2);
        assert_eq!(
            app.review_comments
                .iter()
                .find(|comment| comment.id == reply_id)
                .unwrap()
                .body,
            "Reply from GitHub"
        );
    }

    #[test]
    fn pull_without_baseline_imports_remote_resolution() {
        let mut app = test_app();
        let mut existing = line_comment();
        existing.id = 1;
        existing.provider = Some(provider_link("clean"));
        app.review_comments.push(existing);

        let mut incoming = line_comment();
        incoming.resolved = true;
        incoming.provider = Some(provider_link("clean"));
        incoming.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        incoming.provider.as_mut().unwrap().thread_resolved = Some(true);

        assert_eq!(app.upsert_provider_review_comment(incoming), 1);
        assert!(app.review_comments[0].resolved);
        let provider = app.review_comments[0].provider.as_ref().unwrap();
        assert_eq!(provider.thread_resolved, Some(true));
        assert!(!provider.resolved_dirty);
    }

    #[test]
    fn pull_preserves_pending_review_thread_resolution() {
        let mut app = test_app();
        let mut existing = line_comment();
        existing.id = 1;
        existing.resolved = true;
        existing.provider = Some(provider_link("clean"));
        existing.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        existing.provider.as_mut().unwrap().thread_resolved = Some(false);
        app.review_comments.push(existing);

        let mut incoming = line_comment();
        incoming.resolved = false;
        incoming.provider = Some(provider_link("clean"));
        incoming.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        incoming.provider.as_mut().unwrap().thread_resolved = Some(false);

        assert_eq!(app.upsert_provider_review_comment(incoming), 1);
        assert!(app.review_comments[0].resolved);
        assert_eq!(
            app.review_comments[0]
                .provider
                .as_ref()
                .and_then(|provider| provider.thread_resolved),
            Some(false)
        );
    }

    #[test]
    fn pull_merges_thread_state_while_preserving_dirty_body() {
        let mut app = test_app();
        let mut existing = line_comment();
        existing.id = 1;
        existing.body = "local edit".to_string();
        existing.provider = Some(provider_link("dirty"));
        existing.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        existing.provider.as_mut().unwrap().thread_resolved = Some(false);
        app.review_comments.push(existing);

        let mut incoming = line_comment();
        incoming.body = "remote body".to_string();
        incoming.resolved = true;
        incoming.provider = Some(provider_link("clean"));
        incoming.provider.as_mut().unwrap().thread_id = Some("thread-1".to_string());
        incoming.provider.as_mut().unwrap().thread_resolved = Some(true);

        assert_eq!(app.upsert_provider_review_comment(incoming), 1);
        assert_eq!(app.review_comments[0].body, "local edit");
        assert!(app.review_comments[0].resolved);
        let provider = app.review_comments[0].provider.as_ref().unwrap();
        assert_eq!(provider.sync_state, "dirty");
        assert_eq!(provider.thread_resolved, Some(true));
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
    fn review_comment_focus_cycles_in_document_order() {
        for mode in [ViewMode::UnifiedPane, ViewMode::Split] {
            let diff = MultiFileDiff::from_file_pairs(vec![
                (
                    PathBuf::from("a.txt"),
                    "old\n".to_string(),
                    "new\n".to_string(),
                ),
                (
                    PathBuf::from("b.txt"),
                    "old\n".to_string(),
                    "new\n".to_string(),
                ),
            ]);
            let mut app = App::new(diff, mode, 0, false, None);
            app.set_review_persist_enabled(false);
            app.enable_review_mode();

            let mut line = line_comment();
            line.id = 10;
            line.anchor.file_path = "a.txt".to_string();
            line.anchor.anchor_key = "line|a.txt|new|1".to_string();
            let mut hunk = line.clone();
            hunk.id = 20;
            hunk.resolved = true;
            hunk.anchor.kind = ReviewTargetKind::Hunk;
            hunk.anchor.anchor_key = "hunk|a.txt|-|1".to_string();
            let mut other_file = line.clone();
            other_file.id = 30;
            other_file.outdated = true;
            other_file.anchor.file_index = 1;
            other_file.anchor.file_path = "b.txt".to_string();
            other_file.anchor.anchor_key = "line|b.txt|new|1".to_string();
            let mut deleted = line.clone();
            deleted.id = 40;
            deleted.deleted = true;
            app.review_comments = vec![other_file, hunk, deleted, line];

            assert!(app.focus_next_review_comment());
            assert_eq!(app.active_review_comment_id, Some(10));
            assert!(app.focus_next_review_comment());
            assert_eq!(app.active_review_comment_id, Some(20));
            assert!(app.focus_next_review_comment());
            assert_eq!(app.active_review_comment_id, Some(30));
            assert!(app.focus_next_review_comment());
            assert_eq!(app.active_review_comment_id, Some(10));
            assert!(app.focus_prev_review_comment());
            assert_eq!(app.active_review_comment_id, Some(30));

            app.active_review_comment_id = None;
            assert!(app.focus_prev_review_comment());
            assert_eq!(app.active_review_comment_id, Some(30));
        }
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
        let review_revision = app.review_revision();
        let fold_revision = app.review_fold_anchor_revision();

        assert!(app.delete_review_comment_letter('a'));
        assert_eq!(app.review_comment_count(), 0);
        assert!(app.review_revision() > review_revision);
        assert_ne!(app.review_fold_anchor_revision(), fold_revision);
    }

    #[test]
    fn deleting_thread_root_confirms_and_cascades_owned_replies() {
        let mut app = test_app();
        let mut root = line_comment();
        root.provider = Some(provider_link("clean"));

        let mut local_reply = line_comment();
        local_reply.id = 2;
        local_reply.in_reply_to = Some(1);
        local_reply.anchor.anchor_key = "reply|1|2".to_string();

        let mut nested_reply = line_comment();
        nested_reply.id = 3;
        nested_reply.in_reply_to = Some(2);
        nested_reply.anchor.anchor_key = "reply|1|3".to_string();

        let mut provider_reply = line_comment();
        provider_reply.id = 4;
        provider_reply.anchor.anchor_key = "reply|1|4".to_string();
        provider_reply.provider = Some(provider_link("clean"));
        provider_reply.provider.as_mut().unwrap().comment_id = "11".to_string();
        provider_reply.provider.as_mut().unwrap().in_reply_to_id = Some("10".to_string());

        let mut unowned_reply = provider_reply.clone();
        unowned_reply.id = 5;
        unowned_reply.can_edit = false;
        unowned_reply.anchor.anchor_key = "reply|1|5".to_string();
        unowned_reply.provider.as_mut().unwrap().comment_id = "12".to_string();

        let mut other_review_reply = unowned_reply.clone();
        other_review_reply.id = 6;
        other_review_reply.can_edit = true;
        other_review_reply.anchor.anchor_key = "reply|other|6".to_string();
        other_review_reply.provider.as_mut().unwrap().repo = "other/repo".to_string();

        app.review_comments = vec![
            root,
            local_reply,
            nested_reply,
            provider_reply,
            unowned_reply,
            other_review_reply,
        ];

        assert!(app.delete_review_comment_letter('a'));
        let confirmation = app.review_delete_confirmation_render().unwrap();
        assert_eq!(confirmation.title, "Delete comment");
        assert_eq!(confirmation.body, "Delete this comment and its 3 replies?");
        assert_eq!(app.review_comment_count(), 6);

        app.handle_review_delete_confirmation_key(KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));

        assert_eq!(app.review_comment_count(), 2);
        assert_eq!(
            app.review_comments
                .iter()
                .filter(|comment| !comment.deleted)
                .count(),
            2
        );
        assert!(
            !app.review_comments
                .iter()
                .find(|comment| comment.id == 5)
                .unwrap()
                .can_edit
        );
        assert_eq!(
            app.review_comments
                .iter()
                .find(|comment| comment.id == 4)
                .unwrap()
                .provider
                .as_ref()
                .unwrap()
                .sync_state,
            "deleted"
        );
    }

    #[test]
    fn deleting_reply_is_immediate_and_keeps_parent_and_sibling() {
        let mut app = test_app();
        let parent = line_comment();
        let mut first = line_comment();
        first.id = 2;
        first.in_reply_to = Some(1);
        first.anchor.anchor_key = "reply|1|2".to_string();
        let mut second = first.clone();
        second.id = 3;
        second.anchor.anchor_key = "reply|1|3".to_string();
        app.review_comments = vec![parent, first, second];

        assert!(app.request_delete_comment_by_id(2));
        assert!(!app.review_delete_confirmation_active());
        assert!(!app.review_comments[0].deleted);
        assert!(app.review_comments[1].deleted);
        assert!(!app.review_comments[2].deleted);
    }

    #[test]
    fn clearing_thread_root_in_editor_uses_cascade_confirmation() {
        let mut app = test_app();
        let parent = line_comment();
        let mut reply = line_comment();
        reply.id = 2;
        reply.in_reply_to = Some(1);
        reply.anchor.anchor_key = "reply|1|2".to_string();
        app.review_comments = vec![parent, reply];

        assert!(app.edit_review_comment_letter('a'));
        app.review_clear_editor_text();
        app.review_save_editor();

        assert_eq!(
            app.review_delete_confirmation_render().unwrap().body,
            "Delete this comment and its 1 reply?"
        );
        assert_eq!(app.review_comment_count(), 2);
    }

    #[test]
    fn submit_waits_for_empty_root_cascade_confirmation() {
        let mut app = test_app();
        let parent = line_comment();
        let mut reply = line_comment();
        reply.id = 2;
        reply.in_reply_to = Some(1);
        reply.anchor.anchor_key = "reply|1|2".to_string();
        app.review_comments = vec![parent, reply];

        assert!(app.edit_review_comment_letter('a'));
        app.review_clear_editor_text();
        app.submit_review_and_quit();

        assert!(app.review_delete_confirmation_active());
        assert!(!app.should_quit);
        assert_eq!(app.review_comment_count(), 2);
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
    fn focused_sidebar_comments_stay_visible_until_edited() {
        let mut single = test_app();
        single.review_comments.push(line_comment());
        single.goto_last_step();
        assert!(single.open_review_comment(0));
        assert_eq!(single.active_review_comment_id, Some(1));
        assert_eq!(
            single
                .review_comment_overlays_for_current_file()
                .iter()
                .map(|overlay| overlay.id)
                .collect::<Vec<_>>(),
            vec![1]
        );

        let mut thread = colliding_thread_app();
        assert!(thread.open_review_comment(1));
        assert_eq!(thread.active_review_comment_id, Some(2));
        assert_eq!(
            thread
                .review_comment_overlays_for_current_file()
                .iter()
                .map(|overlay| overlay.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(thread.focus_next_review_comment());
        assert!(thread.focus_prev_review_comment());
        assert_eq!(
            thread
                .review_comment_overlays_for_current_file()
                .iter()
                .map(|overlay| overlay.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        assert!(thread.start_review_comment_reply(1));
        assert_eq!(
            thread
                .review_comment_overlays_for_current_file()
                .iter()
                .map(|overlay| overlay.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        thread.review_cancel_editor();

        let mut new_anchor = thread.review_comments[0].anchor.clone();
        new_anchor.new_range = Some(ReviewRange { start: 2, end: 2 });
        new_anchor.anchor_key = "line|new.txt|new|2".to_string();
        thread.open_review_editor(new_anchor);
        assert_eq!(thread.active_review_comment_id, None);
        assert_eq!(
            thread
                .review_comment_overlays_for_current_file()
                .iter()
                .map(|overlay| overlay.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        thread.review_cancel_editor();

        assert!(thread.edit_review_comment_letter('b'));
        assert_eq!(
            thread
                .review_comment_overlays_for_current_file()
                .iter()
                .map(|overlay| overlay.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn comment_sidebar_click_flashes_review_card() {
        let mut app = test_app();
        let comment = line_comment();
        let id = comment.id;
        let key = comment.anchor.anchor_key.clone();
        app.review_comments.push(comment);

        assert!(app.open_review_comment(0));
        assert!(app.review_preview_flash_active(id, &key));
    }

    #[test]
    fn shared_anchor_preview_hover_tracks_comment_and_action_ids() {
        let mut app = test_app();
        let key = "line|new.txt|new|1".to_string();
        app.add_review_comment_preview_box(0, 0, 10, 1, 1, key.clone());
        app.add_review_preview_delete_box(0, 1, 10, 1, 2, key.clone());
        app.add_review_preview_edit_box(0, 2, 10, 1, 2, key);

        assert!(app.update_topbar_hover(1, 1));
        assert_eq!(app.review_preview_hover_id, Some(2));
        assert_eq!(app.review_preview_delete_hover, Some(2));
        assert_eq!(app.review_preview_edit_hover, None);
        assert!(app.update_topbar_hover(1, 2));
        assert_eq!(app.review_preview_hover_id, Some(2));
        assert_eq!(app.review_preview_delete_hover, None);
        assert_eq!(app.review_preview_edit_hover, Some(2));
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
                snapshot: None,
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
            resolved: false,
            outdated: false,
            reanchored: false,
            deleted: false,
            provider: None,
            in_reply_to: None,
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
    fn same_origin_comment_updates_in_place_without_id_remap() {
        let base = temp_path("same-origin-update");
        let mut writer = persistent_test_app(&base);
        writer.enable_review_mode();
        let mut stale = persistent_test_app(&base);
        stale.enable_review_mode();

        let original = line_comment();
        writer.review_comments.push(original.clone());
        writer.review_next_comment_id = 2;
        writer.persist_review_session();

        let mut updated = original;
        updated.updated_at = updated.updated_at.saturating_add(10);
        updated.anchor.new_range = Some(ReviewRange { start: 2, end: 2 });
        updated.anchor.anchor_key = "line|new.txt|new|2".to_string();
        stale.review_comments.push(updated);
        stale.review_next_comment_id = 2;
        stale.persist_review_session();

        let mut loaded = persistent_test_app(&base);
        loaded.load_review_mode();
        assert_eq!(loaded.review_comments.len(), 1);
        assert_eq!(loaded.review_comments[0].id, 1);
        assert_eq!(loaded.review_comments[0].updated_at, 11);
        assert_eq!(
            loaded.review_comments[0].anchor.new_range,
            Some(ReviewRange { start: 2, end: 2 })
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn author_detail_fill_on_reopen_updates_without_duplicate() {
        let base = temp_path("author-fill-reopen");
        let author = ReviewAuthor {
            name: "Reviewer".to_string(),
            email: Some("reviewer@example.com".to_string()),
            author_type: None,
            usernames: BTreeMap::new(),
            avatar_url: None,
        };
        let mut writer = persistent_test_app(&base);
        writer.set_review_author(Some(author.clone()));
        writer.enable_review_mode();
        assert_eq!(add_line_comment(&mut writer, "comment"), 1);

        let mut enriched = author;
        enriched
            .usernames
            .insert("github".to_string(), "reviewer".to_string());
        enriched.avatar_url = Some("https://example.com/avatar.png".to_string());
        let mut reopened = persistent_test_app(&base);
        reopened.set_review_author(Some(enriched));
        reopened.enable_review_mode();
        assert_eq!(reopened.review_comments.len(), 1);
        assert_eq!(
            reopened.review_comments[0]
                .author
                .as_ref()
                .and_then(|author| author.usernames.get("github"))
                .map(String::as_str),
            Some("reviewer")
        );

        let mut loaded = persistent_test_app(&base);
        loaded.enable_review_mode();
        assert_eq!(loaded.review_comments.len(), 1);
        assert_eq!(loaded.review_comments[0].id, 1);

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn different_same_second_comments_still_remap() {
        let base = temp_path("different-same-second");
        let mut first = persistent_test_app(&base);
        first.enable_review_mode();
        let mut second = persistent_test_app(&base);
        second.enable_review_mode();

        let mut first_comment = line_comment();
        first_comment.body = "first".to_string();
        first.review_comments.push(first_comment);
        first.review_next_comment_id = 2;
        first.persist_review_session();

        let mut second_comment = line_comment();
        second_comment.body = "second".to_string();
        second.review_comments.push(second_comment);
        second.review_next_comment_id = 2;
        second.persist_review_session();

        let mut loaded = persistent_test_app(&base);
        loaded.load_review_mode();
        assert_eq!(loaded.review_comments.len(), 2);
        assert_ne!(loaded.review_comments[0].id, loaded.review_comments[1].id);
        assert_eq!(
            loaded
                .review_comments
                .iter()
                .map(|comment| comment.body.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["first", "second"])
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn create_reply_and_reopen_keeps_one_copy_of_each() {
        let base = temp_path("create-reply-reopen");
        let mut app = persistent_test_app(&base);
        app.enable_review_mode();
        let root_id = add_line_comment(&mut app, "root");
        let reply_id = app
            .add_review_reply_from_cli(root_id, "reply".to_string())
            .unwrap();
        app.persist_review_session();

        let mut loaded = persistent_test_app(&base);
        loaded.load_review_mode();
        assert_eq!(loaded.review_comments.len(), 2);
        assert_eq!(
            loaded
                .review_comments
                .iter()
                .filter(|comment| comment.id == root_id)
                .count(),
            1
        );
        let root = loaded
            .review_comments
            .iter()
            .find(|comment| comment.id == root_id)
            .unwrap();
        let reply = loaded
            .review_comments
            .iter()
            .find(|comment| comment.id == reply_id)
            .unwrap();
        assert_eq!(reply.in_reply_to, Some(root_id));
        assert_eq!(reply.anchor.anchor_key, root.anchor.anchor_key);

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn concurrent_new_comments_keep_both_rows() {
        let base = temp_path("concurrent-add");
        let mut first = persistent_test_app(&base);
        first.enable_review_mode();
        let mut second = persistent_test_app(&base);
        second.enable_review_mode();

        assert_eq!(add_line_comment(&mut first, "first"), 1);
        assert_eq!(add_line_comment(&mut second, "second"), 2);

        let mut loaded = persistent_test_app(&base);
        loaded.load_review_mode();
        let bodies = loaded
            .review_comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>();
        assert!(bodies.contains(&"first"));
        assert!(bodies.contains(&"second"));

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn stale_writer_cannot_resurrect_concurrent_thread_tombstones() {
        let base = temp_path("concurrent-delete-tombstone");
        let mut creator = persistent_test_app(&base);
        creator.enable_review_mode();
        let root_id = add_line_comment(&mut creator, "root");
        let reply_id = creator
            .add_review_reply_from_cli(root_id, "reply".to_string())
            .unwrap();

        let mut stale = persistent_test_app(&base);
        stale.load_review_mode();
        let mut stale_remover = persistent_test_app(&base);
        stale_remover.load_review_mode();
        let mut late_reply_writer = persistent_test_app(&base);
        late_reply_writer.load_review_mode();
        let mut parent_absent_writer = persistent_test_app(&base);
        parent_absent_writer.load_review_mode();
        let mut deleter = persistent_test_app(&base);
        deleter.load_review_mode();

        assert!(deleter.remove_review_comment_from_cli(root_id));
        stale_remover.review_comments.clear();
        stale_remover.persist_review_session();
        assert!(stale.edit_review_comment_from_cli(root_id, "stale edit".to_string()));
        let late_reply_id = late_reply_writer
            .add_review_reply_from_cli(root_id, "late reply".to_string())
            .unwrap();
        assert!(
            late_reply_writer
                .review_comments
                .iter()
                .find(|comment| comment.id == late_reply_id)
                .unwrap()
                .deleted
        );
        parent_absent_writer.review_comments.clear();
        let mut orphan = line_comment();
        orphan.id = late_reply_id;
        orphan.body = "orphan".to_string();
        orphan.in_reply_to = Some(root_id);
        orphan.anchor.anchor_key = "reply|1|orphan".to_string();
        parent_absent_writer.review_comments.push(orphan);
        parent_absent_writer.persist_review_session();
        assert!(parent_absent_writer.review_comments[0].deleted);

        for id in [root_id, reply_id] {
            assert!(
                stale
                    .review_comments
                    .iter()
                    .find(|comment| comment.id == id)
                    .unwrap()
                    .deleted
            );
            assert!(
                stale
                    .review_session_baseline
                    .iter()
                    .find(|comment| comment.id == id)
                    .unwrap()
                    .deleted
            );
        }

        let mut loaded = persistent_test_app(&base);
        loaded.load_review_mode();
        assert_eq!(loaded.review_comment_count(), 0);
        assert_eq!(loaded.review_comments.len(), 4);
        assert!(loaded.review_comments.iter().all(|comment| comment.deleted));

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn stale_provider_reply_to_deleted_root_is_tombstoned() {
        let base = temp_path("concurrent-provider-reply-delete");
        let mut creator = persistent_test_app(&base);
        creator.enable_review_mode();
        let mut root = line_comment();
        let mut provider = provider_link("clean");
        provider.thread_id = Some("thread-1".to_string());
        root.provider = Some(provider);
        creator.review_comments = vec![root];
        creator.review_next_comment_id = 2;
        creator.persist_review_session();

        let mut stale = persistent_test_app(&base);
        stale.load_review_mode();
        let mut deleter = persistent_test_app(&base);
        deleter.load_review_mode();
        assert!(deleter.remove_review_comment_from_cli(1));

        let reply_id = stale
            .add_review_reply_from_cli(1, "late provider reply".to_string())
            .unwrap();
        let reply = stale
            .review_comments
            .iter()
            .find(|comment| comment.id == reply_id)
            .unwrap();
        assert!(reply.deleted);
        assert_eq!(
            reply
                .provider
                .as_ref()
                .map(|provider| provider.sync_state.as_str()),
            Some("deleted")
        );

        let mut loaded = persistent_test_app(&base);
        loaded.load_review_mode();
        assert_eq!(loaded.review_comment_count(), 0);
        assert!(loaded.review_comments.iter().all(|comment| comment.deleted));

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn concurrent_parent_id_remap_updates_local_reply() {
        let base = temp_path("concurrent-reply-remap");
        let mut first = persistent_test_app(&base);
        first.enable_review_mode();
        let mut second = persistent_test_app(&base);
        second.enable_review_mode();

        assert_eq!(add_line_comment(&mut first, "concurrent comment"), 1);
        assert_eq!(add_line_comment(&mut first, "other concurrent comment"), 2);
        let mut parent = line_comment();
        parent.id = 1;
        parent.body = "local parent".to_string();
        let mut child = line_comment();
        child.id = 2;
        child.body = "local reply".to_string();
        child.anchor.anchor_key = "reply|1|2".to_string();
        child.in_reply_to = Some(1);
        second.review_comments = vec![parent, child];
        second.review_next_comment_id = 3;
        second.persist_review_session();

        let mut loaded = persistent_test_app(&base);
        loaded.load_review_mode();
        let parent = loaded
            .review_comments
            .iter()
            .find(|comment| comment.body == "local parent")
            .unwrap();
        let parent_id = parent.id;
        let reply = loaded
            .review_comments
            .iter()
            .find(|comment| comment.body == "local reply")
            .unwrap();
        assert_ne!(parent_id, 1);
        assert_ne!(reply.id, 2);
        assert_eq!(reply.in_reply_to, Some(parent_id));
        assert_eq!(reply.anchor.anchor_key, parent.anchor.anchor_key);

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn stale_writer_does_not_revert_unrelated_comment() {
        let base = temp_path("stale-writer");
        let mut initial = persistent_test_app(&base);
        initial.enable_review_mode();
        let id = add_line_comment(&mut initial, "original");

        let mut editor = persistent_test_app(&base);
        editor.load_review_mode();
        let mut stale = persistent_test_app(&base);
        stale.load_review_mode();

        assert!(editor.edit_review_comment_from_cli(id, "edited".to_string()));
        assert_eq!(add_line_comment(&mut stale, "new"), 2);

        let mut loaded = persistent_test_app(&base);
        loaded.load_review_mode();
        let by_id = loaded
            .review_comments
            .iter()
            .map(|comment| (comment.id, comment.body.as_str()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(by_id.get(&id).copied(), Some("edited"));
        assert_eq!(by_id.get(&2).copied(), Some("new"));

        let _ = std::fs::remove_dir_all(base);
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
        assert!(!external.comments_tab_unseen);
        assert!(external.set_review_comment_resolved_from_cli(id, true));

        loaded.multi_diff.files.clear();
        loaded.last_review_db_check = Instant::now() - Duration::from_secs(2);
        assert!(loaded.maybe_watch_reload_review_state());
        assert_eq!(loaded.review_comment_count(), 2);
        assert!(loaded.review_markdown().contains("external note"));
        let resolved = loaded
            .review_comments
            .iter()
            .find(|comment| comment.id == id)
            .unwrap();
        assert!(resolved.resolved);
        assert!(!resolved.outdated);
        assert!(loaded.comments_tab_unseen);
        loaded.show_comments_sidebar();
        assert!(!loaded.comments_tab_unseen);

        external
            .add_review_comment_from_cli(
                "new.txt",
                ReviewTargetKind::Line,
                Some(ReviewSide::New),
                None,
                Some(ReviewRange { start: 1, end: 1 }),
                "another external note".to_string(),
            )
            .unwrap();
        loaded.last_review_db_check = Instant::now() - Duration::from_secs(2);
        assert!(loaded.maybe_watch_reload_review_state());
        assert!(!loaded.comments_tab_unseen);
        loaded.last_review_db_check = Instant::now() - Duration::from_secs(2);
        assert!(!loaded.maybe_watch_reload_review_state());
        assert!(!loaded.comments_tab_unseen);

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
