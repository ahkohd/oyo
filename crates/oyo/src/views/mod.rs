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

use crate::app::{
    diff_scrollbar_thumb, review::ReviewCommentOverlay, App, DiffScrollbarState,
    FoldContextDirection, FoldContextKey,
};
use crate::avatars::avatar_image;
use crate::keybindings::{GlobalAction, NormalAction};
use crate::markdown::markdown_preview_lines;
use oyo_core::{LineKind, ViewLine, ViewSpan};
use ratatui::{
    layout::{Margin, Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};
use ratatui_image::{Image as TerminalImage, Resize};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) struct FoldContextBand {
    pub(crate) key: FoldContextKey,
    pub(crate) spans: Vec<Span<'static>>,
    pub(crate) top_x: u16,
    pub(crate) top_width: u16,
    pub(crate) bottom_x: u16,
    pub(crate) bottom_width: u16,
}

pub(crate) fn fold_context_background(app: &App) -> Color {
    let surface = app.theme.background_panel.or(app.theme.background_element);
    match (surface, app.theme.background) {
        (Some(surface), Some(background)) if surface == background => {
            color::blend_colors(background, app.theme.text_muted, 0.05).unwrap_or(surface)
        }
        (Some(surface), Some(background)) => {
            color::blend_colors(background, surface, 0.25).unwrap_or(surface)
        }
        (Some(surface), None) => surface,
        (None, Some(background)) => {
            color::blend_colors(background, app.theme.text_muted, 0.05).unwrap_or(background)
        }
        (None, None) => {
            if app.theme_is_light {
                Color::Gray
            } else {
                Color::Black
            }
        }
    }
}

fn fold_scope_hint(text: &str, available_width: usize) -> Option<String> {
    const GAP: &str = "   ";
    let gap_width = GAP.width();
    if available_width <= gap_width.saturating_add("…".width()) {
        return None;
    }
    let text_width = available_width - gap_width;
    if text.width() <= text_width {
        return Some(format!("{GAP}{text}"));
    }

    let prefix_width = text_width.saturating_sub("…".width());
    let mut prefix = String::new();
    let mut used = 0usize;
    for grapheme in text.graphemes(true) {
        let width = grapheme.width();
        if used.saturating_add(width) > prefix_width {
            break;
        }
        prefix.push_str(grapheme);
        used += width;
    }
    Some(format!("{GAP}{prefix}…"))
}

pub(crate) fn fold_context_band(
    app: &App,
    line: &ViewLine,
    visible_width: usize,
    action_idx: Option<usize>,
) -> Option<FoldContextBand> {
    let region = app.fold_context_region_for_line(line)?;
    let full_label = crate::app::fold_context_label(region.hidden_lines);
    let compact_label = format!(" {} ", region.hidden_lines);
    let (up_key, label, down_key) = if let Some(idx) = action_idx {
        let up = crate::app::review::review_index_action_label("u", idx);
        let down = crate::app::review::review_index_action_label("d", idx);
        let full_width = up
            .width()
            .saturating_add(full_label.as_str().width())
            .saturating_add(down.width())
            .saturating_add(4);
        let compact_width = up
            .width()
            .saturating_add(compact_label.as_str().width())
            .saturating_add(down.width())
            .saturating_add(4);
        if visible_width >= full_width {
            (format!("{up} "), full_label, format!("{down} "))
        } else if visible_width >= compact_width {
            (format!("{up} "), compact_label, format!("{down} "))
        } else {
            (up, String::new(), down)
        }
    } else if visible_width >= full_label.as_str().width().saturating_add(2) {
        (String::new(), full_label, String::new())
    } else if visible_width >= compact_label.as_str().width().saturating_add(2) {
        (String::new(), compact_label, String::new())
    } else {
        (String::new(), String::new(), String::new())
    };
    let muted = if app.theme.background.is_none()
        && app.theme.background_panel.is_none()
        && app.theme.background_element.is_none()
        && app.theme_is_light
    {
        app.theme.text
    } else {
        app.theme.text_muted
    };
    let up_key_width = up_key.as_str().width() as u16;
    let down_key_width = down_key.as_str().width() as u16;
    let top_x = 0;
    let top_width = up_key_width.saturating_add("↑".width() as u16);
    let bottom_x = up_key
        .as_str()
        .width()
        .saturating_add("↑".width())
        .saturating_add(label.as_str().width()) as u16;
    let bottom_width = down_key_width.saturating_add("↓".width() as u16);
    let button_style = |direction| {
        if app.fold_context_hover == Some((region.key, direction)) {
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted)
        }
    };
    let controls_width = bottom_x as usize + bottom_width as usize;
    let scope_hint = region
        .scope_hint
        .as_deref()
        .and_then(|hint| fold_scope_hint(hint, visible_width.saturating_sub(controls_width)));
    let mut spans = vec![
        Span::styled(up_key, button_style(FoldContextDirection::Top)),
        Span::styled("↑", button_style(FoldContextDirection::Top)),
        Span::styled(label, Style::default().fg(muted)),
        Span::styled(down_key, button_style(FoldContextDirection::Bottom)),
        Span::styled("↓", button_style(FoldContextDirection::Bottom)),
    ];
    if let Some(scope_hint) = scope_hint {
        spans.push(Span::styled(
            scope_hint,
            Style::default().fg(muted).add_modifier(Modifier::DIM),
        ));
    }
    Some(FoldContextBand {
        key: region.key,
        spans,
        top_x,
        top_width,
        bottom_x,
        bottom_width,
    })
}

