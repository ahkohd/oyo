//! Git integration for detecting changed files

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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

/// Get the path to the git index file.
pub fn get_index_path(path: &Path) -> Result<PathBuf, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--git-path")
        .arg("index")
        .output()?;

    if !output.status.success() {
        return Err(GitError::NotARepo);
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let index_path = PathBuf::from(raw);
    Ok(if index_path.is_absolute() {
        index_path
    } else {
        path.join(index_path)
    })
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

fn is_oyo_review_path(path: &Path) -> bool {
    path.starts_with(".oyo/reviews")
}

fn drop_oyo_review_changes(changes: &mut Vec<ChangedFile>) {
    changes.retain(|change| {
        !is_oyo_review_path(&change.path)
            && change
                .old_path
                .as_ref()
                .is_none_or(|old_path| !is_oyo_review_path(old_path))
    });
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

    drop_oyo_review_changes(&mut changes);

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
    drop_oyo_review_changes(&mut changes);
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
    drop_oyo_review_changes(&mut changes);
    Ok(changes)
}

pub fn get_diff_numstat(
    repo_path: &Path,
    args: &[String],
) -> Result<HashMap<PathBuf, (usize, usize)>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("diff")
        .args(args)
        .arg("--numstat")
        .arg("-z")
        .output()?;
    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    parse_numstat(&output.stdout)
}

fn parse_numstat(output: &[u8]) -> Result<HashMap<PathBuf, (usize, usize)>, GitError> {
    let mut records = output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());
    let mut stats = HashMap::new();
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let insertions = fields.next().unwrap_or_default();
        let deletions = fields.next().unwrap_or_default();
        let path = fields.next().unwrap_or_default();
        let count = |value: &[u8]| {
            if value == b"-" {
                Ok(0)
            } else {
                std::str::from_utf8(value)
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                    .ok_or_else(|| GitError::CommandFailed("Invalid git numstat count".to_string()))
            }
        };
        let value = (count(insertions)?, count(deletions)?);
        if path.is_empty() {
            let old = records
                .next()
                .ok_or_else(|| GitError::CommandFailed("Invalid git numstat rename".to_string()))?;
            let new = records
                .next()
                .ok_or_else(|| GitError::CommandFailed("Invalid git numstat rename".to_string()))?;
            stats.insert(PathBuf::from(String::from_utf8_lossy(old).as_ref()), value);
            stats.insert(PathBuf::from(String::from_utf8_lossy(new).as_ref()), value);
        } else {
            stats.insert(PathBuf::from(String::from_utf8_lossy(path).as_ref()), value);
        }
    }
    Ok(stats)
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

pub fn get_objects_batch(
    repo_path: &Path,
    specs: &[String],
    max_bytes: u64,
) -> Result<Vec<Option<Vec<u8>>>, GitError> {
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("cat-file")
        .arg("--batch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| GitError::CommandFailed("git cat-file stdin was unavailable".to_string()))?;
    let requests = specs.to_vec();
    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        for spec in requests {
            writeln!(stdin, "{spec}")?;
        }
        Ok(())
    });
    let stdout = child.stdout.take().ok_or_else(|| {
        GitError::CommandFailed("git cat-file stdout was unavailable".to_string())
    })?;
    let mut reader = BufReader::new(stdout);
    let mut objects = Vec::with_capacity(specs.len());
    for spec in specs {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Err(GitError::CommandFailed(format!(
                "git cat-file returned no header for {spec}"
            )));
        }
        if header.trim_end().ends_with(" missing") {
            objects.push(Some(Vec::new()));
            continue;
        }
        let size = header
            .split_whitespace()
            .last()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| GitError::CommandFailed(format!("Invalid cat-file header: {header}")))?;
        if size > max_bytes {
            std::io::copy(&mut reader.by_ref().take(size), &mut std::io::sink())?;
            objects.push(None);
        } else {
            let mut bytes = vec![0; size as usize];
            reader.read_exact(&mut bytes)?;
            objects.push(Some(bytes));
        }
        let mut newline = [0u8; 1];
        reader.read_exact(&mut newline)?;
        if newline[0] != b'\n' {
            return Err(GitError::CommandFailed(
                "Invalid git cat-file object terminator".to_string(),
            ));
        }
    }
    drop(reader);
    writer
        .join()
        .map_err(|_| GitError::CommandFailed("git cat-file writer failed".to_string()))??;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(objects)
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
    fn batch_objects_handle_content_missing_and_size_limits() {
        let root = std::env::temp_dir().join(format!(
            "oyo-git-batch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(&root)
            .status()
            .unwrap();
        std::fs::write(root.join("small.txt"), b"one").unwrap();
        std::fs::write(root.join("large.txt"), b"12345").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "fixture",
            ])
            .status()
            .unwrap();

        let objects = get_objects_batch(
            &root,
            &[
                "HEAD:small.txt".to_string(),
                "HEAD:missing.txt".to_string(),
                "HEAD:large.txt".to_string(),
            ],
            4,
        )
        .unwrap();
        assert_eq!(objects[0].as_deref(), Some(b"one".as_slice()));
        assert_eq!(objects[1].as_deref(), Some(b"".as_slice()));
        assert_eq!(objects[2], None);

        let many = vec!["HEAD:small.txt".to_string(); 5_000];
        let objects = get_objects_batch(&root, &many, 4).unwrap();
        assert_eq!(objects.len(), many.len());
        assert!(objects
            .iter()
            .all(|object| object.as_deref() == Some(b"one".as_slice())));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_numstat_paths_renames_and_binary_files() {
        let stats =
            parse_numstat(b"2\t1\tfile.rs\0-\t-\timage.png\x003\t4\t\0old.rs\0new.rs\0").unwrap();
        assert_eq!(stats[Path::new("file.rs")], (2, 1));
        assert_eq!(stats[Path::new("image.png")], (0, 0));
        assert_eq!(stats[Path::new("old.rs")], (3, 4));
        assert_eq!(stats[Path::new("new.rs")], (3, 4));
    }

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

    #[test]
    fn drops_oyo_review_db_changes() {
        let mut changes = vec![
            ChangedFile {
                path: PathBuf::from(".oyo/reviews/workspace/review.db"),
                status: FileStatus::Untracked,
                old_path: None,
            },
            ChangedFile {
                path: PathBuf::from("src/main.rs"),
                status: FileStatus::Modified,
                old_path: None,
            },
        ];

        drop_oyo_review_changes(&mut changes);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, PathBuf::from("src/main.rs"));
    }
}
