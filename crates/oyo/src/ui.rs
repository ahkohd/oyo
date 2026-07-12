//! UI rendering for the TUI

use crate::app::{
    diff_scrollbar_thumb,
    review::{
        PrCommentHit, PrCommentHitAction, ReviewDeleteConfirmationAction,
        ReviewDeleteConfirmationHit, ReviewRemotePickerHit, ReviewSidebarOverflowHit,
        ReviewSyncAction,
    },
    App, FileContextMenuAction, FileContextMenuHit, FilePanelMode, FilePanelScrollbarState,
    ReviewCommentContextMenuHit, ReviewEditorToolbarAction, ReviewEditorToolbarHit,
    ReviewLineAddHit, SelectionToolbarAction, SelectionToolbarHit, StatusModeMenuHit,
    TopbarTabContent, TopbarTabHit, ViewMode, DIFF_VIEW_MIN_WIDTH, FILE_PANEL_MIN_WIDTH,
};
use crate::color;
use crate::config::FilePanelPosition;
use crate::csv_preview::{CsvPreviewSignature, CsvPreviewState};
use crate::keybindings::{
    BindingAction, DashboardAction, DashboardFilterAction, FileFilterAction, GlobalAction,
    HelpAction, LineInputAction, NormalAction, PickerAction, ReviewEditorAction, SelectionAction,
};
use crate::markdown::{
    markdown_preview_lines as render_markdown_preview_lines,
    MarkdownChangeBars as PreviewChangeBars, PreviewLink,
};
use crate::structured_preview::{
    StructuredPreviewChangeBars, StructuredPreviewKind, StructuredPreviewSignature,
};
use crate::syntax::SyntaxSide;
use crate::views::{
    render_blame, render_diff_scrollbar, render_evolution, render_review_note_avatar, render_split,
    render_unified_pane, reserve_diff_scrollbar_lane, review_note_block_with_footer,
    review_note_delete_width, review_note_delete_x_offset, review_note_edit_width,
    review_note_lines, review_note_resolve_width, review_note_resolve_x_offset, review_preview_row,
    ReviewNoteActionHits,
};
use image::GenericImageView;
use oyo_core::{multi::DiffStatus, multi::FileSide, ChangeKind, FileStatus, LineKind};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState,
    },
    Frame,
};
use ratatui_image::Image as TerminalImage;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

fn take_width_prefix(text: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width) > max_width {
            break;
        }
        width = width.saturating_add(ch_width);
        out.push(ch);
    }
    out
}

fn take_width_suffix(text: &str, max_width: usize) -> String {
    let mut out = Vec::new();
    let mut width = 0usize;
    for ch in text.chars().rev() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width) > max_width {
            break;
        }
        width = width.saturating_add(ch_width);
        out.push(ch);
    }
    out.into_iter().rev().collect()
}

fn truncate_filename_keep_ext(name: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text_width(name) <= max_width {
        return name.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }

    let (stem, ext) = match name.rfind('.') {
        Some(idx) if idx > 0 && idx < name.len().saturating_sub(1) => (&name[..idx], &name[idx..]),
        _ => (name, ""),
    };
    let ext_width = text_width(ext);
    if ext_width.saturating_add(1) >= max_width {
        return format!("…{}", take_width_suffix(name, max_width.saturating_sub(1)));
    }

    if ext.is_empty() {
        let keep = max_width.saturating_sub(1);
        let head_width = keep.div_ceil(2);
        let tail_width = keep.saturating_sub(head_width);
        return format!(
            "{}…{}",
            take_width_prefix(stem, head_width),
            take_width_suffix(stem, tail_width)
        );
    }

    let stem_width = max_width.saturating_sub(ext_width).saturating_sub(1);
    let head_width = stem_width.div_ceil(2);
    let tail_width = stem_width.saturating_sub(head_width);
    format!(
        "{}…{}{}",
        take_width_prefix(stem, head_width),
        take_width_suffix(stem, tail_width),
        ext
    )
}

fn text_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn paired<A>(keys: &impl Fn(A) -> String, first: A, second: A) -> String {
    format!("{} / {}", keys(first), keys(second))
}

fn counted_binding_label(binding: &str) -> String {
    binding
        .split(" / ")
        .map(|key| format!("<count>{key}"))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn spans_width(spans: &[Span]) -> usize {
    spans
        .iter()
        .map(|span| text_width(span.content.as_ref()))
        .sum()
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = text_width(&ch.to_string());
        if width + ch_width > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out
}

fn truncate_with_dots(text: &str, max_width: usize) -> String {
    if text_width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    format!(
        "{}...",
        truncate_to_width(text, max_width.saturating_sub(3))
    )
}

fn wrap_editor_line(line: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }
    if line.is_empty() {
        return vec![String::new()];
    }

    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut width = 0usize;

    for ch in line.chars() {
        let ch_width = ch.width().unwrap_or(1).max(1);
        if width + ch_width > max_width && !current.is_empty() {
            out.push(current);
            current = String::new();
            width = 0;
        }

        current.push(ch);
        width += ch_width;

        if width >= max_width {
            out.push(current);
            current = String::new();
            width = 0;
        }
    }

    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn editor_cursor_visual(line: &str, cursor_col: usize, max_width: usize) -> (usize, usize) {
    if max_width == 0 || line.is_empty() {
        return (0, 0);
    }

    let mut row = 0usize;
    let mut col = 0usize;
    for (idx, ch) in line.chars().enumerate() {
        if idx >= cursor_col {
            break;
        }
        let ch_width = ch.width().unwrap_or(1).max(1);
        if col + ch_width > max_width && col > 0 {
            row += 1;
            col = 0;
        }
        col += ch_width;
        if col == max_width {
            row += 1;
            col = 0;
        }
    }

    (row, col)
}

fn format_ratio(current: usize, total: usize) -> String {
    let width = total.to_string().len();
    let current_padded = format!("{:>width$}", current, width = width);
    format!("{}/{}", current_padded, total)
}

fn diff_spinner_frame() -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let idx = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        / 100)
        % FRAMES.len() as u128;
    FRAMES[idx as usize]
}

fn review_sync_status_label(action: ReviewSyncAction) -> &'static str {
    match action {
        ReviewSyncAction::Sync => "syncing…",
        ReviewSyncAction::Pull => "pulling…",
        ReviewSyncAction::Push => "pushing…",
    }
}

fn review_comment_count_label(count: usize) -> String {
    match count {
        0 => "no comment".to_string(),
        1 => "1 comment".to_string(),
        n => format!("{n} comments"),
    }
}

fn review_status_comment_spans(app: &App, style: Style) -> Option<(Vec<Span<'static>>, usize)> {
    let comment_count = app.review_comment_count();
    if comment_count == 0 && !app.review_editor_active() && app.review_sync_status().is_none() {
        return None;
    }
    let count_label = review_comment_count_label(comment_count);
    let status = app.review_sync_status().map(|action| {
        (
            diff_spinner_frame().to_string(),
            review_sync_status_label(action).to_string(),
        )
    });
    let status_width = status
        .as_ref()
        .map(|(spinner, label)| text_width(spinner) + 1 + text_width(label))
        .unwrap_or(0);
    let width = status_width.max(text_width(&count_label)).max(10);
    let mut spans = if let Some((spinner, label)) = status {
        vec![
            Span::styled(
                spinner,
                Style::default()
                    .fg(app.theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", style),
            Span::styled(label, style),
        ]
    } else {
        vec![Span::styled(count_label, style)]
    };
    let used = spans_width(&spans);
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    Some((spans, width))
}

fn clamp_spans_to_width<'a>(spans: &[Span<'a>], max_width: usize) -> Vec<Span<'a>> {
    let mut out = Vec::new();
    let mut remaining = max_width;
    for span in spans {
        if remaining == 0 {
            break;
        }
        let width = text_width(span.content.as_ref());
        if width <= remaining {
            out.push(span.clone());
            remaining -= width;
        } else {
            let truncated = truncate_to_width(span.content.as_ref(), remaining);
            if !truncated.is_empty() {
                out.push(Span::styled(truncated, span.style));
            }
            break;
        }
    }
    out
}

fn pad_spans_left(spans: Vec<Span>, width: usize) -> Vec<Span> {
    let current = spans_width(&spans);
    if current >= width {
        return spans;
    }
    let mut out = spans;
    out.push(Span::raw(" ".repeat(width - current)));
    out
}

fn pad_spans_center(spans: Vec<Span>, width: usize) -> Vec<Span> {
    let current = spans_width(&spans);
    if current >= width {
        return spans;
    }
    let remaining = width - current;
    let left = remaining / 2;
    let right = remaining - left;
    let mut out = Vec::new();
    if left > 0 {
        out.push(Span::raw(" ".repeat(left)));
    }
    out.extend(spans);
    if right > 0 {
        out.push(Span::raw(" ".repeat(right)));
    }
    out
}

fn pad_spans_right(spans: Vec<Span>, width: usize) -> Vec<Span> {
    let current = spans_width(&spans);
    if current >= width {
        return spans;
    }
    let mut out = Vec::new();
    out.push(Span::raw(" ".repeat(width - current)));
    out.extend(spans);
    out
}

/// Truncate a path to fit a given width, using /…/ for middle sections
fn truncate_path(path: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if path.len() <= max_width {
        return path.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() == 1 {
        return truncate_filename_keep_ext(path, max_width);
    }

    // Keep first and last parts, abbreviate middle
    let first = parts[0];
    let last = parts.last().unwrap_or(&"");

    // If just first + last fits with /…/, use that
    let prefix = format!("{}/…/", first);
    let available = max_width.saturating_sub(prefix.len());
    if available > 0 {
        let last_display = truncate_filename_keep_ext(last, available);
        let simple = format!("{prefix}{last_display}");
        if simple.len() <= max_width {
            return simple;
        }
    }

    // Otherwise just show …/filename
    if max_width <= 4 {
        return ".".repeat(max_width);
    }
    let prefix = "…/";
    let available = max_width.saturating_sub(prefix.len());
    if available == 0 {
        return ".".repeat(max_width);
    }
    let last_display = truncate_filename_keep_ext(last, available);
    format!("{prefix}{last_display}")
}

fn truncate_text(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    if text.len() <= max_width {
        return text.to_string();
    }
    let mut acc = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthStr::width(ch.to_string().as_str());
        if width + ch_width > max_width.saturating_sub(3) {
            break;
        }
        acc.push(ch);
        width += ch_width;
    }
    format!("{acc}…")
}

fn truncate_text_from_start(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut suffix = String::new();
    let mut width = 0usize;
    for ch in text.chars().rev() {
        let ch_width = UnicodeWidthStr::width(ch.to_string().as_str());
        if width + ch_width > max_width.saturating_sub(1) {
            break;
        }
        suffix.insert(0, ch);
        width += ch_width;
    }
    format!("…{suffix}")
}

fn no_changes_message(app: &App) -> &str {
    app.no_changes_message
        .as_deref()
        .unwrap_or("No changes found.")
}

fn no_changes_hint_line(app: &App) -> Line<'static> {
    let text_style = Style::default().fg(app.theme.text_muted);
    let key_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let dashboard_style = if app.no_changes_dashboard_hover {
        Style::default()
            .fg(app.theme.text_muted)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text_muted)
    };
    let quit_style = if app.no_changes_quit_hover {
        Style::default()
            .fg(app.theme.text_muted)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text_muted)
    };
    let mut spans = Vec::new();
    if app.watch {
        spans.push(Span::styled("Watching for changes. ", text_style));
    } else {
        spans.push(Span::styled("R", key_style));
        spans.push(Span::styled(" refresh  ", text_style));
    }
    spans.push(Span::styled("ctrl-r", key_style));
    spans.push(Span::styled(" history", dashboard_style));
    spans.push(Span::styled("  ", text_style));
    spans.push(Span::styled("q", key_style));
    spans.push(Span::styled(" quit", quit_style));
    Line::from(spans)
}

fn no_changes_hint_text(app: &App) -> String {
    if app.watch {
        "Watching for changes. ctrl-r history  q quit".to_string()
    } else {
        "R refresh  ctrl-r history  q quit".to_string()
    }
}

fn no_changes_action_hit(
    app: &App,
    area: Rect,
    hint_y: u16,
    prefix: &str,
    action: &str,
) -> (u16, u16, u16, u16) {
    let full_width = text_width(&no_changes_hint_text(app)) as u16;
    let x = area
        .x
        .saturating_add(area.width.saturating_sub(full_width) / 2)
        .saturating_add(text_width(prefix) as u16);
    (x, hint_y, text_width(action) as u16, 1)
}

fn no_changes_dashboard_hit(app: &App, area: Rect, hint_y: u16) -> (u16, u16, u16, u16) {
    let prefix = if app.watch {
        "Watching for changes. "
    } else {
        "R refresh  "
    };
    no_changes_action_hit(app, area, hint_y, prefix, "ctrl-r history")
}

fn no_changes_quit_hit(app: &App, area: Rect, hint_y: u16) -> (u16, u16, u16, u16) {
    let prefix = if app.watch {
        "Watching for changes. ctrl-r history  "
    } else {
        "R refresh  ctrl-r history  "
    };
    no_changes_action_hit(app, area, hint_y, prefix, "q quit")
}

fn draw_no_changes(frame: &mut Frame, app: &mut App, area: Rect) {
    if let Some(bg) = app.theme.background {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }
    let height = 3.min(area.height);
    let y = area.y + area.height.saturating_sub(height) / 2;
    let hint_y = y.saturating_add(2);
    app.no_changes_dashboard_hit =
        (height > 2).then(|| no_changes_dashboard_hit(app, area, hint_y));
    app.no_changes_quit_hit = (height > 2).then(|| no_changes_quit_hit(app, area, hint_y));

    let mut message = Paragraph::new(Line::from(Span::styled(
        no_changes_message(app),
        Style::default()
            .fg(app.theme.text)
            .add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center);
    let mut hint = Paragraph::new(no_changes_hint_line(app)).alignment(Alignment::Center);
    if let Some(bg) = app.theme.background {
        let style = Style::default().bg(bg);
        message = message.style(style);
        hint = hint.style(style);
    }
    frame.render_widget(message, Rect::new(area.x, y, area.width, 1));
    if height > 2 {
        frame.render_widget(hint, Rect::new(area.x, hint_y, area.width, 1));
    }
}

/// Main drawing function
pub fn draw(frame: &mut Frame, app: &mut App) {
    app.clear_review_preview_boxes();
    app.clear_fold_context_hits();
    app.begin_scrollbar_frame();
    app.topbar_tab_hits.clear();
    app.topbar_plus_hit = None;
    app.topbar_scroll_left_hit = None;
    app.topbar_scroll_right_hit = None;
    app.preview_toggle_hit = None;
    app.topbar_sidebar_toggle_hit = None;
    app.status_mode_hit = None;
    app.binary_preview_hit = None;
    app.review_file_comment_hit = None;
    app.file_panel_root_hit = None;
    app.no_changes_dashboard_hit = None;
    app.no_changes_quit_hit = None;
    app.topbar_area = None;

    if app.multi_diff.file_count() == 0
        && app.active_topbar_content() != Some(TopbarTabContent::Help)
    {
        app.clear_diff_selection();
        app.set_diff_selection_cells(Vec::new());
        draw_no_changes(frame, app, frame.area());
        if app.theme_picker_active() {
            draw_theme_picker_popover(frame, app);
        }
        if app.quit_confirmation_active() {
            draw_confirmation(frame, app, true);
        } else {
            app.set_review_delete_confirmation_hits(Vec::new());
        }
        draw_toasts(frame, app);
        return;
    }

    if app.zen_mode {
        // Zen mode: just the content with minimal progress indicator
        draw_content(frame, app, frame.area(), false);
        draw_zen_progress(frame, app);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if app.topbar {
                vec![
                    Constraint::Length(1), // Top bar
                    Constraint::Min(0),    // Main content
                    Constraint::Length(1), // Status bar
                ]
            } else {
                vec![
                    Constraint::Min(0),    // Main content
                    Constraint::Length(1), // Status bar
                ]
            })
            .split(frame.area());

        if app.topbar {
            draw_content(frame, app, chunks[1], true);
            draw_status_bar(frame, app, chunks[2]);
        } else {
            draw_content(frame, app, chunks[0], false);
            draw_status_bar(frame, app, chunks[1]);
        }
    }

    capture_diff_selection_cells(frame, app);
    draw_diff_selection(frame, app);
    draw_selection_toolbar(frame, app);

    // Draw help popover if active
    if app.show_help {
        draw_help_popover(frame, app);
    }

    // Draw file path popup if active
    if app.show_path_popup {
        draw_path_popup(frame, app);
    }

    if app.command_palette_active() {
        draw_command_palette_popover(frame, app);
    }

    if app.file_search_active() {
        draw_file_search_popover(frame, app);
    }

    if app.comment_picker_active() {
        draw_comment_picker_popover(frame, app);
    }

    if app.theme_picker_active() {
        draw_theme_picker_popover(frame, app);
    }

    if app.review_remote_picker_active() {
        draw_review_remote_picker(frame, app);
    } else {
        app.set_review_remote_picker_hits(Vec::new());
    }

    if app.review_mode() {
        if app.review_editor_active() {
            draw_review_editor_overlay(frame, app);
        } else if app.selection_toolbar_visible() || app.diff_selection_mode_active() {
            app.clear_review_preview_boxes();
        }
    } else {
        app.clear_review_preview_boxes();
    }

    draw_review_line_add_button(frame, app);
    draw_file_context_menu(frame, app);
    draw_review_comment_context_menu(frame, app);
    draw_status_mode_menu(frame, app);
    if app.session_rename_active() {
        draw_session_rename_modal(frame, app);
    }
    if app.quit_confirmation_active() {
        draw_confirmation(frame, app, true);
    } else if app.review_delete_confirmation_active() {
        draw_confirmation(frame, app, false);
    } else {
        app.set_review_delete_confirmation_hits(Vec::new());
    }
    draw_toasts(frame, app);
    if app.search_bar_visible() {
        draw_find_bar(frame, app);
    } else {
        app.set_search_bar_hits(None, None, None, None);
    }
}

fn rects_overlap(left: Rect, right: Rect) -> bool {
    left.x < right.x.saturating_add(right.width)
        && left.x.saturating_add(left.width) > right.x
        && left.y < right.y.saturating_add(right.height)
        && left.y.saturating_add(left.height) > right.y
}

fn active_search_match_rect(frame: &mut Frame, app: &App) -> Option<Rect> {
    let row = app.search_target_screen_row()?;
    let (x, _, width, _) = app.diff_view_area?;
    let mut first = None;
    let mut last = None;
    for column in x..x.saturating_add(width) {
        if frame
            .buffer_mut()
            .cell((column, row))
            .is_some_and(|cell| cell.bg == app.theme.accent)
        {
            first.get_or_insert(column);
            last = Some(column);
        }
    }
    match (first, last) {
        (Some(first), Some(last)) => Some(Rect::new(
            first,
            row,
            last.saturating_sub(first).saturating_add(1),
            1,
        )),
        _ => Some(Rect::new(x, row, width, 1)),
    }
}

fn find_bar_area(app: &App, width: u16, match_rect: Option<Rect>) -> Option<Rect> {
    let (x, y, view_width, view_height) = app.diff_view_area?;
    let scrollbar_width = u16::from(app.scrollbar_visible && view_width > 0);
    let content_width = view_width.saturating_sub(scrollbar_width);
    if content_width < 15 || view_height < 3 {
        return None;
    }
    let width = width.min(content_width);
    let height = 3;
    let right = x.saturating_add(content_width.saturating_sub(width));
    let bottom = y.saturating_add(view_height.saturating_sub(height));
    let candidates = [
        Rect::new(right, y, width, height),
        Rect::new(x, y, width, height),
        Rect::new(right, bottom, width, height),
        Rect::new(x, bottom, width, height),
    ];
    candidates
        .iter()
        .copied()
        .find(|area| match_rect.is_none_or(|match_rect| !rects_overlap(*area, match_rect)))
        .or_else(|| candidates.first().copied())
}

fn find_bar_action_style(app: &App, hovered: bool) -> Style {
    let mut style = Style::default().fg(if hovered {
        app.theme.accent
    } else {
        app.theme.text_muted
    });
    if hovered {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn draw_find_bar(frame: &mut Frame, app: &mut App) {
    let (current, total) = app.search_match_position();
    let count = format!("{current}/{total}");
    let match_rect = active_search_match_rect(frame, app);
    let Some(area) = find_bar_area(app, 44, match_rect) else {
        app.set_search_bar_hits(None, None, None, None);
        return;
    };

    frame.render_widget(Clear, area);
    let panel_bg = app.theme.background;
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = panel_bg {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);
    let inner_width = inner.width as usize;
    let spacious = inner_width >= 30;
    let lead_gap = if spacious { 2 } else { 1 };
    let action_gap = if spacious { 2 } else { 1 };
    let clear_gap = if spacious { 3 } else { 1 };
    let fixed_without_count = 2usize
        .saturating_add(1)
        .saturating_add(lead_gap)
        .saturating_add(action_gap)
        .saturating_add(1)
        .saturating_add(action_gap)
        .saturating_add(1)
        .saturating_add(clear_gap)
        .saturating_add(2);
    let count = truncate_text(&count, inner_width.saturating_sub(fixed_without_count));
    let count_width = text_width(&count);
    let fixed_width = fixed_without_count.saturating_add(count_width);
    let query_width = inner_width.saturating_sub(fixed_width);
    let query = truncate_text_from_start(app.search_query(), query_width);
    let query_display_width = text_width(&query);
    let query_pad = query_width.saturating_sub(query_display_width);
    let cursor = if app.search_active() && app.search_cursor_visible {
        "│"
    } else {
        " "
    };
    let prompt_style = Style::default()
        .fg(app.theme.primary)
        .add_modifier(Modifier::BOLD);
    let prev_style = find_bar_action_style(app, app.search_prev_hover);
    let next_style = find_bar_action_style(app, app.search_next_hover);
    let clear_style = find_bar_action_style(app, app.search_clear_hover);
    let line = Line::from(vec![
        Span::styled("❯ ", prompt_style),
        Span::styled(query, Style::default().fg(app.theme.text)),
        Span::styled(cursor, prompt_style),
        Span::raw(" ".repeat(query_pad)),
        Span::raw(" ".repeat(lead_gap)),
        Span::styled(count, Style::default().fg(app.theme.text_muted)),
        Span::raw(" ".repeat(action_gap)),
        Span::styled("‹", prev_style),
        Span::raw(" ".repeat(action_gap)),
        Span::styled("›", next_style),
        Span::raw(" ".repeat(clear_gap)),
        Span::styled("✕", clear_style),
        Span::raw(" "),
    ]);
    let mut paragraph = Paragraph::new(line);
    if let Some(bg) = panel_bg {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, inner);

    let count_x = inner
        .x
        .saturating_add((2 + query_width + 1 + lead_gap) as u16);
    let prev_x = count_x.saturating_add((count_width + action_gap) as u16);
    let next_x = prev_x.saturating_add((1 + action_gap) as u16);
    let clear_x = next_x.saturating_add((1 + clear_gap) as u16);
    app.set_search_bar_hits(
        Some((area.x, area.y, area.width, area.height)),
        Some((prev_x, inner.y, 1, 1)),
        Some((next_x, inner.y, 1, 1)),
        Some((
            clear_x,
            inner.y,
            inner.x.saturating_add(inner.width).saturating_sub(clear_x),
            1,
        )),
    );
}

fn file_context_menu_label(action: FileContextMenuAction) -> &'static str {
    match action {
        FileContextMenuAction::Open => "Open",
        FileContextMenuAction::OpenInNewTab => "Open in new tab",
        FileContextMenuAction::CopyPath => "Copy path",
    }
}

fn view_mode_label(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::UnifiedPane => "Unified",
        ViewMode::Split => "Split",
        ViewMode::Evolution => "Evolution",
        ViewMode::Blame => "Blame",
        ViewMode::Preview => "Preview",
    }
}

fn draw_file_context_menu(frame: &mut Frame, app: &mut App) {
    app.file_context_menu_hits.clear();
    if app.file_panel_rect.is_none() {
        app.close_file_context_menu();
        return;
    }
    let Some(menu) = app.file_context_menu else {
        return;
    };
    let area = frame.area();
    let actions = FileContextMenuAction::ALL;
    if area.width < 10 || area.height < actions.len() as u16 + 2 {
        return;
    }
    let width = 20.min(area.width);
    let height = actions.len() as u16 + 2;
    let x = menu
        .x
        .min(area.x.saturating_add(area.width.saturating_sub(width)));
    let y = menu
        .y
        .min(area.y.saturating_add(area.height.saturating_sub(height)));
    let menu_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, menu_area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background_panel.or(app.theme.background) {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), menu_area);

    let inner = block.inner(menu_area);
    let menu_bg = app.theme.background_panel.or(app.theme.background);
    let hover_bg = app
        .theme
        .background_element
        .or_else(|| menu_bg.and_then(|bg| color::blend_colors(bg, app.theme.accent, 0.16)))
        .or(menu_bg);
    for (idx, action) in actions.into_iter().enumerate() {
        let row = inner.y.saturating_add(idx as u16);
        let hover = app.file_context_menu_hover == Some(action);
        let mut text_style = Style::default().fg(if hover {
            app.theme.accent
        } else {
            app.theme.text
        });
        if hover {
            text_style = text_style.add_modifier(Modifier::BOLD);
        }
        let mut row_style = Style::default();
        if let Some(bg) = if hover { hover_bg } else { menu_bg } {
            row_style = row_style.bg(bg);
        }
        app.file_context_menu_hits.push(FileContextMenuHit {
            action,
            x: inner.x,
            y: row,
            width: inner.width,
            height: 1,
        });
        let label = format!(" {}", file_context_menu_label(action));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_text(&label, inner.width as usize),
                text_style,
            )))
            .style(row_style),
            Rect::new(inner.x, row, inner.width, 1),
        );
    }
}

fn draw_review_comment_context_menu(frame: &mut Frame, app: &mut App) {
    app.review_comment_context_menu_hits.clear();
    let Some(menu) = app.review_comment_context_menu else {
        return;
    };
    let actions = app.review_comment_context_menu_actions();
    if actions.is_empty() {
        app.close_review_comment_context_menu();
        return;
    }
    let area = frame.area();
    if area.width < 10 || area.height < actions.len() as u16 + 2 {
        return;
    }
    let labels = actions
        .iter()
        .map(|action| (*action, app.review_comment_context_menu_label(*action)))
        .collect::<Vec<_>>();
    let label_width = labels
        .iter()
        .map(|(_, label)| label.width())
        .max()
        .unwrap_or(10);
    let width = (label_width as u16).saturating_add(2).min(area.width);
    let height = actions.len() as u16 + 2;
    let x = menu
        .x
        .min(area.x.saturating_add(area.width.saturating_sub(width)));
    let y = menu
        .y
        .min(area.y.saturating_add(area.height.saturating_sub(height)));
    let menu_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, menu_area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background_panel.or(app.theme.background) {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), menu_area);

    let inner = block.inner(menu_area);
    let menu_bg = app.theme.background_panel.or(app.theme.background);
    let hover_bg = app
        .theme
        .background_element
        .or_else(|| menu_bg.and_then(|bg| color::blend_colors(bg, app.theme.accent, 0.16)))
        .or(menu_bg);
    for (idx, (action, label_text)) in labels.into_iter().enumerate() {
        let row = inner.y.saturating_add(idx as u16);
        let hover = app.review_comment_context_menu_hover == Some(action);
        let mut text_style = Style::default().fg(if hover {
            app.theme.accent
        } else {
            app.theme.text
        });
        if hover {
            text_style = text_style.add_modifier(Modifier::BOLD);
        }
        let mut row_style = Style::default();
        if let Some(bg) = if hover { hover_bg } else { menu_bg } {
            row_style = row_style.bg(bg);
        }
        app.review_comment_context_menu_hits
            .push(ReviewCommentContextMenuHit {
                action,
                x: inner.x,
                y: row,
                width: inner.width,
                height: 1,
            });
        let label = format!(" {label_text}");
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_text(&label, inner.width as usize),
                text_style,
            )))
            .style(row_style),
            Rect::new(inner.x, row, inner.width, 1),
        );
    }
}

fn draw_status_mode_menu(frame: &mut Frame, app: &mut App) {
    app.status_mode_menu_hits.clear();
    let Some(menu) = app.status_mode_menu else {
        return;
    };
    let area = frame.area();
    let modes = [
        ViewMode::UnifiedPane,
        ViewMode::Split,
        ViewMode::Evolution,
        ViewMode::Blame,
        ViewMode::Preview,
    ];
    if area.width < 10 || area.height < modes.len() as u16 + 2 {
        return;
    }
    let width = 14.min(area.width);
    let height = modes.len() as u16 + 2;
    let x = menu
        .x
        .min(area.x.saturating_add(area.width.saturating_sub(width)));
    let y = menu
        .y
        .min(area.y.saturating_add(area.height.saturating_sub(height)));
    let menu_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, menu_area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background_panel.or(app.theme.background) {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), menu_area);

    let inner = block.inner(menu_area);
    let menu_bg = app.theme.background_panel.or(app.theme.background);
    let active_bg = app
        .theme
        .background_element
        .or_else(|| menu_bg.and_then(|bg| color::blend_colors(bg, app.theme.accent, 0.16)))
        .or(menu_bg);
    for (idx, mode) in modes.into_iter().enumerate() {
        let row = inner.y.saturating_add(idx as u16);
        let hover = app.status_mode_menu_hover == Some(mode);
        let active = app.view_mode == mode;
        let mut text_style = Style::default().fg(if hover || active {
            app.theme.accent
        } else {
            app.theme.text
        });
        if hover || active {
            text_style = text_style.add_modifier(Modifier::BOLD);
        }
        let mut row_style = Style::default();
        if let Some(bg) = if hover || active { active_bg } else { menu_bg } {
            row_style = row_style.bg(bg);
        }
        app.status_mode_menu_hits.push(StatusModeMenuHit {
            mode,
            x: inner.x,
            y: row,
            width: inner.width,
            height: 1,
        });
        let label = format!(" {}", view_mode_label(mode));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_text(&label, inner.width as usize),
                text_style,
            )))
            .style(row_style),
            Rect::new(inner.x, row, inner.width, 1),
        );
    }
}

fn draw_toasts(frame: &mut Frame, app: &mut App) {
    if !app.toasts_enabled {
        return;
    }
    let area = frame.area();
    app.toast_engine.set_area(area);
    frame.render_widget(&app.toast_engine, area);
    if !app.toast_engine.has_toast() {
        return;
    }

    // The engine renders every queued toast but only exposes the front's rect,
    // so reconstruct each toast's rect by probing which cells belong to a toast.
    // Each is then re-skinned to match the app background, hug the message with a
    // thin rounded frame, and color its severity icon.
    let mut bounds: Vec<Option<(u16, u16, u16, u16)>> = vec![None; app.toast_engine.queue_len()];
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(idx) = app.toast_engine.toast_index_at(x, y) {
                match bounds.get_mut(idx) {
                    Some(Some(b)) => {
                        b.0 = b.0.min(x);
                        b.1 = b.1.min(y);
                        b.2 = b.2.max(x);
                        b.3 = b.3.max(y);
                    }
                    Some(slot @ None) => *slot = Some((x, y, x, y)),
                    None => {}
                }
            }
        }
    }

    let border = app.theme.border_subtle;
    // None => transparent (matches a theme with no background).
    let bg = app.theme.background.unwrap_or(Color::Reset);
    // The toaster crate hard-codes the message fg to white (fine on dark themes,
    // unreadable on light ones), so repaint it with the theme's text color.
    let text = app.theme.text;
    let icons = [
        ('✕', app.theme.error),
        ('✓', app.theme.success),
        ('▲', app.theme.warning),
        ('●', app.theme.info),
    ];
    let mut rects: Vec<Rect> = bounds
        .into_iter()
        .flatten()
        .map(|(x0, y0, x1, y1)| Rect {
            x: x0,
            y: y0,
            width: x1 - x0 + 1,
            height: y1 - y0 + 1,
        })
        .collect();
    // The crate leaves a one-row gap between stacked toasts (not configurable).
    // `rects` is in queue order: index 0 is the front (anchored so its click area
    // stays correct); the rest are pulled flush against it, in the stack's
    // direction (newer toasts sit above the front on bottom positions, below on
    // top positions).
    let upward = rects.len() >= 2 && rects[1].y < rects[0].y;
    let buffer = frame.buffer_mut();
    let mut boundary: Option<u16> = None;
    for r in &mut rects {
        let target_y = match boundary {
            None => r.y,
            Some(b) if upward => b.saturating_sub(r.height),
            Some(b) => b,
        };
        if target_y != r.y {
            let (from, to, w, h) = (r.y, target_y, r.width, r.height);
            let rows: Vec<u16> = if to < from {
                (0..h).collect()
            } else {
                (0..h).rev().collect()
            };
            for row in rows {
                for x in r.x..r.x + w {
                    if let Some(cell) = buffer.cell((x, from + row)).cloned() {
                        if let Some(dst) = buffer.cell_mut((x, to + row)) {
                            *dst = cell;
                        }
                    }
                }
            }
            // Clear the rows the toast vacated.
            let vacated = if to < from {
                (to + h)..(from + h)
            } else {
                from..to
            };
            for row in vacated {
                for x in r.x..r.x + w {
                    if let Some(dst) = buffer.cell_mut((x, row)) {
                        dst.set_symbol(" ").set_bg(bg);
                    }
                }
            }
            r.y = target_y;
        }
        boundary = Some(if upward { r.y } else { r.y + r.height });
        reskin_toast(buffer, *r, border, bg, text, &icons);
    }
}

/// Re-skin one crate-rendered toast in place: match `bg` (transparent when the
/// theme has none), tuck a thin rounded border in over the crate's outer padding
/// rows so the frame hugs the message, recolor the message text to `text`, and
/// color the leading severity icon.
fn reskin_toast(
    buffer: &mut ratatui::buffer::Buffer,
    ta: Rect,
    border: Color,
    bg: Color,
    text: Color,
    icons: &[(char, Color)],
) {
    if ta.width < 2 || ta.height < 3 {
        return;
    }
    let right = ta.x + ta.width - 1;
    let bottom = ta.y + ta.height - 1;
    for y in ta.y..=bottom {
        for x in ta.x..=right {
            let Some(cell) = buffer.cell_mut((x, y)) else {
                continue;
            };
            if y == ta.y || y == bottom {
                let ch = if x == ta.x {
                    if y == ta.y {
                        "╭"
                    } else {
                        "╰"
                    }
                } else if x == right {
                    if y == ta.y {
                        "╮"
                    } else {
                        "╯"
                    }
                } else {
                    "─"
                };
                cell.set_symbol(ch).set_fg(border).set_bg(bg);
            } else if x == ta.x || x == right {
                cell.set_symbol("│").set_fg(border).set_bg(bg);
            } else {
                cell.set_bg(bg).set_fg(text);
            }
        }
    }
    'find: for y in ta.y..=bottom {
        for x in ta.x..=right {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                for (glyph, color) in icons {
                    if cell.symbol().chars().eq(std::iter::once(*glyph)) {
                        cell.set_fg(*color);
                        break 'find;
                    }
                }
            }
        }
    }
}

