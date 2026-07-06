use super::{review::ReviewSide, App, SelectionToolbarAction, SelectionToolbarHit, ViewMode};
use crate::app::utils::copy_to_clipboard;
use crate::config::{ReviewHookStdin, SelectionActionConfig};
use crate::toasts::ToastEvent;
use crossterm::event::KeyEvent;
use keymap::{parser::parse_seq, ToKeyMap};
use oyo_core::{AnimationFrame, LineKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use serde::Serialize;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffSelectionMode {
    Char,
    Line,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DiffSelection {
    start: (u16, u16),
    end: (u16, u16),
    col_start: u16,
    col_end: u16,
    mode: DiffSelectionMode,
    dragged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DiffSelectionCursor {
    point: (u16, u16),
    col_start: u16,
    col_end: u16,
}

#[derive(Serialize)]
struct SelectionActionPayload {
    version: u8,
    action: String,
    file: String,
    repo_root: String,
    view_mode: String,
    side: Option<String>,
    old_range: Option<[usize; 2]>,
    new_range: Option<[usize; 2]>,
    scroll_offset: usize,
    rows: Vec<u16>,
    text: String,
}

impl App {
    pub(crate) fn set_diff_selection_cells(&mut self, cells: Vec<Vec<String>>) {
        self.diff_selection_cells = cells;
    }

    pub(crate) fn set_selection_toolbar_hits(&mut self, hits: Vec<SelectionToolbarHit>) {
        if hits.is_empty() {
            self.selection_toolbar_rect = None;
            self.selection_toolbar_hover = None;
        }
        self.selection_toolbar_hits = hits;
    }

    pub(crate) fn set_selection_toolbar_rect(&mut self, rect: Option<(u16, u16, u16, u16)>) {
        self.selection_toolbar_rect = rect;
    }

    pub(crate) fn clear_diff_selection(&mut self) {
        self.diff_selection = None;
        self.diff_selection_cursor = None;
        self.hide_selection_toolbar();
    }

    fn hide_selection_toolbar(&mut self) {
        self.selection_toolbar_visible = false;
        self.selection_toolbar_rect = None;
        self.selection_toolbar_scroll = 0;
        self.selection_toolbar_width = None;
        self.selection_toolbar_hits.clear();
        self.selection_toolbar_hover = None;
    }

    pub(crate) fn selection_toolbar_visible(&self) -> bool {
        self.selection_toolbar_visible
    }

    pub(crate) fn show_selection_toolbar(&mut self) -> bool {
        if self.review_editor_active() || self.diff_selection_segments().is_empty() {
            return false;
        }
        self.selection_toolbar_visible = true;
        true
    }

    pub(crate) fn start_diff_selection(&mut self, column: u16, row: u16) -> bool {
        if self.review_editor_active() {
            return false;
        }
        if self.selection_toolbar_visible {
            self.clear_diff_selection();
            return true;
        }
        self.diff_selection = None;
        self.diff_selection_cursor = None;
        self.hide_selection_toolbar();
        let Some(point) = self.clamp_diff_selection_point(column, row) else {
            return false;
        };
        let Some((col_start, col_end)) = self.selectable_col_range(point.0) else {
            return false;
        };
        if self
            .diff_selection_cells
            .get(point.1 as usize)
            .and_then(|line| line.get(point.0 as usize))
            .is_some_and(|symbol| symbol.is_empty())
        {
            return false;
        }
        self.file_list_focused = false;
        self.file_filter_active = false;
        self.diff_selection_cursor = None;
        self.diff_selection = Some(DiffSelection {
            start: point,
            end: point,
            col_start,
            col_end,
            mode: DiffSelectionMode::Char,
            dragged: false,
        });
        true
    }

    pub(crate) fn drag_diff_selection(&mut self, column: u16, row: u16) -> bool {
        if self.review_editor_active() {
            return false;
        }
        if self.selection_toolbar_visible {
            return true;
        }
        self.hide_selection_toolbar();
        let Some(point) = self.clamp_diff_selection_point(column, row) else {
            return self.diff_selection.is_some();
        };
        self.set_diff_selection_end(point)
    }

    pub(crate) fn start_keyboard_selection(&mut self) -> bool {
        self.start_keyboard_selection_with_mode(DiffSelectionMode::Char)
    }

    pub(crate) fn start_keyboard_line_selection(&mut self) -> bool {
        self.start_keyboard_selection_with_mode(DiffSelectionMode::Line)
    }

    pub(crate) fn start_keyboard_block_selection(&mut self) -> bool {
        self.start_keyboard_selection_with_mode(DiffSelectionMode::Block)
    }

    fn start_keyboard_selection_with_mode(&mut self, mode: DiffSelectionMode) -> bool {
        let Some((point, (col_start, col_end))) = self.selection_cursor_or_first() else {
            return false;
        };
        self.hide_selection_toolbar();
        self.diff_selection_cursor = None;
        self.diff_selection = Some(DiffSelection {
            start: point,
            end: point,
            col_start,
            col_end,
            mode,
            dragged: true,
        });
        true
    }

    pub(crate) fn move_diff_selection(&mut self, delta_col: i16, delta_row: i16) -> bool {
        if self.diff_selection.is_none() && !self.start_keyboard_selection() {
            return false;
        }
        let Some(selection) = self.diff_selection else {
            return false;
        };
        let Some((_, _, _, height)) = self.diff_view_area else {
            return false;
        };
        if height == 0 || selection.col_start >= selection.col_end {
            return false;
        }
        if selection.mode == DiffSelectionMode::Char && delta_row == 0 && delta_col != 0 {
            let point = if delta_col < 0 {
                prev_char_point(&self.diff_selection_cells, selection)
            } else {
                next_char_point(&self.diff_selection_cells, selection)
            };
            return self.set_diff_selection_end(point.unwrap_or(selection.end));
        }
        let col = (selection.end.0 as i32 + delta_col as i32).clamp(
            selection.col_start as i32,
            selection.col_end.saturating_sub(1) as i32,
        ) as u16;
        let row = (selection.end.1 as i32 + delta_row as i32)
            .clamp(0, height.saturating_sub(1) as i32) as u16;
        self.set_diff_selection_end((col, row))
    }

    pub(crate) fn reanchor_diff_selection(&mut self, delta_col: i16, delta_row: i16) -> bool {
        let Some(selection) = self.selection_for_cursor_move() else {
            return false;
        };
        let point =
            match reanchor_point(&self.diff_selection_cells, selection, delta_col, delta_row) {
                Some(point) => point,
                None if delta_row < 0 || delta_col < 0 => {
                    self.scroll_up();
                    selection.end
                }
                None if delta_row > 0 || delta_col > 0 => {
                    self.scroll_down();
                    selection.end
                }
                None => return true,
            };
        let (col_start, col_end) = self
            .selectable_col_range(point.0)
            .unwrap_or((selection.col_start, selection.col_end));
        self.hide_selection_toolbar();
        self.diff_selection = None;
        self.diff_selection_cursor = Some(DiffSelectionCursor {
            point,
            col_start,
            col_end,
        });
        true
    }

    pub(crate) fn move_diff_selection_to_boundary(&mut self, end: bool) -> bool {
        if self.diff_selection.is_none() && !self.start_keyboard_selection() {
            return false;
        }
        let Some(selection) = self.diff_selection else {
            return false;
        };
        let point = boundary_point(&self.diff_selection_cells, selection, end);
        let Some(point) = point else {
            return false;
        };
        self.set_diff_selection_end(point)
    }

    pub(crate) fn move_diff_selection_half_page_down(&mut self) -> bool {
        if self.diff_selection.is_none() && !self.start_keyboard_selection() {
            return false;
        }
        let Some(selection) = self.diff_selection else {
            return false;
        };
        let rows = self.diff_selection_half_page_rows();
        let point = reanchor_point(&self.diff_selection_cells, selection, 0, rows)
            .or_else(|| boundary_point(&self.diff_selection_cells, selection, true));
        let Some(point) = point else {
            return false;
        };
        self.set_diff_selection_end(point)
    }

    pub(crate) fn reanchor_diff_selection_to_boundary(&mut self, end: bool) -> bool {
        let Some(selection) = self.selection_for_cursor_move_or_first() else {
            return false;
        };
        let Some(point) = boundary_point(&self.diff_selection_cells, selection, end) else {
            return false;
        };
        self.set_diff_selection_cursor(point, selection)
    }

    pub(crate) fn reanchor_diff_selection_half_page_down(&mut self) -> bool {
        let Some(selection) = self.selection_for_cursor_move_or_first() else {
            return false;
        };
        let rows = self.diff_selection_half_page_rows();
        if let Some(point) = reanchor_point(&self.diff_selection_cells, selection, 0, rows) {
            return self.set_diff_selection_cursor(point, selection);
        }
        let height = self
            .diff_view_area
            .map(|(_, _, _, height)| height as usize)
            .unwrap_or(1);
        self.scroll_half_page_down(height.max(1));
        self.set_diff_selection_cursor(selection.end, selection)
    }

    fn diff_selection_half_page_rows(&self) -> i16 {
        self.diff_view_area
            .map(|(_, _, _, height)| (height / 2).max(1).min(i16::MAX as u16) as i16)
            .unwrap_or(1)
    }

    fn set_diff_selection_cursor(&mut self, point: (u16, u16), selection: DiffSelection) -> bool {
        let (col_start, col_end) = self
            .selectable_col_range(point.0)
            .unwrap_or((selection.col_start, selection.col_end));
        self.hide_selection_toolbar();
        self.diff_selection = None;
        self.diff_selection_cursor = Some(DiffSelectionCursor {
            point,
            col_start,
            col_end,
        });
        true
    }

    fn set_diff_selection_end(&mut self, point: (u16, u16)) -> bool {
        self.hide_selection_toolbar();
        let Some(selection) = self.diff_selection.as_mut() else {
            return false;
        };
        selection.dragged |= point != selection.start;
        selection.end = point;
        true
    }

    pub(crate) fn finish_diff_selection(&mut self, column: u16, row: u16) -> bool {
        if self.review_editor_active() {
            return false;
        }
        if self.selection_toolbar_visible {
            return true;
        }
        if self.diff_selection.is_none() {
            return false;
        }
        self.drag_diff_selection(column, row);
        if self
            .diff_selection
            .is_some_and(|selection| !selection.dragged)
        {
            self.diff_selection = None;
            self.diff_selection_cursor = None;
            self.hide_selection_toolbar();
        } else {
            self.selection_toolbar_visible = true;
        }
        true
    }

    pub(crate) fn copy_diff_selection(&mut self) -> bool {
        let text = self.selected_diff_text();
        let copied = copy_to_clipboard(&text);
        if copied {
            self.notify(ToastEvent::CopiedSelection);
        } else {
            self.notify(ToastEvent::CopyFailed);
        }
        copied
    }

    pub(crate) fn selected_diff_text(&self) -> String {
        selected_text(&self.diff_selection_cells, &self.diff_selection_segments())
    }

    pub(crate) fn handle_selection_toolbar_click(&mut self, column: u16, row: u16) -> bool {
        if !self.selection_toolbar_visible || !self.selection_toolbar_contains(column, row) {
            return false;
        }
        if let Some(hit) = self.selection_toolbar_hits.iter().copied().find(|hit| {
            column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height)
        }) {
            return self.run_selection_toolbar_action(hit.action);
        }
        true
    }

    pub(crate) fn dismiss_selection_toolbar_click(&mut self, column: u16, row: u16) -> bool {
        if !self.selection_toolbar_visible || self.selection_toolbar_contains(column, row) {
            return false;
        }
        self.clear_diff_selection();
        true
    }

    fn selection_toolbar_contains(&self, column: u16, row: u16) -> bool {
        self.selection_toolbar_rect
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
    }

    pub(crate) fn mouse_over_selection_toolbar(&self, column: u16, row: u16) -> bool {
        self.selection_toolbar_visible && self.selection_toolbar_contains(column, row)
    }

    pub(crate) fn scroll_selection_toolbar_actions(&mut self, delta: isize) -> bool {
        let action_count = self
            .selection_actions
            .len()
            .saturating_add(2)
            .saturating_add(usize::from(
                self.review_mode() && self.view_mode != ViewMode::Preview,
            ));
        let max_scroll = action_count.saturating_sub(1);
        let old = self.selection_toolbar_scroll.min(max_scroll);
        let next = if delta.is_negative() {
            old.saturating_sub(delta.unsigned_abs())
        } else {
            old.saturating_add(delta as usize).min(max_scroll)
        };
        self.selection_toolbar_scroll = next;
        old != next
    }

    pub(crate) fn handle_selection_action_key(&mut self, key: KeyEvent) -> bool {
        let Some(idx) = self.selection_actions.iter().position(|action| {
            action
                .key
                .as_deref()
                .is_some_and(|binding| key_matches_config(key, binding))
        }) else {
            return false;
        };
        self.run_selection_toolbar_action(SelectionToolbarAction::Custom(idx))
    }

    fn run_selection_toolbar_action(&mut self, action: SelectionToolbarAction) -> bool {
        match action {
            SelectionToolbarAction::Copy => {
                self.copy_diff_selection();
                self.clear_diff_selection();
                true
            }
            SelectionToolbarAction::Comment => {
                if self.view_mode == ViewMode::Preview {
                    return false;
                }
                if !self.start_line_comment_for_selection() {
                    self.start_line_comment();
                }
                self.clear_diff_selection();
                true
            }
            SelectionToolbarAction::Cancel => {
                self.clear_diff_selection();
                true
            }
            SelectionToolbarAction::ScrollLeft => {
                self.scroll_selection_toolbar_actions(-1);
                true
            }
            SelectionToolbarAction::ScrollRight => {
                self.scroll_selection_toolbar_actions(1);
                true
            }
            SelectionToolbarAction::Custom(idx) => {
                self.run_selection_action(idx);
                self.clear_diff_selection();
                true
            }
        }
    }

    fn start_line_comment_for_selection(&mut self) -> bool {
        let Some(row) = self
            .diff_selection_segments()
            .into_iter()
            .map(|(row, _, _)| row)
            .min()
        else {
            return false;
        };
        let Some((_, y, _, _)) = self.diff_view_area else {
            return false;
        };
        self.start_line_comment_at_screen_row_on_side(
            y.saturating_add(row),
            self.diff_selection_review_side(),
        )
    }

    fn diff_selection_review_side(&self) -> Option<ReviewSide> {
        if self.view_mode != ViewMode::Split {
            return None;
        }
        let selection = self.diff_selection.or_else(|| {
            self.diff_selection_cursor.map(|cursor| DiffSelection {
                start: cursor.point,
                end: cursor.point,
                col_start: cursor.col_start,
                col_end: cursor.col_end,
                mode: DiffSelectionMode::Char,
                dragged: true,
            })
        })?;
        let ranges = self.diff_selection_content_ranges();
        let [left, right] = ranges.as_slice() else {
            return None;
        };
        let mid = selection
            .col_start
            .saturating_add(selection.col_end.saturating_sub(selection.col_start) / 2);
        if mid >= right.0 && mid < right.1 {
            Some(ReviewSide::New)
        } else if mid >= left.0 && mid < left.1 {
            Some(ReviewSide::Old)
        } else {
            None
        }
    }

    fn run_selection_action(&mut self, idx: usize) {
        let Some(action) = self.selection_actions.get(idx).cloned() else {
            return;
        };
        if action.command.trim().is_empty() {
            return;
        }
        let label = selection_action_label(&action);
        let success_message = selection_action_success_message(&action, &label);
        let failure_message = selection_action_failure_message(&action, &label);
        let payload = self.selection_action_json(&action);
        let mut command = Command::new(&action.command);
        command.args(&action.args);
        if let Some(root) = self.multi_diff.repo_root() {
            command.current_dir(root);
            command.env("OYO_REPO_ROOT", root);
        }
        command.env("OYO_SELECTION_ACTION", &label);
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        match action.stdin {
            ReviewHookStdin::Json => command.stdin(Stdio::piped()),
            ReviewHookStdin::None => command.stdin(Stdio::null()),
        };

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                self.notify(ToastEvent::SelectionActionFailed(failure_message));
                return;
            }
        };
        if matches!(action.stdin, ReviewHookStdin::Json) {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(payload.as_bytes());
            }
        }
        if !action.blocking {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            self.notify(ToastEvent::SelectionActionStarted(success_message));
            return;
        }
        let timeout = Duration::from_millis(action.timeout_ms.max(1));
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.notify(if status.success() {
                        ToastEvent::SelectionActionStarted(success_message.clone())
                    } else {
                        ToastEvent::SelectionActionFailed(failure_message.clone())
                    });
                    return;
                }
                Ok(None) if started.elapsed() >= timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    self.notify(ToastEvent::SelectionActionFailed(failure_message.clone()));
                    return;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => {
                    self.notify(ToastEvent::SelectionActionFailed(failure_message.clone()));
                    return;
                }
            }
        }
    }

    fn selection_action_json(&mut self, action: &SelectionActionConfig) -> String {
        let rows = self
            .diff_selection_segments()
            .into_iter()
            .map(|(row, _, _)| row)
            .collect::<Vec<_>>();
        let (old_range, new_range, side) = self.selected_line_metadata(&rows);
        let file = self
            .multi_diff
            .files
            .get(self.multi_diff.selected_index)
            .map(|file| file.display_name.clone())
            .unwrap_or_default();
        let repo_root = self
            .multi_diff
            .repo_root()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default();
        let payload = SelectionActionPayload {
            version: 1,
            action: selection_action_label(action),
            file,
            repo_root,
            view_mode: format!("{:?}", self.view_mode),
            side,
            old_range,
            new_range,
            scroll_offset: self.render_scroll_offset(),
            rows,
            text: self.selected_diff_text(),
        };
        serde_json::to_string(&payload).unwrap_or_default()
    }

    fn selected_line_metadata(
        &mut self,
        rows: &[u16],
    ) -> (Option<[usize; 2]>, Option<[usize; 2]>, Option<String>) {
        let render_scroll = self.render_scroll_offset();
        let view = self.current_view_with_frame(AnimationFrame::Idle);
        let mut old_min = None::<usize>;
        let mut old_max = None::<usize>;
        let mut new_min = None::<usize>;
        let mut new_max = None::<usize>;
        let mut old_side = false;
        let mut new_side = false;
        for row in rows {
            let Some(line) = view.get(render_scroll.saturating_add(*row as usize)) else {
                continue;
            };
            if let Some(old_line) = line.old_line {
                old_min = Some(old_min.map_or(old_line, |value| value.min(old_line)));
                old_max = Some(old_max.map_or(old_line, |value| value.max(old_line)));
            }
            if let Some(new_line) = line.new_line {
                new_min = Some(new_min.map_or(new_line, |value| value.min(new_line)));
                new_max = Some(new_max.map_or(new_line, |value| value.max(new_line)));
            }
            match line.kind {
                LineKind::Deleted | LineKind::PendingDelete => old_side = true,
                LineKind::Inserted | LineKind::PendingInsert => new_side = true,
                LineKind::Context | LineKind::Modified | LineKind::PendingModify => {
                    old_side = true;
                    new_side = true;
                }
            }
        }
        let side = match (old_side, new_side) {
            (true, false) => Some("old".to_string()),
            (false, true) => Some("new".to_string()),
            (true, true) => Some("both".to_string()),
            (false, false) => None,
        };
        (
            old_min.zip(old_max).map(|(start, end)| [start, end]),
            new_min.zip(new_max).map(|(start, end)| [start, end]),
            side,
        )
    }

    pub(crate) fn diff_selection_active(&self) -> bool {
        self.diff_selection.is_some()
    }

    pub(crate) fn diff_selection_mode_active(&self) -> bool {
        self.diff_selection.is_some() || self.diff_selection_cursor.is_some()
    }

    pub(crate) fn diff_selection_ranges(&self) -> Vec<(u16, u16, u16)> {
        let Some((x, y, _, _)) = self.diff_view_area else {
            return Vec::new();
        };
        let mut ranges = self
            .diff_selection_segments()
            .into_iter()
            .map(|(row, start_col, end_col)| (y + row, x + start_col, x + end_col))
            .collect::<Vec<_>>();
        if ranges.is_empty() {
            if let Some(cursor) = self.diff_selection_cursor {
                ranges.push((
                    y + cursor.point.1,
                    x + cursor.point.0,
                    x + cursor.point.0.saturating_add(1),
                ));
            }
        }
        ranges
    }

    pub(crate) fn diff_selection_excluded_cols(&self) -> Vec<(u16, u16)> {
        let Some((_, _, width, _)) = self.diff_view_area else {
            return Vec::new();
        };
        let content_ranges = self.diff_selection_content_ranges();
        let selected_range = self
            .diff_selection
            .map(|selection| (selection.col_start, selection.col_end))
            .or_else(|| {
                self.diff_selection_cursor
                    .map(|cursor| (cursor.col_start, cursor.col_end))
            });
        excluded_cols(width, &content_ranges, selected_range)
    }

    fn diff_selection_segments(&self) -> Vec<(u16, u16, u16)> {
        self.diff_selection
            .map(selection_segments)
            .unwrap_or_default()
    }

    pub(crate) fn diff_selection_content_ranges(&self) -> Vec<(u16, u16)> {
        let Some((_, _, width, _)) = self.diff_view_area else {
            return Vec::new();
        };
        let width = if self.scrollbar_visible {
            width.saturating_sub(1)
        } else {
            width
        };
        content_ranges(self.view_mode, width)
    }

    fn selectable_col_range(&self, col: u16) -> Option<(u16, u16)> {
        self.diff_selection_content_ranges()
            .into_iter()
            .find(|(start, end)| col >= *start && col < *end)
    }

    fn selection_cursor_or_first(&self) -> Option<((u16, u16), (u16, u16))> {
        self.diff_selection
            .map(|selection| (selection.end, (selection.col_start, selection.col_end)))
            .or_else(|| {
                self.diff_selection_cursor
                    .map(|cursor| (cursor.point, (cursor.col_start, cursor.col_end)))
            })
            .or_else(|| self.first_selectable_cell())
    }

    fn selection_for_cursor_move(&self) -> Option<DiffSelection> {
        self.diff_selection.or_else(|| {
            self.diff_selection_cursor.map(|cursor| DiffSelection {
                start: cursor.point,
                end: cursor.point,
                col_start: cursor.col_start,
                col_end: cursor.col_end,
                mode: DiffSelectionMode::Char,
                dragged: true,
            })
        })
    }

    fn selection_for_cursor_move_or_first(&self) -> Option<DiffSelection> {
        self.selection_for_cursor_move().or_else(|| {
            self.first_selectable_cell()
                .map(|(point, (col_start, col_end))| DiffSelection {
                    start: point,
                    end: point,
                    col_start,
                    col_end,
                    mode: DiffSelectionMode::Char,
                    dragged: true,
                })
        })
    }

    fn first_selectable_cell(&self) -> Option<((u16, u16), (u16, u16))> {
        let ranges = self.diff_selection_content_ranges();
        for (row, cells) in self.diff_selection_cells.iter().enumerate() {
            for (col_start, col_end) in ranges.iter().copied() {
                let end = (col_end as usize).min(cells.len());
                for col in col_start as usize..end {
                    if cells
                        .get(col)
                        .is_some_and(|symbol| !symbol.is_empty() && symbol != " ")
                    {
                        return Some(((col as u16, row as u16), (col_start, col_end)));
                    }
                }
            }
        }
        None
    }

    fn clamp_diff_selection_point(&self, column: u16, row: u16) -> Option<(u16, u16)> {
        let (x, y, width, height) = self.diff_view_area?;
        if width == 0 || height == 0 {
            return None;
        }
        let end_x = x.saturating_add(width);
        let end_y = y.saturating_add(height);
        if self.diff_selection.is_none()
            && (column < x || column >= end_x || row < y || row >= end_y)
        {
            return None;
        }
        let mut col = column.clamp(x, end_x.saturating_sub(1)).saturating_sub(x);
        let row = row.clamp(y, end_y.saturating_sub(1)).saturating_sub(y);
        if let Some(selection) = self.diff_selection {
            col = col.clamp(selection.col_start, selection.col_end.saturating_sub(1));
        } else if self.selectable_col_range(col).is_none() {
            return None;
        }
        Some((col, row))
    }
}

