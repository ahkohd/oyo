use crate::app::{App, FilePanelMode, FoldContextDirection, TopbarTabContent, ViewMode};
use crate::config;
use crate::keybindings::{
    Dispatch, FileFilterAction, GlobalAction, HelpAction, LineInputAction, NormalAction,
    PickerAction, ReviewEditorAction, SelectionAction,
};
use anyhow::Result;
use crossterm::{
    event::{Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};

use super::{coalesce_key_repeats, open_current_file_in_editor, TuiTerminal};

fn is_force_quit_key(key: KeyEvent) -> bool {
    key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('c' | 'C'))
}

fn should_force_quit(app: &App, key: KeyEvent) -> bool {
    !app.review_editor_active() && is_force_quit_key(key)
}

pub(crate) fn handle_app_key(
    app: &mut App,
    key: KeyEvent,
    pending_event: &mut Option<Event>,
    terminal: &mut TuiTerminal,
    editor_config: &config::EditorConfig,
) -> Result<()> {
    if should_force_quit(app, key) {
        app.force_quit();
        return Ok(());
    }

    if app.settings_leave_confirmation_active() {
        app.handle_settings_leave_key(key);
        return Ok(());
    }

    if app.settings_reset_confirmation_active() {
        app.handle_settings_reset_key(key);
        return Ok(());
    }

    if app.quit_confirmation_active() {
        app.handle_quit_confirmation_key(key);
        return Ok(());
    }

    if app.show_help {
        handle_help_key(app, key);
        return Ok(());
    }

    if app.review_remote_picker_active() {
        app.handle_review_remote_picker_key(key);
        return Ok(());
    }

    if app.review_delete_confirmation_active() {
        app.handle_review_delete_confirmation_key(key);
        return Ok(());
    }

    if app.review_editor_active() {
        handle_review_editor_key(app, key);
        return Ok(());
    }

    if app.status_mode_menu_open() {
        if key.code == KeyCode::Esc {
            app.close_status_mode_menu();
            return Ok(());
        }
        app.close_status_mode_menu();
    }

    if app.review_comment_context_menu.is_some() {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                app.move_review_comment_context_menu_active(true);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.move_review_comment_context_menu_active(false);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                app.activate_review_comment_context_menu();
            }
            _ => {
                app.close_review_comment_context_menu();
            }
        }
        return Ok(());
    }

    if app.file_context_menu.is_some() {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                app.move_file_context_menu_active(true);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.move_file_context_menu_active(false);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                app.activate_file_context_menu();
            }
            _ => {
                app.close_file_context_menu();
            }
        }
        return Ok(());
    }

    if key.code == KeyCode::Esc && app.show_path_popup {
        app.show_path_popup = false;
        return Ok(());
    }

    if app.session_rename_active() {
        handle_session_rename_key(app, key);
        return Ok(());
    }

    if app.keybindings.normal_sequence_pending() {
        return handle_normal_key(app, key, pending_event, terminal, editor_config);
    }

    if handle_global_key(app, key) {
        return Ok(());
    }

    if app.command_palette_active() {
        handle_command_palette_key(app, key);
        return Ok(());
    }

    if app.file_search_active() {
        handle_file_search_key(app, key);
        return Ok(());
    }

    if app.comment_picker_active() {
        handle_comment_picker_key(app, key);
        return Ok(());
    }

    if app.theme_picker_active() {
        handle_theme_picker_key(app, key);
        return Ok(());
    }

    if app.file_filter_active {
        handle_file_filter_key(app, key);
        return Ok(());
    }

    if app.goto_active() {
        handle_goto_key(app, key);
        return Ok(());
    }

    if key.code == KeyCode::Esc && app.search_bar_visible() {
        app.clear_search();
        return Ok(());
    }

    if app.search_active() {
        handle_search_key(app, key);
        return Ok(());
    }

    if app.active_settings_view() {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                app.move_settings_selection(true);
                return Ok(());
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.move_settings_selection(false);
                return Ok(());
            }
            KeyCode::Left | KeyCode::Char('h') => {
                app.adjust_selected_setting(false);
                return Ok(());
            }
            KeyCode::Right | KeyCode::Char('l') => {
                app.adjust_selected_setting(true);
                return Ok(());
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                app.activate_selected_setting();
                return Ok(());
            }
            _ => {}
        }
    }

    if app.diff_selection_mode_active() && handle_selection_key(app, key) {
        return Ok(());
    }

    if app.file_panel_mode == FilePanelMode::Comments
        && app.file_list_focused
        && app.handle_comments_sidebar_action_key(key)
    {
        return Ok(());
    }

    handle_normal_key(app, key, pending_event, terminal, editor_config)
}

fn handle_selection_key(app: &mut App, key: KeyEvent) -> bool {
    if app.selection_toolbar_visible() && app.handle_selection_action_key(key) {
        return true;
    }
    match app.keybindings.selection(key) {
        Dispatch::Matched(SelectionAction::Cancel) => {
            app.clear_diff_selection();
            true
        }
        Dispatch::Matched(SelectionAction::Copy) => {
            app.copy_diff_selection();
            app.clear_diff_selection();
            true
        }
        Dispatch::Matched(SelectionAction::ShowActions) => app.show_selection_toolbar(),
        Dispatch::Matched(SelectionAction::Left) => app.move_diff_selection(-1, 0),
        Dispatch::Matched(SelectionAction::Right) => app.move_diff_selection(1, 0),
        Dispatch::Matched(SelectionAction::Up) => app.move_diff_selection(0, -1),
        Dispatch::Matched(SelectionAction::Down) => app.move_diff_selection(0, 1),
        Dispatch::Matched(SelectionAction::ReanchorLeft) => app.reanchor_diff_selection(-1, 0),
        Dispatch::Matched(SelectionAction::ReanchorRight) => app.reanchor_diff_selection(1, 0),
        Dispatch::Matched(SelectionAction::ReanchorUp) => app.reanchor_diff_selection(0, -1),
        Dispatch::Matched(SelectionAction::ReanchorDown) => app.reanchor_diff_selection(0, 1),
        Dispatch::Matched(SelectionAction::ReanchorStart) => {
            app.reanchor_diff_selection_to_boundary(false)
        }
        Dispatch::Matched(SelectionAction::ReanchorEnd) => {
            app.reanchor_diff_selection_to_boundary(true)
        }
        Dispatch::Matched(SelectionAction::ReanchorHalfPageDown) => {
            app.reanchor_diff_selection_half_page_down()
        }
        Dispatch::Matched(SelectionAction::GotoStart) => app.move_diff_selection_to_boundary(false),
        Dispatch::Matched(SelectionAction::GotoEnd) => app.move_diff_selection_to_boundary(true),
        Dispatch::Matched(SelectionAction::GotoHalfPageDown) => {
            app.move_diff_selection_half_page_down()
        }
        Dispatch::Pending => true,
        Dispatch::Unmatched => false,
    }
}