fn draw_preview_status_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let mode = " PREVIEW ";
    let path = app.current_file_path();
    app.status_mode_hit = Some((area.x, area.y, text_width(mode) as u16, 1));
    let available_width = area.width as usize;
    let left_width = (available_width * 4) / 10;
    let center_width = (available_width * 2) / 10;
    let right_width = available_width.saturating_sub(left_width + center_width);
    let mut left_spans = vec![
        Span::styled(
            mode,
            Style::default()
                .fg(app.theme.background.unwrap_or(Color::Black))
                .bg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            truncate_path(&path, left_width.saturating_sub(text_width(mode) + 1)),
            Style::default().fg(app.theme.text),
        ),
    ];
    left_spans = pad_spans_left(clamp_spans_to_width(&left_spans, left_width), left_width);
    let center_spans = pad_spans_center(
        clamp_spans_to_width(
            &line_input_status_spans(app).unwrap_or_default(),
            center_width,
        ),
        center_width,
    );

    let mut right_spans = Vec::new();
    let mut comments_hit: Option<(usize, usize)> = None;
    let mut comment_style = Style::default().fg(if app.status_comments_hover {
        app.theme.accent
    } else {
        app.theme.text_muted
    });
    if app.status_comments_hover {
        comment_style = comment_style.add_modifier(Modifier::BOLD);
    }
    if let Some((spans, width)) = review_status_comment_spans(app, comment_style) {
        let start = spans_width(&right_spans);
        comments_hit = Some((start, width));
        right_spans.extend(spans);
        right_spans.push(Span::raw("  "));
    }
    let file_count = app.multi_diff.file_count();
    let current_file = app.multi_diff.selected_index + 1;
    let file_label = format!("file {}/{}", current_file, file_count);
    let file_start = spans_width(&right_spans);
    let file_width = text_width(&file_label);
    let mut file_style = Style::default().fg(if app.status_file_hover {
        app.theme.accent
    } else {
        app.theme.text_muted
    });
    if app.status_file_hover {
        file_style = file_style.add_modifier(Modifier::BOLD);
    }
    right_spans.push(Span::styled(file_label, file_style));

    let right_raw_width = spans_width(&right_spans);
    if right_raw_width <= right_width {
        let right_x = area.x.saturating_add((left_width + center_width) as u16);
        let pad = right_width.saturating_sub(right_raw_width);
        if let Some((start, width)) = comments_hit {
            app.status_comments_hit = Some((
                right_x.saturating_add((pad + start) as u16),
                area.y,
                width as u16,
                1,
            ));
        }
        app.status_file_hit = Some((
            right_x.saturating_add((pad + file_start) as u16),
            area.y,
            file_width as u16,
            1,
        ));
    }
    let right_spans = pad_spans_right(clamp_spans_to_width(&right_spans, right_width), right_width);
    let mut spans = Vec::new();
    spans.extend(left_spans);
    spans.extend(center_spans);
    spans.extend(right_spans);
    let mut paragraph = Paragraph::new(Line::from(spans));
    if let Some(bg) = app.theme.background {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, area);
}

fn line_input_status_spans(app: &App) -> Option<Vec<Span<'static>>> {
    let (prefix, query, placeholder) = if app.goto_active() {
        (":", app.goto_query(), "Go to")
    } else {
        return None;
    };
    let query_text = if query.is_empty() {
        placeholder.to_string()
    } else {
        query.to_string()
    };
    let query_style = if query.is_empty() {
        Style::default().fg(app.theme.text_muted)
    } else {
        Style::default().fg(app.theme.text)
    };
    Some(vec![
        Span::styled(
            prefix.to_string(),
            Style::default().fg(app.theme.text_muted),
        ),
        Span::raw(" "),
        Span::styled(query_text, query_style),
    ])
}

fn draw_status_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    app.status_comments_hit = None;
    app.status_file_hit = None;
    if app.view_mode == ViewMode::Preview {
        draw_preview_status_bar(frame, app, area);
        return;
    }
    let state = app.state();
    let (insertions, deletions) = app.stats();

    // View mode indicator
    let mode = match app.view_mode {
        ViewMode::UnifiedPane => " UNIFIED ",
        ViewMode::Split => " SPLIT ",
        ViewMode::Evolution => " EVOLUTION ",
        ViewMode::Blame => " BLAME ",
        ViewMode::Preview => " PREVIEW ",
    };

    app.status_mode_hit = Some((area.x, area.y, text_width(mode) as u16, 1));
    let file_path = app.current_file_path();
    let available_width = area.width as usize;

    let file_name = file_path.rsplit('/').next().unwrap_or(&file_path);
    let scope_full = if let Some(branch) = app.git_branch.as_ref() {
        format!("{}@{}", file_path, branch)
    } else {
        file_path.clone()
    };
    let scope_short = if let Some(branch) = app.git_branch.as_ref() {
        format!("{}@{}", file_name, branch)
    } else {
        file_name.to_string()
    };

    // Step counter and autoplay indicator (flash when autoplay is on)
    let step_current = state.current_step + 1;
    let step_total = state.total_steps;
    let step_text = format!("{}/{}", step_current, step_total);
    let (arrow_style, step_style) = if app.autoplay {
        #[allow(clippy::manual_is_multiple_of)]
        let flash = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            / 500)
            % 2
            == 0;
        if flash {
            (
                Style::default().fg(app.theme.warning),
                Style::default().fg(app.theme.warning),
            )
        } else {
            (
                Style::default().fg(app.theme.warning_dim()),
                Style::default().fg(app.theme.warning_dim()),
            )
        }
    } else {
        (
            Style::default().fg(app.theme.text_muted),
            Style::default().fg(app.theme.text),
        )
    };

    // Hunk counter
    let (current_hunk, total_hunks) = app.hunk_info();
    let hunk_text = if total_hunks > 0 {
        Some(format_ratio(current_hunk, total_hunks))
    } else {
        None
    };
    let hunk_step_text = if app.stepping {
        app.hunk_step_info().and_then(|(current, total)| {
            if current > 0 {
                Some(format_ratio(current, total))
            } else {
                None
            }
        })
    } else {
        None
    };

    // File counter (at the end)
    let file_count = app.multi_diff.file_count();
    let current_file = app.multi_diff.selected_index + 1;
    let file_text = format!("{}/{}", current_file, file_count);

    // Build CENTER section: goto/search prompt or step counter
    let mut center_spans = Vec::new();
    if let Some(input_spans) = line_input_status_spans(app) {
        center_spans = input_spans;
    } else if app.stepping {
        let autoplay_marker = if app.autoplay {
            if app.autoplay_reverse {
                "◀"
            } else {
                "▶"
            }
        } else {
            " "
        };
        center_spans.push(Span::styled(autoplay_marker, arrow_style));
        center_spans.push(Span::raw(" "));
        center_spans.push(Span::styled(
            "step ",
            Style::default().fg(app.theme.text_muted),
        ));
        center_spans.push(Span::styled(step_text.clone(), step_style));
    }

    // Build RIGHT section: stats + hunk + file
    let diff_pending = matches!(
        app.multi_diff.current_file_diff_status(),
        DiffStatus::Loading | DiffStatus::Deferred | DiffStatus::Computing
    ) || app.content_loading_count() > 0
        || app.view_build_pending()
        || app.syntax_warmup_pending();
    let stats_known = insertions > 0 || deletions > 0;
    let mut right_spans = Vec::new();
    let mut comments_hit: Option<(usize, usize)> = None;
    if let Some(ref hunk) = hunk_text {
        let hunk_label = if let Some(ref hunk_step) = hunk_step_text {
            format!("{} {}", hunk_step, hunk)
        } else {
            hunk.to_string()
        };
        right_spans.push(Span::styled(
            hunk_label,
            Style::default().fg(app.theme.text_muted),
        ));
        right_spans.push(Span::raw("  "));
    }
    let spinner = if diff_pending {
        diff_spinner_frame()
    } else {
        " "
    };
    right_spans.push(Span::styled(
        spinner,
        Style::default().fg(app.theme.text_muted),
    ));
    right_spans.push(Span::raw(" "));
    if app.content_loading_count() > 0 {
        right_spans.push(Span::styled(
            format!("loading {} files…", app.content_loading_count()),
            Style::default().fg(app.theme.text_muted),
        ));
    } else if diff_pending && !stats_known {
        right_spans.push(Span::styled(
            "diffing…",
            Style::default().fg(app.theme.text_muted),
        ));
    } else {
        right_spans.push(Span::styled(
            format!("+{}", insertions),
            Style::default().fg(app.theme.success),
        ));
        right_spans.push(Span::raw(" "));
        right_spans.push(Span::styled(
            format!("-{}", deletions),
            Style::default().fg(app.theme.error),
        ));
    }
    if app.files_changed_indicator_active() {
        right_spans.push(Span::raw(" "));
        right_spans.push(Span::styled(
            "changed",
            Style::default().fg(app.theme.warning),
        ));
    }
    let mut comment_style = Style::default().fg(if app.status_comments_hover {
        app.theme.accent
    } else {
        app.theme.text_muted
    });
    if app.status_comments_hover {
        comment_style = comment_style.add_modifier(Modifier::BOLD);
    }
    if let Some((spans, width)) = review_status_comment_spans(app, comment_style) {
        right_spans.push(Span::raw(" "));
        let start = spans_width(&right_spans);
        comments_hit = Some((start, width));
        right_spans.extend(spans);
    }
    right_spans.push(Span::raw("  "));
    let file_label = format!("file {}", file_text);
    let file_start = spans_width(&right_spans);
    let file_width = text_width(&file_label);
    let file_hit = (file_start, file_width);
    let mut file_style = Style::default().fg(if app.status_file_hover {
        app.theme.accent
    } else {
        app.theme.text_muted
    });
    if app.status_file_hover {
        file_style = file_style.add_modifier(Modifier::BOLD);
    }
    right_spans.push(Span::styled(file_label, file_style));

    // Fixed-width footer layout: left/middle/right sections prevent shifting.
    let left_width = (available_width * 4) / 10;
    let center_width = (available_width * 2) / 10;
    let right_width = available_width.saturating_sub(left_width + center_width);
    let left_fixed_width = text_width(mode) + 1;
    let path_max_width = left_width.saturating_sub(left_fixed_width);
    let scope_base = if available_width < 60 {
        scope_short
    } else {
        scope_full
    };
    let display_scope = truncate_path(&scope_base, path_max_width);

    let left_spans = vec![
        Span::styled(
            mode,
            Style::default()
                .fg(app.theme.background.unwrap_or(Color::Black))
                .bg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(display_scope, Style::default().fg(app.theme.text_muted)),
    ];

    let right_raw_width = spans_width(&right_spans);
    if right_raw_width <= right_width {
        let right_x = area.x.saturating_add((left_width + center_width) as u16);
        let pad = right_width.saturating_sub(right_raw_width);
        if let Some((start, width)) = comments_hit {
            app.status_comments_hit = Some((
                right_x.saturating_add((pad + start) as u16),
                area.y,
                width as u16,
                1,
            ));
        }
        let (start, width) = file_hit;
        app.status_file_hit = Some((
            right_x.saturating_add((pad + start) as u16),
            area.y,
            width as u16,
            1,
        ));
    }

    let left_spans = clamp_spans_to_width(&left_spans, left_width);
    let left_spans = pad_spans_left(left_spans, left_width);
    let center_spans = clamp_spans_to_width(&center_spans, center_width);
    let center_spans = pad_spans_center(center_spans, center_width);
    let right_spans = clamp_spans_to_width(&right_spans, right_width);
    let right_spans = pad_spans_right(right_spans, right_width);

    // Build final spans
    let mut spans = Vec::new();
    spans.extend(left_spans);
    spans.extend(center_spans);
    spans.extend(right_spans);

    let status_line = Line::from(spans);
    let mut paragraph = Paragraph::new(status_line);
    if let Some(bg) = app.theme.background {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, area);
}

fn draw_top_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    app.topbar_area = Some((area.x, area.y, area.width, area.height));
    let available_width = area.width as usize;
    app.preview_toggle_hit = None;
    app.topbar_scroll_left_hit = None;
    app.topbar_scroll_right_hit = None;
    app.topbar_sidebar_toggle_hit = None;
    let mut right_spans = if matches!(app.view_mode, ViewMode::Preview) {
        preview_topbar_spans(app)
    } else {
        let (insertions, deletions) = app.stats();
        let diff_pending = matches!(
            app.multi_diff.current_file_diff_status(),
            DiffStatus::Loading | DiffStatus::Deferred | DiffStatus::Computing
        ) || app.content_loading_count() > 0
            || app.view_build_pending()
            || app.syntax_warmup_pending();
        let stats_known = insertions > 0 || deletions > 0;
        if matches!(app.view_mode, ViewMode::Blame) {
            blame_age_legend_spans(app)
        } else if diff_pending {
            if stats_known {
                vec![
                    Span::styled(
                        diff_spinner_frame(),
                        Style::default().fg(app.theme.text_muted),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("+{}", insertions),
                        Style::default().fg(app.theme.success),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("-{}", deletions),
                        Style::default().fg(app.theme.error),
                    ),
                    Span::raw(" "),
                ]
            } else {
                vec![
                    Span::styled(
                        diff_spinner_frame(),
                        Style::default().fg(app.theme.text_muted),
                    ),
                    Span::raw(" "),
                ]
            }
        } else {
            vec![
                Span::styled(
                    format!("+{}", insertions),
                    Style::default().fg(app.theme.success),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("-{}", deletions),
                    Style::default().fg(app.theme.error),
                ),
                Span::raw(" "),
            ]
        }
    };
    let preview_width = if matches!(app.view_mode, ViewMode::Preview)
        && (preview_can_render_markdown(app)
            || preview_structured_kind(app).is_some()
            || preview_can_render_csv(app))
    {
        spans_width(&right_spans).min(available_width)
    } else {
        0
    };

    let sidebar_toggle = app
        .can_show_file_panel()
        .then(|| topbar_sidebar_toggle_spans(app));
    let sidebar_toggle_width = sidebar_toggle
        .as_ref()
        .map(|spans| spans_width(spans))
        .unwrap_or(0);
    let sidebar_on_left = app.file_panel_position == FilePanelPosition::Left;
    if !sidebar_on_left {
        if let Some(spans) = sidebar_toggle.as_ref() {
            right_spans.extend(spans.clone());
        }
    }

    let right_width = spans_width(&right_spans).min(available_width);
    if preview_width > 0 {
        app.preview_toggle_hit = Some((
            area.x
                .saturating_add(available_width.saturating_sub(right_width) as u16),
            area.y,
            preview_width as u16,
            1,
        ));
    }
    if !sidebar_on_left && sidebar_toggle.is_some() && available_width >= sidebar_toggle_width {
        app.topbar_sidebar_toggle_hit = Some((
            area.x
                .saturating_add(available_width.saturating_sub(sidebar_toggle_width) as u16),
            area.y,
            sidebar_toggle_width as u16,
            1,
        ));
    }

    let left_max = available_width.saturating_sub(right_width + 1);
    let left_toggle_width =
        if sidebar_on_left && sidebar_toggle.is_some() && left_max >= sidebar_toggle_width {
            app.topbar_sidebar_toggle_hit = Some((area.x, area.y, sidebar_toggle_width as u16, 1));
            sidebar_toggle_width
        } else {
            0
        };
    let tabs_max = left_max.saturating_sub(left_toggle_width);
    let tab_area = Rect::new(
        area.x.saturating_add(left_toggle_width as u16),
        area.y,
        area.width.saturating_sub(left_toggle_width as u16),
        area.height,
    );
    let mut left_spans = Vec::new();
    if left_toggle_width > 0 {
        if let Some(spans) = sidebar_toggle.as_ref() {
            left_spans.extend(spans.clone());
        }
    }
    let mut tab_spans = topbar_tab_spans(app, tab_area, tabs_max);
    tab_spans = clamp_spans_to_width(&tab_spans, tabs_max);
    tab_spans = pad_spans_left(tab_spans, tabs_max);
    left_spans.extend(tab_spans);

    right_spans = clamp_spans_to_width(&right_spans, right_width + 1);
    let right_spans = pad_spans_right(right_spans, right_width + 1);

    let mut spans = Vec::new();
    spans.extend(left_spans);
    spans.extend(right_spans);

    let mut paragraph = Paragraph::new(Line::from(spans));
    if let Some(bg) = app.theme.background {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, area);
}

fn topbar_sidebar_toggle_spans(app: &App) -> Vec<Span<'static>> {
    let panel_visible = app.file_panel_visible && !app.file_panel_auto_hidden;
    let glyph = match (app.file_panel_position, panel_visible) {
        (FilePanelPosition::Left, true) => "«",
        (FilePanelPosition::Left, false) => "»",
        (FilePanelPosition::Right, true) => "»",
        (FilePanelPosition::Right, false) => "«",
    };
    let mut style = Style::default().fg(if app.topbar_sidebar_toggle_hover {
        app.theme.accent
    } else {
        app.theme.text_muted
    });
    if app.topbar_sidebar_toggle_hover {
        style = style.add_modifier(Modifier::BOLD);
    }
    let label = match app.file_panel_position {
        FilePanelPosition::Left => format!(" {glyph}  "),
        FilePanelPosition::Right => format!("{glyph} "),
    };
    vec![Span::styled(label, style)]
}

fn preview_topbar_spans(app: &App) -> Vec<Span<'static>> {
    if preview_can_render_image(app) {
        return Vec::new();
    }
    if preview_can_render_markdown(app)
        || preview_structured_kind(app).is_some()
        || preview_can_render_csv(app)
    {
        let label = if app.active_preview_rendered() {
            " source "
        } else {
            " preview "
        };
        let mut style = Style::default().fg(if app.preview_toggle_hover {
            app.theme.accent
        } else {
            app.theme.text_muted
        });
        if app.preview_toggle_hover {
            style = style.add_modifier(Modifier::BOLD);
        }
        return vec![Span::styled(label, style)];
    }
    vec![Span::styled(
        " preview ",
        Style::default().fg(app.theme.text_muted),
    )]
}

fn preview_structured_kind(app: &App) -> Option<StructuredPreviewKind> {
    match app.active_topbar_content() {
        Some(TopbarTabContent::File(index)) => app
            .multi_diff
            .files
            .get(index)
            .and_then(|file| StructuredPreviewKind::from_file_name(&file.display_name)),
        _ => None,
    }
}

fn preview_can_render_csv(app: &App) -> bool {
    match app.active_topbar_content() {
        Some(TopbarTabContent::File(index)) => app
            .multi_diff
            .files
            .get(index)
            .map(|file| is_csv_name(&file.display_name))
            .unwrap_or(false),
        _ => false,
    }
}

fn preview_can_render_markdown(app: &App) -> bool {
    match app.active_topbar_content() {
        Some(TopbarTabContent::Help) => true,
        Some(TopbarTabContent::PrComments | TopbarTabContent::OutdatedComments) => false,
        Some(TopbarTabContent::File(index)) => app
            .multi_diff
            .files
            .get(index)
            .map(|file| is_markdown_name(&file.display_name))
            .unwrap_or(false),
        None => false,
    }
}

fn preview_can_render_image(app: &App) -> bool {
    match app.active_topbar_content() {
        Some(TopbarTabContent::File(index)) => app
            .multi_diff
            .files
            .get(index)
            .map(|file| is_image_name(&file.display_name))
            .unwrap_or(false),
        _ => false,
    }
}

fn topbar_tab_spans(app: &mut App, area: Rect, max_width: usize) -> Vec<Span<'static>> {
    app.ensure_topbar_tabs();
    app.topbar_tab_hits.clear();
    app.topbar_plus_hit = None;
    app.topbar_scroll_left_hit = None;
    app.topbar_scroll_right_hit = None;

    let active = app.active_topbar_tab;
    let drag_target = app
        .topbar_drag_target
        .filter(|target| *target <= app.topbar_tabs.len());
    app.topbar_tab_scroll = app
        .topbar_tab_scroll
        .min(app.topbar_tabs.len().saturating_sub(1));
    let hidden_left = app.topbar_tab_scroll > 0;
    let reserve_right_indicator = app.topbar_tabs.len().saturating_sub(app.topbar_tab_scroll) > 1;
    let render_max = max_width.saturating_sub(if reserve_right_indicator { 5 } else { 0 });
    let mut spans = Vec::new();
    let mut col = 0usize;
    if hidden_left && max_width >= 2 {
        app.topbar_scroll_left_hit = Some((area.x, area.y, 2, 1));
        spans.push(Span::styled(
            "‹ ",
            topbar_overflow_style(app, app.topbar_scroll_left_hover),
        ));
        col = 2;
    }
    let mut rendered_until = app.topbar_tab_scroll;
    for (tab_pos, tab) in app
        .topbar_tabs
        .clone()
        .into_iter()
        .enumerate()
        .skip(app.topbar_tab_scroll)
    {
        if drag_target == Some(tab_pos) && col < render_max {
            spans.push(Span::styled(
                "│",
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            col = col.saturating_add(1);
        }
        let remaining = render_max.saturating_sub(col);
        if remaining < 4 {
            break;
        }
        let reconstructed_title = (app.active_topbar_tab == Some(tab.id))
            .then(|| app.outdated_diff_title())
            .flatten();
        let (file_name, changed) = if let Some(title) = reconstructed_title {
            (title, "")
        } else {
            match tab.content {
                TopbarTabContent::File(file_index) => {
                    let Some(file) = app
                        .outdated_live_files()
                        .and_then(|files| files.get(file_index))
                        .or_else(|| app.multi_diff.files.get(file_index))
                    else {
                        continue;
                    };
                    let file_name = file
                        .display_name
                        .rsplit('/')
                        .next()
                        .unwrap_or(&file.display_name)
                        .to_string();
                    let changed = if app.file_changed_on_disk(file_index) {
                        "*"
                    } else {
                        ""
                    };
                    (file_name, changed)
                }
                TopbarTabContent::Help => ("Help".to_string(), ""),
                TopbarTabContent::PrComments => (
                    format!(
                        "{} comments",
                        app.review_provider_kind().long_review_noun_title()
                    ),
                    "",
                ),
                TopbarTabContent::OutdatedComments => ("Outdated comments".to_string(), ""),
            }
        };
        let closeable = app.topbar_close_allowed(tab.id);
        let show_close = closeable && app.topbar_hover_tab == Some(tab.id);
        let close = if closeable {
            if show_close {
                " ×"
            } else {
                "  "
            }
        } else {
            ""
        };
        let name_width = remaining
            .saturating_sub(2 + text_width(changed) + text_width(close))
            .max(1);
        let name = truncate_filename_keep_ext(&file_name, name_width);
        let tab_text = format!(" {name}{changed}{close} ");
        let width = text_width(&tab_text);
        if width > remaining {
            break;
        }
        let start_col = area.x.saturating_add(col as u16);
        let end_col = start_col.saturating_add(width as u16);
        app.topbar_tab_hits.push(TopbarTabHit {
            tab_id: tab.id,
            row: area.y,
            start_col,
            end_col,
            close_col: closeable.then_some(end_col.saturating_sub(2)),
        });
        let active_tab = Some(tab.id) == active;
        let style = topbar_tab_style(app, active_tab, app.topbar_hover_tab == Some(tab.id));
        spans.push(Span::styled(format!(" {name}{changed}"), style));
        if closeable {
            spans.push(Span::styled(" ", style));
            let close_style = topbar_close_style(
                app,
                active_tab,
                app.topbar_hover_tab == Some(tab.id),
                app.topbar_hover_close == Some(tab.id),
            );
            spans.push(Span::styled(
                if show_close { "×" } else { " " },
                close_style,
            ));
        }
        spans.push(Span::styled(" ", style));
        col = col.saturating_add(width);
        rendered_until = tab_pos.saturating_add(1);
        if col < render_max {
            spans.push(Span::raw(" "));
            col = col.saturating_add(1);
        }
    }

    if drag_target == Some(app.topbar_tabs.len()) && col < render_max {
        spans.push(Span::styled(
            "│",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
        col = col.saturating_add(1);
    }

    let hidden_right = rendered_until < app.topbar_tabs.len();
    if hidden_right && col < max_width {
        let label = if col + 2 <= max_width { " ›" } else { "›" };
        let width = text_width(label);
        app.topbar_scroll_right_hit =
            Some((area.x.saturating_add(col as u16), area.y, width as u16, 1));
        spans.push(Span::styled(
            label,
            topbar_overflow_style(app, app.topbar_scroll_right_hover),
        ));
        col = col.saturating_add(width);
    }

    if col + 3 <= max_width {
        let plus_col = area.x.saturating_add(col as u16);
        app.topbar_plus_hit = Some((plus_col, area.y, 3, 1));
        let plus_style = Style::default()
            .fg(if app.topbar_plus_hover {
                app.theme.accent
            } else {
                app.theme.text_muted
            })
            .add_modifier(Modifier::BOLD);
        spans.push(Span::styled(" + ", plus_style));
    }
    spans
}

fn topbar_overflow_style(app: &App, hovered: bool) -> Style {
    let mut style = Style::default().fg(if hovered {
        app.theme.accent
    } else {
        app.theme.text_muted
    });
    if hovered {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn topbar_tab_style(app: &App, active: bool, hovered: bool) -> Style {
    if active {
        return Style::default()
            .fg(app.theme.background.unwrap_or(Color::Black))
            .bg(app.theme.accent)
            .add_modifier(Modifier::BOLD);
    }
    let bg = topbar_inactive_tab_bg(app, hovered);
    let mut style = Style::default().fg(if hovered {
        app.theme.accent
    } else {
        app.theme.text_muted
    });
    if let Some(bg) = bg {
        style = style.bg(bg);
    }
    style
}

fn topbar_close_style(app: &App, active: bool, hovered: bool, close_hovered: bool) -> Style {
    let mut style = if active {
        Style::default()
            .fg(app.theme.background.unwrap_or(Color::Black))
            .bg(app.theme.accent)
    } else {
        let mut style = Style::default().fg(if hovered {
            app.theme.accent
        } else {
            app.theme.text_muted
        });
        if let Some(bg) = topbar_inactive_tab_bg(app, hovered) {
            style = style.bg(bg);
        }
        style
    };
    if close_hovered {
        style = brighten_close_style(style);
    }
    style
}

fn topbar_inactive_tab_bg(app: &App, _hovered: bool) -> Option<Color> {
    app.theme.background_panel.or(app.theme.background)
}

fn brighten_close_style(style: Style) -> Style {
    let style = style.add_modifier(Modifier::BOLD);
    match style.fg {
        Some(fg) => style.fg(brighten_close_color(fg)),
        None => style,
    }
}

fn brighten_close_color(color: Color) -> Color {
    match color {
        Color::Black => Color::DarkGray,
        Color::DarkGray => Color::Gray,
        Color::Gray | Color::White => color,
        Color::Red => Color::LightRed,
        Color::Green => Color::LightGreen,
        Color::Yellow => Color::LightYellow,
        Color::Blue => Color::LightBlue,
        Color::Magenta => Color::LightMagenta,
        Color::Cyan => Color::LightCyan,
        Color::Indexed(value @ 0..=6) => Color::Indexed(value + 8),
        Color::Rgb(r, g, b) => Color::Rgb(
            r.saturating_add((u8::MAX - r) / 4),
            g.saturating_add((u8::MAX - g) / 4),
            b.saturating_add((u8::MAX - b) / 4),
        ),
        _ => color,
    }
}

fn blame_age_legend_spans(app: &App) -> Vec<Span<'static>> {
    let blocks = 10usize;
    let mut spans = Vec::with_capacity(blocks + 3);
    let label_style = Style::default()
        .fg(app.theme.border_subtle)
        .add_modifier(Modifier::DIM);
    spans.push(Span::styled("Older ", label_style));

    let base = app.theme.warning;
    let steps = blocks.saturating_sub(1).max(1) as f32;
    for idx in 0..blocks {
        let t = idx as f32 / steps;
        spans.push(Span::styled(
            "▮",
            Style::default().fg(color::ramp_color(base, t)),
        ));
    }

    spans.push(Span::styled(" Newer", label_style));
    spans.push(Span::raw(" "));
    spans
}

fn draw_content(frame: &mut Frame, app: &mut App, area: Rect, show_topbar: bool) {
    // Auto-hide file panel if viewport is too narrow (need at least 50 cols for diff view)
    // But respect user's manual toggle preference
    let min_width_for_panel = FILE_PANEL_MIN_WIDTH + DIFF_VIEW_MIN_WIDTH;

    let panel_allowed = app.can_show_file_panel();

    // Track if panel would be auto-hidden (for toggle behavior)
    app.file_panel_auto_hidden = panel_allowed
        && app.file_panel_visible
        && area.width < min_width_for_panel
        && !app.file_panel_manually_set;

    let show_panel = if app.file_panel_manually_set {
        // User explicitly toggled, respect their preference
        panel_allowed && app.file_panel_visible
    } else {
        // Auto-hide when viewport is too narrow
        panel_allowed && app.file_panel_visible && area.width >= min_width_for_panel
    };

    if show_panel {
        let panel_width = app.clamp_file_panel_width(area.width);
        app.file_panel_width = panel_width;
        let constraints = if app.file_panel_position == FilePanelPosition::Left {
            [Constraint::Length(panel_width), Constraint::Min(0)]
        } else {
            [Constraint::Min(0), Constraint::Length(panel_width)]
        };
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(area);
        let (panel_area, diff_area) = if app.file_panel_position == FilePanelPosition::Left {
            (chunks[0], chunks[1])
        } else {
            (chunks[1], chunks[0])
        };

        app.file_panel_rect = Some((
            panel_area.x,
            panel_area.y,
            panel_area.width,
            panel_area.height,
        ));
        draw_file_list(frame, app, panel_area);
        if show_topbar {
            let diff_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(diff_area);
            draw_top_bar(frame, app, diff_chunks[0]);
            app.last_viewport_height = diff_chunks[1].height as usize;
            app.diff_view_area = Some((
                diff_chunks[1].x,
                diff_chunks[1].y,
                diff_chunks[1].width,
                diff_chunks[1].height,
            ));
            draw_diff_view(frame, app, diff_chunks[1]);
        } else {
            app.last_viewport_height = diff_area.height as usize;
            app.diff_view_area =
                Some((diff_area.x, diff_area.y, diff_area.width, diff_area.height));
            draw_diff_view(frame, app, diff_area);
        }
    } else {
        // Single file mode, file panel hidden, or viewport too narrow
        app.file_list_area = None;
        app.file_list_rows.clear();
        app.file_list_hover = None;
        app.file_panel_hover = false;
        app.file_filter_area = None;
        app.file_filter_clear_hit = None;
        app.file_panel_mode_toggle_hit = None;
        app.file_panel_mode_toggle_hover = false;
        app.file_panel_root_hit = None;
        app.file_panel_root_hover = false;
        app.file_filter_hover = false;
        app.file_filter_clear_hover = false;
        app.file_panel_rect = None;
        if show_topbar {
            let diff_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(area);
            draw_top_bar(frame, app, diff_chunks[0]);
            app.last_viewport_height = diff_chunks[1].height as usize;
            app.diff_view_area = Some((
                diff_chunks[1].x,
                diff_chunks[1].y,
                diff_chunks[1].width,
                diff_chunks[1].height,
            ));
            draw_diff_view(frame, app, diff_chunks[1]);
        } else {
            app.last_viewport_height = area.height as usize;
            app.diff_view_area = Some((area.x, area.y, area.width, area.height));
            draw_diff_view(frame, app, area);
        }
    }
}

fn reserve_file_scrollbar_lane(area: Rect, visible: bool) -> (Rect, Rect) {
    if !visible || area.width == 0 {
        return (
            area,
            Rect::new(area.x.saturating_add(area.width), area.y, 0, area.height),
        );
    }
    (
        Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height),
        Rect::new(
            area.x.saturating_add(area.width.saturating_sub(1)),
            area.y,
            1,
            area.height,
        ),
    )
}

fn render_file_panel_scrollbar(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    total_items: usize,
    visible_items: usize,
) {
    if area.width == 0 {
        return;
    }
    let track = area.inner(Margin {
        vertical: 0,
        horizontal: 0,
    });
    let x = track.x;
    let panel_bg = app.theme.background_panel.or(app.theme.background);
    let track_style = panel_bg.map(|bg| Style::default().bg(bg));
    let focused = app.file_list_focused || app.file_filter_active;
    let symbol = if focused { "▐" } else { "▕" };
    let mut style = Style::default().fg(app.theme.text_muted);
    if let Some(bg) = panel_bg {
        style = style.bg(bg);
    }
    let buffer = frame.buffer_mut();
    for row in track.y..track.y.saturating_add(track.height) {
        if let Some(cell) = buffer.cell_mut((x, row)) {
            cell.set_symbol(" ");
            if let Some(track_style) = track_style {
                cell.set_style(track_style);
            }
        }
    }
    let scroll = app.file_list_scroll;
    let Some((thumb_top, thumb_height)) =
        diff_scrollbar_thumb(total_items, visible_items, track.height, scroll)
    else {
        return;
    };
    app.set_file_panel_scrollbar(FilePanelScrollbarState {
        x,
        y: track.y,
        height: track.height,
        total_items,
        visible_items,
        thumb_top,
        thumb_height,
    });
    if !focused && !app.file_panel_hover {
        return;
    }
    let start = track.y.saturating_add(thumb_top);
    let end = start
        .saturating_add(thumb_height)
        .min(track.y.saturating_add(track.height));
    for row in start..end {
        if let Some(cell) = buffer.cell_mut((x, row)) {
            cell.set_symbol(symbol).set_style(style);
        }
    }
}

const FILE_PANEL_HEADER_RIGHT_PADDING: u16 = 4;
const COMMENT_LIST_LEFT_PADDING: u16 = 1;
const COMMENT_LIST_RIGHT_PADDING: u16 = 4;
const COMMENT_ACTION_LEFT_PADDING: u16 = 2;
const COMMENT_ACTION_RIGHT_PADDING: u16 = 4;
const COMMENTS_OVERFLOW_LABEL: &str = "\u{22EF}";

fn file_panel_content_area(app: &App, area: Rect) -> Rect {
    if area.width <= 1 {
        return area;
    }
    match app.file_panel_position {
        FilePanelPosition::Left => Rect::new(area.x, area.y, area.width - 1, area.height),
        FilePanelPosition::Right => Rect::new(area.x + 1, area.y, area.width - 1, area.height),
    }
}

fn draw_file_panel_divider(frame: &mut Frame, app: &App, area: Rect, panel_bg: Option<Color>) {
    if area.width == 0 {
        return;
    }
    let (x, symbol) = match app.file_panel_position {
        FilePanelPosition::Left => (area.x.saturating_add(area.width.saturating_sub(1)), "▕"),
        FilePanelPosition::Right => (area.x, "▏"),
    };
    let fg = panel_bg
        .and_then(|bg| color::blend_colors(bg, app.theme.border_subtle, 0.12))
        .unwrap_or(app.theme.border_subtle);
    let mut style = Style::default().fg(fg).add_modifier(Modifier::DIM);
    if let Some(bg) = panel_bg {
        style = style.bg(bg);
    }
    let buffer = frame.buffer_mut();
    for row in area.y..area.y.saturating_add(area.height) {
        if let Some(cell) = buffer.cell_mut((x, row)) {
            cell.set_symbol(symbol).set_style(style);
        }
    }
}

fn draw_file_list(frame: &mut Frame, app: &mut App, area: Rect) {
    app.file_panel_mode_toggle_hit = None;
    app.file_panel_root_hit = None;
    app.comments_sidebar_sync_hit = None;
    app.comments_sidebar_discard_hit = None;
    app.comments_sidebar_overflow_hit = None;
    app.comments_sidebar_overflow_hits.clear();
    let panel_bg = app.theme.background_panel.or(app.theme.background);
    let content_area = file_panel_content_area(app, area);
    draw_file_panel_divider(frame, app, area, panel_bg);

    let show_filter = true;
    let panel_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if show_filter {
            vec![
                Constraint::Length(5), // Header
                Constraint::Min(0),    // List
                Constraint::Length(3), // Filter
                Constraint::Length(2), // Action slot
            ]
        } else {
            vec![
                Constraint::Length(5), // Header
                Constraint::Min(0),    // List
            ]
        })
        .split(content_area);

    let header_area = panel_chunks[0];
    let list_area = panel_chunks[1];
    let filter_area = if show_filter {
        Some(panel_chunks[2])
    } else {
        None
    };
    let action_area = if show_filter {
        Some(panel_chunks[3])
    } else {
        None
    };

    let outdated_files = app.outdated_live_files().map(<[_]>::to_vec);
    let files = outdated_files.as_deref().unwrap_or(&app.multi_diff.files);
    let selected_file = app
        .outdated_live_selected_index()
        .unwrap_or(app.multi_diff.selected_index);

    let mut added = 0usize;
    let mut modified = 0usize;
    let mut deleted = 0usize;
    let mut renamed = 0usize;

    for file in files {
        match file.status {
            FileStatus::Added | FileStatus::Untracked => added += 1,
            FileStatus::Deleted => deleted += 1,
            FileStatus::Modified => modified += 1,
            FileStatus::Renamed => renamed += 1,
        }
    }

    let via_text = if app.multi_diff.is_git_mode() {
        "via git"
    } else {
        "via diff"
    };
    let root_path = app
        .multi_diff
        .repo_root()
        .and_then(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| ".".to_string());
    let header_max_width = header_area
        .width
        .saturating_sub(1 + FILE_PANEL_HEADER_RIGHT_PADDING) as usize;
    let range_display = app.multi_diff.git_range_display();
    let header_text = if let Some((from, to)) = range_display {
        let range_text = format!("{from}..{to}");
        let range_width = text_width(&range_text);
        if header_max_width <= range_width {
            truncate_text(&range_text, header_max_width)
        } else {
            let sep = " › ";
            let sep_width = text_width(sep);
            let root_max_width = header_max_width.saturating_sub(range_width + sep_width + 2);
            let root_display = truncate_path(&root_path, root_max_width);
            if root_display.is_empty() {
                truncate_text(&range_text, header_max_width)
            } else {
                format!("{root_display}{sep}{range_text}")
            }
        }
    } else {
        let root_label = "Root ";
        let root_max_width = header_area
            .width
            .saturating_sub((root_label.len() + 1) as u16 + FILE_PANEL_HEADER_RIGHT_PADDING)
            as usize;
        format!(
            "{}{}",
            root_label,
            truncate_path(&root_path, root_max_width)
        )
    };

    let root_text_width = text_width(&header_text).min(
        header_area
            .width
            .saturating_sub(1 + FILE_PANEL_HEADER_RIGHT_PADDING) as usize,
    );
    if root_text_width > 0 {
        app.file_panel_root_hit = Some((
            header_area.x.saturating_add(1),
            header_area.y.saturating_add(1),
            root_text_width as u16,
            1,
        ));
    }
    let root_style = if app.file_panel_root_hover {
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text_muted)
    };
    let files_active = app.file_panel_mode == FilePanelMode::Files;
    let files_segment = if files_active {
        "• files"
    } else if app.files_tab_unseen {
        "* files"
    } else {
        "  files"
    };
    let comments_segment = if files_active {
        if app.comments_tab_unseen {
            "* comments"
        } else {
            "  comments"
        }
    } else {
        "• comments"
    };
    let files_width = text_width(files_segment);
    let gap_width = 3usize;
    let inactive_x_offset = if files_active {
        1 + files_width + gap_width
    } else {
        1
    };
    let inactive_width = if files_active {
        text_width(comments_segment)
    } else {
        text_width(files_segment)
    };
    if inactive_width > 0 && inactive_x_offset < header_area.width as usize {
        app.file_panel_mode_toggle_hit = Some((
            header_area.x.saturating_add(inactive_x_offset as u16),
            header_area.y.saturating_add(3),
            inactive_width.min(header_area.width as usize - inactive_x_offset) as u16,
            1,
        ));
    }
    let active_segment_style = Style::default()
        .fg(app.theme.text)
        .add_modifier(Modifier::BOLD);
    let mut inactive_segment_style = Style::default().fg(if app.file_panel_mode_toggle_hover {
        app.theme.accent
    } else {
        app.theme.text_muted
    });
    if app.file_panel_mode_toggle_hover {
        inactive_segment_style = inactive_segment_style.add_modifier(Modifier::BOLD);
    }
    let files_style = if files_active {
        active_segment_style
    } else {
        inactive_segment_style
    };
    let comments_style = if files_active {
        inactive_segment_style
    } else {
        active_segment_style
    };
    let mut tabs = vec![Span::raw(" ")];
    if !files_active && app.files_tab_unseen {
        tabs.push(Span::styled("*", Style::default().fg(app.theme.warning)));
        tabs.push(Span::styled(" files", files_style));
    } else {
        tabs.push(Span::styled(files_segment, files_style));
    }
    tabs.push(Span::raw("   "));
    if files_active && app.comments_tab_unseen {
        tabs.push(Span::styled("*", Style::default().fg(app.theme.warning)));
        tabs.push(Span::styled(" comments", comments_style));
    } else {
        tabs.push(Span::styled(comments_segment, comments_style));
    }
    let tabs_line = Line::from(tabs);
    let header_lines = if app.file_panel_mode == FilePanelMode::Comments {
        let comment_count = app.review_comment_count();
        let comments_label = match comment_count {
            1 => "1 comment".to_string(),
            n => format!("{n} comments"),
        };
        vec![
            Line::raw(""),
            Line::from(vec![Span::raw(" "), Span::styled(header_text, root_style)]),
            Line::raw(""),
            tabs_line,
            Line::from(vec![
                Span::raw(" "),
                Span::styled(comments_label, Style::default().fg(app.theme.text_muted)),
            ]),
        ]
    } else {
        vec![
            Line::raw(""),
            Line::from(vec![Span::raw(" "), Span::styled(header_text, root_style)]),
            Line::raw(""),
            tabs_line,
            Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("+{}", added),
                    Style::default().fg(app.theme.success),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("~{}", modified),
                    Style::default().fg(app.theme.warning),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("-{}", deleted),
                    Style::default().fg(app.theme.error),
                ),
                Span::raw(" "),
                Span::styled(format!("→{}", renamed), Style::default().fg(app.theme.info)),
                Span::raw(" "),
                Span::styled(via_text, Style::default().fg(app.theme.text_muted)),
            ]),
        ]
    };

    let mut header = Paragraph::new(header_lines);
    if let Some(bg) = panel_bg {
        header = header.style(Style::default().bg(bg));
    }
    frame.render_widget(header, header_area);
    if app.file_panel_mode == FilePanelMode::Comments {
        draw_comment_list(frame, app, list_area, action_area, filter_area, panel_bg);
        return;
    }
    if let Some(action_area) = action_area {
        let mut blank = Block::default();
        if let Some(bg) = panel_bg {
            blank = blank.style(Style::default().bg(bg));
        }
        frame.render_widget(blank, action_area);
    }

    let filtered_indices = app.filtered_file_indices();
    let total_file_rows = app.file_list_total_rows(&filtered_indices);
    let visible_file_rows = list_area.height.saturating_sub(2) as usize;
    let show_file_scrollbar = app.scrollbar_visible
        && total_file_rows > visible_file_rows
        && visible_file_rows > 0
        && list_area.width > 1;
    let (list_content_area, file_scrollbar_area) =
        reserve_file_scrollbar_lane(list_area, show_file_scrollbar);
    let mut items = Vec::new();
    let mut row_map: Vec<Option<usize>> = Vec::new();
    let mut rendered_rows = 0usize;
    let mut row_index = 0usize;
    let row_offset = app
        .file_list_scroll
        .min(total_file_rows.saturating_sub(visible_file_rows));
    app.file_list_scroll = row_offset;
    let mut current_group: Option<String> = None;

    let mut idx = 0usize;
    while idx < filtered_indices.len() && rendered_rows < visible_file_rows {
        let file_idx = filtered_indices[idx];
        let file = &files[file_idx];
        let group = app.file_list_group(file_idx);

        if current_group.as_deref() != Some(&group) {
            if current_group.is_some() {
                if row_index >= row_offset {
                    items.push(ListItem::new(Line::raw("")));
                    row_map.push(None);
                    rendered_rows += 1;
                }
                row_index += 1;
                if rendered_rows == visible_file_rows {
                    break;
                }
            }
            let header_max = list_content_area.width.saturating_sub(6).max(1) as usize;
            let header_text = truncate_path(&group, header_max);
            let header_line = Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    header_text,
                    Style::default()
                        .fg(app.theme.text_muted)
                        .add_modifier(Modifier::DIM),
                ),
            ]);
            if row_index >= row_offset {
                items.push(ListItem::new(header_line));
                row_map.push(None);
                rendered_rows += 1;
            }
            row_index += 1;
            current_group = Some(group);
            if rendered_rows == visible_file_rows {
                break;
            }
        }

        let status_style = match file.status {
            FileStatus::Added | FileStatus::Untracked => Style::default().fg(app.theme.success),
            FileStatus::Deleted => Style::default().fg(app.theme.error),
            FileStatus::Modified => Style::default().fg(app.theme.warning),
            FileStatus::Renamed => Style::default().fg(app.theme.info),
        };

        let is_selected = file_idx == selected_file;
        let is_hovered = app.file_list_hover == Some(file_idx);
        let selected_bg = if is_selected {
            if app.file_list_focused {
                app.theme.background_element.or(app.theme.background_panel)
            } else {
                app.theme.background_panel
            }
        } else {
            None
        };

        let show_for_row = match app.file_count_mode {
            crate::config::FileCountMode::Active => is_selected,
            crate::config::FileCountMode::Focused => app.file_list_focused,
            crate::config::FileCountMode::All => true,
            crate::config::FileCountMode::Off => false,
        };
        let show_signs = show_for_row && (file.binary || file.insertions > 0 || file.deletions > 0);
        let insert_text = if show_signs && !file.binary {
            format!("+{}", file.insertions)
        } else {
            String::new()
        };
        let delete_text = if show_signs && !file.binary {
            format!("-{}", file.deletions)
        } else {
            String::new()
        };
        let comment_count = app.review_comment_count_for_file(file_idx);
        let show_comment_count = show_for_row && comment_count > 0;
        let comment_text = if show_comment_count {
            format!("*{comment_count}")
        } else {
            String::new()
        };
        let signs_len = if show_comment_count {
            1 + comment_text.len()
        } else {
            0
        } + if show_signs {
            if file.binary {
                1 + "bin".len()
            } else {
                1 + insert_text.len() + 1 + delete_text.len()
            }
        } else {
            0
        };

        let file_changed = app.file_changed_on_disk(file_idx);
        let changed_marker_len = if file_changed { 2 } else { 0 };

        // Truncate filename to fit (preserve extension)
        let file_name = file
            .display_name
            .rsplit('/')
            .next()
            .unwrap_or(&file.display_name);
        let max_name_len = list_content_area
            .width
            .saturating_sub(8 + signs_len as u16 + changed_marker_len as u16)
            .max(1) as usize;
        let name = truncate_filename_keep_ext(file_name, max_name_len);

        let mut icon_style = status_style;
        if let Some(bg) = selected_bg {
            icon_style = icon_style.bg(bg);
        }

        let mut name_style = Style::default().fg(if is_selected || is_hovered {
            app.theme.accent
        } else {
            app.theme.text
        });
        if is_selected || is_hovered {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        if let Some(bg) = selected_bg {
            name_style = name_style.bg(bg);
        }

        let marker_style = if is_selected || is_hovered {
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.text_muted)
        };
        let marker = if is_selected { "•" } else { " " };

        let mut line_spans = vec![
            Span::styled(marker, marker_style),
            Span::raw(" "),
            Span::styled("■", icon_style),
            Span::raw(" "),
            Span::styled(name, name_style),
        ];

        if show_comment_count {
            let comment_style = if app.file_list_focused && is_selected {
                Style::default().fg(app.theme.warning)
            } else {
                Style::default().fg(app.theme.text_muted)
            };
            line_spans.push(Span::raw(" "));
            line_spans.push(Span::styled(comment_text, comment_style));
        }

        if show_signs {
            line_spans.push(Span::raw(" "));
            let sign_style = if app.file_list_focused && is_selected {
                Style::default().fg(app.theme.success)
            } else {
                Style::default().fg(app.theme.text_muted)
            };
            let delete_style = if app.file_list_focused && is_selected {
                Style::default().fg(app.theme.error)
            } else {
                Style::default().fg(app.theme.text_muted)
            };
            if file.binary {
                line_spans.push(Span::styled("bin", sign_style));
            } else {
                line_spans.push(Span::styled(insert_text, sign_style));
                line_spans.push(Span::raw(" "));
                line_spans.push(Span::styled(delete_text, delete_style));
            }
        }

        if file_changed {
            let mut changed_style = Style::default()
                .fg(app.theme.warning)
                .add_modifier(Modifier::BOLD);
            if let Some(bg) = selected_bg {
                changed_style = changed_style.bg(bg);
            }
            line_spans.push(Span::raw(" "));
            line_spans.push(Span::styled("*", changed_style));
        }

        let line = Line::from(line_spans);

        if row_index >= row_offset {
            items.push(ListItem::new(line));
            row_map.push(Some(file_idx));
            rendered_rows += 1;
        }
        row_index += 1;
        idx += 1;
    }

    let mut block = Block::default().padding(ratatui::widgets::Padding::new(1, 0, 1, 0));
    if let Some(bg) = panel_bg {
        block = block.style(Style::default().bg(bg));
    }

    let file_list = List::new(items).block(block);

    app.file_list_area = Some((
        list_content_area.x,
        list_content_area.y,
        list_content_area.width,
        list_content_area.height,
    ));
    app.file_list_rows = row_map;

    frame.render_widget(file_list, list_content_area);
    render_file_panel_scrollbar(
        frame,
        app,
        file_scrollbar_area,
        total_file_rows,
        visible_file_rows,
    );

    let has_query = !app.file_filter.is_empty();
    let no_results = has_query && filtered_indices.is_empty();
    if no_results {
        let mut empty = Paragraph::new(Line::from(Span::styled(
            "No Filter Results",
            Style::default().fg(app.theme.text_muted),
        )))
        .alignment(Alignment::Center)
        .block(Block::default().padding(ratatui::widgets::Padding::new(0, 0, 1, 0)));
        if let Some(bg) = panel_bg {
            empty = empty.style(Style::default().bg(bg));
        }
        frame.render_widget(empty, list_content_area);
    }

    if let Some(filter_area) = filter_area {
        app.file_filter_area = Some((
            filter_area.x,
            filter_area.y,
            filter_area.width,
            filter_area.height,
        ));
        let filter_bg = panel_bg;
        let mut filter = Paragraph::new(file_filter_line(app, has_query, filter_area.width))
            .alignment(Alignment::Left);
        let mut filter_block = Block::default().padding(ratatui::widgets::Padding::new(1, 0, 1, 0));
        if let Some(bg) = filter_bg {
            filter_block = filter_block.style(Style::default().bg(bg));
        }
        filter = filter.block(filter_block);
        frame.render_widget(filter, filter_area);
        if has_query && filter_area.width > 2 {
            let clear_x = filter_area
                .x
                .saturating_add(filter_area.width.saturating_sub(2));
            let clear_y = filter_area.y.saturating_add(1);
            app.file_filter_clear_hit = Some((clear_x, clear_y, 1, 1));
            let clear_style = if app.file_filter_clear_hover {
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.text_muted)
            };
            if let Some(cell) = frame.buffer_mut().cell_mut((clear_x, clear_y)) {
                cell.set_symbol("×").set_style(clear_style);
            }
        } else {
            app.file_filter_clear_hit = None;
            app.file_filter_clear_hover = false;
        }
    } else {
        app.file_filter_area = None;
        app.file_filter_clear_hit = None;
        app.file_filter_hover = false;
        app.file_filter_clear_hover = false;
    }
}

