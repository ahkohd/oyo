mod trees;
use crate::{
    provider_adapter, review_provider_for_host, BuiltDiff, ReviewProviderAdapter,
    ReviewProviderKind,
};
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use oyo_core::git::FileStatus;
use oyo_core::multi::{ContentSource, FileEntry, PendingFileContent};
use oyo_core::MultiFileDiff;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub(crate) struct RemoteTarget {
    pub provider: ReviewProviderKind,
    pub host: String,
    pub repo: String,
    pub kind: RemoteTargetKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteTargetKind {
    Review(u64),
    Commit(String),
    Compare {
        before: String,
        after: String,
        merge_base: bool,
    },
}

pub(crate) struct RemoteSnapshot {
    target: RemoteTarget,
    before_repo: String,
    after_repo: String,
    before: String,
    after: String,
    files: Vec<FileEntry>,
}

fn validate_ref(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains("..")
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.~/".contains(&b))
    {
        bail!("Unsupported remote revision syntax");
    }
    Ok(())
}

fn github_revision_snapshot(
    adapter: &dyn ReviewProviderAdapter,
    target: &RemoteTarget,
) -> Result<RemoteSnapshot> {
    let endpoint = |revision: &str| format!("repos/{}/commits/{}", target.repo, encode(revision));
    let (before, after, files) = match &target.kind {
        RemoteTargetKind::Commit(reference) => {
            let commit = adapter.remote_json(&target.host, &endpoint(reference))?;
            let after = revision(&commit, "/sha")?;
            let parents = commit["parents"]
                .as_array()
                .ok_or_else(|| anyhow!("Missing commit parents"))?;
            let before = if let Some(parent) = parents.first() {
                revision(parent, "/sha")?
            } else {
                String::new()
            };
            let mut files = Vec::new();
            for page in 1..=31 {
                let response = adapter.remote_json(
                    &target.host,
                    &format!("{}?per_page=100&page={page}", endpoint(&after)),
                )?;
                let entries = response["files"]
                    .as_array()
                    .ok_or_else(|| anyhow!("Missing commit files"))?;
                if entries.is_empty() {
                    break;
                }
                files.extend(entries.iter().cloned());
                if files.len() >= 3000 {
                    bail!("GitHub commit file limit reached; refusing a potentially partial diff");
                }
            }
            (before, after, files)
        }
        RemoteTargetKind::Compare { before, after, .. } => {
            let base = revision(
                &adapter.remote_json(&target.host, &endpoint(before))?,
                "/sha",
            )?;
            let after = revision(
                &adapter.remote_json(&target.host, &endpoint(after))?,
                "/sha",
            )?;
            let comparison = adapter.remote_json(
                &target.host,
                &format!("repos/{}/compare/{base}...{after}", target.repo),
            )?;
            let before = revision(&comparison, "/merge_base_commit/sha")?;
            let files = comparison["files"]
                .as_array()
                .ok_or_else(|| anyhow!("Missing comparison files"))?
                .clone();
            if files.len() >= 300 {
                bail!("GitHub comparison file limit reached; refusing a potentially partial diff");
            }
            (before, after, files)
        }
        RemoteTargetKind::Review(_) => unreachable!(),
    };
    Ok(RemoteSnapshot {
        target: target.clone(),
        before_repo: target.repo.clone(),
        after_repo: target.repo.clone(),
        before,
        after,
        files: entries(files, false)?,
    })
}

fn field(value: &Value, pointer: &str) -> Result<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("Provider response missing {pointer}"))
}

fn revision(value: &Value, pointer: &str) -> Result<String> {
    let sha = field(value, pointer)?;
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Provider returned an invalid immutable revision");
    }
    Ok(sha)
}

pub(crate) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

impl RemoteTarget {
    pub(crate) fn canonical_url(&self) -> String {
        let marker = match self.provider {
            ReviewProviderKind::GitHub => "pull",
            ReviewProviderKind::GitLab => "-/merge_requests",
            ReviewProviderKind::Forgejo => "pulls",
        };
        let path = match &self.kind {
            RemoteTargetKind::Review(number) => format!("{marker}/{number}"),
            RemoteTargetKind::Commit(revision) => format!(
                "{}commit/{revision}",
                if matches!(self.provider, ReviewProviderKind::GitLab) {
                    "-/"
                } else {
                    ""
                }
            ),
            RemoteTargetKind::Compare {
                before,
                after,
                merge_base,
            } => format!(
                "{}compare/{before}{}{after}",
                if matches!(self.provider, ReviewProviderKind::GitLab) {
                    "-/"
                } else {
                    ""
                },
                if *merge_base { "..." } else { ".." }
            ),
        };
        format!("https://{}/{}/{path}", self.host, self.repo)
    }

