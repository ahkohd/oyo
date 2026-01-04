use super::{App, ViewMode};

#[derive(Clone, Copy, Debug)]
pub(crate) enum PaletteAction {
    ToggleStepping,
    ToggleViewMode,
    SetViewMode(ViewMode),
    ToggleLineWrap,
    ToggleFoldContext,
    ToggleSyntax,
    ToggleHelp,
    ToggleZen,
    ToggleFilePanel,
    ToggleAutoplay,
    ToggleAutoplayReverse,
    OpenDashboard,
    Quit,
    RefreshCurrentFile,
    RefreshAllFiles,
}

#[derive(Clone, Debug)]
pub(crate) struct PaletteEntry {
    pub label: String,
    pub action: PaletteAction,
}

impl App {
    pub fn start_command_palette(&mut self) {
        self.command_palette_active = true;
        self.command_palette_query.clear();
        self.command_palette_selection = 0;
        self.file_filter_active = false;
        self.clear_search();
        self.clear_goto();
        self.stop_file_search();
    }

    pub fn stop_command_palette(&mut self) {
        self.command_palette_active = false;
    }

    pub fn command_palette_active(&self) -> bool {
        self.command_palette_active
    }

    pub fn command_palette_query(&self) -> &str {
        &self.command_palette_query
    }

    pub fn command_palette_selection(&self) -> usize {
        self.command_palette_selection
    }

    pub fn push_command_palette_char(&mut self, ch: char) {
        self.command_palette_query.push(ch);
        self.command_palette_selection = 0;
    }

    pub fn pop_command_palette_char(&mut self) {
        self.command_palette_query.pop();
        self.command_palette_selection = 0;
    }

    pub fn clear_command_palette_text(&mut self) {
        self.command_palette_query.clear();
        self.command_palette_selection = 0;
    }

    pub fn move_command_palette_selection(&mut self, delta: isize) {
        let entries = self.command_palette_filtered_entries();
        let total = entries.len();
        if total == 0 {
            self.command_palette_selection = 0;
            return;
        }
        let current = self.command_palette_selection.min(total.saturating_sub(1)) as isize;
        let next = (current + delta).clamp(0, total.saturating_sub(1) as isize);
        self.command_palette_selection = next as usize;
    }

    pub fn apply_command_palette_selection(&mut self) {
        let entries = self.command_palette_filtered_entries();
        if entries.is_empty() {
            return;
        }
        let idx = self
            .command_palette_selection
            .min(entries.len().saturating_sub(1));
        let action = entries[idx].action;
        self.execute_palette_action(action);
        self.stop_command_palette();
    }

    pub fn set_command_palette_list_area(
        &mut self,
        area: Option<(u16, u16, u16, u16)>,
        start: usize,
        count: usize,
        item_height: u16,
    ) {
        self.command_palette_list_area = area;
        self.command_palette_list_start = start;
        self.command_palette_list_count = count;
        self.command_palette_item_height = item_height.max(1);
    }

    pub fn handle_command_palette_click(&mut self, column: u16, row: u16) -> bool {
        let Some((x, y, width, height)) = self.command_palette_list_area else {
            return false;
        };
        if row < y || row >= y.saturating_add(height) {
            return false;
        }
        if column < x || column >= x.saturating_add(width) {
            return false;
        }
        let item_height = self.command_palette_item_height.max(1);
        let offset = row.saturating_sub(y) / item_height;
        let offset = offset as usize;
        if offset >= self.command_palette_list_count {
            return false;
        }
        self.command_palette_selection = self.command_palette_list_start.saturating_add(offset);
        self.apply_command_palette_selection();
        true
    }
    pub(crate) fn command_palette_filtered_entries(&mut self) -> Vec<PaletteEntry> {
        let mut entries = self.command_palette_entries();
        let query = self.command_palette_query.trim().to_ascii_lowercase();
        if !query.is_empty() {
            entries.retain(|entry| entry.label.to_ascii_lowercase().contains(&query));
        }
        if entries.is_empty() {
            self.command_palette_selection = 0;
        } else if self.command_palette_selection >= entries.len() {
            self.command_palette_selection = entries.len().saturating_sub(1);
        }
        entries
    }

    fn command_palette_entries(&self) -> Vec<PaletteEntry> {
        let mut entries = vec![
            PaletteEntry {
                label: "Toggle stepping".to_string(),
                action: PaletteAction::ToggleStepping,
            },
            PaletteEntry {
                label: "Cycle view mode".to_string(),
                action: PaletteAction::ToggleViewMode,
            },
            PaletteEntry {
                label: "View: Unified".to_string(),
                action: PaletteAction::SetViewMode(ViewMode::UnifiedPane),
            },
            PaletteEntry {
                label: "View: Split".to_string(),
                action: PaletteAction::SetViewMode(ViewMode::Split),
            },
            PaletteEntry {
                label: "View: Evolution".to_string(),
                action: PaletteAction::SetViewMode(ViewMode::Evolution),
            },
        ];

        if self.blame_enabled {
            entries.push(PaletteEntry {
                label: "View: Blame".to_string(),
                action: PaletteAction::SetViewMode(ViewMode::Blame),
            });
        }

        entries.extend_from_slice(&[
            PaletteEntry {
                label: "Toggle line wrap".to_string(),
                action: PaletteAction::ToggleLineWrap,
            },
            PaletteEntry {
                label: "Toggle context folding".to_string(),
                action: PaletteAction::ToggleFoldContext,
            },
            PaletteEntry {
                label: "Toggle syntax highlight".to_string(),
                action: PaletteAction::ToggleSyntax,
            },
            PaletteEntry {
                label: "Toggle help".to_string(),
                action: PaletteAction::ToggleHelp,
            },
            PaletteEntry {
                label: "Toggle zen mode".to_string(),
                action: PaletteAction::ToggleZen,
            },
        ]);

        if self.is_multi_file() {
            entries.push(PaletteEntry {
                label: "Toggle file panel".to_string(),
                action: PaletteAction::ToggleFilePanel,
            });
            entries.push(PaletteEntry {
                label: "Refresh all files".to_string(),
                action: PaletteAction::RefreshAllFiles,
            });
        }

        entries.push(PaletteEntry {
            label: "Pick commit".to_string(),
            action: PaletteAction::OpenDashboard,
        });

        entries.push(PaletteEntry {
            label: "Refresh current file".to_string(),
            action: PaletteAction::RefreshCurrentFile,
        });

        if self.stepping {
            entries.push(PaletteEntry {
                label: "Toggle autoplay".to_string(),
                action: PaletteAction::ToggleAutoplay,
            });
            entries.push(PaletteEntry {
                label: "Toggle autoplay (reverse)".to_string(),
                action: PaletteAction::ToggleAutoplayReverse,
            });
        }

        entries.push(PaletteEntry {
            label: "Quit".to_string(),
            action: PaletteAction::Quit,
        });

        entries
    }

