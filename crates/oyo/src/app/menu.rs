use super::{palette::PaletteAction, App};
use crate::config::FoldContextMode;
use crate::keybindings::{Dispatch, GlobalAction, MenuAction, NormalAction};
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Category {
    File,
    View,
    Navigate,
    Review,
    Help,
}

impl Category {
    const ALL: [Self; 5] = [
        Self::File,
        Self::View,
        Self::Navigate,
        Self::Review,
        Self::Help,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::View => "View",
            Self::Navigate => "Navigate",
            Self::Review => "Review",
            Self::Help => "Help",
        }
    }
}

impl PaletteAction {
    fn category(self) -> Category {
        use PaletteAction::*;
        match self {
            OpenFileSearch
            | CopyFilePath
            | CopyFilePosition
            | CopyAbsoluteFilePath
            | CopyAbsoluteFilePosition
            | CopyFileCursorPosition
            | CopyAbsoluteFileCursorPosition
            | CopySessionName
            | RenameSession
            | RefreshCurrentFile
            | RefreshAllFiles
            | Quit => Category::File,
            ToggleMenuBar | ToggleViewMode | SetViewMode(_) | ToggleLineWrap
            | ToggleFoldContext | ToggleSyntax | ToggleZen | ToggleFilePanel | OpenThemePicker => {
                Category::View
            }
            OpenDashboard
            | NavigateBack
            | NavigateForward
            | OpenReviewGrep
            | OpenReviewGrepChanges
            | OpenReviewGrepEverything
            | ToggleStepping
            | NextStep
            | PreviousStep
            | ToggleAutoplay
            | ToggleAutoplayReverse => Category::Navigate,
            OpenRemoteReview
            | ToggleFileReviewed
            | ToggleReviewedFilesVisibility
            | ToggleIncrementalReview
            | OpenCommentPicker
            | OpenPrComments
            | OpenOutdatedComments
            | ReviewAction(_)
            | SyncReview => Category::Review,
            ToggleHelp | OpenSettings | OpenMenu | OpenCommandPalette => Category::Help,
        }
    }
}

#[derive(Default)]
pub(crate) struct MenuState {
    pub(crate) open: Option<MenuOpen>,
    categories: Vec<(Rect, usize)>,
    rows: Vec<(Rect, usize)>,
    popup: Option<Rect>,
    previous: Option<Rect>,
    next: Option<Rect>,
}

pub(crate) struct MenuOpen {
    category: usize,
    selected: usize,
}

impl App {
    pub(crate) fn menu_bar_frame_start(&mut self) {
        self.menu.categories.clear();
    }

    pub(crate) fn menu_input_available(&self) -> bool {
        !self.command_palette_active()
            && !self.file_search_active()
            && !self.review_grep_active()
            && !self.comment_picker_active()
            && !self.theme_picker_active()
            && !self.review_editor_active()
            && !self.review_remote_picker_active()
            && self.remote_open.is_none()
            && !self.session_rename_active()
            && !self.settings_leave_confirmation_active()
            && !self.settings_reset_confirmation_active()
            && !self.quit_confirmation_active()
            && !self.review_delete_confirmation_active()
            && !self.show_help
            && !self.show_path_popup
            && self.file_context_menu.is_none()
            && self.review_comment_context_menu.is_none()
            && !self.status_mode_menu_open()
            && !self.file_filter_active
            && !self.search_active
            && !self.goto_active
    }

    pub(crate) fn open_menu(&mut self, category: usize) {
        self.keybindings.clear_sequence();
        self.reset_count();
        self.review_edit_prefix = false;
        self.review_reply_prefix = false;
        self.review_resolve_prefix = false;
        self.review_delete_prefix = false;
        self.review_overflow_prefix = false;
        self.pr_reply_prefix = false;
        self.menu.open = Some(MenuOpen {
            category: category % 5,
            selected: 0,
        });
    }

