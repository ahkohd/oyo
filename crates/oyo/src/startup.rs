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
                let built = remote::build(&url)?;
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
                app.start_content_loading_with_context(worker_contents);
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
    let mut status = "Loading remote snapshot...".to_owned();
    loop {
        if let Some(active) = &worker {
            loop {
                let event = match active.rx.try_recv() {
                    Ok(event) => event,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        status = "Remote loading worker stopped. Press r to retry.".to_owned();
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
                        worker.as_mut().and_then(|worker| worker.contents.take());
                        return Ok(Some((app, prepared.control)));
                    }
                    StartupEvent::Ready(Err(error)) => {
                        status = format!("Remote loading failed\n\n{error}\n\nPress r to retry.");
                        worker = None;
                        break;
                    }
                }
            }
        }
        terminal.draw(|frame| {
            use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
            frame.render_widget(
                Paragraph::new(format!("{status}\n\nq / Esc to quit"))
                    .wrap(Wrap { trim: false })
                    .block(Block::default().borders(Borders::ALL).title("Oyo")),
                frame.area(),
            );
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
