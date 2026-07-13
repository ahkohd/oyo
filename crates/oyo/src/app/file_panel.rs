use super::{
    App, FileContextMenu, FileContextMenuAction, FilePanelMode, DIFF_VIEW_MIN_WIDTH,
    FILE_PANEL_MIN_WIDTH,
};
use crate::app::utils::copy_to_clipboard;
use crate::config::FilePanelPosition;
use crate::toasts::ToastEvent;

fn point_in_rect(rect: (u16, u16, u16, u16), column: u16, row: u16) -> bool {
    let (x, y, width, height) = rect;
    let end_x = x.saturating_add(width);
    let end_y = y.saturating_add(height);
    column >= x && column < end_x && row >= y && row < end_y
}

impl App {
    pub fn show_comments_sidebar(&mut self) {
        self.close_file_context_menu();
        self.file_panel_mode = FilePanelMode::Comments;
        self.comments_tab_unseen = false;
        self.file_panel_visible = true;
        self.file_panel_manually_set = true;
        self.file_panel_auto_hidden = false;
        self.file_list_focused = true;
        self.file_filter.clear();
        self.file_list_scroll = 0;
        self.preload_all_outdated_reconstructions();
    }

    pub fn show_files_sidebar(&mut self) -> bool {
        self.close_file_context_menu();
        if !self.can_show_file_panel() {
            return false;
        }
        self.file_panel_mode = FilePanelMode::Files;
        self.files_tab_unseen = false;
        self.file_panel_visible = true;
        self.file_panel_manually_set = true;
        self.file_panel_auto_hidden = false;
        self.file_list_focused = true;
        self.file_filter.clear();
        self.file_list_scroll = 0;
        true
    }

    pub fn toggle_file_panel_mode(&mut self) {
        self.close_file_context_menu();
        self.file_panel_mode = match self.file_panel_mode {
            FilePanelMode::Files => FilePanelMode::Comments,
            FilePanelMode::Comments => FilePanelMode::Files,
        };
        match self.file_panel_mode {
            FilePanelMode::Files => self.files_tab_unseen = false,
            FilePanelMode::Comments => self.comments_tab_unseen = false,
        }
        self.file_list_scroll = 0;
        self.file_list_focused = true;
        self.file_panel_mode_toggle_hover = false;
        if self.file_panel_mode == FilePanelMode::Comments {
            self.preload_all_outdated_reconstructions();
        }
    }