fn draw_comment_list(
    frame: &mut Frame,
    app: &mut App,
    list_area: Rect,
    action_area: Option<Rect>,
    filter_area: Option<Rect>,
    panel_bg: Option<Color>,
) {
    enum CommentRow {
        Group(String),
        Spacer,
        ItemTitle(usize),
        ItemPreview(usize),
    }

    let mut indices = app.filtered_review_comment_indices();
    indices.sort_by(|a, b| {
        app.review_comment_sidebar_sort_key(*b)
            .cmp(&app.review_comment_sidebar_sort_key(*a))
    });

    let mut rows = Vec::new();
    let mut current_group: Option<String> = None;
    for comment_idx in indices.iter().copied() {
        let Some(group) = app.review_comment_sidebar_bucket(comment_idx) else {
            continue;
        };
        if current_group.as_deref() != Some(group.as_str()) {
            if current_group.is_some() {
                rows.push(CommentRow::Spacer);
            }
            rows.push(CommentRow::Group(group.clone()));
            current_group = Some(group);
        }
        rows.push(CommentRow::ItemTitle(comment_idx));
        rows.push(CommentRow::ItemPreview(comment_idx));
    }

    let total_rows = rows.len();
    let visible_rows = list_area.height.saturating_sub(2) as usize;
    let show_scrollbar = app.scrollbar_visible
        && total_rows > visible_rows
        && visible_rows > 0
        && list_area.width > 1;
    let (list_content_area, scrollbar_area) =
        reserve_file_scrollbar_lane(list_area, show_scrollbar);
    let row_offset = app
        .file_list_scroll
        .min(total_rows.saturating_sub(visible_rows));
    app.file_list_scroll = row_offset;

    let row_width = list_content_area
        .width
        .saturating_sub(COMMENT_LIST_LEFT_PADDING + COMMENT_LIST_RIGHT_PADDING)
        as usize;
    let mut items = Vec::new();
    let mut row_map = Vec::new();
    for row in rows.iter().skip(row_offset).take(visible_rows) {
        match row {
            CommentRow::Group(group) => {
                let header_max = row_width.saturating_sub(2).max(1);
                items.push(ListItem::new(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        truncate_to_width(group, header_max),
                        Style::default()
                            .fg(app.theme.text_muted)
                            .add_modifier(Modifier::DIM),
                    ),
                ])));
                row_map.push(None);
            }
            CommentRow::Spacer => {
                items.push(ListItem::new(Line::raw("")));
                row_map.push(None);
            }
            CommentRow::ItemTitle(comment_idx) => {
                let Some((file_idx, path, mut location, _preview, outdated, resolved)) =
                    app.review_comment_sidebar_item(*comment_idx)
                else {
                    continue;
                };
                if outdated {
                    location = if location.is_empty() {
                        "Outdated".to_string()
                    } else {
                        format!("{location} Outdated")
                    };
                }
                let is_active = app.review_comment_is_active(*comment_idx);
                let is_hovered = app.file_list_hover == Some(*comment_idx);
                let selected_bg = if is_active {
                    if app.file_list_focused {
                        app.theme.background_element.or(app.theme.background_panel)
                    } else {
                        app.theme.background_panel
                    }
                } else {
                    None
                };

                let mut marker_style = if is_active || is_hovered {
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app.theme.text_muted)
                };
                let icon_color = if location.is_empty() {
                    app.theme.text
                } else {
                    app.multi_diff
                        .files
                        .get(file_idx)
                        .map(|file| match file.status {
                            FileStatus::Added | FileStatus::Untracked => app.theme.success,
                            FileStatus::Deleted => app.theme.error,
                            FileStatus::Modified => app.theme.warning,
                            FileStatus::Renamed => app.theme.info,
                        })
                        .unwrap_or(app.theme.warning)
                };
                let mut icon_style = Style::default().fg(icon_color);
                let mut name_style = Style::default().fg(if is_active || is_hovered {
                    app.theme.accent
                } else if outdated || resolved {
                    app.theme.text_muted
                } else {
                    app.theme.text
                });
                if is_active || is_hovered {
                    name_style = name_style.add_modifier(Modifier::BOLD);
                }
                if outdated || resolved {
                    icon_style = icon_style.add_modifier(Modifier::DIM);
                    name_style = name_style.add_modifier(Modifier::DIM);
                }
                let mut location_style = if outdated {
                    Style::default()
                        .fg(app.theme.warning)
                        .add_modifier(Modifier::DIM)
                } else if is_active && app.file_list_focused {
                    Style::default().fg(app.theme.warning)
                } else {
                    Style::default().fg(app.theme.text_muted)
                };
                if let Some(bg) = selected_bg {
                    marker_style = marker_style.bg(bg);
                    icon_style = icon_style.bg(bg);
                    name_style = name_style.bg(bg);
                    location_style = location_style.bg(bg);
                }

                let width = row_width;
                let location = truncate_with_dots(&location, width.saturating_div(3).min(12));
                let suffix_width = if location.is_empty() {
                    0
                } else {
                    1 + UnicodeWidthStr::width(location.as_str())
                };
                let name_width = width.saturating_sub(4 + suffix_width).max(1);
                let name = truncate_with_dots(&path, name_width);
                let marker = if is_active { "•" } else { " " };

                let mut spans = vec![
                    Span::styled(marker, marker_style),
                    Span::raw(" "),
                    Span::styled("■", icon_style),
                    Span::raw(" "),
                    Span::styled(name, name_style),
                ];
                if !location.is_empty() {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(location, location_style));
                }
                let mut item = ListItem::new(Line::from(spans));
                if let Some(bg) = selected_bg {
                    item = item.style(Style::default().bg(bg));
                }
                items.push(item);
                row_map.push(Some(*comment_idx));
            }
            CommentRow::ItemPreview(comment_idx) => {
                let Some((_file_idx, _path, _location, preview, outdated, resolved)) =
                    app.review_comment_sidebar_item(*comment_idx)
                else {
                    continue;
                };
                let is_active = app.review_comment_is_active(*comment_idx);
                let is_hovered = app.file_list_hover == Some(*comment_idx);
                let selected_bg = if is_active {
                    if app.file_list_focused {
                        app.theme.background_element.or(app.theme.background_panel)
                    } else {
                        app.theme.background_panel
                    }
                } else {
                    None
                };
                let mut preview_style = Style::default().fg(if is_active || is_hovered {
                    app.theme.text
                } else {
                    app.theme.text_muted
                });
                if outdated || resolved {
                    preview_style = preview_style.add_modifier(Modifier::DIM);
                }
                if let Some(bg) = selected_bg {
                    preview_style = preview_style.bg(bg);
                }
                let width = row_width.saturating_sub(4);
                let preview = truncate_with_dots(&preview, width);
                let mut item = ListItem::new(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(preview, preview_style),
                ]));
                if let Some(bg) = selected_bg {
                    item = item.style(Style::default().bg(bg));
                }
                items.push(item);
                row_map.push(Some(*comment_idx));
            }
        }
    }

    let mut block = Block::default().padding(ratatui::widgets::Padding::new(
        COMMENT_LIST_LEFT_PADDING,
        COMMENT_LIST_RIGHT_PADDING,
        1,
        0,
    ));
    if let Some(bg) = panel_bg {
        block = block.style(Style::default().bg(bg));
    }
    app.file_list_area = Some((
        list_content_area.x,
        list_content_area.y,
        list_content_area.width,
        list_content_area.height,
    ));
    app.file_list_rows = row_map;
    frame.render_widget(List::new(items).block(block), list_content_area);
    render_file_panel_scrollbar(frame, app, scrollbar_area, total_rows, visible_rows);

    if total_rows == 0 {
        let text = if app.file_filter.is_empty() {
            "No Comments"
        } else {
            "No Filter Results"
        };
        let mut empty = Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(app.theme.text_muted),
        )))
        .alignment(Alignment::Center)
        .block(Block::default().padding(ratatui::widgets::Padding::new(0, 0, 1, 0)));
        if let Some(bg) = panel_bg {
            empty = empty.style(Style::default().bg(bg));
        }
        frame.render_widget(empty, list_content_area);
    }

    if let Some(action_area) = action_area {
        let sync_label = "s sync";
        let discard_label = "d discard";
        let overflow_label = COMMENTS_OVERFLOW_LABEL;
        if let Some(bg) = panel_bg {
            frame.render_widget(
                Paragraph::new("").style(Style::default().bg(bg)),
                action_area,
            );
        }
        let action_inner_width = action_area
            .width
            .saturating_sub(COMMENT_ACTION_LEFT_PADDING + COMMENT_ACTION_RIGHT_PADDING);
        let action_inner = Rect::new(
            action_area.x.saturating_add(COMMENT_ACTION_LEFT_PADDING),
            action_area.y,
            action_inner_width,
            1,
        );
        let sync_x = action_inner.x;
        let discard_x = sync_x
            .saturating_add(text_width(sync_label) as u16)
            .saturating_add(3);
        app.comments_sidebar_sync_hit =
            Some((sync_x, action_area.y, text_width(sync_label) as u16, 1));
        app.comments_sidebar_discard_hit = Some((
            discard_x,
            action_area.y,
            text_width(discard_label) as u16,
            1,
        ));
        let overflow_width = text_width(overflow_label) as u16;
        let overflow_x = action_inner
            .x
            .saturating_add(action_inner.width.saturating_sub(overflow_width));
        app.comments_sidebar_overflow_hit = Some((overflow_x, action_area.y, overflow_width, 1));
        let key_style = Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD);
        let sync_style = Style::default().fg(if app.comments_sidebar_sync_hover {
            app.theme.accent
        } else {
            app.theme.text
        });
        let discard_hover = app.comments_sidebar_discard_hover && app.review_session_has_changes();
        let discard_key_style = if discard_hover {
            Style::default()
                .fg(app.theme.error)
                .add_modifier(Modifier::BOLD)
        } else {
            key_style
        };
        let mut discard_style = Style::default().fg(if app.review_session_has_changes() {
            app.theme.text
        } else {
            app.theme.text_muted
        });
        if discard_hover {
            discard_style = discard_style
                .fg(app.theme.error)
                .add_modifier(Modifier::BOLD);
        }
        let overflow_style = Style::default().fg(
            if app.comments_sidebar_overflow_hover || app.comments_sidebar_overflow_open {
                app.theme.accent
            } else {
                app.theme.text_muted
            },
        );
        let mut line = Paragraph::new(Line::from(vec![
            Span::styled("s", key_style),
            Span::raw(" "),
            Span::styled("sync", sync_style),
            Span::raw("   "),
            Span::styled("d", discard_key_style),
            Span::raw(" "),
            Span::styled("discard", discard_style),
        ]));
        if let Some(bg) = panel_bg {
            line = line.style(Style::default().bg(bg));
        }
        frame.render_widget(line, action_inner);
        let mut overflow = Paragraph::new(Line::from(Span::styled(overflow_label, overflow_style)));
        if let Some(bg) = panel_bg {
            overflow = overflow.style(Style::default().bg(bg));
        }
        frame.render_widget(
            overflow,
            Rect::new(overflow_x, action_area.y, overflow_width, 1),
        );
        if app.comments_sidebar_overflow_open {
            draw_comments_sidebar_overflow_menu(frame, app, action_area, panel_bg);
        }
    }

    if let Some(filter_area) = filter_area {
        app.file_filter_area = Some((
            filter_area.x,
            filter_area.y,
            filter_area.width,
            filter_area.height,
        ));
        let filter_bg = panel_bg;
        let mut filter = Paragraph::new(file_filter_line(
            app,
            !app.file_filter.is_empty(),
            filter_area.width,
        ))
        .alignment(Alignment::Left);
        let mut filter_block = Block::default().padding(ratatui::widgets::Padding::new(1, 0, 1, 0));
        if let Some(bg) = filter_bg {
            filter_block = filter_block.style(Style::default().bg(bg));
        }
        filter = filter.block(filter_block);
        frame.render_widget(filter, filter_area);
        if !app.file_filter.is_empty() && filter_area.width > 2 {
            let clear_x = filter_area
                .x
                .saturating_add(filter_area.width.saturating_sub(2));
            let clear_y = filter_area.y.saturating_add(1);
            app.file_filter_clear_hit = Some((clear_x, clear_y, 1, 1));
            let clear_style = if app.file_filter_clear_hover {
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.text_muted)
            };
            if let Some(cell) = frame.buffer_mut().cell_mut((clear_x, clear_y)) {
                cell.set_symbol("×").set_style(clear_style);
            }
        } else {
            app.file_filter_clear_hit = None;
            app.file_filter_clear_hover = false;
        }
    }
}

fn draw_comments_sidebar_overflow_menu(
    frame: &mut Frame,
    app: &mut App,
    action_area: Rect,
    panel_bg: Option<Color>,
) {
    let rows = [
        (ReviewSyncAction::Pull, "pull"),
        (ReviewSyncAction::Push, "push"),
    ];
    let width = 10u16.min(action_area.width.max(1));
    let height = rows.len() as u16 + 2;
    let x = action_area.x.saturating_add(
        action_area
            .width
            .saturating_sub(COMMENT_ACTION_RIGHT_PADDING + width),
    );
    let y = action_area.y.saturating_sub(height);
    let menu_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, menu_area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = panel_bg {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), menu_area);

    let inner = block.inner(menu_area);
    let menu_bg = panel_bg;
    let hover_bg = app
        .theme
        .background_element
        .or_else(|| menu_bg.and_then(|bg| color::blend_colors(bg, app.theme.accent, 0.16)))
        .or(menu_bg);
    let mut hits = Vec::new();
    for (idx, (action, label)) in rows.iter().enumerate() {
        let row = inner.y.saturating_add(idx as u16);
        let hover = app.comments_sidebar_overflow_menu_hover == Some(*action);
        let mut text_style = Style::default().fg(if hover {
            app.theme.accent
        } else {
            app.theme.text
        });
        if hover {
            text_style = text_style.add_modifier(Modifier::BOLD);
        }
        let mut row_style = Style::default();
        if let Some(bg) = if hover { hover_bg } else { menu_bg } {
            row_style = row_style.bg(bg);
        }
        hits.push(ReviewSidebarOverflowHit {
            x: inner.x,
            y: row,
            width: inner.width,
            height: 1,
            action: *action,
        });
        let label = format!(" {label}");
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_text(&label, inner.width as usize),
                text_style,
            )))
            .style(row_style),
            Rect::new(inner.x, row, inner.width, 1),
        );
    }
    app.set_comments_sidebar_overflow_hits(hits);
}

fn file_filter_line(app: &App, has_query: bool, width: u16) -> Line<'static> {
    if app.file_filter_active {
        let prompt_style = Style::default()
            .fg(app.theme.primary)
            .add_modifier(Modifier::BOLD);
        let query_width = if has_query {
            width.saturating_sub(7)
        } else {
            width.saturating_sub(5)
        };
        let query = truncate_text_from_start(&app.file_filter, query_width as usize);
        return Line::from(vec![
            Span::raw(" "),
            Span::styled("❯ ", prompt_style),
            Span::styled(query, Style::default().fg(app.theme.text)),
            Span::styled(
                if app.file_filter_cursor_visible {
                    "│"
                } else {
                    " "
                },
                prompt_style,
            ),
        ]);
    }

    if has_query {
        let prompt_style = Style::default()
            .fg(app.theme.primary)
            .add_modifier(Modifier::BOLD);
        let query_style = if app.file_filter_hover {
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.text)
        };
        return Line::from(vec![
            Span::raw(" "),
            Span::styled("❯ ", prompt_style),
            Span::styled(
                truncate_text_from_start(&app.file_filter, width.saturating_sub(6) as usize),
                query_style,
            ),
        ]);
    }

    let style = if app.file_filter_hover {
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text_muted)
    };
    let text = format!(
        "{} Filter",
        app.keybindings
            .normal_keys(NormalAction::OpenSearchOrFileFilter)
    );
    Line::from(vec![Span::raw(" "), Span::styled(text, style)])
}

#[derive(Clone, Copy)]
struct PreviewChangeBar {
    kind: LineKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
}

struct PreviewFileCommentMeta {
    action_width: usize,
    comment_id: Option<u64>,
    card_start: Option<usize>,
    card_height: usize,
    anchor_key: Option<String>,
    actions: ReviewNoteActionHits,
}

fn review_file_comment_action_line(app: &App) -> Option<(Line<'static>, usize)> {
    if !app.file_review_comments_supported() || app.review_editor_active() {
        return None;
    }
    let key = app.keybindings.normal_keys(NormalAction::LineComment);
    let label = "comment";
    let width = text_width(&key)
        .saturating_add(1)
        .saturating_add(text_width(label));
    let key_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let label_style = if app.review_file_comment_hover {
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text_muted)
    };
    Some((
        Line::from(vec![
            Span::styled(key, key_style),
            Span::raw(" "),
            Span::styled(label.to_string(), label_style),
        ]),
        width,
    ))
}

fn prepend_review_file_comment_lines(
    app: &mut App,
    lines: &mut Vec<Line<'static>>,
    max_width: usize,
) -> Option<PreviewFileCommentMeta> {
    let (action, action_width) = review_file_comment_action_line(app)?;
    let mut prefix = vec![action, Line::from("")];
    let mut card_start = None;
    let mut card_height = 0usize;
    let mut comment_id = None;
    let mut anchor_key = None;
    let mut actions = ReviewNoteActionHits::default();

    if let Some(overlay) = app.review_file_comment_overlay() {
        let start = prefix.len();
        let key = overlay.anchor_key.clone();
        let note_lines = review_note_lines(app, &overlay, max_width);
        card_height = note_lines.len();
        comment_id = Some(overlay.id);
        actions = review_preview_row(start, card_height, key.clone(), &overlay).actions;
        card_start = Some(start);
        anchor_key = Some(key);
        prefix.extend(note_lines);
        prefix.push(Line::from(""));
    }

    prefix.append(lines);
    *lines = prefix;

    Some(PreviewFileCommentMeta {
        action_width,
        comment_id,
        card_start,
        card_height,
        anchor_key,
        actions,
    })
}

fn add_review_file_comment_hits(
    app: &mut App,
    content_area: Rect,
    visible_lines: usize,
    scroll: usize,
    meta: Option<&PreviewFileCommentMeta>,
) {
    let Some(meta) = meta else {
        return;
    };
    let content_w = content_area.width as usize;
    if scroll == 0 && visible_lines > 0 {
        app.set_review_file_comment_hit(Some((
            content_area.x,
            content_area.y,
            meta.action_width.min(content_w) as u16,
            1,
        )));
    }
    if let (Some(card_start), Some(comment_id), Some(anchor_key)) =
        (meta.card_start, meta.comment_id, meta.anchor_key.as_ref())
    {
        let card_end = card_start.saturating_add(meta.card_height);
        let visible_start = card_start.max(scroll);
        let visible_end = card_end.min(scroll.saturating_add(visible_lines));
        if visible_start < visible_end {
            app.add_review_comment_preview_box(
                content_area.x,
                content_area
                    .y
                    .saturating_add((visible_start - scroll) as u16),
                content_area.width,
                (visible_end - visible_start) as u16,
                comment_id,
                anchor_key.clone(),
            );
        }
        if let Some((offset, x, width)) = meta.actions.edit {
            let action_row = card_start.saturating_add(offset);
            if action_row >= scroll && action_row < scroll.saturating_add(visible_lines) {
                app.add_review_preview_edit_box(
                    content_area.x.saturating_add(x),
                    content_area.y.saturating_add((action_row - scroll) as u16),
                    width,
                    1,
                    comment_id,
                    anchor_key.clone(),
                );
            }
        }
        if let Some((offset, x, width)) = meta.actions.reply {
            let action_row = card_start.saturating_add(offset);
            if action_row >= scroll && action_row < scroll.saturating_add(visible_lines) {
                app.add_review_preview_reply_box(
                    content_area.x.saturating_add(x),
                    content_area.y.saturating_add((action_row - scroll) as u16),
                    width,
                    1,
                    comment_id,
                    anchor_key.clone(),
                );
            }
        }
        if let Some((offset, x, width)) = meta.actions.resolve {
            let action_row = card_start.saturating_add(offset);
            if action_row >= scroll && action_row < scroll.saturating_add(visible_lines) {
                app.add_review_preview_resolve_box(
                    content_area.x.saturating_add(x),
                    content_area.y.saturating_add((action_row - scroll) as u16),
                    width,
                    1,
                    comment_id,
                    anchor_key.clone(),
                );
            }
        }
        if let Some((offset, x, width)) = meta.actions.delete {
            let action_row = card_start.saturating_add(offset);
            if action_row >= scroll && action_row < scroll.saturating_add(visible_lines) {
                app.add_review_preview_delete_box(
                    content_area.x.saturating_add(x),
                    content_area.y.saturating_add((action_row - scroll) as u16),
                    width,
                    1,
                    comment_id,
                    anchor_key.clone(),
                );
            }
        }
        if let Some((offset, x, width)) = meta.actions.overflow {
            let action_row = card_start.saturating_add(offset);
            if action_row >= scroll && action_row < scroll.saturating_add(visible_lines) {
                app.add_review_preview_overflow_box(
                    content_area.x.saturating_add(x),
                    content_area.y.saturating_add((action_row - scroll) as u16),
                    width,
                    1,
                    comment_id,
                    anchor_key.clone(),
                );
            }
        }
    }
}

