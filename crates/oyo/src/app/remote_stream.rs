use super::{App, FileDiskStamp};
use crate::remote::stream::{self, RemoteEvent};
use crate::toasts::ToastEvent;

pub(super) type ConfigureRemoteReview = Box<dyn Fn(&mut App) -> anyhow::Result<()> + Send>;

impl App {
    pub(crate) fn remote_manifest_complete(&self) -> bool {
        self.review_live_diff()
            .remote_snapshot
            .as_ref()
            .is_none_or(|s| {
                s["acquisition"] != "patch-v1"
                    || s["streamState"] == "complete"
                        && self
                            .remote_validation
                            .as_ref()
                            .is_some_and(|proof| proof.matches(s))
            })
    }

    pub(crate) fn remote_manifest_status(&self) -> Option<String> {
        let snapshot = self.multi_diff.remote_snapshot.as_ref()?;
        if snapshot["acquisition"] != "patch-v1" {
            return None;
        }
        match snapshot["streamState"].as_str() {
            Some("receiving") => Some(format!(
                "files {}/{}; unverified, read-only",
                self.multi_diff.file_count(),
                snapshot["expectedFiles"]
                    .as_u64()
                    .map_or_else(|| "?".to_owned(), |count| count.to_string())
            )),
            Some("failed") => Some("unverified patch; refresh to retry".to_owned()),
            _ if self
                .multi_diff
                .file_is_sparse(self.multi_diff.selected_index) =>
            {
                Some(
                    if self
                        .multi_diff
                        .hydration_failure(self.multi_diff.selected_index)
                        .is_some()
                    {
                        "full contents unavailable; refresh to retry"
                    } else {
                        "loading selected file contents"
                    }
                    .to_owned(),
                )
            }
            _ if self
                .multi_diff
                .has_patch_display(self.multi_diff.selected_index)
                && self.multi_diff.current_file_diff_status()
                    == oyo_core::multi::DiffStatus::Ready =>
            {
                Some("metadata change".to_owned())
            }
            _ if self
                .multi_diff
                .has_patch_display(self.multi_diff.selected_index) =>
            {
                Some("full diff preparing; patch retained".to_owned())
            }
            _ => None,
        }
    }

    pub(crate) fn poll_remote_stream(&mut self) -> bool {
        let Some(mut session) = self.remote_stream.take() else {
            return false;
        };
        let mut changed = false;
        let mut finished = false;
        let mut batch_bytes = 0usize;
        let mut batch_files = 0usize;
        for _ in 0..4 {
            match session.try_recv() {
                Ok(RemoteEvent::Files(patches)) => {
                    for patch in patches {
                        batch_bytes += patch.byte_len();
                        batch_files += 1;
                        session.append(&mut self.multi_diff, patch);
                    }
                    self.extend_remote_file_slots();
                    changed = true;
                    if batch_bytes >= 4 * 1024 * 1024 || batch_files >= 128 {
                        break;
                    }
                }
                Ok(RemoteEvent::Complete(proof)) => {
                    if let Err(error) = self.install_remote_proof(proof) {
                        if let Some(snapshot) = self.multi_diff.remote_snapshot.as_mut() {
                            snapshot["streamState"] = "failed".into();
                        }
                        self.notify(ToastEvent::SelectionActionFailed(format!(
                            "Snapshot validation failed: {error}"
                        )));
                        finished = true;
                        changed = true;
                        break;
                    }
                    let selected = self.multi_diff.selected_index;
                    let scroll = self.scroll_offset;
                    let horizontal = self.horizontal_scroll;
                    if let Some(configure) = self.pending_remote_review.take() {
                        if let Err(error) = configure(self) {
                            self.review_mode = false;
                            self.pending_remote_review = Some(configure);
                            self.notify(ToastEvent::SelectionActionFailed(format!(
                                "Review setup failed: {error}"
                            )));
                        }
                    }
                    self.multi_diff.selected_index = selected;
                    self.scroll_offset = scroll;
                    self.horizontal_scroll = horizontal;
                    self.needs_scroll_to_active = false;
                    finished = true;
                    changed = true;
                    break;
                }
                Ok(RemoteEvent::Failed(error)) => {
                    if let Some(snapshot) = self.multi_diff.remote_snapshot.as_mut() {
                        snapshot["streamState"] = "failed".into();
                    }
                    self.notify(ToastEvent::SelectionActionFailed(format!(
                        "Incomplete remote patch: {error}"
                    )));
                    finished = true;
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if let Some(snapshot) = self.multi_diff.remote_snapshot.as_mut() {
                        snapshot["streamState"] = "failed".into();
                    }
                    self.notify(ToastEvent::SelectionActionFailed(
                        "Remote patch stopped before validation".to_owned(),
                    ));
                    finished = true;
                    changed = true;
                    break;
                }
            }
        }
        if !finished {
            self.remote_stream = Some(session);
        }
        if changed {
            self.mark_diff_changed();
        }
        changed
    }

