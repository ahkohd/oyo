//! View rendering modules

mod evolution;
mod split;
mod single_pane;

pub use evolution::render_evolution;
pub use split::render_split;
pub use single_pane::render_single_pane;

use ratatui::text::Span;

pub(crate) fn spans_to_text(spans: &[Span]) -> String {
    let mut out = String::new();
    for span in spans {
        out.push_str(span.content.as_ref());
    }
    out
}

use crate::app::AnimationPhase;
use crate::color;
use crate::config::ResolvedTheme;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};

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
) -> Style {
    let color = if phase == AnimationPhase::Idle {
        base
    } else {
        let t = color::animation_t_linear(phase, progress);
        let eased = color::ease_out(t);
        let (start, end) = if backward { (base, from) } else { (from, base) };
        color::lerp_rgb_color(start, end, eased)
    };

    Style::default().fg(color)
}

/// Compute animation style for deletions using smooth fade (no pulse)
pub fn delete_style(
    phase: AnimationPhase,
    progress: f32,
    backward: bool,
    strikethrough: bool,
    base: Color,
    from: Color,
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
) -> Style {
    let color = if phase == AnimationPhase::Idle {
        base
    } else {
        let t = color::animation_t_linear(phase, progress);
        let eased = color::ease_out(t);
        let (start, end) = if backward { (base, from) } else { (from, base) };
        color::lerp_rgb_color(start, end, eased)
    };

    Style::default().fg(color)
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
