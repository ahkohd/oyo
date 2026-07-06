//! UI rendering for the TUI

use crate::app::{
    diff_scrollbar_thumb, App, FilePanelMode, FilePanelScrollbarState, ReviewEditorToolbarAction,
    ReviewEditorToolbarHit, ReviewLineAddHit, SelectionToolbarAction, SelectionToolbarHit,
    TopbarTabContent, TopbarTabHit, ViewMode, DIFF_VIEW_MIN_WIDTH, FILE_PANEL_MIN_WIDTH,
};
use crate::color;
use crate::config::FilePanelPosition;
use crate::csv_preview::{CsvPreviewSignature, CsvPreviewState};
use crate::keybindings::{
    BindingAction, DashboardAction, DashboardFilterAction, FileFilterAction, GlobalAction,
    HelpAction, LineInputAction, NormalAction, PickerAction, ReviewEditorAction, SelectionAction,
};
use crate::structured_preview::{
    StructuredPreviewChangeBars, StructuredPreviewKind, StructuredPreviewSignature,
};
use crate::syntax::SyntaxSide;
use crate::views::{
    render_blame, render_diff_scrollbar, render_evolution, render_split, render_unified_pane,
    reserve_diff_scrollbar_lane,
};
use image::GenericImageView;
use oyo_core::{multi::DiffStatus, multi::FileSide, ChangeKind, FileStatus, LineKind};
use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState,
    },
    Frame,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

