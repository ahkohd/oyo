use crate::app::{App, ViewMode};
use crate::config;
use crate::keybindings::{
    Dispatch, FileFilterAction, HelpAction, LineInputAction, NormalAction, PickerAction,
    ReviewEditorAction,
};
use anyhow::Result;
use crossterm::{
    event::{Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};

use super::{coalesce_key_repeats, open_current_file_in_editor, TuiTerminal};

pub(crate) fn handle_app_key(
    app: &mut App,
    key: KeyEvent,
    pending_event: &mut Option<Event>,
    terminal: &mut TuiTerminal,
    editor_config: &config::EditorConfig,
) -> Result<()> {
    if app.show_help {
        handle_help_key(app, key);
        return Ok(());
    }

    if app.review_editor_active() {
        handle_review_editor_key(app, key);
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

    if app.file_filter_active {
        handle_file_filter_key(app, key);
        return Ok(());
    }

    if app.goto_active() {
        handle_goto_key(app, key);
        return Ok(());
    }

    if app.search_active() {
        handle_search_key(app, key);
        return Ok(());
    }

    handle_normal_key(app, key, pending_event, terminal, editor_config)
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
            if app.search_query().is_empty() {
                app.clear_search();
            } else {
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

fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
    pending_event: &mut Option<Event>,
    terminal: &mut TuiTerminal,
    editor_config: &config::EditorConfig,
) -> Result<()> {
    if matches!(key.code, KeyCode::Char(c @ '0'..='9') if c != '0' || app.pending_count.is_some()) {
        if let KeyCode::Char(c) = key.code {
            app.keybindings.clear_sequence();
            app.push_count_digit(c as u8 - b'0');
        }
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
    match action {
        NormalAction::Quit => {
            app.reset_count();
            if app.show_path_popup {
                app.show_path_popup = false;
            } else {
                app.submit_review_and_quit();
            }
        }
        NormalAction::StepDown => {
            let count = repeat_count(app, key, pending_event, true)?;
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
            app.yank_current_change();
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
            app.defer_view_build_for_jump();
            app.goto_start();
        }
        NormalAction::GotoEnd => {
            app.reset_count();
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
                app.prev_file();
            }
        }
        NormalAction::NextFile => {
            let count = repeat_count(app, key, pending_event, false)?;
            for _ in 0..count {
                app.next_file();
            }
        }
        NormalAction::ToggleAutoplay => {
            app.reset_count();
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
            if let Ok((_, rows)) = terminal::size() {
                app.scroll_half_page_up(rows.saturating_sub(6) as usize);
            }
        }
        NormalAction::HalfPageDown => {
            app.reset_count();
            if let Ok((_, rows)) = terminal::size() {
                app.scroll_half_page_down(rows.saturating_sub(6) as usize);
            }
        }
        NormalAction::ToggleFileListFocus => {
            app.reset_count();
            if app.is_multi_file() {
                app.file_list_focused = !app.file_list_focused;
                if !app.file_list_focused {
                    app.stop_file_filter();
                }
            }
        }
        NormalAction::IncreaseSpeed => {
            app.reset_count();
            if app.is_multi_file() && app.file_list_focused {
                if let Ok((cols, _)) = terminal::size() {
                    app.resize_file_panel(2, cols);
                }
            } else {
                app.increase_speed();
            }
        }
        NormalAction::DecreaseSpeed => {
            app.reset_count();
            if app.is_multi_file() && app.file_list_focused {
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
            if app.is_multi_file() {
                app.toggle_file_panel();
            }
        }
        NormalAction::ToggleFoldContext => {
            app.reset_count();
            app.toggle_fold_context();
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
        NormalAction::NextConflict => {
            app.reset_count();
            app.next_conflict();
        }
        NormalAction::PrevConflict => {
            app.reset_count();
            app.prev_conflict();
        }
        NormalAction::LineComment => {
            app.reset_count();
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
            app.remove_line_comment_at_cursor();
        }
        NormalAction::RemoveHunkComment => {
            app.reset_count();
            app.remove_hunk_comment_at_cursor();
        }
        NormalAction::ToggleHelp => {
            app.reset_count();
            app.toggle_help();
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
    }
    Ok(())
}
