use super::{App, ViewMode};
use crate::app::utils::copy_to_clipboard;
use crate::config::{list_ui_themes, ThemeConfig};
use crate::toasts::ToastEvent;

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
    OpenFileSearch,
    OpenThemePicker,
    OpenCommentPicker,
    OpenPrComments,
    OpenOutdatedComments,
    CopySessionName,
    RenameSession,
    Quit,
    RefreshCurrentFile,
    RefreshAllFiles,
    ReviewAction(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct PaletteEntry {
    pub label: String,
    pub action: PaletteAction,
}

impl App {
    pub(crate) fn set_control_session_name(&mut self, name: Option<String>) {
        self.control_session_name = name;
    }

    pub(crate) fn control_session_name(&self) -> Option<&str> {
        self.control_session_name.as_deref()
    }

    pub(crate) fn copy_control_session_name(&mut self) {
        let Some(name) = self.control_session_name.clone() else {
            return;
        };
        if copy_to_clipboard(&name) {
            self.notify(ToastEvent::CopiedSessionName);
        } else {
            self.notify(ToastEvent::CopyFailed);
        }
    }

    pub(crate) fn start_session_rename(&mut self) {
        let Some(name) = self.control_session_name.clone() else {
            return;
        };
        self.session_rename_active = true;
        self.session_rename_query = name;
        self.reset_picker_cursor();
        self.file_filter_active = false;
        self.clear_search();
        self.clear_goto();
        self.stop_file_search();
        self.stop_comment_picker();
        self.stop_theme_picker();
    }

    pub(crate) fn session_rename_active(&self) -> bool {
        self.session_rename_active
    }

    pub(crate) fn session_rename_query(&self) -> &str {
        &self.session_rename_query
    }

    pub(crate) fn cancel_session_rename(&mut self) {
        self.session_rename_active = false;
        self.session_rename_query.clear();
    }

    pub(crate) fn clear_session_rename_text(&mut self) {
        self.session_rename_query.clear();
        self.reset_picker_cursor();
    }

    pub(crate) fn push_session_rename_char(&mut self, ch: char) {
        self.session_rename_query.push(ch);
        self.reset_picker_cursor();
    }

    pub(crate) fn pop_session_rename_char(&mut self) {
        self.session_rename_query.pop();
        self.reset_picker_cursor();
    }

    pub(crate) fn submit_session_rename(&mut self) {
        let name = self.session_rename_query.trim();
        if name.is_empty() {
            self.notify(ToastEvent::SelectionActionFailed(
                "Session name cannot be empty".to_string(),
            ));
            return;
        }
        self.pending_session_rename = Some(name.to_string());
        self.session_rename_active = false;
    }

    pub(crate) fn take_pending_session_rename(&mut self) -> Option<String> {
        self.pending_session_rename.take()
    }

    fn reset_picker_cursor(&mut self) {
        self.file_filter_cursor_visible = true;
        self.file_filter_cursor_last_blink = std::time::Instant::now();
    }

    pub fn start_command_palette(&mut self) {
        self.command_palette_active = true;
        self.command_palette_query.clear();
        self.command_palette_selection = 0;
        self.reset_picker_cursor();
        self.file_filter_active = false;
        self.clear_search();
        self.clear_goto();
        self.stop_file_search();
        self.stop_comment_picker();
        self.stop_theme_picker();
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
        self.reset_picker_cursor();
    }

    pub fn pop_command_palette_char(&mut self) {
        self.command_palette_query.pop();
        self.command_palette_selection = 0;
        self.reset_picker_cursor();
    }

    pub fn clear_command_palette_text(&mut self) {
        self.command_palette_query.clear();
        self.command_palette_selection = 0;
        self.reset_picker_cursor();
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
                label: "Toggle step mode".to_string(),
                action: PaletteAction::ToggleStepping,
            },
            PaletteEntry {
                label: "Cycle view modes".to_string(),
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
            PaletteEntry {
                label: "View: Preview".to_string(),
                action: PaletteAction::SetViewMode(ViewMode::Preview),
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
                label: "Cycle context folding".to_string(),
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

        if self.can_show_file_panel() {
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
            label: "History...".to_string(),
            action: PaletteAction::OpenDashboard,
        });

        if self.multi_diff.file_count() > 0 {
            entries.push(PaletteEntry {
                label: "Files...".to_string(),
                action: PaletteAction::OpenFileSearch,
            });
        }

        entries.push(PaletteEntry {
            label: "Themes...".to_string(),
            action: PaletteAction::OpenThemePicker,
        });

        entries.push(PaletteEntry {
            label: "Refresh current file".to_string(),
            action: PaletteAction::RefreshCurrentFile,
        });

        if self.control_session_name().is_some() {
            entries.push(PaletteEntry {
                label: "Copy session name".to_string(),
                action: PaletteAction::CopySessionName,
            });
            entries.push(PaletteEntry {
                label: "Rename session".to_string(),
                action: PaletteAction::RenameSession,
            });
        }

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

        if self.review_mode {
            entries.push(PaletteEntry {
                label: "Comments...".to_string(),
                action: PaletteAction::OpenCommentPicker,
            });
            entries.push(PaletteEntry {
                label: format!(
                    "{} comments",
                    self.review_provider_kind().long_review_noun_title()
                ),
                action: PaletteAction::OpenPrComments,
            });
            entries.push(PaletteEntry {
                label: "Outdated comments".to_string(),
                action: PaletteAction::OpenOutdatedComments,
            });
            for (idx, action) in self.review_actions.iter().enumerate() {
                let show = action.show.is_empty()
                    || action.show.iter().any(|item| item == "command_palette");
                if show {
                    let label = if action.label.trim().is_empty() {
                        action.id.clone()
                    } else {
                        action.label.clone()
                    };
                    if !label.trim().is_empty() {
                        entries.push(PaletteEntry {
                            label: format!("Review: {label}"),
                            action: PaletteAction::ReviewAction(idx),
                        });
                    }
                }
            }
        }

        entries.push(PaletteEntry {
            label: "Quit".to_string(),
            action: PaletteAction::Quit,
        });

        entries
    }

    fn execute_palette_action(&mut self, action: PaletteAction) {
        if self.multi_diff.file_count() == 0
            && !matches!(
                action,
                PaletteAction::ToggleHelp
                    | PaletteAction::OpenDashboard
                    | PaletteAction::OpenThemePicker
                    | PaletteAction::OpenCommentPicker
                    | PaletteAction::OpenPrComments
                    | PaletteAction::OpenOutdatedComments
                    | PaletteAction::CopySessionName
                    | PaletteAction::RenameSession
                    | PaletteAction::Quit
                    | PaletteAction::RefreshAllFiles
            )
        {
            return;
        }
        match action {
            PaletteAction::ToggleStepping => self.toggle_stepping(),
            PaletteAction::ToggleViewMode => self.toggle_view_mode(),
            PaletteAction::SetViewMode(mode) => self.set_view_mode(mode),
            PaletteAction::ToggleLineWrap => self.toggle_line_wrap(),
            PaletteAction::ToggleFoldContext => self.toggle_fold_context(),
            PaletteAction::ToggleSyntax => self.toggle_syntax(),
            PaletteAction::ToggleHelp => self.open_help_tab(),
            PaletteAction::ToggleZen => self.toggle_zen(),
            PaletteAction::ToggleFilePanel => self.toggle_file_panel(),
            PaletteAction::ToggleAutoplay => self.toggle_autoplay(),
            PaletteAction::ToggleAutoplayReverse => self.toggle_autoplay_reverse(),
            PaletteAction::OpenDashboard => self.open_dashboard = true,
            PaletteAction::OpenFileSearch => self.start_file_search(),
            PaletteAction::OpenThemePicker => self.start_theme_picker(),
            PaletteAction::OpenCommentPicker => self.start_comment_picker(),
            PaletteAction::OpenPrComments => self.open_pr_comments_in_current_tab(None),
            PaletteAction::OpenOutdatedComments => self.open_outdated_comments_in_current_tab(None),
            PaletteAction::CopySessionName => self.copy_control_session_name(),
            PaletteAction::RenameSession => self.start_session_rename(),
            PaletteAction::Quit => self.request_quit(),
            PaletteAction::RefreshCurrentFile => self.refresh_current_file(),
            PaletteAction::RefreshAllFiles => self.refresh_all_files(),
            PaletteAction::ReviewAction(idx) => self.run_review_action(idx),
        }
    }

    pub fn start_file_search(&mut self) {
        self.file_search_active = true;
        self.file_search_query.clear();
        self.file_search_selection = 0;
        self.reset_picker_cursor();
        self.file_filter_active = false;
        self.clear_search();
        self.clear_goto();
        self.stop_command_palette();
        self.stop_comment_picker();
        self.stop_theme_picker();
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
        self.reset_picker_cursor();
    }

    pub fn pop_file_search_char(&mut self) {
        self.file_search_query.pop();
        self.file_search_selection = 0;
        self.reset_picker_cursor();
    }

    pub fn clear_file_search_text(&mut self) {
        self.file_search_query.clear();
        self.file_search_selection = 0;
        self.reset_picker_cursor();
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

    pub fn start_comment_picker(&mut self) {
        self.comment_picker_active = true;
        self.comment_picker_query.clear();
        self.comment_picker_selection = 0;
        self.reset_picker_cursor();
        self.file_filter_active = false;
        self.clear_search();
        self.clear_goto();
        self.stop_command_palette();
        self.stop_file_search();
        self.stop_theme_picker();
    }

    pub fn stop_comment_picker(&mut self) {
        self.comment_picker_active = false;
    }

    pub fn comment_picker_active(&self) -> bool {
        self.comment_picker_active
    }

    pub fn comment_picker_query(&self) -> &str {
        &self.comment_picker_query
    }

    pub fn comment_picker_selection(&self) -> usize {
        self.comment_picker_selection
    }

    pub fn push_comment_picker_char(&mut self, ch: char) {
        self.comment_picker_query.push(ch);
        self.comment_picker_selection = 0;
        self.reset_picker_cursor();
    }

    pub fn pop_comment_picker_char(&mut self) {
        self.comment_picker_query.pop();
        self.comment_picker_selection = 0;
        self.reset_picker_cursor();
    }

    pub fn clear_comment_picker_text(&mut self) {
        self.comment_picker_query.clear();
        self.comment_picker_selection = 0;
        self.reset_picker_cursor();
    }

    pub fn move_comment_picker_selection(&mut self, delta: isize) {
        let indices = self.comment_picker_filtered_indices();
        let total = indices.len();
        if total == 0 {
            self.comment_picker_selection = 0;
            return;
        }
        let current = self.comment_picker_selection.min(total.saturating_sub(1)) as isize;
        let next = (current + delta).clamp(0, total.saturating_sub(1) as isize);
        self.comment_picker_selection = next as usize;
    }

    pub fn apply_comment_picker_selection(&mut self) {
        let indices = self.comment_picker_filtered_indices();
        if indices.is_empty() {
            return;
        }
        let idx = self
            .comment_picker_selection
            .min(indices.len().saturating_sub(1));
        let comment_idx = indices[idx];
        self.show_comments_sidebar();
        let _ = self.open_review_comment(comment_idx);
        self.stop_comment_picker();
    }

    pub fn set_comment_picker_list_area(
        &mut self,
        area: Option<(u16, u16, u16, u16)>,
        start: usize,
        count: usize,
        item_height: u16,
    ) {
        self.comment_picker_list_area = area;
        self.comment_picker_list_start = start;
        self.comment_picker_list_count = count;
        self.comment_picker_item_height = item_height.max(1);
    }

    pub fn handle_comment_picker_click(&mut self, column: u16, row: u16) -> bool {
        let Some((x, y, width, height)) = self.comment_picker_list_area else {
            return false;
        };
        if row < y || row >= y.saturating_add(height) {
            return false;
        }
        if column < x || column >= x.saturating_add(width) {
            return false;
        }
        let item_height = self.comment_picker_item_height.max(1);
        let offset = row.saturating_sub(y) / item_height;
        let offset = offset as usize;
        if offset >= self.comment_picker_list_count {
            return false;
        }
        self.comment_picker_selection = self.comment_picker_list_start.saturating_add(offset);
        self.apply_comment_picker_selection();
        true
    }

    pub(crate) fn comment_picker_filtered_indices(&mut self) -> Vec<usize> {
        let mut indices = self.review_comment_indices_for_query(&self.comment_picker_query);
        indices.sort_by(|a, b| {
            self.review_comment_sidebar_sort_key(*b)
                .cmp(&self.review_comment_sidebar_sort_key(*a))
        });
        if indices.is_empty() {
            self.comment_picker_selection = 0;
        } else if self.comment_picker_selection >= indices.len() {
            self.comment_picker_selection = indices.len().saturating_sub(1);
        }
        indices
    }

    pub fn start_theme_picker(&mut self) {
        if !self.theme_picker_active {
            self.theme_picker_restore = Some((self.theme.clone(), self.ui_theme_name.clone()));
        }
        self.theme_picker_active = true;
        self.theme_picker_query.clear();
        self.reset_picker_cursor();
        self.theme_picker_selection = self
            .ui_theme_name
            .as_ref()
            .and_then(|current| list_ui_themes().iter().position(|name| name == current))
            .unwrap_or(0);
        self.file_filter_active = false;
        self.clear_search();
        self.clear_goto();
        self.stop_command_palette();
        self.stop_file_search();
        self.stop_comment_picker();
    }

    pub fn stop_theme_picker(&mut self) {
        if let Some((theme, name)) = self.theme_picker_restore.take() {
            self.theme = theme;
            self.ui_theme_name = name;
        }
        self.theme_picker_active = false;
    }

    pub fn theme_picker_active(&self) -> bool {
        self.theme_picker_active
    }

    pub fn theme_picker_query(&self) -> &str {
        &self.theme_picker_query
    }

    pub fn theme_picker_selection(&self) -> usize {
        self.theme_picker_selection
    }

    pub fn push_theme_picker_char(&mut self, ch: char) {
        self.theme_picker_query.push(ch);
        self.theme_picker_selection = 0;
        self.reset_picker_cursor();
        self.preview_theme_picker_selection();
    }

    pub fn pop_theme_picker_char(&mut self) {
        self.theme_picker_query.pop();
        self.theme_picker_selection = 0;
        self.reset_picker_cursor();
        self.preview_theme_picker_selection();
    }

    pub fn clear_theme_picker_text(&mut self) {
        self.theme_picker_query.clear();
        self.theme_picker_selection = 0;
        self.reset_picker_cursor();
        self.preview_theme_picker_selection();
    }

    pub fn move_theme_picker_selection(&mut self, delta: isize) {
        let names = self.theme_picker_filtered_names();
        let total = names.len();
        if total == 0 {
            self.theme_picker_selection = 0;
            return;
        }
        let current = self.theme_picker_selection.min(total.saturating_sub(1)) as isize;
        let next = (current + delta).clamp(0, total.saturating_sub(1) as isize);
        self.theme_picker_selection = next as usize;
        self.preview_theme_picker_selection();
    }

    pub fn apply_theme_picker_selection(&mut self) {
        let names = self.theme_picker_filtered_names();
        if names.is_empty() {
            return;
        }
        let idx = self
            .theme_picker_selection
            .min(names.len().saturating_sub(1));
        self.apply_ui_theme(&names[idx]);
        self.theme_picker_restore = None;
        self.stop_theme_picker();
    }

    pub fn set_theme_picker_list_area(
        &mut self,
        area: Option<(u16, u16, u16, u16)>,
        start: usize,
        count: usize,
        item_height: u16,
    ) {
        self.theme_picker_list_area = area;
        self.theme_picker_list_start = start;
        self.theme_picker_list_count = count;
        self.theme_picker_item_height = item_height.max(1);
    }

    pub fn handle_theme_picker_click(&mut self, column: u16, row: u16) -> bool {
        let Some((x, y, width, height)) = self.theme_picker_list_area else {
            return false;
        };
        if row < y || row >= y.saturating_add(height) {
            return false;
        }
        if column < x || column >= x.saturating_add(width) {
            return false;
        }
        let item_height = self.theme_picker_item_height.max(1);
        let offset = row.saturating_sub(y) / item_height;
        let offset = offset as usize;
        if offset >= self.theme_picker_list_count {
            return false;
        }
        self.theme_picker_selection = self.theme_picker_list_start.saturating_add(offset);
        self.apply_theme_picker_selection();
        true
    }

    pub(crate) fn theme_picker_filtered_names(&mut self) -> Vec<String> {
        let query = self.theme_picker_query.trim().to_ascii_lowercase();
        let names = list_ui_themes()
            .into_iter()
            .filter(|name| query.is_empty() || name.to_ascii_lowercase().contains(&query))
            .collect::<Vec<_>>();
        if names.is_empty() {
            self.theme_picker_selection = 0;
        } else if self.theme_picker_selection >= names.len() {
            self.theme_picker_selection = names.len().saturating_sub(1);
        }
        names
    }

    fn preview_theme_picker_selection(&mut self) {
        let names = self.theme_picker_filtered_names();
        let Some(name) = names.get(self.theme_picker_selection) else {
            return;
        };
        self.apply_ui_theme(&name.clone());
    }

    pub(crate) fn apply_ui_theme(&mut self, name: &str) {
        self.theme = ThemeConfig {
            name: Some(name.to_string()),
            ..ThemeConfig::default()
        }
        .resolve(self.theme_is_light);
        self.ui_theme_name = Some(name.to_string());
        self.unified_render_cache = None;
        self.blame_render_cache = None;
        self.blame_bar_cache.clear();
    }
}