    fn menu_entries(&self, category: usize) -> Vec<super::palette::PaletteEntry> {
        use PaletteAction::*;
        let order: &[PaletteAction] = match Category::ALL[category] {
            Category::File => &[
                OpenFileSearch,
                CopyFilePath,
                CopyFilePosition,
                CopyAbsoluteFilePath,
                CopyAbsoluteFilePosition,
                CopyFileCursorPosition,
                CopyAbsoluteFileCursorPosition,
                RefreshCurrentFile,
                RefreshAllFiles,
                CopySessionName,
                RenameSession,
                Quit,
            ],
            Category::View => &[
                SetViewMode(super::ViewMode::UnifiedPane),
                SetViewMode(super::ViewMode::Split),
                SetViewMode(super::ViewMode::Evolution),
                SetViewMode(super::ViewMode::Preview),
                SetViewMode(super::ViewMode::Blame),
                ToggleLineWrap,
                ToggleFoldContext,
                ToggleSyntax,
                ToggleFilePanel,
                ToggleMenuBar,
                ToggleZen,
                OpenThemePicker,
            ],
            Category::Navigate => &[
                OpenDashboard,
                NavigateBack,
                NavigateForward,
                OpenReviewGrep,
                OpenReviewGrepChanges,
                OpenReviewGrepEverything,
                PreviousStep,
                NextStep,
                ToggleStepping,
                ToggleAutoplay,
                ToggleAutoplayReverse,
            ],
            Category::Review => &[
                OpenRemoteReview,
                ToggleFileReviewed,
                OpenCommentPicker,
                OpenPrComments,
                OpenOutdatedComments,
                ToggleIncrementalReview,
                ToggleReviewedFilesVisibility,
                SyncReview,
            ],
            Category::Help => &[ToggleHelp, OpenCommandPalette, OpenSettings],
        };
        let mut entries: Vec<_> = self
            .command_entries()
            .into_iter()
            .filter(|entry| {
                entry.action.category() == Category::ALL[category]
                    && !matches!(entry.action, OpenMenu | ToggleViewMode)
            })
            .collect();
        entries.sort_by_key(|entry| {
            order
                .iter()
                .position(|action| *action == entry.action)
                .unwrap_or(order.len())
        });
        entries
    }

    fn move_menu(&mut self, delta: isize) {
        if let Some(open) = &mut self.menu.open {
            open.category = (open.category as isize + delta).rem_euclid(5) as usize;
            open.selected = 0;
        }
    }

    fn move_menu_selection(&mut self, delta: isize) {
        if let Some(open) = &self.menu.open {
            let count = self.menu_entries(open.category).len();
            self.menu.open.as_mut().unwrap().selected =
                (open.selected as isize + delta).rem_euclid(count.max(1) as isize) as usize;
        }
    }

    fn activate_menu(&mut self) {
        let Some(open) = &self.menu.open else { return };
        let entries = self.menu_entries(open.category);
        let Some(entry) = entries.get(open.selected) else {
            return;
        };
        let action = entry.action;
        if self.command_enabled(action) {
            self.close_menu();
            self.execute_palette_action(action);
        }
    }

    pub(crate) fn handle_menu_key(&mut self, key: KeyEvent) {
        match self.keybindings.menu(key) {
            Dispatch::Matched(MenuAction::Cancel) => self.close_menu(),
            Dispatch::Matched(MenuAction::PreviousCategory) => self.move_menu(-1),
            Dispatch::Matched(MenuAction::NextCategory) => self.move_menu(1),
            Dispatch::Matched(MenuAction::NextItem) => self.move_menu_selection(1),
            Dispatch::Matched(MenuAction::PreviousItem) => self.move_menu_selection(-1),
            Dispatch::Matched(MenuAction::FirstItem) => {
                if let Some(open) = &mut self.menu.open {
                    open.selected = 0;
                }
            }
            Dispatch::Matched(MenuAction::LastItem) => {
                if let Some(open) = &self.menu.open {
                    let last = self.menu_entries(open.category).len().saturating_sub(1);
                    self.menu.open.as_mut().unwrap().selected = last;
                }
            }
            Dispatch::Matched(MenuAction::Accept) => self.activate_menu(),
            Dispatch::Pending | Dispatch::Unmatched => {}
        }
    }