fn reanchor_point(
    cells: &[Vec<String>],
    selection: DiffSelection,
    delta_col: i16,
    delta_row: i16,
) -> Option<(u16, u16)> {
    if delta_row == 0 && delta_col != 0 {
        return if delta_col < 0 {
            prev_char_point(cells, selection)
        } else {
            next_char_point(cells, selection)
        };
    }
    let target_row = (selection.end.1 as i32 + delta_row as i32).max(0) as usize;
    let rows: Box<dyn Iterator<Item = usize>> = if delta_row < 0 {
        Box::new((0..=target_row.min(cells.len().saturating_sub(1))).rev())
    } else {
        Box::new(target_row..cells.len())
    };
    rows.filter_map(|row| u16::try_from(row).ok())
        .find_map(|row| {
            row_nearest_col(cells, row, selection.end.0, selection).map(|col| (col, row))
        })
}

fn row_nearest_col(
    cells: &[Vec<String>],
    row: u16,
    preferred_col: u16,
    selection: DiffSelection,
) -> Option<u16> {
    let cells = cells.get(row as usize)?;
    let end = (selection.col_end as usize).min(cells.len());
    let cols = (selection.col_start as usize..end)
        .filter(|col| cells.get(*col).is_some_and(|symbol| !symbol.is_empty()));
    cols.min_by_key(|col| col.abs_diff(preferred_col as usize))
        .and_then(|col| u16::try_from(col).ok())
}

