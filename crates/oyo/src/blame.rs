use oyo_core::multi::BlameSource;
use std::path::Path;
use std::process::Command;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct BlameInfo {
    pub author: String,
    pub date: String,
    pub commit: String,
    pub uncommitted: bool,
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
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo_root)
        .arg("blame")
        .arg("-L")
        .arg(format!("{line},{line}"))
        .arg("--porcelain");

    match source {
        BlameSource::Worktree => {}
        BlameSource::Index => {
            cmd.arg("--cached");
        }
        BlameSource::Commit(commit) => {
            cmd.arg(commit);
        }
    }

    cmd.arg("--").arg(file_path);

    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let first_line = lines.next()?;
    let commit = first_line.split_whitespace().next()?.to_string();
    let mut author = String::new();
    let mut author_time: Option<i64> = None;

    for line in lines {
        if let Some(rest) = line.strip_prefix("author ") {
            author = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("author-time ") {
            author_time = rest.trim().parse::<i64>().ok();
        }
    }

    let uncommitted = commit.chars().all(|c| c == '0') || author == "Not Committed Yet";
    let date = author_time
        .and_then(format_short_date)
        .unwrap_or_default();

    Some(BlameInfo {
        author,
        date,
        commit,
        uncommitted,
    })
}

pub fn format_blame_text(info: &BlameInfo, git_user: Option<&str>) -> String {
    if info.uncommitted {
        return "Uncommitted".to_string();
    }
    let mut author = info.author.clone();
    if let Some(user) = git_user {
        if !user.is_empty() && author == user {
            author = "You".to_string();
        }
    }
    let short = if info.commit.len() > 8 {
        info.commit[..8].to_string()
    } else {
        info.commit.clone()
    };
    let date = if info.date.is_empty() {
        "Unknown".to_string()
    } else {
        info.date.clone()
    };
    format!("{author}, {date} • {short}")
}

fn format_short_date(epoch: i64) -> Option<String> {
    let dt = OffsetDateTime::from_unix_timestamp(epoch).ok()?;
    let month = match dt.month() {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    };
    Some(format!("{month} {}", dt.day()))
}
