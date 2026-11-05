use super::review::ReviewSide;
use super::types::NoStepState;
use super::{App, TopbarTabContent, ViewMode};
use oyo_core::{AnimationFrame, DiffNavigator, DiffResult, StepState, ViewLine};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SourceLine {
    side: ReviewSide,
    line: usize,
}

#[derive(Clone, Debug)]
struct ViewportAnchor {
    source: SourceLine,
    wrapped_row: usize,
    screen_row: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ViewKey {
    file: usize,
    mode: ViewMode,
    stepping: bool,
    wrapped: bool,
    tab: Option<usize>,
}

#[derive(Default)]
pub(super) struct RemotePositions {
    saved: HashMap<ViewKey, ViewportAnchor>,
    promoted: HashSet<usize>,
    visible: Vec<ViewportAnchor>,
    frame: Option<(u16, u16)>,
    measuring: bool,
    recording_frame: bool,
    restore: Option<ViewportAnchor>,
    next_scroll: Option<usize>,
    preserved_scroll: Option<(ViewKey, usize)>,
}

pub(crate) struct RemoteRowGeometry {
    pub side: Option<ReviewSide>,
    pub source_scroll: usize,
    pub rendered_start: usize,
    pub viewport_start: usize,
    pub wrap_count: usize,
}

#[derive(Clone)]
struct LogicalNavigation {
    state: StepState,
    cursor: Option<SourceLine>,
    applied: Option<SourceLine>,
    hunk: Option<SourceLine>,
    at_end: bool,
}

pub(super) struct FilePromotion {
    index: usize,
    current: LogicalNavigation,
    step: Option<LogicalNavigation>,
    no_step: Option<LogicalNavigation>,
    tabs: Vec<(usize, LogicalNavigation)>,
    horizontal: Option<usize>,
}

fn source_for_change(diff: &DiffResult, id: usize) -> Option<SourceLine> {
    let span = diff
        .changes
        .iter()
        .find(|change| change.id == id)?
        .spans
        .first()?;
    if span.kind == oyo_core::ChangeKind::Delete {
        span.old_line.map(|line| SourceLine {
            side: ReviewSide::Old,
            line,
        })
    } else {
        span.new_line
            .map(|line| SourceLine {
                side: ReviewSide::New,
                line,
            })
            .or_else(|| {
                span.old_line.map(|line| SourceLine {
                    side: ReviewSide::Old,
                    line,
                })
            })
    }
}

fn change_for_source(diff: &DiffResult, source: SourceLine) -> Option<usize> {
    let nearest = |side| {
        diff.changes
            .iter()
            .filter_map(|change| {
                change
                    .spans
                    .iter()
                    .filter_map(|span| match side {
                        ReviewSide::Old => span.old_line,
                        ReviewSide::New => span.new_line,
                    })
                    .map(|line| line.abs_diff(source.line))
                    .min()
                    .map(|distance| (change.id, distance))
            })
            .min_by_key(|(_, distance)| *distance)
            .map(|(id, _)| id)
    };
    nearest(source.side).or_else(|| {
        nearest(match source.side {
            ReviewSide::Old => ReviewSide::New,
            ReviewSide::New => ReviewSide::Old,
        })
    })
}

fn logical_navigation(nav: &DiffNavigator, state: StepState) -> LogicalNavigation {
    LogicalNavigation {
        cursor: state
            .cursor_change
            .and_then(|id| source_for_change(nav.diff(), id)),
        applied: state
            .applied_changes
            .last()
            .and_then(|id| source_for_change(nav.diff(), *id)),
        hunk: nav
            .diff()
            .hunks
            .get(state.current_hunk)
            .and_then(|h| h.change_ids.first())
            .and_then(|id| source_for_change(nav.diff(), *id)),
        at_end: state.is_at_end(),
        state,
    }
}

fn remap_navigation(logical: LogicalNavigation, nav: &DiffNavigator) -> StepState {
    let mut state = logical.state;
    state.total_steps = nav.state().total_steps;
    state.total_hunks = nav.state().total_hunks;
    state.cursor_change = logical
        .cursor
        .and_then(|source| change_for_source(nav.diff(), source));
    let applied = logical
        .applied
        .and_then(|source| change_for_source(nav.diff(), source));
    let steps = if logical.at_end {
        nav.diff().significant_changes.len()
    } else {
        applied
            .and_then(|id| {
                nav.diff()
                    .significant_changes
                    .iter()
                    .position(|other| *other == id)
            })
            .map(|n| n + 1)
            .unwrap_or(0)
    };
    state.current_step = steps;
    state.applied_changes = nav.diff().significant_changes[..steps].to_vec();
    state.active_change = None;
    state.animating_hunk = None;
    state.hunk_preview_mode = false;
    state.preview_from_backward = false;
    let anchor = state.cursor_change.or(applied).or_else(|| {
        logical
            .hunk
            .and_then(|source| change_for_source(nav.diff(), source))
    });
    state.current_hunk = anchor
        .and_then(|id| nav.hunk_index_for_change_id(id))
        .or_else(|| {
            logical
                .hunk
                .and_then(|source| change_for_source(nav.diff(), source))
                .and_then(|id| nav.hunk_index_for_change_id(id))
        })
        .unwrap_or(0);
    state.last_nav_was_hunk &= state.cursor_change.is_some();
    state
}

impl App {
    fn remote_view_key(&self) -> ViewKey {
        ViewKey {
            file: self.multi_diff.selected_index,
            mode: self.view_mode,
            stepping: self.stepping,
            wrapped: self.line_wrap,
            tab: self.active_topbar_tab,
        }
    }

