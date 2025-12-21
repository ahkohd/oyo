//! UI rendering for the TUI

use crate::app::{AnimationPhase, App, ViewMode};
use crate::views::{render_evolution, render_side_by_side, render_single_pane};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};
use oyo_core::FileStatus;

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
        ViewMode::SinglePane => " Single",
        ViewMode::SideBySide => " Split",
        ViewMode::Evolution => " Evolution",
    };

    // Format: Single  main  filepath.rs ▶ 2/10 +2 -2         1/2
    let file_path = app.current_file_path();
    let available_width = area.width as usize;
    // Reserve space for mode, branch, separator, step counter, stats, file counter
    let branch_len = app.git_branch.as_ref().map(|b| b.len() + 2).unwrap_or(0);
    let path_max_width = available_width.saturating_sub(50 + branch_len);

    // On narrow viewports, show just the filename
    let display_path = if available_width < 80 {
        // Extract just the filename
        file_path.rsplit('/').next().unwrap_or(&file_path).to_string()
    } else {
        truncate_path(&file_path, path_max_width)
    };

    // Step counter and arrow (flash when autoplay is on)
    let step_text = format!("{}/{}", state.current_step + 1, state.total_steps);
    let (arrow_style, step_style) = if app.autoplay {
        let flash = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            / 500) % 2 == 0;
        if flash {
            (Style::default().fg(Color::Yellow), Style::default().fg(Color::Yellow))
        } else {
            (Style::default().fg(Color::Rgb(180, 140, 0)), Style::default().fg(Color::Rgb(180, 140, 0)))
        }
    } else {
        (Style::default().fg(Color::DarkGray), Style::default().fg(Color::White))
    };

    // Hunk counter
    let (current_hunk, total_hunks) = app.hunk_info();
    let hunk_text = if total_hunks > 0 {
        Some(format!("@@{}/{}", current_hunk, total_hunks))
    } else {
        None
    };
    let hunk_len = hunk_text.as_ref().map(|s| s.len() + 1).unwrap_or(0);

    // File counter (at the end)
    let file_count = app.multi_diff.file_count();
    let current_file = app.multi_diff.selected_index + 1;
    let file_text = format!("{}/{}", current_file, file_count);

    // Calculate padding to push file counter to the right
    let left_content_width = mode.len() + 2 + branch_len + display_path.len() + 3 + step_text.len() + 1
        + hunk_len + format!("+{}", insertions).len() + 1 + format!("-{}", deletions).len();
    let right_width = file_text.len();
    let padding = (area.width as usize).saturating_sub(left_content_width + right_width + 1);

    // Build the status line
    let mut spans = vec![
        Span::styled(mode, Style::default().fg(Color::Magenta)),
        Span::raw("  "),
    ];

    // Add git branch if available
    if let Some(ref branch) = app.git_branch {
        spans.push(Span::styled(branch.as_str(), Style::default().fg(Color::Cyan)));
        spans.push(Span::raw("  "));
    }

    spans.extend([
        Span::styled(display_path, Style::default().fg(Color::White)),
        Span::styled(" ▶", arrow_style),
        Span::styled(step_text, step_style),
        Span::raw(" "),
    ]);

    // Add hunk counter if there are multiple hunks
    if let Some(ref hunk) = hunk_text {
        spans.push(Span::styled(hunk.clone(), Style::default().fg(Color::Blue)));
        spans.push(Span::raw(" "));
    }

    spans.extend([
        Span::styled(format!("+{}", insertions), Style::default().fg(Color::Green)),
        Span::raw(" "),
        Span::styled(format!("-{}", deletions), Style::default().fg(Color::Red)),
        Span::raw(" ".repeat(padding.max(1))),
        Span::styled(file_text, Style::default().fg(Color::White)),
    ]);

    let status_line = Line::from(spans);
    frame.render_widget(Paragraph::new(status_line), area);
}