fn prev_char_point(cells: &[Vec<String>], selection: DiffSelection) -> Option<(u16, u16)> {
    let row = selection.end.1;
    let first = row_first_col(cells, row, selection).unwrap_or(selection.col_start);
    if selection.end.0 > first {
        return Some((selection.end.0.saturating_sub(1), row));
    }
    (0..row)
        .rev()
        .find_map(|prev_row| row_last_col(cells, prev_row, selection).map(|col| (col, prev_row)))
}

fn next_char_point(cells: &[Vec<String>], selection: DiffSelection) -> Option<(u16, u16)> {
    let row = selection.end.1;
    let last = row_last_col(cells, row, selection).unwrap_or(selection.col_end.saturating_sub(1));
    if selection.end.0 < last {
        return Some((selection.end.0.saturating_add(1), row));
    }
    let next_row = row.saturating_add(1) as usize;
    (next_row..cells.len())
        .filter_map(|row| u16::try_from(row).ok())
        .find_map(|row| row_first_col(cells, row, selection).map(|col| (col, row)))
}

fn row_first_col(cells: &[Vec<String>], row: u16, selection: DiffSelection) -> Option<u16> {
    let cells = cells.get(row as usize)?;
    let end = (selection.col_end as usize).min(cells.len());
    (selection.col_start as usize..end)
        .find(|col| cells.get(*col).is_some_and(|symbol| !symbol.is_empty()))
        .and_then(|col| u16::try_from(col).ok())
}

