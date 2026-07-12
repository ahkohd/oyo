use super::{AnimationPhase, FoldContextExpansion, FoldContextKey, FoldContextRegion, ViewMode};
use crate::config::FoldContextMode;
use oyo_core::{Change, ChangeKind, LineKind, StepDirection, ViewLine, ViewSpan, ViewSpanKind};
use ratatui::style::{Color, Modifier};
use ratatui::text::Span;
use regex::Regex;
use rustc_hash::{FxHashMap, FxHashSet};
use std::io::Write;
use std::process::{Command, Stdio};

pub(crate) fn allow_overscroll_state(
    overscroll_enabled: bool,
    auto_center: bool,
    needs_scroll_to_active: bool,
    centered_once: bool,
) -> bool {
    overscroll_enabled && ((auto_center && needs_scroll_to_active) || centered_once)
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
    platform_clipboard(text) || write_osc52_clipboard(text)
}

fn platform_clipboard(text: &str) -> bool {
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
    child.wait().is_ok_and(|status| status.success())
}

fn write_osc52_clipboard(text: &str) -> bool {
    let sequence = osc52_clipboard_sequence(text);
    std::io::stdout()
        .write_all(sequence.as_bytes())
        .and_then(|_| std::io::stdout().flush())
        .is_ok()
}

fn osc52_clipboard_sequence(text: &str) -> String {
    let payload = base64_encode(text.as_bytes());
    let osc = format!("\x1b]52;c;{payload}\x07");
    if std::env::var_os("TMUX").is_some() {
        return format!("\x1bPtmux;\x1b{osc}\x1b\\");
    }
    if std::env::var_os("STY").is_some() {
        return format!("\x1bP{osc}\x1b\\");
    }
    osc
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
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

pub(crate) fn is_conflict_marker(line: &ViewLine) -> bool {
    let text = line.content.trim_start();
    text.starts_with("<<<<<<<") || text.starts_with("=======") || text.starts_with(">>>>>>>")
}

pub(crate) fn apply_highlight_spans(
    spans: Vec<Span<'static>>,
    ranges: &[(usize, usize)],
    bg: Color,
    fg: Option<Color>,
    modifier: Modifier,
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
                out.push(Span::styled(
                    slice.to_string(),
                    style.add_modifier(modifier),
                ));
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
        ViewMode::Preview => (view.len(), None),
    }
}

const FOLD_CONTEXT_MIN_LINES: usize = 8;

pub(crate) fn fold_context_label(hidden_lines: usize) -> String {
    let unit = if hidden_lines == 1 { "line" } else { "lines" };
    format!(" {hidden_lines} unchanged {unit} ")
}

fn folded_context_line(text: String, change_id: usize) -> ViewLine {
    ViewLine {
        content: text.clone(),
        spans: vec![ViewSpan {
            text,
            kind: ViewSpanKind::Equal,
        }],
        kind: LineKind::Context,
        old_line: None,
        new_line: None,
        is_active: false,
        is_active_change: false,
        is_primary_active: false,
        show_hunk_extent: false,
        change_id,
        hunk_index: None,
        has_changes: false,
    }
}

#[cfg(test)]
pub(crate) fn fold_context_view(
    view: Vec<ViewLine>,
    mode: FoldContextMode,
    file_index: usize,
    context_lines: usize,
    expansions: &FxHashMap<FoldContextKey, FoldContextExpansion>,
    comment_anchors: &FxHashSet<usize>,
) -> (Vec<ViewLine>, Vec<FoldContextRegion>) {
    fold_context_view_with_expand_all(
        view,
        mode,
        file_index,
        context_lines,
        expansions,
        comment_anchors,
        false,
    )
}

pub(crate) fn fold_context_view_with_expand_all(
    view: Vec<ViewLine>,
    mode: FoldContextMode,
    file_index: usize,
    context_lines: usize,
    expansions: &FxHashMap<FoldContextKey, FoldContextExpansion>,
    comment_anchors: &FxHashSet<usize>,
    expand_all: bool,
) -> (Vec<ViewLine>, Vec<FoldContextRegion>) {
    if !mode.is_enabled() || view.is_empty() {
        return (view, Vec::new());
    }
    let mut out = Vec::with_capacity(view.len());
    let mut regions = Vec::new();
    let mut idx = 0usize;
    while idx < view.len() {
        let line = &view[idx];
        if matches!(line.kind, LineKind::Context)
            && line.hunk_index.is_none()
            && !line.has_changes
            && !comment_anchors.contains(&line.change_id)
        {
            let start = idx;
            let mut end = idx + 1;
            while end < view.len() {
                let next = &view[end];
                if matches!(next.kind, LineKind::Context)
                    && next.hunk_index.is_none()
                    && !next.has_changes
                    && !comment_anchors.contains(&next.change_id)
                {
                    end += 1;
                } else {
                    break;
                }
            }
            let count = end - start;
            if count >= FOLD_CONTEXT_MIN_LINES {
                let key = FoldContextKey {
                    file_index,
                    start_change_id: view[start].change_id,
                    end_change_id: view[end - 1].change_id,
                };
                let base_top = context_lines.min(count);
                let base_bottom = context_lines.min(count.saturating_sub(base_top));
                let base_hidden = count.saturating_sub(base_top).saturating_sub(base_bottom);
                if base_hidden < FOLD_CONTEXT_MIN_LINES {
                    out.extend(view[start..end].iter().cloned());
                    idx = end;
                    continue;
                }
                let expansion = if expand_all {
                    FoldContextExpansion {
                        top: usize::MAX,
                        bottom: 0,
                    }
                } else {
                    expansions.get(&key).copied().unwrap_or_default()
                };
                let top = base_top.saturating_add(expansion.top).min(count);
                let bottom = base_bottom
                    .saturating_add(expansion.bottom)
                    .min(count.saturating_sub(top));
                let hidden = count.saturating_sub(top).saturating_sub(bottom);
                if hidden == 0 {
                    out.extend(view[start..end].iter().cloned());
                } else {
                    out.extend(view[start..start + top].iter().cloned());
                    out.push(folded_context_line(
                        format!("↑{}↓", fold_context_label(hidden)),
                        key.start_change_id,
                    ));
                    out.extend(view[end - bottom..end].iter().cloned());
                    regions.push(FoldContextRegion {
                        key,
                        hidden_lines: hidden,
                        scope_line: view[start + top].new_line,
                        scope_hint: None,
                    });
                }
                idx = end;
                continue;
            }
        }
        out.push(view[idx].clone());
        idx += 1;
    }
    (out, regions)
}