fn draw_content(frame: &mut Frame, app: &mut App, area: Rect) {
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
    let files = &app.multi_diff.files;

    let items: Vec<ListItem> = files
        .iter()
        .enumerate()
        .skip(app.file_list_scroll)
        .take(area.height.saturating_sub(2) as usize)
        .map(|(idx, file)| {
            let status_icon = match file.status {
                FileStatus::Added | FileStatus::Untracked => "+",
                FileStatus::Deleted => "-",
                FileStatus::Modified => "~",
                FileStatus::Renamed => "→",
            };

            let status_style = match file.status {
                FileStatus::Added | FileStatus::Untracked => Style::default().fg(Color::Green),
                FileStatus::Deleted => Style::default().fg(Color::Red),
                FileStatus::Modified => Style::default().fg(Color::Yellow),
                FileStatus::Renamed => Style::default().fg(Color::Cyan),
            };

            let is_selected = idx == app.multi_diff.selected_index;
            let base_style = if is_selected {
                if app.file_list_focused {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                }
            } else {
                Style::default()
            };

            // Truncate filename to fit
            let max_name_len = area.width.saturating_sub(12) as usize;
            let name = if file.display_name.len() > max_name_len {
                format!("…{}", &file.display_name[file.display_name.len().saturating_sub(max_name_len - 1)..])
            } else {
                file.display_name.clone()
            };

            let stats = format!("+{} -{}", file.insertions, file.deletions);

            let line = Line::from(vec![
                Span::styled(format!("{} ", status_icon), status_style),
                Span::styled(format!("{:<width$}", name, width = max_name_len), base_style),
                Span::styled(format!(" {:>8}", stats), Style::default().fg(Color::DarkGray)),
            ]);

            ListItem::new(line)
        })
        .collect();

    let border_style = if app.file_list_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let file_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(border_style),
        );

    frame.render_widget(file_list, area);
}

fn draw_diff_view(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.view_mode {
        ViewMode::SinglePane => render_single_pane(frame, app, area),
        ViewMode::SideBySide => render_side_by_side(frame, app, area),
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
    let text = Paragraph::new(label)
        .style(Style::default().fg(Color::White));

    frame.render_widget(text, progress_area);
}

fn draw_help_popover(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Calculate popover size and position (centered)
    let popup_width = 44u16.min(area.width.saturating_sub(4));
    let base_height = if app.is_multi_file() { 30 } else { 25 };
    let popup_height = (base_height as u16).min(area.height.saturating_sub(4));
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area behind the popup
    frame.render_widget(Clear, popup_area);

    let key_style = Style::default().fg(Color::Yellow);
    let label_style = Style::default().fg(Color::White);
    let dim_style = Style::default().fg(Color::DarkGray);
    let section_style = Style::default().fg(Color::Cyan);

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
        help_line("< / >", "First/last step".into()),
        help_line("g / G", "Go to start/end".into()),
        help_line("J / K", "Scroll up/down".into()),
        help_line("H / L", "Scroll left/right".into()),
        help_line("0 / $", "Scroll to line start/end".into()),
        help_line("^U / ^D", "Scroll half-page".into()),
        help_line("^G", "Show full file path".into()),
        help_line("z", "Center on active".into()),
        help_line("w", "Toggle line wrap".into()),
        Line::from(""),
        Line::from(Span::styled(" Playback", section_style)),
        help_line("Space", "Toggle autoplay".into()),
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

    let help_block = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .title_alignment(Alignment::Center)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(help_block, popup_area);
}

/// Get the animation color modifier based on current animation state
#[allow(dead_code)]
pub fn get_animation_style(app: &App, is_old_content: bool) -> Style {
    match app.animation_phase {
        AnimationPhase::Idle => Style::default(),
        AnimationPhase::FadeOut => {
            if is_old_content {
                // Fading out old content
                let intensity = ((1.0 - app.animation_progress) * 255.0) as u8;
                Style::default().fg(Color::Rgb(intensity, intensity / 3, intensity / 3))
            } else {
                Style::default().fg(Color::DarkGray)
            }
        }
        AnimationPhase::FadeIn => {
            if !is_old_content {
                // Fading in new content
                let intensity = (app.animation_progress * 255.0) as u8;
                Style::default().fg(Color::Rgb(intensity / 3, intensity, intensity / 3))
            } else {
                Style::default().fg(Color::DarkGray)
            }
        }
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
        format!("…{}", &file_path[file_path.len().saturating_sub(max_path_len - 1)..])
    } else {
        file_path
    };

    let path_block = Paragraph::new(display_path)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" File Path ")
                .title_alignment(Alignment::Center)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Center);

    frame.render_widget(path_block, popup_area);
}