    fn close_menu(&mut self) {
        self.menu.open = None;
        self.keybindings.clear_sequence();
    }

    pub(crate) fn handle_menu_mouse(&mut self, event: MouseEvent) -> bool {
        if !self.menu_input_available() {
            return false;
        }
        let position = (event.column, event.row).into();
        let category = self
            .menu
            .categories
            .iter()
            .find(|(rect, _)| rect.contains(position))
            .map(|(_, index)| *index);
        if self.menu.open.is_some() {
            match event.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if self
                        .menu
                        .previous
                        .is_some_and(|rect| rect.contains(position))
                    {
                        self.move_menu(-1);
                    } else if self.menu.next.is_some_and(|rect| rect.contains(position)) {
                        self.move_menu(1);
                    } else if let Some(category) = category {
                        if self
                            .menu
                            .open
                            .as_ref()
                            .is_some_and(|open| open.category == category)
                        {
                            self.close_menu();
                        } else {
                            self.open_menu(category);
                        }
                    } else if let Some((_, index)) = self
                        .menu
                        .rows
                        .iter()
                        .find(|(rect, _)| rect.contains(position))
                    {
                        self.menu.open.as_mut().unwrap().selected = *index;
                        self.activate_menu();
                    } else if !self.menu.popup.is_some_and(|rect| rect.contains(position)) {
                        self.close_menu();
                    }
                }
                MouseEventKind::Down(_) => self.menu.open = None,
                MouseEventKind::Moved => {
                    if let Some(category) = category {
                        if self
                            .menu
                            .open
                            .as_ref()
                            .is_some_and(|open| open.category != category)
                        {
                            self.open_menu(category);
                        }
                    } else if let Some((_, index)) = self
                        .menu
                        .rows
                        .iter()
                        .find(|(rect, _)| rect.contains(position))
                    {
                        self.menu.open.as_mut().unwrap().selected = *index;
                    }
                }
                MouseEventKind::ScrollDown => self.move_menu_selection(1),
                MouseEventKind::ScrollUp => self.move_menu_selection(-1),
                _ => {}
            }
            return true;
        }
        if let (MouseEventKind::Down(MouseButton::Left), Some(category)) = (event.kind, category) {
            self.open_menu(category);
            return true;
        }
        false
    }

    fn command_hint(&self, action: PaletteAction) -> String {
        use PaletteAction::*;
        let global = match action {
            OpenFileSearch => Some(GlobalAction::OpenFileSearch),
            OpenReviewGrep => Some(GlobalAction::OpenReviewGrep),
            OpenCommandPalette => Some(GlobalAction::OpenCommandPalette),
            OpenCommentPicker => Some(GlobalAction::OpenCommentPicker),
            OpenThemePicker => Some(GlobalAction::OpenThemePicker),
            _ => None,
        };
        if let Some(action) = global {
            return self.keybindings.global_keys(action);
        }
        if self.file_list_focused && self.file_panel_mode == super::FilePanelMode::Comments {
            return String::new();
        }
        if self.active_pr_comments_view() || self.active_outdated_comments_view() {
            return String::new();
        }
        if matches!(action, ToggleAutoplay | ToggleAutoplayReverse)
            && self.view_mode == super::ViewMode::Preview
        {
            return String::new();
        }
        if self.diff_selection_mode_active() {
            return String::new();
        }
        if self.active_topbar_content().is_some_and(|content| {
            matches!(
                content,
                super::TopbarTabContent::Settings | super::TopbarTabContent::Help
            )
        }) {
            return String::new();
        }
        let normal = match action {
            ToggleStepping => NormalAction::ToggleStepping,
            NextStep => NormalAction::StepDown,
            PreviousStep => NormalAction::StepUp,
            ToggleLineWrap => NormalAction::ToggleLineWrap,
            ToggleFoldContext => NormalAction::ToggleFoldContext,
            ToggleSyntax => NormalAction::ToggleSyntax,
            ToggleHelp => NormalAction::ToggleHelp,
            OpenSettings => NormalAction::OpenSettings,
            ToggleZen => NormalAction::ToggleZen,
            ToggleFilePanel => NormalAction::ToggleFilePanel,
            ToggleFileReviewed => NormalAction::ToggleFileReviewed,
            ToggleIncrementalReview => NormalAction::ToggleIncrementalReview,
            ToggleAutoplay => NormalAction::ToggleAutoplay,
            ToggleAutoplayReverse => NormalAction::ToggleAutoplayReverse,
            OpenDashboard => NormalAction::OpenDashboard,
            NavigateBack => NormalAction::NavigateBack,
            NavigateForward => NormalAction::NavigateForward,
            OpenOutdatedComments => NormalAction::OpenOutdatedComments,
            Quit => NormalAction::Quit,
            _ => return String::new(),
        };
        if matches!(action, NextStep | PreviousStep)
            && (!self.stepping
                || self.file_list_focused
                || self.view_mode == super::ViewMode::Preview)
        {
            return String::new();
        }
        self.keybindings.menu_normal_keys(normal)
    }

    fn command_checked(&self, action: PaletteAction) -> bool {
        use PaletteAction::*;
        match action {
            SetViewMode(mode) => mode == self.view_mode,
            ToggleMenuBar => self.menu_bar,
            ToggleLineWrap => self.line_wrap,
            ToggleFoldContext => self.fold_context != FoldContextMode::Off,
            ToggleStepping => self.stepping,
            ToggleSyntax => self.syntax_mode != crate::config::SyntaxMode::Off,
            ToggleAutoplay => self.autoplay && !self.autoplay_reverse,
            ToggleAutoplayReverse => self.autoplay && self.autoplay_reverse,
            ToggleZen => self.zen_mode,
            ToggleFilePanel => self.file_panel_visible,
            ToggleFileReviewed => self.file_is_reviewed(self.review_live_diff().selected_index),
            ToggleReviewedFilesVisibility => self.hides_reviewed_files(),
            ToggleIncrementalReview => {
                self.incremental_review_toggle_label() == Some("Disable incremental review")
            }
            _ => false,
        }
    }
}