    fn review_number(&self) -> Result<u64> {
        match self.kind {
            RemoteTargetKind::Review(number) => Ok(number),
            _ => bail!("Commit and comparison URLs are not supported by this provider adapter"),
        }
    }

    pub(crate) fn parse(url: &str) -> Result<Self> {
        let rest = url
            .strip_prefix("https://")
            .ok_or_else(|| anyhow!("Remote URLs must use HTTPS"))?;
        let (host, path) = rest
            .split_once('/')
            .ok_or_else(|| anyhow!("Missing repository"))?;
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        {
            bail!("Invalid remote host");
        }
        let host = host.to_ascii_lowercase();
        let provider = review_provider_for_host(&host)
            .ok_or_else(|| anyhow!("Unsupported or unauthenticated review provider: {host}"))?;
        let path = path
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        let marker = match provider {
            ReviewProviderKind::GitHub => "/pull/",
            ReviewProviderKind::GitLab => "/-/merge_requests/",
            ReviewProviderKind::Forgejo => "/pulls/",
        };
        let (repo, kind) = if let Some((repo, number)) = path.split_once(marker) {
            let number = number
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| anyhow!("Expected a review number without a subpage"))?;
            (repo, RemoteTargetKind::Review(number))
        } else {
            let commit_marker = if matches!(provider, ReviewProviderKind::GitLab) {
                "/-/commit/"
            } else {
                "/commit/"
            };
            let compare_marker = if matches!(provider, ReviewProviderKind::GitLab) {
                "/-/compare/"
            } else {
                "/compare/"
            };
            if let Some((repo, revision)) = path.split_once(commit_marker) {
                validate_ref(revision)?;
                (repo, RemoteTargetKind::Commit(revision.to_owned()))
            } else if let Some((repo, range)) = path.split_once(compare_marker) {
                let (before, after, merge_base) =
                    if let Some((before, after)) = range.split_once("...") {
                        (before, after, true)
                    } else if let Some((before, after)) = range.split_once("..") {
                        if matches!(provider, ReviewProviderKind::GitHub) {
                            bail!("GitHub two-dot comparisons are unsupported");
                        }
                        (before, after, false)
                    } else {
                        bail!("Expected a two-dot or three-dot comparison");
                    };
                validate_ref(before)?;
                validate_ref(after)?;
                (
                    repo,
                    RemoteTargetKind::Compare {
                        before: before.to_owned(),
                        after: after.to_owned(),
                        merge_base,
                    },
                )
            } else {
                bail!("Unsupported remote target URL");
            }
        };
        if matches!(kind, RemoteTargetKind::Compare { .. }) && url.contains('?') {
            bail!("Comparison URL query options are unsupported; use explicit two-dot or three-dot semantics");
        }
        if repo.split('/').count() < 2
            || repo.split('/').any(|part| {
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
        {
            bail!("Invalid repository path");
        }
        Ok(Self {
            provider,
            host,
            repo: repo.to_owned(),
            kind,
        })
    }
}

fn pages(adapter: &dyn ReviewProviderAdapter, host: &str, endpoint: &str) -> Result<Vec<Value>> {
    let mut all = Vec::new();
    for page in 1..=100 {
        let separator = if endpoint.contains('?') { '&' } else { '?' };
        let value = adapter.remote_json(
            host,
            &format!("{endpoint}{separator}per_page=100&limit=100&page={page}"),
        )?;
        let items = value
            .as_array()
            .ok_or_else(|| anyhow!("Expected a provider file list"))?;
        if items.is_empty() {
            return Ok(all);
        }
        all.extend(items.iter().cloned());
    }
    bail!("Remote file enumeration exceeded its page limit; refusing a partial diff")
}

fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        bail!("Provider returned an unsafe repository path");
    }
    Ok(())
}

