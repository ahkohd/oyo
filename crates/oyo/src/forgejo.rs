use super::{
    run_output_with_stdin, App, ProviderPr, ProviderUser, ReviewProviderAdapter,
    ReviewProviderComment, ReviewProviderKind, ReviewProviderPushOps, ReviewPullRemoteData,
    ReviewRemote,
};
use crate::app::review::{ReviewComment, ReviewRange, ReviewSide, ReviewTargetKind};
use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::Command as ProcessCommand;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const PAGE_SIZE: usize = 50;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FjUser {
    login: String,
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct FjBranch {
    #[serde(rename = "ref")]
    branch: String,
    sha: String,
}

#[derive(Debug, Clone, Deserialize)]
struct FjPullRequest {
    number: u64,
    title: String,
    html_url: String,
    base: FjBranch,
    head: FjBranch,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FjReview {
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) stale: bool,
    #[serde(default)]
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) body: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FjReviewComment {
    pub(crate) id: u64,
    pub(crate) body: String,
    pub(crate) user: FjUser,
    #[serde(default)]
    pub(crate) resolver: Option<FjUser>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) position: usize,
    #[serde(default)]
    pub(crate) original_position: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FjIssueComment {
    pub(crate) id: u64,
    pub(crate) body: String,
    pub(crate) user: FjUser,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FjReviewComments {
    pub(crate) review: FjReview,
    pub(crate) comments: Vec<FjReviewComment>,
}

pub(crate) struct ForgejoProvider;

impl ReviewProviderAdapter for ForgejoProvider {
    fn find_pr(&self, remote: &ReviewRemote, target: Option<&str>) -> Result<ProviderPr> {
        forgejo_pr(remote, target)
    }

    fn whoami(&self, pr: &ProviderPr) -> Result<ProviderUser> {
        forgejo_whoami(&pr.host)
    }

    fn fetch_comments(&self, pr: ProviderPr, user: ProviderUser) -> Result<ReviewPullRemoteData> {
        fetch_comments_for_pull(pr, user)
    }

    fn push_ops<'a>(
        &self,
        pr: &'a ProviderPr,
        user: &'a ProviderUser,
    ) -> Box<dyn ReviewProviderPushOps + 'a> {
        Box::new(ForgejoPushOps { pr, user })
    }
}

pub(crate) struct ForgejoPushOps<'a> {
    pub(crate) pr: &'a ProviderPr,
    pub(crate) user: &'a ProviderUser,
}

