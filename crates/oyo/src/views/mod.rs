//! View rendering modules

mod blame;
mod evolution;
mod split;
mod unified_pane;

pub use blame::render_blame;
pub use evolution::render_evolution;
pub use split::render_split;
pub use unified_pane::render_unified_pane;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::fmt::Write;
use std::fs::OpenOptions;
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use oyo_core::{LineKind, ViewLine, ViewSpan};
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

pub(crate) fn syntax_debug_extra() -> Option<String> {
    let stats = crate::syntax::syntax_debug_stats()?;
    Some(format!(
        "syntax requests={} rendered_hit={} rendered_miss={} highlight_lines={} cached_lines={} warm_lines={}",
        stats.requests,
        stats.rendered_hits,
        stats.rendered_misses,
        stats.highlight_lines,
        stats.cached_lines,
        stats.warm_lines
    ))
}

pub(crate) fn merge_debug_extra(base: Option<String>, extra: Option<String>) -> Option<String> {
    match (base, extra) {
        (Some(mut base), Some(extra)) => {
            base.push(' ');
            base.push_str(&extra);
            Some(base)
        }
        (Some(base), None) => Some(base),
        (None, Some(extra)) => Some(extra),
        (None, None) => None,
    }
}

pub(crate) fn syntax_highlight_window(
    scroll_offset: usize,
    visible_height: usize,
) -> (usize, usize) {
    let pad = (visible_height / 3).clamp(8, 32);
    let start = scroll_offset.saturating_sub(pad);
    let end = scroll_offset.saturating_add(visible_height + pad);
    (start, end)
}