    fn execute_palette_action(&mut self, action: PaletteAction) {
        match action {
            PaletteAction::ToggleStepping => self.toggle_stepping(),
            PaletteAction::ToggleViewMode => self.toggle_view_mode(),
            PaletteAction::SetViewMode(mode) => self.set_view_mode(mode),
            PaletteAction::ToggleLineWrap => self.toggle_line_wrap(),
            PaletteAction::ToggleFoldContext => self.toggle_fold_context(),
            PaletteAction::ToggleSyntax => self.toggle_syntax(),
            PaletteAction::ToggleHelp => self.toggle_help(),
            PaletteAction::ToggleZen => self.toggle_zen(),
            PaletteAction::ToggleFilePanel => self.toggle_file_panel(),
            PaletteAction::ToggleAutoplay => self.toggle_autoplay(),
            PaletteAction::ToggleAutoplayReverse => self.toggle_autoplay_reverse(),
            PaletteAction::OpenDashboard => self.open_dashboard = true,
            PaletteAction::Quit => self.should_quit = true,
            PaletteAction::RefreshCurrentFile => self.refresh_current_file(),
            PaletteAction::RefreshAllFiles => self.refresh_all_files(),
        }
    }

    pub fn start_file_search(&mut self) {
        self.file_search_active = true;
        self.file_search_query.clear();
        self.file_search_selection = 0;
        self.file_filter_active = false;
        self.clear_search();
        self.clear_goto();
        self.stop_command_palette();
    }

    pub fn stop_file_search(&mut self) {
        self.file_search_active = false;
    }

    pub fn file_search_active(&self) -> bool {
        self.file_search_active
    }

    pub fn file_search_query(&self) -> &str {
        &self.file_search_query
    }

    pub fn file_search_selection(&self) -> usize {
        self.file_search_selection
    }

    pub fn push_file_search_char(&mut self, ch: char) {
        self.file_search_query.push(ch);
        self.file_search_selection = 0;
    }

    pub fn pop_file_search_char(&mut self) {
        self.file_search_query.pop();
        self.file_search_selection = 0;
    }

    pub fn clear_file_search_text(&mut self) {
        self.file_search_query.clear();
        self.file_search_selection = 0;
    }

    pub fn move_file_search_selection(&mut self, delta: isize) {
        let indices = self.file_search_filtered_indices();
        let total = indices.len();
        if total == 0 {
            self.file_search_selection = 0;
            return;
        }
        let current = self.file_search_selection.min(total.saturating_sub(1)) as isize;
        let next = (current + delta).clamp(0, total.saturating_sub(1) as isize);
        self.file_search_selection = next as usize;
    }

    pub fn apply_file_search_selection(&mut self) {
        let indices = self.file_search_filtered_indices();
        if indices.is_empty() {
            return;
        }
        let idx = self
            .file_search_selection
            .min(indices.len().saturating_sub(1));
        let file_idx = indices[idx];
        self.select_file(file_idx);
        self.file_list_focused = false;
        self.stop_file_search();
    }

    pub fn set_file_search_list_area(
        &mut self,
        area: Option<(u16, u16, u16, u16)>,
        start: usize,
        count: usize,
        item_height: u16,
    ) {
        self.file_search_list_area = area;
        self.file_search_list_start = start;
        self.file_search_list_count = count;
        self.file_search_item_height = item_height.max(1);
    }

    pub fn handle_file_search_click(&mut self, column: u16, row: u16) -> bool {
        let Some((x, y, width, height)) = self.file_search_list_area else {
            return false;
        };
        if row < y || row >= y.saturating_add(height) {
            return false;
        }
        if column < x || column >= x.saturating_add(width) {
            return false;
        }
        let item_height = self.file_search_item_height.max(1);
        let offset = row.saturating_sub(y) / item_height;
        let offset = offset as usize;
        if offset >= self.file_search_list_count {
            return false;
        }
        self.file_search_selection = self.file_search_list_start.saturating_add(offset);
        self.apply_file_search_selection();
        true
    }

    pub(crate) fn file_search_filtered_indices(&mut self) -> Vec<usize> {
        let indices = self.file_indices_for_query(&self.file_search_query);
        if indices.is_empty() {
            self.file_search_selection = 0;
        } else if self.file_search_selection >= indices.len() {
            self.file_search_selection = indices.len().saturating_sub(1);
        }
        indices
    }
}