    fn extend_remote_file_slots(&mut self) {
        let count = self.multi_diff.file_count();
        self.scroll_offsets_step.resize(count, 0);
        self.scroll_offsets_no_step.resize(count, 0);
        self.horizontal_scrolls_step.resize(count, 0);
        self.horizontal_scrolls_no_step.resize(count, 0);
        self.max_line_widths_step.resize(count, 0);
        self.max_line_widths_no_step.resize(count, 0);
        self.no_step_visited.resize(count, false);
        self.files_visited.resize(count, false);
        self.syntax_caches.resize(count, None);
        self.fold_scope_caches.resize(count, None);
        self.step_state_snapshots.resize(count, None);
        self.no_step_state_snapshots.resize(count, None);
        self.file_disk_baseline
            .resize(count, FileDiskStamp::default());
        self.file_disk_changed.resize(count, false);
        self.file_recently_changed_until.resize(count, None);
    }

    fn install_remote_proof(
        &mut self,
        proof: std::sync::Arc<stream::ValidatedSnapshot>,
    ) -> anyhow::Result<()> {
        let mismatches = proof.hydration_mismatches(&self.multi_diff)?;
        let positions = mismatches
            .iter()
            .filter_map(|index| self.capture_file_promotion(*index))
            .collect::<Vec<_>>();
        let rejected = stream::apply_proof(&mut self.multi_diff, &proof)?;
        for index in &rejected {
            if let Some(cache) = self.syntax_caches.get_mut(*index) {
                *cache = None;
            }
            if let Some(cache) = self.fold_scope_caches.get_mut(*index) {
                *cache = None;
            }
            if let Some(width) = self.max_line_widths_step.get_mut(*index) {
                *width = 0;
            }
            if let Some(width) = self.max_line_widths_no_step.get_mut(*index) {
                *width = 0;
            }
        }
        for position in positions {
            self.restore_file_promotion(position);
        }
        if !rejected.is_empty() {
            self.clear_fold_context_caches();
            self.syntax_scope_cache = None;
            self.mark_diff_changed();
            self.notify(ToastEvent::SelectionActionFailed("Full contents did not match the snapshot. The readable patch is retained; refresh to retry.".to_owned()));
        }
        self.remote_validation = Some(proof);
        Ok(())
    }

    pub(crate) fn ensure_remote_content(&mut self, index: usize) -> bool {
        if !self.multi_diff.ensure_full_content(index) {
            return false;
        }
        self.start_content_loading();
        true
    }

    pub(crate) fn request_remote_content(&mut self, index: usize) -> bool {
        if !self.multi_diff.request_full_content(index) {
            return false;
        }
        self.start_content_loading();
        true
    }

    pub(crate) fn require_selected_complete_content(&mut self) -> bool {
        let index = self.multi_diff.selected_index;
        if !self.multi_diff.file_is_sparse(index) {
            return true;
        }
        self.request_remote_content(index);
        self.notify(ToastEvent::SelectionActionFailed(
            "Loading full contents. The patch remains readable.".to_owned(),
        ));
        false
    }
}

