//! Single pane view - morphs from old to new state

use super::{
    apply_line_bg, apply_spans_bg, clear_leading_ws_bg, diff_line_bg, expand_tabs_in_spans,
    pad_spans_bg, pending_tail_text, render_empty_state, slice_spans, spans_to_text, spans_width,
    truncate_text, wrap_count_for_spans, wrap_count_for_text, TAB_WIDTH,
};
use crate::app::{AnimationPhase, App};
use crate::color;
use crate::config::{DiffForegroundMode, DiffHighlightMode, ModifiedStepMode};
use crate::syntax::SyntaxSide;
use oyo_core::{Change, ChangeKind, LineKind, ViewLine, ViewSpan, ViewSpanKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};

/// Width of the fixed line number gutter (marker + line num + prefix + space)
const GUTTER_WIDTH: u16 = 8; // "▶1234 + "

fn hunk_overflow_wrapped_unified(
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
        let text = super::view_spans_to_text(&line.spans);
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

fn build_inline_modified_spans(
    change: &Change,
    app: &App,
    include_equal: bool,
    use_animation: bool,
) -> Option<Vec<Span<'static>>> {
    let mut spans = Vec::new();
    let mut has_old = false;
    let mut has_new = false;
    let (phase, progress, backward) = if use_animation {
        (
            app.animation_phase,
            app.animation_progress,
            app.is_backward_animation(),
        )
    } else {
        (AnimationPhase::Idle, 1.0, false)
    };
    let use_bg = matches!(
        app.diff_highlight,
        DiffHighlightMode::Text | DiffHighlightMode::Word
    );
    let added_bg = if use_bg {
        super::boost_inline_bg(app, app.theme.diff_added_bg, app.theme.insert_base())
    } else {
        None
    };
    let removed_bg = if use_bg {
        super::boost_inline_bg(app, app.theme.diff_removed_bg, app.theme.delete_base())
    } else {
        None
    };
    let delete_style = super::delete_style(
        phase,
        progress,
        backward,
        app.strikethrough_deletions,
        app.theme.delete_base(),
        app.theme.diff_context,
        removed_bg,
    );
    let insert_style = super::insert_style(
        phase,
        progress,
        backward,
        app.theme.insert_base(),
        app.theme.insert_dim(),
        added_bg,
    );
    let context_style = Style::default().fg(app.theme.diff_context);
    for span in &change.spans {
        match span.kind {
            ChangeKind::Equal => {
                if !include_equal {
                    continue;
                }
                spans.push(Span::styled(span.text.clone(), context_style));
            }
            ChangeKind::Delete => {
                has_old = true;
                let text = &span.text;
                if app.strikethrough_deletions {
                    let trimmed = text.trim_start();
                    let leading_ws_len = text.len() - trimmed.len();
                    if leading_ws_len > 0 && !trimmed.is_empty() {
                        let ws_style = delete_style.remove_modifier(Modifier::CROSSED_OUT);
                        spans.push(Span::styled(text[..leading_ws_len].to_string(), ws_style));
                        spans.push(Span::styled(trimmed.to_string(), delete_style));
                    } else {
                        spans.push(Span::styled(text.to_string(), delete_style));
                    }
                } else {
                    spans.push(Span::styled(text.to_string(), delete_style));
                }
            }
            ChangeKind::Insert => {
                has_new = true;
                spans.push(Span::styled(span.text.clone(), insert_style));
            }
            ChangeKind::Replace => {
                has_old = true;
                has_new = true;
                let text = &span.text;
                if app.strikethrough_deletions {
                    let trimmed = text.trim_start();
                    let leading_ws_len = text.len() - trimmed.len();
                    if leading_ws_len > 0 && !trimmed.is_empty() {
                        let ws_style = delete_style.remove_modifier(Modifier::CROSSED_OUT);
                        spans.push(Span::styled(text[..leading_ws_len].to_string(), ws_style));
                        spans.push(Span::styled(trimmed.to_string(), delete_style));
                    } else {
                        spans.push(Span::styled(text.to_string(), delete_style));
                    }
                } else {
                    spans.push(Span::styled(text.to_string(), delete_style));
                }
                spans.push(Span::styled(
                    span.new_text.clone().unwrap_or_else(|| span.text.clone()),
                    insert_style,
                ));
            }
        }
    }

    if has_old || has_new {
        Some(spans)
    } else {
        None
    }
}

