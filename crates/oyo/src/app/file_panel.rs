use super::{App, DIFF_VIEW_MIN_WIDTH, FILE_PANEL_MIN_WIDTH};

impl App {
    pub fn handle_file_list_click(&mut self, column: u16, row: u16) -> bool {
        if let Some((x, y, width, height)) = self.file_filter_area {
            let end_x = x.saturating_add(width);
            let end_y = y.saturating_add(height);
            if column >= x && column < end_x && row >= y && row < end_y {
                self.file_list_focused = true;
                self.start_file_filter();
                return true;
            }
        }

        let (x, y, width, height) = match self.file_list_area {
            Some(area) => area,
            None => {
                if self.file_list_focused {
                    self.file_list_focused = false;
                    self.file_filter_active = false;
                    return true;
                }
                return false;
            }
        };
        let end_x = x.saturating_add(width);
        let end_y = y.saturating_add(height);
        if column < x || column >= end_x || row < y || row >= end_y {
            if self.file_list_focused {
                self.file_list_focused = false;
                self.file_filter_active = false;
                return true;
            }
            return false;
        }

        let item_start = y.saturating_add(1);
        if row < item_start {
            self.file_list_focused = true;
            return true;
        }

        let row_idx = (row - item_start) as usize;
        if let Some(Some(file_idx)) = self.file_list_rows.get(row_idx) {
            self.file_list_focused = true;
            self.select_file(*file_idx);
            return true;
        }

        self.file_list_focused = true;
        true
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
        let sep_x = x.saturating_add(width.saturating_sub(1));
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
        if let Some((x, _, _, _)) = self.file_panel_rect {
            let width = column.saturating_sub(x).saturating_add(1);
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