impl ReviewProviderPushOps for ForgejoPushOps<'_> {
    fn provider_label(&self) -> &'static str {
        "Forgejo"
    }

    fn create_root(&mut self, comment: &ReviewComment) -> Result<ReviewProviderComment> {
        if matches!(
            comment.anchor.kind,
            ReviewTargetKind::Line | ReviewTargetKind::Hunk
        ) {
            create_inline_comment(self.pr, self.user, comment)
        } else {
            create_issue_comment(self.pr, self.user, &comment.body)
        }
    }

    fn create_reply(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment> {
        let review_id = review_id(provider)?;
        let parent_id = provider
            .in_reply_to_id
            .as_deref()
            .ok_or_else(|| anyhow!("Forgejo reply is missing a parent comment id."))?;
        let parent: FjReviewComment = cb_json(
            &self.pr.host,
            "GET",
            &format!(
                "/repos/{}/pulls/{}/reviews/{review_id}/comments/{parent_id}",
                self.pr.repo, self.pr.number
            ),
            None,
        )?;
        let comment = create_review_comment(
            self.pr,
            review_id,
            &parent.path,
            parent.original_position,
            parent.position,
            body,
        )?;
        Ok(provider_link(
            self.pr,
            self.user,
            &comment,
            "review",
            provider.thread_id.clone(),
            provider.thread_resolved,
            Some(parent_id.to_string()),
        ))
    }

    fn update(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment> {
        cb_no_output(
            &self.pr.host,
            "PATCH",
            &format!(
                "/repos/{}/issues/comments/{}",
                self.pr.repo, provider.comment_id
            ),
            Some(serde_json::json!({ "body": body })),
        )?;
        let mut clean = provider.clone();
        clean.sync_state = "clean".to_string();
        Ok(clean)
    }

    fn delete(&mut self, provider: &ReviewProviderComment) -> Result<()> {
        if provider.api_kind == "issue" {
            return ignore_delete_not_found(cb_no_output(
                &self.pr.host,
                "DELETE",
                &format!(
                    "/repos/{}/issues/comments/{}",
                    self.pr.repo, provider.comment_id
                ),
                None,
            ));
        }
        let review_id = review_id(provider)?;
        let review_path = format!(
            "/repos/{}/pulls/{}/reviews/{review_id}",
            self.pr.repo, self.pr.number
        );
        let delete_empty_review = if provider.in_reply_to_id.is_none() {
            match cb_json::<FjReview>(&self.pr.host, "GET", &review_path, None) {
                Ok(review) => can_delete_empty_review(&review),
                Err(error) if is_not_found(&error) => return Ok(()),
                Err(error) => return Err(error),
            }
        } else {
            false
        };
        ignore_delete_not_found(cb_no_output(
            &self.pr.host,
            "DELETE",
            &format!("{review_path}/comments/{}", provider.comment_id),
            None,
        ))?;
        if delete_empty_review {
            let comments: Vec<FjReviewComment> = match cb_json(
                &self.pr.host,
                "GET",
                &format!("{review_path}/comments"),
                None,
            ) {
                Err(error) if is_not_found(&error) => return Ok(()),
                result => result?,
            };
            if comments.is_empty() {
                ignore_delete_not_found(cb_no_output(&self.pr.host, "DELETE", &review_path, None))?;
            }
        }
        Ok(())
    }

    fn set_thread_resolved(&mut self, _thread_id: &str, _resolved: bool) -> Result<()> {
        anyhow::bail!(
            "the Forgejo API exposes resolved review state as read-only and has no thread resolve endpoint"
        )
    }
}

fn forgejo_whoami(host: &str) -> Result<ProviderUser> {
    let user: FjUser = cb_json(host, "GET", "/user", None)?;
    if let Some(url) = user.avatar_url.as_deref() {
        let _ = crate::avatars::cache_avatar_url(url);
    }
    Ok(ProviderUser {
        login: user.login,
        avatar_url: user.avatar_url,
    })
}

fn forgejo_pr(remote: &ReviewRemote, target: Option<&str>) -> Result<ProviderPr> {
    let number = match target.and_then(parse_pr_number) {
        Some(number) => number,
        None => {
            let branch = target.ok_or_else(|| anyhow!("No pull request branch found."))?;
            let pulls: Vec<FjPullRequest> = cb_pages(
                &remote.host,
                &format!("/repos/{}/pulls?state=open", remote.repo),
            )?;
            let matches = pulls
                .into_iter()
                .filter(|pull| pull.head.branch == branch)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [pull] => pull.number,
                [] => anyhow::bail!("No pull request found for {branch} in {}.", remote.repo),
                _ => anyhow::bail!(
                    "Several pull requests use source branch {branch} in {}. Pass a PR number.",
                    remote.repo
                ),
            }
        }
    };
    let pull: FjPullRequest = cb_json(
        &remote.host,
        "GET",
        &format!("/repos/{}/pulls/{number}", remote.repo),
        None,
    )?;
    Ok(ProviderPr {
        provider: ReviewProviderKind::Forgejo,
        remote: remote.name.clone(),
        host: remote.host.clone(),
        repo: remote.repo.clone(),
        number: pull.number,
        title: pull.title,
        url: pull.html_url,
        base_branch: pull.base.branch,
        head_branch: pull.head.branch,
        base_commit: pull.base.sha,
        start_commit: None,
        head_commit: pull.head.sha,
    })
}