fn handle_global_key(app: &mut App, key: KeyEvent) -> bool {
    match app.keybindings.global(key) {
        Dispatch::Matched(GlobalAction::OpenCommandPalette) => {
            app.reset_count();
            if app.command_palette_active() {
                app.stop_command_palette();
            } else {
                app.start_command_palette();
            }
            true
        }
        Dispatch::Matched(GlobalAction::OpenFileSearch) => {
            if app.multi_diff.file_count() == 0 {
                return false;
            }
            app.reset_count();
            if app.file_search_active() {
                app.stop_file_search();
            } else {
                app.start_file_search();
            }
            true
        }
        Dispatch::Matched(GlobalAction::OpenCommentPicker) => {
            if !app.review_mode() {
                return false;
            }
            app.reset_count();
            if app.comment_picker_active() {
                app.stop_comment_picker();
            } else {
                app.start_comment_picker();
            }
            true
        }
        Dispatch::Matched(GlobalAction::OpenThemePicker) => {
            app.reset_count();
            if app.theme_picker_active() {
                app.stop_theme_picker();
            } else {
                app.start_theme_picker();
            }
            true
        }
        Dispatch::Pending => true,
        Dispatch::Unmatched => false,
    }
}

fn printable_char(key: KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(c)
        }
        _ => None,
    }
}

fn handle_help_key(app: &mut App, key: KeyEvent) {
    match app.keybindings.help(key) {
        Dispatch::Matched(HelpAction::Close) => app.toggle_help(),
        Dispatch::Matched(HelpAction::ScrollDown) => app.help_scroll_down(),
        Dispatch::Matched(HelpAction::ScrollUp) => app.help_scroll_up(),
        Dispatch::Pending | Dispatch::Unmatched => {}
    }
}

fn handle_review_editor_key(app: &mut App, key: KeyEvent) {
    if app.handle_review_action_key(key) {
        return;
    }
    match app.keybindings.review_editor(key) {
        Dispatch::Matched(ReviewEditorAction::Cancel) => {
            if !app.review_cancel_mention_picker() {
                app.review_cancel_editor();
            }
        }
        Dispatch::Matched(ReviewEditorAction::Save) => app.review_save_editor(),
        Dispatch::Matched(ReviewEditorAction::InsertNewline) => {
            if !app.review_accept_mention() {
                app.review_insert_newline();
            }
        }
        Dispatch::Matched(ReviewEditorAction::AcceptMention) => {
            let _ = app.review_accept_mention();
        }
        Dispatch::Matched(ReviewEditorAction::Backspace) => app.review_backspace(),
        Dispatch::Matched(ReviewEditorAction::Delete) => app.review_delete(),
        Dispatch::Matched(ReviewEditorAction::Left) => app.review_move_left(),
        Dispatch::Matched(ReviewEditorAction::Right) => app.review_move_right(),
        Dispatch::Matched(ReviewEditorAction::Up) => {
            if app.review_mention_picker_active() {
                app.review_mention_move_selection(-1);
            } else {
                app.review_move_up();
            }
        }
        Dispatch::Matched(ReviewEditorAction::Down) => {
            if app.review_mention_picker_active() {
                app.review_mention_move_selection(1);
            } else {
                app.review_move_down();
            }
        }
        Dispatch::Matched(ReviewEditorAction::Home) => app.review_move_home(),
        Dispatch::Matched(ReviewEditorAction::End) => app.review_move_end(),
        Dispatch::Matched(ReviewEditorAction::Clear) => app.review_clear_editor_text(),
        Dispatch::Matched(ReviewEditorAction::MentionNext) => {
            if app.review_mention_picker_active() {
                app.review_mention_move_selection(1);
            }
        }
        Dispatch::Matched(ReviewEditorAction::MentionPrev) => {
            if app.review_mention_picker_active() {
                app.review_mention_move_selection(-1);
            }
        }
        Dispatch::Pending => {}
        Dispatch::Unmatched => {
            if let Some(c) = printable_char(key) {
                app.review_insert_char(c);
            }
        }
    }
}

fn handle_command_palette_key(app: &mut App, key: KeyEvent) {
    match app.keybindings.command_palette(key) {
        Dispatch::Matched(PickerAction::Cancel) => app.stop_command_palette(),
        Dispatch::Matched(PickerAction::Accept) => app.apply_command_palette_selection(),
        Dispatch::Matched(PickerAction::Backspace) => {
            if app.command_palette_query().is_empty() {
                app.stop_command_palette();
            } else {
                app.pop_command_palette_char();
            }
        }
        Dispatch::Matched(PickerAction::Clear) => app.clear_command_palette_text(),
        Dispatch::Matched(PickerAction::SelectNext) => app.move_command_palette_selection(1),
        Dispatch::Matched(PickerAction::SelectPrev) => app.move_command_palette_selection(-1),
        Dispatch::Pending => {}
        Dispatch::Unmatched => {
            if let Some(c) = printable_char(key) {
                app.push_command_palette_char(c);
            }
        }
    }
}

fn handle_theme_picker_key(app: &mut App, key: KeyEvent) {
    match app.keybindings.theme_picker(key) {
        Dispatch::Matched(PickerAction::Cancel) => app.stop_theme_picker(),
        Dispatch::Matched(PickerAction::Accept) => app.apply_theme_picker_selection(),
        Dispatch::Matched(PickerAction::Backspace) => {
            if app.theme_picker_query().is_empty() {
                app.stop_theme_picker();
            } else {
                app.pop_theme_picker_char();
            }
        }
        Dispatch::Matched(PickerAction::Clear) => app.clear_theme_picker_text(),
        Dispatch::Matched(PickerAction::SelectNext) => app.move_theme_picker_selection(1),
        Dispatch::Matched(PickerAction::SelectPrev) => app.move_theme_picker_selection(-1),
        Dispatch::Pending => {}
        Dispatch::Unmatched => {
            if let Some(c) = printable_char(key) {
                app.push_theme_picker_char(c);
            }
        }
    }
}

