use crate::*;
use std::sync::{mpsc, Arc};

pub(crate) struct PreparedRemote {
    diff: MultiFileDiff,
    branch: Option<String>,
    restore_review: crate::app::startup::RestoreRemoteReview,
    control: Result<control::ControlSession, String>,
}

pub(crate) enum StartupEvent {
    Progress(&'static str),
    Ready(Result<Box<PreparedRemote>, String>),
}

struct StartupWorker {
    context: remote::loading::ReadContext,
    rx: mpsc::Receiver<StartupEvent>,
    contents: Option<remote::loading::ReadContext>,
}

impl Drop for StartupWorker {
    fn drop(&mut self) {
        self.context.cancel();
        if let Some(contents) = &self.contents {
            contents.cancel();
        }
    }
}

fn start(
    url: String,
    config: Arc<config::Config>,
    args: Arc<Args>,
    _light: bool,
    session: Option<String>,
    last_applied_seq: u64,
) -> StartupWorker {
    let context = remote::loading::ReadContext::default();
    let worker_context = context.clone();
    let contents = remote::loading::ReadContext::default();
    let worker_contents = contents.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        worker_context.run(|| {
            let result = (|| -> Result<PreparedRemote> {
                let (built, patch_session) = remote::stream::start(&url, worker_contents.clone())?;
                let _ = tx.send(StartupEvent::Progress("Preparing review..."));
                if worker_context.is_cancelled() {
                    anyhow::bail!("Remote loading cancelled");
                }
                let speed = if args.speed != 200 {
                    args.speed
                } else {
                    config.playback.speed
                };
                let mut app = App::new(
                    built.multi_diff,
                    resolve_view_mode(args.view, &config),
                    speed,
                    args.autoplay || config.playback.autoplay,
                    built.branch,
                );
                app.remote_validation = built.remote_validation;
                app.remote_stream = patch_session;
                app.start_content_loading_with_context(worker_contents);
                if app.remote_stream.is_some() {
                    let review_config = config.clone();
                    let review_args = args.clone();
                    let review_url = url.clone();
                    let workspace = built.workspace_root;
                    app.pending_remote_review = Some(Box::new(move |app| {
                        configure_review_state_for_app(
                            app,
                            &review_config,
                            &review_args,
                            workspace.clone(),
                            &InputMode::Remote {
                                url: review_url.clone(),
                            },
                            None,
                            None,
                            true,
                        )
                    }));
                } else {
                    configure_review_state_for_app(
                        &mut app,
                        &config,
                        &args,
                        built.workspace_root,
                        &InputMode::Remote { url: url.clone() },
                        None,
                        None,
                        true,
                    )?;
                }
                if worker_context.is_cancelled() {
                    anyhow::bail!("Remote loading cancelled");
                }
                let workspace = current_review_workspace()?;
                let control = control::ControlSession::start(
                    &workspace,
                    &app.control_target_label(),
                    session.as_deref(),
                    last_applied_seq,
                )
                .map_err(|e| e.to_string());
                let branch = Some(url);
                let (diff, restore_review) = app.into_remote_startup();
                Ok(PreparedRemote {
                    diff,
                    branch,
                    restore_review,
                    control,
                })
            })()
            .map_err(|error| error.to_string());
            let _ = tx.send(StartupEvent::Ready(result.map(Box::new)));
        })
    });
    StartupWorker {
        context,
        rx,
        contents: Some(contents),
    }
}

