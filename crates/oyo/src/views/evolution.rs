//! Evolution view - shows file morphing without deletion markers
//! Deleted lines simply disappear, showing the file as it evolves

use super::{
    expand_tabs_in_spans, pending_tail_text, render_empty_state, slice_spans, spans_to_text,
    spans_width, truncate_text, view_spans_to_text, wrap_count_for_spans, wrap_count_for_text,
    TAB_WIDTH,
};
use crate::app::{AnimationPhase, App};
use crate::syntax::SyntaxSide;
use oyo_core::{LineKind, StepDirection, ViewLine, ViewSpanKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};

/// Width of the fixed line number gutter (marker + line num + space + blank sign + space)
const GUTTER_WIDTH: u16 = 8; // "▶1234   " (matches single-pane width)

fn hunk_overflow_wrapped_evolution(
    view_lines: &[ViewLine],
    hunk_idx: usize,
    wrap_width: usize,
    scroll_offset: usize,
    viewport_height: usize,
) -> Option<(bool, bool)> {
    let mut display_idx = 0usize;
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;

    for line in view_lines {
        if matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete) {
            continue;
        }
        let text = view_spans_to_text(&line.spans);
        let wrap_count = wrap_count_for_text(&text, wrap_width).max(1);
        if line.hunk_index == Some(hunk_idx) {
            if start.is_none() {
                start = Some(display_idx);
            }
            end = Some(display_idx.saturating_add(wrap_count.saturating_sub(1)));
        }
        display_idx = display_idx.saturating_add(wrap_count);
    }

    let (start, end) = match (start, end) {
        (Some(start), Some(end)) => (start, end),
        _ => return None,
    };
    let visible_start = scroll_offset;
    let visible_end = scroll_offset.saturating_add(viewport_height.saturating_sub(1));
    Some((start < visible_start, end > visible_end))
}