pub(crate) fn review_note_line_spans(
    app: &App,
    overlay: &ReviewCommentOverlay,
    line: &str,
) -> Vec<Span<'static>> {
    let anchor_key = &overlay.anchor_key;
    let highlighted = app.review_preview_hover_id == Some(overlay.id)
        || (app.review_preview_hover_id.is_none()
            && app.review_preview_hover.as_deref() == Some(anchor_key))
        || app.review_preview_flash_active(overlay.id, anchor_key);
    let reply_hovered = app.review_preview_reply_hover == Some(overlay.id);
    let resolve_hovered = app.review_preview_resolve_hover == Some(overlay.id);
    let delete_hovered = app.review_preview_delete_hover == Some(overlay.id);
    let overflow_hovered = app.review_preview_overflow_hover == Some(overlay.id);
    let resolved = overlay.resolved;
    let border = Style::default().fg(if highlighted {
        app.theme.accent
    } else if resolved || overlay.outdated {
        app.theme.text_muted
    } else {
        app.theme.warning
    });
    let title = Style::default()
        .fg(if resolved {
            app.theme.text_muted
        } else {
            app.theme.warning
        })
        .add_modifier(Modifier::BOLD);
    let text = Style::default().fg(if resolved {
        app.theme.text_muted
    } else {
        app.theme.text
    });
    let key = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let delete = if delete_hovered {
        Style::default()
            .fg(app.theme.error)
            .add_modifier(Modifier::BOLD)
    } else {
        text
    };

    if let Some(rest) = line
        .strip_prefix("╭ ")
        .and_then(|line| line.strip_suffix('╮'))
    {
        let (label, rule) = rest.rsplit_once(' ').unwrap_or((rest, ""));
        return vec![
            Span::styled("╭ ".to_string(), border),
            Span::styled(label.to_string(), title),
            Span::styled(format!(" {rule}╮"), border),
        ];
    }
    if let Some(rule) = line
        .strip_prefix('╰')
        .and_then(|line| line.strip_suffix('╯'))
        .filter(|line| line.chars().all(|ch| ch == '─'))
    {
        return vec![
            Span::styled("╰".to_string(), border),
            Span::styled(rule.to_string(), border),
            Span::styled("╯".to_string(), border),
        ];
    }
    if let Some(rest) = line
        .strip_prefix("╰ ")
        .and_then(|line| line.strip_suffix('╯'))
    {
        let (label, rule) = rest
            .rsplit_once(' ')
            .filter(|(_, rule)| rule.chars().all(|ch| ch == '─'))
            .unwrap_or((rest, ""));
        let mut spans = vec![Span::styled("╰ ".to_string(), border)];
        for (idx, action) in label.split("   ").enumerate() {
            if idx > 0 {
                spans.push(Span::styled("   ".to_string(), text));
            }
            if let Some((action_key, action_label)) = action.split_once(' ') {
                let is_delete = action_key.starts_with('x');
                let is_reply = action_key.starts_with('r');
                let is_resolve = action_key.starts_with('v');
                let is_overflow = action_key.starts_with('o');
                let edit_hovered = action_key.starts_with('i')
                    && app.review_preview_edit_hover == Some(overlay.id);
                let hovered = app.pr_comment_action_hover_key.as_deref() == Some(action_key)
                    || edit_hovered
                    || (is_reply && reply_hovered)
                    || (is_resolve && resolve_hovered)
                    || (is_overflow && overflow_hovered);
                let delete_active = is_delete && (hovered || delete_hovered);
                let action_key_style = if delete_active {
                    Style::default()
                        .fg(app.theme.error)
                        .add_modifier(Modifier::BOLD)
                } else if is_resolve && hovered {
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    key
                };
                let action_label_style = if delete_active {
                    action_key_style
                } else if hovered {
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else if is_delete {
                    delete
                } else {
                    text
                };
                spans.push(Span::styled(action_key.to_string(), action_key_style));
                spans.push(Span::styled(format!(" {action_label}"), action_label_style));
            } else {
                spans.push(Span::styled(action.to_string(), text));
            }
        }
        if !rule.is_empty() {
            spans.push(Span::styled(format!(" {rule}"), border));
        }
        spans.push(Span::styled("╯".to_string(), border));
        return spans;
    }
    vec![Span::styled(line.to_string(), text)]
}