pub(crate) fn fetch_comments_for_pull(
    pr: ProviderPr,
    user: ProviderUser,
) -> Result<ReviewPullRemoteData> {
    let reviews: Vec<FjReview> = cb_pages(
        &pr.host,
        &format!("/repos/{}/pulls/{}/reviews", pr.repo, pr.number),
    )?;
    let mut review_comments = Vec::new();
    for review in reviews {
        let comments = cb_json(
            &pr.host,
            "GET",
            &format!(
                "/repos/{}/pulls/{}/reviews/{}/comments",
                pr.repo, pr.number, review.id
            ),
            None,
        )?;
        review_comments.push(FjReviewComments { review, comments });
    }
    let issue_comments = cb_json(
        &pr.host,
        "GET",
        &format!("/repos/{}/issues/{}/comments", pr.repo, pr.number),
        None,
    )?;
    Ok(ReviewPullRemoteData::Forgejo {
        pr,
        user,
        review_comments,
        issue_comments,
    })
}

pub(crate) fn review_comments_to_oyo(
    app: &App,
    pr: &ProviderPr,
    current_login: &str,
    reviews: Vec<FjReviewComments>,
) -> Vec<Result<ReviewComment>> {
    let mut mapped = Vec::new();
    for review in reviews {
        // ponytail: Forgejo exposes no thread id; it groups review comments by file and line.
        let mut groups = BTreeMap::<(String, usize, usize), Vec<FjReviewComment>>::new();
        for comment in review.comments {
            groups
                .entry((
                    comment.path.clone(),
                    comment.original_position,
                    comment.position,
                ))
                .or_default()
                .push(comment);
        }
        for comments in groups.values_mut() {
            comments.sort_by_key(|comment| (parse_time(&comment.created_at), comment.id));
            let root_id = comments.first().map(|comment| comment.id.to_string());
            let resolved = comments.iter().any(|comment| comment.resolver.is_some());
            for comment in comments.drain(..) {
                let thread_id = root_id
                    .as_deref()
                    .map(|root| format!("review:{}:{root}", review.review.id));
                let in_reply_to_id = root_id
                    .as_deref()
                    .filter(|root| *root != comment.id.to_string())
                    .map(str::to_string);
                mapped.push(inline_comment_to_oyo(
                    app,
                    pr,
                    current_login,
                    review.review.stale,
                    resolved,
                    thread_id,
                    in_reply_to_id,
                    comment,
                ));
            }
        }
    }
    mapped
}

#[allow(clippy::too_many_arguments)]
fn inline_comment_to_oyo(
    app: &App,
    pr: &ProviderPr,
    current_login: &str,
    stale: bool,
    resolved: bool,
    thread_id: Option<String>,
    in_reply_to_id: Option<String>,
    comment: FjReviewComment,
) -> Result<ReviewComment> {
    let old_range = (comment.original_position > 0).then_some(ReviewRange {
        start: comment.original_position,
        end: comment.original_position,
    });
    let new_range = (comment.position > 0).then_some(ReviewRange {
        start: comment.position,
        end: comment.position,
    });
    let side = if new_range.is_some() {
        ReviewSide::New
    } else if old_range.is_some() {
        ReviewSide::Old
    } else {
        anyhow::bail!("Forgejo review comment {} has no position.", comment.id);
    };
    let data = serde_json::json!({
        "version": 1,
        "comments": [{
            "file": comment.path,
            "kind": "line",
            "side": side.as_str(),
            "oldRange": old_range,
            "newRange": new_range,
            "author": author_json("forgejo", &comment.user),
            "canEdit": comment.user.login == current_login,
            "provider": provider_json(
                pr,
                &comment,
                "review",
                thread_id,
                Some(resolved),
                in_reply_to_id,
            ),
            "createdAt": parse_time(&comment.created_at),
            "updatedAt": parse_time(&comment.updated_at),
            "resolved": resolved,
            "outdated": stale,
            "body": comment.body
        }]
    });
    parse_one(app, data)
}

pub(crate) fn issue_comment_to_oyo(
    app: &App,
    pr: &ProviderPr,
    current_login: &str,
    comment: FjIssueComment,
) -> Result<ReviewComment> {
    let data = serde_json::json!({
        "version": 1,
        "comments": [{
            "file": pr.title,
            "kind": "pr",
            "author": author_json("forgejo", &comment.user),
            "canEdit": comment.user.login == current_login,
            "provider": issue_provider_json(pr, &comment),
            "createdAt": parse_time(&comment.created_at),
            "updatedAt": parse_time(&comment.updated_at),
            "body": comment.body
        }]
    });
    parse_one(app, data)
}

