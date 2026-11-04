use crate::{
    provider_adapter, provider_pr, provider_whoami, App, ProviderPr, ReviewComment,
    ReviewProviderComment, ReviewProviderPushOps, ReviewRemote, ReviewSyncAction, ReviewSyncWorker,
    ReviewSyncWorkerResult,
};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Debug)]
pub(crate) enum WriteFailure {
    NotAttempted(anyhow::Error),
    MayHaveSucceeded(anyhow::Error),
}

impl std::fmt::Display for WriteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAttempted(error) | Self::MayHaveSucceeded(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for WriteFailure {}

#[derive(Clone)]
pub(crate) struct RemoteReviewContext {
    snapshot: Value,
    target: crate::remote::RemoteTarget,
    journal: Option<std::path::PathBuf>,
    generation: u64,
}

impl RemoteReviewContext {
    pub(crate) fn from_app(app: &App) -> Result<Self> {
        let snapshot = app
            .review_live_diff()
            .remote_snapshot
            .clone()
            .ok_or_else(|| anyhow!("Remote review snapshot is unavailable"))?;
        if snapshot["acquisition"] == "patch-v1"
            && !app
                .remote_validation
                .as_ref()
                .is_some_and(|proof| proof.matches(&snapshot))
        {
            bail!("Remote snapshot is incomplete or unvalidated. Wait for validation or refresh. Drafts were kept.");
        }
        let provider = match snapshot["provider"].as_str() {
            Some("github") => crate::ReviewProviderKind::GitHub,
            Some("gitlab") => crate::ReviewProviderKind::GitLab,
            Some("forgejo") => crate::ReviewProviderKind::Forgejo,
            _ => bail!("Remote review provider is unavailable"),
        };
        let target = crate::remote::RemoteTarget::parse_with_provider(
            snapshot["target"]
                .as_str()
                .ok_or_else(|| anyhow!("Remote review identity is unavailable"))?,
            Some(provider),
        )?;
        if !matches!(target.kind, crate::remote::RemoteTargetKind::Review(_)) {
            bail!("Provider sync requires a review URL. Commit and comparison URLs have no review identity.");
        }
        let journal = app
            .review_paths()
            .db_file
            .map(|path| path.with_file_name("provider-sync.db"));
        Ok(Self {
            snapshot,
            target,
            journal,
            generation: app.remote_sync_generation(),
        })
    }

    pub(crate) fn matches_app(&self, app: &App) -> bool {
        app.remote_sync_generation() == self.generation
            && app.review_live_diff().remote_snapshot.as_ref() == Some(&self.snapshot)
    }

    pub(crate) fn recover_receipts(&self, app: &mut App) -> Result<()> {
        let Some(path) = self.journal.as_ref().filter(|path| path.exists()) else {
            return Ok(());
        };
        let conn = rusqlite::Connection::open(path)?;
        let mut stmt = conn.prepare(
            "SELECT operation_key, submitted, receipt FROM mutations WHERE receipt IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (key, submitted, receipt) = row?;
            let key: Value = serde_json::from_str(&key)?;
            if key[0].as_str() != Some(self.target.canonical_url().as_str()) {
                continue;
            }
            let submitted = serde_json::from_str(&submitted)?;
            let provider = serde_json::from_str(&receipt)?;
            app.apply_review_comment_receipt(&submitted, provider);
        }
        Ok(())
    }

    fn provider_pr(&self) -> Result<ProviderPr> {
        let crate::remote::RemoteTargetKind::Review(number) = self.target.kind else {
            bail!("Provider sync requires a review URL");
        };
        provider_pr(
            &ReviewRemote {
                name: self.target.host.clone(),
                provider: self.target.provider,
                host: self.target.host.clone(),
                repo: self.target.repo.clone(),
            },
            Some(&number.to_string()),
        )
    }

    fn verify_revision(&self, canceled: &AtomicBool) -> Result<()> {
        if canceled.load(Ordering::Acquire) {
            bail!("Review sync canceled; unpublished drafts were kept");
        }
        if !provider_adapter(self.target.provider)?
            .remote_snapshot_matches(&self.target, &self.snapshot)?
        {
            bail!("The provider revision changed since this diff was loaded. Refresh and review the new revision before pushing. Drafts were kept.");
        }
        if canceled.load(Ordering::Acquire) {
            bail!("Review sync canceled; unpublished drafts were kept");
        }
        Ok(())
    }

    fn verify_new_comments(&self, comments: &[ReviewComment]) -> Result<()> {
        for comment in comments {
            let content_pending = comment.can_edit
                && match comment.provider.as_ref() {
                    None => !comment.deleted && comment.in_reply_to.is_none(),
                    Some(provider) if provider.comment_id.is_empty() => !comment.deleted,
                    Some(provider) => comment.deleted || provider.sync_state == "dirty",
                };
            let resolution_pending = !comment.deleted
                && comment.provider.as_ref().is_some_and(|provider| {
                    provider.api_kind == "review"
                        && provider.thread_id.is_some()
                        && provider
                            .thread_resolved
                            .is_some_and(|remote| remote != comment.resolved)
                });
            for (pending, reviewed) in [
                (content_pending, comment.pending_snapshot.as_ref()),
                (resolution_pending, comment.resolution_snapshot.as_ref()),
            ] {
                if pending
                    && !reviewed.is_some_and(|snapshot| {
                        crate::remote::same_review_snapshot(snapshot, &self.snapshot)
                    })
                {
                    bail!("Comment {} has pending changes from a different reviewed revision or no reviewed version. Refresh, review the change and save the pending edit or toggle its resolution again before pushing. Drafts were kept.", comment.id);
                }
            }
        }
        for comment in comments
            .iter()
            .filter(|c| c.provider.is_none() && !c.deleted && c.in_reply_to.is_none())
        {
            if matches!(
                comment.anchor.kind,
                crate::app::review::ReviewTargetKind::PullRequest
            ) {
                continue;
            }
            let snapshot = comment
                .anchor
                .snapshot
                .as_ref()
                .and_then(|s| s.target.as_ref())
                .and_then(|t| t.remote_snapshot.as_ref());
            if comment.outdated
                || !snapshot.is_some_and(|snapshot| {
                    crate::remote::same_review_snapshot(snapshot, &self.snapshot)
                })
            {
                bail!("Comment {} belongs to an older or unavailable snapshot. Re-anchor it against the displayed revision before pushing. Drafts were kept.", comment.id);
            }
        }
        Ok(())
    }
}

struct ReviewedPushOps<'a> {
    inner: Box<dyn ReviewProviderPushOps + 'a>,
    context: &'a RemoteReviewContext,
    canceled: &'a AtomicBool,
    selected: Option<ReviewComment>,
}