pub(crate) fn remote_app(
    terminal: &mut TuiTerminal,
    url: &str,
    config: Arc<config::Config>,
    args: Arc<Args>,
    light: bool,
    session: Option<String>,
    last_applied_seq: u64,
) -> Result<Option<(App, Result<control::ControlSession, String>)>> {
    let mut worker = Some(start(
        url.to_owned(),
        config.clone(),
        args.clone(),
        light,
        session.clone(),
        last_applied_seq,
    ));
    let theme = config.ui.theme.resolve(light);
    let mut status = "Loading remote snapshot...".to_owned();
    let mut prepared_app = None;
    loop {
        if let Some(active) = worker.as_ref().filter(|_| prepared_app.is_none()) {
            loop {
                let event = match active.rx.try_recv() {
                    Ok(event) => event,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        status = "Unable to load review.".to_owned();
                        worker = None;
                        break;
                    }
                };
                match event {
                    StartupEvent::Progress(message) => status = message.to_owned(),
                    StartupEvent::Ready(Ok(prepared)) => {
                        let prepared = *prepared;
                        let speed = if args.speed != 200 {
                            args.speed
                        } else {
                            config.playback.speed
                        };
                        let mut app = App::new(
                            prepared.diff,
                            resolve_view_mode(args.view, &config),
                            speed,
                            args.autoplay || config.playback.autoplay,
                            prepared.branch,
                        );
                        apply_config_to_app(&mut app, &config, &args, light);
                        (prepared.restore_review)(&mut app);
                        app.move_selection_off_hidden_reviewed_file();
                        prepared_app = Some((app, prepared.control));
                        status = "Loading diff...".to_owned();
                        break;
                    }
                    StartupEvent::Ready(Err(error)) => {
                        status = format!("Unable to load review\n\n{error}");
                        worker = None;
                        break;
                    }
                }
            }
        }
        if let Some((app, _)) = prepared_app.as_mut() {
            app.poll_content_responses();
            app.poll_diff_responses();
            app.move_selection_off_hidden_reviewed_file();
            app.queue_current_file_diff();
            if app.multi_diff.file_count() == 0
                || !matches!(
                    app.multi_diff.diff_status(app.multi_diff.selected_index),
                    oyo_core::multi::DiffStatus::Loading
                        | oyo_core::multi::DiffStatus::Deferred
                        | oyo_core::multi::DiffStatus::Computing
                )
            {
                worker.as_mut().and_then(|worker| worker.contents.take());
                return Ok(prepared_app.take());
            }
        }
        terminal.draw(|frame| {
            use ratatui::{
                layout::{Alignment, Rect},
                style::{Modifier, Style},
                text::Line,
                widgets::{Block, Paragraph, Wrap},
            };
            let area = frame.area();
            let mut background = Style::default().fg(theme.text);
            if let Some(color) = theme.background {
                background = background.bg(color);
            }
            frame.render_widget(Block::default().style(background), area);
            let width = area.width.min(64);
            let text_rows = Paragraph::new(status.as_str())
                .wrap(Wrap { trim: true })
                .line_count(width)
                .min(usize::from(u16::MAX)) as u16;
            let height = area.height.min(text_rows.saturating_add(2));
            let content = Rect::new(
                area.x + area.width.saturating_sub(width) / 2,
                area.y + area.height.saturating_sub(height) / 2,
                width,
                height,
            );
            let hint = if width < 15 && worker.is_none() {
                "r/q"
            } else if width < 28 && worker.is_none() {
                "r retry  q quit"
            } else if width < 6 {
                "q"
            } else if width < 15 {
                "q quit"
            } else if worker.is_some() {
                "q / Esc to quit"
            } else {
                "r to retry   q / Esc to quit"
            };
            let body_height = height.saturating_sub(2);
            frame.render_widget(
                Paragraph::new(status.as_str())
                    .style(
                        Style::default()
                            .fg(if worker.is_some() {
                                theme.accent
                            } else {
                                theme.error
                            })
                            .add_modifier(Modifier::BOLD),
                    )
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true }),
                Rect::new(content.x, content.y, width, body_height.max(1).min(height)),
            );
            if height > 2 {
                frame.render_widget(
                    Paragraph::new(Line::from(hint))
                        .style(Style::default().fg(theme.text_muted))
                        .alignment(Alignment::Center),
                    Rect::new(content.x, content.y + height - 1, width, 1),
                );
            }
        })?;
        if input_event_poll(Duration::from_millis(16))? {
            if let Event::Key(key) = input_event_read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(None)
                    }
                    KeyCode::Char('r') if worker.is_none() => {
                        status = "Loading remote snapshot...".to_owned();
                        worker = Some(start(
                            url.to_owned(),
                            config.clone(),
                            args.clone(),
                            light,
                            session.clone(),
                            last_applied_seq,
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
}