fn truncate_filename_keep_ext(name: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if name.len() <= max_width {
        return name.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let (stem, ext) = match name.rfind('.') {
        Some(idx) if idx > 0 && idx < name.len().saturating_sub(1) => (&name[..idx], &name[idx..]),
        _ => (name, ""),
    };
    let ext_len = ext.len();
    if ext_len >= max_width {
        let suffix_len = max_width.saturating_sub(3);
        return format!("…{}", &name[name.len().saturating_sub(suffix_len)..]);
    }

    if ext_len == 0 {
        let stem_keep = max_width.saturating_sub(3);
        let head_len = stem_keep.div_ceil(2);
        let tail_len = stem_keep.saturating_sub(head_len);
        let head = &stem[..head_len.min(stem.len())];
        let tail = if tail_len > 0 && tail_len <= stem.len() {
            &stem[stem.len().saturating_sub(tail_len)..]
        } else {
            ""
        };
        return format!("{head}…{tail}");
    }

    let max_stem_len = max_width.saturating_sub(ext_len);
    if max_stem_len <= 3 {
        let dots = ".".repeat(max_stem_len);
        return format!("{dots}{ext}");
    }

    let stem_keep = max_stem_len.saturating_sub(3);
    let head_len = stem_keep.div_ceil(2);
    let tail_len = stem_keep.saturating_sub(head_len);
    let head = &stem[..head_len.min(stem.len())];
    let tail = if tail_len > 0 && tail_len <= stem.len() {
        &stem[stem.len().saturating_sub(tail_len)..]
    } else {
        ""
    };
    format!("{head}…{tail}{ext}")
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

fn no_changes_hint(app: &App) -> &str {
    if app.watch {
        "Watching for changes. Press q to quit."
    } else {
        "Press R to refresh or q to quit."
    }
}

fn draw_no_changes(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(bg) = app.theme.background {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }
    let text = vec![
        Line::from(Span::styled(
            no_changes_message(app),
            Style::default()
                .fg(app.theme.text)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            no_changes_hint(app),
            Style::default().fg(app.theme.text_muted),
        )),
    ];
    let height = 3.min(area.height);
    let y = area.y + area.height.saturating_sub(height) / 2;
    let mut paragraph = Paragraph::new(text).alignment(Alignment::Center);
    if let Some(bg) = app.theme.background {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, Rect::new(area.x, y, area.width, height));
}

/// Main drawing function
pub fn draw(frame: &mut Frame, app: &mut App) {
    app.clear_review_preview_boxes();
    app.begin_scrollbar_frame();
    app.topbar_tab_hits.clear();
    app.topbar_plus_hit = None;
    app.topbar_scroll_left_hit = None;
    app.topbar_scroll_right_hit = None;
    app.preview_toggle_hit = None;
    app.topbar_sidebar_toggle_hit = None;
    app.status_mode_hit = None;
    app.topbar_area = None;

    if app.multi_diff.file_count() == 0 {
        app.clear_diff_selection();
        app.set_diff_selection_cells(Vec::new());
        draw_no_changes(frame, app, frame.area());
        if app.show_help {
            draw_help_popover(frame, app);
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
    draw_toasts(frame, app);
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
        reskin_toast(buffer, *r, border, bg, &icons);
    }
}

/// Re-skin one crate-rendered toast in place: match `bg` (transparent when the
/// theme has none), tuck a thin rounded border in over the crate's outer padding
/// rows so the frame hugs the message, and color the leading severity icon.
fn reskin_toast(
    buffer: &mut ratatui::buffer::Buffer,
    ta: Rect,
    border: Color,
    bg: Color,
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
                cell.set_bg(bg);
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
    let state = if preview_can_render_markdown(app)
        || preview_structured_kind(app).is_some()
        || preview_can_render_csv(app)
    {
        if app.active_preview_rendered() {
            "preview"
        } else {
            "source"
        }
    } else {
        "source"
    };
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
    let right_spans = pad_spans_right(
        vec![Span::styled(
            state,
            Style::default().fg(app.theme.text_muted),
        )],
        right_width,
    );
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
    } else if app.search_active() {
        ("/", app.search_query(), "Search")
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
        DiffStatus::Deferred | DiffStatus::Computing
    ) || app.view_build_pending()
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
    if diff_pending && !stats_known {
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
    if app.files_changed_on_disk {
        right_spans.push(Span::raw(" "));
        right_spans.push(Span::styled(
            "changed",
            Style::default().fg(app.theme.warning),
        ));
    }
    let comment_count = app.review_comment_count();
    if comment_count > 0 || app.review_editor_active() {
        right_spans.push(Span::raw(" "));
        let comments_label = match comment_count {
            0 => "no comment".to_string(),
            1 => "1 comment".to_string(),
            n => format!("{n} comments"),
        };
        let start = spans_width(&right_spans);
        let width = text_width(&comments_label);
        comments_hit = Some((start, width));
        let mut style = Style::default().fg(if app.status_comments_hover {
            app.theme.accent
        } else {
            app.theme.text_muted
        });
        if app.status_comments_hover {
            style = style.add_modifier(Modifier::BOLD);
        }
        right_spans.push(Span::styled(comments_label, style));
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
    right_spans.push(Span::raw(" "));

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
            DiffStatus::Deferred | DiffStatus::Computing
        ) || app.view_build_pending()
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
        .is_multi_file()
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
        Some(TopbarTabContent::File(index)) => app
            .multi_diff
            .files
            .get(index)
            .map(|file| is_markdown_name(&file.display_name))
            .unwrap_or(false),
        None => false,
    }
}

fn topbar_tab_spans(app: &mut App, area: Rect, max_width: usize) -> Vec<Span<'static>> {
    app.ensure_topbar_tabs();
    app.topbar_tab_hits.clear();
    app.topbar_plus_hit = None;
    app.topbar_scroll_left_hit = None;
    app.topbar_scroll_right_hit = None;

    let closeable = app.topbar_tabs.len() > 1;
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
        let (file_name, changed) = match tab.content {
            TopbarTabContent::File(file_index) => {
                let Some(file) = app.multi_diff.files.get(file_index) else {
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
        };
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

    let panel_allowed = app.is_multi_file() || app.file_panel_mode == FilePanelMode::Comments;

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
    let scroll = app.file_list_scroll;
    let Some((thumb_top, thumb_height)) =
        diff_scrollbar_thumb(total_items, visible_items, track.height, scroll)
    else {
        return;
    };
    let x = track.x;
    app.set_file_panel_scrollbar(FilePanelScrollbarState {
        x,
        y: track.y,
        height: track.height,
        total_items,
        visible_items,
        thumb_top,
        thumb_height,
    });
    let focused = app.file_list_focused || app.file_filter_active;
    if !focused && !app.file_panel_hover {
        return;
    }
    let symbol = if focused { "▐" } else { "▕" };
    let style = Style::default().fg(app.theme.text_muted);
    let start = track.y.saturating_add(thumb_top);
    let end = start
        .saturating_add(thumb_height)
        .min(track.y.saturating_add(track.height));
    let buffer = frame.buffer_mut();
    for row in start..end {
        if let Some(cell) = buffer.cell_mut((x, row)) {
            cell.set_symbol(symbol).set_style(style);
        }
    }
}

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
    let panel_bg = app.theme.background_panel.or(app.theme.background);
    let content_area = file_panel_content_area(app, area);
    draw_file_panel_divider(frame, app, area, panel_bg);

    let show_filter = true;
    let panel_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if show_filter {
            vec![
                Constraint::Length(4), // Header
                Constraint::Min(0),    // List
                Constraint::Length(3), // Filter
            ]
        } else {
            vec![
                Constraint::Length(4), // Header
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

    let files = &app.multi_diff.files;
    let file_count = app.multi_diff.file_count();

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
    let mode_toggle_text = match app.file_panel_mode {
        FilePanelMode::Files => "comments",
        FilePanelMode::Comments => "files",
    };
    let mode_toggle_width = text_width(mode_toggle_text);
    let header_max_width = header_area
        .width
        .saturating_sub(mode_toggle_width as u16 + 3) as usize;
    let range_display = app.multi_diff.git_range_display();
    let header_text = if let Some((from, to)) = range_display {
        let range_text = format!("{from}..{to}");
        let range_width = text_width(&range_text);
        if header_max_width <= range_width {
            truncate_text(&range_text, header_max_width)
        } else {
            let sep = " • ";
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
            .saturating_sub((root_label.len() + 1) as u16) as usize;
        format!(
            "{}{}",
            root_label,
            truncate_path(&root_path, root_max_width)
        )
    };

    let root_style = Style::default()
        .fg(app.theme.border_subtle)
        .add_modifier(Modifier::DIM);
    let header_lines = if app.file_panel_mode == FilePanelMode::Comments {
        let comment_count = app.review_comment_count();
        let comments_label = match comment_count {
            1 => "1 comment".to_string(),
            n => format!("{n} comments"),
        };
        vec![
            Line::from(vec![Span::raw(" "), Span::styled(header_text, root_style)]),
            Line::raw(""),
            Line::from(vec![
                Span::raw(" "),
                Span::styled("●", Style::default().fg(app.theme.text_muted)),
                Span::raw(" "),
                Span::styled(comments_label, Style::default().fg(app.theme.text)),
                Span::raw(" "),
                Span::styled("review", Style::default().fg(app.theme.text_muted)),
            ]),
            Line::raw(""),
        ]
    } else {
        vec![
            Line::from(vec![Span::raw(" "), Span::styled(header_text, root_style)]),
            Line::raw(""),
            Line::from(vec![
                Span::raw(" "),
                Span::styled("●", Style::default().fg(app.theme.text_muted)),
                Span::raw(" "),
                Span::styled(
                    format!("{} files", file_count),
                    Style::default().fg(app.theme.text),
                ),
                Span::raw(" "),
                Span::styled(via_text, Style::default().fg(app.theme.text_muted)),
            ]),
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
            ]),
        ]
    };

    let mut header = Paragraph::new(header_lines);
    if let Some(bg) = panel_bg {
        header = header.style(Style::default().bg(bg));
    }
    frame.render_widget(header, header_area);
    if header_area.width > mode_toggle_width as u16 + 1 {
        let toggle_x = header_area.x.saturating_add(
            header_area
                .width
                .saturating_sub(mode_toggle_width as u16 + 1),
        );
        app.file_panel_mode_toggle_hit =
            Some((toggle_x, header_area.y, mode_toggle_width as u16, 1));
        let mut toggle_style = Style::default().fg(if app.file_panel_mode_toggle_hover {
            app.theme.accent
        } else {
            app.theme.text_muted
        });
        if app.file_panel_mode_toggle_hover {
            toggle_style = toggle_style.add_modifier(Modifier::BOLD);
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(mode_toggle_text, toggle_style))),
            Rect::new(toggle_x, header_area.y, mode_toggle_width as u16, 1),
        );
    }

    if app.file_panel_mode == FilePanelMode::Comments {
        draw_comment_list(frame, app, list_area, filter_area, panel_bg);
        return;
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

        let is_selected = file_idx == app.multi_diff.selected_index;
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
        let filter_bg = app
            .theme
            .background_element
            .or(app.theme.background_panel)
            .or(app.theme.background);
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
    filter_area: Option<Rect>,
    panel_bg: Option<Color>,
) {
    let indices = app.filtered_review_comment_indices();
    let total_rows = indices.len();
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

    let mut items = Vec::new();
    let mut row_map = Vec::new();
    for comment_idx in indices.iter().skip(row_offset).take(visible_rows).copied() {
        let Some((_file_idx, path, location, preview)) =
            app.review_comment_sidebar_item(comment_idx)
        else {
            continue;
        };
        let is_active = app.review_comment_is_active(comment_idx);
        let is_hovered = app.file_list_hover == Some(comment_idx);
        let selected_bg = if is_active {
            if app.file_list_focused {
                app.theme.background_element.or(app.theme.background_panel)
            } else {
                app.theme.background_panel
            }
        } else {
            None
        };
        let marker_style = if is_active || is_hovered {
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.text_muted)
        };
        let marker = if is_active { "•" } else { " " };
        let name_width = list_content_area.width.saturating_sub(12) as usize;
        let name = truncate_path(&path, name_width);
        let mut name_style = Style::default().fg(if is_active || is_hovered {
            app.theme.accent
        } else {
            app.theme.text
        });
        if is_active || is_hovered {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        if let Some(bg) = selected_bg {
            name_style = name_style.bg(bg);
        }
        let location_style = if is_active && app.file_list_focused {
            Style::default().fg(app.theme.warning)
        } else {
            Style::default().fg(app.theme.text_muted)
        };
        let preview_width = list_content_area.width.saturating_sub(6) as usize;
        let line = Line::from(vec![
            Span::styled(marker, marker_style),
            Span::raw(" "),
            Span::styled(name, name_style),
            Span::raw(" "),
            Span::styled(location, location_style),
            Span::raw(" "),
            Span::styled(
                truncate_to_width(&preview, preview_width),
                Style::default().fg(app.theme.text_muted),
            ),
        ]);
        items.push(ListItem::new(line));
        row_map.push(Some(comment_idx));
    }

    let mut block = Block::default().padding(ratatui::widgets::Padding::new(1, 0, 1, 0));
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

    if let Some(filter_area) = filter_area {
        app.file_filter_area = Some((
            filter_area.x,
            filter_area.y,
            filter_area.width,
            filter_area.height,
        ));
        let filter_bg = app
            .theme
            .background_element
            .or(app.theme.background_panel)
            .or(app.theme.background);
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

struct PreviewChangeBars {
    marker: String,
    marker_width: usize,
    styles: HashMap<usize, Style>,
}

impl PreviewChangeBars {
    fn gutter_width(&self) -> usize {
        self.marker_width + 1
    }
}

fn render_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    if let Some(bg) = app.theme.background {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }
    let (content_area, scrollbar_area) = reserve_diff_scrollbar_lane(app, area);
    let (title, text, side, binary, base_dir) = preview_document(app);
    app.clear_preview_link_boxes();
    // Number of leading lines pinned to the top (CSV header and separator).
    let mut sticky_rows = 0usize;
    let (mut lines, links) = if binary {
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
        // Pin the top padding, header, and separator to the top.
        sticky_rows = 3;
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

    // Map visible links from content coordinates to on-screen click boxes.
    let content_w = content_area.width as usize;
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

fn preview_document(app: &App) -> (String, String, Option<SyntaxSide>, bool, Option<PathBuf>) {
    match app.active_topbar_content() {
        Some(TopbarTabContent::Help) => {
            ("Help.md".to_string(), help_markdown(app), None, false, None)
        }
        Some(TopbarTabContent::File(index)) => {
            let Some(file) = app.multi_diff.files.get(index) else {
                return (String::new(), String::new(), None, false, None);
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
            let base_dir = app
                .multi_diff
                .source_path(index, file_side)
                .or_else(|| Some(PathBuf::from(&file.display_name)))
                .and_then(|path| path.parent().map(Path::to_path_buf));
            (
                file.display_name.clone(),
                text,
                Some(side),
                file.binary,
                base_dir,
            )
        }
        None => (String::new(), String::new(), None, false, None),
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
        TopbarTabContent::Help => return None,
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
    bars.map(|bars| bars.marker_width + 1).unwrap_or(0)
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

    // A blank top-padding line above the (pinned) header.
    let mut top_spans = Vec::new();
    push_preview_change_gutter(&mut top_spans, change_bars, None, None);
    let mut lines = vec![Line::from(top_spans)];

    // Header row (blank row-number gutter).
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

/// Colors and derived styles used while rendering a Markdown preview.
struct MarkdownStyles {
    text: Style,
    muted: Style,
    code: Style,
    link: Style,
    rule: Style,
    list_marker: Style,
    check_done: Style,
    check_todo: Style,
    table_border: Style,
    table_head: Style,
    code_lang: Style,
    /// Per-level heading styles (index 0 = H1 .. index 5 = H6).
    headings: [Style; 6],
    /// Per-level heading marker glyphs (shrinking blocks).
    heading_marks: [&'static str; 6],
    /// Per-level heading band, tinted with the heading's own color
    /// (None on transparent/ANSI themes where blending isn't possible).
    heading_bands: [Option<Color>; 6],
    /// Depth-cycled colors for list bullet markers.
    list_colors: [Color; 4],
    /// Background chip behind inline code / highlights (None when transparent).
    inline_bg: Option<Color>,
    /// Panel background behind fenced code blocks (None on transparent themes).
    code_bg: Option<Color>,
}

impl MarkdownStyles {
    fn from_theme(theme: &crate::config::ResolvedTheme) -> Self {
        let bold = Modifier::BOLD;
        let heading_colors = [
            theme.accent,
            theme.info,
            theme.success,
            theme.warning,
            theme.primary,
            theme.text_muted,
        ];
        let headings =
            std::array::from_fn(|i| Style::default().fg(heading_colors[i]).add_modifier(bold));

        // markview draws bands/chips/panels with explicit background colors, so
        // they show even on transparent themes. Derive a surface to blend from:
        // the real page background when opaque, otherwise a mode-appropriate
        // neutral inferred from the text color's luminance (light text = dark
        // theme). Blending returns None for ANSI/named colors, which degrades
        // gracefully to no background.
        let dark_mode = crate::color::relative_luminance(theme.text)
            .map(|l| l > 0.5)
            .unwrap_or(true);
        let surface = theme
            .background
            .or(theme.background_panel)
            .or(theme.background_element)
            .unwrap_or(if dark_mode {
                Color::Rgb(0x1b, 0x1d, 0x22)
            } else {
                Color::Rgb(0xe8, 0xe9, 0xec)
            });
        let wash = |fg: Color, alpha: f32| crate::color::blend_colors(surface, fg, alpha);

        // Per-level heading bands, tinted with each heading's own color.
        let heading_bands = std::array::from_fn(|i| wash(heading_colors[i], 0.18));
        // Inline chip reads clearly; the code panel is a subtler lift.
        let inline_bg = wash(theme.text, 0.16);
        let code_bg = wash(theme.text, 0.09);
        MarkdownStyles {
            text: Style::default().fg(theme.text),
            muted: Style::default().fg(theme.text_muted),
            code: Style::default().fg(theme.warning).bg_opt(inline_bg),
            link: Style::default()
                .fg(theme.info)
                .add_modifier(Modifier::UNDERLINED),
            rule: Style::default().fg(theme.border_subtle),
            list_marker: Style::default().fg(theme.accent).add_modifier(bold),
            check_done: Style::default().fg(theme.success).add_modifier(bold),
            check_todo: Style::default().fg(theme.text_muted),
            table_border: Style::default().fg(theme.border_subtle),
            table_head: Style::default().fg(theme.accent).add_modifier(bold),
            code_lang: Style::default()
                .fg(theme.accent)
                .add_modifier(bold)
                .bg_opt(code_bg),
            headings,
            heading_marks: ["█ ", "▊ ", "▌ ", "▎ ", "▏ ", "· "],
            heading_bands,
            list_colors: [theme.accent, theme.success, theme.info, theme.warning],
            inline_bg,
            code_bg,
        }
    }
}

/// A GitHub-style callout (`> [!NOTE]`) rendered as a colored, titled quote.
struct Callout {
    icon: &'static str,
    label: &'static str,
    color: Color,
}

impl Callout {
    fn from_kind(kind: BlockQuoteKind, theme: &crate::config::ResolvedTheme) -> Self {
        match kind {
            BlockQuoteKind::Note => Callout {
                icon: "ℹ",
                label: "Note",
                color: theme.info,
            },
            BlockQuoteKind::Tip => Callout {
                icon: "✎",
                label: "Tip",
                color: theme.success,
            },
            BlockQuoteKind::Important => Callout {
                icon: "★",
                label: "Important",
                color: theme.primary,
            },
            BlockQuoteKind::Warning => Callout {
                icon: "▲",
                label: "Warning",
                color: theme.warning,
            },
            BlockQuoteKind::Caution => Callout {
                icon: "✖",
                label: "Caution",
                color: theme.error,
            },
        }
    }
}

/// One active block-quote level; the border is repeated on every wrapped line.
struct QuoteFrame {
    border: Style,
}

#[derive(Default)]
struct MarkdownList {
    next: Option<u64>,
    marker_width: usize,
}

/// A table cell holds styled spans, so inline code, emphasis, and links keep
/// their colors instead of being flattened to plain text.
type TableCell = Vec<Span<'static>>;

#[derive(Default)]
struct MarkdownTable {
    rows: Vec<Vec<TableCell>>,
    current_row: Vec<TableCell>,
    current_cell: TableCell,
    in_cell: bool,
}

struct MarkdownCodeBlock {
    lang: Option<String>,
    text: String,
}

struct MarkdownImage {
    dest_url: String,
    alt: String,
}

type CodeHighlighter<'a> = dyn FnMut(Option<&str>, &str) -> Option<Vec<Vec<Span<'static>>>> + 'a;

/// A clickable hyperlink located in content (line/column) coordinates, produced
/// by the renderer and later mapped to screen coordinates in `render_preview`.
pub(crate) struct PreviewLink {
    pub(crate) line: usize,
    pub(crate) col: usize,
    pub(crate) width: usize,
    pub(crate) url: String,
}

fn markdown_preview_lines(
    text: &str,
    app: &mut App,
    width: usize,
    base_dir: Option<&Path>,
    change_bars: Option<&PreviewChangeBars>,
) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
    let styles = MarkdownStyles::from_theme(&app.theme);
    let theme = app.theme.clone();
    let mut highlight = |lang: Option<&str>, code: &str| app.highlight_code_block(lang, code);
    let content_width = width
        .saturating_sub(preview_change_gutter_width(change_bars))
        .max(1);
    let mut renderer = MarkdownRenderer::new(
        &styles,
        &theme,
        content_width,
        base_dir,
        &mut highlight,
        change_bars,
    );
    renderer.run(text);
    renderer.finish()
}

struct MarkdownRenderer<'a> {
    styles: &'a MarkdownStyles,
    theme: &'a crate::config::ResolvedTheme,
    width: usize,
    base_dir: Option<PathBuf>,
    highlight: &'a mut CodeHighlighter<'a>,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    style_stack: Vec<Style>,
    lists: Vec<MarkdownList>,
    quotes: Vec<QuoteFrame>,
    link_urls: Vec<String>,
    images: Vec<MarkdownImage>,
    table: Option<MarkdownTable>,
    code_block: Option<MarkdownCodeBlock>,
    pending_marker: Option<Span<'static>>,
    /// True while emitting the body of a completed task-list item.
    done_task: bool,
    /// Clickable links collected in content coordinates.
    links: Vec<PreviewLink>,
    /// (index into `current`, url) for link spans on the line being built.
    current_link_marks: Vec<(usize, String)>,
    change_bars: Option<&'a PreviewChangeBars>,
    line_change_styles: Vec<Option<Style>>,
    current_change_style: Option<Style>,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(
        styles: &'a MarkdownStyles,
        theme: &'a crate::config::ResolvedTheme,
        width: usize,
        base_dir: Option<&Path>,
        highlight: &'a mut CodeHighlighter<'a>,
        change_bars: Option<&'a PreviewChangeBars>,
    ) -> Self {
        MarkdownRenderer {
            styles,
            theme,
            width: width.max(1),
            base_dir: base_dir.map(Path::to_path_buf),
            highlight,
            lines: Vec::new(),
            current: Vec::new(),
            style_stack: vec![styles.text],
            lists: Vec::new(),
            quotes: Vec::new(),
            link_urls: Vec::new(),
            images: Vec::new(),
            table: None,
            code_block: None,
            pending_marker: None,
            done_task: false,
            links: Vec::new(),
            current_link_marks: Vec::new(),
            change_bars,
            line_change_styles: Vec::new(),
            current_change_style: None,
        }
    }

    fn run(&mut self, text: &str) {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_TASKLISTS);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_FOOTNOTES);
        options.insert(Options::ENABLE_GFM);
        options.insert(Options::ENABLE_DEFINITION_LIST);
        options.insert(Options::ENABLE_HEADING_ATTRIBUTES);

        // The current heading level while inside a heading (for band + level style).
        let mut heading_level: Option<u8> = None;
        let line_starts = markdown_line_starts(text);

        for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
            self.note_change_range(&line_starts, range);
            match event {
                Event::Start(tag) => match tag {
                    Tag::Paragraph => {}
                    Tag::Heading { level, .. } => {
                        self.blank();
                        let idx = (level as usize).clamp(1, 6) - 1;
                        heading_level = Some(level as u8);
                        self.current.push(Span::styled(
                            self.styles.heading_marks[idx].to_string(),
                            self.styles.headings[idx],
                        ));
                        self.style_stack.push(self.styles.headings[idx]);
                    }
                    Tag::BlockQuote(kind) => {
                        let callout = kind.map(|k| Callout::from_kind(k, self.theme));
                        let border = match &callout {
                            Some(c) => Style::default().fg(c.color).add_modifier(Modifier::BOLD),
                            None => self.styles.muted,
                        };
                        self.quotes.push(QuoteFrame { border });
                        if let Some(c) = callout {
                            let title = Style::default().fg(c.color).add_modifier(Modifier::BOLD);
                            self.current
                                .push(Span::styled(format!("{} {}", c.icon, c.label), title));
                            self.flush();
                        }
                        self.style_stack.push(self.styles.muted);
                    }
                    Tag::CodeBlock(kind) => {
                        self.blank();
                        let lang = match kind {
                            CodeBlockKind::Fenced(lang) => {
                                let lang = lang.trim();
                                (!lang.is_empty()).then(|| lang.to_string())
                            }
                            CodeBlockKind::Indented => None,
                        };
                        self.code_block = Some(MarkdownCodeBlock {
                            lang,
                            text: String::new(),
                        });
                    }
                    Tag::List(start) => {
                        // In a tight list the parent item's text arrives before
                        // its nested list; flush it onto its own line first.
                        self.flush();
                        self.lists.push(MarkdownList {
                            next: start,
                            marker_width: 0,
                        });
                    }
                    Tag::Item => {
                        // Each item's task state is independent; a TaskListMarker
                        // sets it back on if this item is a completed task.
                        self.done_task = false;
                        let depth = self.lists.len();
                        let marker = match self.lists.last().and_then(|l| l.next) {
                            Some(next) => format!("{next}. "),
                            None => markdown_bullet(depth).to_string(),
                        };
                        if let Some(list) = self.lists.last_mut() {
                            list.marker_width = text_width(&marker);
                        }
                        // Ordered markers keep the accent color; bullets cycle
                        // color by nesting depth like markview.
                        let marker_style = if self.lists.last().and_then(|l| l.next).is_some() {
                            self.styles.list_marker
                        } else {
                            let color = self.styles.list_colors[(depth.saturating_sub(1)) % 4];
                            Style::default().fg(color).add_modifier(Modifier::BOLD)
                        };
                        self.pending_marker = Some(Span::styled(marker, marker_style));
                    }
                    Tag::FootnoteDefinition(label) => {
                        self.blank();
                        self.current
                            .push(Span::styled(format!("[{label}] "), self.styles.muted));
                    }
                    Tag::DefinitionList | Tag::DefinitionListTitle => {}
                    Tag::DefinitionListDefinition => {
                        self.current
                            .push(Span::styled("  ".to_string(), self.styles.muted));
                    }
                    Tag::Table(_) => self.table = Some(MarkdownTable::default()),
                    Tag::TableHead | Tag::TableRow => {
                        if let Some(table) = self.table.as_mut() {
                            table.current_row.clear();
                        }
                    }
                    Tag::TableCell => {
                        if let Some(table) = self.table.as_mut() {
                            table.current_cell.clear();
                            table.in_cell = true;
                        }
                    }
                    Tag::Emphasis => self
                        .style_stack
                        .push(self.top_style().add_modifier(Modifier::ITALIC)),
                    Tag::Strong => self
                        .style_stack
                        .push(self.top_style().add_modifier(Modifier::BOLD)),
                    Tag::Strikethrough => self
                        .style_stack
                        .push(self.top_style().add_modifier(Modifier::CROSSED_OUT)),
                    Tag::Superscript | Tag::Subscript => self
                        .style_stack
                        .push(self.top_style().add_modifier(Modifier::DIM)),
                    Tag::Link { dest_url, .. } => {
                        self.link_urls.push(dest_url.to_string());
                        self.style_stack.push(self.styles.link);
                    }
                    Tag::Image { dest_url, .. } => {
                        self.images.push(MarkdownImage {
                            dest_url: dest_url.to_string(),
                            alt: String::new(),
                        });
                        self.style_stack.push(self.styles.link);
                    }
                    Tag::HtmlBlock | Tag::MetadataBlock(_) => {
                        self.style_stack.push(self.styles.muted)
                    }
                },
                Event::End(end) => match end {
                    TagEnd::Paragraph => {
                        self.flush();
                        self.blank();
                    }
                    TagEnd::Heading(_) => {
                        self.style_stack.pop();
                        let level = heading_level.take().unwrap_or(1);
                        self.flush_heading(level);
                        self.blank();
                    }
                    TagEnd::BlockQuote(_) => {
                        self.style_stack.pop();
                        self.flush();
                        // Drop the dangling border-only line the closing blank
                        // separator left after the quote's last content line.
                        if self.lines.last().is_some_and(markdown_line_is_quote_border) {
                            self.lines.pop();
                            self.line_change_styles.pop();
                        }
                        self.quotes.pop();
                        self.blank();
                    }
                    TagEnd::CodeBlock => {
                        if let Some(block) = self.code_block.take() {
                            self.render_code_block(block);
                        }
                        self.blank();
                    }
                    TagEnd::List(_) => {
                        self.lists.pop();
                        self.blank();
                    }
                    TagEnd::Item => {
                        self.flush();
                        self.pending_marker = None;
                        if let Some(list) = self.lists.last_mut() {
                            if let Some(next) = list.next.as_mut() {
                                *next = next.saturating_add(1);
                            }
                        }
                    }
                    TagEnd::FootnoteDefinition
                    | TagEnd::DefinitionList
                    | TagEnd::DefinitionListTitle
                    | TagEnd::DefinitionListDefinition => {
                        self.flush();
                    }
                    TagEnd::Table => {
                        if let Some(table) = self.table.take() {
                            self.render_table(table);
                            self.blank();
                        }
                    }
                    TagEnd::TableHead | TagEnd::TableRow => {
                        if let Some(table) = self.table.as_mut() {
                            table.rows.push(std::mem::take(&mut table.current_row));
                        }
                    }
                    TagEnd::TableCell => {
                        if let Some(table) = self.table.as_mut() {
                            let cell = trim_cell_spans(std::mem::take(&mut table.current_cell));
                            table.current_row.push(cell);
                            table.in_cell = false;
                        }
                    }
                    TagEnd::Emphasis
                    | TagEnd::Strong
                    | TagEnd::Strikethrough
                    | TagEnd::Superscript
                    | TagEnd::Subscript
                    | TagEnd::HtmlBlock
                    | TagEnd::MetadataBlock(_) => {
                        self.style_stack.pop();
                    }
                    TagEnd::Link => {
                        self.style_stack.pop();
                        // Push the URL suffix while the link is still active so
                        // it becomes part of the same clickable region.
                        if let Some(url) = self.link_urls.last().cloned() {
                            self.push_text(&format!(" ({url})"), self.styles.muted);
                        }
                        self.link_urls.pop();
                    }
                    TagEnd::Image => {
                        self.style_stack.pop();
                        if let Some(image) = self.images.pop() {
                            self.render_image(image);
                        }
                    }
                },
                Event::Text(text) => {
                    if let Some(image) = self.images.last_mut() {
                        image.alt.push_str(text.as_ref());
                    } else if let Some(block) = self.code_block.as_mut() {
                        block.text.push_str(text.as_ref());
                    } else {
                        let style = self.top_style();
                        self.push_text(text.as_ref(), style);
                    }
                }
                Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                    if let Some(image) = self.images.last_mut() {
                        image.alt.push_str(text.as_ref());
                    } else {
                        self.push_inline_code(text.as_ref());
                    }
                }
                Event::Html(text) | Event::InlineHtml(text) => {
                    self.push_text(text.as_ref(), self.styles.muted)
                }
                Event::FootnoteReference(label) => {
                    self.push_text(&format!("[{label}]"), self.styles.muted)
                }
                Event::SoftBreak => {
                    if let Some(image) = self.images.last_mut() {
                        image.alt.push(' ');
                    } else {
                        let style = self.top_style();
                        self.push_text(" ", style);
                    }
                }
                Event::HardBreak => self.flush(),
                Event::Rule => {
                    self.blank();
                    // A centered diamond ornament on a full-width rule.
                    let mid = self.width / 2;
                    let left = "─".repeat(mid);
                    let right = "─".repeat(self.width.saturating_sub(mid + 1));
                    self.push_current_line(Line::from(Span::styled(
                        format!("{left}◇{right}"),
                        self.styles.rule,
                    )));
                    self.blank();
                }
                Event::TaskListMarker(done) => {
                    // Completed items get dimmed + struck-through body text.
                    self.done_task = done;
                    let (glyph, style) = if done {
                        ("▣ ", self.styles.check_done)
                    } else {
                        ("▢ ", self.styles.check_todo)
                    };
                    if self.pending_marker.is_some() {
                        if let Some(list) = self.lists.last_mut() {
                            list.marker_width = text_width(glyph);
                        }
                        self.pending_marker = Some(Span::styled(glyph.to_string(), style));
                    } else {
                        self.push_text(glyph, style);
                    }
                }
            }
        }
    }

    fn finish(mut self) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
        self.flush();
        while self.lines.last().is_some_and(markdown_line_is_blank) {
            self.lines.pop();
            self.line_change_styles.pop();
        }
        if self.lines.is_empty() {
            self.push_line(Line::from(""), None);
        }
        self.add_change_gutters();
        (self.lines, self.links)
    }

    fn top_style(&self) -> Style {
        self.style_stack.last().copied().unwrap_or_default()
    }

    fn note_change_range(&mut self, line_starts: &[usize], range: std::ops::Range<usize>) {
        let Some(change_bars) = self.change_bars else {
            return;
        };
        if range.is_empty() {
            return;
        }
        let start = markdown_line_for_byte(line_starts, range.start);
        let end = markdown_line_for_byte(line_starts, range.end.saturating_sub(1));
        for line in start..=end {
            if let Some(style) = change_bars.styles.get(&line) {
                self.current_change_style.get_or_insert(*style);
            }
        }
    }

    fn push_line(&mut self, line: Line<'static>, style: Option<Style>) {
        self.lines.push(line);
        self.line_change_styles.push(style);
    }

    fn push_current_line(&mut self, line: Line<'static>) {
        let style = self.current_change_style.take();
        self.push_line(line, style);
    }

    fn add_change_gutters(&mut self) {
        let Some(change_bars) = self.change_bars else {
            return;
        };
        for (idx, line) in self.lines.iter_mut().enumerate() {
            let style = self.line_change_styles.get(idx).copied().flatten();
            let marker = match style {
                Some(style) => Span::styled(change_bars.marker.clone(), style),
                None => Span::raw(" ".repeat(change_bars.marker_width)),
            };
            let mut spans = vec![marker, Span::raw(" ")];
            spans.extend(std::mem::take(&mut line.spans));
            line.spans = spans;
        }
        let gutter = change_bars.gutter_width();
        for link in &mut self.links {
            link.col = link.col.saturating_add(gutter);
        }
    }

    /// Leading spans repeated at the start of every visual line: block-quote
    /// borders followed by the list marker (or hanging-indent padding).
    fn line_prefix(&mut self) -> Vec<Span<'static>> {
        let mut prefix = Vec::new();
        for quote in &self.quotes {
            prefix.push(Span::styled("▎ ".to_string(), quote.border));
        }
        let depth = self.lists.len();
        if depth > 0 {
            let indent = 2 * (depth - 1);
            if let Some(marker) = self.pending_marker.take() {
                if indent > 0 {
                    prefix.push(Span::styled(" ".repeat(indent), self.styles.text));
                }
                prefix.push(marker);
            } else {
                let cont = indent + self.lists.last().map_or(0, |l| l.marker_width);
                if cont > 0 {
                    prefix.push(Span::styled(" ".repeat(cont), self.styles.text));
                }
            }
        }
        prefix
    }

    fn flush(&mut self) {
        if self.current.is_empty() {
            self.current_link_marks.clear();
            return;
        }
        let line_idx = self.lines.len();
        let prefix = self.line_prefix();
        let prefix_w: usize = prefix.iter().map(|s| text_width(&s.content)).sum();
        if !self.current_link_marks.is_empty() {
            let mut col = prefix_w;
            let mut cols = Vec::with_capacity(self.current.len());
            for span in &self.current {
                cols.push(col);
                col += text_width(&span.content);
            }
            let span_w = |i: usize| self.current.get(i).map_or(0, |s| text_width(&s.content));
            // Coalesce consecutive same-url spans into one clickable region.
            let mut marks = std::mem::take(&mut self.current_link_marks)
                .into_iter()
                .peekable();
            while let Some((idx, url)) = marks.next() {
                let Some(&start) = cols.get(idx) else {
                    continue;
                };
                let mut end = start + span_w(idx);
                while let Some((nidx, nurl)) = marks.peek() {
                    if *nurl == url && cols.get(*nidx) == Some(&end) {
                        end += span_w(*nidx);
                        marks.next();
                    } else {
                        break;
                    }
                }
                self.links.push(PreviewLink {
                    line: line_idx,
                    col: start,
                    width: end - start,
                    url,
                });
            }
        }
        let mut line = prefix;
        line.append(&mut self.current);
        self.push_current_line(Line::from(line));
    }

    fn push_prefixed_line(&mut self, spans: &mut Vec<Span<'static>>, style: Option<Style>) {
        let mut line = self.line_prefix();
        line.append(spans);
        self.push_line(Line::from(line), style);
    }

    /// Emit the current heading line on a full-width band tinted with the
    /// heading's own color (markview-style colored heading bars).
    fn flush_heading(&mut self, level: u8) {
        // Heading links aren't tracked as clickable; drop any pending marks.
        self.current_link_marks.clear();
        if self.current.is_empty() {
            return;
        }
        let idx = (level as usize).clamp(1, 6) - 1;
        let mut spans = self.line_prefix();
        spans.append(&mut self.current);
        if let Some(band) = self.styles.heading_bands[idx] {
            for span in &mut spans {
                span.style = span.style.bg(band);
            }
            let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
            if used < self.width {
                spans.push(Span::styled(
                    " ".repeat(self.width - used),
                    Style::default().bg(band),
                ));
            }
        }
        self.push_current_line(Line::from(spans));
    }

    fn blank(&mut self) {
        if self.quotes.is_empty() {
            if !self.lines.last().is_some_and(markdown_line_is_blank) {
                self.push_line(Line::from(""), None);
            }
        } else {
            // Continue the quote border through the blank separator line.
            let prefix: Vec<Span<'static>> = self
                .quotes
                .iter()
                .map(|q| Span::styled("▎".to_string(), q.border))
                .collect();
            self.push_line(Line::from(prefix), None);
        }
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if let Some(table) = self.table.as_mut() {
            let cell = text.replace('\n', " ");
            if !cell.is_empty() {
                table.current_cell.push(Span::styled(cell, style));
            }
            return;
        }
        let style = self.done_style(style);
        let linking = self.link_urls.last().cloned();
        for (idx, part) in text.split('\n').enumerate() {
            if idx > 0 {
                self.flush();
            }
            if !part.is_empty() {
                self.current.push(Span::styled(part.to_string(), style));
                if let Some(url) = &linking {
                    self.current_link_marks
                        .push((self.current.len() - 1, url.clone()));
                }
            }
        }
    }

    /// Fade and strike a style when inside a completed task-list item.
    fn done_style(&self, style: Style) -> Style {
        if self.done_task {
            style.add_modifier(Modifier::DIM | Modifier::CROSSED_OUT)
        } else {
            style
        }
    }

    /// Inline code / math as a padded background "chip" (` code `), matching
    /// markview. In table cells the padding is dropped to keep columns tight,
    /// but the code keeps its color/background.
    fn push_inline_code(&mut self, text: &str) {
        if let Some(table) = self.table.as_mut() {
            let cell = text.replace('\n', " ");
            if !cell.is_empty() {
                table
                    .current_cell
                    .push(Span::styled(cell, self.styles.code));
            }
            return;
        }
        let chip = if self.styles.inline_bg.is_some() {
            format!(" {} ", text.replace('\n', " "))
        } else {
            text.replace('\n', " ")
        };
        let style = self.done_style(self.styles.code);
        self.current.push(Span::styled(chip, style));
    }

    fn render_code_block(&mut self, block: MarkdownCodeBlock) {
        let highlighted = (self.highlight)(block.lang.as_deref(), &block.text);
        let bg = self.styles.code_bg;
        let code_lines: Vec<&str> = block.text.lines().collect();

        // Shrink-wrap the panel to its content (like display: inline-block):
        // the wider of the longest code line and the language label, rather than
        // stretching to the viewport. PAD columns of inner padding each side, a
        // minimum width (≈45% of the viewport) so short snippets still read as a
        // panel, and a blank padded row below for vertical breathing room.
        const PAD: usize = 2;
        let min_width = (self.width * 45 / 100).min(self.width);
        let label = block.lang.as_deref().unwrap_or("code");
        let code_max = code_lines.iter().map(|l| text_width(l)).max().unwrap_or(0);
        let width = (PAD + code_max + PAD)
            .max(PAD + text_width(label) + PAD)
            .max(min_width)
            .min(self.width);
        let change_style = self.current_change_style.take();
        let pad_style = Style::default().bg_opt(bg);
        let pad = |n: usize| Span::styled(" ".repeat(n), pad_style);

        // Header row: language label near the right edge, with a one-column
        // trailing space before the panel border.
        let header = vec![
            pad(width.saturating_sub(text_width(label) + 1)),
            Span::styled(format!("{label} "), self.styles.code_lang),
        ];
        self.push_line(Line::from(header), change_style);

        for (idx, raw) in code_lines.iter().enumerate() {
            let mut spans = vec![pad(PAD)];
            match highlighted.as_ref().and_then(|rows| rows.get(idx)) {
                Some(row) => {
                    for span in row {
                        let mut style = span.style;
                        if let Some(bg) = bg {
                            style = style.bg(bg);
                        }
                        spans.push(Span::styled(span.content.to_string(), style));
                    }
                }
                None => {
                    let mut style = Style::default().fg(self.styles.code.fg.unwrap_or_default());
                    if let Some(bg) = bg {
                        style = style.bg(bg);
                    }
                    spans.push(Span::styled(raw.to_string(), style));
                }
            }
            pad_row(&mut spans, width, bg);
            self.push_line(Line::from(spans), change_style);
        }
        self.push_line(
            Line::from(Span::styled(" ".repeat(width), pad_style)),
            change_style,
        );
    }

    fn render_image(&mut self, image: MarkdownImage) {
        if self.table.is_some() {
            let label = image_fallback_label(&image.alt, &image.dest_url);
            self.push_text(&label, self.styles.muted);
            return;
        }
        self.flush();
        let change_style = self.current_change_style.take();
        let Some(path) = self.local_image_path(&image.dest_url) else {
            let mut line = vec![Span::styled(
                image_fallback_label(&image.alt, &image.dest_url),
                self.styles.muted,
            )];
            self.push_prefixed_line(&mut line, change_style);
            return;
        };
        let bg = self
            .theme
            .background
            .or(self.theme.background_panel)
            .unwrap_or(Color::Black);
        let Some(image_lines) = image_preview_lines(&path, self.width, bg) else {
            let mut line = vec![Span::styled(
                image_fallback_label(&image.alt, &image.dest_url),
                self.styles.muted,
            )];
            self.push_prefixed_line(&mut line, change_style);
            return;
        };
        let alt = image.alt.trim();
        if !alt.is_empty() {
            let mut caption = vec![Span::styled(format!("image: {alt}"), self.styles.muted)];
            self.push_prefixed_line(&mut caption, change_style);
        }
        for line in image_lines {
            let mut spans = line.spans;
            self.push_prefixed_line(&mut spans, change_style);
        }
        self.blank();
    }

    fn local_image_path(&self, url: &str) -> Option<PathBuf> {
        if url.is_empty() || url.starts_with('#') || url.starts_with("data:") || url.contains("://")
        {
            return None;
        }
        let path = url
            .split(['?', '#'])
            .next()
            .filter(|path| !path.is_empty())?;
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            self.base_dir.as_ref()?.join(path)
        };
        path.is_file().then_some(path)
    }

    fn render_table(&mut self, table: MarkdownTable) {
        let change_style = self.current_change_style.take();
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let mut widths = vec![0usize; cols];
        for row in &table.rows {
            for (idx, cell) in row.iter().enumerate() {
                widths[idx] = widths[idx].max(cell_width(cell));
            }
        }
        for width in &mut widths {
            *width = (*width).max(1);
        }

        let border = self.styles.table_border;
        let empty: TableCell = Vec::new();
        self.push_line(
            Line::from(Span::styled(table_border("╭", "┬", "╮", &widths), border)),
            change_style,
        );
        for (row_idx, row) in table.rows.iter().enumerate() {
            let is_head = row_idx == 0;
            let mut spans = vec![Span::styled("│".to_string(), border)];
            for (col, width) in widths.iter().enumerate().take(cols) {
                let cell = row.get(col).unwrap_or(&empty);
                spans.push(Span::styled(" ".to_string(), self.styles.text));
                for span in cell {
                    let style = self.table_cell_style(span.style, is_head);
                    spans.push(Span::styled(span.content.to_string(), style));
                }
                let pad = width.saturating_sub(cell_width(cell));
                spans.push(Span::styled(
                    format!("{} ", " ".repeat(pad)),
                    self.styles.text,
                ));
                spans.push(Span::styled("│".to_string(), border));
            }
            self.push_line(Line::from(spans), change_style);
            if is_head && table.rows.len() > 1 {
                self.push_line(
                    Line::from(Span::styled(table_border("├", "┼", "┤", &widths), border)),
                    change_style,
                );
            }
        }
        self.push_line(
            Line::from(Span::styled(table_border("╰", "┴", "╯", &widths), border)),
            change_style,
        );
    }

    /// Style a table cell span. Header cells make plain body text accent-bold
    /// (like a header) while leaving inline code / links their own color, and
    /// bolden special spans so the header row still reads as a header.
    fn table_cell_style(&self, span: Style, is_head: bool) -> Style {
        if !is_head {
            return span;
        }
        if span.fg == self.styles.text.fg {
            self.styles.table_head.add_modifier(span.add_modifier)
        } else {
            span.add_modifier(Modifier::BOLD)
        }
    }
}