    pub(super) fn remote_scroll_preserved(&self) -> bool {
        self.remote_positions
            .preserved_scroll
            .is_some_and(|(key, scroll)| {
                key == self.remote_view_key() && scroll == self.scroll_offset
            })
    }

    pub(super) fn release_remote_scroll(&mut self) {
        self.remote_positions.preserved_scroll = None;
    }

    pub(crate) fn begin_remote_position_frame(&mut self, area: Rect) {
        self.remote_positions.frame = Some((area.width, area.height));
        if self.remote_positions.measuring {
            self.remote_positions.visible.clear();
        }
        self.remote_positions.recording_frame = false;
        self.remote_positions.next_scroll = None;
    }

    pub(crate) fn record_remote_render_row(
        &mut self,
        line: &ViewLine,
        geometry: RemoteRowGeometry,
    ) {
        if self.outdated_diff_view.is_some() || !self.is_remote_session() {
            return;
        }
        if !self.remote_positions.recording_frame {
            self.remote_positions.visible.clear();
            self.remote_positions.recording_frame = true;
        }
        let sides = if let Some(side) = geometry.side {
            [Some(side), None]
        } else {
            [Some(ReviewSide::Old), Some(ReviewSide::New)]
        };
        for side in sides.into_iter().flatten() {
            let Some(number) = (match side {
                ReviewSide::Old => line.old_line,
                ReviewSide::New => line.new_line,
            }) else {
                continue;
            };
            let source = SourceLine { side, line: number };
            let start = geometry.rendered_start.max(geometry.viewport_start);
            let end = geometry.rendered_start.saturating_add(geometry.wrap_count);
            if start < end
                && start
                    < geometry
                        .viewport_start
                        .saturating_add(self.last_viewport_height)
            {
                self.remote_positions.visible.push(ViewportAnchor {
                    source,
                    wrapped_row: start - geometry.rendered_start,
                    screen_row: start - geometry.viewport_start,
                });
            }
            if let Some(target) = self
                .remote_positions
                .restore
                .as_ref()
                .filter(|target| target.source == source)
            {
                let wrapped = target
                    .wrapped_row
                    .min(geometry.wrap_count.saturating_sub(1));
                let next = if self.line_wrap {
                    geometry
                        .source_scroll
                        .saturating_add(wrapped)
                        .saturating_sub(target.screen_row)
                } else {
                    let delta = geometry.rendered_start as i64
                        - geometry.viewport_start as i64
                        - target.screen_row as i64;
                    self.scroll_offset.saturating_add_signed(delta as isize)
                };
                self.remote_positions.next_scroll.get_or_insert(next);
            }
        }
    }

    pub(crate) fn end_remote_position_frame(&mut self) {
        if let Some(next) = self.remote_positions.next_scroll.take() {
            if self.remote_positions.restore.is_some() {
                self.remote_positions.preserved_scroll = Some((self.remote_view_key(), next));
            }
            if next != self.scroll_offset {
                self.scroll_offset = next;
                self.needs_scroll_to_active = false;
                self.mark_diff_changed();
            }
        }
    }

