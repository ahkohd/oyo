use crate::config::{FoldContextMode, ResolvedTheme, ToastPositionConfig};
use ratatui::style::Color;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
    text::Line,
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
};
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToastEvent {
    CopiedSelection,
    CopiedLine,
    CopiedHunk,
    CopiedPatch,
    CopiedPath,
    CopiedSessionName,
    CopiedToast,
    CopyFailed,
    LineWrap(bool),
    Syntax(bool),
    Zen(bool),
    IncrementalReview(bool),
    IncrementalReviewUnavailable,
    Animation(bool),
    Stepping(bool),
    Strikethrough(bool),
    FoldContext(FoldContextMode),
    EvoSyntaxFull(bool),
    PreviewRendered(bool),
    CommentSaved,
    CommentDeleted,
    CommentsCleared,
    ReviewSubmitted,
    SessionRenamed,
    SettingsSaved,
    SettingsReverted,
    SelectionActionStarted(String),
    SelectionActionFailed(String),
}

impl ToastEvent {
    fn message(&self) -> String {
        match self {
            Self::CopiedSelection => "Selection copied".to_string(),
            Self::CopiedLine => "Line copied".to_string(),
            Self::CopiedHunk => "Hunk copied".to_string(),
            Self::CopiedPatch => "Patch copied".to_string(),
            Self::CopiedPath => "Path copied".to_string(),
            Self::CopiedSessionName => "Session name copied".to_string(),
            Self::CopiedToast => "Toast copied".to_string(),
            Self::CopyFailed => "Could not copy to clipboard".to_string(),
            Self::LineWrap(true) => "Line wrap on".to_string(),
            Self::LineWrap(false) => "Line wrap off".to_string(),
            Self::Syntax(true) => "Syntax highlighting on".to_string(),
            Self::Syntax(false) => "Syntax highlighting off".to_string(),
            Self::Zen(true) => "Zen mode on".to_string(),
            Self::Zen(false) => "Zen mode off".to_string(),
            Self::IncrementalReview(true) => "Incremental review on".to_string(),
            Self::IncrementalReview(false) => "Incremental review off".to_string(),
            Self::IncrementalReviewUnavailable => {
                "Incremental review needs a saved review".to_string()
            }
            Self::Animation(true) => "Animation on".to_string(),
            Self::Animation(false) => "Animation off".to_string(),
            Self::Stepping(true) => "Step mode on".to_string(),
            Self::Stepping(false) => "Scroll mode on".to_string(),
            Self::Strikethrough(true) => "Deleted text strikethrough on".to_string(),
            Self::Strikethrough(false) => "Deleted text strikethrough off".to_string(),
            Self::FoldContext(FoldContextMode::Off) => "Full context".to_string(),
            Self::FoldContext(FoldContextMode::Expandable) => {
                "Expandable context folds".to_string()
            }
            Self::EvoSyntaxFull(true) => "Full evolution syntax on".to_string(),
            Self::EvoSyntaxFull(false) => "Context evolution syntax on".to_string(),
            Self::PreviewRendered(true) => "Preview mode".to_string(),
            Self::PreviewRendered(false) => "Source mode".to_string(),
            Self::CommentSaved => "Comment saved".to_string(),
            Self::CommentDeleted => "Comment deleted".to_string(),
            Self::CommentsCleared => "Comments cleared".to_string(),
            Self::ReviewSubmitted => "Review ready".to_string(),
            Self::SessionRenamed => "Session renamed".to_string(),
            Self::SettingsSaved => "Settings saved".to_string(),
            Self::SettingsReverted => "Settings reverted".to_string(),
            Self::SelectionActionStarted(message) if message.trim().is_empty() => {
                "Selection action started".to_string()
            }
            Self::SelectionActionStarted(message) => message.clone(),
            Self::SelectionActionFailed(message) if message.trim().is_empty() => {
                "Selection action failed".to_string()
            }
            Self::SelectionActionFailed(message) => message.clone(),
        }
    }