pub(crate) fn inline_comment_body(comment: &ReviewComment) -> Result<serde_json::Value> {
    let anchor = &comment.anchor;
    let side = anchor.side.unwrap_or_else(|| {
        if anchor.new_range.is_some() {
            ReviewSide::New
        } else {
            ReviewSide::Old
        }
    });
    let line = match side {
        ReviewSide::Old => anchor.old_range,
        ReviewSide::New => anchor.new_range,
    }
    .map(|range| range.end)
    .ok_or_else(|| {
        anyhow!(
            "Comment {} has no {}-side line anchor",
            comment.id,
            side.as_str()
        )
    })?;
    let mut body = serde_json::json!({
        "path": anchor.file_path,
        "body": comment.body,
    });
    match side {
        ReviewSide::Old => body["old_position"] = serde_json::json!(line),
        ReviewSide::New => body["new_position"] = serde_json::json!(line),
    }
    Ok(body)
}

fn create_inline_comment(
    pr: &ProviderPr,
    user: &ProviderUser,
    comment: &ReviewComment,
) -> Result<ReviewProviderComment> {
    let review: FjReview = cb_json(
        &pr.host,
        "POST",
        &format!("/repos/{}/pulls/{}/reviews", pr.repo, pr.number),
        Some(serde_json::json!({
            "body": "",
            "event": "COMMENT",
            "commit_id": pr.head_commit,
            "comments": [inline_comment_body(comment)?],
        })),
    )?;
    let comments: Vec<FjReviewComment> = cb_json(
        &pr.host,
        "GET",
        &format!(
            "/repos/{}/pulls/{}/reviews/{}/comments",
            pr.repo, pr.number, review.id
        ),
        None,
    )?;
    let created = comments
        .into_iter()
        .max_by_key(|comment| comment.id)
        .ok_or_else(|| anyhow!("Forgejo returned no created review comment."))?;
    let thread_id = format!("review:{}:{}", review.id, created.id);
    Ok(provider_link(
        pr,
        user,
        &created,
        "review",
        Some(thread_id),
        Some(created.resolver.is_some()),
        None,
    ))
}

fn create_review_comment(
    pr: &ProviderPr,
    review_id: u64,
    path: &str,
    old_position: usize,
    new_position: usize,
    body: &str,
) -> Result<FjReviewComment> {
    let mut data = serde_json::json!({ "path": path, "body": body });
    if new_position > 0 {
        data["new_position"] = serde_json::json!(new_position);
    } else if old_position > 0 {
        data["old_position"] = serde_json::json!(old_position);
    } else {
        anyhow::bail!("Forgejo review comment has no position.");
    }
    cb_json(
        &pr.host,
        "POST",
        &format!(
            "/repos/{}/pulls/{}/reviews/{review_id}/comments",
            pr.repo, pr.number
        ),
        Some(data),
    )
}

fn create_issue_comment(
    pr: &ProviderPr,
    user: &ProviderUser,
    body: &str,
) -> Result<ReviewProviderComment> {
    let comment: FjIssueComment = cb_json(
        &pr.host,
        "POST",
        &format!("/repos/{}/issues/{}/comments", pr.repo, pr.number),
        Some(serde_json::json!({ "body": body })),
    )?;
    Ok(issue_provider_link(pr, user, &comment))
}

fn parse_one(app: &App, data: serde_json::Value) -> Result<ReviewComment> {
    app.parse_review_comments_json_for_sync(&data.to_string())
        .map_err(|error| anyhow!(error))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Provider comment did not map to this diff."))
}

fn author_json(provider: &str, author: &FjUser) -> serde_json::Value {
    let mut usernames = BTreeMap::new();
    usernames.insert(provider.to_string(), author.login.clone());
    serde_json::json!({
        "name": author
            .full_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| author.login.clone()),
        "usernames": usernames,
        "avatarUrl": author.avatar_url,
    })
}

