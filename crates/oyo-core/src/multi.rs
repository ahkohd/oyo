//! Multi-file diff support

use crate::change::{Change, ChangeSpan};
use crate::diff::{DiffEngine, DiffResult};
use crate::git::{ChangedFile, FileStatus};
use crate::step::{DiffNavigator, StepDirection};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MultiDiffError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Git error: {0}")]
    Git(#[from] crate::git::GitError),
}

/// A file entry in a multi-file diff
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub old_source_path: Option<PathBuf>,
    pub new_source_path: Option<PathBuf>,
    pub display_name: String,
    pub status: FileStatus,
    pub insertions: usize,
    pub deletions: usize,
    pub binary: bool,
}

#[derive(Debug, Clone)]
pub struct RawFileDiff {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub old_source_path: Option<PathBuf>,
    pub new_source_path: Option<PathBuf>,
    pub status: FileStatus,
    pub old_content: String,
    pub new_content: String,
    pub binary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSide {
    Old,
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentSource {
    Empty,
    File(PathBuf),
    GitObject {
        repo_root: PathBuf,
        spec: String,
    },
    JjFile {
        repo_root: PathBuf,
        revision: String,
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingFileContent {
    pub file_index: usize,
    pub old: ContentSource,
    pub new: ContentSource,
}

pub struct PreparedFileContent {
    old: Arc<str>,
    new: Arc<str>,
    binary: bool,
    precomputed: Option<PrecomputedDiff>,
    status: DiffStatus,
}

#[derive(Debug, Clone)]
struct SourceRoots {
    old: PathBuf,
    new: PathBuf,
}

/// Multi-file diff session
pub struct MultiFileDiff {
    /// All files being diffed
    pub files: Vec<FileEntry>,
    /// Currently selected file index
    pub selected_index: usize,
    /// Navigators for each file (lazy loaded)
    navigators: Vec<Option<DiffNavigator>>,
    /// True when the current navigator is built from a placeholder diff
    navigator_is_placeholder: Vec<bool>,
    /// Repository root (if in git mode)
    #[allow(dead_code)]
    repo_root: Option<PathBuf>,
    /// Git diff mode (if in git mode)
    git_mode: Option<GitDiffMode>,
    /// Real file roots for non-git diffs, when known.
    source_roots: Option<SourceRoots>,
    /// Old contents for each file
    old_contents: Vec<Arc<str>>,
    /// New contents for each file
    new_contents: Vec<Arc<str>>,
    /// Precomputed diffs (used for large files to avoid expensive diffing on demand)
    precomputed_diffs: Vec<Option<PrecomputedDiff>>,
    /// Diff readiness state per file
    diff_statuses: Vec<DiffStatus>,
    /// Content requests not yet handed to the UI worker.
    pending_content: Vec<Option<PendingFileContent>>,
    /// Keep provider stats stable when real content arrives.
    stats_authoritative: bool,
    /// Blame sources supplied by non-Git frontends backed by Git objects.
    blame_sources_override: Option<(BlameSource, BlameSource)>,
}

#[derive(Debug, Clone)]
enum GitDiffMode {
    Uncommitted,
    Staged,
    IndexRange { from: String, to_index: bool },
    Range { from: String, to: String },
}

/// Source for blame lookups.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BlameSource {
    Worktree,
    Index,
    Commit(String),
    JjRev(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    Loading,
    Ready,
    Deferred,
    Computing,
    Failed,
    Disabled,
}

#[derive(Debug, Clone)]
enum PrecomputedDiff {
    Placeholder(DiffResult),
    Ready(DiffResult),
}

const DEFAULT_DIFF_MAX_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_FULL_CONTEXT_MAX_BYTES: u64 = 2 * 1024 * 1024;
static DIFF_MAX_BYTES: AtomicU64 = AtomicU64::new(DEFAULT_DIFF_MAX_BYTES);
static FULL_CONTEXT_MAX_BYTES: AtomicU64 = AtomicU64::new(DEFAULT_FULL_CONTEXT_MAX_BYTES);
static DIFF_DEFER: AtomicBool = AtomicBool::new(true);

pub const DEFAULT_SCAN_IGNORE_GLOBS: &[&str] =
    &[".git/**", ".jj/**", ".hg/**", ".svn/**", ".oyo/reviews/**"];

#[derive(Debug, Clone)]
pub struct DirectoryScanOptions {
    pub git_ignore: bool,
    pub ignore_globs: Vec<String>,
}

impl Default for DirectoryScanOptions {
    fn default() -> Self {
        Self {
            git_ignore: true,
            ignore_globs: DEFAULT_SCAN_IGNORE_GLOBS
                .iter()
                .map(|pattern| (*pattern).to_string())
                .collect(),
        }
    }
}

impl MultiFileDiff {
    const MAX_TEXT_BYTES: u64 = 32 * 1024 * 1024;
    const MAX_WORD_LEVEL_BYTES: u64 = 2 * 1024 * 1024;
    const MAX_LINE_CHARS: usize = 16_384;

    pub fn set_diff_max_bytes(max_bytes: u64) {
        let limit = max_bytes.max(1);
        DIFF_MAX_BYTES.store(limit, Ordering::Relaxed);
    }

    pub fn set_full_context_max_bytes(max_bytes: u64) {
        let limit = max_bytes.max(1);
        FULL_CONTEXT_MAX_BYTES.store(limit, Ordering::Relaxed);
    }

    pub fn set_diff_defer(enabled: bool) {
        DIFF_DEFER.store(enabled, Ordering::Relaxed);
    }

    fn diff_max_bytes() -> u64 {
        DIFF_MAX_BYTES.load(Ordering::Relaxed)
    }

    fn full_context_max_bytes() -> u64 {
        FULL_CONTEXT_MAX_BYTES.load(Ordering::Relaxed)
    }

    fn diff_defer_enabled() -> bool {
        DIFF_DEFER.load(Ordering::Relaxed)
    }

    fn decode_bytes(bytes: Vec<u8>) -> (String, bool) {
        if bytes.is_empty() {
            return (String::new(), false);
        }
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            return (String::new(), true);
        }
        let text = String::from_utf8_lossy(&bytes).to_string();
        (Self::normalize_text(text), false)
    }

    fn text_too_large(size: u64) -> bool {
        size > Self::MAX_TEXT_BYTES
    }

    fn read_text_or_binary(path: &Path) -> (String, bool) {
        if let Ok(metadata) = path.metadata() {
            if Self::text_too_large(metadata.len()) {
                return (String::new(), true);
            }
        }
        let bytes = std::fs::read(path).unwrap_or_default();
        Self::decode_bytes(bytes)
    }

    fn read_git_commit_or_binary(repo_root: &Path, commit: &str, path: &Path) -> (String, bool) {
        if let Some(size) = crate::git::get_file_at_commit_size(repo_root, commit, path) {
            if Self::text_too_large(size) {
                return (String::new(), true);
            }
        }
        let bytes =
            crate::git::get_file_at_commit_bytes(repo_root, commit, path).unwrap_or_default();
        Self::decode_bytes(bytes)
    }

    fn read_git_index_or_binary(repo_root: &Path, path: &Path) -> (String, bool) {
        if let Some(size) = crate::git::get_staged_content_size(repo_root, path) {
            if Self::text_too_large(size) {
                return (String::new(), true);
            }
        }
        let bytes = crate::git::get_staged_content_bytes(repo_root, path).unwrap_or_default();
        Self::decode_bytes(bytes)
    }

    fn read_git_batch_or_binary(
        repo_root: &Path,
        specs: Vec<Option<String>>,
    ) -> Result<Vec<(String, bool)>, MultiDiffError> {
        let requested = specs.iter().flatten().cloned().collect::<Vec<_>>();
        let mut objects =
            crate::git::get_objects_batch(repo_root, &requested, Self::MAX_TEXT_BYTES)?.into_iter();
        Ok(specs
            .into_iter()
            .map(|spec| match spec {
                None => (String::new(), false),
                Some(_) => match objects.next().flatten() {
                    Some(bytes) => Self::decode_bytes(bytes),
                    None => (String::new(), true),
                },
            })
            .collect())
    }

    fn diff_strings(old: &str, new: &str) -> crate::diff::DiffResult {
        let max_len = old.len().max(new.len()) as u64;
        let word_level = max_len <= Self::MAX_WORD_LEVEL_BYTES;
        let context_limit = Self::full_context_max_bytes().min(Self::diff_max_bytes());
        let context_lines = if max_len > context_limit {
            3
        } else {
            usize::MAX
        };
        DiffEngine::new()
            .with_word_level(word_level)
            .with_context(context_lines)
            .diff_strings(old, new)
    }

    pub fn compute_diff(old: &str, new: &str) -> crate::diff::DiffResult {
        Self::diff_strings(old, new)
    }

    fn should_defer_diff(old: &str, new: &str) -> bool {
        let max_len = old.len().max(new.len()) as u64;
        max_len > Self::diff_max_bytes()
    }

    fn context_only_diff(text: &str) -> DiffResult {
        let mut changes = Vec::new();
        for (change_id, line) in text.split('\n').enumerate() {
            let line_num = change_id + 1;
            let span = ChangeSpan::equal(line).with_lines(Some(line_num), Some(line_num));
            changes.push(Change::single(change_id, span));
        }

        DiffResult {
            changes,
            significant_changes: Vec::new(),
            hunks: Vec::new(),
            insertions: 0,
            deletions: 0,
        }
    }

    fn diff_stats(old: &str, new: &str, binary: bool) -> (usize, usize) {
        if binary {
            return (0, 0);
        }
        let max_len = old.len().max(new.len()) as u64;
        if max_len > Self::MAX_WORD_LEVEL_BYTES {
            let old_lines = old.lines().count();
            let new_lines = new.lines().count();
            if old_lines == 0 {
                return (new_lines, 0);
            }
            if new_lines == 0 {
                return (0, old_lines);
            }
            return (0, 0);
        }
        let diff = Self::diff_strings(old, new);
        (diff.insertions, diff.deletions)
    }

    fn normalize_text(text: String) -> String {
        if !text.lines().any(|line| line.len() > Self::MAX_LINE_CHARS) {
            return text;
        }
        let mut out = String::new();
        for chunk in text.split_inclusive('\n') {
            let (line, has_newline) = if let Some(line) = chunk.strip_suffix('\n') {
                (line, true)
            } else {
                (chunk, false)
            };
            if line.len() > Self::MAX_LINE_CHARS {
                let cutoff = line
                    .char_indices()
                    .nth(Self::MAX_LINE_CHARS)
                    .map(|(idx, _)| idx)
                    .unwrap_or_else(|| line.len());
                out.push_str(&line[..cutoff]);
                out.push('…');
            } else {
                out.push_str(line);
            }
            if has_newline {
                out.push('\n');
            }
        }
        out
    }

    fn maybe_defer_diff(
        old_content: String,
        new_content: String,
        binary: bool,
    ) -> (String, String, Option<PrecomputedDiff>, DiffStatus) {
        if binary {
            return (String::new(), String::new(), None, DiffStatus::Disabled);
        }
        if Self::should_defer_diff(&old_content, &new_content) {
            let display = if new_content.is_empty() {
                old_content.clone()
            } else {
                new_content.clone()
            };
            let diff = Self::context_only_diff(&display);
            let status = if Self::diff_defer_enabled() {
                DiffStatus::Deferred
            } else {
                DiffStatus::Disabled
            };
            return (
                old_content,
                new_content,
                Some(PrecomputedDiff::Placeholder(diff)),
                status,
            );
        }
        (old_content, new_content, None, DiffStatus::Ready)
    }

    pub fn from_pending_files(
        repo_root: Option<PathBuf>,
        entries: Vec<(FileEntry, ContentSource, ContentSource)>,
        stats_authoritative: bool,
    ) -> Self {
        let count = entries.len();
        let mut files = Vec::with_capacity(count);
        let mut pending_content = Vec::with_capacity(count);
        for (file_index, (file, old, new)) in entries.into_iter().enumerate() {
            files.push(file);
            pending_content.push(Some(PendingFileContent {
                file_index,
                old,
                new,
            }));
        }
        Self {
            files,
            selected_index: 0,
            navigators: (0..count).map(|_| None).collect(),
            navigator_is_placeholder: vec![false; count],
            repo_root,
            git_mode: None,
            source_roots: None,
            old_contents: vec![Arc::from(""); count],
            new_contents: vec![Arc::from(""); count],
            precomputed_diffs: (0..count)
                .map(|_| {
                    Some(PrecomputedDiff::Placeholder(Self::context_only_diff(
                        "Loading file...",
                    )))
                })
                .collect(),
            diff_statuses: vec![DiffStatus::Loading; count],
            pending_content,
            stats_authoritative,
            blame_sources_override: None,
        }
    }

    /// Create from already loaded file contents.
    pub fn from_raw_files(repo_root: Option<PathBuf>, raw_files: Vec<RawFileDiff>) -> Self {
        let eager_stats = raw_files.len() <= 4;
        let mut files = Vec::new();
        let mut old_contents = Vec::new();
        let mut new_contents = Vec::new();
        let mut precomputed_diffs = Vec::new();
        let mut diff_statuses = Vec::new();

        for raw in raw_files {
            let binary = raw.binary;
            let (insertions, deletions) = if eager_stats {
                Self::diff_stats(&raw.old_content, &raw.new_content, binary)
            } else {
                (0, 0)
            };
            let (old_content, new_content, precomputed, diff_status) =
                Self::maybe_defer_diff(raw.old_content, raw.new_content, binary);

            files.push(FileEntry {
                display_name: raw.path.display().to_string(),
                path: raw.path,
                old_path: raw.old_path,
                old_source_path: raw.old_source_path,
                new_source_path: raw.new_source_path,
                status: raw.status,
                insertions,
                deletions,
                binary,
            });
            old_contents.push(Arc::from(old_content));
            new_contents.push(Arc::from(new_content));
            precomputed_diffs.push(precomputed);
            diff_statuses.push(diff_status);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();
        let navigator_is_placeholder = vec![false; files.len()];
        Self {
            files,
            selected_index: 0,
            navigators,
            navigator_is_placeholder,
            repo_root,
            git_mode: None,
            source_roots: None,
            old_contents,
            new_contents,
            precomputed_diffs,
            diff_statuses,
            pending_content: Vec::new(),
            stats_authoritative: false,
            blame_sources_override: None,
        }
    }

    fn from_git_pending(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
        stats: std::collections::HashMap<PathBuf, (usize, usize)>,
        git_mode: GitDiffMode,
        sources: impl Fn(&ChangedFile) -> (ContentSource, ContentSource),
    ) -> Self {
        let mut files = Vec::with_capacity(changes.len());
        let mut pending_content = Vec::with_capacity(changes.len());
        for (file_index, change) in changes.into_iter().enumerate() {
            let (old, new) = sources(&change);
            let (insertions, deletions) = stats.get(&change.path).copied().unwrap_or_else(|| {
                if change.status == FileStatus::Untracked {
                    std::fs::read_to_string(repo_root.join(&change.path))
                        .map(|content| (content.lines().count(), 0))
                        .unwrap_or((0, 0))
                } else {
                    (0, 0)
                }
            });
            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                old_source_path: None,
                new_source_path: None,
                status: change.status,
                insertions,
                deletions,
                binary: false,
            });
            pending_content.push(Some(PendingFileContent {
                file_index,
                old,
                new,
            }));
        }
        let count = files.len();
        Self {
            files,
            selected_index: 0,
            navigators: (0..count).map(|_| None).collect(),
            navigator_is_placeholder: vec![false; count],
            repo_root: Some(repo_root),
            git_mode: Some(git_mode),
            source_roots: None,
            old_contents: vec![Arc::from(""); count],
            new_contents: vec![Arc::from(""); count],
            precomputed_diffs: (0..count)
                .map(|_| {
                    Some(PrecomputedDiff::Placeholder(Self::context_only_diff(
                        "Loading file...",
                    )))
                })
                .collect(),
            diff_statuses: vec![DiffStatus::Loading; count],
            pending_content,
            stats_authoritative: true,
            blame_sources_override: None,
        }
    }

    pub fn from_git_changes_lazy(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
    ) -> Result<Self, MultiDiffError> {
        let stats = match crate::git::get_diff_numstat(&repo_root, &["HEAD".to_string()]) {
            Ok(stats) => stats,
            Err(_)
                if changes.iter().all(|change| {
                    matches!(change.status, FileStatus::Added | FileStatus::Untracked)
                }) =>
            {
                crate::git::get_diff_numstat(
                    &repo_root,
                    &["4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string()],
                )
                .unwrap_or_default()
            }
            Err(error) => return Err(error.into()),
        };
        let root = repo_root.clone();
        Ok(Self::from_git_pending(
            repo_root,
            changes,
            stats,
            GitDiffMode::Uncommitted,
            move |change| {
                let old = match change.status {
                    FileStatus::Added | FileStatus::Untracked => ContentSource::Empty,
                    _ => ContentSource::GitObject {
                        repo_root: root.clone(),
                        spec: format!(
                            "HEAD:{}",
                            change.old_path.as_deref().unwrap_or(&change.path).display()
                        ),
                    },
                };
                let new = match change.status {
                    FileStatus::Deleted => ContentSource::Empty,
                    _ => ContentSource::File(root.join(&change.path)),
                };
                (old, new)
            },
        ))
    }

    pub fn from_git_staged_lazy(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
    ) -> Result<Self, MultiDiffError> {
        let stats = crate::git::get_diff_numstat(&repo_root, &["--cached".to_string()])?;
        let root = repo_root.clone();
        Ok(Self::from_git_pending(
            repo_root,
            changes,
            stats,
            GitDiffMode::Staged,
            move |change| {
                let old = match change.status {
                    FileStatus::Added | FileStatus::Untracked => ContentSource::Empty,
                    _ => ContentSource::GitObject {
                        repo_root: root.clone(),
                        spec: format!(
                            "HEAD:{}",
                            change.old_path.as_deref().unwrap_or(&change.path).display()
                        ),
                    },
                };
                let new = match change.status {
                    FileStatus::Deleted => ContentSource::Empty,
                    _ => ContentSource::GitObject {
                        repo_root: root.clone(),
                        spec: format!(":{}", change.path.display()),
                    },
                };
                (old, new)
            },
        ))
    }

    pub fn from_git_index_range_lazy(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
        from: String,
        to_index: bool,
    ) -> Result<Self, MultiDiffError> {
        let mut args = vec!["--cached".to_string()];
        if !to_index {
            args.push("-R".to_string());
        }
        args.push(from.clone());
        let stats = crate::git::get_diff_numstat(&repo_root, &args)?;
        let root = repo_root.clone();
        let mode_from = from.clone();
        Ok(Self::from_git_pending(
            repo_root,
            changes,
            stats,
            GitDiffMode::IndexRange { from, to_index },
            move |change| {
                let old = match change.status {
                    FileStatus::Added | FileStatus::Untracked => ContentSource::Empty,
                    _ if to_index => ContentSource::GitObject {
                        repo_root: root.clone(),
                        spec: format!(
                            "{mode_from}:{}",
                            change.old_path.as_deref().unwrap_or(&change.path).display()
                        ),
                    },
                    _ => ContentSource::GitObject {
                        repo_root: root.clone(),
                        spec: format!(
                            ":{}",
                            change.old_path.as_deref().unwrap_or(&change.path).display()
                        ),
                    },
                };
                let new = match change.status {
                    FileStatus::Deleted => ContentSource::Empty,
                    _ if to_index => ContentSource::GitObject {
                        repo_root: root.clone(),
                        spec: format!(":{}", change.path.display()),
                    },
                    _ => ContentSource::GitObject {
                        repo_root: root.clone(),
                        spec: format!("{mode_from}:{}", change.path.display()),
                    },
                };
                (old, new)
            },
        ))
    }

    pub fn from_git_range_lazy(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
        from: String,
        to: String,
    ) -> Result<Self, MultiDiffError> {
        let stats = crate::git::get_diff_numstat(&repo_root, &[format!("{from}..{to}")])?;
        let root = repo_root.clone();
        let old_rev = from.clone();
        let new_rev = to.clone();
        Ok(Self::from_git_pending(
            repo_root,
            changes,
            stats,
            GitDiffMode::Range { from, to },
            move |change| {
                let old = match change.status {
                    FileStatus::Added | FileStatus::Untracked => ContentSource::Empty,
                    _ => ContentSource::GitObject {
                        repo_root: root.clone(),
                        spec: format!(
                            "{old_rev}:{}",
                            change.old_path.as_deref().unwrap_or(&change.path).display()
                        ),
                    },
                };
                let new = match change.status {
                    FileStatus::Deleted => ContentSource::Empty,
                    _ => ContentSource::GitObject {
                        repo_root: root.clone(),
                        spec: format!("{new_rev}:{}", change.path.display()),
                    },
                };
                (old, new)
            },
        ))
    }

    /// Create from a list of changed files (git mode)
    pub fn from_git_changes(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
    ) -> Result<Self, MultiDiffError> {
        let mut files = Vec::new();
        let mut old_contents = Vec::new();
        let mut new_contents = Vec::new();
        let mut precomputed_diffs = Vec::new();
        let mut diff_statuses = Vec::new();
        let stats = match crate::git::get_diff_numstat(&repo_root, &["HEAD".to_string()]) {
            Ok(stats) => stats,
            Err(_)
                if changes.iter().all(|change| {
                    matches!(change.status, FileStatus::Added | FileStatus::Untracked)
                }) =>
            {
                crate::git::get_diff_numstat(
                    &repo_root,
                    &["4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string()],
                )
                .unwrap_or_default()
            }
            Err(error) => return Err(error.into()),
        };
        let old_sides = Self::read_git_batch_or_binary(
            &repo_root,
            changes
                .iter()
                .map(|change| match change.status {
                    FileStatus::Added | FileStatus::Untracked => None,
                    _ => Some(format!(
                        "HEAD:{}",
                        change.old_path.as_deref().unwrap_or(&change.path).display()
                    )),
                })
                .collect(),
        )?;
        for (change, (old_content, old_binary)) in changes.into_iter().zip(old_sides) {
            let (new_content, new_binary) = match change.status {
                FileStatus::Deleted => (String::new(), false),
                _ => {
                    let full_path = repo_root.join(&change.path);
                    Self::read_text_or_binary(&full_path)
                }
            };

            let binary = old_binary || new_binary;
            let (insertions, deletions) = stats.get(&change.path).copied().unwrap_or_else(|| {
                if change.status == FileStatus::Untracked && !binary {
                    (new_content.lines().count(), 0)
                } else {
                    (0, 0)
                }
            });
            let (old_content, new_content, precomputed, diff_status) =
                Self::maybe_defer_diff(old_content, new_content, binary);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                old_source_path: None,
                new_source_path: None,
                status: change.status,
                insertions,
                deletions,
                binary,
            });

            old_contents.push(Arc::from(old_content));
            new_contents.push(Arc::from(new_content));
            precomputed_diffs.push(precomputed);
            diff_statuses.push(diff_status);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();
        let navigator_is_placeholder = vec![false; files.len()];

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            navigator_is_placeholder,
            repo_root: Some(repo_root),
            git_mode: Some(GitDiffMode::Uncommitted),
            source_roots: None,
            old_contents,
            new_contents,
            precomputed_diffs,
            diff_statuses,
            pending_content: Vec::new(),
            stats_authoritative: true,
            blame_sources_override: None,
        })
    }

    /// Create from staged git changes (index vs HEAD)
    pub fn from_git_staged(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
    ) -> Result<Self, MultiDiffError> {
        let mut files = Vec::new();
        let mut old_contents = Vec::new();
        let mut new_contents = Vec::new();
        let mut precomputed_diffs = Vec::new();
        let mut diff_statuses = Vec::new();
        let stats = crate::git::get_diff_numstat(&repo_root, &["--cached".to_string()])?;
        let old_sides = Self::read_git_batch_or_binary(
            &repo_root,
            changes
                .iter()
                .map(|change| match change.status {
                    FileStatus::Added | FileStatus::Untracked => None,
                    _ => Some(format!(
                        "HEAD:{}",
                        change.old_path.as_deref().unwrap_or(&change.path).display()
                    )),
                })
                .collect(),
        )?;
        let new_sides = Self::read_git_batch_or_binary(
            &repo_root,
            changes
                .iter()
                .map(|change| match change.status {
                    FileStatus::Deleted => None,
                    _ => Some(format!(":{}", change.path.display())),
                })
                .collect(),
        )?;
        for ((change, (old_content, old_binary)), (new_content, new_binary)) in
            changes.into_iter().zip(old_sides).zip(new_sides)
        {
            let binary = old_binary || new_binary;
            let (insertions, deletions) = stats.get(&change.path).copied().unwrap_or((0, 0));
            let (old_content, new_content, precomputed, diff_status) =
                Self::maybe_defer_diff(old_content, new_content, binary);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                old_source_path: None,
                new_source_path: None,
                status: change.status,
                insertions,
                deletions,
                binary,
            });

            old_contents.push(Arc::from(old_content));
            new_contents.push(Arc::from(new_content));
            precomputed_diffs.push(precomputed);
            diff_statuses.push(diff_status);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();
        let navigator_is_placeholder = vec![false; files.len()];

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            navigator_is_placeholder,
            repo_root: Some(repo_root),
            git_mode: Some(GitDiffMode::Staged),
            source_roots: None,
            old_contents,
            new_contents,
            precomputed_diffs,
            diff_statuses,
            pending_content: Vec::new(),
            stats_authoritative: true,
            blame_sources_override: None,
        })
    }

    /// Create from a git range where one side is the staged index
    pub fn from_git_index_range(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
        from: String,
        to_index: bool,
    ) -> Result<Self, MultiDiffError> {
        let mut files = Vec::new();
        let mut old_contents = Vec::new();
        let mut new_contents = Vec::new();
        let mut precomputed_diffs = Vec::new();
        let mut diff_statuses = Vec::new();
        let mut stat_args = vec!["--cached".to_string()];
        if !to_index {
            stat_args.push("-R".to_string());
        }
        stat_args.push(from.clone());
        let stats = crate::git::get_diff_numstat(&repo_root, &stat_args)?;
        let old_sides = Self::read_git_batch_or_binary(
            &repo_root,
            changes
                .iter()
                .map(|change| match change.status {
                    FileStatus::Added | FileStatus::Untracked => None,
                    _ if to_index => Some(format!(
                        "{from}:{}",
                        change.old_path.as_deref().unwrap_or(&change.path).display()
                    )),
                    _ => Some(format!(
                        ":{}",
                        change.old_path.as_deref().unwrap_or(&change.path).display()
                    )),
                })
                .collect(),
        )?;
        let new_sides = Self::read_git_batch_or_binary(
            &repo_root,
            changes
                .iter()
                .map(|change| match change.status {
                    FileStatus::Deleted => None,
                    _ if to_index => Some(format!(":{}", change.path.display())),
                    _ => Some(format!("{from}:{}", change.path.display())),
                })
                .collect(),
        )?;
        for ((change, (old_content, old_binary)), (new_content, new_binary)) in
            changes.into_iter().zip(old_sides).zip(new_sides)
        {
            let binary = old_binary || new_binary;
            let (insertions, deletions) = stats.get(&change.path).copied().unwrap_or((0, 0));
            let (old_content, new_content, precomputed, diff_status) =
                Self::maybe_defer_diff(old_content, new_content, binary);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                old_source_path: None,
                new_source_path: None,
                status: change.status,
                insertions,
                deletions,
                binary,
            });

            old_contents.push(Arc::from(old_content));
            new_contents.push(Arc::from(new_content));
            precomputed_diffs.push(precomputed);
            diff_statuses.push(diff_status);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();
        let navigator_is_placeholder = vec![false; files.len()];

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            navigator_is_placeholder,
            repo_root: Some(repo_root),
            git_mode: Some(GitDiffMode::IndexRange { from, to_index }),
            source_roots: None,
            old_contents,
            new_contents,
            precomputed_diffs,
            diff_statuses,
            pending_content: Vec::new(),
            stats_authoritative: true,
            blame_sources_override: None,
        })
    }

    /// Create from a git range (from..to)
    pub fn from_git_range(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
        from: String,
        to: String,
    ) -> Result<Self, MultiDiffError> {
        let mut files = Vec::new();
        let mut old_contents = Vec::new();
        let mut new_contents = Vec::new();
        let mut precomputed_diffs = Vec::new();
        let mut diff_statuses = Vec::new();
        let stats = crate::git::get_diff_numstat(&repo_root, &[format!("{from}..{to}")])?;
        let old_sides = Self::read_git_batch_or_binary(
            &repo_root,
            changes
                .iter()
                .map(|change| match change.status {
                    FileStatus::Added | FileStatus::Untracked => None,
                    _ => Some(format!(
                        "{from}:{}",
                        change.old_path.as_deref().unwrap_or(&change.path).display()
                    )),
                })
                .collect(),
        )?;
        let new_sides = Self::read_git_batch_or_binary(
            &repo_root,
            changes
                .iter()
                .map(|change| match change.status {
                    FileStatus::Deleted => None,
                    _ => Some(format!("{to}:{}", change.path.display())),
                })
                .collect(),
        )?;
        for ((change, (old_content, old_binary)), (new_content, new_binary)) in
            changes.into_iter().zip(old_sides).zip(new_sides)
        {
            let binary = old_binary || new_binary;
            let (insertions, deletions) = stats.get(&change.path).copied().unwrap_or((0, 0));
            let (old_content, new_content, precomputed, diff_status) =
                Self::maybe_defer_diff(old_content, new_content, binary);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                old_source_path: None,
                new_source_path: None,
                status: change.status,
                insertions,
                deletions,
                binary,
            });

            old_contents.push(Arc::from(old_content));
            new_contents.push(Arc::from(new_content));
            precomputed_diffs.push(precomputed);
            diff_statuses.push(diff_status);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();
        let navigator_is_placeholder = vec![false; files.len()];

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            navigator_is_placeholder,
            repo_root: Some(repo_root),
            git_mode: Some(GitDiffMode::Range { from, to }),
            source_roots: None,
            old_contents,
            new_contents,
            precomputed_diffs,
            diff_statuses,
            pending_content: Vec::new(),
            stats_authoritative: true,
            blame_sources_override: None,
        })
    }

    /// Create from two directories
    pub fn from_directories(old_dir: &Path, new_dir: &Path) -> Result<Self, MultiDiffError> {
        Self::from_directories_with_options(old_dir, new_dir, &DirectoryScanOptions::default())
    }

    pub fn from_directories_with_options(
        old_dir: &Path,
        new_dir: &Path,
        scan_options: &DirectoryScanOptions,
    ) -> Result<Self, MultiDiffError> {
        let mut files = Vec::new();
        let mut old_contents = Vec::new();
        let mut new_contents = Vec::new();
        let mut precomputed_diffs = Vec::new();
        let mut diff_statuses = Vec::new();
        // Collect all files from both directories
        let mut all_files = std::collections::HashSet::new();

        if old_dir.is_dir() {
            collect_files(old_dir, old_dir, &mut all_files, scan_options)?;
        }
        if new_dir.is_dir() {
            collect_files(new_dir, new_dir, &mut all_files, scan_options)?;
        }

        let mut all_files: Vec<_> = all_files.into_iter().collect();
        all_files.sort();

        for rel_path in all_files {
            let old_path = old_dir.join(&rel_path);
            let new_path = new_dir.join(&rel_path);

            let old_exists = old_path.exists();
            let new_exists = new_path.exists();

            let status = if !old_exists {
                FileStatus::Added
            } else if !new_exists {
                FileStatus::Deleted
            } else {
                FileStatus::Modified
            };

            let (old_content, old_binary, old_bytes) = if old_exists {
                if let Ok(metadata) = old_path.metadata() {
                    if Self::text_too_large(metadata.len()) {
                        (String::new(), true, Vec::new())
                    } else {
                        let bytes = std::fs::read(&old_path).unwrap_or_default();
                        let (content, binary) = Self::decode_bytes(bytes.clone());
                        (content, binary, bytes)
                    }
                } else {
                    (String::new(), false, Vec::new())
                }
            } else {
                (String::new(), false, Vec::new())
            };
            let (new_content, new_binary, new_bytes) = if new_exists {
                if let Ok(metadata) = new_path.metadata() {
                    if Self::text_too_large(metadata.len()) {
                        (String::new(), true, Vec::new())
                    } else {
                        let bytes = std::fs::read(&new_path).unwrap_or_default();
                        let (content, binary) = Self::decode_bytes(bytes.clone());
                        (content, binary, bytes)
                    }
                } else {
                    (String::new(), false, Vec::new())
                }
            } else {
                (String::new(), false, Vec::new())
            };
            let binary = old_binary || new_binary;

            // Skip if no changes
            if !binary && old_bytes == new_bytes {
                continue;
            }

            let (insertions, deletions) = Self::diff_stats(&old_content, &new_content, binary);
            let (old_content, new_content, precomputed, diff_status) =
                Self::maybe_defer_diff(old_content, new_content, binary);

            files.push(FileEntry {
                display_name: rel_path.display().to_string(),
                path: rel_path,
                old_path: None,
                old_source_path: None,
                new_source_path: None,
                status,
                insertions,
                deletions,
                binary,
            });

            old_contents.push(Arc::from(old_content));
            new_contents.push(Arc::from(new_content));
            precomputed_diffs.push(precomputed);
            diff_statuses.push(diff_status);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();
        let navigator_is_placeholder = vec![false; files.len()];

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            navigator_is_placeholder,
            repo_root: None,
            git_mode: None,
            source_roots: Some(SourceRoots {
                old: old_dir.to_path_buf(),
                new: new_dir.to_path_buf(),
            }),
            old_contents,
            new_contents,
            precomputed_diffs,
            diff_statuses,
            pending_content: Vec::new(),
            stats_authoritative: false,
            blame_sources_override: None,
        })
    }

    /// Create from a single file pair
    pub fn from_file_pair(
        _old_path: PathBuf,
        new_path: PathBuf,
        old_content: String,
        new_content: String,
    ) -> Self {
        Self::from_file_pair_with_sources(
            new_path.clone(),
            old_content.into_bytes(),
            new_content.into_bytes(),
            None,
            Some(new_path),
        )
    }

    /// Create from a single file pair (bytes, with binary detection).
    pub fn from_file_pair_bytes(new_path: PathBuf, old_bytes: Vec<u8>, new_bytes: Vec<u8>) -> Self {
        Self::from_file_pair_with_sources(new_path, old_bytes, new_bytes, None, None)
    }

    pub fn from_file_pair_with_sources(
        new_path: PathBuf,
        old_bytes: Vec<u8>,
        new_bytes: Vec<u8>,
        old_source: Option<PathBuf>,
        new_source: Option<PathBuf>,
    ) -> Self {
        let (old_content, old_binary) = Self::decode_bytes(old_bytes);
        let (new_content, new_binary) = Self::decode_bytes(new_bytes);
        let binary = old_binary || new_binary;
        let (insertions, deletions) = Self::diff_stats(&old_content, &new_content, binary);
        let (old_content, new_content, precomputed, diff_status) =
            Self::maybe_defer_diff(old_content, new_content, binary);

        let files = vec![FileEntry {
            display_name: new_path.display().to_string(),
            path: new_path,
            old_path: None,
            old_source_path: old_source,
            new_source_path: new_source,
            status: FileStatus::Modified,
            insertions,
            deletions,
            binary,
        }];

        Self {
            files,
            selected_index: 0,
            navigators: vec![None],
            navigator_is_placeholder: vec![false],
            repo_root: None,
            git_mode: None,
            source_roots: None,
            old_contents: vec![Arc::from(old_content)],
            new_contents: vec![Arc::from(new_content)],
            precomputed_diffs: vec![precomputed],
            diff_statuses: vec![diff_status],
            pending_content: Vec::new(),
            stats_authoritative: false,
            blame_sources_override: None,
        }
    }

    /// Create from multiple file pairs.
    pub fn from_file_pairs(pairs: Vec<(PathBuf, String, String)>) -> Self {
        let mut files = Vec::with_capacity(pairs.len());
        let mut old_contents = Vec::with_capacity(pairs.len());
        let mut new_contents = Vec::with_capacity(pairs.len());
        let mut precomputed_diffs = Vec::with_capacity(pairs.len());
        let mut diff_statuses = Vec::with_capacity(pairs.len());

        for (path, old_content, new_content) in pairs {
            let (old_content, old_binary) = Self::decode_bytes(old_content.into_bytes());
            let (new_content, new_binary) = Self::decode_bytes(new_content.into_bytes());
            let binary = old_binary || new_binary;
            let (insertions, deletions) = Self::diff_stats(&old_content, &new_content, binary);
            let (old_content, new_content, precomputed, diff_status) =
                Self::maybe_defer_diff(old_content, new_content, binary);
            files.push(FileEntry {
                display_name: path.display().to_string(),
                path,
                old_path: None,
                old_source_path: None,
                new_source_path: None,
                status: FileStatus::Modified,
                insertions,
                deletions,
                binary,
            });
            old_contents.push(Arc::from(old_content));
            new_contents.push(Arc::from(new_content));
            precomputed_diffs.push(precomputed);
            diff_statuses.push(diff_status);
        }

        Self {
            files,
            selected_index: 0,
            navigators: (0..old_contents.len()).map(|_| None).collect(),
            navigator_is_placeholder: vec![false; old_contents.len()],
            repo_root: None,
            git_mode: None,
            source_roots: None,
            old_contents,
            new_contents,
            precomputed_diffs,
            diff_statuses,
            pending_content: Vec::new(),
            stats_authoritative: false,
            blame_sources_override: None,
        }
    }

    /// Get the navigator for the currently selected file
    pub fn current_navigator(&mut self) -> &mut DiffNavigator {
        if self.navigators[self.selected_index].is_none() {
            let mut placeholder = false;
            let lazy_maps = self.file_is_large(self.selected_index);
            let diff = if let Some(slot) = self.precomputed_diffs.get_mut(self.selected_index) {
                match slot.take() {
                    Some(PrecomputedDiff::Placeholder(diff)) => {
                        placeholder = true;
                        diff
                    }
                    Some(PrecomputedDiff::Ready(diff)) => diff,
                    None => Self::diff_strings(
                        self.old_contents[self.selected_index].as_ref(),
                        self.new_contents[self.selected_index].as_ref(),
                    ),
                }
            } else {
                Self::diff_strings(
                    self.old_contents[self.selected_index].as_ref(),
                    self.new_contents[self.selected_index].as_ref(),
                )
            };
            if !placeholder && !self.stats_authoritative {
                if let Some(file) = self.files.get_mut(self.selected_index) {
                    file.insertions = diff.insertions;
                    file.deletions = diff.deletions;
                }
            }
            let navigator = DiffNavigator::new(
                diff,
                self.old_contents[self.selected_index].clone(),
                self.new_contents[self.selected_index].clone(),
                lazy_maps,
            );
            self.navigators[self.selected_index] = Some(navigator);
            if let Some(flag) = self.navigator_is_placeholder.get_mut(self.selected_index) {
                *flag = placeholder;
            }
        }
        self.navigators[self.selected_index].as_mut().unwrap()
    }

    /// Get the current file entry
    pub fn current_file(&self) -> Option<&FileEntry> {
        self.files.get(self.selected_index)
    }

    pub fn take_pending_content_for(&mut self, idx: usize) -> Option<PendingFileContent> {
        self.pending_content.get_mut(idx)?.take()
    }

    pub fn take_pending_content(&mut self) -> Vec<PendingFileContent> {
        self.pending_content
            .iter_mut()
            .filter_map(Option::take)
            .collect()
    }

    pub fn pending_content_count(&self) -> usize {
        self.diff_statuses
            .iter()
            .filter(|status| **status == DiffStatus::Loading)
            .count()
    }

    pub fn prepare_file_content(
        old_bytes: Option<Vec<u8>>,
        new_bytes: Option<Vec<u8>>,
    ) -> PreparedFileContent {
        let (old, old_binary) = old_bytes
            .map(Self::decode_bytes)
            .unwrap_or_else(|| (String::new(), true));
        let (new, new_binary) = new_bytes
            .map(Self::decode_bytes)
            .unwrap_or_else(|| (String::new(), true));
        let (old, new, mut precomputed, status) =
            Self::maybe_defer_diff(old, new, old_binary || new_binary);
        if status == DiffStatus::Ready {
            precomputed = Some(PrecomputedDiff::Ready(Self::diff_strings(&old, &new)));
        }
        PreparedFileContent {
            old: Arc::from(old),
            new: Arc::from(new),
            binary: old_binary || new_binary,
            precomputed,
            status,
        }
    }

    pub fn apply_prepared_content(&mut self, idx: usize, content: PreparedFileContent) -> bool {
        if self.diff_status(idx) != DiffStatus::Loading {
            return false;
        }
        self.old_contents[idx] = content.old;
        self.new_contents[idx] = content.new;
        self.precomputed_diffs[idx] = content.precomputed;
        self.diff_statuses[idx] = content.status;
        self.files[idx].binary = content.binary;
        self.navigators[idx] = None;
        self.navigator_is_placeholder[idx] = false;
        true
    }

    pub fn file_contents(&self, idx: usize) -> Option<(&str, &str)> {
        if self.diff_status(idx) == DiffStatus::Loading {
            return None;
        }
        let old = self.old_contents.get(idx)?;
        let new = self.new_contents.get(idx)?;
        Some((old.as_ref(), new.as_ref()))
    }

    pub fn file_contents_arc(&self, idx: usize) -> Option<(Arc<str>, Arc<str>)> {
        if self.diff_status(idx) == DiffStatus::Loading {
            return None;
        }
        let old = self.old_contents.get(idx)?;
        let new = self.new_contents.get(idx)?;
        Some((old.clone(), new.clone()))
    }

    pub fn set_source_roots(&mut self, old: PathBuf, new: PathBuf) {
        self.source_roots = Some(SourceRoots { old, new });
    }

    pub fn clear_source_roots(&mut self) {
        self.source_roots = None;
    }

    pub fn source_path(&self, idx: usize, side: FileSide) -> Option<PathBuf> {
        let file = self.files.get(idx)?;
        if let Some(path) = match side {
            FileSide::Old => file.old_source_path.as_ref(),
            FileSide::New => file.new_source_path.as_ref(),
        } {
            return Some(path.clone());
        }

        let rel_path = match side {
            FileSide::Old => file.old_path.as_ref().or_else(|| {
                if self.source_roots.is_some() || self.repo_root.is_some() {
                    Some(&file.path)
                } else {
                    None
                }
            })?,
            FileSide::New => &file.path,
        };

        if rel_path.is_absolute() {
            return Some(rel_path.clone());
        }

        if let Some(roots) = &self.source_roots {
            let root = match side {
                FileSide::Old => &roots.old,
                FileSide::New => &roots.new,
            };
            return Some(root.join(rel_path));
        }

        self.repo_root.as_ref().map(|root| root.join(rel_path))
    }

    pub fn existing_source_path(&self, idx: usize, side: FileSide) -> Option<PathBuf> {
        let path = self.source_path(idx, side)?;
        path.is_file().then_some(path)
    }

    /// Check if the current file is binary
    pub fn current_file_is_binary(&self) -> bool {
        self.files
            .get(self.selected_index)
            .map(|f| f.binary)
            .unwrap_or(false)
    }

    /// True when diffing is not ready for the current file (deferred/disabled)
    pub fn current_file_diff_disabled(&self) -> bool {
        matches!(
            self.diff_statuses.get(self.selected_index),
            Some(
                DiffStatus::Loading
                    | DiffStatus::Deferred
                    | DiffStatus::Computing
                    | DiffStatus::Failed
                    | DiffStatus::Disabled
            )
        )
    }

    pub fn diff_status(&self, idx: usize) -> DiffStatus {
        self.diff_statuses
            .get(idx)
            .copied()
            .unwrap_or(DiffStatus::Ready)
    }

    pub fn file_is_large(&self, idx: usize) -> bool {
        let old_len = self.old_contents.get(idx).map(|s| s.len()).unwrap_or(0);
        let new_len = self.new_contents.get(idx).map(|s| s.len()).unwrap_or(0);
        (old_len.max(new_len) as u64) > Self::diff_max_bytes()
    }

    pub fn current_file_is_large(&self) -> bool {
        self.file_is_large(self.selected_index)
    }

    pub fn current_navigator_is_placeholder(&self) -> bool {
        self.navigator_is_placeholder
            .get(self.selected_index)
            .copied()
            .unwrap_or(false)
    }

    pub fn current_file_diff_status(&self) -> DiffStatus {
        self.diff_status(self.selected_index)
    }

    pub fn mark_diff_computing(&mut self, idx: usize) {
        if let Some(status) = self.diff_statuses.get_mut(idx) {
            *status = DiffStatus::Computing;
        }
    }

    pub fn mark_diff_failed(&mut self, idx: usize) {
        if let Some(status) = self.diff_statuses.get_mut(idx) {
            *status = DiffStatus::Failed;
        }
    }

    pub fn apply_diff_result(&mut self, idx: usize, diff: DiffResult) {
        if let Some(status) = self.diff_statuses.get_mut(idx) {
            *status = DiffStatus::Ready;
        }
        let insertions = diff.insertions;
        let deletions = diff.deletions;
        if let Some(slot) = self.precomputed_diffs.get_mut(idx) {
            *slot = Some(PrecomputedDiff::Ready(diff));
        }
        if !self.stats_authoritative {
            if let Some(file) = self.files.get_mut(idx) {
                file.insertions = insertions;
                file.deletions = deletions;
            }
        }
    }

    pub fn ensure_full_navigator(&mut self, idx: usize) {
        if !matches!(self.diff_status(idx), DiffStatus::Ready) {
            return;
        }
        let needs_refresh = self
            .navigator_is_placeholder
            .get(idx)
            .copied()
            .unwrap_or(false);
        if self.navigators.get(idx).and_then(|n| n.as_ref()).is_some() && !needs_refresh {
            return;
        }
        let diff = if let Some(slot) = self.precomputed_diffs.get_mut(idx) {
            match slot.take() {
                Some(PrecomputedDiff::Ready(diff)) => diff,
                Some(PrecomputedDiff::Placeholder(diff)) => diff,
                None => Self::diff_strings(
                    self.old_contents[idx].as_ref(),
                    self.new_contents[idx].as_ref(),
                ),
            }
        } else {
            Self::diff_strings(
                self.old_contents[idx].as_ref(),
                self.new_contents[idx].as_ref(),
            )
        };
        let lazy_maps = self.file_is_large(idx);
        let navigator = DiffNavigator::new(
            diff,
            self.old_contents[idx].clone(),
            self.new_contents[idx].clone(),
            lazy_maps,
        );
        if let Some(slot) = self.navigators.get_mut(idx) {
            *slot = Some(navigator);
        }
        if let Some(flag) = self.navigator_is_placeholder.get_mut(idx) {
            *flag = false;
        }
    }

    /// Select next file
    pub fn next_file(&mut self) -> bool {
        if self.selected_index < self.files.len().saturating_sub(1) {
            self.selected_index += 1;
            true
        } else {
            false
        }
    }

    /// Select previous file
    pub fn prev_file(&mut self) -> bool {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            true
        } else {
            false
        }
    }

    /// Select file by index
    pub fn select_file(&mut self, index: usize) {
        if index < self.files.len() {
            self.selected_index = index;
        }
    }

    /// Total number of files
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Repository root path (git mode only)
    pub fn repo_root(&self) -> Option<&Path> {
        self.repo_root.as_deref()
    }

    /// True if this diff was created from git changes
    pub fn is_git_mode(&self) -> bool {
        self.repo_root.is_some()
    }

    pub fn uses_git_index(&self) -> bool {
        matches!(
            self.git_mode,
            Some(GitDiffMode::Staged | GitDiffMode::IndexRange { .. })
        )
    }

    fn change_key(path: &Path, old_path: Option<&Path>, status: FileStatus) -> String {
        format!(
            "{}\0{}\0{:?}",
            path.display(),
            old_path
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            status
        )
    }

    fn change_signature_for_files(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .files
            .iter()
            .map(|file| Self::change_key(&file.path, file.old_path.as_deref(), file.status))
            .collect();
        keys.sort();
        keys
    }

    /// True if the git path/status list changed since this diff was built.
    pub fn git_change_list_changed(&self) -> bool {
        let Some(repo_root) = self.repo_root.as_ref() else {
            return false;
        };
        let Some(mode) = self.git_mode.as_ref() else {
            return false;
        };

        let changes = match mode {
            GitDiffMode::Uncommitted => crate::git::get_uncommitted_changes(repo_root),
            GitDiffMode::Staged => crate::git::get_staged_changes(repo_root),
            GitDiffMode::Range { from, to } => crate::git::get_changes_between(repo_root, from, to),
            GitDiffMode::IndexRange { from, to_index } => {
                crate::git::get_changes_between_index(repo_root, from, !to_index)
            }
        };
        let Ok(changes) = changes else {
            return false;
        };

        let mut fresh: Vec<String> = changes
            .iter()
            .map(|change| Self::change_key(&change.path, change.old_path.as_deref(), change.status))
            .collect();
        fresh.sort();
        fresh != self.change_signature_for_files()
    }

    /// Return a display-friendly git range for header usage (if applicable).
    pub fn git_range_display(&self) -> Option<(String, String)> {
        let mode = self.git_mode.as_ref()?;
        match mode {
            GitDiffMode::Range { from, to } => Some((format_ref(from), format_ref(to))),
            GitDiffMode::IndexRange { from, to_index } => {
                let staged = "STAGED".to_string();
                if *to_index {
                    Some((format_ref(from), staged))
                } else {
                    Some((staged, format_ref(from)))
                }
            }
            _ => None,
        }
    }

    pub fn set_blame_sources(&mut self, old: BlameSource, new: BlameSource) {
        self.blame_sources_override = Some((old, new));
    }

    /// Blame sources for old/new content.
    pub fn blame_sources(&self) -> Option<(BlameSource, BlameSource)> {
        if let Some(sources) = &self.blame_sources_override {
            return Some(sources.clone());
        }
        let mode = self.git_mode.as_ref()?;
        let sources = match mode {
            GitDiffMode::Uncommitted => (
                BlameSource::Commit("HEAD".to_string()),
                BlameSource::Worktree,
            ),
            GitDiffMode::Staged => (BlameSource::Commit("HEAD".to_string()), BlameSource::Index),
            GitDiffMode::Range { from, to } => (
                BlameSource::Commit(from.clone()),
                BlameSource::Commit(to.clone()),
            ),
            GitDiffMode::IndexRange { from, to_index } => {
                if *to_index {
                    (BlameSource::Commit(from.clone()), BlameSource::Index)
                } else {
                    (BlameSource::Index, BlameSource::Commit(from.clone()))
                }
            }
        };
        Some(sources)
    }

    /// Get the step direction of current navigator (if loaded)
    pub fn current_step_direction(&self) -> StepDirection {
        if let Some(Some(nav)) = self.navigators.get(self.selected_index) {
            nav.state().step_direction
        } else {
            StepDirection::None
        }
    }

    /// Check if we have multiple files
    pub fn is_multi_file(&self) -> bool {
        self.files.len() > 1
    }

    /// Get total stats across all files
    pub fn total_stats(&self) -> (usize, usize) {
        self.files.iter().fold((0, 0), |(ins, del), f| {
            (ins + f.insertions, del + f.deletions)
        })
    }

    /// Check if current file's old content is empty
    pub fn current_old_is_empty(&self) -> bool {
        self.old_contents
            .get(self.selected_index)
            .map(|s| s.is_empty())
            .unwrap_or(true)
    }

    /// Check if current file's new content is empty
    pub fn current_new_is_empty(&self) -> bool {
        self.new_contents
            .get(self.selected_index)
            .map(|s| s.is_empty())
            .unwrap_or(true)
    }

    /// Refresh all files from git (re-scan for uncommitted changes)
    /// Returns true if successful, false if not in git mode
    pub fn refresh_all_from_git(&mut self) -> bool {
        let repo_root = match &self.repo_root {
            Some(root) => root.clone(),
            None => return false,
        };
        let mode = match &self.git_mode {
            Some(mode) => mode.clone(),
            None => return false,
        };

        // Get fresh list of changes
        let changes = match mode {
            GitDiffMode::Uncommitted => crate::git::get_uncommitted_changes(&repo_root),
            GitDiffMode::Staged => crate::git::get_staged_changes(&repo_root),
            GitDiffMode::Range { ref from, ref to } => {
                crate::git::get_changes_between(&repo_root, from, to)
            }
            GitDiffMode::IndexRange { ref from, to_index } => {
                crate::git::get_changes_between_index(&repo_root, from, !to_index)
            }
        };
        let changes = match changes {
            Ok(c) => c,
            Err(_) => return false,
        };

        // Rebuild the entire diff state
        let mut files = Vec::new();
        let mut old_contents = Vec::new();
        let mut new_contents = Vec::new();
        let mut precomputed_diffs = Vec::new();
        let mut diff_statuses = Vec::new();
        for change in changes {
            let old_path = change
                .old_path
                .clone()
                .unwrap_or_else(|| change.path.clone());
            let (old_content, old_binary, new_content, new_binary) = match mode {
                GitDiffMode::Uncommitted => {
                    let (old_content, old_binary) = match change.status {
                        FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                        _ => Self::read_git_commit_or_binary(&repo_root, "HEAD", &old_path),
                    };
                    let (new_content, new_binary) = match change.status {
                        FileStatus::Deleted => (String::new(), false),
                        _ => {
                            let full_path = repo_root.join(&change.path);
                            Self::read_text_or_binary(&full_path)
                        }
                    };
                    (old_content, old_binary, new_content, new_binary)
                }
                GitDiffMode::Staged => {
                    let (old_content, old_binary) = match change.status {
                        FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                        _ => Self::read_git_commit_or_binary(&repo_root, "HEAD", &old_path),
                    };
                    let (new_content, new_binary) = match change.status {
                        FileStatus::Deleted => (String::new(), false),
                        _ => Self::read_git_index_or_binary(&repo_root, &change.path),
                    };
                    (old_content, old_binary, new_content, new_binary)
                }
                GitDiffMode::Range { ref from, ref to } => {
                    let (old_content, old_binary) = match change.status {
                        FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                        _ => Self::read_git_commit_or_binary(&repo_root, from, &old_path),
                    };
                    let (new_content, new_binary) = match change.status {
                        FileStatus::Deleted => (String::new(), false),
                        _ => Self::read_git_commit_or_binary(&repo_root, to, &change.path),
                    };
                    (old_content, old_binary, new_content, new_binary)
                }
                GitDiffMode::IndexRange { ref from, to_index } => {
                    if to_index {
                        let (old_content, old_binary) = match change.status {
                            FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                            _ => Self::read_git_commit_or_binary(&repo_root, from, &old_path),
                        };
                        let (new_content, new_binary) = match change.status {
                            FileStatus::Deleted => (String::new(), false),
                            _ => Self::read_git_index_or_binary(&repo_root, &change.path),
                        };
                        (old_content, old_binary, new_content, new_binary)
                    } else {
                        let (old_content, old_binary) = match change.status {
                            FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                            _ => Self::read_git_index_or_binary(&repo_root, &old_path),
                        };
                        let (new_content, new_binary) = match change.status {
                            FileStatus::Deleted => (String::new(), false),
                            _ => Self::read_git_commit_or_binary(&repo_root, from, &change.path),
                        };
                        (old_content, old_binary, new_content, new_binary)
                    }
                }
            };

            let binary = old_binary || new_binary;
            let (insertions, deletions) = Self::diff_stats(&old_content, &new_content, binary);
            let (old_content, new_content, precomputed, diff_status) =
                Self::maybe_defer_diff(old_content, new_content, binary);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                old_source_path: None,
                new_source_path: None,
                status: change.status,
                insertions,
                deletions,
                binary,
            });

            old_contents.push(Arc::from(old_content));
            new_contents.push(Arc::from(new_content));
            precomputed_diffs.push(precomputed);
            diff_statuses.push(diff_status);
        }

        // Update state
        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();
        let navigator_is_placeholder = vec![false; files.len()];
        self.files = files;
        self.old_contents = old_contents;
        self.new_contents = new_contents;
        self.precomputed_diffs = precomputed_diffs;
        self.diff_statuses = diff_statuses;
        self.navigators = navigators;
        self.navigator_is_placeholder = navigator_is_placeholder;

        // Clamp selected index to valid range
        if self.selected_index >= self.files.len() {
            self.selected_index = self.files.len().saturating_sub(1);
        }

        true
    }

    /// Refresh the current file from disk (re-read and re-diff)
    pub fn refresh_current_file(&mut self) {
        self.refresh_file(self.selected_index);
    }

    /// Refresh a file from disk (re-read and re-diff)
    pub fn refresh_file(&mut self, idx: usize) {
        if idx >= self.files.len() || self.diff_status(idx) == DiffStatus::Loading {
            return;
        }
        let file = &self.files[idx];
        let old_path = file.old_path.clone().unwrap_or_else(|| file.path.clone());

        // Get fresh content based on mode
        let (old_content, old_binary, new_content, new_binary) =
            match (&self.repo_root, &self.git_mode) {
                (Some(repo_root), Some(GitDiffMode::Uncommitted)) => {
                    let (old_content, old_binary) = match file.status {
                        FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                        _ => Self::read_git_commit_or_binary(repo_root, "HEAD", &old_path),
                    };
                    let (new_content, new_binary) = match file.status {
                        FileStatus::Deleted => (String::new(), false),
                        _ => {
                            let full_path = repo_root.join(&file.path);
                            Self::read_text_or_binary(&full_path)
                        }
                    };
                    (old_content, old_binary, new_content, new_binary)
                }
                (Some(repo_root), Some(GitDiffMode::Staged)) => {
                    let (old_content, old_binary) = match file.status {
                        FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                        _ => Self::read_git_commit_or_binary(repo_root, "HEAD", &old_path),
                    };
                    let (new_content, new_binary) = match file.status {
                        FileStatus::Deleted => (String::new(), false),
                        _ => Self::read_git_index_or_binary(repo_root, &file.path),
                    };
                    (old_content, old_binary, new_content, new_binary)
                }
                (Some(repo_root), Some(GitDiffMode::Range { from, to })) => {
                    let (old_content, old_binary) = match file.status {
                        FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                        _ => Self::read_git_commit_or_binary(repo_root, from, &old_path),
                    };
                    let (new_content, new_binary) = match file.status {
                        FileStatus::Deleted => (String::new(), false),
                        _ => Self::read_git_commit_or_binary(repo_root, to, &file.path),
                    };
                    (old_content, old_binary, new_content, new_binary)
                }
                (Some(repo_root), Some(GitDiffMode::IndexRange { from, to_index })) => {
                    if *to_index {
                        let (old_content, old_binary) = match file.status {
                            FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                            _ => Self::read_git_commit_or_binary(repo_root, from, &old_path),
                        };
                        let (new_content, new_binary) = match file.status {
                            FileStatus::Deleted => (String::new(), false),
                            _ => Self::read_git_index_or_binary(repo_root, &file.path),
                        };
                        (old_content, old_binary, new_content, new_binary)
                    } else {
                        let (old_content, old_binary) = match file.status {
                            FileStatus::Added | FileStatus::Untracked => (String::new(), false),
                            _ => Self::read_git_index_or_binary(repo_root, &old_path),
                        };
                        let (new_content, new_binary) = match file.status {
                            FileStatus::Deleted => (String::new(), false),
                            _ => Self::read_git_commit_or_binary(repo_root, from, &file.path),
                        };
                        (old_content, old_binary, new_content, new_binary)
                    }
                }
                _ => {
                    let old_content = self.old_contents[idx].as_ref().to_string();
                    let old_source_path = file.old_source_path.clone().or_else(|| {
                        self.source_roots.as_ref().map(|roots| {
                            roots.old.join(file.old_path.as_ref().unwrap_or(&file.path))
                        })
                    });
                    let (old_content, old_binary) = old_source_path
                        .filter(|path| path.is_file())
                        .map(|path| Self::read_text_or_binary(&path))
                        .unwrap_or((old_content, false));
                    let new_path = self
                        .source_path(idx, FileSide::New)
                        .unwrap_or_else(|| file.path.clone());
                    let (new_content, new_binary) = Self::read_text_or_binary(&new_path);
                    (old_content, old_binary, new_content, new_binary)
                }
            };

        let binary = old_binary || new_binary;
        let (insertions, deletions) = Self::diff_stats(&old_content, &new_content, binary);
        let (old_content, new_content, precomputed, diff_status) =
            Self::maybe_defer_diff(old_content, new_content, binary);

        self.old_contents[idx] = Arc::from(old_content);
        self.new_contents[idx] = Arc::from(new_content);
        self.files[idx].binary = binary;
        self.files[idx].insertions = insertions;
        self.files[idx].deletions = deletions;
        if let Some(slot) = self.precomputed_diffs.get_mut(idx) {
            *slot = precomputed;
        }
        if let Some(status) = self.diff_statuses.get_mut(idx) {
            *status = diff_status;
        }

        // Clear the navigator so it gets rebuilt on next access
        self.navigators[idx] = None;
        if let Some(flag) = self.navigator_is_placeholder.get_mut(idx) {
            *flag = false;
        }
    }
}