fn entries(values: Vec<Value>, gitlab: bool) -> Result<Vec<FileEntry>> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| {
            let path = field(&value, if gitlab { "/new_path" } else { "/filename" })?;
            validate_path(&path)?;
            if !seen.insert(path.clone()) {
                bail!("Provider repeated a changed file during pagination");
            }
            let status = if gitlab {
                if value["new_file"].as_bool() == Some(true) {
                    FileStatus::Added
                } else if value["deleted_file"].as_bool() == Some(true) {
                    FileStatus::Deleted
                } else if value["renamed_file"].as_bool() == Some(true) {
                    FileStatus::Renamed
                } else {
                    FileStatus::Modified
                }
            } else {
                match value["status"].as_str() {
                    Some("added") => FileStatus::Added,
                    Some("removed" | "deleted") => FileStatus::Deleted,
                    Some("renamed") => FileStatus::Renamed,
                    Some("modified" | "changed") => FileStatus::Modified,
                    other => bail!("Unsupported remote file status: {other:?}"),
                }
            };
            let old_path = if matches!(status, FileStatus::Renamed) {
                Some(
                    field(
                        &value,
                        if gitlab {
                            "/old_path"
                        } else {
                            "/previous_filename"
                        },
                    )?
                    .into(),
                )
            } else {
                None
            };
            if let Some(old_path) = &old_path {
                let old_path: &std::path::PathBuf = old_path;
                validate_path(&old_path.to_string_lossy())?;
            }
            Ok(FileEntry {
                path: path.clone().into(),
                display_name: path,
                old_path,
                old_source_path: None,
                new_source_path: None,
                status,
                insertions: value["additions"].as_u64().unwrap_or(0) as usize,
                deletions: value["deletions"].as_u64().unwrap_or(0) as usize,
                binary: false,
            })
        })
        .collect()
}

fn verify_count(metadata: &Value, files: &[FileEntry]) -> Result<()> {
    let count = metadata["changed_files"]
        .as_u64()
        .ok_or_else(|| anyhow!("Provider did not report a complete changed-file count"))?;
    if count != files.len() as u64 {
        bail!(
            "Provider file list is incomplete: expected {count}, received {}",
            files.len()
        );
    }
    Ok(())
}

pub(crate) fn github_snapshot(
    adapter: &dyn ReviewProviderAdapter,
    target: &RemoteTarget,
) -> Result<RemoteSnapshot> {
    if !matches!(target.kind, RemoteTargetKind::Review(_)) {
        return github_revision_snapshot(adapter, target);
    }
    let endpoint = format!("repos/{}/pulls/{}", target.repo, target.review_number()?);
    let pr = adapter.remote_json(&target.host, &endpoint)?;
    let head = revision(&pr, "/head/sha")?;
    let base = revision(&pr, "/base/sha")?;
    let comparison = adapter.remote_json(
        &target.host,
        &format!("repos/{}/compare/{base}...{head}", target.repo),
    )?;
    let before = revision(&comparison, "/merge_base_commit/sha")?;
    let files = entries(
        pages(adapter, &target.host, &format!("{endpoint}/files"))?,
        false,
    )?;
    verify_count(&pr, &files)?;
    let end = adapter.remote_json(&target.host, &endpoint)?;
    if end["head"]["sha"] != pr["head"]["sha"] || end["base"]["sha"] != pr["base"]["sha"] {
        bail!("Review changed while loading; retry to load a consistent snapshot");
    }
    Ok(RemoteSnapshot {
        target: RemoteTarget {
            repo: field(&pr, "/base/repo/full_name")?,
            ..target.clone()
        },
        before_repo: field(&pr, "/base/repo/full_name")?,
        after_repo: field(&pr, "/head/repo/full_name")?,
        before,
        after: head,
        files,
    })
}

