use super::App;
use crate::{remote, BuiltDiff};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc;
use unicode_segmentation::UnicodeSegmentation;

pub(crate) struct RemoteOpen {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) phase: RemoteOpenPhase,
    pub(crate) footer_hits: Vec<crate::footer::FooterHit<crate::keybindings::LineInputAction>>,
}

pub(crate) enum RemoteOpenPhase {
    Editing(Option<String>),
    Loading(RemoteOpenWorker),
    Preparing(Box<App>),
}

pub(crate) struct OpenedRemote {
    pub(crate) built: BuiltDiff,
    pub(crate) patch_session: Option<remote::stream::PatchSession>,
}

pub(crate) struct RemoteOpenWorker {
    context: Option<remote::loading::ReadContext>,
    rx: mpsc::Receiver<Result<OpenedRemote, String>>,
}

impl Drop for RemoteOpenWorker {
    fn drop(&mut self) {
        if let Some(context) = &self.context {
            context.cancel();
        }
    }
}

impl RemoteOpen {
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        bindings: &mut crate::keybindings::Keybindings,
    ) -> bool {
        use crate::keybindings::{Dispatch, LineInputAction};
        if key.code == KeyCode::Esc {
            bindings.clear_sequence();
            return self.handle_action(LineInputAction::Cancel);
        }
        match bindings.goto(key) {
            Dispatch::Matched(action) => return self.handle_action(action),
            Dispatch::Pending => {}
            Dispatch::Unmatched => self.edit(key),
        }
        false
    }

    pub(crate) fn handle_action(&mut self, action: crate::keybindings::LineInputAction) -> bool {
        use crate::keybindings::LineInputAction;
        self.footer_hits.clear();
        match action {
            LineInputAction::Cancel => return true,
            LineInputAction::Accept => self.submit(),
            LineInputAction::Clear if !self.busy() => {
                self.text.clear();
                self.cursor = 0;
                self.phase = RemoteOpenPhase::Editing(None);
            }
            LineInputAction::Backspace => {
                self.edit(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            }
            _ => {}
        }
        false
    }

    pub(crate) fn mouse_action(
        &self,
        event: crossterm::event::MouseEvent,
    ) -> Option<crate::keybindings::LineInputAction> {
        if event.kind == crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
        {
            return crate::footer::action_at(&self.footer_hits, event.column, event.row);
        }
        None
    }

    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            phase: RemoteOpenPhase::Editing(None),
            footer_hits: Vec::new(),
        }
    }

    pub(crate) fn busy(&self) -> bool {
        !matches!(self.phase, RemoteOpenPhase::Editing(_))
    }

    pub(crate) fn error(&self) -> Option<&str> {
        match &self.phase {
            RemoteOpenPhase::Editing(error) => error.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn fail(&mut self, error: String) {
        self.footer_hits.clear();
        self.phase = RemoteOpenPhase::Editing(Some(error));
    }

    pub(crate) fn submit(&mut self) {
        self.footer_hits.clear();
        if self.busy() {
            return;
        }
        let url = self.text.trim().to_owned();
        if url.is_empty() {
            self.fail("Enter a remote review URL.".into());
            return;
        }
        let context = remote::loading::ReadContext::default();
        let worker_context = context.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = worker_context.run(|| {
                remote::stream::start(&url, worker_context.clone())
                    .map(|(built, patch_session)| OpenedRemote {
                        built,
                        patch_session,
                    })
                    .map_err(|error| error.to_string())
            });
            let _ = tx.send(result);
        });
        self.phase = RemoteOpenPhase::Loading(RemoteOpenWorker {
            context: Some(context),
            rx,
        });
    }

    pub(crate) fn poll(&mut self) -> Option<Result<OpenedRemote, String>> {
        let RemoteOpenPhase::Loading(worker) = &mut self.phase else {
            return None;
        };
        match worker.rx.try_recv() {
            Ok(result) => {
                if result.is_ok() {
                    worker.context.take();
                }
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Remote loader stopped.".into())),
        }
    }

    pub(crate) fn insert(&mut self, text: &str) {
        self.footer_hits.clear();
        if self.busy() {
            return;
        }
        let text: String = text.chars().filter(|ch| !ch.is_control()).collect();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.phase = RemoteOpenPhase::Editing(None);
    }

    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |ch| self.cursor + ch.len())
    }

    pub(crate) fn edit(&mut self, key: KeyEvent) {
        self.footer_hits.clear();
        if self.busy() {
            return;
        }
        match key.code {
            KeyCode::Left => self.cursor = self.previous(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Delete => {
                self.text.drain(self.cursor..self.next());
                self.phase = RemoteOpenPhase::Editing(None);
            }
            KeyCode::Backspace => {
                let start = self.previous();
                self.text.drain(start..self.cursor);
                self.cursor = start;
                self.phase = RemoteOpenPhase::Editing(None);
            }
            KeyCode::Char('a') if key.modifiers == KeyModifiers::CONTROL => self.cursor = 0,
            KeyCode::Char('e') if key.modifiers == KeyModifiers::CONTROL => {
                self.cursor = self.text.len()
            }
            KeyCode::Char(ch)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.insert(&ch.to_string())
            }
            _ => {}
        }
    }
}