fn collect_files(
    dir: &Path,
    base: &Path,
    files: &mut std::collections::HashSet<PathBuf>,
    scan_options: &DirectoryScanOptions,
) -> Result<(), std::io::Error> {
    let mut builder = WalkBuilder::new(dir);
    builder
        .standard_filters(false)
        .hidden(false)
        .parents(scan_options.git_ignore)
        .ignore(false)
        .git_ignore(scan_options.git_ignore)
        .git_global(scan_options.git_ignore)
        .git_exclude(scan_options.git_ignore)
        .require_git(false);

    if !scan_options.ignore_globs.is_empty() {
        let mut overrides = OverrideBuilder::new(base);
        for pattern in &scan_options.ignore_globs {
            if let Some(dir_pattern) = pattern.strip_suffix("/**") {
                if !dir_pattern.is_empty() {
                    overrides
                        .add(&format!("!{dir_pattern}"))
                        .map_err(ignore_error_to_io)?;
                }
            }
            overrides
                .add(&format!("!{pattern}"))
                .map_err(ignore_error_to_io)?;
        }
        builder.overrides(overrides.build().map_err(ignore_error_to_io)?);
    }

    for entry in builder.build() {
        let entry = entry.map_err(ignore_error_to_io)?;
        let path = entry.path();
        if path == dir {
            continue;
        }
        if path.is_file() {
            if let Ok(rel) = path.strip_prefix(base) {
                files.insert(rel.to_path_buf());
            }
        }
    }
    Ok(())
}