fn provider_json(
    pr: &ProviderPr,
    comment: &FjReviewComment,
    api_kind: &str,
    thread_id: Option<String>,
    thread_resolved: Option<bool>,
    in_reply_to_id: Option<String>,
) -> serde_json::Value {
    serde_json::json!({
        "provider": "forgejo",
        "remote": pr.remote,
        "repo": pr.repo,
        "prNumber": pr.number,
        "commentId": comment.id.to_string(),
        "inReplyToId": in_reply_to_id,
        "threadId": thread_id,
        "threadResolved": thread_resolved,
        "authorUsername": comment.user.login,
        "prTitle": pr.title,
        "prUrl": pr.url,
        "apiKind": api_kind,
        "syncState": "clean"
    })
}

fn issue_provider_json(pr: &ProviderPr, comment: &FjIssueComment) -> serde_json::Value {
    serde_json::json!({
        "provider": "forgejo",
        "remote": pr.remote,
        "repo": pr.repo,
        "prNumber": pr.number,
        "commentId": comment.id.to_string(),
        "authorUsername": comment.user.login,
        "prTitle": pr.title,
        "prUrl": pr.url,
        "apiKind": "issue",
        "syncState": "clean"
    })
}

#[allow(clippy::too_many_arguments)]
fn provider_link(
    pr: &ProviderPr,
    user: &ProviderUser,
    comment: &FjReviewComment,
    api_kind: &str,
    thread_id: Option<String>,
    thread_resolved: Option<bool>,
    in_reply_to_id: Option<String>,
) -> ReviewProviderComment {
    ReviewProviderComment {
        provider: "forgejo".to_string(),
        remote: pr.remote.clone(),
        repo: pr.repo.clone(),
        pr_number: pr.number,
        comment_id: comment.id.to_string(),
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

fn issue_provider_link(
    pr: &ProviderPr,
    user: &ProviderUser,
    comment: &FjIssueComment,
) -> ReviewProviderComment {
    ReviewProviderComment {
        provider: "forgejo".to_string(),
        remote: pr.remote.clone(),
        repo: pr.repo.clone(),
        pr_number: pr.number,
        comment_id: comment.id.to_string(),
        in_reply_to_id: None,
        thread_id: None,
        thread_resolved: None,
        resolved_dirty: false,
        author_username: Some(user.login.clone()),
        pr_title: Some(pr.title.clone()),
        pr_url: Some(pr.url.clone()),
        api_kind: "issue".to_string(),
        sync_state: "clean".to_string(),
    }
}

fn review_id(provider: &ReviewProviderComment) -> Result<u64> {
    provider
        .thread_id
        .as_deref()
        .and_then(|thread| thread.strip_prefix("review:"))
        .and_then(|thread| thread.split(':').next())
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| anyhow!("Forgejo review comment is missing its review id."))
}

fn cb_json<T: DeserializeOwned>(
    host: &str,
    method: &str,
    endpoint: &str,
    body: Option<serde_json::Value>,
) -> Result<T> {
    let data = cb_output(host, method, endpoint, body)?;
    serde_json::from_str(&data).with_context(|| format!("Invalid Forgejo response from {endpoint}"))
}

fn cb_no_output(
    host: &str,
    method: &str,
    endpoint: &str,
    body: Option<serde_json::Value>,
) -> Result<()> {
    cb_output(host, method, endpoint, body).map(|_| ())
}

fn cb_output(
    host: &str,
    method: &str,
    endpoint: &str,
    body: Option<serde_json::Value>,
) -> Result<String> {
    let token = forgejo_token(host)?;
    let mut config = format!(
        "silent\nshow-error\nfail-with-body\nurl = \"{}\"\nrequest = \"{}\"\nheader = \"Accept: application/json\"\nheader = \"Authorization: token {}\"\n",
        curl_config_escape(&format!("https://{host}/api/v1{endpoint}")),
        curl_config_escape(method),
        curl_config_escape(&token),
    );
    if let Some(body) = body {
        config.push_str("header = \"Content-Type: application/json\"\n");
        config.push_str(&format!(
            "data = \"{}\"\n",
            curl_config_escape(&body.to_string())
        ));
    }
    let mut command = ProcessCommand::new("curl");
    command.arg("--config").arg("-");
    run_output_with_stdin(command, &config)
}

pub(crate) fn has_forgejo_token(host: &str) -> bool {
    forgejo_token(host).is_ok()
}

fn forgejo_token(host: &str) -> Result<String> {
    let token = if host == "codeberg.org" {
        let path = dirs::config_dir()
            .ok_or_else(|| anyhow!("No config directory found."))?
            .join("codeberg-cli/config.toml");
        let config: toml::Value = toml::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?,
        )?;
        config
            .get("token")
            .and_then(toml::Value::as_str)
            .map(str::to_string)
    } else {
        let path = dirs::data_local_dir()
            .ok_or_else(|| anyhow!("No local data directory found."))?
            .join("forgejo-cli/keys.json");
        let keys: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?,
        )?;
        keys.pointer(&format!("/hosts/{}/token", json_pointer_escape(host)))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    token
        .filter(|token| !token.is_empty() && !token.chars().any(char::is_control))
        .ok_or_else(|| anyhow!("No valid Forgejo token found for {host}."))
}

