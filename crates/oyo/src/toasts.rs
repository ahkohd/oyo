use ratatui::style::Color;
use ratatui_comfy_toaster::{ToastBorderMode, ToastBuilder, ToastType};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToastEvent {
    CopiedSelection,
    CopiedLine,
    CopiedHunk,
    CopiedPatch,
    CopiedToast,
    CopyFailed,
    LineWrap(bool),
    Syntax(bool),
    Zen(bool),
    Sidebar(bool),
    Animation(bool),
    Stepping(bool),
    Strikethrough(bool),
    FoldContext(bool),
    EvoSyntaxFull(bool),
    PreviewRendered(bool),
    CommentSaved,
    CommentDeleted,
    CommentsCleared,
    ReviewSubmitted,
}

impl ToastEvent {
    fn message(self) -> &'static str {
        match self {
            Self::CopiedSelection => "Selection copied",
            Self::CopiedLine => "Line copied",
            Self::CopiedHunk => "Hunk copied",
            Self::CopiedPatch => "Patch copied",
            Self::CopiedToast => "Toast copied",
            Self::CopyFailed => "Could not copy to clipboard",
            Self::LineWrap(true) => "Line wrap on",
            Self::LineWrap(false) => "Line wrap off",
            Self::Syntax(true) => "Syntax highlighting on",
            Self::Syntax(false) => "Syntax highlighting off",
            Self::Zen(true) => "Zen mode on",
            Self::Zen(false) => "Zen mode off",
            Self::Sidebar(true) => "Sidebar shown",
            Self::Sidebar(false) => "Sidebar hidden",
            Self::Animation(true) => "Animation on",
            Self::Animation(false) => "Animation off",
            Self::Stepping(true) => "Step-through mode on",
            Self::Stepping(false) => "Scroll-only mode on",
            Self::Strikethrough(true) => "Deleted text strikethrough on",
            Self::Strikethrough(false) => "Deleted text strikethrough off",
            Self::FoldContext(true) => "Context folding on",
            Self::FoldContext(false) => "Context folding off",
            Self::EvoSyntaxFull(true) => "Full evolution syntax on",
            Self::EvoSyntaxFull(false) => "Context evolution syntax on",
            Self::PreviewRendered(true) => "Preview mode",
            Self::PreviewRendered(false) => "Source mode",
            Self::CommentSaved => "Comment saved",
            Self::CommentDeleted => "Comment deleted",
            Self::CommentsCleared => "Comments cleared",
            Self::ReviewSubmitted => "Review ready",
        }
    }

    fn toast_type(self) -> ToastType {
        match self {
            Self::CopyFailed => ToastType::Error,
            Self::CommentDeleted | Self::CommentsCleared => ToastType::Warning,
            Self::CopiedSelection
            | Self::CopiedLine
            | Self::CopiedHunk
            | Self::CopiedPatch
            | Self::CopiedToast
            | Self::CommentSaved
            | Self::ReviewSubmitted => ToastType::Success,
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