fn take_width_prefix(text: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width) > max_width {
            break;
        }
        width = width.saturating_add(ch_width);
        out.push(ch);
    }
    out
}

fn take_width_suffix(text: &str, max_width: usize) -> String {
    let mut out = Vec::new();
    let mut width = 0usize;
    for ch in text.chars().rev() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width) > max_width {
            break;
        }
        width = width.saturating_add(ch_width);
        out.push(ch);
    }
    out.into_iter().rev().collect()
}

fn truncate_middle_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let keep = max_width.saturating_sub(1);
    let head_width = keep.div_ceil(2);
    let tail_width = keep.saturating_sub(head_width);
    format!(
        "{}…{}",
        take_width_prefix(text, head_width),
        take_width_suffix(text, tail_width)
    )
}

#[derive(Clone)]
pub(crate) struct ReviewNoteAvatar {
    pub(crate) url: Option<String>,
    pub(crate) seed: String,
    pub(crate) row_offset: usize,
    pub(crate) x_offset: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

#[derive(Clone, Copy)]
enum ReviewActionKind {
    Edit,
    Reply,
    Resolve,
    Delete,
    Overflow,
}

#[derive(Clone, Default, Debug)]
pub(crate) struct ReviewNoteActionHits {
    pub(crate) edit: Option<(usize, u16, u16)>,
    pub(crate) reply: Option<(usize, u16, u16)>,
    pub(crate) resolve: Option<(usize, u16, u16)>,
    pub(crate) delete: Option<(usize, u16, u16)>,
    pub(crate) overflow: Option<(usize, u16, u16)>,
}

#[derive(Clone)]
pub(crate) struct ReviewNoteBlock {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) avatar: Option<ReviewNoteAvatar>,
    pub(crate) snapshot_rows: Option<(usize, usize)>,
}

fn wrap_review_card_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    if width == 0 {
        return vec![spans];
    }
    let mut lines = vec![Vec::new()];
    let mut col = 0usize;
    for span in spans {
        let style = span.style;
        let mut buf = String::new();
        for grapheme in span.content.graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if col > 0 && col.saturating_add(grapheme_width) > width {
                if !buf.is_empty() {
                    lines.last_mut().unwrap().push(Span::styled(buf, style));
                    buf = String::new();
                }
                lines.push(Vec::new());
                col = 0;
            }
            buf.push_str(grapheme);
            col = col.saturating_add(grapheme_width);
        }
        if !buf.is_empty() {
            lines.last_mut().unwrap().push(Span::styled(buf, style));
        }
    }
    lines
}

fn review_card_content_line(
    border: Style,
    text: Style,
    content_width: usize,
    content: Vec<Span<'static>>,
    fill_wrapped_background: bool,
) -> Line<'static> {
    let used = spans_width(&content);
    let fill = if fill_wrapped_background {
        let mut backgrounds = content
            .iter()
            .filter(|span| !span.content.is_empty())
            .map(|span| span.style.bg);
        match backgrounds.next().flatten() {
            Some(bg) if backgrounds.all(|candidate| candidate == Some(bg)) => {
                Style::default().bg(bg)
            }
            _ => text,
        }
    } else {
        text
    };
    let mut spans = vec![Span::styled("│ ".to_string(), border)];
    spans.extend(content);
    if used < content_width {
        spans.push(Span::styled(" ".repeat(content_width - used), fill));
    }
    spans.push(Span::styled(" │".to_string(), border));
    Line::from(spans)
}