fn render_terminal_image_preview(
    frame: &mut Frame,
    app: &mut App,
    content_area: Rect,
    path: &Path,
) -> bool {
    let mut prefix_lines = Vec::new();
    let meta =
        prepend_review_file_comment_lines(app, &mut prefix_lines, content_area.width as usize);
    let prefix_height = (prefix_lines.len() as u16).min(content_area.height);
    let image_area = Rect {
        y: content_area.y.saturating_add(prefix_height),
        height: content_area.height.saturating_sub(prefix_height),
        ..content_area
    };
    if image_area.width == 0 || image_area.height == 0 {
        return false;
    }

    let Some(protocol) = app
        .ensure_terminal_image_preview(path, Size::new(image_area.width, image_area.height))
        .cloned()
    else {
        return false;
    };

    app.set_preview_search_lines(preview_search_text_lines(&prefix_lines));
    highlight_preview_search_lines(app, &mut prefix_lines);
    add_review_file_comment_hits(app, content_area, prefix_height as usize, 0, meta.as_ref());
    let bg = app.theme.background;

    if prefix_height > 0 {
        let mut paragraph = Paragraph::new(prefix_lines);
        if let Some(bg) = bg {
            paragraph = paragraph.style(Style::default().bg(bg));
        }
        frame.render_widget(
            paragraph,
            Rect::new(
                content_area.x,
                content_area.y,
                content_area.width,
                prefix_height,
            ),
        );
    }
    frame.render_widget(
        TerminalImage::new(&protocol).allow_clipping(true),
        image_area,
    );
    true
}

fn render_outdated_comments_view(frame: &mut Frame, app: &mut App, area: Rect) {
    if let Some(bg) = app.theme.background {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }
    let (content_area, scrollbar_area) = reserve_diff_scrollbar_lane(app, area);
    app.clear_preview_link_boxes();
    app.clear_review_preview_boxes();
    app.set_pr_comment_hits(Vec::new());
    app.set_pr_comment_add_hit(None);

    let comments = app.outdated_comment_overlays();
    let mut lines = vec![
        Line::from(Span::styled(
            "Outdated comments",
            Style::default()
                .fg(app.theme.text)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    let mut cards = Vec::new();
    let mut avatars = Vec::new();
    let mut focus_rows = Vec::new();

    if comments.is_empty() {
        lines.push(Line::from(Span::styled(
            "No outdated comments.",
            Style::default()
                .fg(app.theme.text_muted)
                .add_modifier(Modifier::DIM),
        )));
        lines.push(Line::from(""));
    }

    for comment in comments {
        let start = lines.len();
        let mut footer_actions = Vec::new();
        if let Some(label) = comment.overlay.edit_label.as_ref() {
            footer_actions.push(format!("{label} edit"));
        }
        if let Some(label) = comment.overlay.resolve_label.as_ref() {
            footer_actions.push(format!(
                "{label} {}",
                if comment.overlay.resolved {
                    "unresolve"
                } else {
                    "resolve"
                }
            ));
        }
        if let Some(label) = comment.overlay.delete_label.as_ref() {
            footer_actions.push(format!("{label} delete"));
        }
        let block = review_note_block_with_footer(
            app,
            &comment.overlay,
            content_area.width as usize,
            &footer_actions.join("   "),
        );
        let height = block.lines.len();
        let footer_row = start.saturating_add(height.saturating_sub(1));
        let snapshot_rows = block.snapshot_rows;
        let anchor_key = comment.overlay.anchor_key.clone();
        let edit_width = review_note_edit_width(&comment.overlay);
        let resolve_x = review_note_resolve_x_offset(&comment.overlay);
        let resolve_width = review_note_resolve_width(&comment.overlay);
        let delete_x = review_note_delete_x_offset(&comment.overlay);
        let delete_width = review_note_delete_width(&comment.overlay);
        if let Some(avatar) = block.avatar.clone() {
            avatars.push((start.saturating_add(avatar.row_offset), avatar));
        }
        lines.extend(block.lines);
        lines.push(Line::from(""));
        cards.push((
            comment.id,
            start,
            height,
            footer_row,
            snapshot_rows,
            anchor_key,
            edit_width,
            resolve_x,
            resolve_width,
            delete_x,
            delete_width,
        ));
        focus_rows.push((comment.id, start));
    }

    let visible_lines = content_area.height as usize;
    let total_lines = lines.len().max(1);
    if let Some(focus) = app.outdated_comment_focus.take() {
        if let Some((_, row)) = focus_rows.iter().find(|(id, _)| *id == focus) {
            app.scroll_offset = row.saturating_sub(1);
        }
    }
    app.clamp_scroll(total_lines, visible_lines, false);
    let scroll = app.scroll_offset.min(total_lines.saturating_sub(1));
    let viewport_end = scroll.saturating_add(visible_lines);

    for (
        comment_id,
        row,
        height,
        footer_row,
        snapshot_rows,
        anchor_key,
        edit_width,
        resolve_x,
        resolve_width,
        delete_x,
        delete_width,
    ) in cards
    {
        let end = row.saturating_add(height.max(1));
        let visible_start = row.max(scroll);
        let visible_end = end.min(viewport_end);
        if visible_start < visible_end {
            app.add_review_comment_preview_box(
                content_area.x,
                content_area
                    .y
                    .saturating_add(visible_start.saturating_sub(scroll) as u16),
                content_area.width,
                visible_end.saturating_sub(visible_start) as u16,
                comment_id,
                anchor_key.clone(),
            );
        }
        if let Some((snapshot_start, snapshot_end)) = snapshot_rows {
            let snapshot_start = row.saturating_add(snapshot_start).max(scroll);
            let snapshot_end = row.saturating_add(snapshot_end).min(viewport_end);
            if snapshot_start < snapshot_end {
                app.add_review_preview_passive_box(
                    content_area.x.saturating_add(2),
                    content_area
                        .y
                        .saturating_add(snapshot_start.saturating_sub(scroll) as u16),
                    content_area.width.saturating_sub(4),
                    snapshot_end.saturating_sub(snapshot_start) as u16,
                    anchor_key.clone(),
                );
            }
        }
        if footer_row < scroll || footer_row >= viewport_end {
            continue;
        }
        let y = content_area
            .y
            .saturating_add(footer_row.saturating_sub(scroll) as u16);
        if edit_width > 0 {
            app.add_review_preview_edit_box(
                content_area.x.saturating_add(2),
                y,
                edit_width,
                1,
                comment_id,
                anchor_key.clone(),
            );
        }
        if resolve_width > 0 {
            app.add_review_preview_resolve_box(
                content_area.x.saturating_add(resolve_x),
                y,
                resolve_width,
                1,
                comment_id,
                anchor_key.clone(),
            );
        }
        if delete_width > 0 {
            app.add_review_preview_delete_box(
                content_area.x.saturating_add(delete_x),
                y,
                delete_width,
                1,
                comment_id,
                anchor_key,
            );
        }
    }

    let visible = lines
        .into_iter()
        .skip(scroll)
        .take(visible_lines)
        .collect::<Vec<_>>();
    let mut paragraph = Paragraph::new(visible);
    if let Some(bg) = app.theme.background {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, content_area);

    for (row, avatar) in avatars {
        if row < scroll || row >= viewport_end {
            continue;
        }
        render_review_note_avatar(
            frame,
            app,
            content_area,
            row.saturating_sub(scroll),
            &avatar,
        );
    }

    render_diff_scrollbar(
        frame,
        app,
        scrollbar_area,
        total_lines,
        visible_lines,
        app.scroll_offset,
    );
}

fn render_pr_comments_view(frame: &mut Frame, app: &mut App, area: Rect) {
    if let Some(bg) = app.theme.background {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }
    let (content_area, scrollbar_area) = reserve_diff_scrollbar_lane(app, area);
    app.clear_preview_link_boxes();
    app.set_pr_comment_hits(Vec::new());
    app.set_pr_comment_add_hit(None);

    let provider = app.review_provider_kind();
    let review_noun = provider.long_review_noun();
    let title = app.pull_request_title();
    let has_pr_target = app.pull_request_comment_target_available();
    let overlays = app.pull_request_comment_overlays();
    let mut lines = Vec::<Line<'static>>::new();
    let mut hit_rows = Vec::<(usize, u16, u16, PrCommentHitAction)>::new();
    let mut card_rows = Vec::<(u64, usize, usize, String)>::new();
    let mut delete_rows = Vec::<(u64, usize, u16, u16, String)>::new();
    let mut avatar_rows = Vec::new();
    let mut focus_rows = Vec::<(u64, usize)>::new();
    lines.push(Line::from(Span::styled(
        title,
        Style::default()
            .fg(app.theme.text)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    if overlays.is_empty() {
        let message = if has_pr_target {
            format!("No {review_noun} comments.")
        } else {
            format!("No {review_noun} found.")
        };
        lines.push(Line::from(Span::styled(
            message,
            Style::default()
                .fg(app.theme.text_muted)
                .add_modifier(Modifier::DIM),
        )));
        lines.push(Line::from(""));
    }

    for (id, number, overlay) in overlays {
        let start = lines.len();
        let reply_key = app
            .pull_request_reply_label(id)
            .unwrap_or_else(|| format!("r{number}"));
        let reply = format!("{reply_key} reply");
        let mut actions = Vec::<(String, PrCommentHitAction)>::new();
        if overlay.can_edit {
            let edit_key = app
                .pull_request_edit_label(id)
                .unwrap_or_else(|| format!("i{number}"));
            actions.push((format!("{edit_key} edit"), PrCommentHitAction::Edit(id)));
        }
        if has_pr_target {
            actions.push((reply, PrCommentHitAction::Reply(id)));
        }
        if overlay.can_edit {
            let delete_key = app
                .pull_request_delete_label(id)
                .unwrap_or_else(|| format!("x{number}"));
            actions.push((
                format!("{delete_key} delete"),
                PrCommentHitAction::Delete(id),
            ));
        }
        let footer = actions
            .iter()
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>()
            .join("   ");
        let block =
            review_note_block_with_footer(app, &overlay, content_area.width as usize, &footer);
        let card_height = block.lines.len();
        let footer_row = start.saturating_add(card_height.saturating_sub(1));
        let anchor_key = overlay.anchor_key.clone();
        card_rows.push((id, start, card_height, anchor_key.clone()));
        if let Some(avatar) = block.avatar.clone() {
            avatar_rows.push((start.saturating_add(avatar.row_offset), avatar));
        }
        lines.extend(block.lines);
        lines.push(Line::from(""));
        focus_rows.push((id, start));
        if overlay.can_edit {
            hit_rows.push((start, 0, content_area.width, PrCommentHitAction::Open(id)));
        }
        let mut action_x = 2;
        for (idx, (label, action)) in actions.into_iter().enumerate() {
            if idx > 0 {
                action_x += 3;
            }
            let width = text_width(&label) as u16;
            hit_rows.push((footer_row, action_x, width, action));
            if matches!(action, PrCommentHitAction::Delete(_)) {
                delete_rows.push((id, footer_row, action_x, width, anchor_key.clone()));
            }
            action_x += width;
        }
    }

    let add_row = has_pr_target.then_some(lines.len());
    if has_pr_target {
        let add_hover = app.pr_comment_add_hover;
        let add_key = Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD);
        let add_label = if add_hover {
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.text)
        };
        lines.push(Line::from(vec![
            Span::styled("c", add_key),
            Span::styled(" add comment", add_label),
        ]));
    }

    let visible_lines = content_area.height as usize;
    let total_lines = lines.len().max(1);
    if let Some(focus) = app.pr_comment_focus.take() {
        if let Some((_, row)) = focus_rows.iter().find(|(id, _)| *id == focus) {
            app.scroll_offset = row.saturating_sub(1);
        }
    }
    app.clamp_scroll(total_lines, visible_lines, false);
    let scroll = app.scroll_offset.min(total_lines.saturating_sub(1));
    let viewport_end = scroll.saturating_add(visible_lines);

    for (id, row, height, anchor_key) in card_rows {
        let end = row.saturating_add(height.max(1));
        let visible_start = row.max(scroll);
        let visible_end = end.min(viewport_end);
        if visible_start >= visible_end {
            continue;
        }
        app.add_review_comment_preview_box(
            content_area.x,
            content_area
                .y
                .saturating_add(visible_start.saturating_sub(scroll) as u16),
            content_area.width,
            visible_end.saturating_sub(visible_start) as u16,
            id,
            anchor_key,
        );
    }
    for (id, row, x_offset, width, anchor_key) in delete_rows {
        if row < scroll || row >= viewport_end {
            continue;
        }
        app.add_review_preview_delete_box(
            content_area.x.saturating_add(x_offset),
            content_area
                .y
                .saturating_add(row.saturating_sub(scroll) as u16),
            width,
            1,
            id,
            anchor_key,
        );
    }

    let mut hits = Vec::new();
    for (row, x_offset, width, action) in hit_rows {
        if row < scroll || row >= scroll.saturating_add(visible_lines) {
            continue;
        }
        hits.push(PrCommentHit {
            x: content_area.x.saturating_add(x_offset),
            y: content_area
                .y
                .saturating_add(row.saturating_sub(scroll) as u16),
            width,
            height: 1,
            action,
        });
    }
    app.set_pr_comment_hits(hits);
    if let Some(add_row) =
        add_row.filter(|row| *row >= scroll && *row < scroll.saturating_add(visible_lines))
    {
        app.set_pr_comment_add_hit(Some((
            content_area.x,
            content_area
                .y
                .saturating_add(add_row.saturating_sub(scroll) as u16),
            text_width("c add comment") as u16,
            1,
        )));
    }

    let visible = lines
        .into_iter()
        .skip(scroll)
        .take(visible_lines)
        .collect::<Vec<_>>();
    let mut paragraph = Paragraph::new(visible);
    if let Some(bg) = app.theme.background {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, content_area);

    for (row, avatar) in avatar_rows {
        if row < scroll || row >= viewport_end {
            continue;
        }
        render_review_note_avatar(
            frame,
            app,
            content_area,
            row.saturating_sub(scroll),
            &avatar,
        );
    }

    render_diff_scrollbar(
        frame,
        app,
        scrollbar_area,
        total_lines,
        visible_lines,
        app.scroll_offset,
    );
}

fn render_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.active_pr_comments_view() {
        render_pr_comments_view(frame, app, area);
        return;
    }
    if app.active_outdated_comments_view() {
        render_outdated_comments_view(frame, app, area);
        return;
    }
    if let Some(bg) = app.theme.background {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }
    let (content_area, scrollbar_area) = reserve_diff_scrollbar_lane(app, area);
    let (title, text, side, binary, base_dir, image_path) = preview_document(app);
    app.clear_preview_link_boxes();
    if let Some(path) = image_path
        .as_deref()
        .filter(|_| app.active_preview_rendered())
    {
        if render_terminal_image_preview(frame, app, content_area, path) {
            return;
        }
    }
    // Number of leading lines pinned to the top (CSV header and separator).
    let mut sticky_rows = 0usize;
    let (mut lines, links) =
        if let Some(path) = image_path.filter(|_| app.active_preview_rendered()) {
            image_file_preview_lines(
                &path,
                content_area.width as usize,
                content_area.height as usize,
                &app.theme,
            )
        } else if binary {
            (
                vec![
                    Line::from(Span::styled(
                        title,
                        Style::default()
                            .fg(app.theme.text)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Preview is not available for binary files.",
                        Style::default().fg(app.theme.text_muted),
                    )),
                ],
                Vec::new(),
            )
        } else if let Some(kind) =
            preview_structured_kind(app).filter(|_| app.active_preview_rendered())
        {
            let change_bars = structured_preview_change_bars(app, kind, side);
            structured_preview_lines(
                &title,
                &text,
                app,
                content_area.width as usize,
                content_area.height as usize,
                kind,
                change_bars.as_ref(),
            )
        } else if preview_can_render_csv(app) && app.active_preview_rendered() {
            // Pin the header and separator to the top.
            sticky_rows = 2;
            let change_bars = preview_change_bars(app, side);
            csv_preview_lines(
                &title,
                &text,
                app,
                content_area.width as usize,
                change_bars.as_ref(),
            )
        } else if preview_can_render_markdown(app) && app.active_preview_rendered() {
            let change_bars = preview_change_bars(app, side);
            markdown_preview_lines(
                &text,
                app,
                content_area.width as usize,
                base_dir.as_deref(),
                change_bars.as_ref(),
            )
        } else {
            let change_bars = preview_change_bars(app, side);
            (
                source_preview_lines(app, &title, &text, side, change_bars.as_ref()),
                Vec::new(),
            )
        };
    let file_comment_meta =
        prepend_review_file_comment_lines(app, &mut lines, content_area.width as usize);
    app.set_preview_search_lines(preview_search_text_lines(&lines));
    highlight_preview_search_lines(app, &mut lines);
    let visible_lines = content_area.height as usize;

    // Sticky-header path: pin the first `sticky_rows` lines and scroll the body.
    if sticky_rows > 0 && lines.len() > sticky_rows {
        let body = lines.split_off(sticky_rows);
        let header_lines = lines;
        let body_total = body.len();
        let body_h = visible_lines.saturating_sub(sticky_rows).max(1);
        app.clamp_scroll(body_total, body_h, false);
        let scroll = app.scroll_offset.min(body_total.saturating_sub(1));
        let header_area = Rect {
            height: sticky_rows as u16,
            ..content_area
        };
        let body_area = Rect {
            y: content_area.y + sticky_rows as u16,
            height: content_area.height.saturating_sub(sticky_rows as u16),
            ..content_area
        };
        let body_visible = body
            .into_iter()
            .skip(scroll)
            .take(body_h)
            .collect::<Vec<_>>();
        let mut header_par = Paragraph::new(header_lines);
        let mut body_par = Paragraph::new(body_visible);
        if let Some(bg) = app.theme.background {
            header_par = header_par.style(Style::default().bg(bg));
            body_par = body_par.style(Style::default().bg(bg));
        }
        frame.render_widget(header_par, header_area);
        frame.render_widget(body_par, body_area);
        render_diff_scrollbar(
            frame,
            app,
            scrollbar_area,
            body_total,
            body_h,
            app.scroll_offset,
        );
        return;
    }

    let total_lines = lines.len().max(1);
    app.clamp_scroll(total_lines, visible_lines, false);
    let scroll = app.scroll_offset.min(total_lines.saturating_sub(1));

    // Map visible action rows from content coordinates to on-screen click boxes.
    let content_w = content_area.width as usize;
    add_review_file_comment_hits(
        app,
        content_area,
        visible_lines,
        scroll,
        file_comment_meta.as_ref(),
    );

    // Map visible links from content coordinates to on-screen click boxes.
    for link in &links {
        if link.line < scroll || link.line >= scroll + visible_lines || link.col >= content_w {
            continue;
        }
        let screen_x = content_area.x + link.col as u16;
        let screen_y = content_area.y + (link.line - scroll) as u16;
        let width = link.width.min(content_w - link.col) as u16;
        if width > 0 {
            app.add_preview_link_box(screen_x, screen_y, width, link.url.clone());
        }
    }
    let visible = lines
        .into_iter()
        .skip(scroll)
        .take(visible_lines)
        .collect::<Vec<_>>();
    let mut paragraph = Paragraph::new(visible);
    if let Some(bg) = app.theme.background {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, content_area);
    render_diff_scrollbar(
        frame,
        app,
        scrollbar_area,
        total_lines,
        visible_lines,
        app.scroll_offset,
    );
}

fn preview_search_text_lines(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(line_text).collect()
}

fn highlight_preview_search_lines(app: &App, lines: &mut [Line<'static>]) {
    for (idx, line) in lines.iter_mut().enumerate() {
        let text = line_text(line);
        let spans = std::mem::take(&mut line.spans);
        line.spans = app.highlight_search_spans(spans, &text, app.search_target() == Some(idx));
    }
}

fn line_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn preview_document(
    app: &App,
) -> (
    String,
    String,
    Option<SyntaxSide>,
    bool,
    Option<PathBuf>,
    Option<PathBuf>,
) {
    match app.active_topbar_content() {
        Some(TopbarTabContent::Help) => (
            "Help.md".to_string(),
            help_markdown(app),
            None,
            false,
            None,
            None,
        ),
        Some(TopbarTabContent::File(index)) => {
            let Some(file) = app.multi_diff.files.get(index) else {
                return (String::new(), String::new(), None, false, None, None);
            };
            let side = if matches!(file.status, FileStatus::Deleted) {
                SyntaxSide::Old
            } else {
                SyntaxSide::New
            };
            let text = app
                .multi_diff
                .file_contents(index)
                .map(|(old, new)| match side {
                    SyntaxSide::Old => old.to_string(),
                    SyntaxSide::New => new.to_string(),
                })
                .unwrap_or_default();
            let file_side = match side {
                SyntaxSide::Old => FileSide::Old,
                SyntaxSide::New => FileSide::New,
            };
            let source_path = app
                .multi_diff
                .existing_source_path(index, file_side)
                .or_else(|| app.multi_diff.source_path(index, file_side))
                .or_else(|| Some(PathBuf::from(&file.display_name)));
            let base_dir = source_path
                .as_ref()
                .and_then(|path| path.parent().map(Path::to_path_buf));
            let image_path = is_image_name(&file.display_name)
                .then(|| source_path.clone())
                .flatten()
                .filter(|path| path.is_file());
            (
                file.display_name.clone(),
                text,
                Some(side),
                file.binary,
                base_dir,
                image_path,
            )
        }
        Some(TopbarTabContent::PrComments | TopbarTabContent::OutdatedComments) | None => {
            (String::new(), String::new(), None, false, None, None)
        }
    }
}

fn preview_change_bars(app: &mut App, side: Option<SyntaxSide>) -> Option<PreviewChangeBars> {
    if !app.preview_change_bars {
        return None;
    }
    let side = side?;
    let mut raw = HashMap::<usize, PreviewChangeBar>::new();
    {
        let diff = app.multi_diff.current_navigator().diff();
        for change_id in &diff.significant_changes {
            let Some(change) = diff
                .changes
                .get(*change_id)
                .filter(|change| change.id == *change_id)
                .or_else(|| diff.changes.iter().find(|change| change.id == *change_id))
            else {
                continue;
            };
            for span in change.changes() {
                let Some((line, bar)) = preview_change_bar_for_span(side, span) else {
                    continue;
                };
                raw.entry(line)
                    .and_modify(|old| *old = merge_preview_change_bars(*old, bar))
                    .or_insert(bar);
            }
        }
    }
    if raw.is_empty() {
        return None;
    }
    let marker = app.extent_marker.clone();
    let marker_width = text_width(&marker).max(1);
    let styles = raw
        .into_iter()
        .map(|(line, bar)| {
            (
                line,
                crate::views::extent_marker_style(app, bar.kind, true, bar.old_line, bar.new_line),
            )
        })
        .collect();
    Some(PreviewChangeBars {
        marker,
        marker_width,
        styles,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StructuredDiffKind {
    Added,
    Deleted,
    Modified,
}

fn structured_preview_change_bars(
    app: &App,
    kind: StructuredPreviewKind,
    side: Option<SyntaxSide>,
) -> Option<StructuredPreviewChangeBars> {
    if !app.preview_change_bars {
        return None;
    }
    let side = side?;
    let index = match app.active_topbar_content()? {
        TopbarTabContent::File(index) => index,
        TopbarTabContent::Help
        | TopbarTabContent::PrComments
        | TopbarTabContent::OutdatedComments => return None,
    };
    let (old, new) = app.multi_diff.file_contents(index)?;
    let old = parse_structured_change_value(kind, old).ok()?;
    let new = parse_structured_change_value(kind, new).ok()?;
    let mut changed = HashMap::new();
    diff_structured_values("", old.as_ref(), new.as_ref(), &mut changed);

    let styles = changed
        .into_iter()
        .filter_map(|(path, kind)| {
            if !structured_diff_kind_visible(side, kind) {
                return None;
            }
            Some((path, structured_diff_style(app, kind)))
        })
        .collect::<HashMap<_, _>>();
    if styles.is_empty() {
        return None;
    }
    let marker = app.extent_marker.clone();
    Some(StructuredPreviewChangeBars {
        marker_width: text_width(&marker).max(1),
        marker,
        styles,
    })
}

fn parse_structured_change_value(
    kind: StructuredPreviewKind,
    text: &str,
) -> Result<Option<serde_json::Value>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value = match kind {
        StructuredPreviewKind::Json => {
            serde_json::from_str(text).map_err(|error| error.to_string())?
        }
        StructuredPreviewKind::Yaml => parse_yaml_change_value(text)?,
        StructuredPreviewKind::Toml => {
            let value = toml::from_str::<toml::Value>(text).map_err(|error| error.to_string())?;
            serde_json::to_value(value).map_err(|error| error.to_string())?
        }
    };
    Ok(Some(value))
}

fn parse_yaml_change_value(text: &str) -> Result<serde_json::Value, String> {
    let docs = yaml_rust::YamlLoader::load_from_str(text).map_err(|error| error.to_string())?;
    let values = docs
        .into_iter()
        .map(yaml_change_value)
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() == 1 {
        Ok(values.into_iter().next().unwrap())
    } else {
        Ok(serde_json::Value::Array(values))
    }
}

fn yaml_change_value(value: yaml_rust::Yaml) -> Result<serde_json::Value, String> {
    Ok(match value {
        yaml_rust::Yaml::Real(value) => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::String(value)),
        yaml_rust::Yaml::Integer(value) => serde_json::Value::Number(value.into()),
        yaml_rust::Yaml::String(value) => serde_json::Value::String(value),
        yaml_rust::Yaml::Boolean(value) => serde_json::Value::Bool(value),
        yaml_rust::Yaml::Array(values) => serde_json::Value::Array(
            values
                .into_iter()
                .map(yaml_change_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        yaml_rust::Yaml::Hash(values) => {
            let mut out = serde_json::Map::new();
            for (key, value) in values {
                let yaml_rust::Yaml::String(key) = key else {
                    return Err("YAML preview change bars need string keys".to_string());
                };
                out.insert(key, yaml_change_value(value)?);
            }
            serde_json::Value::Object(out)
        }
        yaml_rust::Yaml::Null => serde_json::Value::Null,
        yaml_rust::Yaml::BadValue => return Err("YAML parser returned a bad value".to_string()),
        yaml_rust::Yaml::Alias(_) => {
            return Err("YAML aliases are not supported in preview change bars".to_string())
        }
    })
}

fn diff_structured_values(
    path: &str,
    old: Option<&serde_json::Value>,
    new: Option<&serde_json::Value>,
    changed: &mut HashMap<String, StructuredDiffKind>,
) {
    match (old, new) {
        (None, None) => {}
        (None, Some(value)) => {
            collect_structured_paths(path, value, StructuredDiffKind::Added, changed)
        }
        (Some(value), None) => {
            collect_structured_paths(path, value, StructuredDiffKind::Deleted, changed)
        }
        (Some(old), Some(new)) if old == new => {}
        (Some(serde_json::Value::Object(old)), Some(serde_json::Value::Object(new))) => {
            for key in old.keys().chain(new.keys()) {
                let child = structured_child_key(path, key);
                diff_structured_values(&child, old.get(key), new.get(key), changed);
            }
        }
        (Some(serde_json::Value::Array(old)), Some(serde_json::Value::Array(new))) => {
            for index in 0..old.len().max(new.len()) {
                let child = format!("{path}[{index}]");
                diff_structured_values(&child, old.get(index), new.get(index), changed);
            }
        }
        (Some(_), Some(_)) => {
            changed.insert(path.to_string(), StructuredDiffKind::Modified);
        }
    }
}

fn collect_structured_paths(
    path: &str,
    value: &serde_json::Value,
    kind: StructuredDiffKind,
    changed: &mut HashMap<String, StructuredDiffKind>,
) {
    changed
        .entry(path.to_string())
        .and_modify(|old| *old = merge_structured_diff_kind(*old, kind))
        .or_insert(kind);
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                collect_structured_paths(&structured_child_key(path, key), value, kind, changed);
            }
        }
        serde_json::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_structured_paths(&format!("{path}[{index}]"), value, kind, changed);
            }
        }
        _ => {}
    }
}

fn structured_child_key(parent: &str, key: &str) -> String {
    if crate::jless::lineprinter::JS_IDENTIFIER.is_match(key) {
        format!("{parent}.{key}")
    } else {
        format!(
            "{parent}[{}]",
            serde_json::to_string(key).unwrap_or_default()
        )
    }
}

fn merge_structured_diff_kind(
    old: StructuredDiffKind,
    new: StructuredDiffKind,
) -> StructuredDiffKind {
    if old == new {
        old
    } else {
        StructuredDiffKind::Modified
    }
}

fn structured_diff_kind_visible(side: SyntaxSide, kind: StructuredDiffKind) -> bool {
    matches!(
        (side, kind),
        (
            SyntaxSide::New,
            StructuredDiffKind::Added | StructuredDiffKind::Modified
        ) | (
            SyntaxSide::Old,
            StructuredDiffKind::Deleted | StructuredDiffKind::Modified
        )
    )
}

fn structured_diff_style(app: &App, kind: StructuredDiffKind) -> Style {
    match kind {
        StructuredDiffKind::Added => {
            crate::views::extent_marker_style(app, LineKind::Inserted, true, None, Some(1))
        }
        StructuredDiffKind::Deleted => {
            crate::views::extent_marker_style(app, LineKind::Deleted, true, Some(1), None)
        }
        StructuredDiffKind::Modified => {
            crate::views::extent_marker_style(app, LineKind::Modified, true, Some(1), Some(1))
        }
    }
}

fn preview_change_bar_for_span(
    side: SyntaxSide,
    span: &oyo_core::ChangeSpan,
) -> Option<(usize, PreviewChangeBar)> {
    match (side, span.kind) {
        (SyntaxSide::New, ChangeKind::Insert) => span.new_line.map(|line| {
            (
                line,
                PreviewChangeBar {
                    kind: LineKind::Inserted,
                    old_line: None,
                    new_line: Some(line),
                },
            )
        }),
        (SyntaxSide::Old, ChangeKind::Delete) => span.old_line.map(|line| {
            (
                line,
                PreviewChangeBar {
                    kind: LineKind::Deleted,
                    old_line: Some(line),
                    new_line: None,
                },
            )
        }),
        (SyntaxSide::New, ChangeKind::Replace) => span.new_line.map(|line| {
            (
                line,
                PreviewChangeBar {
                    kind: LineKind::Modified,
                    old_line: span.old_line,
                    new_line: Some(line),
                },
            )
        }),
        (SyntaxSide::Old, ChangeKind::Replace) => span.old_line.map(|line| {
            (
                line,
                PreviewChangeBar {
                    kind: LineKind::Modified,
                    old_line: Some(line),
                    new_line: span.new_line,
                },
            )
        }),
        _ => None,
    }
}

fn merge_preview_change_bars(old: PreviewChangeBar, new: PreviewChangeBar) -> PreviewChangeBar {
    if old.kind == new.kind {
        return old;
    }
    PreviewChangeBar {
        kind: LineKind::Modified,
        old_line: old.old_line.or(new.old_line),
        new_line: old.new_line.or(new.new_line),
    }
}

fn push_preview_change_gutter(
    spans: &mut Vec<Span<'static>>,
    bars: Option<&PreviewChangeBars>,
    source_line: Option<usize>,
    bg: Option<Color>,
) {
    let Some(bars) = bars else {
        return;
    };
    let style = source_line.and_then(|line| bars.styles.get(&line)).copied();
    let marker = match style {
        Some(style) => Span::styled(bars.marker.clone(), style.bg_opt(bg)),
        None => Span::styled(" ".repeat(bars.marker_width), Style::default().bg_opt(bg)),
    };
    spans.push(marker);
    spans.push(Span::styled(" ".to_string(), Style::default().bg_opt(bg)));
}

fn preview_change_gutter_width(bars: Option<&PreviewChangeBars>) -> usize {
    bars.map(PreviewChangeBars::gutter_width).unwrap_or(0)
}

fn markdown_preview_lines(
    text: &str,
    app: &mut App,
    width: usize,
    base_dir: Option<&Path>,
    change_bars: Option<&PreviewChangeBars>,
) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
    let theme = app.theme.clone();
    let mut highlight = |lang: Option<&str>, code: &str| app.highlight_code_block(lang, code);
    render_markdown_preview_lines(text, &theme, width, base_dir, &mut highlight, change_bars)
}

fn structured_preview_lines(
    title: &str,
    text: &str,
    app: &mut App,
    width: usize,
    height: usize,
    kind: StructuredPreviewKind,
    change_bars: Option<&StructuredPreviewChangeBars>,
) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
    let signature = StructuredPreviewSignature::new(kind, title, text);
    let theme = app.theme.clone();
    let scroll_offset = app.scroll_offset;
    match app.ensure_structured_preview(signature, text) {
        Ok(state) => {
            let viewer_width = width
                .saturating_sub(
                    change_bars
                        .map(StructuredPreviewChangeBars::gutter_width)
                        .unwrap_or(0),
                )
                .max(1);
            state.set_dimensions(viewer_width as u16, height as u16);
            state.set_top_visible_offset(scroll_offset);
            let lines = state.lines(&theme, width, change_bars);
            app.sync_scroll_from_structured_preview();
            (lines, Vec::new())
        }
        Err(error) => {
            let label = match kind {
                StructuredPreviewKind::Json => "JSON",
                StructuredPreviewKind::Yaml => "YAML",
                StructuredPreviewKind::Toml => "TOML",
            };
            (
                vec![
                    Line::from(Span::styled(
                        format!("Could not parse {label} preview."),
                        Style::default().fg(app.theme.warning),
                    )),
                    Line::from(Span::styled(
                        error,
                        Style::default().fg(app.theme.text_muted),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Use source view to read the file as text.",
                        Style::default().fg(app.theme.text_muted),
                    )),
                ],
                Vec::new(),
            )
        }
    }
}

fn csv_preview_lines(
    title: &str,
    text: &str,
    app: &mut App,
    width: usize,
    change_bars: Option<&PreviewChangeBars>,
) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
    let signature = CsvPreviewSignature::new(title, text);
    let theme = app.theme.clone();
    match app.ensure_csv_preview(signature, text) {
        Ok(state) => (
            csv_table_lines(state, &theme, width, change_bars),
            Vec::new(),
        ),
        Err(error) => (
            vec![
                Line::from(Span::styled(
                    "Could not parse CSV preview.",
                    Style::default().fg(app.theme.warning),
                )),
                Line::from(Span::styled(
                    error,
                    Style::default().fg(app.theme.text_muted),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Use source view to read the file as text.",
                    Style::default().fg(app.theme.text_muted),
                )),
            ],
            Vec::new(),
        ),
    }
}