    fn toast_type(&self) -> ToastType {
        match self {
            Self::CopyFailed | Self::SelectionActionFailed(_) => ToastType::Error,
            Self::CommentDeleted | Self::CommentsCleared | Self::IncrementalReviewUnavailable => {
                ToastType::Warning
            }
            Self::CopiedSelection
            | Self::CopiedLine
            | Self::CopiedHunk
            | Self::CopiedPatch
            | Self::CopiedPath
            | Self::CopiedSessionName
            | Self::CopiedToast
            | Self::CommentSaved
            | Self::ReviewSubmitted
            | Self::SessionRenamed
            | Self::SettingsSaved
            | Self::SettingsReverted
            | Self::SelectionActionStarted(_) => ToastType::Success,
            _ => ToastType::Info,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ToastType {
    Success,
    Error,
    Warning,
    Info,
}

pub(crate) enum ToastMouseButton {
    Left,
    Right,
}

pub(crate) enum ToastInteraction {
    Dismissed,
    CopyRequested(String),
    None,
}

struct Notification {
    id: u64,
    message: String,
    severity: ToastType,
    expires: Option<Instant>,
}

struct PaintedNotification {
    id: u64,
    message: String,
    rect: Rect,
}

#[derive(Default)]
pub(crate) struct Toasts {
    next_id: u64,
    painted: Vec<PaintedNotification>,
    notifications: Vec<Notification>,
}

impl Toasts {
    pub(crate) fn push(&mut self, event: ToastEvent) {
        self.tick();
        if self.notifications.len() == 3 {
            self.notifications.remove(0);
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.notifications.push(Notification {
            id: self.next_id,
            message: event.message(),
            severity: event.toast_type(),
            expires: None,
        });
    }

    pub(crate) fn tick(&mut self) -> bool {
        let count = self.notifications.len();
        let now = Instant::now();
        self.notifications
            .retain(|toast| toast.expires.is_none_or(|expires| expires > now));
        count != self.notifications.len()
    }

    #[cfg(test)]
    pub(crate) fn queue_len(&self) -> usize {
        self.notifications.len()
    }

    #[cfg(test)]
    pub(crate) fn current_message(&self) -> Option<&str> {
        self.notifications
            .last()
            .map(|toast| toast.message.as_str())
    }

    #[cfg(test)]
    pub(crate) fn dismiss(&mut self) -> bool {
        self.notifications.pop().is_some()
    }

    pub(crate) fn handle_click(
        &mut self,
        column: u16,
        row: u16,
        button: ToastMouseButton,
    ) -> ToastInteraction {
        let Some(painted) = self
            .painted
            .iter()
            .find(|painted| painted.rect.contains(Position::new(column, row)))
        else {
            return ToastInteraction::None;
        };
        match button {
            ToastMouseButton::Left => {
                self.notifications.retain(|toast| toast.id != painted.id);
                ToastInteraction::Dismissed
            }
            ToastMouseButton::Right => ToastInteraction::CopyRequested(painted.message.clone()),
        }
    }

    pub(crate) fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        position: ToastPositionConfig,
        theme: &ResolvedTheme,
    ) {
        self.tick();
        self.painted.clear();
        if area.width < 7 || area.height < 3 {
            return;
        }
        let margin = u16::from(area.width > 20 && area.height > 6);
        let available = Rect::new(
            area.x + margin,
            area.y + margin,
            area.width.saturating_sub(margin * 2),
            area.height.saturating_sub(margin * 2),
        );
        let max_width = available.width.min(54);
        let mut layouts = Vec::new();
        let mut total = 0;
        for (index, toast) in self.notifications.iter().enumerate().rev() {
            let mut lines = wrap_message(&toast.message, usize::from(max_width - 6));
            let remaining = available.height.saturating_sub(total);
            if remaining < 3 {
                break;
            }
            let limit = usize::from(remaining.saturating_sub(2).min(8));
            if lines.len() > limit {
                lines.truncate(limit);
                let last = lines.last_mut().unwrap();
                while last.width() >= usize::from(max_width - 6) {
                    let end = last
                        .grapheme_indices(true)
                        .next_back()
                        .map_or(0, |(index, _)| index);
                    last.truncate(end);
                }
                last.push('…');
            }
            let width = (lines.iter().map(|line| line.width()).max().unwrap_or(0) as u16 + 6)
                .min(max_width);
            let height = lines.len() as u16 + 2;
            total += height;
            layouts.push((index, lines, width, height));
        }
        let bottom = matches!(
            position,
            ToastPositionConfig::BottomLeft | ToastPositionConfig::BottomRight
        );
        let mut y = if bottom {
            available.bottom()
        } else if position == ToastPositionConfig::Center {
            available.y + (available.height - total) / 2
        } else {
            available.y
        };
        for (index, lines, width, height) in layouts {
            let x = match position {
                ToastPositionConfig::TopLeft | ToastPositionConfig::BottomLeft => available.x,
                ToastPositionConfig::Center => available.x + (available.width - width) / 2,
                _ => available.right() - width,
            };
            if bottom {
                y -= height;
            }
            let rect = Rect::new(x, y, width, height);
            let toast = &mut self.notifications[index];
            self.painted.push(PaintedNotification {
                id: toast.id,
                message: toast.message.clone(),
                rect,
            });
            toast
                .expires
                .get_or_insert_with(|| Instant::now() + Duration::from_secs(2));
            let bg = theme.background.unwrap_or(Color::Reset);
            Clear.render(rect, buffer);
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.border_subtle))
                .style(Style::default().bg(bg))
                .render(rect, buffer);
            let (icon, color) = match toast.severity {
                ToastType::Success => ("✓", theme.success),
                ToastType::Error => ("✕", theme.error),
                ToastType::Warning => ("▲", theme.warning),
                ToastType::Info => ("●", theme.info),
            };
            buffer.set_string(x + 2, y + 1, icon, Style::default().fg(color).bg(bg));
            Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>())
                .style(Style::default().fg(theme.text).bg(bg))
                .render(Rect::new(x + 4, y + 1, width - 5, height - 2), buffer);
            if !bottom {
                y += height;
            }
        }
    }
}