fn review_snapshot_code_spans(
    app: &mut App,
    path: &str,
    code: &str,
    code_color: Color,
    accent: Color,
) -> Vec<Vec<Span<'static>>> {
    let snapshot = code.lines().any(|line| line.starts_with("→ "));
    let marked = code
        .lines()
        .map(|line| line.starts_with("→ "))
        .collect::<Vec<_>>();
    let source = if snapshot {
        code.lines()
            .map(|line| {
                line.strip_prefix("→ ")
                    .or_else(|| line.strip_prefix("  "))
                    .unwrap_or(line)
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        code.to_string()
    };
    let mut rows = app.preview_source_spans(path, &source).unwrap_or_else(|| {
        let mut rows = source
            .lines()
            .map(|line| {
                vec![Span::styled(
                    line.to_string(),
                    Style::default().fg(code_color),
                )]
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            rows.push(vec![Span::styled("", Style::default().fg(code_color))]);
        }
        rows
    });
    if snapshot {
        for (idx, row) in rows.iter_mut().enumerate() {
            if marked.get(idx).copied().unwrap_or(false) {
                for span in row.iter_mut() {
                    span.style = span.style.add_modifier(Modifier::BOLD);
                }
                row.insert(
                    0,
                    Span::styled(
                        "→ ",
                        Style::default().fg(accent).add_modifier(Modifier::BOLD),
                    ),
                );
            } else {
                row.insert(0, Span::styled("  ", Style::default().fg(code_color)));
            }
        }
    }
    rows
}

fn review_note_action_rows(overlay: &ReviewCommentOverlay) -> Vec<Vec<(ReviewActionKind, String)>> {
    let mut actions = Vec::new();
    if overlay.can_edit {
        if let Some(label) = overlay.edit_label.as_ref() {
            actions.push((ReviewActionKind::Edit, format!("{label} edit")));
        }
    }
    if let Some(label) = overlay.reply_label.as_ref() {
        actions.push((ReviewActionKind::Reply, format!("{label} reply")));
    }
    if let Some(label) = overlay.resolve_label.as_ref() {
        let action = if overlay.resolved {
            "unresolve"
        } else {
            "resolve"
        };
        actions.push((ReviewActionKind::Resolve, format!("{label} {action}")));
    }
    if overlay.can_edit {
        if let Some(label) = overlay.delete_label.as_ref() {
            actions.push((ReviewActionKind::Delete, format!("{label} delete")));
        }
    }
    if let Some(label) = overlay.overflow_label.as_ref() {
        actions.push((ReviewActionKind::Overflow, format!("{label} …")));
    }
    (!actions.is_empty())
        .then_some(actions)
        .into_iter()
        .collect()
}

fn review_note_block_inner(
    app: &mut App,
    overlay: &ReviewCommentOverlay,
    visible_width: usize,
    with_avatar: bool,
    footer_label: Option<&str>,
) -> ReviewNoteBlock {
    const AVATAR_WIDTH: u16 = 2;
    const AVATAR_HEIGHT: u16 = 2;

    let max_width = visible_width.max(12);
    let content_width = max_width.saturating_sub(4).max(1);
    let avatar = (with_avatar && app.image_picker.is_some() && content_width > 8).then(|| {
        ReviewNoteAvatar {
            url: overlay.avatar_url.clone(),
            seed: overlay.avatar_seed.clone(),
            row_offset: 0,
            x_offset: 2,
            width: AVATAR_WIDTH,
            height: AVATAR_HEIGHT,
        }
    });
    let avatar_prefix_width = avatar
        .as_ref()
        .map(|_| AVATAR_WIDTH as usize + 1)
        .unwrap_or(0);
    let title_text = if overlay.resolved {
        format!("✓ {}", overlay.title)
    } else {
        overlay.title.clone()
    };
    let title = truncate_middle_width(
        &title_text,
        max_width.saturating_sub(4 + avatar_prefix_width),
    );
    let rule = "─".repeat(
        max_width.saturating_sub(UnicodeWidthStr::width(title.as_str()) + 4 + avatar_prefix_width),
    );
    let key = &overlay.anchor_key;
    let mut lines = vec![Line::from(review_note_line_spans(
        app,
        overlay,
        &format!("╭ {}{title} {rule}╮", " ".repeat(avatar_prefix_width)),
    ))];

    let body = if overlay.body.is_empty() {
        "(empty)"
    } else {
        overlay.body.as_str()
    };
    let theme = app.theme.clone();
    let syntax_path = overlay.syntax_path.clone();
    let snapshot_code = overlay.snapshot_code.clone();
    let mut highlight = |_lang: Option<&str>, code: &str| {
        let path = syntax_path.as_deref()?;
        let snapshot = snapshot_code.as_deref()?;
        (code.trim_end_matches(['\r', '\n']) == snapshot)
            .then(|| review_snapshot_code_spans(app, path, code, theme.diff_context, theme.accent))
    };
    let (mut body_lines, _) =
        markdown_preview_lines(body, &theme, content_width, None, &mut highlight, None);
    let snapshot_label = overlay.outdated.then(|| {
        body_lines.iter().rposition(|line| {
            line.spans
                .first()
                .is_some_and(|span| span.content.trim_start().starts_with("Snapshot "))
        })
    });
    let snapshot_label = snapshot_label.flatten();
    if let Some(label) = snapshot_label.and_then(|row| body_lines.get_mut(row)) {
        if let Some(name) = label.spans.first_mut() {
            name.style = Style::default().fg(theme.text_muted);
        }
    }
    let border = Style::default().fg(
        if app.review_preview_hover_id == Some(overlay.id)
            || (app.review_preview_hover_id.is_none()
                && app.review_preview_hover.as_deref() == Some(key))
            || app.review_preview_flash_active(overlay.id, key)
        {
            app.theme.accent
        } else if overlay.resolved || overlay.outdated {
            app.theme.text_muted
        } else {
            app.theme.warning
        },
    );
    let text = Style::default().fg(if overlay.resolved {
        app.theme.text_muted
    } else {
        app.theme.text
    });
    lines.push(review_card_content_line(
        border,
        text,
        content_width,
        Vec::new(),
        false,
    ));
    let mut snapshot_start = None;
    for (row, line) in body_lines.into_iter().enumerate() {
        let wrapped_lines = wrap_review_card_spans(line.spans, content_width);
        let wrapped = wrapped_lines.len() > 1;
        for spans in wrapped_lines {
            lines.push(review_card_content_line(
                border,
                text,
                content_width,
                spans,
                wrapped,
            ));
        }
        if Some(row) == snapshot_label && overlay.snapshot_code.is_some() {
            snapshot_start = Some(lines.len());
        }
    }
    let snapshot_rows = snapshot_start.map(|start| (start, lines.len()));
    lines.push(review_card_content_line(
        border,
        text,
        content_width,
        Vec::new(),
        false,
    ));
    if let Some(footer_label) = footer_label {
        let bottom_rule = if footer_label.is_empty() {
            "─".repeat(max_width.saturating_sub(2))
        } else {
            "─".repeat(max_width.saturating_sub(UnicodeWidthStr::width(footer_label) + 4))
        };
        let bottom_line = if footer_label.is_empty() {
            format!("╰{bottom_rule}╯")
        } else {
            format!("╰ {footer_label} {bottom_rule}╯")
        };
        lines.push(Line::from(review_note_line_spans(
            app,
            overlay,
            &bottom_line,
        )));
    } else {
        let action_rows = review_note_action_rows(overlay);
        if action_rows.is_empty() {
            lines.push(Line::from(review_note_line_spans(
                app,
                overlay,
                &format!("╰{}╯", "─".repeat(max_width.saturating_sub(2))),
            )));
        } else {
            let last_row = action_rows.len().saturating_sub(1);
            for (row_idx, row) in action_rows.into_iter().enumerate() {
                let labels = row.into_iter().map(|(_, label)| label).collect::<Vec<_>>();
                let label = labels.join("   ");
                let mut spans = review_note_line_spans(app, overlay, &format!("╰ {label} ╯"));
                if row_idx != last_row {
                    spans.first_mut().unwrap().content = "├ ".into();
                    spans.last_mut().unwrap().content = "┤".into();
                }
                let rule = "─"
                    .repeat(max_width.saturating_sub(UnicodeWidthStr::width(label.as_str()) + 4));
                let last = spans.len().saturating_sub(1);
                spans.insert(last, Span::styled(format!(" {rule}"), border));
                lines.push(Line::from(spans));
            }
        }
    }
    if overlay.thread_continues {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("│", Style::default().fg(app.theme.text_muted)),
        ]));
    }
    ReviewNoteBlock {
        lines,
        avatar,
        snapshot_rows,
    }
}