fn row_last_col(cells: &[Vec<String>], row: u16, selection: DiffSelection) -> Option<u16> {
    let cells = cells.get(row as usize)?;
    let end = (selection.col_end as usize).min(cells.len());
    (selection.col_start as usize..end)
        .rev()
        .find(|col| cells.get(*col).is_some_and(|symbol| !symbol.is_empty()))
        .and_then(|col| u16::try_from(col).ok())
}

fn boundary_point(
    cells: &[Vec<String>],
    selection: DiffSelection,
    end: bool,
) -> Option<(u16, u16)> {
    if end {
        last_selectable_cell_in_range(cells, selection.col_start, selection.col_end)
    } else {
        first_selectable_cell_in_range(cells, selection.col_start, selection.col_end)
    }
}

fn first_selectable_cell_in_range(
    cells: &[Vec<String>],
    col_start: u16,
    col_end: u16,
) -> Option<(u16, u16)> {
    for (row, cells) in cells.iter().enumerate() {
        let end = (col_end as usize).min(cells.len());
        for col in col_start as usize..end {
            if cells.get(col).is_some_and(|symbol| !symbol.is_empty()) {
                return Some((col as u16, row as u16));
            }
        }
    }
    None
}

fn last_selectable_cell_in_range(
    cells: &[Vec<String>],
    col_start: u16,
    col_end: u16,
) -> Option<(u16, u16)> {
    for (row, cells) in cells.iter().enumerate().rev() {
        let end = (col_end as usize).min(cells.len());
        for col in (col_start as usize..end).rev() {
            if cells.get(col).is_some_and(|symbol| !symbol.is_empty()) {
                return Some((col as u16, row as u16));
            }
        }
    }
    None
}