    fn measure_remote_position_frame(&mut self) {
        if self.remote_positions.measuring {
            return;
        }
        let Some((width, height)) = self.remote_positions.frame else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        self.remote_positions.measuring = true;
        self.unified_render_cache = None;
        let backend = ratatui::backend::TestBackend::new(width, height);
        let Ok(mut terminal) = ratatui::Terminal::new(backend);
        let _ = terminal.draw(|frame| crate::ui::draw(frame, self));
        self.remote_positions.measuring = false;
    }

    pub(super) fn remember_remote_viewport(&mut self) {
        if self.outdated_diff_view.is_some()
            || self.remote_positions.measuring
            || !self.is_remote_session()
            || self.view_mode == ViewMode::Preview
        {
            return;
        }
        let preferred = self.review_line_add_side.unwrap_or(ReviewSide::New);
        self.measure_remote_position_frame();
        let anchor = self
            .remote_positions
            .visible
            .iter()
            .min_by_key(|anchor| (anchor.screen_row, anchor.source.side != preferred))
            .cloned()
            .or_else(|| {
                let view = self.current_view_with_frame(AnimationFrame::Idle);
                view.iter()
                    .skip(self.scroll_offset.saturating_sub(self.view_window_start))
                    .find_map(|line| {
                        line.new_line
                            .map(|n| (ReviewSide::New, n))
                            .or_else(|| line.old_line.map(|n| (ReviewSide::Old, n)))
                    })
                    .map(|(side, line)| ViewportAnchor {
                        source: SourceLine { side, line },
                        wrapped_row: 0,
                        screen_row: 0,
                    })
            });
        if let Some(anchor) = anchor {
            let key = self.remote_view_key();
            self.remote_positions.saved.insert(key, anchor);
        }
    }

    pub(crate) fn restore_remote_viewport(&mut self) {
        if self.outdated_diff_view.is_some()
            || self.remote_positions.measuring
            || !self
                .remote_positions
                .promoted
                .contains(&self.multi_diff.selected_index)
            || self.view_mode == ViewMode::Preview
        {
            return;
        }
        let key = self.remote_view_key();
        let Some(mut anchor) = self.remote_positions.saved.remove(&key) else {
            return;
        };
        let view = self.current_view_with_frame(AnimationFrame::Idle);
        let nearest = |side| {
            view.iter()
                .filter_map(|line| match side {
                    ReviewSide::Old => line.old_line,
                    ReviewSide::New => line.new_line,
                })
                .min_by_key(|line| line.abs_diff(anchor.source.line))
        };
        if let Some(line) = nearest(anchor.source.side) {
            anchor.source.line = line;
        } else {
            let side = match anchor.source.side {
                ReviewSide::Old => ReviewSide::New,
                ReviewSide::New => ReviewSide::Old,
            };
            if let Some(line) = nearest(side) {
                anchor.source = SourceLine { side, line };
            }
        }
        let mut row = 0usize;
        let mut target = None;
        for line in view.iter() {
            let present = match anchor.source.side {
                ReviewSide::Old => line.old_line,
                ReviewSide::New => line.new_line,
            };
            if present == Some(anchor.source.line) {
                target = Some(row);
                break;
            }
            let visible = match self.view_mode {
                ViewMode::Split => {
                    present.is_some()
                        || super::utils::is_fold_line(line)
                        || (self.split_align_lines
                            && (line.old_line.is_some() || line.new_line.is_some()))
                }
                ViewMode::Evolution => !matches!(
                    line.kind,
                    oyo_core::LineKind::Deleted | oyo_core::LineKind::PendingDelete
                ),
                _ => true,
            };
            row += usize::from(visible);
        }
        if let Some(target) = target {
            self.scroll_offset = self.view_window_start + target.saturating_sub(anchor.screen_row);
        }
        self.needs_scroll_to_active = false;
        self.mark_diff_changed();
        self.remote_positions.preserved_scroll = Some((key, self.scroll_offset));
        self.remote_positions.restore = Some(anchor);
        for _ in 0..3 {
            let before = self.scroll_offset;
            self.measure_remote_position_frame();
            if before == self.scroll_offset {
                break;
            }
        }
        self.remote_positions.restore = None;
        self.needs_scroll_to_active = false;
    }

