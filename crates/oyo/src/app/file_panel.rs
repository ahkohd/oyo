use super::{App, FilePanelMode, DIFF_VIEW_MIN_WIDTH, FILE_PANEL_MIN_WIDTH};
use crate::config::FilePanelPosition;

fn point_in_rect(rect: (u16, u16, u16, u16), column: u16, row: u16) -> bool {
    let (x, y, width, height) = rect;
    let end_x = x.saturating_add(width);
    let end_y = y.saturating_add(height);
    column >= x && column < end_x && row >= y && row < end_y
}

impl App {
    pub fn show_comments_sidebar(&mut self) {
        self.file_panel_mode = FilePanelMode::Comments;
        self.file_panel_visible = true;
        self.file_panel_manually_set = true;
        self.file_panel_auto_hidden = false;
        self.file_list_focused = true;
        self.file_filter.clear();
        self.file_list_scroll = 0;
    }

    pub fn show_files_sidebar(&mut self) -> bool {
        if !self.is_multi_file() {
            return false;
        }
        self.file_panel_mode = FilePanelMode::Files;
        self.file_panel_visible = true;
        self.file_panel_manually_set = true;
        self.file_panel_auto_hidden = false;
        self.file_list_focused = true;
        self.file_filter.clear();
        self.file_list_scroll = 0;
        true
    }

    pub fn toggle_file_panel_mode(&mut self) {
        self.file_panel_mode = match self.file_panel_mode {
            FilePanelMode::Files => FilePanelMode::Comments,
            FilePanelMode::Comments => FilePanelMode::Files,
        };
        self.file_list_scroll = 0;
        self.file_list_focused = true;
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