fn selection_segments(selection: DiffSelection) -> Vec<(u16, u16, u16)> {
    let row_start = selection.start.1.min(selection.end.1);
    let row_end = selection.start.1.max(selection.end.1);
    let mut out = Vec::new();
    match selection.mode {
        DiffSelectionMode::Char => {
            let mut start = selection.start;
            let mut end = selection.end;
            if (end.1, end.0) < (start.1, start.0) {
                std::mem::swap(&mut start, &mut end);
            }
            for row in start.1..=end.1 {
                let start_col = if row == start.1 {
                    start.0
                } else {
                    selection.col_start
                };
                let end_col = if row == end.1 {
                    end.0.saturating_add(1)
                } else {
                    selection.col_end
                };
                push_segment(&mut out, row, start_col, end_col, selection);
            }
        }
        DiffSelectionMode::Line => {
            for row in row_start..=row_end {
                push_segment(
                    &mut out,
                    row,
                    selection.col_start,
                    selection.col_end,
                    selection,
                );
            }
        }
        DiffSelectionMode::Block => {
            let start_col = selection.start.0.min(selection.end.0);
            let end_col = selection.start.0.max(selection.end.0).saturating_add(1);
            for row in row_start..=row_end {
                push_segment(&mut out, row, start_col, end_col, selection);
            }
        }
    }
    out
}

