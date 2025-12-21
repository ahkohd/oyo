//! Side-by-side view with synchronized stepping

use crate::app::{AnimationPhase, App};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use oyo_core::{LineKind, ViewSpanKind};

/// Width of the fixed line number gutter
const GUTTER_WIDTH: u16 = 6; // "▶1234 " or " 1234 "

/// Render the side-by-side view
pub fn render_side_by_side(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height as usize;
    app.ensure_active_visible_if_needed(visible_height);
    let animation_frame = app.animation_frame();
    let total_lines = app.multi_diff.current_navigator().current_view_with_frame(animation_frame).len();
    app.clamp_scroll(total_lines, visible_height, app.allow_overscroll());

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

            // Gutter marker: primary marker for focus, extent marker for hunk, blank otherwise
            let (active_marker, active_style) = if view_line.is_primary_active {
                (primary_marker.as_str(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            } else if view_line.is_active {
                (extent_marker.as_str(), Style::default().fg(Color::DarkGray))
            } else {
                (" ", Style::default())
            };

            // Build gutter line
            let gutter_spans = vec![
                Span::styled(active_marker, active_style),
                Span::styled(line_num_str, Style::default().fg(Color::DarkGray)),
                Span::styled(" ", Style::default()),
            ];
            gutter_lines.push(Line::from(gutter_spans));

            // Build content line
            let mut content_spans: Vec<Span> = Vec::new();
            for view_span in &view_line.spans {
                let style = get_old_span_style(view_span.kind, view_line.is_active, app);
                // For deleted spans, don't strikethrough leading whitespace
                if matches!(
                    view_span.kind,
                    ViewSpanKind::Deleted | ViewSpanKind::PendingDelete
                ) {
                    let text = &view_span.text;
                    let trimmed = text.trim_start();
                    let leading_ws_len = text.len() - trimmed.len();
                    if leading_ws_len > 0 && !trimmed.is_empty() {
                        let ws_style = style.remove_modifier(Modifier::CROSSED_OUT);
                        content_spans.push(Span::styled(text[..leading_ws_len].to_string(), ws_style));
                        content_spans.push(Span::styled(trimmed.to_string(), style));
                    } else {
                        content_spans.push(Span::styled(view_span.text.clone(), style));
                    }
                } else {
                    content_spans.push(Span::styled(view_span.text.clone(), style));
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

    // Render gutter (no horizontal scroll)
    let gutter_paragraph = if app.line_wrap {
        Paragraph::new(gutter_lines).scroll((app.scroll_offset as u16, 0))
    } else {
        Paragraph::new(gutter_lines)
    };
    frame.render_widget(gutter_paragraph, gutter_area);

    // Render content with horizontal scroll
    let content_paragraph = if app.line_wrap {
        Paragraph::new(content_lines)
            .wrap(Wrap { trim: false })
            .scroll((app.scroll_offset as u16, 0))
    } else {
        Paragraph::new(content_lines)
            .scroll((0, app.horizontal_scroll as u16))
    };
    frame.render_widget(content_paragraph, content_area);

    // Render border
    let border = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::DarkGray));
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

            // Gutter marker: right-pane primary marker for focus, extent marker for hunk, blank otherwise
            let (active_marker, active_style) = if view_line.is_primary_active {
                (primary_marker_right.as_str(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            } else if view_line.is_active {
                (extent_marker_right.as_str(), Style::default().fg(Color::DarkGray))
            } else {
                (" ", Style::default())
            };

            // Build gutter line
            let gutter_spans = vec![
                Span::styled(line_num_str, Style::default().fg(Color::DarkGray)),
                Span::styled(" ", Style::default()),
            ];
            gutter_lines.push(Line::from(gutter_spans));

            // Build content line
            let mut content_spans: Vec<Span> = Vec::new();
            for view_span in &view_line.spans {
                let style = get_new_span_style(view_span.kind, view_line.is_active, app);
                content_spans.push(Span::styled(view_span.text.clone(), style));
            }
            content_lines.push(Line::from(content_spans));

            // Build marker line
            marker_lines.push(Line::from(Span::styled(active_marker, active_style)));

            line_idx += 1;
        }
    }

    // Render gutter (no horizontal scroll)
    let gutter_paragraph = if app.line_wrap {
        Paragraph::new(gutter_lines).scroll((app.scroll_offset as u16, 0))
    } else {
        Paragraph::new(gutter_lines)
    };
    frame.render_widget(gutter_paragraph, gutter_area);

    // Render content with horizontal scroll
    let content_paragraph = if app.line_wrap {
        Paragraph::new(content_lines)
            .wrap(Wrap { trim: false })
            .scroll((app.scroll_offset as u16, 0))
    } else {
        Paragraph::new(content_lines)
            .scroll((0, app.horizontal_scroll as u16))
    };
    frame.render_widget(content_paragraph, content_area);

    // Render marker (no horizontal scroll)
    let marker_paragraph = if app.line_wrap {
        Paragraph::new(marker_lines).scroll((app.scroll_offset as u16, 0))
    } else {
        Paragraph::new(marker_lines)
    };
    frame.render_widget(marker_paragraph, marker_area);
}

fn get_old_span_style(kind: ViewSpanKind, is_active: bool, app: &App) -> Style {
    match kind {
        ViewSpanKind::Equal => Style::default().fg(Color::White),
        ViewSpanKind::Deleted => {
            Style::default().fg(Color::Red).add_modifier(Modifier::CROSSED_OUT)
        }
        ViewSpanKind::Inserted => {
            // In old pane, inserted content shouldn't appear
            Style::default().fg(Color::DarkGray)
        }
        ViewSpanKind::PendingDelete => {
            if is_active {
                if app.is_backward_animation() {
                    // Backward: restore from red to white
                    match app.animation_phase {
                        AnimationPhase::FadeOut => {
                            let progress = app.animation_progress;
                            let g = (progress * 0.5 * 255.0) as u8;
                            let b = (progress * 0.5 * 255.0) as u8;
                            let mut style = Style::default().fg(Color::Rgb(255, g, b));
                            if progress < 0.7 {
                                style = style.add_modifier(Modifier::CROSSED_OUT);
                            }
                            style
                        }
                        AnimationPhase::FadeIn => {
                            let progress = app.animation_progress;
                            let g = ((0.5 + progress * 0.5) * 255.0) as u8;
                            let b = ((0.5 + progress * 0.5) * 255.0) as u8;
                            Style::default().fg(Color::Rgb(255, g, b))
                        }
                        AnimationPhase::Idle => Style::default().fg(Color::White),
                    }
                } else {
                    // Forward: transition from white to red
                    match app.animation_phase {
                        AnimationPhase::FadeOut => {
                            let progress = app.animation_progress;
                            let r = 255;
                            let g = ((1.0 - progress * 0.5) * 255.0) as u8;
                            let b = ((1.0 - progress * 0.5) * 255.0) as u8;
                            Style::default().fg(Color::Rgb(r, g, b))
                        }
                        AnimationPhase::FadeIn => {
                            let progress = app.animation_progress;
                            let r = 255;
                            let g = ((0.5 - progress * 0.5) * 255.0) as u8;
                            let b = ((0.5 - progress * 0.5) * 255.0) as u8;
                            let mut style = Style::default().fg(Color::Rgb(r, g, b));
                            if progress > 0.3 {
                                style = style.add_modifier(Modifier::CROSSED_OUT);
                            }
                            style
                        }
                        AnimationPhase::Idle => {
                            Style::default()
                                .fg(Color::Red)
                                .add_modifier(Modifier::CROSSED_OUT)
                        }
                    }
                }
            } else {
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::CROSSED_OUT)
            }
        }
        ViewSpanKind::PendingInsert => {
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
        }
    }
}

fn get_new_span_style(kind: ViewSpanKind, is_active: bool, app: &App) -> Style {
    match kind {
        ViewSpanKind::Equal => Style::default().fg(Color::White),
        ViewSpanKind::Inserted => {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        }
        ViewSpanKind::Deleted => {
            // In new pane, deleted content shouldn't appear
            Style::default().fg(Color::DarkGray)
        }
        ViewSpanKind::PendingInsert => {
            if is_active {
                if app.is_backward_animation() {
                    // Backward: fade out (line will disappear)
                    match app.animation_phase {
                        AnimationPhase::FadeOut => {
                            let progress = app.animation_progress;
                            let intensity = ((1.0 - progress * 0.5) * 200.0) as u8 + 55;
                            Style::default()
                                .fg(Color::Rgb(intensity / 3, intensity, intensity / 3))
                                .add_modifier(Modifier::BOLD)
                        }
                        AnimationPhase::FadeIn => {
                            let progress = app.animation_progress;
                            let intensity = ((0.5 - progress * 0.5) * 200.0) as u8 + 30;
                            Style::default()
                                .fg(Color::Rgb(intensity / 3, intensity, intensity / 3))
                                .add_modifier(Modifier::DIM)
                        }
                        AnimationPhase::Idle => {
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
                        }
                    }
                } else {
                    // Forward: fade in (line appears)
                    match app.animation_phase {
                        AnimationPhase::FadeOut => {
                            Style::default()
                                .fg(Color::Rgb(30, 60, 30))
                                .add_modifier(Modifier::DIM)
                        }
                        AnimationPhase::FadeIn => {
                            let intensity = (app.animation_progress * 200.0) as u8 + 55;
                            Style::default()
                                .fg(Color::Rgb(intensity / 3, intensity, intensity / 3))
                                .add_modifier(Modifier::BOLD)
                        }
                        AnimationPhase::Idle => {
                            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                        }
                    }
                }
            } else {
                Style::default().fg(Color::Green).add_modifier(Modifier::DIM)
            }
        }
        ViewSpanKind::PendingDelete => {
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
        }
    }
}