pub(crate) fn draw_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    app.menu.categories.clear();
    if area.is_empty() {
        return;
    }
    let style = Style::default()
        .fg(app.theme.text_muted)
        .bg(app.theme.background.unwrap_or(ratatui::style::Color::Reset));
    frame.render_widget(Paragraph::new("").style(style), area);
    let compact = area.width < 46;
    let mut x = area.x;
    let categories: Vec<_> = if compact {
        vec![(0, "Menu")]
    } else {
        Category::ALL
            .iter()
            .enumerate()
            .map(|(index, category)| (index, category.label()))
            .collect()
    };
    for (index, label) in categories {
        let width = (label.width() as u16 + 4).min(area.right().saturating_sub(x));
        if width == 0 {
            break;
        }
        let rect = Rect::new(x, area.y, width, 1);
        let active = app
            .menu
            .open
            .as_ref()
            .is_some_and(|open| compact || open.category == index);
        let item_style = if active {
            style
                .fg(app.theme.background.unwrap_or(ratatui::style::Color::Reset))
                .bg(app.theme.accent)
        } else {
            style.fg(app.theme.text)
        };
        frame.render_widget(
            Paragraph::new(format!("  {label}  ")).style(item_style),
            rect,
        );
        app.menu.categories.push((
            rect,
            if compact {
                app.menu.open.as_ref().map_or(0, |open| open.category)
            } else {
                index
            },
        ));
        x = x.saturating_add(width);
    }
}

