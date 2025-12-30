//! UI rendering for the TUI

use crate::app::{App, ViewMode, DIFF_VIEW_MIN_WIDTH, FILE_PANEL_MIN_WIDTH};
use crate::views::{render_evolution, render_split, render_unified_pane};
use oyo_core::FileStatus;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
    Frame,
};
use unicode_width::UnicodeWidthStr;

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
        return format!("...{}", &name[name.len().saturating_sub(suffix_len)..]);
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
        return format!("{head}...{tail}");
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
    format!("{head}...{tail}{ext}")
}

fn text_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
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

/// Truncate a path to fit a given width, using /.../ for middle sections
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

    // If just first + last fits with /.../, use that
    let prefix = format!("{}/.../", first);
    let available = max_width.saturating_sub(prefix.len());
    if available > 0 {
        let last_display = truncate_filename_keep_ext(last, available);
        let simple = format!("{prefix}{last_display}");
        if simple.len() <= max_width {
            return simple;
        }
    }

    // Otherwise just show .../filename
    if max_width <= 4 {
        return ".".repeat(max_width);
    }
    let prefix = ".../";
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
    format!("{acc}...")
}

/// Main drawing function
pub fn draw(frame: &mut Frame, app: &mut App) {
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

    // Draw help popover if active
    if app.show_help {
        draw_help_popover(frame, app);
    }

    // Draw file path popup if active
    if app.show_path_popup {
        draw_path_popup(frame, app);
    }
}