fn ignore_error_to_io(error: ignore::Error) -> std::io::Error {
    std::io::Error::other(error)
}

fn format_ref(reference: &str) -> String {
    match reference {
        "HEAD" => "HEAD".to_string(),
        "INDEX" => "STAGED".to_string(),
        _ => shorten_hash(reference),
    }
}

fn shorten_hash(hash: &str) -> String {
    hash.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static DIFF_SETTINGS_LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("oyo-core-{name}-{}-{nanos}", std::process::id()))
    }

    fn run_git(root: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn display_names(diff: &MultiFileDiff) -> Vec<String> {
        diff.files
            .iter()
            .map(|file| file.display_name.clone())
            .collect()
    }

    #[test]
    fn lazy_git_diff_builds_metadata_before_loading_content() {
        let root = temp_dir("lazy-git-content");
        std::fs::create_dir_all(&root).unwrap();
        assert!(std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(&root)
            .status()
            .unwrap()
            .success());
        run_git(&root, &["config", "user.email", "test@example.com"]);
        run_git(&root, &["config", "user.name", "Test"]);
        for idx in 0..5 {
            write_file(&root.join(format!("file-{idx}.txt")), "old\n");
        }
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-qm", "base"]);
        for idx in 0..5 {
            write_file(&root.join(format!("file-{idx}.txt")), "new\n");
        }

        let changes = crate::git::get_uncommitted_changes(&root).unwrap();
        let mut diff = MultiFileDiff::from_git_changes_lazy(root.clone(), changes).unwrap();
        assert_eq!(diff.file_count(), 5);
        assert!(diff
            .diff_statuses
            .iter()
            .all(|status| *status == DiffStatus::Loading));
        assert_eq!((diff.files[0].insertions, diff.files[0].deletions), (1, 1));
        assert!(diff.file_contents(0).is_none());
        diff.refresh_file(1);
        assert_eq!(diff.diff_status(1), DiffStatus::Loading);
        assert!(diff.file_contents(1).is_none());

        let pending = diff.take_pending_content();
        assert_eq!(pending.len(), 5);
        let content =
            MultiFileDiff::prepare_file_content(Some(b"old\n".to_vec()), Some(b"new\n".to_vec()));
        assert!(diff.apply_prepared_content(0, content));
        assert_eq!(diff.diff_status(0), DiffStatus::Ready);
        assert_eq!(diff.file_contents(0), Some(("old\n", "new\n")));
        diff.current_navigator();
        assert_eq!((diff.files[0].insertions, diff.files[0].deletions), (1, 1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn uncommitted_diff_supports_staged_files_before_the_first_commit() {
        let root = temp_dir("unborn-head");
        std::fs::create_dir_all(&root).unwrap();
        assert!(std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(&root)
            .status()
            .unwrap()
            .success());
        write_file(&root.join("new.txt"), "one\ntwo\n");
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["add", "new.txt"])
            .status()
            .unwrap()
            .success());

        let changes = crate::git::get_uncommitted_changes(&root).unwrap();
        let diff = MultiFileDiff::from_git_changes(root.clone(), changes).unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].insertions, 2);
        assert_eq!(diff.files[0].deletions, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn raw_file_refresh_keeps_immutable_old_content() {
        let root = temp_dir("raw-refresh");
        let path = root.join("README.md");
        write_file(&path, "base\nbefore\nduring\n");
        let mut diff = MultiFileDiff::from_raw_files(
            Some(root.clone()),
            vec![RawFileDiff {
                path: PathBuf::from("README.md"),
                old_path: None,
                old_source_path: None,
                new_source_path: Some(path),
                status: FileStatus::Modified,
                old_content: "base\n".to_string(),
                new_content: "base\nbefore\n".to_string(),
                binary: false,
            }],
        );

        diff.refresh_file(0);

        assert_eq!(
            diff.file_contents(0),
            Some(("base\n", "base\nbefore\nduring\n"))
        );
        assert_eq!(diff.files[0].insertions, 2);
        assert_eq!(diff.files[0].deletions, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn directory_scan_includes_dotfiles() {
        let root = temp_dir("dotfiles");
        let old_dir = root.join("old");
        let new_dir = root.join("new");
        std::fs::create_dir_all(&old_dir).unwrap();
        write_file(
            &new_dir.join(".github/actions/foo/action.yml"),
            "name: test\n",
        );
        write_file(&new_dir.join(".env.example"), "KEY=value\n");

        let diff = MultiFileDiff::from_directories(&old_dir, &new_dir).unwrap();
        let names = display_names(&diff);
        assert!(names.contains(&".github/actions/foo/action.yml".to_string()));
        assert!(names.contains(&".env.example".to_string()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn directory_scan_respects_gitignore_when_enabled() {
        let root = temp_dir("gitignore");
        let old_dir = root.join("old");
        let new_dir = root.join("new");
        write_file(&old_dir.join(".gitignore"), "ignored.txt\n");
        write_file(&new_dir.join(".gitignore"), "ignored.txt\n");
        write_file(&old_dir.join("ignored.txt"), "old\n");
        write_file(&new_dir.join("ignored.txt"), "new\n");

        let ignored = MultiFileDiff::from_directories_with_options(
            &old_dir,
            &new_dir,
            &DirectoryScanOptions {
                git_ignore: true,
                ignore_globs: Vec::new(),
            },
        )
        .unwrap();
        assert!(!display_names(&ignored).contains(&"ignored.txt".to_string()));

        let included = MultiFileDiff::from_directories_with_options(
            &old_dir,
            &new_dir,
            &DirectoryScanOptions {
                git_ignore: false,
                ignore_globs: Vec::new(),
            },
        )
        .unwrap();
        assert!(display_names(&included).contains(&"ignored.txt".to_string()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn directory_scan_skips_vcs_metadata_by_default() {
        let root = temp_dir("vcs-metadata");
        let old_dir = root.join("old");
        let new_dir = root.join("new");
        write_file(&old_dir.join(".git/config"), "old\n");
        write_file(&new_dir.join(".git/config"), "new\n");

        let diff = MultiFileDiff::from_directories(&old_dir, &new_dir).unwrap();
        assert!(!display_names(&diff).contains(&".git/config".to_string()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn directory_diff_exposes_source_paths() {
        let root = temp_dir("source-paths");
        let old_dir = root.join("old");
        let new_dir = root.join("new");
        write_file(&old_dir.join("file.txt"), "old\n");
        write_file(&new_dir.join("file.txt"), "new\n");

        let diff = MultiFileDiff::from_directories(&old_dir, &new_dir).unwrap();
        assert_eq!(
            diff.existing_source_path(0, FileSide::Old),
            Some(old_dir.join("file.txt"))
        );
        assert_eq!(
            diff.existing_source_path(0, FileSide::New),
            Some(new_dir.join("file.txt"))
        );

        write_file(&old_dir.join("file.txt"), "older\n");
        let mut diff = diff;
        diff.refresh_current_file();
        assert_eq!(diff.file_contents(0).map(|(old, _)| old), Some("older\n"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_pair_exposes_explicit_source_path() {
        let root = temp_dir("file-pair-source");
        let old_path = root.join("old.txt");
        let new_path = root.join("new.txt");
        write_file(&old_path, "old\n");
        write_file(&new_path, "new\n");

        let diff = MultiFileDiff::from_file_pair_with_sources(
            PathBuf::from("display.txt"),
            b"old\n".to_vec(),
            b"new\n".to_vec(),
            Some(old_path.clone()),
            Some(new_path.clone()),
        );
        assert_eq!(diff.existing_source_path(0, FileSide::Old), Some(old_path));
        assert_eq!(diff.existing_source_path(0, FileSide::New), Some(new_path));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn deferred_diff_upgrades_to_ready() {
        let _guard = DIFF_SETTINGS_LOCK.lock().unwrap();
        MultiFileDiff::set_diff_max_bytes(32);
        MultiFileDiff::set_diff_defer(true);

        let content = "a".repeat(128);
        let mut diff = MultiFileDiff::from_file_pair_bytes(
            PathBuf::from("file.txt"),
            content.clone().into_bytes(),
            content.into_bytes(),
        );

        assert_eq!(diff.diff_status(0), DiffStatus::Deferred);

        let computed = MultiFileDiff::compute_diff(
            diff.old_contents[0].as_ref(),
            diff.new_contents[0].as_ref(),
        );
        diff.apply_diff_result(0, computed);
        assert_eq!(diff.diff_status(0), DiffStatus::Ready);

        MultiFileDiff::set_diff_max_bytes(DEFAULT_DIFF_MAX_BYTES);
        MultiFileDiff::set_diff_defer(true);
    }
}