    pub(super) fn capture_file_promotion(&mut self, index: usize) -> Option<FilePromotion> {
        if index == self.multi_diff.selected_index {
            self.remember_remote_viewport();
        }
        let nav = self.multi_diff.navigator_for(index)?;
        let current = logical_navigation(nav, nav.state().clone());
        let step = self
            .step_state_snapshots
            .get(index)
            .and_then(Clone::clone)
            .map(|state| logical_navigation(nav, state));
        let no_step = self
            .no_step_state_snapshots
            .get(index)
            .and_then(|slot| *slot)
            .map(|saved| {
                let mut state = nav.state().clone();
                state.cursor_change = saved.cursor_change;
                state.current_hunk = saved.current_hunk;
                state.last_nav_was_hunk = saved.last_nav_was_hunk;
                logical_navigation(nav, state)
            });
        let tabs = self
            .topbar_tabs
            .iter()
            .filter(|tab| tab.content == TopbarTabContent::File(index))
            .filter_map(|tab| {
                tab.navigator_state
                    .clone()
                    .map(|state| (tab.id, logical_navigation(nav, state)))
            })
            .collect();
        Some(FilePromotion {
            index,
            current,
            step,
            no_step,
            tabs,
            horizontal: (index == self.multi_diff.selected_index).then_some(self.horizontal_scroll),
        })
    }

