//! Split view with synchronized stepping

use super::{
    apply_line_bg, apply_spans_bg, clear_leading_ws_bg, diff_line_bg, expand_tabs_in_spans,
    pad_spans_bg, pending_tail_text, render_empty_state, slice_spans, spans_to_text, spans_width,
    truncate_text, view_spans_to_text, wrap_count_for_spans, wrap_count_for_text, TAB_WIDTH,
};
use crate::app::{AnimationPhase, App};
use crate::color;
use crate::config::{DiffForegroundMode, DiffHighlightMode};
use crate::syntax::SyntaxSide;
use oyo_core::{
    AnimationFrame, ChangeKind, LineKind, StepDirection, ViewLine, ViewSpan, ViewSpanKind,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

fn split_old_bg_kind(kind: LineKind) -> LineKind {
    match kind {
        LineKind::Modified | LineKind::PendingModify => LineKind::Deleted,
        _ => kind,
    }
}

fn split_new_bg_kind(kind: LineKind) -> LineKind {
    match kind {
        LineKind::Modified | LineKind::PendingModify => LineKind::Inserted,
        _ => kind,
    }
}

fn line_num_style_for_kind(kind: LineKind, app: &App) -> Style {
    let insert_base = color::gradient_color(&app.theme.insert, 0.5);
    let delete_base = color::gradient_color(&app.theme.delete, 0.5);
    let modify_base = color::gradient_color(&app.theme.modify, 0.5);
    match kind {
        LineKind::Inserted | LineKind::PendingInsert => {
            Style::default().fg(Color::Rgb(insert_base.r, insert_base.g, insert_base.b))
        }
        LineKind::Deleted | LineKind::PendingDelete => {
            Style::default().fg(Color::Rgb(delete_base.r, delete_base.g, delete_base.b))
        }
        LineKind::Modified | LineKind::PendingModify => {
            Style::default().fg(Color::Rgb(modify_base.r, modify_base.g, modify_base.b))
        }
        LineKind::Context => Style::default().fg(app.theme.diff_line_number),
    }
}

fn align_fill_span(app: &App, width: usize) -> Span<'static> {
    if width == 0 || app.split_align_fill.is_empty() {
        return Span::raw("");
    }
    let full_len = if app.line_wrap {
        width
    } else {
        width.saturating_add(app.horizontal_scroll)
    };
    let mut out = String::with_capacity(full_len);
    for ch in app.split_align_fill.chars().cycle().take(full_len) {
        out.push(ch);
    }
    let text = if app.line_wrap {
        out
    } else {
        out.chars()
            .skip(app.horizontal_scroll)
            .take(width)
            .collect()
    };
    let mut fg = color::dim_color(app.theme.text_muted);
    if let Some(bg) = app.theme.background {
        if let Some(blended) = color::blend_colors(bg, fg, 0.5) {
            fg = blended;
        }
    }
    Span::styled(text, Style::default().fg(fg).add_modifier(Modifier::DIM))
}

fn align_fill_gutter_span(app: &App, width: usize) -> Span<'static> {
    if width == 0 || app.split_align_fill.is_empty() {
        return Span::raw(" ".repeat(width));
    }
    let mut out = String::with_capacity(width);
    for ch in app.split_align_fill.chars().cycle().take(width) {
        out.push(ch);
    }
    let mut fg = color::dim_color(app.theme.text_muted);
    if let Some(bg) = app.theme.background {
        if let Some(blended) = color::blend_colors(bg, fg, 0.5) {
            fg = blended;
        }
    }
    Span::styled(out, Style::default().fg(fg).add_modifier(Modifier::DIM))
}

fn push_virtual_line_old(
    text: &str,
    app: &App,
    visible_width: usize,
    max_line_width: &mut usize,
    content_lines: &mut Vec<Line>,
    gutter_lines: &mut Vec<Line>,
) {
    let virtual_style = Style::default()
        .fg(app.theme.text_muted)
        .add_modifier(Modifier::ITALIC);
    let mut virtual_spans = vec![Span::styled(text.to_string(), virtual_style)];
    virtual_spans = expand_tabs_in_spans(&virtual_spans, TAB_WIDTH);

    let virtual_width = spans_width(&virtual_spans);
    *max_line_width = (*max_line_width).max(virtual_width);

    let virtual_wrap = if app.line_wrap {
        wrap_count_for_spans(&virtual_spans, visible_width)
    } else {
        1
    };

    let mut display_virtual = virtual_spans;
    if !app.line_wrap {
        display_virtual = slice_spans(&display_virtual, app.horizontal_scroll, visible_width);
    }
    content_lines.push(Line::from(display_virtual));
    gutter_lines.push(Line::from(vec![
        Span::raw(" "),
        Span::raw("    "),
        Span::raw(" "),
    ]));
    if app.line_wrap && virtual_wrap > 1 {
        for _ in 1..virtual_wrap {
            gutter_lines.push(Line::from(Span::raw(" ")));
        }
    }
}

