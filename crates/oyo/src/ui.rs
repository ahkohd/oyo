//! UI rendering for the TUI

use crate::app::{
    diff_scrollbar_thumb, App, FilePanelScrollbarState, TopbarTabContent, TopbarTabHit, ViewMode,
    DIFF_VIEW_MIN_WIDTH, FILE_PANEL_MIN_WIDTH,
};
use crate::color;
use crate::config::FilePanelPosition;
use crate::keybindings::{
    BindingAction, DashboardAction, DashboardFilterAction, FileFilterAction, GlobalAction,
    HelpAction, LineInputAction, NormalAction, PickerAction, ReviewEditorAction, SelectionAction,
};
use crate::syntax::SyntaxSide;
use crate::views::{
    render_blame, render_diff_scrollbar, render_evolution, render_split, render_unified_pane,
    reserve_diff_scrollbar_lane,
};
use image::GenericImageView;
use oyo_core::{multi::DiffStatus, multi::FileSide, FileStatus};
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
use std::path::{Path, PathBuf};
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
    app.preview_toggle_hit = None;

    if app.multi_diff.file_count() == 0 {
        app.clear_diff_selection();
        app.set_diff_selection_cells(Vec::new());
        draw_no_changes(frame, app, frame.area());
        if app.show_help {
            draw_help_popover(frame, app);
        }
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
            app.clear_review_preview_boxes();
            draw_review_editor_overlay(frame, app);
        } else if !matches!(
            app.view_mode,
            ViewMode::UnifiedPane
                | ViewMode::Split
                | ViewMode::Evolution
                | ViewMode::Blame
                | ViewMode::Preview
        ) {
            draw_review_comment_overlays(frame, app);
        }
    } else {
        app.clear_review_preview_boxes();
    }
}

fn draw_preview_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mode = " PREVIEW ";
    let path = app.current_file_path();
    let state = if preview_can_render_markdown(app) {
        if app.active_preview_rendered() {
            "preview"
        } else {
            "source"
        }
    } else {
        "source"
    };
    let available_width = area.width as usize;
    let left_width = (available_width * 6) / 10;
    let right_width = available_width.saturating_sub(left_width);
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
    let right_spans = pad_spans_right(
        vec![Span::styled(
            state,
            Style::default().fg(app.theme.text_muted),
        )],
        right_width,
    );
    let mut spans = Vec::new();
    spans.extend(left_spans);
    spans.extend(right_spans);
    let mut paragraph = Paragraph::new(Line::from(spans));
    if let Some(bg) = app.theme.background {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, area);
}

