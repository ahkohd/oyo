//! UI rendering for the TUI

use crate::app::{App, ViewMode};
use crate::views::{render_evolution, render_single_pane, render_split};
use oyo_core::FileStatus;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

/// Truncate a path to fit a given width, using /.../ for middle sections
fn truncate_path(path: &str, max_width: usize) -> String {
    if path.len() <= max_width {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 2 {
        // Just truncate from start if path is simple
        let suffix_len = max_width.saturating_sub(3);
        return format!("...{}", &path[path.len().saturating_sub(suffix_len)..]);
    }

    // Keep first and last parts, abbreviate middle
    let first = parts[0];
    let last = parts.last().unwrap_or(&"");

    // If just first + last fits with /.../, use that
    let simple = format!("{}/.../{}", first, last);
    if simple.len() <= max_width {
        return simple;
    }

    // Otherwise just show .../filename
    let suffix_len = max_width.saturating_sub(4);
    format!(".../{}", &last[last.len().saturating_sub(suffix_len)..])
}

/// Main drawing function
pub fn draw(frame: &mut Frame, app: &mut App) {
    if app.zen_mode {
        // Zen mode: just the content with minimal progress indicator
        draw_content(frame, app, frame.area());
        draw_zen_progress(frame, app);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),    // Main content
                Constraint::Length(1), // Status bar
            ])
            .split(frame.area());

        draw_content(frame, app, chunks[0]);
        draw_status_bar(frame, app, chunks[1]);
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
        ViewMode::SinglePane => " SINGLE ",
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

    // File counter (at the end)
    let file_count = app.multi_diff.file_count();
    let current_file = app.multi_diff.selected_index + 1;
    let file_text = format!("{}/{}", current_file, file_count);

    // Build CENTER section: search prompt or step counter
    let mut center_spans = Vec::new();
    let show_search = app.search_active();
    if show_search {
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
        right_spans.push(Span::styled(
            format!("hunk {}", hunk),
            Style::default().fg(app.theme.text_muted),
        ));
    }
    right_spans.push(Span::raw("  "));
    right_spans.push(Span::styled(
        format!("file {}", file_text),
        Style::default().fg(app.theme.text_muted),
    ));
    right_spans.push(Span::raw(" "));

    // Build LEFT section: mode + scope (path + branch)
    let center_width: usize = center_spans.iter().map(|s| s.content.len()).sum();
    let right_width: usize = right_spans.iter().map(|s| s.content.len()).sum();
    let left_fixed_width = mode.len() + 1;
    let min_padding = 2;
    let path_max_width = available_width
        .saturating_sub(center_width + right_width + left_fixed_width + min_padding * 2);
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

    // Calculate widths
    let left_width: usize = left_spans.iter().map(|s| s.content.len()).sum();
    // Calculate padding to center the middle section

    // Distribute padding: left_pad centers the center section, right_pad pushes right to edge
    let center_start = available_width / 2 - center_width / 2;
    let left_pad = center_start.saturating_sub(left_width);
    let right_pad = available_width.saturating_sub(center_start + center_width + right_width);

    // Build final spans
    let mut spans = left_spans;
    spans.push(Span::raw(" ".repeat(left_pad.max(1))));
    spans.extend(center_spans);
    spans.push(Span::raw(" ".repeat(right_pad.max(1))));
    spans.extend(right_spans);

    let status_line = Line::from(spans);
    let mut paragraph = Paragraph::new(status_line);
    if let Some(bg) = app.theme.background_element.or(app.theme.background) {
        paragraph = paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(paragraph, area);
}