pub(crate) fn in_syntax_window(
    window: Option<(usize, usize)>,
    line_start: usize,
    line_end: usize,
) -> bool {
    match window {
        Some((start, end)) => line_end >= start && line_start < end,
        None => true,
    }
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
    format!("… +{} steps", count)
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

pub(crate) fn push_wrapped_bg_line(
    bg_lines: &mut Vec<Line<'static>>,
    wrap_width: usize,
    wrap_count: usize,
    bg: Option<Color>,
) {
    let count = wrap_count.max(1);
    if wrap_width == 0 {
        for _ in 0..count {
            bg_lines.push(Line::from(Span::raw("")));
        }
        return;
    }
    for _ in 0..count {
        let span = if let Some(bg) = bg {
            Span::styled(" ".repeat(wrap_width), Style::default().bg(bg))
        } else {
            Span::raw("")
        };
        bg_lines.push(Line::from(span));
    }
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
    format!("{}…", &text[..suffix_len])
}

use crate::app::{AnimationPhase, App, ViewMode};
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

pub(crate) fn show_extent_marker(app: &App, view_line: &ViewLine) -> bool {
    if !view_line.show_hunk_extent {
        return false;
    }
    if app.diff_extent_marker_context {
        return true;
    }
    if matches!(view_line.kind, LineKind::Context) && !view_line.has_changes {
        return false;
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DebugViewKey {
    file_index: usize,
    view_mode: ViewMode,
    stepping: bool,
    line_wrap: bool,
    scroll_offset: usize,
    render_scroll_offset: usize,
    viewport_height: usize,
    window_start: usize,
    window_total: usize,
    view_len: usize,
    current_hunk: usize,
    last_nav_was_hunk: bool,
    cursor_change: Option<usize>,
    show_hunk_extent_while_stepping: bool,
    placeholder_view: bool,
}

fn view_debug_path() -> Option<&'static PathBuf> {
    static VIEW_DEBUG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    VIEW_DEBUG_PATH.get_or_init(|| {
        std::env::var_os("OYO_DEBUG_VIEW")?;
        let path = std::env::var_os("OYO_DEBUG_VIEW_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("oyo_view_debug.log"));
        Some(path)
    });
    VIEW_DEBUG_PATH.get().and_then(|opt| opt.as_ref())
}

pub(crate) fn view_debug_enabled() -> bool {
    view_debug_path().is_some()
}

fn view_debug_nav_enabled() -> bool {
    std::env::var_os("OYO_DEBUG_VIEW_NAV").is_some()
}

fn view_debug_nav_path() -> Option<PathBuf> {
    std::env::var_os("OYO_DEBUG_VIEW")?;
    let path = std::env::var_os("OYO_DEBUG_VIEW_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("oyo_view_debug.log"));
    Some(path)
}

fn view_debug_every() -> bool {
    std::env::var_os("OYO_DEBUG_VIEW_EVERY").is_some()
}

fn view_debug_context_lines() -> usize {
    static CONTEXT: OnceLock<usize> = OnceLock::new();
    *CONTEXT.get_or_init(|| {
        std::env::var("OYO_DEBUG_VIEW_CONTEXT")
            .ok()
            .and_then(|val| val.parse::<usize>().ok())
            .unwrap_or(0)
    })
}

fn view_debug_max_lines() -> usize {
    static MAX_LINES: OnceLock<usize> = OnceLock::new();
    *MAX_LINES.get_or_init(|| {
        std::env::var("OYO_DEBUG_VIEW_MAX_LINES")
            .ok()
            .and_then(|val| val.parse::<usize>().ok())
            .unwrap_or(200)
    })
}

fn view_debug_filters() -> Option<&'static Vec<String>> {
    static FILTERS: OnceLock<Option<Vec<String>>> = OnceLock::new();
    FILTERS
        .get_or_init(|| {
            let raw = std::env::var("OYO_DEBUG_VIEW_FILTER").ok()?;
            let filters: Vec<String> = raw
                .split(',')
                .map(|part| part.trim())
                .filter(|part| !part.is_empty())
                .map(|part| part.to_ascii_lowercase())
                .collect();
            if filters.is_empty() {
                None
            } else {
                Some(filters)
            }
        })
        .as_ref()
}

fn view_debug_file_allowed(file_name: &str) -> bool {
    let Some(filters) = view_debug_filters() else {
        return true;
    };
    let haystack = file_name.to_ascii_lowercase();
    filters.iter().any(|filter| haystack.contains(filter))
}

fn view_debug_step_filter() -> Option<bool> {
    static STEP_FILTER: OnceLock<Option<bool>> = OnceLock::new();
    *STEP_FILTER.get_or_init(|| {
        let raw = std::env::var("OYO_DEBUG_VIEW_STEP").ok()?;
        let val = raw.trim().to_ascii_lowercase();
        match val.as_str() {
            "step" | "stepping" | "on" | "true" | "1" => Some(true),
            "nostep" | "no-step" | "off" | "false" | "0" => Some(false),
            "any" | "both" | "*" | "" => None,
            _ => None,
        }
    })
}

fn view_debug_step_allowed(stepping: bool) -> bool {
    match view_debug_step_filter() {
        Some(expected) => stepping == expected,
        None => true,
    }
}

fn view_debug_should_log(key: DebugViewKey) -> bool {
    static LAST_KEY: OnceLock<Mutex<Option<DebugViewKey>>> = OnceLock::new();
    let store = LAST_KEY.get_or_init(|| Mutex::new(None));
    let mut guard = store.lock().unwrap();
    if guard.as_ref() == Some(&key) {
        return false;
    }
    *guard = Some(key);
    true
}

fn view_debug_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn view_debug_open(path: &PathBuf) -> Option<std::fs::File> {
    static CLEARED: AtomicBool = AtomicBool::new(false);
    let truncate = std::env::var_os("OYO_DEBUG_VIEW_CLEAR").is_some();
    let mut opts = OpenOptions::new();
    opts.create(true);
    if truncate && !CLEARED.swap(true, Ordering::Relaxed) {
        opts.write(true).truncate(true);
    } else {
        opts.append(true);
    }
    opts.open(path).ok()
}

pub(crate) fn log_view_nav_event(app: &mut App, action: &str, moved: bool) {
    let Some(path) = view_debug_nav_path() else {
        return;
    };
    if !view_debug_nav_enabled() {
        return;
    }
    let file_name = app.current_file_path();
    if !view_debug_file_allowed(&file_name) {
        return;
    }
    if !view_debug_step_allowed(app.stepping) {
        return;
    }
    let file_index = app.multi_diff.selected_index;
    let view_mode = app.view_mode;
    let stepping = app.stepping;
    let scroll_global = app.scroll_offset;
    let render_scroll = app.render_scroll_offset();
    let window_start = app.view_window_start();
    let windowed = app.view_windowed();
    let state = app.multi_diff.current_navigator().state().clone();
    let ts = view_debug_timestamp_ms();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "OYO_VIEW_NAV ts_ms={} action={} moved={} file_index={} file=\"{}\" view_mode={:?} stepping={} scroll_global={} render_scroll={} window_start={} windowed={} current_step={} current_hunk={} cursor_change={} last_nav_was_hunk={} step_direction={:?}",
        ts,
        action,
        moved,
        file_index,
        file_name,
        view_mode,
        stepping,
        scroll_global,
        render_scroll,
        window_start,
        windowed,
        state.current_step,
        state.current_hunk,
        fmt_opt_usize(state.cursor_change),
        state.last_nav_was_hunk,
        state.step_direction
    );
    if let Some(mut file) = view_debug_open(&path) {
        let _ = file.write_all(out.as_bytes());
    }
}

fn debug_truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(3);
    if keep == 0 {
        return "...".to_string();
    }
    let mut cut = 0usize;
    for (idx, _) in text.char_indices() {
        if idx > keep {
            break;
        }
        cut = idx;
    }
    format!("{}...", &text[..cut])
}

fn fmt_opt_usize(value: Option<usize>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn fmt_range(start: usize, end: usize) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{}-{}", start, end)
    }
}