fn draw_status_bar(frame: &mut Frame, app: &mut App, area: Rect) {
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
    let show_goto = app.goto_active();
    let show_search = app.search_active();
    if show_goto {
        center_spans.push(Span::styled(":", Style::default().fg(app.theme.text_muted)));
        center_spans.push(Span::raw(" "));
        let query = app.goto_query();
        let query_text = if app.goto_active() && query.is_empty() {
            "Go to".to_string()
        } else {
            query.to_string()
        };
        let query_style = if app.goto_active() && query.is_empty() {
            Style::default().fg(app.theme.text_muted)
        } else {
            Style::default().fg(app.theme.text)
        };
        center_spans.push(Span::styled(query_text, query_style));
    } else if show_search {
        center_spans.push(Span::styled("/", Style::default().fg(app.theme.text_muted)));
        center_spans.push(Span::raw(" "));
        let query = app.search_query();
        let query_text = if app.search_active() && query.is_empty() {
            "Search".to_string()
        } else {
            query.to_string()
        };
        let query_style = if app.search_active() && query.is_empty() {
            Style::default().fg(app.theme.text_muted)
        } else {
            Style::default().fg(app.theme.text)
        };
        center_spans.push(Span::styled(query_text, query_style));
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
        right_spans.push(Span::styled(
            comments_label,
            Style::default().fg(app.theme.primary),
        ));
    }
    right_spans.push(Span::raw("  "));
    right_spans.push(Span::styled(
        format!("file {}", file_text),
        Style::default().fg(app.theme.text_muted),
    ));
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
    let available_width = area.width as usize;
    app.preview_toggle_hit = None;
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
    let right_width = spans_width(&right_spans).min(available_width);
    if matches!(app.view_mode, ViewMode::Preview)
        && preview_can_render_markdown(app)
        && right_width > 0
    {
        app.preview_toggle_hit = Some((
            area.x
                .saturating_add(available_width.saturating_sub(right_width) as u16),
            area.y,
            right_width as u16,
            1,
        ));
    }
    let left_max = available_width.saturating_sub(right_width + 1);
    let mut left_spans = topbar_tab_spans(app, area, left_max);
    left_spans = clamp_spans_to_width(&left_spans, left_max);
    left_spans = pad_spans_left(left_spans, left_max);

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

fn preview_topbar_spans(app: &App) -> Vec<Span<'static>> {
    if preview_can_render_markdown(app) {
        let label = if app.active_preview_rendered() {
            " source "
        } else {
            " preview "
        };
        return vec![Span::styled(label, Style::default().fg(app.theme.accent))];
    }
    vec![Span::styled(
        " preview ",
        Style::default().fg(app.theme.text_muted),
    )]
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

    let closeable = app.topbar_tabs.len() > 1;
    let active = app.active_topbar_tab;
    let drag_target = app
        .topbar_drag_target
        .filter(|target| *target <= app.topbar_tabs.len());
    let mut spans = Vec::new();
    let mut col = 0usize;
    for (tab_pos, tab) in app.topbar_tabs.clone().into_iter().enumerate() {
        if drag_target == Some(tab_pos) && col < max_width {
            spans.push(Span::styled(
                "│",
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            col = col.saturating_add(1);
        }
        let remaining = max_width.saturating_sub(col);
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
        let style = if active_tab {
            Style::default()
                .fg(app.theme.background.unwrap_or(Color::Black))
                .bg(app.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.text_muted)
        };
        spans.push(Span::styled(format!(" {name}{changed}"), style));
        if closeable {
            spans.push(Span::styled(" ", style));
            let close_style = if app.topbar_hover_close == Some(tab.id) {
                brighten_close_style(style)
            } else {
                style
            };
            spans.push(Span::styled(
                if show_close { "×" } else { " " },
                close_style,
            ));
        }
        spans.push(Span::styled(" ", style));
        col = col.saturating_add(width);
        if col < max_width {
            spans.push(Span::raw(" "));
            col = col.saturating_add(1);
        }
    }

    if drag_target == Some(app.topbar_tabs.len()) && col < max_width {
        spans.push(Span::styled(
            "│",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
        col = col.saturating_add(1);
    }

    if col + 1 < max_width {
        spans.push(Span::raw(" "));
        let plus_col = area.x.saturating_add(col.saturating_add(1) as u16);
        app.topbar_plus_hit = Some((plus_col, area.y, 1, 1));
        let plus_style = Style::default()
            .fg(if app.topbar_plus_hover {
                app.theme.accent
            } else {
                app.theme.text_muted
            })
            .add_modifier(Modifier::BOLD);
        spans.push(Span::styled("+", plus_style));
    }
    spans
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
        Color::DarkGray | Color::Gray | Color::White => color,
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
    spans.push(Span::styled(
        "Older ",
        Style::default().fg(app.theme.text_muted),
    ));

    let base = app.theme.warning;
    let steps = blocks.saturating_sub(1).max(1) as f32;
    for idx in 0..blocks {
        let t = idx as f32 / steps;
        spans.push(Span::styled(
            "▮",
            Style::default().fg(color::ramp_color(base, t)),
        ));
    }

    spans.push(Span::styled(
        " Newer",
        Style::default().fg(app.theme.text_muted),
    ));
    spans.push(Span::raw(" "));
    spans
}

fn draw_content(frame: &mut Frame, app: &mut App, area: Rect, show_topbar: bool) {
    // Auto-hide file panel if viewport is too narrow (need at least 50 cols for diff view)
    // But respect user's manual toggle preference
    let min_width_for_panel = FILE_PANEL_MIN_WIDTH + DIFF_VIEW_MIN_WIDTH;

    // Track if panel would be auto-hidden (for toggle behavior)
    app.file_panel_auto_hidden = app.is_multi_file()
        && app.file_panel_visible
        && area.width < min_width_for_panel
        && !app.file_panel_manually_set;

    let show_panel = if app.file_panel_manually_set {
        // User explicitly toggled, respect their preference
        app.is_multi_file() && app.file_panel_visible
    } else {
        // Auto-hide when viewport is too narrow
        app.is_multi_file() && app.file_panel_visible && area.width >= min_width_for_panel
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
        app.file_filter_area = None;
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
    let symbol = if app.file_list_focused || app.file_filter_active {
        "▐"
    } else {
        "▕"
    };
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

fn draw_file_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let content_area = area;
    let panel_bg = app.theme.background_panel.or(app.theme.background);

    let show_filter =
        app.file_list_focused || app.file_filter_active || !app.file_filter.is_empty();
    let panel_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if show_filter {
            vec![
                Constraint::Length(5), // Header
                Constraint::Min(0),    // List
                Constraint::Length(3), // Filter
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
    let header_max_width = header_area.width.saturating_sub(1) as usize;
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

    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(header_text, Style::default().fg(app.theme.text_muted)),
        ]),
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
    ];

    let mut header = Paragraph::new(header_lines);
    if let Some(bg) = panel_bg {
        header = header.style(Style::default().bg(bg));
    }
    frame.render_widget(header, header_area);

    let filtered_indices = app.filtered_file_indices();
    let visible_file_rows = list_area.height.saturating_sub(2) as usize;
    let show_file_scrollbar = app.scrollbar_visible
        && filtered_indices.len() > visible_file_rows
        && visible_file_rows > 0
        && list_area.width > 1;
    let (list_content_area, file_scrollbar_area) =
        reserve_file_scrollbar_lane(list_area, show_file_scrollbar);
    let mut items = Vec::new();
    let mut row_map: Vec<Option<usize>> = Vec::new();
    let mut remaining = visible_file_rows;
    let mut current_group: Option<String> = None;

    let mut idx = app.file_list_scroll;
    while idx < filtered_indices.len() && remaining > 0 {
        let file_idx = filtered_indices[idx];
        let file = &files[file_idx];
        let group = match file.display_name.rsplit_once('/') {
            Some((dir, _)) => dir.to_string(),
            None => "Root Path".to_string(),
        };

        if current_group.as_deref() != Some(&group) {
            if current_group.is_some() && remaining > 0 {
                items.push(ListItem::new(Line::raw("")));
                row_map.push(None);
                remaining -= 1;
                if remaining == 0 {
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
            items.push(ListItem::new(header_line));
            row_map.push(None);
            current_group = Some(group);
            remaining -= 1;
            if remaining == 0 {
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
        let signs_len = if show_signs {
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

        let mut name_style = Style::default().fg(app.theme.text);
        if is_selected {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        if let Some(bg) = selected_bg {
            name_style = name_style.bg(bg);
        }

        let marker_style = if is_selected {
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

        items.push(ListItem::new(line));
        row_map.push(Some(file_idx));
        remaining -= 1;
        idx += 1;
    }

    let mut block = Block::default().padding(ratatui::widgets::Padding::new(1, 1, 1, 0));
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
        filtered_indices.len(),
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
        let filter_text = if app.file_filter_active {
            if has_query {
                format!("> {}", app.file_filter)
            } else {
                "> Filter file name".to_string()
            }
        } else if has_query {
            app.file_filter.clone()
        } else {
            format!(
                "{} Filter",
                app.keybindings
                    .normal_keys(NormalAction::OpenSearchOrFileFilter)
            )
        };
        let filter_style = if app.file_filter_active {
            Style::default().fg(app.theme.text)
        } else {
            Style::default().fg(app.theme.text_muted)
        };
        let mut filter = Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(filter_text, filter_style),
        ]))
        .alignment(Alignment::Left);
        let mut filter_block = Block::default().padding(ratatui::widgets::Padding::new(1, 1, 1, 0));
        if let Some(bg) = filter_bg {
            filter_block = filter_block.style(Style::default().bg(bg));
        }
        filter = filter.block(filter_block);
        frame.render_widget(filter, filter_area);
    } else {
        app.file_filter_area = None;
    }
}

fn render_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    if let Some(bg) = app.theme.background {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }
    let (content_area, scrollbar_area) = reserve_diff_scrollbar_lane(app, area);
    let (title, text, side, binary, base_dir) = preview_document(app);
    app.clear_preview_link_boxes();
    let (lines, links) = if binary {
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
    } else if preview_can_render_markdown(app) && app.active_preview_rendered() {
        markdown_preview_lines(&text, app, content_area.width as usize, base_dir.as_deref())
    } else {
        (source_preview_lines(app, &title, &text, side), Vec::new())
    };
    let total_lines = lines.len().max(1);
    let visible_lines = content_area.height as usize;
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

fn source_preview_lines(
    app: &mut App,
    file_name: &str,
    text: &str,
    side: Option<SyntaxSide>,
) -> Vec<Line<'static>> {
    let text_style = Style::default().fg(app.theme.text);
    let highlighted = if side.is_none() {
        app.preview_source_spans(file_name, text)
    } else {
        None
    };
    let mut lines = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if let Some(spans) = highlighted
            .as_ref()
            .and_then(|lines| lines.get(idx))
            .cloned()
        {
            lines.push(Line::from(spans));
            continue;
        }
        if let Some(side) = side {
            if let Some(spans) = app.syntax_spans_for_line(side, Some(idx + 1)) {
                lines.push(Line::from(spans));
                continue;
            }
        }
        lines.push(Line::from(Span::styled(line.to_string(), text_style)));
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
) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
    let styles = MarkdownStyles::from_theme(&app.theme);
    let theme = app.theme.clone();
    let mut highlight = |lang: Option<&str>, code: &str| app.highlight_code_block(lang, code);
    let mut renderer = MarkdownRenderer::new(&styles, &theme, width, base_dir, &mut highlight);
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
}

impl<'a> MarkdownRenderer<'a> {
    fn new(
        styles: &'a MarkdownStyles,
        theme: &'a crate::config::ResolvedTheme,
        width: usize,
        base_dir: Option<&Path>,
        highlight: &'a mut CodeHighlighter<'a>,
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

        for event in Parser::new_ext(text, options) {
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
                    self.lines.push(Line::from(Span::styled(
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
        }
        if self.lines.is_empty() {
            self.lines.push(Line::from(""));
        }
        (self.lines, self.links)
    }

    fn top_style(&self) -> Style {
        self.style_stack.last().copied().unwrap_or_default()
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
        self.lines.push(Line::from(line));
    }

    fn push_prefixed_line(&mut self, spans: &mut Vec<Span<'static>>) {
        let mut line = self.line_prefix();
        line.append(spans);
        self.lines.push(Line::from(line));
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
        self.lines.push(Line::from(spans));
    }

    fn blank(&mut self) {
        if self.quotes.is_empty() {
            if !self.lines.last().is_some_and(markdown_line_is_blank) {
                self.lines.push(Line::from(""));
            }
        } else {
            // Continue the quote border through the blank separator line.
            let prefix: Vec<Span<'static>> = self
                .quotes
                .iter()
                .map(|q| Span::styled("▎".to_string(), q.border))
                .collect();
            self.lines.push(Line::from(prefix));
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
        let pad_style = Style::default().bg_opt(bg);
        let pad = |n: usize| Span::styled(" ".repeat(n), pad_style);
        let blank_row = |lines: &mut Vec<Line<'static>>| {
            lines.push(Line::from(Span::styled(" ".repeat(width), pad_style)));
        };

        // Header row: language label near the right edge, with a one-column
        // trailing space before the panel border.
        let header = vec![
            pad(width.saturating_sub(text_width(label) + 1)),
            Span::styled(format!("{label} "), self.styles.code_lang),
        ];
        self.lines.push(Line::from(header));

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
            self.lines.push(Line::from(spans));
        }
        blank_row(&mut self.lines);
    }

    fn render_image(&mut self, image: MarkdownImage) {
        if self.table.is_some() {
            let label = image_fallback_label(&image.alt, &image.dest_url);
            self.push_text(&label, self.styles.muted);
            return;
        }
        self.flush();
        let Some(path) = self.local_image_path(&image.dest_url) else {
            let mut line = vec![Span::styled(
                image_fallback_label(&image.alt, &image.dest_url),
                self.styles.muted,
            )];
            self.push_prefixed_line(&mut line);
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
            self.push_prefixed_line(&mut line);
            return;
        };
        let alt = image.alt.trim();
        if !alt.is_empty() {
            let mut caption = vec![Span::styled(format!("image: {alt}"), self.styles.muted)];
            self.push_prefixed_line(&mut caption);
        }
        for line in image_lines {
            let mut spans = line.spans;
            self.push_prefixed_line(&mut spans);
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
        self.lines.push(Line::from(Span::styled(
            table_border("╭", "┬", "╮", &widths),
            border,
        )));
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
            self.lines.push(Line::from(spans));
            if is_head && table.rows.len() > 1 {
                self.lines.push(Line::from(Span::styled(
                    table_border("├", "┼", "┤", &widths),
                    border,
                )));
            }
        }
        self.lines.push(Line::from(Span::styled(
            table_border("╰", "┴", "╯", &widths),
            border,
        )));
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
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../CONFIG.md")),
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

fn draw_review_comment_overlays(frame: &mut Frame, app: &mut App) {
    app.clear_review_preview_boxes();

    let Some((x, y, width, height)) = app.diff_view_area else {
        return;
    };
    if width < 20 || height < 4 {
        return;
    }

    let diff_area = Rect::new(x, y, width, height);
    let overlays = app.review_comment_overlays_for_current_file();
    if overlays.is_empty() {
        return;
    }

    let scroll_offset = app.render_scroll_offset();
    let diff_bottom = diff_area.y.saturating_add(diff_area.height);

    if matches!(app.view_mode, ViewMode::UnifiedPane | ViewMode::Split) {
        return;
    }

    // Other modes: keep compact card previews.
    let max_popup_width = diff_area.width.saturating_sub(8);
    if max_popup_width < 16 {
        return;
    }
    let popup_width = if max_popup_width < 24 {
        max_popup_width
    } else {
        max_popup_width.min(40)
    };

    let mut next_free_y = diff_area.y;
    for overlay in overlays.into_iter().take(16) {
        if overlay.display_idx < scroll_offset {
            continue;
        }
        let row = overlay.display_idx.saturating_sub(scroll_offset) as u16;
        if row >= diff_area.height {
            continue;
        }

        let anchor_y = diff_area.y.saturating_add(row);
        let preferred_y = anchor_y.saturating_add(1);
        // Height 3 => 1 inner row (excerpt only)
        let popup_height = 3u16;
        let mut popup_y = preferred_y.max(next_free_y);

        // Keep collapsed preview below its anchor line when possible.
        if popup_y.saturating_add(popup_height) > diff_bottom {
            let fallback = diff_bottom.saturating_sub(popup_height);
            if fallback <= anchor_y {
                // No room below this anchor; skip instead of covering the anchor line.
                continue;
            }
            popup_y = fallback.max(next_free_y);
            if popup_y.saturating_add(popup_height) > diff_bottom || popup_y <= anchor_y {
                continue;
            }
        }

        let popup_x = diff_area.x.saturating_add(
            diff_area
                .width
                .saturating_sub(popup_width)
                .saturating_sub(1),
        );
        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.border_subtle));
        if let Some(bg) = app.theme.background_panel.or(app.theme.background) {
            block = block.style(Style::default().bg(bg));
        }
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let preview_text = app.review_preview_hint_text(&overlay);
        let preview = truncate_text(&preview_text, inner.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                preview,
                Style::default().fg(app.theme.text),
            ))),
            inner,
        );
        app.add_review_preview_box(
            popup_area.x,
            popup_area.y,
            popup_area.width,
            popup_area.height,
            overlay.anchor_key,
        );

        next_free_y = popup_y.saturating_add(popup_height);
    }
}

fn draw_review_editor_overlay(frame: &mut Frame, app: &mut App) {
    let Some(editor) = app.review_editor_render() else {
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

    let max_popup_width = editor_area.width.saturating_sub(2);
    let popup_width = if max_popup_width < 24 {
        max_popup_width
    } else {
        max_popup_width.min(72)
    };
    let desired_popup_height = (editor.lines.len() as u16).saturating_add(5).clamp(6, 12);
    let min_popup_height = 4u16;
    let mut popup_height =
        desired_popup_height.min(editor_area.height.saturating_sub(1).max(min_popup_height));

    let popup_x = editor_area.x.saturating_add(1);
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

    let mut footer_parts = vec![
        format!(
            "{} save",
            app.keybindings.review_editor_keys(ReviewEditorAction::Save)
        ),
        format!(
            "{} cancel",
            app.keybindings
                .review_editor_keys(ReviewEditorAction::Cancel)
        ),
        "@ mention".to_string(),
    ];
    footer_parts.extend(
        app.review_action_labels_for_editor()
            .into_iter()
            .map(|(key, label)| format!("{key} {label}")),
    );

    let mut block = Block::default()
        .title(Span::styled(
            editor.title,
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            format!(" {} ", footer_parts.join(" | ")),
            Style::default().fg(app.theme.text_muted),
        ))
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background_panel.or(app.theme.background) {
        block = block.style(Style::default().bg(bg));
    }

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    let anchor_line = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default().fg(app.theme.text_muted)),
        Span::styled(
            editor.anchor_label,
            Style::default().fg(app.theme.text_muted),
        ),
    ]));
    frame.render_widget(anchor_line, chunks[0]);

    let text_area = chunks[1];
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
    use crate::config::ResolvedTheme;
    use ratatui::style::Modifier;
    use ratatui::text::Line;

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
        let mut renderer = MarkdownRenderer::new(&styles, &theme, width, None, &mut highlight);
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
    fn markdown_table_uses_rounded_borders() {
        let lines = render_md("| key | value |\n| --- | --- |\n| a | 1 |\n", 80);
        let text = flatten(&lines);
        assert!(text.contains("╭"), "rounded top-left corner: {text}");
        assert!(text.contains("│ key │ value │"), "header row: {text}");
        assert!(text.contains("╰"), "rounded bottom-left corner: {text}");
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