fn push_virtual_line_new(
    text: &str,
    app: &App,
    visible_width: usize,
    max_line_width: &mut usize,
    content_lines: &mut Vec<Line>,
    gutter_lines: &mut Vec<Line>,
    marker_lines: &mut Vec<Line>,
) {
    let virtual_style = Style::default()
        .fg(app.theme.text_muted)
        .add_modifier(Modifier::ITALIC);
    let mut virtual_spans = vec![Span::styled(text.to_string(), virtual_style)];
    virtual_spans = expand_tabs_in_spans(&virtual_spans, TAB_WIDTH);

    let virtual_width = spans_width(&virtual_spans);
    *max_line_width = (*max_line_width).max(virtual_width);

    let virtual_wrap = if app.line_wrap {
        wrap_count_for_spans(&virtual_spans, visible_width)
    } else {
        1
    };

    let mut display_virtual = virtual_spans;
    if !app.line_wrap {
        display_virtual = slice_spans(&display_virtual, app.horizontal_scroll, visible_width);
    }
    content_lines.push(Line::from(display_virtual));
    gutter_lines.push(Line::from(vec![Span::raw("    "), Span::raw(" ")]));
    marker_lines.push(Line::from(Span::raw(" ")));
    if app.line_wrap && virtual_wrap > 1 {
        for _ in 1..virtual_wrap {
            gutter_lines.push(Line::from(vec![Span::raw("    "), Span::raw(" ")]));
            marker_lines.push(Line::from(Span::raw(" ")));
        }
    }
}

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
    app.multi_diff
        .current_navigator()
        .set_show_hunk_extent_while_stepping(app.stepping);
    let view_lines = app
        .multi_diff
        .current_navigator()
        .current_view_with_frame(AnimationFrame::Idle);
    let step_direction = app.multi_diff.current_step_direction();
    let preview_hunk = app.multi_diff.current_navigator().state().current_hunk;

    // Split into two panes
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let old_width = chunks[0]
        .width
        .saturating_sub(GUTTER_WIDTH + OLD_BORDER_WIDTH) as usize;
    let new_width = chunks[1]
        .width
        .saturating_sub(NEW_GUTTER_WIDTH + NEW_MARKER_WIDTH) as usize;
    let hunk_overflow = if app.line_wrap {
        split_hunk_overflow_wrapped(
            app,
            &view_lines,
            preview_hunk,
            visible_height,
            old_width,
            new_width,
        )
    } else {
        app.hunk_hint_overflow(preview_hunk, visible_height)
    };

    if app.line_wrap {
        let (display_len, active_idx) = split_wrap_display_metrics(
            app,
            &view_lines,
            old_width,
            new_width,
            app.scroll_offset,
            step_direction,
            app.split_align_lines,
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
            app.split_align_lines,
        );
        app.clamp_scroll(display_len, visible_height, app.allow_overscroll());
    }
    if !app.line_wrap {
        app.clamp_horizontal_scroll_cached(old_width.min(new_width));
    }
    app.reset_current_max_line_width();

    render_old_pane(frame, app, chunks[0], hunk_overflow);
    render_new_pane(frame, app, chunks[1], hunk_overflow);
}

