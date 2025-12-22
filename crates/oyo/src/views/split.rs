//! Split view with synchronized stepping

use super::render_empty_state;
use crate::app::{AnimationPhase, App};
use crate::syntax::SyntaxSide;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use oyo_core::{LineKind, ViewSpanKind};

/// Width of the fixed line number gutter
const GUTTER_WIDTH: u16 = 6; // "▶1234 " or " 1234 "

/// Render the split view
pub fn render_split(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height as usize;
    app.ensure_active_visible_if_needed(visible_height);
    let animation_frame = app.animation_frame();
    let view_lines = app.multi_diff.current_navigator().current_view_with_frame(animation_frame);
    let step_direction = app.multi_diff.current_step_direction();
    let (display_len, _) = crate::app::display_metrics(
        &view_lines,
        app.view_mode,
        app.animation_phase,
        app.scroll_offset,
        step_direction,
    );
    app.clamp_scroll(display_len, visible_height, app.allow_overscroll());

    // Split into two panes
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_old_pane(frame, app, chunks[0]);
    render_new_pane(frame, app, chunks[1]);
}

fn render_old_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    // Clone markers to avoid borrow conflicts
    let primary_marker = app.primary_marker.clone();
    let extent_marker = app.extent_marker.clone();

    let animation_frame = app.animation_frame();
    let view_lines = app.multi_diff.current_navigator().current_view_with_frame(animation_frame);
    let visible_height = area.height as usize;
    let visible_width = area.width.saturating_sub(GUTTER_WIDTH + 1) as usize; // +1 for border

    // Split into gutter (fixed) and content (scrollable), plus border
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(GUTTER_WIDTH),
            Constraint::Min(0),
            Constraint::Length(1), // For border
        ])
        .split(area);

    let gutter_area = chunks[0];
    let content_area = chunks[1];
    let border_area = chunks[2];

    let mut gutter_lines: Vec<Line> = Vec::new();
    let mut content_lines: Vec<Line> = Vec::new();
    let mut line_idx = 0;
    let mut max_line_width: usize = 0;

    for view_line in view_lines.iter() {
        if let Some(old_line_num) = view_line.old_line {
            // When wrapping, we need all lines
            if !app.line_wrap && line_idx < app.scroll_offset {
                line_idx += 1;
                continue;
            }
            if !app.line_wrap && gutter_lines.len() >= visible_height {
                break;
            }

            let line_num_str = format!("{:4}", old_line_num);

            // Gutter marker: primary marker for focus, extent marker for hunk nav, blank otherwise
            let (active_marker, active_style) = if view_line.is_primary_active {
                (primary_marker.as_str(), Style::default().fg(app.theme.primary).add_modifier(Modifier::BOLD))
            } else if view_line.show_hunk_extent {
                (extent_marker.as_str(), Style::default().fg(app.theme.diff_ext_marker))
            } else {
                (" ", Style::default())
            };

            // Build gutter line
            let gutter_spans = vec![
                Span::styled(active_marker, active_style),
                Span::styled(line_num_str, Style::default().fg(app.theme.diff_line_number)),
                Span::styled(" ", Style::default()),
            ];
            gutter_lines.push(Line::from(gutter_spans));

            // Build content line
            let mut content_spans: Vec<Span> = Vec::new();
            let mut used_syntax = false;
            if app.syntax_enabled() && matches!(view_line.kind, LineKind::Context) {
                if let Some(spans) = app.syntax_spans_for_line(SyntaxSide::Old, Some(old_line_num))
                {
                    content_spans = spans;
                    used_syntax = true;
                }
            }
            if !used_syntax {
                for view_span in &view_line.spans {
                    let style =
                        get_old_span_style(view_span.kind, view_line.kind, view_line.is_active, app);
                    // For deleted spans, don't strikethrough leading whitespace
                    if app.strikethrough_deletions
                        && matches!(
                            view_span.kind,
                            ViewSpanKind::Deleted | ViewSpanKind::PendingDelete
                        )
                    {
                        let text = &view_span.text;
                        let trimmed = text.trim_start();
                        let leading_ws_len = text.len() - trimmed.len();
                        if leading_ws_len > 0 && !trimmed.is_empty() {
                            let ws_style = style.remove_modifier(Modifier::CROSSED_OUT);
                            content_spans.push(Span::styled(
                                text[..leading_ws_len].to_string(),
                                ws_style,
                            ));
                            content_spans.push(Span::styled(trimmed.to_string(), style));
                        } else {
                            content_spans.push(Span::styled(view_span.text.clone(), style));
                        }
                    } else {
                        content_spans.push(Span::styled(view_span.text.clone(), style));
                    }
                }
            }

            // Track max line width
            let line_width: usize = content_spans.iter().map(|s| s.content.len()).sum();
            max_line_width = max_line_width.max(line_width);

            content_lines.push(Line::from(content_spans));
            line_idx += 1;
        }
    }

    // Clamp horizontal scroll
    app.clamp_horizontal_scroll(max_line_width, visible_width);

    // Background style (if set)
    let bg_style = app.theme.background.map(|bg| Style::default().bg(bg));

    // Render gutter (no horizontal scroll)
    let mut gutter_paragraph = if app.line_wrap {
        Paragraph::new(gutter_lines).scroll((app.scroll_offset as u16, 0))
    } else {
        Paragraph::new(gutter_lines)
    };
    if let Some(style) = bg_style {
        gutter_paragraph = gutter_paragraph.style(style);
    }
    frame.render_widget(gutter_paragraph, gutter_area);

    // Render content with horizontal scroll (or empty state)
    if content_lines.is_empty() {
        let has_changes = !app.multi_diff.current_navigator().diff().significant_changes.is_empty();
        render_empty_state(frame, content_area, &app.theme, has_changes);
    } else {
        let mut content_paragraph = if app.line_wrap {
            Paragraph::new(content_lines)
                .wrap(Wrap { trim: false })
                .scroll((app.scroll_offset as u16, 0))
        } else {
            Paragraph::new(content_lines)
                .scroll((0, app.horizontal_scroll as u16))
        };
        if let Some(style) = bg_style {
            content_paragraph = content_paragraph.style(style);
        }
        frame.render_widget(content_paragraph, content_area);
    }

    // Render border
    let mut border = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(app.theme.border_subtle));
    if let Some(style) = bg_style {
        border = border.style(style);
    }
    frame.render_widget(border, border_area);
}