pub(super) fn with_unloaded_context(view: Vec<oyo_core::ViewLine>) -> Vec<oyo_core::ViewLine> {
    use oyo_core::{LineKind, ViewLine, ViewSpan, ViewSpanKind};
    let mut result = Vec::with_capacity(view.len() + 8);
    let mut old = 0usize;
    let mut new = 0usize;
    let gap = |next: usize| ViewLine {
        content: "Context is not loaded.".to_owned(),
        spans: vec![ViewSpan {
            text: "Context is not loaded.".to_owned(),
            kind: ViewSpanKind::Equal,
        }],
        kind: LineKind::Unloaded,
        old_line: None,
        new_line: None,
        is_active: false,
        is_active_change: false,
        is_primary_active: false,
        show_hunk_extent: false,
        change_id: next,
        hunk_index: None,
        has_changes: false,
    };
    for line in view {
        if line.old_line.is_some_and(|n| n > old.saturating_add(1))
            || line.new_line.is_some_and(|n| n > new.saturating_add(1))
        {
            result.push(gap(line.change_id));
        }
        if let Some(n) = line.old_line {
            old = n;
        }
        if let Some(n) = line.new_line {
            new = n;
        }
        result.push(line);
    }
    if let Some(last) = result.last() {
        if old > 0 || new > 0 {
            result.push(gap(last.change_id));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{TopbarTabContent, ViewMode};
    use oyo_core::multi::ContentSource;
    use oyo_core::patch::PatchRecord;
    use oyo_core::AnimationFrame;
    use oyo_core::MultiFileDiff;
    use serde_json::json;

    fn sparse_app() -> App {
        let patch=PatchRecord::parse(b"diff --git a/f b/f\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -40 +40 @@\n-old\n+new\n@@ -80 +80 @@\n-before\n+after\n".to_vec()).unwrap();
        let mut diff = MultiFileDiff::from_pending_files(None, Vec::new(), true);
        diff.append_patch(patch, ContentSource::Empty, ContentSource::Empty);
        diff.remote_snapshot = Some(
            json!({"acquisition":"patch-v1","streamState":"receiving","expectedFiles":2,"target":"https://github.com/o/r/pull/1","provider":"github"}),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.stepping = false;
        app.multi_diff.current_navigator().goto_end();
        app
    }

    #[test]
    fn a_json_completion_flag_does_not_authorize_review_or_sync() {
        let mut app = sparse_app();
        assert!(!app.remote_manifest_complete());
        app.multi_diff.remote_snapshot.as_mut().unwrap()["streamState"] = "complete".into();
        assert!(!app.remote_manifest_complete());
        app.enable_review_mode();
        assert!(!app.review_mode);
        assert!(crate::remote_sync::RemoteReviewContext::from_app(&app).is_err());
    }

    #[test]
    fn selected_nonzero_file_owns_initial_tab_and_append_keeps_state() {
        let mut diff = MultiFileDiff::from_file_pairs(vec![
            ("zero".into(), "a".into(), "b".into()),
            ("one".into(), "a".into(), "c".into()),
        ]);
        diff.selected_index = 1;
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        assert_eq!(app.topbar_tabs[0].content, TopbarTabContent::File(1));
        app.scroll_offsets_no_step[1] = 17;
        app.files_visited[1] = true;
        app.extend_remote_file_slots();
        assert_eq!(app.scroll_offsets_no_step[1], 17);
        assert!(app.files_visited[1]);
        assert_eq!(app.multi_diff.selected_index, 1);
    }

    #[test]
    fn hydration_restores_latest_logical_position_across_changed_diff_shape() {
        let mut app = sparse_app();
        let view = app.current_view_with_frame(AnimationFrame::Idle);
        let row = view
            .iter()
            .position(|line| line.old_line == Some(80))
            .unwrap();
        app.scroll_offset = row;
        app.horizontal_scroll = 7;
        let id = app
            .multi_diff
            .current_navigator()
            .diff()
            .changes
            .iter()
            .find(|c| c.spans[0].old_line == Some(80))
            .unwrap()
            .id;
        app.multi_diff
            .current_navigator()
            .set_cursor_hunk(1, Some(id));
        let position = app.capture_file_promotion(0).unwrap();
        let old = (1..=100)
            .map(|n| {
                if n == 40 {
                    "old".to_owned()
                } else if n == 80 {
                    "before".to_owned()
                } else {
                    format!("line {n}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let new = old.replace("old\n", "new\n").replace("before\n", "after\n");
        assert!(app.multi_diff.request_full_content(0));
        app.multi_diff.take_pending_content();
        assert!(app.multi_diff.apply_prepared_content(
            0,
            MultiFileDiff::prepare_remote_file_content(old.into_bytes(), new.into_bytes())
        ));
        app.restore_file_promotion(position);
        assert_eq!(app.horizontal_scroll, 7);
        let nav = app.multi_diff.current_navigator();
        let cursor = nav.state().cursor_change.unwrap();
        assert!(nav
            .diff()
            .changes
            .iter()
            .find(|c| c.id == cursor)
            .unwrap()
            .spans
            .iter()
            .any(|s| s.old_line == Some(80)));
        let view = app.current_view_with_frame(AnimationFrame::Idle);
        assert_eq!(view[app.scroll_offset].old_line, Some(80));
        assert!(view
            .iter()
            .all(|line| line.kind != oyo_core::LineKind::Unloaded));
    }

    #[test]
    fn unloaded_context_has_no_original_line_or_blame_location() {
        let mut app = sparse_app();
        let view = app.current_view_with_frame(AnimationFrame::Idle);
        let gaps = view
            .iter()
            .filter(|line| line.kind == oyo_core::LineKind::Unloaded)
            .collect::<Vec<_>>();
        assert!(!gaps.is_empty());
        assert!(gaps
            .iter()
            .all(|line| line.old_line.is_none() && line.new_line.is_none() && !line.has_changes));
        assert!(view.iter().any(|line| line.old_line == Some(40)));
        assert!(view.iter().any(|line| line.old_line == Some(80)));
    }
}
