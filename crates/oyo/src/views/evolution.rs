//! Evolution view - shows file morphing without deletion markers
//! Deleted lines simply disappear, showing the file as it evolves

use crate::app::{AnimationPhase, App};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};
use oyo_core::{LineKind, ViewSpanKind};

/// Width of the fixed line number gutter (marker + line num + space)
const GUTTER_WIDTH: u16 = 7; // "▶ 1234 "

/// Render the evolution view - file morphing without deletion markers
pub fn render_evolution(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height as usize;
    let visible_width = area.width.saturating_sub(GUTTER_WIDTH) as usize;

    // Clone markers to avoid borrow conflicts
    let primary_marker = app.primary_marker.clone();
    let extent_marker = app.extent_marker.clone();

    app.ensure_active_visible_if_needed(visible_height);
    let animation_frame = app.animation_frame();
    let view_lines = app.multi_diff.current_navigator().current_view_with_frame(animation_frame);
    app.clamp_scroll(view_lines.len(), visible_height, app.allow_overscroll());

    // Split area into gutter (fixed) and content (scrollable)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(GUTTER_WIDTH),
            Constraint::Min(0),
        ])
        .split(area);

    let gutter_area = chunks[0];
    let content_area = chunks[1];

    // Build separate gutter and content lines - skip deleted lines entirely
    let mut gutter_lines: Vec<Line> = Vec::new();
    let mut content_lines: Vec<Line> = Vec::new();
    let mut display_line_num = 0usize;
    let mut max_line_width: usize = 0;

    for view_line in view_lines.iter() {
        // Skip lines that are deleted or pending delete (they disappear in evolution view)
        match view_line.kind {
            LineKind::Deleted => continue,
            LineKind::PendingDelete => {
                // Show pending deletes with fade animation, then they disappear
                if !view_line.is_active {
                    continue;
                }
                // Active pending delete: show during animation, hide when idle
                if app.animation_phase == AnimationPhase::Idle {
                    continue;
                }
                // Show it fading out during both phases
            }
            _ => {}
        }

        display_line_num += 1;

        // Handle scrolling - when wrapping, we need all lines
        if !app.line_wrap && display_line_num <= app.scroll_offset {
            continue;
        }
        if !app.line_wrap && gutter_lines.len() >= visible_height {
            break;
        }

        let line_num = view_line.new_line.or(view_line.old_line).unwrap_or(0);
        let line_num_str = format!("{:4}", line_num);

        // In evolution mode, use subtle line number coloring based on type
        let line_num_style = match view_line.kind {
            LineKind::Context => Style::default().fg(Color::DarkGray),
            LineKind::Inserted | LineKind::PendingInsert => Style::default().fg(Color::Green),
            LineKind::Modified | LineKind::PendingModify => Style::default().fg(Color::Yellow),
            LineKind::PendingDelete => {
                // Fade the line number too during animation
                if view_line.is_active && app.animation_phase != AnimationPhase::Idle {
                    match app.animation_phase {
                        AnimationPhase::FadeOut => {
                            let progress = app.animation_progress;
                            let g = ((1.0 - progress * 0.5) * 255.0) as u8;
                            Style::default().fg(Color::Rgb(255, g / 4, g / 4))
                        }
                        AnimationPhase::FadeIn => {
                            let progress = app.animation_progress;
                            let intensity = ((1.0 - progress) * 127.0) as u8;
                            Style::default().fg(Color::Rgb(intensity, intensity / 8, intensity / 8))
                        }
                        AnimationPhase::Idle => Style::default().fg(Color::Red),
                    }
                } else {
                    Style::default().fg(Color::Red)
                }
            }
            LineKind::Deleted => Style::default().fg(Color::DarkGray),
        };

        // Gutter marker: primary marker for focus, extent marker for hunk, blank otherwise
        // Add trailing space to maintain consistent gutter width
        let (active_marker, active_style) = if view_line.is_primary_active {
            (format!("{} ", primary_marker), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        } else if view_line.is_active {
            (format!("{} ", extent_marker), Style::default().fg(Color::DarkGray))
        } else {
            ("  ".to_string(), Style::default())
        };

        // Build gutter line (fixed, no horizontal scroll)
        let gutter_spans = vec![
            Span::styled(active_marker, active_style),
            Span::styled(line_num_str, line_num_style),
            Span::styled(" ", Style::default()),
        ];
        gutter_lines.push(Line::from(gutter_spans));

        // Build content line (scrollable)
        let mut content_spans: Vec<Span> = Vec::new();
        for view_span in &view_line.spans {
            let style = get_evolution_span_style(view_span.kind, view_line.kind, view_line.is_active, app);
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

    // Render scrollbar (if enabled)
    if app.scrollbar_visible {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        // Calculate total displayable lines (excluding deleted)
        let total_displayable = view_lines
            .iter()
            .filter(|l| !matches!(l.kind, LineKind::Deleted | LineKind::PendingDelete))
            .count();

        let mut scrollbar_state =
            ScrollbarState::new(total_displayable).position(app.scroll_offset);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn get_evolution_span_style(span_kind: ViewSpanKind, line_kind: LineKind, is_active: bool, app: &App) -> Style {
    // Check if this is a modification line - use yellow instead of green
    let is_modification = matches!(line_kind, LineKind::Modified | LineKind::PendingModify);

    match span_kind {
        ViewSpanKind::Equal => Style::default().fg(Color::White),
        ViewSpanKind::Inserted => {
            if is_modification {
                // Modified content shows as yellow
                if is_active {
                    get_modify_animation_style(app)
                } else {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                }
            } else {
                // Pure insertion shows as green
                if is_active {
                    get_insert_animation_style(app)
                } else {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                }
            }
        }
        ViewSpanKind::Deleted => {
            // In evolution view, deleted content is hidden
            Style::default().fg(Color::DarkGray)
        }
        ViewSpanKind::PendingInsert => {
            if is_modification {
                if is_active {
                    get_pending_modify_style(app)
                } else {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM)
                }
            } else {
                if is_active {
                    get_pending_insert_style(app)
                } else {
                    Style::default().fg(Color::Green).add_modifier(Modifier::DIM)
                }
            }
        }
        ViewSpanKind::PendingDelete => {
            // Show fading out during animation
            if is_active {
                get_pending_delete_style(app)
            } else {
                Style::default().fg(Color::DarkGray)
            }
        }
    }
}

fn get_insert_animation_style(app: &App) -> Style {
    match app.animation_phase {
        AnimationPhase::FadeOut => Style::default()
            .fg(Color::Rgb(30, 80, 30))
            .add_modifier(Modifier::DIM),
        AnimationPhase::FadeIn => {
            let intensity = (app.animation_progress * 200.0) as u8 + 55;
            Style::default()
                .fg(Color::Rgb(intensity / 3, intensity, intensity / 3))
                .add_modifier(Modifier::BOLD)
        }
        AnimationPhase::Idle => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    }
}

fn get_pending_delete_style(app: &App) -> Style {
    if app.is_backward_animation() {
        // Backward in evolution: line reappears (fade in from nothing)
        match app.animation_phase {
            AnimationPhase::FadeOut => {
                // Start dim/dark
                let progress = app.animation_progress;
                let intensity = (progress * 0.5 * 255.0) as u8;
                Style::default()
                    .fg(Color::Rgb(intensity, intensity, intensity))
                    .add_modifier(Modifier::DIM)
            }
            AnimationPhase::FadeIn => {
                // Fade in to normal
                let progress = app.animation_progress;
                let intensity = ((0.5 + progress * 0.5) * 255.0) as u8;
                Style::default().fg(Color::Rgb(intensity, intensity, intensity))
            }
            AnimationPhase::Idle => Style::default().fg(Color::White),
        }
    } else {
        // Forward: line fades out and disappears
        match app.animation_phase {
            AnimationPhase::FadeOut => {
                let progress = app.animation_progress;
                let r = 255;
                let gb = ((1.0 - progress * 0.5) * 255.0) as u8;
                Style::default().fg(Color::Rgb(r, gb, gb))
            }
            AnimationPhase::FadeIn => {
                let progress = app.animation_progress;
                let intensity = ((1.0 - progress) * 0.5 + 0.0) * 255.0;
                let r = (intensity * 2.0).min(255.0) as u8;
                let g = (intensity * 0.2) as u8;
                let b = (intensity * 0.2) as u8;

                let mut style = Style::default().fg(Color::Rgb(r, g, b));
                if progress > 0.2 {
                    style = style.add_modifier(Modifier::CROSSED_OUT);
                }
                if progress > 0.5 {
                    style = style.add_modifier(Modifier::DIM);
                }
                style
            }
            AnimationPhase::Idle => Style::default().fg(Color::DarkGray),
        }
    }
}

fn get_pending_insert_style(app: &App) -> Style {
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
            AnimationPhase::Idle => Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        }
    } else {
        // Forward: fade in (line appears)
        match app.animation_phase {
            AnimationPhase::FadeOut => Style::default()
                .fg(Color::Rgb(30, 60, 30))
                .add_modifier(Modifier::DIM),
            AnimationPhase::FadeIn => {
                let intensity = (app.animation_progress * 200.0) as u8 + 55;
                Style::default()
                    .fg(Color::Rgb(intensity / 3, intensity, intensity / 3))
                    .add_modifier(Modifier::BOLD)
            }
            AnimationPhase::Idle => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        }
    }
}

fn get_modify_animation_style(app: &App) -> Style {
    match app.animation_phase {
        AnimationPhase::FadeOut => Style::default()
            .fg(Color::Rgb(80, 80, 30))
            .add_modifier(Modifier::DIM),
        AnimationPhase::FadeIn => {
            let intensity = (app.animation_progress * 200.0) as u8 + 55;
            Style::default()
                .fg(Color::Rgb(intensity, intensity, intensity / 3))
                .add_modifier(Modifier::BOLD)
        }
        AnimationPhase::Idle => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    }
}

fn get_pending_modify_style(app: &App) -> Style {
    match app.animation_phase {
        AnimationPhase::FadeOut => Style::default()
            .fg(Color::Rgb(60, 60, 30))
            .add_modifier(Modifier::DIM),
        AnimationPhase::FadeIn => {
            let intensity = (app.animation_progress * 200.0) as u8 + 55;
            Style::default()
                .fg(Color::Rgb(intensity, intensity, intensity / 3))
                .add_modifier(Modifier::BOLD)
        }
        AnimationPhase::Idle => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    }
}
