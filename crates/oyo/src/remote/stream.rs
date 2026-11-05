use super::identity::PreparedReview;
use super::*;
use oyo_core::patch::{read_patch, terminal_text, PatchRecord};
use std::io::{BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct ValidatedSnapshot {
    descriptor: Value,
}

impl ValidatedSnapshot {
    pub(crate) fn matches(&self, descriptor: &Value) -> bool {
        &self.descriptor == descriptor
    }

    pub(super) fn legacy(descriptor: Value) -> Arc<Self> {
        Self::verified(descriptor)
    }

    pub(super) fn verified(descriptor: Value) -> Arc<Self> {
        Arc::new(Self { descriptor })
    }
}

pub(crate) enum RemoteEvent {
    Files(Vec<PatchRecord>),
    Complete(Arc<ValidatedSnapshot>),
    Failed(String),
}

struct EventEnvelope {
    generation: u64,
    sequence: u64,
    event: RemoteEvent,
}

pub(crate) struct PatchSession {
    rx: mpsc::Receiver<EventEnvelope>,
    context: loading::ReadContext,
    identity: PreparedReview,
    generation: u64,
    sequence: u64,
}

impl PatchSession {
    pub(crate) fn try_recv(&mut self) -> std::result::Result<RemoteEvent, mpsc::TryRecvError> {
        let envelope = self.rx.try_recv()?;
        if envelope.generation != self.generation
            || (!matches!(&envelope.event, RemoteEvent::Failed(_))
                && envelope.sequence != self.sequence)
        {
            self.context.cancel();
            return Ok(RemoteEvent::Failed(
                "Remote event belongs to a different snapshot generation".to_owned(),
            ));
        }
        self.sequence += 1;
        Ok(envelope.event)
    }

    pub(crate) fn append(&self, diff: &mut MultiFileDiff, patch: PatchRecord) {
        self.identity.append(diff, patch);
    }
}

impl Drop for PatchSession {
    fn drop(&mut self) {
        self.context.cancel();
    }
}

static PATCH_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub(crate) fn set_patch_enabled(enabled: bool) {
    PATCH_ENABLED.store(enabled, Ordering::Relaxed);
}

pub(super) fn patch_enabled() -> bool {
    PATCH_ENABLED.load(Ordering::Relaxed)
}

pub(crate) fn github_streaming(host: &str) -> bool {
    if !PATCH_ENABLED.load(Ordering::Relaxed) {
        return false;
    }
    if super::github::ReadTransport::for_host(host) == super::github::ReadTransport::PublicHttps {
        return true;
    }
    static CAPABILITY: OnceLock<bool> = OnceLock::new();
    *CAPABILITY.get_or_init(|| {
        let mut command = Command::new("gh");
        command.args(["api", "--help"]);
        run_command(command, None).is_ok_and(|help| help.contains("--allow-escape-sequences"))
    })
}

fn send_event(
    tx: &mpsc::SyncSender<EventEnvelope>,
    context: &loading::ReadContext,
    mut envelope: EventEnvelope,
) -> Result<()> {
    loop {
        if context.is_cancelled() {
            bail!("Remote loading cancelled");
        }
        match tx.try_send(envelope) {
            Ok(()) => return Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => bail!("Remote review closed"),
            Err(mpsc::TrySendError::Full(value)) => {
                envelope = value;
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn patch_transport(
    identity: &PreparedReview,
    tx: mpsc::SyncSender<EventEnvelope>,
    context: loading::ReadContext,
    generation: u64,
) -> Result<(Vec<FileEntry>, u64)> {
    let (command, transport) = identity.request();
    patch_response_command(
        command,
        identity.expected_files(),
        tx,
        context,
        generation,
        transport,
    )
}

#[cfg(test)]
fn patch_command(
    command: Command,
    expected_files: usize,
    tx: mpsc::SyncSender<EventEnvelope>,
    context: loading::ReadContext,
    generation: u64,
) -> Result<(Vec<FileEntry>, u64)> {
    patch_response_command(
        command,
        expected_files,
        tx,
        context,
        generation,
        super::http::ResponseFormat::Body,
    )
}

fn patch_response_command(
    mut command: Command,
    expected_files: usize,
    tx: mpsc::SyncSender<EventEnvelope>,
    context: loading::ReadContext,
    generation: u64,
    transport: super::http::ResponseFormat,
) -> Result<(Vec<FileEntry>, u64)> {
    let _permit = loading::TransportPermit::acquire()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = loading::RegisteredChild::spawn(&mut command)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Missing patch stream"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Missing patch diagnostics"))?;
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(65537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let parser_context = context.clone();
    let parser = std::thread::spawn(move || -> Result<(Vec<FileEntry>, u64)> {
        let mut files = Vec::new();
        let mut sequence = 0;
        let mut pending = Vec::new();
        let mut pending_bytes = 0usize;
        let mut useful_sent = false;
        read_patch(
            &mut super::http::ResponseBody::new(BufReader::new(stdout), transport),
            |patch| {
                if files.len() >= expected_files || files.len() >= 100000 {
                    return Err(
                        "Remote patch exceeds its authoritative file count or loading limit".into(),
                    );
                }
                let first_useful = !useful_sent && patch.has_changed_lines();
                files.push(patch.entry.clone());
                pending_bytes += patch.byte_len();
                pending.push(patch);
                if first_useful || pending.len() >= 64 || pending_bytes >= 1024 * 1024 {
                    send_event(
                        &tx,
                        &parser_context,
                        EventEnvelope {
                            generation,
                            sequence,
                            event: RemoteEvent::Files(std::mem::take(&mut pending)),
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    sequence += 1;
                    pending_bytes = 0;
                    useful_sent |= first_useful;
                }
                Ok(())
            },
        )
        .map_err(|e| anyhow!(e.to_string()))?;
        if !pending.is_empty() {
            send_event(
                &tx,
                &parser_context,
                EventEnvelope {
                    generation,
                    sequence,
                    event: RemoteEvent::Files(pending),
                },
            )?;
            sequence += 1;
        }
        Ok((files, sequence))
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut status = None;
    let mut parser = Some(parser);
    let mut parsed = None;
    loop {
        if parser.as_ref().is_some_and(|parser| parser.is_finished()) {
            parsed = Some(
                parser
                    .take()
                    .unwrap()
                    .join()
                    .map_err(|_| anyhow!("Patch reader stopped"))?,
            );
            if parsed.as_ref().is_some_and(|result| result.is_err()) {
                child.stop();
            }
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
        if parsed.is_some() && errors.is_finished() && status.is_some() {
            break;
        }
        if context.is_cancelled() || Instant::now() >= deadline {
            context.cancel();
            child.stop();
            bail!("Remote patch cancelled or timed out after 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let parsed = parsed.ok_or_else(|| anyhow!("Patch reader stopped"))??;
    let errors = errors
        .join()
        .map_err(|_| anyhow!("Patch diagnostics stopped"))??;
    if !status.is_some_and(|s| s.success()) {
        bail!(
            "Remote patch request failed: {}",
            terminal_text(String::from_utf8_lossy(&errors).trim())
        );
    }
    Ok(parsed)
}

pub(crate) fn start(
    url: &str,
    context: loading::ReadContext,
) -> Result<(BuiltDiff, Option<PatchSession>)> {
    let target = RemoteTarget::parse(url)?;
    if !matches!(target.kind, RemoteTargetKind::Review(_)) {
        return Ok((super::build_legacy(url)?, None));
    }
    let Some(identity) = provider_adapter(target.provider)?.prepare_review_stream(&target)? else {
        return Ok((super::build_legacy(url)?, None));
    };
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::sync_channel(2);
    let worker_context = context.child(false);
    let session_context = worker_context.clone();
    let worker_identity = identity.clone();
    std::thread::spawn(move || {
        worker_context.run(|| {
            let result = patch_transport(
                &worker_identity,
                tx.clone(),
                worker_context.clone(),
                generation,
            )
            .and_then(|(files, sequence)| {
                worker_identity
                    .verify(&files)
                    .map(|proof| (proof, sequence))
            });
            let sequence = match &result {
                Ok((_, n)) => *n,
                Err(_) => u64::MAX,
            };
            let event = match result {
                Ok((proof, _)) => RemoteEvent::Complete(proof),
                Err(error) => RemoteEvent::Failed(terminal_text(&error.to_string())),
            };
            let _ = send_event(
                &tx,
                &worker_context,
                EventEnvelope {
                    generation,
                    sequence,
                    event,
                },
            );
        });
    });
    let mut session = PatchSession {
        rx,
        context: session_context,
        identity,
        generation,
        sequence: 0,
    };
    let mut diff = MultiFileDiff::from_pending_files(None, Vec::new(), true);
    diff.remote_snapshot = Some(session.identity.descriptor(&[], "receiving"));
    let deadline = Instant::now() + Duration::from_secs(35);
    let mut proof = None;
    loop {
        if context.is_cancelled() || Instant::now() >= deadline {
            bail!("Remote loading cancelled or timed out");
        }
        match session.try_recv() {
            Ok(RemoteEvent::Files(patches)) => {
                let mut selected = None;
                for patch in patches {
                    if selected.is_none() && patch.has_changed_lines() {
                        selected = Some(diff.file_count());
                    }
                    session.append(&mut diff, patch);
                }
                if let Some(index) = selected {
                    diff.selected_index = index;
                    break;
                }
            }
            Ok(RemoteEvent::Complete(validated)) => {
                diff.remote_snapshot = Some(validated.descriptor.clone());
                proof = Some(validated);
                break;
            }
            Ok(RemoteEvent::Failed(error)) => bail!("{error}"),
            Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
            Err(mpsc::TryRecvError::Disconnected) => bail!("Remote patch worker stopped"),
        }
    }
    let built = BuiltDiff {
        multi_diff: diff,
        branch: Some(url.to_owned()),
        workspace_root: None,
        remote_validation: proof.clone(),
    };
    Ok((built, if proof.is_some() { None } else { Some(session) }))
}

pub(crate) fn finish(url: &str) -> Result<BuiltDiff> {
    let context = loading::current();
    let (mut built, mut session) = start(url, context.clone())?;
    if let Some(session) = session.as_mut() {
        loop {
            if context.is_cancelled() {
                bail!("Remote loading cancelled");
            }
            match session.try_recv() {
                Ok(RemoteEvent::Files(patches)) => {
                    for patch in patches {
                        session.append(&mut built.multi_diff, patch);
                    }
                }
                Ok(RemoteEvent::Complete(proof)) => {
                    built.multi_diff.remote_snapshot = Some(proof.descriptor.clone());
                    built.remote_validation = Some(proof);
                    break;
                }
                Ok(RemoteEvent::Failed(error)) => bail!("{error}"),
                Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
                Err(mpsc::TryRecvError::Disconnected) => bail!("Remote patch worker stopped"),
            }
        }
    }
    Ok(built)
}

pub(crate) fn apply_proof(diff: &mut MultiFileDiff, proof: &ValidatedSnapshot) {
    diff.remote_snapshot = Some(proof.descriptor.clone());
}

#[cfg(test)]
mod tests {
    use super::super::identity::github_metadata_matches;
    use super::*;
    use serde_json::json;

    #[test]
    fn public_http_parser_error_survives_stopping_a_live_error_response() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'HTTP/2 403\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 1793833862\r\nRetry-After: 90\r\n\r\n'; sleep 60"]);
        let (tx, _rx) = mpsc::sync_channel(2);
        let start = Instant::now();
        let error = patch_response_command(
            command,
            1,
            tx,
            loading::ReadContext::default(),
            1,
            super::super::http::ResponseFormat::Headers(ReviewProviderKind::GitHub),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("rate limit reached")
                && error.contains("1793833862")
                && error.contains("Retry-After: 90"),
            "{error}"
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn metadata_only_identity_check_supports_node_sized_manifests_and_forks() {
        let descriptor = json!({"afterRevision":"head","reviewVersion":{"baseTip":"base"},"beforeRepository":"nodejs/node","afterRepository":"targos/node","beforeRepositoryId":1,"afterRepositoryId":2,"expectedFiles":3420,"expectedAdditions":168778,"expectedDeletions":99119});
        let metadata = json!({"head":{"sha":"head","repo":{"full_name":"targos/node","id":2}},"base":{"sha":"base","repo":{"full_name":"nodejs/node","id":1}},"changed_files":3420,"additions":168778,"deletions":99119});
        assert!(github_metadata_matches(&metadata, &descriptor));
        for path in [
            "/head/sha",
            "/base/sha",
            "/head/repo/full_name",
            "/base/repo/full_name",
            "/head/repo/id",
            "/base/repo/id",
            "/changed_files",
            "/additions",
            "/deletions",
        ] {
            let mut changed = metadata.clone();
            *changed.pointer_mut(path).unwrap() = json!("changed");
            assert!(!github_metadata_matches(&changed, &descriptor), "{path}");
        }
    }

    #[test]
    fn validated_proof_is_bound_to_the_exact_descriptor() {
        let descriptor = json!({"streamState":"complete","files":[{"path":"f"}]});
        let proof = ValidatedSnapshot {
            descriptor: descriptor.clone(),
        };
        assert!(proof.matches(&descriptor));
        let mut changed = descriptor;
        changed["files"] = json!([]);
        assert!(!proof.matches(&changed));
    }
    fn transport_patch() -> &'static str {
        "diff --git a/f b/f\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n"
    }

    fn controlled_transport(script: String, expected: usize) -> Result<(Vec<FileEntry>, u64)> {
        let context = loading::ReadContext::default();
        let (tx, rx) = mpsc::sync_channel(2);
        let consumer = std::thread::spawn(move || while rx.recv().is_ok() {});
        let mut command = Command::new("python3");
        command.args(["-c", &script]);
        let result = context.run(|| patch_command(command, expected, tx, context.clone(), 123));
        consumer.join().unwrap();
        result
    }

    #[test]
    fn successful_stdout_eof_waits_for_delayed_diagnostics_and_exit() {
        let text = serde_json::to_string(transport_patch()).unwrap();
        let script=format!("import os,sys,time\nsys.stdout.write({text})\nsys.stdout.flush()\nos.close(1)\ntime.sleep(0.15)\nsys.stderr.write('done')\n");
        let start = Instant::now();
        let (files, _) = controlled_transport(script, 1).unwrap();
        assert_eq!(files.len(), 1);
        assert!(start.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn malformed_or_over_count_stream_stops_the_owned_child_promptly() {
        for (patch, expected) in [
            (transport_patch().replace("+new\n", ""), 1),
            (transport_patch().to_owned(), 0),
        ] {
            let text = serde_json::to_string(&patch).unwrap();
            let script=format!("import os,sys,time\nsys.stdout.write({text})\nsys.stdout.flush()\nos.close(1)\ntime.sleep(10)\n");
            let start = Instant::now();
            assert!(controlled_transport(script, expected).is_err());
            assert!(start.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    fn full_queue_cancellation_releases_transport_without_a_consumer() {
        let context = loading::ReadContext::default();
        let worker_context = context.clone();
        let (tx, rx) = mpsc::sync_channel(2);
        let patch = transport_patch();
        let mut payload = String::new();
        for index in 0..400 {
            payload.push_str(
                &patch
                    .replace("a/f", &format!("a/f{index}"))
                    .replace("b/f", &format!("b/f{index}")),
            );
        }
        let text = serde_json::to_string(&payload).unwrap();
        let mut command = Command::new("python3");
        command.args([
            "-c",
            &format!(
                "import sys,time\nsys.stdout.write({text})\nsys.stdout.flush()\ntime.sleep(10)"
            ),
        ]);
        let worker = std::thread::spawn(move || {
            worker_context.run(|| patch_command(command, 400, tx, worker_context.clone(), 123))
        });
        std::thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        context.cancel();
        drop(rx);
        assert!(worker.join().unwrap().is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