fn build_modified_only_spans(
    change: &Change,
    app: &App,
    use_animation: bool,
) -> Option<Vec<Span<'static>>> {
    let mut spans = Vec::new();
    let (phase, progress, backward) = if use_animation {
        (
            app.animation_phase,
            app.animation_progress,
            app.is_backward_animation(),
        )
    } else {
        (AnimationPhase::Idle, 1.0, false)
    };
    let use_bg = matches!(
        app.diff_highlight,
        DiffHighlightMode::Text | DiffHighlightMode::Word
    );
    let modified_bg = if use_bg {
        super::boost_inline_bg(app, app.theme.diff_modified_bg, app.theme.modify_base())
    } else {
        None
    };
    let modify_style = super::modify_style(
        phase,
        progress,
        backward,
        app.theme.modify_base(),
        app.theme.diff_context,
        modified_bg,
    );
    let context_style = Style::default().fg(app.theme.diff_context);
    for span in &change.spans {
        match span.kind {
            ChangeKind::Equal => {
                spans.push(Span::styled(span.text.clone(), context_style));
            }
            ChangeKind::Insert => {
                spans.push(Span::styled(span.text.clone(), modify_style));
            }
            ChangeKind::Replace => {
                spans.push(Span::styled(
                    span.new_text.clone().unwrap_or_else(|| span.text.clone()),
                    modify_style,
                ));
            }
            ChangeKind::Delete => {}
        }
    }
    if spans.is_empty() {
        None
    } else {
        Some(spans)
    }
}