    pub(super) fn restore_file_promotion(&mut self, promotion: FilePromotion) {
        let index = promotion.index;
        if self.multi_diff.has_patch_display(index)
            && !self.multi_diff.file_is_sparse(index)
            && self.multi_diff.diff_status(index) != oyo_core::multi::DiffStatus::Ready
        {
            return;
        }
        self.multi_diff.ensure_full_navigator(index);
        let Some(nav) = self.multi_diff.navigator_for(index) else {
            return;
        };
        let mut current = remap_navigation(promotion.current, nav);
        let step = promotion.step.map(|state| remap_navigation(state, nav));
        let no_step = promotion.no_step.map(|state| {
            let state = remap_navigation(state, nav);
            NoStepState {
                current_hunk: state.current_hunk,
                cursor_change: state.cursor_change,
                last_nav_was_hunk: state.last_nav_was_hunk,
            }
        });
        let tabs = promotion
            .tabs
            .into_iter()
            .map(|(id, state)| (id, remap_navigation(state, nav)))
            .collect::<Vec<_>>();
        if index == self.multi_diff.selected_index && promotion.horizontal.is_none() {
            if let Some((_, state)) = tabs
                .iter()
                .find(|(id, _)| Some(*id) == self.active_topbar_tab)
            {
                current = state.clone();
            }
        }
        self.multi_diff.restore_navigator_state(index, current);
        self.step_state_snapshots[index] = step;
        self.no_step_state_snapshots[index] = no_step;
        for (id, state) in tabs {
            if let Some(tab) = self.topbar_tabs.iter_mut().find(|tab| tab.id == id) {
                tab.navigator_state = Some(state);
            }
        }
        self.remote_positions.promoted.insert(index);
        self.mark_diff_changed();
        if index == self.multi_diff.selected_index {
            if self.diff_selection.is_some() || self.diff_selection_cursor.is_some() {
                self.clear_diff_selection();
                self.notify(crate::toasts::ToastEvent::SelectionActionStarted(
                    "Selection cleared after displayed contents changed. Select the text again."
                        .to_owned(),
                ));
            }
            self.clear_fold_context_caches();
            self.restore_remote_viewport();
            if let Some(horizontal) = promotion.horizontal {
                self.horizontal_scroll = horizontal;
            }
            self.needs_scroll_to_active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::review::{ReviewRange, ReviewTargetKind};
    use crate::config::SyntaxMode;
    use oyo_core::multi::{ContentSource, MultiFileDiff};
    use oyo_core::patch::PatchRecord;
    use serde_json::json;

    fn contents() -> (String, String) {
        let old = (1..=100)
            .map(|n| {
                if n == 40 {
                    "old ".repeat(80)
                } else if n == 80 {
                    "before".to_owned()
                } else {
                    format!("line {n}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let new = old
            .replace(&"old ".repeat(80), &"new ".repeat(80))
            .replace("before\n", "after\n");
        (old, new)
    }

    fn app(mode: ViewMode, wrapped: bool, aligned: bool) -> App {
        let bytes=format!("diff --git a/f b/f\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -40 +40 @@\n-{}\n+{}\n@@ -80 +80 @@\n-before\n+after\n","old ".repeat(80),"new ".repeat(80)).into_bytes();
        let mut diff = MultiFileDiff::from_pending_files(None, Vec::new(), true);
        diff.append_patch(
            PatchRecord::parse(bytes).unwrap(),
            ContentSource::Empty,
            ContentSource::Empty,
        );
        diff.append_patch(PatchRecord::parse(b"diff --git a/g b/g\nindex 1111111..2222222 100644\n--- a/g\n+++ b/g\n@@ -1 +1 @@\n-old\n+new\n".to_vec()).unwrap(),ContentSource::Empty,ContentSource::Empty);
        diff.remote_snapshot = Some(
            json!({"acquisition":"patch-v1","streamState":"receiving","expectedFiles":2,"target":"https://github.com/o/r/pull/1","provider":"github"}),
        );
        let mut app = App::new(diff, mode, 0, false, None);
        app.stepping = false;
        app.syntax_mode = SyntaxMode::Off;
        app.line_wrap = wrapped;
        app.split_align_lines = aligned;
        app.multi_diff.current_navigator().goto_end();
        app.needs_scroll_to_active = false;
        app.centered_once = true;
        app
    }

    fn draw(app: &mut App) {
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let Ok(mut terminal) = ratatui::Terminal::new(backend);
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
    }

    fn apply(app: &mut App, index: usize) {
        let state = app.capture_file_promotion(index).unwrap();
        let (old, new) = contents();
        assert!(app.multi_diff.request_full_content(index));
        app.multi_diff.take_pending_content();
        assert!(app.multi_diff.apply_prepared_content(
            index,
            MultiFileDiff::prepare_remote_file_content(old.into_bytes(), new.into_bytes())
        ));
        app.restore_file_promotion(state);
    }

    #[test]
    fn rejected_full_bytes_return_to_nearest_patch_anchor_without_opening_context() {
        use oyo_core::multi::{ExpectedFileContent, PendingFileContent};
        use oyo_core::patch::BlobId;
        let (old, new) = contents();
        let old_source = ContentSource::RemoteBlob {
            provider: "gitlab".to_owned(),
            host: "fixture.test".to_owned(),
            repo: "11".to_owned(),
            revision: "1".repeat(40),
            path: "f".to_owned(),
        };
        let new_source = ContentSource::RemoteBlob {
            provider: "gitlab".to_owned(),
            host: "fixture.test".to_owned(),
            repo: "22".to_owned(),
            revision: "2".repeat(40),
            path: "f".to_owned(),
        };
        let patch=format!("diff --git a/f b/f\nindex {}..{} 100644\n--- a/f\n+++ b/f\n@@ -40 +40 @@\n-{}\n+{}\n@@ -80 +80 @@\n-before\n+after\n",BlobId::from_bytes(old.as_bytes()).unwrap(),BlobId::from_bytes(new.as_bytes()).unwrap(),"old ".repeat(80),"new ".repeat(80));
        let mut diff = MultiFileDiff::from_pending_files(None, Vec::new(), true);
        diff.append_patch(
            PatchRecord::parse(patch.into_bytes()).unwrap(),
            old_source.clone(),
            new_source.clone(),
        );
        diff.remote_snapshot = Some(json!({"acquisition":"patch-v1","streamState":"receiving"}));
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.stepping = false;
        app.syntax_mode = SyntaxMode::Off;
        assert!(app.multi_diff.request_full_content(0));
        app.multi_diff.take_pending_content();
        app.multi_diff.apply_prepared_content(
            0,
            MultiFileDiff::prepare_remote_file_content(
                old.clone().into_bytes(),
                new.replace("line 50\n", "counterfeit 50\n").into_bytes(),
            ),
        );
        app.multi_diff.current_navigator().goto_end();
        app.needs_scroll_to_active = false;
        app.centered_once = true;
        draw(&mut app);
        assert!(app.goto_line_on_side(true, 50));
        draw(&mut app);
        let nav = app.multi_diff.current_navigator();
        let before = source_for_change(nav.diff(), nav.state().cursor_change.unwrap()).unwrap();
        assert_eq!(before.line, 50);
        let horizontal = app.horizontal_scroll;
        let position = app.capture_file_promotion(0).unwrap();
        let expected = vec![ExpectedFileContent {
            request: PendingFileContent {
                file_index: 0,
                old: old_source,
                new: new_source,
            },
            old: Some(BlobId::from_bytes(old.as_bytes()).unwrap()),
            new: Some(BlobId::from_bytes(new.as_bytes()).unwrap()),
        }];
        assert_eq!(
            app.multi_diff
                .bind_remote_content_proof(expected.into())
                .unwrap(),
            vec![0]
        );
        app.restore_file_promotion(position);
        assert_eq!(app.horizontal_scroll, horizontal);
        assert!(app.fold_context.is_enabled());
        let nav = app.multi_diff.current_navigator();
        let cursor = nav.state().cursor_change.unwrap();
        assert_eq!(
            source_for_change(nav.diff(), cursor),
            Some(SourceLine {
                side: before.side,
                line: 40
            })
        );
        assert!(app.multi_diff.file_is_sparse(0));
        assert!(app.multi_diff.file_contents(0).is_none());
        assert!(!app.multi_diff.ensure_full_content(0));
    }

    #[test]
    fn selection_created_during_hydration_is_cleared_when_layout_promotes() {
        let mut app = app(ViewMode::UnifiedPane, false, true);
        draw(&mut app);
        assert!(app.multi_diff.request_full_content(0));
        app.multi_diff.take_pending_content();
        app.diff_view_area = Some((0, 0, 20, 2));
        app.set_diff_selection_cells(vec![vec!["x".to_owned(); 20]; 2]);
        assert!(app.start_diff_selection(8, 0));
        assert!(app.finish_diff_selection(9, 0));
        assert!(app.selection_toolbar_visible());
        let promotion = app.capture_file_promotion(0).unwrap();
        let (old, new) = contents();
        assert!(app.multi_diff.apply_prepared_content(
            0,
            MultiFileDiff::prepare_remote_file_content(old.into_bytes(), new.into_bytes())
        ));
        app.restore_file_promotion(promotion);
        assert!(app.diff_selection.is_none());
        assert!(app.diff_selection_cursor.is_none());
        assert!(!app.selection_toolbar_visible());
    }

    #[test]
    fn inactive_eviction_restores_the_returning_duplicate_tabs_cursor_and_horizontal() {
        let mut app = app(ViewMode::UnifiedPane, false, true);
        apply(&mut app, 0);
        let first_tab = app.active_topbar_tab.unwrap();
        let set_cursor = |app: &mut App, line| {
            let nav = app.multi_diff.current_navigator();
            let id = nav
                .diff()
                .changes
                .iter()
                .find(|change| change.spans.iter().any(|span| span.old_line == Some(line)))
                .unwrap()
                .id;
            let mut state = nav.state().clone();
            state.cursor_change = Some(id);
            nav.set_state(state);
        };
        set_cursor(&mut app, 40);
        app.horizontal_scroll = 30;
        app.save_active_topbar_tab_state();
        app.open_file_in_new_topbar_tab(0);
        let second_tab = app.active_topbar_tab.unwrap();
        set_cursor(&mut app, 80);
        app.horizontal_scroll = 12;
        app.save_active_topbar_tab_state();
        app.open_file_in_new_topbar_tab(1);
        app.horizontal_scroll = 0;
        let saved = app.capture_file_promotion(0).unwrap();
        assert!(saved.horizontal.is_none());
        assert!(app.multi_diff.evict_remote_content(0));
        app.step_state_snapshots[0] = None;
        app.no_step_state_snapshots[0] = None;
        for tab in app
            .topbar_tabs
            .iter_mut()
            .filter(|tab| tab.content == TopbarTabContent::File(0))
        {
            tab.navigator_state = None;
        }
        app.select_topbar_tab(first_tab);
        let (old, new) = contents();
        app.multi_diff.request_full_content(0);
        app.multi_diff.take_pending_content_for(0);
        app.multi_diff.apply_prepared_content(
            0,
            MultiFileDiff::prepare_remote_file_content(old.into_bytes(), new.into_bytes()),
        );
        app.restore_file_promotion(saved);
        let nav = app.multi_diff.current_navigator();
        assert_eq!(
            source_for_change(nav.diff(), nav.state().cursor_change.unwrap())
                .unwrap()
                .line,
            40
        );
        assert_eq!(app.horizontal_scroll, 30);
        app.select_topbar_tab(second_tab);
        let nav = app.multi_diff.current_navigator();
        assert_eq!(
            source_for_change(nav.diff(), nav.state().cursor_change.unwrap())
                .unwrap()
                .line,
            80
        );
        assert_eq!(app.horizontal_scroll, 12);
    }

    #[test]
    fn inactive_file_and_tab_states_rebase_before_return_and_mode_toggle() {
        let mut app = app(ViewMode::UnifiedPane, false, true);
        draw(&mut app);
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
        app.multi_diff.current_navigator().set_hunk_scope(true);
        app.save_step_state_snapshot(0);
        app.save_no_step_state_snapshot(0);
        app.save_active_topbar_tab_state();
        let mut tab = app.topbar_tabs[0].clone();
        tab.id = 2;
        app.topbar_tabs.push(tab);
        app.select_file(1);
        let selected = app.multi_diff.selected_index;
        apply(&mut app, 0);
        assert_eq!(app.multi_diff.selected_index, selected);
        let saved = app
            .topbar_tabs
            .iter()
            .find(|tab| tab.id == 2)
            .unwrap()
            .navigator_state
            .as_ref()
            .unwrap();
        assert_eq!(
            source_for_change(
                app.multi_diff.navigator_for(0).unwrap().diff(),
                saved.cursor_change.unwrap()
            ),
            Some(SourceLine {
                side: ReviewSide::Old,
                line: 80
            })
        );
        app.select_file(0);
        let nav = app.multi_diff.current_navigator();
        assert_eq!(
            source_for_change(nav.diff(), nav.state().cursor_change.unwrap()),
            Some(SourceLine {
                side: ReviewSide::Old,
                line: 80
            })
        );
        app.toggle_stepping();
        let nav = app.multi_diff.current_navigator();
        assert_eq!(
            source_for_change(nav.diff(), nav.state().cursor_change.unwrap()),
            Some(SourceLine {
                side: ReviewSide::Old,
                line: 80
            })
        );
        app.select_topbar_tab(2);
        let nav = app.multi_diff.current_navigator();
        assert_eq!(
            source_for_change(nav.diff(), nav.state().cursor_change.unwrap()),
            Some(SourceLine {
                side: ReviewSide::Old,
                line: 80
            })
        );
    }

    #[test]
    fn promotion_keeps_rendered_source_and_wrapped_offset_with_split_and_notes() {
        for mode in [ViewMode::UnifiedPane, ViewMode::Split] {
            for wrapped in [false, true] {
                for aligned in [false, true] {
                    for side in [ReviewSide::Old, ReviewSide::New] {
                        for notes in [false, true] {
                            let mut app = app(mode, wrapped, aligned);
                            if notes {
                                let (old, new) = contents();
                                let mut full = App::new(
                                    MultiFileDiff::from_file_pair("f".into(), "f".into(), old, new),
                                    mode,
                                    0,
                                    false,
                                    None,
                                );
                                full.review_mode = true;
                                full.set_review_persist_enabled(false);
                                full.add_review_comment_from_cli(
                                    "f",
                                    ReviewTargetKind::Line,
                                    Some(ReviewSide::Old),
                                    Some(ReviewRange { start: 40, end: 40 }),
                                    None,
                                    "Retained draft near the changed line. ".repeat(8),
                                )
                                .unwrap();
                                app.review_comments = full.review_comments.clone();
                                app.review_mode = true;
                                app.set_review_persist_enabled(false);
                            }
                            app.scroll_offset = if wrapped { 3 } else { 1 };
                            app.review_line_add_side = Some(side);
                            draw(&mut app);
                            app.review_line_add_side = Some(side);
                            app.remember_remote_viewport();
                            let anchor = app
                                .remote_positions
                                .saved
                                .get(&app.remote_view_key())
                                .unwrap()
                                .clone();
                            app.review_line_add_side = Some(side);
                            apply(&mut app, 0);
                            draw(&mut app);
                            let visible=app.remote_positions.visible.iter().find(|other|other.source==anchor.source).unwrap_or_else(||panic!("source missing {mode:?} wrapped={wrapped} aligned={aligned} side={side:?} notes={notes} anchor={anchor:?} visible={:?} scroll={}",app.remote_positions.visible,app.scroll_offset));
                            assert_eq!((visible.wrapped_row,visible.screen_row),(anchor.wrapped_row,anchor.screen_row),"{mode:?} wrapped={wrapped} aligned={aligned} side={side:?} notes={notes} anchor={anchor:?} actual={visible:?} scroll={}",app.scroll_offset);
                            assert_eq!(app.review_comments.len(), usize::from(notes));
                        }
                    }
                }
            }
        }
    }
}
