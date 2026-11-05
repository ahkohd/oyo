use super::*;
use oyo_core::multi::ExpectedFileContent;
use oyo_core::patch::{BlobId, BlobSide, PatchContentKind, PatchRecord};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub(super) struct PinnedRepository<'a> {
    pub provider: ReviewProviderKind,
    pub host: &'a str,
    pub repo: &'a str,
    pub revision: &'a str,
}

impl PinnedRepository<'_> {
    fn request(&self, endpoint: &str, accept: &str) -> Result<http::ReadRequest> {
        match self.provider {
            ReviewProviderKind::GitLab => Ok(gitlab::request(self.host, endpoint, accept)),
            ReviewProviderKind::Forgejo => {
                crate::forgejo::read_request(self.host, endpoint, accept)
            }
            ReviewProviderKind::GitHub => {
                bail!("Content proof is unavailable for this acquisition")
            }
        }
    }

    fn source(&self, path: &std::path::Path) -> ContentSource {
        ContentSource::RemoteBlob {
            provider: self.provider.id().to_owned(),
            host: self.host.to_owned(),
            repo: self.repo.to_owned(),
            revision: self.revision.to_owned(),
            path: path.to_string_lossy().into_owned(),
        }
    }

    fn tree(&self) -> Result<BTreeMap<std::path::PathBuf, BlobSide>> {
        let gitlab = self.provider == ReviewProviderKind::GitLab;
        let root = if gitlab {
            format!("projects/{}/repository/tree", encode(self.repo))
        } else {
            format!(
                "/repos/{}/git/trees/{}",
                repository_path(self.repo),
                self.revision
            )
        };
        let mut result = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut cursors = BTreeSet::new();
        let mut cursor: Option<String> = None;
        let mut total = None;
        for page in 1..=1001 {
            if loading::cancelled() {
                bail!("Remote loading cancelled");
            }
            let endpoint = if gitlab {
                let mut path = format!(
                    "{root}?ref={}&recursive=true&pagination=keyset&per_page=100",
                    self.revision
                );
                if let Some(cursor) = &cursor {
                    path.push_str(&format!("&page_token={}", encode(cursor)));
                }
                path
            } else {
                format!("{root}?recursive=true&per_page=100&page={page}")
            };
            let response = self
                .request(&endpoint, "Accept: application/json")?
                .execute()?;
            let value: Value = serde_json::from_slice(&response.body)?;
            let entries = if gitlab {
                value
                    .as_array()
                    .ok_or_else(|| anyhow!("Missing GitLab tree entries"))?
                    .as_slice()
            } else {
                if value["sha"].as_str() != Some(self.revision)
                    || value["page"].as_u64() != Some(page)
                {
                    bail!("Provider changed the pinned tree or page identity");
                }
                let count = value["total_count"]
                    .as_u64()
                    .filter(|n| *n <= 100000)
                    .ok_or_else(|| anyhow!("Missing or excessive repository tree total"))?;
                if total
                    .replace(count)
                    .is_some_and(|previous| previous != count)
                {
                    bail!("Provider changed the immutable tree total");
                }
                match value.get("tree") {
                    Some(Value::Array(entries)) => entries.as_slice(),
                    Some(Value::Null) => &[],
                    _ => bail!("Missing Forgejo tree entries"),
                }
            };
            if entries.len() > 100 {
                bail!("Repository tree page exceeds its requested bound");
            }
            for entry in entries {
                let path = field(entry, "/path")?;
                validate_path(&path)?;
                if seen.len() >= 100000 || !seen.insert(path.clone()) {
                    bail!("Repository tree repeats a path or exceeds its loading limit");
                }
                let object = field(entry, if gitlab { "/id" } else { "/sha" })?;
                let id = BlobId::parse(&object).map_err(|error| anyhow!(error.to_string()))?;
                let mode = u32::from_str_radix(&field(entry, "/mode")?, 8)?;
                match entry["type"].as_str() {
                    Some("tree") if mode == 0o40000 => continue,
                    Some("blob") if matches!(mode, 0o100644 | 0o100755 | 0o120000) => {}
                    _ => bail!("Repository tree contains an unsupported object type or mode"),
                }
                result.insert(std::path::PathBuf::from(path), BlobSide { id, mode });
            }
            if gitlab {
                let next = next_gitlab_cursor(
                    response.headers.get("link").map(String::as_str),
                    &root,
                    self.revision,
                )?;
                if let Some(next) = next {
                    if entries.is_empty() || !cursors.insert(next.clone()) {
                        bail!("GitLab tree pagination stopped advancing");
                    }
                    cursor = Some(next);
                } else {
                    validate_hierarchy(&result)?;
                    return Ok(result);
                }
            } else if entries.is_empty() {
                if Some(seen.len() as u64) != total {
                    bail!("Provider returned an incomplete immutable tree");
                }
                validate_hierarchy(&result)?;
                return Ok(result);
            } else if total.is_some_and(|count| seen.len() as u64 > count) {
                bail!("Provider exceeded its immutable tree total");
            }
        }
        bail!("Repository tree exceeds the 1000-page loading limit")
    }

    fn blob(&self, path: &std::path::Path, id: BlobId) -> Result<Vec<u8>> {
        let source = self.source(path);
        if let Some(bytes) = cached_content(&source) {
            id.verify(&bytes)
                .map_err(|error| anyhow!(error.to_string()))?;
            return Ok(bytes);
        }
        let endpoint = match self.provider {
            ReviewProviderKind::GitLab => {
                format!("projects/{}/repository/blobs/{}/raw", encode(self.repo), id)
            }
            ReviewProviderKind::Forgejo => format!(
                "/repos/{}/raw/{}?ref={}",
                repository_path(self.repo),
                repository_path(&path.to_string_lossy()),
                self.revision
            ),
            ReviewProviderKind::GitHub => unreachable!(),
        };
        let bytes = self
            .request(&endpoint, "Accept: application/octet-stream")?
            .execute()?
            .body;
        if bytes.len() > 32 * 1024 * 1024 {
            bail!("Remote blob exceeds the 32 MiB loading limit");
        }
        id.verify(&bytes)
            .map_err(|error| anyhow!(error.to_string()))?;
        Ok(bytes)
    }
}