fn render_new_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    // Clone markers to avoid borrow conflicts
    let primary_marker_right = app.primary_marker_right.clone();
    let extent_marker_right = app.extent_marker_right.clone();

    let animation_frame = app.animation_frame();
    let view_lines = app.multi_diff.current_navigator().current_view_with_frame(animation_frame);
    let visible_height = area.height as usize;

    // Split into gutter (fixed) and content (scrollable)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(5), // "1234 "
            Constraint::Min(0),
            Constraint::Length(1), // For active marker
        ])
        .split(area);

    let gutter_area = chunks[0];
    let content_area = chunks[1];
    let marker_area = chunks[2];

    let mut gutter_lines: Vec<Line> = Vec::new();
    let mut content_lines: Vec<Line> = Vec::new();
    let mut marker_lines: Vec<Line> = Vec::new();
    let mut line_idx = 0;

    for view_line in view_lines.iter() {
        if let Some(new_line_num) = view_line.new_line {
            // Skip lines that represent deletions (they don't exist in new file)
            if matches!(view_line.kind, LineKind::Deleted | LineKind::PendingDelete) {
                continue;
            }

            // When wrapping, we need all lines
            if !app.line_wrap && line_idx < app.scroll_offset {
                line_idx += 1;
                continue;
            }
            if !app.line_wrap && gutter_lines.len() >= visible_height {
                break;
            }

            let line_num_str = format!("{:4}", new_line_num);

            // Gutter marker: right-pane primary marker for focus, extent marker for hunk nav, blank otherwise
            let (active_marker, active_style) = if view_line.is_primary_active {
                (primary_marker_right.as_str(), Style::default().fg(app.theme.primary).add_modifier(Modifier::BOLD))
            } else if view_line.show_hunk_extent {
                (extent_marker_right.as_str(), Style::default().fg(app.theme.diff_ext_marker))
            } else {
                (" ", Style::default())
            };

            // Build gutter line
            let gutter_spans = vec![
                Span::styled(line_num_str, Style::default().fg(app.theme.diff_line_number)),
                Span::styled(" ", Style::default()),
            ];
            gutter_lines.push(Line::from(gutter_spans));

            // Build content line
            let mut content_spans: Vec<Span> = Vec::new();
            let mut used_syntax = false;
            if app.syntax_enabled() && matches!(view_line.kind, LineKind::Context) {
                if let Some(spans) = app.syntax_spans_for_line(SyntaxSide::New, Some(new_line_num))
                {
                    content_spans = spans;
                    used_syntax = true;
                }
            }
            if !used_syntax {
                for view_span in &view_line.spans {
                    let style =
                        get_new_span_style(view_span.kind, view_line.kind, view_line.is_active, app);
                    content_spans.push(Span::styled(view_span.text.clone(), style));
                }
            }
            content_lines.push(Line::from(content_spans));

            // Build marker line
            marker_lines.push(Line::from(Span::styled(active_marker, active_style)));

            line_idx += 1;
        }
    }

    // Background style (if set)
    let bg_style = app.theme.background.map(|bg| Style::default().bg(bg));

    // Render gutter (no horizontal scroll)
    let mut gutter_paragraph = if app.line_wrap {
        Paragraph::new(gutter_lines).scroll((app.scroll_offset as u16, 0))
    } else {
        Paragraph::new(gutter_lines)
    };
    if let Some(style) = bg_style {
        gutter_paragraph = gutter_paragraph.style(style);
    }
    frame.render_widget(gutter_paragraph, gutter_area);

    // Render content with horizontal scroll (or empty state)
    if content_lines.is_empty() {
        let has_changes = !app.multi_diff.current_navigator().diff().significant_changes.is_empty();
        render_empty_state(frame, content_area, &app.theme, has_changes);
    } else {
        let mut content_paragraph = if app.line_wrap {
            Paragraph::new(content_lines)
                .wrap(Wrap { trim: false })
                .scroll((app.scroll_offset as u16, 0))
        } else {
            Paragraph::new(content_lines)
                .scroll((0, app.horizontal_scroll as u16))
        };
        if let Some(style) = bg_style {
            content_paragraph = content_paragraph.style(style);
        }
        frame.render_widget(content_paragraph, content_area);
    }

    // Render marker (no horizontal scroll)
    let mut marker_paragraph = if app.line_wrap {
        Paragraph::new(marker_lines).scroll((app.scroll_offset as u16, 0))
    } else {
        Paragraph::new(marker_lines)
    };
    if let Some(style) = bg_style {
        marker_paragraph = marker_paragraph.style(style);
    }
    frame.render_widget(marker_paragraph, marker_area);
}