pub(crate) fn draw_popup(frame: &mut Frame, app: &mut App) {
    app.menu.rows.clear();
    app.menu.popup = None;
    app.menu.previous = None;
    app.menu.next = None;
    if !app.menu_input_available() {
        app.menu.open = None;
        return;
    }
    let Some(open) = &app.menu.open else { return };
    let category = open.category;
    let entries = app.menu_entries(category);
    let selected = open.selected.min(entries.len().saturating_sub(1));
    app.menu.open.as_mut().unwrap().selected = selected;
    let screen = frame.area();
    if screen.width < 4 || screen.height < 4 {
        return;
    }
    let desired = entries
        .iter()
        .map(|entry| entry.label.width() + app.command_hint(entry.action).width() + 9)
        .max()
        .unwrap_or(20) as u16;
    let enabled = entries
        .get(selected)
        .is_some_and(|entry| app.command_enabled(entry.action));
    let footer_text = menu_footer(app, enabled);
    let width = desired
        .max(footer_text.width().saturating_add(2).min(u16::MAX as usize) as u16)
        .max(28)
        .min(screen.width);
    let height = (entries.len() as u16 + 4).min(screen.height.saturating_sub(1));
    let anchor = app
        .menu
        .categories
        .iter()
        .find(|(_, index)| *index == category)
        .map(|(rect, _)| *rect);
    let x = anchor
        .map_or(screen.x, |rect| rect.x)
        .min(screen.right().saturating_sub(width));
    let y = anchor
        .map_or(screen.y, |rect| rect.bottom())
        .min(screen.bottom().saturating_sub(height));
    let area = Rect::new(x, y, width, height);
    app.menu.popup = Some(area);
    let style = Style::default()
        .fg(app.theme.text)
        .bg(app.theme.background.unwrap_or(ratatui::style::Color::Reset));
    frame.render_widget(Clear, area);
    let count = height.saturating_sub(4) as usize;
    let start = selected.saturating_sub(count.saturating_sub(1));
    let title = format!(
        " < {} > {}",
        Category::ALL[category].label(),
        if start > 0 || start + count < entries.len() {
            "..."
        } else {
            ""
        }
    );
    app.menu.previous = Some(Rect::new(
        area.x + 1,
        area.y,
        3.min(width.saturating_sub(2)),
        1,
    ));
    let next_x = area.x + 5 + Category::ALL[category].label().len() as u16;
    if next_x < area.right().saturating_sub(1) {
        app.menu.next = Some(Rect::new(
            next_x,
            area.y,
            2.min(area.right() - 1 - next_x),
            1,
        ));
    }
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(app.theme.border))
            .style(style),
        area,
    );
    for (offset, entry) in entries.iter().enumerate().skip(start).take(count) {
        let rect = Rect::new(
            area.x + 1,
            area.y + 1 + (offset - start) as u16,
            width - 2,
            1,
        );
        let enabled = app.command_enabled(entry.action);
        let hint = app.command_hint(entry.action);
        let check = if app.command_checked(entry.action) {
            "*"
        } else {
            " "
        };
        let available = rect.width.saturating_sub(4) as usize;
        let hint = if hint.width() + 6 < available {
            hint
        } else {
            String::new()
        };
        let label_width = available.saturating_sub(hint.width() + usize::from(!hint.is_empty()));
        let label = fit_label(&entry.label, label_width);
        let padding = available.saturating_sub(label.width() + hint.width());
        let selected = offset == selected;
        let row_style = if selected {
            style
                .fg(app.theme.background.unwrap_or(ratatui::style::Color::Reset))
                .bg(if enabled {
                    app.theme.accent
                } else {
                    app.theme.text_muted
                })
        } else if enabled {
            style
        } else {
            style.fg(app.theme.text_muted).add_modifier(Modifier::DIM)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::raw(format!(
                " {check} {label}{}{} ",
                " ".repeat(padding),
                hint
            ))]))
            .style(row_style),
            rect,
        );
        app.menu.rows.push((rect, offset));
    }
    let footer = Rect::new(area.x + 1, area.bottom() - 2, width - 2, 1);
    let hint = if footer_text.width() <= footer.width as usize {
        footer_text
    } else {
        let cancel = app.keybindings.menu_keys(MenuAction::Cancel);
        let cancel = cancel.split(" / ").next().unwrap_or("");
        let short = format!(" {cancel} close");
        if short.width() <= footer.width as usize {
            short
        } else {
            fit_label(cancel, footer.width as usize)
        }
    };
    frame.render_widget(
        Paragraph::new(hint).style(style.fg(app.theme.text_muted)),
        footer,
    );
}