fn wrap_message(message: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for source in message.split('\n') {
        let mut line = String::new();
        for word in source.split_whitespace() {
            if !line.is_empty() {
                if line.width() + 1 + word.width() > width {
                    lines.push(std::mem::take(&mut line));
                } else {
                    line.push(' ');
                }
            }
            for grapheme in word.graphemes(true) {
                if line.width() + grapheme.width() > width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                if grapheme.width() <= width {
                    line.push_str(grapheme);
                } else {
                    line.push('…');
                }
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_stay_inside_every_viewport_and_click_the_visible_message() {
        for position in [
            ToastPositionConfig::TopLeft,
            ToastPositionConfig::TopRight,
            ToastPositionConfig::BottomLeft,
            ToastPositionConfig::BottomRight,
            ToastPositionConfig::Center,
        ] {
            for width in 0..65 {
                for height in 0..20 {
                    let mut toasts = Toasts::default();
                    for message in ["first", "日本語 👩‍💻 e\u{301}\nsecond line", "last"]
                    {
                        toasts.push(ToastEvent::SelectionActionStarted(message.into()));
                    }
                    let area = Rect::new(2, 3, width, height);
                    let mut buffer = Buffer::empty(area);
                    toasts.render(area, &mut buffer, position, &ResolvedTheme::default());
                    let visible: Vec<_> = toasts
                        .painted
                        .iter()
                        .map(|painted| (painted.rect, painted.message.clone()))
                        .collect();
                    for (index, (rect, message)) in visible.iter().enumerate() {
                        assert_eq!(rect.intersection(area), *rect);
                        for (other, _) in &visible[index + 1..] {
                            assert!(rect.intersection(*other).is_empty());
                        }
                        assert!(
                            matches!(toasts.handle_click(rect.x, rect.y, ToastMouseButton::Right), ToastInteraction::CopyRequested(text) if text == *message)
                        );
                    }
                    if let Some((rect, _)) = visible.last() {
                        assert!(matches!(
                            toasts.handle_click(rect.x, rect.y, ToastMouseButton::Left),
                            ToastInteraction::Dismissed
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn bursts_keep_latest_three_and_expire_without_stale_targets() {
        let mut toasts = Toasts::default();
        for index in 0..100 {
            toasts.push(ToastEvent::SelectionActionStarted(index.to_string()));
        }
        assert_eq!(toasts.queue_len(), 3);
        assert_eq!(toasts.current_message(), Some("99"));
        for toast in &mut toasts.notifications {
            toast.expires = Some(Instant::now());
        }
        assert!(toasts.tick());
        assert_eq!(toasts.queue_len(), 0);
        assert!(!toasts.tick());
    }

    #[test]
    fn expired_painted_cards_consume_clicks_until_redraw() {
        for button in [ToastMouseButton::Left, ToastMouseButton::Right] {
            let mut toasts = Toasts::default();
            toasts.push(ToastEvent::CopiedLine);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 50, 10));
            toasts.render(
                buffer.area,
                &mut buffer,
                ToastPositionConfig::TopLeft,
                &ResolvedTheme::default(),
            );
            let rect = toasts.painted[0].rect;
            toasts.notifications[0].expires = Some(Instant::now());
            assert!(toasts.tick());
            assert!(!matches!(
                toasts.handle_click(rect.x, rect.y, button),
                ToastInteraction::None
            ));
            toasts.render(
                buffer.area,
                &mut buffer,
                ToastPositionConfig::TopLeft,
                &ResolvedTheme::default(),
            );
            assert!(matches!(
                toasts.handle_click(rect.x, rect.y, ToastMouseButton::Right),
                ToastInteraction::None
            ));
        }
    }

    #[test]
    fn enqueue_eviction_preserves_painted_copy_identity_until_redraw() {
        let mut toasts = Toasts::default();
        toasts.push(ToastEvent::CopiedLine);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 50, 10));
        toasts.render(
            buffer.area,
            &mut buffer,
            ToastPositionConfig::TopLeft,
            &ResolvedTheme::default(),
        );
        let rect = toasts.painted[0].rect;
        for _ in 0..100 {
            toasts.push(ToastEvent::CopiedPath);
        }
        assert_eq!(toasts.queue_len(), 3);
        assert_eq!(toasts.painted.len(), 1);
        assert!(
            matches!(toasts.handle_click(rect.x, rect.y, ToastMouseButton::Right), ToastInteraction::CopyRequested(message) if message == "Line copied")
        );
        assert!(matches!(
            toasts.handle_click(rect.x, rect.y, ToastMouseButton::Left),
            ToastInteraction::Dismissed
        ));
        assert_eq!(toasts.queue_len(), 3);
        toasts.render(
            buffer.area,
            &mut buffer,
            ToastPositionConfig::BottomRight,
            &ResolvedTheme::default(),
        );
        assert!(matches!(
            toasts.handle_click(rect.x, rect.y, ToastMouseButton::Right),
            ToastInteraction::None
        ));
    }

    #[test]
    fn lifetime_starts_with_first_visible_frame_and_theme_colors_are_used() {
        let mut toasts = Toasts::default();
        toasts.push(ToastEvent::CopiedLine);
        assert!(toasts.notifications[0].expires.is_none());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 50, 10));
        let theme = ResolvedTheme {
            background: Some(Color::White),
            text: Color::Black,
            success: Color::Blue,
            ..ResolvedTheme::default()
        };
        toasts.render(
            buffer.area,
            &mut buffer,
            ToastPositionConfig::TopLeft,
            &theme,
        );
        let toast = &toasts.notifications[0];
        assert!(toast.expires.is_some());
        let expires = toast.expires;
        let rect = toasts.painted[0].rect;
        assert_eq!(buffer[(rect.x + 4, rect.y + 1)].fg, Color::Black);
        assert_eq!(buffer[(rect.x + 4, rect.y + 1)].bg, Color::White);
        assert_eq!(buffer[(rect.x + 2, rect.y + 1)].fg, Color::Blue);
        toasts.render(
            buffer.area,
            &mut buffer,
            ToastPositionConfig::BottomRight,
            &theme,
        );
        assert_eq!(toasts.notifications[0].expires, expires);
    }

    #[test]
    fn wrapping_preserves_multiline_graphemes_and_breaks_long_words() {
        assert_eq!(wrap_message("日本", 1), ["…", "…"]);
        assert_eq!(
            wrap_message("one two\n\n日本語", 4),
            ["one", "two", "", "日本", "語"]
        );
        assert_eq!(
            wrap_message("👩‍💻e\u{301}abcdef", 3),
            ["👩‍💻e\u{301}", "abc", "def"]
        );
    }
}