fn render_old_pane(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    hunk_overflow: Option<(bool, bool)>,
) {
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
    let (preview_mode, preview_hunk) = {
        let state = app.multi_diff.current_navigator().state();
        (state.hunk_preview_mode, state.current_hunk)
    };
    let pending_insert_only = if app.stepping {
        app.pending_insert_only_in_current_hunk()
    } else {
        0
    };
    let show_virtual = app.allow_virtual_lines();
    let pending_text = if show_virtual && pending_insert_only > 0 {
        Some(pending_tail_text(pending_insert_only))
    } else {
        None
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(pending) = pending_text {
        parts.push(pending);
    }
    if let Some(hint) = app.last_step_hint_text() {
        parts.push(hint.to_string());
    }
    if let Some(hint) = app.hunk_edge_hint_text() {
        parts.push(hint.to_string());
    }
    let virtual_text = if show_virtual && !parts.is_empty() {
        if pending_insert_only == 0 {
            if let Some(first) = parts.first_mut() {
                *first = format!("... {first}");
            }
        }
        Some(parts.join(" • "))
    } else {
        None
    };
    let (overflow_above, overflow_below) = if virtual_text.is_some() {
        hunk_overflow.unwrap_or((false, false))
    } else {
        (false, false)
    };
    let old_visible = |line: &ViewLine| -> bool {
        let old_present = line.old_line.is_some();
        let new_present = line.new_line.is_some()
            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete);
        old_present || (app.split_align_lines && new_present)
    };
    let cursor_in_target = view_lines
        .iter()
        .any(|line| line.is_primary_active && line.hunk_index == Some(preview_hunk));
    let cursor_visible = if app.line_wrap {
        cursor_in_target
    } else {
        let mut display_idx = 0usize;
        let mut cursor_display = None;
        for line in view_lines.iter() {
            if !old_visible(line) {
                continue;
            }
            if line.is_primary_active {
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
    let visible_indices: Vec<usize> = view_lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| if old_visible(line) { Some(idx) } else { None })
        .collect();
    let mut next_visible_hunk: Vec<Option<usize>> = vec![None; view_lines.len()];
    let mut next_hunk: Option<usize> = None;
    for idx in visible_indices.iter().rev() {
        next_visible_hunk[*idx] = next_hunk;
        if view_lines[*idx].hunk_index.is_some() {
            next_hunk = view_lines[*idx].hunk_index;
        }
    }
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
        line.is_primary_active && line.hunk_index == Some(preview_hunk)
    });
    let cursor_at_first = cursor_idx
        .map(|idx| prev_visible_hunk_map[idx] != Some(preview_hunk))
        .unwrap_or(false);
    let cursor_at_last = cursor_idx
        .map(|idx| next_visible_hunk[idx] != Some(preview_hunk))
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
    let mut prev_visible_hunk: Option<usize> = None;
    let mut virtual_inserted = false;

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

    for (idx, view_line) in view_lines.iter().enumerate() {
        let old_present = view_line.old_line.is_some();
        let new_present = view_line.new_line.is_some()
            && !matches!(view_line.kind, LineKind::Deleted | LineKind::PendingDelete);
        if !(old_present || (app.split_align_lines && new_present)) {
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

        let line_hunk = view_line.hunk_index;
        let is_first_in_hunk = line_hunk.is_some() && prev_visible_hunk != line_hunk;
        let is_last_in_hunk = line_hunk.is_some() && next_visible_hunk[idx] != line_hunk;
        if let Some(text) = virtual_text.as_ref() {
            if !virtual_inserted
                && !prefer_cursor
                && force_top
                && line_hunk == Some(preview_hunk)
                && is_first_in_hunk
            {
                push_virtual_line_old(
                    text,
                    app,
                    visible_width,
                    &mut max_line_width,
                    &mut content_lines,
                    &mut gutter_lines,
                );
                virtual_inserted = true;
            }
        }

        if !old_present {
            let wrap_count = if app.line_wrap {
                split_new_line_wrap_count(app, view_line, visible_width)
            } else {
                1
            };
            let fill_span = align_fill_span(app, visible_width);
            let marker_fill = align_fill_gutter_span(app, 1);
            let gutter_fill = align_fill_gutter_span(app, 4);
            let sign_fill = align_fill_gutter_span(app, 1);
            gutter_lines.push(Line::from(vec![marker_fill, gutter_fill, sign_fill]));
            content_lines.push(Line::from(fill_span.clone()));
            if app.line_wrap && wrap_count > 1 {
                for _ in 1..wrap_count {
                    let marker_fill = align_fill_gutter_span(app, 1);
                    let gutter_fill = align_fill_gutter_span(app, 4);
                    let sign_fill = align_fill_gutter_span(app, 1);
                    gutter_lines.push(Line::from(vec![marker_fill, gutter_fill, sign_fill]));
                    content_lines.push(Line::from(fill_span.clone()));
                }
            }
            if let Some(text) = virtual_text.as_ref() {
                if !virtual_inserted
                    && !force_top
                    && line_hunk == Some(preview_hunk)
                    && is_last_in_hunk
                {
                    push_virtual_line_old(
                        text,
                        app,
                        visible_width,
                        &mut max_line_width,
                        &mut content_lines,
                        &mut gutter_lines,
                    );
                    virtual_inserted = true;
                }
            }
            prev_visible_hunk = line_hunk;
            line_idx += 1;
            continue;
        }

        if let Some(old_line_num) = view_line.old_line {
            let line_num_str = format!("{:4}", old_line_num);
            let bg_kind = split_old_bg_kind(view_line.kind);
            let line_num_style = line_num_style_for_kind(bg_kind, app);
            let line_bg_gutter = if app.diff_bg {
                diff_line_bg(bg_kind, &app.theme)
            } else {
                None
            };

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

            // Build gutter line
            let mut gutter_spans = vec![
                Span::styled(active_marker, active_style),
                Span::styled(line_num_str, line_num_style),
                Span::styled(" ", Style::default()),
            ];
            if let Some(bg) = line_bg_gutter {
                gutter_spans = gutter_spans
                    .into_iter()
                    .enumerate()
                    .map(|(idx, span)| {
                        if idx == 0 {
                            span
                        } else {
                            Span::styled(span.content, span.style.bg(bg))
                        }
                    })
                    .collect();
            }
            gutter_lines.push(Line::from(gutter_spans));

            let display_idx = line_idx;
            // Build content line
            let mut content_spans: Vec<Span<'static>> = Vec::new();
            let mut used_syntax = false;
            let pure_context = matches!(view_line.kind, LineKind::Context)
                && !view_line.has_changes
                && !view_line.is_active_change
                && view_line
                    .spans
                    .iter()
                    .all(|span| matches!(span.kind, ViewSpanKind::Equal));
            let wants_diff_syntax =
                app.diff_fg == DiffForegroundMode::Syntax && app.syntax_enabled();
            let in_preview_hunk =
                preview_mode && view_line.hunk_index == Some(preview_hunk) && wants_diff_syntax;
            let preview_modified = in_preview_hunk
                && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify);
            let highlight_inline = matches!(
                app.diff_highlight,
                DiffHighlightMode::Text | DiffHighlightMode::Word
            );
            let modified_line =
                matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify);
            let can_use_diff_syntax = wants_diff_syntax && !modified_line;
            if app.syntax_enabled()
                && !preview_modified
                && !view_line.is_active_change
                && (pure_context || can_use_diff_syntax || in_preview_hunk)
            {
                if let Some(spans) = app.syntax_spans_for_line(SyntaxSide::Old, Some(old_line_num))
                {
                    content_spans = spans;
                    used_syntax = true;
                }
            }
            if !used_syntax {
                let mut rebuilt_spans: Vec<ViewSpan> = Vec::new();
                let is_applied = app
                    .multi_diff
                    .current_navigator()
                    .state()
                    .applied_changes
                    .contains(&view_line.change_id);
                let show_inline = view_line.old_line.is_some()
                    && view_line.new_line.is_some()
                    && (view_line.is_active || is_applied || (highlight_inline && modified_line));
                let spans = if show_inline {
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
                    let highlight_allowed =
                        matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
                            || !view_line.is_active
                            || (view_line.is_active
                                && !matches!(app.diff_highlight, DiffHighlightMode::None)
                                && (!app.diff_bg || app.diff_fg == DiffForegroundMode::Theme));
                    let style = get_old_span_style(
                        view_span.kind,
                        view_line.kind,
                        view_line.is_active,
                        app,
                        highlight_allowed,
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

            let line_bg_line = if app.diff_bg {
                diff_line_bg(bg_kind, &app.theme)
            } else {
                None
            };
            if let Some(bg) = line_bg_line {
                content_spans = apply_line_bg(content_spans, bg, visible_width, app.line_wrap);
            }

            let highlight_allowed =
                matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
                    || !view_line.is_active
                    || (view_line.is_active
                        && !matches!(app.diff_highlight, DiffHighlightMode::None)
                        && (!app.diff_bg || app.diff_fg == DiffForegroundMode::Theme));
            if highlight_allowed
                && !app.diff_bg
                && matches!(
                    app.diff_highlight,
                    DiffHighlightMode::Text | DiffHighlightMode::Word
                )
                && used_syntax
            {
                if let Some(bg) = diff_line_bg(bg_kind, &app.theme) {
                    content_spans = apply_spans_bg(content_spans, bg);
                }
            }

            if highlight_allowed {
                if !app.diff_bg {
                    if app.diff_highlight == DiffHighlightMode::Text {
                        if !view_line.is_active {
                            content_spans =
                                clear_leading_ws_bg(content_spans, Some(app.theme.diff_context));
                        }
                    } else if app.diff_highlight == DiffHighlightMode::Word {
                        content_spans = clear_leading_ws_bg(content_spans, None);
                    }
                } else if app.diff_highlight == DiffHighlightMode::Word {
                    content_spans = super::replace_leading_ws_bg(content_spans, None, line_bg_line);
                }
            }

            let mut italic_line = false;
            if app.syntax_enabled() {
                if used_syntax {
                    italic_line = super::line_is_italic(&content_spans);
                } else if let Some(spans) =
                    app.syntax_spans_for_line(SyntaxSide::Old, Some(old_line_num))
                {
                    italic_line = super::line_is_italic(&spans);
                }
            }

            let line_text = spans_to_text(&content_spans);
            let is_active_match = app.search_target() == Some(display_idx)
                && has_query
                && line_text.to_ascii_lowercase().contains(&query);
            content_spans = app.highlight_search_spans(content_spans, &line_text, is_active_match);
            if italic_line {
                content_spans = super::apply_italic_spans(content_spans);
            }

            content_spans = expand_tabs_in_spans(&content_spans, TAB_WIDTH);

            let line_width = spans_width(&content_spans);
            max_line_width = max_line_width.max(line_width);

            let wrap_count = if app.line_wrap {
                wrap_count_for_spans(&content_spans, visible_width)
            } else {
                1
            };
            let mut display_spans = content_spans;
            if !app.line_wrap {
                display_spans = slice_spans(&display_spans, app.horizontal_scroll, visible_width);
                if app.diff_bg {
                    if let Some(bg) = diff_line_bg(bg_kind, &app.theme) {
                        display_spans = pad_spans_bg(display_spans, bg, visible_width);
                    }
                }
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
                    && line_hunk == Some(preview_hunk)
                    && view_line.is_primary_active
                {
                    push_virtual_line_old(
                        text,
                        app,
                        visible_width,
                        &mut max_line_width,
                        &mut content_lines,
                        &mut gutter_lines,
                    );
                    virtual_inserted = true;
                }
            }
            if let Some(text) = virtual_text.as_ref() {
                if !virtual_inserted
                    && !prefer_cursor
                    && !force_top
                    && line_hunk == Some(preview_hunk)
                    && is_last_in_hunk
                {
                    push_virtual_line_old(
                        text,
                        app,
                        visible_width,
                        &mut max_line_width,
                        &mut content_lines,
                        &mut gutter_lines,
                    );
                    virtual_inserted = true;
                }
            }

            if let Some(hint_text) = app.step_edge_hint_for_change(view_line.change_id) {
                let virtual_style = Style::default()
                    .fg(app.theme.text_muted)
                    .add_modifier(Modifier::ITALIC);
                let mut virtual_spans = vec![Span::styled(hint_text.to_string(), virtual_style)];
                virtual_spans = expand_tabs_in_spans(&virtual_spans, TAB_WIDTH);

                let virtual_width = spans_width(&virtual_spans);
                max_line_width = max_line_width.max(virtual_width);

                let virtual_wrap = if app.line_wrap {
                    wrap_count_for_spans(&virtual_spans, visible_width)
                } else {
                    1
                };

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
                ]));
                if app.line_wrap && virtual_wrap > 1 {
                    for _ in 1..virtual_wrap {
                        gutter_lines.push(Line::from(Span::raw(" ")));
                    }
                }
            }
            prev_visible_hunk = line_hunk;
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
                    if app.line_wrap && debug_wrap > 1 {
                        for _ in 1..debug_wrap {
                            gutter_lines.push(Line::from(Span::raw(" ")));
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
            Paragraph::new(content_lines)
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

    app.update_current_max_line_width(max_line_width);
}

fn render_new_pane(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    hunk_overflow: Option<(bool, bool)>,
) {
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
    let (preview_mode, preview_hunk) = {
        let state = app.multi_diff.current_navigator().state();
        (state.hunk_preview_mode, state.current_hunk)
    };
    let pending_insert_only = if app.stepping {
        app.pending_insert_only_in_current_hunk()
    } else {
        0
    };
    let show_virtual = app.allow_virtual_lines();
    let pending_text = if show_virtual && pending_insert_only > 0 {
        Some(pending_tail_text(pending_insert_only))
    } else {
        None
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(pending) = pending_text {
        parts.push(pending);
    }
    if let Some(hint) = app.last_step_hint_text() {
        parts.push(hint.to_string());
    }
    if let Some(hint) = app.hunk_edge_hint_text() {
        parts.push(hint.to_string());
    }
    let virtual_text = if show_virtual && !parts.is_empty() {
        if pending_insert_only == 0 {
            if let Some(first) = parts.first_mut() {
                *first = format!("... {first}");
            }
        }
        Some(parts.join(" • "))
    } else {
        None
    };
    let (overflow_above, overflow_below) = if virtual_text.is_some() {
        hunk_overflow.unwrap_or((false, false))
    } else {
        (false, false)
    };
    let new_visible = |line: &ViewLine| -> bool {
        let old_present = line.old_line.is_some();
        let new_present = line.new_line.is_some()
            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete);
        new_present || (app.split_align_lines && old_present)
    };
    let cursor_in_target = view_lines
        .iter()
        .any(|line| line.is_primary_active && line.hunk_index == Some(preview_hunk));
    let cursor_visible = if app.line_wrap {
        cursor_in_target
    } else {
        let mut display_idx = 0usize;
        let mut cursor_display = None;
        for line in view_lines.iter() {
            if !new_visible(line) {
                continue;
            }
            if line.is_primary_active {
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
    let visible_indices: Vec<usize> = view_lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| if new_visible(line) { Some(idx) } else { None })
        .collect();
    let mut next_visible_hunk: Vec<Option<usize>> = vec![None; view_lines.len()];
    let mut next_hunk: Option<usize> = None;
    for idx in visible_indices.iter().rev() {
        next_visible_hunk[*idx] = next_hunk;
        if view_lines[*idx].hunk_index.is_some() {
            next_hunk = view_lines[*idx].hunk_index;
        }
    }
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
        line.is_primary_active && line.hunk_index == Some(preview_hunk)
    });
    let cursor_at_first = cursor_idx
        .map(|idx| prev_visible_hunk_map[idx] != Some(preview_hunk))
        .unwrap_or(false);
    let cursor_at_last = cursor_idx
        .map(|idx| next_visible_hunk[idx] != Some(preview_hunk))
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
    let mut prev_visible_hunk: Option<usize> = None;
    let mut virtual_inserted = false;

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
    let mut max_line_width: usize = 0;

    for (idx, view_line) in view_lines.iter().enumerate() {
        let old_present = view_line.old_line.is_some();
        let new_present = view_line.new_line.is_some()
            && !matches!(view_line.kind, LineKind::Deleted | LineKind::PendingDelete);
        if !(new_present || (app.split_align_lines && old_present)) {
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

        let line_hunk = view_line.hunk_index;
        let is_first_in_hunk = line_hunk.is_some() && prev_visible_hunk != line_hunk;
        let is_last_in_hunk = line_hunk.is_some() && next_visible_hunk[idx] != line_hunk;
        if let Some(text) = virtual_text.as_ref() {
            if !virtual_inserted
                && !prefer_cursor
                && force_top
                && line_hunk == Some(preview_hunk)
                && is_first_in_hunk
            {
                push_virtual_line_new(
                    text,
                    app,
                    visible_width,
                    &mut max_line_width,
                    &mut content_lines,
                    &mut gutter_lines,
                    &mut marker_lines,
                );
                virtual_inserted = true;
            }
        }

        if !new_present {
            let wrap_count = if app.line_wrap {
                split_old_line_wrap_count(app, view_line, visible_width)
            } else {
                1
            };
            let fill_span = align_fill_span(app, visible_width);
            let gutter_fill = align_fill_gutter_span(app, 4);
            let sign_fill = align_fill_gutter_span(app, 1);
            gutter_lines.push(Line::from(vec![gutter_fill, sign_fill]));
            content_lines.push(Line::from(fill_span.clone()));
            marker_lines.push(Line::from(Span::raw(" ")));
            if app.line_wrap && wrap_count > 1 {
                for _ in 1..wrap_count {
                    let gutter_fill = align_fill_gutter_span(app, 4);
                    let sign_fill = align_fill_gutter_span(app, 1);
                    gutter_lines.push(Line::from(vec![gutter_fill, sign_fill]));
                    content_lines.push(Line::from(fill_span.clone()));
                    marker_lines.push(Line::from(Span::raw(" ")));
                }
            }
            if let Some(text) = virtual_text.as_ref() {
                if !virtual_inserted
                    && !force_top
                    && line_hunk == Some(preview_hunk)
                    && is_last_in_hunk
                {
                    push_virtual_line_new(
                        text,
                        app,
                        visible_width,
                        &mut max_line_width,
                        &mut content_lines,
                        &mut gutter_lines,
                        &mut marker_lines,
                    );
                    virtual_inserted = true;
                }
            }
            prev_visible_hunk = line_hunk;
            line_idx += 1;
            continue;
        }

        if let Some(new_line_num) = view_line.new_line {
            let line_num_str = format!("{:4}", new_line_num);
            let bg_kind = split_new_bg_kind(view_line.kind);
            let line_num_style = line_num_style_for_kind(bg_kind, app);
            let line_bg_gutter = if app.diff_bg {
                diff_line_bg(bg_kind, &app.theme)
            } else {
                None
            };

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

            // Build gutter line
            let mut gutter_spans = vec![
                Span::styled(line_num_str, line_num_style),
                Span::styled(" ", Style::default()),
            ];
            if let Some(bg) = line_bg_gutter {
                gutter_spans = gutter_spans
                    .into_iter()
                    .map(|span| Span::styled(span.content, span.style.bg(bg)))
                    .collect();
            }
            gutter_lines.push(Line::from(gutter_spans));

            let display_idx = line_idx;
            // Build content line
            let mut content_spans: Vec<Span<'static>> = Vec::new();
            let mut used_syntax = false;
            let pure_context = matches!(view_line.kind, LineKind::Context)
                && !view_line.has_changes
                && !view_line.is_active_change
                && view_line
                    .spans
                    .iter()
                    .all(|span| matches!(span.kind, ViewSpanKind::Equal));
            let wants_diff_syntax =
                app.diff_fg == DiffForegroundMode::Syntax && app.syntax_enabled();
            let in_preview_hunk =
                preview_mode && view_line.hunk_index == Some(preview_hunk) && wants_diff_syntax;
            let preview_modified = in_preview_hunk
                && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify);
            let highlight_inline = matches!(
                app.diff_highlight,
                DiffHighlightMode::Text | DiffHighlightMode::Word
            );
            let modified_line =
                matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify);
            let can_use_diff_syntax = wants_diff_syntax && !modified_line;
            if app.syntax_enabled()
                && !preview_modified
                && !view_line.is_active_change
                && (pure_context || can_use_diff_syntax || in_preview_hunk)
            {
                let use_old = view_line.kind == LineKind::Context && view_line.has_changes;
                let side = if use_old {
                    SyntaxSide::Old
                } else {
                    SyntaxSide::New
                };
                let line_num = if use_old {
                    view_line.old_line.or(Some(new_line_num))
                } else {
                    Some(new_line_num)
                };
                if let Some(spans) = app.syntax_spans_for_line(side, line_num) {
                    content_spans = spans;
                    used_syntax = true;
                }
            }
            if !used_syntax {
                let mut rebuilt_spans: Vec<ViewSpan> = Vec::new();
                let is_applied = app
                    .multi_diff
                    .current_navigator()
                    .state()
                    .applied_changes
                    .contains(&view_line.change_id);
                let show_inline = view_line.old_line.is_some()
                    && view_line.new_line.is_some()
                    && (view_line.is_active || is_applied || (highlight_inline && modified_line));
                let spans = if show_inline {
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
                    let highlight_allowed =
                        matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
                            || !view_line.is_active
                            || (view_line.is_active
                                && !matches!(app.diff_highlight, DiffHighlightMode::None)
                                && (!app.diff_bg || app.diff_fg == DiffForegroundMode::Theme));
                    let style = get_new_span_style(
                        view_span.kind,
                        view_line.kind,
                        view_line.is_active,
                        app,
                        highlight_allowed,
                    );
                    content_spans.push(Span::styled(view_span.text.clone(), style));
                }
            }
            let line_bg_line = if app.diff_bg {
                diff_line_bg(bg_kind, &app.theme)
            } else {
                None
            };
            if let Some(bg) = line_bg_line {
                content_spans = apply_line_bg(content_spans, bg, visible_width, app.line_wrap);
            }

            let highlight_allowed =
                matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
                    || !view_line.is_active
                    || (view_line.is_active
                        && !matches!(app.diff_highlight, DiffHighlightMode::None)
                        && (!app.diff_bg || app.diff_fg == DiffForegroundMode::Theme));
            if highlight_allowed
                && !app.diff_bg
                && matches!(
                    app.diff_highlight,
                    DiffHighlightMode::Text | DiffHighlightMode::Word
                )
                && used_syntax
            {
                if let Some(bg) = diff_line_bg(bg_kind, &app.theme) {
                    content_spans = apply_spans_bg(content_spans, bg);
                }
            }

            if highlight_allowed {
                if !app.diff_bg {
                    if app.diff_highlight == DiffHighlightMode::Text {
                        if !view_line.is_active {
                            content_spans =
                                clear_leading_ws_bg(content_spans, Some(app.theme.diff_context));
                        }
                    } else if app.diff_highlight == DiffHighlightMode::Word {
                        content_spans = clear_leading_ws_bg(content_spans, None);
                    }
                } else if app.diff_highlight == DiffHighlightMode::Word {
                    content_spans = super::replace_leading_ws_bg(content_spans, None, line_bg_line);
                }
            }

            let mut italic_line = false;
            if app.syntax_enabled() {
                if used_syntax {
                    italic_line = super::line_is_italic(&content_spans);
                } else {
                    let use_old = view_line.kind == LineKind::Context && view_line.has_changes;
                    let side = if use_old {
                        SyntaxSide::Old
                    } else {
                        SyntaxSide::New
                    };
                    let line_num = if use_old {
                        view_line.old_line.or(Some(new_line_num))
                    } else {
                        Some(new_line_num)
                    };
                    if let Some(spans) = app.syntax_spans_for_line(side, line_num) {
                        italic_line = super::line_is_italic(&spans);
                    }
                }
            }

            let line_text = spans_to_text(&content_spans);
            let is_active_match = app.search_target() == Some(display_idx)
                && has_query
                && line_text.to_ascii_lowercase().contains(&query);
            content_spans = app.highlight_search_spans(content_spans, &line_text, is_active_match);
            if italic_line {
                content_spans = super::apply_italic_spans(content_spans);
            }

            content_spans = expand_tabs_in_spans(&content_spans, TAB_WIDTH);

            let line_width = spans_width(&content_spans);
            max_line_width = max_line_width.max(line_width);

            let wrap_count = if app.line_wrap {
                wrap_count_for_spans(&content_spans, visible_width)
            } else {
                1
            };
            let mut display_spans = content_spans;
            if !app.line_wrap {
                display_spans = slice_spans(&display_spans, app.horizontal_scroll, visible_width);
                if app.diff_bg {
                    if let Some(bg) = diff_line_bg(bg_kind, &app.theme) {
                        display_spans = pad_spans_bg(display_spans, bg, visible_width);
                    }
                }
            }
            content_lines.push(Line::from(display_spans));

            // Build marker line
            marker_lines.push(Line::from(Span::styled(active_marker, active_style)));
            if app.line_wrap && wrap_count > 1 {
                for _ in 1..wrap_count {
                    gutter_lines.push(Line::from(Span::raw(" ")));
                    marker_lines.push(Line::from(Span::raw(" ")));
                }
            }

            if let Some(text) = virtual_text.as_ref() {
                if !virtual_inserted
                    && prefer_cursor
                    && line_hunk == Some(preview_hunk)
                    && view_line.is_primary_active
                {
                    push_virtual_line_new(
                        text,
                        app,
                        visible_width,
                        &mut max_line_width,
                        &mut content_lines,
                        &mut gutter_lines,
                        &mut marker_lines,
                    );
                    virtual_inserted = true;
                }
            }
            if let Some(text) = virtual_text.as_ref() {
                if !virtual_inserted
                    && !prefer_cursor
                    && !force_top
                    && line_hunk == Some(preview_hunk)
                    && is_last_in_hunk
                {
                    push_virtual_line_new(
                        text,
                        app,
                        visible_width,
                        &mut max_line_width,
                        &mut content_lines,
                        &mut gutter_lines,
                        &mut marker_lines,
                    );
                    virtual_inserted = true;
                }
            }

            if let Some(hint_text) = app.step_edge_hint_for_change(view_line.change_id) {
                let virtual_style = Style::default()
                    .fg(app.theme.text_muted)
                    .add_modifier(Modifier::ITALIC);
                let mut virtual_spans = vec![Span::styled(hint_text.to_string(), virtual_style)];
                virtual_spans = expand_tabs_in_spans(&virtual_spans, TAB_WIDTH);

                let virtual_width = spans_width(&virtual_spans);
                max_line_width = max_line_width.max(virtual_width);

                let virtual_wrap = if app.line_wrap {
                    wrap_count_for_spans(&virtual_spans, visible_width)
                } else {
                    1
                };

                let mut display_virtual = virtual_spans;
                if !app.line_wrap {
                    display_virtual =
                        slice_spans(&display_virtual, app.horizontal_scroll, visible_width);
                }
                content_lines.push(Line::from(display_virtual));
                gutter_lines.push(Line::from(vec![Span::raw("    "), Span::raw(" ")]));
                marker_lines.push(Line::from(Span::raw(" ")));
                if app.line_wrap && virtual_wrap > 1 {
                    for _ in 1..virtual_wrap {
                        gutter_lines.push(Line::from(Span::raw(" ")));
                        marker_lines.push(Line::from(Span::raw(" ")));
                    }
                }
            }

            prev_visible_hunk = line_hunk;
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
                    if app.line_wrap && debug_wrap > 1 {
                        for _ in 1..debug_wrap {
                            gutter_lines.push(Line::from(Span::raw(" ")));
                            marker_lines.push(Line::from(Span::raw(" ")));
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
            Paragraph::new(content_lines)
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

    app.update_current_max_line_width(max_line_width);
}

fn split_wrap_display_metrics(
    app: &mut App,
    view: &[ViewLine],
    old_width: usize,
    new_width: usize,
    scroll_offset: usize,
    step_direction: StepDirection,
    align_lines: bool,
) -> (usize, Option<usize>) {
    let mut old_len = 0usize;
    let mut new_len = 0usize;
    let mut old_primary_idx: Option<usize> = None;
    let mut new_primary_idx: Option<usize> = None;
    let mut old_fallback_idx: Option<usize> = None;
    let mut new_fallback_idx: Option<usize> = None;

    for line in view {
        let old_present = line.old_line.is_some();
        let new_present = line.new_line.is_some()
            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete);

        let old_wrap = if old_present {
            split_old_line_wrap_count(app, line, old_width)
        } else if align_lines && new_present {
            split_new_line_wrap_count(app, line, old_width)
        } else {
            0
        };
        let new_wrap = if new_present {
            split_new_line_wrap_count(app, line, new_width)
        } else if align_lines && old_present {
            split_old_line_wrap_count(app, line, new_width)
        } else {
            0
        };

        if old_wrap > 0 {
            if old_present {
                if line.is_primary_active {
                    old_primary_idx = Some(old_len);
                } else if line.is_active && old_fallback_idx.is_none() {
                    old_fallback_idx = Some(old_len);
                }
            }
            old_len += old_wrap;
        }
        if new_wrap > 0 {
            if new_present {
                if line.is_primary_active {
                    new_primary_idx = Some(new_len);
                } else if line.is_active && new_fallback_idx.is_none() {
                    new_fallback_idx = Some(new_len);
                }
            }
            new_len += new_wrap;
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

fn split_hunk_overflow_wrapped(
    app: &mut App,
    view: &[ViewLine],
    hunk_idx: usize,
    viewport_height: usize,
    old_width: usize,
    new_width: usize,
) -> Option<(bool, bool)> {
    let mut old_idx = 0usize;
    let mut new_idx = 0usize;
    let mut old_start: Option<usize> = None;
    let mut old_end: Option<usize> = None;
    let mut new_start: Option<usize> = None;
    let mut new_end: Option<usize> = None;

    for line in view {
        let old_present = line.old_line.is_some();
        let new_present = line.new_line.is_some()
            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete);

        let old_wrap = if old_present {
            split_old_line_wrap_count(app, line, old_width)
        } else if app.split_align_lines && new_present {
            split_new_line_wrap_count(app, line, old_width)
        } else {
            0
        };
        let new_wrap = if new_present {
            split_new_line_wrap_count(app, line, new_width)
        } else if app.split_align_lines && old_present {
            split_old_line_wrap_count(app, line, new_width)
        } else {
            0
        };

        if old_wrap > 0 {
            if line.hunk_index == Some(hunk_idx) {
                if old_start.is_none() {
                    old_start = Some(old_idx);
                }
                old_end = Some(old_idx.saturating_add(old_wrap.saturating_sub(1)));
            }
            old_idx = old_idx.saturating_add(old_wrap);
        }
        if new_wrap > 0 {
            if line.hunk_index == Some(hunk_idx) {
                if new_start.is_none() {
                    new_start = Some(new_idx);
                }
                new_end = Some(new_idx.saturating_add(new_wrap.saturating_sub(1)));
            }
            new_idx = new_idx.saturating_add(new_wrap);
        }
    }

    let old_bounds = old_start.zip(old_end);
    let new_bounds = new_start.zip(new_end);
    let (start, end) = match (old_bounds, new_bounds) {
        (Some(old), Some(new)) => {
            let old_dist = (old.0 as isize - app.scroll_offset as isize).abs();
            let new_dist = (new.0 as isize - app.scroll_offset as isize).abs();
            if old_dist < new_dist {
                old
            } else {
                new
            }
        }
        (Some(old), None) => old,
        (None, Some(new)) => new,
        (None, None) => return None,
    };

    let visible_start = app.scroll_offset;
    let visible_end = app
        .scroll_offset
        .saturating_add(viewport_height.saturating_sub(1));
    Some((start < visible_start, end > visible_end))
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

fn get_old_span_style(
    kind: ViewSpanKind,
    _line_kind: LineKind,
    is_active: bool,
    app: &App,
    highlight_allowed: bool,
) -> Style {
    let theme = &app.theme;
    let use_bg = highlight_allowed
        && matches!(
            app.diff_highlight,
            DiffHighlightMode::Text | DiffHighlightMode::Word
        );
    let removed_bg = if use_bg {
        super::boost_inline_bg(app, theme.diff_removed_bg, theme.delete_base())
    } else {
        None
    };
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
                    removed_bg,
                )
            } else {
                super::delete_style(
                    AnimationPhase::Idle,
                    0.0,
                    false,
                    false,
                    theme.delete_base(),
                    theme.delete_dim(),
                    removed_bg,
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
                    removed_bg,
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
                    removed_bg,
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
    highlight_allowed: bool,
) -> Style {
    let theme = &app.theme;
    let use_bg = highlight_allowed
        && matches!(
            app.diff_highlight,
            DiffHighlightMode::Text | DiffHighlightMode::Word
        );
    let added_bg = if use_bg {
        super::boost_inline_bg(app, theme.diff_added_bg, theme.insert_base())
    } else {
        None
    };
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
                added_bg,
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
                    added_bg,
                )
            } else {
                // Non-active pending insert: show dim
                let mut style = Style::default().fg(theme.insert_dim());
                if let Some(bg) = added_bg {
                    style = style.bg(bg);
                }
                style
            }
        }
        ViewSpanKind::PendingDelete => {
            let mut style = Style::default().fg(theme.delete_dim());
            let removed_bg = if use_bg { theme.diff_removed_bg } else { None };
            if let Some(bg) = removed_bg {
                style = style.bg(bg);
            }
            style
        }
    }
}