fn markdown_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn markdown_line_for_byte(starts: &[usize], byte: usize) -> usize {
    match starts.binary_search(&byte) {
        Ok(index) => index + 1,
        Err(index) => index.max(1),
    }
}

fn image_fallback_label(alt: &str, url: &str) -> String {
    let alt = alt.trim();
    if alt.is_empty() {
        format!("image: {url}")
    } else {
        format!("image: {alt} ({url})")
    }
}

fn image_preview_lines(path: &Path, max_width: usize, bg: Color) -> Option<Vec<Line<'static>>> {
    const MAX_IMAGE_ROWS: u32 = 20;
    let image = image::ImageReader::open(path).ok()?.decode().ok()?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || max_width == 0 {
        return None;
    }
    let max_cols = max_width.clamp(1, 80) as u32;
    let max_pixel_rows = MAX_IMAGE_ROWS * 2;
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

/// Total display width of a table cell's spans.
fn cell_width(cell: &[Span<'_>]) -> usize {
    cell.iter().map(|s| text_width(&s.content)).sum()
}

/// Trim leading/trailing whitespace across a cell's styled spans.
fn trim_cell_spans(mut cell: TableCell) -> TableCell {
    while let Some(first) = cell.first_mut() {
        let trimmed = first.content.trim_start();
        if trimmed.is_empty() {
            cell.remove(0);
        } else {
            if trimmed.len() != first.content.len() {
                first.content = trimmed.to_string().into();
            }
            break;
        }
    }
    while let Some(last) = cell.last_mut() {
        let trimmed = last.content.trim_end();
        if trimmed.is_empty() {
            cell.pop();
        } else {
            if trimmed.len() != last.content.len() {
                last.content = trimmed.to_string().into();
            }
            break;
        }
    }
    cell
}

/// Depth-aware bullet marker for unordered list items (1-based depth).
fn markdown_bullet(depth: usize) -> &'static str {
    match depth {
        0 | 1 => "● ",
        2 => "○ ",
        3 => "◆ ",
        _ => "▸ ",
    }
}

