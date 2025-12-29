//! View rendering modules

mod evolution;
mod single_pane;
mod split;

pub use evolution::render_evolution;
pub use single_pane::render_single_pane;
pub use split::render_split;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;

use oyo_core::{LineKind, ViewSpan};
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) fn spans_to_text(spans: &[Span]) -> String {
    let mut out = String::new();
    for span in spans {
        out.push_str(span.content.as_ref());
    }
    out
}

pub(crate) fn view_spans_to_text(spans: &[ViewSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        out.push_str(&span.text);
    }
    out
}

pub(crate) fn spans_width(spans: &[Span]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

pub(crate) fn line_is_italic(spans: &[Span]) -> bool {
    let mut has_text = false;
    for span in spans {
        if span.content.trim().is_empty() {
            continue;
        }
        has_text = true;
        if !span.style.add_modifier.contains(Modifier::ITALIC) {
            return false;
        }
    }
    has_text
}

pub(crate) fn apply_italic_spans(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|span| Span::styled(span.content, span.style.add_modifier(Modifier::ITALIC)))
        .collect()
}

pub(crate) fn boost_inline_bg(app: &App, base_bg: Option<Color>, accent: Color) -> Option<Color> {
    if !app.diff_bg {
        return base_bg;
    }
    let base = base_bg?;
    color::blend_colors(base, accent, 0.10).or(Some(base))
}

pub(crate) fn pending_tail_text(count: usize) -> String {
    format!("... +{} more", count)
}

pub(crate) fn diff_line_bg(kind: LineKind, theme: &ResolvedTheme) -> Option<Color> {
    match kind {
        LineKind::Inserted | LineKind::PendingInsert => theme.diff_added_bg,
        LineKind::Deleted | LineKind::PendingDelete => theme.diff_removed_bg,
        LineKind::Modified | LineKind::PendingModify => theme.diff_modified_bg,
        _ => None,
    }
}

pub(crate) fn apply_line_bg(
    spans: Vec<Span<'static>>,
    bg: Color,
    visible_width: usize,
    line_wrap: bool,
) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = spans
        .into_iter()
        .map(|span| {
            let style = if span.style.bg.is_some() {
                span.style
            } else {
                span.style.bg(bg)
            };
            Span::styled(span.content, style)
        })
        .collect();

    if !line_wrap {
        let pad = visible_width.saturating_sub(spans_width(&out));
        if pad > 0 {
            out.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
        }
    }

    out
}

pub(crate) fn apply_spans_bg(spans: Vec<Span<'static>>, bg: Color) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|span| Span::styled(span.content, span.style.bg(bg)))
        .collect()
}

pub(crate) fn clear_leading_ws_bg(
    spans: Vec<Span<'static>>,
    clear_when_fg: Option<Color>,
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut at_line_start = true;

    for span in spans {
        if !at_line_start {
            out.push(span);
            continue;
        }

        let text = span.content.as_ref();
        if text.is_empty() {
            continue;
        }

        let mut ws_len = 0usize;
        for (idx, ch) in text.char_indices() {
            if ch.is_whitespace() {
                ws_len = idx + ch.len_utf8();
            } else {
                break;
            }
        }

        if ws_len == 0 {
            out.push(span);
            at_line_start = false;
            continue;
        }

        let (ws, rest) = text.split_at(ws_len);
        let should_clear = match clear_when_fg {
            Some(fg) => span.style.fg == Some(fg),
            None => true,
        };
        if !ws.is_empty() {
            let ws_style = if should_clear {
                Style {
                    bg: None,
                    ..span.style
                }
            } else {
                span.style
            };
            out.push(Span::styled(ws.to_string(), ws_style));
        }
        if !rest.is_empty() {
            out.push(Span::styled(rest.to_string(), span.style));
            at_line_start = false;
        }
    }

    out
}