fn draw_content(frame: &mut Frame, app: &mut App, area: Rect) {
    app.last_viewport_height = area.height as usize;
    // Auto-hide file panel if viewport is too narrow (need at least 50 cols for diff view)
    // But respect user's manual toggle preference
    let min_width_for_panel = 85; // 35 (panel) + 50 (diff view)

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
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(35), // File list width
                Constraint::Min(0),     // Diff view
            ])
            .split(area);

        draw_file_list(frame, app, chunks[0]);
        draw_diff_view(frame, app, chunks[1]);
    } else {
        // Single file mode, file panel hidden, or viewport too narrow
        draw_diff_view(frame, app, area);
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
    let root_label = "Root ";
    let root_max_width = header_area
        .width
        .saturating_sub((root_label.len() + 1) as u16) as usize;
    let root_display = truncate_path(&root_path, root_max_width);

    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(root_label, Style::default().fg(app.theme.text_muted)),
            Span::styled(root_display, Style::default().fg(app.theme.text_muted)),
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
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
            let header_max = list_area.width.saturating_sub(2).max(1) as usize;
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

        // Truncate filename to fit
        let file_name = file
            .display_name
            .rsplit('/')
            .next()
            .unwrap_or(&file.display_name);
        let max_name_len = list_area.width.saturating_sub(4 + signs_len as u16).max(1) as usize;
        let name = if file_name.len() > max_name_len {
            if max_name_len == 1 {
                "…".to_string()
            } else {
                format!(
                    "…{}",
                    &file_name[file_name.len().saturating_sub(max_name_len - 1)..]
                )
            }
        } else {
            file_name.to_string()
        };

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
        remaining -= 1;
        idx += 1;
    }

    let mut block = Block::default().padding(ratatui::widgets::Padding::new(1, 1, 1, 0));
    if let Some(bg) = panel_bg {
        block = block.style(Style::default().bg(bg));
    }

    let file_list = List::new(items).block(block);

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
    }
}

fn draw_diff_view(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.view_mode {
        ViewMode::SinglePane => render_single_pane(frame, app, area),
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

fn draw_help_popover(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Calculate popover size and position (centered)
    let popup_width = 44u16.min(area.width.saturating_sub(4));
    let base_height = if app.is_multi_file() { 31 } else { 26 };
    let popup_height = (base_height as u16).min(area.height.saturating_sub(4));
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area behind the popup
    frame.render_widget(Clear, popup_area);

    let key_style = Style::default().fg(app.theme.accent);
    let label_style = Style::default().fg(app.theme.text);
    let dim_style = Style::default().fg(app.theme.text_muted);
    let section_style = Style::default().fg(app.theme.primary);

    // Helper to create a padded key-value line
    let help_line = |key: &str, desc: String| -> Line {
        Line::from(vec![
            Span::styled(format!("  {:<12}", key), key_style),
            Span::styled(desc, label_style),
        ])
    };

    let mut lines = vec![
        Line::from(Span::styled(" Navigation", section_style)),
        help_line("j / k / ↑↓", "Step forward/back".into()),
        help_line("h / l / ←→", "Prev/next hunk".into()),
        help_line("b / e", "Hunk begin/end".into()),
        help_line("p / P", "Peek old (change/hunk)".into()),
        help_line("y / Y", "Yank line/hunk".into()),
        help_line("/", "Search (diff pane)".into()),
        help_line("n / N", "Next/prev match".into()),
        help_line("< / >", "First/last applied step".into()),
        help_line("gg / G", "Go to start/end".into()),
        help_line("J / K", "Scroll up/down".into()),
        help_line("H / L", "Scroll left/right".into()),
        help_line("0 / $", "Scroll to line start/end".into()),
        help_line("^U / ^D", "Scroll half-page".into()),
        help_line("^G", "Show full file path".into()),
        help_line("z", "Center on active".into()),
        help_line("w", "Toggle line wrap".into()),
        help_line("t", "Toggle syntax highlight".into()),
        help_line("T", "Toggle syntax scopes".into()),
        help_line("s", "Toggle stepping".into()),
        help_line("S", "Toggle strikethrough".into()),
        Line::from(""),
        Line::from(Span::styled(" Playback", section_style)),
        help_line("Space / B", "Autoplay forward/reverse".into()),
        help_line("+ / -", format!("Speed ({}ms)", app.animation_speed)),
        help_line("a", "Toggle animation".into()),
        Line::from(""),
        Line::from(Span::styled(" View", section_style)),
        help_line("Tab", "Cycle view mode".into()),
        help_line("Z", "Zen mode".into()),
        help_line("r", "Refresh from disk".into()),
    ];

    if app.is_multi_file() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(" Files", section_style)));
        lines.push(help_line("[ / ]", "Prev/next file".into()));
        lines.push(help_line("f", "Toggle file panel".into()));
        lines.push(help_line("Enter", "Focus file list".into()));
        lines.push(help_line("/", "Filter files (when focused)".into()));
        lines.push(help_line("r", "Refresh all (when focused)".into()));
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

    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(app.theme.border_active));
    if let Some(bg) = app.theme.background_panel {
        block = block.style(Style::default().bg(bg));
    }

    let help_block = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Left);

    frame.render_widget(help_block, popup_area);
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