impl ReviewedPushOps<'_> {
    fn mutate(
        &mut self,
        operation: &str,
        execute: impl FnOnce(&mut dyn ReviewProviderPushOps) -> Result<ReviewProviderComment>,
    ) -> Result<ReviewProviderComment> {
        self.context.verify_revision(self.canceled)?;
        let comment = self
            .selected
            .as_ref()
            .ok_or_else(|| anyhow!("Missing pending operation"))?;
        let path = self
            .context
            .journal
            .as_ref()
            .ok_or_else(|| anyhow!("Publishing requires persistent review storage"))?;
        let conn = rusqlite::Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS mutations (operation_key TEXT PRIMARY KEY, submitted TEXT NOT NULL, receipt TEXT);",
        )?;
        let create = matches!(operation, "create" | "reply");
        let key = serde_json::to_string(&(
            self.context.target.canonical_url(),
            &comment.local_key,
            operation,
        ))?;
        use rusqlite::OptionalExtension;
        let receipt: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT submitted, receipt FROM mutations WHERE operation_key = ?1",
                [&key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((submitted, receipt)) = receipt {
            return match receipt {
                Some(receipt) => {
                    let provider = serde_json::from_str(&receipt)?;
                    let submitted: ReviewComment = serde_json::from_str(&submitted)?;
                    if submitted.body == comment.body {
                        Ok(provider)
                    } else {
                        self.inner.update(&provider, &comment.body)
                    }
                },
                None => Err(anyhow!("Comment {} has an uncertain publication outcome. Pull and inspect the provider before retrying; automatic reposting is blocked to prevent duplicates.", comment.id)),
            };
        }
        let review_db = rusqlite::Connection::open_with_flags(
            path.with_file_name("review.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let saved: Option<String> = review_db.query_row(
            "SELECT comment_json FROM comments WHERE json_extract(comment_json, '$.localKey') = ?1",
            [&comment.local_key],
            |row| row.get(0),
        ).optional()?;
        let saved: ReviewComment = saved
            .map(|saved| serde_json::from_str(&saved))
            .transpose()?
            .ok_or_else(|| anyhow!("Draft identity was not saved. Check review storage and save the draft before pushing."))?;
        if saved.deleted
            || saved.body != comment.body
            || saved.pending_snapshot != comment.pending_snapshot
        {
            bail!("The saved draft changed during publication. Check the draft and push again.");
        }
        conn.execute(
            "INSERT INTO mutations (operation_key, submitted, receipt) VALUES (?1, ?2, NULL)",
            rusqlite::params![&key, serde_json::to_string(comment)?],
        )?;
        match execute(self.inner.as_mut()) {
            Ok(provider) => {
                conn.execute(
                    "UPDATE mutations SET receipt = ?2 WHERE operation_key = ?1",
                    rusqlite::params![key, serde_json::to_string(&provider)?],
                )?;
                Ok(provider)
            }
            Err(error) => {
                let message = error.to_string();
                let explicit_rejection = [
                    "HTTP 400", "HTTP 401", "HTTP 403", "HTTP 404", "HTTP 409", "HTTP 422",
                    "HTTP 429",
                ]
                .iter()
                .any(|status| {
                    message.contains(status)
                        || message.contains(&format!("returned error: {}", &status[5..]))
                });
                let retry_safe = match error.downcast_ref::<WriteFailure>() {
                    Some(WriteFailure::NotAttempted(_)) => true,
                    Some(WriteFailure::MayHaveSucceeded(_)) => false,
                    None => explicit_rejection,
                };
                if retry_safe || !create {
                    conn.execute("DELETE FROM mutations WHERE operation_key = ?1", [&key])?;
                }
                Err(error)
            }
        }
    }
}

