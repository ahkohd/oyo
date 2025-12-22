//! Evolution view - shows file morphing without deletion markers
//! Deleted lines simply disappear, showing the file as it evolves

use super::render_empty_state;
use crate::app::{AnimationPhase, App};
use crate::syntax::SyntaxSide;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};
use oyo_core::{LineKind, ViewSpanKind};

/// Width of the fixed line number gutter (marker + line num + space + blank sign + space)
const GUTTER_WIDTH: u16 = 8; // "▶1234   " (matches single-pane width)

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
    let step_direction = app.multi_diff.current_step_direction();
    let (display_len, _) = crate::app::display_metrics(
        &view_lines,
        app.view_mode,
        app.animation_phase,
        app.scroll_offset,
        step_direction,
    );
    app.clamp_scroll(display_len, visible_height, app.allow_overscroll());

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
            LineKind::Context => Style::default().fg(app.theme.diff_line_number),
            LineKind::Inserted | LineKind::PendingInsert => {
                // Use insert gradient base color for line numbers
                let rgb = crate::color::gradient_color(&app.theme.insert, 0.5);
                Style::default().fg(Color::Rgb(rgb.r, rgb.g, rgb.b))
            }
            LineKind::Modified | LineKind::PendingModify => {
                // Use modify gradient base color for line numbers
                let rgb = crate::color::gradient_color(&app.theme.modify, 0.5);
                Style::default().fg(Color::Rgb(rgb.r, rgb.g, rgb.b))
            }
            LineKind::PendingDelete => {
                // Fade the line number too during animation
                if view_line.is_active && app.animation_phase != AnimationPhase::Idle {
                    let t = crate::color::animation_t(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                    );
                    let rgb = crate::color::gradient_color(&app.theme.delete, t);
                    Style::default().fg(Color::Rgb(rgb.r, rgb.g, rgb.b))
                } else {
                    // Use delete gradient base color
                    let rgb = crate::color::gradient_color(&app.theme.delete, 0.5);
                    Style::default().fg(Color::Rgb(rgb.r, rgb.g, rgb.b))
                }
            }
            LineKind::Deleted => Style::default().fg(app.theme.text_muted),
        };

        // Gutter marker: primary marker for focus, extent marker for hunk nav, blank otherwise
        let (active_marker, active_style) = if view_line.is_primary_active {
            (primary_marker.as_str(), Style::default().fg(app.theme.primary).add_modifier(Modifier::BOLD))
        } else if view_line.show_hunk_extent {
            (extent_marker.as_str(), Style::default().fg(app.theme.diff_ext_marker))
        } else {
            (" ", Style::default())
        };

        // Build gutter line (fixed, no horizontal scroll)
        // Matches single-pane: marker(1) + line_num(4) + space(1) + blank_sign(1) + space(1) = 8
        let gutter_spans = vec![
            Span::styled(active_marker, active_style),
            Span::styled(line_num_str, line_num_style),
            Span::styled(" ", Style::default()),
            Span::styled(" ", Style::default()), // blank sign column (matches single-pane)
            Span::styled(" ", Style::default()),
        ];
        gutter_lines.push(Line::from(gutter_spans));

        // Build content line (scrollable)
        let mut content_spans: Vec<Span> = Vec::new();
        let mut used_syntax = false;
        let pure_context = matches!(view_line.kind, LineKind::Context)
            && !view_line.has_changes
            && !view_line.is_active
            && view_line
                .spans
                .iter()
                .all(|span| matches!(span.kind, ViewSpanKind::Equal));
        if app.syntax_enabled() && pure_context {
            let side = if view_line.new_line.is_some() {
                SyntaxSide::New
            } else {
                SyntaxSide::Old
            };
            let line_num = view_line.new_line.or(view_line.old_line);
            if let Some(spans) = app.syntax_spans_for_line(side, line_num) {
                content_spans = spans;
                used_syntax = true;
            }
        }
        if !used_syntax {
            for view_span in &view_line.spans {
                let style =
                    get_evolution_span_style(view_span.kind, view_line.kind, view_line.is_active, app);
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

        // Render scrollbar (if enabled)
        if app.scrollbar_visible {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));

            let visible_lines = content_area.height as usize;
            if display_len > visible_lines {
                let mut scrollbar_state =
                    ScrollbarState::new(display_len).position(app.scroll_offset);

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
    }
}

fn get_evolution_span_style(span_kind: ViewSpanKind, line_kind: LineKind, is_active: bool, app: &App) -> Style {
    let theme = &app.theme;
    // Check if this is a modification line - use modify gradient instead of insert
    let is_modification = matches!(line_kind, LineKind::Modified | LineKind::PendingModify);

    match span_kind {
        ViewSpanKind::Equal => Style::default().fg(theme.diff_context),
        ViewSpanKind::Inserted => {
            if is_modification {
                // Modified content: use modify gradient
                super::modify_style(
                    AnimationPhase::Idle,
                    0.0,
                    false,
                    theme.modify_base(),
                    theme.diff_context,
                )
            } else {
                // Pure insertion: use insert colors
                super::insert_style(
                    AnimationPhase::Idle,
                    0.0,
                    false,
                    theme.insert_base(),
                    theme.diff_context,
                )
            }
        }
        ViewSpanKind::Deleted => {
            // In evolution view, deleted content is hidden
            Style::default().fg(theme.text_muted)
        }
        ViewSpanKind::PendingInsert => {
            if is_modification {
                if is_active {
                    super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        theme.modify_base(),
                        theme.diff_context,
                    )
                } else {
                    Style::default().fg(theme.modify_dim())
                }
            } else {
                if is_active {
                    super::insert_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        theme.insert_base(),
                        theme.diff_context,
                    )
                } else {
                    Style::default().fg(theme.insert_dim())
                }
            }
        }
        ViewSpanKind::PendingDelete => {
            if is_active {
                if is_modification {
                    super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        theme.modify_base(),
                        theme.diff_context,
                    )
                } else {
                    super::delete_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        app.strikethrough_deletions,
                        theme.delete_base(),
                        theme.diff_context,
                    )
                }
            } else {
                Style::default().fg(theme.text_muted)
            }
        }
    }
}