/// Render the unified pane morphing view
pub fn render_unified_pane(frame: &mut Frame, app: &mut App, area: Rect) {
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
    app.multi_diff
        .current_navigator()
        .set_show_hunk_extent_while_stepping(app.stepping);
    let view_lines = app
        .multi_diff
        .current_navigator()
        .current_view_with_frame(animation_frame);
    let blame_extra_rows = if matches!(app.view_mode, crate::app::ViewMode::Blame) {
        app.blame_extra_rows.clone()
    } else {
        None
    };
    let extra_total = blame_extra_rows
        .as_ref()
        .map(|rows| rows.iter().copied().sum::<usize>())
        .unwrap_or(0);
    if !app.line_wrap {
        app.clamp_scroll(
            view_lines.len().saturating_add(extra_total),
            visible_height,
            app.allow_overscroll(),
        );
    }
    let debug_target = app.syntax_scope_target(&view_lines);
    let mut bg_lines: Option<Vec<Line<'static>>> = if app.line_wrap && app.diff_bg {
        Some(Vec::new())
    } else {
        None
    };

    // Split area into gutter (fixed) and content (scrollable)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(GUTTER_WIDTH), Constraint::Min(0)])
        .split(area);

    let gutter_area = chunks[0];
    let content_area = chunks[1];

    // Build separate line number and content lines
    let mut gutter_lines: Vec<Line> = Vec::new();
    let mut content_lines: Vec<Line> = Vec::new();
    let mut max_line_width: usize = 0;
    let wrap_width = visible_width;
    let mut display_len = if app.line_wrap {
        0
    } else {
        view_lines.len().saturating_add(extra_total)
    };
    let mut primary_display_idx: Option<usize> = None;
    let mut active_display_idx: Option<usize> = None;

    let query = app.search_query().trim().to_ascii_lowercase();
    let has_query = !query.is_empty();
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
    let cursor_in_target = view_lines
        .iter()
        .any(|line| line.is_primary_active && line.hunk_index == Some(preview_hunk));
    let cursor_visible = if app.line_wrap {
        app.cursor_visible_in_wrap(visible_height)
    } else {
        view_lines
            .iter()
            .position(|line| line.is_primary_active)
            .map(|idx| {
                idx >= app.scroll_offset && idx < app.scroll_offset.saturating_add(visible_height)
            })
            .unwrap_or(false)
    };
    let (overflow_above, overflow_below) = if virtual_text.is_some() {
        if app.line_wrap {
            hunk_overflow_wrapped_unified(
                &view_lines,
                preview_hunk,
                wrap_width,
                app.scroll_offset,
                visible_height,
            )
            .unwrap_or((false, false))
        } else {
            app.hunk_hint_overflow(preview_hunk, visible_height)
                .unwrap_or((false, false))
        }
    } else {
        (false, false)
    };
    let mut next_visible_hunk: Vec<Option<usize>> = vec![None; view_lines.len()];
    let mut next_hunk: Option<usize> = None;
    for (idx, line) in view_lines.iter().enumerate().rev() {
        next_visible_hunk[idx] = next_hunk;
        if line.hunk_index.is_some() {
            next_hunk = line.hunk_index;
        }
    }
    let mut prev_visible_hunk_map: Vec<Option<usize>> = vec![None; view_lines.len()];
    let mut prev_hunk: Option<usize> = None;
    for (idx, line) in view_lines.iter().enumerate() {
        prev_visible_hunk_map[idx] = prev_hunk;
        if line.hunk_index.is_some() {
            prev_hunk = line.hunk_index;
        }
    }
    let cursor_idx = view_lines
        .iter()
        .position(|line| line.is_primary_active && line.hunk_index == Some(preview_hunk));
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
    for (idx, view_line) in view_lines.iter().enumerate() {
        // When wrapping, we need all lines for proper wrap calculation
        // When not wrapping, skip lines before scroll offset
        if !app.line_wrap && idx < app.scroll_offset {
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
                if let Some(bg_lines) = bg_lines.as_mut() {
                    super::push_wrapped_bg_line(bg_lines, wrap_width, virtual_wrap, None);
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

        let line_num = view_line.old_line.or(view_line.new_line).unwrap_or(0);
        let line_num_str = format!("{:4}", line_num);

        // Line number color from theme - use gradient base for diff types
        let insert_base = color::gradient_color(&app.theme.insert, 0.5);
        let delete_base = color::gradient_color(&app.theme.delete, 0.5);
        let modify_base = color::gradient_color(&app.theme.modify, 0.5);

        let line_num_style = match view_line.kind {
            LineKind::Context => Style::default().fg(app.theme.diff_line_number),
            LineKind::Inserted | LineKind::PendingInsert => {
                Style::default().fg(Color::Rgb(insert_base.r, insert_base.g, insert_base.b))
            }
            LineKind::Deleted | LineKind::PendingDelete => {
                Style::default().fg(Color::Rgb(delete_base.r, delete_base.g, delete_base.b))
            }
            LineKind::Modified | LineKind::PendingModify => {
                Style::default().fg(Color::Rgb(modify_base.r, modify_base.g, modify_base.b))
            }
        };

        let line_bg_gutter = if app.diff_bg {
            diff_line_bg(view_line.kind, &app.theme)
        } else {
            None
        };

        // Sign column should fade with the line animation
        let (mut line_prefix, mut sign_style) = match view_line.kind {
            LineKind::Context => (" ", Style::default().fg(app.theme.diff_line_number)),
            LineKind::Inserted | LineKind::PendingInsert => {
                if view_line.is_active {
                    (
                        "+",
                        super::insert_style(
                            app.animation_phase,
                            app.animation_progress,
                            app.is_backward_animation(),
                            app.theme.insert_base(),
                            app.theme.diff_context,
                            None,
                        ),
                    )
                } else {
                    ("+", Style::default().fg(app.theme.insert_base()))
                }
            }
            LineKind::Deleted | LineKind::PendingDelete => {
                if view_line.is_active {
                    (
                        "-",
                        super::delete_style(
                            app.animation_phase,
                            app.animation_progress,
                            app.is_backward_animation(),
                            false,
                            app.theme.delete_base(),
                            app.theme.diff_context,
                            None,
                        ),
                    )
                } else {
                    ("-", Style::default().fg(app.theme.delete_base()))
                }
            }
            LineKind::Modified | LineKind::PendingModify => {
                if view_line.is_active {
                    (
                        "~",
                        super::modify_style(
                            app.animation_phase,
                            app.animation_progress,
                            app.is_backward_animation(),
                            app.theme.modify_base(),
                            app.theme.diff_context,
                            None,
                        ),
                    )
                } else {
                    ("~", Style::default().fg(app.theme.modify_base()))
                }
            }
        };
        if !app.gutter_signs {
            line_prefix = " ";
            sign_style = Style::default();
        }

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

        // Build gutter line (fixed, no horizontal scroll)
        let mut gutter_spans = vec![
            Span::styled(active_marker, active_style),
            Span::styled(line_num_str, line_num_style),
            Span::styled(" ", Style::default()),
            Span::styled(line_prefix, sign_style),
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

        // Build content line (scrollable)
        let mut content_spans: Vec<Span<'static>> = Vec::new();
        let highlight_allowed =
            matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
                || !view_line.is_active
                || (view_line.is_active
                    && !matches!(app.diff_highlight, DiffHighlightMode::None)
                    && (!app.diff_bg || app.diff_fg == DiffForegroundMode::Theme));
        let mut used_syntax = false;
        let mut used_inline_modified = false;
        let mut peek_spans: Vec<ViewSpan> = Vec::new();
        let mut has_peek = false;
        let peek_mode = app.peek_mode_for_line(view_line);
        if peek_mode == Some(crate::app::PeekMode::Old)
            && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
        {
            if let Some(change) = app
                .multi_diff
                .current_navigator()
                .diff()
                .changes
                .get(view_line.change_id)
            {
                for span in &change.spans {
                    match span.kind {
                        ChangeKind::Equal => peek_spans.push(ViewSpan {
                            text: span.text.clone(),
                            kind: ViewSpanKind::Equal,
                        }),
                        ChangeKind::Delete | ChangeKind::Replace => {
                            peek_spans.push(ViewSpan {
                                text: span.text.clone(),
                                kind: ViewSpanKind::Deleted,
                            });
                        }
                        ChangeKind::Insert => {}
                    }
                }
            }
            if !peek_spans.is_empty() {
                has_peek = true;
            }
        }
        let wants_diff_syntax = app.diff_fg == DiffForegroundMode::Syntax && app.syntax_enabled();
        let in_preview_hunk =
            preview_mode && view_line.hunk_index == Some(preview_hunk) && wants_diff_syntax;
        if !used_inline_modified
            && in_preview_hunk
            && !has_peek
            && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
        {
            let change = {
                let nav = app.multi_diff.current_navigator();
                nav.diff().changes.get(view_line.change_id).cloned()
            };
            if let Some(change) = change {
                if let Some(spans) = build_inline_modified_spans(&change, app, true, true) {
                    content_spans = spans;
                    used_inline_modified = true;
                }
            }
        }

        let pure_context = matches!(view_line.kind, LineKind::Context)
            && !view_line.has_changes
            && !view_line.is_active_change
            && view_line
                .spans
                .iter()
                .all(|span| matches!(span.kind, ViewSpanKind::Equal));
        let can_use_diff_syntax = wants_diff_syntax
            && !has_peek
            && !matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify);
        if !used_inline_modified
            && app.syntax_enabled()
            && !view_line.is_active_change
            && (pure_context || can_use_diff_syntax || in_preview_hunk)
        {
            let use_old = view_line.kind == LineKind::Context && view_line.has_changes;
            let side = if use_old {
                SyntaxSide::Old
            } else if view_line.new_line.is_some() {
                SyntaxSide::New
            } else {
                SyntaxSide::Old
            };
            let line_num = if use_old {
                view_line.old_line
            } else {
                view_line.new_line.or(view_line.old_line)
            };
            if let Some(spans) = app.syntax_spans_for_line(side, line_num) {
                content_spans = spans;
                used_syntax = true;
            }
        }
        if !used_syntax
            && app.stepping
            && view_line.is_active
            && !has_peek
            && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
        {
            let peek_override = app.is_peek_override_for_line(view_line);
            let is_modified_peek =
                peek_override && peek_mode == Some(crate::app::PeekMode::Modified);
            let default_modified_only =
                app.unified_modified_step_mode == ModifiedStepMode::Modified;
            let change = {
                let nav = app.multi_diff.current_navigator();
                nav.diff().changes.get(view_line.change_id).cloned()
            };
            if let Some(change) = change {
                let use_modified_only = if peek_override {
                    is_modified_peek
                } else {
                    default_modified_only
                };
                if use_modified_only {
                    let use_animation = !is_modified_peek;
                    if let Some(spans) = build_modified_only_spans(&change, app, use_animation) {
                        content_spans = spans;
                        used_inline_modified = true;
                    }
                } else if let Some(spans) = build_inline_modified_spans(&change, app, true, true) {
                    content_spans = spans;
                    used_inline_modified = true;
                }
            }
        }

        if !used_syntax && !used_inline_modified {
            let mut rebuilt_spans: Vec<ViewSpan> = Vec::new();
            let spans = if has_peek {
                &peek_spans
            } else if !app.stepping
                && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
            {
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
                            ChangeKind::Delete => rebuilt_spans.push(ViewSpan {
                                text: span.text.clone(),
                                kind: ViewSpanKind::Deleted,
                            }),
                            ChangeKind::Insert => rebuilt_spans.push(ViewSpan {
                                text: span.text.clone(),
                                kind: ViewSpanKind::Inserted,
                            }),
                            ChangeKind::Replace => {
                                rebuilt_spans.push(ViewSpan {
                                    text: span.text.clone(),
                                    kind: ViewSpanKind::Deleted,
                                });
                                rebuilt_spans.push(ViewSpan {
                                    text: span
                                        .new_text
                                        .clone()
                                        .unwrap_or_else(|| span.text.clone()),
                                    kind: ViewSpanKind::Inserted,
                                });
                            }
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

            let is_modified_line =
                matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify);
            let treat_as_context = has_peek || (!app.stepping && is_modified_line);
            let style_line_kind = if treat_as_context {
                LineKind::Context
            } else {
                view_line.kind
            };
            for view_span in spans {
                let mut style = get_span_style(
                    view_span.kind,
                    style_line_kind,
                    view_line.is_active,
                    app,
                    highlight_allowed,
                );
                if !app.stepping && is_modified_line && app.diff_bg {
                    match view_span.kind {
                        ViewSpanKind::Deleted | ViewSpanKind::PendingDelete => {
                            if style.bg.is_none() {
                                if let Some(bg) = app.theme.diff_removed_bg {
                                    style = style.bg(bg);
                                }
                            }
                        }
                        ViewSpanKind::Inserted | ViewSpanKind::PendingInsert => {
                            if style.bg.is_none() {
                                if let Some(bg) = app.theme.diff_added_bg {
                                    style = style.bg(bg);
                                }
                            }
                        }
                        _ => {}
                    }
                }
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
                        // Render leading whitespace without strikethrough
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
            diff_line_bg(view_line.kind, &app.theme)
        } else {
            None
        };
        if let Some(bg) = line_bg_line {
            content_spans = apply_line_bg(content_spans, bg, visible_width, app.line_wrap);
        }

        if highlight_allowed
            && !app.diff_bg
            && matches!(
                app.diff_highlight,
                DiffHighlightMode::Text | DiffHighlightMode::Word
            )
            && used_syntax
        {
            if let Some(bg) = diff_line_bg(view_line.kind, &app.theme) {
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
                } else if view_line.new_line.is_some() {
                    SyntaxSide::New
                } else {
                    SyntaxSide::Old
                };
                let line_num = if use_old {
                    view_line.old_line
                } else {
                    view_line.new_line.or(view_line.old_line)
                };
                if let Some(spans) = app.syntax_spans_for_line(side, line_num) {
                    italic_line = super::line_is_italic(&spans);
                }
            }
        }

        let line_text = spans_to_text(&content_spans);
        let is_active_match = app.search_target() == Some(idx)
            && has_query
            && line_text.to_ascii_lowercase().contains(&query);
        content_spans = app.highlight_search_spans(content_spans, &line_text, is_active_match);
        if italic_line {
            content_spans = super::apply_italic_spans(content_spans);
        }

        if app.line_wrap {
            if view_line.is_primary_active && primary_display_idx.is_none() {
                primary_display_idx = Some(display_len);
            }
            if view_line.is_active && active_display_idx.is_none() {
                active_display_idx = Some(display_len);
            }
        }

        content_spans = expand_tabs_in_spans(&content_spans, TAB_WIDTH);

        // Track max line width for horizontal scroll clamping
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
            if app.diff_bg {
                if let Some(bg) = diff_line_bg(view_line.kind, &app.theme) {
                    display_spans = pad_spans_bg(display_spans, bg, visible_width);
                }
            }
        }

        if let Some(bg_lines) = bg_lines.as_mut() {
            super::push_wrapped_bg_line(bg_lines, wrap_width, wrap_count, line_bg_line);
        }
        content_lines.push(Line::from(display_spans));
        if app.line_wrap && wrap_count > 1 {
            let (wrap_marker, wrap_style) = if view_line.show_hunk_extent {
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
            for _ in 1..wrap_count {
                if let Some(bg) = line_bg_gutter {
                    let pad = " ".repeat(GUTTER_WIDTH as usize - 1);
                    gutter_lines.push(Line::from(vec![
                        Span::styled(wrap_marker, wrap_style),
                        Span::styled(pad, Style::default().bg(bg)),
                    ]));
                } else {
                    gutter_lines.push(Line::from(Span::styled(wrap_marker, wrap_style)));
                }
            }
        }
        let extra_rows = blame_extra_rows
            .as_ref()
            .and_then(|rows| rows.get(idx).copied())
            .unwrap_or(0);
        if extra_rows > 0 {
            if app.line_wrap {
                display_len += extra_rows;
            }
            if let Some(bg_lines) = bg_lines.as_mut() {
                super::push_wrapped_bg_line(bg_lines, wrap_width, extra_rows, None);
            }
            for _ in 0..extra_rows {
                content_lines.push(Line::from(Span::raw("")));
                gutter_lines.push(Line::from(Span::raw(" ")));
            }
        }
        if let Some(text) = virtual_text.as_ref() {
            if !virtual_inserted
                && prefer_cursor
                && line_hunk == Some(preview_hunk)
                && view_line.is_primary_active
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
                if let Some(bg_lines) = bg_lines.as_mut() {
                    super::push_wrapped_bg_line(bg_lines, wrap_width, virtual_wrap, None);
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
                && line_hunk == Some(preview_hunk)
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
                if let Some(bg_lines) = bg_lines.as_mut() {
                    super::push_wrapped_bg_line(bg_lines, wrap_width, virtual_wrap, None);
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
            if let Some(bg_lines) = bg_lines.as_mut() {
                super::push_wrapped_bg_line(bg_lines, wrap_width, virtual_wrap, None);
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
            if let Some(bg_lines) = bg_lines.as_mut() {
                super::push_wrapped_bg_line(bg_lines, wrap_width, virtual_wrap, None);
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

        if let Some((debug_idx, ref label)) = debug_target {
            if debug_idx == idx {
                let debug_text = truncate_text(&format!("  {}", label), visible_width);
                let debug_style = Style::default().fg(app.theme.text_muted);
                let debug_wrap = if app.line_wrap {
                    wrap_count_for_text(&debug_text, wrap_width)
                } else {
                    1
                };
                gutter_lines.push(Line::from(Span::raw(" ")));
                if let Some(bg_lines) = bg_lines.as_mut() {
                    super::push_wrapped_bg_line(bg_lines, wrap_width, debug_wrap, None);
                }
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
        let has_bg_overlay = bg_lines.is_some();
        if let Some(bg_lines) = bg_lines {
            let mut bg_paragraph = Paragraph::new(bg_lines).scroll((app.scroll_offset as u16, 0));
            if let Some(style) = bg_style {
                bg_paragraph = bg_paragraph.style(style);
            }
            frame.render_widget(bg_paragraph, content_area);
        }
        if !has_bg_overlay {
            if let Some(style) = bg_style {
                content_paragraph = content_paragraph.style(style);
            }
        }
        frame.render_widget(content_paragraph, content_area);

        // Render scrollbar (if enabled)
        if app.scrollbar_visible {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));

            let total_lines = if app.line_wrap {
                display_len
            } else {
                view_lines.len()
            };
            let visible_lines = content_area.height as usize;
            if total_lines > visible_lines {
                let mut scrollbar_state =
                    ScrollbarState::new(total_lines).position(app.scroll_offset);

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

fn get_span_style(
    kind: ViewSpanKind,
    line_kind: LineKind,
    is_active: bool,
    app: &App,
    highlight_allowed: bool,
) -> Style {
    let backward = app.is_backward_animation();
    let theme = &app.theme;
    let is_modification = matches!(line_kind, LineKind::Modified | LineKind::PendingModify);
    let highlight_bg = highlight_allowed
        && matches!(
            app.diff_highlight,
            DiffHighlightMode::Text | DiffHighlightMode::Word
        );
    let modified_bg = if highlight_bg {
        super::boost_inline_bg(app, theme.diff_modified_bg, theme.modify_base())
    } else {
        None
    };
    let added_bg = if highlight_bg {
        super::boost_inline_bg(app, theme.diff_added_bg, theme.insert_base())
    } else {
        None
    };
    let removed_bg = if highlight_bg {
        super::boost_inline_bg(app, theme.diff_removed_bg, theme.delete_base())
    } else {
        None
    };

    match kind {
        ViewSpanKind::Equal => Style::default().fg(theme.diff_context),
        ViewSpanKind::Inserted => {
            if is_modification {
                if is_active {
                    return super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        backward,
                        theme.modify_base(),
                        theme.diff_context,
                        modified_bg,
                    );
                }
                let mut style = Style::default().fg(theme.modify_base());
                if let Some(bg) = modified_bg {
                    style = style.bg(bg);
                }
                return style;
            }
            if is_active {
                super::insert_style(
                    app.animation_phase,
                    app.animation_progress,
                    backward,
                    theme.insert_base(),
                    theme.diff_context,
                    added_bg,
                )
            } else {
                super::insert_style(
                    crate::app::AnimationPhase::Idle,
                    1.0,
                    false,
                    theme.insert_base(),
                    theme.diff_context,
                    added_bg,
                )
            }
        }
        ViewSpanKind::Deleted => {
            if is_modification {
                if is_active {
                    return super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        backward,
                        theme.modify_base(),
                        theme.diff_context,
                        modified_bg,
                    );
                }
                let mut style = Style::default().fg(theme.modify_base());
                if let Some(bg) = modified_bg {
                    style = style.bg(bg);
                }
                return style;
            }
            if is_active {
                super::delete_style(
                    app.animation_phase,
                    app.animation_progress,
                    backward,
                    app.strikethrough_deletions,
                    theme.delete_base(),
                    theme.diff_context,
                    removed_bg,
                )
            } else {
                super::delete_style(
                    crate::app::AnimationPhase::Idle,
                    1.0,
                    false,
                    app.strikethrough_deletions,
                    theme.delete_base(),
                    theme.diff_context,
                    removed_bg,
                )
            }
        }
        ViewSpanKind::PendingInsert => {
            if is_modification {
                if is_active {
                    return super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        backward,
                        theme.modify_base(),
                        theme.diff_context,
                        modified_bg,
                    );
                }
                let mut style = Style::default().fg(theme.modify_dim());
                if let Some(bg) = modified_bg {
                    style = style.bg(bg);
                }
                return style;
            }
            if is_active {
                super::insert_style(
                    app.animation_phase,
                    app.animation_progress,
                    backward,
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
            if is_modification {
                if is_active {
                    return super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        backward,
                        theme.modify_base(),
                        theme.diff_context,
                        modified_bg,
                    );
                }
                let mut style = Style::default().fg(theme.modify_dim());
                if let Some(bg) = modified_bg {
                    style = style.bg(bg);
                }
                return style;
            }
            if is_active {
                super::delete_style(
                    app.animation_phase,
                    app.animation_progress,
                    backward,
                    app.strikethrough_deletions,
                    theme.delete_base(),
                    theme.diff_context,
                    removed_bg,
                )
            } else {
                let mut style = Style::default().fg(theme.delete_dim());
                if let Some(bg) = removed_bg {
                    style = style.bg(bg);
                }
                style
            }
        }
    }
}