impl ReviewProviderPushOps for ReviewedPushOps<'_> {
    fn select_comment(&mut self, comment: &ReviewComment) {
        self.selected = Some(comment.clone());
    }
    fn provider_label(&self) -> &'static str {
        self.inner.provider_label()
    }
    fn create_root(&mut self, comment: &ReviewComment) -> Result<ReviewProviderComment> {
        self.mutate("create", |inner| inner.create_root(comment))
    }
    fn create_reply(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment> {
        self.mutate("reply", |inner| inner.create_reply(provider, body))
    }
    fn update(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment> {
        self.context.verify_revision(self.canceled)?;
        self.inner.update(provider, body)
    }
    fn delete(&mut self, provider: &ReviewProviderComment) -> Result<()> {
        self.context.verify_revision(self.canceled)?;
        self.inner.delete(provider)
    }
    fn set_thread_resolved(&mut self, thread_id: &str, resolved: bool) -> Result<()> {
        self.context.verify_revision(self.canceled)?;
        self.inner.set_thread_resolved(thread_id, resolved)
    }
}

pub(crate) fn spawn(
    action: ReviewSyncAction,
    context: RemoteReviewContext,
    comments: Vec<ReviewComment>,
) -> ReviewSyncWorker {
    let (tx, rx) = std::sync::mpsc::channel();
    let canceled = Arc::new(AtomicBool::new(false));
    let worker = ReviewSyncWorker {
        rx,
        remote_context: Some(context.clone()),
        canceled: canceled.clone(),
    };
    std::thread::spawn(move || {
        let result = (|| {
            let pr = context.provider_pr()?;
            let user = provider_whoami(&pr)?;
            if canceled.load(Ordering::Acquire) {
                bail!("Review sync canceled");
            }
            if action == ReviewSyncAction::Push {
                if context.snapshot["afterRevision"].as_str() != Some(pr.head_commit.as_str()) {
                    bail!("The provider head changed. Refresh and review the new revision before pushing. Drafts were kept.");
                }
                let base_matches = match pr.provider {
                    crate::ReviewProviderKind::GitLab => {
                        context.snapshot["beforeRevision"].as_str() == Some(pr.base_commit.as_str())
                            && context.snapshot["reviewVersion"]["start_sha"].as_str()
                                == pr.start_commit.as_deref()
                    }
                    _ => {
                        context.snapshot["reviewVersion"]["baseTip"].as_str()
                            == Some(pr.base_commit.as_str())
                    }
                };
                if !base_matches {
                    bail!("The provider base changed. Refresh and review the new revision before pushing. Drafts were kept.");
                }
                context.verify_revision(&canceled)?;
                context.verify_new_comments(&comments)?;
                let mut ops = ReviewedPushOps {
                    inner: provider_adapter(pr.provider)?.push_ops(&pr, &user),
                    context: &context,
                    canceled: &canceled,
                    selected: None,
                };
                let outcome =
                    crate::push_review_comments_to_provider_with_ops(comments, &pr, &mut ops)?;
                drop(ops);
                Ok(ReviewSyncWorkerResult::Push {
                    action,
                    pr: Box::new(pr),
                    user,
                    outcome,
                })
            } else {
                let data = crate::fetch_provider_comments_for_pull(pr, user)?;
                Ok(ReviewSyncWorkerResult::Pull {
                    action,
                    data: Box::new(data),
                })
            }
        })();
        let _ = tx.send(result);
    });
    worker
}

pub(crate) fn run_cli(
    action: ReviewSyncAction,
    url: &str,
    json: bool,
    config: &crate::config::Config,
    args: &crate::Args,
) -> Result<()> {
    let mut app = crate::review_app_for_target(config, args, Some(url), true)?;
    let context = RemoteReviewContext::from_app(&app)?;
    context.recover_receipts(&mut app)?;
    let worker = spawn(action, context, app.review_comments_for_sync());
    let result = worker
        .rx
        .recv()
        .map_err(|_| anyhow!("Review sync worker stopped"))??;
    let output = match result {
        ReviewSyncWorkerResult::Pull { data, .. } => {
            let pr = data.pr().clone();
            app.set_review_pull_request_target(Some(crate::review_pull_request_target(&pr)));
            crate::set_review_pr_target_metadata(&mut app, &pr);
            let stats = crate::apply_provider_comments_to_app(&mut app, *data)?;
            serde_json::json!({"ok": true, "pulled": stats.pulled, "skipped": stats.skipped})
        }
        ReviewSyncWorkerResult::Push { outcome, .. } => {
            let stats = crate::apply_push_outcome_to_app(&mut app, outcome);
            serde_json::json!({"ok": stats.warnings.is_empty(), "created": stats.created, "updated": stats.updated, "deleted": stats.deleted, "threadsUpdated": stats.threads_updated, "warnings": stats.warnings})
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::review_json_with_target(&app, output.clone()))?
        );
    } else if action == ReviewSyncAction::Pull {
        println!("Pulled {} comments.", output["pulled"]);
    } else {
        println!(
            "Pushed {} created, {} updated and {} deleted comments; {} thread updates.",
            output["created"], output["updated"], output["deleted"], output["threadsUpdated"]
        );
    }
    if output["ok"] == false {
        bail!(
            "Some changes could not be published. Drafts were kept: {}",
            output["warnings"]
        );
    }
    Ok(())
}
