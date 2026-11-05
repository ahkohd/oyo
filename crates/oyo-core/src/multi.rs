//! Multi-file diff support

use crate::change::{Change, ChangeSpan};
use crate::diff::{DiffEngine, DiffResult};
use crate::git::{ChangedFile, FileStatus};
use crate::patch::BlobId;
use crate::step::{DiffNavigator, StepDirection};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Read};
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
    RemoteBlob {
        provider: String,
        host: String,
        repo: String,
        revision: String,
        path: String,
    },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObservedContent {
    pub old: BlobId,
    pub new: BlobId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedFileContent {
    pub request: PendingFileContent,
    pub old: Option<BlobId>,
    pub new: Option<BlobId>,
}

impl ExpectedFileContent {
    fn accepts(&self, request: &PendingFileContent, observed: Option<ObservedContent>) -> bool {
        if request != &self.request {
            return false;
        }
        let Some(observed) = observed else {
            return false;
        };
        for (source, expected, actual) in [
            (&request.old, self.old, observed.old),
            (&request.new, self.new, observed.new),
        ] {
            match (source, expected) {
                (ContentSource::Empty, None) if actual.verify(&[]).is_ok() => {}
                (ContentSource::Empty, _) | (_, None) => return false,
                (_, Some(expected)) if expected == actual => {}
                _ => return false,
            }
        }
        true
    }
}

#[derive(Clone)]
enum RemoteMaterial {
    Patch {
        patch: Arc<crate::patch::PatchRecord>,
        request: Box<PendingFileContent>,
        hydration: HydrationState,
    },
    Full {
        original_patch: Arc<crate::patch::PatchRecord>,
        request: Box<PendingFileContent>,
        observed: Option<ObservedContent>,
        display_patch: Option<Arc<crate::patch::PatchRecord>>,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub enum HydrationState {
    Idle,
    Loading,
    Failed(String),
}

pub struct PreparedFileContent {
    observed: Option<ObservedContent>,
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
    pub remote_snapshot: Option<serde_json::Value>,
    remote_material: std::collections::HashMap<usize, RemoteMaterial>,
    remote_content_proof: Option<Arc<[ExpectedFileContent]>>,
    remote_prefetch: RemotePrefetch,
}

const REMOTE_CLASS_BYTES: usize = 64 * 1024 * 1024;
const REMOTE_NEARBY_FILES: usize = 8;
const REMOTE_TAIL_FILES: usize = 1024;
static CONTENT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);

struct RemotePrefetch {
    snapshot: u64,
    revisions: Vec<u64>,
    swept: Vec<bool>,
    nearby_attempted: std::collections::HashSet<usize>,
    selection: Option<usize>,
    cursor: usize,
    resident: std::collections::VecDeque<usize>,
    presented: bool,
    suspended: bool,
}

impl Default for RemotePrefetch {
    fn default() -> Self {
        Self {
            snapshot: CONTENT_SNAPSHOT.fetch_add(1, Ordering::Relaxed),
            revisions: Vec::new(),
            swept: Vec::new(),
            nearby_attempted: std::collections::HashSet::new(),
            selection: None,
            cursor: 0,
            resident: std::collections::VecDeque::new(),
            presented: false,
            suspended: false,
        }
    }
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
    Buffered,
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

    fn read_link_bytes(path: &Path) -> Option<Vec<u8>> {
        let target = std::fs::read_link(path).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Some(target.as_os_str().as_bytes().to_vec())
        }
        #[cfg(not(unix))]
        {
            Some(target.to_string_lossy().as_bytes().to_vec())
        }
    }

    fn read_git_worktree_or_binary(path: &Path) -> (String, bool) {
        if path
            .symlink_metadata()
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Self::read_link_bytes(path)
                .map(Self::decode_bytes)
                .unwrap_or_default();
        }
        Self::read_text_or_binary(path)
    }

    fn files_equal(left_path: &Path, right_path: &Path) -> bool {
        let (Ok(left_metadata), Ok(right_metadata)) = (left_path.metadata(), right_path.metadata())
        else {
            return false;
        };
        if left_metadata.len() != right_metadata.len() {
            return false;
        }

        let (Ok(left_file), Ok(right_file)) = (
            std::fs::File::open(left_path),
            std::fs::File::open(right_path),
        ) else {
            return false;
        };
        let mut left = BufReader::new(left_file);
        let mut right = BufReader::new(right_file);
        let mut left_buffer = [0; 8192];
        let mut right_buffer = [0; 8192];
        let mut remaining = left_metadata.len();
        while remaining > 0 {
            let chunk_len = remaining.min(left_buffer.len() as u64) as usize;
            if left.read_exact(&mut left_buffer[..chunk_len]).is_err()
                || right.read_exact(&mut right_buffer[..chunk_len]).is_err()
            {
                return false;
            }
            if left_buffer[..chunk_len] != right_buffer[..chunk_len] {
                return false;
            }
            remaining -= chunk_len as u64;
        }
        true
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

    fn git_numstat_for_mode(
        repo_root: &Path,
        mode: &GitDiffMode,
    ) -> Option<std::collections::HashMap<PathBuf, (usize, usize)>> {
        Self::git_numstat_for_mode_with_paths(repo_root, mode, None)
    }

    fn git_numstat_for_file(
        repo_root: &Path,
        mode: &GitDiffMode,
        paths: &[PathBuf],
    ) -> Option<std::collections::HashMap<PathBuf, (usize, usize)>> {
        Self::git_numstat_for_mode_with_paths(repo_root, mode, Some(paths))
    }

    fn git_numstat_for_mode_with_paths(
        repo_root: &Path,
        mode: &GitDiffMode,
        paths: Option<&[PathBuf]>,
    ) -> Option<std::collections::HashMap<PathBuf, (usize, usize)>> {
        let get_stats = |args: &[String]| match paths {
            Some(paths) => crate::git::get_diff_numstat_for_paths(repo_root, args, paths),
            None => crate::git::get_diff_numstat(repo_root, args),
        };
        let args = match mode {
            GitDiffMode::Uncommitted => vec!["HEAD".to_string()],
            GitDiffMode::Staged => vec!["--cached".to_string()],
            GitDiffMode::Range { from, to } => vec![format!("{from}..{to}")],
            GitDiffMode::IndexRange { from, to_index } => {
                let mut args = vec!["--cached".to_string()];
                if !to_index {
                    args.push("-R".to_string());
                }
                args.push(from.clone());
                args
            }
        };
        match get_stats(&args) {
            Ok(stats) => Some(stats),
            Err(_) if matches!(mode, GitDiffMode::Uncommitted) => {
                get_stats(&["4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string()]).ok()
            }
            Err(_) => None,
        }
    }

    fn normalize_text(text: String) -> String {
        if !text
            .lines()
            .any(|line| line.chars().count() > Self::MAX_LINE_CHARS)
        {
            return text;
        }
        let mut out = String::new();
        for chunk in text.split_inclusive('\n') {
            let (line, has_newline) = if let Some(line) = chunk.strip_suffix('\n') {
                (line, true)
            } else {
                (chunk, false)
            };
            if line.chars().count() > Self::MAX_LINE_CHARS {
                let cutoff = line
                    .char_indices()
                    .nth(Self::MAX_LINE_CHARS)
                    .map(|(idx, _)| idx)
                    .unwrap_or_else(|| line.len());
                let mut hasher = DefaultHasher::new();
                line.hash(&mut hasher);
                out.push_str(&line[..cutoff]);
                out.push('…');
                // Preserve tail differences without feeding huge lines to the diff view.
                out.push_str(&format!("{:016x}", hasher.finish()));
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
        }
    }

    fn load_uncommitted_symlink_contents(&mut self) {
        let Some(repo_root) = self.repo_root.clone() else {
            return;
        };
        for idx in 0..self.files.len() {
            let file = self.files[idx].clone();
            let full_path = repo_root.join(&file.path);
            if !full_path
                .symlink_metadata()
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                continue;
            }

            let new = Self::read_link_bytes(&full_path);
            let old_path = file.old_path.as_deref().unwrap_or(&file.path);
            let old = match file.status {
                FileStatus::Added | FileStatus::Untracked => Some(Vec::new()),
                _ => crate::git::get_file_at_commit_bytes(&repo_root, "HEAD", old_path).ok(),
            };
            self.pending_content[idx] = None;
            let content = Self::prepare_file_content(old, new);
            self.apply_prepared_content(idx, content);
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
        let mut diff = Self::from_git_pending(
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
        );
        diff.load_uncommitted_symlink_contents();
        Ok(diff)
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
                    Self::read_git_worktree_or_binary(&full_path)
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
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

            let (old_content, old_binary, old_bytes, old_fully_read) = if old_exists {
                if let Ok(metadata) = old_path.metadata() {
                    if Self::text_too_large(metadata.len()) {
                        (String::new(), true, Vec::new(), false)
                    } else {
                        match std::fs::read(&old_path) {
                            Ok(bytes) => {
                                let (content, binary) = Self::decode_bytes(bytes.clone());
                                (content, binary, bytes, true)
                            }
                            Err(_) => (String::new(), false, Vec::new(), false),
                        }
                    }
                } else {
                    (String::new(), false, Vec::new(), false)
                }
            } else {
                (String::new(), false, Vec::new(), false)
            };
            let (new_content, new_binary, new_bytes, new_fully_read) = if new_exists {
                if let Ok(metadata) = new_path.metadata() {
                    if Self::text_too_large(metadata.len()) {
                        (String::new(), true, Vec::new(), false)
                    } else {
                        match std::fs::read(&new_path) {
                            Ok(bytes) => {
                                let (content, binary) = Self::decode_bytes(bytes.clone());
                                (content, binary, bytes, true)
                            }
                            Err(_) => (String::new(), false, Vec::new(), false),
                        }
                    }
                } else {
                    (String::new(), false, Vec::new(), false)
                }
            } else {
                (String::new(), false, Vec::new(), false)
            };
            let binary = old_binary || new_binary;

            // Skip if no changes
            if old_exists
                && new_exists
                && ((old_fully_read && new_fully_read && old_bytes == new_bytes)
                    || (!(old_fully_read && new_fully_read)
                        && Self::files_equal(&old_path, &new_path)))
            {
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
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

    pub fn from_remote_file_pair(
        path: PathBuf,
        old_content: String,
        new_content: String,
        snapshot: serde_json::Value,
    ) -> Self {
        let file = FileEntry {
            display_name: path.display().to_string(),
            path,
            old_path: None,
            old_source_path: None,
            new_source_path: None,
            status: FileStatus::Modified,
            insertions: 0,
            deletions: 0,
            binary: false,
        };
        let mut diff = Self::from_pending_files(
            None,
            vec![(file, ContentSource::Empty, ContentSource::Empty)],
            false,
        );
        diff.take_pending_content_for(0);
        diff.apply_prepared_content(
            0,
            Self::prepare_remote_file_content(old_content.into_bytes(), new_content.into_bytes()),
        );
        diff.remote_snapshot = Some(snapshot);
        diff
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
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
            remote_snapshot: None,
            remote_material: std::collections::HashMap::new(),
            remote_content_proof: None,
            remote_prefetch: RemotePrefetch::default(),
        }
    }

    /// Get the navigator for the currently selected file
    pub fn navigator_for(&self, index: usize) -> Option<&DiffNavigator> {
        self.navigators.get(index)?.as_ref()
    }

    pub fn restore_navigator_state(&mut self, index: usize, state: crate::step::StepState) -> bool {
        self.navigators
            .get_mut(index)
            .and_then(Option::as_mut)
            .is_some_and(|nav| nav.set_state(state))
    }

    pub fn current_navigator(&mut self) -> &mut DiffNavigator {
        self.ensure_patch_navigator(self.selected_index);
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

    pub fn append_patch(
        &mut self,
        patch: crate::patch::PatchRecord,
        old: ContentSource,
        new: ContentSource,
    ) -> usize {
        let index = self.files.len();
        let request = PendingFileContent {
            file_index: index,
            old,
            new,
        };
        self.files.push(patch.entry.clone());
        self.navigators.push(None);
        self.navigator_is_placeholder.push(false);
        self.old_contents.push(Arc::from(""));
        self.new_contents.push(Arc::from(""));
        self.precomputed_diffs.push(None);
        self.diff_statuses.push(DiffStatus::Ready);
        self.pending_content.push(None);
        self.remote_material.insert(
            index,
            RemoteMaterial::Patch {
                patch: Arc::new(patch),
                request: Box::new(request),
                hydration: HydrationState::Idle,
            },
        );
        index
    }

    fn ensure_patch_navigator(&mut self, index: usize) -> bool {
        let Some(patch) = self.patch_for_display(index) else {
            return false;
        };
        if self.navigators[index].is_none() {
            let diff = patch.display_diff().unwrap_or_else(|_| {
                Self::context_only_diff("Patch text is unavailable. Request full contents.")
            });
            self.navigators[index] = Some(
                if matches!(
                    self.remote_material.get(&index),
                    Some(RemoteMaterial::Full { .. })
                ) {
                    DiffNavigator::new(
                        diff,
                        self.old_contents[index].clone(),
                        self.new_contents[index].clone(),
                        true,
                    )
                } else {
                    DiffNavigator::from_patch(diff)
                },
            );
        }
        true
    }

    fn patch_for_display(&self, index: usize) -> Option<&Arc<crate::patch::PatchRecord>> {
        match self.remote_material.get(&index)? {
            RemoteMaterial::Patch { patch, .. } => Some(patch),
            RemoteMaterial::Full { display_patch, .. } => display_patch.as_ref(),
        }
    }

    pub fn has_patch_display(&self, index: usize) -> bool {
        self.patch_for_display(index).is_some()
    }

    pub fn file_is_sparse(&self, index: usize) -> bool {
        matches!(
            self.remote_material.get(&index),
            Some(RemoteMaterial::Patch { .. })
        )
    }

    pub fn ensure_full_content(&mut self, index: usize) -> bool {
        if matches!(
            self.remote_material.get(&index),
            Some(RemoteMaterial::Patch {
                hydration: HydrationState::Failed(_),
                ..
            })
        ) {
            return false;
        }
        self.request_full_content(index)
    }

    pub fn request_full_content(&mut self, index: usize) -> bool {
        let Some(RemoteMaterial::Patch {
            request, hydration, ..
        }) = self.remote_material.get_mut(&index)
        else {
            return false;
        };
        if *hydration == HydrationState::Loading {
            return false;
        }
        *hydration = HydrationState::Loading;
        self.pending_content[index] = Some(request.as_ref().clone());
        true
    }

    pub fn cancel_content_request(&mut self, index: usize) {
        if let Some(RemoteMaterial::Patch { hydration, .. }) = self.remote_material.get_mut(&index)
        {
            if *hydration == HydrationState::Loading {
                *hydration = HydrationState::Idle;
            }
        }
        if let Some(pending) = self.pending_content.get_mut(index) {
            *pending = None;
        }
    }

    pub fn content_snapshot(&self) -> u64 {
        self.remote_prefetch.snapshot
    }

    pub fn content_revision(&self, index: usize) -> u64 {
        self.remote_prefetch
            .revisions
            .get(index)
            .copied()
            .unwrap_or(0)
    }

    pub fn mark_content_presented(&mut self) {
        if !self.files.is_empty() {
            self.remote_prefetch.presented = true;
        }
    }

    pub fn suspend_remote_prefetch(&mut self) -> bool {
        !std::mem::replace(&mut self.remote_prefetch.suspended, true)
    }

    pub fn remote_prefetch_enabled(&self) -> bool {
        self.remote_snapshot
            .as_ref()
            .is_some_and(|s| s["acquisition"] == "patch-v1")
    }

    pub fn remote_content_bytes(&self, index: usize) -> usize {
        if self
            .remote_material
            .get(&index)
            .is_some_and(|m| matches!(m, RemoteMaterial::Full { .. }))
        {
            self.old_contents[index].len() + self.new_contents[index].len()
        } else {
            0
        }
    }

    pub fn remote_retained_bytes(&self) -> usize {
        self.remote_prefetch
            .resident
            .iter()
            .map(|i| self.remote_content_bytes(*i))
            .sum()
    }

    pub fn next_remote_prefetch(&mut self) -> Option<usize> {
        if !self.remote_prefetch_enabled()
            || !self.remote_prefetch.presented
            || self.remote_prefetch.suspended
        {
            return None;
        }
        let selected = self.selected_index;
        if self.remote_prefetch.selection != Some(selected) {
            if let Some(previous) = self.remote_prefetch.selection {
                if self.remote_prefetch.resident.contains(&previous) {
                    self.remote_prefetch.resident.retain(|i| *i != previous);
                    self.remote_prefetch.resident.push_back(previous);
                }
            }
            self.remote_prefetch.selection = Some(selected);
            self.remote_prefetch.nearby_attempted.clear();
        }
        self.remote_prefetch.swept.resize(self.files.len(), false);
        for index in selected.saturating_add(1)
            ..self
                .files
                .len()
                .min(selected.saturating_add(1 + REMOTE_NEARBY_FILES))
        {
            if self.remote_prefetch.nearby_attempted.insert(index) {
                self.remote_prefetch.swept[index] = true;
                if self.prefetch_eligible(index) {
                    return Some(index);
                }
            }
        }
        while self.remote_prefetch.cursor < self.files.len() {
            let index = self.remote_prefetch.cursor;
            self.remote_prefetch.cursor += 1;
            if !std::mem::replace(&mut self.remote_prefetch.swept[index], true)
                && index != selected
                && self.prefetch_eligible(index)
            {
                return Some(index);
            }
        }
        None
    }

    fn prefetch_eligible(&self, index: usize) -> bool {
        !self.files[index].binary
            && matches!(
                self.remote_material.get(&index),
                Some(RemoteMaterial::Patch {
                    hydration: HydrationState::Idle,
                    ..
                })
            )
    }

    pub fn remote_retention_victims(&self) -> Vec<usize> {
        let selected = self.selected_index;
        let nearby = selected.saturating_add(1)..selected.saturating_add(1 + REMOTE_NEARBY_FILES);
        let mut near_bytes = 0usize;
        let mut keep = std::collections::HashSet::new();
        let mut victims = Vec::new();
        for index in nearby.clone().take(REMOTE_NEARBY_FILES) {
            if !self.remote_prefetch.resident.contains(&index) {
                continue;
            }
            let size = self.remote_content_bytes(index);
            if near_bytes + size <= REMOTE_CLASS_BYTES {
                near_bytes += size;
                keep.insert(index);
            } else {
                victims.push(index);
            }
        }
        let mut tail_bytes = 0usize;
        let mut tail_count = 0usize;
        for index in self.remote_prefetch.resident.iter().rev().copied() {
            if index == selected || keep.contains(&index) || nearby.contains(&index) {
                continue;
            }
            let size = self.remote_content_bytes(index);
            if tail_bytes + size <= REMOTE_CLASS_BYTES && tail_count < REMOTE_TAIL_FILES {
                tail_bytes += size;
                tail_count += 1;
            } else {
                victims.push(index);
            }
        }
        victims
    }

    pub fn evict_remote_content(&mut self, index: usize) -> bool {
        if index == self.selected_index {
            return false;
        }
        let Some(RemoteMaterial::Full {
            original_patch,
            request,
            ..
        }) = self.remote_material.get(&index)
        else {
            return false;
        };
        let patch = original_patch.clone();
        let request = request.clone();
        self.files[index] = patch.entry.clone();
        self.remote_material.insert(
            index,
            RemoteMaterial::Patch {
                patch,
                request,
                hydration: HydrationState::Idle,
            },
        );
        self.old_contents[index] = Arc::from("");
        self.new_contents[index] = Arc::from("");
        self.precomputed_diffs[index] = None;
        self.navigators[index] = None;
        self.navigator_is_placeholder[index] = false;
        self.pending_content[index] = None;
        self.diff_statuses[index] = DiffStatus::Ready;
        self.remote_prefetch.revisions.resize(self.files.len(), 0);
        self.remote_prefetch.revisions[index] =
            self.remote_prefetch.revisions[index].wrapping_add(1);
        self.remote_prefetch.resident.retain(|i| *i != index);
        true
    }

    pub fn file_modes(&self, index: usize) -> Option<(u32, u32)> {
        match self.remote_material.get(&index)? {
            RemoteMaterial::Patch { patch, .. } => Some((patch.old_mode, patch.new_mode)),
            RemoteMaterial::Full { original_patch, .. } => {
                Some((original_patch.old_mode, original_patch.new_mode))
            }
        }
    }

    pub fn hydration_failure(&self, index: usize) -> Option<&str> {
        match self.remote_material.get(&index)? {
            RemoteMaterial::Patch {
                hydration: HydrationState::Failed(error),
                ..
            } => Some(error),
            _ => None,
        }
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

    pub fn failed_file_content(message: &str) -> PreparedFileContent {
        PreparedFileContent {
            observed: None,
            old: Arc::from(""),
            new: Arc::from(""),
            binary: false,
            precomputed: Some(PrecomputedDiff::Placeholder(Self::context_only_diff(
                message,
            ))),
            status: DiffStatus::Failed,
        }
    }

    pub fn take_pending_content_for_sync(&mut self, idx: usize) -> Option<PendingFileContent> {
        let request = self.pending_content.get(idx)?.as_ref()?;
        if matches!(request.old, ContentSource::RemoteBlob { .. })
            || matches!(request.new, ContentSource::RemoteBlob { .. })
        {
            return None;
        }
        self.take_pending_content_for(idx)
    }

    pub fn prepare_remote_file_content(old: Vec<u8>, new: Vec<u8>) -> PreparedFileContent {
        Self::prepare_remote_content(old, new, false)
    }

    pub fn prepare_buffered_remote_content(old: Vec<u8>, new: Vec<u8>) -> PreparedFileContent {
        Self::prepare_remote_content(old, new, true)
    }

    fn prepare_remote_content(old: Vec<u8>, new: Vec<u8>, buffered: bool) -> PreparedFileContent {
        if old.len() > 32 * 1024 * 1024 || new.len() > 32 * 1024 * 1024 {
            return Self::failed_file_content("Remote file exceeds its loading limit");
        }
        let (Ok(old_id), Ok(new_id)) = (BlobId::from_bytes(&old), BlobId::from_bytes(&new)) else {
            return Self::failed_file_content("Remote file identity is unavailable");
        };
        let observed = ObservedContent {
            old: old_id,
            new: new_id,
        };
        let binary = old.contains(&0)
            || new.contains(&0)
            || std::str::from_utf8(&old).is_err()
            || std::str::from_utf8(&new).is_err();
        let old = String::from_utf8(old).unwrap_or_default();
        let new = String::from_utf8(new).unwrap_or_default();
        let (old, new, mut precomputed, status) = if buffered && !binary {
            {
                let status = if Self::should_defer_diff(&old, &new) && !Self::diff_defer_enabled() {
                    DiffStatus::Disabled
                } else {
                    DiffStatus::Buffered
                };
                (old, new, None, status)
            }
        } else {
            Self::maybe_defer_diff(old, new, binary)
        };
        if status == DiffStatus::Ready {
            precomputed = Some(PrecomputedDiff::Ready(Self::diff_strings(&old, &new)));
        }
        PreparedFileContent {
            observed: Some(observed),
            old: Arc::from(old),
            new: Arc::from(new),
            binary,
            precomputed,
            status,
        }
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
            observed: None,
            old: Arc::from(old),
            new: Arc::from(new),
            binary: old_binary || new_binary,
            precomputed,
            status,
        }
    }

    pub fn apply_prepared_content(&mut self, idx: usize, mut content: PreparedFileContent) -> bool {
        if let (Some(expected), Some(RemoteMaterial::Patch { request, .. })) =
            (&self.remote_content_proof, self.remote_material.get(&idx))
        {
            if content.status != DiffStatus::Failed
                && !expected
                    .get(idx)
                    .is_some_and(|expected| expected.accepts(request, content.observed))
            {
                content =
                    Self::failed_file_content("Full contents do not match the validated snapshot");
            }
        }
        if let Some(RemoteMaterial::Patch {
            patch,
            request,
            hydration,
        }) = self.remote_material.get_mut(&idx)
        {
            if *hydration != HydrationState::Loading {
                return false;
            }
            if content.status == DiffStatus::Failed {
                *hydration = HydrationState::Failed(
                    "Full contents could not be loaded. The patch is retained.".to_owned(),
                );
                return true;
            }
            if patch.content_kind() == crate::patch::PatchContentKind::BinaryObjects {
                content.old = Arc::from("");
                content.new = Arc::from("");
                content.binary = true;
                content.precomputed = None;
                content.status = DiffStatus::Disabled;
            }
            let original_patch = patch.clone();
            let request = request.clone();
            let display_patch = ((!patch.has_changed_lines()
                || content.status != DiffStatus::Ready)
                && !content.binary)
                .then(|| patch.clone());
            self.remote_material.insert(
                idx,
                RemoteMaterial::Full {
                    original_patch,
                    request,
                    observed: content.observed,
                    display_patch,
                },
            );
        } else if self.diff_status(idx) != DiffStatus::Loading {
            return false;
        }
        self.remote_prefetch.revisions.resize(self.files.len(), 0);
        self.remote_prefetch.revisions[idx] = self.remote_prefetch.revisions[idx].wrapping_add(1);
        if self.remote_material.contains_key(&idx) {
            self.remote_prefetch.resident.retain(|index| *index != idx);
            self.remote_prefetch.resident.push_back(idx);
        }
        self.old_contents[idx] = content.old;
        self.new_contents[idx] = content.new;
        self.precomputed_diffs[idx] = content.precomputed;
        self.diff_statuses[idx] = content.status;
        self.files[idx].binary = content.binary;
        if !self.has_patch_display(idx) {
            self.navigators[idx] = None;
            self.navigator_is_placeholder[idx] = false;
        }
        true
    }

    pub fn remote_content_mismatches(
        &self,
        expected: &[ExpectedFileContent],
    ) -> Result<Vec<usize>, &'static str> {
        if expected.len() != self.files.len() {
            return Err("Content proof has a different file manifest");
        }
        let mut mismatches = Vec::new();
        for (index, expected) in expected.iter().enumerate() {
            let (request, observed) = match self.remote_material.get(&index) {
                Some(RemoteMaterial::Patch { request, .. }) => (request, None),
                Some(RemoteMaterial::Full {
                    request, observed, ..
                }) => (request, *observed),
                None => return Err("Content proof lacks a matching remote file"),
            };
            if expected.old.is_none() != matches!(request.old, ContentSource::Empty)
                || expected.new.is_none() != matches!(request.new, ContentSource::Empty)
            {
                return Err("Content proof disagrees with object presence");
            }
            if expected.request.file_index != index || expected.request != **request {
                return Err("Content proof belongs to a different immutable request");
            }
            if matches!(
                self.remote_material.get(&index),
                Some(RemoteMaterial::Full { .. })
            ) && !expected.accepts(request, observed)
            {
                mismatches.push(index);
            }
        }
        Ok(mismatches)
    }

    pub fn bind_remote_content_proof(
        &mut self,
        expected: Arc<[ExpectedFileContent]>,
    ) -> Result<Vec<usize>, &'static str> {
        let mismatches = self.remote_content_mismatches(&expected)?;
        self.remote_content_proof = Some(expected);
        for index in &mismatches {
            let Some(RemoteMaterial::Full {
                original_patch,
                request,
                ..
            }) = self.remote_material.get(index)
            else {
                continue;
            };
            let patch = original_patch.clone();
            let request = request.clone();
            self.files[*index] = patch.entry.clone();
            self.remote_material.insert(
                *index,
                RemoteMaterial::Patch {
                    patch,
                    request,
                    hydration: HydrationState::Failed(
                        "Full contents do not match the validated snapshot. The patch is retained."
                            .to_owned(),
                    ),
                },
            );
            self.remote_prefetch.revisions.resize(self.files.len(), 0);
            self.remote_prefetch.revisions[*index] =
                self.remote_prefetch.revisions[*index].wrapping_add(1);
            self.remote_prefetch.resident.retain(|i| i != index);
            self.old_contents[*index] = Arc::from("");
            self.new_contents[*index] = Arc::from("");
            self.precomputed_diffs[*index] = None;
            self.pending_content[*index] = None;
            self.diff_statuses[*index] = DiffStatus::Ready;
            self.navigators[*index] = None;
            self.navigator_is_placeholder[*index] = false;
        }
        Ok(mismatches)
    }

    pub fn file_contents(&self, idx: usize) -> Option<(&str, &str)> {
        if self.file_is_sparse(idx) {
            return None;
        }
        if matches!(
            self.diff_status(idx),
            DiffStatus::Loading | DiffStatus::Failed
        ) || (self.remote_snapshot.is_some()
            && self.files.get(idx).is_some_and(|file| file.binary))
        {
            return None;
        }
        let old = self.old_contents.get(idx)?;
        let new = self.new_contents.get(idx)?;
        Some((old.as_ref(), new.as_ref()))
    }

    pub fn file_contents_arc(&self, idx: usize) -> Option<(Arc<str>, Arc<str>)> {
        if self.file_is_sparse(idx) {
            return None;
        }
        if matches!(
            self.diff_status(idx),
            DiffStatus::Loading | DiffStatus::Failed
        ) || (self.remote_snapshot.is_some()
            && self.files.get(idx).is_some_and(|file| file.binary))
        {
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
        if self.remote_snapshot.is_some() {
            return None;
        }
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
        if self.has_patch_display(self.selected_index) {
            return false;
        }
        matches!(
            self.diff_statuses.get(self.selected_index),
            Some(
                DiffStatus::Loading
                    | DiffStatus::Buffered
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
        if let Some(RemoteMaterial::Full {
            display_patch,
            original_patch,
            ..
        }) = self.remote_material.get_mut(&idx)
        {
            if original_patch.has_changed_lines() && display_patch.take().is_some() {
                self.navigator_is_placeholder[idx] = true;
            }
        }
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
        if self.ensure_patch_navigator(idx) {
            return;
        }
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
        let Some(stats) = Self::git_numstat_for_mode(&repo_root, &mode) else {
            return false;
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
                            Self::read_git_worktree_or_binary(&full_path)
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
        let git_mode = self.git_mode.is_some();
        let git_stats = match (&self.repo_root, &self.git_mode) {
            (Some(repo_root), Some(mode)) => {
                let file = &self.files[idx];
                let old_path = file.old_path.as_deref().unwrap_or(&file.path);
                let mut paths = vec![file.path.clone()];
                if old_path != file.path {
                    paths.push(old_path.to_path_buf());
                }
                Self::git_numstat_for_file(repo_root, mode, &paths)
            }
            _ => None,
        };
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
                            Self::read_git_worktree_or_binary(&full_path)
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
        let (insertions, deletions) = if let Some(stats) = git_stats {
            stats
                .get(&self.files[idx].path)
                .copied()
                .unwrap_or_else(|| {
                    if self.files[idx].status == FileStatus::Untracked && !binary {
                        (new_content.lines().count(), 0)
                    } else {
                        (0, 0)
                    }
                })
        } else if git_mode {
            (self.files[idx].insertions, self.files[idx].deletions)
        } else {
            Self::diff_stats(&old_content, &new_content, binary)
        };
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
        _ if (7..=64).contains(&reference.len())
            && reference.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            shorten_hash(reference)
        }
        _ => reference.to_string(),
    }
}

fn shorten_hash(hash: &str) -> String {
    hash.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    fn proof_bound_patch() -> (
        super::MultiFileDiff,
        std::sync::Arc<[super::ExpectedFileContent]>,
    ) {
        use super::*;
        let old = ContentSource::RemoteBlob {
            provider: "gitlab".to_owned(),
            host: "fixture.test".to_owned(),
            repo: "11".to_owned(),
            revision: "1".repeat(40),
            path: "f".to_owned(),
        };
        let new = ContentSource::RemoteBlob {
            provider: "gitlab".to_owned(),
            host: "fixture.test".to_owned(),
            repo: "22".to_owned(),
            revision: "2".repeat(40),
            path: "f".to_owned(),
        };
        let patch=crate::patch::PatchRecord::parse(b"diff --git a/f b/f\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n".to_vec()).unwrap();
        let mut diff = MultiFileDiff::from_pending_files(None, Vec::new(), true);
        diff.append_patch(patch, old.clone(), new.clone());
        diff.remote_snapshot =
            Some(serde_json::json!({"acquisition":"patch-v1","streamState":"receiving"}));
        let expected = vec![ExpectedFileContent {
            request: PendingFileContent {
                file_index: 0,
                old,
                new,
            },
            old: Some(BlobId::from_bytes(b"old\n").unwrap()),
            new: Some(BlobId::from_bytes(b"new\n").unwrap()),
        }];
        (diff, expected.into())
    }

    #[test]
    fn remote_retention_protects_nearby_files_while_a_bounded_tail_advances() {
        let mut diff = MultiFileDiff::from_pending_files(None, Vec::new(), true);
        let padding = "same\n".repeat(800_000);
        let old = format!("old\n{padding}");
        let new = format!("new\n{padding}");
        for index in 0..40 {
            let name = format!("file{index}");
            let patch=format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -1 +1 @@\n-old\n+new\n");
            diff.append_patch(
                crate::patch::PatchRecord::parse(patch.into_bytes()).unwrap(),
                ContentSource::Empty,
                ContentSource::Empty,
            );
        }
        diff.remote_snapshot =
            Some(serde_json::json!({"acquisition":"patch-v1","streamState":"receiving"}));
        diff.mark_content_presented();
        for index in 0..40 {
            assert!(diff.request_full_content(index));
            diff.take_pending_content_for(index);
            assert!(diff.apply_prepared_content(
                index,
                MultiFileDiff::prepare_buffered_remote_content(
                    old.as_bytes().to_vec(),
                    new.as_bytes().to_vec()
                )
            ));
            for victim in diff.remote_retention_victims() {
                assert!(diff.evict_remote_content(victim));
            }
            assert!(diff.remote_retained_bytes() <= 192 * 1024 * 1024);
            assert!(diff.navigator_for(index).is_none());
        }
        assert!(diff.file_contents(0).is_some());
        for index in 1..=8 {
            assert!(diff.file_contents(index).is_some());
        }
        assert!(diff.file_is_sparse(9));
        assert!(diff.file_contents(39).is_some());
        assert_eq!(
            diff.diff_status(39),
            if MultiFileDiff::diff_defer_enabled() {
                DiffStatus::Buffered
            } else {
                DiffStatus::Disabled
            }
        );
    }

    #[test]
    fn buffered_remote_bytes_are_available_without_a_computed_diff_or_navigator() {
        let (mut diff, expected) = proof_bound_patch();
        assert!(diff.request_full_content(0));
        diff.take_pending_content();
        let content = super::MultiFileDiff::prepare_buffered_remote_content(
            b"old\n".to_vec(),
            b"new\n".to_vec(),
        );
        assert!(diff.apply_prepared_content(0, content));
        assert_eq!(diff.diff_status(0), super::DiffStatus::Buffered);
        assert!(diff.precomputed_diffs[0].is_none());
        assert!(diff.navigators[0].is_none());
        assert!(diff.has_patch_display(0));
        assert_eq!(diff.file_contents(0), Some(("old\n", "new\n")));
        assert!(diff.bind_remote_content_proof(expected).unwrap().is_empty());
        assert_eq!(diff.remote_retained_bytes(), 8);
    }

    #[test]
    fn prefetch_waits_for_presentation_and_sweeps_once_even_after_eviction() {
        use super::*;
        let (mut diff, _) = proof_bound_patch();
        for index in 1..14 {
            let patch = format!("diff --git a/f{index} b/f{index}\nindex 1111111..2222222 100644\n--- a/f{index}\n+++ b/f{index}\n@@ -1 +1 @@\n-old\n+new\n");
            diff.append_patch(
                crate::patch::PatchRecord::parse(patch.into_bytes()).unwrap(),
                ContentSource::Empty,
                ContentSource::Empty,
            );
        }
        assert_eq!(diff.next_remote_prefetch(), None);
        diff.mark_content_presented();
        for index in 1..14 {
            assert_eq!(diff.next_remote_prefetch(), Some(index));
            assert!(diff.ensure_full_content(index));
            diff.take_pending_content();
            assert!(diff.apply_prepared_content(
                index,
                MultiFileDiff::prepare_buffered_remote_content(
                    b"old\n".to_vec(),
                    b"new\n".to_vec()
                )
            ));
            assert!(diff.evict_remote_content(index));
            assert!(diff.file_is_sparse(index));
        }
        assert_eq!(diff.next_remote_prefetch(), None);
        diff.selected_index = 1;
        assert_eq!(diff.next_remote_prefetch(), Some(2));
        diff.suspend_remote_prefetch();
        assert_eq!(diff.next_remote_prefetch(), None);
    }

    #[test]
    fn provider_binary_classification_survives_ascii_hydration() {
        use super::*;
        let (_, expected) = proof_bound_patch();
        let patch = crate::patch::PatchRecord::parse(
            format!(
                "diff --git a/f b/f\nindex {}..{} 100644\nBinary files a/f and b/f differ\n",
                expected[0].old.unwrap(),
                expected[0].new.unwrap()
            )
            .into_bytes(),
        )
        .unwrap();
        let mut diff = MultiFileDiff::from_pending_files(None, Vec::new(), true);
        diff.append_patch(
            patch,
            expected[0].request.old.clone(),
            expected[0].request.new.clone(),
        );
        diff.remote_snapshot = Some(serde_json::json!({"acquisition":"patch-v1"}));
        assert!(diff.request_full_content(0));
        diff.take_pending_content();
        assert!(diff.apply_prepared_content(
            0,
            MultiFileDiff::prepare_remote_file_content(b"old\n".to_vec(), b"new\n".to_vec())
        ));
        assert!(diff.files[0].binary);
        assert!(diff.file_contents(0).is_none());
        assert!(diff.bind_remote_content_proof(expected).unwrap().is_empty());
        assert!(diff.files[0].binary);
    }

    #[test]
    fn content_proof_demotes_bad_installed_bytes_and_guards_every_later_installation() {
        let (mut diff, expected) = proof_bound_patch();
        assert!(diff.request_full_content(0));
        diff.take_pending_content();
        assert!(diff.apply_prepared_content(
            0,
            super::MultiFileDiff::prepare_remote_file_content(
                b"old\n".to_vec(),
                b"wrong\xff".to_vec()
            )
        ));
        assert!(!diff.file_is_sparse(0));
        assert!(diff.files[0].binary);
        assert_eq!(diff.remote_content_mismatches(&expected).unwrap(), vec![0]);
        assert_eq!(diff.bind_remote_content_proof(expected).unwrap(), vec![0]);
        assert!(diff.file_is_sparse(0));
        assert!(!diff.files[0].binary);
        assert!(diff.file_contents(0).is_none());
        assert!(diff.hydration_failure(0).is_some());
        assert!(!diff.ensure_full_content(0));
        let nav = diff.current_navigator();
        assert!(nav
            .diff()
            .changes
            .iter()
            .any(|change| change.spans.iter().any(|span| span.text == "new")));
        assert!(diff.request_full_content(0));
        diff.take_pending_content();
        assert!(diff.apply_prepared_content(
            0,
            super::MultiFileDiff::prepare_remote_file_content(
                b"old\n".to_vec(),
                b"wrong\n".to_vec()
            )
        ));
        assert!(diff.file_is_sparse(0));
        assert!(diff.hydration_failure(0).is_some());
        assert!(diff.request_full_content(0));
        diff.take_pending_content();
        assert!(diff.apply_prepared_content(
            0,
            super::MultiFileDiff::prepare_remote_file_content(b"old\n".to_vec(), b"new\n".to_vec())
        ));
        assert_eq!(diff.file_contents(0), Some(("old\n", "new\n")));
    }

    #[test]
    fn content_proof_binds_fork_request_and_distinguishes_absence_from_empty_blobs() {
        use super::*;
        let (mut diff, expected) = proof_bound_patch();
        let mut wrong = expected.to_vec();
        wrong[0].request.new = wrong[0].request.old.clone();
        assert!(diff.bind_remote_content_proof(wrong.into()).is_err());
        let empty = BlobId::from_bytes(&[]).unwrap();
        let request = PendingFileContent {
            file_index: 0,
            old: ContentSource::Empty,
            new: expected[0].request.new.clone(),
        };
        let present_empty = ExpectedFileContent {
            request: request.clone(),
            old: None,
            new: Some(empty),
        };
        let bytes = Some(ObservedContent {
            old: empty,
            new: empty,
        });
        assert!(present_empty.accepts(&request, bytes));
        let absent = ExpectedFileContent {
            request: request.clone(),
            old: None,
            new: None,
        };
        assert!(!absent.accepts(&request, bytes));
        assert!(!present_empty.accepts(&request, None));
    }

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

    #[test]
    fn identical_binary_files_are_skipped_in_directory_diffs() {
        let root = temp_dir("identical-binary");
        let old_dir = root.join("old");
        let new_dir = root.join("new");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("image.bin"), [0, 1, 2]).unwrap();
        std::fs::create_dir_all(&new_dir).unwrap();
        std::fs::copy(old_dir.join("image.bin"), new_dir.join("image.bin")).unwrap();
        for dir in [&old_dir, &new_dir] {
            let large = std::fs::File::create(dir.join("large.bin")).unwrap();
            large.set_len(MultiFileDiff::MAX_TEXT_BYTES + 1).unwrap();
        }

        let diff = MultiFileDiff::from_directories(&old_dir, &new_dir).unwrap();

        assert!(diff.files.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn long_multibyte_lines_are_not_truncated_by_byte_length() {
        let line = "é".repeat(10_000);
        let diff = MultiFileDiff::from_file_pairs(vec![(
            PathBuf::from("unicode.txt"),
            line.clone(),
            line.clone(),
        )]);

        assert_eq!(diff.file_contents(0), Some((line.as_str(), line.as_str())));
    }

    #[cfg(unix)]
    #[test]
    fn uncommitted_symlink_diff_reads_the_link_target() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlink-target");
        std::fs::create_dir_all(&root).unwrap();
        run_git(&root, &["init", "-q"]);
        run_git(&root, &["config", "user.email", "test@example.com"]);
        run_git(&root, &["config", "user.name", "Test"]);
        write_file(&root.join("a.txt"), "alpha content\n");
        write_file(&root.join("b.txt"), "beta content\n");
        symlink("a.txt", root.join("link")).unwrap();
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-qm", "base"]);

        std::fs::remove_file(root.join("link")).unwrap();
        symlink("b.txt", root.join("link")).unwrap();

        let changes = crate::git::get_uncommitted_changes(&root).unwrap();
        let mut diff = MultiFileDiff::from_git_changes(root.clone(), changes).unwrap();
        assert_eq!(diff.file_contents(0), Some(("a.txt", "b.txt")));
        diff.refresh_file(0);
        assert_eq!(diff.file_contents(0), Some(("a.txt", "b.txt")));

        let changes = crate::git::get_uncommitted_changes(&root).unwrap();
        let lazy = MultiFileDiff::from_git_changes_lazy(root.clone(), changes).unwrap();
        assert_eq!(lazy.file_contents(0), Some(("a.txt", "b.txt")));
        assert_eq!(lazy.pending_content_count(), 0);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn git_refreshes_keep_numstat_for_large_text_files() {
        use std::fmt::Write as _;

        let root = temp_dir("large-refresh-numstat");
        std::fs::create_dir_all(&root).unwrap();
        run_git(&root, &["init", "-q"]);
        run_git(&root, &["config", "user.email", "test@example.com"]);
        run_git(&root, &["config", "user.name", "Test"]);

        let mut old = String::with_capacity(6_000_000);
        let mut new = String::with_capacity(6_000_000);
        for line in 0..300_000 {
            writeln!(&mut old, "row {line:06} original payload").unwrap();
            if line < 300 {
                writeln!(&mut new, "row {line:06} updated payload").unwrap();
            } else {
                writeln!(&mut new, "row {line:06} original payload").unwrap();
            }
        }
        assert!(new.len() > MultiFileDiff::MAX_WORD_LEVEL_BYTES as usize);
        write_file(&root.join("big.txt"), &old);
        run_git(&root, &["add", "big.txt"]);
        run_git(&root, &["commit", "-qm", "base"]);
        write_file(&root.join("big.txt"), &new);

        let changes = crate::git::get_uncommitted_changes(&root).unwrap();
        let mut diff = MultiFileDiff::from_git_changes(root.clone(), changes).unwrap();
        assert_eq!(
            (diff.files[0].insertions, diff.files[0].deletions),
            (300, 300)
        );
        diff.refresh_file(0);
        assert_eq!(
            (diff.files[0].insertions, diff.files[0].deletions),
            (300, 300)
        );
        assert!(diff.refresh_all_from_git());
        assert_eq!(
            (diff.files[0].insertions, diff.files[0].deletions),
            (300, 300)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    fn display_names(diff: &MultiFileDiff) -> Vec<String> {
        diff.files
            .iter()
            .map(|file| file.display_name.clone())
            .collect()
    }

    #[test]
    fn long_line_changes_after_display_limit_remain_diffable() {
        let old = format!("{}A\n", "x".repeat(20_000));
        let new = format!("{}B\n", "x".repeat(20_000));
        let mut diff = MultiFileDiff::from_file_pairs(vec![(PathBuf::from("long.txt"), old, new)]);

        assert!(!diff
            .current_navigator()
            .diff()
            .significant_changes
            .is_empty());
    }

    #[test]
    fn git_range_labels_preserve_ref_names_and_shorten_object_ids() {
        assert_eq!(format_ref("main"), "main");
        assert_eq!(format_ref("feature/login"), "feature/login");
        assert_eq!(format_ref("release/1.0"), "release/1.0");
        assert_eq!(format_ref("0123456789abcdef"), "0123456");
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

#[cfg(test)]
mod remote_content_tests {
    use super::*;

    #[test]
    fn remote_text_preserves_the_tail_of_long_lines() {
        let text = format!("{}tail\n", "x".repeat(20_000));
        let prepared =
            MultiFileDiff::prepare_remote_file_content(Vec::new(), text.as_bytes().to_vec());
        assert_eq!(&*prepared.new, text);
        assert!(!prepared.binary);
    }

    #[test]
    fn failed_remote_content_is_not_available_as_empty_text() {
        let entry = FileEntry {
            path: "file.txt".into(),
            old_path: None,
            old_source_path: None,
            new_source_path: None,
            display_name: "file.txt".into(),
            status: FileStatus::Modified,
            insertions: 0,
            deletions: 0,
            binary: false,
        };
        let mut diff = MultiFileDiff::from_pending_files(
            None,
            vec![(entry, ContentSource::Empty, ContentSource::Empty)],
            false,
        );
        diff.remote_snapshot = Some(serde_json::json!({"version":1}));
        assert!(
            diff.apply_prepared_content(0, MultiFileDiff::failed_file_content("Download failed"))
        );
        assert!(diff.file_contents(0).is_none());
        assert!(diff.file_contents_arc(0).is_none());
        assert!(!diff.files[0].binary);
        assert_eq!(diff.diff_status(0), DiffStatus::Failed);
    }
}
