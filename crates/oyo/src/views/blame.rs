use crate::app::{App, BlameDisplay};
use crate::views::{expand_tabs_in_spans, wrap_count_for_spans, TAB_WIDTH};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use time::OffsetDateTime;

const BLAME_GUTTER_PERCENT: u16 = 32;
const CONTENT_GUTTER_WIDTH: u16 = 8;
const BLAME_BAR: &str = "▌";

pub fn render_blame(frame: &mut Frame, app: &mut App, area: Rect) {
    app.poll_blame_responses();
    if app.current_file_is_binary() {
        super::render_empty_state(frame, area, &app.theme, false, true);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(BLAME_GUTTER_PERCENT),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(area);
    let blame_area = chunks[0];
    let gap_area = chunks[1];
    let content_area = chunks[2];

    let visible_height = area.height as usize;
    let wrap_width = content_area.width.saturating_sub(CONTENT_GUTTER_WIDTH) as usize;
    if !app.line_wrap {
        app.clamp_horizontal_scroll_cached(wrap_width);
    }
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

    let now = OffsetDateTime::now_utc().unix_timestamp();
    let time_bucket = now / 60;
    let text_width = blame_area.width.saturating_sub(2) as usize;
    let file_index = app.multi_diff.selected_index;
    let state = app.multi_diff.current_navigator().state().clone();
    let cache_key = crate::app::BlameRenderKey {
        file_index,
        current_step: state.current_step,
        current_hunk: state.current_hunk,
        hunk_preview_mode: state.hunk_preview_mode,
        preview_from_backward: state.preview_from_backward,
        stepping: app.stepping,
        line_wrap: app.line_wrap,
        wrap_width,
        blame_width: blame_area.width,
        view_len: view_lines.len(),
        animation_frame,
        cache_rev: app.blame_cache_revision,
        time_bucket,
    };

    let rebuild_cache = app
        .blame_render_cache
        .as_ref()
        .map(|cache| cache.key != cache_key)
        .unwrap_or(true);

    if rebuild_cache {
        let mut blame_keys: Vec<Option<String>> = Vec::with_capacity(view_lines.len());
        let mut blame_texts: Vec<Option<String>> = Vec::with_capacity(view_lines.len());
        let mut blame_displays: Vec<Option<BlameDisplay>> = Vec::with_capacity(view_lines.len());

        for view_line in &view_lines {
            if let Some(display) = app.blame_display_for_view_line(view_line, now) {
                blame_keys.push(Some(display.group_key.clone()));
                blame_texts.push(Some(display.text.clone()));
                blame_displays.push(Some(display));
            } else {
                blame_keys.push(None);
                blame_texts.push(None);
                blame_displays.push(None);
            }
        }

        let mut display_texts: Vec<String> = vec![String::new(); view_lines.len()];
        let mut extra_rows_after_line: Vec<usize> = vec![0; view_lines.len()];
        let mut extra_texts_after_line: Vec<Vec<String>> = vec![Vec::new(); view_lines.len()];
        let mut idx = 0usize;
        while idx < view_lines.len() {
            let key = match &blame_keys[idx] {
                Some(key) => key.clone(),
                None => {
                    idx += 1;
                    continue;
                }
            };
            let mut end = idx + 1;
            while end < view_lines.len() && blame_keys[end].as_deref() == Some(&key) {
                end += 1;
            }

            let text = blame_texts[idx].clone().unwrap_or_default();
            let wrapped = wrap_blame_text(&text, text_width);
            let group_len = end - idx;
            for offset in 0..group_len {
                if let Some(segment) = wrapped.get(offset) {
                    display_texts[idx + offset] = segment.clone();
                }
            }
            if wrapped.len() > group_len {
                let extras: Vec<String> = wrapped[group_len..].to_vec();
                let last_idx = end.saturating_sub(1);
                extra_rows_after_line[last_idx] = extras.len();
                extra_texts_after_line[last_idx] = extras;
            }

            idx = end;
        }

        let mut wrap_counts = Vec::with_capacity(view_lines.len());
        if app.line_wrap && wrap_width > 0 {
            for view_line in &view_lines {
                let mut content_spans = vec![Span::raw(view_line.content.clone())];
                content_spans = expand_tabs_in_spans(&content_spans, TAB_WIDTH);
                let wrap_count = wrap_count_for_spans(&content_spans, wrap_width);
                wrap_counts.push(wrap_count.max(1));
            }
        } else {
            wrap_counts.resize(view_lines.len(), 1);
        }

        let mut bar_colors: Vec<Option<Color>> = Vec::with_capacity(view_lines.len());
        for (idx, view_line) in view_lines.iter().enumerate() {
            let display = blame_displays[idx].as_ref();
            let color = app.blame_bar_color_for_view_line(view_line, display);
            bar_colors.push(color);
        }

        app.blame_render_cache = Some(crate::app::BlameRenderCache {
            key: cache_key,
            wrap_counts,
            extra_rows_after_line,
            extra_texts_after_line,
            display_texts,
            bar_colors,
        });
    }

    let extra_rows_clone = app
        .blame_render_cache
        .as_ref()
        .map(|cache| cache.extra_rows_after_line.clone())
        .unwrap_or_default();
    app.blame_extra_rows = Some(extra_rows_clone);
    super::render_unified_pane(frame, app, content_area);
    app.blame_extra_rows = None;

    let cache = match app.blame_render_cache.as_ref() {
        Some(cache) => cache,
        None => return,
    };
    let extra_rows_after_line = cache.extra_rows_after_line.clone();
    let wrap_counts = cache.wrap_counts.clone();

    let mut blame_scroll_offset = app.scroll_offset;
    if !app.line_wrap && app.scroll_offset > 0 {
        let max_idx = app.scroll_offset.min(extra_rows_after_line.len());
        let extra_before = extra_rows_after_line[..max_idx]
            .iter()
            .copied()
            .sum::<usize>();
        blame_scroll_offset = blame_scroll_offset.saturating_add(extra_before);
    }
    let visible_start = blame_scroll_offset;
    let visible_end = visible_start + visible_height.saturating_sub(1);
    let mut display_idx = 0usize;
    let mut visible_flags = Vec::with_capacity(view_lines.len());
    let mut visible_indices = Vec::new();

    for (idx, _view_line) in view_lines.iter().enumerate() {
        let wrap_count = wrap_counts.get(idx).copied().unwrap_or(1);
        let extra_rows = extra_rows_after_line.get(idx).copied().unwrap_or(0);
        let line_start = display_idx;
        let line_end = display_idx
            .saturating_add(wrap_count)
            .saturating_add(extra_rows)
            .saturating_sub(1);
        let line_visible = line_end >= visible_start && line_start <= visible_end;
        visible_flags.push(line_visible);
        if line_visible {
            visible_indices.push(idx);
        }
        display_idx = display_idx
            .saturating_add(wrap_count)
            .saturating_add(extra_rows);
    }

    app.prefetch_blame_for_view(&view_lines, &visible_indices, visible_height);

    let cache = match app.blame_render_cache.as_ref() {
        Some(cache) => cache,
        None => return,
    };
    let extra_texts_after_line = &cache.extra_texts_after_line;
    let display_texts = &cache.display_texts;
    let bar_colors = &cache.bar_colors;

    let mut blame_lines: Vec<Line> = Vec::new();
    for (idx, _view_line) in view_lines.iter().enumerate() {
        let wrap_count = wrap_counts.get(idx).copied().unwrap_or(1);
        let line_visible = visible_flags[idx];
        let bar_span = match bar_colors.get(idx).copied().flatten() {
            Some(color) => Span::styled(BLAME_BAR, Style::default().fg(color)),
            None => Span::raw(" "),
        };
        let text_style = Style::default().fg(app.theme.text_muted);
        if line_visible {
            let text = display_texts.get(idx).cloned().unwrap_or_default();
            blame_lines.push(Line::from(vec![
                bar_span.clone(),
                Span::raw(" "),
                Span::styled(text, text_style),
            ]));
        } else {
            blame_lines.push(Line::from(Span::raw(" ")));
        }
        if app.line_wrap && wrap_count > 1 {
            for _ in 1..wrap_count {
                if line_visible {
                    blame_lines.push(Line::from(vec![
                        bar_span.clone(),
                        Span::raw(" "),
                        Span::raw(""),
                    ]));
                } else {
                    blame_lines.push(Line::from(Span::raw(" ")));
                }
            }
        }
        let extra_rows = extra_rows_after_line.get(idx).copied().unwrap_or(0);
        if extra_rows > 0 {
            if line_visible {
                for extra_text in &extra_texts_after_line[idx] {
                    blame_lines.push(Line::from(vec![
                        bar_span.clone(),
                        Span::raw(" "),
                        Span::styled(extra_text.clone(), text_style),
                    ]));
                }
            } else {
                for _ in 0..extra_rows {
                    blame_lines.push(Line::from(Span::raw(" ")));
                }
            }
        }
    }

    let mut blame_paragraph = Paragraph::new(blame_lines);
    if blame_scroll_offset > 0 {
        blame_paragraph = blame_paragraph.scroll((blame_scroll_offset as u16, 0));
    }
    if let Some(bg) = app.theme.background {
        blame_paragraph = blame_paragraph.style(Style::default().bg(bg));
    }
    frame.render_widget(blame_paragraph, blame_area);

    if let Some(bg) = app.theme.background {
        let gap = Paragraph::new("").style(Style::default().bg(bg));
        frame.render_widget(gap, gap_area);
    }
}

fn wrap_blame_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for ch in text.chars() {
        if current_len >= width {
            lines.push(current);
            current = String::new();
            current_len = 0;
        }
        current.push(ch);
        current_len += 1;
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}
