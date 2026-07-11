use super::{
    run_output, run_output_with_stdin, App, ProviderPr, ProviderUser, ReviewProviderAdapter,
    ReviewProviderComment, ReviewProviderKind, ReviewProviderPushOps, ReviewPullRemoteData,
    ReviewRemote,
};
use crate::app::review::{ReviewComment, ReviewRange, ReviewSide, ReviewTargetKind};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::Command as ProcessCommand;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Debug, Deserialize)]
struct GlUser {
    username: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GlDiffRefs {
    base_sha: String,
    start_sha: String,
    head_sha: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GlMergeRequestSummary {
    iid: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct GlMergeRequest {
    iid: u64,
    title: String,
    web_url: String,
    source_branch: String,
    target_branch: String,
    diff_refs: GlDiffRefs,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GlAuthor {
    pub(crate) username: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GlPosition {
    #[serde(default)]
    pub(crate) new_path: Option<String>,
    #[serde(default)]
    pub(crate) old_path: Option<String>,
    #[serde(default)]
    pub(crate) new_line: Option<usize>,
    #[serde(default)]
    pub(crate) old_line: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GlNote {
    pub(crate) id: u64,
    pub(crate) body: String,
    pub(crate) author: GlAuthor,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    #[serde(default)]
    pub(crate) system: bool,
    #[serde(default)]
    pub(crate) position: Option<GlPosition>,
    #[serde(default)]
    pub(crate) resolved: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GlDiscussion {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) notes: Vec<GlNote>,
    #[serde(default)]
    pub(crate) resolved: Option<bool>,
}

pub(crate) struct GitlabProvider;

impl ReviewProviderAdapter for GitlabProvider {
    fn find_pr(&self, remote: &ReviewRemote, target: Option<&str>) -> Result<ProviderPr> {
        glab_pr(remote, target)
    }

    fn whoami(&self, pr: &ProviderPr) -> Result<ProviderUser> {
        glab_whoami(&pr.host)
    }

    fn fetch_comments(&self, pr: ProviderPr, user: ProviderUser) -> Result<ReviewPullRemoteData> {
        fetch_comments_for_pull(pr, user)
    }

    fn push_ops<'a>(
        &self,
        pr: &'a ProviderPr,
        user: &'a ProviderUser,
    ) -> Box<dyn ReviewProviderPushOps + 'a> {
        Box::new(GitlabPushOps { pr, user })
    }
}

pub(crate) struct GitlabPushOps<'a> {
    pub(crate) pr: &'a ProviderPr,
    pub(crate) user: &'a ProviderUser,
}

impl ReviewProviderPushOps for GitlabPushOps<'_> {
    fn provider_label(&self) -> &'static str {
        "GitLab"
    }

    fn create_root(&mut self, comment: &ReviewComment) -> Result<ReviewProviderComment> {
        if matches!(
            comment.anchor.kind,
            ReviewTargetKind::Line | ReviewTargetKind::Hunk
        ) {
            create_positioned_discussion(self.pr, self.user, comment)
        } else {
            create_general_discussion(self.pr, self.user, &comment.body)
        }
    }

    fn create_reply(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment> {
        let discussion_id = provider
            .thread_id
            .as_deref()
            .ok_or_else(|| anyhow!("Reply is missing a GitLab discussion id."))?;
        let endpoint = format!(
            "projects/{}/merge_requests/{}/discussions/{discussion_id}/notes",
            project_path(self.pr),
            self.pr.number
        );
        let note: GlNote = glab_api_json(
            &self.pr.host,
            "POST",
            &endpoint,
            serde_json::json!({ "body": body }),
        )?;
        Ok(provider_link(
            self.pr,
            self.user,
            &note,
            provider.api_kind.as_str(),
            Some(discussion_id.to_string()),
            provider.thread_resolved,
            provider.in_reply_to_id.clone(),
        ))
    }

    fn update(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment> {
        let discussion_id = provider
            .thread_id
            .as_deref()
            .ok_or_else(|| anyhow!("GitLab comment is missing a discussion id."))?;
        let endpoint = format!(
            "projects/{}/merge_requests/{}/discussions/{discussion_id}/notes/{}",
            project_path(self.pr),
            self.pr.number,
            provider.comment_id
        );
        let note: GlNote = glab_api_json(
            &self.pr.host,
            "PUT",
            &endpoint,
            serde_json::json!({ "body": body }),
        )?;
        Ok(provider_link(
            self.pr,
            self.user,
            &note,
            provider.api_kind.as_str(),
            Some(discussion_id.to_string()),
            provider.thread_resolved,
            provider.in_reply_to_id.clone(),
        ))
    }

    fn delete(&mut self, provider: &ReviewProviderComment) -> Result<()> {
        let endpoint = format!(
            "projects/{}/merge_requests/{}/notes/{}",
            project_path(self.pr),
            self.pr.number,
            provider.comment_id
        );
        ignore_delete_not_found(glab_api_no_output(&self.pr.host, "DELETE", &endpoint))
    }

    fn set_thread_resolved(&mut self, thread_id: &str, resolved: bool) -> Result<()> {
        let endpoint = format!(
            "projects/{}/merge_requests/{}/discussions/{thread_id}?resolved={resolved}",
            project_path(self.pr),
            self.pr.number
        );
        glab_api_no_output(&self.pr.host, "PUT", &endpoint)
    }
}

pub(crate) fn glab_whoami(host: &str) -> Result<ProviderUser> {
    let user: GlUser = glab_json(host, "user")?;
    if let Some(url) = user.avatar_url.as_deref() {
        let _ = crate::avatars::cache_avatar_url(url);
    }
    Ok(ProviderUser {
        login: user.username,
        avatar_url: user.avatar_url,
    })
}

pub(crate) fn glab_pr(remote: &ReviewRemote, target: Option<&str>) -> Result<ProviderPr> {
    let iid = match target.and_then(parse_mr_iid) {
        Some(iid) => iid,
        None => {
            let branch = target.ok_or_else(|| anyhow!("No merge request branch found."))?;
            let endpoint = format!(
                "projects/{}/merge_requests?state=opened&source_branch={}",
                percent_encode(&remote.repo),
                percent_encode(branch)
            );
            let matches: Vec<GlMergeRequestSummary> = glab_json(&remote.host, &endpoint)?;
            match matches.as_slice() {
                [mr] => mr.iid,
                [] => {
                    anyhow::bail!("No merge request found for {branch} in {}.", remote.repo)
                }
                _ => anyhow::bail!(
                    "Several merge requests use source branch {branch} in {}. Pass an MR IID.",
                    remote.repo
                ),
            }
        }
    };
    let mr: GlMergeRequest = glab_json(
        &remote.host,
        &format!(
            "projects/{}/merge_requests/{iid}",
            percent_encode(&remote.repo)
        ),
    )?;
    Ok(ProviderPr {
        provider: ReviewProviderKind::GitLab,
        remote: remote.name.clone(),
        host: remote.host.clone(),
        repo: remote.repo.clone(),
        number: mr.iid,
        title: mr.title,
        url: mr.web_url,
        base_branch: mr.target_branch,
        head_branch: mr.source_branch,
        base_commit: mr.diff_refs.base_sha,
        start_commit: Some(mr.diff_refs.start_sha),
        head_commit: mr.diff_refs.head_sha,
    })
}

pub(crate) fn fetch_comments_for_pull(
    pr: ProviderPr,
    user: ProviderUser,
) -> Result<super::ReviewPullRemoteData> {
    let endpoint = format!(
        "projects/{}/merge_requests/{}/discussions?per_page=100",
        project_path(&pr),
        pr.number
    );
    let discussions: Vec<GlDiscussion> = glab_json_paginated(&pr.host, &endpoint)?;
    Ok(super::ReviewPullRemoteData::GitLab {
        pr,
        user,
        discussions,
    })
}

pub(crate) fn discussion_to_review_comments(
    app: &App,
    pr: &ProviderPr,
    current_login: &str,
    discussion: GlDiscussion,
) -> Vec<Result<ReviewComment>> {
    let user_notes = discussion
        .notes
        .iter()
        .filter(|note| !note.system)
        .collect::<Vec<_>>();
    let root_note_id = user_notes.first().map(|note| note.id.to_string());
    let root_position = user_notes.iter().find_map(|note| note.position.clone());
    let thread_resolved = discussion
        .resolved
        .or_else(|| user_notes.iter().find_map(|note| note.resolved))
        .unwrap_or(false);
    discussion
        .notes
        .into_iter()
        .filter(|note| !note.system)
        .map(|note| {
            let position = note.position.clone().or_else(|| root_position.clone());
            match position {
                Some(position) => inline_note_to_review_comment(
                    app,
                    pr,
                    current_login,
                    &discussion.id,
                    root_note_id.as_deref(),
                    thread_resolved,
                    note,
                    position,
                ),
                None => conversation_note_to_review_comment(
                    app,
                    pr,
                    current_login,
                    &discussion.id,
                    root_note_id.as_deref(),
                    note,
                ),
            }
        })
        .collect()
}

pub(crate) fn positioned_discussion_body(
    pr: &ProviderPr,
    comment: &ReviewComment,
) -> Result<serde_json::Value> {
    let anchor = &comment.anchor;
    let side = anchor.side.unwrap_or_else(|| {
        if anchor.new_range.is_some() {
            ReviewSide::New
        } else {
            ReviewSide::Old
        }
    });
    match side {
        ReviewSide::Old => anchor.old_range,
        ReviewSide::New => anchor.new_range,
    }
    .ok_or_else(|| {
        anyhow!(
            "Comment {} has no {}-side line anchor",
            comment.id,
            side.as_str()
        )
    })?;
    let start_sha = pr
        .start_commit
        .as_deref()
        .ok_or_else(|| anyhow!("GitLab merge request is missing diff refs."))?;
    let mut position = serde_json::json!({
        "position_type": "text",
        "base_sha": pr.base_commit,
        "start_sha": start_sha,
        "head_sha": pr.head_commit,
        "new_path": anchor.file_path,
        "old_path": anchor.file_path,
    });
    if anchor.kind == ReviewTargetKind::Line {
        if let Some(range) = anchor.old_range {
            position["old_line"] = serde_json::json!(range.end);
        }
        if let Some(range) = anchor.new_range {
            position["new_line"] = serde_json::json!(range.end);
        }
    } else {
        match side {
            ReviewSide::Old => {
                position["old_line"] = serde_json::json!(anchor.old_range.unwrap().end)
            }
            ReviewSide::New => {
                position["new_line"] = serde_json::json!(anchor.new_range.unwrap().end)
            }
        }
    }
    Ok(serde_json::json!({
        "body": comment.body,
        "position": position,
    }))
}

fn create_positioned_discussion(
    pr: &ProviderPr,
    user: &ProviderUser,
    comment: &ReviewComment,
) -> Result<ReviewProviderComment> {
    let endpoint = format!(
        "projects/{}/merge_requests/{}/discussions",
        project_path(pr),
        pr.number
    );
    let discussion: GlDiscussion = glab_api_json(
        &pr.host,
        "POST",
        &endpoint,
        positioned_discussion_body(pr, comment)?,
    )?;
    let note = ensure_positioned_discussion(&discussion)?;
    let discussion_id = discussion.id.clone();
    let thread_resolved = note.resolved.or(discussion.resolved).or(Some(false));
    Ok(provider_link(
        pr,
        user,
        note,
        "review",
        Some(discussion_id),
        thread_resolved,
        None,
    ))
}

pub(crate) fn ensure_positioned_discussion(discussion: &GlDiscussion) -> Result<&GlNote> {
    let note = discussion
        .notes
        .iter()
        .find(|note| !note.system)
        .ok_or_else(|| anyhow!("GitLab returned no created note."))?;
    if note.position.is_none() {
        anyhow::bail!("GitLab inline comment creation returned an unpositioned discussion.");
    }
    Ok(note)
}

fn create_general_discussion(
    pr: &ProviderPr,
    user: &ProviderUser,
    body: &str,
) -> Result<ReviewProviderComment> {
    let endpoint = format!(
        "projects/{}/merge_requests/{}/discussions",
        project_path(pr),
        pr.number
    );
    let discussion: GlDiscussion = glab_api_json(
        &pr.host,
        "POST",
        &endpoint,
        serde_json::json!({ "body": body }),
    )?;
    let note = discussion
        .notes
        .iter()
        .find(|note| !note.system)
        .ok_or_else(|| anyhow!("GitLab returned no created note."))?;
    Ok(provider_link(
        pr,
        user,
        note,
        "issue",
        Some(discussion.id),
        None,
        None,
    ))
}

#[allow(clippy::too_many_arguments)]
fn inline_note_to_review_comment(
    app: &App,
    pr: &ProviderPr,
    current_login: &str,
    discussion_id: &str,
    root_note_id: Option<&str>,
    thread_resolved: bool,
    note: GlNote,
    position: GlPosition,
) -> Result<ReviewComment> {
    let side = if position.new_line.is_some() {
        ReviewSide::New
    } else if position.old_line.is_some() {
        ReviewSide::Old
    } else {
        anyhow::bail!("GitLab note has no line.");
    };
    let file = match side {
        ReviewSide::New => position.new_path.or_else(|| position.old_path.clone()),
        ReviewSide::Old => position.old_path.or(position.new_path),
    }
    .ok_or_else(|| anyhow!("GitLab note has no path."))?;
    let old_range = position.old_line.map(|line| ReviewRange {
        start: line,
        end: line,
    });
    let new_range = position.new_line.map(|line| ReviewRange {
        start: line,
        end: line,
    });
    let in_reply_to_id = root_note_id
        .filter(|root| *root != note.id.to_string())
        .map(str::to_string);
    let data = serde_json::json!({
        "version": 1,
        "comments": [{
            "file": file,
            "kind": "line",
            "side": side.as_str(),
            "oldRange": old_range,
            "newRange": new_range,
            "author": author_json("gitlab", &note.author),
            "canEdit": note.author.username == current_login,
            "provider": provider_json(
                pr,
                &note,
                "review",
                Some(discussion_id),
                Some(thread_resolved),
                in_reply_to_id,
            ),
            "createdAt": parse_time(&note.created_at),
            "updatedAt": parse_time(&note.updated_at),
            "resolved": thread_resolved,
            "body": note.body
        }]
    });
    parse_one(app, data)
}

fn conversation_note_to_review_comment(
    app: &App,
    pr: &ProviderPr,
    current_login: &str,
    discussion_id: &str,
    root_note_id: Option<&str>,
    note: GlNote,
) -> Result<ReviewComment> {
    let in_reply_to_id = root_note_id
        .filter(|root| *root != note.id.to_string())
        .map(str::to_string);
    let data = serde_json::json!({
        "version": 1,
        "comments": [{
            "file": pr.title,
            "kind": "pr",
            "author": author_json("gitlab", &note.author),
            "canEdit": note.author.username == current_login,
            "provider": provider_json(
                pr,
                &note,
                "issue",
                Some(discussion_id),
                None,
                in_reply_to_id,
            ),
            "createdAt": parse_time(&note.created_at),
            "updatedAt": parse_time(&note.updated_at),
            "body": note.body
        }]
    });
    parse_one(app, data)
}

fn parse_one(app: &App, data: serde_json::Value) -> Result<ReviewComment> {
    let comments = app
        .parse_review_comments_json_for_sync(&data.to_string())
        .map_err(|error| anyhow!(error))?;
    comments
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Provider comment did not map to this diff."))
}

fn author_json(provider: &str, author: &GlAuthor) -> serde_json::Value {
    let mut usernames = BTreeMap::new();
    usernames.insert(provider.to_string(), author.username.clone());
    serde_json::json!({
        "name": author.name.clone().unwrap_or_else(|| author.username.clone()),
        "usernames": usernames,
        "avatarUrl": author.avatar_url,
    })
}

fn provider_json(
    pr: &ProviderPr,
    note: &GlNote,
    api_kind: &str,
    thread_id: Option<&str>,
    thread_resolved: Option<bool>,
    in_reply_to_id: Option<String>,
) -> serde_json::Value {
    serde_json::json!({
        "provider": "gitlab",
        "remote": pr.remote,
        "repo": pr.repo,
        "prNumber": pr.number,
        "commentId": note.id.to_string(),
        "inReplyToId": in_reply_to_id,
        "threadId": thread_id,
        "threadResolved": thread_resolved,
        "authorUsername": note.author.username,
        "prTitle": pr.title,
        "prUrl": pr.url,
        "apiKind": api_kind,
        "syncState": "clean"
    })
}

fn provider_link(
    pr: &ProviderPr,
    user: &ProviderUser,
    note: &GlNote,
    api_kind: &str,
    thread_id: Option<String>,
    thread_resolved: Option<bool>,
    in_reply_to_id: Option<String>,
) -> ReviewProviderComment {
    ReviewProviderComment {
        provider: "gitlab".to_string(),
        remote: pr.remote.clone(),
        repo: pr.repo.clone(),
        pr_number: pr.number,
        comment_id: note.id.to_string(),
        in_reply_to_id,
        thread_id,
        thread_resolved,
        resolved_dirty: false,
        author_username: Some(user.login.clone()),
        pr_title: Some(pr.title.clone()),
        pr_url: Some(pr.url.clone()),
        api_kind: api_kind.to_string(),
        sync_state: "clean".to_string(),
    }
}

fn glab_json<T: for<'de> Deserialize<'de>>(host: &str, endpoint: &str) -> Result<T> {
    let mut command = glab_api_command(host);
    command.arg(endpoint);
    let data = run_output(command)?;
    serde_json::from_str(&data).map_err(|error| anyhow!(error))
}

fn glab_json_paginated<T: for<'de> Deserialize<'de>>(host: &str, endpoint: &str) -> Result<Vec<T>> {
    let mut command = glab_api_command(host);
    command.arg(endpoint).arg("--paginate");
    let data = run_output(command)?;
    parse_paginated_json(&data)
}

fn parse_paginated_json<T: for<'de> Deserialize<'de>>(data: &str) -> Result<Vec<T>> {
    let mut items = Vec::new();
    for page in serde_json::Deserializer::from_str(data).into_iter::<Vec<T>>() {
        items.extend(page?);
    }
    Ok(items)
}

fn glab_api_json<T: for<'de> Deserialize<'de>>(
    host: &str,
    method: &str,
    endpoint: &str,
    body: serde_json::Value,
) -> Result<T> {
    let mut command = glab_api_command(host);
    command
        .arg("-X")
        .arg(method)
        .arg(endpoint)
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("--input")
        .arg("-");
    let data = run_output_with_stdin(command, &body.to_string())?;
    serde_json::from_str(&data).map_err(|error| anyhow!(error))
}

fn glab_api_no_output(host: &str, method: &str, endpoint: &str) -> Result<()> {
    let mut command = glab_api_command(host);
    command.arg("-X").arg(method).arg(endpoint).arg("--silent");
    run_output(command).map(|_| ())
}

fn glab_api_command(host: &str) -> ProcessCommand {
    let mut command = ProcessCommand::new("glab");
    command.arg("api").arg("--hostname").arg(host);
    command
}

fn ignore_delete_not_found(result: Result<()>) -> Result<()> {
    match result {
        Err(error) if error.to_string().to_ascii_lowercase().contains("404") => Ok(()),
        result => result,
    }
}

fn parse_time(value: &str) -> u64 {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .and_then(|date| u64::try_from(date.unix_timestamp()).ok())
        .unwrap_or(0)
}

fn parse_mr_iid(target: &str) -> Option<u64> {
    let value = target.strip_prefix('!').unwrap_or(target);
    (!value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

fn project_path(pr: &ProviderPr) -> String {
    percent_encode(&pr.repo)
}

pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{glab_api_command, parse_paginated_json, GlMergeRequestSummary};
    use std::ffi::OsStr;

    #[test]
    fn glab_api_uses_the_remote_host() {
        let command = glab_api_command("gitlab.example.org");
        assert_eq!(command.get_program(), OsStr::new("glab"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["api", "--hostname", "gitlab.example.org"]
                .map(OsStr::new)
                .to_vec()
        );
    }

    #[test]
    fn merge_request_search_only_requires_iid() {
        let matches: Vec<GlMergeRequestSummary> =
            serde_json::from_str(r#"[{"iid":1,"title":"Review"}]"#).unwrap();
        assert_eq!(matches[0].iid, 1);
    }

    #[test]
    fn paginated_json_flattens_concatenated_pages() {
        let items: Vec<serde_json::Value> =
            parse_paginated_json(r#"[{"id":1}][{"id":2},{"id":3}]"#).unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| item["id"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }
}