fn menu_footer(app: &App, enabled: bool) -> String {
    let cancel = app.keybindings.menu_keys(MenuAction::Cancel);
    if enabled {
        format!(
            " {} select  {} close",
            app.keybindings.menu_keys(MenuAction::Accept),
            cancel
        )
    } else {
        format!(" Unavailable  {cancel} close")
    }
}

fn fit_label(label: &str, width: usize) -> String {
    if label.width() <= width {
        return label.to_string();
    }
    let mut result = String::new();
    for ch in label.chars() {
        if result.width() + unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) + 1 > width {
            break;
        }
        result.push(ch);
    }
    if width > 0 {
        result.push('~');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ViewMode;
    use crossterm::event::{KeyCode, KeyModifiers};
    use oyo_core::MultiFileDiff;
    use ratatui::{backend::TestBackend, Terminal};

    fn app() -> App {
        App::new(
            MultiFileDiff::from_file_pair(
                "demo".into(),
                "demo".into(),
                "old\n".into(),
                "new\n".into(),
            ),
            ViewMode::UnifiedPane,
            0,
            false,
            None,
        )
    }

    #[test]
    fn sidebar_and_preview_hints_do_not_advertise_different_commands() {
        let mut app = app();
        app.file_panel_mode = super::super::FilePanelMode::Comments;
        app.file_list_focused = true;
        assert!(app.command_hint(PaletteAction::ToggleStepping).is_empty());
        assert!(app.take_review_sync_requested().is_none());
        app.file_list_focused = false;
        app.view_mode = ViewMode::Preview;
        assert!(app.command_hint(PaletteAction::ToggleAutoplay).is_empty());
        assert!(app
            .command_hint(PaletteAction::ToggleAutoplayReverse)
            .is_empty());
    }

    #[test]
    fn sparse_content_rejects_review_marking_and_configured_actions() {
        use oyo_core::{multi::ContentSource, patch::PatchRecord};
        let mut app = app();
        let patch = PatchRecord::parse(b"diff --git a/f b/f\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -40 +40 @@\n-old\n+new\n".to_vec()).unwrap();
        let mut diff = MultiFileDiff::from_pending_files(None, Vec::new(), true);
        diff.append_patch(patch, ContentSource::Empty, ContentSource::Empty);
        app.multi_diff = diff;
        app.review_mode = true;
        app.review_actions = vec![crate::config::ReviewActionConfig {
            id: "inert".into(),
            command: "false".into(),
            ..Default::default()
        }];
        assert!(app.remote_manifest_complete());
        assert!(app.multi_diff.file_is_sparse(0));
        assert!(!app.command_enabled(PaletteAction::ToggleFileReviewed));
        assert!(!app.command_enabled(PaletteAction::ReviewAction(0)));
        app.execute_palette_action(PaletteAction::ReviewAction(0));
        assert!(!app.file_is_reviewed(0));
    }

    #[test]
    fn checked_toggles_and_footer_fit_the_available_popup_width() {
        let mut app = app();
        assert!(app.command_checked(PaletteAction::ToggleSyntax));
        app.autoplay = true;
        app.autoplay_reverse = true;
        assert!(app.command_checked(PaletteAction::ToggleAutoplayReverse));
        assert!(!app.command_checked(PaletteAction::ToggleAutoplay));
        app.open_menu(1);
        let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("esc close"));
    }

    #[test]
    fn menu_selects_view_and_palette_can_restore_hidden_row() {
        let mut app = app();
        app.open_menu(1);
        app.move_menu_selection(1);
        app.handle_menu_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.view_mode, ViewMode::Split);
        assert!(app.menu.open.is_none());
        app.execute_palette_action(PaletteAction::ToggleMenuBar);
        assert!(!app.menu_bar);
        app.start_command_palette();
        for ch in "Toggle menu bar".chars() {
            app.push_command_palette_char(ch);
        }
        app.apply_command_palette_selection();
        assert!(app.menu_bar);
    }

    #[test]
    fn unavailable_review_does_not_execute_after_manifest_changes() {
        let mut app = app();
        app.review_mode = true;
        app.review_actions = vec![crate::config::ReviewActionConfig {
            id: "inert".into(),
            command: "this-command-must-never-be-started".into(),
            ..Default::default()
        }];
        let action = PaletteAction::ToggleFileReviewed;
        assert!(app.command_enabled(action));
        for state in ["receiving", "failed", "complete"] {
            app.multi_diff.remote_snapshot =
                Some(serde_json::json!({"acquisition":"patch-v1", "streamState":state}));
            assert!(!app.command_enabled(action));
            app.execute_palette_action(action);
            app.execute_palette_action(PaletteAction::SyncReview);
            assert!(!app.command_enabled(PaletteAction::ReviewAction(0)));
            app.execute_palette_action(PaletteAction::ReviewAction(0));
            assert!(app.review_hook_warnings.is_empty());
            assert!(!app.file_is_reviewed(0));
            assert!(app.take_review_sync_requested().is_none());
        }
    }

    #[test]
    fn menu_resize_clicks_and_disabled_rows_stay_inside_menu() {
        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(90, 25)).unwrap();
        app.open_menu(3);
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let row = app
            .menu
            .rows
            .iter()
            .find(|(_, index)| *index == 1)
            .unwrap()
            .0;
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: row.x,
            row: row.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(app.handle_menu_mouse(event));
        assert!(app.menu.open.is_some());
        assert!(!app.file_is_reviewed(0));
        for (width, height) in [(45, 9), (46, 9), (20, 5), (4, 4), (1, 1), (100, 40)] {
            terminal.backend_mut().resize(width, height);
            terminal.resize(Rect::new(0, 0, width, height)).unwrap();
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
            for (rect, _) in &app.menu.rows {
                assert!(rect.right() <= width && rect.bottom() <= height);
            }
        }
        let event = MouseEvent {
            column: 99,
            row: 39,
            ..event
        };
        assert!(app.handle_menu_mouse(event));
        assert!(app.menu.open.is_none());
        assert!(!app.handle_menu_mouse(event));
    }

    #[test]
    fn remapped_menu_controls_and_reachable_hints_match() {
        let config: crate::config::Config = toml::from_str(
            r#"
[keybindings.menu]
next_category = ["n"]
accept = ["a"]
cancel = ["c"]
[keybindings.normal]
toggle_line_wrap = ["f6 w"]
open_theme_picker = []
quit = ["ctrl-t"]
"#,
        )
        .unwrap();
        let mut app = app();
        app.keybindings = crate::keybindings::Keybindings::from_config(&config.keybindings);
        assert_eq!(app.command_hint(PaletteAction::ToggleLineWrap), "f6 w");
        assert_eq!(app.command_hint(PaletteAction::Quit), "");
        app.open_menu(0);
        app.handle_menu_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        app.handle_menu_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(app.view_mode, ViewMode::UnifiedPane);
        assert!(app.menu.open.is_none());
        app.open_menu(0);
        app.handle_menu_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(app.menu.open.is_none());
    }

    #[test]
    fn modal_and_pending_prefixes_cannot_leak_through_menu() {
        let mut app = app();
        app.review_reply_prefix = true;
        app.pending_count = Some(5);
        app.keybindings
            .normal(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.open_menu(0);
        assert!(!app.review_reply_prefix);
        assert!(app.pending_count.is_none());
        app.close_menu();
        app.start_remote_open();
        assert!(!app.menu_input_available());
        app.remote_open = None;
        app.start_command_palette();
        assert!(!app.menu_input_available());
    }
}
