use super::{App, TopbarTab, TopbarTabContent, ViewHistoryRecipe, ViewMode};
use crate::config::{
    BlameMode, Config, DiffExtentMarkerMode, DiffForegroundMode, DiffHighlightMode, FileCountMode,
    FilePanelPosition, FoldContextMode, GitIgnoreMode, SyntaxMode, TimeMode,
};
use crate::toasts::ToastEvent;
use crossterm::event::{KeyCode, KeyEvent};
use toml_edit::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum SettingItem {
    ViewMode,
    FoldContext,
    LineWrap,
    Zen,
    Scrollbar,
    GutterSigns,
    Watch,
    Topbar,
    AutoCenter,
    Overscroll,
    ConfirmQuit,
    StrikethroughDeletions,
    Stepping,
    Syntax,
    SyntaxTheme,
    DiffBackground,
    DiffForeground,
    DiffHighlight,
    DiffExtentMarker,
    PreviewChangeBars,
    DiffDefer,
    BlameEnabled,
    BlameMode,
    BlameHunkHint,
    TimeMode,
    FilePanelVisible,
    FilePanelPosition,
    FileCounts,
    FileGitIgnore,
    Animation,
    Autoplay,
    AutoStepOnEnter,
    AutoStepBlankFiles,
    Theme,
}

impl SettingItem {
    pub(crate) const ALL: [Self; 34] = [
        Self::ViewMode,
        Self::FoldContext,
        Self::LineWrap,
        Self::Zen,
        Self::Scrollbar,
        Self::GutterSigns,
        Self::Watch,
        Self::Topbar,
        Self::AutoCenter,
        Self::Overscroll,
        Self::ConfirmQuit,
        Self::StrikethroughDeletions,
        Self::Stepping,
        Self::Syntax,
        Self::SyntaxTheme,
        Self::DiffBackground,
        Self::DiffForeground,
        Self::DiffHighlight,
        Self::DiffExtentMarker,
        Self::PreviewChangeBars,
        Self::DiffDefer,
        Self::BlameEnabled,
        Self::BlameMode,
        Self::BlameHunkHint,
        Self::TimeMode,
        Self::FilePanelVisible,
        Self::FilePanelPosition,
        Self::FileCounts,
        Self::FileGitIgnore,
        Self::Animation,
        Self::Autoplay,
        Self::AutoStepOnEnter,
        Self::AutoStepBlankFiles,
        Self::Theme,
    ];

    pub(crate) fn uses_picker(self) -> bool {
        self.picker_cta().is_some()
    }