    pub fn handle_status_comments_mouse_down(&mut self, column: u16, row: u16) -> bool {
        let hit = self
            .status_comments_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            });
        if !hit {
            return false;
        }
        self.show_comments_sidebar();
        true
    }

    pub fn handle_status_file_mouse_down(&mut self, column: u16, row: u16) -> bool {
        let hit = self.status_file_hit.is_some_and(|(x, y, width, height)| {
            column >= x
                && column < x.saturating_add(width)
                && row >= y
                && row < y.saturating_add(height)
        });
        hit && self.show_files_sidebar()
    }

    pub(crate) fn file_list_item_at(&self, column: u16, row: u16) -> Option<usize> {
        let (x, y, width, height) = self.file_list_area?;
        let end_x = x.saturating_add(width);
        let end_y = y.saturating_add(height);
        let item_start = y.saturating_add(1);
        if column < x || column >= end_x || row < item_start || row >= end_y {
            return None;
        }
        self.file_list_rows
            .get(row.saturating_sub(item_start) as usize)
            .copied()
            .flatten()
    }

    pub fn open_file_context_menu(&mut self, column: u16, row: u16) -> bool {
        if self.file_panel_mode != FilePanelMode::Files {
            return false;
        }
        let Some(file_index) = self.file_list_item_at(column, row) else {
            return false;
        };
        self.close_status_mode_menu();
        self.close_review_comment_context_menu();
        self.file_context_menu = Some(FileContextMenu {
            file_index,
            x: column,
            y: row,
        });
        self.file_context_menu_hover = FileContextMenuAction::ALL.first().copied();
        self.file_list_hover = Some(file_index);
        self.file_list_focused = true;
        self.stop_file_filter();
        true
    }

    pub fn close_file_context_menu(&mut self) -> bool {
        let was_open = self.file_context_menu.take().is_some();
        self.file_context_menu_hits.clear();
        self.file_context_menu_hover = None;
        was_open
    }

    pub(crate) fn file_context_menu_action_at(
        &self,
        column: u16,
        row: u16,
    ) -> Option<FileContextMenuAction> {
        self.file_context_menu_hits
            .iter()
            .find(|hit| point_in_rect((hit.x, hit.y, hit.width, hit.height), column, row))
            .map(|hit| hit.action)
    }

    pub fn update_file_context_menu_hover(&mut self, column: u16, row: u16) -> bool {
        let hover = self.file_context_menu_action_at(column, row);
        if self.file_context_menu_hover == hover {
            return false;
        }
        self.file_context_menu_hover = hover;
        true
    }

    pub(crate) fn move_file_context_menu_active(&mut self, forward: bool) -> bool {
        let actions = FileContextMenuAction::ALL;
        let position = self
            .file_context_menu_hover
            .and_then(|active| actions.iter().position(|action| *action == active));
        let next = if forward {
            position.map_or(0, |index| (index + 1) % actions.len())
        } else {
            position.map_or(actions.len() - 1, |index| {
                index.checked_sub(1).unwrap_or(actions.len() - 1)
            })
        };
        self.file_context_menu_hover = Some(actions[next]);
        true
    }

    pub(crate) fn activate_file_context_menu(&mut self) -> bool {
        let Some(action) = self
            .file_context_menu_hover
            .or_else(|| FileContextMenuAction::ALL.first().copied())
        else {
            return false;
        };
        self.run_file_context_menu_action(action);
        self.close_file_context_menu();
        true
    }

    pub fn handle_file_context_menu_click(&mut self, column: u16, row: u16) -> bool {
        if self.file_context_menu.is_none() {
            return false;
        }
        if let Some(action) = self.file_context_menu_action_at(column, row) {
            self.run_file_context_menu_action(action);
            self.close_file_context_menu();
            return true;
        }
        self.close_file_context_menu();
        true
    }

    fn run_file_context_menu_action(&mut self, action: FileContextMenuAction) {
        let Some(file_index) = self.file_context_menu.map(|menu| menu.file_index) else {
            return;
        };
        match action {
            FileContextMenuAction::Open => self.select_file(file_index),
            FileContextMenuAction::OpenInNewTab => self.open_file_in_new_topbar_tab(file_index),
            FileContextMenuAction::CopyPath => {
                let Some(path) = self
                    .multi_diff
                    .files
                    .get(file_index)
                    .map(|file| file.display_name.clone())
                else {
                    return;
                };
                if copy_to_clipboard(&path) {
                    self.notify(ToastEvent::CopiedPath);
                } else {
                    self.notify(ToastEvent::CopyFailed);
                }
            }
        }
    }

    pub fn handle_file_list_click(&mut self, column: u16, row: u16, new_tab: bool) -> bool {
        if self
            .file_panel_mode_toggle_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.toggle_file_panel_mode();
            return true;
        }

        if self
            .file_panel_root_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.open_dashboard = true;
            return true;
        }

        if self.handle_comments_sidebar_overflow_click(column, row) {
            return true;
        }

        if self
            .comments_sidebar_sync_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.run_comments_sidebar_sync();
            return true;
        }

        if self
            .comments_sidebar_discard_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.request_discard_review_session_changes();
            return true;
        }

        if self
            .file_filter_clear_hit
            .is_some_and(|(x, y, width, height)| {
                column >= x
                    && column < x.saturating_add(width)
                    && row >= y
                    && row < y.saturating_add(height)
            })
        {
            self.clear_file_filter();
            return true;
        }

        if let Some((x, y, width, height)) = self.file_filter_area {
            let end_x = x.saturating_add(width);
            let end_y = y.saturating_add(height);
            if column >= x && column < end_x && row >= y && row < end_y {
                self.file_list_focused = true;
                self.focus_file_filter();
                return true;
            }
        }

        let (x, y, width, height) = match self.file_list_area {
            Some(area) => area,
            None => {
                if self.file_list_focused || self.file_filter_active {
                    self.file_list_focused = false;
                    self.stop_file_filter();
                    return true;
                }
                return false;
            }
        };
        let end_x = x.saturating_add(width);
        let end_y = y.saturating_add(height);
        if column < x || column >= end_x || row < y || row >= end_y {
            if self.file_list_focused || self.file_filter_active {
                self.file_list_focused = false;
                self.stop_file_filter();
                return true;
            }
            return false;
        }

        let item_start = y.saturating_add(1);
        if row < item_start {
            self.file_list_focused = true;
            self.stop_file_filter();
            return true;
        }

        let row_idx = (row - item_start) as usize;
        if let Some(item_idx) = self.file_list_rows.get(row_idx).copied().flatten() {
            self.file_list_focused = true;
            self.stop_file_filter();
            if self.file_panel_mode == FilePanelMode::Comments {
                return self.open_review_comment(item_idx);
            }
            if new_tab {
                self.open_file_in_new_topbar_tab(item_idx);
            } else {
                self.select_file(item_idx);
            }
            return true;
        }

        self.file_list_focused = true;
        self.stop_file_filter();
        true
    }

    pub fn mouse_over_file_panel(&self, column: u16, row: u16) -> bool {
        self.file_panel_rect
            .map(|rect| point_in_rect(rect, column, row))
            .unwrap_or(false)
    }

    pub fn toggle_file_panel(&mut self) {
        if self.file_panel_manually_set {
            // Already manually controlled, just toggle
            self.file_panel_visible = !self.file_panel_visible;
        } else {
            // First manual toggle
            self.file_panel_manually_set = true;
            if self.file_panel_auto_hidden {
                // Panel was auto-hidden, show it
                self.file_panel_visible = true;
            } else {
                // Panel was visible, hide it
                self.file_panel_visible = false;
            }
        }
        if !self.file_panel_visible {
            self.close_file_context_menu();
            self.file_list_focused = false;
            self.file_panel_hover = false;
            self.file_filter_hover = false;
            self.file_filter_clear_hover = false;
        }
    }

    pub fn clamp_file_panel_width(&self, viewport_width: u16) -> u16 {
        let max_panel = viewport_width
            .saturating_sub(DIFF_VIEW_MIN_WIDTH)
            .max(FILE_PANEL_MIN_WIDTH);
        self.file_panel_width.clamp(FILE_PANEL_MIN_WIDTH, max_panel)
    }

    pub fn resize_file_panel(&mut self, delta: i16, viewport_width: u16) {
        let next = (self.file_panel_width as i16).saturating_add(delta);
        let next = next.max(FILE_PANEL_MIN_WIDTH as i16) as u16;
        self.file_panel_width = next;
        self.file_panel_width = self.clamp_file_panel_width(viewport_width);
        self.file_panel_manually_set = true;
    }

    pub fn start_file_panel_resize(&mut self, column: u16, row: u16) -> bool {
        let (x, y, width, height) = match self.file_panel_rect {
            Some(rect) => rect,
            None => return false,
        };
        let sep_x = if self.file_panel_position == FilePanelPosition::Left {
            x.saturating_add(width.saturating_sub(1))
        } else {
            x
        };
        let end_y = y.saturating_add(height);
        if column == sep_x && row >= y && row < end_y {
            self.file_panel_resizing = true;
            self.file_panel_manually_set = true;
            return true;
        }
        false
    }

    pub fn drag_file_panel_resize(&mut self, column: u16, viewport_width: u16) -> bool {
        if !self.file_panel_resizing {
            return false;
        }
        if let Some((x, _, width, _)) = self.file_panel_rect {
            let width = if self.file_panel_position == FilePanelPosition::Left {
                column.saturating_sub(x).saturating_add(1)
            } else {
                x.saturating_add(width).saturating_sub(column)
            };
            self.file_panel_width = width;
            self.file_panel_width = self.clamp_file_panel_width(viewport_width);
            return true;
        }
        false
    }

    pub fn end_file_panel_resize(&mut self) {
        self.file_panel_resizing = false;
    }
}
