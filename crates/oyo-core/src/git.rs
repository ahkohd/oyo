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

/// Summary stats for a commit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitStats {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

/// Commit metadata for log views
#[derive(Debug, Clone)]
pub struct CommitEntry {
    pub id: String,
    pub short_id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_time: Option<i64>,
    pub summary: String,
    pub stats: Option<CommitStats>,
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

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
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

/// Get list of staged changed files (index vs HEAD)
pub fn get_staged_changes(repo_path: &Path) -> Result<Vec<ChangedFile>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("diff")
        .arg("--cached")
        .arg("--name-status")
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

/// Get changes between a commit and the staged index (commit vs index)
pub fn get_changes_between_index(
    repo_path: &Path,
    from: &str,
    reverse: bool,
) -> Result<Vec<ChangedFile>, GitError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo_path)
        .arg("diff")
        .arg("--cached")
        .arg("--name-status");
    if reverse {
        cmd.arg("-R");
    }
    cmd.arg(from);

    let output = cmd.output()?;

    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let mut changes = Vec::new();
    parse_name_status(&String::from_utf8_lossy(&output.stdout), &mut changes);
    Ok(changes)
}

/// Get recent commits with short stats
pub fn get_recent_commits(repo_path: &Path, limit: usize) -> Result<Vec<CommitEntry>, GitError> {
    let format = "%H%x1f%h%x1f%P%x1f%an%x1f%at%x1f%s";
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("log")
        .arg("-n")
        .arg(limit.to_string())
        .arg(format!("--pretty=format:{format}"))
        .arg("--shortstat")
        .output()?;

    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let mut commits = Vec::new();
    let mut last_idx: Option<usize> = None;

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.contains('\u{1f}') {
            let parts: Vec<&str> = line.split('\u{1f}').collect();
            if parts.len() < 6 {
                continue;
            }
            let parents = if parts[2].trim().is_empty() {
                Vec::new()
            } else {
                parts[2].split_whitespace().map(|s| s.to_string()).collect()
            };
            let author_time = parts[4].trim().parse::<i64>().ok();
            commits.push(CommitEntry {
                id: parts[0].to_string(),
                short_id: parts[1].to_string(),
                parents,
                author: parts[3].to_string(),
                author_time,
                summary: parts[5].to_string(),
                stats: None,
            });
            last_idx = Some(commits.len() - 1);
            continue;
        }

        if let Some(stats) = parse_shortstat(line) {
            if let Some(idx) = last_idx {
                commits[idx].stats = Some(stats);
            }
        }
    }

    Ok(commits)
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

pub fn get_file_at_commit_bytes(
    repo_path: &Path,
    commit: &str,
    file: &Path,
) -> Result<Vec<u8>, GitError> {
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

    Ok(output.stdout)
}

pub fn get_file_at_commit_size(repo_path: &Path, commit: &str, file: &Path) -> Option<u64> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("cat-file")
        .arg("-s")
        .arg(format!("{}:{}", commit, file.display()))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
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

pub fn get_staged_content_bytes(repo_path: &Path, file: &Path) -> Result<Vec<u8>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("show")
        .arg(format!(":{}", file.display()))
        .output()?;

    if !output.status.success() {
        return get_file_at_commit_bytes(repo_path, "HEAD", file);
    }

    Ok(output.stdout)
}

pub fn get_staged_content_size(repo_path: &Path, file: &Path) -> Option<u64> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("cat-file")
        .arg("-s")
        .arg(format!(":{}", file.display()))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

pub fn get_head_content_bytes(repo_path: &Path, file: &Path) -> Result<Vec<u8>, GitError> {
    get_file_at_commit_bytes(repo_path, "HEAD", file)
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

fn parse_shortstat(line: &str) -> Option<CommitStats> {
    if !line.contains("file changed") && !line.contains("files changed") {
        return None;
    }

    let mut files_changed = 0usize;
    let mut insertions = 0usize;
    let mut deletions = 0usize;

    for part in line.split(',') {
        let part = part.trim();
        let count = part
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        if part.contains("file changed") || part.contains("files changed") {
            files_changed = count;
        } else if part.contains("insertion") {
            insertions = count;
        } else if part.contains("deletion") {
            deletions = count;
        }
    }

    Some(CommitStats {
        files_changed,
        insertions,
        deletions,
    })
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