fn push_segment(
    out: &mut Vec<(u16, u16, u16)>,
    row: u16,
    start_col: u16,
    end_col: u16,
    selection: DiffSelection,
) {
    let start_col = start_col.clamp(selection.col_start, selection.col_end);
    let end_col = end_col.clamp(selection.col_start, selection.col_end);
    if start_col < end_col {
        out.push((row, start_col, end_col));
    }
}

fn content_ranges(view_mode: ViewMode, width: u16) -> Vec<(u16, u16)> {
    let mut ranges = match view_mode {
        ViewMode::Preview => vec![(0, width)],
        ViewMode::UnifiedPane | ViewMode::Evolution => vec![(8.min(width), width)],
        ViewMode::Blame => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(32), Constraint::Min(0)])
                .split(Rect::new(0, 0, width, 1));
            vec![(chunks[1].x.saturating_add(8).min(width), width)]
        }
        ViewMode::Split => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(Rect::new(0, 0, width, 1));
            let left_end = chunks[0].width;
            vec![
                (6.min(width), left_end.saturating_sub(1).min(width)),
                (
                    left_end.saturating_add(5).min(width),
                    width.saturating_sub(1),
                ),
            ]
        }
    };
    ranges.retain(|(start, end)| start < end);
    ranges
}