fn csv_table_lines(
    state: &mut CsvPreviewState,
    theme: &crate::config::ResolvedTheme,
    width: usize,
    change_bars: Option<&PreviewChangeBars>,
) -> Vec<Line<'static>> {
    if state.rows().is_empty() {
        return vec![Line::from("")];
    }
    let cols = state.rows().iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return vec![Line::from("")];
    }

    // A row-number gutter on the left, sized to the largest data row number
    // (the header row is not counted). Data area gets the remaining width.
    let table_width = width
        .saturating_sub(preview_change_gutter_width(change_bars))
        .max(1);
    let data_rows = state.rows().len().saturating_sub(1);
    let gutter_w = data_rows.to_string().len().max(2);
    let prefix_w = gutter_w + 3; // "{n:>gw} │ "
    let data_width = table_width.saturating_sub(prefix_w).max(1);

    // Columns are sized to their full content (no truncation); horizontal
    // scrolling handles anything wider than the viewport.
    let all_widths = csv_column_widths(state.rows(), cols);
    let (visible_cols, widths) = csv_visible_columns(state, &all_widths, data_width);
    let rows = state.rows();
    if visible_cols.is_empty() {
        return vec![Line::from("")];
    }

    let header_style = Style::default().fg(theme.text).add_modifier(Modifier::BOLD);
    let cell_style = Style::default().fg(theme.text);
    let num_style = Style::default().fg(theme.text_muted);
    let rule_style = Style::default().fg(theme.border_subtle);
    // Zebra-stripe background for alternating rows, blended so it also shows on
    // transparent themes (None on ANSI/named-color themes → no stripe).
    let surface = theme
        .background
        .or(theme.background_panel)
        .or(theme.background_element)
        .unwrap_or(Color::Rgb(0x1b, 0x1d, 0x22));
    let stripe_bg = crate::color::blend_colors(surface, theme.text, 0.06);
    let bar = || Span::styled(" │ ".to_string(), rule_style);

    // Header row (blank row-number gutter).
    let mut lines = Vec::new();
    let mut hspans = Vec::new();
    push_preview_change_gutter(&mut hspans, change_bars, Some(1), None);
    hspans.extend([Span::styled(" ".repeat(gutter_w), num_style), bar()]);
    csv_push_row_cells(&mut hspans, &rows[0], &visible_cols, &widths, header_style);
    lines.push(Line::from(hspans));

    // Separator line, crossing the gutter divider.
    let mut separator = Vec::new();
    push_preview_change_gutter(&mut separator, change_bars, None, None);
    separator.push(Span::styled(
        format!(
            "{}┼{}",
            "─".repeat(gutter_w + 1),
            "─".repeat(table_width.saturating_sub(gutter_w + 2)),
        ),
        rule_style,
    ));
    lines.push(Line::from(separator));

    // Data rows with zebra striping on alternating rows.
    for (row_idx, row) in rows.iter().enumerate().skip(1) {
        let bg = if row_idx % 2 == 0 { stripe_bg } else { None };
        let num = Span::styled(format!("{row_idx:>gutter_w$}"), num_style.bg_opt(bg));
        let bar_span = Span::styled(" │ ".to_string(), rule_style.bg_opt(bg));
        let base = cell_style.bg_opt(bg);
        let mut data_spans = Vec::new();
        csv_push_row_cells(&mut data_spans, row, &visible_cols, &widths, base);
        if let Some(bg) = bg {
            // Extend the stripe across the full data width.
            let used: usize = data_spans.iter().map(|s| text_width(&s.content)).sum();
            if used < data_width {
                data_spans.push(Span::styled(
                    " ".repeat(data_width - used),
                    Style::default().bg(bg),
                ));
            }
        }
        let mut spans = Vec::new();
        // ponytail: assumes one CSV record per source line; use CSV byte positions if multiline records matter.
        push_preview_change_gutter(&mut spans, change_bars, Some(row_idx + 1), bg);
        spans.extend([num, bar_span]);
        spans.extend(data_spans);
        lines.push(Line::from(spans));
    }
    lines
}

/// Append a table row's cells (left-aligned, 2-space gaps, no truncation) to
/// `spans`, all sharing the `base` style.
fn csv_push_row_cells(
    spans: &mut Vec<Span<'static>>,
    row: &[String],
    visible_cols: &[usize],
    widths: &[usize],
    base: Style,
) {
    for (visible_idx, col) in visible_cols.iter().copied().enumerate() {
        let cw = widths[visible_idx];
        let cell = row.get(col).map(String::as_str).unwrap_or("");
        let pad = cw.saturating_sub(text_width(cell));
        spans.push(Span::styled(cell.to_string(), base));
        spans.push(Span::styled(format!("{}  ", " ".repeat(pad)), base));
    }
}

fn csv_column_widths(rows: &[Vec<String>], cols: usize) -> Vec<usize> {
    let mut widths = vec![1usize; cols];
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(text_width(cell));
        }
    }
    widths
}

fn csv_visible_columns(
    state: &mut CsvPreviewState,
    widths: &[usize],
    total_width: usize,
) -> (Vec<usize>, Vec<usize>) {
    let mut offset = state.col_offset().min(widths.len().saturating_sub(1));
    if state.selected_col() < offset {
        offset = state.selected_col();
    }
    loop {
        let cols = csv_columns_from(offset, widths, total_width);
        if cols.contains(&state.selected_col()) || offset == state.selected_col() {
            state.set_col_offset(offset);
            let visible_widths = cols.iter().map(|col| widths[*col]).collect();
            return (cols, visible_widths);
        }
        offset = offset.saturating_add(1).min(state.selected_col());
    }
}

fn csv_columns_from(offset: usize, widths: &[usize], total_width: usize) -> Vec<usize> {
    let mut cols = Vec::new();
    let mut used = 0usize;
    for (col, width) in widths.iter().enumerate().skip(offset) {
        let next = width.saturating_add(2);
        if !cols.is_empty() && used.saturating_add(next) > total_width {
            break;
        }
        cols.push(col);
        used = used.saturating_add(next);
        if used >= total_width {
            break;
        }
    }
    if cols.is_empty() && offset < widths.len() {
        cols.push(offset);
    }
    cols
}

fn source_preview_lines(
    app: &mut App,
    file_name: &str,
    text: &str,
    side: Option<SyntaxSide>,
    change_bars: Option<&PreviewChangeBars>,
) -> Vec<Line<'static>> {
    let text_style = Style::default().fg(app.theme.text);
    let highlighted = if side.is_none() {
        app.preview_source_spans(file_name, text)
    } else {
        None
    };
    let mut lines = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let source_line = idx + 1;
        let mut row_spans = Vec::new();
        push_preview_change_gutter(&mut row_spans, change_bars, Some(source_line), None);
        if let Some(spans) = highlighted
            .as_ref()
            .and_then(|lines| lines.get(idx))
            .cloned()
        {
            row_spans.extend(spans);
            lines.push(Line::from(row_spans));
            continue;
        }
        if let Some(side) = side {
            if let Some(spans) = app.syntax_spans_for_line(side, Some(source_line)) {
                row_spans.extend(spans);
                lines.push(Line::from(row_spans));
                continue;
            }
        }
        row_spans.push(Span::styled(line.to_string(), text_style));
        lines.push(Line::from(row_spans));
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn image_file_preview_lines(
    path: &Path,
    max_width: usize,
    max_rows: usize,
    theme: &crate::config::ResolvedTheme,
) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
    let bg = theme
        .background
        .or(theme.background_panel)
        .unwrap_or(Color::Black);
    let lines = image_preview_lines(path, max_width, max_rows, bg).unwrap_or_else(|| {
        vec![
            Line::from(Span::styled(
                "Could not render image preview.",
                Style::default().fg(theme.warning),
            )),
            Line::from(Span::styled(
                path.display().to_string(),
                Style::default().fg(theme.text_muted),
            )),
        ]
    });
    (lines, Vec::new())
}

fn image_preview_lines(
    path: &Path,
    max_width: usize,
    max_rows: usize,
    bg: Color,
) -> Option<Vec<Line<'static>>> {
    let image = image::ImageReader::open(path).ok()?.decode().ok()?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || max_width == 0 || max_rows == 0 {
        return None;
    }
    let max_cols = max_width.clamp(1, 80) as u32;
    let max_pixel_rows = (max_rows as u32).saturating_mul(2).max(1);
    let scale = (max_cols as f32 / width as f32)
        .min(max_pixel_rows as f32 / height as f32)
        .min(1.0);
    let target_width = ((width as f32 * scale).round() as u32).max(1);
    let target_height = ((height as f32 * scale).round() as u32).max(1);
    let resized = image
        .resize(
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        )
        .to_rgba8();
    let mut lines = Vec::new();
    for y in (0..target_height).step_by(2) {
        let mut spans = Vec::with_capacity(target_width as usize);
        for x in 0..target_width {
            let top = *resized.get_pixel(x, y);
            let bottom = if y + 1 < target_height {
                *resized.get_pixel(x, y + 1)
            } else {
                image::Rgba([0, 0, 0, 0])
            };
            spans.push(Span::styled(
                "▀".to_string(),
                Style::default()
                    .fg(rgba_over_bg(top, bg))
                    .bg(rgba_over_bg(bottom, bg)),
            ));
        }
        lines.push(Line::from(spans));
    }
    Some(lines)
}

fn rgba_over_bg(pixel: image::Rgba<u8>, bg: Color) -> Color {
    let [r, g, b, a] = pixel.0;
    if a == 255 {
        return Color::Rgb(r, g, b);
    }
    let (br, bgc, bb) = color_rgb(bg).unwrap_or((0, 0, 0));
    if a == 0 {
        return Color::Rgb(br, bgc, bb);
    }
    let alpha = a as f32 / 255.0;
    Color::Rgb(
        ((r as f32 * alpha) + (br as f32 * (1.0 - alpha))).round() as u8,
        ((g as f32 * alpha) + (bgc as f32 * (1.0 - alpha))).round() as u8,
        ((b as f32 * alpha) + (bb as f32 * (1.0 - alpha))).round() as u8,
    )
}

fn color_rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((255, 0, 0)),
        Color::Green => Some((0, 255, 0)),
        Color::Yellow => Some((255, 255, 0)),
        Color::Blue => Some((0, 0, 255)),
        Color::Magenta => Some((255, 0, 255)),
        Color::Cyan => Some((0, 255, 255)),
        Color::Gray => Some((128, 128, 128)),
        Color::DarkGray => Some((64, 64, 64)),
        Color::LightRed => Some((255, 128, 128)),
        Color::LightGreen => Some((128, 255, 128)),
        Color::LightYellow => Some((255, 255, 128)),
        Color::LightBlue => Some((128, 128, 255)),
        Color::LightMagenta => Some((255, 128, 255)),
        Color::LightCyan => Some((128, 255, 255)),
        Color::White => Some((255, 255, 255)),
        _ => None,
    }
}

/// Small extension so background colors that are `Option<Color>` (transparent
/// themes) can be applied fluently while building styles.
trait StyleBgOpt {
    fn bg_opt(self, color: Option<Color>) -> Self;
}

impl StyleBgOpt for Style {
    fn bg_opt(self, color: Option<Color>) -> Self {
        match color {
            Some(color) => self.bg(color),
            None => self,
        }
    }
}

fn is_markdown_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".mdx")
}

fn is_csv_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".csv")
}

fn is_image_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

fn help_markdown(app: &App) -> String {
    let mut out = String::new();
    out.push_str("# Oyo help\n\n");
    out.push_str(
        "Oyo helps you review changes without leaving the terminal. This help tab is the built-in manual. Use search to jump to a section.\n\n",
    );
    out.push_str("## Contents\n\n");
    out.push_str("- quick start\n");
    out.push_str("- tabs and preview\n");
    out.push_str("- keybindings\n");
    out.push_str("- configuration reference\n");
    out.push_str("- diff viewer reference\n");
    out.push_str("- review hooks reference\n");
    out.push_str("- theme reference\n\n");

    out.push_str("## Quick start\n\n");
    out.push_str("- use `j` and `k` to move through changes in step mode\n");
    out.push_str("- use `J` and `K` to scroll\n");
    out.push_str("- use `tab` to change view\n");
    out.push_str("- use `ctrl-shift-p` to open file search\n");
    out.push_str("- use `ctrl-t` to open theme picker\n");
    out.push_str("- use `?` to focus this help tab\n\n");

    out.push_str("## Tabs and preview\n\n");
    out.push_str("Each tab is a separate view. The same file can be open in more than one tab. Each tab keeps its own view mode, step mode and scroll position.\n\n");
    out.push_str("Preview mode shows file content instead of a diff. Markdown and image files open as rendered previews. Markdown, CSV and structured previews can switch between source and preview.\n\n");

    out.push_str("## Keybindings\n\n");
    out.push_str("These are your active keybindings, including config overrides.\n\n");
    out.push_str(&keybinding_section::<GlobalAction, _>(
        "Global keys",
        |action| app.keybindings.global_keys(action),
    ));
    out.push_str(&keybinding_section::<NormalAction, _>(
        "Normal mode",
        |action| app.keybindings.normal_keys(action),
    ));
    out.push_str(&keybinding_section::<SelectionAction, _>(
        "Selection mode",
        |action| app.keybindings.selection_keys(action),
    ));
    out.push_str(&keybinding_section::<PickerAction, _>(
        "Command palette",
        |action| app.keybindings.command_palette_keys(action),
    ));
    out.push_str(&keybinding_section::<PickerAction, _>(
        "File search",
        |action| app.keybindings.file_search_keys(action),
    ));
    out.push_str(&keybinding_section::<PickerAction, _>(
        "Theme picker",
        |action| app.keybindings.theme_picker_keys(action),
    ));
    out.push_str(&keybinding_section::<FileFilterAction, _>(
        "File filter",
        |action| app.keybindings.file_filter_keys(action),
    ));
    out.push_str(&keybinding_section::<LineInputAction, _>(
        "Go to input",
        |action| app.keybindings.goto_keys(action),
    ));
    out.push_str(&keybinding_section::<LineInputAction, _>(
        "Search input",
        |action| app.keybindings.search_keys(action),
    ));
    out.push_str(&keybinding_section::<ReviewEditorAction, _>(
        "Review editor",
        |action| app.keybindings.review_editor_keys(action),
    ));
    out.push_str(&keybinding_section::<DashboardAction, _>(
        "History",
        |action| app.keybindings.dashboard_keys(action),
    ));
    out.push_str(&keybinding_section::<DashboardFilterAction, _>(
        "History filter",
        |action| app.keybindings.dashboard_filter_keys(action),
    ));

    append_embedded_doc(
        &mut out,
        "Configuration reference",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/CONFIG.md")),
    );
    append_embedded_doc(
        &mut out,
        "Diff viewer reference",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/DIFF_VIEWER.md")),
    );
    append_embedded_doc(
        &mut out,
        "Keybindings reference",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/KEYBINDINGS.md")),
    );
    append_embedded_doc(
        &mut out,
        "Review hooks reference",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/REVIEW_HOOKS.md")),
    );
    append_embedded_doc(
        &mut out,
        "Theme reference",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/THEME.md")),
    );
    out
}

fn keybinding_section<A, F>(title: &str, keys: F) -> String
where
    A: BindingAction + 'static,
    F: Fn(A) -> String,
{
    let mut out = format!("## {title}\n\n| Action | Keys | What it does |\n| --- | --- | --- |\n");
    for action in A::all().iter().copied() {
        out.push_str(&format!(
            "| `{}` | `{}` | {} |\n",
            markdown_table_escape(action.id()),
            markdown_table_escape(&keys(action)),
            markdown_table_escape(action.description()),
        ));
    }
    out.push('\n');
    out
}

fn markdown_table_escape(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn append_embedded_doc(out: &mut String, title: &str, text: &str) {
    out.push_str("---\n\n");
    out.push_str("# ");
    out.push_str(title);
    out.push_str("\n\n");
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
}

fn draw_diff_view(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.outdated_reconstruction_pending() {
        frame.render_widget(Clear, area);
        if let Some(background) = app.theme.background {
            frame.render_widget(
                Block::default().style(Style::default().bg(background)),
                area,
            );
        }
        let row = Rect::new(
            area.x,
            area.y.saturating_add(area.height / 2),
            area.width,
            1.min(area.height),
        );
        frame.render_widget(
            Paragraph::new(format!("{} Reconstructing...", diff_spinner_frame()))
                .style(Style::default().fg(app.theme.text))
                .alignment(Alignment::Center),
            row,
        );
        return;
    }
    if app.multi_diff.file_count() == 0
        && app.active_topbar_content() != Some(TopbarTabContent::Help)
    {
        draw_no_changes(frame, app, area);
        return;
    }
    match app.view_mode {
        ViewMode::UnifiedPane => render_unified_pane(frame, app, area),
        ViewMode::Split => render_split(frame, app, area),
        ViewMode::Evolution => render_evolution(frame, app, area),
        ViewMode::Blame => render_blame(frame, app, area),
        ViewMode::Preview => render_preview(frame, app, area),
    }
}

fn capture_diff_selection_cells(frame: &mut Frame, app: &mut App) {
    let Some((x, y, width, height)) = app.diff_view_area else {
        app.set_diff_selection_cells(Vec::new());
        return;
    };
    let frame_area = frame.area();
    let max_x = x
        .saturating_add(width)
        .min(frame_area.x.saturating_add(frame_area.width));
    let max_y = y
        .saturating_add(height)
        .min(frame_area.y.saturating_add(frame_area.height));
    let excluded = app.diff_selection_excluded_cols();
    let excluded_rows = app.diff_selection_excluded_rows();
    let content_ranges = app.diff_selection_content_ranges();
    let cells = {
        let buffer = frame.buffer_mut();
        (y..max_y)
            .map(|row| {
                let mut cells = (x..max_x)
                    .map(|col| {
                        let local_col = col.saturating_sub(x);
                        if excluded_rows.contains(&row.saturating_sub(y))
                            || excluded
                                .iter()
                                .any(|(start, end)| local_col >= *start && local_col < *end)
                        {
                            String::new()
                        } else {
                            buffer
                                .cell((col, row))
                                .map(|cell| {
                                    let symbol = cell.symbol();
                                    let align_fill = app.view_mode == ViewMode::Split
                                        && !app.split_align_fill.is_empty()
                                        && cell.style().add_modifier.contains(Modifier::DIM)
                                        && symbol
                                            .chars()
                                            .all(|ch| app.split_align_fill.contains(ch));
                                    if align_fill {
                                        String::new()
                                    } else {
                                        symbol.to_string()
                                    }
                                })
                                .unwrap_or_else(|| " ".to_string())
                        }
                    })
                    .collect::<Vec<_>>();
                trim_diff_selection_padding(&mut cells, &content_ranges);
                cells
            })
            .collect::<Vec<_>>()
    };
    app.set_diff_selection_cells(cells);
}

fn trim_diff_selection_padding(cells: &mut [String], content_ranges: &[(u16, u16)]) {
    for (start, end) in content_ranges.iter().copied() {
        let start = start as usize;
        let end = (end as usize).min(cells.len());
        if start >= end {
            continue;
        }
        let keep_end = cells[start..end]
            .iter()
            .rposition(|symbol| !symbol.is_empty() && symbol != " ")
            .map(|idx| start + idx + 1)
            .unwrap_or(start);
        for symbol in &mut cells[keep_end..end] {
            if symbol == " " {
                symbol.clear();
            }
        }
    }
}

fn draw_diff_selection(frame: &mut Frame, app: &App) {
    let ranges = app.diff_selection_ranges();
    if ranges.is_empty() {
        return;
    }
    let style = Style::default()
        .fg(app.theme.background.unwrap_or(Color::Black))
        .bg(app.theme.accent);
    let Some((x, y, _, _)) = app.diff_view_area else {
        return;
    };
    let buffer = frame.buffer_mut();
    for (row, start_col, end_col) in ranges {
        for col in start_col..end_col {
            let local_row = row.saturating_sub(y) as usize;
            let local_col = col.saturating_sub(x) as usize;
            if app
                .diff_selection_cells
                .get(local_row)
                .and_then(|line| line.get(local_col))
                .is_some_and(|symbol| !symbol.is_empty())
            {
                if let Some(cell) = buffer.cell_mut((col, row)) {
                    cell.set_style(style);
                }
            }
        }
    }
}

fn draw_review_line_add_button(frame: &mut Frame, app: &mut App) {
    app.clear_review_line_add_hit();
    let Some(row) = app.review_line_add_row else {
        return;
    };
    if !app.review_mode()
        || app.view_mode == ViewMode::Preview
        || app.current_file_is_binary()
        || app.review_editor_active()
        || app.selection_toolbar_visible()
    {
        return;
    }
    let Some((_, y, _, height)) = app.diff_view_area else {
        return;
    };
    if row < y || row >= y.saturating_add(height) {
        return;
    }
    let Some(x) = app.review_line_add_button_x() else {
        return;
    };
    let style = Style::default()
        .fg(if app.review_line_add_hover {
            app.theme.accent
        } else {
            app.theme.text_muted
        })
        .add_modifier(Modifier::BOLD);
    app.review_line_add_hit = Some(ReviewLineAddHit {
        x,
        y: row,
        width: 3,
        height: 1,
        row,
    });
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(" + ", style))),
        Rect::new(x, row, 3, 1),
    );
}

fn draw_selection_toolbar(frame: &mut Frame, app: &mut App) {
    if app.review_editor_active() || !app.selection_toolbar_visible() {
        app.set_selection_toolbar_hits(Vec::new());
        return;
    }
    let ranges = app.diff_selection_ranges();
    if ranges.is_empty() {
        app.set_selection_toolbar_hits(Vec::new());
        return;
    }
    let Some((view_x, view_y, view_width, view_height)) = app.diff_view_area else {
        app.set_selection_toolbar_hits(Vec::new());
        return;
    };
    if view_width < 12 || view_height == 0 {
        app.set_selection_toolbar_hits(Vec::new());
        return;
    }

    // Actions as (key, label): the key shows in the accent color, the label in
    // regular text.
    let mut items: Vec<(String, String, SelectionToolbarAction)> = vec![(
        app.keybindings.selection_keys(SelectionAction::Copy),
        "copy".to_string(),
        SelectionToolbarAction::Copy,
    )];
    if app.review_mode() && app.view_mode != ViewMode::Preview {
        items.push((
            "m".to_string(),
            "comment".to_string(),
            SelectionToolbarAction::Comment,
        ));
    }
    items.push((
        "esc".to_string(),
        "cancel".to_string(),
        SelectionToolbarAction::Cancel,
    ));
    for (idx, action) in app.selection_actions.iter().enumerate() {
        let key = action.key.as_deref().unwrap_or_default().trim().to_string();
        let label = if action.label.trim().is_empty() {
            action.id.clone()
        } else {
            action.label.clone()
        };
        if label.is_empty() {
            continue;
        }
        items.push((key, label, SelectionToolbarAction::Custom(idx)));
    }

    // Item display width: `key label`, or just `label` when there's no key.
    let item_width = |key: &str, label: &str| {
        if key.is_empty() {
            label.chars().count()
        } else {
            key.chars().count() + 1 + label.chars().count()
        }
    };
    // Fit items into the view width (2 border + 2 inner padding + optional arrows).
    const CHROME: usize = 4; // border (2) + inner padding (2)
    const ARROW: usize = 2;
    let max_scroll = items.len().saturating_sub(1);
    app.selection_toolbar_scroll = app.selection_toolbar_scroll.min(max_scroll);
    let scroll = app.selection_toolbar_scroll;
    let fit_width = app
        .selection_toolbar_width
        .unwrap_or(view_width)
        .min(view_width) as usize;
    let fit_items = |reserve_left: bool, reserve_right: bool| {
        let reserve = usize::from(reserve_left) * ARROW + usize::from(reserve_right) * ARROW;
        let limit = fit_width.saturating_sub(CHROME + reserve);
        let mut fitted: Vec<(String, String, SelectionToolbarAction, usize)> = Vec::new();
        let mut inner = 0usize;
        for (key, label, action) in items.iter().skip(scroll) {
            let iw = item_width(key, label);
            let gap = if fitted.is_empty() { 0 } else { 2 };
            if inner + gap + iw > limit {
                break;
            }
            inner += gap + iw;
            fitted.push((key.clone(), label.clone(), *action, iw));
        }
        (fitted, inner + reserve)
    };
    let hidden_left = scroll > 0;
    let (mut fitted, mut inner) = fit_items(hidden_left, false);
    let mut hidden_right = scroll + fitted.len() < items.len();
    if hidden_right {
        (fitted, inner) = fit_items(hidden_left, true);
        hidden_right = scroll + fitted.len() < items.len();
    }
    if fitted.is_empty() {
        app.set_selection_toolbar_hits(Vec::new());
        return;
    }

    // Block-quote preview of the selected text, shown above the actions. Up to
    // four lines, then a fifth row with an ellipsis when more was selected.
    const MAX_QUOTE_LINES: usize = 4;
    let preview_raw = app.selected_diff_text();
    let preview_lines: Vec<String> = preview_raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let has_quote = !preview_lines.is_empty();
    let has_more = preview_lines.len() > MAX_QUOTE_LINES;
    let quote_rows = preview_lines.len().min(MAX_QUOTE_LINES) + usize::from(has_more);

    // The float may grow up to ~50% of the viewport to preview the selection.
    let max_content = ((view_width as usize) / 2)
        .saturating_sub(CHROME)
        .max(inner);
    let quote_want = if has_quote {
        2 + preview_lines
            .iter()
            .take(MAX_QUOTE_LINES)
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let content_inner = inner.max(quote_want).min(max_content);
    let measured_w = (content_inner + CHROME) as u16;
    let popup_w = app
        .selection_toolbar_width
        .unwrap_or(measured_w)
        .min(view_width)
        .max((CHROME + 1) as u16);
    app.selection_toolbar_width = Some(popup_w);
    let height: u16 = if has_quote {
        (quote_rows + 3) as u16
    } else {
        3
    };

    let first_row = ranges.iter().map(|(r, _, _)| *r).min().unwrap_or(view_y);
    let last_row = ranges.iter().map(|(r, _, _)| *r).max().unwrap_or(view_y);
    let first_col = ranges
        .iter()
        .filter(|(r, _, _)| *r == first_row)
        .map(|(_, s, _)| *s)
        .min()
        .unwrap_or(view_x);
    let view_bottom = view_y.saturating_add(view_height);
    // Float above the selection when there's room, otherwise below it.
    let py = if first_row >= view_y.saturating_add(height) {
        first_row - height
    } else {
        last_row
            .saturating_add(1)
            .min(view_bottom.saturating_sub(height))
    };
    let px = first_col
        .saturating_sub(1)
        .min(view_x.saturating_add(view_width).saturating_sub(popup_w))
        .max(view_x);

    // Match the app background (transparent when the theme has none), like the
    // toast. `Clear` erases the content behind the float, so it reads cleanly.
    let float_bg = app.theme.background.unwrap_or(Color::Reset);

    let popup_area = Rect::new(px, py, popup_w, height);
    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_subtle))
        .style(Style::default().bg(float_bg));
    let content_area = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let hovered = app.selection_toolbar_hover;
    let key_style = Style::default()
        .fg(app.theme.accent)
        .bg(float_bg)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(app.theme.text).bg(float_bg);
    let quote_bar = Style::default().fg(app.theme.text_muted).bg(float_bg);
    let quote_text = quote_bar.add_modifier(Modifier::ITALIC);
    let dim = Style::default().bg(float_bg);
    // Hovered action brightens and bolds its label (like the topbar "+").
    let hover_key = key_style;
    let hover_label = Style::default()
        .fg(app.theme.accent)
        .bg(float_bg)
        .add_modifier(Modifier::BOLD);

    let one_row = |x: u16, y: u16| Rect::new(x, y, content_area.width, 1);
    let actions_y = content_area.y + quote_rows as u16;

    if has_quote {
        // A heavy left bar plus truncated, italic preview lines.
        let budget = content_inner.saturating_sub(2);
        for (i, line) in preview_lines.iter().take(MAX_QUOTE_LINES).enumerate() {
            let shown = if line.chars().count() > budget {
                let mut s: String = line.chars().take(budget.saturating_sub(1)).collect();
                s.push('…');
                s
            } else {
                line.clone()
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" ", dim),
                    Span::styled("┃ ", quote_bar),
                    Span::styled(shown, quote_text),
                ])),
                one_row(content_area.x, content_area.y + i as u16),
            );
        }
        if has_more {
            // Continuation row: the block bar plus an ellipsis.
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" ", dim),
                    Span::styled("┃ ", quote_bar),
                    Span::styled("…", quote_bar),
                ])),
                one_row(content_area.x, content_area.y + MAX_QUOTE_LINES as u16),
            );
        }
    }

    let mut spans = vec![Span::styled(" ", dim)];
    let mut hits = Vec::new();
    let mut col = content_area.x.saturating_add(1); // after the left pad
    if hidden_left {
        let action = SelectionToolbarAction::ScrollLeft;
        let style = topbar_overflow_style(app, hovered == Some(action));
        hits.push(SelectionToolbarHit {
            action,
            x: col,
            y: actions_y,
            width: ARROW as u16,
            height: 1,
        });
        spans.push(Span::styled("‹ ", style));
        col = col.saturating_add(ARROW as u16);
    }
    for (idx, (key, label, action, iw)) in fitted.into_iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("  ", dim));
            col = col.saturating_add(2);
        }
        hits.push(SelectionToolbarHit {
            action,
            x: col,
            y: actions_y,
            width: iw as u16,
            height: 1,
        });
        let is_hover = hovered == Some(action);
        let (ks, ls) = if is_hover {
            (hover_key, hover_label)
        } else {
            (key_style, label_style)
        };
        if key.is_empty() {
            spans.push(Span::styled(label, ls));
        } else {
            spans.push(Span::styled(key, ks));
            spans.push(Span::styled(format!(" {label}"), ls));
        }
        col = col.saturating_add(iw as u16);
    }
    if hidden_right {
        let action = SelectionToolbarAction::ScrollRight;
        let style = topbar_overflow_style(app, hovered == Some(action));
        hits.push(SelectionToolbarHit {
            action,
            x: col,
            y: actions_y,
            width: ARROW as u16,
            height: 1,
        });
        spans.push(Span::styled(" ›", style));
    }
    spans.push(Span::styled(" ", dim));
    app.set_selection_toolbar_rect(Some((
        popup_area.x,
        popup_area.y,
        popup_area.width,
        popup_area.height,
    )));
    app.set_selection_toolbar_hits(hits);
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        one_row(content_area.x, actions_y),
    );
}

