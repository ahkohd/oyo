use super::{AnimationPhase, ViewMode};
use oyo_core::{Change, ChangeKind, LineKind, StepDirection, ViewLine};
use ratatui::style::Color;
use ratatui::text::Span;
use regex::Regex;
use std::io::Write;
use std::process::{Command, Stdio};

pub(crate) fn allow_overscroll_state(
    auto_center: bool,
    needs_scroll_to_active: bool,
    centered_once: bool,
) -> bool {
    (auto_center && needs_scroll_to_active) || centered_once
}

pub(crate) fn max_scroll(
    total_lines: usize,
    viewport_height: usize,
    allow_overscroll: bool,
) -> usize {
    if allow_overscroll {
        total_lines
            .saturating_sub(1)
            .saturating_sub(viewport_height / 2)
    } else {
        total_lines.saturating_sub(viewport_height)
    }
}

pub(crate) fn copy_to_clipboard(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        write_to_clipboard_cmd("pbcopy", &[], text)
    }
    #[cfg(target_os = "linux")]
    {
        if write_to_clipboard_cmd("wl-copy", &["--type", "text/plain"], text) {
            return true;
        }
        if write_to_clipboard_cmd("xclip", &["-selection", "clipboard"], text) {
            return true;
        }
        write_to_clipboard_cmd("xsel", &["--clipboard", "--input"], text)
    }
    #[cfg(target_os = "windows")]
    {
        write_to_clipboard_cmd("clip", &[], text)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

fn write_to_clipboard_cmd(cmd: &str, args: &[&str], text: &str) -> bool {
    let mut child = match Command::new(cmd).args(args).stdin(Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().is_ok()
}

pub(crate) fn old_text_for_change(change: &Change) -> String {
    let mut text = String::new();
    for span in &change.spans {
        match span.kind {
            ChangeKind::Equal => text.push_str(&span.text),
            ChangeKind::Delete | ChangeKind::Replace => text.push_str(&span.text),
            ChangeKind::Insert => {}
        }
    }
    text
}

pub(crate) fn inline_text_for_change(change: &Change) -> String {
    let mut text = String::new();
    for span in &change.spans {
        match span.kind {
            ChangeKind::Equal => text.push_str(&span.text),
            ChangeKind::Delete => text.push_str(&span.text),
            ChangeKind::Insert => text.push_str(&span.text),
            ChangeKind::Replace => {
                text.push_str(&span.text);
                text.push_str(&span.new_text.clone().unwrap_or_else(|| span.text.clone()));
            }
        }
    }
    text
}

pub(crate) fn modified_only_text_for_change(change: &Change) -> String {
    let mut text = String::new();
    for span in &change.spans {
        match span.kind {
            ChangeKind::Equal => text.push_str(&span.text),
            ChangeKind::Delete => {}
            ChangeKind::Insert => text.push_str(&span.text),
            ChangeKind::Replace => {
                text.push_str(&span.new_text.clone().unwrap_or_else(|| span.text.clone()));
            }
        }
    }
    text
}

pub(crate) fn line_has_query(text: &str, regex: &Regex) -> bool {
    regex.is_match(text)
}

pub(crate) fn match_ranges(text: &str, regex: &Regex) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    for mat in regex.find_iter(text) {
        ranges.push((mat.start(), mat.end()));
    }
    ranges
}

pub(crate) fn apply_highlight_spans(
    spans: Vec<Span<'static>>,
    ranges: &[(usize, usize)],
    bg: Color,
    fg: Option<Color>,
) -> Vec<Span<'static>> {
    if ranges.is_empty() {
        return spans;
    }
    let mut out: Vec<Span> = Vec::new();
    let mut range_idx = 0usize;
    let mut offset = 0usize;

    for span in spans {
        let text = span.content.as_ref();
        let span_len = text.len();
        let span_start = offset;
        let span_end = offset + span_len;

        if span_len == 0 {
            continue;
        }

        while range_idx < ranges.len() && ranges[range_idx].1 <= span_start {
            range_idx += 1;
        }

        let mut cursor = span_start;
        while range_idx < ranges.len() && ranges[range_idx].0 < span_end {
            let (r_start, r_end) = ranges[range_idx];
            let before_end = r_start.max(span_start);
            if before_end > cursor {
                let slice = &text[(cursor - span_start)..(before_end - span_start)];
                out.push(Span::styled(slice.to_string(), span.style));
            }
            let highlight_start = r_start.max(span_start);
            let highlight_end = r_end.min(span_end);
            if highlight_end > highlight_start {
                let slice = &text[(highlight_start - span_start)..(highlight_end - span_start)];
                let mut style = span.style.bg(bg);
                if let Some(fg) = fg {
                    style = style.fg(fg);
                }
                out.push(Span::styled(slice.to_string(), style));
            }
            cursor = highlight_end;
            if r_end <= span_end {
                range_idx += 1;
            } else {
                break;
            }
        }

        if cursor < span_end {
            let slice = &text[(cursor - span_start)..(span_end - span_start)];
            out.push(Span::styled(slice.to_string(), span.style));
        }

        offset += span_len;
    }

    out
}