fn handle_file_search_key(app: &mut App, key: KeyEvent) {
    match app.keybindings.file_search(key) {
        Dispatch::Matched(PickerAction::Cancel) => app.stop_file_search(),
        Dispatch::Matched(PickerAction::Accept) => app.apply_file_search_selection(),
        Dispatch::Matched(PickerAction::Backspace) => {
            if app.file_search_query().is_empty() {
                app.stop_file_search();
            } else {
                app.pop_file_search_char();
            }
        }
        Dispatch::Matched(PickerAction::Clear) => app.clear_file_search_text(),
        Dispatch::Matched(PickerAction::SelectNext) => app.move_file_search_selection(1),
        Dispatch::Matched(PickerAction::SelectPrev) => app.move_file_search_selection(-1),
        Dispatch::Pending => {}
        Dispatch::Unmatched => {
            if let Some(c) = printable_char(key) {
                app.push_file_search_char(c);
            }
        }
    }
}

fn handle_comment_picker_key(app: &mut App, key: KeyEvent) {
    match app.keybindings.comment_picker(key) {
        Dispatch::Matched(PickerAction::Cancel) => app.stop_comment_picker(),
        Dispatch::Matched(PickerAction::Accept) => app.apply_comment_picker_selection(),
        Dispatch::Matched(PickerAction::Backspace) => {
            if app.comment_picker_query().is_empty() {
                app.stop_comment_picker();
            } else {
                app.pop_comment_picker_char();
            }
        }
        Dispatch::Matched(PickerAction::Clear) => app.clear_comment_picker_text(),
        Dispatch::Matched(PickerAction::SelectNext) => app.move_comment_picker_selection(1),
        Dispatch::Matched(PickerAction::SelectPrev) => app.move_comment_picker_selection(-1),
        Dispatch::Pending => {}
        Dispatch::Unmatched => {
            if let Some(c) = printable_char(key) {
                app.push_comment_picker_char(c);
            }
        }
    }
}

fn handle_file_filter_key(app: &mut App, key: KeyEvent) {
    match app.keybindings.file_filter(key) {
        Dispatch::Matched(FileFilterAction::Close) => app.stop_file_filter(),
        Dispatch::Matched(FileFilterAction::Backspace) => app.pop_file_filter_char(),
        Dispatch::Matched(FileFilterAction::Clear) => app.clear_file_filter(),
        Dispatch::Pending => {}
        Dispatch::Unmatched => {
            if let Some(c) = printable_char(key) {
                app.push_file_filter_char(c);
            }
        }
    }
}

fn handle_session_rename_key(app: &mut App, key: KeyEvent) {
    match app.keybindings.goto(key) {
        Dispatch::Matched(LineInputAction::Cancel) => app.cancel_session_rename(),
        Dispatch::Matched(LineInputAction::Accept) => app.submit_session_rename(),
        Dispatch::Matched(LineInputAction::Backspace) => {
            if !app.session_rename_query().is_empty() {
                app.pop_session_rename_char();
            }
        }
        Dispatch::Matched(LineInputAction::Clear) => app.clear_session_rename_text(),
        Dispatch::Pending => {}
        Dispatch::Unmatched => {
            if let Some(c) = printable_char(key) {
                app.push_session_rename_char(c);
            }
        }
    }
}

fn handle_goto_key(app: &mut App, key: KeyEvent) {
    match app.keybindings.goto(key) {
        Dispatch::Matched(LineInputAction::Cancel) => app.clear_goto(),
        Dispatch::Matched(LineInputAction::Accept) => {
            app.apply_goto();
            app.clear_goto();
        }
        Dispatch::Matched(LineInputAction::Backspace) => {
            if app.goto_query().is_empty() {
                app.clear_goto();
            } else {
                app.pop_goto_char();
            }
        }
        Dispatch::Matched(LineInputAction::Clear) => app.clear_goto_text(),
        Dispatch::Pending => {}
        Dispatch::Unmatched => {
            if let Some(c) = printable_char(key) {
                app.push_goto_char(c);
            }
        }
    }
}

fn handle_search_key(app: &mut App, key: KeyEvent) {
    match app.keybindings.search(key) {
        Dispatch::Matched(LineInputAction::Cancel) => app.clear_search(),
        Dispatch::Matched(LineInputAction::Accept) => {
            app.stop_search();
            app.search_next();
        }
        Dispatch::Matched(LineInputAction::Backspace) => {
            if !app.search_query().is_empty() {
                app.pop_search_char();
            }
        }
        Dispatch::Matched(LineInputAction::Clear) => app.clear_search_text(),
        Dispatch::Pending => {}
        Dispatch::Unmatched => {
            if let Some(c) = printable_char(key) {
                app.push_search_char(c);
            }
        }
    }
}

fn count_digit(key: KeyEvent, pending_count: bool) -> Option<u8> {
    if !key.modifiers.is_empty() {
        return None;
    }
    let KeyCode::Char(c @ '0'..='9') = key.code else {
        return None;
    };
    if c != '0' || pending_count {
        Some(c as u8 - b'0')
    } else {
        None
    }
}

fn repeat_count(
    app: &mut App,
    key: KeyEvent,
    pending_event: &mut Option<Event>,
    coalesce: bool,
) -> Result<usize> {
    if app.pending_count.is_some() {
        Ok(app.take_count())
    } else if coalesce {
        Ok(coalesce_key_repeats(key, pending_event)?)
    } else {
        Ok(app.take_count())
    }
}

fn handle_fold_context_action_key(app: &mut App, key: KeyEvent) -> bool {
    if !key.modifiers.is_empty() {
        return false;
    }
    if let Some(direction) = app.fold_context_prefix.take() {
        match key.code {
            KeyCode::Char(c @ 'a'..='z') => {
                app.expand_visible_context_fold_letter(c, direction);
                return true;
            }
            KeyCode::Char(c @ '1'..='9') => {
                app.expand_visible_context_fold_number((c as u8 - b'0') as usize, direction);
                return true;
            }
            _ => {}
        }
    }
    let direction = match key.code {
        KeyCode::Char('u') => Some(FoldContextDirection::Top),
        KeyCode::Char('d') => Some(FoldContextDirection::Bottom),
        _ => None,
    };
    if let Some(direction) = direction.filter(|_| app.has_visible_context_folds()) {
        app.fold_context_prefix = Some(direction);
        app.keybindings.clear_sequence();
        app.reset_count();
        return true;
    }
    false
}

fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
    pending_event: &mut Option<Event>,
    terminal: &mut TuiTerminal,
    editor_config: &config::EditorConfig,
) -> Result<()> {
    if handle_fold_context_action_key(app, key) {
        return Ok(());
    }
    if app.review_mode() && key.modifiers.is_empty() {
        let sequence_pending = app.keybindings.normal_sequence_pending();
        if app.active_pr_comments_view() && app.pr_reply_prefix {
            app.pr_reply_prefix = false;
            match key.code {
                KeyCode::Char(c @ 'a'..='z') => {
                    app.reply_to_pull_request_comment_letter(c);
                    return Ok(());
                }
                KeyCode::Char(c @ '1'..='9') => {
                    app.reply_to_pull_request_comment_number((c as u8 - b'0') as usize);
                    return Ok(());
                }
                _ => {}
            }
        }
        if app.review_edit_prefix {
            app.review_edit_prefix = false;
            match key.code {
                KeyCode::Char(c @ 'a'..='z') => {
                    app.edit_review_comment_letter(c);
                    return Ok(());
                }
                KeyCode::Char(c @ '1'..='9') => {
                    app.edit_review_comment_number((c as u8 - b'0') as usize);
                    return Ok(());
                }
                _ => {}
            }
        }
        if app.review_reply_prefix {
            app.review_reply_prefix = false;
            match key.code {
                KeyCode::Char(c @ 'a'..='z') => {
                    app.reply_to_review_comment_letter(c);
                    return Ok(());
                }
                KeyCode::Char(c @ '1'..='9') => {
                    app.reply_to_review_comment_number((c as u8 - b'0') as usize);
                    return Ok(());
                }
                _ => {}
            }
        }
        if app.review_resolve_prefix {
            app.review_resolve_prefix = false;
            match key.code {
                KeyCode::Char(c @ 'a'..='z') => {
                    app.resolve_review_comment_letter(c);
                    return Ok(());
                }
                KeyCode::Char(c @ '1'..='9') => {
                    app.resolve_review_comment_number((c as u8 - b'0') as usize);
                    return Ok(());
                }
                _ => {}
            }
        }
        if app.review_delete_prefix {
            app.review_delete_prefix = false;
            match key.code {
                KeyCode::Char(c @ 'a'..='z') => {
                    app.delete_review_comment_letter(c);
                    return Ok(());
                }
                KeyCode::Char(c @ '1'..='9') => {
                    app.delete_review_comment_number((c as u8 - b'0') as usize);
                    return Ok(());
                }
                _ => {}
            }
        }
        if app.review_overflow_prefix {
            app.review_overflow_prefix = false;
            match key.code {
                KeyCode::Char(c @ 'a'..='z') => {
                    app.open_review_comment_context_menu_letter(c);
                    return Ok(());
                }
                KeyCode::Char(c @ '1'..='9') => {
                    app.open_review_comment_context_menu_number((c as u8 - b'0') as usize);
                    return Ok(());
                }
                _ => {}
            }
        }
        if app.active_pr_comments_view() && !sequence_pending {
            match key.code {
                KeyCode::Char('c') => {
                    app.start_pull_request_comment();
                    return Ok(());
                }
                KeyCode::Char('r') => {
                    app.pr_reply_prefix = true;
                    app.review_edit_prefix = false;
                    app.review_reply_prefix = false;
                    app.review_resolve_prefix = false;
                    app.review_delete_prefix = false;
                    app.review_overflow_prefix = false;
                    app.keybindings.clear_sequence();
                    app.reset_count();
                    return Ok(());
                }
                _ => {}
            }
        }
        if !sequence_pending && matches!(key.code, KeyCode::Char('i')) {
            app.review_edit_prefix = true;
            app.review_reply_prefix = false;
            app.review_resolve_prefix = false;
            app.review_delete_prefix = false;
            app.review_overflow_prefix = false;
            app.pr_reply_prefix = false;
            app.keybindings.clear_sequence();
            app.reset_count();
            return Ok(());
        }
        if !sequence_pending
            && matches!(key.code, KeyCode::Char('r'))
            && app.inline_review_reply_available()
        {
            app.review_reply_prefix = true;
            app.review_edit_prefix = false;
            app.review_resolve_prefix = false;
            app.review_delete_prefix = false;
            app.review_overflow_prefix = false;
            app.pr_reply_prefix = false;
            app.keybindings.clear_sequence();
            app.reset_count();
            return Ok(());
        }
        if !sequence_pending
            && matches!(key.code, KeyCode::Char('v'))
            && app.inline_review_actions_available()
        {
            app.review_resolve_prefix = true;
            app.review_edit_prefix = false;
            app.review_reply_prefix = false;
            app.review_delete_prefix = false;
            app.review_overflow_prefix = false;
            app.pr_reply_prefix = false;
            app.keybindings.clear_sequence();
            app.reset_count();
            return Ok(());
        }
        if !sequence_pending
            && matches!(key.code, KeyCode::Char('x'))
            && app.inline_review_actions_available()
        {
            app.review_delete_prefix = true;
            app.review_edit_prefix = false;
            app.review_reply_prefix = false;
            app.review_resolve_prefix = false;
            app.review_overflow_prefix = false;
            app.pr_reply_prefix = false;
            app.keybindings.clear_sequence();
            app.reset_count();
            return Ok(());
        }
        if !sequence_pending
            && matches!(key.code, KeyCode::Char('o'))
            && app.inline_review_actions_available()
        {
            app.review_overflow_prefix = true;
            app.review_edit_prefix = false;
            app.review_reply_prefix = false;
            app.review_resolve_prefix = false;
            app.review_delete_prefix = false;
            app.pr_reply_prefix = false;
            app.keybindings.clear_sequence();
            app.reset_count();
            return Ok(());
        }
    }

    if let Some(digit) = count_digit(key, app.pending_count.is_some()) {
        app.clear_diff_selection();
        app.keybindings.clear_sequence();
        app.push_count_digit(digit);
        return Ok(());
    }

    match app.keybindings.normal(key) {
        Dispatch::Matched(action) => {
            dispatch_normal_action(app, action, key, pending_event, terminal, editor_config)?;
        }
        Dispatch::Pending => {}
        Dispatch::Unmatched => app.reset_count(),
    }
    Ok(())
}

