//! Single pane view - morphs from old to new state

use super::{render_empty_state, spans_to_text, truncate_text};
use crate::app::App;
use crate::color;
use crate::syntax::SyntaxSide;
use oyo_core::{ChangeKind, LineKind, ViewSpan, ViewSpanKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};

/// Width of the fixed line number gutter (marker + line num + prefix + space)
const GUTTER_WIDTH: u16 = 8; // "▶1234 + "

/// Render the single-pane morphing view
pub fn render_single_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height as usize;
    let visible_width = area.width.saturating_sub(GUTTER_WIDTH) as usize;

    // Clone markers to avoid borrow conflicts
    let primary_marker = app.primary_marker.clone();
    let extent_marker = app.extent_marker.clone();

    app.ensure_active_visible_if_needed(visible_height);
    let animation_frame = app.animation_frame();
    let view_lines = app
        .multi_diff
        .current_navigator()
        .current_view_with_frame(animation_frame);
    app.clamp_scroll(view_lines.len(), visible_height, app.allow_overscroll());
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

    let query = app.search_query().trim().to_ascii_lowercase();
    let has_query = !query.is_empty();
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
        let gutter_spans = vec![
            Span::styled(active_marker, active_style),
            Span::styled(line_num_str, line_num_style),
            Span::styled(" ", Style::default()),
            Span::styled(line_prefix, sign_style),
            Span::styled(" ", Style::default()),
        ];
        gutter_lines.push(Line::from(gutter_spans));

        // Build content line (scrollable)
        let mut content_spans: Vec<Span<'static>> = Vec::new();
        let mut used_syntax = false;
        let mut peek_spans: Vec<ViewSpan> = Vec::new();
        let mut has_peek = false;
        if app.peek_active_for_line(view_line)
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

        let line_text = spans_to_text(&content_spans);
        let is_active_match = app.search_target() == Some(idx)
            && has_query
            && line_text.to_ascii_lowercase().contains(&query);
        content_spans = app.highlight_search_spans(content_spans, &line_text, is_active_match);

        // Track max line width for horizontal scroll clamping
        let line_width: usize = content_spans.iter().map(|s| s.content.len()).sum();
        max_line_width = max_line_width.max(line_width);

        content_lines.push(Line::from(content_spans));

        if let Some((debug_idx, ref label)) = debug_target {
            if debug_idx == idx {
                let debug_text = truncate_text(&format!("  {}", label), visible_width);
                let debug_style = Style::default().fg(app.theme.text_muted);
                gutter_lines.push(Line::from(Span::raw(" ")));
                content_lines.push(Line::from(Span::styled(debug_text, debug_style)));
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

        // Render scrollbar (if enabled)
        if app.scrollbar_visible {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));

            let total_lines = view_lines.len();
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
                    );
                }
                return Style::default().fg(theme.modify_base());
            }
            if is_active {
                super::insert_style(
                    app.animation_phase,
                    app.animation_progress,
                    backward,
                    theme.insert_base(),
                    theme.diff_context,
                )
            } else {
                super::insert_style(
                    crate::app::AnimationPhase::Idle,
                    1.0,
                    false,
                    theme.insert_base(),
                    theme.diff_context,
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
                    );
                }
                return Style::default().fg(theme.modify_base());
            }
            if is_active {
                super::delete_style(
                    app.animation_phase,
                    app.animation_progress,
                    backward,
                    app.strikethrough_deletions,
                    theme.delete_base(),
                    theme.diff_context,
                )
            } else {
                super::delete_style(
                    crate::app::AnimationPhase::Idle,
                    1.0,
                    false,
                    app.strikethrough_deletions,
                    theme.delete_base(),
                    theme.diff_context,
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
                    );
                }
                return Style::default().fg(theme.modify_dim());
            }
            if is_active {
                super::insert_style(
                    app.animation_phase,
                    app.animation_progress,
                    backward,
                    theme.insert_base(),
                    theme.diff_context,
                )
            } else {
                Style::default().fg(theme.insert_dim())
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
                    );
                }
                return Style::default().fg(theme.modify_dim());
            }
            if is_active {
                super::delete_style(
                    app.animation_phase,
                    app.animation_progress,
                    backward,
                    app.strikethrough_deletions,
                    theme.delete_base(),
                    theme.diff_context,
                )
            } else {
                Style::default().fg(theme.delete_dim())
            }
        }
    }
}