fn excluded_cols(
    width: u16,
    content_ranges: &[(u16, u16)],
    selected_range: Option<(u16, u16)>,
) -> Vec<(u16, u16)> {
    let mut allowed = selected_range
        .map(|range| vec![range])
        .unwrap_or_else(|| content_ranges.to_vec());
    allowed.sort_unstable();

    let mut excluded = Vec::new();
    let mut col = 0;
    for (start, end) in allowed {
        let start = start.min(width);
        let end = end.min(width);
        if start > col {
            excluded.push((col, start));
        }
        col = col.max(end);
    }
    if col < width {
        excluded.push((col, width));
    }
    excluded
}

fn selection_action_label(action: &SelectionActionConfig) -> String {
    if action.label.trim().is_empty() {
        action.id.clone()
    } else {
        action.label.clone()
    }
}

fn selection_action_success_message(action: &SelectionActionConfig, label: &str) -> String {
    action
        .message
        .as_deref()
        .filter(|message| !message.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if label.trim().is_empty() {
                "Selection action started".to_string()
            } else {
                format!("{label} started")
            }
        })
}

fn selection_action_failure_message(action: &SelectionActionConfig, label: &str) -> String {
    action
        .failure_message
        .as_deref()
        .filter(|message| !message.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if label.trim().is_empty() {
                "Selection action failed".to_string()
            } else {
                format!("{label} failed")
            }
        })
}