fn dispatch_normal_action(
    app: &mut App,
    action: NormalAction,
    key: KeyEvent,
    pending_event: &mut Option<Event>,
    terminal: &mut TuiTerminal,
    editor_config: &config::EditorConfig,
) -> Result<()> {
    if app.multi_diff.file_count() == 0
        && app.active_topbar_content() != Some(TopbarTabContent::Help)
        && !matches!(
            action,
            NormalAction::Quit
                | NormalAction::Refresh
                | NormalAction::OpenDashboard
                | NormalAction::OpenCommandPalette
                | NormalAction::ToggleHelp
                | NormalAction::OpenCommentPicker
                | NormalAction::OpenOutdatedComments
                | NormalAction::OpenSettings
                | NormalAction::OpenThemePicker
                | NormalAction::NavigateBack
                | NormalAction::NavigateForward
        )
    {
        app.reset_count();
        return Ok(());
    }

    if !matches!(
        action,
        NormalAction::YankChange
            | NormalAction::StartSelection
            | NormalAction::StartLineSelection
            | NormalAction::StartBlockSelection
    ) {
        app.clear_diff_selection();
    }

    match action {
        NormalAction::Quit => {
            app.reset_count();
            if app.show_path_popup {
                app.show_path_popup = false;
            } else {
                app.request_quit();
            }
        }
        NormalAction::StepDown => {
            let count = repeat_count(app, key, pending_event, true)?;
            if !app.file_list_focused
                && (app.csv_preview_move_down(count) || app.structured_preview_move_down(count))
            {
                return Ok(());
            }
            for _ in 0..count {
                if app.file_list_focused {
                    app.next_file();
                } else if app.stepping {
                    app.next_step();
                } else {
                    app.scroll_down();
                }
            }
        }
        NormalAction::StepUp => {
            let count = repeat_count(app, key, pending_event, true)?;
            if !app.file_list_focused
                && (app.csv_preview_move_up(count) || app.structured_preview_move_up(count))
            {
                return Ok(());
            }
            for _ in 0..count {
                if app.file_list_focused {
                    app.prev_file();
                } else if app.stepping {
                    app.prev_step();
                } else {
                    app.scroll_up();
                }
            }
        }
        NormalAction::NextHunk => {
            let count = repeat_count(app, key, pending_event, true)?;
            if !app.file_list_focused {
                let mut handled = false;
                handled |= app.csv_preview_move_right(count);
                for _ in 0..count {
                    handled |= app.structured_preview_move_right();
                }
                if handled {
                    return Ok(());
                }
            }
            app.defer_view_build_for_jump();
            for _ in 0..count {
                if app.stepping {
                    app.next_hunk();
                } else {
                    app.next_hunk_scroll();
                }
            }
        }
        NormalAction::PrevHunk => {
            let count = repeat_count(app, key, pending_event, true)?;
            if !app.file_list_focused {
                let mut handled = false;
                handled |= app.csv_preview_move_left(count);
                for _ in 0..count {
                    handled |= app.structured_preview_move_left();
                }
                if handled {
                    return Ok(());
                }
            }
            app.defer_view_build_for_jump();
            for _ in 0..count {
                if app.stepping {
                    app.prev_hunk();
                } else {
                    app.prev_hunk_scroll();
                }
            }
        }
        NormalAction::HunkStart => {
            app.reset_count();
            app.defer_view_build_for_jump();
            if app.stepping {
                app.goto_hunk_start();
            } else {
                app.goto_hunk_start_scroll();
            }
        }
        NormalAction::HunkEnd => {
            app.reset_count();
            if app.structured_preview_expand_node_and_siblings(false) {
                return Ok(());
            }
            app.defer_view_build_for_jump();
            if app.stepping {
                app.goto_hunk_end();
            } else {
                app.goto_hunk_end_scroll();
            }
        }
        NormalAction::BlameHint => {
            app.reset_count();
            if app.blame_enabled {
                app.trigger_blame_hint();
            }
        }
        NormalAction::TogglePeekChange => {
            app.reset_count();
            if app.stepping {
                app.toggle_peek_old_change();
            }
        }
        NormalAction::TogglePeekHunk => {
            app.reset_count();
            if app.stepping {
                app.toggle_peek_old_hunk();
            }
        }
        NormalAction::YankChange => {
            app.reset_count();
            if app.diff_selection_active() {
                app.copy_diff_selection();
                app.clear_diff_selection();
            } else {
                app.yank_current_change();
            }
        }
        NormalAction::YankHunk => {
            app.reset_count();
            app.yank_current_hunk();
        }
        NormalAction::YankChangePatch => {
            app.reset_count();
            app.yank_current_change_patch();
        }
        NormalAction::YankHunkPatch => {
            app.reset_count();
            app.yank_current_hunk_patch();
        }
        NormalAction::StartSelection => {
            app.reset_count();
            app.start_keyboard_selection();
        }
        NormalAction::StartLineSelection => {
            app.reset_count();
            app.start_keyboard_line_selection();
        }
        NormalAction::StartBlockSelection => {
            app.reset_count();
            app.start_keyboard_block_selection();
        }
        NormalAction::TogglePathPopup => {
            app.reset_count();
            app.toggle_path_popup();
        }
        NormalAction::OpenEditor => {
            app.reset_count();
            open_current_file_in_editor(terminal, app, editor_config)?;
        }
        NormalAction::GotoStart => {
            app.reset_count();
            if app.csv_preview_focus_top() || app.structured_preview_focus_top() {
                return Ok(());
            }
            app.defer_view_build_for_jump();
            app.goto_start();
        }
        NormalAction::GotoEnd => {
            app.reset_count();
            if app.csv_preview_focus_bottom() || app.structured_preview_focus_bottom() {
                return Ok(());
            }
            app.defer_view_build_for_jump();
            app.goto_end();
        }
        NormalAction::FirstStep => {
            app.reset_count();
            app.defer_view_build_for_jump();
            if app.stepping {
                app.goto_first_step();
            } else {
                app.goto_first_hunk_scroll();
            }
        }
        NormalAction::LastStep => {
            app.reset_count();
            app.defer_view_build_for_jump();
            if app.stepping {
                app.goto_last_step();
            } else {
                app.goto_last_hunk_scroll();
            }
        }
        NormalAction::PrevFile => {
            let count = repeat_count(app, key, pending_event, false)?;
            for _ in 0..count {
                app.prev_file_wrapped();
            }
        }
        NormalAction::NextFile => {
            let count = repeat_count(app, key, pending_event, false)?;
            for _ in 0..count {
                app.next_file_wrapped();
            }
        }
        NormalAction::ToggleAutoplay => {
            app.reset_count();
            if app.structured_preview_toggle_collapsed() {
                return Ok(());
            }
            if app.stepping {
                app.toggle_autoplay();
            }
        }
        NormalAction::ToggleAutoplayReverse => {
            app.reset_count();
            if app.stepping {
                app.toggle_autoplay_reverse();
            }
        }
        NormalAction::ToggleViewMode => {
            app.reset_count();
            app.toggle_view_mode();
        }
        NormalAction::ToggleViewModeReverse => {
            app.reset_count();
            app.toggle_view_mode_reverse();
        }
        NormalAction::OpenDashboard => {
            app.reset_count();
            app.open_dashboard = true;
        }
        NormalAction::NavigateBack => {
            app.reset_count();
            app.navigate_view_back();
        }
        NormalAction::NavigateForward => {
            app.reset_count();
            app.navigate_view_forward();
        }
        NormalAction::ScrollUp => {
            let count = repeat_count(app, key, pending_event, false)?;
            for _ in 0..count {
                app.scroll_up();
            }
        }
        NormalAction::ScrollDown => {
            let count = repeat_count(app, key, pending_event, false)?;
            for _ in 0..count {
                app.scroll_down();
            }
        }
        NormalAction::HalfPageUp => {
            app.reset_count();
            if app.structured_preview_jump_up(None) {
                return Ok(());
            }
            if let Ok((_, rows)) = terminal::size() {
                app.scroll_half_page_up(rows.saturating_sub(6) as usize);
            }
        }
        NormalAction::HalfPageDown => {
            app.reset_count();
            if app.structured_preview_jump_down(None) {
                return Ok(());
            }
            if let Ok((_, rows)) = terminal::size() {
                app.scroll_half_page_down(rows.saturating_sub(6) as usize);
            }
        }
        NormalAction::ToggleFileListFocus => {
            app.reset_count();
            if app.can_show_file_panel() {
                app.file_list_focused = !app.file_list_focused;
                if !app.file_list_focused {
                    app.stop_file_filter();
                }
            }
        }
        NormalAction::IncreaseSpeed => {
            app.reset_count();
            if app.can_show_file_panel() && app.file_list_focused {
                if let Ok((cols, _)) = terminal::size() {
                    app.resize_file_panel(2, cols);
                }
            } else {
                app.increase_speed();
            }
        }
        NormalAction::DecreaseSpeed => {
            app.reset_count();
            if app.can_show_file_panel() && app.file_list_focused {
                if let Ok((cols, _)) = terminal::size() {
                    app.resize_file_panel(-2, cols);
                }
            } else {
                app.decrease_speed();
            }
        }
        NormalAction::ToggleAnimation => {
            app.reset_count();
            app.toggle_animation();
        }
        NormalAction::ToggleLineWrap => {
            app.reset_count();
            app.toggle_line_wrap();
        }
        NormalAction::ToggleSyntax => {
            app.reset_count();
            app.toggle_syntax();
        }
        NormalAction::ToggleEvoSyntax => {
            app.reset_count();
            if app.structured_preview_expand_node_and_siblings(true) {
                return Ok(());
            }
            if app.view_mode == ViewMode::Evolution {
                app.toggle_evo_syntax();
            }
        }
        NormalAction::ToggleStepping => {
            app.reset_count();
            app.toggle_stepping();
        }
        NormalAction::ToggleStrikethrough => {
            app.reset_count();
            app.toggle_strikethrough_deletions();
        }
        NormalAction::ScrollLeft => {
            let count = repeat_count(app, key, pending_event, false)?;
            for _ in 0..count {
                app.scroll_left();
            }
        }
        NormalAction::ScrollRight => {
            let count = repeat_count(app, key, pending_event, false)?;
            for _ in 0..count {
                app.scroll_right();
            }
        }
        NormalAction::LineStart => {
            app.reset_count();
            app.scroll_to_line_start();
        }
        NormalAction::LineEnd => {
            app.reset_count();
            app.scroll_to_line_end();
        }
        NormalAction::CenterActive => {
            app.reset_count();
            if let Ok((_, rows)) = terminal::size() {
                app.center_on_active(rows.saturating_sub(4) as usize);
            }
        }
        NormalAction::ToggleZen => {
            app.reset_count();
            app.toggle_zen();
        }
        NormalAction::ReplayStep => app.replay_step(),
        NormalAction::Refresh => {
            app.reset_count();
            if app.multi_diff.is_git_mode() {
                app.refresh_all_files();
            } else {
                app.refresh_current_file();
            }
        }
        NormalAction::ToggleFilePanel => {
            app.reset_count();
            if app.can_show_file_panel() {
                app.toggle_file_panel();
            }
        }
        NormalAction::ToggleFoldContext => {
            app.reset_count();
            app.toggle_fold_context();
        }
        NormalAction::ExpandAllFolds => {
            app.reset_count();
            app.expand_all_context_folds();
        }
        NormalAction::OpenSearchOrFileFilter => {
            app.reset_count();
            if app.file_list_focused {
                app.start_file_filter();
            } else {
                app.start_search();
            }
        }
        NormalAction::OpenGoto => {
            app.reset_count();
            if !app.file_list_focused {
                app.start_goto();
            }
        }
        NormalAction::SearchNext => {
            app.reset_count();
            app.search_next();
        }
        NormalAction::SearchPrev => {
            app.reset_count();
            app.search_prev();
        }
        NormalAction::FocusNextComment => {
            let count = repeat_count(app, key, pending_event, false)?;
            for _ in 0..count {
                app.focus_next_review_comment();
            }
        }
        NormalAction::FocusPrevComment => {
            let count = repeat_count(app, key, pending_event, false)?;
            for _ in 0..count {
                app.focus_prev_review_comment();
            }
        }
        NormalAction::NextConflict => {
            app.reset_count();
            if app.structured_preview_collapse_node_and_siblings(false) {
                return Ok(());
            }
            app.next_conflict();
        }
        NormalAction::PrevConflict => {
            app.reset_count();
            if app.structured_preview_collapse_node_and_siblings(true) {
                return Ok(());
            }
            app.prev_conflict();
        }
        NormalAction::LineComment => {
            app.reset_count();
            if app.structured_preview_toggle_mode() {
                return Ok(());
            }
            app.start_line_comment();
        }
        NormalAction::HunkComment => {
            app.reset_count();
            app.start_hunk_comment();
        }
        NormalAction::ClearComments => {
            app.reset_count();
            app.clear_all_review_comments();
        }
        NormalAction::RemoveLineComment => {
            app.reset_count();
            if !app.remove_hovered_review_comment() {
                app.remove_line_comment_at_cursor();
            }
        }
        NormalAction::RemoveHunkComment => {
            app.reset_count();
            app.remove_hunk_comment_at_cursor();
        }
        NormalAction::ToggleHelp => {
            app.reset_count();
            app.open_help_tab();
        }
        NormalAction::OpenCommandPalette => {
            app.reset_count();
            if app.command_palette_active() {
                app.stop_command_palette();
            } else {
                app.start_command_palette();
            }
        }
        NormalAction::OpenFileSearch => {
            app.reset_count();
            if app.file_search_active() {
                app.stop_file_search();
            } else {
                app.start_file_search();
            }
        }
        NormalAction::OpenCommentPicker => {
            app.reset_count();
            if app.comment_picker_active() {
                app.stop_comment_picker();
            } else {
                app.start_comment_picker();
            }
        }
        NormalAction::OpenOutdatedComments => {
            app.reset_count();
            app.open_outdated_comments_in_current_tab(None);
        }
        NormalAction::OpenSettings => {
            app.reset_count();
            app.open_settings_tab();
        }
        NormalAction::OpenThemePicker => {
            app.reset_count();
            if app.theme_picker_active() {
                app.stop_theme_picker();
            } else {
                app.start_theme_picker();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        FileContextMenuAction, ReviewCommentContextMenuAction, SettingItem, SettingsTarget,
    };
    use crate::{ReviewRange, ReviewSide, ReviewTargetKind};
    use oyo_core::MultiFileDiff;

    fn key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::empty())
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    fn test_terminal() -> TuiTerminal {
        let backend = ratatui::backend::CrosstermBackend::new(
            Box::new(Vec::<u8>::new()) as Box<dyn std::io::Write>
        );
        ratatui::Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 80, 24)),
            },
        )
        .unwrap()
    }

    #[test]
    fn control_c_is_the_force_quit_key_outside_the_review_editor() {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        assert!(should_force_quit(&app, ctrl('c')));
        assert!(!should_force_quit(&app, key('c')));
        assert!(!should_force_quit(
            &app,
            KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            )
        ));

        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.start_line_comment();
        assert!(app.review_editor_active());
        assert!(!should_force_quit(&app, ctrl('c')));
    }

    #[test]
    fn global_palette_binding_opens_from_search_mode() {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.start_search();

        assert!(handle_global_key(&mut app, ctrl('p')));
        assert!(app.command_palette_active());
        assert!(!app.search_active());
    }

    #[test]
    fn empty_file_tab_keeps_palette_and_history_available() {
        let diff = MultiFileDiff::from_raw_files(None, Vec::new());
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        assert!(handle_global_key(&mut app, ctrl('p')));
        assert!(app.command_palette_active());
        app.stop_command_palette();

        let mut terminal = test_terminal();
        let mut pending_event = None;
        handle_app_key(
            &mut app,
            ctrl('r'),
            &mut pending_event,
            &mut terminal,
            &config::EditorConfig::default(),
        )
        .unwrap();
        assert!(app.open_dashboard);
    }

    #[test]
    fn bracket_file_navigation_wraps_but_focused_list_navigation_clamps() {
        let diff = MultiFileDiff::from_file_pairs(
            ["a.txt", "b.txt", "c.txt"]
                .into_iter()
                .map(|path| (path.into(), "old\n".to_string(), "new\n".to_string()))
                .collect(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        let mut terminal = test_terminal();
        let mut pending_event = None;
        let editor_config = config::EditorConfig::default();

        app.select_file(2);
        handle_app_key(
            &mut app,
            key(']'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert_eq!(app.current_file_path(), "a.txt");
        handle_app_key(
            &mut app,
            key('['),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert_eq!(app.current_file_path(), "c.txt");

        app.select_file(0);
        handle_app_key(
            &mut app,
            key('2'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        handle_app_key(
            &mut app,
            key(']'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert_eq!(app.current_file_path(), "c.txt");

        app.next_file();
        assert_eq!(app.current_file_path(), "c.txt");
        app.select_file(0);
        app.prev_file();
        assert_eq!(app.current_file_path(), "a.txt");
    }

    #[test]
    fn context_menus_support_wrapped_keyboard_navigation_and_activation() {
        let diff = MultiFileDiff::from_file_pairs(vec![
            ("a.txt".into(), "old\n".to_string(), "new\n".to_string()),
            ("b.txt".into(), "old\n".to_string(), "new\n".to_string()),
        ]);
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.goto_last_step();
        app.add_review_comment_from_cli(
            "a.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 1, end: 1 }),
            "copy me".to_string(),
        )
        .unwrap();
        assert!(app.open_review_comment_context_menu_letter('a'));
        assert_eq!(
            app.review_comment_context_menu_hover,
            Some(ReviewCommentContextMenuAction::Body)
        );

        let mut terminal = test_terminal();
        let mut pending_event = None;
        let editor_config = config::EditorConfig::default();
        let mut press = |app: &mut App, code| {
            handle_app_key(
                app,
                KeyEvent::new(code, KeyModifiers::empty()),
                &mut pending_event,
                &mut terminal,
                &editor_config,
            )
            .unwrap();
        };

        press(&mut app, KeyCode::Up);
        assert_eq!(
            app.review_comment_context_menu_hover,
            Some(ReviewCommentContextMenuAction::MarkdownQuote)
        );
        press(&mut app, KeyCode::Down);
        assert_eq!(
            app.review_comment_context_menu_hover,
            Some(ReviewCommentContextMenuAction::Body)
        );
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(
            app.review_comment_context_menu_hover,
            Some(ReviewCommentContextMenuAction::Id)
        );
        press(&mut app, KeyCode::Enter);
        assert!(app.review_comment_context_menu.is_none());

        assert!(app.open_review_comment_context_menu_letter('a'));
        press(&mut app, KeyCode::Esc);
        assert!(app.review_comment_context_menu.is_none());

        app.file_list_area = Some((0, 0, 20, 4));
        app.file_list_rows = vec![Some(1)];
        assert!(app.open_file_context_menu(1, 1));
        assert_eq!(
            app.file_context_menu_hover,
            Some(FileContextMenuAction::Open)
        );
        press(&mut app, KeyCode::Up);
        assert_eq!(
            app.file_context_menu_hover,
            Some(FileContextMenuAction::CopyPath)
        );
        press(&mut app, KeyCode::Down);
        assert_eq!(
            app.file_context_menu_hover,
            Some(FileContextMenuAction::Open)
        );
        press(&mut app, KeyCode::Enter);
        assert!(app.file_context_menu.is_none());
        assert_eq!(app.current_file_path(), "b.txt");

        app.select_file(0);
        assert!(app.open_file_context_menu(1, 1));
        press(&mut app, KeyCode::Esc);
        assert!(app.file_context_menu.is_none());
        assert_eq!(app.current_file_path(), "a.txt");
    }

    #[test]
    fn settings_key_sequence_opens_and_reuses_settings_tab() {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        let mut terminal = test_terminal();
        let mut pending_event = None;
        let editor_config = config::EditorConfig::default();
        for ch in ['g', 's', 'g', 's'] {
            handle_app_key(
                &mut app,
                key(ch),
                &mut pending_event,
                &mut terminal,
                &editor_config,
            )
            .unwrap();
        }
        assert_eq!(
            app.active_topbar_content(),
            Some(TopbarTabContent::Settings)
        );
        assert_eq!(
            app.topbar_tabs
                .iter()
                .filter(|tab| tab.content == TopbarTabContent::Settings)
                .count(),
            1
        );
        handle_app_key(
            &mut app,
            key('k'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert_eq!(
            app.settings_selected_target(),
            SettingsTarget::ResetDefaults
        );
        handle_app_key(
            &mut app,
            key('j'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert_eq!(
            app.settings_selected_target(),
            SettingsTarget::Item(SettingItem::ViewMode)
        );
        handle_app_key(
            &mut app,
            key('h'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert_eq!(app.view_mode, ViewMode::Evolution);
        handle_app_key(
            &mut app,
            key('l'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert_eq!(app.view_mode, ViewMode::UnifiedPane);
    }

    #[test]
    fn review_prefixes_do_not_swallow_pending_normal_sequences() {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.start_line_comment();
        app.review_insert_char('x');
        app.review_save_editor();
        assert_eq!(app.review_comment_count(), 1);
        let mut terminal = test_terminal();
        let mut pending_event = None;
        let editor_config = config::EditorConfig::default();

        handle_app_key(
            &mut app,
            key('g'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert!(app.keybindings.normal_sequence_pending());
        handle_app_key(
            &mut app,
            key('o'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();

        assert!(app.active_outdated_comments_view());
        assert!(!app.review_overflow_prefix);

        handle_app_key(
            &mut app,
            key('g'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        handle_app_key(
            &mut app,
            key('f'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert!(app.file_search_active());
    }

    #[test]
    fn inline_review_uses_r_for_reply_and_v_for_resolve() {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.goto_last_step();
        app.start_line_comment();
        app.review_insert_char('x');
        app.review_save_editor();
        let mut terminal = test_terminal();
        let mut pending_event = None;
        let editor_config = config::EditorConfig::default();

        handle_app_key(
            &mut app,
            key('r'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert!(app.review_reply_prefix);
        handle_app_key(
            &mut app,
            key('a'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert!(app.review_editor_active());
        app.review_cancel_editor();

        handle_app_key(
            &mut app,
            key('v'),
            &mut pending_event,
            &mut terminal,
            &editor_config,
        )
        .unwrap();
        assert!(app.review_resolve_prefix);
    }

    #[test]
    fn escape_closes_path_popup_without_quitting() {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.show_path_popup = true;
        let mut terminal = test_terminal();
        let mut pending_event = None;

        handle_app_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
            &mut pending_event,
            &mut terminal,
            &config::EditorConfig::default(),
        )
        .unwrap();

        assert!(!app.show_path_popup);
        assert!(!app.should_quit);
    }

    #[test]
    fn escape_closes_search_after_accept_without_quitting() {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "needle\n".to_string(),
            "needle\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.start_search();
        for ch in "needle".chars() {
            app.push_search_char(ch);
        }
        handle_search_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );
        assert!(!app.search_active());
        assert!(app.search_bar_visible());
        assert!(app.search_target().is_some());
        let mut terminal = test_terminal();
        let mut pending_event = None;

        handle_app_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
            &mut pending_event,
            &mut terminal,
            &config::EditorConfig::default(),
        )
        .unwrap();

        assert!(!app.search_bar_visible());
        assert_eq!(app.search_target(), None);
        assert!(!app.quit_confirmation_active());
        assert!(!app.should_quit);
    }

    #[test]
    fn backspace_keeps_empty_search_open() {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.start_search();

        handle_search_key(
            &mut app,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()),
        );

        assert!(app.search_active());
        assert!(app.search_query().is_empty());
    }

    #[test]
    fn count_digits_require_plain_digit_keys() {
        assert_eq!(count_digit(key('1'), false), Some(1));
        assert_eq!(count_digit(key('0'), false), None);
        assert_eq!(count_digit(key('0'), true), Some(0));
        assert_eq!(count_digit(ctrl('1'), false), None);
    }

    #[test]
    fn visible_fold_shortcuts_reveal_from_each_side() {
        let content = (1..=40)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let diff = MultiFileDiff::from_file_pair(
            "fold.txt".into(),
            "fold.txt".into(),
            content.clone(),
            content,
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_fold_context_mode(crate::config::FoldContextMode::Expandable);

        let register_fold = |app: &mut App| {
            let view = app.current_view_with_frame(oyo_core::AnimationFrame::Idle);
            let region = view
                .iter()
                .find_map(|line| app.fold_context_region_for_line(line))
                .unwrap();
            app.fold_context_hits = vec![
                crate::app::FoldContextHit {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                    key: region.key,
                    direction: FoldContextDirection::Top,
                },
                crate::app::FoldContextHit {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 1,
                    key: region.key,
                    direction: FoldContextDirection::Bottom,
                },
            ];
        };

        register_fold(&mut app);
        assert!(handle_fold_context_action_key(&mut app, key('u')));
        assert_eq!(app.fold_context_prefix, Some(FoldContextDirection::Top));
        assert!(handle_fold_context_action_key(&mut app, key('a')));
        assert!(app
            .current_view_with_frame(oyo_core::AnimationFrame::Idle)
            .iter()
            .any(|line| line.content == "↑ 14 unchanged lines ↓"));

        register_fold(&mut app);
        assert!(handle_fold_context_action_key(&mut app, key('d')));
        assert_eq!(app.fold_context_prefix, Some(FoldContextDirection::Bottom));
        assert!(handle_fold_context_action_key(&mut app, key('a')));
        assert!(app
            .current_view_with_frame(oyo_core::AnimationFrame::Idle)
            .iter()
            .all(|line| !crate::app::is_fold_line(line)));
    }
}