pub(crate) fn gitlab_snapshot(
    adapter: &dyn ReviewProviderAdapter,
    target: &RemoteTarget,
) -> Result<RemoteSnapshot> {
    let project =
        adapter.remote_json(&target.host, &format!("projects/{}", encode(&target.repo)))?;
    let canonical_target = RemoteTarget {
        repo: field(&project, "/path_with_namespace")?,
        ..target.clone()
    };
    let target = &canonical_target;
    if !matches!(target.kind, RemoteTargetKind::Review(_)) {
        return trees::snapshot(adapter, target);
    }
    let endpoint = format!(
        "projects/{}/merge_requests/{}",
        encode(&target.repo),
        target.review_number()?
    );
    let pr = adapter.remote_json(&target.host, &endpoint)?;
    let before = revision(&pr, "/diff_refs/base_sha")?;
    let after = revision(&pr, "/diff_refs/head_sha")?;
    let files = entries(
        pages(adapter, &target.host, &format!("{endpoint}/diffs"))?,
        true,
    )?;
    let expected = pr["changes_count"]
        .as_str()
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| {
            anyhow!(
                "GitLab did not report an exact file count; refusing a potentially truncated diff"
            )
        })?;
    if files.len() != expected {
        bail!("GitLab returned an incomplete file list");
    }
    let end = adapter.remote_json(&target.host, &endpoint)?;
    if end["diff_refs"] != pr["diff_refs"] {
        bail!("Merge request changed while loading; retry");
    }
    let source = pr["source_project_id"]
        .as_u64()
        .ok_or_else(|| anyhow!("Missing source project"))?;
    let destination = pr["target_project_id"]
        .as_u64()
        .ok_or_else(|| anyhow!("Missing target project"))?;
    Ok(RemoteSnapshot {
        target: target.clone(),
        before_repo: destination.to_string(),
        after_repo: source.to_string(),
        before,
        after,
        files,
    })
}

pub(crate) fn forgejo_snapshot(
    adapter: &dyn ReviewProviderAdapter,
    target: &RemoteTarget,
) -> Result<RemoteSnapshot> {
    if !matches!(target.kind, RemoteTargetKind::Review(_)) {
        return trees::snapshot(adapter, target);
    }
    let endpoint = format!("/repos/{}/pulls/{}", target.repo, target.review_number()?);
    let pr = adapter.remote_json(&target.host, &endpoint)?;
    let before = revision(&pr, "/merge_base")?;
    let after = revision(&pr, "/head/sha")?;
    let files = entries(
        pages(adapter, &target.host, &format!("{endpoint}/files"))?,
        false,
    )?;
    verify_count(&pr, &files)?;
    let end = adapter.remote_json(&target.host, &endpoint)?;
    if end["head"]["sha"] != pr["head"]["sha"] || end["base"]["sha"] != pr["base"]["sha"] {
        bail!("Pull request changed while loading; retry");
    }
    Ok(RemoteSnapshot {
        target: RemoteTarget {
            repo: field(&pr, "/base/repo/full_name")?,
            ..target.clone()
        },
        before_repo: field(&pr, "/base/repo/full_name")?,
        after_repo: field(&pr, "/head/repo/full_name")?,
        before,
        after,
        files,
    })
}

pub(crate) fn build(url: &str) -> Result<BuiltDiff> {
    let target = RemoteTarget::parse(url)?;
    let snapshot = provider_adapter(target.provider)?.remote_snapshot(&target)?;
    let target = snapshot.target.clone();
    let source = |repo: &str, revision: &str, path: &str| ContentSource::RemoteBlob {
        provider: snapshot.target.provider.id().to_owned(),
        host: snapshot.target.host.clone(),
        repo: repo.to_owned(),
        revision: revision.to_owned(),
        path: path.to_owned(),
    };
    let manifest = snapshot.files.iter().map(|file| serde_json::json!({"path": file.path, "oldPath": file.old_path, "status": format!("{:?}", file.status)})).collect::<Vec<_>>();
    let mut entries = Vec::new();
    for file in snapshot.files {
        let old = if snapshot.before.is_empty() || matches!(file.status, FileStatus::Added) {
            ContentSource::Empty
        } else {
            source(
                &snapshot.before_repo,
                &snapshot.before,
                &file
                    .old_path
                    .as_ref()
                    .unwrap_or(&file.path)
                    .to_string_lossy(),
            )
        };
        let new = if matches!(file.status, FileStatus::Deleted) {
            ContentSource::Empty
        } else {
            source(
                &snapshot.after_repo,
                &snapshot.after,
                &file.path.to_string_lossy(),
            )
        };
        entries.push((file, old, new));
    }
    let mut multi_diff = MultiFileDiff::from_pending_files(None, entries, false);
    multi_diff.remote_snapshot = Some(serde_json::json!({
        "version": 1, "target": target.canonical_url(), "provider": target.provider.id(), "host": target.host,
        "beforeRepository": snapshot.before_repo, "afterRepository": snapshot.after_repo,
        "beforeRevision": snapshot.before, "afterRevision": snapshot.after, "files": manifest,
    }));
    Ok(BuiltDiff {
        multi_diff,
        branch: Some(url.to_owned()),
        workspace_root: None,
    })
}