fn key_matches_config(key: KeyEvent, binding: &str) -> bool {
    let Ok(seq) = parse_seq(binding) else {
        return false;
    };
    if seq.len() != 1 {
        return false;
    }
    key.to_keymap().ok().as_ref() == seq.first()
}

fn selected_text(cells: &[Vec<String>], segments: &[(u16, u16, u16)]) -> String {
    let mut lines = Vec::new();
    for (row, start_col, end_col) in segments.iter().copied() {
        let Some(cells) = cells.get(row as usize) else {
            continue;
        };
        let start_col = start_col as usize;
        let end_col = (end_col as usize).min(cells.len());
        if start_col >= end_col || start_col >= cells.len() {
            lines.push(String::new());
            continue;
        }
        let mut line = cells[start_col..end_col].concat();
        while line.ends_with(' ') {
            line.pop();
        }
        lines.push(line);
    }
    lines.join("\n").trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        content_ranges, excluded_cols, next_char_point, prev_char_point, reanchor_point,
        selected_text, selection_segments, DiffSelection, DiffSelectionMode,
    };
    use crate::app::ViewMode;

    fn cells(lines: &[&str]) -> Vec<Vec<String>> {
        lines
            .iter()
            .map(|line| line.chars().map(|ch| ch.to_string()).collect())
            .collect()
    }

    #[test]
    fn selected_text_spans_rows_and_trims_padding() {
        let cells = cells(&["abcdef  ", "ghijkl  ", "mnopqr  "]);
        let text = selected_text(&cells, &[(0, 2, 8), (1, 0, 8), (2, 0, 4)]);
        assert_eq!(text, "cdef\nghijkl\nmnop");

        let cells = vec![
            vec!["".into(), "".into(), "a".into(), "b".into(), " ".into()],
            vec!["".into(), "".into(), "c".into(), "d".into(), " ".into()],
        ];
        let text = selected_text(&cells, &[(0, 0, 5), (1, 0, 5)]);
        assert_eq!(text, "ab\ncd");
    }

    #[test]
    fn char_selection_wraps_h_l_between_lines() {
        let cells = cells(&["  abc", "de", "   ", "fgh"]);
        let mut selection = DiffSelection {
            start: (0, 1),
            end: (0, 1),
            col_start: 0,
            col_end: 5,
            mode: DiffSelectionMode::Char,
            dragged: true,
        };
        assert_eq!(prev_char_point(&cells, selection), Some((4, 0)));
        selection.end = (4, 0);
        assert_eq!(next_char_point(&cells, selection), Some((0, 1)));
    }

    #[test]
    fn reanchor_moves_from_selection_end() {
        let cells = cells(&["abc", "def", "ghi"]);
        let selection = DiffSelection {
            start: (0, 0),
            end: (2, 1),
            col_start: 0,
            col_end: 3,
            mode: DiffSelectionMode::Char,
            dragged: true,
        };
        assert_eq!(reanchor_point(&cells, selection, 1, 0), Some((0, 2)));
    }

    #[test]
    fn line_selection_expands_to_pane_width() {
        let selection = DiffSelection {
            start: (12, 4),
            end: (18, 2),
            col_start: 8,
            col_end: 40,
            mode: DiffSelectionMode::Line,
            dragged: true,
        };
        assert_eq!(
            selection_segments(selection),
            vec![(2, 8, 40), (3, 8, 40), (4, 8, 40)]
        );
    }

    #[test]
    fn block_selection_uses_rectangle() {
        let selection = DiffSelection {
            start: (12, 4),
            end: (18, 2),
            col_start: 8,
            col_end: 40,
            mode: DiffSelectionMode::Block,
            dragged: true,
        };
        assert_eq!(
            selection_segments(selection),
            vec![(2, 12, 19), (3, 12, 19), (4, 12, 19)]
        );
    }

    #[test]
    fn split_selection_ranges_are_one_pane_only() {
        let ranges = content_ranges(ViewMode::Split, 100);
        assert_eq!(ranges, vec![(6, 49), (55, 99)]);
        assert_eq!(
            excluded_cols(100, &ranges, Some(ranges[0])),
            vec![(0, 6), (49, 100)]
        );
        assert_eq!(
            excluded_cols(100, &ranges, Some(ranges[1])),
            vec![(0, 55), (99, 100)]
        );
    }
}
