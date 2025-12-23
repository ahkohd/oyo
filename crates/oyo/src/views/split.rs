//! Split view with synchronized stepping

use super::{
    expand_tabs_in_spans, render_empty_state, spans_to_text, spans_width, truncate_text,
    wrap_count_for_spans, wrap_count_for_text, TAB_WIDTH,
};
use crate::app::{AnimationPhase, App};
use crate::syntax::SyntaxSide;
use oyo_core::{
    AnimationFrame, ChangeKind, LineKind, StepDirection, ViewLine, ViewSpan, ViewSpanKind,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

/// Width of the fixed line number gutter
const GUTTER_WIDTH: u16 = 6; // "▶1234 " or " 1234 "
const OLD_BORDER_WIDTH: u16 = 1;
const NEW_GUTTER_WIDTH: u16 = 5; // "1234 "
const NEW_MARKER_WIDTH: u16 = 1;

/// Render the split view
pub fn render_split(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height as usize;
    if app.line_wrap {
        app.handle_search_scroll_if_needed(visible_height);
    } else {
        app.ensure_active_visible_if_needed(visible_height);
    }
    let view_lines = app
        .multi_diff
        .current_navigator()
        .current_view_with_frame(AnimationFrame::Idle);
    let step_direction = app.multi_diff.current_step_direction();

    // Split into two panes
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    if app.line_wrap {
        let old_width = chunks[0]
            .width
            .saturating_sub(GUTTER_WIDTH + OLD_BORDER_WIDTH) as usize;
        let new_width = chunks[1]
            .width
            .saturating_sub(NEW_GUTTER_WIDTH + NEW_MARKER_WIDTH) as usize;
        let (display_len, active_idx) = split_wrap_display_metrics(
            app,
            &view_lines,
            old_width,
            new_width,
            app.scroll_offset,
            step_direction,
        );
        app.ensure_active_visible_if_needed_wrapped(visible_height, display_len, active_idx);
        app.clamp_scroll(display_len, visible_height, app.allow_overscroll());
    } else {
        let (display_len, _) = crate::app::display_metrics(
            &view_lines,
            app.view_mode,
            app.animation_phase,
            app.scroll_offset,
            step_direction,
        );
        app.clamp_scroll(display_len, visible_height, app.allow_overscroll());
    }

    render_old_pane(frame, app, chunks[0]);
    render_new_pane(frame, app, chunks[1]);
}

fn render_old_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    // Clone markers to avoid borrow conflicts
    let primary_marker = app.primary_marker.clone();
    let extent_marker = app.extent_marker.clone();

    let view_lines = app
        .multi_diff
        .current_navigator()
        .current_view_with_frame(AnimationFrame::Idle);
    let visible_height = area.height as usize;
    let visible_width = area.width.saturating_sub(GUTTER_WIDTH + 1) as usize; // +1 for border
    let debug_target = app.syntax_scope_target(&view_lines);

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
    let query = app.search_query().trim().to_ascii_lowercase();
    let has_query = !query.is_empty();
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
                (
                    primary_marker.as_str(),
                    Style::default()
                        .fg(app.theme.primary)
                        .add_modifier(Modifier::BOLD),
                )
            } else if view_line.show_hunk_extent {
                (
                    extent_marker.as_str(),
                    Style::default().fg(app.theme.diff_ext_marker),
                )
            } else {
                (" ", Style::default())
            };

            // Build gutter line
            let gutter_spans = vec![
                Span::styled(active_marker, active_style),
                Span::styled(
                    line_num_str,
                    Style::default().fg(app.theme.diff_line_number),
                ),
                Span::styled(" ", Style::default()),
            ];
            gutter_lines.push(Line::from(gutter_spans));

            let display_idx = line_idx;
            // Build content line
            let mut content_spans: Vec<Span<'static>> = Vec::new();
            let mut used_syntax = false;
            let pure_context = matches!(view_line.kind, LineKind::Context)
                && !view_line.has_changes
                && !view_line.is_active
                && view_line
                    .spans
                    .iter()
                    .all(|span| matches!(span.kind, ViewSpanKind::Equal));
            if app.syntax_enabled() && pure_context {
                if let Some(spans) = app.syntax_spans_for_line(SyntaxSide::Old, Some(old_line_num))
                {
                    content_spans = spans;
                    used_syntax = true;
                }
            }
            if !used_syntax {
                let mut rebuilt_spans: Vec<ViewSpan> = Vec::new();
                let spans =
                    if matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify) {
                        if let Some(change) = app
                            .multi_diff
                            .current_navigator()
                            .diff()
                            .changes
                            .get(view_line.change_id)
                        {
                            for span in &change.spans {
                                match span.kind {
                                    ChangeKind::Equal => rebuilt_spans.push(ViewSpan {
                                        text: span.text.clone(),
                                        kind: ViewSpanKind::Equal,
                                    }),
                                    ChangeKind::Delete | ChangeKind::Replace => {
                                        rebuilt_spans.push(ViewSpan {
                                            text: span.text.clone(),
                                            kind: ViewSpanKind::Deleted,
                                        });
                                    }
                                    ChangeKind::Insert => {}
                                }
                            }
                        }
                        if rebuilt_spans.is_empty() {
                            &view_line.spans
                        } else {
                            &rebuilt_spans
                        }
                    } else {
                        &view_line.spans
                    };

                for view_span in spans {
                    let style = get_old_span_style(
                        view_span.kind,
                        view_line.kind,
                        view_line.is_active,
                        app,
                    );
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
                            content_spans
                                .push(Span::styled(text[..leading_ws_len].to_string(), ws_style));
                            content_spans.push(Span::styled(trimmed.to_string(), style));
                        } else {
                            content_spans.push(Span::styled(view_span.text.clone(), style));
                        }
                    } else {
                        content_spans.push(Span::styled(view_span.text.clone(), style));
                    }
                }
            }

            let line_text = spans_to_text(&content_spans);
            let is_active_match = app.search_target() == Some(display_idx)
                && has_query
                && line_text.to_ascii_lowercase().contains(&query);
            content_spans = app.highlight_search_spans(content_spans, &line_text, is_active_match);

            if app.line_wrap {
                content_spans = expand_tabs_in_spans(&content_spans, TAB_WIDTH);
            }

            let line_width = spans_width(&content_spans);
            max_line_width = max_line_width.max(line_width);

            let wrap_count = if app.line_wrap {
                wrap_count_for_spans(&content_spans, visible_width)
            } else {
                1
            };
            content_lines.push(Line::from(content_spans));
            if app.line_wrap {
                if wrap_count > 1 {
                    for _ in 1..wrap_count {
                        gutter_lines.push(Line::from(Span::raw(" ")));
                    }
                }
            }
            line_idx += 1;

            if let Some((debug_idx, _)) = debug_target {
                if debug_idx == display_idx {
                    let debug_wrap = if app.line_wrap {
                        wrap_count_for_text("", visible_width)
                    } else {
                        1
                    };
                    gutter_lines.push(Line::from(Span::raw(" ")));
                    content_lines.push(Line::from(Span::raw("")));
                    if app.line_wrap {
                        if debug_wrap > 1 {
                            for _ in 1..debug_wrap {
                                gutter_lines.push(Line::from(Span::raw(" ")));
                            }
                        }
                    }
                }
            }
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
        let has_changes = !app
            .multi_diff
            .current_navigator()
            .diff()
            .significant_changes
            .is_empty();
        render_empty_state(frame, content_area, &app.theme, has_changes);
    } else {
        let mut content_paragraph = if app.line_wrap {
            Paragraph::new(content_lines)
                .wrap(Wrap { trim: false })
                .scroll((app.scroll_offset as u16, 0))
        } else {
            Paragraph::new(content_lines).scroll((0, app.horizontal_scroll as u16))
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
    let view_lines = app
        .multi_diff
        .current_navigator()
        .current_view_with_frame(animation_frame);
    let visible_height = area.height as usize;
    let debug_target = app.syntax_scope_target(&view_lines);

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
    let visible_width = content_area.width as usize;

    let mut gutter_lines: Vec<Line> = Vec::new();
    let mut content_lines: Vec<Line> = Vec::new();
    let mut marker_lines: Vec<Line> = Vec::new();
    let mut line_idx = 0;
    let query = app.search_query().trim().to_ascii_lowercase();
    let has_query = !query.is_empty();

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
                (
                    primary_marker_right.as_str(),
                    Style::default()
                        .fg(app.theme.primary)
                        .add_modifier(Modifier::BOLD),
                )
            } else if view_line.show_hunk_extent {
                (
                    extent_marker_right.as_str(),
                    Style::default().fg(app.theme.diff_ext_marker),
                )
            } else {
                (" ", Style::default())
            };

            // Build gutter line
            let gutter_spans = vec![
                Span::styled(
                    line_num_str,
                    Style::default().fg(app.theme.diff_line_number),
                ),
                Span::styled(" ", Style::default()),
            ];
            gutter_lines.push(Line::from(gutter_spans));

            let display_idx = line_idx;
            // Build content line
            let mut content_spans: Vec<Span<'static>> = Vec::new();
            let mut used_syntax = false;
            let pure_context = matches!(view_line.kind, LineKind::Context)
                && !view_line.has_changes
                && !view_line.is_active
                && view_line
                    .spans
                    .iter()
                    .all(|span| matches!(span.kind, ViewSpanKind::Equal));
            if app.syntax_enabled() && pure_context {
                if let Some(spans) = app.syntax_spans_for_line(SyntaxSide::New, Some(new_line_num))
                {
                    content_spans = spans;
                    used_syntax = true;
                }
            }
            if !used_syntax {
                let mut rebuilt_spans: Vec<ViewSpan> = Vec::new();
                let spans =
                    if matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify) {
                        if let Some(change) = app
                            .multi_diff
                            .current_navigator()
                            .diff()
                            .changes
                            .get(view_line.change_id)
                        {
                            for span in &change.spans {
                                match span.kind {
                                    ChangeKind::Equal => rebuilt_spans.push(ViewSpan {
                                        text: span.text.clone(),
                                        kind: ViewSpanKind::Equal,
                                    }),
                                    ChangeKind::Insert => rebuilt_spans.push(ViewSpan {
                                        text: span.text.clone(),
                                        kind: if view_line.is_active {
                                            ViewSpanKind::PendingInsert
                                        } else {
                                            ViewSpanKind::Inserted
                                        },
                                    }),
                                    ChangeKind::Replace => rebuilt_spans.push(ViewSpan {
                                        text: span
                                            .new_text
                                            .clone()
                                            .unwrap_or_else(|| span.text.clone()),
                                        kind: if view_line.is_active {
                                            ViewSpanKind::PendingInsert
                                        } else {
                                            ViewSpanKind::Inserted
                                        },
                                    }),
                                    ChangeKind::Delete => {}
                                }
                            }
                        }
                        if rebuilt_spans.is_empty() {
                            &view_line.spans
                        } else {
                            &rebuilt_spans
                        }
                    } else {
                        &view_line.spans
                    };
                for view_span in spans {
                    let style = get_new_span_style(
                        view_span.kind,
                        view_line.kind,
                        view_line.is_active,
                        app,
                    );
                    content_spans.push(Span::styled(view_span.text.clone(), style));
                }
            }
            let line_text = spans_to_text(&content_spans);
            let is_active_match = app.search_target() == Some(display_idx)
                && has_query
                && line_text.to_ascii_lowercase().contains(&query);
            content_spans = app.highlight_search_spans(content_spans, &line_text, is_active_match);

            if app.line_wrap {
                content_spans = expand_tabs_in_spans(&content_spans, TAB_WIDTH);
            }

            let wrap_count = if app.line_wrap {
                wrap_count_for_spans(&content_spans, visible_width)
            } else {
                1
            };
            content_lines.push(Line::from(content_spans));

            // Build marker line
            marker_lines.push(Line::from(Span::styled(active_marker, active_style)));
            if app.line_wrap {
                if wrap_count > 1 {
                    for _ in 1..wrap_count {
                        gutter_lines.push(Line::from(Span::raw(" ")));
                        marker_lines.push(Line::from(Span::raw(" ")));
                    }
                }
            }

            line_idx += 1;

            if let Some((debug_idx, ref label)) = debug_target {
                if debug_idx == display_idx {
                    let debug_text = truncate_text(&format!("  {}", label), visible_width);
                    let debug_style = Style::default().fg(app.theme.text_muted);
                    let debug_wrap = if app.line_wrap {
                        wrap_count_for_text(&debug_text, visible_width)
                    } else {
                        1
                    };
                    gutter_lines.push(Line::from(Span::raw(" ")));
                    content_lines.push(Line::from(Span::styled(debug_text, debug_style)));
                    marker_lines.push(Line::from(Span::raw(" ")));
                    if app.line_wrap {
                        if debug_wrap > 1 {
                            for _ in 1..debug_wrap {
                                gutter_lines.push(Line::from(Span::raw(" ")));
                                marker_lines.push(Line::from(Span::raw(" ")));
                            }
                        }
                    }
                }
            }
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
        let has_changes = !app
            .multi_diff
            .current_navigator()
            .diff()
            .significant_changes
            .is_empty();
        render_empty_state(frame, content_area, &app.theme, has_changes);
    } else {
        let mut content_paragraph = if app.line_wrap {
            Paragraph::new(content_lines)
                .wrap(Wrap { trim: false })
                .scroll((app.scroll_offset as u16, 0))
        } else {
            Paragraph::new(content_lines).scroll((0, app.horizontal_scroll as u16))
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

fn split_wrap_display_metrics(
    app: &mut App,
    view: &[ViewLine],
    old_width: usize,
    new_width: usize,
    scroll_offset: usize,
    step_direction: StepDirection,
) -> (usize, Option<usize>) {
    let mut old_len = 0usize;
    let mut new_len = 0usize;
    let mut old_primary_idx: Option<usize> = None;
    let mut new_primary_idx: Option<usize> = None;
    let mut old_fallback_idx: Option<usize> = None;
    let mut new_fallback_idx: Option<usize> = None;

    for line in view {
        if line.old_line.is_some() {
            if line.is_primary_active {
                old_primary_idx = Some(old_len);
            } else if line.is_active && old_fallback_idx.is_none() {
                old_fallback_idx = Some(old_len);
            }
            let wrap_count = split_old_line_wrap_count(app, line, old_width);
            old_len += wrap_count;
        }
        if line.new_line.is_some() {
            if matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete) {
                continue;
            }
            if line.is_primary_active {
                new_primary_idx = Some(new_len);
            } else if line.is_active && new_fallback_idx.is_none() {
                new_fallback_idx = Some(new_len);
            }
            let wrap_count = split_new_line_wrap_count(app, line, new_width);
            new_len += wrap_count;
        }
    }

    let display_len = old_len.max(new_len);
    let (old_idx, new_idx) = if old_primary_idx.is_some() || new_primary_idx.is_some() {
        (old_primary_idx, new_primary_idx)
    } else {
        (old_fallback_idx, new_fallback_idx)
    };

    let active_idx = match (old_idx, new_idx) {
        (Some(old), Some(new)) => {
            let old_dist = (old as isize - scroll_offset as isize).abs();
            let new_dist = (new as isize - scroll_offset as isize).abs();
            if old_dist < new_dist {
                Some(old)
            } else if new_dist < old_dist {
                Some(new)
            } else {
                match step_direction {
                    StepDirection::Forward | StepDirection::None => Some(new),
                    StepDirection::Backward => Some(old),
                }
            }
        }
        (Some(old), None) => Some(old),
        (None, Some(new)) => Some(new),
        (None, None) => None,
    };

    (display_len, active_idx)
}

fn split_old_line_wrap_count(app: &mut App, line: &ViewLine, wrap_width: usize) -> usize {
    if matches!(line.kind, LineKind::Modified | LineKind::PendingModify) {
        if let Some(change) = app
            .multi_diff
            .current_navigator()
            .diff()
            .changes
            .get(line.change_id)
        {
            let mut text = String::new();
            for span in &change.spans {
                match span.kind {
                    ChangeKind::Equal | ChangeKind::Delete | ChangeKind::Replace => {
                        text.push_str(&span.text);
                    }
                    ChangeKind::Insert => {}
                }
            }
            if !text.is_empty() {
                return wrap_count_for_text(&text, wrap_width);
            }
        }
    }

    let text = view_spans_to_text(&line.spans);
    wrap_count_for_text(&text, wrap_width)
}

fn split_new_line_wrap_count(app: &mut App, line: &ViewLine, wrap_width: usize) -> usize {
    if matches!(line.kind, LineKind::Modified | LineKind::PendingModify) {
        if let Some(change) = app
            .multi_diff
            .current_navigator()
            .diff()
            .changes
            .get(line.change_id)
        {
            let mut text = String::new();
            for span in &change.spans {
                match span.kind {
                    ChangeKind::Equal | ChangeKind::Insert => {
                        text.push_str(&span.text);
                    }
                    ChangeKind::Replace => {
                        let new_text = span.new_text.as_ref().unwrap_or(&span.text);
                        text.push_str(new_text);
                    }
                    ChangeKind::Delete => {}
                }
            }
            if !text.is_empty() {
                return wrap_count_for_text(&text, wrap_width);
            }
        }
    }

    let text = view_spans_to_text(&line.spans);
    wrap_count_for_text(&text, wrap_width)
}

fn view_spans_to_text(spans: &[ViewSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        out.push_str(&span.text);
    }
    out
}

fn get_old_span_style(
    kind: ViewSpanKind,
    _line_kind: LineKind,
    is_active: bool,
    app: &App,
) -> Style {
    let theme = &app.theme;
    match kind {
        ViewSpanKind::Equal => Style::default().fg(theme.diff_context),
        ViewSpanKind::Deleted => {
            // Active delete should fade from context to delete color.
            let mut style = if is_active {
                super::delete_style(
                    app.animation_phase,
                    app.animation_progress,
                    app.is_backward_animation(),
                    app.strikethrough_deletions,
                    theme.delete_base(),
                    theme.diff_context,
                )
            } else {
                super::delete_style(
                    AnimationPhase::Idle,
                    0.0,
                    false,
                    false,
                    theme.delete_base(),
                    theme.delete_dim(),
                )
            };
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
                    theme.delete_dim(),
                );
                if app.strikethrough_deletions {
                    style = style.add_modifier(Modifier::CROSSED_OUT);
                }
                style
            }
        }
        ViewSpanKind::PendingInsert => Style::default()
            .fg(theme.text_muted)
            .add_modifier(Modifier::DIM),
    }
}

fn get_new_span_style(
    kind: ViewSpanKind,
    _line_kind: LineKind,
    is_active: bool,
    app: &App,
) -> Style {
    let theme = &app.theme;
    match kind {
        ViewSpanKind::Equal => Style::default().fg(theme.diff_context),
        ViewSpanKind::Inserted => {
            // Completed insertion: base color
            super::insert_style(
                AnimationPhase::Idle,
                0.0,
                false,
                theme.insert_base(),
                theme.insert_dim(),
            )
        }
        ViewSpanKind::Deleted => {
            // In new pane, deleted content shouldn't appear
            Style::default().fg(theme.text_muted)
        }
        ViewSpanKind::PendingInsert => {
            if is_active {
                super::insert_style(
                    app.animation_phase,
                    app.animation_progress,
                    app.is_backward_animation(),
                    theme.insert_base(),
                    theme.insert_dim(),
                )
            } else {
                // Non-active pending insert: show dim
                Style::default().fg(theme.insert_dim())
            }
        }
        ViewSpanKind::PendingDelete => Style::default().fg(theme.delete_dim()),
    }
}