    pub(crate) fn picker_cta(self) -> Option<&'static str> {
        match self {
            Self::Theme => Some("change theme…"),
            Self::SyntaxTheme => Some("change syntax theme…"),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsTarget {
    Item(SettingItem),
    Save,
    Revert,
    ResetDefaults,
}

impl SettingsTarget {
    const COUNT: usize = SettingItem::ALL.len() + 3;

    fn from_index(index: usize) -> Self {
        match index % Self::COUNT {
            index if index < SettingItem::ALL.len() => Self::Item(SettingItem::ALL[index]),
            index if index == SettingItem::ALL.len() => Self::Save,
            index if index == SettingItem::ALL.len() + 1 => Self::Revert,
            _ => Self::ResetDefaults,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Item(item) => item as usize,
            Self::Save => SettingItem::ALL.len(),
            Self::Revert => SettingItem::ALL.len() + 1,
            Self::ResetDefaults => SettingItem::ALL.len() + 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsEntry {
    Spacer,
    Section(&'static str),
    Item(SettingItem),
}

pub(crate) const SETTINGS_ENTRIES: [SettingsEntry; 47] = [
    SettingsEntry::Section("General"),
    SettingsEntry::Item(SettingItem::ViewMode),
    SettingsEntry::Item(SettingItem::FoldContext),
    SettingsEntry::Item(SettingItem::LineWrap),
    SettingsEntry::Item(SettingItem::Zen),
    SettingsEntry::Item(SettingItem::Scrollbar),
    SettingsEntry::Item(SettingItem::GutterSigns),
    SettingsEntry::Item(SettingItem::Watch),
    SettingsEntry::Item(SettingItem::Topbar),
    SettingsEntry::Item(SettingItem::AutoCenter),
    SettingsEntry::Item(SettingItem::Overscroll),
    SettingsEntry::Item(SettingItem::ConfirmQuit),
    SettingsEntry::Item(SettingItem::StrikethroughDeletions),
    SettingsEntry::Item(SettingItem::Stepping),
    SettingsEntry::Spacer,
    SettingsEntry::Section("Diff"),
    SettingsEntry::Item(SettingItem::Syntax),
    SettingsEntry::Item(SettingItem::SyntaxTheme),
    SettingsEntry::Item(SettingItem::DiffBackground),
    SettingsEntry::Item(SettingItem::DiffForeground),
    SettingsEntry::Item(SettingItem::DiffHighlight),
    SettingsEntry::Item(SettingItem::DiffExtentMarker),
    SettingsEntry::Item(SettingItem::PreviewChangeBars),
    SettingsEntry::Item(SettingItem::DiffDefer),
    SettingsEntry::Spacer,
    SettingsEntry::Section("Blame"),
    SettingsEntry::Item(SettingItem::BlameEnabled),
    SettingsEntry::Item(SettingItem::BlameMode),
    SettingsEntry::Item(SettingItem::BlameHunkHint),
    SettingsEntry::Spacer,
    SettingsEntry::Section("Time"),
    SettingsEntry::Item(SettingItem::TimeMode),
    SettingsEntry::Spacer,
    SettingsEntry::Section("Files"),
    SettingsEntry::Item(SettingItem::FilePanelVisible),
    SettingsEntry::Item(SettingItem::FilePanelPosition),
    SettingsEntry::Item(SettingItem::FileCounts),
    SettingsEntry::Item(SettingItem::FileGitIgnore),
    SettingsEntry::Spacer,
    SettingsEntry::Section("Playback"),
    SettingsEntry::Item(SettingItem::Animation),
    SettingsEntry::Item(SettingItem::Autoplay),
    SettingsEntry::Item(SettingItem::AutoStepOnEnter),
    SettingsEntry::Item(SettingItem::AutoStepBlankFiles),
    SettingsEntry::Spacer,
    SettingsEntry::Section("Appearance"),
    SettingsEntry::Item(SettingItem::Theme),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsRow {
    Spacer,
    Section(&'static str),
    Item {
        item: SettingItem,
        label: &'static str,
        value: String,
        hint: &'static str,
        dirty: bool,
    },
    Actions {
        dirty: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SettingHit {
    pub(crate) target: SettingsTarget,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SettingValue {
    Bool(bool),
    Text(String),
    OptionalText(Option<String>),
}

impl SettingValue {
    fn bool(&self) -> bool {
        let Self::Bool(value) = self else {
            unreachable!("setting value type")
        };
        *value
    }

    fn text(&self) -> &str {
        let Self::Text(value) = self else {
            unreachable!("setting value type")
        };
        value
    }

    fn optional_text(&self) -> Option<String> {
        let Self::OptionalText(value) = self else {
            unreachable!("setting value type")
        };
        value.clone()
    }

    fn toml_value(&self) -> Option<Value> {
        match self {
            Self::Bool(value) => Some(Value::from(*value)),
            Self::Text(value) => Some(Value::from(value.clone())),
            Self::OptionalText(Some(value)) => Some(Value::from(value.clone())),
            Self::OptionalText(None) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SettingsSnapshot {
    values: Vec<SettingValue>,
}

impl SettingsSnapshot {
    fn live(app: &App) -> Self {
        Self {
            values: SettingItem::ALL
                .into_iter()
                .map(|item| app.live_setting_value(item))
                .collect(),
        }
    }

    fn config(config: &Config) -> Self {
        let view_mode = config.parse_view_mode().unwrap_or(ViewMode::UnifiedPane);
        let syntax_theme = if config.ui.syntax.theme.trim().is_empty() {
            config
                .ui
                .theme
                .name
                .clone()
                .unwrap_or_else(|| "ansi".to_string())
        } else {
            config.ui.syntax.theme.clone()
        };
        Self {
            values: SettingItem::ALL
                .into_iter()
                .map(|item| match item {
                    SettingItem::ViewMode => text(view_mode_name(view_mode)),
                    SettingItem::FoldContext => text(fold_context_name(config.ui.fold_context)),
                    SettingItem::LineWrap => bool_value(config.ui.line_wrap),
                    SettingItem::Zen => bool_value(config.ui.zen),
                    SettingItem::Scrollbar => bool_value(config.ui.scrollbar),
                    SettingItem::GutterSigns => bool_value(config.ui.gutter_signs),
                    SettingItem::Watch => bool_value(config.ui.watch),
                    SettingItem::Topbar => bool_value(config.ui.topbar),
                    SettingItem::AutoCenter => bool_value(config.ui.auto_center),
                    SettingItem::Overscroll => bool_value(config.ui.overscroll),
                    SettingItem::ConfirmQuit => bool_value(config.ui.confirm_quit),
                    SettingItem::StrikethroughDeletions => {
                        bool_value(config.ui.strikethrough_deletions)
                    }
                    SettingItem::Stepping => bool_value(config.ui.stepping),
                    SettingItem::Syntax => {
                        bool_value(matches!(config.ui.syntax.mode, SyntaxMode::On))
                    }
                    SettingItem::SyntaxTheme => text(&syntax_theme),
                    SettingItem::DiffBackground => bool_value(config.ui.diff.bg),
                    SettingItem::DiffForeground => text(diff_foreground_name(config.ui.diff.fg)),
                    SettingItem::DiffHighlight => {
                        text(diff_highlight_name(config.ui.diff.highlight))
                    }
                    SettingItem::DiffExtentMarker => {
                        text(diff_extent_marker_name(config.ui.diff.extent_marker))
                    }
                    SettingItem::PreviewChangeBars => {
                        bool_value(config.ui.diff.preview_change_bars)
                    }
                    SettingItem::DiffDefer => bool_value(config.ui.diff.defer),
                    SettingItem::BlameEnabled => bool_value(config.ui.blame.enabled),
                    SettingItem::BlameMode => text(blame_mode_name(config.ui.blame.mode)),
                    SettingItem::BlameHunkHint => bool_value(config.ui.blame.hunk_hint),
                    SettingItem::TimeMode => text(time_mode_name(config.ui.time.mode)),
                    SettingItem::FilePanelVisible => bool_value(config.files.panel_visible),
                    SettingItem::FilePanelPosition => {
                        text(file_panel_position_name(config.files.panel_position))
                    }
                    SettingItem::FileCounts => text(file_count_mode_name(config.files.counts)),
                    SettingItem::FileGitIgnore => {
                        text(git_ignore_mode_name(config.files.scan.git_ignore))
                    }
                    SettingItem::Animation => bool_value(config.playback.animation),
                    SettingItem::Autoplay => bool_value(config.playback.autoplay),
                    SettingItem::AutoStepOnEnter => bool_value(config.playback.auto_step_on_enter),
                    SettingItem::AutoStepBlankFiles => {
                        bool_value(config.playback.auto_step_blank_files)
                    }
                    SettingItem::Theme => SettingValue::OptionalText(config.ui.theme.name.clone()),
                })
                .collect(),
        }
    }

    fn get(&self, item: SettingItem) -> &SettingValue {
        &self.values[item as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsResetAction {
    Confirm,
    Cancel,
}

impl SettingsResetAction {
    fn next(self) -> Self {
        match self {
            Self::Confirm => Self::Cancel,
            Self::Cancel => Self::Confirm,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SettingsResetConfirmation {
    selected: SettingsResetAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SettingsResetHit {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) action: SettingsResetAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsLeaveAction {
    Save,
    Discard,
    Cancel,
}

impl SettingsLeaveAction {
    fn next(self, forward: bool) -> Self {
        let index = match self {
            Self::Save => 0,
            Self::Discard => 1,
            Self::Cancel => 2,
        };
        match (index + if forward { 1 } else { 2 }) % 3 {
            0 => Self::Save,
            1 => Self::Discard,
            _ => Self::Cancel,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SettingsLeaveHit {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) action: SettingsLeaveAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsLeaveTarget {
    SelectTab(usize),
    CloseTab(usize),
    SelectFile(usize),
    OpenFileNew(usize),
    NewTab,
    HelpCurrent,
    HelpTab,
    PrCommentsCurrent(Option<u64>),
    PrCommentsTab(Option<u64>),
    OutdatedCommentsCurrent(Option<u64>),
    OutdatedCommentsTab(Option<u64>),
    History(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SettingsLeaveConfirmation {
    target: SettingsLeaveTarget,
    selected: SettingsLeaveAction,
}

impl App {
    pub(crate) fn active_settings_view(&self) -> bool {
        self.active_topbar_content() == Some(TopbarTabContent::Settings)
    }

    pub(crate) fn open_settings_in_current_tab(&mut self) {
        let already_open = self.active_settings_view();
        self.restore_live_diff_after_outdated_view();
        self.save_active_topbar_tab_state();
        let Some(tab_id) = self.active_topbar_tab else {
            return;
        };
        let settings_view_mode = self
            .topbar_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.view_mode)
            .unwrap_or(self.view_mode);
        let Some(tab) = self.topbar_tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        tab.content = TopbarTabContent::Settings;
        tab.scroll_offset = 0;
        tab.horizontal_scroll = 0;
        tab.preview_rendered = true;
        tab.navigator_state = None;
        self.view_mode = settings_view_mode;
        self.preview_forced_by_content = false;
        self.scroll_offset = 0;
        self.horizontal_scroll = 0;
        self.clear_diff_selection();
        self.settings_selection = self.settings_selection.min(SettingsTarget::COUNT - 1);
        if !already_open {
            self.begin_settings_session();
        }
    }

    pub(crate) fn open_settings_tab(&mut self) {
        if self.active_settings_view() {
            return;
        }
        let history_origin = self.view_history_origin();
        let was_replaying = self.view_history_replaying;
        self.view_history_replaying = true;
        self.restore_live_diff_after_outdated_view();
        if let Some(id) = self
            .topbar_tabs
            .iter()
            .find(|tab| tab.content == TopbarTabContent::Settings)
            .map(|tab| tab.id)
        {
            self.select_topbar_tab(id);
        } else {
            self.save_active_topbar_tab_state();
            let settings_view_mode = self
                .active_topbar_tab
                .and_then(|active| self.topbar_tabs.iter().find(|tab| tab.id == active))
                .map(|tab| tab.view_mode)
                .unwrap_or(self.view_mode);
            let id = self.next_topbar_tab_id;
            self.next_topbar_tab_id = self.next_topbar_tab_id.saturating_add(1);
            self.topbar_tabs.push(TopbarTab {
                id,
                content: TopbarTabContent::Settings,
                view_mode: settings_view_mode,
                step_view_mode: self.step_view_mode,
                stepping: self.stepping,
                scroll_offset: 0,
                horizontal_scroll: 0,
                preview_rendered: true,
                navigator_state: None,
            });
            self.active_topbar_tab = Some(id);
            self.view_mode = settings_view_mode;
            self.preview_forced_by_content = false;
            self.scroll_offset = 0;
            self.horizontal_scroll = 0;
            self.clear_diff_selection();
            self.begin_settings_session();
        }
        self.view_history_replaying = was_replaying;
        if let Some(tab_id) = self.active_topbar_tab {
            self.record_view_landing(history_origin, ViewHistoryRecipe::Settings { tab_id });
        }
    }

    pub(super) fn begin_settings_session(&mut self) {
        let live = SettingsSnapshot::live(self);
        let path = self.settings_config_path();
        let saved = match path
            .as_deref()
            .ok_or_else(|| "Could not determine the config path".to_string())
            .and_then(Config::load_from_path)
        {
            Ok(config) => SettingsSnapshot::config(&config),
            Err(error) => {
                self.notify(ToastEvent::SelectionActionFailed(format!(
                    "Could not read settings: {error}"
                )));
                live.clone()
            }
        };
        self.settings_open_state = Some(live);
        self.settings_saved_state = Some(saved);
        self.settings_leave_confirmation = None;
        self.settings_reset_confirmation = None;
        self.settings_reset_hits.clear();
        self.settings_reset_hover = None;
        self.settings_leave_hits.clear();
        self.settings_leave_hover = None;
    }

    fn settings_config_path(&self) -> Option<std::path::PathBuf> {
        self.settings_config_path_override
            .clone()
            .or_else(Config::config_path_for_write)
    }

    pub(crate) fn settings_dirty(&self) -> bool {
        self.settings_saved_state
            .as_ref()
            .is_some_and(|saved| SettingsSnapshot::live(self) != *saved)
    }

    pub(crate) fn setting_dirty(&self, item: SettingItem) -> bool {
        self.settings_saved_state
            .as_ref()
            .is_some_and(|saved| self.live_setting_value(item) != *saved.get(item))
    }

    #[cfg(test)]
    pub(crate) fn settings_dirty_keys(&self) -> Vec<String> {
        let live = SettingsSnapshot::live(self);
        self.settings_saved_state
            .as_ref()
            .map(|saved| {
                SettingItem::ALL
                    .into_iter()
                    .filter(|item| live.get(*item) != saved.get(*item))
                    .map(|item| setting_config_path(item).join("."))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn settings_rows(&self) -> Vec<SettingsRow> {
        let mut rows = SETTINGS_ENTRIES
            .into_iter()
            .map(|entry| match entry {
                SettingsEntry::Spacer => SettingsRow::Spacer,
                SettingsEntry::Section(label) => SettingsRow::Section(label),
                SettingsEntry::Item(item) => {
                    let (label, value, hint) = self.setting_display(item);
                    SettingsRow::Item {
                        item,
                        label,
                        value,
                        hint,
                        dirty: self.setting_dirty(item),
                    }
                }
            })
            .collect::<Vec<_>>();
        rows.push(SettingsRow::Spacer);
        rows.push(SettingsRow::Spacer);
        rows.push(SettingsRow::Actions {
            dirty: self.settings_dirty(),
        });
        rows
    }

    fn setting_display(&self, item: SettingItem) -> (&'static str, String, &'static str) {
        match item {
            SettingItem::ViewMode => (
                "View mode",
                view_mode_name(self.view_mode).into(),
                "cycle diff layout",
            ),
            SettingItem::FoldContext => (
                "Fold context",
                fold_context_name(self.fold_context).into(),
                "collapse unchanged lines",
            ),
            SettingItem::LineWrap => ("Line wrap", on_off(self.line_wrap), "wrap long diff lines"),
            SettingItem::Zen => ("Zen", on_off(self.zen_mode), "hide surrounding UI"),
            SettingItem::Scrollbar => (
                "Scrollbar",
                on_off(self.scrollbar_visible),
                "show diff and sidebar scrollbars",
            ),
            SettingItem::GutterSigns => (
                "Gutter signs",
                on_off(self.gutter_signs),
                "show added and deleted signs",
            ),
            SettingItem::Watch => ("Watch", on_off(self.watch), "refresh changed files"),
            SettingItem::Topbar => ("Top bar", on_off(self.topbar), "show file tabs"),
            SettingItem::AutoCenter => (
                "Auto-center",
                on_off(self.auto_center),
                "center the active change",
            ),
            SettingItem::Overscroll => (
                "Overscroll",
                on_off(self.overscroll),
                "allow space beyond the diff",
            ),
            SettingItem::ConfirmQuit => (
                "Confirm quit",
                on_off(self.confirm_quit),
                "confirm before closing",
            ),
            SettingItem::StrikethroughDeletions => (
                "Strikethrough",
                on_off(self.strikethrough_deletions),
                "strike deleted text",
            ),
            SettingItem::Stepping => (
                "Step mode",
                on_off(self.stepping),
                "start and work change by change",
            ),
            SettingItem::Syntax => (
                "Syntax",
                on_off(matches!(self.syntax_mode, SyntaxMode::On)),
                "highlight source code",
            ),
            SettingItem::SyntaxTheme => (
                "Syntax theme",
                if self.syntax_theme.is_empty() {
                    "automatic".into()
                } else {
                    self.syntax_theme.clone()
                },
                "open syntax theme picker",
            ),
            SettingItem::DiffBackground => (
                "Diff background",
                on_off(self.diff_bg),
                "fill changed lines",
            ),
            SettingItem::DiffForeground => (
                "Diff foreground",
                diff_foreground_name(self.diff_fg).into(),
                "cycle text colour source",
            ),
            SettingItem::DiffHighlight => (
                "Diff highlight",
                diff_highlight_name(self.diff_highlight).into(),
                "cycle inline highlights",
            ),
            SettingItem::DiffExtentMarker => (
                "Extent marker",
                diff_extent_marker_name(self.diff_extent_marker).into(),
                "cycle marker colours",
            ),
            SettingItem::PreviewChangeBars => (
                "Preview bars",
                on_off(self.preview_change_bars),
                "mark changed preview lines",
            ),
            SettingItem::DiffDefer => (
                "Deferred diffs",
                on_off(self.diff_defer),
                "defer large diff work",
            ),
            SettingItem::BlameEnabled => ("Blame", on_off(self.blame_enabled), "allow blame hints"),
            SettingItem::BlameMode => (
                "Blame mode",
                blame_mode_name(self.blame_mode).into(),
                "cycle one-shot or toggle",
            ),
            SettingItem::BlameHunkHint => (
                "Blame hunk hint",
                on_off(self.blame_hunk_hint_enabled),
                "show blame after hunk jumps",
            ),
            SettingItem::TimeMode => (
                "Time mode",
                time_mode_name(self.time_format.mode()).into(),
                "cycle blame time format",
            ),
            SettingItem::FilePanelVisible => (
                "File panel",
                on_off(self.file_panel_visible),
                "show files by default",
            ),
            SettingItem::FilePanelPosition => (
                "File panel side",
                file_panel_position_name(self.file_panel_position).into(),
                "show the file panel on the left or right",
            ),
            SettingItem::FileCounts => (
                "File counts",
                file_count_mode_name(self.file_count_mode).into(),
                "cycle change count visibility",
            ),
            SettingItem::FileGitIgnore => (
                "Git ignore",
                git_ignore_mode_name(self.file_git_ignore_mode).into(),
                "(restart) directory scans",
            ),
            SettingItem::Animation => (
                "Animation",
                on_off(self.animation_enabled),
                "animate step transitions",
            ),
            SettingItem::Autoplay => (
                "Autoplay",
                on_off(self.autoplay),
                "advance changes automatically",
            ),
            SettingItem::AutoStepOnEnter => (
                "Auto-step enter",
                on_off(self.auto_step_on_enter),
                "step when entering a file",
            ),
            SettingItem::AutoStepBlankFiles => (
                "Auto-step blank",
                on_off(self.auto_step_blank_files),
                "reveal initially blank files",
            ),
            SettingItem::Theme => (
                "Colour theme",
                self.ui_theme_name
                    .clone()
                    .unwrap_or_else(|| "default".into()),
                "open colour theme picker",
            ),
        }
    }

    pub(crate) fn settings_selected_target(&self) -> SettingsTarget {
        SettingsTarget::from_index(self.settings_selection)
    }

    pub(crate) fn move_settings_selection(&mut self, forward: bool) {
        self.settings_selection = if forward {
            (self.settings_selection + 1) % SettingsTarget::COUNT
        } else {
            self.settings_selection
                .checked_sub(1)
                .unwrap_or(SettingsTarget::COUNT - 1)
        };
    }

    pub(crate) fn adjust_selected_setting(&mut self, forward: bool) {
        match self.settings_selected_target() {
            SettingsTarget::Item(item) if !item.uses_picker() => {
                let value = self.adjacent_setting_value(item, forward);
                self.apply_setting_value(item, &value);
            }
            SettingsTarget::Item(_) => {}
            target => {
                let first = SettingsTarget::Save.index();
                let index = target.index().saturating_sub(first);
                self.settings_selection = first + (index + if forward { 1 } else { 2 }) % 3;
            }
        }
    }

    pub(crate) fn set_settings_hits(&mut self, hits: Vec<SettingHit>) {
        self.settings_hits = hits;
    }

    pub(crate) fn update_settings_hover(&mut self, column: u16, row: u16) -> bool {
        let hover = self.active_settings_view().then(|| {
            self.settings_hits.iter().find_map(|hit| {
                (row == hit.y && column >= hit.x && column < hit.x.saturating_add(hit.width))
                    .then_some(hit.target)
            })
        });
        let hover = hover.flatten();
        let mut changed = self.settings_hover != hover;
        if let Some(SettingsTarget::Item(item)) = hover {
            let selection = SettingsTarget::Item(item).index();
            if self.settings_selection != selection {
                self.settings_selection = selection;
                changed = true;
            }
        }
        self.settings_hover = hover;
        changed
    }

    pub(crate) fn handle_settings_click(&mut self, column: u16, row: u16) -> bool {
        if !self.active_settings_view() {
            return false;
        }
        let Some(hit) = self
            .settings_hits
            .iter()
            .find(|hit| row == hit.y && column >= hit.x && column < hit.x.saturating_add(hit.width))
            .copied()
        else {
            return false;
        };
        self.settings_selection = hit.target.index();
        self.activate_settings_target(hit.target);
        true
    }

    pub(crate) fn activate_selected_setting(&mut self) {
        self.activate_settings_target(self.settings_selected_target());
    }

    fn activate_settings_target(&mut self, target: SettingsTarget) {
        match target {
            SettingsTarget::Item(item) => self.activate_settings_item(item),
            SettingsTarget::Save => {
                self.save_settings();
            }
            SettingsTarget::Revert => self.revert_settings(),
            SettingsTarget::ResetDefaults => self.request_settings_reset(),
        }
    }

    fn activate_settings_item(&mut self, item: SettingItem) {
        match item {
            SettingItem::SyntaxTheme => {
                self.start_syntax_theme_picker();
                return;
            }
            SettingItem::Theme => {
                self.start_theme_picker();
                return;
            }
            _ => {}
        }
        let next = self.adjacent_setting_value(item, true);
        self.apply_setting_value(item, &next);
    }

    fn live_setting_value(&self, item: SettingItem) -> SettingValue {
        match item {
            SettingItem::ViewMode => text(view_mode_name(self.view_mode)),
            SettingItem::FoldContext => text(fold_context_name(self.fold_context)),
            SettingItem::LineWrap => bool_value(self.line_wrap),
            SettingItem::Zen => bool_value(self.zen_mode),
            SettingItem::Scrollbar => bool_value(self.scrollbar_visible),
            SettingItem::GutterSigns => bool_value(self.gutter_signs),
            SettingItem::Watch => bool_value(self.watch),
            SettingItem::Topbar => bool_value(self.topbar),
            SettingItem::AutoCenter => bool_value(self.auto_center),
            SettingItem::Overscroll => bool_value(self.overscroll),
            SettingItem::ConfirmQuit => bool_value(self.confirm_quit),
            SettingItem::StrikethroughDeletions => bool_value(self.strikethrough_deletions),
            SettingItem::Stepping => bool_value(self.stepping),
            SettingItem::Syntax => bool_value(matches!(self.syntax_mode, SyntaxMode::On)),
            SettingItem::SyntaxTheme => text(&self.syntax_theme),
            SettingItem::DiffBackground => bool_value(self.diff_bg),
            SettingItem::DiffForeground => text(diff_foreground_name(self.diff_fg)),
            SettingItem::DiffHighlight => text(diff_highlight_name(self.diff_highlight)),
            SettingItem::DiffExtentMarker => text(diff_extent_marker_name(self.diff_extent_marker)),
            SettingItem::PreviewChangeBars => bool_value(self.preview_change_bars),
            SettingItem::DiffDefer => bool_value(self.diff_defer),
            SettingItem::BlameEnabled => bool_value(self.blame_enabled),
            SettingItem::BlameMode => text(blame_mode_name(self.blame_mode)),
            SettingItem::BlameHunkHint => bool_value(self.blame_hunk_hint_enabled),
            SettingItem::TimeMode => text(time_mode_name(self.time_format.mode())),
            SettingItem::FilePanelVisible => bool_value(self.file_panel_visible),
            SettingItem::FilePanelPosition => {
                text(file_panel_position_name(self.file_panel_position))
            }
            SettingItem::FileCounts => text(file_count_mode_name(self.file_count_mode)),
            SettingItem::FileGitIgnore => text(git_ignore_mode_name(self.file_git_ignore_mode)),
            SettingItem::Animation => bool_value(self.animation_enabled),
            SettingItem::Autoplay => bool_value(self.autoplay),
            SettingItem::AutoStepOnEnter => bool_value(self.auto_step_on_enter),
            SettingItem::AutoStepBlankFiles => bool_value(self.auto_step_blank_files),
            SettingItem::Theme => SettingValue::OptionalText(self.ui_theme_name.clone()),
        }
    }

    fn adjacent_setting_value(&self, item: SettingItem, forward: bool) -> SettingValue {
        match item {
            SettingItem::ViewMode => text(view_mode_name(cycle_value(
                self.view_mode,
                &[ViewMode::UnifiedPane, ViewMode::Split, ViewMode::Evolution],
                forward,
            ))),
            SettingItem::FoldContext => text(fold_context_name(cycle_value(
                self.fold_context,
                &[FoldContextMode::Expandable, FoldContextMode::Off],
                forward,
            ))),
            SettingItem::LineWrap
            | SettingItem::Zen
            | SettingItem::Scrollbar
            | SettingItem::GutterSigns
            | SettingItem::Watch
            | SettingItem::Topbar
            | SettingItem::AutoCenter
            | SettingItem::Overscroll
            | SettingItem::ConfirmQuit
            | SettingItem::StrikethroughDeletions
            | SettingItem::Stepping
            | SettingItem::Syntax
            | SettingItem::DiffBackground
            | SettingItem::PreviewChangeBars
            | SettingItem::DiffDefer
            | SettingItem::BlameEnabled
            | SettingItem::BlameHunkHint
            | SettingItem::FilePanelVisible
            | SettingItem::Animation
            | SettingItem::Autoplay
            | SettingItem::AutoStepOnEnter
            | SettingItem::AutoStepBlankFiles => bool_value(!self.live_setting_value(item).bool()),
            SettingItem::DiffForeground => text(diff_foreground_name(cycle_value(
                self.diff_fg,
                &[DiffForegroundMode::Theme, DiffForegroundMode::Syntax],
                forward,
            ))),
            SettingItem::DiffHighlight => text(diff_highlight_name(cycle_value(
                self.diff_highlight,
                &[
                    DiffHighlightMode::Text,
                    DiffHighlightMode::Word,
                    DiffHighlightMode::None,
                ],
                forward,
            ))),
            SettingItem::DiffExtentMarker => text(diff_extent_marker_name(cycle_value(
                self.diff_extent_marker,
                &[DiffExtentMarkerMode::Neutral, DiffExtentMarkerMode::Diff],
                forward,
            ))),
            SettingItem::BlameMode => text(blame_mode_name(cycle_value(
                self.blame_mode,
                &[BlameMode::OneShot, BlameMode::Toggle],
                forward,
            ))),
            SettingItem::TimeMode => text(time_mode_name(cycle_value(
                self.time_format.mode(),
                &[TimeMode::Relative, TimeMode::Absolute, TimeMode::Custom],
                forward,
            ))),
            SettingItem::FilePanelPosition => text(file_panel_position_name(cycle_value(
                self.file_panel_position,
                &[FilePanelPosition::Left, FilePanelPosition::Right],
                forward,
            ))),
            SettingItem::FileCounts => text(file_count_mode_name(cycle_value(
                self.file_count_mode,
                &[
                    FileCountMode::Active,
                    FileCountMode::Focused,
                    FileCountMode::All,
                    FileCountMode::Off,
                ],
                forward,
            ))),
            SettingItem::FileGitIgnore => text(git_ignore_mode_name(cycle_value(
                self.file_git_ignore_mode,
                &[GitIgnoreMode::Auto, GitIgnoreMode::On, GitIgnoreMode::Off],
                forward,
            ))),
            SettingItem::SyntaxTheme | SettingItem::Theme => self.live_setting_value(item),
        }
    }

    fn apply_setting_value(&mut self, item: SettingItem, value: &SettingValue) {
        match item {
            SettingItem::ViewMode => {
                if let Some(mode) = parse_view_mode(value.text()) {
                    self.set_view_mode(mode);
                    self.sync_settings_tab_view_state();
                }
            }
            SettingItem::FoldContext => self.set_fold_context_mode(if value.text() == "off" {
                FoldContextMode::Off
            } else {
                FoldContextMode::Expandable
            }),
            SettingItem::LineWrap => {
                set_with_toggle(self.line_wrap, value.bool(), || self.toggle_line_wrap())
            }
            SettingItem::Zen => set_with_toggle(self.zen_mode, value.bool(), || self.toggle_zen()),
            SettingItem::Scrollbar => self.scrollbar_visible = value.bool(),
            SettingItem::GutterSigns => self.gutter_signs = value.bool(),
            SettingItem::Watch => self.watch = value.bool(),
            SettingItem::Topbar => self.topbar = value.bool(),
            SettingItem::AutoCenter => self.auto_center = value.bool(),
            SettingItem::Overscroll => self.overscroll = value.bool(),
            SettingItem::ConfirmQuit => self.confirm_quit = value.bool(),
            SettingItem::StrikethroughDeletions => {
                if self.strikethrough_deletions != value.bool() {
                    self.toggle_strikethrough_deletions();
                }
            }
            SettingItem::Stepping => {
                if self.stepping != value.bool() {
                    self.toggle_stepping();
                    self.sync_settings_tab_view_state();
                }
            }
            SettingItem::Syntax => {
                let enabled = value.bool();
                if matches!(self.syntax_mode, SyntaxMode::On) != enabled {
                    self.toggle_syntax();
                }
            }
            SettingItem::SyntaxTheme => self.set_syntax_theme(value.text().to_string()),
            SettingItem::DiffBackground => {
                self.diff_bg = value.bool();
                self.unified_render_cache = None;
            }
            SettingItem::DiffForeground => {
                self.diff_fg = if value.text() == "syntax" {
                    DiffForegroundMode::Syntax
                } else {
                    DiffForegroundMode::Theme
                };
                self.unified_render_cache = None;
            }
            SettingItem::DiffHighlight => {
                self.diff_highlight = match value.text() {
                    "word" => DiffHighlightMode::Word,
                    "none" => DiffHighlightMode::None,
                    _ => DiffHighlightMode::Text,
                };
                self.unified_render_cache = None;
            }
            SettingItem::DiffExtentMarker => {
                self.diff_extent_marker = if value.text() == "diff" {
                    DiffExtentMarkerMode::Diff
                } else {
                    DiffExtentMarkerMode::Neutral
                };
                self.unified_render_cache = None;
            }
            SettingItem::PreviewChangeBars => self.preview_change_bars = value.bool(),
            SettingItem::DiffDefer => {
                self.diff_defer = value.bool();
                oyo_core::MultiFileDiff::set_diff_defer(self.diff_defer);
            }
            SettingItem::BlameEnabled => self.set_blame_enabled(value.bool()),
            SettingItem::BlameMode => self.set_blame_mode(if value.text() == "toggle" {
                BlameMode::Toggle
            } else {
                BlameMode::OneShot
            }),
            SettingItem::BlameHunkHint => self.set_blame_hunk_hint_enabled(value.bool()),
            SettingItem::TimeMode => {
                self.time_format.set_mode(match value.text() {
                    "absolute" => TimeMode::Absolute,
                    "custom" => TimeMode::Custom,
                    _ => TimeMode::Relative,
                });
                self.clear_blame_step_hint();
                self.clear_blame_hunk_hint();
            }
            SettingItem::FilePanelVisible => self.set_file_panel_visible(value.bool()),
            SettingItem::FilePanelPosition => {
                self.file_panel_position = if value.text() == "right" {
                    FilePanelPosition::Right
                } else {
                    FilePanelPosition::Left
                };
            }
            SettingItem::FileCounts => {
                self.file_count_mode = match value.text() {
                    "focused" => FileCountMode::Focused,
                    "all" => FileCountMode::All,
                    "off" => FileCountMode::Off,
                    _ => FileCountMode::Active,
                }
            }
            SettingItem::FileGitIgnore => {
                self.file_git_ignore_mode = match value.text() {
                    "on" => GitIgnoreMode::On,
                    "off" => GitIgnoreMode::Off,
                    _ => GitIgnoreMode::Auto,
                }
            }
            SettingItem::Animation => {
                if self.animation_enabled != value.bool() {
                    self.toggle_animation();
                }
            }
            SettingItem::Autoplay => {
                if self.autoplay != value.bool() {
                    if value.bool() {
                        self.toggle_autoplay();
                    } else {
                        self.stop_autoplay();
                    }
                }
            }
            SettingItem::AutoStepOnEnter => self.auto_step_on_enter = value.bool(),
            SettingItem::AutoStepBlankFiles => self.auto_step_blank_files = value.bool(),
            SettingItem::Theme => self.set_ui_theme_name(value.optional_text()),
        }
    }

    fn apply_settings_snapshot(&mut self, snapshot: &SettingsSnapshot) {
        for item in SettingItem::ALL {
            if !matches!(
                item,
                SettingItem::ViewMode | SettingItem::Stepping | SettingItem::BlameEnabled
            ) {
                self.apply_setting_value(item, snapshot.get(item));
            }
        }
        for item in [
            SettingItem::BlameEnabled,
            SettingItem::Stepping,
            SettingItem::ViewMode,
        ] {
            self.apply_setting_value(item, snapshot.get(item));
        }
    }

    fn sync_settings_tab_view_state(&mut self) {
        let Some(tab_id) = self.active_topbar_tab else {
            return;
        };
        if let Some(tab) = self.topbar_tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.view_mode = self.view_mode;
            tab.step_view_mode = self.step_view_mode;
            tab.stepping = self.stepping;
        }
    }

    pub(crate) fn save_settings(&mut self) -> bool {
        let live = SettingsSnapshot::live(self);
        let Some(saved) = self.settings_saved_state.as_ref() else {
            return true;
        };
        let changes = SettingItem::ALL
            .into_iter()
            .filter(|item| live.get(*item) != saved.get(*item))
            .map(|item| {
                (
                    setting_config_path(item).to_vec(),
                    live.get(item).toml_value(),
                )
            })
            .collect::<Vec<_>>();
        if changes.is_empty() {
            return true;
        }
        let Some(path) = self.settings_config_path() else {
            self.notify(ToastEvent::SelectionActionFailed(
                "Could not save settings: Could not determine the config path".to_string(),
            ));
            return false;
        };
        if let Err(error) = Config::write_config_values(&path, &changes) {
            self.notify(ToastEvent::SelectionActionFailed(format!(
                "Could not save settings: {error}"
            )));
            return false;
        }
        // Keep runtime-normalized values such as the implicit syntax theme.
        self.settings_saved_state = Some(live);
        self.notify(ToastEvent::SettingsSaved);
        true
    }

    pub(crate) fn revert_settings(&mut self) {
        let Some(snapshot) = self.settings_open_state.clone() else {
            return;
        };
        if SettingsSnapshot::live(self) == snapshot {
            return;
        }
        self.apply_settings_snapshot(&snapshot);
        self.notify(ToastEvent::SettingsReverted);
    }

    pub(crate) fn reset_settings_to_defaults(&mut self) {
        self.apply_settings_snapshot(&SettingsSnapshot::config(&Config::default()));
    }

    fn request_settings_reset(&mut self) {
        self.settings_reset_confirmation = Some(SettingsResetConfirmation {
            selected: SettingsResetAction::Confirm,
        });
        self.settings_reset_hits.clear();
        self.settings_reset_hover = None;
    }

    pub(crate) fn settings_reset_confirmation_active(&self) -> bool {
        self.settings_reset_confirmation.is_some()
    }

    pub(crate) fn settings_reset_selected_action(&self) -> SettingsResetAction {
        self.settings_reset_confirmation
            .map(|confirmation| confirmation.selected)
            .unwrap_or(SettingsResetAction::Confirm)
    }

    pub(crate) fn set_settings_reset_hits(&mut self, hits: Vec<SettingsResetHit>) {
        self.settings_reset_hits = hits;
    }

    pub(crate) fn cancel_settings_reset_confirmation(&mut self) {
        self.settings_reset_confirmation = None;
        self.settings_reset_hits.clear();
        self.settings_reset_hover = None;
    }

    fn resolve_settings_reset(&mut self, action: SettingsResetAction) {
        if action == SettingsResetAction::Confirm {
            self.reset_settings_to_defaults();
        }
        self.cancel_settings_reset_confirmation();
    }

    pub(crate) fn handle_settings_reset_key(&mut self, key: KeyEvent) -> bool {
        let Some(confirmation) = self.settings_reset_confirmation.as_mut() else {
            return false;
        };
        match key.code {
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Tab
            | KeyCode::Char('h' | 'j' | 'k' | 'l') => {
                confirmation.selected = confirmation.selected.next();
            }
            KeyCode::Char('r' | 'R' | 'y' | 'Y') => {
                self.resolve_settings_reset(SettingsResetAction::Confirm)
            }
            KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                self.resolve_settings_reset(SettingsResetAction::Cancel)
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let action = confirmation.selected;
                self.resolve_settings_reset(action);
            }
            _ => {}
        }
        true
    }

    pub(crate) fn update_settings_reset_hover(&mut self, column: u16, row: u16) -> bool {
        let hover = self.settings_reset_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.action)
        });
        if self.settings_reset_hover == hover {
            return false;
        }
        self.settings_reset_hover = hover;
        true
    }

    pub(crate) fn handle_settings_reset_click(&mut self, column: u16, row: u16) -> bool {
        let Some(action) = self.settings_reset_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.action)
        }) else {
            return false;
        };
        self.resolve_settings_reset(action);
        true
    }

    pub(super) fn request_settings_leave(&mut self, target: SettingsLeaveTarget) -> bool {
        if self.active_settings_view() && self.settings_reset_confirmation_active() {
            self.cancel_settings_reset_confirmation();
        }
        if self.settings_leave_replaying || !self.active_settings_view() || !self.settings_dirty() {
            return false;
        }
        if self.settings_leave_confirmation.is_none() {
            self.settings_leave_confirmation = Some(SettingsLeaveConfirmation {
                target,
                selected: SettingsLeaveAction::Save,
            });
            self.settings_leave_hits.clear();
            self.settings_leave_hover = None;
        }
        true
    }

    pub(crate) fn settings_leave_confirmation_active(&self) -> bool {
        self.settings_leave_confirmation.is_some()
    }

    pub(crate) fn settings_leave_selected_action(&self) -> SettingsLeaveAction {
        self.settings_leave_confirmation
            .map(|confirmation| confirmation.selected)
            .unwrap_or(SettingsLeaveAction::Save)
    }

    pub(crate) fn set_settings_leave_hits(&mut self, hits: Vec<SettingsLeaveHit>) {
        self.settings_leave_hits = hits;
    }

    pub(crate) fn cancel_settings_leave_confirmation(&mut self) {
        self.settings_leave_confirmation = None;
        self.settings_leave_hits.clear();
        self.settings_leave_hover = None;
    }

    pub(crate) fn handle_settings_leave_key(&mut self, key: KeyEvent) -> bool {
        let Some(confirmation) = self.settings_leave_confirmation.as_mut() else {
            return false;
        };
        match key.code {
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h' | 'k') => {
                confirmation.selected = confirmation.selected.next(false);
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Tab | KeyCode::Char('j' | 'l') => {
                confirmation.selected = confirmation.selected.next(true);
            }
            KeyCode::Char('s' | 'S') => self.resolve_settings_leave(SettingsLeaveAction::Save),
            KeyCode::Char('d' | 'D') => self.resolve_settings_leave(SettingsLeaveAction::Discard),
            KeyCode::Esc | KeyCode::Char('c' | 'C') => self.cancel_settings_leave_confirmation(),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let action = confirmation.selected;
                self.resolve_settings_leave(action);
            }
            _ => {}
        }
        true
    }

    pub(crate) fn update_settings_leave_hover(&mut self, column: u16, row: u16) -> bool {
        let hover = self.settings_leave_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.action)
        });
        if self.settings_leave_hover == hover {
            return false;
        }
        self.settings_leave_hover = hover;
        true
    }

    pub(crate) fn handle_settings_leave_click(&mut self, column: u16, row: u16) -> bool {
        let Some(action) = self.settings_leave_hits.iter().find_map(|hit| {
            (column >= hit.x
                && column < hit.x.saturating_add(hit.width)
                && row >= hit.y
                && row < hit.y.saturating_add(hit.height))
            .then_some(hit.action)
        }) else {
            return false;
        };
        self.resolve_settings_leave(action);
        true
    }

    fn resolve_settings_leave(&mut self, action: SettingsLeaveAction) {
        let Some(confirmation) = self.settings_leave_confirmation else {
            return;
        };
        match action {
            SettingsLeaveAction::Save if !self.save_settings() => return,
            SettingsLeaveAction::Save => {}
            SettingsLeaveAction::Discard => self.revert_settings(),
            SettingsLeaveAction::Cancel => {
                self.cancel_settings_leave_confirmation();
                return;
            }
        }
        let target = confirmation.target;
        self.cancel_settings_leave_confirmation();
        self.settings_open_state = None;
        self.settings_saved_state = None;
        self.settings_leave_replaying = true;
        match target {
            SettingsLeaveTarget::SelectTab(tab_id) => self.select_topbar_tab(tab_id),
            SettingsLeaveTarget::CloseTab(tab_id) => self.close_topbar_tab(tab_id),
            SettingsLeaveTarget::SelectFile(index) => self.select_file(index),
            SettingsLeaveTarget::OpenFileNew(index) => self.open_file_in_new_topbar_tab(index),
            SettingsLeaveTarget::NewTab => self.new_topbar_tab(),
            SettingsLeaveTarget::HelpCurrent => self.open_help_in_current_tab(),
            SettingsLeaveTarget::HelpTab => self.open_help_tab(),
            SettingsLeaveTarget::PrCommentsCurrent(focus) => {
                self.open_pr_comments_in_current_tab(focus)
            }
            SettingsLeaveTarget::PrCommentsTab(focus) => self.open_pr_comments_tab(focus),
            SettingsLeaveTarget::OutdatedCommentsCurrent(focus) => {
                self.open_outdated_comments_in_current_tab(focus)
            }
            SettingsLeaveTarget::OutdatedCommentsTab(focus) => {
                self.open_outdated_comments_tab(focus)
            }
            SettingsLeaveTarget::History(backward) => {
                self.navigate_view_history(backward);
            }
        }
        self.settings_leave_replaying = false;
    }
}

fn cycle_value<T: Copy + PartialEq>(current: T, values: &[T], forward: bool) -> T {
    let index = values.iter().position(|value| *value == current);
    match index {
        Some(index) => values[(index + if forward { 1 } else { values.len() - 1 }) % values.len()],
        None if forward => values[0],
        None => values[values.len() - 1],
    }
}

fn bool_value(value: bool) -> SettingValue {
    SettingValue::Bool(value)
}

fn text(value: &str) -> SettingValue {
    SettingValue::Text(value.to_string())
}

fn set_with_toggle(current: bool, target: bool, toggle: impl FnOnce()) {
    if current != target {
        toggle();
    }
}

fn on_off(value: bool) -> String {
    if value { "on" } else { "off" }.to_string()
}

fn view_mode_name(value: ViewMode) -> &'static str {
    match value {
        ViewMode::UnifiedPane => "unified",
        ViewMode::Split => "split",
        ViewMode::Evolution => "evolution",
        ViewMode::Blame => "blame",
        ViewMode::Preview => "preview",
    }
}

fn parse_view_mode(value: &str) -> Option<ViewMode> {
    match value {
        "unified" => Some(ViewMode::UnifiedPane),
        "split" => Some(ViewMode::Split),
        "evolution" => Some(ViewMode::Evolution),
        "blame" => Some(ViewMode::Blame),
        "preview" => Some(ViewMode::Preview),
        _ => None,
    }
}

fn fold_context_name(value: FoldContextMode) -> &'static str {
    match value {
        FoldContextMode::Expandable => "expandable",
        FoldContextMode::Off => "off",
    }
}

fn diff_foreground_name(value: DiffForegroundMode) -> &'static str {
    match value {
        DiffForegroundMode::Theme => "theme",
        DiffForegroundMode::Syntax => "syntax",
    }
}

fn diff_highlight_name(value: DiffHighlightMode) -> &'static str {
    match value {
        DiffHighlightMode::Text => "text",
        DiffHighlightMode::Word => "word",
        DiffHighlightMode::None => "none",
    }
}

fn diff_extent_marker_name(value: DiffExtentMarkerMode) -> &'static str {
    match value {
        DiffExtentMarkerMode::Neutral => "neutral",
        DiffExtentMarkerMode::Diff => "diff",
    }
}

fn blame_mode_name(value: BlameMode) -> &'static str {
    match value {
        BlameMode::OneShot => "one_shot",
        BlameMode::Toggle => "toggle",
    }
}

fn time_mode_name(value: TimeMode) -> &'static str {
    match value {
        TimeMode::Relative => "relative",
        TimeMode::Absolute => "absolute",
        TimeMode::Custom => "custom",
    }
}

fn file_panel_position_name(value: FilePanelPosition) -> &'static str {
    match value {
        FilePanelPosition::Left => "left",
        FilePanelPosition::Right => "right",
    }
}

fn file_count_mode_name(value: FileCountMode) -> &'static str {
    match value {
        FileCountMode::Active => "active",
        FileCountMode::Focused => "focused",
        FileCountMode::All => "all",
        FileCountMode::Off => "off",
    }
}

fn git_ignore_mode_name(value: GitIgnoreMode) -> &'static str {
    match value {
        GitIgnoreMode::Auto => "auto",
        GitIgnoreMode::On => "on",
        GitIgnoreMode::Off => "off",
    }
}

fn setting_config_path(item: SettingItem) -> &'static [&'static str] {
    match item {
        SettingItem::ViewMode => &["ui", "view_mode"],
        SettingItem::FoldContext => &["ui", "fold_context"],
        SettingItem::LineWrap => &["ui", "line_wrap"],
        SettingItem::Zen => &["ui", "zen"],
        SettingItem::Scrollbar => &["ui", "scrollbar"],
        SettingItem::GutterSigns => &["ui", "gutter_signs"],
        SettingItem::Watch => &["ui", "watch"],
        SettingItem::Topbar => &["ui", "topbar"],
        SettingItem::AutoCenter => &["ui", "auto_center"],
        SettingItem::Overscroll => &["ui", "overscroll"],
        SettingItem::ConfirmQuit => &["ui", "confirm_quit"],
        SettingItem::StrikethroughDeletions => &["ui", "strikethrough_deletions"],
        SettingItem::Stepping => &["ui", "stepping"],
        SettingItem::Syntax => &["ui", "syntax", "mode"],
        SettingItem::SyntaxTheme => &["ui", "syntax", "theme"],
        SettingItem::DiffBackground => &["ui", "diff", "bg"],
        SettingItem::DiffForeground => &["ui", "diff", "fg"],
        SettingItem::DiffHighlight => &["ui", "diff", "highlight"],
        SettingItem::DiffExtentMarker => &["ui", "diff", "extent_marker"],
        SettingItem::PreviewChangeBars => &["ui", "diff", "preview_change_bars"],
        SettingItem::DiffDefer => &["ui", "diff", "defer"],
        SettingItem::BlameEnabled => &["ui", "blame", "enabled"],
        SettingItem::BlameMode => &["ui", "blame", "mode"],
        SettingItem::BlameHunkHint => &["ui", "blame", "hunk_hint"],
        SettingItem::TimeMode => &["ui", "time", "mode"],
        SettingItem::FilePanelVisible => &["files", "panel_visible"],
        SettingItem::FilePanelPosition => &["files", "panel_position"],
        SettingItem::FileCounts => &["files", "counts"],
        SettingItem::FileGitIgnore => &["files", "scan", "git_ignore"],
        SettingItem::Animation => &["playback", "animation"],
        SettingItem::Autoplay => &["playback", "autoplay"],
        SettingItem::AutoStepOnEnter => &["playback", "auto_step_on_enter"],
        SettingItem::AutoStepBlankFiles => &["playback", "auto_step_blank_files"],
        SettingItem::Theme => &["ui", "theme", "name"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oyo_core::MultiFileDiff;
    use std::path::PathBuf;

    fn test_app(path: PathBuf) -> App {
        let diff = MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        if let Ok(config) = Config::load_from_path(&path) {
            app.apply_settings_snapshot(&SettingsSnapshot::config(&config));
        }
        app.settings_config_path_override = Some(path);
        app
    }

    #[test]
    fn settings_tab_reuses_one_tab_and_records_history_when_clean() {
        let dir = std::env::temp_dir().join(format!("oyo-settings-history-{}", std::process::id()));
        let mut app = test_app(dir.join("config.toml"));
        app.ensure_topbar_tabs();
        app.open_settings_tab();
        let settings_id = app.active_topbar_tab.unwrap();
        app.open_settings_tab();
        assert_eq!(app.active_topbar_tab, Some(settings_id));
        assert_eq!(
            app.topbar_tabs
                .iter()
                .filter(|tab| tab.content == TopbarTabContent::Settings)
                .count(),
            1
        );
        assert!(app.navigate_view_back());
        assert!(matches!(
            app.active_topbar_content(),
            Some(TopbarTabContent::File(_))
        ));
        assert!(app.navigate_view_forward());
        assert_eq!(
            app.active_topbar_content(),
            Some(TopbarTabContent::Settings)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn content_forced_preview_does_not_become_a_settings_change() {
        let dir = std::env::temp_dir().join(format!(
            "oyo-settings-forced-preview-{}",
            std::process::id()
        ));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[ui]\nview_mode = \"unified\"\n").unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        let mut app = test_app(path.clone());
        app.ensure_topbar_tabs();
        let tab_id = app.active_topbar_tab.unwrap();
        app.topbar_tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .unwrap()
            .view_mode = ViewMode::UnifiedPane;
        app.view_mode = ViewMode::Preview;
        app.preview_forced_by_content = true;

        app.open_settings_tab();

        assert_eq!(app.view_mode, ViewMode::UnifiedPane);
        assert!(!app.preview_forced_by_content);
        assert!(!app.settings_dirty());
        assert!(app.save_settings());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_changes_are_live_but_only_save_writes_them() {
        let dir = std::env::temp_dir().join(format!("oyo-settings-stage-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            "[ui]\nline_wrap = false # keep\n\n[playback]\nautoplay = false\n",
        )
        .unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        let mut app = test_app(path.clone());
        app.ensure_topbar_tabs();
        app.open_settings_tab();
        app.activate_settings_item(SettingItem::LineWrap);
        app.activate_settings_item(SettingItem::Autoplay);
        assert!(app.line_wrap);
        assert!(app.autoplay);
        assert!(app.settings_dirty());
        assert!(app.setting_dirty(SettingItem::LineWrap));
        assert!(app.setting_dirty(SettingItem::Autoplay));
        assert!(!app.setting_dirty(SettingItem::Scrollbar));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        assert!(app.save_settings());
        assert!(!app.settings_dirty());
        assert!(SettingItem::ALL
            .into_iter()
            .all(|item| !app.setting_dirty(item)));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nline_wrap = true # keep\n\n[playback]\nautoplay = true\n"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn one_save_persists_every_changed_row_to_its_mapped_path() {
        let dir = std::env::temp_dir().join(format!("oyo-settings-all-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = test_app(path.clone());
        app.ensure_topbar_tabs();
        app.open_settings_tab();
        for item in SettingItem::ALL
            .into_iter()
            .filter(|item| !matches!(item, SettingItem::Theme | SettingItem::SyntaxTheme))
        {
            app.activate_settings_item(item);
        }
        for item in [SettingItem::SyntaxTheme, SettingItem::Theme] {
            app.activate_settings_item(item);
            for ch in "nord".chars() {
                app.push_theme_picker_char(ch);
            }
            app.apply_theme_picker_selection();
        }
        assert!(!path.exists());
        assert!(app.save_settings());
        let parsed =
            toml::from_str::<toml::Value>(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for item in SettingItem::ALL {
            let mut actual = &parsed;
            for key in setting_config_path(item) {
                actual = &actual[*key];
            }
            match app.live_setting_value(item) {
                SettingValue::Bool(expected) => {
                    assert_eq!(actual.as_bool(), Some(expected), "{item:?}")
                }
                SettingValue::Text(expected) => {
                    assert_eq!(actual.as_str(), Some(expected.as_str()), "{item:?}")
                }
                SettingValue::OptionalText(Some(expected)) => {
                    assert_eq!(actual.as_str(), Some(expected.as_str()), "{item:?}")
                }
                SettingValue::OptionalText(None) => panic!("theme should be selected"),
            }
        }
        oyo_core::MultiFileDiff::set_diff_defer(true);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn revert_restores_open_state_without_writing() {
        let dir = std::env::temp_dir().join(format!("oyo-settings-reset-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[ui]\nline_wrap = false\n").unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        let mut app = test_app(path.clone());
        app.line_wrap = false;
        app.ensure_topbar_tabs();
        app.open_settings_tab();
        app.activate_settings_item(SettingItem::LineWrap);
        app.revert_settings();
        assert!(!app.line_wrap);
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_override_is_dirty_on_open_and_revert_keeps_the_open_state() {
        let dir =
            std::env::temp_dir().join(format!("oyo-settings-override-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[ui]\nline_wrap = false\n").unwrap();
        let mut app = test_app(path.clone());
        app.line_wrap = true;
        app.ensure_topbar_tabs();
        app.open_settings_tab();
        assert!(app.settings_dirty());
        app.activate_settings_item(SettingItem::LineWrap);
        app.revert_settings();
        assert!(app.line_wrap);
        assert!(app.settings_dirty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nline_wrap = false\n"
        );
        assert!(app.save_settings());
        assert!(!app.settings_dirty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nline_wrap = true\n"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_save_and_revert_toasts_skip_noops() {
        let dir = std::env::temp_dir().join(format!("oyo-settings-toasts-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[ui]\nscrollbar = true\n").unwrap();
        let mut app = test_app(path.clone());
        app.ensure_topbar_tabs();
        app.open_settings_tab();
        while app.toast_engine.dismiss() {}

        assert!(app.save_settings());
        assert_eq!(app.toast_engine.queue_len(), 0);

        app.activate_settings_item(SettingItem::Scrollbar);
        assert!(app.save_settings());
        assert!(app
            .toast_engine
            .current_message()
            .is_some_and(|message| message.contains("Settings saved")));
        app.toast_engine.dismiss();
        assert!(app.save_settings());
        assert_eq!(app.toast_engine.queue_len(), 0);

        app.revert_settings();
        assert!(app
            .toast_engine
            .current_message()
            .is_some_and(|message| message.contains("Settings reverted")));
        app.toast_engine.dismiss();
        app.revert_settings();
        assert_eq!(app.toast_engine.queue_len(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn file_panel_position_cycles_live_and_uses_standard_settings_lifecycle() {
        let dir = std::env::temp_dir().join(format!(
            "oyo-settings-panel-position-{}",
            std::process::id()
        ));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[files]\npanel_position = \"right\" # keep\n").unwrap();
        let mut app = test_app(path.clone());
        app.ensure_topbar_tabs();
        app.open_settings_tab();

        assert_eq!(SettingItem::ALL.len(), 34);
        assert!(SETTINGS_ENTRIES.contains(&SettingsEntry::Item(SettingItem::FilePanelPosition)));
        assert_eq!(app.file_panel_position, FilePanelPosition::Right);
        assert_eq!(
            app.setting_display(SettingItem::FilePanelPosition),
            (
                "File panel side",
                "right".to_string(),
                "show the file panel on the left or right"
            )
        );

        app.settings_selection = SettingItem::FilePanelPosition as usize;
        app.adjust_selected_setting(false);
        assert_eq!(app.file_panel_position, FilePanelPosition::Left);
        assert!(app.setting_dirty(SettingItem::FilePanelPosition));
        app.adjust_selected_setting(true);
        assert_eq!(app.file_panel_position, FilePanelPosition::Right);
        assert!(!app.setting_dirty(SettingItem::FilePanelPosition));

        app.activate_selected_setting();
        assert_eq!(app.file_panel_position, FilePanelPosition::Left);
        app.revert_settings();
        assert_eq!(app.file_panel_position, FilePanelPosition::Right);
        app.reset_settings_to_defaults();
        assert_eq!(app.file_panel_position, FilePanelPosition::Left);
        assert!(app.settings_dirty());
        assert!(app.save_settings());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[files]\npanel_position = \"left\" # keep\n"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_controls_wrap_after_items() {
        let mut app = test_app(std::env::temp_dir().join("oyo-settings-nav.toml"));
        assert_eq!(
            app.settings_selected_target(),
            SettingsTarget::Item(SettingItem::ViewMode)
        );
        app.move_settings_selection(false);
        assert_eq!(
            app.settings_selected_target(),
            SettingsTarget::ResetDefaults
        );
        app.move_settings_selection(true);
        assert_eq!(
            app.settings_selected_target(),
            SettingsTarget::Item(SettingItem::ViewMode)
        );
        app.settings_selection = SettingItem::ALL.len() - 1;
        app.move_settings_selection(true);
        assert_eq!(app.settings_selected_target(), SettingsTarget::Save);
        app.move_settings_selection(true);
        assert_eq!(app.settings_selected_target(), SettingsTarget::Revert);
        app.move_settings_selection(true);
        assert_eq!(
            app.settings_selected_target(),
            SettingsTarget::ResetDefaults
        );
    }

    #[test]
    fn settings_adjust_both_directions_and_leave_pickers_to_enter() {
        let mut app = test_app(std::env::temp_dir().join("oyo-settings-adjust.toml"));

        app.settings_selection = SettingItem::ViewMode as usize;
        app.adjust_selected_setting(false);
        assert_eq!(app.view_mode, ViewMode::Evolution);
        app.adjust_selected_setting(true);
        assert_eq!(app.view_mode, ViewMode::UnifiedPane);

        app.settings_selection = SettingItem::LineWrap as usize;
        let line_wrap = app.line_wrap;
        app.adjust_selected_setting(false);
        assert_eq!(app.line_wrap, !line_wrap);
        app.adjust_selected_setting(true);
        assert_eq!(app.line_wrap, line_wrap);

        app.settings_selection = SettingItem::SyntaxTheme as usize;
        let syntax_theme = app.syntax_theme.clone();
        app.adjust_selected_setting(false);
        app.adjust_selected_setting(true);
        assert_eq!(app.syntax_theme, syntax_theme);
        assert!(!app.theme_picker_active());

        app.settings_selection = SettingsTarget::Save.index();
        app.adjust_selected_setting(false);
        assert_eq!(
            app.settings_selected_target(),
            SettingsTarget::ResetDefaults
        );
        app.adjust_selected_setting(true);
        assert_eq!(app.settings_selected_target(), SettingsTarget::Save);
        app.adjust_selected_setting(true);
        assert_eq!(app.settings_selected_target(), SettingsTarget::Revert);
    }

    #[test]
    fn reset_to_defaults_stages_defaults_and_revert_restores_open_state() {
        let dir =
            std::env::temp_dir().join(format!("oyo-settings-defaults-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            "# keep\n[ui]\nline_wrap = true\n\n[ui.syntax]\ntheme = \"nord\"\n\n[ui.theme]\nname = \"nord\"\n",
        )
        .unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        let mut app = test_app(path.clone());
        app.ensure_topbar_tabs();
        app.open_settings_tab();
        let open = SettingsSnapshot::live(&app);
        let defaults = SettingsSnapshot::config(&Config::default());

        app.settings_selection = SettingsTarget::ResetDefaults.index();
        app.activate_selected_setting();
        assert!(app.settings_reset_confirmation_active());
        assert_eq!(SettingsSnapshot::live(&app), open);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        app.handle_settings_reset_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(!app.settings_reset_confirmation_active());
        assert_eq!(SettingsSnapshot::live(&app), open);

        app.activate_selected_setting();
        app.handle_settings_reset_key(KeyEvent::new(
            KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(SettingsSnapshot::live(&app), defaults);
        assert!(app.settings_dirty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        app.settings_selection = SettingsTarget::Revert.index();
        app.activate_selected_setting();
        assert_eq!(SettingsSnapshot::live(&app), open);
        assert!(!app.settings_dirty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        app.settings_selection = SettingsTarget::ResetDefaults.index();
        app.activate_selected_setting();
        app.handle_settings_reset_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.settings_selection = SettingsTarget::Save.index();
        app.activate_selected_setting();
        assert!(!app.settings_dirty());
        assert_eq!(
            SettingsSnapshot::config(&Config::load_from_path(&path).unwrap()),
            defaults
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("# keep"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn picker_accept_stages_theme_until_save_and_cancel_restores() {
        let dir =
            std::env::temp_dir().join(format!("oyo-settings-theme-stage-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "# untouched\n").unwrap();
        let mut app = test_app(path.clone());
        app.ensure_topbar_tabs();
        app.open_settings_tab();

        app.activate_settings_item(SettingItem::SyntaxTheme);
        for ch in "nord".chars() {
            app.push_theme_picker_char(ch);
        }
        app.apply_theme_picker_selection();
        assert_eq!(app.syntax_theme, "nord");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# untouched\n");

        let original_ui = app.ui_theme_name.clone();
        app.activate_settings_item(SettingItem::Theme);
        for ch in "nord".chars() {
            app.push_theme_picker_char(ch);
        }
        app.stop_theme_picker();
        assert_eq!(app.ui_theme_name, original_ui);
        assert!(app.save_settings());
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("[ui.syntax]\ntheme = \"nord\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn leave_prompt_cancel_discard_and_save_have_distinct_results() {
        let dir = std::env::temp_dir().join(format!("oyo-settings-leave-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[ui]\nline_wrap = false\n").unwrap();
        let mut app = test_app(path.clone());
        app.ensure_topbar_tabs();
        let file_tab = app.active_topbar_tab.unwrap();
        app.open_settings_tab();
        app.activate_settings_item(SettingItem::LineWrap);

        app.select_topbar_tab(file_tab);
        assert!(app.settings_leave_confirmation_active());
        app.handle_settings_leave_key(KeyEvent::new(
            KeyCode::Right,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            app.settings_leave_selected_action(),
            SettingsLeaveAction::Discard
        );
        app.handle_settings_leave_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(app.active_settings_view());
        assert!(app.line_wrap);

        app.select_topbar_tab(file_tab);
        app.resolve_settings_leave(SettingsLeaveAction::Discard);
        assert!(!app.active_settings_view());
        assert!(!app.line_wrap);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nline_wrap = false\n"
        );

        app.open_settings_tab();
        app.activate_settings_item(SettingItem::LineWrap);
        app.select_topbar_tab(file_tab);
        app.resolve_settings_leave(SettingsLeaveAction::Save);
        assert!(!app.active_settings_view());
        assert!(app.line_wrap);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nline_wrap = true\n"
        );

        app.open_settings_tab();
        let settings_tab = app.active_topbar_tab.unwrap();
        app.activate_settings_item(SettingItem::LineWrap);
        app.close_active_topbar_tab();
        assert!(app.settings_leave_confirmation_active());
        app.resolve_settings_leave(SettingsLeaveAction::Discard);
        assert!(!app.topbar_tabs.iter().any(|tab| tab.id == settings_tab));
        assert!(app.line_wrap);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn current_view_and_history_entry_points_cannot_bypass_leave_prompt() {
        let dir = std::env::temp_dir().join(format!("oyo-settings-guards-{}", std::process::id()));
        let mut app = test_app(dir.join("config.toml"));
        app.ensure_topbar_tabs();
        app.open_settings_tab();
        app.activate_settings_item(SettingItem::LineWrap);

        app.open_help_in_current_tab();
        assert!(app.settings_leave_confirmation_active());
        app.cancel_settings_leave_confirmation();
        assert!(app.active_settings_view());

        app.new_topbar_tab();
        assert!(app.settings_leave_confirmation_active());
        app.cancel_settings_leave_confirmation();
        assert!(app.active_settings_view());

        app.select_file(0);
        assert!(app.settings_leave_confirmation_active());
        app.cancel_settings_leave_confirmation();
        assert!(app.active_settings_view());

        assert!(app.navigate_view_back());
        assert!(app.settings_leave_confirmation_active());
        app.cancel_settings_leave_confirmation();
        assert!(app.active_settings_view());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_leave_save_stays_in_settings() {
        let path = std::env::temp_dir().join(format!("oyo-settings-fail-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "not a directory").unwrap();
        let mut app = test_app(path.join("config.toml"));
        app.ensure_topbar_tabs();
        let file_tab = app.active_topbar_tab.unwrap();
        app.open_settings_tab();
        while app.toast_engine.dismiss() {}
        app.activate_settings_item(SettingItem::Scrollbar);
        app.select_topbar_tab(file_tab);
        app.resolve_settings_leave(SettingsLeaveAction::Save);
        assert!(app.active_settings_view());
        assert!(app.settings_dirty());
        assert!(app
            .toast_engine
            .current_message()
            .is_some_and(|message| message.contains("Could not save settings")));
        assert!(!app
            .toast_engine
            .current_message()
            .is_some_and(|message| message.contains("Settings saved")));
        let _ = std::fs::remove_file(path);
    }
}