/// Render the evolution view - file morphing without deletion markers
pub fn render_evolution(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height as usize;
    let visible_width = area.width.saturating_sub(GUTTER_WIDTH) as usize;
    if !app.line_wrap {
        app.clamp_horizontal_scroll_cached(visible_width);
    }
    if app.current_file_is_binary() {
        render_empty_state(frame, area, &app.theme, false, true);
        return;
    }

    // Clone markers to avoid borrow conflicts
    let primary_marker = app.primary_marker.clone();
    let extent_marker = app.extent_marker.clone();

    if app.line_wrap {
        app.handle_search_scroll_if_needed(visible_height);
    } else {
        app.ensure_active_visible_if_needed(visible_height);
    }
    let animation_frame = app.animation_frame();
    let show_extent = app.stepping && !app.multi_diff.current_navigator().state().is_at_start();
    app.multi_diff
        .current_navigator()
        .set_show_hunk_extent_while_stepping(show_extent);
    let view_lines = app
        .multi_diff
        .current_navigator()
        .current_view_with_frame(animation_frame);
    let step_direction = app.multi_diff.current_step_direction();
    let mut display_len = 0usize;
    if !app.line_wrap {
        let (len, _) = crate::app::display_metrics(
            &view_lines,
            app.view_mode,
            app.animation_phase,
            app.scroll_offset,
            step_direction,
            app.split_align_lines,
        );
        app.clamp_scroll(len, visible_height, app.allow_overscroll());
        display_len = len;
    }
    let debug_target = app.syntax_scope_target(&view_lines);
    let pending_insert_only = if app.stepping {
        app.pending_insert_only_in_current_hunk()
    } else {
        0
    };
    let current_hunk = app.multi_diff.current_navigator().state().current_hunk;
    let show_virtual = app.allow_virtual_lines();
    let pending_text = if show_virtual && pending_insert_only > 0 {
        Some(pending_tail_text(pending_insert_only))
    } else {
        None
    };
    let mut parts: Vec<(String, bool)> = Vec::new();
    if let Some(pending) = pending_text {
        parts.push((pending, true));
    }
    if let Some(hint) = app.last_step_hint_text() {
        parts.push((hint.to_string(), true));
    }
    if let Some(hint) = app.hunk_edge_hint_text() {
        parts.push((hint.to_string(), true));
    }
    if let Some(hint) = app.blame_hunk_hint_text() {
        parts.push((hint.to_string(), false));
    }
    let virtual_text = if show_virtual && !parts.is_empty() {
        if pending_insert_only == 0 {
            if let Some((first, allow_prefix)) = parts.first_mut() {
                if *allow_prefix {
                    *first = format!("... {first}");
                }
            }
        }
        Some(
            parts
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
                .join(" • "),
        )
    } else {
        None
    };
    let (overflow_above, overflow_below) = if virtual_text.is_some() {
        if app.line_wrap {
            hunk_overflow_wrapped_evolution(
                &view_lines,
                current_hunk,
                visible_width,
                app.scroll_offset,
                visible_height,
            )
            .unwrap_or((false, false))
        } else {
            app.hunk_hint_overflow(current_hunk, visible_height)
                .unwrap_or((false, false))
        }
    } else {
        (false, false)
    };

    // Split area into gutter (fixed) and content (scrollable)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(GUTTER_WIDTH), Constraint::Min(0)])
        .split(area);

    let gutter_area = chunks[0];
    let content_area = chunks[1];

    // Build separate gutter and content lines - skip deleted lines entirely
    let mut gutter_lines: Vec<Line> = Vec::new();
    let mut content_lines: Vec<Line> = Vec::new();
    let mut display_line_num = 0usize;
    let mut max_line_width: usize = 0;
    let wrap_width = visible_width;
    let mut primary_display_idx: Option<usize> = None;
    let mut active_display_idx: Option<usize> = None;
    let hunk_preview_mode = app.multi_diff.current_navigator().state().hunk_preview_mode;
    let animation_phase = app.animation_phase;
    let is_visible = |line: &ViewLine| -> bool {
        match line.kind {
            LineKind::Deleted => false,
            LineKind::PendingDelete => {
                if hunk_preview_mode {
                    return false;
                }
                if !line.is_active_change {
                    return false;
                }
                animation_phase != AnimationPhase::Idle
            }
            _ => true,
        }
    };
    let step_direction = app.multi_diff.current_step_direction();
    let primary_raw_idx = view_lines.iter().position(|line| line.is_primary_active);
    let visible_indices: Vec<usize> = view_lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| if is_visible(line) { Some(idx) } else { None })
        .collect();
    let mut next_visible_hunk: Vec<Option<usize>> = vec![None; view_lines.len()];
    let mut next_hunk: Option<usize> = None;
    for idx in visible_indices.iter().rev() {
        next_visible_hunk[*idx] = next_hunk;
        if view_lines[*idx].hunk_index.is_some() {
            next_hunk = view_lines[*idx].hunk_index;
        }
    }
    let mut prev_visible_hunk: Option<usize> = None;
    let mut virtual_inserted = false;
    let fallback_primary = primary_raw_idx.and_then(|idx| {
        if visible_indices.is_empty() {
            return None;
        }
        if is_visible(&view_lines[idx]) {
            return Some(idx);
        }
        let (mut before, mut after) = (None, None);
        for &vis in &visible_indices {
            if vis < idx {
                before = Some(vis);
            } else if vis > idx {
                after = Some(vis);
                break;
            }
        }
        let prefer_after = !matches!(step_direction, StepDirection::Backward);
        if prefer_after {
            after.or(before)
        } else {
            before.or(after)
        }
    });
    let cursor_in_target = view_lines.iter().enumerate().any(|(raw_idx, line)| {
        let is_primary = line.is_primary_active || fallback_primary == Some(raw_idx);
        is_primary && line.hunk_index == Some(current_hunk)
    });
    let cursor_visible = if app.line_wrap {
        cursor_in_target
    } else {
        let mut display_idx = 0usize;
        let mut cursor_display = None;
        for (raw_idx, line) in view_lines.iter().enumerate() {
            if !is_visible(line) {
                continue;
            }
            let is_primary = line.is_primary_active || fallback_primary == Some(raw_idx);
            if is_primary {
                cursor_display = Some(display_idx);
                break;
            }
            display_idx += 1;
        }
        cursor_display
            .map(|idx| {
                idx >= app.scroll_offset && idx < app.scroll_offset.saturating_add(visible_height)
            })
            .unwrap_or(false)
    };
    let mut prev_visible_hunk_map: Vec<Option<usize>> = vec![None; view_lines.len()];
    let mut prev_hunk: Option<usize> = None;
    for idx in visible_indices.iter() {
        prev_visible_hunk_map[*idx] = prev_hunk;
        if view_lines[*idx].hunk_index.is_some() {
            prev_hunk = view_lines[*idx].hunk_index;
        }
    }
    let cursor_idx = visible_indices.iter().copied().find(|idx| {
        let line = &view_lines[*idx];
        let is_primary = line.is_primary_active || fallback_primary == Some(*idx);
        is_primary && line.hunk_index == Some(current_hunk)
    });
    let cursor_at_first = cursor_idx
        .map(|idx| prev_visible_hunk_map[idx] != Some(current_hunk))
        .unwrap_or(false);
    let cursor_at_last = cursor_idx
        .map(|idx| next_visible_hunk[idx] != Some(current_hunk))
        .unwrap_or(false);
    let fully_visible = !overflow_above && !overflow_below;
    let mut force_top = overflow_below && !overflow_above;
    let mut force_bottom = overflow_above && !overflow_below;
    if fully_visible {
        if cursor_at_first {
            force_top = true;
        } else if cursor_at_last {
            force_bottom = true;
        }
    }
    let mut prefer_cursor =
        cursor_in_target && cursor_visible && !force_top && !force_bottom && !fully_visible;
    if pending_insert_only > 0 && cursor_in_target && cursor_visible {
        prefer_cursor = true;
    }

    let query = app.search_query().trim().to_ascii_lowercase();
    let has_query = !query.is_empty();
    for (raw_idx, view_line) in view_lines.iter().enumerate() {
        // Skip lines that are deleted or pending delete (they disappear in evolution view)
        if !is_visible(view_line) {
            continue;
        }

        if app.line_wrap {
            let display_idx = display_len;
            let is_primary = view_line.is_primary_active || fallback_primary == Some(raw_idx);
            if is_primary && primary_display_idx.is_none() {
                primary_display_idx = Some(display_idx);
            }
            if (view_line.is_active || is_primary) && active_display_idx.is_none() {
                active_display_idx = Some(display_idx);
            }
        }

        display_line_num += 1;
        let display_idx = display_line_num - 1;

        // Handle scrolling - when wrapping, we need all lines
        if !app.line_wrap && display_line_num <= app.scroll_offset {
            continue;
        }
        if !app.line_wrap && gutter_lines.len() >= visible_height {
            break;
        }

        let line_hunk = view_line.hunk_index;
        let is_first_in_hunk = line_hunk.is_some() && prev_visible_hunk != line_hunk;
        let is_last_in_hunk = line_hunk.is_some() && next_visible_hunk[raw_idx] != line_hunk;
        if let Some(text) = virtual_text.as_ref() {
            if !virtual_inserted
                && !prefer_cursor
                && force_top
                && line_hunk == Some(current_hunk)
                && is_first_in_hunk
            {
                let virtual_style = Style::default()
                    .fg(app.theme.text_muted)
                    .add_modifier(Modifier::ITALIC);
                let mut virtual_spans = vec![Span::styled(text.clone(), virtual_style)];
                virtual_spans = expand_tabs_in_spans(&virtual_spans, TAB_WIDTH);

                let virtual_width = spans_width(&virtual_spans);
                max_line_width = max_line_width.max(virtual_width);

                let virtual_wrap = if app.line_wrap {
                    wrap_count_for_spans(&virtual_spans, wrap_width)
                } else {
                    1
                };
                if app.line_wrap {
                    display_len += virtual_wrap;
                }

                let mut display_virtual = virtual_spans;
                if !app.line_wrap {
                    display_virtual =
                        slice_spans(&display_virtual, app.horizontal_scroll, visible_width);
                }
                content_lines.push(Line::from(display_virtual));
                gutter_lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::raw("    "),
                    Span::raw(" "),
                    Span::raw(" "),
                    Span::raw(" "),
                ]));
                if app.line_wrap && virtual_wrap > 1 {
                    for _ in 1..virtual_wrap {
                        gutter_lines.push(Line::from(Span::raw(" ")));
                    }
                }
                virtual_inserted = true;
            }
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
                if view_line.is_active_change && app.animation_phase != AnimationPhase::Idle {
                    let mut t = crate::color::animation_t_linear(
                        app.animation_phase,
                        app.animation_progress,
                    );
                    if app.is_backward_animation() {
                        t = 1.0 - t;
                    }
                    let t = crate::color::ease_out(t);
                    let color = crate::color::lerp_rgb_color(
                        app.theme.diff_line_number,
                        app.theme.delete_base(),
                        t,
                    );
                    Style::default().fg(color)
                } else {
                    Style::default().fg(app.theme.diff_line_number)
                }
            }
            LineKind::Deleted => Style::default().fg(app.theme.text_muted),
        };

        // Gutter marker: primary marker for focus, extent marker for hunk nav, blank otherwise
        let is_primary = view_line.is_primary_active || fallback_primary == Some(raw_idx);
        let (active_marker, active_style) = if is_primary {
            (
                primary_marker.as_str(),
                Style::default()
                    .fg(app.theme.primary)
                    .add_modifier(Modifier::BOLD),
            )
        } else if view_line.show_hunk_extent {
            (
                extent_marker.as_str(),
                super::extent_marker_style(
                    app,
                    view_line.kind,
                    view_line.has_changes,
                    view_line.old_line,
                    view_line.new_line,
                ),
            )
        } else {
            (" ", Style::default())
        };

        // Build gutter line (fixed, no horizontal scroll)
        let gutter_spans = vec![
            Span::styled(active_marker, active_style),
            Span::styled(line_num_str, line_num_style),
            Span::styled(" ", Style::default()),
            Span::styled(" ", Style::default()),
            Span::styled(" ", Style::default()),
        ];
        // Evolution view ignores diff background modes to keep the morph view clean.
        gutter_lines.push(Line::from(gutter_spans));

        // Build content line (scrollable)
        let mut content_spans: Vec<Span<'static>> = Vec::new();
        let mut used_syntax = false;
        let allow_syntax = app.syntax_enabled()
            && match app.evo_syntax {
                crate::config::EvoSyntaxMode::Context => !view_line.has_changes,
                crate::config::EvoSyntaxMode::Full => !view_line.is_active_change,
            };
        if allow_syntax {
            let use_old = match view_line.kind {
                LineKind::Deleted | LineKind::PendingDelete => true,
                LineKind::Inserted
                | LineKind::Modified
                | LineKind::PendingInsert
                | LineKind::PendingModify => false,
                LineKind::Context => view_line.has_changes,
            };
            let side = if use_old {
                SyntaxSide::Old
            } else {
                SyntaxSide::New
            };
            let line_num = if use_old {
                view_line.old_line.or(view_line.new_line)
            } else {
                view_line.new_line.or(view_line.old_line)
            };
            if let Some(spans) = app.syntax_spans_for_line(side, line_num) {
                content_spans = spans;
                used_syntax = true;
            }
        }
        if !used_syntax {
            for view_span in &view_line.spans {
                let style = get_evolution_span_style(
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

        // Evolution view ignores diff background modes to keep the morph view clean.

        let line_text = spans_to_text(&content_spans);
        let is_active_match = app.search_target() == Some(display_idx)
            && has_query
            && line_text.to_ascii_lowercase().contains(&query);
        content_spans = app.highlight_search_spans(content_spans, &line_text, is_active_match);

        content_spans = expand_tabs_in_spans(&content_spans, TAB_WIDTH);

        // Track max line width
        let line_width = spans_width(&content_spans);
        max_line_width = max_line_width.max(line_width);

        let wrap_count = if app.line_wrap {
            wrap_count_for_spans(&content_spans, wrap_width)
        } else {
            1
        };
        if app.line_wrap {
            display_len += wrap_count;
        }

        let mut display_spans = content_spans;
        if !app.line_wrap {
            display_spans = slice_spans(&display_spans, app.horizontal_scroll, visible_width);
        }
        content_lines.push(Line::from(display_spans));
        if app.line_wrap && wrap_count > 1 {
            for _ in 1..wrap_count {
                gutter_lines.push(Line::from(Span::raw(" ")));
            }
        }
        if let Some(text) = virtual_text.as_ref() {
            if !virtual_inserted
                && prefer_cursor
                && line_hunk == Some(current_hunk)
                && (view_line.is_primary_active || fallback_primary == Some(raw_idx))
            {
                let virtual_style = Style::default()
                    .fg(app.theme.text_muted)
                    .add_modifier(Modifier::ITALIC);
                let mut virtual_spans = vec![Span::styled(text.clone(), virtual_style)];
                virtual_spans = expand_tabs_in_spans(&virtual_spans, TAB_WIDTH);

                let virtual_width = spans_width(&virtual_spans);
                max_line_width = max_line_width.max(virtual_width);

                let virtual_wrap = if app.line_wrap {
                    wrap_count_for_spans(&virtual_spans, wrap_width)
                } else {
                    1
                };
                if app.line_wrap {
                    display_len += virtual_wrap;
                }

                let mut display_virtual = virtual_spans;
                if !app.line_wrap {
                    display_virtual =
                        slice_spans(&display_virtual, app.horizontal_scroll, visible_width);
                }
                content_lines.push(Line::from(display_virtual));
                gutter_lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::raw("    "),
                    Span::raw(" "),
                    Span::raw(" "),
                    Span::raw(" "),
                ]));
                if app.line_wrap && virtual_wrap > 1 {
                    for _ in 1..virtual_wrap {
                        gutter_lines.push(Line::from(Span::raw(" ")));
                    }
                }
                virtual_inserted = true;
            }
        }
        if let Some(text) = virtual_text.as_ref() {
            if !virtual_inserted
                && !prefer_cursor
                && !force_top
                && line_hunk == Some(current_hunk)
                && is_last_in_hunk
            {
                let virtual_style = Style::default()
                    .fg(app.theme.text_muted)
                    .add_modifier(Modifier::ITALIC);
                let mut virtual_spans = vec![Span::styled(text.clone(), virtual_style)];
                virtual_spans = expand_tabs_in_spans(&virtual_spans, TAB_WIDTH);

                let virtual_width = spans_width(&virtual_spans);
                max_line_width = max_line_width.max(virtual_width);

                let virtual_wrap = if app.line_wrap {
                    wrap_count_for_spans(&virtual_spans, wrap_width)
                } else {
                    1
                };
                if app.line_wrap {
                    display_len += virtual_wrap;
                }

                let mut display_virtual = virtual_spans;
                if !app.line_wrap {
                    display_virtual =
                        slice_spans(&display_virtual, app.horizontal_scroll, visible_width);
                }
                content_lines.push(Line::from(display_virtual));
                gutter_lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::raw("    "),
                    Span::raw(" "),
                    Span::raw(" "),
                    Span::raw(" "),
                ]));
                if app.line_wrap && virtual_wrap > 1 {
                    for _ in 1..virtual_wrap {
                        gutter_lines.push(Line::from(Span::raw(" ")));
                    }
                }
                virtual_inserted = true;
            }
        }
        prev_visible_hunk = line_hunk;

        if let Some(hint_text) = app.step_edge_hint_for_change(view_line.change_id) {
            let virtual_style = Style::default()
                .fg(app.theme.text_muted)
                .add_modifier(Modifier::ITALIC);
            let mut virtual_spans = vec![Span::styled(hint_text.to_string(), virtual_style)];
            virtual_spans = expand_tabs_in_spans(&virtual_spans, TAB_WIDTH);

            let virtual_width = spans_width(&virtual_spans);
            max_line_width = max_line_width.max(virtual_width);

            let virtual_wrap = if app.line_wrap {
                wrap_count_for_spans(&virtual_spans, wrap_width)
            } else {
                1
            };
            if app.line_wrap {
                display_len += virtual_wrap;
            }

            let mut display_virtual = virtual_spans;
            if !app.line_wrap {
                display_virtual =
                    slice_spans(&display_virtual, app.horizontal_scroll, visible_width);
            }
            content_lines.push(Line::from(display_virtual));
            gutter_lines.push(Line::from(vec![
                Span::raw(" "),
                Span::raw("    "),
                Span::raw(" "),
                Span::raw(" "),
                Span::raw(" "),
            ]));
            if app.line_wrap && virtual_wrap > 1 {
                for _ in 1..virtual_wrap {
                    gutter_lines.push(Line::from(Span::raw(" ")));
                }
            }
        }

        if let Some(hint_text) = app.blame_step_hint_for_change(view_line.change_id) {
            let virtual_style = Style::default()
                .fg(app.theme.text_muted)
                .add_modifier(Modifier::ITALIC);
            let mut virtual_spans = vec![Span::styled(hint_text.to_string(), virtual_style)];
            virtual_spans = expand_tabs_in_spans(&virtual_spans, TAB_WIDTH);

            let virtual_width = spans_width(&virtual_spans);
            max_line_width = max_line_width.max(virtual_width);

            let virtual_wrap = if app.line_wrap {
                wrap_count_for_spans(&virtual_spans, wrap_width)
            } else {
                1
            };
            if app.line_wrap {
                display_len += virtual_wrap;
            }

            let mut display_virtual = virtual_spans;
            if !app.line_wrap {
                display_virtual =
                    slice_spans(&display_virtual, app.horizontal_scroll, visible_width);
            }
            content_lines.push(Line::from(display_virtual));
            gutter_lines.push(Line::from(Span::raw(" ")));
            if app.line_wrap && virtual_wrap > 1 {
                for _ in 1..virtual_wrap {
                    gutter_lines.push(Line::from(Span::raw(" ")));
                }
            }
        }

        if let Some((debug_idx, ref label)) = debug_target {
            if debug_idx == display_idx {
                let debug_text = truncate_text(&format!("  {}", label), visible_width);
                let debug_style = Style::default().fg(app.theme.text_muted);
                let debug_wrap = if app.line_wrap {
                    wrap_count_for_text(&debug_text, wrap_width)
                } else {
                    1
                };
                gutter_lines.push(Line::from(Span::raw(" ")));
                content_lines.push(Line::from(Span::styled(debug_text, debug_style)));
                if app.line_wrap {
                    display_len += debug_wrap;
                    if debug_wrap > 1 {
                        for _ in 1..debug_wrap {
                            gutter_lines.push(Line::from(Span::raw(" ")));
                        }
                    }
                }
            }
        }
    }

    if app.line_wrap {
        app.ensure_active_visible_if_needed_wrapped(
            visible_height,
            display_len,
            primary_display_idx.or(active_display_idx),
        );
        app.clamp_scroll(display_len, visible_height, app.allow_overscroll());
    }

    // Clamp horizontal scroll
    app.clamp_horizontal_scroll(max_line_width, visible_width);

    app.set_current_max_line_width(max_line_width);

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
        render_empty_state(
            frame,
            content_area,
            &app.theme,
            has_changes,
            app.current_file_is_binary(),
        );
    } else {
        let mut content_paragraph = if app.line_wrap {
            Paragraph::new(content_lines)
                .wrap(Wrap { trim: false })
                .scroll((app.scroll_offset as u16, 0))
        } else {
            Paragraph::new(content_lines)
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

fn get_evolution_span_style(
    span_kind: ViewSpanKind,
    line_kind: LineKind,
    is_active: bool,
    app: &App,
) -> Style {
    let theme = &app.theme;
    // Check if this is a modification line - use modify gradient instead of insert
    let is_modification = matches!(line_kind, LineKind::Modified | LineKind::PendingModify);
    let added_bg = None;
    let removed_bg = None;
    let modified_bg = None;

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
                    modified_bg,
                )
            } else {
                // Pure insertion: use insert colors
                super::insert_style(
                    AnimationPhase::Idle,
                    0.0,
                    false,
                    theme.insert_base(),
                    theme.diff_context,
                    added_bg,
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
                        modified_bg,
                    )
                } else {
                    let mut style = Style::default().fg(theme.modify_dim());
                    if let Some(bg) = modified_bg {
                        style = style.bg(bg);
                    }
                    style
                }
            } else if is_active {
                super::insert_style(
                    app.animation_phase,
                    app.animation_progress,
                    app.is_backward_animation(),
                    theme.insert_base(),
                    theme.diff_context,
                    added_bg,
                )
            } else {
                let mut style = Style::default().fg(theme.insert_dim());
                if let Some(bg) = added_bg {
                    style = style.bg(bg);
                }
                style
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
                        modified_bg,
                    )
                } else {
                    super::delete_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        app.strikethrough_deletions,
                        theme.delete_base(),
                        theme.diff_context,
                        removed_bg,
                    )
                }
            } else {
                Style::default().fg(theme.text_muted)
            }
        }
    }
}
