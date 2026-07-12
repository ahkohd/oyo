use crate::config::FoldContextMode;
use ratatui::style::Color;
use ratatui_comfy_toaster::{ToastBorderMode, ToastBuilder, ToastType};
use std::time::Duration;

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
            Self::CommentDeleted | Self::CommentsCleared => ToastType::Warning,
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
            | Self::SelectionActionStarted(_) => ToastType::Success,
            _ => ToastType::Info,
        }
    }
}

/// Leading icon for the message, colored by severity in the renderer.
pub(crate) fn toast_icon(toast_type: ToastType) -> char {
    match toast_type {
        ToastType::Success => '✓',
        ToastType::Error => '✕',
        ToastType::Warning => '▲',
        ToastType::Info => '●',
    }
}

pub(crate) fn toast_builder(event: ToastEvent, bg: Color) -> ToastBuilder {
    let toast_type = event.toast_type();
    let message = format!("{} {}", toast_icon(toast_type), event.message());
    ToastBuilder::new(message.into())
        .toast_type(toast_type)
        .toast_bg(bg)
        // Side rails (not Full) keep the toast area tight — no extra top/bottom
        // border rows — so the re-skinned box has no wasted margin.
        .border_mode(ToastBorderMode::SideRails)
        .duration(Duration::from_secs(2))
}
