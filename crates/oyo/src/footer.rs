use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

pub(crate) struct FooterAction<A> {
    pub(crate) action: Option<A>,
    pub(crate) spans: Vec<Span<'static>>,
}

impl<A> FooterAction<A> {
    pub(crate) fn new(action: Option<A>, spans: Vec<Span<'static>>) -> Self {
        Self { action, spans }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FooterHit<A> {
    pub(crate) area: Rect,
    pub(crate) action: A,
}

pub(crate) fn action_at<A: Copy>(hits: &[FooterHit<A>], column: u16, row: u16) -> Option<A> {
    hits.iter()
        .find(|hit| hit.area.contains((column, row).into()))
        .map(|hit| hit.action)
}

pub(crate) fn footer_line<A: Copy>(
    area: Rect,
    border: Style,
    actions: Vec<FooterAction<A>>,
) -> (Line<'static>, Vec<FooterHit<A>>) {
    let width = usize::from(area.width);
    if width < 4 || area.height == 0 {
        let edge = match width {
            0 => String::new(),
            1 => "╯".to_owned(),
            _ => format!("╰{}╯", "─".repeat(width - 2)),
        };
        return (Line::from(Span::styled(edge, border)), Vec::new());
    }
    let mut spans = vec![Span::styled("╰ ", border)];
    let mut hits = Vec::new();
    let mut used = 0;
    for action in actions {
        let action_width = action
            .spans
            .iter()
            .map(|span| span.content.width())
            .sum::<usize>();
        let gap = if used == 0 { 0 } else { 4 };
        if action_width == 0 || used + gap + action_width > width - 4 {
            continue;
        }
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        if let Some(action) = action.action {
            hits.push(FooterHit {
                area: Rect::new(
                    area.x + 2 + (used + gap) as u16,
                    area.y,
                    action_width as u16,
                    1,
                ),
                action,
            });
        }
        spans.extend(action.spans);
        used += gap + action_width;
    }
    spans.push(Span::styled(
        format!(" {}╯", "─".repeat(width - used - 4)),
        border,
    ));
    (Line::from(spans), hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footer_actions_own_only_their_visible_cells() {
        let area = Rect::new(3, 5, 32, 1);
        let (line, hits) = footer_line(
            area,
            Style::default(),
            vec![
                FooterAction::new(Some(1), vec![Span::raw("界 select")]),
                FooterAction::new(Some(2), vec![Span::raw("esc cancel")]),
            ],
        );
        assert_eq!(line.width(), 32);
        assert_eq!(action_at(&hits, 5, 5), Some(1));
        assert_eq!(action_at(&hits, 13, 5), Some(1));
        for x in [3, 4, 14, 15, 16, 17, 28, 34] {
            assert_eq!(action_at(&hits, x, 5), None);
        }
        assert_eq!(action_at(&hits, 18, 5), Some(2));
        assert_eq!(action_at(&hits, 27, 5), Some(2));
        assert_eq!(action_at(&hits, 18, 6), None);
    }

    #[test]
    fn omitted_actions_and_messages_never_create_click_targets() {
        for width in 0..40 {
            let area = Rect::new(0, 0, width, 1);
            let (line, hits) = footer_line(
                area,
                Style::default(),
                vec![
                    FooterAction::new(Some(1), vec![Span::raw("f6 f7 f8 select")]),
                    FooterAction::new(None, vec![Span::raw("Unavailable")]),
                    FooterAction::new(Some(2), vec![Span::raw("esc")]),
                ],
            );
            assert_eq!(line.width(), usize::from(width));
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert_eq!(
                hits.iter().any(|hit| hit.action == 1),
                text.contains("f6 f7 f8 select")
            );
            assert_eq!(hits.iter().any(|hit| hit.action == 2), text.contains("esc"));
            for hit in hits {
                assert_eq!(hit.area.intersection(area), hit.area);
            }
        }
        let (_, hits) = footer_line(
            Rect::new(0, 0, 30, 0),
            Style::default(),
            vec![FooterAction::new(Some(1), vec![Span::raw("enter select")])],
        );
        assert!(hits.is_empty());
    }
}