fn draw_status_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let state = app.state();
    let (insertions, deletions) = app.stats();

    // View mode indicator
    let mode = match app.view_mode {
        ViewMode::UnifiedPane => " UNIFIED ",
        ViewMode::Split => " SPLIT ",
        ViewMode::Evolution => " EVOLUTION ",
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
        Some(format!("{}/{}", current_hunk, total_hunks))
    } else {
        None
    };
    let hunk_step_text = if app.stepping {
        app.hunk_step_info().and_then(|(current, total)| {
            if current > 0 {
                Some(format!("{}/{}", current, total))
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
    let mut right_spans = vec![
        Span::styled(
            format!("+{}", insertions),
            Style::default().fg(app.theme.success),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{}", deletions),
            Style::default().fg(app.theme.error),
        ),
    ];
    if let Some(ref hunk) = hunk_text {
        right_spans.push(Span::raw("  "));
        let hunk_label = if let Some(ref hunk_step) = hunk_step_text {
            format!("hunk {} · {}", hunk, hunk_step)
        } else {
            format!("hunk {}", hunk)
        };
        right_spans.push(Span::styled(
            hunk_label,
            Style::default().fg(app.theme.text_muted),
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
    let (insertions, deletions) = app.stats();
    let file = app.multi_diff.current_file();
    let available_width = area.width as usize;
    let right_width = text_width(&format!("-{}  +{}", deletions, insertions));
    let left_max = available_width.saturating_sub(right_width + 2);

    let (name_text, status_style) = if let Some(file) = file {
        let file_name = file
            .display_name
            .rsplit('/')
            .next()
            .unwrap_or(&file.display_name);
        let name = truncate_filename_keep_ext(file_name, left_max.saturating_sub(3));
        let status_style = match file.status {
            FileStatus::Added | FileStatus::Untracked => Style::default().fg(app.theme.success),
            FileStatus::Deleted => Style::default().fg(app.theme.error),
            FileStatus::Modified => Style::default().fg(app.theme.warning),
            FileStatus::Renamed => Style::default().fg(app.theme.info),
        };
        (name, status_style)
    } else {
        (String::new(), Style::default().fg(app.theme.text_muted))
    };

    let mut left_spans = vec![
        Span::raw(" "),
        Span::styled("■", status_style),
        Span::raw(" "),
        Span::styled(name_text, Style::default().fg(app.theme.text)),
    ];
    left_spans = clamp_spans_to_width(&left_spans, left_max);
    left_spans = pad_spans_left(left_spans, left_max);

    let right_spans = vec![
        Span::styled(
            format!("-{}", deletions),
            Style::default().fg(app.theme.error),
        ),
        Span::raw("  "),
        Span::styled(
            format!("+{}", insertions),
            Style::default().fg(app.theme.success),
        ),
        Span::raw(" "),
    ];
    let right_spans = clamp_spans_to_width(&right_spans, right_width + 1);
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
        // Split: file list on left, diff view on right
        let panel_width = app.clamp_file_panel_width(area.width);
        app.file_panel_width = panel_width;
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(panel_width), // File list width
                Constraint::Min(0),              // Diff view
            ])
            .split(area);

        app.file_panel_rect = Some((chunks[0].x, chunks[0].y, chunks[0].width, chunks[0].height));
        draw_file_list(frame, app, chunks[0]);
        if show_topbar {
            let diff_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(chunks[1]);
            draw_top_bar(frame, app, diff_chunks[0]);
            app.last_viewport_height = diff_chunks[1].height as usize;
            draw_diff_view(frame, app, diff_chunks[1]);
        } else {
            app.last_viewport_height = chunks[1].height as usize;
            draw_diff_view(frame, app, chunks[1]);
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
            draw_diff_view(frame, app, diff_chunks[1]);
        } else {
            app.last_viewport_height = area.height as usize;
            draw_diff_view(frame, app, area);
        }
    }
}

fn draw_file_list(frame: &mut Frame, app: &mut App, area: Rect) {
    // Split area: content on left, separator on right
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),    // Content
            Constraint::Length(1), // Separator
        ])
        .split(area);

    let content_area = chunks[0];
    let separator_area = chunks[1];

    // Border color based on focus
    let border_fg = if app.file_list_focused {
        app.theme.border_active
    } else {
        app.theme.border_subtle
    };
    let panel_bg = app.theme.background_panel.or(app.theme.background);

    // Draw right separator - use main background, not panel background
    let mut separator_style = Style::default().fg(border_fg);
    if let Some(bg) = app.theme.background {
        separator_style = separator_style.bg(bg);
    }
    let separator_text = "▏\n".repeat(separator_area.height as usize);
    let separator = Paragraph::new(separator_text).style(separator_style);
    frame.render_widget(separator, separator_area);

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
    let mut items = Vec::new();
    let mut row_map: Vec<Option<usize>> = Vec::new();
    let mut remaining = list_area.height.saturating_sub(2) as usize;
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
            let header_max = list_area.width.saturating_sub(6).max(1) as usize;
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
        let show_signs = show_for_row && (file.insertions > 0 || file.deletions > 0);
        let insert_text = if show_signs {
            format!("+{}", file.insertions)
        } else {
            String::new()
        };
        let delete_text = if show_signs {
            format!("-{}", file.deletions)
        } else {
            String::new()
        };
        let signs_len = if show_signs {
            1 + insert_text.len() + 1 + delete_text.len()
        } else {
            0
        };

        // Truncate filename to fit (preserve extension)
        let file_name = file
            .display_name
            .rsplit('/')
            .next()
            .unwrap_or(&file.display_name);
        let max_name_len = list_area.width.saturating_sub(8 + signs_len as u16).max(1) as usize;
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
            line_spans.push(Span::styled(insert_text, sign_style));
            line_spans.push(Span::raw(" "));
            line_spans.push(Span::styled(delete_text, delete_style));
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

    app.file_list_area = Some((list_area.x, list_area.y, list_area.width, list_area.height));
    app.file_list_rows = row_map;

    frame.render_widget(file_list, list_area);

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
        frame.render_widget(empty, list_area);
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
            "\"/\" Filter".to_string()
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

fn draw_diff_view(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.view_mode {
        ViewMode::UnifiedPane => render_unified_pane(frame, app, area),
        ViewMode::Split => render_split(frame, app, area),
        ViewMode::Evolution => render_evolution(frame, app, area),
    }
}

fn draw_zen_progress(frame: &mut Frame, app: &mut App) {
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

    let mut help_keys = vec![
        "j / k / ↑↓",
        "h / l / ←→",
        "b / e",
        "p / P",
        "y / Y",
        "/",
        "n / N",
        ":<line>",
        ":h<num>",
        ":s<num>",
        "< / >",
        "gg / G",
        "J / K",
        "H / L",
        "0 / $",
        "^U / ^D",
        "^G",
        "z",
        "w",
        "t",
        "s",
        "S",
        "Space / B",
        "+ / -",
        "a",
        "Tab",
        "Z",
        "r",
    ];
    if app.is_multi_file() {
        help_keys.extend_from_slice(&["[ / ]", "f", "Enter", "j / k / ↑↓", "/", "r"]);
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
    push_help_line(&mut lines, "j / k / ↑↓", "Step forward/back");
    push_help_line(&mut lines, "h / l / ←→", "Prev/next hunk");
    push_help_line(&mut lines, "b / e", "Hunk begin/end");
    push_help_line(&mut lines, "p", "Peek change");
    push_help_line(&mut lines, "P", "Peek old hunk");
    push_help_line(&mut lines, "y / Y", "Yank line/hunk");
    push_help_line(&mut lines, "/", "Search (diff pane)");
    push_help_line(&mut lines, "n / N", "Next/prev match");
    push_help_line(&mut lines, ":<line>", "Go to line");
    push_help_line(&mut lines, ":h<num>", "Go to hunk");
    push_help_line(&mut lines, ":s<num>", "Go to step");
    push_help_line(&mut lines, "< / >", "First/last step (or hunk in no-step)");
    push_help_line(&mut lines, "gg / G", "Go to start/end");
    push_help_line(&mut lines, "J / K", "Scroll up/down");
    push_help_line(&mut lines, "H / L", "Scroll left/right");
    push_help_line(&mut lines, "0 / $", "Scroll to line start/end");
    push_help_line(&mut lines, "^U / ^D", "Scroll half-page");
    push_help_line(&mut lines, "^G", "Show full file path");
    push_help_line(&mut lines, "z", "Center on active");
    push_help_line(&mut lines, "w", "Toggle line wrap");
    push_help_line(&mut lines, "t", "Toggle syntax highlight");
    if app.view_mode == ViewMode::Evolution {
        push_help_line(&mut lines, "E", "Toggle evo syntax (context/full)");
    }
    push_help_line(&mut lines, "s", "Toggle stepping");
    push_help_line(&mut lines, "S", "Toggle strikethrough");
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" Playback", section_style)));
    push_help_line(&mut lines, "Space / B", "Autoplay forward/reverse");
    push_help_line(&mut lines, "r", "Replay last step");
    push_help_line(&mut lines, "nr", "Replay last n steps");
    push_help_line(
        &mut lines,
        "+ / -",
        &format!("Speed ({}ms)", app.animation_speed),
    );
    push_help_line(&mut lines, "a", "Toggle animation");
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" View", section_style)));
    push_help_line(&mut lines, "Tab", "Cycle view mode");
    push_help_line(&mut lines, "Z", "Zen mode");
    push_help_line(&mut lines, "R", "Refresh from disk");

    if app.is_multi_file() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(" Files", section_style)));
        push_help_line(&mut lines, "[ / ]", "Prev/next file");
        push_help_line(&mut lines, "f", "Toggle file panel");
        push_help_line(&mut lines, "Enter", "Focus file list");
        push_help_line(&mut lines, "j / k / ↑↓", "Move selection (focused)");
        push_help_line(&mut lines, "/", "Filter files (when focused)");
        push_help_line(&mut lines, "R", "Refresh all (when focused)");
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(format!("  {:<12}", "?"), key_style),
        Span::styled("Close help", dim_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled(format!("  {:<12}", "q / Esc"), key_style),
        Span::styled("Quit", label_style),
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
        .title(" Help ")
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background_panel {
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
    if let Some(bg) = app.theme.background_panel {
        block = block.style(Style::default().bg(bg));
    }

    let path_block = Paragraph::new(display_path)
        .block(block)
        .style(Style::default().fg(app.theme.text))
        .alignment(Alignment::Center);

    frame.render_widget(path_block, popup_area);
}