fn get_old_span_style(kind: ViewSpanKind, line_kind: LineKind, is_active: bool, app: &App) -> Style {
    let theme = &app.theme;
    let is_modification = matches!(line_kind, LineKind::Modified | LineKind::PendingModify);
    match kind {
        ViewSpanKind::Equal => Style::default().fg(theme.diff_context),
        ViewSpanKind::Deleted => {
            if is_modification {
                if is_active {
                    return super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        theme.modify_base(),
                        theme.diff_context,
                    );
                }
                return Style::default().fg(theme.modify_base());
            }
            // Completed deletion: base color with optional strikethrough
            let mut style = super::delete_style(
                AnimationPhase::Idle,
                0.0,
                false,
                false,
                theme.delete_base(),
                theme.diff_context,
            );
            if app.strikethrough_deletions {
                style = style.add_modifier(Modifier::CROSSED_OUT);
            }
            style
        }
        ViewSpanKind::Inserted => {
            // In old pane, inserted content shouldn't appear
            Style::default().fg(theme.text_muted)
        }
        ViewSpanKind::PendingDelete => {
            if is_modification {
                if is_active {
                    return super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        theme.modify_base(),
                        theme.diff_context,
                    );
                }
                return Style::default().fg(theme.modify_dim());
            }
            if is_active {
                super::delete_style(
                    app.animation_phase,
                    app.animation_progress,
                    app.is_backward_animation(),
                    app.strikethrough_deletions,
                    theme.delete_base(),
                    theme.diff_context,
                )
            } else {
                // Non-active pending delete: show as completed
                let mut style = super::delete_style(
                    AnimationPhase::Idle,
                    0.0,
                    false,
                    false,
                    theme.delete_base(),
                    theme.diff_context,
                );
                if app.strikethrough_deletions {
                    style = style.add_modifier(Modifier::CROSSED_OUT);
                }
                style
            }
        }
        ViewSpanKind::PendingInsert => {
            Style::default().fg(theme.text_muted).add_modifier(Modifier::DIM)
        }
    }
}

fn get_new_span_style(kind: ViewSpanKind, line_kind: LineKind, is_active: bool, app: &App) -> Style {
    let theme = &app.theme;
    let is_modification = matches!(line_kind, LineKind::Modified | LineKind::PendingModify);
    match kind {
        ViewSpanKind::Equal => Style::default().fg(theme.diff_context),
        ViewSpanKind::Inserted => {
            if is_modification {
                if is_active {
                    return super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        theme.modify_base(),
                        theme.diff_context,
                    );
                }
                return Style::default().fg(theme.modify_base());
            }
            // Completed insertion: base color
            super::insert_style(
                AnimationPhase::Idle,
                0.0,
                false,
                theme.insert_base(),
                theme.diff_context,
            )
        }
        ViewSpanKind::Deleted => {
            // In new pane, deleted content shouldn't appear
            Style::default().fg(theme.text_muted)
        }
        ViewSpanKind::PendingInsert => {
            if is_modification {
                if is_active {
                    return super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        theme.modify_base(),
                        theme.diff_context,
                    );
                }
                return Style::default().fg(theme.modify_dim());
            }
            if is_active {
                super::insert_style(
                    app.animation_phase,
                    app.animation_progress,
                    app.is_backward_animation(),
                    theme.insert_base(),
                    theme.diff_context,
                )
            } else {
                // Non-active pending insert: show dim
                Style::default().fg(theme.insert_dim())
            }
        }
        ViewSpanKind::PendingDelete => {
            Style::default().fg(theme.delete_dim())
        }
    }
}