pub(crate) fn review_note_block(
    app: &mut App,
    overlay: &ReviewCommentOverlay,
    visible_width: usize,
) -> ReviewNoteBlock {
    review_note_block_inner(app, overlay, visible_width, true, None)
}

pub(crate) fn review_note_block_with_footer(
    app: &mut App,
    overlay: &ReviewCommentOverlay,
    visible_width: usize,
    footer_label: &str,
) -> ReviewNoteBlock {
    review_note_block_inner(app, overlay, visible_width, true, Some(footer_label))
}

#[derive(Clone, Debug)]
pub(crate) struct ReviewPreviewRow {
    pub(crate) id: u64,
    pub(crate) row_idx: usize,
    pub(crate) row_span: usize,
    pub(crate) anchor_key: String,
    pub(crate) indent: u16,
    pub(crate) actions: ReviewNoteActionHits,
}

fn review_action_width(label: Option<&str>, action: &str) -> u16 {
    label
        .map(|label| format!("{label} {action}").width() as u16)
        .unwrap_or(0)
}

pub(crate) fn review_note_edit_width(overlay: &ReviewCommentOverlay) -> u16 {
    if overlay.can_edit {
        review_action_width(overlay.edit_label.as_deref(), "edit")
    } else {
        0
    }
}

pub(crate) fn review_note_reply_width(overlay: &ReviewCommentOverlay) -> u16 {
    review_action_width(overlay.reply_label.as_deref(), "reply")
}

pub(crate) fn review_note_reply_x_offset(overlay: &ReviewCommentOverlay) -> u16 {
    let edit_width = review_note_edit_width(overlay);
    if edit_width == 0 {
        2
    } else {
        2u16.saturating_add(edit_width).saturating_add(3)
    }
}

pub(crate) fn review_note_resolve_width(overlay: &ReviewCommentOverlay) -> u16 {
    review_action_width(
        overlay.resolve_label.as_deref(),
        if overlay.resolved {
            "unresolve"
        } else {
            "resolve"
        },
    )
}

pub(crate) fn review_note_resolve_x_offset(overlay: &ReviewCommentOverlay) -> u16 {
    let reply_width = review_note_reply_width(overlay);
    let mut offset = review_note_reply_x_offset(overlay);
    if reply_width > 0 {
        offset = offset.saturating_add(reply_width).saturating_add(3);
    }
    offset
}

pub(crate) fn review_note_delete_x_offset(overlay: &ReviewCommentOverlay) -> u16 {
    let resolve_width = review_note_resolve_width(overlay);
    let mut offset = review_note_resolve_x_offset(overlay);
    if resolve_width > 0 {
        offset = offset.saturating_add(resolve_width).saturating_add(3);
    }
    offset
}