fn draw_review_editor_overlay(frame: &mut Frame, app: &mut App) {
    let Some(editor) = app.review_editor_render() else {
        app.clear_review_editor_toolbar();
        return;
    };
    let Some((x, y, width, height)) = app.diff_view_area else {
        return;
    };
    if width < 20 || height < 4 {
        return;
    }

    let diff_area = Rect::new(x, y, width, height);
    let editor_area = if app.view_mode == ViewMode::Split {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(diff_area);
        if editor.prefer_right {
            panes[1]
        } else {
            panes[0]
        }
    } else {
        diff_area
    };

    let render_scroll = app.render_scroll_offset() as isize;
    let max_row = editor_area.height.saturating_sub(1) as isize;
    let anchor_span_rows = editor.anchor_display_span.and_then(|(start_idx, end_idx)| {
        let start_rel = start_idx as isize - render_scroll;
        let end_rel = end_idx as isize - render_scroll;
        if end_rel < 0 || start_rel > max_row {
            None
        } else {
            Some((
                start_rel.clamp(0, max_row) as u16,
                end_rel.clamp(0, max_row) as u16,
            ))
        }
    });

    let anchor_row = anchor_span_rows
        .map(|(start, _)| start)
        .or_else(|| {
            editor.display_idx_hint.map(|idx| {
                idx.saturating_sub(app.render_scroll_offset())
                    .min(editor_area.height.saturating_sub(1) as usize) as u16
            })
        })
        .unwrap_or(0)
        .min(editor_area.height.saturating_sub(1));
    let forbidden_rows = anchor_span_rows.unwrap_or((anchor_row, anchor_row));

    let popup_x = editor_area.x.saturating_add(1);
    let editor_right = editor_area.x.saturating_add(editor_area.width);
    let popup_width = editor_right.saturating_sub(1).saturating_sub(popup_x);
    if popup_width < 4 {
        return;
    }
    let max_popup_height = editor_area.height.saturating_sub(1).max(6);
    let text_wrap_width = popup_width.saturating_sub(4).max(1) as usize;
    let desired_text_lines = editor
        .lines
        .iter()
        .map(|line| wrap_editor_line(line, text_wrap_width).len())
        .sum::<usize>()
        .max(1)
        .min(u16::MAX as usize) as u16;
    let desired_popup_height = desired_text_lines
        .saturating_add(2)
        .clamp(6, max_popup_height);
    let min_popup_height = 4u16;
    let mut popup_height =
        desired_popup_height.min(editor_area.height.saturating_sub(1).max(min_popup_height));

    let area_top = editor_area.y;
    let area_bottom = editor_area.y.saturating_add(editor_area.height);
    let forbidden_top_y = editor_area.y.saturating_add(forbidden_rows.0);
    let forbidden_bottom_y = editor_area.y.saturating_add(forbidden_rows.1);

    // Prefer placing below the anchored line/hunk so the referenced lines remain visible.
    // Keep a one-row gap when possible to avoid touching the referenced line(s).
    // Fall back to above, and for hunk anchors use the hunk middle when space is tight.
    let placement_gap = 1u16;

    let below_y_gap = forbidden_bottom_y
        .saturating_add(1)
        .saturating_add(placement_gap);
    let below_space_gap = area_bottom.saturating_sub(below_y_gap);

    let below_y_tight = forbidden_bottom_y.saturating_add(1);
    let below_space_tight = area_bottom.saturating_sub(below_y_tight);

    let above_space = forbidden_top_y.saturating_sub(area_top);
    let above_space_gap = above_space.saturating_sub(placement_gap);

    let popup_y = if below_space_gap >= min_popup_height {
        popup_height = popup_height.min(below_space_gap);
        below_y_gap
    } else if below_space_tight >= min_popup_height {
        popup_height = popup_height.min(below_space_tight);
        below_y_tight
    } else if above_space_gap >= min_popup_height {
        popup_height = popup_height.min(above_space_gap);
        forbidden_top_y
            .saturating_sub(placement_gap)
            .saturating_sub(popup_height)
    } else if above_space >= min_popup_height {
        popup_height = popup_height.min(above_space);
        forbidden_top_y.saturating_sub(popup_height)
    } else if editor.anchor_is_hunk {
        popup_height = popup_height.min(editor_area.height.max(1));
        let center_y =
            forbidden_top_y.saturating_add(forbidden_bottom_y.saturating_sub(forbidden_top_y) / 2);
        let max_y = area_bottom.saturating_sub(popup_height);
        center_y
            .saturating_sub(popup_height / 2)
            .max(area_top)
            .min(max_y)
    } else {
        popup_height = popup_height.min(editor_area.height.max(1));
        let max_y = area_bottom.saturating_sub(popup_height);
        let mut y = below_y_tight.min(max_y).max(area_top);
        let overlaps_forbidden = y <= forbidden_bottom_y
            && y.saturating_add(popup_height).saturating_sub(1) >= forbidden_top_y;
        if overlaps_forbidden && above_space > 0 {
            popup_height = popup_height.min(above_space);
            y = forbidden_top_y.saturating_sub(popup_height);
        }
        y
    };

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let mut actions = vec![
        (
            app.keybindings.review_editor_keys(ReviewEditorAction::Save),
            "save".to_string(),
            ReviewEditorToolbarAction::Save,
        ),
        (
            app.keybindings
                .review_editor_keys(ReviewEditorAction::Cancel),
            "cancel".to_string(),
            ReviewEditorToolbarAction::Cancel,
        ),
        (
            "@".to_string(),
            "mention".to_string(),
            ReviewEditorToolbarAction::Mention,
        ),
    ];
    actions.extend(
        app.review_action_entries_for_editor()
            .into_iter()
            .map(|(idx, key, label)| (key, label, ReviewEditorToolbarAction::Custom(idx))),
    );

    let item_width = |key: &str, label: &str| {
        if key.is_empty() {
            label.chars().count()
        } else {
            key.chars().count() + 1 + label.chars().count()
        }
    };
    const ARROW: usize = 2;
    let max_scroll = actions.len().saturating_sub(1);
    app.review_editor_toolbar_scroll = app.review_editor_toolbar_scroll.min(max_scroll);
    let scroll = app.review_editor_toolbar_scroll;
    let fit_actions = |reserve_left: bool, reserve_right: bool| {
        let reserve = usize::from(reserve_left) * ARROW + usize::from(reserve_right) * ARROW;
        let limit = (popup_width as usize).saturating_sub(4 + reserve);
        let mut fitted: Vec<(String, String, ReviewEditorToolbarAction, usize)> = Vec::new();
        let mut used = 0usize;
        for (key, label, action) in actions.iter().skip(scroll) {
            let width = item_width(key, label);
            let gap = if fitted.is_empty() { 0 } else { 2 };
            if used + gap + width > limit {
                break;
            }
            used += gap + width;
            fitted.push((key.clone(), label.clone(), *action, width));
        }
        fitted
    };
    let hidden_left = scroll > 0;
    let mut fitted = fit_actions(hidden_left, false);
    let mut hidden_right = scroll + fitted.len() < actions.len();
    if hidden_right {
        fitted = fit_actions(hidden_left, true);
        hidden_right = scroll + fitted.len() < actions.len();
    }
    let footer_y = popup_area
        .y
        .saturating_add(popup_area.height.saturating_sub(1));
    let footer_area = Rect::new(
        popup_area.x.saturating_add(1),
        footer_y,
        popup_area.width.saturating_sub(2),
        1,
    );
    let mut footer_spans = vec![Span::raw(" ")];
    let mut toolbar_hits = Vec::new();
    let mut col = footer_area.x.saturating_add(1);
    let key_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(app.theme.text);
    let hover_label = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let hovered = app.review_editor_toolbar_hover;
    if hidden_left {
        let action = ReviewEditorToolbarAction::ScrollLeft;
        toolbar_hits.push(ReviewEditorToolbarHit {
            action,
            x: col,
            y: footer_y,
            width: ARROW as u16,
            height: 1,
        });
        footer_spans.push(Span::styled(
            "‹ ",
            topbar_overflow_style(app, hovered == Some(action)),
        ));
        col = col.saturating_add(ARROW as u16);
    }
    for (idx, (key, label, action, width)) in fitted.into_iter().enumerate() {
        if idx > 0 {
            footer_spans.push(Span::raw("  "));
            col = col.saturating_add(2);
        }
        toolbar_hits.push(ReviewEditorToolbarHit {
            action,
            x: col,
            y: footer_y,
            width: width as u16,
            height: 1,
        });
        let is_hover = hovered == Some(action);
        let label_style = if is_hover { hover_label } else { label_style };
        if key.is_empty() {
            footer_spans.push(Span::styled(label, label_style));
        } else {
            footer_spans.push(Span::styled(key, key_style));
            footer_spans.push(Span::styled(format!(" {label}"), label_style));
        }
        col = col.saturating_add(width as u16);
    }
    if hidden_right {
        let action = ReviewEditorToolbarAction::ScrollRight;
        toolbar_hits.push(ReviewEditorToolbarHit {
            action,
            x: col,
            y: footer_y,
            width: ARROW as u16,
            height: 1,
        });
        footer_spans.push(Span::styled(
            " ›",
            topbar_overflow_style(app, hovered == Some(action)),
        ));
    }
    footer_spans.push(Span::raw(" "));
    app.set_review_editor_toolbar_hits(
        Some((
            footer_area.x,
            footer_area.y,
            footer_area.width,
            footer_area.height,
        )),
        toolbar_hits,
    );

    let title = truncate_to_width(&editor.title, popup_width.saturating_sub(4) as usize);
    let mut block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_active));
    let editor_bg = app.theme.background;
    if let Some(bg) = editor_bg {
        block = block.style(Style::default().bg(bg));
    }

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    frame.render_widget(
        Paragraph::new(Line::from(footer_spans)).style(Style::default().bg_opt(editor_bg)),
        footer_area,
    );

    let text_area = inner;
    let padded_text_area = text_area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 0,
    });
    let text_area = if padded_text_area.width == 0 {
        text_area
    } else {
        padded_text_area
    };

    let visible_lines = text_area.height.max(1) as usize;
    let wrap_width = text_area.width.max(1) as usize;
    app.set_review_editor_wrap_width(wrap_width);

    let mut visual_lines: Vec<String> = Vec::new();
    let mut cursor_visual_row = 0usize;
    let mut cursor_visual_col = 0usize;

    for (logical_row, line) in editor.lines.iter().enumerate() {
        let wrapped = wrap_editor_line(line, wrap_width);
        if logical_row < editor.cursor_row {
            cursor_visual_row = cursor_visual_row.saturating_add(wrapped.len());
        } else if logical_row == editor.cursor_row {
            let (row_in_line, col_in_line) =
                editor_cursor_visual(line, editor.cursor_col, wrap_width);
            cursor_visual_row = cursor_visual_row.saturating_add(row_in_line);
            cursor_visual_col = col_in_line;
        }
        visual_lines.extend(wrapped);
    }

    if visual_lines.is_empty() {
        visual_lines.push(String::new());
    }

    let max_start = visual_lines.len().saturating_sub(visible_lines);
    let start_row = cursor_visual_row
        .saturating_add(1)
        .saturating_sub(visible_lines)
        .min(max_start);
    let end_row = (start_row + visible_lines).min(visual_lines.len());

    let text_lines: Vec<Line> = visual_lines[start_row..end_row]
        .iter()
        .map(|line| {
            Line::from(Span::styled(
                if line.is_empty() {
                    " ".to_string()
                } else {
                    line.clone()
                },
                Style::default().fg(app.theme.text).bg_opt(editor_bg),
            ))
        })
        .collect();

    frame.render_widget(
        Paragraph::new(text_lines).style(Style::default().bg_opt(editor_bg)),
        text_area,
    );

    let cursor_screen_row = cursor_visual_row.saturating_sub(start_row);
    let cursor_x = text_area.x.saturating_add(cursor_visual_col as u16).min(
        text_area
            .x
            .saturating_add(text_area.width.saturating_sub(1)),
    );
    let cursor_y = text_area.y.saturating_add(cursor_screen_row as u16).min(
        text_area
            .y
            .saturating_add(text_area.height.saturating_sub(1)),
    );

    if let Some(mentions) = app.review_mention_render() {
        let max_items = mentions.items.len().min(5);
        if max_items > 0 && diff_area.width > 14 && diff_area.height > 4 {
            let start_idx = mentions
                .scroll_start
                .min(mentions.items.len().saturating_sub(max_items));
            let end_idx = (start_idx + max_items).min(mentions.items.len());

            let max_text_width = mentions.items[start_idx..end_idx]
                .iter()
                .map(|item| text_width(item))
                .max()
                .unwrap_or(0)
                .min(diff_area.width.saturating_sub(10) as usize);
            let popup_width = (max_text_width as u16)
                .saturating_add(4)
                .max(12)
                .min(diff_area.width.saturating_sub(2).max(1));
            let popup_height = (max_items as u16)
                .saturating_add(2)
                .min(diff_area.height.saturating_sub(2).max(3));

            let min_x = diff_area.x.saturating_add(1);
            let max_x = diff_area
                .x
                .saturating_add(diff_area.width)
                .saturating_sub(popup_width)
                .saturating_sub(1);
            let min_y = diff_area.y.saturating_add(1);
            let max_y = diff_area
                .y
                .saturating_add(diff_area.height)
                .saturating_sub(popup_height)
                .saturating_sub(1);

            let mention_area = if min_x > max_x || min_y > max_y {
                Rect::new(min_x, min_y, popup_width, popup_height)
            } else {
                // Keep placement close to cursor; only collision-bound against diff view bounds.
                let clamp_u16 = |value: u16, lo: u16, hi: u16| value.max(lo).min(hi);
                let right_x = cursor_x.saturating_add(1);
                let left_x = cursor_x.saturating_sub(popup_width.saturating_add(1));
                let centered_x = cursor_x.saturating_sub(popup_width / 2);
                let below_y = cursor_y;
                let above_y = cursor_y.saturating_sub(popup_height);

                let candidates = [
                    (right_x, below_y),
                    (right_x, above_y),
                    (left_x, below_y),
                    (left_x, above_y),
                    (centered_x, below_y),
                    (centered_x, above_y),
                    (right_x, cursor_y),
                    (left_x, cursor_y),
                    (centered_x, cursor_y),
                    (min_x, below_y),
                    (max_x, below_y),
                    (min_x, above_y),
                    (max_x, above_y),
                    (min_x, min_y),
                ];

                let mut fallback = Rect::new(
                    clamp_u16(right_x, min_x, max_x),
                    clamp_u16(below_y, min_y, max_y),
                    popup_width,
                    popup_height,
                );

                for (cx, cy) in candidates {
                    let x = clamp_u16(cx, min_x, max_x);
                    let y = clamp_u16(cy, min_y, max_y);
                    let rect = Rect::new(x, y, popup_width, popup_height);
                    let contains_cursor = cursor_x >= rect.x
                        && cursor_x < rect.x.saturating_add(rect.width)
                        && cursor_y >= rect.y
                        && cursor_y < rect.y.saturating_add(rect.height);
                    if !contains_cursor {
                        fallback = rect;
                        break;
                    }
                }

                fallback
            };
            frame.render_widget(Clear, mention_area);

            let mut mention_block = Block::default()
                .title(Span::styled(
                    format!(" @{} ", mentions.query),
                    Style::default().fg(app.theme.text_muted),
                ))
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(app.theme.border_subtle));
            if let Some(bg) = app.theme.background_panel.or(app.theme.background) {
                mention_block = mention_block.style(Style::default().bg(bg));
            }
            let inner = mention_block.inner(mention_area);
            frame.render_widget(mention_block, mention_area);

            let mut mention_lines: Vec<Line> = Vec::new();
            for (local_idx, item) in mentions.items[start_idx..end_idx].iter().enumerate() {
                let idx = start_idx + local_idx;
                let text = truncate_text(item, inner.width.saturating_sub(2) as usize);
                let style = if idx == mentions.selected {
                    Style::default().fg(app.theme.accent)
                } else {
                    Style::default().fg(app.theme.text)
                };
                mention_lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(text, style),
                    Span::raw(" "),
                ]));
            }
            frame.render_widget(Paragraph::new(mention_lines), inner);
        }
    }

    frame.set_cursor_position((cursor_x, cursor_y));
}

fn draw_zen_progress(frame: &mut Frame, app: &mut App) {
    if app.multi_diff.file_count() == 0 {
        return;
    }
    let state = app.state();
    let label = format!(" {}/{} ", state.current_step + 1, state.total_steps);

    // Position in bottom-right corner
    let area = frame.area();
    let width = label.len() as u16;
    let x = area.width.saturating_sub(width + 1);
    let y = area.height.saturating_sub(1);

    let progress_area = Rect::new(x, y, width, 1);
    let text = Paragraph::new(label).style(Style::default().fg(app.theme.text));

    frame.render_widget(text, progress_area);
}

fn review_sync_action_label(action: ReviewSyncAction) -> &'static str {
    match action {
        ReviewSyncAction::Sync => "Sync with remote",
        ReviewSyncAction::Pull => "Pull from remote",
        ReviewSyncAction::Push => "Push to remote",
    }
}

fn draw_review_remote_picker(frame: &mut Frame, app: &mut App) {
    let Some(picker) = app.review_remote_picker_render().cloned() else {
        app.set_review_remote_picker_hits(Vec::new());
        return;
    };
    let query = picker.query.trim().to_ascii_lowercase();
    let filtered = picker
        .remotes
        .iter()
        .enumerate()
        .filter(|(_, remote)| {
            query.is_empty()
                || remote.name.to_ascii_lowercase().contains(&query)
                || remote.label.to_ascii_lowercase().contains(&query)
        })
        .collect::<Vec<_>>();
    let area = frame.area();
    let list_width = filtered
        .iter()
        .map(|(_, remote)| text_width(&remote.name) + 2 + text_width(&remote.label))
        .max()
        .unwrap_or(20)
        .max(text_width(review_sync_action_label(picker.action)))
        .min(58);
    let popup_width = (list_width as u16)
        .saturating_add(6)
        .min(area.width.saturating_sub(4))
        .max(32);
    let list_height = filtered.len().clamp(1, 8);
    let popup_height = (list_height as u16)
        .saturating_add(6)
        .min(area.height.saturating_sub(2))
        .max(7);
    let popup_x = area
        .x
        .saturating_add(area.width.saturating_sub(popup_width) / 2);
    let popup_y = area
        .y
        .saturating_add(area.height.saturating_sub(popup_height) / 2);
    let popup = Rect::new(popup_x, popup_y, popup_width, popup_height);
    frame.render_widget(Clear, popup);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title(format!(" {} ", review_sync_action_label(picker.action)))
        .title_alignment(Alignment::Left)
        .border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), popup);
    let inner = block.inner(popup);
    let padded = inner.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let content = if padded.width > 0 && padded.height > 0 {
        padded
    } else {
        inner
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(content);
    frame.render_widget(
        Paragraph::new(vec![picker_input_line(
            app,
            &picker.query,
            "Search remotes…",
            chunks[0].width,
        )])
        .alignment(Alignment::Left),
        chunks[0],
    );

    if filtered.is_empty() {
        app.set_review_remote_picker_hits(Vec::new());
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No results",
                Style::default().fg(app.theme.text_muted),
            )))
            .alignment(Alignment::Center),
            chunks[1],
        );
        return;
    }

    let selected_pos = filtered
        .iter()
        .position(|(idx, _)| *idx == picker.selected)
        .unwrap_or(0);
    let visible_count = list_height.min(chunks[1].height as usize).max(1);
    let start = selected_pos.saturating_add(1).saturating_sub(visible_count);
    let end = (start + visible_count).min(filtered.len());
    let mut hits = Vec::new();
    let mut lines = Vec::new();
    for (local_idx, (idx, remote)) in filtered[start..end].iter().enumerate() {
        let row = chunks[1].y.saturating_add(local_idx as u16);
        let selected = picker.selected == *idx || app.review_remote_picker_hover == Some(*idx);
        let mut style = Style::default().fg(if selected {
            app.theme.accent
        } else {
            app.theme.text
        });
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        let label_width = chunks[1].width.saturating_sub(2) as usize;
        let text = truncate_with_dots(&format!("{}  {}", remote.name, remote.label), label_width);
        lines.push(Line::from(vec![
            Span::styled(if selected { "› " } else { "  " }, style),
            Span::styled(text, style),
        ]));
        hits.push(ReviewRemotePickerHit {
            x: chunks[1].x,
            y: row,
            width: chunks[1].width,
            height: 1,
            index: *idx,
        });
    }
    app.set_review_remote_picker_hits(hits);
    frame.render_widget(Paragraph::new(lines), chunks[1]);
}

fn draw_session_rename_modal(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let title = "Rename session";
    let body = "Enter a session name.";
    let footer_label = "enter save    esc cancel";
    let query = app.session_rename_query();
    let content_width = [
        text_width(title),
        text_width(body),
        text_width(query).max(text_width("Session name")),
        text_width(footer_label),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
    .min(52);
    let popup_width = (content_width as u16)
        .saturating_add(8)
        .max(32)
        .min(area.width.saturating_sub(2).max(1));
    let popup_height = 6u16.min(area.height.saturating_sub(2).max(1));
    let popup_x = area
        .x
        .saturating_add(area.width.saturating_sub(popup_width) / 2);
    let popup_y = area
        .y
        .saturating_add(area.height.saturating_sub(popup_height) / 2);
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let border = Style::default().fg(app.theme.accent);
    let title_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(app.theme.text);
    let dim_style = Style::default().fg(app.theme.text_muted);
    let prompt_style = Style::default()
        .fg(app.theme.primary)
        .add_modifier(Modifier::BOLD);
    let total_width = popup_width as usize;
    let content_width = total_width.saturating_sub(4);
    let title = truncate_with_dots(title, total_width.saturating_sub(4));
    let title_rule = "─".repeat(total_width.saturating_sub(text_width(&title).saturating_add(4)));

    let content_line = |spans: Vec<Span<'static>>| {
        let used = spans_width(&spans);
        let mut out = vec![Span::styled("│ ".to_string(), border)];
        out.extend(spans);
        out.push(Span::raw(" ".repeat(content_width.saturating_sub(used))));
        out.push(Span::styled(" │".to_string(), border));
        Line::from(out)
    };

    let query_width = content_width.saturating_sub(4);
    let query_text = if query.is_empty() {
        "Session name".to_string()
    } else {
        truncate_text_from_start(query, query_width)
    };
    let query_style = if query.is_empty() {
        dim_style
    } else {
        text_style
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("╭ ".to_string(), border),
        Span::styled(title, title_style),
        Span::styled(format!(" {title_rule}╮"), border),
    ])];
    lines.push(content_line(Vec::new()));
    lines.push(content_line(vec![Span::styled(
        body.to_string(),
        text_style,
    )]));
    lines.push(content_line(vec![
        Span::styled("❯ ".to_string(), prompt_style),
        Span::styled(query_text, query_style),
        Span::styled(
            if app.file_filter_cursor_visible {
                "│".to_string()
            } else {
                " ".to_string()
            },
            prompt_style,
        ),
    ]));
    lines.push(content_line(Vec::new()));

    let footer_width = text_width(footer_label);
    let footer_rule = "─".repeat(total_width.saturating_sub(footer_width.saturating_add(4)));
    lines.push(Line::from(vec![
        Span::styled("╰ ".to_string(), border),
        Span::styled("enter".to_string(), prompt_style),
        Span::raw(" save    "),
        Span::styled("esc".to_string(), prompt_style),
        Span::raw(" cancel"),
        Span::styled(format!(" {footer_rule}╯"), border),
    ]));

    frame.render_widget(Paragraph::new(lines), popup_area);
}

fn draw_confirmation(frame: &mut Frame, app: &mut App, quit: bool) {
    let (title, body, confirm_label, tone) = if quit {
        (
            "Quit".to_string(),
            "Are you sure you want to quit?".to_string(),
            "enter quit".to_string(),
            app.theme.warning,
        )
    } else {
        let Some(confirmation) = app.review_delete_confirmation_render() else {
            app.set_review_delete_confirmation_hits(Vec::new());
            return;
        };
        (
            confirmation.title,
            confirmation.body,
            confirmation.confirm_label,
            app.theme.error,
        )
    };
    let body_lines = body.lines().collect::<Vec<_>>();
    let footer_label = format!("{confirm_label}    esc cancel");
    let area = frame.area();
    let content_width = body_lines
        .iter()
        .map(|line| text_width(line))
        .fold(text_width(&title), usize::max)
        .max(text_width(&footer_label))
        .min(52);
    let popup_width = (content_width as u16)
        .saturating_add(8)
        .min(area.width.saturating_sub(4))
        .max(28);
    let body_capacity = area.height.saturating_sub(6) as usize;
    let visible_body = body_lines.len().min(body_capacity);
    let popup_height = (visible_body as u16)
        .saturating_add(4)
        .min(area.height.saturating_sub(2))
        .max(4);
    let popup_x = area
        .x
        .saturating_add(area.width.saturating_sub(popup_width) / 2);
    let popup_y = area
        .y
        .saturating_add(area.height.saturating_sub(popup_height) / 2);
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    if let Some(background) = app.theme.background {
        frame.render_widget(
            Block::default().style(Style::default().bg(background)),
            popup_area,
        );
    }

    let border = Style::default().fg(tone);
    let title_style = Style::default().fg(tone).add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(app.theme.text);
    let key_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let confirm_hover =
        app.review_delete_confirmation_hover == Some(ReviewDeleteConfirmationAction::Confirm);
    let cancel_hover =
        app.review_delete_confirmation_hover == Some(ReviewDeleteConfirmationAction::Cancel);
    let confirm_key_style = if confirm_hover {
        Style::default().fg(tone).add_modifier(Modifier::BOLD)
    } else {
        key_style
    };
    let confirm_label_style = if confirm_hover {
        Style::default().fg(tone).add_modifier(Modifier::BOLD)
    } else {
        text_style
    };
    let cancel_label_style = if cancel_hover {
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        text_style
    };

    let total_width = popup_width as usize;
    let content_width = total_width.saturating_sub(4);
    let title = truncate_with_dots(&title, total_width.saturating_sub(4));
    let title_rule = "─".repeat(total_width.saturating_sub(text_width(&title).saturating_add(4)));
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("╭ ".to_string(), border),
        Span::styled(title, title_style),
        Span::styled(format!(" {title_rule}╮"), border),
    ]));

    let content_line = |spans: Vec<Span<'static>>| {
        let used = spans_width(&spans);
        let mut out = vec![Span::styled("│ ".to_string(), border)];
        out.extend(spans);
        out.push(Span::raw(" ".repeat(content_width.saturating_sub(used))));
        out.push(Span::styled(" │".to_string(), border));
        Line::from(out)
    };

    lines.push(content_line(Vec::new()));
    for line in body_lines
        .iter()
        .take(popup_height.saturating_sub(4) as usize)
    {
        lines.push(content_line(vec![Span::styled(
            truncate_with_dots(line, content_width),
            text_style,
        )]));
    }
    lines.push(content_line(Vec::new()));

    let (confirm_key, confirm_text) = confirm_label
        .split_once(' ')
        .unwrap_or((confirm_label.as_str(), ""));
    let confirm_width = text_width(&confirm_label) as u16;
    let cancel_label = "esc cancel";
    let footer_width = text_width(&confirm_label) + 4 + text_width(cancel_label);
    let footer_rule = "─".repeat(total_width.saturating_sub(footer_width.saturating_add(4)));
    lines.push(Line::from(vec![
        Span::styled("╰ ".to_string(), border),
        Span::styled(confirm_key.to_string(), confirm_key_style),
        Span::raw(" "),
        Span::styled(confirm_text.to_string(), confirm_label_style),
        Span::raw("    "),
        Span::styled("esc".to_string(), key_style),
        Span::raw(" "),
        Span::styled("cancel".to_string(), cancel_label_style),
        Span::styled(format!(" {footer_rule}╯"), border),
    ]));

    frame.render_widget(Paragraph::new(lines), popup_area);

    let action_y = popup_area
        .y
        .saturating_add(popup_area.height.saturating_sub(1));
    let confirm_x = popup_area.x.saturating_add(2);
    let cancel_x = confirm_x.saturating_add(confirm_width).saturating_add(4);
    app.set_review_delete_confirmation_hits(vec![
        ReviewDeleteConfirmationHit {
            x: confirm_x,
            y: action_y,
            width: confirm_width,
            height: 1,
            action: ReviewDeleteConfirmationAction::Confirm,
        },
        ReviewDeleteConfirmationHit {
            x: cancel_x,
            y: action_y,
            width: text_width(cancel_label) as u16,
            height: 1,
            action: ReviewDeleteConfirmationAction::Cancel,
        },
    ]);
}

fn draw_help_popover(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Calculate popover size and position (centered)
    let popup_width = 61u16.min(area.width.saturating_sub(4));
    let key_style = Style::default().fg(app.theme.accent);
    let label_style = Style::default().fg(app.theme.text);
    let dim_style = Style::default().fg(app.theme.text_muted);
    let section_style = Style::default().fg(app.theme.primary);

    let global = |action| app.keybindings.global_keys(action);
    let normal = |action| app.keybindings.normal_keys(action);
    let help = |action| app.keybindings.help_keys(action);
    let mut help_keys = vec![
        paired(&normal, NormalAction::StepDown, NormalAction::StepUp),
        paired(&normal, NormalAction::PrevHunk, NormalAction::NextHunk),
        paired(&normal, NormalAction::HunkStart, NormalAction::HunkEnd),
        paired(
            &normal,
            NormalAction::TogglePeekChange,
            NormalAction::TogglePeekHunk,
        ),
        paired(&normal, NormalAction::YankChange, NormalAction::YankHunk),
        normal(NormalAction::OpenSearchOrFileFilter),
        paired(&normal, NormalAction::SearchNext, NormalAction::SearchPrev),
        paired(
            &normal,
            NormalAction::FocusNextComment,
            NormalAction::FocusPrevComment,
        ),
        paired(
            &normal,
            NormalAction::NextConflict,
            NormalAction::PrevConflict,
        ),
        paired(
            &normal,
            NormalAction::LineComment,
            NormalAction::HunkComment,
        ),
        paired(
            &normal,
            NormalAction::RemoveLineComment,
            NormalAction::RemoveHunkComment,
        ),
        normal(NormalAction::ClearComments),
        ":<line>".to_string(),
        ":h<num>".to_string(),
        ":s<num>".to_string(),
        paired(&normal, NormalAction::FirstStep, NormalAction::LastStep),
        paired(&normal, NormalAction::GotoStart, NormalAction::GotoEnd),
        paired(&normal, NormalAction::ScrollDown, NormalAction::ScrollUp),
        paired(&normal, NormalAction::ScrollLeft, NormalAction::ScrollRight),
        paired(&normal, NormalAction::LineStart, NormalAction::LineEnd),
        paired(
            &normal,
            NormalAction::HalfPageUp,
            NormalAction::HalfPageDown,
        ),
        normal(NormalAction::TogglePathPopup),
        normal(NormalAction::OpenEditor),
        normal(NormalAction::CenterActive),
        normal(NormalAction::ToggleLineWrap),
        paired(
            &normal,
            NormalAction::ToggleFoldContext,
            NormalAction::ExpandAllFolds,
        ),
        normal(NormalAction::ToggleSyntax),
        normal(NormalAction::ToggleStepping),
        normal(NormalAction::ToggleStrikethrough),
        paired(
            &normal,
            NormalAction::ToggleAutoplay,
            NormalAction::ToggleAutoplayReverse,
        ),
        paired(
            &normal,
            NormalAction::IncreaseSpeed,
            NormalAction::DecreaseSpeed,
        ),
        normal(NormalAction::ToggleAnimation),
        normal(NormalAction::ToggleViewMode),
        normal(NormalAction::ToggleZen),
        normal(NormalAction::ReplayStep),
        global(GlobalAction::OpenCommandPalette),
        global(GlobalAction::OpenFileSearch),
        help(HelpAction::Close),
        normal(NormalAction::Quit),
    ];
    if app.can_show_file_panel() {
        help_keys.extend([
            paired(&normal, NormalAction::PrevFile, NormalAction::NextFile),
            normal(NormalAction::ToggleFilePanel),
            normal(NormalAction::ToggleFileListFocus),
            paired(&normal, NormalAction::StepDown, NormalAction::StepUp),
            normal(NormalAction::OpenSearchOrFileFilter),
        ]);
    }

    let content_width = popup_width.saturating_sub(2) as usize;
    let max_key_width = help_keys
        .iter()
        .map(|key| text_width(key))
        .max()
        .unwrap_or(0);
    let min_desc_width = 16usize;
    let max_key_pad = max_key_width.saturating_add(2).min(12);
    let key_pad = max_key_pad.min(content_width.saturating_sub(min_desc_width).max(2));
    let key_field_width = key_pad.saturating_sub(2);
    let desc_width = content_width.saturating_sub(key_pad).max(1);
    let indent = " ".repeat(key_pad + 5);

    let wrap_text = |text: &str| -> Vec<String> {
        if desc_width == 0 {
            return vec![String::new()];
        }
        let mut lines = Vec::new();
        let mut current = String::new();
        let mut current_width = 0usize;

        let push_chunk = |lines: &mut Vec<String>, chunk: &str| {
            if !chunk.is_empty() {
                lines.push(chunk.to_string());
            }
        };

        let push_word = |lines: &mut Vec<String>, word: &str| {
            let word_width = text_width(word);
            if word_width <= desc_width {
                push_chunk(lines, word);
                return;
            }

            let mut chunk = String::new();
            let mut chunk_width = 0usize;
            for ch in word.chars() {
                let ch_width = text_width(&ch.to_string());
                if chunk_width + ch_width > desc_width && !chunk.is_empty() {
                    lines.push(chunk.clone());
                    chunk.clear();
                    chunk_width = 0;
                }
                if ch_width <= desc_width {
                    chunk.push(ch);
                    chunk_width += ch_width;
                }
            }
            if !chunk.is_empty() {
                lines.push(chunk);
            }
        };

        for word in text.split_whitespace() {
            let word_width = text_width(word);
            if current.is_empty() {
                if word_width <= desc_width {
                    current.push_str(word);
                    current_width = word_width;
                } else {
                    push_word(&mut lines, word);
                }
                continue;
            }

            if current_width + 1 + word_width <= desc_width {
                current.push(' ');
                current.push_str(word);
                current_width += 1 + word_width;
            } else {
                lines.push(current);
                current = String::new();
                current_width = 0;
                if word_width <= desc_width {
                    current.push_str(word);
                    current_width = word_width;
                } else {
                    push_word(&mut lines, word);
                }
            }
        }

        if !current.is_empty() {
            lines.push(current);
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        lines
    };

    let truncate_key = |key: &str| -> String {
        if key_field_width == 0 {
            return String::new();
        }
        let mut out = String::new();
        let mut width = 0usize;
        for ch in key.chars() {
            let ch_width = text_width(&ch.to_string());
            if width + ch_width > key_field_width {
                break;
            }
            out.push(ch);
            width += ch_width;
        }
        out
    };

    let push_help_line = |lines: &mut Vec<Line>, key: &str, desc: &str| {
        let key_text = format!(
            "  {:<width$}     ",
            truncate_key(key),
            width = key_field_width
        );
        let wrapped = wrap_text(desc);
        for (idx, line) in wrapped.into_iter().enumerate() {
            let left = if idx == 0 {
                key_text.clone()
            } else {
                indent.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(left, key_style),
                Span::styled(line, label_style),
            ]));
        }
    };

    let mut lines = vec![Line::from(Span::styled(" Navigation", section_style))];
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::StepDown, NormalAction::StepUp),
        "Step forward/back",
    );
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::PrevHunk, NormalAction::NextHunk),
        "Prev/next hunk",
    );
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::HunkStart, NormalAction::HunkEnd),
        "Hunk begin/end",
    );
    push_help_line(&mut lines, &normal(NormalAction::BlameHint), "Blame (step)");
    push_help_line(
        &mut lines,
        &normal(NormalAction::TogglePeekChange),
        "Peek change",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::TogglePeekHunk),
        "Peek old hunk",
    );
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::YankChange, NormalAction::YankHunk),
        "Yank line/hunk",
    );
    push_help_line(
        &mut lines,
        &paired(
            &normal,
            NormalAction::YankChangePatch,
            NormalAction::YankHunkPatch,
        ),
        "Copy patch (line/hunk)",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::OpenSearchOrFileFilter),
        "Search (diff pane)",
    );
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::SearchNext, NormalAction::SearchPrev),
        "Next/prev match",
    );
    push_help_line(
        &mut lines,
        &paired(
            &normal,
            NormalAction::FocusNextComment,
            NormalAction::FocusPrevComment,
        ),
        "Next/prev review comment",
    );
    push_help_line(
        &mut lines,
        &paired(
            &normal,
            NormalAction::NextConflict,
            NormalAction::PrevConflict,
        ),
        "Next/prev conflict",
    );
    push_help_line(
        &mut lines,
        &paired(
            &normal,
            NormalAction::LineComment,
            NormalAction::HunkComment,
        ),
        "Add/update line/hunk comment",
    );
    push_help_line(
        &mut lines,
        &paired(
            &normal,
            NormalAction::RemoveLineComment,
            NormalAction::RemoveHunkComment,
        ),
        "Remove line/hunk comment",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::ClearComments),
        "Clear all comments",
    );
    push_help_line(&mut lines, ":<line>", "Go to line");
    push_help_line(&mut lines, ":h<num>", "Go to hunk");
    push_help_line(&mut lines, ":s<num>", "Go to step");
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::FirstStep, NormalAction::LastStep),
        "First/last step (or hunk in no-step)",
    );
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::GotoStart, NormalAction::GotoEnd),
        "Go to start/end",
    );
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::ScrollDown, NormalAction::ScrollUp),
        "Scroll up/down",
    );
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::ScrollLeft, NormalAction::ScrollRight),
        "Scroll left/right",
    );
    push_help_line(
        &mut lines,
        &paired(&normal, NormalAction::LineStart, NormalAction::LineEnd),
        "Scroll to line start/end",
    );
    push_help_line(
        &mut lines,
        &paired(
            &normal,
            NormalAction::HalfPageUp,
            NormalAction::HalfPageDown,
        ),
        "Scroll half-page",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::TogglePathPopup),
        "Show full file path",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::OpenEditor),
        "Open file in editor",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::CenterActive),
        "Center on active",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::ToggleLineWrap),
        "Toggle line wrap",
    );
    push_help_line(
        &mut lines,
        &paired(
            &normal,
            NormalAction::ToggleFoldContext,
            NormalAction::ExpandAllFolds,
        ),
        "Toggle/expand all folds",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::ToggleSyntax),
        "Toggle syntax highlight",
    );
    if app.view_mode == ViewMode::Evolution {
        push_help_line(
            &mut lines,
            &normal(NormalAction::ToggleEvoSyntax),
            "Toggle evo syntax (context/full)",
        );
    }
    push_help_line(
        &mut lines,
        &normal(NormalAction::ToggleStepping),
        "Toggle step mode",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::ToggleStrikethrough),
        "Toggle strikethrough",
    );
    push_help_line(
        &mut lines,
        &global(GlobalAction::OpenCommandPalette),
        "Command palette",
    );
    push_help_line(
        &mut lines,
        &global(GlobalAction::OpenFileSearch),
        "Quick file search",
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" Playback", section_style)));
    push_help_line(
        &mut lines,
        &paired(
            &normal,
            NormalAction::ToggleAutoplay,
            NormalAction::ToggleAutoplayReverse,
        ),
        "Autoplay forward/reverse",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::ReplayStep),
        "Replay last step",
    );
    push_help_line(
        &mut lines,
        &counted_binding_label(&normal(NormalAction::ReplayStep)),
        "Replay last n steps",
    );
    push_help_line(
        &mut lines,
        &paired(
            &normal,
            NormalAction::IncreaseSpeed,
            NormalAction::DecreaseSpeed,
        ),
        &format!("Speed ({}ms)", app.animation_speed),
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::ToggleAnimation),
        "Toggle animation",
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" View", section_style)));
    push_help_line(
        &mut lines,
        &normal(NormalAction::ToggleViewMode),
        "Cycle view modes",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::ToggleViewModeReverse),
        "Cycle view modes (reverse)",
    );
    push_help_line(&mut lines, &normal(NormalAction::ToggleZen), "Zen mode");
    push_help_line(
        &mut lines,
        &normal(NormalAction::Refresh),
        "Refresh all files",
    );

    if app.can_show_file_panel() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(" Files", section_style)));
        push_help_line(
            &mut lines,
            &paired(&normal, NormalAction::PrevFile, NormalAction::NextFile),
            "Prev/next file",
        );
        push_help_line(
            &mut lines,
            &normal(NormalAction::ToggleFilePanel),
            "Toggle file panel",
        );
        push_help_line(
            &mut lines,
            &normal(NormalAction::ToggleFileListFocus),
            "Focus file list",
        );
        push_help_line(
            &mut lines,
            &paired(&normal, NormalAction::StepDown, NormalAction::StepUp),
            "Move selection (focused)",
        );
        push_help_line(
            &mut lines,
            &normal(NormalAction::OpenSearchOrFileFilter),
            "Filter files (when focused)",
        );
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(format!("  {:<12}", help(HelpAction::Close)), key_style),
        Span::styled("Close help", dim_style),
    ]));
    let quit_label = "Quit (prints comments if any)";
    lines.push(Line::from(vec![
        Span::styled(format!("  {:<12}", normal(NormalAction::Quit)), key_style),
        Span::styled(quit_label, label_style),
    ]));

    let base_height = if app.can_show_file_panel() { 31 } else { 26 };
    let min_height = (base_height as u16).min(area.height.saturating_sub(4));
    let needed_height = (lines.len() as u16).saturating_add(2);
    let popup_height = needed_height
        .max(min_height)
        .min(area.height.saturating_sub(4));
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area behind the popup
    frame.render_widget(Clear, popup_area);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_active));
    block = block.border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background {
        block = block.style(Style::default().bg(bg));
    }

    let inner_height = popup_height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_height);
    app.help_max_scroll = max_scroll;
    let scroll = app.help_scroll.min(max_scroll) as u16;
    let total_lines = max_scroll + inner_height;
    let help_block = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Left)
        .scroll((scroll, 0));

    frame.render_widget(help_block, popup_area);

    // Render scrollbar if content overflows
    if max_scroll > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(total_lines).position(scroll as usize);
        frame.render_stateful_widget(
            scrollbar,
            popup_area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn draw_path_popup(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let file_path = app.current_file_path();

    // Calculate popup size based on path length
    let popup_width = (file_path.len() as u16 + 6).min(area.width.saturating_sub(4));
    let popup_height = 3u16;
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area behind the popup
    frame.render_widget(Clear, popup_area);

    // Truncate path if too long for popup
    let max_path_len = (popup_width.saturating_sub(4)) as usize;
    let display_path = if file_path.len() > max_path_len {
        format!(
            "…{}",
            &file_path[file_path.len().saturating_sub(max_path_len - 1)..]
        )
    } else {
        file_path
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(" File Path ")
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(app.theme.border_active));
    block = block.border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background {
        block = block.style(Style::default().bg(bg));
    }

    let path_block = Paragraph::new(display_path)
        .block(block)
        .style(Style::default().fg(app.theme.text))
        .alignment(Alignment::Center);

    frame.render_widget(path_block, popup_area);
}

fn picker_input_line(app: &App, query: &str, placeholder: &str, width: u16) -> Line<'static> {
    let prompt_style = Style::default()
        .fg(app.theme.primary)
        .add_modifier(Modifier::BOLD);
    let content_width = width.saturating_sub(3) as usize;
    let (text, text_style) = if query.is_empty() {
        (
            truncate_text(placeholder, content_width),
            Style::default().fg(app.theme.text_muted),
        )
    } else {
        (
            truncate_text_from_start(query, content_width),
            Style::default().fg(app.theme.text),
        )
    };
    let prompt = Span::styled("❯ ", prompt_style);
    let text = Span::styled(text, text_style);
    let cursor = Span::styled(
        if app.file_filter_cursor_visible {
            "│"
        } else {
            " "
        },
        prompt_style,
    );
    if query.is_empty() {
        Line::from(vec![prompt, cursor, text])
    } else {
        Line::from(vec![prompt, text, cursor])
    }
}