pub(crate) fn replace_leading_ws_bg(
    spans: Vec<Span<'static>>,
    clear_when_fg: Option<Color>,
    replacement_bg: Option<Color>,
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut at_line_start = true;

    for span in spans {
        if !at_line_start {
            out.push(span);
            continue;
        }

        let text = span.content.as_ref();
        if text.is_empty() {
            continue;
        }

        let mut ws_len = 0usize;
        for (idx, ch) in text.char_indices() {
            if ch.is_whitespace() {
                ws_len = idx + ch.len_utf8();
            } else {
                break;
            }
        }

        if ws_len == 0 {
            out.push(span);
            at_line_start = false;
            continue;
        }

        let (ws, rest) = text.split_at(ws_len);
        let should_clear = match clear_when_fg {
            Some(fg) => span.style.fg == Some(fg),
            None => true,
        };
        if !ws.is_empty() {
            let ws_style = if should_clear {
                Style {
                    bg: replacement_bg,
                    ..span.style
                }
            } else {
                span.style
            };
            out.push(Span::styled(ws.to_string(), ws_style));
        }
        if !rest.is_empty() {
            out.push(Span::styled(rest.to_string(), span.style));
            at_line_start = false;
        }
    }

    out
}

pub(crate) const TAB_WIDTH: usize = 8;

pub(crate) fn expand_tabs_in_spans(spans: &[Span], tab_width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut col = 0usize;

    for span in spans {
        let mut buf = String::new();
        for g in span.content.as_ref().graphemes(true) {
            if g == "\n" {
                buf.push('\n');
                col = 0;
                continue;
            }
            if g == "\t" {
                let spaces = tab_width.saturating_sub(col % tab_width);
                for _ in 0..spaces {
                    buf.push(' ');
                }
                col = col.saturating_add(spaces);
                continue;
            }
            buf.push_str(g);
            col = col.saturating_add(UnicodeWidthStr::width(g));
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, span.style));
        }
    }

    out
}

pub(crate) fn expand_tabs_in_text(text: &str, tab_width: usize) -> String {
    let mut out = String::new();
    let mut col = 0usize;

    for g in text.graphemes(true) {
        if g == "\n" {
            out.push('\n');
            col = 0;
            continue;
        }
        if g == "\t" {
            let spaces = tab_width.saturating_sub(col % tab_width);
            for _ in 0..spaces {
                out.push(' ');
            }
            col = col.saturating_add(spaces);
            continue;
        }
        out.push_str(g);
        col = col.saturating_add(UnicodeWidthStr::width(g));
    }

    out
}

pub(crate) fn slice_spans(
    spans: &[Span<'static>],
    start_col: usize,
    width: usize,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let line_width = spans_width(spans);
    if start_col >= line_width {
        return Vec::new();
    }
    let end_col = start_col.saturating_add(width);
    let mut out = Vec::new();
    let mut col = 0usize;

    for span in spans {
        if span.content.is_empty() {
            continue;
        }
        let mut buf = String::new();
        for g in span.content.as_ref().graphemes(true) {
            if g == "\n" {
                col = 0;
                continue;
            }
            let g_width = UnicodeWidthStr::width(g);
            let next_col = col.saturating_add(g_width);
            if next_col <= start_col {
                col = next_col;
                continue;
            }
            if col >= end_col {
                break;
            }
            buf.push_str(g);
            col = next_col;
            if col >= end_col {
                break;
            }
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, span.style));
        }
        if col >= end_col {
            break;
        }
    }

    out
}

pub(crate) fn pad_spans_bg(
    mut spans: Vec<Span<'static>>,
    bg: Color,
    width: usize,
) -> Vec<Span<'static>> {
    let current_width = spans_width(&spans);
    if current_width < width {
        spans.push(Span::styled(
            " ".repeat(width - current_width),
            Style::default().bg(bg),
        ));
    }
    spans
}

pub(crate) fn wrap_count_for_spans(spans: &[Span], wrap_width: usize) -> usize {
    let graphemes = spans
        .iter()
        .flat_map(|span| graphemes_for_text(span.content.as_ref()));
    wrap_count_for_graphemes(graphemes, wrap_width)
}

pub(crate) fn wrap_count_for_text(text: &str, wrap_width: usize) -> usize {
    let expanded = expand_tabs_in_text(text, TAB_WIDTH);
    let graphemes = graphemes_for_text(&expanded);
    wrap_count_for_graphemes(graphemes, wrap_width)
}

struct GraphemeInfo {
    width: u16,
    is_whitespace: bool,
}

fn graphemes_for_text(text: &str) -> impl Iterator<Item = GraphemeInfo> + '_ {
    text.graphemes(true).filter(|g| *g != "\n").map(|g| {
        let is_whitespace =
            g == "\u{200b}" || (g.chars().all(char::is_whitespace) && g != "\u{00a0}");
        let width = UnicodeWidthStr::width(g).min(u16::MAX as usize) as u16;
        GraphemeInfo {
            width,
            is_whitespace,
        }
    })
}

