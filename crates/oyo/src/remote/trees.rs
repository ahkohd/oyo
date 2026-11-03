use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone)]
struct Commit {
    sha: String,
    parents: Vec<String>,
}

fn commit(
    adapter: &dyn ReviewProviderAdapter,
    target: &RemoteTarget,
    reference: &str,
) -> Result<Commit> {
    let endpoint = match target.provider {
        ReviewProviderKind::GitLab => format!(
            "projects/{}/repository/commits/{}",
            encode(&target.repo),
            encode(reference)
        ),
        ReviewProviderKind::Forgejo => format!(
            "/repos/{}/git/commits/{}?stat=false&files=false",
            target.repo,
            encode(reference)
        ),
        ReviewProviderKind::GitHub => unreachable!(),
    };
    let value = adapter.remote_json(&target.host, &endpoint)?;
    let (sha, parents) = if matches!(target.provider, ReviewProviderKind::GitLab) {
        let parents = value["parent_ids"]
            .as_array()
            .ok_or_else(|| anyhow!("Missing commit parents"))?
            .iter()
            .map(|id| revision(&serde_json::json!({"sha":id}), "/sha"))
            .collect::<Result<Vec<_>>>()?;
        (revision(&value, "/id")?, parents)
    } else {
        let parents = value["parents"]
            .as_array()
            .ok_or_else(|| anyhow!("Missing commit parents"))?
            .iter()
            .map(|parent| revision(parent, "/sha"))
            .collect::<Result<Vec<_>>>()?;
        (revision(&value, "/sha")?, parents)
    };
    Ok(Commit { sha, parents })
}

fn ancestor_graph(
    adapter: &dyn ReviewProviderAdapter,
    target: &RemoteTarget,
    tip: &str,
    graph: &mut BTreeMap<String, Vec<String>>,
) -> Result<BTreeSet<String>> {
    let mut seen = BTreeSet::new();
    let mut pending = VecDeque::from([tip.to_owned()]);
    while let Some(sha) = pending.pop_front() {
        if !seen.insert(sha.clone()) {
            continue;
        }
        if !graph.contains_key(&sha) {
            if graph.len() >= 2048 {
                bail!("Forgejo merge-base search exceeded 2048 commits; use an explicit two-dot comparison for endpoint semantics");
            }
            let item = commit(adapter, target, &sha)?;
            if item.sha != sha {
                bail!("Provider changed an immutable commit identity");
            }
            graph.insert(sha.clone(), item.parents);
        }
        pending.extend(graph[&sha].iter().cloned());
    }
    Ok(seen)
}

