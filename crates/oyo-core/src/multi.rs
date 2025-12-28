//! Multi-file diff support

use crate::diff::DiffEngine;
use crate::git::{ChangedFile, FileStatus};
use crate::step::{DiffNavigator, StepDirection};
use std::path::{Path, PathBuf};
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
    pub display_name: String,
    pub status: FileStatus,
    pub insertions: usize,
    pub deletions: usize,
}

/// Multi-file diff session
pub struct MultiFileDiff {
    /// All files being diffed
    pub files: Vec<FileEntry>,
    /// Currently selected file index
    pub selected_index: usize,
    /// Navigators for each file (lazy loaded)
    navigators: Vec<Option<DiffNavigator>>,
    /// Repository root (if in git mode)
    #[allow(dead_code)]
    repo_root: Option<PathBuf>,
    /// Git diff mode (if in git mode)
    git_mode: Option<GitDiffMode>,
    /// Old contents for each file
    old_contents: Vec<String>,
    /// New contents for each file
    new_contents: Vec<String>,
}

#[derive(Debug, Clone)]
enum GitDiffMode {
    Uncommitted,
    Staged,
    IndexRange { from: String, to_index: bool },
    Range { from: String, to: String },
}

impl MultiFileDiff {
    /// Create from a list of changed files (git mode)
    pub fn from_git_changes(
        repo_root: PathBuf,
        changes: Vec<ChangedFile>,
    ) -> Result<Self, MultiDiffError> {
        let mut files = Vec::new();
        let mut old_contents = Vec::new();
        let mut new_contents = Vec::new();
        let engine = DiffEngine::new().with_word_level(true);

        for change in changes {
            // Get old and new content
            let old_content = match change.status {
                FileStatus::Added | FileStatus::Untracked => String::new(),
                _ => crate::git::get_head_content(&repo_root, &change.path).unwrap_or_default(),
            };

            let new_content = match change.status {
                FileStatus::Deleted => String::new(),
                _ => {
                    let full_path = repo_root.join(&change.path);
                    std::fs::read_to_string(&full_path).unwrap_or_default()
                }
            };

            // Compute diff stats
            let diff = engine.diff_strings(&old_content, &new_content);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                status: change.status,
                insertions: diff.insertions,
                deletions: diff.deletions,
            });

            old_contents.push(old_content);
            new_contents.push(new_content);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            repo_root: Some(repo_root),
            git_mode: Some(GitDiffMode::Uncommitted),
            old_contents,
            new_contents,
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
        let engine = DiffEngine::new().with_word_level(true);