impl App {
    pub(crate) fn start_remote_open(&mut self) {
        if self.request_settings_leave(super::settings::SettingsLeaveTarget::OpenRemote) {
            return;
        }
        self.stop_command_palette();
        self.reset_picker_cursor();
        self.remote_open = Some(RemoteOpen::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ViewMode;
    use oyo_core::MultiFileDiff;

    #[test]
    fn paste_and_cursor_edits_stay_on_grapheme_boundaries() {
        let mut entry = RemoteOpen::new();
        entry.insert("https://github.com/a/b/pull/1\r\n");
        entry.edit(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        entry.edit(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        entry.insert("2");
        assert_eq!(entry.text, "https://github.com/a/b/pull/2");
        entry.edit(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        entry.insert("e\u{301}");
        entry.edit(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(entry.cursor, 0);
        assert_eq!(entry.text, "https://github.com/a/b/pull/2");
    }

    #[test]
    fn closing_a_load_cancels_its_context_and_rejects_late_replies() {
        let mut app = App::new(
            MultiFileDiff::from_file_pairs(vec![("local".into(), "old".into(), "new".into())]),
            ViewMode::UnifiedPane,
            0,
            false,
            None,
        );
        app.start_remote_open();
        let context = remote::loading::ReadContext::default();
        let (tx, rx) = mpsc::channel();
        app.remote_open.as_mut().unwrap().phase = RemoteOpenPhase::Loading(RemoteOpenWorker {
            context: Some(context.clone()),
            rx,
        });
        app.remote_open = None;
        assert!(context.is_cancelled());
        app.start_remote_open();
        assert!(tx.send(Err("late failure".into())).is_err());
        let entry = app.remote_open.as_mut().unwrap();
        assert!(entry.poll().is_none());
        assert!(entry.error().is_none());
        assert_eq!(app.multi_diff.files[0].display_name, "local");
    }

    #[test]
    fn receiving_a_usable_review_transfers_cancellation_ownership() {
        let mut entry = RemoteOpen::new();
        let context = remote::loading::ReadContext::default();
        let (tx, rx) = mpsc::channel();
        entry.phase = RemoteOpenPhase::Loading(RemoteOpenWorker {
            context: Some(context.clone()),
            rx,
        });
        tx.send(Ok(OpenedRemote {
            built: BuiltDiff {
                multi_diff: MultiFileDiff::from_file_pairs(vec![(
                    "f".into(),
                    "old".into(),
                    "new".into(),
                )]),
                branch: None,
                workspace_root: None,
                remote_validation: None,
            },
            patch_session: None,
        }))
        .ok()
        .unwrap();
        let opened = entry.poll().unwrap().ok().unwrap();
        drop(entry);
        assert!(!context.is_cancelled());
        assert_eq!(opened.built.multi_diff.file_count(), 1);
    }

    #[test]
    fn load_errors_keep_the_url_editable_and_busy_input_cannot_start_another_worker() {
        let mut entry = RemoteOpen::new();
        entry.insert("bad-url");
        entry.fail("Invalid URL".into());
        entry.edit(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(entry.text, "bad-ur");
        assert!(entry.error().is_none());
        let context = remote::loading::ReadContext::default();
        let (_tx, rx) = mpsc::channel();
        entry.phase = RemoteOpenPhase::Loading(RemoteOpenWorker {
            context: Some(context),
            rx,
        });
        entry.insert("ignored");
        entry.submit();
        assert_eq!(entry.text, "bad-ur");
        assert!(entry.busy());
    }
}