fn json_pointer_escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn curl_config_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn cb_pages<T: DeserializeOwned>(host: &str, endpoint: &str) -> Result<Vec<T>> {
    let mut items = Vec::new();
    for page in 1.. {
        let separator = if endpoint.contains('?') { '&' } else { '?' };
        let path = format!("{endpoint}{separator}page={page}&limit={PAGE_SIZE}");
        let mut batch: Vec<T> = cb_json(host, "GET", &path, None)?;
        let done = batch.len() < PAGE_SIZE;
        items.append(&mut batch);
        if done {
            break;
        }
    }
    Ok(items)
}

fn can_delete_empty_review(review: &FjReview) -> bool {
    review.state == "COMMENT" && review.body.trim().is_empty()
}

fn ignore_delete_not_found(result: Result<()>) -> Result<()> {
    match result {
        Err(error) if is_not_found(&error) => Ok(()),
        result => result,
    }
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error.to_string().lines().any(|line| {
        let line = line.trim().to_ascii_lowercase();
        (line.starts_with("curl:") && line.contains("404"))
            || line.starts_with("404 ")
            || line.contains("clienterror: 404 ")
            || line.contains("http 404")
            || line.contains("404 not found")
    })
}

fn parse_time(value: &str) -> u64 {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .and_then(|date| u64::try_from(date.unix_timestamp()).ok())
        .unwrap_or(0)
}

