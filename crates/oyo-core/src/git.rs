//! Git integration for detecting changed files

use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GitError {
    #[error("Not a git repository")]
    NotARepo,
    #[error("Git command failed: {0}")]
    CommandFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Status of a file in git
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

/// A changed file in git
#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub status: FileStatus,
    /// For renamed files, the original path
    pub old_path: Option<PathBuf>,
}

/// Check if a directory is a git repository
pub fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--git-dir")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the current git branch name
pub fn get_current_branch(path: &Path) -> Result<String, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .output()?;

    if !output.status.success() {
        return Err(GitError::NotARepo);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the root of the git repository
pub fn get_repo_root(path: &Path) -> Result<PathBuf, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()?;

    if !output.status.success() {
        return Err(GitError::NotARepo);
    }

    let root = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_string();
    Ok(PathBuf::from(root))
}

/// Get list of uncommitted changed files (staged and unstaged)
pub fn get_uncommitted_changes(repo_path: &Path) -> Result<Vec<ChangedFile>, GitError> {
    let mut changes = Vec::new();

    // Get staged changes
    let staged = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("diff")
        .arg("--cached")
        .arg("--name-status")
        .output()?;

    if staged.status.success() {
        parse_name_status(&String::from_utf8_lossy(&staged.stdout), &mut changes);
    }

    // Get unstaged changes
    let unstaged = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("diff")
        .arg("--name-status")
        .output()?;

    if unstaged.status.success() {
        parse_name_status(&String::from_utf8_lossy(&unstaged.stdout), &mut changes);
    }

    // Get untracked files
    let untracked = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("ls-files")
        .arg("--others")
        .arg("--exclude-standard")
        .output()?;

    if untracked.status.success() {
        for line in String::from_utf8_lossy(&untracked.stdout).lines() {
            let line = line.trim();
            if !line.is_empty() {
                changes.push(ChangedFile {
                    path: PathBuf::from(line),
                    status: FileStatus::Untracked,
                    old_path: None,
                });
            }
        }
    }

    // Deduplicate by path
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes.dedup_by(|a, b| a.path == b.path);

    Ok(changes)
}

/// Get changes between two commits or refs
pub fn get_changes_between(
    repo_path: &Path,
    from: &str,
    to: &str,
) -> Result<Vec<ChangedFile>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("diff")
        .arg("--name-status")
        .arg(format!("{}..{}", from, to))
        .output()?;

    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let mut changes = Vec::new();
    parse_name_status(&String::from_utf8_lossy(&output.stdout), &mut changes);
    Ok(changes)
}

/// Get the content of a file at a specific commit
pub fn get_file_at_commit(repo_path: &Path, commit: &str, file: &Path) -> Result<String, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("show")
        .arg(format!("{}:{}", commit, file.display()))
        .output()?;

    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get the staged content of a file
pub fn get_staged_content(repo_path: &Path, file: &Path) -> Result<String, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("show")
        .arg(format!(":{}", file.display()))
        .output()?;

    if !output.status.success() {
        // File might not be staged, try HEAD
        return get_file_at_commit(repo_path, "HEAD", file);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get the HEAD content of a file
pub fn get_head_content(repo_path: &Path, file: &Path) -> Result<String, GitError> {
    get_file_at_commit(repo_path, "HEAD", file)
}

fn parse_name_status(output: &str, changes: &mut Vec<ChangedFile>) {
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split('\t').collect();
        if parts.is_empty() {
            continue;
        }

        let status_char = parts[0].chars().next().unwrap_or(' ');
        let status = match status_char {
            'M' => FileStatus::Modified,
            'A' => FileStatus::Added,
            'D' => FileStatus::Deleted,
            'R' => FileStatus::Renamed,
            _ => continue,
        };

        if parts.len() >= 2 {
            let path = PathBuf::from(parts.last().unwrap());
            let old_path = if status == FileStatus::Renamed && parts.len() >= 3 {
                Some(PathBuf::from(parts[1]))
            } else {
                None
            };

            changes.push(ChangedFile {
                path,
                status,
                old_path,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_name_status() {
        let output = "M\tsrc/main.rs\nA\tsrc/new.rs\nD\tsrc/old.rs\n";
        let mut changes = Vec::new();
        parse_name_status(output, &mut changes);

        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].status, FileStatus::Modified);
        assert_eq!(changes[1].status, FileStatus::Added);
        assert_eq!(changes[2].status, FileStatus::Deleted);
    }
}