fn merge_base(
    adapter: &dyn ReviewProviderAdapter,
    target: &RemoteTarget,
    before: &str,
    after: &str,
) -> Result<String> {
    if before == after {
        return Ok(before.to_owned());
    }
    if matches!(target.provider, ReviewProviderKind::GitLab) {
        return revision(
            &adapter.remote_json(
                &target.host,
                &format!(
                    "projects/{}/repository/merge_base?refs%5B%5D={before}&refs%5B%5D={after}",
                    encode(&target.repo)
                ),
            )?,
            "/id",
        );
    }
    let mut graph = BTreeMap::new();
    let left = ancestor_graph(adapter, target, before, &mut graph)?;
    let right = ancestor_graph(adapter, target, after, &mut graph)?;
    let common = left.intersection(&right).cloned().collect::<BTreeSet<_>>();
    let mut best = common.clone();
    for candidate in &common {
        for parent in &graph[candidate] {
            best.remove(parent);
        }
    }
    if best.len() != 1 {
        bail!("Comparison has no unique merge base; use an explicit two-dot comparison");
    }
    Ok(best.into_iter().next().unwrap())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeEntry {
    blob: String,
    mode: String,
}

fn tree(
    adapter: &dyn ReviewProviderAdapter,
    target: &RemoteTarget,
    revision_id: &str,
) -> Result<BTreeMap<String, TreeEntry>> {
    if revision_id.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut result = BTreeMap::new();
    let mut seen = HashSet::new();
    for page in 1..=1000 {
        let gitlab = matches!(target.provider, ReviewProviderKind::GitLab);
        let endpoint = if gitlab {
            format!("projects/{}/repository/tree?ref={revision_id}&recursive=true&per_page=100&page={page}", encode(&target.repo))
        } else {
            format!(
                "/repos/{}/git/trees/{revision_id}?recursive=true&per_page=100&page={page}",
                target.repo
            )
        };
        let value = adapter.remote_json(&target.host, &endpoint)?;
        let entries = if gitlab {
            value.as_array()
        } else {
            value["tree"].as_array()
        }
        .ok_or_else(|| anyhow!("Missing repository tree entries"))?;
        for entry in entries {
            let path = field(entry, "/path")?;
            validate_path(&path)?;
            if !seen.insert(path.clone()) {
                bail!("Provider repeated a tree entry during pagination");
            }
            match entry["type"].as_str() {
                Some("tree") => continue,
                Some("blob") => {}
                _ => bail!("Remote submodule comparisons are unsupported"),
            }
            let blob = revision(entry, if gitlab { "/id" } else { "/sha" })?;
            result.insert(
                path,
                TreeEntry {
                    blob,
                    mode: field(entry, "/mode")?,
                },
            );
        }
        if gitlab && entries.is_empty() {
            return Ok(result);
        }
        if !gitlab {
            let total = value["total_count"]
                .as_u64()
                .ok_or_else(|| anyhow!("Missing tree entry count"))?;
            if value["truncated"].as_bool() == Some(false) {
                if seen.len() as u64 != total {
                    bail!("Provider returned an incomplete repository tree");
                }
                return Ok(result);
            }
            if entries.is_empty() {
                bail!("Provider truncated the repository tree");
            }
        }
    }
    bail!("Repository tree exceeds the 1000-page limit")
}

fn changed_files(
    before: BTreeMap<String, TreeEntry>,
    after: BTreeMap<String, TreeEntry>,
) -> Result<Vec<FileEntry>> {
    let paths = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut files = Vec::new();
    for path in paths {
        let old = before.get(&path);
        let new = after.get(&path);
        if old == new {
            continue;
        }
        if let (Some(old), Some(new)) = (old, new) {
            if old.mode != new.mode {
                bail!("Remote comparison contains a file-mode change, which is unsupported");
            }
        }
        let status = match (old, new) {
            (None, Some(_)) => FileStatus::Added,
            (Some(_), None) => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        files.push(FileEntry {
            path: path.clone().into(),
            display_name: path,
            old_path: None,
            old_source_path: None,
            new_source_path: None,
            status,
            insertions: 0,
            deletions: 0,
            binary: false,
        });
    }
    Ok(files)
}

pub(super) fn snapshot(
    adapter: &dyn ReviewProviderAdapter,
    target: &RemoteTarget,
) -> Result<RemoteSnapshot> {
    let (before, after) = match &target.kind {
        RemoteTargetKind::Commit(reference) => {
            let after = commit(adapter, target, reference)?;
            (
                after.parents.first().cloned().unwrap_or_default(),
                after.sha,
            )
        }
        RemoteTargetKind::Compare {
            before,
            after,
            merge_base: use_merge_base,
        } => {
            let before = commit(adapter, target, before)?.sha;
            let after = commit(adapter, target, after)?.sha;
            let before = if *use_merge_base {
                merge_base(adapter, target, &before, &after)?
            } else {
                before
            };
            (before, after)
        }
        RemoteTargetKind::Review(_) => unreachable!(),
    };
    let files = changed_files(
        tree(adapter, target, &before)?,
        tree(adapter, target, &after)?,
    )?;
    Ok(RemoteSnapshot {
        target: target.clone(),
        before_repo: target.repo.clone(),
        after_repo: target.repo.clone(),
        before,
        after,
        files,
    })
}