fn wrap_count_for_graphemes<I>(graphemes: I, wrap_width: usize) -> usize
where
    I: Iterator<Item = GraphemeInfo>,
{
    if wrap_width == 0 {
        return 1;
    }
    let max_width = wrap_width.min(u16::MAX as usize) as u16;
    let trim = false;
    let mut rows = 0usize;
    let mut line_width = 0u16;
    let mut word_width = 0u16;
    let mut word_count = 0usize;
    let mut whitespace_width = 0u16;
    let mut whitespace_count = 0usize;
    let mut pending_line_count = 0usize;
    let mut pending_whitespace: VecDeque<u16> = VecDeque::new();
    let mut non_whitespace_previous = false;

    for grapheme in graphemes {
        let symbol_width = grapheme.width;
        if symbol_width > max_width {
            continue;
        }

        let is_whitespace = grapheme.is_whitespace;
        let word_found = non_whitespace_previous && is_whitespace;
        let untrimmed_overflow = pending_line_count == 0
            && !trim
            && word_width + whitespace_width + symbol_width > max_width;

        if word_found || untrimmed_overflow {
            if (pending_line_count > 0 || !trim) && whitespace_count > 0 {
                line_width = line_width.saturating_add(whitespace_width);
                pending_line_count += whitespace_count;
            }
            if word_count > 0 {
                line_width = line_width.saturating_add(word_width);
                pending_line_count += word_count;
            }

            pending_whitespace.clear();
            whitespace_width = 0;
            whitespace_count = 0;
            word_width = 0;
            word_count = 0;
        }

        let line_full = line_width >= max_width;
        let pending_word_overflow =
            symbol_width > 0 && line_width + whitespace_width + word_width >= max_width;

        if line_full || pending_word_overflow {
            rows += 1;
            pending_line_count = 0;
            let mut remaining_width = max_width.saturating_sub(line_width);
            line_width = 0;

            while let Some(width) = pending_whitespace.front().copied() {
                if width > remaining_width {
                    break;
                }
                whitespace_width = whitespace_width.saturating_sub(width);
                remaining_width = remaining_width.saturating_sub(width);
                pending_whitespace.pop_front();
                whitespace_count = whitespace_count.saturating_sub(1);
            }

            if is_whitespace && whitespace_count == 0 {
                non_whitespace_previous = !is_whitespace;
                continue;
            }
        }

        if is_whitespace {
            whitespace_width = whitespace_width.saturating_add(symbol_width);
            whitespace_count += 1;
            pending_whitespace.push_back(symbol_width);
        } else {
            word_width = word_width.saturating_add(symbol_width);
            word_count += 1;
        }

        non_whitespace_previous = !is_whitespace;
    }

    if pending_line_count == 0 && word_count == 0 && whitespace_count > 0 {
        rows += 1;
    }
    if (pending_line_count > 0 || !trim) && whitespace_count > 0 {
        pending_line_count += whitespace_count;
    }
    if word_count > 0 {
        pending_line_count += word_count;
    }
    if pending_line_count > 0 {
        rows += 1;
    }
    rows.max(1)
}

pub(crate) fn truncate_text(text: &str, max_width: usize) -> String {
    if max_width == 0 || text.len() <= max_width {
        return text.to_string();
    }
    let suffix_len = max_width.saturating_sub(3);
    format!("{}...", &text[..suffix_len])
}

use crate::app::{AnimationPhase, App};
use crate::color;
use crate::config::{DiffExtentMarkerMode, DiffExtentMarkerScope, ResolvedTheme};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};

pub(crate) fn extent_marker_style(
    app: &App,
    kind: LineKind,
    has_changes: bool,
    old_line: Option<usize>,
    new_line: Option<usize>,
) -> Style {
    let color = match app.diff_extent_marker {
        DiffExtentMarkerMode::Neutral => app.theme.diff_ext_marker,
        DiffExtentMarkerMode::Diff => match app.diff_extent_marker_scope {
            DiffExtentMarkerScope::Progress => match kind {
                LineKind::Inserted | LineKind::PendingInsert => app.theme.insert_base(),
                LineKind::Deleted | LineKind::PendingDelete => app.theme.delete_base(),
                LineKind::Modified | LineKind::PendingModify => app.theme.modify_base(),
                LineKind::Context => app.theme.diff_ext_marker,
            },
            DiffExtentMarkerScope::Hunk => {
                if !has_changes {
                    app.theme.diff_ext_marker
                } else if old_line.is_none() {
                    app.theme.insert_base()
                } else if new_line.is_none() {
                    app.theme.delete_base()
                } else {
                    app.theme.modify_base()
                }
            }
        },
    };
    Style::default().fg(color)
}

