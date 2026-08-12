use crate::app::{AnimationPhase, App, FilePanelMode, TopbarTab, TopbarTabContent, ViewMode};
use anyhow::{anyhow, Context, Result};
use oyo_core::{AnimationFrame, ViewLine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{hash_map::DefaultHasher, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlSessionInfo {
    pub name: String,
    pub pid: u32,
    pub workspace: PathBuf,
    pub target: String,
    pub socket: PathBuf,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "command",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ControlRequest {
    Get,
    Rename {
        name: String,
    },
    Where,
    Diff {
        include_patch: bool,
    },
    Next {
        count: usize,
    },
    Prev {
        count: usize,
    },
    Hunk {
        mode: String,
        count: usize,
    },
    File {
        target: Option<String>,
        new_tab: bool,
        count: usize,
    },
    Goto {
        file: Option<String>,
        new_line: Option<usize>,
        old_line: Option<usize>,
        hunk: Option<usize>,
        step: Option<usize>,
        start: bool,
        end: bool,
    },
    Target {
        target: Option<String>,
        worktree: bool,
        staged: bool,
    },
    Play {
        from: Option<String>,
        to: Option<String>,
        delay_ms: u64,
    },
    Pause,
    Cancel,
    View {
        mode: String,
    },
    Step {
        mode: String,
    },
    Watch {
        mode: String,
    },
    Speed {
        mode: String,
    },
    Animation {
        mode: String,
    },
    Wrap {
        mode: String,
    },
    Syntax {
        mode: String,
    },
    Zen {
        mode: String,
    },
    Sidebar {
        mode: String,
    },
    Tab {
        kind: String,
        file: Option<String>,
    },
    Action {
        id: String,
        count: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlResponse {
    pub ok: bool,
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub applied: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub queued: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub message: Option<String>,
    pub data: Value,
}

impl ControlResponse {
    fn ok(session: &str, message: impl Into<String>, data: Value) -> Self {
        Self {
            ok: true,
            session: Some(session.to_string()),
            applied: false,
            queued: false,
            seq: None,
            message: Some(message.into()),
            data,
        }
    }

    fn applied(session: &str, message: impl Into<String>, data: Value) -> Self {
        Self {
            applied: true,
            ..Self::ok(session, message, data)
        }
    }

    fn queued(session: &str, seq: u64, message: impl Into<String>) -> Self {
        Self {
            ok: true,
            session: Some(session.to_string()),
            applied: false,
            queued: true,
            seq: Some(seq),
            message: Some(message.into()),
            data: json!({ "queued": true, "seq": seq }),
        }
    }

    fn err(session: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            session: session.map(str::to_string),
            applied: false,
            queued: false,
            seq: None,
            message: Some(message.into()),
            data: Value::Null,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ControlTarget {
    Revision(String),
    Worktree,
    Staged,
}

#[derive(Debug, Default)]
pub(crate) struct ControlPoll {
    pub redraw: bool,
    pub target: Option<(ControlTarget, u64)>,
}

const CONTROL_QUEUE_LIMIT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayPoint {
    Current,
    Start,
    End,
    Step(usize),
}

struct QueuedPlay {
    from: PlayPoint,
    to: PlayPoint,
    delay: Duration,
    target: Option<usize>,
    next_at: Instant,
}

struct QueuedControl {
    seq: u64,
    request: ControlRequest,
}

pub(crate) struct ControlSession {
    info: ControlSessionInfo,
    listener: UnixListener,
    meta_path: PathBuf,
    queue: VecDeque<QueuedControl>,
    play: Option<(u64, QueuedPlay)>,
    in_flight_seq: Option<u64>,
    next_seq: u64,
    last_applied_seq: u64,
}

fn is_false(value: &bool) -> bool {
    !*value
}

struct GotoTarget<'a> {
    file: Option<&'a str>,
    new_line: Option<usize>,
    old_line: Option<usize>,
    hunk: Option<usize>,
    step: Option<usize>,
    start: bool,
    end: bool,
}

impl ControlSession {
    pub(crate) fn start(
        workspace: &Path,
        target: &str,
        requested_name: Option<&str>,
        last_applied_seq: u64,
    ) -> Result<Self> {
        let pid = std::process::id();
        let dir = session_workspace_dir(workspace);
        fs::create_dir_all(&dir).context("Failed to create Oyo control session directory")?;
        cleanup_stale_sessions_in(&dir);
        let socket = dir.join(format!("{pid}.sock"));
        let meta_path = dir.join(format!("{pid}.json"));
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).context("Failed to bind Oyo control socket")?;
        set_user_only(&socket);
        listener
            .set_nonblocking(true)
            .context("Failed to configure Oyo control socket")?;
        let name = requested_name
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_session_name(workspace, pid));
        let info = ControlSessionInfo {
            name,
            pid,
            workspace: workspace.to_path_buf(),
            target: target.to_string(),
            socket,
            created_at: now_secs(),
        };
        write_info(&meta_path, &info)?;
        Ok(Self {
            info,
            listener,
            meta_path,
            queue: VecDeque::new(),
            play: None,
            in_flight_seq: None,
            next_seq: last_applied_seq.saturating_add(1),
            last_applied_seq,
        })
    }

    pub(crate) fn poll(&mut self, app: &mut App) -> ControlPoll {
        let mut poll = ControlPoll::default();
        self.info.target = app.control_target_label();
        let _ = write_info(&self.meta_path, &self.info);
        loop {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    poll.redraw = true;
                    let response = self.handle_stream(app, &mut stream);
                    let _ = serde_json::to_writer(&mut stream, &response);
                    let _ = stream.write_all(b"\n");
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        if self.finish_in_flight(app) {
            poll.redraw = true;
        }
        if let Some(target) = self.advance_async(app) {
            poll.redraw = true;
            poll.target = Some(target);
        }
        poll
    }

    fn handle_stream(&mut self, app: &mut App, stream: &mut UnixStream) -> ControlResponse {
        let mut input = String::new();
        if let Err(error) = stream.read_to_string(&mut input) {
            return ControlResponse::err(Some(&self.info.name), error.to_string());
        }
        let request = match serde_json::from_str::<ControlRequest>(&input) {
            Ok(request) => request,
            Err(error) => return ControlResponse::err(Some(&self.info.name), error.to_string()),
        };
        if matches!(request, ControlRequest::Cancel) {
            return self.cancel(app, "Cancelled control queue.");
        }
        if let ControlRequest::Rename { name } = request {
            return self.rename_session(app, &name);
        }
        if async_request(app, &request) {
            return match self.enqueue(request) {
                Ok(seq) => ControlResponse::queued(
                    &self.info.name,
                    seq,
                    format!("Control command queued as seq {seq}."),
                ),
                Err(error) => ControlResponse::err(Some(&self.info.name), error.to_string()),
            };
        }
        match apply_request(app, &self.info, self.last_applied_seq, request) {
            Ok(response) => response,
            Err(error) => ControlResponse::err(Some(&self.info.name), error.to_string()),
        }
    }
}

impl ControlSession {
    pub(crate) fn name(&self) -> &str {
        &self.info.name
    }

    pub(crate) fn rename(&mut self, name: &str) -> Result<ControlSessionInfo> {
        let name = validated_session_name(name)?;
        if list_sessions()
            .into_iter()
            .any(|session| session.pid != self.info.pid && session.name == name)
        {
            anyhow::bail!("An Oyo session named '{name}' is already running");
        }
        self.info.name = name;
        write_info(&self.meta_path, &self.info)?;
        Ok(self.info.clone())
    }

    fn rename_session(&mut self, app: &mut App, name: &str) -> ControlResponse {
        match self.rename(name) {
            Ok(info) => {
                let name = info.name.clone();
                app.set_control_session_name(Some(name.clone()));
                ControlResponse::applied(
                    &name,
                    format!("Session renamed to {name}."),
                    serde_json::to_value(info).unwrap_or(Value::Null),
                )
            }
            Err(error) => ControlResponse::err(Some(&self.info.name), error.to_string()),
        }
    }

    pub(crate) fn preempt(&mut self, app: &mut App) -> bool {
        if self.queue.is_empty() && self.play.is_none() && self.in_flight_seq.is_none() {
            return false;
        }
        self.clear_async(app);
        true
    }

    fn enqueue(&mut self, request: ControlRequest) -> Result<u64> {
        let queued = self.queue.len()
            + usize::from(self.play.is_some())
            + usize::from(self.in_flight_seq.is_some());
        if queued >= CONTROL_QUEUE_LIMIT {
            anyhow::bail!("Control queue full. Cancel or wait.");
        }
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.queue.push_back(QueuedControl { seq, request });
        Ok(seq)
    }

    fn cancel(&mut self, app: &mut App, message: &str) -> ControlResponse {
        let cleared = self.clear_async(app);
        ControlResponse::applied(
            &self.info.name,
            message,
            json!({ "clearedThroughSeq": cleared, "lastAppliedSeq": self.last_applied_seq }),
        )
    }

    fn clear_async(&mut self, app: &mut App) -> u64 {
        self.queue.clear();
        self.play = None;
        self.in_flight_seq = None;
        app.stop_control_motion();
        self.next_seq.saturating_sub(1)
    }

    fn finish_in_flight(&mut self, app: &App) -> bool {
        if app.animation_phase != AnimationPhase::Idle {
            return false;
        }
        let Some(seq) = self.in_flight_seq.take() else {
            return false;
        };
        self.last_applied_seq = self.last_applied_seq.max(seq);
        true
    }

    fn advance_async(&mut self, app: &mut App) -> Option<(ControlTarget, u64)> {
        if self.in_flight_seq.is_some() {
            return None;
        }
        if let Some((seq, mut play)) = self.play.take() {
            if self.advance_play(app, seq, &mut play) {
                return None;
            }
            self.play = Some((seq, play));
            return None;
        }
        let queued = self.queue.pop_front()?;
        if let ControlRequest::Play { from, to, delay_ms } = queued.request {
            self.play = Some((
                queued.seq,
                QueuedPlay {
                    from: parse_play_point(from.as_deref(), PlayPoint::Current),
                    to: parse_play_point(to.as_deref(), PlayPoint::End),
                    delay: Duration::from_millis(delay_ms),
                    target: None,
                    next_at: Instant::now(),
                },
            ));
            return self.advance_async(app);
        }
        if let Some(target) = control_target_from_request(&queued.request) {
            self.last_applied_seq = self.last_applied_seq.max(queued.seq);
            return Some((target, queued.seq));
        }
        let _ = apply_request(app, &self.info, self.last_applied_seq, queued.request);
        if app.animation_phase == AnimationPhase::Idle {
            self.last_applied_seq = self.last_applied_seq.max(queued.seq);
        } else {
            self.in_flight_seq = Some(queued.seq);
        }
        None
    }

    fn advance_play(&mut self, app: &mut App, seq: u64, play: &mut QueuedPlay) -> bool {
        if play.target.is_none() {
            app.stop_autoplay();
            if !app.stepping {
                app.toggle_stepping();
            }
            apply_play_goto(app, play.from);
            let (current, step_count) = app_step_index(app);
            play.target = Some(play_target(play.to, current, step_count));
            play.next_at = Instant::now() + play.delay;
            if play.target == Some(current) && app.animation_phase == AnimationPhase::Idle {
                self.last_applied_seq = self.last_applied_seq.max(seq);
                return true;
            }
            return false;
        }
        if app.animation_phase != AnimationPhase::Idle || Instant::now() < play.next_at {
            return false;
        }
        let (current, step_count) = app_step_index(app);
        let target = play
            .target
            .unwrap_or(current)
            .min(step_count.saturating_sub(1));
        if current == target {
            self.last_applied_seq = self.last_applied_seq.max(seq);
            return true;
        }
        if current < target {
            app.next_step();
        } else {
            app.prev_step();
        }
        let (next, _) = app_step_index(app);
        play.next_at = Instant::now() + play.delay;
        if next == target && app.animation_phase == AnimationPhase::Idle {
            self.last_applied_seq = self.last_applied_seq.max(seq);
            true
        } else {
            false
        }
    }
}

impl Drop for ControlSession {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.info.socket);
        let _ = fs::remove_file(&self.meta_path);
    }
}

pub(crate) fn list_sessions() -> Vec<ControlSessionInfo> {
    let mut out = Vec::new();
    let root = sessions_root();
    let Ok(workspaces) = fs::read_dir(root) else {
        return out;
    };
    for workspace in workspaces.flatten() {
        let Ok(files) = fs::read_dir(workspace.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(info) = read_info(&path) else {
                continue;
            };
            if session_live(&info) {
                out.push(info);
            } else {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(&info.socket);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub(crate) fn resolve_session(
    name: Option<&str>,
    workspace: Option<&Path>,
) -> Result<ControlSessionInfo> {
    let mut sessions = list_sessions();
    if let Some(name) = name {
        sessions.retain(|session| session.name == name || session.pid.to_string() == name);
    } else if let Some(workspace) = workspace {
        sessions.retain(|session| same_path(&session.workspace, workspace));
    }
    match sessions.as_slice() {
        [] => Err(anyhow!("No Oyo session is running for this workspace.")),
        [session] => Ok(session.clone()),
        _ => Err(anyhow!(
            "More than one Oyo session is running. Pass --session.\n\nRun:\n  oy control list"
        )),
    }
}

pub(crate) fn send_request(
    session: &ControlSessionInfo,
    request: &ControlRequest,
) -> Result<ControlResponse> {
    let mut stream = UnixStream::connect(&session.socket).with_context(|| {
        format!(
            "Failed to connect to Oyo session '{}'. Run `oy control list`.",
            session.name
        )
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    serde_json::to_writer(&mut stream, request)?;
    stream.shutdown(std::net::Shutdown::Write).ok();
    let mut output = String::new();
    stream.read_to_string(&mut output)?;
    let response = serde_json::from_str::<ControlResponse>(&output)?;
    if response.ok {
        Ok(response)
    } else {
        Err(anyhow!(
            "{}",
            response
                .message
                .unwrap_or_else(|| "Control command failed".to_string())
        ))
    }
}

fn control_target_from_request(request: &ControlRequest) -> Option<ControlTarget> {
    let ControlRequest::Target {
        target,
        worktree,
        staged,
    } = request
    else {
        return None;
    };
    match (target.as_deref(), *worktree, *staged) {
        (Some(_), true, _) | (Some(_), _, true) | (_, true, true) => None,
        (_, true, false) => Some(ControlTarget::Worktree),
        (_, false, true) => Some(ControlTarget::Staged),
        (Some(target), false, false) => Some(ControlTarget::Revision(target.to_string())),
        (None, false, false) => None,
    }
}

fn async_request(app: &App, request: &ControlRequest) -> bool {
    match request {
        ControlRequest::Play { .. } => true,
        ControlRequest::Target { .. } => control_target_from_request(request).is_some(),
        ControlRequest::Next { .. } | ControlRequest::Prev { .. } => app.stepping,
        ControlRequest::Hunk { .. } => app.stepping || app.animation_enabled,
        ControlRequest::Goto { .. } => app.stepping || app.animation_enabled,
        ControlRequest::File { target, .. } => {
            matches!(target.as_deref(), Some("next" | "prev" | "previous"))
                && (app.stepping || app.animation_enabled)
        }
        ControlRequest::Action { id, .. } => action_async(app, id),
        _ => false,
    }
}

fn action_async(app: &App, id: &str) -> bool {
    let movement = matches!(
        id,
        "normal.step_down"
            | "normal.step_up"
            | "normal.next_hunk"
            | "normal.prev_hunk"
            | "normal.hunk_start"
            | "normal.hunk_end"
            | "normal.goto_start"
            | "normal.goto_end"
            | "normal.first_step"
            | "normal.last_step"
            | "normal.prev_file"
            | "normal.next_file"
            | "normal.scroll_up"
            | "normal.scroll_down"
            | "normal.next_conflict"
            | "normal.prev_conflict"
            | "normal.replay_step"
    );
    movement && (app.stepping || app.animation_enabled)
}

fn parse_play_point(value: Option<&str>, default: PlayPoint) -> PlayPoint {
    let Some(value) = value else {
        return default;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "current" | "now" => PlayPoint::Current,
        "start" | "first" => PlayPoint::Start,
        "end" | "last" => PlayPoint::End,
        value => value.parse().map(PlayPoint::Step).unwrap_or(default),
    }
}

fn app_step_index(app: &mut App) -> (usize, usize) {
    let state = app.multi_diff.current_navigator().state().clone();
    (
        state.current_step.min(state.total_steps.saturating_sub(1)),
        state.total_steps,
    )
}

fn play_target(point: PlayPoint, current: usize, step_count: usize) -> usize {
    match point {
        PlayPoint::Current => current,
        PlayPoint::Start => 0,
        PlayPoint::End => step_count.saturating_sub(1),
        PlayPoint::Step(step) => step.max(1).min(step_count).saturating_sub(1),
    }
}

fn apply_play_goto(app: &mut App, point: PlayPoint) {
    match point {
        PlayPoint::Current => {}
        PlayPoint::Start => app.goto_start(),
        PlayPoint::End => app.goto_end(),
        PlayPoint::Step(step) => {
            let _ = apply_goto(
                app,
                GotoTarget {
                    file: None,
                    new_line: None,
                    old_line: None,
                    hunk: None,
                    step: Some(step),
                    start: false,
                    end: false,
                },
            );
        }
    }
}

fn apply_request(
    app: &mut App,
    info: &ControlSessionInfo,
    last_applied_seq: u64,
    request: ControlRequest,
) -> Result<ControlResponse> {
    if matches!(
        &request,
        ControlRequest::Next { .. }
            | ControlRequest::Prev { .. }
            | ControlRequest::Hunk { .. }
            | ControlRequest::File { .. }
            | ControlRequest::Goto { .. }
            | ControlRequest::View { .. }
            | ControlRequest::Step { .. }
            | ControlRequest::Action { .. }
    ) {
        app.mark_user_input();
    }
    match request {
        ControlRequest::Get => Ok(ControlResponse::ok(
            &info.name,
            format_session(info),
            serde_json::to_value(info)?,
        )),
        ControlRequest::Rename { .. } => anyhow::bail!("Rename must be applied by the session"),
        ControlRequest::Where => Ok(where_response(app, info, last_applied_seq)),
        ControlRequest::Diff { include_patch } => Ok(diff_response(app, info, include_patch)),
        ControlRequest::Next { count } => {
            for _ in 0..count.max(1) {
                if app.stepping {
                    app.next_step();
                } else {
                    app.scroll_down();
                }
            }
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Prev { count } => {
            for _ in 0..count.max(1) {
                if app.stepping {
                    app.prev_step();
                } else {
                    app.scroll_up();
                }
            }
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Hunk { mode, count } => {
            apply_hunk(app, &mode, count.max(1))?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::File {
            target,
            new_tab,
            count,
        } => {
            match target.as_deref() {
                Some("next") => {
                    for _ in 0..count.max(1) {
                        app.next_file();
                    }
                }
                Some("prev") | Some("previous") => {
                    for _ in 0..count.max(1) {
                        app.prev_file();
                    }
                }
                Some(path) => select_file(app, path, new_tab)?,
                None => {}
            }
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Goto {
            file,
            new_line,
            old_line,
            hunk,
            step,
            start,
            end,
        } => {
            apply_goto(
                app,
                GotoTarget {
                    file: file.as_deref(),
                    new_line,
                    old_line,
                    hunk,
                    step,
                    start,
                    end,
                },
            )?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Target { .. } => anyhow::bail!("Pass a target, --worktree or --staged"),
        ControlRequest::Play { .. } => anyhow::bail!("Play must be queued"),
        ControlRequest::Pause => {
            app.stop_autoplay();
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Cancel => {
            app.stop_control_motion();
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::View { mode } => {
            apply_view(app, &mode)?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Step { mode } => {
            apply_bool_mode(&mode, app.stepping, |value| {
                if value != app.stepping {
                    app.toggle_stepping();
                }
            })?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Watch { mode } => {
            apply_bool_mode(&mode, app.watch, |value| app.watch = value)?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Speed { mode } => {
            match mode.as_str() {
                "increase" | "up" => app.increase_speed(),
                "decrease" | "down" => app.decrease_speed(),
                _ => anyhow::bail!("Use increase or decrease"),
            }
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Animation { mode } => {
            apply_toggle_mode(
                &mode,
                app.animation_enabled,
                |app| app.toggle_animation(),
                app,
            )?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Wrap { mode } => {
            apply_toggle_mode(&mode, app.line_wrap, |app| app.toggle_line_wrap(), app)?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Syntax { mode } => {
            apply_toggle_mode(&mode, app.syntax_enabled(), |app| app.toggle_syntax(), app)?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Zen { mode } => {
            apply_toggle_mode(&mode, app.zen_mode, |app| app.toggle_zen(), app)?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Sidebar { mode } => {
            apply_sidebar(app, &mode)?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Tab { kind, file } => {
            apply_tab(app, &kind, file.as_deref())?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
        ControlRequest::Action { id, count } => {
            apply_action(app, &id, count.max(1))?;
            Ok(applied_where_response(app, info, last_applied_seq))
        }
    }
}

fn where_response(
    app: &mut App,
    info: &ControlSessionInfo,
    last_applied_seq: u64,
) -> ControlResponse {
    let data = context_json(app, info, last_applied_seq);
    ControlResponse::ok(&info.name, format_context(&data), data)
}

fn applied_where_response(
    app: &mut App,
    info: &ControlSessionInfo,
    last_applied_seq: u64,
) -> ControlResponse {
    let data = context_json(app, info, last_applied_seq);
    ControlResponse::applied(&info.name, format_context(&data), data)
}

fn diff_response(app: &mut App, info: &ControlSessionInfo, include_patch: bool) -> ControlResponse {
    let files = app
        .multi_diff
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            json!({
                "index": index,
                "path": file.display_name,
                "status": format!("{:?}", file.status),
                "insertions": file.insertions,
                "deletions": file.deletions,
                "binary": file.binary,
            })
        })
        .collect::<Vec<_>>();
    let data = json!({
        "session": info.name,
        "target": app.control_target_label(),
        "fileCount": app.multi_diff.file_count(),
        "currentFile": app.current_file_path(),
        "includePatch": include_patch,
        "files": files,
    });
    ControlResponse::ok(&info.name, "Diff listed.", data)
}

fn context_json(app: &mut App, info: &ControlSessionInfo, last_applied_seq: u64) -> Value {
    let state = app.multi_diff.current_navigator().state().clone();
    let tab = match app.active_topbar_content() {
        Some(TopbarTabContent::File(_)) => "file",
        Some(TopbarTabContent::Help) => "help",
        Some(TopbarTabContent::Settings) => "settings",
        Some(TopbarTabContent::PrComments) => "pr-comments",
        Some(TopbarTabContent::OutdatedComments) => "outdated-comments",
        None => "none",
    };
    let sidebar = if !app.file_panel_visible {
        "closed"
    } else {
        match app.file_panel_mode {
            FilePanelMode::Files => "files",
            FilePanelMode::Comments => "comments",
        }
    };
    let step = if state.total_steps == 0 {
        0
    } else {
        state.current_step.saturating_add(1).min(state.total_steps)
    };
    let active_tab_id = app.active_topbar_tab;
    let tabs = app
        .topbar_tabs
        .iter()
        .map(|tab| topbar_tab_json(app, tab, Some(tab.id) == active_tab_id))
        .collect::<Vec<_>>();
    let active_tab = app
        .topbar_tabs
        .iter()
        .find(|tab| Some(tab.id) == active_tab_id)
        .map(|tab| topbar_tab_json(app, tab, true))
        .unwrap_or(Value::Null);
    let cursor = cursor_json(app, state.cursor_change.or(state.active_change));
    let selection = app.control_selection_json();
    json!({
        "session": info.name,
        "pid": info.pid,
        "workspace": info.workspace,
        "target": app.control_target_label(),
        "file": app.current_file_path(),
        "fileIndex": app.multi_diff.selected_index,
        "fileCount": app.multi_diff.file_count(),
        "hunk": state.current_hunk.saturating_add(1),
        "hunkCount": state.total_hunks,
        "step": step,
        "stepIndex": state.current_step,
        "stepCount": state.total_steps,
        "sidebar": sidebar,
        "tab": tab,
        "activeTab": active_tab,
        "tabs": tabs,
        "view": view_mode_name(app.view_mode),
        "stepMode": app.stepping,
        "autoplay": app.autoplay,
        "watch": app.watch,
        "lineWrap": app.line_wrap,
        "syntax": app.syntax_enabled(),
        "zen": app.zen_mode,
        "lastAppliedSeq": last_applied_seq,
        "cursor": cursor,
        "selection": selection,
    })
}

fn topbar_tab_json(app: &App, tab: &TopbarTab, active: bool) -> Value {
    let (kind, file_index, file) = match tab.content {
        TopbarTabContent::File(index) => (
            "file",
            Some(index),
            app.multi_diff
                .files
                .get(index)
                .map(|file| file.display_name.clone()),
        ),
        TopbarTabContent::Help => ("help", None, None),
        TopbarTabContent::Settings => ("settings", None, None),
        TopbarTabContent::PrComments => ("pr-comments", None, None),
        TopbarTabContent::OutdatedComments => ("outdated-comments", None, None),
    };
    json!({
        "id": tab.id,
        "kind": kind,
        "active": active,
        "fileIndex": file_index,
        "file": file,
        "view": view_mode_name(if active { app.view_mode } else { tab.view_mode }),
        "stepMode": if active { app.stepping } else { tab.stepping },
    })
}

fn cursor_json(app: &mut App, change_id: Option<usize>) -> Value {
    let file = app.current_file_path();
    let view = app.current_view_with_frame(AnimationFrame::Idle);
    let render_offset = app.render_scroll_offset();
    let indexed = view.iter().enumerate().collect::<Vec<_>>();
    let focused = change_id.and_then(|id| {
        indexed
            .iter()
            .copied()
            .find(|(_, line)| {
                line.change_id == id
                    && (line.is_primary_active || line.is_active_change)
                    && (line.old_line.is_some() || line.new_line.is_some())
            })
            .or_else(|| {
                indexed.iter().copied().find(|(_, line)| {
                    line.change_id == id && (line.old_line.is_some() || line.new_line.is_some())
                })
            })
    });
    let Some((index, line)) = focused.or_else(|| {
        view.get(render_offset)
            .map(|line| (render_offset, line))
            .filter(|(_, line)| line.old_line.is_some() || line.new_line.is_some())
    }) else {
        return Value::Null;
    };
    line_json(&file, index.saturating_sub(render_offset), line)
}

fn line_json(file: &str, row: usize, line: &ViewLine) -> Value {
    let side = match (line.old_line, line.new_line) {
        (Some(_), Some(_)) => "both",
        (Some(_), None) => "old",
        (None, Some(_)) => "new",
        (None, None) => "none",
    };
    json!({
        "file": file,
        "side": side,
        "line": line.new_line.or(line.old_line),
        "oldLine": line.old_line,
        "newLine": line.new_line,
        "row": row,
        "changeId": line.change_id,
        "hunk": line.hunk_index.map(|hunk| hunk.saturating_add(1)),
    })
}

fn line_label(value: &Value) -> String {
    if value.is_null() {
        return "none".to_string();
    }
    let side = value["side"].as_str().unwrap_or("none");
    let line = value["line"].as_u64().unwrap_or(0);
    if line == 0 {
        return "none".to_string();
    }
    format!("{side} {line}")
}

fn selection_label(value: &Value) -> String {
    if !value["active"].as_bool().unwrap_or(false) {
        return "none".to_string();
    }
    let side = value["side"].as_str().unwrap_or("selection");
    let range = value["newRange"]
        .as_object()
        .or_else(|| value["oldRange"].as_object());
    let Some(range) = range else {
        return value["kind"].as_str().unwrap_or("selection").to_string();
    };
    let start = range.get("start").and_then(|v| v.as_u64()).unwrap_or(0);
    let end = range.get("end").and_then(|v| v.as_u64()).unwrap_or(start);
    if start == end {
        format!("{side} {start}")
    } else {
        format!("{side} {start} to {end}")
    }
}

fn format_context(data: &Value) -> String {
    format!(
        "Session: {}\nWorkspace: {}\nTarget: {}\nFile: {}\nCursor: {}\nSelection: {}\nHunk: {} of {}\nStep: {} of {}\nLast applied seq: {}\nSidebar: {}\nTab: {}\nView: {}\nStep mode: {}\nAutoplay: {}\nWatch: {}",
        data["session"].as_str().unwrap_or_default(),
        data["workspace"].as_str().unwrap_or_default(),
        data["target"].as_str().unwrap_or_default(),
        data["file"].as_str().unwrap_or_default(),
        line_label(&data["cursor"]),
        selection_label(&data["selection"]),
        data["hunk"].as_u64().unwrap_or(0),
        data["hunkCount"].as_u64().unwrap_or(0),
        data["step"].as_u64().unwrap_or(0),
        data["stepCount"].as_u64().unwrap_or(0),
        data["lastAppliedSeq"].as_u64().unwrap_or(0),
        data["sidebar"].as_str().unwrap_or_default(),
        data["tab"].as_str().unwrap_or_default(),
        data["view"].as_str().unwrap_or_default(),
        on_off(data["stepMode"].as_bool().unwrap_or(false)),
        on_off(data["autoplay"].as_bool().unwrap_or(false)),
        on_off(data["watch"].as_bool().unwrap_or(false)),
    )
}

fn format_session(info: &ControlSessionInfo) -> String {
    format!(
        "Session: {}\nWorkspace: {}\nTarget: {}\nPID: {}",
        info.name,
        info.workspace.display(),
        info.target,
        info.pid
    )
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

fn apply_bool_mode(mode: &str, current: bool, mut set: impl FnMut(bool)) -> Result<()> {
    match mode {
        "on" => set(true),
        "off" => set(false),
        "toggle" => set(!current),
        _ => anyhow::bail!("Use on, off or toggle"),
    }
    Ok(())
}

fn apply_toggle_mode(
    mode: &str,
    current: bool,
    mut toggle: impl FnMut(&mut App),
    app: &mut App,
) -> Result<()> {
    match mode {
        "on" if !current => toggle(app),
        "off" if current => toggle(app),
        "on" | "off" => {}
        "toggle" => toggle(app),
        _ => anyhow::bail!("Use on, off or toggle"),
    }
    Ok(())
}

fn apply_hunk(app: &mut App, mode: &str, count: usize) -> Result<()> {
    match mode {
        "next" => repeat(count, || {
            if app.stepping {
                app.next_hunk();
            } else {
                app.next_hunk_scroll();
            }
        }),
        "prev" | "previous" => repeat(count, || {
            if app.stepping {
                app.prev_hunk();
            } else {
                app.prev_hunk_scroll();
            }
        }),
        "start" => {
            if app.stepping {
                app.goto_hunk_start();
            } else {
                app.goto_hunk_start_scroll();
            }
        }
        "end" => {
            if app.stepping {
                app.goto_hunk_end();
            } else {
                app.goto_hunk_end_scroll();
            }
        }
        _ => anyhow::bail!("Use next, prev, start or end"),
    }
    Ok(())
}

fn apply_goto(app: &mut App, target: GotoTarget<'_>) -> Result<()> {
    if let Some(file) = target.file {
        select_file(app, file, false)?;
    }
    let target_count = [
        target.new_line.is_some(),
        target.old_line.is_some(),
        target.hunk.is_some(),
        target.step.is_some(),
        target.start,
        target.end,
    ]
    .into_iter()
    .filter(|value| *value)
    .count();
    if target_count != 1 {
        anyhow::bail!("Specify exactly one navigation target");
    }
    if target.start {
        app.goto_start();
        return Ok(());
    }
    if target.end {
        app.goto_end();
        return Ok(());
    }
    let query = if let Some(line) = target.new_line.or(target.old_line) {
        line.to_string()
    } else if let Some(hunk) = target.hunk {
        format!("h{hunk}")
    } else if let Some(step) = target.step {
        format!("s{step}")
    } else {
        unreachable!();
    };
    app.start_goto();
    app.clear_goto_text();
    for ch in query.chars() {
        app.push_goto_char(ch);
    }
    app.apply_goto();
    app.clear_goto();
    Ok(())
}

fn apply_view(app: &mut App, mode: &str) -> Result<()> {
    match mode {
        "next" => app.toggle_view_mode(),
        "prev" | "previous" => app.toggle_view_mode_reverse(),
        "unified" => app.set_view_mode(ViewMode::UnifiedPane),
        "split" => app.set_view_mode(ViewMode::Split),
        "evolution" | "evo" => app.set_view_mode(ViewMode::Evolution),
        "blame" => app.set_view_mode(ViewMode::Blame),
        "preview" => app.set_view_mode(ViewMode::Preview),
        _ => anyhow::bail!("Unknown view mode: {mode}"),
    }
    Ok(())
}

fn apply_sidebar(app: &mut App, mode: &str) -> Result<()> {
    match mode {
        "files" => {
            app.file_panel_visible = true;
            app.file_panel_mode = FilePanelMode::Files;
        }
        "comments" => {
            app.file_panel_visible = true;
            app.file_panel_mode = FilePanelMode::Comments;
        }
        "close" | "off" => app.file_panel_visible = false,
        "toggle" => app.toggle_file_panel(),
        "focus" => {
            if app.can_show_file_panel() {
                app.file_list_focused = true;
                app.file_panel_visible = true;
            }
        }
        _ => anyhow::bail!("Unknown sidebar mode: {mode}"),
    }
    Ok(())
}

fn apply_tab(app: &mut App, kind: &str, file: Option<&str>) -> Result<()> {
    match kind {
        "help" => app.open_help_tab(),
        "settings" => app.open_settings_tab(),
        "pr-comments" | "pr" => app.open_pr_comments_tab(None),
        "outdated-comments" | "outdated" => app.open_outdated_comments_tab(None),
        "close" => app.close_active_topbar_tab(),
        "file" => {
            let path = file.ok_or_else(|| anyhow!("Pass a file path"))?;
            select_file(app, path, true)?;
        }
        _ => anyhow::bail!("Unknown tab target: {kind}"),
    }
    Ok(())
}

fn apply_action(app: &mut App, id: &str, count: usize) -> Result<()> {
    match id {
        "normal.step_down" => {
            for _ in 0..count {
                if app.stepping {
                    app.next_step();
                } else {
                    app.scroll_down();
                }
            }
        }
        "normal.step_up" => {
            for _ in 0..count {
                if app.stepping {
                    app.prev_step();
                } else {
                    app.scroll_up();
                }
            }
        }
        "normal.next_hunk" => repeat(count, || {
            if app.stepping {
                app.next_hunk();
            } else {
                app.next_hunk_scroll();
            }
        }),
        "normal.prev_hunk" => repeat(count, || {
            if app.stepping {
                app.prev_hunk();
            } else {
                app.prev_hunk_scroll();
            }
        }),
        "normal.hunk_start" => {
            if app.stepping {
                app.goto_hunk_start();
            } else {
                app.goto_hunk_start_scroll();
            }
        }
        "normal.hunk_end" => {
            if app.stepping {
                app.goto_hunk_end();
            } else {
                app.goto_hunk_end_scroll();
            }
        }
        "normal.goto_start" => app.goto_start(),
        "normal.goto_end" => app.goto_end(),
        "normal.first_step" => {
            if app.stepping {
                app.goto_first_step();
            } else {
                app.goto_first_hunk_scroll();
            }
        }
        "normal.last_step" => {
            if app.stepping {
                app.goto_last_step();
            } else {
                app.goto_last_hunk_scroll();
            }
        }
        "normal.prev_file" => repeat(count, || app.prev_file()),
        "normal.next_file" => repeat(count, || app.next_file()),
        "normal.navigate_back" => {
            app.navigate_view_back();
        }
        "normal.navigate_forward" => {
            app.navigate_view_forward();
        }
        "normal.toggle_autoplay" => app.toggle_autoplay(),
        "normal.toggle_autoplay_reverse" => app.toggle_autoplay_reverse(),
        "normal.toggle_view_mode" => app.toggle_view_mode(),
        "normal.toggle_view_mode_reverse" => app.toggle_view_mode_reverse(),
        "normal.scroll_up" => repeat(count, || app.scroll_up()),
        "normal.scroll_down" => repeat(count, || app.scroll_down()),
        "normal.toggle_file_list_focus" => app.file_list_focused = !app.file_list_focused,
        "normal.increase_speed" => app.increase_speed(),
        "normal.decrease_speed" => app.decrease_speed(),
        "normal.toggle_animation" => app.toggle_animation(),
        "normal.toggle_line_wrap" => app.toggle_line_wrap(),
        "normal.toggle_syntax" => app.toggle_syntax(),
        "normal.toggle_evo_syntax" => app.toggle_evo_syntax(),
        "normal.toggle_stepping" => app.toggle_stepping(),
        "normal.toggle_strikethrough" => app.toggle_strikethrough_deletions(),
        "normal.scroll_left" => repeat(count, || app.scroll_left()),
        "normal.scroll_right" => repeat(count, || app.scroll_right()),
        "normal.line_start" => app.scroll_to_line_start(),
        "normal.line_end" => app.scroll_to_line_end(),
        "normal.toggle_zen" => app.toggle_zen(),
        "normal.replay_step" => app.replay_step(),
        "normal.refresh" => app.refresh_all_files(),
        "normal.toggle_file_panel" => app.toggle_file_panel(),
        "normal.toggle_fold_context" => app.toggle_fold_context(),
        "normal.expand_all_folds" => {
            app.expand_all_context_folds();
        }
        "normal.search_next" => app.search_next(),
        "normal.search_prev" => app.search_prev(),
        "normal.focus_next_comment" => repeat(count, || {
            app.focus_next_review_comment();
        }),
        "normal.focus_prev_comment" => repeat(count, || {
            app.focus_prev_review_comment();
        }),
        "normal.next_conflict" => repeat(count, || app.next_conflict()),
        "normal.prev_conflict" => repeat(count, || app.prev_conflict()),
        "normal.toggle_help" => app.open_help_tab(),
        "normal.open_command_palette" => app.start_command_palette(),
        "normal.open_file_search" => app.start_file_search(),
        "normal.open_review_grep" => app.start_review_grep(),
        "normal.open_comment_picker" => app.start_comment_picker(),
        "normal.open_theme_picker" => app.start_theme_picker(),
        "normal.blame_hint" => app.trigger_blame_hint(),
        "normal.toggle_peek_change" => app.toggle_peek_old_change(),
        "normal.toggle_peek_hunk" => app.toggle_peek_old_hunk(),
        "help.close" => app.close_active_topbar_tab(),
        _ => anyhow::bail!("Unsupported control action: {id}"),
    }
    Ok(())
}

fn repeat(count: usize, mut f: impl FnMut()) {
    for _ in 0..count {
        f();
    }
}

fn select_file(app: &mut App, path: &str, new_tab: bool) -> Result<()> {
    let matches = app
        .multi_diff
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| {
            file.display_name == path
                || file.path == Path::new(path)
                || file.display_name.ends_with(path)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let index = match matches.as_slice() {
        [] => anyhow::bail!("No visible diff file matches {path}."),
        [index] => *index,
        _ => anyhow::bail!("More than one visible diff file matches {path}."),
    };
    if new_tab {
        if let Some(id) = app
            .topbar_tabs
            .iter()
            .find(|tab| tab.content == TopbarTabContent::File(index))
            .map(|tab| tab.id)
        {
            app.select_topbar_tab(id);
        } else {
            app.open_file_in_new_topbar_tab(index);
        }
    } else {
        app.select_file(index);
    }
    Ok(())
}

fn view_mode_name(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::UnifiedPane => "unified",
        ViewMode::Split => "split",
        ViewMode::Evolution => "evolution",
        ViewMode::Blame => "blame",
        ViewMode::Preview => "preview",
    }
}

fn sessions_root() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("oyo")
        .join("sessions")
}

fn session_workspace_dir(workspace: &Path) -> PathBuf {
    sessions_root().join(hash_path(workspace))
}

fn hash_path(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn default_session_name(workspace: &Path, pid: u32) -> String {
    let repo = workspace
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("oyo");
    format!("{}-{pid}", sanitize_name(repo))
}

fn validated_session_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("Session name cannot be empty");
    }
    Ok(value.to_string())
}

fn sanitize_name(value: &str) -> String {
    let value = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    value.trim_matches('-').to_string()
}

fn write_info(path: &Path, info: &ControlSessionInfo) -> Result<()> {
    let data = serde_json::to_vec_pretty(info)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    set_user_only(path);
    file.write_all(&data)?;
    Ok(())
}

fn read_info(path: &Path) -> Option<ControlSessionInfo> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn cleanup_stale_sessions_in(dir: &Path) {
    let Ok(files) = fs::read_dir(dir) else {
        return;
    };
    for file in files.flatten() {
        let path = file.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(info) = read_info(&path) else {
            continue;
        };
        if !session_live(&info) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(info.socket);
        }
    }
}

fn set_user_only(path: &Path) {
    #[cfg(unix)]
    {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

fn session_live(info: &ControlSessionInfo) -> bool {
    info.socket.exists() && pid_alive(info.pid)
}

fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid == 0 {
        return false;
    }
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let a = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let b = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    a == b
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{default_session_name, pid_alive, sanitize_name, validated_session_name};
    use std::path::Path;
    use std::process::Command;

    #[test]
    fn session_name_uses_repo_and_pid() {
        assert_eq!(sanitize_name("oyo repo"), "oyo-repo");
        assert_eq!(default_session_name(Path::new("/tmp/oyo"), 42), "oyo-42");
    }

    #[test]
    fn session_rename_rejects_empty_names() {
        assert!(validated_session_name("   ").is_err());
        assert_eq!(validated_session_name(" review-a ").unwrap(), "review-a");
    }

    #[test]
    fn pid_liveness_detects_running_and_exited_processes() {
        assert!(pid_alive(std::process::id()));

        let mut child = Command::new("sh").arg("-c").arg("exit 0").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!pid_alive(pid));
    }
}