fn evolution_visible_flags(view: &[ViewLine], animation_phase: AnimationPhase) -> Vec<bool> {
    let mut has_visible = false;
    for line in view {
        match line.kind {
            LineKind::Deleted => {}
            LineKind::PendingDelete => {
                if line.is_active && animation_phase != AnimationPhase::Idle {
                    has_visible = true;
                    break;
                }
            }
            _ => {
                has_visible = true;
                break;
            }
        }
    }

    let show_deleted_fallback = !has_visible;
    let mut flags = Vec::with_capacity(view.len());
    for line in view {
        let visible = match line.kind {
            LineKind::Deleted => show_deleted_fallback,
            LineKind::PendingDelete => {
                if show_deleted_fallback {
                    true
                } else {
                    line.is_active && animation_phase != AnimationPhase::Idle
                }
            }
            _ => true,
        };
        flags.push(visible);
    }
    flags
}

pub(crate) fn maybe_log_view_debug(
    app: &mut App,
    view: &[ViewLine],
    pane: &str,
    visible_height: usize,
    visible_width: usize,
    render_scroll_offset: usize,
    extra: Option<String>,
) {
    let Some(path) = view_debug_path() else {
        return;
    };

    let file_index = app.multi_diff.selected_index;
    let view_mode = app.view_mode;
    let stepping = app.stepping;
    let line_wrap = app.line_wrap;
    let scroll_offset = app.scroll_offset;
    let window_start = app.view_window_start();
    let window_total = app.render_total_lines(view.len());
    let diff_status = app.multi_diff.current_file_diff_status();
    let placeholder_view = app.multi_diff.current_navigator_is_placeholder();
    let animation_phase = app.animation_phase;
    let step_direction = app.multi_diff.current_step_direction();
    let file_name = app.current_file_path();
    if !view_debug_file_allowed(&file_name) {
        return;
    }
    if !view_debug_step_allowed(app.stepping) {
        return;
    }

    let (
        current_hunk,
        total_hunks,
        last_nav_was_hunk,
        cursor_change,
        show_extent_step,
        scope_hunk,
        scope_from_cursor,
    ) = {
        let nav = app.multi_diff.current_navigator();
        let state = nav.state();
        let total_hunks = nav.hunks().len();
        let mut scope_hunk = if total_hunks > 0 {
            Some(state.current_hunk)
        } else {
            None
        };
        let mut scope_from_cursor = false;
        if state.last_nav_was_hunk {
            if let Some(cursor) = state.cursor_change {
                if let Some(hunk) = nav.hunk_index_for_change_id_exact(cursor) {
                    scope_hunk = Some(hunk);
                    scope_from_cursor = true;
                }
            }
        }
        (
            state.current_hunk,
            total_hunks,
            state.last_nav_was_hunk,
            state.cursor_change,
            state.show_hunk_extent_while_stepping,
            scope_hunk,
            scope_from_cursor,
        )
    };

    let key = DebugViewKey {
        file_index,
        view_mode,
        stepping,
        line_wrap,
        scroll_offset,
        render_scroll_offset,
        viewport_height: visible_height,
        window_start,
        window_total,
        view_len: view.len(),
        current_hunk,
        last_nav_was_hunk,
        cursor_change,
        show_hunk_extent_while_stepping: show_extent_step,
        placeholder_view,
    };

    if !view_debug_every() && !view_debug_should_log(key) {
        return;
    }

    let mut out = String::new();
    let ts = view_debug_timestamp_ms();
    let _ = writeln!(
        out,
        "OYO_VIEW_DEBUG ts_ms={} pane={} file_index={} file=\"{}\"",
        ts, pane, file_index, file_name
    );
    let _ = writeln!(
        out,
        "mode={:?} stepping={} line_wrap={} diff_status={:?} placeholder={} view_len={} windowed={} window_start={} window_total={} viewport_h={} viewport_w={} scroll_global={} render_scroll={}",
        view_mode,
        stepping,
        line_wrap,
        diff_status,
        placeholder_view,
        view.len(),
        app.view_windowed(),
        window_start,
        window_total,
        visible_height,
        visible_width,
        scroll_offset,
        render_scroll_offset
    );
    let _ = writeln!(
        out,
        "state current_hunk={} total_hunks={} last_nav_was_hunk={} cursor_change={} show_extent_step={} scope_hunk={} scope_from_cursor={} step_direction={:?} animation_phase={:?}",
        current_hunk,
        total_hunks,
        last_nav_was_hunk,
        fmt_opt_usize(cursor_change),
        show_extent_step,
        fmt_opt_usize(scope_hunk),
        scope_from_cursor,
        step_direction,
        animation_phase
    );
    if let Some(extra) = extra {
        let _ = writeln!(out, "extra {}", extra);
    }

    if view_mode == ViewMode::Split && line_wrap {
        let _ = writeln!(out, "note split_wrap_indices=approx");
    }

    let visible_start = render_scroll_offset;
    let visible_end = render_scroll_offset.saturating_add(visible_height.saturating_sub(1));
    let context = view_debug_context_lines();
    let log_start = visible_start.saturating_sub(context);
    let log_end = visible_end.saturating_add(context);
    let _ = writeln!(
        out,
        "visible_render_range={}..{} context={}",
        visible_start, visible_end, context
    );

    let max_lines = view_debug_max_lines();
    let mut logged = 0usize;

    match view_mode {
        ViewMode::UnifiedPane | ViewMode::Blame => {
            let mut display_idx = 0usize;
            for (raw_idx, line) in view.iter().enumerate() {
                let text = view_spans_to_text(&line.spans)
                    .replace('\n', "\\n")
                    .replace('\r', "\\r");
                let wrap = if line_wrap {
                    wrap_count_for_text(&text, visible_width).max(1)
                } else {
                    1
                };
                let start = display_idx;
                let end = display_idx.saturating_add(wrap.saturating_sub(1));
                let in_range = end >= log_start && start <= log_end;
                if in_range {
                    logged += 1;
                    if max_lines != 0 && logged > max_lines {
                        let _ = writeln!(out, "lines truncated (max={})", max_lines);
                        break;
                    }
                    let global_start = window_start.saturating_add(start);
                    let global_end = window_start.saturating_add(end);
                    let scope = scope_hunk.is_some_and(|h| line.hunk_index == Some(h));
                    let _ = writeln!(
                        out,
                        "L raw={} disp={} gdisp={} h={} scope={} show={} kind={:?} changes={} old={} new={} act={} prim={} id={} wrap={} txt=\"{}\"",
                        raw_idx,
                        fmt_range(start, end),
                        fmt_range(global_start, global_end),
                        fmt_opt_usize(line.hunk_index),
                        scope,
                        line.show_hunk_extent,
                        line.kind,
                        line.has_changes,
                        fmt_opt_usize(line.old_line),
                        fmt_opt_usize(line.new_line),
                        line.is_active,
                        line.is_primary_active,
                        line.change_id,
                        wrap,
                        debug_truncate(&text, 120)
                    );
                }
                display_idx = display_idx.saturating_add(wrap);
            }
        }
        ViewMode::Evolution => {
            let visible_flags = evolution_visible_flags(view, animation_phase);
            let mut display_idx = 0usize;
            for (raw_idx, line) in view.iter().enumerate() {
                if !visible_flags.get(raw_idx).copied().unwrap_or(false) {
                    continue;
                }
                let text = view_spans_to_text(&line.spans)
                    .replace('\n', "\\n")
                    .replace('\r', "\\r");
                let wrap = if line_wrap {
                    wrap_count_for_text(&text, visible_width).max(1)
                } else {
                    1
                };
                let start = display_idx;
                let end = display_idx.saturating_add(wrap.saturating_sub(1));
                let in_range = end >= log_start && start <= log_end;
                if in_range {
                    logged += 1;
                    if max_lines != 0 && logged > max_lines {
                        let _ = writeln!(out, "lines truncated (max={})", max_lines);
                        break;
                    }
                    let global_start = window_start.saturating_add(start);
                    let global_end = window_start.saturating_add(end);
                    let scope = scope_hunk.is_some_and(|h| line.hunk_index == Some(h));
                    let _ = writeln!(
                        out,
                        "L raw={} disp={} gdisp={} h={} scope={} show={} kind={:?} changes={} old={} new={} act={} prim={} id={} wrap={} txt=\"{}\"",
                        raw_idx,
                        fmt_range(start, end),
                        fmt_range(global_start, global_end),
                        fmt_opt_usize(line.hunk_index),
                        scope,
                        line.show_hunk_extent,
                        line.kind,
                        line.has_changes,
                        fmt_opt_usize(line.old_line),
                        fmt_opt_usize(line.new_line),
                        line.is_active,
                        line.is_primary_active,
                        line.change_id,
                        wrap,
                        debug_truncate(&text, 120)
                    );
                }
                display_idx = display_idx.saturating_add(wrap);
            }
        }
        ViewMode::Split => {
            let align_lines = app.split_align_lines;
            let mut old_idx = 0usize;
            let mut new_idx = 0usize;
            for (raw_idx, line) in view.iter().enumerate() {
                let fold_line = crate::app::is_fold_line(line);
                let old_present = line.old_line.is_some() || fold_line;
                let new_present = (line.new_line.is_some()
                    && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete))
                    || fold_line;
                let old_idx_start = if old_present || (align_lines && new_present) {
                    Some(old_idx)
                } else {
                    None
                };
                let new_idx_start = if new_present || (align_lines && old_present) {
                    Some(new_idx)
                } else {
                    None
                };
                if old_idx_start.is_some() {
                    old_idx = old_idx.saturating_add(1);
                }
                if new_idx_start.is_some() {
                    new_idx = new_idx.saturating_add(1);
                }
                let in_range = old_idx_start
                    .map(|idx| idx >= log_start && idx <= log_end)
                    .unwrap_or(false)
                    || new_idx_start
                        .map(|idx| idx >= log_start && idx <= log_end)
                        .unwrap_or(false);
                if in_range {
                    logged += 1;
                    if max_lines != 0 && logged > max_lines {
                        let _ = writeln!(out, "lines truncated (max={})", max_lines);
                        break;
                    }
                    let text = view_spans_to_text(&line.spans)
                        .replace('\n', "\\n")
                        .replace('\r', "\\r");
                    let global_old = old_idx_start.map(|idx| window_start.saturating_add(idx));
                    let global_new = new_idx_start.map(|idx| window_start.saturating_add(idx));
                    let scope = scope_hunk.is_some_and(|h| line.hunk_index == Some(h));
                    let _ = writeln!(
                        out,
                        "L raw={} old={} new={} gold={} gnew={} h={} scope={} show={} kind={:?} changes={} old_line={} new_line={} act={} prim={} id={} txt=\"{}\"",
                        raw_idx,
                        fmt_opt_usize(old_idx_start),
                        fmt_opt_usize(new_idx_start),
                        fmt_opt_usize(global_old),
                        fmt_opt_usize(global_new),
                        fmt_opt_usize(line.hunk_index),
                        scope,
                        line.show_hunk_extent,
                        line.kind,
                        line.has_changes,
                        fmt_opt_usize(line.old_line),
                        fmt_opt_usize(line.new_line),
                        line.is_active,
                        line.is_primary_active,
                        line.change_id,
                        debug_truncate(&text, 120)
                    );
                }
            }
        }
    }

    if let Some(mut file) = view_debug_open(path) {
        let _ = IoWrite::write_all(&mut file, out.as_bytes());
    }
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
fn render_empty_state(
    frame: &mut Frame,
    area: Rect,
    theme: &ResolvedTheme,
    has_changes: bool,
    is_binary: bool,
) {
    // Fill entire area with background
    if let Some(bg) = theme.background {
        let bg_fill = Paragraph::new("").style(Style::default().bg(bg));
        frame.render_widget(bg_fill, area);
    }

    let (primary_text, show_hint) = if is_binary {
        ("Binary file (preview disabled)", false)
    } else if has_changes {
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