pub(crate) fn review_note_delete_width(overlay: &ReviewCommentOverlay) -> u16 {
    if !overlay.can_edit {
        return 0;
    }
    review_action_width(overlay.delete_label.as_deref(), "delete")
}

pub(crate) fn review_preview_row(
    row_idx: usize,
    row_span: usize,
    anchor_key: String,
    overlay: &ReviewCommentOverlay,
) -> ReviewPreviewRow {
    let rows = review_note_action_rows(overlay);
    let trailing_rows = usize::from(overlay.thread_continues);
    let first_row = row_span.saturating_sub(rows.len().saturating_add(trailing_rows));
    let mut actions = ReviewNoteActionHits::default();
    for (row_offset, row) in rows.into_iter().enumerate() {
        let mut x = 2u16;
        for (kind, label) in row {
            let width = UnicodeWidthStr::width(label.as_str()) as u16;
            let hit = Some((first_row.saturating_add(row_offset), x, width));
            match kind {
                ReviewActionKind::Edit => actions.edit = hit,
                ReviewActionKind::Reply => actions.reply = hit,
                ReviewActionKind::Resolve => actions.resolve = hit,
                ReviewActionKind::Delete => actions.delete = hit,
                ReviewActionKind::Overflow => actions.overflow = hit,
            }
            x = x.saturating_add(width).saturating_add(3);
        }
    }
    ReviewPreviewRow {
        id: overlay.id,
        row_idx,
        row_span,
        anchor_key,
        indent: 0,
        actions,
    }
}

pub(crate) fn review_note_lines(
    app: &mut App,
    overlay: &ReviewCommentOverlay,
    visible_width: usize,
) -> Vec<Line<'static>> {
    review_note_block_inner(app, overlay, visible_width, false, None).lines
}

pub(crate) fn render_review_note_avatar(
    frame: &mut Frame,
    app: &App,
    content_area: Rect,
    local_row: usize,
    avatar: &ReviewNoteAvatar,
) {
    let Some(picker) = app.image_picker.as_ref().cloned() else {
        return;
    };
    let area = Rect::new(
        content_area.x.saturating_add(avatar.x_offset),
        content_area.y.saturating_add(local_row as u16),
        avatar.width,
        avatar.height,
    );
    if area.y >= content_area.y.saturating_add(content_area.height) {
        return;
    }
    let image = avatar_image(avatar.url.as_deref(), &avatar.seed);
    let Ok(protocol) =
        picker.new_protocol(image, Size::new(area.width, area.height), Resize::Fit(None))
    else {
        return;
    };
    frame.render_widget(TerminalImage::new(&protocol).allow_clipping(true), area);
}

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

pub(crate) fn reserve_diff_scrollbar_lane(app: &App, area: Rect) -> (Rect, Rect) {
    if !app.scrollbar_visible || area.width == 0 {
        return (
            area,
            Rect::new(area.x.saturating_add(area.width), area.y, 0, area.height),
        );
    }
    (
        Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height),
        Rect::new(
            area.x.saturating_add(area.width.saturating_sub(1)),
            area.y,
            1,
            area.height,
        ),
    )
}

