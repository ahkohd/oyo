//! Single pane view - morphs from old to new state

use super::{
    apply_line_bg, apply_spans_bg, clear_leading_ws_bg, diff_line_bg, expand_tabs_in_spans,
    render_empty_state, spans_to_text, spans_width, truncate_text, wrap_count_for_spans,
    wrap_count_for_text, TAB_WIDTH,
};
use crate::app::{AnimationPhase, App};
use crate::color;
use crate::config::{DiffBackgroundMode, DiffForegroundMode, ModifiedStepMode};
use crate::syntax::SyntaxSide;
use oyo_core::{Change, ChangeKind, LineKind, ViewSpan, ViewSpanKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};

/// Width of the fixed line number gutter (marker + line num + prefix + space)
const GUTTER_WIDTH: u16 = 8; // "▶1234 + "

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
    let use_bg = app.diff_bg == DiffBackgroundMode::Text;
    let added_bg = if use_bg {
        app.theme.diff_added_bg
    } else {
        None
    };
    let removed_bg = if use_bg {
        app.theme.diff_removed_bg
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
    let use_bg = app.diff_bg == DiffBackgroundMode::Text;
    let modified_bg = if use_bg {
        app.theme.diff_modified_bg
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

/// Render the single-pane morphing view
pub fn render_single_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height as usize;
    let visible_width = area.width.saturating_sub(GUTTER_WIDTH) as usize;

    // Clone markers to avoid borrow conflicts
    let primary_marker = app.primary_marker.clone();
    let extent_marker = app.extent_marker.clone();

    if app.line_wrap {
        app.handle_search_scroll_if_needed(visible_height);
    } else {
        app.ensure_active_visible_if_needed(visible_height);
    }
    let animation_frame = app.animation_frame();
    let view_lines = app
        .multi_diff
        .current_navigator()
        .current_view_with_frame(animation_frame);
    if !app.line_wrap {
        app.clamp_scroll(view_lines.len(), visible_height, app.allow_overscroll());
    }
    let debug_target = app.syntax_scope_target(&view_lines);

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
    let mut display_len = if app.line_wrap { 0 } else { view_lines.len() };
    let mut primary_display_idx: Option<usize> = None;
    let mut active_display_idx: Option<usize> = None;

    let query = app.search_query().trim().to_ascii_lowercase();
    let has_query = !query.is_empty();
    let (preview_mode, preview_hunk) = {
        let state = app.multi_diff.current_navigator().state();
        (state.hunk_preview_mode, state.current_hunk)
    };
    for (idx, view_line) in view_lines.iter().enumerate() {
        // When wrapping, we need all lines for proper wrap calculation
        // When not wrapping, skip lines before scroll offset
        if !app.line_wrap && idx < app.scroll_offset {
            continue;
        }
        if !app.line_wrap && gutter_lines.len() >= visible_height {
            break;
        }

        let line_num = view_line.old_line.or(view_line.new_line).unwrap_or(0);
        let line_num_str = format!("{:4}", line_num);

        // Line number color from theme - use gradient base for diff types
        let insert_base = color::gradient_color(&app.theme.insert, 0.5);
        let delete_base = color::gradient_color(&app.theme.delete, 0.5);
        let modify_base = color::gradient_color(&app.theme.modify, 0.5);

        let (line_prefix, line_num_style) = match view_line.kind {
            LineKind::Context => (" ", Style::default().fg(app.theme.diff_line_number)),
            LineKind::Inserted => (
                "+",
                Style::default().fg(Color::Rgb(insert_base.r, insert_base.g, insert_base.b)),
            ),
            LineKind::Deleted => (
                "-",
                Style::default().fg(Color::Rgb(delete_base.r, delete_base.g, delete_base.b)),
            ),
            LineKind::Modified => (
                "~",
                Style::default().fg(Color::Rgb(modify_base.r, modify_base.g, modify_base.b)),
            ),
            LineKind::PendingDelete => (
                "-",
                Style::default().fg(Color::Rgb(delete_base.r, delete_base.g, delete_base.b)),
            ),
            LineKind::PendingInsert => (
                "+",
                Style::default().fg(Color::Rgb(insert_base.r, insert_base.g, insert_base.b)),
            ),
            LineKind::PendingModify => (
                "~",
                Style::default().fg(Color::Rgb(modify_base.r, modify_base.g, modify_base.b)),
            ),
        };

        let line_bg_gutter = if app.diff_bg == DiffBackgroundMode::Line {
            diff_line_bg(view_line.kind, &app.theme)
        } else {
            None
        };

        // Sign column should fade with the line animation
        let sign_style = match view_line.kind {
            LineKind::Context => Style::default().fg(app.theme.diff_line_number),
            LineKind::Inserted | LineKind::PendingInsert => {
                if view_line.is_active {
                    super::insert_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        app.theme.insert_base(),
                        app.theme.diff_context,
                        None,
                    )
                } else {
                    Style::default().fg(app.theme.insert_base())
                }
            }
            LineKind::Deleted | LineKind::PendingDelete => {
                if view_line.is_active {
                    super::delete_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        false,
                        app.theme.delete_base(),
                        app.theme.diff_context,
                        None,
                    )
                } else {
                    Style::default().fg(app.theme.delete_base())
                }
            }
            LineKind::Modified | LineKind::PendingModify => {
                if view_line.is_active {
                    super::modify_style(
                        app.animation_phase,
                        app.animation_progress,
                        app.is_backward_animation(),
                        app.theme.modify_base(),
                        app.theme.diff_context,
                        None,
                    )
                } else {
                    Style::default().fg(app.theme.modify_base())
                }
            }
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
                Style::default().fg(app.theme.diff_ext_marker),
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
            && (app.stepping
                || !matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify));
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
            let default_modified_only = app.single_modified_step_mode == ModifiedStepMode::Modified;
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

            let style_line_kind = if has_peek
                || (!app.stepping
                    && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify))
            {
                LineKind::Context
            } else {
                view_line.kind
            };
            for view_span in spans {
                let style =
                    get_span_style(view_span.kind, style_line_kind, view_line.is_active, app);
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

        let line_bg_line = if app.diff_bg == DiffBackgroundMode::Line {
            diff_line_bg(view_line.kind, &app.theme)
        } else {
            None
        };
        if let Some(bg) = line_bg_line {
            content_spans = apply_line_bg(content_spans, bg, visible_width, app.line_wrap);
        }

        if app.diff_bg == DiffBackgroundMode::Text && used_syntax {
            if let Some(bg) = diff_line_bg(view_line.kind, &app.theme) {
                content_spans = apply_spans_bg(content_spans, bg);
            }
        }

        if app.diff_bg == DiffBackgroundMode::Text {
            content_spans = clear_leading_ws_bg(content_spans);
        }

        let line_text = spans_to_text(&content_spans);
        let is_active_match = app.search_target() == Some(idx)
            && has_query
            && line_text.to_ascii_lowercase().contains(&query);
        content_spans = app.highlight_search_spans(content_spans, &line_text, is_active_match);

        if app.line_wrap {
            if view_line.is_primary_active && primary_display_idx.is_none() {
                primary_display_idx = Some(display_len);
            }
            if view_line.is_active && active_display_idx.is_none() {
                active_display_idx = Some(display_len);
            }
        }

        if app.line_wrap {
            content_spans = expand_tabs_in_spans(&content_spans, TAB_WIDTH);
        }

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

        content_lines.push(Line::from(content_spans));
        if app.line_wrap && wrap_count > 1 {
            for _ in 1..wrap_count {
                gutter_lines.push(Line::from(Span::raw(" ")));
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

fn get_span_style(kind: ViewSpanKind, line_kind: LineKind, is_active: bool, app: &App) -> Style {
    let backward = app.is_backward_animation();
    let theme = &app.theme;
    let is_modification = matches!(line_kind, LineKind::Modified | LineKind::PendingModify);
    let use_bg = app.diff_bg == DiffBackgroundMode::Text;
    let added_bg = if use_bg { theme.diff_added_bg } else { None };
    let removed_bg = if use_bg { theme.diff_removed_bg } else { None };
    let modified_bg = if use_bg { theme.diff_modified_bg } else { None };

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