pub fn display_metrics(
    view: &[ViewLine],
    view_mode: ViewMode,
    animation_phase: AnimationPhase,
    scroll_offset: usize,
    step_direction: StepDirection,
    split_align_lines: bool,
) -> (usize, Option<usize>) {
    match view_mode {
        ViewMode::UnifiedPane => {
            let idx = view
                .iter()
                .position(|l| l.is_primary_active)
                .or_else(|| view.iter().position(|l| l.is_active));
            (view.len(), idx)
        }
        ViewMode::Blame => {
            let idx = view
                .iter()
                .position(|l| l.is_primary_active)
                .or_else(|| view.iter().position(|l| l.is_active));
            (view.len(), idx)
        }
        ViewMode::Evolution => evolution_display_metrics(view, animation_phase),
        ViewMode::Split => {
            split_display_metrics(view, scroll_offset, step_direction, split_align_lines)
        }
    }
}

pub(crate) fn evolution_display_metrics(
    view: &[ViewLine],
    animation_phase: AnimationPhase,
) -> (usize, Option<usize>) {
    let mut display_len = 0usize;
    let mut primary_idx: Option<usize> = None;
    let mut any_active_idx: Option<usize> = None;

    for line in view {
        let visible = match line.kind {
            LineKind::Deleted => false,
            LineKind::PendingDelete => line.is_active && animation_phase != AnimationPhase::Idle,
            _ => true,
        };

        if visible {
            if line.is_primary_active && primary_idx.is_none() {
                primary_idx = Some(display_len);
            }
            if line.is_active && any_active_idx.is_none() {
                any_active_idx = Some(display_len);
            }
            display_len += 1;
        }
    }

    (display_len, primary_idx.or(any_active_idx))
}

pub(crate) fn split_display_metrics(
    view: &[ViewLine],
    scroll_offset: usize,
    step_direction: StepDirection,
    split_align_lines: bool,
) -> (usize, Option<usize>) {
    let mut old_count = 0usize;
    let mut new_count = 0usize;
    let mut old_primary_idx: Option<usize> = None;
    let mut new_primary_idx: Option<usize> = None;
    let mut old_fallback_idx: Option<usize> = None;
    let mut new_fallback_idx: Option<usize> = None;

    for line in view {
        let old_present = line.old_line.is_some();
        let new_present = line.new_line.is_some()
            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete);
        if old_present || (split_align_lines && new_present) {
            if line.is_primary_active {
                old_primary_idx = Some(old_count);
            } else if line.is_active && old_fallback_idx.is_none() {
                old_fallback_idx = Some(old_count);
            }
            old_count += 1;
        }
        if new_present || (split_align_lines && old_present) {
            if line.is_primary_active {
                new_primary_idx = Some(new_count);
            } else if line.is_active && new_fallback_idx.is_none() {
                new_fallback_idx = Some(new_count);
            }
            new_count += 1;
        }
    }

    let display_len = old_count.max(new_count);

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