fn repository_path(path: &str) -> String {
    path.split('/').map(encode).collect::<Vec<_>>().join("/")
}

fn validate_hierarchy(tree: &BTreeMap<std::path::PathBuf, BlobSide>) -> Result<()> {
    for path in tree.keys() {
        if path
            .ancestors()
            .skip(1)
            .any(|parent| tree.contains_key(parent))
        {
            bail!("Repository tree traverses a non-directory object");
        }
    }
    Ok(())
}

fn next_gitlab_cursor(link: Option<&str>, root: &str, revision: &str) -> Result<Option<String>> {
    let mut next = None;
    for part in link.into_iter().flat_map(|value| value.split(',')) {
        let (target, parameters) = part
            .trim()
            .split_once('>')
            .ok_or_else(|| anyhow!("Malformed GitLab pagination link"))?;
        if !parameters
            .split(';')
            .filter_map(|parameter| parameter.trim().strip_prefix("rel="))
            .any(|relations| {
                relations
                    .trim_matches('"')
                    .split_whitespace()
                    .any(|relation| relation.eq_ignore_ascii_case("next"))
            })
        {
            continue;
        }
        if next.is_some() {
            bail!("GitLab returned multiple next-page links");
        }
        let url = url::Url::parse(
            target
                .strip_prefix('<')
                .ok_or_else(|| anyhow!("Malformed GitLab pagination target"))?,
        )?;
        if url.scheme() != "https"
            || url.path() != format!("/api/v4/{root}")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            bail!("GitLab tree continuation changes its endpoint");
        }
        let mut cursor = None;
        let mut keys = BTreeSet::new();
        for (key, value) in url.query_pairs() {
            if !keys.insert(key.to_string()) {
                bail!("GitLab continuation repeats a parameter");
            }
            match key.as_ref() {
                "ref" if value != revision => {
                    bail!("GitLab tree continuation changes its pinned revision")
                }
                "page_token" => cursor = Some(value.to_string()),
                _ => {}
            }
        }
        let cursor = cursor
            .filter(|value| {
                !value.is_empty() && value.len() <= 8192 && !value.chars().any(char::is_control)
            })
            .ok_or_else(|| anyhow!("GitLab tree continuation lacks a bounded keyset cursor"))?;
        next = Some(cursor);
    }
    Ok(next)
}

struct FileProof<'a> {
    patch: &'a PatchRecord,
    old: Option<BlobSide>,
    new: Option<BlobSide>,
}

type VerifiedBlobs = Mutex<VecDeque<(BlobId, Arc<[u8]>)>>;

impl FileProof<'_> {
    fn verify(
        &self,
        before: PinnedRepository<'_>,
        after: PinnedRepository<'_>,
        cache: &VerifiedBlobs,
    ) -> Result<()> {
        let patch = self.patch;
        if patch.content_kind() != PatchContentKind::Text {
            return Ok(());
        }
        let old_path = patch.entry.old_path.as_deref().unwrap_or(&patch.entry.path);
        let old = if let Some(side) = self.old {
            let cached = cache
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .find(|(id, _)| *id == side.id)
                .map(|(_, bytes)| bytes.clone());
            if let Some(bytes) = cached {
                bytes
            } else {
                let bytes: Arc<[u8]> = before.blob(old_path, side.id)?.into();
                let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
                if !cache.iter().any(|(id, _)| *id == side.id) {
                    while cache.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() + bytes.len()
                        > 64 * 1024 * 1024
                    {
                        cache.pop_front();
                    }
                    cache.push_back((side.id, bytes.clone()));
                }
                bytes
            }
        } else {
            Arc::from([])
        };
        let new = patch
            .reconstruct_text(&old)
            .map_err(|error| anyhow!(error.to_string()))?;
        if let Some(side) = self.new {
            side.id
                .verify(&new)
                .map_err(|error| anyhow!(error.to_string()))?;
        } else if !new.is_empty() {
            bail!("Deleted patch leaves object bytes");
        }
        if self.old.is_some() {
            cache_content(before.source(old_path), old.to_vec());
        }
        if self.new.is_some() {
            cache_content(after.source(&patch.entry.path), new);
        }
        Ok(())
    }
}