fn git_blob(
    kind: ReviewProviderKind,
    host: &str,
    repo: &str,
    commit: &str,
    path: &str,
) -> Result<Value> {
    let adapter = provider_adapter(kind)?;
    let prefix = if matches!(kind, ReviewProviderKind::Forgejo) {
        "/"
    } else {
        ""
    };
    let mut tree = commit.to_owned();
    let components = path.split('/').collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let mut found = None;
        for page in 1..=1000 {
            let value = adapter.remote_json(
                host,
                &format!("{prefix}repos/{repo}/git/trees/{tree}?per_page=100&page={page}"),
            )?;
            if matches!(kind, ReviewProviderKind::GitHub)
                && value["truncated"].as_bool() == Some(true)
            {
                bail!("Provider truncated the repository tree");
            }
            let entries = value["tree"]
                .as_array()
                .ok_or_else(|| anyhow!("Missing repository tree"))?;
            if let Some(entry) = entries
                .iter()
                .find(|entry| entry["path"].as_str() == Some(component))
            {
                found = Some(entry.clone());
                break;
            }
            if matches!(kind, ReviewProviderKind::GitHub)
                || value["truncated"].as_bool() == Some(false)
                || entries.is_empty()
            {
                break;
            }
        }
        let entry = found.as_ref().ok_or_else(|| anyhow!("File is absent from its immutable repository tree or the tree exceeded the page limit"))?;
        let sha = revision(entry, "/sha")?;
        if index + 1 == components.len() {
            if entry["type"].as_str() != Some("blob") {
                bail!("Remote submodules are unsupported");
            }
            if entry["size"]
                .as_u64()
                .is_some_and(|size| size > 32 * 1024 * 1024)
            {
                bail!("File exceeds the 32 MiB loading limit");
            }
            return adapter.remote_json(host, &format!("{prefix}repos/{repo}/git/blobs/{sha}"));
        }
        if entry["type"].as_str() != Some("tree") {
            bail!("Remote path traverses a non-directory object");
        }
        tree = sha;
    }
    bail!("Empty repository path")
}

fn load_uncached(source: &ContentSource) -> Result<Vec<u8>> {
    let ContentSource::RemoteBlob {
        provider,
        host,
        repo,
        revision,
        path,
    } = source
    else {
        return if matches!(source, ContentSource::Empty) {
            Ok(Vec::new())
        } else {
            Err(anyhow!("Invalid remote content source"))
        };
    };
    let kind = match provider.as_str() {
        "github" => ReviewProviderKind::GitHub,
        "gitlab" => ReviewProviderKind::GitLab,
        "forgejo" => ReviewProviderKind::Forgejo,
        _ => bail!("Unknown remote provider"),
    };
    let endpoint = match kind {
        ReviewProviderKind::GitLab => format!(
            "projects/{}/repository/files/{}?ref={revision}",
            encode(repo),
            encode(path)
        ),
        ReviewProviderKind::GitHub => format!(
            "repos/{repo}/contents/{}?ref={revision}",
            path.split('/').map(encode).collect::<Vec<_>>().join("/")
        ),
        ReviewProviderKind::Forgejo => format!(
            "/repos/{repo}/contents/{}?ref={revision}",
            path.split('/').map(encode).collect::<Vec<_>>().join("/")
        ),
    };
    let value = if matches!(
        kind,
        ReviewProviderKind::GitHub | ReviewProviderKind::Forgejo
    ) {
        git_blob(kind, host, repo, revision, path)?
    } else {
        provider_adapter(kind)?.remote_json(host, &endpoint)?
    };
    let size = value["size"]
        .as_u64()
        .ok_or_else(|| anyhow!("Missing remote file size"))?;
    if size > 32 * 1024 * 1024 {
        bail!("File exceeds the 32 MiB loading limit");
    }
    if value["type"]
        .as_str()
        .is_some_and(|kind| !matches!(kind, "file" | "blob"))
    {
        bail!("Remote file type is unsupported");
    }
    if value["encoding"].as_str() != Some("base64") {
        bail!("Provider did not supply full base64 file contents");
    }
    let content = value["content"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing remote file contents"))?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(
        content
            .bytes()
            .filter(|b| !b.is_ascii_whitespace())
            .collect::<Vec<_>>(),
    )?;
    if decoded.len() as u64 != size {
        bail!("Incomplete remote file contents");
    }
    Ok(decoded)
}

