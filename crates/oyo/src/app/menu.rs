use super::{palette::PaletteAction, App};
use crate::config::{FoldContextMode, ResolvedTheme};
use crate::footer::{FooterAction, FooterHit};
use crate::keybindings::Keybindings;
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
            OpenDashboard
            | OpenRemoteReview
            | OpenFileSearch
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
            NavigateBack
            | NavigateForward
            | OpenReviewGrep
            | OpenReviewGrepChanges
            | OpenReviewGrepEverything
            | ToggleStepping
            | NextStep
            | PreviousStep
            | ToggleAutoplay
            | ToggleAutoplayReverse => Category::Navigate,
            ToggleFileReviewed
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

#[derive(Default, Debug)]
pub(crate) struct MenuState {
    pub(crate) open: Option<MenuOpen>,
    categories: Vec<(Rect, usize)>,
    rows: Vec<(Rect, usize)>,
    popup: Option<Rect>,
    footer_hits: Vec<FooterHit<MenuAction>>,
    compact: bool,
}

impl MenuState {
    pub(crate) fn clear_geometry(&mut self) {
        self.categories.clear();
        self.rows.clear();
        self.popup = None;
        self.footer_hits.clear();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuPage {
    Commands,
    Categories,
}

#[derive(Debug)]
pub(crate) struct MenuOpen {
    page: MenuPage,
    category: usize,
    selected: usize,
}

pub(crate) trait MenuHost {
    fn menu(&self) -> &MenuState;
    fn menu_mut(&mut self) -> &mut MenuState;
    fn keybindings_mut(&mut self) -> &mut Keybindings;
    fn keybindings(&self) -> &Keybindings;
    fn theme(&self) -> &ResolvedTheme;
    fn command_entries(&self) -> Vec<super::palette::PaletteEntry>;
    fn command_enabled(&self, action: PaletteAction) -> bool;
    fn command_hint(&self, action: PaletteAction) -> String;
    fn command_checked(&self, action: PaletteAction) -> bool;
    fn execute_palette_action(&mut self, action: PaletteAction);
    fn menu_input_available(&self) -> bool;
    fn reset_menu_input(&mut self) {
        self.keybindings_mut().clear_sequence();
    }
    fn menu_bar_frame_start(&mut self) {
        self.menu_mut().clear_geometry();
        self.menu_mut().compact = false;
    }
    fn open_menu(&mut self, category: usize) {
        self.reset_menu_input();
        self.menu_mut().rows.clear();
        self.menu_mut().popup = None;
        self.menu_mut().footer_hits.clear();
        self.menu_mut().open = Some(MenuOpen {
            page: MenuPage::Commands,
            category: category % 5,
            selected: 0,
        });
    }

    fn menu_entries(&self, category: usize) -> Vec<super::palette::PaletteEntry> {
        use PaletteAction::*;
        let order: &[PaletteAction] = match Category::ALL[category] {
            Category::File => &[
                OpenDashboard,
                OpenRemoteReview,
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
        for entry in &mut entries {
            if entry.action == PaletteAction::OpenDashboard {
                entry.label = "Open…".into();
            }
            if entry.action == PaletteAction::OpenRemoteReview {
                entry.label = "Open remote review…".into();
            }
        }
        entries.sort_by_key(|entry| {
            order
                .iter()
                .position(|action| *action == entry.action)
                .unwrap_or(order.len())
        });
        entries
    }

    fn move_menu(&mut self, delta: isize) {
        if let Some(open) = &mut self.menu_mut().open {
            let current = if open.page == MenuPage::Categories {
                open.selected
            } else {
                open.category
            };
            open.category = (current as isize + delta).rem_euclid(5) as usize;
            open.page = MenuPage::Commands;
            open.selected = 0;
        }
    }

    fn move_menu_selection(&mut self, delta: isize) {
        if let Some(open) = &self.menu().open {
            let count = self.menu_item_count();
            self.menu_mut().open.as_mut().unwrap().selected =
                (open.selected as isize + delta).rem_euclid(count.max(1) as isize) as usize;
        }
    }

    fn menu_item_count(&self) -> usize {
        self.menu().open.as_ref().map_or(0, |open| match open.page {
            MenuPage::Categories => Category::ALL.len(),
            MenuPage::Commands => self.menu_entries(open.category).len(),
        })
    }

    fn choose_menu_category(&mut self) {
        let selected = self.menu().open.as_ref().map_or(0, |open| open.category);
        self.open_menu(selected);
        let open = self.menu_mut().open.as_mut().unwrap();
        open.page = MenuPage::Categories;
        open.selected = selected;
    }

    fn activate_menu(&mut self) {
        let Some(open) = &self.menu().open else {
            return;
        };
        if open.page == MenuPage::Categories {
            let category = open.selected;
            self.open_menu(category);
            return;
        }
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

    fn handle_menu_key(&mut self, key: KeyEvent) {
        match self.keybindings_mut().menu(key) {
            Dispatch::Matched(action) => self.apply_menu_action(action),
            Dispatch::Pending | Dispatch::Unmatched => {}
        }
    }

    fn apply_menu_action(&mut self, action: MenuAction) {
        if self.menu().open.is_none() {
            return;
        }
        match action {
            MenuAction::Cancel => self.close_menu(),
            MenuAction::PreviousCategory => self.move_menu(-1),
            MenuAction::NextCategory => self.move_menu(1),
            MenuAction::NextItem => self.move_menu_selection(1),
            MenuAction::PreviousItem => self.move_menu_selection(-1),
            MenuAction::FirstItem => {
                if let Some(open) = &mut self.menu_mut().open {
                    open.selected = 0;
                }
            }
            MenuAction::LastItem => {
                let last = self.menu_item_count().saturating_sub(1);
                if let Some(open) = &mut self.menu_mut().open {
                    open.selected = last;
                }
            }
            MenuAction::Accept => self.activate_menu(),
        }
    }

    fn close_menu(&mut self) {
        self.menu_mut().open = None;
        self.menu_mut().footer_hits.clear();
        self.keybindings_mut().clear_sequence();
    }

    fn handle_menu_mouse(&mut self, event: MouseEvent) -> bool {
        if !self.menu_input_available() {
            return false;
        }
        let position = (event.column, event.row).into();
        let category = self
            .menu()
            .categories
            .iter()
            .find(|(rect, _)| rect.contains(position))
            .map(|(_, index)| *index);
        if self.menu().open.is_some() {
            match event.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(action) =
                        crate::footer::action_at(&self.menu().footer_hits, event.column, event.row)
                    {
                        self.apply_menu_action(action);
                    } else if let Some(category) = category {
                        if self.menu().compact {
                            if self
                                .menu()
                                .open
                                .as_ref()
                                .is_some_and(|open| open.page == MenuPage::Categories)
                            {
                                self.close_menu();
                            } else {
                                self.choose_menu_category();
                            }
                        } else if self.menu().open.as_ref().is_some_and(|open| {
                            open.category == category && open.page == MenuPage::Commands
                        }) {
                            self.close_menu();
                        } else {
                            self.open_menu(category);
                        }
                    } else if let Some((_, index)) = self
                        .menu()
                        .rows
                        .iter()
                        .find(|(rect, _)| rect.contains(position))
                    {
                        self.menu_mut().open.as_mut().unwrap().selected = *index;
                        self.activate_menu();
                    } else if !self
                        .menu()
                        .popup
                        .is_some_and(|rect| rect.contains(position))
                    {
                        self.close_menu();
                    }
                }
                MouseEventKind::Down(_) => self.menu_mut().open = None,
                MouseEventKind::Moved => {
                    if let Some(category) = category {
                        if !self.menu().compact
                            && self
                                .menu()
                                .open
                                .as_ref()
                                .is_some_and(|open| open.category != category)
                        {
                            self.open_menu(category);
                        }
                    } else if let Some((_, index)) = self
                        .menu()
                        .rows
                        .iter()
                        .find(|(rect, _)| rect.contains(position))
                    {
                        self.menu_mut().open.as_mut().unwrap().selected = *index;
                    }
                }
                MouseEventKind::ScrollDown => self.move_menu_selection(1),
                MouseEventKind::ScrollUp => self.move_menu_selection(-1),
                _ => {}
            }
            return true;
        }
        if let (MouseEventKind::Down(MouseButton::Left), Some(category)) = (event.kind, category) {
            if self.menu().compact {
                self.choose_menu_category();
            } else {
                self.open_menu(category);
            }
            return true;
        }
        false
    }
}

impl MenuHost for App {
    fn menu(&self) -> &MenuState {
        &self.menu
    }
    fn menu_mut(&mut self) -> &mut MenuState {
        &mut self.menu
    }
    fn keybindings_mut(&mut self) -> &mut Keybindings {
        &mut self.keybindings
    }
    fn keybindings(&self) -> &Keybindings {
        &self.keybindings
    }
    fn theme(&self) -> &ResolvedTheme {
        &self.theme
    }
    fn command_entries(&self) -> Vec<super::palette::PaletteEntry> {
        App::command_entries(self)
    }
    fn command_enabled(&self, action: PaletteAction) -> bool {
        App::command_enabled(self, action)
    }
    fn execute_palette_action(&mut self, action: PaletteAction) {
        App::execute_palette_action(self, action);
    }
    fn reset_menu_input(&mut self) {
        self.keybindings.clear_sequence();
        self.reset_count();
        self.review_edit_prefix = false;
        self.review_reply_prefix = false;
        self.review_resolve_prefix = false;
        self.review_delete_prefix = false;
        self.review_overflow_prefix = false;
        self.pr_reply_prefix = false;
    }
    fn menu_input_available(&self) -> bool {
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

fn sidebar_style(app: &impl MenuHost) -> Style {
    Style::default().fg(app.theme().text).bg(app
        .theme()
        .background_panel
        .or(app.theme().background)
        .unwrap_or(ratatui::style::Color::Reset))
}

fn sidebar_selected_style(app: &impl MenuHost) -> Style {
    sidebar_style(app)
        .fg(app.theme().accent)
        .bg(app
            .theme()
            .background_element
            .or(app.theme().background_panel)
            .or(app.theme().background)
            .unwrap_or(ratatui::style::Color::Reset))
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn draw_bar(frame: &mut Frame, app: &mut impl MenuHost, area: Rect) {
    app.menu_mut().categories.clear();
    if area.is_empty() {
        return;
    }
    let style = sidebar_style(app);
    frame.render_widget(Paragraph::new("").style(style), area);
    let compact = area.width < 46;
    app.menu_mut().compact = compact;
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
            .menu()
            .open
            .as_ref()
            .is_some_and(|open| compact || open.category == index);
        frame.render_widget(
            Paragraph::new(format!("  {label}  ")).style(if active {
                sidebar_selected_style(app)
            } else {
                style
            }),
            rect,
        );
        app.menu_mut().categories.push((rect, index));
        x = x.saturating_add(width);
    }
}

pub(crate) fn draw_popup(frame: &mut Frame, app: &mut impl MenuHost) {
    app.menu_mut().rows.clear();
    app.menu_mut().popup = None;
    app.menu_mut().footer_hits.clear();
    if !app.menu_input_available() {
        app.menu_mut().open = None;
        return;
    }
    let Some(open) = &app.menu().open else { return };
    let category = open.category;
    let choosing = open.page == MenuPage::Categories;
    let entries = app.menu_entries(category);
    let total = if choosing {
        Category::ALL.len()
    } else {
        entries.len()
    };
    let selected = open.selected.min(total.saturating_sub(1));
    app.menu_mut().open.as_mut().unwrap().selected = selected;
    let screen = frame.area();
    if screen.width < 4 || screen.height < 4 {
        return;
    }
    let mut items = Vec::new();
    if choosing {
        for (index, category) in Category::ALL.iter().enumerate() {
            items.push((Some(index), category.label(), None));
        }
    } else {
        for (index, entry) in entries.iter().enumerate() {
            if index > 0 && menu_group(entry.action) != menu_group(entries[index - 1].action) {
                items.push((None, "", None));
            }
            items.push((Some(index), entry.label.as_str(), Some(entry.action)));
        }
    }
    let selected_row = items
        .iter()
        .position(|(index, _, _)| *index == Some(selected))
        .unwrap_or(0);
    let desired = if choosing {
        28
    } else {
        entries
            .iter()
            .map(|entry| entry.label.width() + app.command_hint(entry.action).width() + 9)
            .max()
            .unwrap_or(20)
            .min(u16::MAX as usize) as u16
    };
    let enabled = choosing
        || entries
            .get(selected)
            .is_some_and(|entry| app.command_enabled(entry.action));
    let footer = menu_footer_actions(app, enabled);
    let footer_width: usize = footer
        .iter()
        .map(|action| {
            action
                .spans
                .iter()
                .map(|span| span.content.width())
                .sum::<usize>()
        })
        .sum::<usize>()
        + footer.len().saturating_sub(1) * 4;
    let width = desired
        .max(footer_width.saturating_add(4).min(u16::MAX as usize) as u16)
        .max(28)
        .min(screen.width);
    let height = (items.len().saturating_add(2).min(u16::MAX as usize) as u16)
        .min(screen.height.saturating_sub(1));
    let anchor = app
        .menu()
        .categories
        .iter()
        .find(|(_, index)| app.menu().compact || *index == category)
        .map(|(rect, _)| *rect);
    let x = anchor
        .map_or(screen.x, |rect| rect.x)
        .min(screen.right().saturating_sub(width));
    let y = anchor
        .map_or(screen.y, |rect| rect.bottom())
        .min(screen.bottom().saturating_sub(height));
    let area = Rect::new(x, y, width, height);
    app.menu_mut().popup = Some(area);
    let style = sidebar_style(app);
    let border = Style::default().fg(app.theme().border_active);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(border)
            .style(style),
        area,
    );
    let count = height.saturating_sub(2) as usize;
    let start = selected_row.saturating_sub(count.saturating_sub(1));
    for (offset, (index, label, action)) in items.into_iter().enumerate().skip(start).take(count) {
        let rect = Rect::new(
            area.x + 1,
            area.y + 1 + (offset - start) as u16,
            width - 2,
            1,
        );
        let Some(index) = index else {
            frame.render_widget(
                Paragraph::new("─".repeat(rect.width as usize)).style(style.fg(app.theme().border)),
                rect,
            );
            continue;
        };
        let enabled = action.is_none_or(|action| app.command_enabled(action));
        let hint = action.map_or_else(String::new, |action| app.command_hint(action));
        let check = if action.is_some_and(|action| app.command_checked(action)) {
            "•"
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
        let label = fit_label(label, label_width);
        let padding = available.saturating_sub(label.width() + hint.width());
        let mut row_style = if index == selected {
            sidebar_selected_style(app)
        } else {
            style
        };
        if !enabled {
            row_style = row_style
                .fg(app.theme().text_muted)
                .add_modifier(Modifier::DIM);
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::raw(format!(
                " {check} {label}{}{} ",
                " ".repeat(padding),
                hint
            ))))
            .style(row_style),
            rect,
        );
        app.menu_mut().rows.push((rect, index));
    }
    let actions = if footer_width + 4 <= width as usize {
        footer
    } else {
        menu_cancel_actions(app, width as usize)
    };
    let footer_area = Rect::new(area.x, area.bottom() - 1, width, 1);
    let (line, hits) = crate::footer::footer_line(footer_area, border, actions);
    app.menu_mut().footer_hits = hits;
    frame.render_widget(Paragraph::new(line).style(style), footer_area);
}

fn primary_menu_key(app: &impl MenuHost, action: MenuAction) -> String {
    app.keybindings()
        .menu_keys(action)
        .split(" / ")
        .next()
        .unwrap_or("")
        .to_string()
}

fn menu_footer_actions(app: &impl MenuHost, enabled: bool) -> Vec<FooterAction<MenuAction>> {
    let key_style = Style::default()
        .fg(app.theme().primary)
        .add_modifier(Modifier::BOLD);
    let accept = primary_menu_key(app, MenuAction::Accept);
    let cancel = primary_menu_key(app, MenuAction::Cancel);
    let mut actions = Vec::new();
    if !enabled {
        actions.push(FooterAction::new(None, vec![Span::raw("Unavailable")]));
    } else if !accept.is_empty() {
        actions.push(FooterAction::new(
            Some(MenuAction::Accept),
            vec![Span::styled(accept, key_style), Span::raw(" select")],
        ));
    }
    if !cancel.is_empty() {
        actions.push(FooterAction::new(
            Some(MenuAction::Cancel),
            vec![Span::styled(cancel, key_style), Span::raw(" cancel")],
        ));
    }
    actions
}

fn menu_cancel_actions(app: &impl MenuHost, width: usize) -> Vec<FooterAction<MenuAction>> {
    let cancel = primary_menu_key(app, MenuAction::Cancel);
    if cancel.is_empty() {
        return Vec::new();
    }
    let key_style = Style::default()
        .fg(app.theme().primary)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled(cancel.clone(), key_style)];
    if cancel.width() + 11 <= width {
        spans.push(Span::raw(" cancel"));
    }
    vec![FooterAction::new(Some(MenuAction::Cancel), spans)]
}

fn menu_group(action: PaletteAction) -> u8 {
    use PaletteAction::*;
    match action {
        OpenDashboard | OpenRemoteReview => 0,
        OpenFileSearch
        | CopyFilePath
        | CopyFilePosition
        | CopyAbsoluteFilePath
        | CopyAbsoluteFilePosition
        | CopyFileCursorPosition
        | CopyAbsoluteFileCursorPosition => 1,
        RefreshCurrentFile | RefreshAllFiles => 2,
        CopySessionName | RenameSession => 3,
        Quit => 4,
        SetViewMode(_) => 0,
        ToggleLineWrap | ToggleFoldContext | ToggleSyntax => 1,
        ToggleFilePanel | ToggleMenuBar | ToggleZen => 2,
        OpenThemePicker => 3,
        NavigateBack | NavigateForward => 0,
        OpenReviewGrep | OpenReviewGrepChanges | OpenReviewGrepEverything => 1,
        PreviousStep | NextStep | ToggleStepping => 2,
        ToggleAutoplay | ToggleAutoplayReverse => 3,
        ToggleFileReviewed => 0,
        OpenCommentPicker | OpenPrComments | OpenOutdatedComments => 1,
        ToggleIncrementalReview | ToggleReviewedFilesVisibility => 2,
        ReviewAction(_) | SyncReview => 3,
        ToggleHelp | OpenCommandPalette => 0,
        OpenSettings => 1,
        OpenMenu | ToggleViewMode => 0,
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
    fn footer_select_and_cancel_use_guarded_menu_actions() {
        let mut app = app();
        app.open_menu(1);
        app.menu.open.as_mut().unwrap().selected = 1;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let hit = app
            .menu
            .footer_hits
            .iter()
            .find(|hit| hit.action == MenuAction::Accept)
            .copied()
            .unwrap();
        let mouse = |hit: FooterHit<MenuAction>| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.area.x,
            row: hit.area.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(app.handle_menu_mouse(mouse(hit)));
        assert_eq!(app.view_mode, ViewMode::Split);
        assert!(app.menu.open.is_none());
        app.open_menu(0);
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let hit = app
            .menu
            .footer_hits
            .iter()
            .find(|hit| hit.action == MenuAction::Cancel)
            .copied()
            .unwrap();
        assert!(app.handle_menu_mouse(mouse(hit)));
        assert!(app.menu.open.is_none());
        assert_eq!(app.view_mode, ViewMode::Split);
    }

    #[test]
    fn separators_are_visible_and_inert_in_every_category() {
        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        for category in 0..5 {
            app.open_menu(category);
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
            let popup = app.menu.popup.unwrap();
            let separator_y = (popup.y + 1..popup.bottom() - 1)
                .find(|y| terminal.backend().buffer()[(popup.x + 1, *y)].symbol() == "─")
                .unwrap();
            let selected = app.menu.open.as_ref().unwrap().selected;
            assert!(app.handle_menu_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: popup.x + 2,
                row: separator_y,
                modifiers: KeyModifiers::NONE
            }));
            assert_eq!(app.menu.open.as_ref().unwrap().selected, selected);
            assert_eq!(app.menu.rows.len(), app.menu_entries(category).len());
        }
    }

    #[test]
    fn narrow_footer_does_not_advertise_an_unbound_cancel() {
        let mut app = app();
        let config: crate::config::Config =
            toml::from_str("[keybindings.menu]\ncancel = []").unwrap();
        app.keybindings = crate::keybindings::Keybindings::from_config(&config.keybindings);
        for width in 11..=15 {
            app.open_menu(0);
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
            let popup = app.menu.popup.unwrap();
            let buffer = terminal.backend().buffer();
            let footer: String = (popup.x..popup.right())
                .map(|x| buffer[(x, popup.bottom() - 1)].symbol())
                .collect();
            assert!(!footer.contains("cancel"));
        }
    }

    #[test]
    fn menu_uses_sidebar_colors_bullets_and_the_modal_bottom_border() {
        use ratatui::style::Color;
        let mut app = app();
        app.theme.background = Some(Color::Red);
        app.theme.background_panel = Some(Color::Blue);
        app.theme.background_element = Some(Color::Green);
        app.theme.accent = Color::Yellow;
        app.open_menu(1);
        let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let popup = app.menu.popup.unwrap();
        let first = app.menu.rows[0].0;
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].bg, Color::Blue);
        assert_eq!(buffer[(9, 0)].bg, Color::Green);
        assert_eq!(buffer[(9, 0)].fg, Color::Yellow);
        assert_eq!(buffer[(first.x + 1, first.y)].symbol(), "•");
        let top: String = (popup.x..popup.right())
            .map(|x| buffer[(x, popup.y)].symbol())
            .collect();
        assert!(!top.contains("View"));
        let bottom: String = (popup.x..popup.right())
            .map(|x| buffer[(x, popup.bottom() - 1)].symbol())
            .collect();
        assert!(bottom.starts_with("╰ "));
        assert!(bottom.contains("enter select    esc cancel"));
        assert!(!bottom.contains("space"));
    }

    #[test]
    fn compact_category_chooser_keeps_mouse_switching_without_border_arrows() {
        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let click = |x, y| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(app.handle_menu_mouse(click(3, 0)));
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert!(app.menu.open.as_ref().unwrap().page == MenuPage::Categories);
        let review = app.menu.rows[3].0;
        assert!(app.handle_menu_mouse(click(review.x, review.y)));
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert_eq!(app.menu.open.as_ref().unwrap().category, 3);
        assert!(app.menu.open.as_ref().unwrap().page == MenuPage::Commands);
        assert!(app.handle_menu_mouse(click(3, 0)));
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let view = app.menu.rows[1].0;
        assert!(app.handle_menu_mouse(click(view.x, view.y)));
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert_eq!(app.menu.open.as_ref().unwrap().category, 1);
    }

    #[test]
    fn category_bar_and_tabs_are_adjacent_without_an_unused_row() {
        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert_eq!(app.topbar_area.unwrap().1, 1);
        app.menu_bar = false;
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert_eq!(app.topbar_area.unwrap().1, 0);
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
        assert!(text.contains("esc cancel"));
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