fn draw_command_palette_popover(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let popup_width = 56u16.min(area.width.saturating_sub(4));
    let max_height = (area.height / 2).saturating_sub(2).max(6);
    let entries = app.command_palette_filtered_entries();
    let selection = app.command_palette_selection();
    let item_height = 1u16;
    let overhead = 6u16;
    let max_list_height = max_height.saturating_sub(overhead).max(1) as usize;
    let list_height = entries.len().max(1).min(max_list_height);
    let popup_height = (list_height as u16)
        .saturating_add(overhead)
        .min(max_height);

    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let desired_y = area.height / 4;
    let max_y = area.height.saturating_sub(popup_height);
    let popup_y = desired_y.min(max_y);
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded);
    block = block.border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), popup_area);
    let inner = block.inner(popup_area);
    let padded = inner.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let content = if padded.width > 0 && padded.height > 0 {
        padded
    } else {
        inner
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(content);

    let input_line = picker_input_line(
        app,
        app.command_palette_query(),
        "Search for commands…",
        chunks[0].width,
    );
    frame.render_widget(
        Paragraph::new(vec![input_line]).alignment(Alignment::Left),
        chunks[0],
    );

    if entries.is_empty() {
        app.set_command_palette_list_area(None, 0, 0, 1);
        let line = Line::from(Span::styled(
            "No results",
            Style::default().fg(app.theme.text_muted),
        ));
        frame.render_widget(
            Paragraph::new(vec![line]).alignment(Alignment::Center),
            chunks[1],
        );
        return;
    }

    let mut start = 0usize;
    if selection >= list_height {
        start = selection + 1 - list_height;
    }
    let end = (start + list_height).min(entries.len());
    let visible = &entries[start..end];
    let list_width = chunks[1].width.saturating_sub(2) as usize;
    app.set_command_palette_list_area(
        Some((chunks[1].x, chunks[1].y, chunks[1].width, chunks[1].height)),
        start,
        visible.len(),
        item_height,
    );

    let items: Vec<ListItem> = visible
        .iter()
        .map(|entry| {
            let label = truncate_text(&entry.label, list_width);
            ListItem::new(Line::from(Span::styled(
                label,
                Style::default().fg(app.theme.text),
            )))
        })
        .collect();

    let mut state = ListState::default();
    let selection_in_view = selection.saturating_sub(start);
    state.select(Some(selection_in_view.min(visible.len().saturating_sub(1))));
    let mut highlight_style = Style::default().fg(app.theme.accent);
    if let Some(bg) = app.theme.background_element.or(app.theme.background_panel) {
        highlight_style = highlight_style.bg(bg);
    }
    let list = List::new(items).highlight_style(highlight_style);
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

fn draw_theme_picker_popover(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let popup_width = 60u16.min(area.width.saturating_sub(4));
    let max_height = (area.height / 2).saturating_sub(2).max(6);
    let names = app.theme_picker_filtered_names();
    let selection = app.theme_picker_selection();
    let item_height = 1u16;
    let overhead = 6u16;
    let max_list_height = max_height.saturating_sub(overhead).max(1) as usize;
    let list_height = names.len().max(1).min(max_list_height);
    let popup_height = (list_height as u16)
        .saturating_add(overhead)
        .min(max_height);

    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let desired_y = area.height / 4;
    let max_y = area.height.saturating_sub(popup_height);
    let popup_y = desired_y.min(max_y);
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded);
    block = block.border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), popup_area);
    let inner = block.inner(popup_area);
    let padded = inner.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let content = if padded.width > 0 && padded.height > 0 {
        padded
    } else {
        inner
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(content);

    let input_line = picker_input_line(
        app,
        app.theme_picker_query(),
        "Search for themes…",
        chunks[0].width,
    );
    frame.render_widget(
        Paragraph::new(vec![input_line]).alignment(Alignment::Left),
        chunks[0],
    );

    if names.is_empty() {
        app.set_theme_picker_list_area(None, 0, 0, 1);
        let line = Line::from(Span::styled(
            "No results",
            Style::default().fg(app.theme.text_muted),
        ));
        frame.render_widget(
            Paragraph::new(vec![line]).alignment(Alignment::Center),
            chunks[1],
        );
        return;
    }

    let mut start = 0usize;
    if selection >= list_height {
        start = selection + 1 - list_height;
    }
    let end = (start + list_height).min(names.len());
    let visible = &names[start..end];
    let list_width = chunks[1].width.saturating_sub(2) as usize;
    app.set_theme_picker_list_area(
        Some((chunks[1].x, chunks[1].y, chunks[1].width, chunks[1].height)),
        start,
        visible.len(),
        item_height,
    );

    let items: Vec<ListItem> = visible
        .iter()
        .map(|name| {
            let current = app.ui_theme_name.as_deref() == Some(name.as_str());
            let suffix = if current { " current" } else { "" };
            let label = truncate_text(&format!("{name}{suffix}"), list_width);
            let style = if current {
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.text)
            };
            ListItem::new(Line::from(Span::styled(label, style)))
        })
        .collect();

    let mut state = ListState::default();
    let selection_in_view = selection.saturating_sub(start);
    state.select(Some(selection_in_view.min(visible.len().saturating_sub(1))));
    let mut highlight_style = Style::default().fg(app.theme.accent);
    if let Some(bg) = app.theme.background_element.or(app.theme.background_panel) {
        highlight_style = highlight_style.bg(bg);
    }
    let list = List::new(items).highlight_style(highlight_style);
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