pub(super) fn verify(
    before: PinnedRepository<'_>,
    after: PinnedRepository<'_>,
    patches: &[PatchRecord],
    requests: &[PendingFileContent],
) -> Result<Vec<ExpectedFileContent>> {
    if requests.len() != patches.len() {
        bail!("Content proof has a different request manifest");
    }
    let context = loading::current();
    let (old_tree, new_tree) = std::thread::scope(|scope| -> Result<_> {
        let old = scope.spawn(|| context.run(|| before.tree()));
        let new = after.tree();
        Ok((
            old.join()
                .map_err(|_| anyhow!("Immutable tree worker stopped"))??,
            new?,
        ))
    })?;
    let old_changed = old_tree
        .iter()
        .filter(|(path, side)| new_tree.get(*path) != Some(side))
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    let new_changed = new_tree
        .iter()
        .filter(|(path, side)| old_tree.get(*path) != Some(side))
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    let mut covered_old = BTreeSet::new();
    let mut covered_new = BTreeSet::new();
    let mut files = Vec::new();
    for patch in patches {
        let path = &patch.entry.path;
        let old_path = patch.entry.old_path.as_deref().unwrap_or(path);
        let old = if patch.entry.status == FileStatus::Added {
            None
        } else {
            if !old_changed.contains(old_path) || !covered_old.insert(old_path.to_owned()) {
                bail!("Patch repeats or invents an old changed side");
            }
            Some(
                *old_tree
                    .get(old_path)
                    .ok_or_else(|| anyhow!("Patch old path is absent from its pinned tree"))?,
            )
        };
        let new = if patch.entry.status == FileStatus::Deleted {
            None
        } else {
            if !new_changed.contains(path) || !covered_new.insert(path.clone()) {
                bail!("Patch repeats or invents a new changed side");
            }
            Some(
                *new_tree
                    .get(path)
                    .ok_or_else(|| anyhow!("Patch new path is absent from its pinned tree"))?,
            )
        };
        patch
            .validate_sides(old, new)
            .map_err(|error| anyhow!(error.to_string()))?;
        files.push(FileProof { patch, old, new });
    }
    if covered_old != old_changed || covered_new != new_changed {
        bail!("Patch omits changed immutable tree entries");
    }
    let cache = VerifiedBlobs::default();
    let index = AtomicUsize::new(0);
    let failure = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..4.min(files.len()) {
            scope.spawn(|| {
                context.run(|| loop {
                    if failure
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .is_some()
                    {
                        break;
                    }
                    let Some(file) = files.get(index.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let result = if loading::cancelled() {
                        Err(anyhow!("Remote loading cancelled"))
                    } else {
                        file.verify(before, after, &cache)
                    };
                    if let Err(error) = result {
                        let mut failed = failure.lock().unwrap_or_else(|error| error.into_inner());
                        if failed.is_none() {
                            *failed = Some(error);
                        }
                        break;
                    }
                })
            });
        }
    });
    if let Some(error) = failure
        .into_inner()
        .unwrap_or_else(|error| error.into_inner())
    {
        return Err(error);
    }
    Ok(files
        .iter()
        .zip(requests)
        .map(|(file, request)| ExpectedFileContent {
            request: request.clone(),
            old: file.old.map(|side| side.id),
            new: file.new.map(|side| side.id),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyset_continuation_preserves_endpoint_and_revision_and_cannot_supply_a_request_url() {
        let root = "projects/11/repository/tree";
        let revision = "1111111111111111111111111111111111111111";
        let target = format!(
            "https://api.fixture.test/api/v4/{root}?ref={revision}&page_token=opaque%26cursor"
        );
        for relation in ["next", "\"next\"", "\"prev next\""] {
            assert_eq!(
                next_gitlab_cursor(Some(&format!("<{target}>; rel={relation}")), root, revision)
                    .unwrap(),
                Some("opaque&cursor".to_owned())
            );
        }
        assert!(next_gitlab_cursor(None, root, revision).unwrap().is_none());
        for target in [
            target.replace("projects/11", "projects/22"),
            target.replace(revision, "2222222222222222222222222222222222222222"),
            format!("{target}&page_token=other"),
            target.replace("opaque%26cursor", "%00"),
            target.replace("https:", "http:"),
        ] {
            assert!(
                next_gitlab_cursor(Some(&format!("<{target}>; rel=\"next\"")), root, revision)
                    .is_err(),
                "{target}"
            );
        }
    }
}
