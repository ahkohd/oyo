use super::stream::ValidatedSnapshot;
use super::*;
use std::sync::Arc;

#[derive(Clone)]
enum Coverage {
    ImmutableTrees,
    Changes {
        files: usize,
        additions: usize,
        deletions: usize,
    },
}

impl Coverage {
    fn files(&self) -> Option<usize> {
        match self {
            Self::ImmutableTrees => None,
            Self::Changes { files, .. } => Some(*files),
        }
    }
    fn verify(&self, files: &[FileEntry]) -> Result<()> {
        if self.files().is_some_and(|expected| files.len() != expected) {
            bail!(
                "Provider patch is incomplete: expected {} files; received {}",
                self.files().unwrap(),
                files.len()
            );
        }
        if let Self::Changes {
            additions,
            deletions,
            ..
        } = self
        {
            let actual_additions = files
                .iter()
                .try_fold(0usize, |sum, file| sum.checked_add(file.insertions))
                .ok_or_else(|| anyhow!("Patch addition count overflow"))?;
            let actual_deletions = files
                .iter()
                .try_fold(0usize, |sum, file| sum.checked_add(file.deletions))
                .ok_or_else(|| anyhow!("Patch deletion count overflow"))?;
            if actual_additions != *additions || actual_deletions != *deletions {
                bail!("Provider patch is incomplete: expected {additions} additions and {deletions} deletions; received {actual_additions} and {actual_deletions}");
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
enum RawPatchSource {
    GithubCompare { base_tip: String },
    PublicComparison(String),
    ReviewApi,
}

#[derive(Clone)]
pub(crate) struct PreparedReview {
    target: RemoteTarget,
    before_repo: String,
    after_repo: String,
    before: String,
    after: String,
    descriptor: Value,
    coverage: Coverage,
    source: RawPatchSource,
}

fn count(metadata: &Value, pointer: &str) -> Result<usize> {
    usize::try_from(
        metadata
            .pointer(pointer)
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("Missing exact provider count {pointer}"))?,
    )
    .map_err(Into::into)
}

fn repository_id(metadata: &Value, pointer: &str) -> Result<u64> {
    metadata
        .pointer(pointer)
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or_else(|| anyhow!("Missing provider repository identity {pointer}"))
}

fn web_path(path: &str) -> String {
    path.split('/').map(encode).collect::<Vec<_>>().join("/")
}

fn metadata_endpoint(target: &RemoteTarget) -> Result<String> {
    let number = target.review_number()?;
    Ok(match target.provider {
        ReviewProviderKind::GitHub => format!("repos/{}/pulls/{number}", target.repo),
        ReviewProviderKind::GitLab => {
            format!("projects/{}/merge_requests/{number}", encode(&target.repo))
        }
        ReviewProviderKind::Forgejo => format!("/repos/{}/pulls/{number}", target.repo),
    })
}

impl PreparedReview {
    pub(crate) fn github(target: &RemoteTarget) -> Result<Option<Self>> {
        if !super::stream::patch_enabled() {
            return Ok(None);
        }
        Self::resolve(target).map(Some)
    }

    pub(crate) fn gitlab(target: &RemoteTarget) -> Result<Option<Self>> {
        if !super::stream::patch_enabled() {
            return Ok(None);
        }
        Self::resolve(target).map(Some)
    }

    pub(crate) fn forgejo(target: &RemoteTarget) -> Result<Option<Self>> {
        if !super::stream::patch_enabled() {
            return Ok(None);
        }
        Self::resolve(target).map(Some)
    }

    fn resolve(target: &RemoteTarget) -> Result<Self> {
        let adapter = provider_adapter(target.provider)?;
        let metadata = adapter.remote_json(&target.host, &metadata_endpoint(target)?)?;
        let number_key = if target.provider == ReviewProviderKind::GitLab {
            "iid"
        } else {
            "number"
        };
        if metadata[number_key].as_u64() != Some(target.review_number()?) {
            bail!("Provider returned a different review identity");
        }
        let (
            target,
            before_repo,
            after_repo,
            before,
            after,
            before_id,
            after_id,
            review_version,
            coverage,
            source,
            source_path,
        ) = match target.provider {
            ReviewProviderKind::GitHub => {
                let after = revision(&metadata, "/head/sha")?;
                let base_tip = revision(&metadata, "/base/sha")?;
                let comparison = adapter.remote_json(
                    &target.host,
                    &format!(
                        "repos/{}/compare/{base_tip}...{after}?per_page=1&page=2",
                        target.repo
                    ),
                )?;
                let before = revision(&comparison, "/merge_base_commit/sha")?;
                let before_repo = field(&metadata, "/base/repo/full_name")?;
                let after_repo = field(&metadata, "/head/repo/full_name")?;
                let before_id = repository_id(&metadata, "/base/repo/id")?;
                let after_id = repository_id(&metadata, "/head/repo/id")?;
                let version = serde_json::json!({"baseTip":base_tip,"head":after});
                let coverage = Coverage::Changes {
                    files: count(&metadata, "/changed_files")?,
                    additions: count(&metadata, "/additions")?,
                    deletions: count(&metadata, "/deletions")?,
                };
                (
                    RemoteTarget {
                        repo: before_repo.clone(),
                        ..target.clone()
                    },
                    before_repo,
                    after_repo,
                    before,
                    after,
                    before_id,
                    after_id,
                    version,
                    coverage,
                    RawPatchSource::GithubCompare { base_tip },
                    None,
                )
            }
            ReviewProviderKind::GitLab => {
                let before = revision(&metadata, "/diff_refs/base_sha")?;
                revision(&metadata, "/diff_refs/start_sha")?;
                let after = revision(&metadata, "/diff_refs/head_sha")?;
                let before_id = repository_id(&metadata, "/target_project_id")?;
                let after_id = repository_id(&metadata, "/source_project_id")?;
                let project = adapter
                    .remote_json(&target.host, &format!("projects/{}", encode(&target.repo)))?;
                if repository_id(&project, "/id")? != before_id {
                    bail!("GitLab review target project changed");
                }
                let repo = field(&project, "/path_with_namespace")?;
                let source_project =
                    adapter.remote_json(&target.host, &format!("projects/{after_id}"))?;
                if repository_id(&source_project, "/id")? != after_id {
                    bail!("GitLab review source project changed");
                }
                let source_path = field(&source_project, "/path_with_namespace")?;
                let url = format!("https://{}/{}/-/compare/{before}...{after}?format=diff&from_project_id={after_id}",target.host,web_path(&source_path));
                (
                    RemoteTarget {
                        repo,
                        ..target.clone()
                    },
                    before_id.to_string(),
                    after_id.to_string(),
                    before,
                    after,
                    before_id,
                    after_id,
                    metadata["diff_refs"].clone(),
                    Coverage::ImmutableTrees,
                    if super::gitlab::anonymous(&target.host) {
                        RawPatchSource::PublicComparison(url)
                    } else {
                        RawPatchSource::ReviewApi
                    },
                    Some(source_path),
                )
            }
            ReviewProviderKind::Forgejo => {
                let before = revision(&metadata, "/merge_base")?;
                revision(&metadata, "/base/sha")?;
                let after = revision(&metadata, "/head/sha")?;
                let before_repo = field(&metadata, "/base/repo/full_name")?;
                let after_repo = field(&metadata, "/head/repo/full_name")?;
                let before_id = repository_id(&metadata, "/base/repo/id")?;
                let after_id = repository_id(&metadata, "/head/repo/id")?;
                let version = serde_json::json!({"baseTip":metadata["base"]["sha"],"head":after});
                let coverage = Coverage::ImmutableTrees;
                (
                    RemoteTarget {
                        repo: before_repo.clone(),
                        ..target.clone()
                    },
                    before_repo,
                    after_repo,
                    before,
                    after,
                    before_id,
                    after_id,
                    version,
                    coverage,
                    RawPatchSource::ReviewApi,
                    None,
                )
            }
        };
        if coverage.files().is_some_and(|count| count > 100000) {
            bail!("Remote review exceeds the 100000-file loading limit");
        }
        let mut descriptor = serde_json::json!({"version":1,"acquisition":"patch-v1","streamState":"receiving","expectedFiles":coverage.files(),"beforeRepositoryId":before_id,"afterRepositoryId":after_id,"target":target.canonical_url(),"provider":target.provider.id(),"host":target.host,"beforeRepository":before_repo,"afterRepository":after_repo,"beforeRevision":before,"afterRevision":after,"reviewVersion":review_version,"files":[]});
        if let Coverage::Changes {
            additions,
            deletions,
            ..
        } = coverage
        {
            descriptor["expectedAdditions"] = additions.into();
            descriptor["expectedDeletions"] = deletions.into();
        }
        if matches!(coverage, Coverage::ImmutableTrees) {
            descriptor["coverage"] = "immutable-trees".into();
        }
        descriptor["sourceRepositoryPath"] = source_path.into();
        let (source_branch, target_branch) = match target.provider {
            ReviewProviderKind::GitLab => (
                field(&metadata, "/source_branch")?,
                field(&metadata, "/target_branch")?,
            ),
            _ => (
                field(&metadata, "/head/ref")?,
                field(&metadata, "/base/ref")?,
            ),
        };
        descriptor["sourceBranch"] = source_branch.into();
        descriptor["targetBranch"] = target_branch.into();
        Ok(Self {
            target,
            before_repo,
            after_repo,
            before,
            after,
            descriptor,
            coverage,
            source,
        })
    }

    pub(super) fn expected_files(&self) -> Option<usize> {
        self.coverage.files()
    }

    pub(super) fn request(&self) -> Result<super::http::ReadRequest> {
        Ok(match &self.source {
            RawPatchSource::GithubCompare { base_tip } => {
                let (command, format) = super::github::patch(
                    &self.target.host,
                    &self.target.repo,
                    base_tip,
                    &self.after,
                );
                super::http::ReadRequest {
                    command,
                    input: None,
                    format,
                }
            }
            RawPatchSource::PublicComparison(url) => {
                super::http::ReadRequest::public(url, "Accept: text/plain", self.target.provider)
            }
            RawPatchSource::ReviewApi => {
                let endpoint = metadata_endpoint(&self.target)?;
                match self.target.provider {
                    ReviewProviderKind::GitLab => super::gitlab::request(
                        &self.target.host,
                        &format!("{endpoint}/raw_diffs"),
                        "Accept: text/plain",
                    ),
                    ReviewProviderKind::Forgejo => crate::forgejo::read_request(
                        &self.target.host,
                        &format!("{endpoint}.diff?binary=true"),
                        "Accept: text/plain",
                    )?,
                    ReviewProviderKind::GitHub => unreachable!(),
                }
            }
        })
    }

    pub(super) fn descriptor(&self, files: &[FileEntry], state: &str) -> Value {
        let mut descriptor = self.descriptor.clone();
        descriptor["streamState"] = state.into();
        if state == "complete" && matches!(self.coverage, Coverage::ImmutableTrees) {
            descriptor["expectedFiles"] = files.len().into();
        }
        descriptor["files"] = files.iter().map(|file|serde_json::json!({"path":file.path,"oldPath":file.old_path,"status":format!("{:?}",file.status)})).collect::<Vec<_>>().into();
        descriptor
    }

    fn content_request(
        &self,
        index: usize,
        patch: &oyo_core::patch::PatchRecord,
    ) -> PendingFileContent {
        let source =
            |repo: &str, revision: &str, path: &std::path::Path| ContentSource::RemoteBlob {
                provider: self.target.provider.id().to_owned(),
                host: self.target.host.clone(),
                repo: repo.to_owned(),
                revision: revision.to_owned(),
                path: path.to_string_lossy().into_owned(),
            };
        let old = if patch.entry.status == FileStatus::Added {
            ContentSource::Empty
        } else {
            source(
                &self.before_repo,
                &self.before,
                patch.entry.old_path.as_deref().unwrap_or(&patch.entry.path),
            )
        };
        let new = if patch.entry.status == FileStatus::Deleted {
            ContentSource::Empty
        } else {
            source(&self.after_repo, &self.after, &patch.entry.path)
        };
        PendingFileContent {
            file_index: index,
            old,
            new,
        }
    }

    pub(super) fn append(&self, diff: &mut MultiFileDiff, patch: oyo_core::patch::PatchRecord) {
        let request = self.content_request(diff.file_count(), &patch);
        diff.append_patch(patch, request.old, request.new);
    }

    pub(super) fn verify(
        &self,
        patches: &[oyo_core::patch::PatchRecord],
    ) -> Result<Arc<ValidatedSnapshot>> {
        let files = patches
            .iter()
            .map(|patch| patch.entry.clone())
            .collect::<Vec<_>>();
        self.coverage.verify(&files)?;
        let content = if matches!(self.coverage, Coverage::ImmutableTrees) {
            let pinned = |repo, revision| super::content_proof::PinnedRepository {
                provider: self.target.provider,
                host: &self.target.host,
                repo,
                revision,
            };
            let requests = patches
                .iter()
                .enumerate()
                .map(|(index, patch)| self.content_request(index, patch))
                .collect::<Vec<_>>();
            Some(super::content_proof::verify(
                pinned(&self.before_repo, &self.before),
                pinned(&self.after_repo, &self.after),
                patches,
                &requests,
            )?)
        } else {
            None
        };
        if !verify_current(&self.target, &self.descriptor)? {
            bail!("Review changed while loading; refresh to load a consistent snapshot");
        }
        let descriptor = self.descriptor(&files, "complete");
        Ok(if let Some(content) = content {
            ValidatedSnapshot::verified_delta(descriptor, content)
        } else {
            ValidatedSnapshot::verified(descriptor)
        })
    }
}

pub(crate) fn verify_current(target: &RemoteTarget, descriptor: &Value) -> Result<bool> {
    let adapter = provider_adapter(target.provider)?;
    let metadata = adapter.remote_json(&target.host, &metadata_endpoint(target)?)?;
    let number_key = if target.provider == ReviewProviderKind::GitLab {
        "iid"
    } else {
        "number"
    };
    if metadata[number_key].as_u64() != Some(target.review_number()?) {
        return Ok(false);
    }
    let matches = match target.provider {
        ReviewProviderKind::GitHub => github_metadata_matches(&metadata, descriptor),
        ReviewProviderKind::Forgejo => {
            github_metadata_matches(&metadata, descriptor)
                && metadata["merge_base"] == descriptor["beforeRevision"]
        }
        ReviewProviderKind::GitLab => {
            let mut valid = metadata["diff_refs"] == descriptor["reviewVersion"]
                && metadata["source_project_id"] == descriptor["afterRepositoryId"]
                && metadata["target_project_id"] == descriptor["beforeRepositoryId"]
                && (descriptor["coverage"] == "immutable-trees"
                    || metadata["changes_count"]
                        .as_str()
                        .and_then(|value| value.parse::<u64>().ok())
                        == descriptor["expectedFiles"].as_u64());
            if valid {
                for (id, path) in [
                    (
                        descriptor["beforeRepositoryId"].as_u64(),
                        Some(target.repo.as_str()),
                    ),
                    (
                        descriptor["afterRepositoryId"].as_u64(),
                        descriptor["sourceRepositoryPath"].as_str(),
                    ),
                ] {
                    let (Some(id), Some(path)) = (id, path) else {
                        return Ok(false);
                    };
                    let project = adapter.remote_json(&target.host, &format!("projects/{id}"))?;
                    valid &= project["id"].as_u64() == Some(id)
                        && project["path_with_namespace"].as_str() == Some(path);
                }
            }
            valid
        }
    };
    let (source_branch, target_branch) = match target.provider {
        ReviewProviderKind::GitLab => (&metadata["source_branch"], &metadata["target_branch"]),
        _ => (&metadata["head"]["ref"], &metadata["base"]["ref"]),
    };
    Ok(matches
        && (descriptor["sourceBranch"].is_null() || source_branch == &descriptor["sourceBranch"])
        && (descriptor["targetBranch"].is_null() || target_branch == &descriptor["targetBranch"]))
}

pub(super) fn github_metadata_matches(metadata: &Value, descriptor: &Value) -> bool {
    metadata["head"]["sha"] == descriptor["afterRevision"]
        && metadata["base"]["sha"] == descriptor["reviewVersion"]["baseTip"]
        && metadata["base"]["repo"]["full_name"] == descriptor["beforeRepository"]
        && metadata["head"]["repo"]["full_name"] == descriptor["afterRepository"]
        && (descriptor["beforeRepositoryId"].is_null()
            || metadata["base"]["repo"]["id"] == descriptor["beforeRepositoryId"])
        && (descriptor["afterRepositoryId"].is_null()
            || metadata["head"]["repo"]["id"] == descriptor["afterRepositoryId"])
        && (descriptor["coverage"] == "immutable-trees"
            || descriptor["expectedFiles"].is_null()
            || metadata["changed_files"] == descriptor["expectedFiles"])
        && (descriptor["expectedAdditions"].is_null()
            || metadata["additions"] == descriptor["expectedAdditions"])
        && (descriptor["expectedDeletions"].is_null()
            || metadata["deletions"] == descriptor["expectedDeletions"])
}