fn draw_file_search_popover(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let popup_width = 60u16.min(area.width.saturating_sub(4));
    let max_height = (area.height / 2).saturating_sub(2).max(6);
    let indices = app.file_search_filtered_indices();
    let selection = app.file_search_selection();
    let item_height = 1u16;
    let overhead = 6u16;
    let max_list_height = max_height.saturating_sub(overhead).max(1) as usize;
    let list_height = indices.len().max(1).min(max_list_height);
    let popup_height = (list_height as u16)
        .saturating_add(overhead)
        .min(max_height);

    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let desired_y = area.height / 4;
    let max_y = area.height.saturating_sub(popup_height);
    let popup_y = desired_y.min(max_y);
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded);
    block = block.border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), popup_area);
    let inner = block.inner(popup_area);
    let padded = inner.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let content = if padded.width > 0 && padded.height > 0 {
        padded
    } else {
        inner
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(content);

    let input_line = picker_input_line(
        app,
        app.file_search_query(),
        "Search for files…",
        chunks[0].width,
    );
    frame.render_widget(
        Paragraph::new(vec![input_line]).alignment(Alignment::Left),
        chunks[0],
    );

    if indices.is_empty() {
        app.set_file_search_list_area(None, 0, 0, 1);
        let line = Line::from(Span::styled(
            "No results",
            Style::default().fg(app.theme.text_muted),
        ));
        frame.render_widget(
            Paragraph::new(vec![line]).alignment(Alignment::Center),
            chunks[1],
        );
        return;
    }

    let mut start = 0usize;
    if selection >= list_height {
        start = selection + 1 - list_height;
    }
    let end = (start + list_height).min(indices.len());
    let visible = &indices[start..end];
    let list_width = chunks[1].width.saturating_sub(2) as usize;
    app.set_file_search_list_area(
        Some((chunks[1].x, chunks[1].y, chunks[1].width, chunks[1].height)),
        start,
        visible.len(),
        item_height,
    );

    let items: Vec<ListItem> = visible
        .iter()
        .map(|idx| {
            let name = app.multi_diff.files[*idx].display_name.clone();
            let label = truncate_path(&name, list_width);
            ListItem::new(Line::from(Span::styled(
                label,
                Style::default().fg(app.theme.text),
            )))
        })
        .collect();

    let mut state = ListState::default();
    let selection_in_view = selection.saturating_sub(start);
    state.select(Some(selection_in_view.min(visible.len().saturating_sub(1))));
    let mut highlight_style = Style::default().fg(app.theme.accent);
    if let Some(bg) = app.theme.background_element.or(app.theme.background_panel) {
        highlight_style = highlight_style.bg(bg);
    }
    let list = List::new(items).highlight_style(highlight_style);
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

fn draw_comment_picker_popover(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let popup_width = 72u16.min(area.width.saturating_sub(4));
    let max_height = (area.height / 2).saturating_sub(2).max(6);
    let indices = app.comment_picker_filtered_indices();
    let selection = app.comment_picker_selection();
    let item_height = 1u16;
    let overhead = 6u16;
    let max_list_height = max_height.saturating_sub(overhead).max(1) as usize;
    let list_height = indices.len().max(1).min(max_list_height);
    let popup_height = (list_height as u16)
        .saturating_add(overhead)
        .min(max_height);

    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let desired_y = area.height / 4;
    let max_y = area.height.saturating_sub(popup_height);
    let popup_y = desired_y.min(max_y);
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded);
    block = block.border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background {
        block = block.style(Style::default().bg(bg));
    }
    frame.render_widget(block.clone(), popup_area);
    let inner = block.inner(popup_area);
    let padded = inner.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let content = if padded.width > 0 && padded.height > 0 {
        padded
    } else {
        inner
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(content);

    let input_line = picker_input_line(
        app,
        app.comment_picker_query(),
        "Search for comments…",
        chunks[0].width,
    );
    frame.render_widget(
        Paragraph::new(vec![input_line]).alignment(Alignment::Left),
        chunks[0],
    );

    if indices.is_empty() {
        app.set_comment_picker_list_area(None, 0, 0, 1);
        let line = Line::from(Span::styled(
            "No comments",
            Style::default().fg(app.theme.text_muted),
        ));
        frame.render_widget(
            Paragraph::new(vec![line]).alignment(Alignment::Center),
            chunks[1],
        );
        return;
    }

    let mut start = 0usize;
    if selection >= list_height {
        start = selection + 1 - list_height;
    }
    let end = (start + list_height).min(indices.len());
    let visible = &indices[start..end];
    let list_width = chunks[1].width.saturating_sub(2) as usize;
    app.set_comment_picker_list_area(
        Some((chunks[1].x, chunks[1].y, chunks[1].width, chunks[1].height)),
        start,
        visible.len(),
        item_height,
    );

    let items: Vec<ListItem> = visible
        .iter()
        .filter_map(|idx| app.review_comment_sidebar_item(*idx))
        .map(
            |(_file_index, title, location, preview, outdated, resolved)| {
                let target = if location.is_empty() {
                    title
                } else {
                    format!("{title} {location}")
                };
                let label = truncate_text(&format!("{target} - {preview}"), list_width);
                let mut style = Style::default().fg(if outdated || resolved {
                    app.theme.text_muted
                } else {
                    app.theme.text
                });
                if outdated || resolved {
                    style = style.add_modifier(Modifier::DIM);
                }
                ListItem::new(Line::from(Span::styled(label, style)))
            },
        )
        .collect();

    let mut state = ListState::default();
    let selection_in_view = selection.saturating_sub(start);
    state.select(Some(selection_in_view.min(visible.len().saturating_sub(1))));
    let mut highlight_style = Style::default().fg(app.theme.accent);
    if let Some(bg) = app.theme.background_element.or(app.theme.background_panel) {
        highlight_style = highlight_style.bg(bg);
    }
    let list = List::new(items).highlight_style(highlight_style);
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

#[cfg(test)]
mod tests {
    use super::counted_binding_label;
    use crate::app::{App, FoldContextDirection, SelectionToolbarAction, ViewMode};
    use crate::config::{FoldContextMode, ResolvedTheme, SyntaxMode};
    use crate::markdown::{
        markdown_line_is_quote_border, markdown_preview_lines as render_markdown_preview_lines,
        MarkdownChangeBars, PreviewLink,
    };
    use crate::structured_preview::StructuredPreviewKind;
    use crate::syntax::SyntaxSide;
    use oyo_core::MultiFileDiff;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Line;
    use ratatui::Terminal;
    use std::collections::HashMap;

    /// Render Markdown through the real preview renderer without needing an
    /// `App`, using the default theme and no syntax highlighting.
    fn render_md(md: &str, width: usize) -> Vec<Line<'static>> {
        render_md_themed(md, width, ResolvedTheme::default())
    }

    fn render_md_themed(md: &str, width: usize, theme: ResolvedTheme) -> Vec<Line<'static>> {
        render_md_full(md, width, theme).0
    }

    fn render_md_full(
        md: &str,
        width: usize,
        theme: ResolvedTheme,
    ) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
        let mut highlight = |_lang: Option<&str>, _code: &str| None;
        render_markdown_preview_lines(md, &theme, width, None, &mut highlight, None)
    }

    /// An opaque RGB theme so background-dependent features (heading bands,
    /// inline chips, code panels) are actually exercised. Foreground tokens are
    /// RGB too, since `blend_colors` only blends true-color values.
    fn rgb_theme() -> ResolvedTheme {
        use ratatui::style::Color::Rgb;
        ResolvedTheme {
            text: Rgb(0xcd, 0xd6, 0xf4),
            accent: Rgb(0xf5, 0xc2, 0xe7),
            info: Rgb(0x89, 0xb4, 0xfa),
            success: Rgb(0xa6, 0xe3, 0xa1),
            warning: Rgb(0xf9, 0xe2, 0xaf),
            primary: Rgb(0xcb, 0xa6, 0xf7),
            background: Some(Rgb(0x1e, 0x1e, 0x2e)),
            background_element: Some(Rgb(0x31, 0x32, 0x44)),
            background_panel: Some(Rgb(0x28, 0x28, 0x38)),
            ..ResolvedTheme::default()
        }
    }

    fn line_width(line: &Line<'static>) -> usize {
        line.spans
            .iter()
            .map(|s| super::text_width(&s.content))
            .sum()
    }

    /// Flatten every span's text with lines joined by `\n` for substring checks.
    fn flatten(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn topbar_tabs_show_overflow_indicators() {
        let diff = MultiFileDiff::from_file_pairs(
            (0..6)
                .map(|idx| {
                    (
                        std::path::PathBuf::from(format!("file-{idx}")),
                        "old\n".to_string(),
                        "new\n".to_string(),
                    )
                })
                .collect(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        for _ in 0..4 {
            app.new_topbar_tab();
        }

        let text = super::topbar_tab_spans(&mut app, Rect::new(0, 0, 18, 1), 18)
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.contains('›'));
        assert!(text.contains(" + "));
        assert!(app.topbar_scroll_right_hit.is_some());
        assert!(app.topbar_plus_hit.is_some());

        app.topbar_tab_scroll = 1;
        let text = super::topbar_tab_spans(&mut app, Rect::new(0, 0, 18, 1), 18)
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.starts_with("‹ "));
        assert!(app.topbar_scroll_left_hit.is_some());
    }

    #[test]
    fn no_changes_quit_hint_styles_hotkey_and_hover() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 50, false, None);
        app.watch = true;
        app.no_changes_dashboard_hover = true;
        app.no_changes_quit_hover = true;

        let line = super::no_changes_hint_line(&app);

        assert_eq!(line.spans[1].content.as_ref(), "ctrl-r");
        assert_eq!(line.spans[1].style.fg, Some(app.theme.accent));
        assert_eq!(line.spans[2].style.fg, Some(app.theme.text_muted));
        assert!(line.spans[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[4].content.as_ref(), "q");
        assert_eq!(line.spans[4].style.fg, Some(app.theme.accent));
        assert!(line.spans[5].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn picker_cursor_precedes_placeholder_and_follows_query() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 50, false, None);
        let text = |line: ratatui::text::Line<'_>| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        };

        assert_eq!(
            text(super::picker_input_line(&app, "", "Search for files…", 30)),
            "❯ │Search for files…"
        );
        assert_eq!(
            text(super::picker_input_line(
                &app,
                "abc",
                "Search for files…",
                30
            )),
            "❯ abc│"
        );

        app.file_filter_cursor_visible = false;
        assert_eq!(
            text(super::picker_input_line(&app, "", "Search for files…", 30)),
            "❯  Search for files…"
        );
    }

    #[test]
    fn file_filter_active_text_uses_prompt_and_ibeam() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 50, false, None);
        app.file_filter_active = true;
        app.file_filter = "abc".to_string();

        let line = super::file_filter_line(&app, true, 20);
        let text = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(text, " ❯ abc│");
        assert_eq!(line.spans[1].style.fg, Some(app.theme.primary));
        assert_eq!(line.spans[3].style.fg, Some(app.theme.primary));
    }

    #[test]
    fn file_filter_active_keeps_tail_when_truncated() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 50, false, None);
        app.file_filter_active = true;
        app.file_filter = "abcdefghijkl".to_string();

        let text = super::file_filter_line(&app, true, 12)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(text, " ❯ …ijkl│");
    }

    #[test]
    fn file_filter_empty_active_has_no_placeholder() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 50, false, None);
        app.file_filter_active = true;

        let text = super::file_filter_line(&app, false, 20)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(text, " ❯ │");
    }

    #[test]
    fn file_filter_blurred_query_keeps_prompt() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 50, false, None);
        app.file_filter = "abc".to_string();

        let line = super::file_filter_line(&app, true, 20);
        let text = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(text, " ❯ abc");
        assert_eq!(line.spans[1].style.fg, Some(app.theme.primary));
    }

    #[test]
    fn file_filter_hint_uses_hover_style() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 50, false, None);
        app.file_filter_hover = true;

        let line = super::file_filter_line(&app, false, 20);
        assert_eq!(line.spans[1].content.as_ref(), "/ Filter");
        assert_eq!(line.spans[1].style.fg, Some(app.theme.accent));
        assert!(line.spans[1].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn search_prompt_is_not_in_status_bar() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("README.md"),
            std::path::PathBuf::from("README.md"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 50, false, None);
        app.start_search();
        assert!(super::line_input_status_spans(&app).is_none());
    }

    #[test]
    fn image_files_are_renderable_previews() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("image.png"),
            std::path::PathBuf::from("image.png"),
            String::new(),
            String::new(),
        );
        let app = App::new(multi, ViewMode::Preview, 50, false, None);
        assert!(super::preview_can_render_image(&app));
    }

    #[test]
    fn image_preview_topbar_has_no_source_toggle() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("image.png"),
            std::path::PathBuf::from("image.png"),
            String::new(),
            String::new(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 50, false, None);
        let backend = TestBackend::new(50, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::draw_top_bar(frame, &mut app, Rect::new(0, 0, 50, 1)))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(!text.contains("source"), "topbar text: {text:?}");
        assert!(app.preview_toggle_hit.is_none());
    }

    #[test]
    fn preview_status_bar_has_no_render_state_label() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("README.md"),
            std::path::PathBuf::from("README.md"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 50, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.start_line_comment();
        app.review_insert_char('x');
        app.review_save_editor();
        app.view_mode = ViewMode::Preview;
        let backend = TestBackend::new(50, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::draw_preview_status_bar(frame, &mut app, Rect::new(0, 0, 50, 1)))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(!text.contains("source"), "status text: {text:?}");
        assert!(!text.contains("preview"), "status text: {text:?}");
        assert!(text.contains("1 comment"), "status text: {text:?}");
        assert!(text.contains("file 1/1"), "status text: {text:?}");
        assert!(app.status_comments_hit.is_some());
        assert!(app.status_file_hit.is_some());
    }

    #[test]
    fn find_bar_renders_controls_and_avoids_active_match() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("search.txt"),
            std::path::PathBuf::from("search.txt"),
            String::new(),
            "........................................target\none\ntwo\nthree\ntarget\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 0, false, None);
        app.file_panel_visible = false;
        app.toggle_stepping();
        app.start_search();
        for ch in "target".chars() {
            app.push_search_char(ch);
        }
        app.search_next();
        app.search_prev();

        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let (view_x, view_y, view_width, _) = app.diff_view_area.unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(view_x, view_y)].symbol(), "╭");
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("❯ target│"), "find bar text: {text:?}");
        assert!(text.contains("1/2"), "find bar text: {text:?}");
        assert!(text.contains('│'), "find bar text: {text:?}");
        assert!(text.contains('‹'), "find bar text: {text:?}");
        assert!(text.contains('›'), "find bar text: {text:?}");
        assert!(text.contains('✕'), "find bar text: {text:?}");

        let (prev_x, prev_y, _, _) = app.search_prev_hit.expect("previous hit");
        assert!(app.update_search_bar_hover(prev_x, prev_y));
        assert!(app.search_prev_hover);
        let (next_x, next_y, _, _) = app.search_next_hit.expect("next hit");
        assert!(app.handle_search_bar_click(next_x, next_y));
        assert_eq!(app.search_target(), Some(4));
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let content_width = view_width.saturating_sub(u16::from(app.scrollbar_visible));
        let right_x = view_x.saturating_add(content_width.saturating_sub(44));
        assert_eq!(terminal.backend().buffer()[(right_x, view_y)].symbol(), "╭");

        let (bar_x, bar_y, _, _) = app.search_bar_hit.expect("find bar hit");
        assert!(app.handle_search_bar_click(bar_x, bar_y));
        assert!(app.search_active());

        let (clear_x, clear_y, clear_width, _) = app.search_clear_hit.expect("clear hit");
        assert!(app.handle_search_bar_click(
            clear_x.saturating_add(clear_width.saturating_sub(1)),
            clear_y,
        ));
        assert!(!app.search_active());
        assert!(app.search_query().is_empty());
        assert!(!app.search_bar_visible());
    }

    #[test]
    fn expandable_fold_renders_and_expands_by_mouse_in_unified_and_split() {
        let _guard = crate::test_utils::DiffSettingsGuard::default();
        let content = (1..=40)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        for mode in [ViewMode::UnifiedPane, ViewMode::Split] {
            let multi = MultiFileDiff::from_file_pair(
                std::path::PathBuf::from("fold.txt"),
                std::path::PathBuf::from("fold.txt"),
                content.clone(),
                content.clone(),
            );
            let mut app = App::new(multi, mode, 0, false, None);
            app.file_panel_visible = false;
            app.theme = rgb_theme();
            app.set_fold_context_mode(FoldContextMode::Expandable);

            let backend = TestBackend::new(100, 18);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                text.contains("ua ↑ 34 unchanged lines da ↓"),
                "fold text: {text:?}"
            );
            assert_eq!(
                app.fold_context_hits.len(),
                if mode == ViewMode::Split { 4 } else { 2 }
            );
            let top = app
                .fold_context_hits
                .iter()
                .find(|hit| hit.direction == FoldContextDirection::Top)
                .copied()
                .unwrap();
            let initial_bottom = app
                .fold_context_hits
                .iter()
                .find(|hit| hit.direction == FoldContextDirection::Bottom)
                .copied()
                .unwrap();
            let top_arrow_x = top.x.saturating_add(top.width.saturating_sub(1));
            let bottom_arrow_x = initial_bottom
                .x
                .saturating_add(initial_bottom.width.saturating_sub(1));
            let up_key = &terminal.backend().buffer()[(top.x, top.y)];
            let down_key = &terminal.backend().buffer()[(initial_bottom.x, initial_bottom.y)];
            assert_eq!(up_key.fg, app.theme.text_muted);
            assert_eq!(down_key.fg, app.theme.text_muted);
            assert!(!up_key.modifier.contains(Modifier::BOLD));
            assert!(!down_key.modifier.contains(Modifier::BOLD));
            let top_arrow = &terminal.backend().buffer()[(top_arrow_x, top.y)];
            assert_eq!(top_arrow.fg, app.theme.text_muted);
            assert!(!top_arrow.modifier.contains(Modifier::BOLD));

            assert!(app.update_topbar_hover(top.x, top.y));
            terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
            let top_arrow = &terminal.backend().buffer()[(top_arrow_x, top.y)];
            assert_eq!(top_arrow.fg, app.theme.accent);
            assert!(top_arrow.modifier.contains(Modifier::BOLD));
            let up_key = &terminal.backend().buffer()[(top.x, top.y)];
            let down_key = &terminal.backend().buffer()[(initial_bottom.x, initial_bottom.y)];
            assert_eq!(up_key.symbol(), "u");
            assert_eq!(up_key.fg, app.theme.accent);
            assert!(up_key.modifier.contains(Modifier::BOLD));
            assert_eq!(down_key.fg, app.theme.text_muted);
            assert!(!down_key.modifier.contains(Modifier::BOLD));
            let fold_bg = crate::views::fold_context_background(&app);
            let (view_x, _, view_width, _) = app.diff_view_area.unwrap();
            assert_eq!(terminal.backend().buffer()[(view_x, top.y)].bg, fold_bg);
            assert_eq!(
                terminal.backend().buffer()
                    [(view_x.saturating_add(view_width.saturating_sub(2)), top.y)]
                    .bg,
                fold_bg
            );

            assert!(app.update_topbar_hover(bottom_arrow_x, initial_bottom.y));
            terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
            let up_key = &terminal.backend().buffer()[(top.x, top.y)];
            let down_key = &terminal.backend().buffer()[(initial_bottom.x, initial_bottom.y)];
            assert!(!up_key.modifier.contains(Modifier::BOLD));
            assert!(down_key.modifier.contains(Modifier::BOLD));
            assert!(
                terminal.backend().buffer()[(bottom_arrow_x, initial_bottom.y)]
                    .modifier
                    .contains(Modifier::BOLD)
            );

            assert!(app.handle_fold_context_click(top.x, top.y));
            terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                text.contains("ua ↑ 14 unchanged lines da ↓"),
                "fold text: {text:?}"
            );

            let bottom = app
                .fold_context_hits
                .iter()
                .find(|hit| hit.direction == FoldContextDirection::Bottom)
                .copied()
                .unwrap();
            assert!(app.handle_fold_context_click(
                bottom.x.saturating_add(bottom.width.saturating_sub(1)),
                bottom.y,
            ));
            terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
            assert!(app.fold_context_hits.is_empty());
        }
    }

    #[test]
    fn visual_selection_skips_fold_bands() {
        let _guard = crate::test_utils::DiffSettingsGuard::default();
        let content = (1..=40)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        for mode in [ViewMode::UnifiedPane, ViewMode::Split] {
            let multi = MultiFileDiff::from_file_pair(
                std::path::PathBuf::from("fold.txt"),
                std::path::PathBuf::from("fold.txt"),
                content.clone(),
                content.clone(),
            );
            let mut app = App::new(multi, mode, 0, false, None);
            app.file_panel_visible = false;
            app.theme = rgb_theme();
            app.set_fold_context_mode(FoldContextMode::Expandable);

            let backend = TestBackend::new(100, 18);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
            let fold_hit = app.fold_context_hits.first().copied().unwrap();
            let (view_x, view_y, _, _) = app.diff_view_area.unwrap();
            let fold_row = fold_hit.y.saturating_sub(view_y);
            let content_x = view_x + app.diff_selection_content_ranges()[0].0;

            assert!(!app.start_diff_selection(content_x, fold_hit.y));
            assert!(app.start_diff_selection(content_x, fold_hit.y.saturating_sub(1)));
            assert!(app.drag_diff_selection(content_x.saturating_add(6), fold_hit.y));
            assert_eq!(
                app.control_selection_json()["end"]["row"].as_u64(),
                Some(fold_row.saturating_add(1) as u64)
            );
            let selected = app.selected_diff_text();
            assert_eq!(selected, "line 3\nline 38", "mode: {mode:?}");
            assert!(app.move_diff_selection(0, -1));
            assert_eq!(
                app.control_selection_json()["end"]["row"].as_u64(),
                Some(fold_row.saturating_sub(1) as u64)
            );
            assert!(app.move_diff_selection(0, 1));
            assert_eq!(
                app.control_selection_json()["end"]["row"].as_u64(),
                Some(fold_row.saturating_add(1) as u64)
            );

            terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
            let fold_hit = app.fold_context_hits.first().copied().unwrap();
            assert_eq!(
                terminal.backend().buffer()[(fold_hit.x, fold_hit.y)].bg,
                crate::views::fold_context_background(&app)
            );
        }
    }

    #[test]
    fn visible_folds_get_contextual_direction_keys() {
        let _guard = crate::test_utils::DiffSettingsGuard::default();
        let old = (1..=60)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut new_lines = old.lines().map(str::to_string).collect::<Vec<_>>();
        new_lines[29] = "changed line".to_string();
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("fold.txt"),
            std::path::PathBuf::from("fold.txt"),
            format!("{old}\n"),
            format!("{}\n", new_lines.join("\n")),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 0, false, None);
        app.file_panel_visible = false;
        app.set_fold_context_mode(FoldContextMode::Expandable);
        app.next_step();

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("ua ↑"), "fold text: {text:?}");
        assert!(text.contains("da ↓"), "fold text: {text:?}");
        assert!(text.contains("ub ↑"), "fold text: {text:?}");
        assert!(text.contains("db ↓"), "fold text: {text:?}");
    }

    #[test]
    fn expandable_fold_hitboxes_follow_wrapped_context() {
        let _guard = crate::test_utils::DiffSettingsGuard::default();
        let content = (1..=40)
            .map(|line| {
                if line <= 3 {
                    "x".repeat(120)
                } else {
                    format!("line {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        for mode in [ViewMode::UnifiedPane, ViewMode::Split] {
            let multi = MultiFileDiff::from_file_pair(
                std::path::PathBuf::from("fold.txt"),
                std::path::PathBuf::from("fold.txt"),
                content.clone(),
                content.clone(),
            );
            let mut app = App::new(multi, mode, 0, false, None);
            app.file_panel_visible = false;
            app.line_wrap = true;
            app.set_fold_context_mode(FoldContextMode::Expandable);

            let backend = TestBackend::new(40, 80);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
            let expected_hits = if mode == ViewMode::Split { 2 } else { 1 };
            for (direction, symbol) in [
                (FoldContextDirection::Top, "↑"),
                (FoldContextDirection::Bottom, "↓"),
            ] {
                let hits = app
                    .fold_context_hits
                    .iter()
                    .filter(|hit| hit.direction == direction)
                    .collect::<Vec<_>>();
                assert_eq!(hits.len(), expected_hits);
                for hit in hits {
                    let arrow_x = hit.x.saturating_add(hit.width.saturating_sub(1));
                    assert_eq!(
                        terminal.backend().buffer()[(arrow_x, hit.y)].symbol(),
                        symbol
                    );
                }
            }
            let bottom = app
                .fold_context_hits
                .iter()
                .find(|hit| hit.direction == FoldContextDirection::Bottom)
                .copied()
                .unwrap();
            assert!(app.handle_fold_context_click(bottom.x, bottom.y));
        }
    }

    #[test]
    fn binary_preview_shows_file_comment_action_and_comment() {
        let multi = MultiFileDiff::from_file_pair_bytes(
            std::path::PathBuf::from("file.bin"),
            vec![0, 1],
            vec![0, 2],
        );
        let mut app = App::new(multi, ViewMode::Preview, 50, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        assert!(app.start_file_comment());
        app.review_insert_char('o');
        app.review_insert_char('k');
        app.review_save_editor();

        let backend = TestBackend::new(50, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 50, 8)))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("m comment"), "preview text: {text:?}");
        assert!(text.contains("ok"), "preview text: {text:?}");
        assert!(app.review_file_comment_hit.is_some());
    }

    #[test]
    fn preview_file_comment_actions_use_their_drawn_hitboxes() {
        let path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/assets/preview.png"
        ));
        let image = std::fs::read(&path).unwrap();
        let multi = MultiFileDiff::from_file_pair_bytes(path, Vec::new(), image);
        let mut app = App::new(multi, ViewMode::Preview, 80, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.diff_view_area = Some((0, 0, 80, 20));
        assert!(app.start_file_comment());
        for ch in "Review this file".chars() {
            app.review_insert_char(ch);
        }
        app.review_save_editor();
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 80, 20)))
            .unwrap();
        let lines = ascii_buffer_lines(&terminal);

        let (resolve_x, resolve_y) = text_pos(&lines, "va resolve").unwrap();
        assert!(app.handle_review_preview_click(resolve_x + 2, resolve_y));
        let comments: serde_json::Value =
            serde_json::from_str(&app.review_comments_json()).unwrap();
        assert_eq!(comments["comments"][0]["resolved"], true);
        assert!(!app.review_editor_active());

        let (overflow_x, overflow_y) = text_pos(&lines, "oa").unwrap();
        assert!(app.handle_review_preview_click(overflow_x + 1, overflow_y));
        assert!(app.review_comment_context_menu.is_some());
    }

    fn draw_diff_snapshot(app: &mut App) -> (Vec<String>, Vec<Vec<Style>>) {
        let width = if app.view_mode == ViewMode::Split {
            140
        } else {
            80
        };
        let backend = TestBackend::new(width, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::draw_diff_view(frame, app, Rect::new(0, 0, width, 12)))
            .unwrap();
        let area = terminal.backend().buffer().area;
        let mut lines = Vec::new();
        let mut styles = Vec::new();
        for y in 0..area.height {
            let mut line = String::new();
            let mut row_styles = Vec::new();
            for x in 0..area.width {
                let cell = &terminal.backend().buffer()[(x, y)];
                let symbol = cell.symbol();
                if symbol.is_ascii() {
                    line.push_str(symbol);
                } else {
                    line.push(' ');
                }
                row_styles.push(cell.style());
            }
            lines.push(line);
            styles.push(row_styles);
        }
        (lines, styles)
    }

    fn rendered_diff_with_comment(mode: ViewMode) -> (App, Vec<String>) {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("README.md"),
            std::path::PathBuf::from("README.md"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, mode, 80, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.start_line_comment();
        for ch in "hello".chars() {
            app.review_insert_char(ch);
        }
        app.review_save_editor();

        let (lines, _) = draw_diff_snapshot(&mut app);
        (app, lines)
    }

    fn text_pos(lines: &[String], needle: &str) -> Option<(u16, u16)> {
        lines
            .iter()
            .enumerate()
            .find_map(|(y, line)| line.find(needle).map(|x| (x as u16, y as u16)))
    }

    fn mark_pr_target(app: &mut App) {
        app.set_review_target_metadata(Some(crate::app::review::ReviewTargetMetadata {
            label: "PR".to_string(),
            vcs: "git".to_string(),
            jj_change_id: None,
            jj_commit_id: None,
            git_base_ref: None,
            git_head_ref: None,
            git_base_commit: None,
            git_head_commit: None,
            branch: None,
            pr_provider: Some("github".to_string()),
            pr_repo: Some("owner/repo".to_string()),
            pr_number: Some(1),
            author: None,
            timestamp: None,
            bookmarks: None,
        }));
    }

    fn assert_review_card_action_hover(mode: ViewMode) {
        let (mut app, lines) = rendered_diff_with_comment(mode);
        let text = lines.join("\n");
        assert!(text.contains("ia edit"), "diff text: {text:?}");
        assert!(text.contains("ra reply"), "diff text: {text:?}");
        assert!(text.contains("va resolve"), "diff text: {text:?}");
        assert!(text.contains("xa delete"), "diff text: {text:?}");

        let (edit_x, edit_y) = text_pos(&lines, "ia edit").expect("edit label");
        assert!(app.update_topbar_hover(edit_x + 3, edit_y));
        let (hover_lines, hover_styles) = draw_diff_snapshot(&mut app);
        let (edit_x, edit_y) = text_pos(&hover_lines, "ia edit").expect("edit label");
        assert_eq!(
            hover_styles[edit_y as usize][edit_x as usize + 3].fg,
            Some(app.theme.accent)
        );

        let (reply_x, reply_y) = text_pos(&hover_lines, "ra reply").expect("reply label");
        assert!(app.update_topbar_hover(reply_x + 3, reply_y));
        let (hover_lines, hover_styles) = draw_diff_snapshot(&mut app);
        let (reply_x, reply_y) = text_pos(&hover_lines, "ra reply").expect("reply label");
        assert_eq!(
            hover_styles[reply_y as usize][reply_x as usize + 3].fg,
            Some(app.theme.accent)
        );

        let (overflow_x, overflow_y) = text_pos(&hover_lines, "oa").expect("overflow label");
        assert!(app.update_topbar_hover(overflow_x + 1, overflow_y));
        let (hover_lines, hover_styles) = draw_diff_snapshot(&mut app);
        let (overflow_x, overflow_y) = text_pos(&hover_lines, "oa").expect("overflow label");
        assert_eq!(
            hover_styles[overflow_y as usize][overflow_x as usize + 1].fg,
            Some(app.theme.accent)
        );

        let (delete_x, delete_y) = text_pos(&hover_lines, "xa delete").expect("delete label");
        assert!(app.update_topbar_hover(delete_x + 3, delete_y));
        let (hover_lines, hover_styles) = draw_diff_snapshot(&mut app);
        let (delete_x, delete_y) = text_pos(&hover_lines, "xa delete").expect("delete label");
        assert_eq!(
            hover_styles[delete_y as usize][delete_x as usize + 3].fg,
            Some(app.theme.error)
        );
    }

    #[test]
    fn unified_review_card_shows_edit_action() {
        assert_review_card_action_hover(ViewMode::UnifiedPane);
    }

    #[test]
    fn split_review_card_shows_edit_action() {
        assert_review_card_action_hover(ViewMode::Split);
    }

    fn ascii_buffer_lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let area = terminal.backend().buffer().area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| {
                        let symbol = terminal.backend().buffer()[(x, y)].symbol();
                        if symbol.is_ascii() {
                            symbol.to_string()
                        } else {
                            " ".repeat(unicode_width::UnicodeWidthStr::width(symbol).max(1))
                        }
                    })
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn inactive_sidebar_tab_renders_unseen_marker_without_moving_hit_area() {
        let multi = MultiFileDiff::from_file_pairs(vec![
            (
                std::path::PathBuf::from("a.txt"),
                "old\n".to_string(),
                "new\n".to_string(),
            ),
            (
                std::path::PathBuf::from("b.txt"),
                "old\n".to_string(),
                "new\n".to_string(),
            ),
        ]);
        let mut app = App::new(multi, ViewMode::UnifiedPane, 0, false, None);
        app.theme.warning = Color::Yellow;
        app.theme.text_muted = Color::Blue;
        app.comments_tab_unseen = true;
        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let unseen_hit = app.file_panel_mode_toggle_hit;
        let lines = ascii_buffer_lines(&terminal);
        assert!(lines.join("\n").contains("* comments"));
        let (star_x, star_y) = text_pos(&lines, "* comments").unwrap();
        assert_eq!(
            terminal.backend().buffer()[(star_x, star_y)].fg,
            app.theme.warning
        );
        assert_eq!(
            terminal.backend().buffer()[(star_x + 2, star_y)].fg,
            app.theme.text_muted
        );

        app.comments_tab_unseen = false;
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        assert_eq!(app.file_panel_mode_toggle_hit, unseen_hit);

        app.show_comments_sidebar();
        app.files_tab_unseen = true;
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let lines = ascii_buffer_lines(&terminal);
        assert!(lines.join("\n").contains("* files"));
        let (star_x, star_y) = text_pos(&lines, "* files").unwrap();
        assert_eq!(
            terminal.backend().buffer()[(star_x, star_y)].fg,
            app.theme.warning
        );
        assert_eq!(
            terminal.backend().buffer()[(star_x + 2, star_y)].fg,
            app.theme.text_muted
        );
    }

    #[test]
    fn quit_confirmation_modal_renders_mouse_actions() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("old.txt"),
            std::path::PathBuf::from("new.txt"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::UnifiedPane, 0, false, None);
        app.theme.background = Some(Color::Blue);
        app.request_quit();
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let lines = ascii_buffer_lines(&terminal);
        let text = lines.join("\n");

        assert!(text.contains("Quit"), "screen text: {text:?}");
        assert!(
            text.contains("Are you sure you want to quit?"),
            "screen text: {text:?}"
        );
        assert!(text.contains("enter quit"), "screen text: {text:?}");
        assert!(text.contains("esc cancel"), "screen text: {text:?}");
        let (body_x, body_y) = text_pos(&lines, "Are you sure").unwrap();
        assert_eq!(
            terminal.backend().buffer()[(body_x, body_y)].bg,
            Color::Blue
        );
        let (cancel_x, cancel_y) = text_pos(&lines, "esc cancel").unwrap();
        assert!(app.handle_quit_confirmation_click(cancel_x, cancel_y));
        assert!(!app.quit_confirmation_active());
        assert!(!app.should_quit);
    }

    fn outdated_comments_app_with_snapshot(line_text: &str) -> App {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("src/lib.rs"),
            std::path::PathBuf::from("src/lib.rs"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 80, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.apply_review_comments_from_cli(
            &serde_json::json!({
                "version": 1,
                "comments": [{
                    "id": 7,
                    "file": "src/lib.rs",
                    "kind": "line",
                    "side": "new",
                    "newRange": { "start": 1, "end": 1 },
                    "anchorSnapshot": {
                        "side": "new",
                        "lineNumber": 42,
                        "lineText": line_text,
                        "contextBefore": ["fn answer() {"],
                        "contextAfter": ["}"]
                    },
                    "outdated": true,
                    "body": "Update the answer."
                }]
            })
            .to_string(),
        )
        .unwrap();
        app.open_outdated_comments_in_current_tab(Some(7));
        app
    }

    fn outdated_comments_app() -> App {
        outdated_comments_app_with_snapshot("let answer = 41;")
    }

    #[test]
    fn outdated_comments_view_shows_snapshot_and_actions() {
        let mut app = outdated_comments_app();
        app.diff_view_area = Some((0, 0, 100, 20));
        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 100, 20)))
            .unwrap();
        let lines = ascii_buffer_lines(&terminal);
        let text = lines.join("\n");

        assert!(text.contains("Outdated"), "preview text: {text:?}");
        assert!(text.contains("src/lib.rs:42"), "preview text: {text:?}");
        assert!(!text.contains("Original"), "preview text: {text:?}");
        assert!(
            text.contains("Update the answer."),
            "preview text: {text:?}"
        );
        assert!(text.contains("let answer = 41;"), "preview text: {text:?}");
        assert!(text.contains("fn answer()"), "preview text: {text:?}");
        assert!(text.contains("Snapshot"), "preview text: {text:?}");
        assert!(
            !text.contains("Captured snapshot:"),
            "preview text: {text:?}"
        );
        assert!(text.contains("ia edit"), "preview text: {text:?}");
        assert!(text.contains("va resolve"), "preview text: {text:?}");
        assert!(text.contains("xa delete"), "preview text: {text:?}");

        let buffer = terminal.backend().buffer();
        let (_, edit_y) = text_pos(&lines, "ia edit").unwrap();
        let (_, resolve_y) = text_pos(&lines, "va resolve").unwrap();
        assert_eq!(edit_y, resolve_y);
        assert_eq!(buffer[(0, resolve_y)].symbol(), "╰");
        assert_eq!(buffer[(0, 2)].symbol(), "╭");
        assert_eq!(buffer[(0, 2)].fg, app.theme.text_muted);
        let (snapshot_x, snapshot_y) = text_pos(&lines, "Snapshot").unwrap();
        let (location_x, location_y) = text_pos(&lines, "src/lib.rs:42").unwrap();
        assert_eq!(location_y, snapshot_y);
        assert_eq!(buffer[(snapshot_x, snapshot_y)].fg, app.theme.text_muted);
        assert!(!buffer[(snapshot_x, snapshot_y)]
            .modifier
            .contains(Modifier::DIM));
        assert_eq!(buffer[(location_x, location_y)].fg, app.theme.warning);
        assert!(!buffer[(location_x, location_y)]
            .modifier
            .contains(Modifier::DIM));
        let (let_x, arrow_y) = text_pos(&lines, "let answer").unwrap();
        let arrow_x = let_x - 2;
        assert_eq!(buffer[(arrow_x, arrow_y)].symbol(), "→");
        assert_eq!(buffer[(arrow_x, arrow_y)].fg, app.theme.accent);
        assert!(buffer[(arrow_x, arrow_y)].modifier.contains(Modifier::BOLD));
        let let_cell = &buffer[(let_x, arrow_y)];
        assert_ne!(let_cell.fg, app.theme.diff_context);
        assert_ne!(let_cell.fg, app.theme.warning);
        assert!(app.handle_review_preview_click(let_x, arrow_y));
        assert!(!app.review_editor_active());
        let (body_x, body_y) = text_pos(&lines, "Update the answer.").unwrap();
        assert!(app.handle_review_preview_click(body_x, body_y));
        assert!(app.review_editor_active());
        app.review_cancel_editor();

        let (resolve_x, resolve_y) = text_pos(&lines, "va resolve").unwrap();
        assert!(app.handle_review_preview_click(resolve_x + 2, resolve_y));
        let comments: serde_json::Value =
            serde_json::from_str(&app.review_comments_json()).unwrap();
        assert_eq!(comments["comments"][0]["resolved"], true);
        let (delete_x, delete_y) = text_pos(&lines, "xa delete").unwrap();
        assert!(app.handle_review_preview_click(delete_x + 2, delete_y));
        let comments: serde_json::Value =
            serde_json::from_str(&app.review_comments_json()).unwrap();
        assert!(comments["comments"].as_array().unwrap().is_empty());
    }

    #[test]
    fn snapshot_identifier_inside_code_stays_passive() {
        let mut app = outdated_comments_app_with_snapshot("Snapshot");
        app.diff_view_area = Some((0, 0, 80, 20));
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 80, 20)))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let (arrow_x, arrow_y) = (0..buffer.area.height)
            .find_map(|y| {
                (0..buffer.area.width)
                    .find(|x| buffer[(*x, y)].symbol() == "→")
                    .map(|x| (x, y))
            })
            .unwrap();

        assert!(app.handle_review_preview_click(arrow_x + 2, arrow_y));
        assert!(!app.review_editor_active());
    }

    #[test]
    fn outdated_snapshot_uses_diff_text_color_when_syntax_is_off() {
        let mut app = outdated_comments_app();
        app.syntax_mode = SyntaxMode::Off;
        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 100, 20)))
            .unwrap();
        let lines = ascii_buffer_lines(&terminal);
        let (line_x, line_y) = text_pos(&lines, "let answer = 41;").unwrap();

        assert_eq!(
            terminal.backend().buffer()[(line_x, line_y)].fg,
            app.theme.diff_context
        );
    }

    #[test]
    fn wrapped_snapshot_code_background_fills_the_continuation_row() {
        let mut app = outdated_comments_app_with_snapshot(
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyzEND",
        );
        app.theme = rgb_theme();
        app.syntax_mode = SyntaxMode::Off;
        app.line_wrap = true;
        app.scrollbar_visible = false;
        app.diff_view_area = Some((0, 0, 50, 20));
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 50, 20)))
            .unwrap();
        let lines = ascii_buffer_lines(&terminal);
        let (code_x, row) = text_pos(&lines, "END").unwrap();
        let buffer = terminal.backend().buffer();
        let code_bg = buffer[(code_x, row)].bg;

        assert_ne!(code_bg, app.theme.background.unwrap());
        assert_eq!(buffer[(47, row)].symbol(), " ");
        assert_eq!(buffer[(47, row)].bg, code_bg);
        assert!(app.handle_review_preview_click(code_x, row));
        assert!(!app.review_editor_active());
        let (context_x, context_y) = text_pos(&lines, "}").unwrap();
        assert!(app.handle_review_preview_click(context_x, context_y));
        assert!(!app.review_editor_active());

        let mut short = outdated_comments_app();
        short.theme = rgb_theme();
        short.syntax_mode = SyntaxMode::Off;
        short.line_wrap = true;
        short.scrollbar_visible = false;
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut short, Rect::new(0, 0, 50, 20)))
            .unwrap();
        let lines = ascii_buffer_lines(&terminal);
        let (code_x, row) = text_pos(&lines, "let answer").unwrap();
        let buffer = terminal.backend().buffer();

        assert_ne!(buffer[(47, row)].bg, buffer[(code_x, row)].bg);
        assert_eq!(buffer[(47, row)].bg, short.theme.background.unwrap());
    }

    #[test]
    fn outdated_comments_view_has_empty_state() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("src/lib.rs"),
            std::path::PathBuf::from("src/lib.rs"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 80, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.open_outdated_comments_in_current_tab(None);
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 60, 8)))
            .unwrap();
        let text = ascii_buffer_lines(&terminal).join("\n");

        assert!(
            text.contains("No outdated comments."),
            "preview text: {text:?}"
        );
    }

    #[test]
    fn pr_comment_view_shows_edit_action_for_editable_comments() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("README.md"),
            std::path::PathBuf::from("README.md"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 80, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        mark_pr_target(&mut app);
        app.start_pull_request_comment();
        for ch in "hello".chars() {
            app.review_insert_char(ch);
        }
        app.review_save_editor();
        app.open_pr_comments_in_current_tab(None);

        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 80, 10)))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("ia edit"), "preview text: {text:?}");
        assert!(text.contains("xa delete"), "preview text: {text:?}");
        assert!(text.contains("ra reply"), "preview text: {text:?}");
        let edit_hit = app
            .pr_comment_hits
            .iter()
            .find(|hit| matches!(hit.action, crate::app::review::PrCommentHitAction::Edit(_)))
            .copied()
            .expect("edit hit");
        assert!(app.update_topbar_hover(edit_hit.x, edit_hit.y));
        assert_eq!(app.pr_comment_action_hover_key.as_deref(), Some("ia"));
    }

    #[test]
    fn discovered_pr_without_comments_shows_add_comment() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("README.md"),
            std::path::PathBuf::from("README.md"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 80, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.set_review_pull_request_target(Some(crate::app::review::ReviewPullRequestTarget {
            provider: "github".to_string(),
            remote: "origin".to_string(),
            repo: "owner/repo".to_string(),
            number: 1,
            title: "Review this".to_string(),
        }));
        app.open_pr_comments_in_current_tab(None);

        let backend = TestBackend::new(80, 7);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 80, 7)))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("Review this"), "preview text: {text:?}");
        assert!(
            text.contains("No pull request comments."),
            "preview text: {text:?}"
        );
        assert!(text.contains("add comment"), "preview text: {text:?}");
        assert!(app.pr_comment_add_hit.is_some());
        assert!(app.start_pull_request_comment());
    }

    #[test]
    fn gitlab_review_uses_merge_request_copy() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("README.md"),
            std::path::PathBuf::from("README.md"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 80, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        let mut metadata = crate::app::review::ReviewTargetMetadata {
            label: "MR".to_string(),
            vcs: "git".to_string(),
            jj_change_id: None,
            jj_commit_id: None,
            git_base_ref: None,
            git_head_ref: None,
            git_base_commit: None,
            git_head_commit: None,
            branch: None,
            pr_provider: Some("gitlab".to_string()),
            pr_repo: Some("owner/repo".to_string()),
            pr_number: Some(1),
            author: None,
            timestamp: None,
            bookmarks: None,
        };
        app.set_review_target_metadata(Some(metadata.clone()));
        app.open_pr_comments_in_current_tab(None);

        let tab = super::topbar_tab_spans(&mut app, Rect::new(0, 0, 80, 1), 80)
            .into_iter()
            .map(|span| span.content.into_owned())
            .collect::<String>();
        assert!(tab.contains("Merge request comments"), "tab text: {tab:?}");

        let backend = TestBackend::new(80, 7);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 80, 7)))
            .unwrap();
        let text = ascii_buffer_lines(&terminal).join("\n");
        assert!(
            text.contains("No merge request comments."),
            "preview text: {text:?}"
        );

        metadata.pr_provider = Some("github".to_string());
        app.set_review_target_metadata(Some(metadata));
        assert_eq!(app.review_provider_kind().short_review_noun(), "PR");
    }

    #[test]
    fn pr_comment_view_without_pr_hides_add_comment() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("README.md"),
            std::path::PathBuf::from("README.md"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 80, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.open_pr_comments_in_current_tab(None);

        let backend = TestBackend::new(80, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_preview(frame, &mut app, Rect::new(0, 0, 80, 6)))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            text.contains("No pull request found."),
            "preview text: {text:?}"
        );
        assert!(!text.contains("add comment"), "preview text: {text:?}");
        assert!(app.pr_comment_add_hit.is_none());
        assert!(!app.start_pull_request_comment());
    }

    #[test]
    fn preview_selection_toolbar_hides_comment_action() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 50, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.diff_view_area = Some((0, 0, 50, 5));
        app.set_diff_selection_cells(vec![vec!["x".to_string(); 50]; 5]);
        assert!(app.start_diff_selection(0, 0));
        assert!(app.finish_diff_selection(1, 0));

        let backend = TestBackend::new(50, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::draw_selection_toolbar(frame, &mut app))
            .unwrap();

        assert!(!app
            .selection_toolbar_hits
            .iter()
            .any(|hit| hit.action == SelectionToolbarAction::Comment));
    }

    #[test]
    fn blame_age_legend_labels_use_subtle_style() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("old.txt"),
            std::path::PathBuf::from("new.txt"),
            "old".to_string(),
            "new".to_string(),
        );
        let app = App::new(multi, ViewMode::Blame, 50, false, None);
        let spans = super::blame_age_legend_spans(&app);
        let older = spans.first().unwrap();
        let newer = spans.iter().find(|span| span.content == " Newer").unwrap();
        assert_eq!(older.style.fg, Some(app.theme.border_subtle));
        assert_eq!(newer.style.fg, Some(app.theme.border_subtle));
        assert!(older.style.add_modifier.contains(Modifier::DIM));
        assert!(newer.style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn search_next_uses_rendered_preview_lines() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("README.md"),
            std::path::PathBuf::from("README.md"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 50, false, None);
        app.set_preview_search_lines(vec!["alpha".to_string(), "target".to_string()]);
        app.start_search();
        for ch in "target".chars() {
            app.push_search_char(ch);
        }
        app.search_next();
        assert_eq!(app.search_target(), Some(1));
    }

    #[test]
    fn counted_binding_label_uses_current_binding() {
        assert_eq!(counted_binding_label("r"), "<count>r");
        assert_eq!(counted_binding_label("g r"), "<count>g r");
        assert_eq!(
            counted_binding_label("r / ctrl-r"),
            "<count>r / <count>ctrl-r"
        );
    }

    #[test]
    fn markdown_local_image_renders_halfblocks() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/assets/preview.png"
        ));
        let lines = super::image_preview_lines(path, 16, 20, ratatui::style::Color::Black)
            .expect("image should render");
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content == "▀"),
            "image renders with halfblocks"
        );
    }

    #[test]
    fn terminal_image_protocol_can_be_cached() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/assets/preview.png"
        ));
        let multi = MultiFileDiff::from_file_pair(
            path.to_path_buf(),
            path.to_path_buf(),
            String::new(),
            String::new(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 50, false, None);
        app.set_image_picker(ratatui_image::picker::Picker::halfblocks());

        assert!(app
            .ensure_terminal_image_preview(path, ratatui::layout::Size::new(10, 5))
            .is_some());
        assert!(app.image_preview_cache.is_some());
    }

    #[test]
    fn links_are_collected_with_positions() {
        // Marker "● " (2 cols) precedes the link text on the item line.
        let (lines, links) = render_md_full("- see [Oyo](https://oyo.dev) here\n", 80, rgb_theme());
        assert_eq!(links.len(), 1, "one link collected");
        let link = &links[0];
        assert_eq!(link.url, "https://oyo.dev");
        // The clickable region covers the link text plus the "(url)" suffix.
        let line_text: String = lines[link.line]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let clickable: String = line_text.chars().skip(link.col).take(link.width).collect();
        assert!(
            clickable.starts_with("Oyo"),
            "covers link text: {clickable:?}"
        );
        assert!(
            clickable.contains("https://oyo.dev"),
            "covers url: {clickable:?}"
        );
    }

    #[test]
    fn markdown_preview_can_show_change_bars() {
        let theme = rgb_theme();
        let bars = MarkdownChangeBars {
            marker: "|".to_string(),
            marker_width: 1,
            styles: HashMap::from([(1, Style::default().fg(theme.accent))]),
        };
        let mut highlight = |_lang: Option<&str>, _code: &str| None;
        let (lines, _) = render_markdown_preview_lines(
            "# Changed\n\nPlain\n",
            &theme,
            80,
            None,
            &mut highlight,
            Some(&bars),
        );
        let changed = flatten(&lines)
            .lines()
            .find(|line| line.contains("Changed"))
            .unwrap()
            .to_string();
        assert!(changed.starts_with("| "), "change bar: {changed:?}");
    }

    #[test]
    fn markdown_table_uses_rounded_borders() {
        let lines = render_md("| key | value |\n| --- | --- |\n| a | 1 |\n", 80);
        let text = flatten(&lines);
        assert!(text.contains("╭"), "rounded top-left corner: {text}");
        assert!(text.contains("│ key │ value │"), "header row: {text}");
        assert!(text.contains("╰"), "rounded bottom-left corner: {text}");
    }

    #[test]
    fn csv_preview_shows_cells() {
        let sig = crate::csv_preview::CsvPreviewSignature::new("data.csv", "name,age\nOyo,1\n");
        let mut state = crate::csv_preview::CsvPreviewState::new(sig, "name,age\nOyo,1\n").unwrap();
        let lines = super::csv_table_lines(&mut state, &rgb_theme(), 80, None);
        let flat: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        // Borderless csvlens layout: header, separator, rows.
        assert!(
            flat[0].contains("name") && flat[0].contains("age"),
            "header: {flat:?}"
        );
        assert!(flat[1].contains('┼'), "gutter separator: {flat:?}");
        assert!(
            !flat.iter().any(|l| l.contains('╭')),
            "no box borders: {flat:?}"
        );
        // Row 1 has a gutter number, a divider, then the cells.
        let row = flat.iter().find(|l| l.contains("Oyo")).unwrap();
        assert!(
            row.trim_start().starts_with("1 │"),
            "row-number gutter: {row:?}"
        );
        assert!(row.contains("Oyo") && row.contains('1'), "cells: {row:?}");
    }

    #[test]
    fn csv_preview_can_show_change_bars() {
        let theme = rgb_theme();
        let sig = crate::csv_preview::CsvPreviewSignature::new("data.csv", "name,age\nOyo,1\n");
        let mut state = crate::csv_preview::CsvPreviewState::new(sig, "name,age\nOyo,1\n").unwrap();
        let bars = super::PreviewChangeBars {
            marker: "|".to_string(),
            marker_width: 1,
            styles: HashMap::from([(2, Style::default().fg(theme.accent))]),
        };
        let lines = super::csv_table_lines(&mut state, &theme, 80, Some(&bars));
        let flat: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let row = flat.iter().find(|l| l.contains("Oyo")).unwrap();
        assert!(row.starts_with("|  1 │"), "change bar before row: {row:?}");
    }

    #[test]
    fn preview_change_bars_mark_new_source_lines_and_can_be_disabled() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("data.txt"),
            std::path::PathBuf::from("data.txt"),
            "a\nb\n".to_string(),
            "a\nB\nc\n".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 50, false, None);

        let bars = super::preview_change_bars(&mut app, Some(SyntaxSide::New)).unwrap();
        assert!(bars.styles.contains_key(&2), "modified line is marked");
        assert!(bars.styles.contains_key(&3), "inserted line is marked");
        assert!(
            !bars.styles.contains_key(&1),
            "unchanged line is not marked"
        );

        app.preview_change_bars = false;
        assert!(super::preview_change_bars(&mut app, Some(SyntaxSide::New)).is_none());
    }

    #[test]
    fn structured_preview_change_bars_mark_json_yaml_and_toml_paths() {
        let cases = [
            (
                StructuredPreviewKind::Json,
                "data.json",
                r#"{"a":1,"b":2}"#,
                r#"{"a":1,"b":3,"c":4}"#,
                ".b",
                ".c",
            ),
            (
                StructuredPreviewKind::Yaml,
                "data.yaml",
                "a: 1\nb: 2\n",
                "a: 1\nb: 3\nc: 4\n",
                ".b",
                ".c",
            ),
            (
                StructuredPreviewKind::Toml,
                "data.toml",
                "a = 1\n[server]\nport = 1\n",
                "a = 1\n[server]\nport = 2\nname = 'Oyo'\n",
                ".server.port",
                ".server.name",
            ),
        ];

        for (kind, name, old, new, modified, added) in cases {
            let multi = MultiFileDiff::from_file_pair(
                std::path::PathBuf::from(name),
                std::path::PathBuf::from(name),
                old.to_string(),
                new.to_string(),
            );
            let app = App::new(multi, ViewMode::Preview, 50, false, None);
            let bars = super::structured_preview_change_bars(&app, kind, Some(SyntaxSide::New))
                .expect("structured bars");
            assert!(
                bars.styles.contains_key(modified),
                "modified path {modified} for {name}"
            );
            assert!(
                bars.styles.contains_key(added),
                "added path {added} for {name}"
            );
            assert!(!bars.styles.contains_key(".a"), "unchanged path for {name}");
        }
    }

    #[test]
    fn table_cells_keep_inline_styles() {
        let theme = rgb_theme();
        let code_fg = theme.warning;
        let link_fg = theme.info;
        let lines = render_md_themed("| a | b |\n| --- | --- |\n| `x` | [y](u) |\n", 80, theme);
        // Body row: inline code keeps the code color, link keeps the link color.
        let code = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == "x")
            .expect("inline code span in cell");
        assert_eq!(code.style.fg, Some(code_fg), "inline code colored in cell");
        assert!(code.style.bg.is_some(), "inline code chip has background");
        let link = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == "y")
            .expect("link span in cell");
        assert_eq!(link.style.fg, Some(link_fg), "link colored in cell");
        assert!(
            link.style.add_modifier.contains(Modifier::UNDERLINED),
            "link underlined in cell"
        );
    }

    #[test]
    fn table_header_plain_text_is_accent_bold() {
        let theme = rgb_theme();
        let accent = theme.accent;
        let lines = render_md_themed("| Head |\n| --- |\n| body |\n", 80, theme);
        let head = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == "Head")
            .expect("header cell span");
        assert_eq!(head.style.fg, Some(accent), "header text uses accent");
        assert!(
            head.style.add_modifier.contains(Modifier::BOLD),
            "header bold"
        );
    }

    #[test]
    fn headings_use_per_level_markers() {
        let lines = render_md("# One\n\n## Two\n", 40);
        let text = flatten(&lines);
        assert!(text.contains("█ One"), "h1 marker: {text}");
        assert!(text.contains("▊ Two"), "h2 marker: {text}");
    }

    #[test]
    fn github_callout_renders_titled_border() {
        let lines = render_md("> [!WARNING]\n> Be careful here.\n", 40);
        let text = flatten(&lines);
        assert!(text.contains("▲ Warning"), "callout title: {text}");
        // Every content line of the quote carries the left border.
        assert!(text.contains("▎"), "callout border: {text}");
        assert!(text.contains("Be careful here."), "callout body: {text}");
    }

    #[test]
    fn task_list_marker_replaces_bullet() {
        let lines = render_md("- [x] done\n- [ ] todo\n", 40);
        let text = flatten(&lines);
        assert!(text.contains("▣ done"), "checked item: {text}");
        assert!(text.contains("▢ todo"), "unchecked item: {text}");
        // The bullet should be replaced, not appended alongside the checkbox.
        assert!(!text.contains("● ▣"), "bullet not doubled: {text}");
    }

    #[test]
    fn completed_task_text_is_dimmed_and_struck() {
        let lines = render_md("- [x] done\n- [ ] todo\n", 40);
        let done = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("done"))
            .expect("done text span");
        assert!(done.style.add_modifier.contains(Modifier::CROSSED_OUT));
        assert!(done.style.add_modifier.contains(Modifier::DIM));
        // The checkbox marker itself stays crisp (not struck through).
        let mark = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains('▣'))
            .expect("done marker");
        assert!(!mark.style.add_modifier.contains(Modifier::CROSSED_OUT));
        // Pending items keep normal styling.
        let todo = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("todo"))
            .expect("todo text span");
        assert!(!todo.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn ordered_and_nested_bullets() {
        let lines = render_md("1. first\n2. second\n\n- top\n  - nested\n", 40);
        let text = flatten(&lines);
        assert!(text.contains("1. first"), "ordered start: {text}");
        assert!(text.contains("2. second"), "ordered increment: {text}");
        assert!(text.contains("● top"), "depth-1 bullet: {text}");
        assert!(text.contains("○ nested"), "depth-2 bullet: {text}");
    }

    #[test]
    fn code_block_is_a_content_sized_panel() {
        // A line wider than the 45% minimum (0.45 * 80 = 36) so content wins.
        let long = "let answer = 40 + 2; // a comfortably wide line";
        let src = format!("```rust\n{long}\n```\n");
        let lines = render_md_themed(&src, 80, rgb_theme());
        let text = flatten(&lines);
        assert!(text.contains("rust"), "language label: {text}");
        assert!(text.contains(long), "code body: {text}");
        assert!(!text.contains('▎'), "no left rail: {text}");
        // Panel shrink-wraps to content (2 cols padding each side), not viewport.
        let code_line = lines
            .iter()
            .find(|l| flatten(std::slice::from_ref(l)).contains(long))
            .unwrap();
        assert_eq!(line_width(code_line), 2 + super::text_width(long) + 2);
    }

    #[test]
    fn code_block_has_a_minimum_width() {
        // A tiny snippet still renders as a panel ≈45% of the viewport wide.
        let lines = render_md_themed("```\nx\n```\n", 80, rgb_theme());
        let code_line = lines
            .iter()
            .find(|l| flatten(std::slice::from_ref(l)).contains('x'))
            .unwrap();
        assert_eq!(line_width(code_line), 36, "min width is 45% of 80");
    }

    #[test]
    fn code_block_has_a_bottom_padding_row() {
        // Trailing text so the bottom padding row isn't trimmed as a final blank.
        let lines = render_md_themed("```rust\nx\n```\n\ntail\n", 80, rgb_theme());
        // header (lang), code, bottom blank pad — three backed panel rows.
        let panel: Vec<_> = lines
            .iter()
            .filter(|l| line_width(l) == 36 && l.spans.iter().all(|s| s.style.bg.is_some()))
            .collect();
        assert_eq!(panel.len(), 3, "header, code, and one bottom pad row");
        // The last panel row is blank padding (spaces only).
        let last = panel.last().unwrap();
        assert!(last.spans.iter().all(|s| s.content.trim().is_empty()));
    }

    #[test]
    fn inline_code_is_a_padded_chip_on_opaque_theme() {
        let lines = render_md_themed("a `code` b\n", 40, rgb_theme());
        // The chip span is padded and carries a background.
        let chip = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("code"))
            .expect("code chip span");
        assert_eq!(chip.content.as_ref(), " code ", "chip is padded");
        assert!(chip.style.bg.is_some(), "chip has a background");
    }

    #[test]
    fn heading_band_fills_width_and_is_tinted() {
        let lines = render_md_themed("# Title\n", 40, rgb_theme());
        let heading = lines
            .iter()
            .find(|l| flatten(std::slice::from_ref(l)).contains("Title"))
            .expect("heading line");
        assert_eq!(line_width(heading), 40, "band spans the full width");
        assert!(
            heading.spans.iter().all(|s| s.style.bg.is_some()),
            "every span on the heading line is tinted"
        );
    }

    #[test]
    fn backgrounds_render_on_transparent_rgb_theme() {
        // Mirrors an evergarden-winter-style config: no page background, but
        // real RGB foreground colors. Bands/chips/panels must still render.
        use ratatui::style::Color::Rgb;
        let theme = ResolvedTheme {
            text: Rgb(0xd3, 0xc6, 0xaa),
            accent: Rgb(0x83, 0xc0, 0x92),
            info: Rgb(0x7f, 0xbb, 0xb3),
            success: Rgb(0xa7, 0xc0, 0x80),
            warning: Rgb(0xdb, 0xbc, 0x7f),
            primary: Rgb(0xd6, 0x99, 0xb6),
            background: None,
            background_panel: None,
            background_element: None,
            ..ResolvedTheme::default()
        };
        let md = "# Title\n\ninline `code` here\n\n```rust\nfn f() {}\n```\n";
        let lines = render_md_themed(md, 40, theme);

        let heading = lines
            .iter()
            .find(|l| flatten(std::slice::from_ref(l)).contains("Title"))
            .unwrap();
        assert!(
            heading.spans.iter().all(|s| s.style.bg.is_some()),
            "heading band renders without a page background"
        );
        let chip = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("code"))
            .unwrap();
        assert!(chip.style.bg.is_some(), "inline chip renders");
        let code_line = lines
            .iter()
            .find(|l| flatten(std::slice::from_ref(l)).contains("fn f()"))
            .unwrap();
        assert!(
            code_line.spans.iter().any(|s| s.style.bg.is_some()),
            "code panel renders"
        );
    }

    #[test]
    fn code_block_language_is_right_aligned() {
        let lines = render_md_themed("```rust\nx\n```\n", 40, rgb_theme());
        let header = lines
            .iter()
            .find(|l| l.spans.last().is_some_and(|s| s.content.contains("rust")))
            .expect("code header line");
        assert_eq!(line_width(header), 18, "panel at min width (45% of 40)");
        assert!(!header.spans[0].content.contains('▎'), "no left rail");
        // Label is the last span, pinned to the right edge of the panel.
        assert!(
            header.spans.last().unwrap().content.starts_with("rust"),
            "language at the right edge"
        );
    }

    #[test]
    fn horizontal_rule_spans_width() {
        let lines = render_md("a\n\n---\n\nb\n", 12);
        let rule = lines
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content.contains('─')))
            .expect("a rule line");
        let width: usize = rule.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(width, 12, "rule fills the content width");
    }

    #[test]
    fn empty_document_yields_one_line() {
        let lines = render_md("", 40);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn block_quote_has_no_trailing_border() {
        let lines = render_md("> one\n>\n> two\n", 40);
        // Border continues between paragraphs but not past the last line.
        assert!(markdown_line_is_quote_border(
            &lines[1] // the blank separator inside the quote
        ));
        assert!(!markdown_line_is_quote_border(lines.last().unwrap()));
    }
}