// ============================================================================
// HSL-based animation styles (configurable colors, smooth gradients)
// ============================================================================

/// Compute animation style for insertions using a smooth fade (no pulse)
pub fn insert_style(
    phase: AnimationPhase,
    progress: f32,
    backward: bool,
    base: Color,
    from: Color,
    bg: Option<Color>,
) -> Style {
    let color = if phase == AnimationPhase::Idle {
        base
    } else {
        let t = color::animation_t_linear(phase, progress);
        let eased = color::ease_out(t);
        let (start, end) = if backward { (base, from) } else { (from, base) };
        color::lerp_rgb_color(start, end, eased)
    };

    let mut style = Style::default().fg(color);
    if let Some(bg) = bg {
        style = style.bg(bg);
    }
    style
}

/// Compute animation style for deletions using smooth fade (no pulse)
pub fn delete_style(
    phase: AnimationPhase,
    progress: f32,
    backward: bool,
    strikethrough: bool,
    base: Color,
    from: Color,
    bg: Option<Color>,
) -> Style {
    let color = if phase == AnimationPhase::Idle {
        base
    } else {
        let t = color::animation_t_linear(phase, progress);
        let eased = color::ease_out(t);
        let (start, end) = if backward { (base, from) } else { (from, base) };
        color::lerp_rgb_color(start, end, eased)
    };

    let mut style = Style::default().fg(color);
    if let Some(bg) = bg {
        style = style.bg(bg);
    }

    // Strikethrough timing based on raw progress
    if strikethrough && should_strikethrough(phase, progress, backward) {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    style
}

/// Compute animation style for modifications using a smooth fade (no pulse)
pub fn modify_style(
    phase: AnimationPhase,
    progress: f32,
    backward: bool,
    base: Color,
    from: Color,
    bg: Option<Color>,
) -> Style {
    let color = if phase == AnimationPhase::Idle {
        base
    } else {
        let t = color::animation_t_linear(phase, progress);
        let eased = color::ease_out(t);
        let (start, end) = if backward { (base, from) } else { (from, base) };
        color::lerp_rgb_color(start, end, eased)
    };

    let mut style = Style::default().fg(color);
    if let Some(bg) = bg {
        style = style.bg(bg);
    }
    style
}

/// Determine if strikethrough should be shown based on animation progress
fn should_strikethrough(phase: AnimationPhase, progress: f32, backward: bool) -> bool {
    match phase {
        AnimationPhase::Idle => true,
        AnimationPhase::FadeOut => {
            if backward {
                // Backward: removing strikethrough, remove early
                progress < 0.7
            } else {
                // Forward: adding strikethrough, don't show yet
                false
            }
        }
        AnimationPhase::FadeIn => {
            if backward {
                // Backward: strikethrough already removed
                false
            } else {
                // Forward: add strikethrough partway through
                progress > 0.3
            }
        }
    }
}

/// Render empty state message centered in area.
/// Shows hint line only if viewport has enough height and width.
fn render_empty_state(frame: &mut Frame, area: Rect, theme: &ResolvedTheme, has_changes: bool) {
    // Fill entire area with background
    if let Some(bg) = theme.background {
        let bg_fill = Paragraph::new("").style(Style::default().bg(bg));
        frame.render_widget(bg_fill, area);
    }

    let (primary_text, show_hint) = if has_changes {
        ("No content at this step", true)
    } else {
        ("No changes in this file", false)
    };
    let primary = Line::from(Span::styled(
        primary_text,
        Style::default().fg(theme.text_muted),
    ));

    let show_hint = show_hint && area.height >= 2 && area.width >= 28;
    let (lines, height) = if show_hint {
        let hint = Line::from(Span::styled(
            "j/k to step, h/l for hunks",
            Style::default()
                .fg(theme.text_muted)
                .add_modifier(Modifier::DIM),
        ));
        (vec![primary, hint], 2)
    } else {
        (vec![primary], 1)
    };

    let y_offset = area.height.saturating_sub(height) / 2;
    let centered_area = Rect {
        x: area.x,
        y: area.y + y_offset,
        width: area.width,
        height,
    };

    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, centered_area);
}