        for change in changes {
            let old_path = change
                .old_path
                .clone()
                .unwrap_or_else(|| change.path.clone());
            let old_content = match change.status {
                FileStatus::Added | FileStatus::Untracked => String::new(),
                _ => crate::git::get_head_content(&repo_root, &old_path).unwrap_or_default(),
            };

            let new_content = match change.status {
                FileStatus::Deleted => String::new(),
                _ => crate::git::get_staged_content(&repo_root, &change.path).unwrap_or_default(),
            };

            let diff = engine.diff_strings(&old_content, &new_content);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                status: change.status,
                insertions: diff.insertions,
                deletions: diff.deletions,
            });

            old_contents.push(old_content);
            new_contents.push(new_content);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            repo_root: Some(repo_root),
            git_mode: Some(GitDiffMode::Staged),
            old_contents,
            new_contents,
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
        let engine = DiffEngine::new().with_word_level(true);

        for change in changes {
            let old_path = change
                .old_path
                .clone()
                .unwrap_or_else(|| change.path.clone());
            let (old_content, new_content) = if to_index {
                let old_content = match change.status {
                    FileStatus::Added | FileStatus::Untracked => String::new(),
                    _ => crate::git::get_file_at_commit(&repo_root, &from, &old_path)
                        .unwrap_or_default(),
                };
                let new_content = match change.status {
                    FileStatus::Deleted => String::new(),
                    _ => {
                        crate::git::get_staged_content(&repo_root, &change.path).unwrap_or_default()
                    }
                };
                (old_content, new_content)
            } else {
                let old_content = match change.status {
                    FileStatus::Added | FileStatus::Untracked => String::new(),
                    _ => crate::git::get_staged_content(&repo_root, &old_path).unwrap_or_default(),
                };
                let new_content = match change.status {
                    FileStatus::Deleted => String::new(),
                    _ => crate::git::get_file_at_commit(&repo_root, &from, &change.path)
                        .unwrap_or_default(),
                };
                (old_content, new_content)
            };

            let diff = engine.diff_strings(&old_content, &new_content);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                status: change.status,
                insertions: diff.insertions,
                deletions: diff.deletions,
            });

            old_contents.push(old_content);
            new_contents.push(new_content);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            repo_root: Some(repo_root),
            git_mode: Some(GitDiffMode::IndexRange { from, to_index }),
            old_contents,
            new_contents,
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
        let engine = DiffEngine::new().with_word_level(true);

        for change in changes {
            let old_path = change
                .old_path
                .clone()
                .unwrap_or_else(|| change.path.clone());
            let old_content = match change.status {
                FileStatus::Added | FileStatus::Untracked => String::new(),
                _ => {
                    crate::git::get_file_at_commit(&repo_root, &from, &old_path).unwrap_or_default()
                }
            };

            let new_content = match change.status {
                FileStatus::Deleted => String::new(),
                _ => crate::git::get_file_at_commit(&repo_root, &to, &change.path)
                    .unwrap_or_default(),
            };

            let diff = engine.diff_strings(&old_content, &new_content);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                status: change.status,
                insertions: diff.insertions,
                deletions: diff.deletions,
            });

            old_contents.push(old_content);
            new_contents.push(new_content);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            repo_root: Some(repo_root),
            git_mode: Some(GitDiffMode::Range { from, to }),
            old_contents,
            new_contents,
        })
    }

    /// Create from two directories
    pub fn from_directories(old_dir: &Path, new_dir: &Path) -> Result<Self, MultiDiffError> {
        let mut files = Vec::new();
        let mut old_contents = Vec::new();
        let mut new_contents = Vec::new();
        let engine = DiffEngine::new().with_word_level(true);

        // Collect all files from both directories
        let mut all_files = std::collections::HashSet::new();

        if old_dir.is_dir() {
            collect_files(old_dir, old_dir, &mut all_files)?;
        }
        if new_dir.is_dir() {
            collect_files(new_dir, new_dir, &mut all_files)?;
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

            let old_content = if old_exists {
                std::fs::read_to_string(&old_path).unwrap_or_default()
            } else {
                String::new()
            };

            let new_content = if new_exists {
                std::fs::read_to_string(&new_path).unwrap_or_default()
            } else {
                String::new()
            };

            // Skip if no changes
            if old_content == new_content {
                continue;
            }

            let diff = engine.diff_strings(&old_content, &new_content);

            files.push(FileEntry {
                display_name: rel_path.display().to_string(),
                path: rel_path,
                old_path: None,
                status,
                insertions: diff.insertions,
                deletions: diff.deletions,
            });

            old_contents.push(old_content);
            new_contents.push(new_content);
        }

        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();

        Ok(Self {
            files,
            selected_index: 0,
            navigators,
            repo_root: None,
            git_mode: None,
            old_contents,
            new_contents,
        })
    }

    /// Create from a single file pair
    pub fn from_file_pair(
        _old_path: PathBuf,
        new_path: PathBuf,
        old_content: String,
        new_content: String,
    ) -> Self {
        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(&old_content, &new_content);

        let files = vec![FileEntry {
            display_name: new_path.display().to_string(),
            path: new_path,
            old_path: None,
            status: FileStatus::Modified,
            insertions: diff.insertions,
            deletions: diff.deletions,
        }];

        Self {
            files,
            selected_index: 0,
            navigators: vec![None],
            repo_root: None,
            git_mode: None,
            old_contents: vec![old_content],
            new_contents: vec![new_content],
        }
    }

    /// Create from multiple file pairs.
    pub fn from_file_pairs(pairs: Vec<(PathBuf, String, String)>) -> Self {
        let engine = DiffEngine::new().with_word_level(true);
        let mut files = Vec::with_capacity(pairs.len());
        let mut old_contents = Vec::with_capacity(pairs.len());
        let mut new_contents = Vec::with_capacity(pairs.len());

        for (path, old_content, new_content) in pairs {
            let diff = engine.diff_strings(&old_content, &new_content);
            files.push(FileEntry {
                display_name: path.display().to_string(),
                path,
                old_path: None,
                status: FileStatus::Modified,
                insertions: diff.insertions,
                deletions: diff.deletions,
            });
            old_contents.push(old_content);
            new_contents.push(new_content);
        }

        Self {
            files,
            selected_index: 0,
            navigators: (0..old_contents.len()).map(|_| None).collect(),
            repo_root: None,
            git_mode: None,
            old_contents,
            new_contents,
        }
    }

    /// Get the navigator for the currently selected file
    pub fn current_navigator(&mut self) -> &mut DiffNavigator {
        if self.navigators[self.selected_index].is_none() {
            let engine = DiffEngine::new().with_word_level(true);
            let diff = engine.diff_strings(
                &self.old_contents[self.selected_index],
                &self.new_contents[self.selected_index],
            );
            let navigator = DiffNavigator::new(
                diff,
                self.old_contents[self.selected_index].clone(),
                self.new_contents[self.selected_index].clone(),
            );
            self.navigators[self.selected_index] = Some(navigator);
        }
        self.navigators[self.selected_index].as_mut().unwrap()
    }

    /// Get the current file entry
    pub fn current_file(&self) -> Option<&FileEntry> {
        self.files.get(self.selected_index)
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
        let engine = DiffEngine::new().with_word_level(true);

        for change in changes {
            let old_path = change
                .old_path
                .clone()
                .unwrap_or_else(|| change.path.clone());
            let (old_content, new_content) =
                match mode {
                    GitDiffMode::Uncommitted => {
                        let old_content = match change.status {
                            FileStatus::Added | FileStatus::Untracked => String::new(),
                            _ => crate::git::get_head_content(&repo_root, &old_path)
                                .unwrap_or_default(),
                        };
                        let new_content = match change.status {
                            FileStatus::Deleted => String::new(),
                            _ => {
                                let full_path = repo_root.join(&change.path);
                                std::fs::read_to_string(&full_path).unwrap_or_default()
                            }
                        };
                        (old_content, new_content)
                    }
                    GitDiffMode::Staged => {
                        let old_content = match change.status {
                            FileStatus::Added | FileStatus::Untracked => String::new(),
                            _ => crate::git::get_head_content(&repo_root, &old_path)
                                .unwrap_or_default(),
                        };
                        let new_content = match change.status {
                            FileStatus::Deleted => String::new(),
                            _ => crate::git::get_staged_content(&repo_root, &change.path)
                                .unwrap_or_default(),
                        };
                        (old_content, new_content)
                    }
                    GitDiffMode::Range { ref from, ref to } => {
                        let old_content = match change.status {
                            FileStatus::Added | FileStatus::Untracked => String::new(),
                            _ => crate::git::get_file_at_commit(&repo_root, from, &old_path)
                                .unwrap_or_default(),
                        };
                        let new_content = match change.status {
                            FileStatus::Deleted => String::new(),
                            _ => crate::git::get_file_at_commit(&repo_root, to, &change.path)
                                .unwrap_or_default(),
                        };
                        (old_content, new_content)
                    }
                    GitDiffMode::IndexRange { ref from, to_index } => {
                        if to_index {
                            let old_content = match change.status {
                                FileStatus::Added | FileStatus::Untracked => String::new(),
                                _ => crate::git::get_file_at_commit(&repo_root, from, &old_path)
                                    .unwrap_or_default(),
                            };
                            let new_content = match change.status {
                                FileStatus::Deleted => String::new(),
                                _ => crate::git::get_staged_content(&repo_root, &change.path)
                                    .unwrap_or_default(),
                            };
                            (old_content, new_content)
                        } else {
                            let old_content = match change.status {
                                FileStatus::Added | FileStatus::Untracked => String::new(),
                                _ => crate::git::get_staged_content(&repo_root, &old_path)
                                    .unwrap_or_default(),
                            };
                            let new_content = match change.status {
                                FileStatus::Deleted => String::new(),
                                _ => crate::git::get_file_at_commit(&repo_root, from, &change.path)
                                    .unwrap_or_default(),
                            };
                            (old_content, new_content)
                        }
                    }
                };

            let diff = engine.diff_strings(&old_content, &new_content);

            files.push(FileEntry {
                display_name: change.path.display().to_string(),
                path: change.path,
                old_path: change.old_path,
                status: change.status,
                insertions: diff.insertions,
                deletions: diff.deletions,
            });

            old_contents.push(old_content);
            new_contents.push(new_content);
        }

        // Update state
        let navigators: Vec<Option<DiffNavigator>> = (0..files.len()).map(|_| None).collect();
        self.files = files;
        self.old_contents = old_contents;
        self.new_contents = new_contents;
        self.navigators = navigators;

        // Clamp selected index to valid range
        if self.selected_index >= self.files.len() {
            self.selected_index = self.files.len().saturating_sub(1);
        }

        true
    }

    /// Refresh the current file from disk (re-read and re-diff)
    pub fn refresh_current_file(&mut self) {
        let idx = self.selected_index;
        let file = &self.files[idx];
        let old_path = file.old_path.clone().unwrap_or_else(|| file.path.clone());

        // Get fresh content based on mode
        let (old_content, new_content) = match (&self.repo_root, &self.git_mode) {
            (Some(repo_root), Some(GitDiffMode::Uncommitted)) => {
                let old_content = match file.status {
                    FileStatus::Added | FileStatus::Untracked => String::new(),
                    _ => crate::git::get_head_content(repo_root, &old_path).unwrap_or_default(),
                };
                let new_content = match file.status {
                    FileStatus::Deleted => String::new(),
                    _ => {
                        let full_path = repo_root.join(&file.path);
                        std::fs::read_to_string(&full_path).unwrap_or_default()
                    }
                };
                (old_content, new_content)
            }
            (Some(repo_root), Some(GitDiffMode::Staged)) => {
                let old_content = match file.status {
                    FileStatus::Added | FileStatus::Untracked => String::new(),
                    _ => crate::git::get_head_content(repo_root, &old_path).unwrap_or_default(),
                };
                let new_content = match file.status {
                    FileStatus::Deleted => String::new(),
                    _ => crate::git::get_staged_content(repo_root, &file.path).unwrap_or_default(),
                };
                (old_content, new_content)
            }
            (Some(repo_root), Some(GitDiffMode::Range { from, to })) => {
                let old_content = match file.status {
                    FileStatus::Added | FileStatus::Untracked => String::new(),
                    _ => crate::git::get_file_at_commit(repo_root, from, &old_path)
                        .unwrap_or_default(),
                };
                let new_content = match file.status {
                    FileStatus::Deleted => String::new(),
                    _ => crate::git::get_file_at_commit(repo_root, to, &file.path)
                        .unwrap_or_default(),
                };
                (old_content, new_content)
            }
            (Some(repo_root), Some(GitDiffMode::IndexRange { from, to_index })) => {
                if *to_index {
                    let old_content = match file.status {
                        FileStatus::Added | FileStatus::Untracked => String::new(),
                        _ => crate::git::get_file_at_commit(repo_root, from, &old_path)
                            .unwrap_or_default(),
                    };
                    let new_content = match file.status {
                        FileStatus::Deleted => String::new(),
                        _ => crate::git::get_staged_content(repo_root, &file.path)
                            .unwrap_or_default(),
                    };
                    (old_content, new_content)
                } else {
                    let old_content = match file.status {
                        FileStatus::Added | FileStatus::Untracked => String::new(),
                        _ => {
                            crate::git::get_staged_content(repo_root, &old_path).unwrap_or_default()
                        }
                    };
                    let new_content = match file.status {
                        FileStatus::Deleted => String::new(),
                        _ => crate::git::get_file_at_commit(repo_root, from, &file.path)
                            .unwrap_or_default(),
                    };
                    (old_content, new_content)
                }
            }
            _ => {
                let new_content = std::fs::read_to_string(&file.path).unwrap_or_default();
                (self.old_contents[idx].clone(), new_content)
            }
        };

        // Update stored content
        self.old_contents[idx] = old_content;
        self.new_contents[idx] = new_content;

        // Recompute diff stats
        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(&self.old_contents[idx], &self.new_contents[idx]);

        // Update file entry stats
        self.files[idx].insertions = diff.insertions;
        self.files[idx].deletions = diff.deletions;

        // Clear the navigator so it gets rebuilt on next access
        self.navigators[idx] = None;
    }
}

fn collect_files(
    dir: &Path,
    base: &Path,
    files: &mut std::collections::HashSet<PathBuf>,
) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        // Skip hidden files and common ignore patterns
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
        }

        if path.is_dir() {
            collect_files(&path, base, files)?;
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(base) {
                files.insert(rel.to_path_buf());
            }
        }
    }
    Ok(())
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
