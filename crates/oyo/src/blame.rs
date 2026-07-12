use oyo_core::multi::BlameSource;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct BlameInfo {
    pub author: String,
    pub commit: String,
    pub uncommitted: bool,
    pub author_time: Option<i64>,
    pub summary: String,
}

pub fn load_git_user_name(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("config")
        .arg("user.name")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

pub fn blame_line(
    repo_root: &Path,
    file_path: &Path,
    line: usize,
    source: &BlameSource,
) -> Option<BlameInfo> {
    let entries = blame_range(repo_root, file_path, line, line, source)?;
    entries
        .into_iter()
        .find(|(entry_line, _)| *entry_line == line)
        .map(|(_, info)| info)
}

pub fn blame_range(
    repo_root: &Path,
    file_path: &Path,
    start: usize,
    end: usize,
    source: &BlameSource,
) -> Option<Vec<(usize, BlameInfo)>> {
    if let BlameSource::JjRev(revision) = source {
        return blame_range_jj(repo_root, file_path, start, end, revision);
    }

    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo_root)
        .arg("blame")
        .arg("-L")
        .arg(format!("{start},{end}"))
        .arg("--line-porcelain");

    match source {
        BlameSource::Worktree => {}
        BlameSource::Index => {
            cmd.arg("--cached");
        }
        BlameSource::Commit(commit) => {
            cmd.arg(commit);
        }
        BlameSource::JjRev(_) => unreachable!(),
    }

    cmd.arg("--").arg(file_path);

    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();
    let mut current_commit: Option<String> = None;
    let mut current_author = String::new();
    let mut current_author_time: Option<i64> = None;
    let mut current_summary = String::new();
    let mut current_line = 0usize;
    let mut remaining = 0usize;

    for line in stdout.lines() {
        if line.starts_with('\t') {
            if remaining == 0 {
                continue;
            }
            let commit = current_commit.clone().unwrap_or_default();
            let uncommitted =
                commit.chars().all(|c| c == '0') || current_author == "Not Committed Yet";
            let info = BlameInfo {
                author: current_author.clone(),
                commit,
                uncommitted,
                author_time: current_author_time,
                summary: current_summary.clone(),
            };
            entries.push((current_line, info));
            current_line = current_line.saturating_add(1);
            remaining = remaining.saturating_sub(1);
            if remaining == 0 {
                current_commit = None;
                current_author.clear();
                current_author_time = None;
            }
            continue;
        }

        if current_commit.is_none() {
            let mut parts = line.split_whitespace();
            let commit = parts.next();
            let _orig_line = parts.next();
            let final_line = parts.next();
            let group_size = parts.next();
            let (Some(commit), Some(final_line), Some(group_size)) =
                (commit, final_line, group_size)
            else {
                continue;
            };
            let Ok(line_num) = final_line.parse::<usize>() else {
                continue;
            };
            let Ok(group_size) = group_size.parse::<usize>() else {
                continue;
            };
            current_commit = Some(commit.to_string());
            current_author.clear();
            current_author_time = None;
            current_summary.clear();
            current_line = line_num;
            remaining = group_size.max(1);
            continue;
        }

        if let Some(rest) = line.strip_prefix("author ") {
            current_author = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("author-time ") {
            current_author_time = rest.trim().parse::<i64>().ok();
        } else if let Some(rest) = line.strip_prefix("summary ") {
            current_summary = rest.to_string();
        }
    }

    Some(entries)
}

const JJ_ANNOTATE_TEMPLATE: &str = "commit.change_id().shortest(8) ++ \"\\x1f\" ++ commit.author().name() ++ \"\\x1f\" ++ commit.author().timestamp().format(\"%s\") ++ \"\\x1f\" ++ if(commit.current_working_copy(), \"1\", \"0\") ++ \"\\x1f\" ++ commit.description().first_line() ++ \"\\n\"";

fn blame_range_jj(
    repo_root: &Path,
    file_path: &Path,
    start: usize,
    end: usize,
    revision: &str,
) -> Option<Vec<(usize, BlameInfo)>> {
    let output = Command::new("jj")
        .current_dir(repo_root)
        .args([
            "file",
            "annotate",
            "-r",
            revision,
            "-T",
            JJ_ANNOTATE_TEMPLATE,
            "--",
        ])
        .arg(file_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                let line_number = index + 1;
                (line_number >= start && line_number <= end)
                    .then(|| parse_jj_annotate_line(line).map(|info| (line_number, info)))?
            })
            .collect(),
    )
}

fn parse_jj_annotate_line(line: &str) -> Option<BlameInfo> {
    let mut fields = line.splitn(5, '\x1f');
    let commit = fields.next()?.to_string();
    let author = fields.next()?.to_string();
    let author_time = fields.next()?.parse::<i64>().ok();
    let uncommitted = fields.next()? == "1";
    let summary = fields.next()?.to_string();
    Some(BlameInfo {
        author,
        commit,
        uncommitted,
        author_time,
        summary,
    })
}

pub fn format_blame_github_text(
    info: &BlameInfo,
    git_user: Option<&str>,
    time_text: &str,
) -> String {
    if info.uncommitted {
        return "Uncommitted".to_string();
    }
    let mut author = info.author.clone();
    if let Some(user) = git_user {
        if !user.is_empty() && author == user {
            author = "You".to_string();
        }
    }
    let relative = time_text;
    let short = short_commit(&info.commit);
    if info.summary.is_empty() {
        format!("{author}, {relative} {short}")
    } else {
        format!("{author}, {relative} {short} {}", info.summary)
    }
}

pub fn format_blame_hint_text(
    info: &BlameInfo,
    git_user: Option<&str>,
    time_text: &str,
    max_summary_len: usize,
) -> String {
    if info.uncommitted {
        return "Uncommitted".to_string();
    }
    let mut author = info.author.clone();
    if let Some(user) = git_user {
        if !user.is_empty() && author == user {
            author = "You".to_string();
        }
    }
    let relative = time_text;
    let short = short_commit(&info.commit);
    if info.summary.is_empty() {
        return format!("{author}, {relative} {short}");
    }
    let summary = truncate_with_ellipsis(&info.summary, max_summary_len);
    format!("{author}, {relative} {short} {summary}")
}

fn truncate_with_ellipsis(text: &str, max_len: usize) -> String {
    if max_len == 0 || text.len() <= max_len {
        return text.to_string();
    }
    let suffix_len = max_len.saturating_sub(3);
    format!("{}…", &text[..suffix_len])
}

fn short_commit(commit: &str) -> String {
    if commit.len() > 8 {
        commit[..8].to_string()
    } else {
        commit.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jj_annotate_line() {
        let info =
            parse_jj_annotate_line("qpvuntsm\x1fAda\x1f1710000000\x1f1\x1fworking copy").unwrap();
        assert_eq!(info.commit, "qpvuntsm");
        assert_eq!(info.author, "Ada");
        assert_eq!(info.author_time, Some(1_710_000_000));
        assert!(info.uncommitted);
        assert_eq!(info.summary, "working copy");
    }
}