fn parse_pr_number(target: &str) -> Option<u64> {
    let value = target.strip_prefix('#').unwrap_or(target);
    (!value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ViewMode;
    use oyo_core::MultiFileDiff;
    use std::path::PathBuf;

    fn test_pr() -> ProviderPr {
        ProviderPr {
            provider: ReviewProviderKind::Forgejo,
            remote: "origin".to_string(),
            host: "codeberg.org".to_string(),
            repo: "owner/repo".to_string(),
            number: 1,
            title: "Review".to_string(),
            url: "https://codeberg.org/owner/repo/pulls/1".to_string(),
            base_branch: "main".to_string(),
            head_branch: "feature".to_string(),
            base_commit: "base".to_string(),
            start_commit: None,
            head_commit: "head".to_string(),
        }
    }

    fn test_app() -> App {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("app.py"),
            PathBuf::from("app.py"),
            String::new(),
            "one\ntwo\nthree\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app
    }

    fn review_comment(id: u64, body: &str, resolver: bool) -> FjReviewComment {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "body": body,
            "user": { "login": "reviewer", "full_name": "Reviewer" },
            "resolver": resolver.then(|| serde_json::json!({ "login": "maintainer" })),
            "pull_request_review_id": 10,
            "created_at": "2026-07-11T11:38:35+02:00",
            "updated_at": "2026-07-11T11:38:35+02:00",
            "path": "app.py",
            "position": 2,
            "original_position": 0
        }))
        .unwrap()
    }

    #[test]
    fn curl_config_values_cannot_add_options() {
        let escaped = curl_config_escape("token\nheader = \"Injected: yes\"");
        assert!(!escaped.contains('\n'));
        assert!(escaped.contains("\\n"));
        assert!(escaped.contains("\\\"Injected: yes\\\""));
    }

    #[test]
    fn parses_pull_request_numbers() {
        assert_eq!(parse_pr_number("#12"), Some(12));
        assert_eq!(parse_pr_number("12"), Some(12));
        assert_eq!(parse_pr_number("feature"), None);
    }

    #[test]
    fn same_line_comments_round_trip_as_a_resolved_thread() {
        let comments = review_comments_to_oyo(
            &test_app(),
            &test_pr(),
            "reviewer",
            vec![FjReviewComments {
                review: FjReview {
                    id: 10,
                    stale: false,
                    state: "COMMENT".to_string(),
                    body: String::new(),
                },
                comments: vec![
                    review_comment(20, "root", false),
                    review_comment(21, "reply", true),
                ],
            }],
        );
        let comments = comments.into_iter().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(comments.len(), 2);
        assert!(comments.iter().all(|comment| comment.resolved));
        assert_eq!(
            comments[0].provider.as_ref().unwrap().thread_id.as_deref(),
            Some("review:10:20")
        );
        assert_eq!(
            comments[1]
                .provider
                .as_ref()
                .unwrap()
                .in_reply_to_id
                .as_deref(),
            Some("20")
        );
    }

    #[test]
    fn only_empty_comment_reviews_are_cleanup_candidates() {
        let mut review = FjReview {
            id: 10,
            stale: false,
            state: "COMMENT".to_string(),
            body: String::new(),
        };
        assert!(can_delete_empty_review(&review));
        review.state = "APPROVED".to_string();
        assert!(!can_delete_empty_review(&review));
        review.state = "COMMENT".to_string();
        review.body = "Summary".to_string();
        assert!(!can_delete_empty_review(&review));
        assert!(is_not_found(&anyhow!("404 The target couldn't be found.")));
        assert!(is_not_found(&anyhow!(
            "curl: (22) The requested URL returned error: 404"
        )));
        assert!(!is_not_found(&anyhow!("Comment 404 is invalid.")));
    }

    #[test]
    fn maps_pull_request_conversation_comments() {
        let comment: FjIssueComment = serde_json::from_value(serde_json::json!({
            "id": 30,
            "body": "conversation",
            "user": { "login": "reviewer" },
            "created_at": "2026-07-11T11:38:35+02:00",
            "updated_at": "2026-07-11T11:38:35+02:00"
        }))
        .unwrap();
        let comment = issue_comment_to_oyo(&test_app(), &test_pr(), "reviewer", comment).unwrap();
        assert_eq!(comment.anchor.kind, ReviewTargetKind::PullRequest);
        assert_eq!(comment.provider.as_ref().unwrap().api_kind, "issue");
    }

    #[test]
    fn resolve_reports_the_forgejo_api_gap() {
        let pr = test_pr();
        let user = ProviderUser {
            login: "reviewer".to_string(),
            avatar_url: None,
        };
        let mut ops = ForgejoPushOps {
            pr: &pr,
            user: &user,
        };
        let error = ops.set_thread_resolved("review:10:20", true).unwrap_err();
        assert!(error.to_string().contains("read-only"));
        assert!(error.to_string().contains("no thread resolve endpoint"));
    }

    #[test]
    fn inline_payload_uses_only_the_declared_side() {
        let app = test_app();
        let mut comment = app
            .parse_review_comments_json_for_sync(
                r#"{"version":1,"comments":[{"file":"app.py","kind":"line","side":"new","oldRange":{"start":1,"end":1},"newRange":{"start":2,"end":2},"body":"note"}]}"#,
            )
            .unwrap()
            .remove(0);
        let body = inline_comment_body(&comment).unwrap();
        assert_eq!(body["new_position"], 2);
        assert!(body.get("old_position").is_none());

        comment.anchor.side = Some(ReviewSide::Old);
        let body = inline_comment_body(&comment).unwrap();
        assert_eq!(body["old_position"], 1);
        assert!(body.get("new_position").is_none());
    }
}
