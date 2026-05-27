use crate::config::KeybindingsConfig;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use keymap::{parser::parse_seq, Config, Item, KeyMap, Matcher, ToKeyMap};
use std::collections::{BTreeMap, HashSet};
use std::hash::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum KeybindingMode {
    Normal,
    Help,
    ReviewEditor,
    CommandPalette,
    FileSearch,
    FileFilter,
    Goto,
    Search,
    Dashboard,
    DashboardFilter,
}

impl KeybindingMode {
    fn id(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Help => "help",
            Self::ReviewEditor => "review_editor",
            Self::CommandPalette => "command_palette",
            Self::FileSearch => "file_search",
            Self::FileFilter => "file_filter",
            Self::Goto => "goto",
            Self::Search => "search",
            Self::Dashboard => "dashboard",
            Self::DashboardFilter => "dashboard_filter",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum NormalAction {
    Quit,
    StepDown,
    StepUp,
    NextHunk,
    PrevHunk,
    HunkStart,
    HunkEnd,
    BlameHint,
    TogglePeekChange,
    TogglePeekHunk,
    YankChange,
    YankHunk,
    YankChangePatch,
    YankHunkPatch,
    TogglePathPopup,
    OpenEditor,
    GotoStart,
    GotoEnd,
    FirstStep,
    LastStep,
    PrevFile,
    NextFile,
    ToggleAutoplay,
    ToggleAutoplayReverse,
    ToggleViewMode,
    ToggleViewModeReverse,
    ScrollUp,
    ScrollDown,
    HalfPageUp,
    HalfPageDown,
    ToggleFileListFocus,
    IncreaseSpeed,
    DecreaseSpeed,
    ToggleAnimation,
    ToggleLineWrap,
    ToggleSyntax,
    ToggleEvoSyntax,
    ToggleStepping,
    ToggleStrikethrough,
    ScrollLeft,
    ScrollRight,
    LineStart,
    LineEnd,
    CenterActive,
    ToggleZen,
    ReplayStep,
    Refresh,
    ToggleFilePanel,
    ToggleFoldContext,
    OpenSearchOrFileFilter,
    OpenGoto,
    SearchNext,
    SearchPrev,
    NextConflict,
    PrevConflict,
    LineComment,
    HunkComment,
    ClearComments,
    RemoveLineComment,
    RemoveHunkComment,
    ToggleHelp,
    OpenCommandPalette,
    OpenFileSearch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HelpAction {
    Close,
    ScrollDown,
    ScrollUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ReviewEditorAction {
    Cancel,
    Save,
    InsertNewline,
    AcceptMention,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Clear,
    MentionNext,
    MentionPrev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PickerAction {
    Cancel,
    Accept,
    Backspace,
    Clear,
    SelectNext,
    SelectPrev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LineInputAction {
    Cancel,
    Accept,
    Backspace,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FileFilterAction {
    Close,
    Backspace,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DashboardAction {
    Quit,
    StartFilter,
    ClearPin,
    TogglePin,
    Accept,
    SelectNext,
    SelectPrev,
    PageDown,
    PageUp,
    SelectFirst,
    SelectLast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DashboardFilterAction {
    Cancel,
    Accept,
    Clear,
    Backspace,
    SelectNext,
    SelectPrev,
    PageDown,
    PageUp,
    SelectFirst,
    SelectLast,
}

pub(crate) trait BindingAction: Copy + Eq + Hash {
    fn id(self) -> &'static str;
    fn description(self) -> &'static str;
    fn defaults(self) -> &'static [&'static str];
    fn all() -> &'static [Self];
}

macro_rules! binding_action {
    ($ty:ty, [$($variant:ident => ($id:literal, $desc:literal, [$($key:literal),* $(,)?])),* $(,)?]) => {
        impl BindingAction for $ty {
            fn id(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)*
                }
            }

            fn description(self) -> &'static str {
                match self {
                    $(Self::$variant => $desc,)*
                }
            }

            fn defaults(self) -> &'static [&'static str] {
                match self {
                    $(Self::$variant => &[$($key),*],)*
                }
            }

            fn all() -> &'static [Self] {
                &[$(Self::$variant),*]
            }
        }
    };
}

binding_action!(NormalAction, [
    Quit => ("quit", "Quit (prints comments if any)", ["q", "esc"]),
    StepDown => ("step_down", "Step forward", ["j", "down"]),
    StepUp => ("step_up", "Step backward", ["k", "up"]),
    NextHunk => ("next_hunk", "Next hunk", ["l", "right"]),
    PrevHunk => ("prev_hunk", "Previous hunk", ["h", "left"]),
    HunkStart => ("hunk_start", "Hunk begin", ["b"]),
    HunkEnd => ("hunk_end", "Hunk end", ["e"]),
    BlameHint => ("blame_hint", "Blame current step", ["g b"]),
    TogglePeekChange => ("toggle_peek_change", "Peek change", ["p"]),
    TogglePeekHunk => ("toggle_peek_hunk", "Peek old hunk", ["P"]),
    YankChange => ("yank_change", "Yank line", ["y"]),
    YankHunk => ("yank_hunk", "Yank hunk", ["Y"]),
    YankChangePatch => ("yank_change_patch", "Copy line patch", ["g y"]),
    YankHunkPatch => ("yank_hunk_patch", "Copy hunk patch", ["g Y"]),
    TogglePathPopup => ("toggle_path_popup", "Show full file path", ["ctrl-g"]),
    OpenEditor => ("open_editor", "Open file in editor", ["o", "ctrl-e"]),
    GotoStart => ("goto_start", "Go to start", ["g g", "home"]),
    GotoEnd => ("goto_end", "Go to end", ["G", "end"]),
    FirstStep => ("first_step", "First step (or hunk in no-step)", ["<"]),
    LastStep => ("last_step", "Last step (or hunk in no-step)", [">"]),
    PrevFile => ("prev_file", "Previous file", ["["]),
    NextFile => ("next_file", "Next file", ["]"]),
    ToggleAutoplay => ("toggle_autoplay", "Autoplay forward", ["space"]),
    ToggleAutoplayReverse => ("toggle_autoplay_reverse", "Autoplay reverse", ["B"]),
    ToggleViewMode => ("toggle_view_mode", "Cycle view mode", ["tab"]),
    ToggleViewModeReverse => ("toggle_view_mode_reverse", "Cycle view mode reverse", ["backtab"]),
    ScrollUp => ("scroll_up", "Scroll up", ["K"]),
    ScrollDown => ("scroll_down", "Scroll down", ["J"]),
    HalfPageUp => ("half_page_up", "Scroll half-page up", ["ctrl-u"]),
    HalfPageDown => ("half_page_down", "Scroll half-page down", ["ctrl-d"]),
    ToggleFileListFocus => ("toggle_file_list_focus", "Focus file list", ["enter", "ctrl-a"]),
    IncreaseSpeed => ("increase_speed", "Increase speed", ["+", "="]),
    DecreaseSpeed => ("decrease_speed", "Decrease speed", ["-"]),
    ToggleAnimation => ("toggle_animation", "Toggle animation", ["a"]),
    ToggleLineWrap => ("toggle_line_wrap", "Toggle line wrap", ["w"]),
    ToggleSyntax => ("toggle_syntax", "Toggle syntax highlight", ["t"]),
    ToggleEvoSyntax => ("toggle_evo_syntax", "Toggle evo syntax", ["E"]),
    ToggleStepping => ("toggle_stepping", "Toggle stepping", ["s"]),
    ToggleStrikethrough => ("toggle_strikethrough", "Toggle strikethrough", ["S"]),
    ScrollLeft => ("scroll_left", "Scroll left", ["H"]),
    ScrollRight => ("scroll_right", "Scroll right", ["L"]),
    LineStart => ("line_start", "Scroll to line start", ["0"]),
    LineEnd => ("line_end", "Scroll to line end", ["$"]),
    CenterActive => ("center_active", "Center on active", ["z"]),
    ToggleZen => ("toggle_zen", "Zen mode", ["Z"]),
    ReplayStep => ("replay_step", "Replay last step", ["r"]),
    Refresh => ("refresh", "Refresh files", ["R"]),
    ToggleFilePanel => ("toggle_file_panel", "Toggle file panel", ["ctrl-f"]),
    ToggleFoldContext => ("toggle_fold_context", "Toggle context folding", ["f"]),
    OpenSearchOrFileFilter => ("open_search_or_file_filter", "Search or filter files", ["/"]),
    OpenGoto => ("open_goto", "Go to line/hunk/step", [":"]),
    SearchNext => ("search_next", "Next match", ["n"]),
    SearchPrev => ("search_prev", "Previous match", ["N"]),
    NextConflict => ("next_conflict", "Next conflict", ["c"]),
    PrevConflict => ("prev_conflict", "Previous conflict", ["C"]),
    LineComment => ("line_comment", "Add/update line comment", ["m"]),
    HunkComment => ("hunk_comment", "Add/update hunk comment", ["M"]),
    ClearComments => ("clear_comments", "Clear all comments", ["ctrl-x"]),
    RemoveLineComment => ("remove_line_comment", "Remove line comment", ["x"]),
    RemoveHunkComment => ("remove_hunk_comment", "Remove hunk comment", ["X"]),
    ToggleHelp => ("toggle_help", "Toggle help", ["?"]),
    OpenCommandPalette => ("open_command_palette", "Command palette", ["ctrl-p"]),
    OpenFileSearch => ("open_file_search", "Quick file search", ["ctrl-shift-p"]),
]);

binding_action!(HelpAction, [
    Close => ("close", "Close help", ["esc", "q", "?"]),
    ScrollDown => ("scroll_down", "Scroll down", ["j", "down"]),
    ScrollUp => ("scroll_up", "Scroll up", ["k", "up"]),
]);

binding_action!(ReviewEditorAction, [
    Cancel => ("cancel", "Cancel editor", ["esc"]),
    Save => ("save", "Save comment", ["ctrl-enter"]),
    InsertNewline => ("insert_newline", "Insert newline", ["enter"]),
    AcceptMention => ("accept_mention", "Accept mention", ["tab"]),
    Backspace => ("backspace", "Backspace", ["backspace"]),
    Delete => ("delete", "Delete", ["delete"]),
    Left => ("left", "Move left", ["left"]),
    Right => ("right", "Move right", ["right"]),
    Up => ("up", "Move up", ["up"]),
    Down => ("down", "Move down", ["down"]),
    Home => ("home", "Move to line start", ["home"]),
    End => ("end", "Move to line end", ["end"]),
    Clear => ("clear", "Clear text", ["ctrl-u"]),
    MentionNext => ("mention_next", "Next mention candidate", ["ctrl-n"]),
    MentionPrev => ("mention_prev", "Previous mention candidate", ["ctrl-p"]),
]);

binding_action!(PickerAction, [
    Cancel => ("cancel", "Cancel", ["esc"]),
    Accept => ("accept", "Accept", ["enter"]),
    Backspace => ("backspace", "Backspace", ["backspace"]),
    Clear => ("clear", "Clear query", ["ctrl-u"]),
    SelectNext => ("select_next", "Select next", ["down"]),
    SelectPrev => ("select_prev", "Select previous", ["up"]),
]);

binding_action!(LineInputAction, [
    Cancel => ("cancel", "Cancel", ["esc"]),
    Accept => ("accept", "Accept", ["enter"]),
    Backspace => ("backspace", "Backspace", ["backspace"]),
    Clear => ("clear", "Clear query", ["ctrl-u"]),
]);

binding_action!(FileFilterAction, [
    Close => ("close", "Close filter", ["esc", "enter"]),
    Backspace => ("backspace", "Backspace", ["backspace"]),
    Clear => ("clear", "Clear filter", ["ctrl-u"]),
]);

binding_action!(DashboardAction, [
    Quit => ("quit", "Quit dashboard", ["esc", "q"]),
    StartFilter => ("start_filter", "Filter commits", ["/"]),
    ClearPin => ("clear_pin", "Clear pinned range start", ["r"]),
    TogglePin => ("toggle_pin", "Mark range start", ["space"]),
    Accept => ("accept", "Open selection", ["enter"]),
    SelectNext => ("select_next", "Select next", ["j", "down"]),
    SelectPrev => ("select_prev", "Select previous", ["k", "up"]),
    PageDown => ("page_down", "Page down", ["pagedown"]),
    PageUp => ("page_up", "Page up", ["pageup"]),
    SelectFirst => ("select_first", "Select first", ["g", "home"]),
    SelectLast => ("select_last", "Select last", ["G", "end"]),
]);

binding_action!(DashboardFilterAction, [
    Cancel => ("cancel", "Cancel filter", ["esc"]),
    Accept => ("accept", "Open selection", ["enter"]),
    Clear => ("clear", "Clear filter", ["ctrl-u"]),
    Backspace => ("backspace", "Backspace", ["backspace"]),
    SelectNext => ("select_next", "Select next", ["j", "down"]),
    SelectPrev => ("select_prev", "Select previous", ["k", "up"]),
    PageDown => ("page_down", "Page down", ["pagedown"]),
    PageUp => ("page_up", "Page up", ["pageup"]),
    SelectFirst => ("select_first", "Select first", ["g", "home"]),
    SelectLast => ("select_last", "Select last", ["G", "end"]),
]);

#[derive(Debug)]
pub(crate) struct Keybindings {
    normal: ModeBindings<NormalAction>,
    help: ModeBindings<HelpAction>,
    review_editor: ModeBindings<ReviewEditorAction>,
    command_palette: ModeBindings<PickerAction>,
    file_search: ModeBindings<PickerAction>,
    file_filter: ModeBindings<FileFilterAction>,
    goto: ModeBindings<LineInputAction>,
    search: ModeBindings<LineInputAction>,
    dashboard: ModeBindings<DashboardAction>,
    dashboard_filter: ModeBindings<DashboardFilterAction>,
    active_sequence_mode: Option<KeybindingMode>,
}

#[derive(Debug)]
struct ModeBindings<A> {
    mode: KeybindingMode,
    config: Config<A>,
    effective: BTreeMap<&'static str, Vec<String>>,
    prefix_matcher: Matcher<()>,
    buffer: Vec<KeyEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dispatch<A> {
    Matched(A),
    Pending,
    Unmatched,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::from_config(&KeybindingsConfig::default())
    }
}

impl Keybindings {
    pub(crate) fn from_config(config: &KeybindingsConfig) -> Self {
        let mut warnings = Vec::new();
        Self::from_config_with_warnings(config, &mut warnings)
    }

    pub(crate) fn from_config_with_warnings(
        config: &KeybindingsConfig,
        warnings: &mut Vec<String>,
    ) -> Self {
        warn_unknown_modes(config, warnings);
        Self {
            normal: ModeBindings::build(KeybindingMode::Normal, config, warnings),
            help: ModeBindings::build(KeybindingMode::Help, config, warnings),
            review_editor: ModeBindings::build(KeybindingMode::ReviewEditor, config, warnings),
            command_palette: ModeBindings::build(KeybindingMode::CommandPalette, config, warnings),
            file_search: ModeBindings::build(KeybindingMode::FileSearch, config, warnings),
            file_filter: ModeBindings::build(KeybindingMode::FileFilter, config, warnings),
            goto: ModeBindings::build(KeybindingMode::Goto, config, warnings),
            search: ModeBindings::build(KeybindingMode::Search, config, warnings),
            dashboard: ModeBindings::build(KeybindingMode::Dashboard, config, warnings),
            dashboard_filter: ModeBindings::build(
                KeybindingMode::DashboardFilter,
                config,
                warnings,
            ),
            active_sequence_mode: None,
        }
    }

    pub(crate) fn clear_sequence(&mut self) {
        match self.active_sequence_mode.take() {
            Some(KeybindingMode::Normal) => self.normal.clear_sequence(),
            Some(KeybindingMode::Help) => self.help.clear_sequence(),
            Some(KeybindingMode::ReviewEditor) => self.review_editor.clear_sequence(),
            Some(KeybindingMode::CommandPalette) => self.command_palette.clear_sequence(),
            Some(KeybindingMode::FileSearch) => self.file_search.clear_sequence(),
            Some(KeybindingMode::FileFilter) => self.file_filter.clear_sequence(),
            Some(KeybindingMode::Goto) => self.goto.clear_sequence(),
            Some(KeybindingMode::Search) => self.search.clear_sequence(),
            Some(KeybindingMode::Dashboard) => self.dashboard.clear_sequence(),
            Some(KeybindingMode::DashboardFilter) => self.dashboard_filter.clear_sequence(),
            None => {}
        }
    }

    pub(crate) fn normal(&mut self, key: KeyEvent) -> Dispatch<NormalAction> {
        self.prepare_mode(KeybindingMode::Normal);
        dispatch_mode(&mut self.active_sequence_mode, &mut self.normal, key)
    }

    pub(crate) fn help(&mut self, key: KeyEvent) -> Dispatch<HelpAction> {
        self.prepare_mode(KeybindingMode::Help);
        dispatch_mode(&mut self.active_sequence_mode, &mut self.help, key)
    }

    pub(crate) fn review_editor(&mut self, key: KeyEvent) -> Dispatch<ReviewEditorAction> {
        self.prepare_mode(KeybindingMode::ReviewEditor);
        dispatch_mode(&mut self.active_sequence_mode, &mut self.review_editor, key)
    }

    pub(crate) fn command_palette(&mut self, key: KeyEvent) -> Dispatch<PickerAction> {
        self.prepare_mode(KeybindingMode::CommandPalette);
        dispatch_mode(
            &mut self.active_sequence_mode,
            &mut self.command_palette,
            key,
        )
    }

    pub(crate) fn file_search(&mut self, key: KeyEvent) -> Dispatch<PickerAction> {
        self.prepare_mode(KeybindingMode::FileSearch);
        dispatch_mode(&mut self.active_sequence_mode, &mut self.file_search, key)
    }

    pub(crate) fn file_filter(&mut self, key: KeyEvent) -> Dispatch<FileFilterAction> {
        self.prepare_mode(KeybindingMode::FileFilter);
        dispatch_mode(&mut self.active_sequence_mode, &mut self.file_filter, key)
    }

    pub(crate) fn goto(&mut self, key: KeyEvent) -> Dispatch<LineInputAction> {
        self.prepare_mode(KeybindingMode::Goto);
        dispatch_mode(&mut self.active_sequence_mode, &mut self.goto, key)
    }

    pub(crate) fn search(&mut self, key: KeyEvent) -> Dispatch<LineInputAction> {
        self.prepare_mode(KeybindingMode::Search);
        dispatch_mode(&mut self.active_sequence_mode, &mut self.search, key)
    }

    pub(crate) fn dashboard(&mut self, key: KeyEvent) -> Dispatch<DashboardAction> {
        self.prepare_mode(KeybindingMode::Dashboard);
        dispatch_mode(&mut self.active_sequence_mode, &mut self.dashboard, key)
    }

    pub(crate) fn dashboard_filter(&mut self, key: KeyEvent) -> Dispatch<DashboardFilterAction> {
        self.prepare_mode(KeybindingMode::DashboardFilter);
        dispatch_mode(
            &mut self.active_sequence_mode,
            &mut self.dashboard_filter,
            key,
        )
    }

    pub(crate) fn normal_keys(&self, action: NormalAction) -> String {
        self.normal.keys_label(action)
    }

    pub(crate) fn help_keys(&self, action: HelpAction) -> String {
        self.help.keys_label(action)
    }

    pub(crate) fn review_editor_keys(&self, action: ReviewEditorAction) -> String {
        self.review_editor.keys_label(action)
    }

    pub(crate) fn dashboard_keys(&self, action: DashboardAction) -> String {
        self.dashboard.keys_label(action)
    }

    fn prepare_mode(&mut self, mode: KeybindingMode) {
        if self
            .active_sequence_mode
            .is_some_and(|active| active != mode)
        {
            self.clear_sequence();
        }
    }
}

fn dispatch_mode<A: BindingAction + 'static>(
    active_sequence_mode: &mut Option<KeybindingMode>,
    mode: &mut ModeBindings<A>,
    key: KeyEvent,
) -> Dispatch<A> {
    let result = mode.dispatch(key);
    match result {
        Dispatch::Pending => *active_sequence_mode = Some(mode.mode),
        _ => *active_sequence_mode = None,
    }
    result
}

impl<A: BindingAction + 'static> ModeBindings<A> {
    fn build(mode: KeybindingMode, config: &KeybindingsConfig, warnings: &mut Vec<String>) -> Self {
        let mut effective = default_bindings::<A>();
        if let Some(overrides) = config.modes.get(mode.id()) {
            for id in overrides.keys() {
                if !A::all().iter().any(|action| action.id() == id) {
                    warnings.push(format!(
                        "Ignoring unknown keybinding action '{}.{}'",
                        mode.id(),
                        id
                    ));
                }
            }
            for action in A::all() {
                if let Some(keys) = overrides.get(action.id()) {
                    effective.insert(action.id(), keys.clone());
                }
            }
        }

        if let Err(error) = validate_effective_bindings::<A>(&effective) {
            warnings.push(format!(
                "Ignoring [keybindings.{}]: {}; using defaults for this mode",
                mode.id(),
                error
            ));
            effective = default_bindings::<A>();
        }

        let items = A::all()
            .iter()
            .map(|action| {
                (
                    *action,
                    Item::new(
                        effective.get(action.id()).cloned().unwrap_or_default(),
                        action.description().to_string(),
                    ),
                )
            })
            .collect();
        let config = Config::new(items);
        let prefix_matcher = prefix_matcher(&effective);
        Self {
            mode,
            config,
            effective,
            prefix_matcher,
            buffer: Vec::new(),
        }
    }

    fn dispatch(&mut self, key: KeyEvent) -> Dispatch<A> {
        let mut retry = Some(canonical_key(key));
        while let Some(next_key) = retry.take() {
            self.buffer.push(next_key);
            if let Some(action) = self.config.get_seq(&self.buffer).copied() {
                self.buffer.clear();
                return Dispatch::Matched(action);
            }
            let keymaps = match keymaps_for_events(&self.buffer) {
                Some(keymaps) => keymaps,
                None => {
                    self.buffer.clear();
                    return Dispatch::Unmatched;
                }
            };
            if self.prefix_matcher.get(&keymaps).is_some() {
                return Dispatch::Pending;
            }
            let failed_len = self.buffer.len();
            self.buffer.clear();
            if failed_len > 1 {
                retry = Some(next_key);
            }
        }
        Dispatch::Unmatched
    }

    fn clear_sequence(&mut self) {
        self.buffer.clear();
    }

    fn keys_label(&self, action: A) -> String {
        self.effective
            .get(action.id())
            .map(|keys| keys.join(" / "))
            .unwrap_or_default()
    }
}

fn warn_unknown_modes(config: &KeybindingsConfig, warnings: &mut Vec<String>) {
    for mode in config.modes.keys() {
        if ![
            KeybindingMode::Normal.id(),
            KeybindingMode::Help.id(),
            KeybindingMode::ReviewEditor.id(),
            KeybindingMode::CommandPalette.id(),
            KeybindingMode::FileSearch.id(),
            KeybindingMode::FileFilter.id(),
            KeybindingMode::Goto.id(),
            KeybindingMode::Search.id(),
            KeybindingMode::Dashboard.id(),
            KeybindingMode::DashboardFilter.id(),
        ]
        .contains(&mode.as_str())
        {
            warnings.push(format!(
                "Ignoring unknown keybinding mode [keybindings.{}]",
                mode
            ));
        }
    }
}

fn default_bindings<A: BindingAction + 'static>() -> BTreeMap<&'static str, Vec<String>> {
    A::all()
        .iter()
        .map(|action| {
            (
                action.id(),
                action
                    .defaults()
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect(),
            )
        })
        .collect()
}

fn validate_effective_bindings<A: BindingAction + 'static>(
    bindings: &BTreeMap<&'static str, Vec<String>>,
) -> Result<(), String> {
    let mut seen: Vec<(Vec<KeyMap>, &'static str, String)> = Vec::new();
    let mut ids = HashSet::new();
    for action in A::all() {
        ids.insert(action.id());
    }

    for (id, keys) in bindings {
        if !ids.contains(id) {
            continue;
        }
        for key in keys {
            let parsed = parse_seq(key)
                .map_err(|error| format!("invalid key '{}' for '{}': {}", key, id, error))?;
            if let Some((_, other_id, other_key)) = seen.iter().find(|(existing, _, _)| {
                existing == &parsed
                    || existing.starts_with(&parsed)
                    || parsed.starts_with(existing.as_slice())
            }) {
                return Err(format!(
                    "key '{}' for '{}' conflicts with '{}' for '{}'",
                    key, id, other_key, other_id
                ));
            }
            seen.push((parsed, id, key.clone()));
        }
    }
    Ok(())
}

fn prefix_matcher(bindings: &BTreeMap<&'static str, Vec<String>>) -> Matcher<()> {
    let mut matcher = Matcher::new();
    for keys in bindings.values() {
        for key in keys {
            if let Ok(sequence) = parse_seq(key) {
                for len in 1..sequence.len() {
                    matcher.add(sequence[..len].to_vec(), ());
                }
            }
        }
    }
    matcher
}

fn keymaps_for_events(events: &[KeyEvent]) -> Option<Vec<KeyMap>> {
    events
        .iter()
        .map(|event| event.to_keymap().ok())
        .collect::<Option<Vec<_>>>()
}

fn canonical_key(mut key: KeyEvent) -> KeyEvent {
    let KeyCode::Char(c) = key.code else {
        return key;
    };
    if !key.modifiers.contains(KeyModifiers::SHIFT) || !c.is_ascii_uppercase() {
        return key;
    }
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        key.code = KeyCode::Char(c.to_ascii_lowercase());
    } else {
        key.modifiers.remove(KeyModifiers::SHIFT);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::empty())
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    #[test]
    fn keybinding_override_replaces_one_action_and_keeps_other_defaults() {
        let config: crate::config::Config = toml::from_str(
            r#"
            [keybindings.normal]
            step_down = ["v"]
            "#,
        )
        .unwrap();
        let mut warnings = Vec::new();
        let mut bindings =
            Keybindings::from_config_with_warnings(&config.keybindings, &mut warnings);

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            bindings.normal(key('v')),
            Dispatch::Matched(NormalAction::StepDown)
        );
        assert_eq!(
            bindings.normal(key('j')),
            Dispatch::Unmatched,
            "overriding an action should replace, not add to, its default keys"
        );
        assert_eq!(
            bindings.normal(key('q')),
            Dispatch::Matched(NormalAction::Quit),
            "omitted actions should keep their default bindings"
        );
    }

    #[test]
    fn sequence_prefix_waits_and_failed_sequence_retries_latest_key() {
        let mut bindings = Keybindings::default();

        assert_eq!(bindings.normal(key('g')), Dispatch::Pending);
        assert_eq!(
            bindings.normal(key('j')),
            Dispatch::Matched(NormalAction::StepDown)
        );
    }

    #[test]
    fn duplicate_effective_key_falls_back_mode_to_defaults() {
        let config = KeybindingsConfig {
            modes: BTreeMap::from([(
                "normal".to_string(),
                BTreeMap::from([("toggle_help".to_string(), vec!["q".to_string()])]),
            )]),
        };
        let mut warnings = Vec::new();
        let mut bindings = Keybindings::from_config_with_warnings(&config, &mut warnings);

        assert!(warnings
            .iter()
            .any(|warning| warning.contains("using defaults for this mode")));
        assert_eq!(
            bindings.normal(key('?')),
            Dispatch::Matched(NormalAction::ToggleHelp)
        );
        assert_eq!(
            bindings.normal(key('q')),
            Dispatch::Matched(NormalAction::Quit)
        );
    }

    #[test]
    fn prefix_conflict_falls_back_mode_to_defaults() {
        let config = KeybindingsConfig {
            modes: BTreeMap::from([(
                "normal".to_string(),
                BTreeMap::from([("goto_start".to_string(), vec!["g".to_string()])]),
            )]),
        };
        let mut warnings = Vec::new();
        let mut bindings = Keybindings::from_config_with_warnings(&config, &mut warnings);

        assert!(warnings
            .iter()
            .any(|warning| warning.contains("using defaults for this mode")));
        assert_eq!(
            bindings.normal(key('g')),
            Dispatch::Pending,
            "prefix conflicts should be rejected so longer default sequences remain reachable"
        );
    }

    #[test]
    fn switching_modes_clears_pending_sequence() {
        let mut bindings = Keybindings::default();

        assert_eq!(bindings.normal(key('g')), Dispatch::Pending);
        assert_eq!(
            bindings.help(key('q')),
            Dispatch::Matched(HelpAction::Close)
        );
        assert_eq!(
            bindings.normal(key('g')),
            Dispatch::Pending,
            "returning to normal mode should not reuse the stale first g"
        );
    }

    #[test]
    fn ctrl_shift_binding_matches_configured_default() {
        let mut bindings = Keybindings::default();
        let key = KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );

        assert_eq!(
            bindings.normal(key),
            Dispatch::Matched(NormalAction::OpenFileSearch)
        );
        assert_eq!(
            bindings.normal(ctrl('p')),
            Dispatch::Matched(NormalAction::OpenCommandPalette)
        );
        let key = KeyEvent::new(
            KeyCode::Char('P'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(
            bindings.normal(key),
            Dispatch::Matched(NormalAction::OpenFileSearch)
        );
    }

    #[test]
    fn shifted_uppercase_events_match_uppercase_defaults() {
        let mut bindings = Keybindings::default();
        let key = KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT);

        assert_eq!(
            bindings.normal(key),
            Dispatch::Matched(NormalAction::TogglePeekHunk)
        );
    }
}
