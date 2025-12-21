//! View rendering modules

mod evolution;
mod split;
mod single_pane;

pub use evolution::render_evolution;
pub use split::render_split;
pub use single_pane::render_single_pane;

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Render empty state message centered in area.
/// Shows hint line only if viewport has enough height and width.
fn render_empty_state(frame: &mut Frame, area: Rect) {
    let primary = Line::from(Span::styled(
        "No content at this step",
        Style::default().fg(Color::DarkGray),
    ));

    let show_hint = area.height >= 2 && area.width >= 28;
    let (lines, height) = if show_hint {
        let hint = Line::from(Span::styled(
            "j/k to step, h/l for hunks",
            Style::default()
                .fg(Color::DarkGray)
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