pub(crate) fn render_diff_scrollbar(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    total_lines: usize,
    visible_lines: usize,
    scroll_offset: usize,
) {
    let track = area.inner(Margin {
        vertical: 0,
        horizontal: 0,
    });
    if track.width == 0 {
        return;
    }
    let x = track.x;
    let mut style = Style::default().fg(app.theme.text_muted);
    let track_style = app.theme.background.map(|bg| Style::default().bg(bg));
    if let Some(bg) = app.theme.background {
        style = style.bg(bg);
    }
    let buffer = frame.buffer_mut();
    for row in track.y..track.y.saturating_add(track.height) {
        if let Some(cell) = buffer.cell_mut((x, row)) {
            cell.set_symbol(" ");
            if let Some(track_style) = track_style {
                cell.set_style(track_style);
            }
        }
    }
    let Some((thumb_top, thumb_height)) =
        diff_scrollbar_thumb(total_lines, visible_lines, track.height, scroll_offset)
    else {
        return;
    };
    app.set_diff_scrollbar(DiffScrollbarState {
        x,
        y: track.y,
        height: track.height,
        total_lines,
        visible_lines,
        thumb_top,
        thumb_height,
    });
    let symbol = if app.file_list_focused || app.file_filter_active {
        "▕"
    } else {
        "▐"
    };
    let start = track.y.saturating_add(thumb_top);
    let end = start
        .saturating_add(thumb_height)
        .min(track.y.saturating_add(track.height));
    for row in start..end {
        if let Some(cell) = buffer.cell_mut((x, row)) {
            cell.set_symbol(symbol).set_style(style);
        }
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

fn review_note_is_footer(spans: &[Span]) -> bool {
    spans_to_text(spans).trim_start().starts_with('╰')
}

pub(crate) fn fit_review_note_footer(
    spans: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    if !review_note_is_footer(&spans) || width == 0 || spans_width(&spans) <= width {
        return spans;
    }
    let border_style = spans.last().map(|span| span.style).unwrap_or_default();
    let mut clipped = slice_spans(&spans, 0, width.saturating_sub(1));
    clipped.push(Span::styled("╯", border_style));
    clipped
}

pub(crate) fn review_note_wrap_count(spans: &[Span], wrap_width: usize) -> usize {
    if review_note_is_footer(spans) {
        1
    } else {
        wrap_count_for_spans(spans, wrap_width)
    }
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

use crate::app::{AnimationPhase, ViewMode};
use crate::color;
use crate::config::{DiffExtentMarkerMode, DiffExtentMarkerScope, ResolvedTheme};
use ratatui::{layout::Alignment, widgets::Paragraph};

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
    let no_step_hunk_line = !app.stepping && view_line.hunk_index.is_some();
    if !view_line.show_hunk_extent && !no_step_hunk_line {
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

pub(crate) fn extent_marker_text<'a>(
    marker: &'a str,
    deleted_marker: &'a str,
    view_line: &ViewLine,
) -> &'a str {
    if matches!(view_line.kind, LineKind::Deleted | LineKind::PendingDelete) {
        deleted_marker
    } else {
        marker
    }
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
        ViewMode::Preview => {}
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

fn review_file_comment_action(app: &App) -> Option<(Vec<Span<'static>>, usize)> {
    if !app.file_review_comments_supported() || app.review_editor_active() {
        return None;
    }
    let key = app.keybindings.normal_keys(NormalAction::LineComment);
    let label = "comment";
    let width = key.width().saturating_add(1).saturating_add(label.width());
    let key_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let label_style = if app.review_file_comment_hover {
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text_muted)
    };
    Some((
        vec![
            Span::styled(key, key_style),
            Span::raw(" "),
            Span::styled(label.to_string(), label_style),
        ],
        width,
    ))
}

fn review_file_comment_card(
    app: &mut App,
    width: usize,
) -> Option<(ReviewCommentOverlay, Vec<Line<'static>>)> {
    let overlay = app.review_file_comment_overlay()?;
    let lines = review_note_lines(app, &overlay, width);
    Some((overlay, lines))
}

fn add_review_file_comment_card_hits(
    app: &mut App,
    area: Rect,
    y: u16,
    row: usize,
    visible_height: u16,
    overlay: &ReviewCommentOverlay,
) {
    app.add_review_comment_preview_box(
        area.x,
        y.saturating_add(row as u16),
        area.width,
        visible_height,
        overlay.id,
        overlay.anchor_key.clone(),
    );
    let preview = review_preview_row(
        row,
        visible_height as usize,
        overlay.anchor_key.clone(),
        overlay,
    );
    if let Some((offset, x, width)) = preview.actions.edit {
        app.add_review_preview_edit_box(
            area.x.saturating_add(x),
            y.saturating_add(row as u16).saturating_add(offset as u16),
            width,
            1,
            overlay.id,
            overlay.anchor_key.clone(),
        );
    }
    if let Some((offset, x, width)) = preview.actions.reply {
        app.add_review_preview_reply_box(
            area.x.saturating_add(x),
            y.saturating_add(row as u16).saturating_add(offset as u16),
            width,
            1,
            overlay.id,
            overlay.anchor_key.clone(),
        );
    }
    if let Some((offset, x, width)) = preview.actions.resolve {
        app.add_review_preview_resolve_box(
            area.x.saturating_add(x),
            y.saturating_add(row as u16).saturating_add(offset as u16),
            width,
            1,
            overlay.id,
            overlay.anchor_key.clone(),
        );
    }
    if let Some((offset, x, width)) = preview.actions.delete {
        app.add_review_preview_delete_box(
            area.x.saturating_add(x),
            y.saturating_add(row as u16).saturating_add(offset as u16),
            width,
            1,
            overlay.id,
            overlay.anchor_key.clone(),
        );
    }
    if let Some((offset, x, width)) = preview.actions.overflow {
        app.add_review_preview_overflow_box(
            area.x.saturating_add(x),
            y.saturating_add(row as u16).saturating_add(offset as u16),
            width,
            1,
            overlay.id,
            overlay.anchor_key.clone(),
        );
    }
}

pub(crate) fn render_binary_empty_state(frame: &mut Frame, app: &mut App, area: Rect) {
    app.binary_preview_hit = None;
    app.set_review_file_comment_hit(None);

    if let Some(bg) = app.theme.background {
        let bg_fill = Paragraph::new("").style(Style::default().bg(bg));
        frame.render_widget(bg_fill, area);
    }

    let comment_action = review_file_comment_action(app);
    let comment_card = review_file_comment_card(app, area.width as usize);

    if !app.current_file_is_image() {
        if comment_action.is_none() && comment_card.is_none() {
            render_empty_state_text(
                frame,
                area,
                &app.theme,
                "Binary file (preview disabled)",
                false,
            );
            return;
        }

        let mut lines = vec![Line::from(Span::styled(
            "Binary file (preview disabled)",
            Style::default().fg(app.theme.text_muted),
        ))];
        let action_row = comment_action.as_ref().map(|_| lines.len());
        if let Some((comment_spans, _)) = comment_action.as_ref() {
            lines.push(Line::from(comment_spans.clone()));
        }
        let card_row = comment_card.as_ref().map(|_| lines.len().saturating_add(1));
        if let Some((_, card_lines)) = comment_card.as_ref() {
            lines.push(Line::from(""));
            lines.extend(card_lines.clone());
        }

        let height = (lines.len() as u16).min(area.height);
        let y = area
            .y
            .saturating_add(area.height.saturating_sub(height) / 2);
        if let (Some(row), Some((_, comment_width))) = (action_row, comment_action.as_ref()) {
            if row < height as usize {
                let comment_width = *comment_width;
                let x = area
                    .x
                    .saturating_add(area.width.saturating_sub(comment_width as u16) / 2);
                app.set_review_file_comment_hit(Some((
                    x,
                    y.saturating_add(row as u16),
                    comment_width.min(area.width as usize) as u16,
                    1,
                )));
            }
        }
        if let (Some(row), Some((overlay, card_lines))) = (card_row, comment_card.as_ref()) {
            if row < height as usize {
                let visible_height = card_lines.len().min(height as usize - row) as u16;
                if visible_height == card_lines.len() as u16 {
                    add_review_file_comment_card_hits(app, area, y, row, visible_height, overlay);
                } else {
                    app.add_review_preview_box(
                        area.x,
                        y.saturating_add(row as u16),
                        area.width,
                        visible_height,
                        overlay.anchor_key.clone(),
                    );
                }
            }
        }

        frame.render_widget(
            Paragraph::new(lines).alignment(Alignment::Center),
            Rect::new(area.x, y, area.width, height),
        );
        return;
    }

    let key = app
        .keybindings
        .global_keys(GlobalAction::OpenCommandPalette);
    let label = "preview";
    let preview_width = key.width().saturating_add(1).saturating_add(label.width());
    let gap = usize::from(comment_action.is_some()) * 2;
    let comment_width = comment_action
        .as_ref()
        .map(|(_, width)| *width)
        .unwrap_or(0);
    let width = preview_width
        .saturating_add(gap)
        .saturating_add(comment_width);
    let key_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let label_style = if app.binary_preview_hover {
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text_muted)
    };
    let mut spans = vec![
        Span::styled(key, key_style),
        Span::raw(" "),
        Span::styled(label.to_string(), label_style),
    ];
    if let Some((comment_spans, _)) = comment_action.as_ref() {
        spans.push(Span::raw("  "));
        spans.extend(comment_spans.clone());
    }

    let mut lines = vec![Line::from(spans)];
    let card_row = comment_card.as_ref().map(|_| lines.len().saturating_add(1));
    if let Some((_, card_lines)) = comment_card.as_ref() {
        lines.push(Line::from(""));
        lines.extend(card_lines.clone());
    }
    let height = (lines.len() as u16).min(area.height);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    let x = area
        .x
        .saturating_add(area.width.saturating_sub(width as u16) / 2);
    app.binary_preview_hit = Some((x, y, preview_width.min(area.width as usize) as u16, 1));
    if comment_action.is_some() && width <= area.width as usize {
        app.set_review_file_comment_hit(Some((
            x.saturating_add((preview_width + gap) as u16),
            y,
            comment_width as u16,
            1,
        )));
    }
    if let (Some(row), Some((overlay, card_lines))) = (card_row, comment_card.as_ref()) {
        if row < height as usize {
            let visible_height = card_lines.len().min(height as usize - row) as u16;
            if visible_height == card_lines.len() as u16 {
                add_review_file_comment_card_hits(app, area, y, row, visible_height, overlay);
            } else {
                app.add_review_preview_box(
                    area.x,
                    y.saturating_add(row as u16),
                    area.width,
                    visible_height,
                    overlay.anchor_key.clone(),
                );
            }
        }
    }

    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        Rect::new(area.x, y, area.width, height),
    );
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
    let (primary_text, show_hint) = if is_binary {
        ("Binary file (preview disabled)", false)
    } else if has_changes {
        ("No content at this step", true)
    } else {
        ("No changes in this file", false)
    };
    render_empty_state_text(frame, area, theme, primary_text, show_hint);
}

fn render_empty_state_text(
    frame: &mut Frame,
    area: Rect,
    theme: &ResolvedTheme,
    primary_text: &str,
    show_hint: bool,
) {
    // Fill entire area with background
    if let Some(bg) = theme.background {
        let bg_fill = Paragraph::new("").style(Style::default().bg(bg));
        frame.render_widget(bg_fill, area);
    }

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
