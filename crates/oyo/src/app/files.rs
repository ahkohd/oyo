use super::{
    AnimationPhase, App, FileDiskStamp, FilePanelMode, PreviewLinkBox, TopbarTab, TopbarTabContent,
    ViewMode,
};
use crate::csv_preview::{CsvPreviewSignature, CsvPreviewState};
use crate::structured_preview::{StructuredPreviewSignature, StructuredPreviewState};
use crate::toasts::ToastEvent;
use oyo_core::multi::FileSide;
use std::time::{Duration, Instant};

/// Whether a URL is safe to hand to the OS opener: only http(s)/mailto, so we
/// never launch `file://`, `javascript:`, or other schemes from preview clicks.
fn is_openable_url(url: &str) -> bool {
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|s| url.len() > s.len() && url[..s.len()].eq_ignore_ascii_case(s))
}

/// Open a URL with the operating system's default handler. The URL is passed as
/// a single argument so it can never be interpreted by a shell.
fn open_url(url: &str) {
    if !is_openable_url(url) {
        return;
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    let _ = command.spawn();
}

impl App {
    pub fn clear_preview_link_boxes(&mut self) {
        self.preview_link_boxes.clear();
    }

    pub fn add_preview_link_box(&mut self, x: u16, y: u16, width: u16, url: String) {
        self.preview_link_boxes
            .push(PreviewLinkBox { x, y, width, url });
    }

    /// Open a previewed hyperlink if the click landed on one. Returns whether a
    /// link was hit (so the caller can stop further click handling).
    pub fn handle_preview_link_click(&mut self, column: u16, row: u16) -> bool {
        if self.view_mode != ViewMode::Preview {
            return false;
        }
        let url = self.preview_link_boxes.iter().rev().find_map(|b| {
            (row == b.y && column >= b.x && column < b.x.saturating_add(b.width))
                .then(|| b.url.clone())
        });
        match url {
            Some(url) => {
                open_url(&url);
                true
            }
            None => false,
        }
    }

    // File navigation methods
    pub fn next_file(&mut self) {
        if !self.file_filter.is_empty() {
            let indices = self.filtered_file_indices();
            if indices.is_empty() {
                return;
            }
            let current = self.multi_diff.selected_index;
            let pos = indices.iter().position(|&i| i == current);
            let next_index = match pos {
                Some(p) if p + 1 < indices.len() => indices[p + 1],
                None => indices[0],
                _ => return,
            };
            self.select_file(next_index);
            return;
        }

        let current = self.multi_diff.selected_index;
        let next_index = current.saturating_add(1);
        if next_index < self.multi_diff.file_count() {
            self.select_file(next_index);
        }
    }

    pub fn prev_file(&mut self) {
        if !self.file_filter.is_empty() {
            let indices = self.filtered_file_indices();
            if indices.is_empty() {
                return;
            }
            let current = self.multi_diff.selected_index;
            let pos = indices.iter().position(|&i| i == current);
            let prev_index = match pos {
                Some(p) if p > 0 => indices[p - 1],
                None => indices[indices.len().saturating_sub(1)],
                _ => return,
            };
            self.select_file(prev_index);
            return;
        }

        let current = self.multi_diff.selected_index;
        if current > 0 {
            self.select_file(current - 1);
        }
    }

    pub fn scroll_file_panel_up(&mut self) {
        self.file_list_scroll = self.file_list_scroll.saturating_sub(1);
    }

    pub fn scroll_file_panel_down(&mut self) {
        let total_rows = if self.file_panel_mode == FilePanelMode::Comments {
            self.filtered_review_comment_indices().len()
        } else {
            let indices = self.filtered_file_indices();
            self.file_list_total_rows(&indices)
        };
        let max_scroll = total_rows.saturating_sub(self.file_list_visible_rows());
        self.file_list_scroll = self.file_list_scroll.saturating_add(1).min(max_scroll);
    }

    pub(super) fn next_file_wrapped(&mut self) -> bool {
        if !self.file_filter.is_empty() {
            let indices = self.filtered_file_indices();
            if indices.is_empty() {
                return false;
            }
            let current = self.multi_diff.selected_index;
            let pos = indices.iter().position(|&i| i == current).unwrap_or(0);
            let next_index = if pos + 1 < indices.len() {
                indices[pos + 1]
            } else {
                indices[0]
            };
            if next_index == current {
                return false;
            }
            self.select_file(next_index);
            return true;
        }

        let count = self.multi_diff.file_count();
        if count == 0 {
            return false;
        }
        let current = self.multi_diff.selected_index;
        let next_index = if current + 1 < count { current + 1 } else { 0 };
        if next_index == current {
            return false;
        }
        self.select_file(next_index);
        true
    }

    pub(super) fn prev_file_wrapped(&mut self) -> bool {
        if !self.file_filter.is_empty() {
            let indices = self.filtered_file_indices();
            if indices.is_empty() {
                return false;
            }
            let current = self.multi_diff.selected_index;
            let pos = indices.iter().position(|&i| i == current).unwrap_or(0);
            let prev_index = if pos > 0 {
                indices[pos - 1]
            } else {
                indices[indices.len().saturating_sub(1)]
            };
            if prev_index == current {
                return false;
            }
            self.select_file(prev_index);
            return true;
        }

        let count = self.multi_diff.file_count();
        if count == 0 {
            return false;
        }
        let current = self.multi_diff.selected_index;
        if current == 0 {
            self.select_file(count - 1);
            return count > 1;
        }
        self.select_file(current - 1);
        true
    }

    pub fn select_file(&mut self, index: usize) {
        if index >= self.multi_diff.file_count() {
            return;
        }
        self.save_active_topbar_tab_state();
        self.replace_active_topbar_tab_file(index);
        self.select_file_in_active_tab(index);
    }

    fn select_file_in_active_tab(&mut self, index: usize) {
        let old_index = self.multi_diff.selected_index;
        if old_index != index && self.review_editor_active() {
            self.review_cancel_editor();
        }
        self.clear_step_edge_hint();
        self.clear_hunk_edge_hint();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        if !self.stepping {
            self.save_no_step_state_snapshot(old_index);
        }
        self.save_scroll_position_for(old_index);
        self.multi_diff.select_file(index);
        self.restore_scroll_position_for(self.multi_diff.selected_index);
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.view_build_defer = false;
        self.view_build_pending = false;
        self.reset_search_for_file_switch();
        self.centered_once = false;
        self.update_file_list_scroll();
        self.handle_file_enter();
    }

    pub(crate) fn ensure_topbar_tabs(&mut self) {
        let count = self.multi_diff.file_count();
        if count == 0 {
            self.topbar_tabs.clear();
            self.active_topbar_tab = None;
            self.topbar_drag_target = None;
            return;
        }
        self.topbar_tabs.retain(|tab| match tab.content {
            TopbarTabContent::File(index) => index < count,
            TopbarTabContent::Help => true,
        });
        let live_ids: Vec<usize> = self.topbar_tabs.iter().map(|tab| tab.id).collect();
        self.structured_previews
            .retain(|id, _| live_ids.iter().any(|live| live == id));
        self.csv_previews
            .retain(|id, _| live_ids.iter().any(|live| live == id));
        if self.topbar_tabs.is_empty() {
            self.add_topbar_tab_for(self.multi_diff.selected_index.min(count.saturating_sub(1)));
        }
        if self
            .active_topbar_tab
            .is_none_or(|id| !self.topbar_tabs.iter().any(|tab| tab.id == id))
        {
            self.active_topbar_tab = self.topbar_tabs.first().map(|tab| tab.id);
        }
        if self
            .topbar_drag_target
            .is_some_and(|idx| idx > self.topbar_tabs.len())
        {
            self.topbar_drag_target = None;
        }
    }

    pub(crate) fn open_file_in_new_topbar_tab(&mut self, file_index: usize) {
        if file_index >= self.multi_diff.file_count() {
            return;
        }
        let id = self.add_topbar_tab_for(file_index);
        self.select_topbar_tab(id);
    }

    fn add_topbar_tab_for(&mut self, file_index: usize) -> usize {
        let id = self.next_topbar_tab_id;
        self.next_topbar_tab_id = self.next_topbar_tab_id.saturating_add(1);
        self.topbar_tabs.push(TopbarTab {
            id,
            content: TopbarTabContent::File(file_index),
            view_mode: self.view_mode,
            step_view_mode: self.step_view_mode,
            stepping: self.stepping,
            scroll_offset: self.scroll_offset,
            horizontal_scroll: self.horizontal_scroll,
            preview_rendered: true,
            navigator_state: None,
        });
        id
    }

    fn replace_active_topbar_tab_file(&mut self, index: usize) {
        let Some(active) = self.active_topbar_tab else {
            self.active_topbar_tab = Some(self.add_topbar_tab_for(index));
            return;
        };
        let Some(tab) = self.topbar_tabs.iter_mut().find(|tab| tab.id == active) else {
            self.active_topbar_tab = Some(self.add_topbar_tab_for(index));
            return;
        };
        if tab.content != TopbarTabContent::File(index) {
            self.structured_previews.remove(&active);
            self.csv_previews.remove(&active);
            tab.content = TopbarTabContent::File(index);
            tab.navigator_state = None;
            tab.scroll_offset = 0;
            tab.horizontal_scroll = 0;
            tab.preview_rendered = true;
        }
        tab.view_mode = self.view_mode;
        tab.step_view_mode = self.step_view_mode;
        tab.stepping = self.stepping;
    }

    pub(crate) fn save_active_topbar_tab_state(&mut self) {
        if self.multi_diff.file_count() == 0 {
            return;
        }
        let Some(active) = self.active_topbar_tab else {
            return;
        };
        let content = self
            .topbar_tabs
            .iter()
            .find(|tab| tab.id == active)
            .map(|tab| tab.content)
            .unwrap_or(TopbarTabContent::File(self.multi_diff.selected_index));
        let content = match content {
            TopbarTabContent::File(_) => TopbarTabContent::File(self.multi_diff.selected_index),
            TopbarTabContent::Help => TopbarTabContent::Help,
        };
        let view_mode = self.view_mode;
        let step_view_mode = self.step_view_mode;
        let stepping = self.stepping;
        let scroll_offset = self.scroll_offset;
        let horizontal_scroll = self.horizontal_scroll;
        let navigator_state = match content {
            TopbarTabContent::File(_) => Some(self.multi_diff.current_navigator().state().clone()),
            TopbarTabContent::Help => None,
        };
        if let Some(tab) = self.topbar_tabs.iter_mut().find(|tab| tab.id == active) {
            tab.content = content;
            tab.view_mode = view_mode;
            tab.step_view_mode = step_view_mode;
            tab.stepping = stepping;
            tab.scroll_offset = scroll_offset;
            tab.horizontal_scroll = horizontal_scroll;
            tab.navigator_state = navigator_state;
        }
    }

    pub(crate) fn new_topbar_tab(&mut self) {
        if self.multi_diff.file_count() == 0 {
            return;
        }
        self.save_active_topbar_tab_state();
        let id = self.next_topbar_tab_id;
        self.next_topbar_tab_id = self.next_topbar_tab_id.saturating_add(1);
        let mut tab = self
            .active_topbar_tab
            .and_then(|active| {
                self.topbar_tabs
                    .iter()
                    .find(|tab| tab.id == active)
                    .cloned()
            })
            .unwrap_or_else(|| TopbarTab {
                id,
                content: TopbarTabContent::File(self.multi_diff.selected_index),
                view_mode: self.view_mode,
                step_view_mode: self.step_view_mode,
                stepping: self.stepping,
                scroll_offset: self.scroll_offset,
                horizontal_scroll: self.horizontal_scroll,
                preview_rendered: true,
                navigator_state: None,
            });
        tab.id = id;
        self.topbar_tabs.push(tab);
        self.active_topbar_tab = Some(id);
    }

    fn close_topbar_tab(&mut self, tab_id: usize) {
        if self.topbar_tabs.len() <= 1 {
            return;
        }
        let Some(pos) = self.topbar_tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        self.structured_previews.remove(&tab_id);
        self.csv_previews.remove(&tab_id);
        self.topbar_tabs.remove(pos);
        if self.active_topbar_tab == Some(tab_id) {
            let next_pos = pos.min(self.topbar_tabs.len().saturating_sub(1));
            if let Some(next) = self.topbar_tabs.get(next_pos).map(|tab| tab.id) {
                self.select_topbar_tab(next);
            }
        }
    }

    pub(crate) fn select_topbar_tab(&mut self, tab_id: usize) {
        if self.active_topbar_tab == Some(tab_id) {
            return;
        }
        if self.multi_diff.file_count() == 0 {
            return;
        }
        let Some(tab) = self
            .topbar_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned()
        else {
            return;
        };
        if let TopbarTabContent::File(index) = tab.content {
            if index >= self.multi_diff.file_count() {
                return;
            }
        }
        let old_index = self.multi_diff.selected_index;
        let target_file = match tab.content {
            TopbarTabContent::File(index) => Some(index),
            TopbarTabContent::Help => None,
        };
        if target_file != Some(old_index) && self.review_editor_active() {
            self.review_cancel_editor();
        }
        self.save_active_topbar_tab_state();
        if !self.stepping {
            self.save_no_step_state_snapshot(old_index);
        }
        self.save_scroll_position_for(old_index);
        self.clear_step_edge_hint();
        self.clear_hunk_edge_hint();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        self.active_topbar_tab = Some(tab_id);
        self.view_mode = tab.view_mode;
        self.step_view_mode = tab.step_view_mode;
        self.stepping = tab.stepping;
        self.scroll_offset = tab.scroll_offset;
        self.horizontal_scroll = tab.horizontal_scroll;
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.view_build_defer = false;
        self.view_build_pending = false;
        self.reset_search_for_file_switch();
        self.centered_once = false;
        match tab.content {
            TopbarTabContent::File(index) => {
                self.multi_diff.select_file(index);
                self.update_file_list_scroll();
                let restored = tab
                    .navigator_state
                    .map(|state| self.multi_diff.current_navigator().set_state(state))
                    .unwrap_or(false);
                if restored {
                    self.queue_current_file_diff();
                } else {
                    self.handle_file_enter();
                }
            }
            TopbarTabContent::Help => {
                self.view_mode = ViewMode::Preview;
                self.clear_diff_selection();
            }
        }
    }

    pub(crate) fn open_help_tab(&mut self) {
        if self.multi_diff.file_count() == 0 {
            self.toggle_help();
            return;
        }
        if let Some(id) = self
            .topbar_tabs
            .iter()
            .find(|tab| tab.content == TopbarTabContent::Help)
            .map(|tab| tab.id)
        {
            self.select_topbar_tab(id);
            return;
        }
        self.save_active_topbar_tab_state();
        let id = self.next_topbar_tab_id;
        self.next_topbar_tab_id = self.next_topbar_tab_id.saturating_add(1);
        self.topbar_tabs.push(TopbarTab {
            id,
            content: TopbarTabContent::Help,
            view_mode: ViewMode::Preview,
            step_view_mode: self.step_view_mode,
            stepping: self.stepping,
            scroll_offset: 0,
            horizontal_scroll: 0,
            preview_rendered: true,
            navigator_state: None,
        });
        self.active_topbar_tab = Some(id);
        self.view_mode = ViewMode::Preview;
        self.scroll_offset = 0;
        self.horizontal_scroll = 0;
        self.clear_diff_selection();
        self.show_help = false;
    }

    pub(crate) fn active_topbar_content(&self) -> Option<TopbarTabContent> {
        self.active_topbar_tab.and_then(|id| {
            self.topbar_tabs
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| tab.content)
        })
    }

    pub(crate) fn active_preview_rendered(&self) -> bool {
        self.active_topbar_tab
            .and_then(|id| self.topbar_tabs.iter().find(|tab| tab.id == id))
            .map(|tab| tab.preview_rendered)
            .unwrap_or(true)
    }

    pub(crate) fn toggle_preview_rendered(&mut self) {
        let mut rendered = None;
        if let Some(active) = self.active_topbar_tab {
            if let Some(tab) = self.topbar_tabs.iter_mut().find(|tab| tab.id == active) {
                tab.preview_rendered = !tab.preview_rendered;
                rendered = Some(tab.preview_rendered);
            }
        }
        if let Some(rendered) = rendered {
            self.notify(ToastEvent::PreviewRendered(rendered));
        }
    }

    pub(crate) fn ensure_csv_preview(
        &mut self,
        signature: CsvPreviewSignature,
        text: &str,
    ) -> Result<&mut CsvPreviewState, String> {
        let Some(tab_id) = self.active_topbar_tab else {
            return Err("No active tab".to_string());
        };
        let rebuild = self
            .csv_previews
            .get(&tab_id)
            .is_none_or(|state| state.signature() != &signature);
        if rebuild {
            match CsvPreviewState::new(signature, text) {
                Ok(state) => {
                    self.csv_previews.insert(tab_id, state);
                }
                Err(error) => {
                    self.csv_previews.remove(&tab_id);
                    return Err(error);
                }
            }
        }
        self.csv_previews
            .get_mut(&tab_id)
            .ok_or_else(|| "No CSV preview".to_string())
    }

    pub(crate) fn active_csv_preview_mut(&mut self) -> Option<&mut CsvPreviewState> {
        if self.view_mode != ViewMode::Preview || !self.active_preview_rendered() {
            return None;
        }
        let tab_id = self.active_topbar_tab?;
        self.csv_previews.get_mut(&tab_id)
    }

    pub(crate) fn sync_scroll_from_csv_preview(&mut self) {
        // Three rows (top padding, header, and separator) stay pinned, so the
        // scrollable body is shorter than the viewport.
        let viewport_height = self.last_viewport_height.saturating_sub(3).max(1);
        if let Some(state) = self.active_csv_preview_mut() {
            let line = state.selected_visual_line();
            if line < self.scroll_offset {
                self.scroll_offset = line;
            } else if line >= self.scroll_offset.saturating_add(viewport_height) {
                self.scroll_offset = line.saturating_sub(viewport_height.saturating_sub(1));
            }
        }
    }

    pub(crate) fn csv_preview_move_down(&mut self, count: usize) -> bool {
        let Some(state) = self.active_csv_preview_mut().filter(|_| count > 0) else {
            return false;
        };
        state.move_down(count);
        self.sync_scroll_from_csv_preview();
        true
    }

    pub(crate) fn csv_preview_move_up(&mut self, count: usize) -> bool {
        let Some(state) = self.active_csv_preview_mut().filter(|_| count > 0) else {
            return false;
        };
        state.move_up(count);
        self.sync_scroll_from_csv_preview();
        true
    }

    pub(crate) fn csv_preview_move_left(&mut self, count: usize) -> bool {
        let Some(state) = self.active_csv_preview_mut().filter(|_| count > 0) else {
            return false;
        };
        state.move_left(count);
        true
    }

    pub(crate) fn csv_preview_move_right(&mut self, count: usize) -> bool {
        let Some(state) = self.active_csv_preview_mut().filter(|_| count > 0) else {
            return false;
        };
        state.move_right(count);
        true
    }

    pub(crate) fn csv_preview_focus_top(&mut self) -> bool {
        let Some(state) = self.active_csv_preview_mut() else {
            return false;
        };
        state.focus_top();
        self.sync_scroll_from_csv_preview();
        true
    }

    pub(crate) fn csv_preview_focus_bottom(&mut self) -> bool {
        let Some(state) = self.active_csv_preview_mut() else {
            return false;
        };
        state.focus_bottom();
        self.sync_scroll_from_csv_preview();
        true
    }

    pub(crate) fn ensure_structured_preview(
        &mut self,
        signature: StructuredPreviewSignature,
        text: &str,
    ) -> Result<&mut StructuredPreviewState, String> {
        let Some(tab_id) = self.active_topbar_tab else {
            return Err("No active tab".to_string());
        };
        let rebuild = self
            .structured_previews
            .get(&tab_id)
            .is_none_or(|state| state.signature() != &signature);
        if rebuild {
            match StructuredPreviewState::new(signature, text) {
                Ok(state) => {
                    self.structured_previews.insert(tab_id, state);
                }
                Err(error) => {
                    self.structured_previews.remove(&tab_id);
                    return Err(error);
                }
            }
        }
        self.structured_previews
            .get_mut(&tab_id)
            .ok_or_else(|| "No structured preview".to_string())
    }

    pub(crate) fn active_structured_preview_mut(&mut self) -> Option<&mut StructuredPreviewState> {
        if self.view_mode != ViewMode::Preview || !self.active_preview_rendered() {
            return None;
        }
        let tab_id = self.active_topbar_tab?;
        self.structured_previews.get_mut(&tab_id)
    }

    pub(crate) fn sync_scroll_from_structured_preview(&mut self) {
        if let Some(state) = self.active_structured_preview_mut() {
            self.scroll_offset = state.top_visible_offset();
        }
    }

    pub(crate) fn structured_preview_move_down(&mut self, count: usize) -> bool {
        let Some(state) = self.active_structured_preview_mut().filter(|_| count > 0) else {
            return false;
        };
        state.move_down(count);
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_move_up(&mut self, count: usize) -> bool {
        let Some(state) = self.active_structured_preview_mut().filter(|_| count > 0) else {
            return false;
        };
        state.move_up(count);
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_move_left(&mut self) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        state.move_left();
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_move_right(&mut self) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        state.move_right();
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_focus_top(&mut self) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        state.focus_top();
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_focus_bottom(&mut self) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        state.focus_bottom();
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_jump_up(&mut self, count: Option<usize>) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        state.jump_up(count);
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_jump_down(&mut self, count: Option<usize>) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        state.jump_down(count);
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_toggle_collapsed(&mut self) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        state.toggle_collapsed();
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_collapse_node_and_siblings(&mut self, deep: bool) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        if deep {
            state.deep_collapse_node_and_siblings();
        } else {
            state.collapse_node_and_siblings();
        }
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_expand_node_and_siblings(&mut self, deep: bool) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        if deep {
            state.deep_expand_node_and_siblings();
        } else {
            state.expand_node_and_siblings();
        }
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn structured_preview_toggle_mode(&mut self) -> bool {
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        state.toggle_mode();
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn handle_structured_preview_click(&mut self, column: u16, row: u16) -> bool {
        if self.view_mode != ViewMode::Preview || !self.active_preview_rendered() {
            return false;
        }
        let Some((x, y, width, height)) = self.diff_view_area else {
            return false;
        };
        if column < x
            || column >= x.saturating_add(width)
            || row < y
            || row >= y.saturating_add(height)
        {
            return false;
        }
        let Some(state) = self.active_structured_preview_mut() else {
            return false;
        };
        state.click(row.saturating_sub(y).saturating_add(1));
        self.sync_scroll_from_structured_preview();
        true
    }

    pub(crate) fn mouse_over_topbar(&self, column: u16, row: u16) -> bool {
        self.topbar_area.is_some_and(|(x, y, width, height)| {
            column >= x
                && column < x.saturating_add(width)
                && row >= y
                && row < y.saturating_add(height)
        })
    }

    pub(crate) fn scroll_topbar_tabs(&mut self, delta: isize) -> bool {
        let max_scroll = self.topbar_tabs.len().saturating_sub(1);
        let old = self.topbar_tab_scroll.min(max_scroll);
        let next = if delta.is_negative() {
            old.saturating_sub(delta.unsigned_abs())
        } else {
            old.saturating_add(delta as usize).min(max_scroll)
        };
        self.topbar_tab_scroll = next;
        old != next
    }

    pub(crate) fn handle_status_bar_mouse_down(
        &mut self,
        column: u16,
        row: u16,
        reverse: bool,
    ) -> bool {
        let hit = self.status_mode_hit.is_some_and(|(x, y, width, height)| {
            column >= x
                && column < x.saturating_add(width)
                && row >= y
                && row < y.saturating_add(height)
        });
        if !hit {
            return false;
        }
        if reverse {
            self.toggle_view_mode_reverse();
        } else {
            self.toggle_view_mode();
        }
        true
    }

    pub(crate) fn handle_topbar_mouse_down(&mut self, column: u16, row: u16) -> bool {
        self.update_topbar_hover(column, row);
        if self
            .preview_toggle_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.toggle_preview_rendered();
            return true;
        }
        if self
            .topbar_sidebar_toggle_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.toggle_file_panel();
            return true;
        }
        if self
            .topbar_scroll_left_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.scroll_topbar_tabs(-1);
            return true;
        }
        if self
            .topbar_scroll_right_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.scroll_topbar_tabs(1);
            return true;
        }
        if self.topbar_plus_hit.is_some_and(|(x, y, width, height)| {
            column >= x
                && column < x.saturating_add(width)
                && row >= y
                && row < y.saturating_add(height)
        }) {
            self.new_topbar_tab();
            self.start_file_search();
            return true;
        }
        let Some(hit) = self.topbar_hit(column, row) else {
            return false;
        };
        if hit.close_col == Some(column) && self.topbar_tabs.len() > 1 {
            self.close_topbar_tab(hit.tab_id);
            return true;
        }
        self.select_topbar_tab(hit.tab_id);
        self.topbar_drag_tab = Some(hit.tab_id);
        self.topbar_drag_target = None;
        true
    }

    pub(crate) fn drag_topbar_tab(&mut self, column: u16, row: u16) -> bool {
        let Some(dragged) = self.topbar_drag_tab else {
            return false;
        };
        self.topbar_drag_target = self.topbar_drop_target(dragged, column, row);
        true
    }

    pub(crate) fn finish_topbar_drag(&mut self) -> bool {
        let Some(dragged) = self.topbar_drag_tab.take() else {
            return false;
        };
        if let Some(target) = self.topbar_drag_target.take() {
            self.move_topbar_tab(dragged, target);
        }
        true
    }

    fn move_topbar_tab(&mut self, tab_id: usize, target: usize) {
        let Some(from) = self.topbar_tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let tab = self.topbar_tabs.remove(from);
        let mut to = target.min(self.topbar_tabs.len() + 1);
        if to > from {
            to = to.saturating_sub(1);
        }
        self.topbar_tabs.insert(to.min(self.topbar_tabs.len()), tab);
    }

    pub(crate) fn update_topbar_hover(&mut self, column: u16, row: u16) -> bool {
        let hit = self.topbar_hit(column, row);
        let hover = hit.map(|hit| hit.tab_id);
        let close_hover = hit
            .filter(|hit| hit.close_col == Some(column))
            .map(|hit| hit.tab_id);
        let plus_hover = self.topbar_plus_hit.is_some_and(|(x, y, width, height)| {
            column >= x
                && column < x.saturating_add(width)
                && row >= y
                && row < y.saturating_add(height)
        });
        let scroll_left_hover = self
            .topbar_scroll_left_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            });
        let scroll_right_hover =
            self.topbar_scroll_right_hit
                .is_some_and(|(x, y, width, height)| {
                    column >= x
                        && column < x.saturating_add(width)
                        && row >= y
                        && row < y.saturating_add(height)
                });
        let preview_hover = self
            .preview_toggle_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            });
        let sidebar_hover = self
            .topbar_sidebar_toggle_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            });
        let status_comments_hover =
            self.status_comments_hit
                .is_some_and(|(x, y, width, height)| {
                    column >= x
                        && column < x.saturating_add(width)
                        && row >= y
                        && row < y.saturating_add(height)
                });
        let status_file_hover = self.status_file_hit.is_some_and(|(x, y, width, height)| {
            column >= x
                && column < x.saturating_add(width)
                && row >= y
                && row < y.saturating_add(height)
        });
        let file_panel_hover = self.mouse_over_file_panel(column, row);
        let file_panel_mode_toggle_hover =
            self.file_panel_mode_toggle_hit
                .is_some_and(|(x, y, width, height)| {
                    column >= x
                        && column < x.saturating_add(width)
                        && row >= y
                        && row < y.saturating_add(height)
                });
        let file_filter_hover = self.file_filter_area.is_some_and(|(x, y, width, height)| {
            column >= x
                && column < x.saturating_add(width)
                && row >= y
                && row < y.saturating_add(height)
        });
        let file_filter_clear_hover =
            self.file_filter_clear_hit
                .is_some_and(|(x, y, width, height)| {
                    column >= x
                        && column < x.saturating_add(width)
                        && row >= y
                        && row < y.saturating_add(height)
                });
        let file_hover = self.file_list_area.and_then(|(x, y, width, height)| {
            let in_list = column >= x
                && column < x.saturating_add(width)
                && row >= y.saturating_add(1)
                && row < y.saturating_add(height);
            if !in_list {
                return None;
            }
            self.file_list_rows
                .get(row.saturating_sub(y.saturating_add(1)) as usize)
                .copied()
                .flatten()
        });
        let selection_hover = self
            .selection_toolbar_hits
            .iter()
            .find(|hit| {
                column >= hit.x
                    && column < hit.x.saturating_add(hit.width)
                    && row >= hit.y
                    && row < hit.y.saturating_add(hit.height)
            })
            .map(|hit| hit.action);
        let (review_line_add_row, review_line_add_hover) =
            self.review_line_add_hover_at(column, row);
        let review_preview_hit = self.review_preview_boxes.iter().rev().find_map(|hit| {
            let end_x = hit.x.saturating_add(hit.width);
            let end_y = hit.y.saturating_add(hit.height);
            (column >= hit.x && column < end_x && row >= hit.y && row < end_y)
                .then(|| (hit.anchor_key.clone(), hit.delete))
        });
        let review_preview_hover = review_preview_hit
            .as_ref()
            .map(|(anchor_key, _)| anchor_key.clone());
        let review_preview_delete_hover =
            review_preview_hit.and_then(|(anchor_key, delete)| delete.then_some(anchor_key));
        let review_editor_hover = self
            .review_editor_toolbar_hits
            .iter()
            .find(|hit| {
                column >= hit.x
                    && column < hit.x.saturating_add(hit.width)
                    && row >= hit.y
                    && row < hit.y.saturating_add(hit.height)
            })
            .map(|hit| hit.action);
        if self.topbar_hover_tab == hover
            && self.topbar_hover_close == close_hover
            && self.topbar_plus_hover == plus_hover
            && self.topbar_scroll_left_hover == scroll_left_hover
            && self.topbar_scroll_right_hover == scroll_right_hover
            && self.preview_toggle_hover == preview_hover
            && self.topbar_sidebar_toggle_hover == sidebar_hover
            && self.status_comments_hover == status_comments_hover
            && self.status_file_hover == status_file_hover
            && self.file_list_hover == file_hover
            && self.file_panel_hover == file_panel_hover
            && self.file_panel_mode_toggle_hover == file_panel_mode_toggle_hover
            && self.file_filter_hover == file_filter_hover
            && self.file_filter_clear_hover == file_filter_clear_hover
            && self.selection_toolbar_hover == selection_hover
            && self.review_line_add_row == review_line_add_row
            && self.review_line_add_hover == review_line_add_hover
            && self.review_preview_hover == review_preview_hover
            && self.review_preview_delete_hover == review_preview_delete_hover
            && self.review_editor_toolbar_hover == review_editor_hover
        {
            return false;
        }
        self.selection_toolbar_hover = selection_hover;
        self.review_line_add_row = review_line_add_row;
        self.review_line_add_hover = review_line_add_hover;
        self.review_preview_hover = review_preview_hover;
        self.review_preview_delete_hover = review_preview_delete_hover;
        self.review_editor_toolbar_hover = review_editor_hover;
        self.topbar_hover_tab = hover;
        self.topbar_hover_close = close_hover;
        self.topbar_plus_hover = plus_hover;
        self.topbar_scroll_left_hover = scroll_left_hover;
        self.topbar_scroll_right_hover = scroll_right_hover;
        self.preview_toggle_hover = preview_hover;
        self.topbar_sidebar_toggle_hover = sidebar_hover;
        self.status_comments_hover = status_comments_hover;
        self.status_file_hover = status_file_hover;
        self.file_list_hover = file_hover;
        self.file_panel_hover = file_panel_hover;
        self.file_panel_mode_toggle_hover = file_panel_mode_toggle_hover;
        self.file_filter_hover = file_filter_hover;
        self.file_filter_clear_hover = file_filter_clear_hover;
        true
    }

    fn topbar_drop_target(&self, tab_id: usize, column: u16, row: u16) -> Option<usize> {
        let from = self.topbar_tabs.iter().position(|tab| tab.id == tab_id)?;
        let target = self.topbar_tab_insert_index(column, row)?;
        if target == from || target == from + 1 {
            return None;
        }
        Some(target)
    }

    fn topbar_tab_insert_index(&self, column: u16, row: u16) -> Option<usize> {
        if self.topbar_tab_hits.first()?.row != row {
            return None;
        }
        for hit in &self.topbar_tab_hits {
            let pos = self
                .topbar_tabs
                .iter()
                .position(|tab| tab.id == hit.tab_id)?;
            if column < hit.start_col {
                return Some(pos);
            }
            if column < hit.end_col {
                let midpoint = hit
                    .start_col
                    .saturating_add((hit.end_col - hit.start_col) / 2);
                return Some(if column < midpoint { pos } else { pos + 1 });
            }
        }
        let last = self.topbar_tab_hits.last()?;
        let last_pos = self
            .topbar_tabs
            .iter()
            .position(|tab| tab.id == last.tab_id)?;
        let end_col = self
            .topbar_plus_hit
            .map(|(x, _, _, _)| x)
            .unwrap_or(last.end_col);
        (column <= end_col).then_some(last_pos + 1)
    }

    fn topbar_hit(&self, column: u16, row: u16) -> Option<super::TopbarTabHit> {
        self.topbar_tab_hits
            .iter()
            .copied()
            .find(|hit| row == hit.row && column >= hit.start_col && column < hit.end_col)
    }

    pub fn start_file_filter(&mut self) {
        self.file_filter.clear();
        self.focus_file_filter();
    }

    pub(crate) fn focus_file_filter(&mut self) {
        self.file_filter_active = true;
        self.file_filter_cursor_visible = true;
        self.file_filter_cursor_last_blink = std::time::Instant::now();
        self.file_list_scroll = 0;
        self.ensure_selection_matches_filter();
        self.update_file_list_scroll();
    }

    pub fn stop_file_filter(&mut self) {
        self.file_filter_active = false;
        self.file_filter_cursor_visible = true;
    }

    pub fn push_file_filter_char(&mut self, ch: char) {
        self.file_filter.push(ch);
        self.reset_file_filter_cursor();
        self.on_filter_changed();
    }

    pub fn pop_file_filter_char(&mut self) {
        self.file_filter.pop();
        self.reset_file_filter_cursor();
        self.on_filter_changed();
    }

    pub fn clear_file_filter(&mut self) {
        self.file_filter.clear();
        self.reset_file_filter_cursor();
        self.on_filter_changed();
    }

    fn reset_file_filter_cursor(&mut self) {
        self.file_filter_cursor_visible = true;
        self.file_filter_cursor_last_blink = std::time::Instant::now();
    }

    /// Check if current file would be blank at step 0 (new file: empty old, non-empty new)
    fn is_blank_at_step0(&self) -> bool {
        self.multi_diff.current_old_is_empty() && !self.multi_diff.current_new_is_empty()
    }

    /// Handle entering a file (marks visited, optionally auto-steps to first change)
    /// Called on initial file and when switching files.
    pub fn handle_file_enter(&mut self) {
        if self.multi_diff.file_count() == 0 {
            return;
        }
        self.queue_current_file_diff();
        if self.stepping && !self.current_file_diff_ready() {
            return;
        }
        self.finish_file_enter();
    }

    pub(crate) fn finish_file_enter(&mut self) {
        if self.multi_diff.file_count() == 0 {
            return;
        }
        let idx = self.multi_diff.selected_index;

        if !self.stepping {
            if !self.files_visited[idx] {
                self.files_visited[idx] = true;
            }
            // If in no-step mode, ensure full content is shown immediately
            self.ensure_step_state_snapshot(idx);
            self.multi_diff.current_navigator().goto_end();
            self.multi_diff.current_navigator().clear_active_change();
            self.animation_phase = AnimationPhase::Idle;
            self.animation_progress = 1.0;
            if !self.restore_no_step_state_snapshot(idx) {
                if self.no_step_auto_jump_on_enter && !self.no_step_visited[idx] {
                    self.goto_hunk_index_scroll(0);
                } else {
                    self.set_cursor_for_current_scroll();
                    self.multi_diff.current_navigator().set_hunk_scope(false);
                }
            }
            self.no_step_visited[idx] = true;
            // Don't mess with scroll_offset here; it might have been restored by next_file/prev_file
            return;
        }

        // Only process on first visit to this file
        if self.files_visited[idx] {
            return;
        }

        let is_large = self.multi_diff.file_is_large(idx);
        if is_large {
            self.files_visited[idx] = true;
            return;
        }

        // Mark as visited
        self.files_visited[idx] = true;

        let state = self.multi_diff.current_navigator().state();
        let at_step_0 = state.current_step == 0;
        let has_steps = state.total_steps > 1;
        if !at_step_0 || !has_steps {
            return;
        }

        // Auto-step for blank files (new files) regardless of view mode
        if self.auto_step_blank_files && self.is_blank_at_step0() {
            self.next_step();
            return;
        }

        // Regular auto-step on enter (not for Evolution mode)
        if self.auto_step_on_enter && self.view_mode != ViewMode::Evolution {
            self.next_step();
        }
    }

    pub fn is_multi_file(&self) -> bool {
        self.multi_diff.is_multi_file()
    }

    fn update_file_list_scroll(&mut self) {
        let indices = self.filtered_file_indices();
        if indices.is_empty() {
            self.file_list_scroll = 0;
            return;
        }

        // Keep selected file visible in the file list.
        let selected = self.multi_diff.selected_index;
        let selected_row = self.file_list_row_for_file(&indices, selected).unwrap_or(0);
        if selected_row < self.file_list_scroll {
            self.file_list_scroll = selected_row;
        }
        let visible_rows = self.file_list_visible_rows();
        if selected_row >= self.file_list_scroll.saturating_add(visible_rows) {
            self.file_list_scroll = selected_row.saturating_sub(visible_rows - 1);
        }
        let max_scroll = self
            .file_list_total_rows(&indices)
            .saturating_sub(visible_rows);
        self.file_list_scroll = self.file_list_scroll.min(max_scroll);
    }

    pub(crate) fn file_list_total_rows(&self, indices: &[usize]) -> usize {
        let mut rows = 0usize;
        let mut current_group: Option<String> = None;
        for &index in indices {
            let group = self.file_list_group(index);
            if current_group.as_deref() != Some(group.as_str()) {
                if current_group.is_some() {
                    rows += 1;
                }
                rows += 1;
                current_group = Some(group);
            }
            rows += 1;
        }
        rows
    }

    fn file_list_row_for_file(&self, indices: &[usize], target: usize) -> Option<usize> {
        let mut row = 0usize;
        let mut current_group: Option<String> = None;
        for &index in indices {
            let group = self.file_list_group(index);
            if current_group.as_deref() != Some(group.as_str()) {
                if current_group.is_some() {
                    row += 1;
                }
                row += 1;
                current_group = Some(group);
            }
            if index == target {
                return Some(row);
            }
            row += 1;
        }
        None
    }

    pub(crate) fn file_list_group(&self, index: usize) -> String {
        self.multi_diff
            .files
            .get(index)
            .and_then(|file| {
                file.display_name
                    .rsplit_once('/')
                    .map(|(dir, _)| dir.to_string())
            })
            .unwrap_or_else(|| "Root Path".to_string())
    }

    fn file_list_visible_rows(&self) -> usize {
        self.file_list_area
            .map(|(_, _, _, height)| height.saturating_sub(2) as usize)
            .unwrap_or(20)
            .max(1)
    }

    fn on_filter_changed(&mut self) {
        self.file_list_scroll = 0;
        self.ensure_selection_matches_filter();
        self.update_file_list_scroll();
    }

    fn ensure_selection_matches_filter(&mut self) {
        if self.file_filter.is_empty() {
            return;
        }
        let indices = self.filtered_file_indices();
        if indices.is_empty() {
            return;
        }
        if !indices.contains(&self.multi_diff.selected_index) {
            self.select_file(indices[0]);
        }
    }

    pub fn filtered_file_indices(&self) -> Vec<usize> {
        self.file_indices_for_query(&self.file_filter)
    }

    pub(super) fn file_indices_for_query(&self, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return (0..self.multi_diff.files.len()).collect();
        }
        let query = query.to_ascii_lowercase();
        self.multi_diff
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.display_name.to_ascii_lowercase().contains(&query))
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Get current file path for display
    pub fn current_file_path(&self) -> String {
        if self.active_topbar_content() == Some(TopbarTabContent::Help) {
            return "Help".to_string();
        }
        self.multi_diff
            .current_file()
            .map(|f| f.display_name.clone())
            .unwrap_or_default()
    }

    pub(crate) fn git_index_stamp(&self) -> FileDiskStamp {
        if !self.multi_diff.uses_git_index() {
            return FileDiskStamp::default();
        }
        let Some(repo_root) = self.multi_diff.repo_root() else {
            return FileDiskStamp::default();
        };
        let Ok(path) = oyo_core::git::get_index_path(repo_root) else {
            return FileDiskStamp::default();
        };
        match std::fs::metadata(path) {
            Ok(meta) => FileDiskStamp {
                modified: meta.modified().ok(),
                len: meta.len(),
                exists: true,
            },
            Err(_) => FileDiskStamp::default(),
        }
    }

    fn disk_stamp_for_index(&self, idx: usize) -> FileDiskStamp {
        let Some(file) = self.multi_diff.files.get(idx) else {
            return FileDiskStamp::default();
        };

        let full_path = self
            .multi_diff
            .source_path(idx, FileSide::New)
            .unwrap_or_else(|| {
                if let Some(repo_root) = self.multi_diff.repo_root() {
                    repo_root.join(&file.path)
                } else {
                    file.path.clone()
                }
            });

        match std::fs::metadata(&full_path) {
            Ok(meta) => FileDiskStamp {
                modified: meta.modified().ok(),
                len: meta.len(),
                exists: true,
            },
            Err(_) => FileDiskStamp::default(),
        }
    }

    pub(crate) fn rebuild_file_disk_baseline(&mut self) {
        let file_count = self.multi_diff.file_count();
        self.file_disk_baseline = (0..file_count)
            .map(|idx| self.disk_stamp_for_index(idx))
            .collect();
        self.file_disk_changed = vec![false; file_count];
    }

    fn refresh_file_disk_baseline_for(&mut self, idx: usize) {
        if self.file_disk_baseline.len() != self.multi_diff.file_count() {
            self.rebuild_file_disk_baseline();
            return;
        }
        let stamp = self.disk_stamp_for_index(idx);
        if let Some(slot) = self.file_disk_baseline.get_mut(idx) {
            *slot = stamp;
        }
    }

    fn recompute_file_change_state(&mut self) -> bool {
        let old_any = self.files_changed_on_disk;
        let old_changed = self.file_disk_changed.clone();
        let file_count = self.multi_diff.file_count();
        if self.file_disk_baseline.len() != file_count {
            self.rebuild_file_disk_baseline();
        }
        if self.file_disk_changed.len() != file_count {
            self.file_disk_changed = vec![false; file_count];
        }

        let mut any_changed = false;
        for idx in 0..file_count {
            let changed = self.disk_stamp_for_index(idx) != self.file_disk_baseline[idx];
            if let Some(slot) = self.file_disk_changed.get_mut(idx) {
                *slot = changed;
            }
            any_changed |= changed;
        }
        self.files_changed_on_disk = any_changed;
        old_any != self.files_changed_on_disk || old_changed != self.file_disk_changed
    }

    pub(crate) fn file_changed_on_disk(&self, idx: usize) -> bool {
        self.file_disk_changed.get(idx).copied().unwrap_or(false)
    }

    /// Check if tracked files changed on disk since the last refresh baseline.
    pub fn maybe_check_file_changes(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_fs_check) < Duration::from_secs(1) {
            return false;
        }
        self.last_fs_check = now;
        self.recompute_file_change_state()
    }

    pub(crate) fn maybe_watch_refresh_git_files(&mut self) -> bool {
        if !self.watch || !self.multi_diff.is_git_mode() {
            return false;
        }
        let now = Instant::now();
        if now.duration_since(self.last_git_watch_check) < Duration::from_secs(1) {
            return false;
        }
        self.last_git_watch_check = now;

        let index_stamp = self.git_index_stamp();
        let changed =
            self.multi_diff.git_change_list_changed() || index_stamp != self.git_index_baseline;
        if !changed {
            return false;
        }

        self.refresh_all_files();
        true
    }

    pub(crate) fn maybe_watch_refresh_changed_files(&mut self) -> bool {
        if !self.watch || !self.files_changed_on_disk {
            return false;
        }

        let changed: Vec<usize> = self
            .file_disk_changed
            .iter()
            .enumerate()
            .filter_map(|(idx, changed)| changed.then_some(idx))
            .collect();
        if changed.is_empty() {
            return false;
        }

        let current = self.multi_diff.selected_index;
        for idx in changed {
            if idx == current {
                self.refresh_current_file();
                continue;
            }
            self.multi_diff.refresh_file(idx);
            if idx < self.syntax_caches.len() {
                self.syntax_caches[idx] = None;
            }
            self.refresh_file_disk_baseline_for(idx);
        }
        self.recompute_file_change_state();
        true
    }

    /// Refresh current file from disk
    pub fn refresh_current_file(&mut self) {
        if self.multi_diff.file_count() == 0 {
            return;
        }
        // Preserve no-step hunk scope/cursor context when possible.
        let preserve_no_step_hunk = if !self.stepping {
            let nav = self.multi_diff.current_navigator();
            let state = nav.state();
            if state.last_nav_was_hunk {
                let cursor_rank = nav
                    .diff()
                    .hunks
                    .get(state.current_hunk)
                    .and_then(|hunk| {
                        state
                            .cursor_change
                            .and_then(|cursor| hunk.change_ids.iter().position(|id| *id == cursor))
                    })
                    .unwrap_or(0);
                Some((state.current_hunk, cursor_rank))
            } else {
                None
            }
        } else {
            None
        };

        self.multi_diff.refresh_current_file();

        // The navigator is rebuilt at step 0 after refresh; jump to the end
        // so all changes remain visible.
        {
            let nav = self.multi_diff.current_navigator();
            nav.goto_end();
            if !self.stepping {
                // Keep no-step state semantics after refresh.
                nav.clear_active_change();
            }
        }

        if !self.stepping {
            let restored_hunk_scope = if let Some((prev_hunk, prev_cursor_rank)) =
                preserve_no_step_hunk
            {
                let nav = self.multi_diff.current_navigator();
                let total_hunks = nav.state().total_hunks;
                if total_hunks > 0 {
                    let hunk_idx = prev_hunk.min(total_hunks.saturating_sub(1));
                    let cursor_change = nav.diff().hunks.get(hunk_idx).and_then(|hunk| {
                        if hunk.change_ids.is_empty() {
                            None
                        } else {
                            let idx = prev_cursor_rank.min(hunk.change_ids.len().saturating_sub(1));
                            hunk.change_ids.get(idx).copied()
                        }
                    });
                    nav.set_cursor_hunk(hunk_idx, cursor_change);
                    nav.set_hunk_scope(true);
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if !restored_hunk_scope {
                self.set_cursor_for_current_scroll();
                self.multi_diff.current_navigator().set_hunk_scope(false);
            }
        }

        let idx = self.multi_diff.selected_index;
        if idx < self.syntax_caches.len() {
            self.syntax_caches[idx] = None;
        }
        self.ensure_syntax_cache();

        self.refresh_file_disk_baseline_for(idx);
        self.recompute_file_change_state();
    }

    /// Refresh all files from git (re-scan for uncommitted changes)
    pub fn refresh_all_files(&mut self) {
        if self.multi_diff.refresh_all_from_git() {
            // Reset scroll states for all files
            let file_count = self.multi_diff.file_count();
            self.scroll_offsets_step = vec![0; file_count];
            self.scroll_offsets_no_step = vec![0; file_count];
            self.horizontal_scrolls_step = vec![0; file_count];
            self.horizontal_scrolls_no_step = vec![0; file_count];
            self.max_line_widths_step = vec![0; file_count];
            self.max_line_widths_no_step = vec![0; file_count];
            self.no_step_visited = vec![false; file_count];
            self.files_visited = vec![false; file_count];
            self.syntax_caches = vec![None; file_count];
            self.step_state_snapshots = vec![None; file_count];
            self.no_step_state_snapshots = vec![None; file_count];
            self.scroll_offset = 0;
            self.horizontal_scroll = 0;
            self.needs_scroll_to_active = true;
            self.centered_once = false;
            self.handle_file_enter();

            self.rebuild_file_disk_baseline();
            self.git_index_baseline = self.git_index_stamp();
            self.files_changed_on_disk = false;
            self.invalidate_review_repo_file_cache();
        }
    }

    /// Get the total number of lines in the current view
    #[allow(dead_code)]
    pub fn total_lines(&mut self) -> usize {
        let frame = self.animation_frame();
        self.current_view_with_frame(frame).len()
    }

    /// Get statistics about the current file's diff
    pub fn stats(&mut self) -> (usize, usize) {
        if self.multi_diff.file_count() == 0 || self.current_file_is_binary() {
            return (0, 0);
        }
        let diff = self.multi_diff.current_navigator().diff();
        (diff.insertions, diff.deletions)
    }

    pub fn current_file_is_binary(&self) -> bool {
        self.multi_diff.current_file_is_binary()
    }
}

#[cfg(test)]
mod link_tests {
    use super::is_openable_url;

    #[test]
    fn only_web_and_mailto_urls_open() {
        assert!(is_openable_url("https://example.com"));
        assert!(is_openable_url("http://example.com/path?q=1"));
        assert!(is_openable_url("HTTPS://EXAMPLE.COM"));
        assert!(is_openable_url("mailto:me@victorare.mu"));
        // Rejected schemes.
        assert!(!is_openable_url("file:///etc/passwd"));
        assert!(!is_openable_url("javascript:alert(1)"));
        assert!(!is_openable_url("./assets/logo.png"));
        assert!(!is_openable_url("ftp://example.com"));
        assert!(!is_openable_url(""));
        assert!(!is_openable_url("https://"));
    }
}