pub(crate) fn is_fold_line(line: &ViewLine) -> bool {
    matches!(line.kind, LineKind::Context)
        && line.hunk_index.is_none()
        && line.old_line.is_none()
        && line.new_line.is_none()
        && !line.has_changes
}

pub(crate) fn evolution_display_metrics(
    view: &[ViewLine],
    animation_phase: AnimationPhase,
) -> (usize, Option<usize>) {
    let mut display_len = 0usize;
    let mut primary_idx: Option<usize> = None;
    let mut any_active_idx: Option<usize> = None;

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
        let fold_line = is_fold_line(line);
        let old_present = line.old_line.is_some() || fold_line;
        let new_present = (line.new_line.is_some()
            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete))
            || fold_line;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn context_line(change_id: usize) -> ViewLine {
        ViewLine {
            content: format!("line {change_id}"),
            spans: vec![ViewSpan {
                text: format!("line {change_id}"),
                kind: ViewSpanKind::Equal,
            }],
            kind: LineKind::Context,
            old_line: Some(change_id + 1),
            new_line: Some(change_id + 1),
            is_active: false,
            is_active_change: false,
            is_primary_active: false,
            show_hunk_extent: false,
            change_id,
            hunk_index: None,
            has_changes: false,
        }
    }

    #[test]
    fn base64_encode_pads_short_chunks() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");
    }

    #[test]
    fn expandable_context_fold_keeps_edges_and_expands_from_both_sides() {
        let view = (0..30).map(context_line).collect::<Vec<_>>();
        let mut expansions = FxHashMap::default();
        let comment_anchors = FxHashSet::default();

        let (folded, regions) = fold_context_view(
            view.clone(),
            FoldContextMode::Expandable,
            0,
            3,
            &expansions,
            &comment_anchors,
        );
        assert_eq!(folded.len(), 7);
        assert_eq!(folded[3].content, "↑ 24 unchanged lines ↓");
        assert_eq!(regions[0].hidden_lines, 24);

        expansions.insert(regions[0].key, FoldContextExpansion { top: 20, bottom: 0 });
        let (folded, regions) = fold_context_view(
            view.clone(),
            FoldContextMode::Expandable,
            0,
            3,
            &expansions,
            &comment_anchors,
        );
        assert_eq!(folded.len(), 27);
        assert_eq!(folded[23].content, "↑ 4 unchanged lines ↓");

        expansions.insert(regions[0].key, FoldContextExpansion { top: 23, bottom: 0 });
        let (folded, regions) = fold_context_view(
            view.clone(),
            FoldContextMode::Expandable,
            0,
            3,
            &expansions,
            &comment_anchors,
        );
        assert_eq!(folded[26].content, "↑ 1 unchanged line ↓");

        expansions.insert(regions[0].key, FoldContextExpansion { top: 20, bottom: 4 });
        let (folded, regions) = fold_context_view(
            view,
            FoldContextMode::Expandable,
            0,
            3,
            &expansions,
            &comment_anchors,
        );
        assert_eq!(folded.len(), 30);
        assert!(regions.is_empty());
    }

    #[test]
    fn comment_anchors_split_folds_and_keep_edge_context() {
        let view = (0..60).map(context_line).collect::<Vec<_>>();
        let comment_anchors = FxHashSet::from_iter([20, 40]);
        let (folded, regions) = fold_context_view(
            view,
            FoldContextMode::Expandable,
            0,
            3,
            &FxHashMap::default(),
            &comment_anchors,
        );

        assert_eq!(regions.len(), 3);
        for anchor in [20, 40] {
            let index = folded
                .iter()
                .position(|line| line.change_id == anchor && !is_fold_line(line))
                .unwrap();
            assert_eq!(folded[index - 3].change_id, anchor - 3);
            assert_eq!(folded[index + 3].change_id, anchor + 3);
        }
    }

    #[test]
    fn off_and_small_gaps_stay_open_and_zero_context_is_maximally_compact() {
        let expansions = FxHashMap::default();
        let comment_anchors = FxHashSet::default();
        let (full, regions) = fold_context_view(
            (0..30).map(context_line).collect(),
            FoldContextMode::Off,
            0,
            3,
            &expansions,
            &comment_anchors,
        );
        assert_eq!(full.len(), 30);
        assert!(regions.is_empty());

        let (small, regions) = fold_context_view(
            (0..13).map(context_line).collect(),
            FoldContextMode::Expandable,
            0,
            3,
            &expansions,
            &comment_anchors,
        );
        assert_eq!(small.len(), 13);
        assert!(regions.is_empty());

        let (compact, regions) = fold_context_view(
            (0..8).map(context_line).collect(),
            FoldContextMode::Expandable,
            0,
            0,
            &expansions,
            &comment_anchors,
        );
        assert_eq!(compact.len(), 1);
        assert_eq!(compact[0].content, "↑ 8 unchanged lines ↓");
        assert_eq!(regions[0].hidden_lines, 8);
    }
}