type BlobCache = std::collections::VecDeque<(ContentSource, Vec<u8>)>;
static BLOB_CACHE: std::sync::Mutex<BlobCache> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

fn load(source: &ContentSource) -> Result<Vec<u8>> {
    if let Ok(cache) = BLOB_CACHE.lock() {
        if let Some((_, bytes)) = cache.iter().find(|(key, _)| key == source) {
            return Ok(bytes.clone());
        }
    }
    let bytes = load_uncached(source)?;
    if let Ok(mut cache) = BLOB_CACHE.lock() {
        while cache.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() + bytes.len()
            > 64 * 1024 * 1024
        {
            if cache.pop_front().is_none() {
                break;
            }
        }
        cache.push_back((source.clone(), bytes.clone()));
    }
    Ok(bytes)
}

pub(crate) fn load_pair(request: &PendingFileContent) -> Result<(Vec<u8>, Vec<u8>)> {
    Ok((load(&request.old)?, load(&request.new)?))
}

pub(crate) fn run_command(
    mut command: std::process::Command,
    input: Option<&str>,
) -> Result<String> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    const LIMIT: u64 = 48 * 1024 * 1024;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = input {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes())?;
        }
    }
    drop(child.stdin.take());
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Missing transport output"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Missing transport diagnostics"))?;
    let output_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let error_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(65537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut status = None;
    loop {
        if status.is_none() {
            status = child.try_wait()?;
        }
        if status.is_some() && output_reader.is_finished() && error_reader.is_finished() {
            break;
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            bail!("Remote request timed out after 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let bytes = output_reader
        .join()
        .map_err(|_| anyhow!("Transport reader failed"))??;
    let errors = error_reader
        .join()
        .map_err(|_| anyhow!("Transport diagnostics failed"))??;
    if bytes.len() as u64 > LIMIT {
        bail!("Remote response exceeds the 48 MiB transport limit");
    }
    let status = status.ok_or_else(|| anyhow!("Remote request timed out after 30 seconds"))?;
    if !status.success() {
        bail!(
            "Remote request failed: {}",
            String::from_utf8_lossy(&errors).trim()
        );
    }
    Ok(String::from_utf8(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn urls_preserve_provider_host_and_nested_repository() {
        let target =
            RemoteTarget::parse("https://gitlab.com/group/sub/project/-/merge_requests/7").unwrap();
        assert_eq!(target.repo, "group/sub/project");
        assert_eq!(target.kind, RemoteTargetKind::Review(7));
        assert_eq!(
            RemoteTarget::parse("https://codeberg.org/owner/repo/pulls/12")
                .unwrap()
                .host,
            "codeberg.org"
        );
        assert!(RemoteTarget::parse("https://github.com/owner/repo/compare/a..b").is_err());
        assert!(RemoteTarget::parse("https://user@github.com/owner/repo/pull/1").is_err());
        assert!(RemoteTarget::parse("https://github.com/owner/../pull/1").is_err());
    }

    #[test]
    fn rename_paths_and_absent_sides_are_distinct() {
        let files = entries(
            vec![
                json!({"filename":"new.rs","previous_filename":"old.rs","status":"renamed"}),
                json!({"filename":"added.rs","status":"added"}),
                json!({"filename":"deleted.rs","status":"removed"}),
            ],
            false,
        )
        .unwrap();
        assert_eq!(
            files[0].old_path.as_deref(),
            Some(std::path::Path::new("old.rs"))
        );
        assert!(matches!(files[1].status, FileStatus::Added));
        assert!(matches!(files[2].status, FileStatus::Deleted));
    }

    #[test]
    fn incomplete_and_duplicate_files_are_rejected() {
        let file = json!({"filename":"file.rs","status":"modified"});
        assert!(entries(vec![file.clone(), file.clone()], false).is_err());
        let files = entries(vec![file], false).unwrap();
        assert!(verify_count(&json!({"changed_files": 2}), &files).is_err());
        assert!(entries(vec![json!({"new_path":"file.rs","too_large":true})], true).is_ok());
    }

    #[test]
    fn revisions_must_be_full_object_ids() {
        assert!(revision(&json!({"sha":"main"}), "/sha").is_err());
        assert_eq!(
            revision(&json!({"sha":"a".repeat(40)}), "/sha").unwrap(),
            "a".repeat(40)
        );
    }
}