/// Pad a row of spans with a trailing background block out to `width`.
fn pad_row(spans: &mut Vec<Span<'static>>, width: usize, bg: Option<Color>) {
    let Some(bg) = bg else { return };
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    if used < width {
        spans.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(bg),
        ));
    }
}

fn markdown_line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

/// True when a line contains only block-quote border glyphs and whitespace
/// (the continuation border drawn on blank lines inside a quote).
fn markdown_line_is_quote_border(line: &Line<'_>) -> bool {
    let mut saw_border = false;
    for span in &line.spans {
        for ch in span.content.chars() {
            match ch {
                '▎' => saw_border = true,
                c if c.is_whitespace() => {}
                _ => return false,
            }
        }
    }
    saw_border
}

fn table_border(left: &str, middle: &str, right: &str, widths: &[usize]) -> String {
    let mut line = String::from(left);
    for (idx, width) in widths.iter().enumerate() {
        if idx > 0 {
            line.push_str(middle);
        }
        line.push_str(&"─".repeat(width.saturating_add(2)));
    }
    line.push_str(right);
    line
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
    out.push_str("- use `?` to focus this help tab\n\n");

    out.push_str("## Tabs and preview\n\n");
    out.push_str("Each tab is a separate view. The same file can be open in more than one tab. Each tab keeps its own view mode, step mode and scroll position.\n\n");
    out.push_str("Preview mode shows file content instead of a diff. Markdown files open as rendered Markdown. Use the top-right `source` or `preview` button to switch between source and preview.\n\n");

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
        "Dashboard",
        |action| app.keybindings.dashboard_keys(action),
    ));
    out.push_str(&keybinding_section::<DashboardFilterAction, _>(
        "Dashboard filter",
        |action| app.keybindings.dashboard_filter_keys(action),
    ));

    append_embedded_doc(
        &mut out,
        "Configuration reference",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/CONFIG.md")),
    );
    append_embedded_doc(
        &mut out,
        "Diff viewer reference",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/DIFF_VIEWER.md"
        )),
    );
    append_embedded_doc(
        &mut out,
        "Keybindings reference",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/KEYBINDINGS.md"
        )),
    );
    append_embedded_doc(
        &mut out,
        "Review hooks reference",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/REVIEW_HOOKS.md"
        )),
    );
    append_embedded_doc(
        &mut out,
        "Theme reference",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/THEME.md")),
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
    if app.multi_diff.file_count() == 0 {
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
    let content_ranges = app.diff_selection_content_ranges();
    let cells = {
        let buffer = frame.buffer_mut();
        (y..max_y)
            .map(|row| {
                let mut cells = (x..max_x)
                    .map(|col| {
                        let local_col = col.saturating_sub(x);
                        if excluded
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
    if !app.review_mode() || app.review_editor_active() || app.selection_toolbar_visible() {
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
    if app.review_mode() {
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
    if let Some(bg) = app.theme.background_panel.or(app.theme.background) {
        block = block.style(Style::default().bg(bg));
    }

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    frame.render_widget(Paragraph::new(Line::from(footer_spans)), footer_area);

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
                Style::default().fg(app.theme.text),
            ))
        })
        .collect();

    frame.render_widget(Paragraph::new(text_lines), text_area);

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
    if app.is_multi_file() {
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
        &normal(NormalAction::ToggleFoldContext),
        "Toggle context folding",
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
        "Toggle stepping",
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
        "Cycle view mode",
    );
    push_help_line(
        &mut lines,
        &normal(NormalAction::ToggleViewModeReverse),
        "Cycle view mode (reverse)",
    );
    push_help_line(&mut lines, &normal(NormalAction::ToggleZen), "Zen mode");
    push_help_line(
        &mut lines,
        &normal(NormalAction::Refresh),
        "Refresh all files",
    );

    if app.is_multi_file() {
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

    let base_height = if app.is_multi_file() { 31 } else { 26 };
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

    let query = app.command_palette_query();
    let placeholder = "Search for commands…";
    let (query_text, query_style) = if query.is_empty() {
        (placeholder, Style::default().fg(app.theme.text_muted))
    } else {
        (query, Style::default().fg(app.theme.text))
    };
    let input_line = Line::from(vec![
        Span::styled("› ", Style::default().fg(app.theme.primary)),
        Span::styled(query_text, query_style),
    ]);
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

    let query = app.file_search_query();
    let placeholder = "Search for files…";
    let (query_text, query_style) = if query.is_empty() {
        (placeholder, Style::default().fg(app.theme.text_muted))
    } else {
        (query, Style::default().fg(app.theme.text))
    };
    let input_line = Line::from(vec![
        Span::styled("› ", Style::default().fg(app.theme.primary)),
        Span::styled(query_text, query_style),
    ]);
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

#[cfg(test)]
mod tests {
    use super::{counted_binding_label, MarkdownRenderer, MarkdownStyles};
    use crate::app::{App, ViewMode};
    use crate::config::ResolvedTheme;
    use crate::structured_preview::StructuredPreviewKind;
    use crate::syntax::SyntaxSide;
    use oyo_core::MultiFileDiff;
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Line;
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
    ) -> (Vec<Line<'static>>, Vec<super::PreviewLink>) {
        let styles = MarkdownStyles::from_theme(&theme);
        let mut highlight = |_lang: Option<&str>, _code: &str| None;
        let mut renderer =
            MarkdownRenderer::new(&styles, &theme, width, None, &mut highlight, None);
        renderer.run(md);
        renderer.finish()
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
    fn search_prompt_is_available_for_preview_status_bar() {
        let multi = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("README.md"),
            std::path::PathBuf::from("README.md"),
            "old".to_string(),
            "new".to_string(),
        );
        let mut app = App::new(multi, ViewMode::Preview, 50, false, None);
        app.start_search();
        let text: String = super::line_input_status_spans(&app)
            .unwrap()
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "/ Search");
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
        let lines = super::image_preview_lines(path, 16, ratatui::style::Color::Black)
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
        let styles = MarkdownStyles::from_theme(&theme);
        let bars = super::PreviewChangeBars {
            marker: "|".to_string(),
            marker_width: 1,
            styles: HashMap::from([(1, Style::default().fg(theme.accent))]),
        };
        let mut highlight = |_lang: Option<&str>, _code: &str| None;
        let mut renderer =
            MarkdownRenderer::new(&styles, &theme, 80, None, &mut highlight, Some(&bars));
        renderer.run("# Changed\n\nPlain\n");
        let (lines, _) = renderer.finish();
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
        // Borderless csvlens layout: top padding, header, separator, rows.
        assert!(flat[0].trim().is_empty(), "top padding line: {flat:?}");
        assert!(
            flat[1].contains("name") && flat[1].contains("age"),
            "header: {flat:?}"
        );
        assert!(flat[2].contains('┼'), "gutter separator: {flat:?}");
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
        assert!(super::markdown_line_is_quote_border(
            &lines[1] // the blank separator inside the quote
        ));
        assert!(!super::markdown_line_is_quote_border(lines.last().unwrap()));
    }
}
