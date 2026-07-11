use crate::config::ResolvedTheme;
use crate::jless::flatjson::{
    parse_top_level_json, parse_top_level_yaml, FlatJson, Index, OptionIndex, PathType, Row,
};
use crate::jless::lineprinter::{LineNumber, LinePrinter};
use crate::jless::terminal::{Color as JlessColor, Style as JlessStyle, Terminal};
use crate::jless::truncatedstrview::TruncatedStrView;
use crate::jless::types::TTYDimensions;
use crate::jless::viewer::{Action, JsonViewer, Mode};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fmt::{self, Write};
use std::hash::{Hash, Hasher};

const TAB_SIZE: isize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StructuredPreviewKind {
    Json,
    Yaml,
    Toml,
}

impl StructuredPreviewKind {
    pub(crate) fn from_file_name(file_name: &str) -> Option<Self> {
        let name = file_name.rsplit('/').next().unwrap_or(file_name);
        if matches!(name, "Cargo.lock" | "poetry.lock" | "uv.lock") {
            return Some(Self::Toml);
        }
        let ext = name.rsplit('.').next()?.to_ascii_lowercase();
        match ext.as_str() {
            "json" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            "toml" => Some(Self::Toml),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct StructuredPreviewSignature {
    kind: StructuredPreviewKind,
    file_name: String,
    len: usize,
    hash: u64,
}

impl StructuredPreviewSignature {
    pub(crate) fn new(kind: StructuredPreviewKind, file_name: &str, text: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        Self {
            kind,
            file_name: file_name.to_string(),
            len: text.len(),
            hash: hasher.finish(),
        }
    }
}

pub(crate) struct StructuredPreviewChangeBars {
    pub(crate) marker: String,
    pub(crate) marker_width: usize,
    pub(crate) styles: HashMap<String, Style>,
}

impl StructuredPreviewChangeBars {
    pub(crate) fn gutter_width(&self) -> usize {
        self.marker_width + 1
    }
}

pub(crate) struct StructuredPreviewState {
    signature: StructuredPreviewSignature,
    viewer: JsonViewer,
    truncated_row_value_views: HashMap<usize, TruncatedStrView>,
}

impl StructuredPreviewState {
    pub(crate) fn new(signature: StructuredPreviewSignature, text: &str) -> Result<Self, String> {
        let flatjson = match signature.kind {
            StructuredPreviewKind::Json => parse_top_level_json(text.to_string()),
            StructuredPreviewKind::Yaml => parse_top_level_yaml(text.to_string()),
            StructuredPreviewKind::Toml => parse_top_level_toml(text),
        }?;
        let viewer = JsonViewer::new(flatjson, Mode::Data);
        Ok(Self {
            signature,
            viewer,
            truncated_row_value_views: HashMap::new(),
        })
    }

    pub(crate) fn signature(&self) -> &StructuredPreviewSignature {
        &self.signature
    }

    pub(crate) fn set_dimensions(&mut self, width: u16, height: u16) {
        self.viewer
            .perform_action(Action::ResizeViewerDimensions(TTYDimensions {
                width,
                height,
            }));
    }

    pub(crate) fn move_up(&mut self, count: usize) {
        self.viewer.perform_action(Action::MoveUp(count));
    }

    pub(crate) fn move_down(&mut self, count: usize) {
        self.viewer.perform_action(Action::MoveDown(count));
    }

    pub(crate) fn move_left(&mut self) {
        self.viewer.perform_action(Action::MoveLeft);
    }

    pub(crate) fn move_right(&mut self) {
        self.viewer.perform_action(Action::MoveRight);
    }

    pub(crate) fn toggle_collapsed(&mut self) {
        self.viewer.perform_action(Action::ToggleCollapsed);
    }

    pub(crate) fn collapse_node_and_siblings(&mut self) {
        self.viewer.perform_action(Action::CollapseNodeAndSiblings);
    }

    pub(crate) fn deep_collapse_node_and_siblings(&mut self) {
        self.viewer
            .perform_action(Action::DeepCollapseNodeAndSiblings);
    }

    pub(crate) fn expand_node_and_siblings(&mut self) {
        self.viewer.perform_action(Action::ExpandNodeAndSiblings);
    }

    pub(crate) fn deep_expand_node_and_siblings(&mut self) {
        self.viewer
            .perform_action(Action::DeepExpandNodeAndSiblings);
    }

    pub(crate) fn toggle_mode(&mut self) {
        self.viewer.perform_action(Action::ToggleMode);
        self.truncated_row_value_views.clear();
    }

    pub(crate) fn focus_top(&mut self) {
        self.viewer.perform_action(Action::FocusTop);
    }

    pub(crate) fn focus_bottom(&mut self) {
        self.viewer.perform_action(Action::FocusBottom);
    }

    pub(crate) fn jump_up(&mut self, count: Option<usize>) {
        self.viewer.perform_action(Action::JumpUp(count));
    }

    pub(crate) fn jump_down(&mut self, count: Option<usize>) {
        self.viewer.perform_action(Action::JumpDown(count));
    }

    pub(crate) fn scroll_up(&mut self, count: usize) {
        self.viewer.perform_action(Action::ScrollUp(count));
    }

    pub(crate) fn scroll_down(&mut self, count: usize) {
        self.viewer.perform_action(Action::ScrollDown(count));
    }

    pub(crate) fn click(&mut self, row: u16) {
        self.viewer.perform_action(Action::Click(row));
    }

    pub(crate) fn set_top_visible_offset(&mut self, offset: usize) {
        if let Some(index) = self.visible_index_at(offset) {
            self.viewer.top_row = index;
        }
    }

    pub(crate) fn top_visible_offset(&self) -> usize {
        self.visible_offset_of(self.viewer.top_row).unwrap_or(0)
    }

    pub(crate) fn lines(
        &mut self,
        theme: &ResolvedTheme,
        width: usize,
        change_bars: Option<&StructuredPreviewChangeBars>,
    ) -> Vec<Line<'static>> {
        let width = width
            .saturating_sub(
                change_bars
                    .map(StructuredPreviewChangeBars::gutter_width)
                    .unwrap_or(0),
            )
            .max(1) as isize;
        let indices = self.visible_indices();
        let max_line_number_width = (self.viewer.flatjson.0.len() + 1)
            .checked_ilog10()
            .map(|digits| digits as isize + 1)
            .unwrap_or(1)
            .max(2);
        let mode = self.viewer.mode;
        let focused_row = self.viewer.focused_row;
        let flatjson = &self.viewer.flatjson;
        let cache = &mut self.truncated_row_value_views;

        indices
            .into_iter()
            .map(|index| {
                render_structured_row(
                    flatjson,
                    mode,
                    focused_row,
                    index,
                    max_line_number_width,
                    width,
                    theme,
                    cache,
                    change_bars,
                )
            })
            .collect()
    }

    fn visible_indices(&self) -> Vec<Index> {
        let mut indices = Vec::new();
        let mut row = (!self.viewer.flatjson.0.is_empty()).then_some(0usize);
        while let Some(index) = row {
            indices.push(index);
            row = match self.viewer.mode {
                Mode::Line => option_index(self.viewer.flatjson.next_visible_row(index)),
                Mode::Data => option_index(self.viewer.flatjson.next_item(index)),
            };
        }
        indices
    }

    fn visible_index_at(&self, offset: usize) -> Option<Index> {
        self.visible_indices().get(offset).copied()
    }

    fn visible_offset_of(&self, row: Index) -> Option<usize> {
        self.visible_indices()
            .iter()
            .position(|index| *index == row)
    }
}

fn option_index(index: OptionIndex) -> Option<Index> {
    match index {
        OptionIndex::Nil => None,
        OptionIndex::Index(index) => Some(index),
    }
}

fn parse_top_level_toml(text: &str) -> Result<FlatJson, String> {
    let table = toml::from_str::<toml::Table>(text).map_err(|error| error.to_string())?;
    let json =
        serde_json::to_string(&toml::Value::Table(table)).map_err(|error| error.to_string())?;
    parse_top_level_json(json)
}

#[allow(clippy::too_many_arguments)]
fn render_structured_row(
    flatjson: &FlatJson,
    mode: Mode,
    focused_row: Index,
    index: Index,
    max_line_number_width: isize,
    width: isize,
    theme: &ResolvedTheme,
    cache: &mut HashMap<usize, TruncatedStrView>,
    change_bars: Option<&StructuredPreviewChangeBars>,
) -> Line<'static> {
    let row = &flatjson[index];
    let indentation = row.depth as isize * TAB_SIZE;
    let focused = index == focused_row;
    let focused_because_matching_container_pair =
        focused_container_pair(row, flatjson, focused_row);
    let trailing_comma = trailing_comma(row, flatjson, mode);
    let mut terminal = SpanTerminal::new(theme);
    let dummy_range = 0..0;
    let mut printer = LinePrinter {
        mode,
        terminal: &mut terminal,
        flatjson,
        row,
        line_number: LineNumber {
            absolute: None,
            relative: None,
            max_width: max_line_number_width,
        },
        width,
        indentation,
        focused,
        focused_because_matching_container_pair,
        trailing_comma,
        search_matches: None,
        focused_search_match: &dummy_range,
        emphasize_focused_search_match: true,
        cached_truncated_value: Some(cache.entry(index)),
    };
    let _ = printer.print_line();
    let mut spans = Vec::new();
    push_structured_change_gutter(&mut spans, flatjson, index, change_bars);
    spans.extend(terminal.into_spans());
    Line::from(spans)
}

fn push_structured_change_gutter(
    spans: &mut Vec<Span<'static>>,
    flatjson: &FlatJson,
    index: Index,
    change_bars: Option<&StructuredPreviewChangeBars>,
) {
    let Some(change_bars) = change_bars else {
        return;
    };
    let style = structured_row_change_style(flatjson, index, change_bars);
    match style {
        Some(style) => spans.push(Span::styled(change_bars.marker.clone(), style)),
        None => spans.push(Span::raw(" ".repeat(change_bars.marker_width))),
    }
    spans.push(Span::raw(" "));
}

fn structured_row_change_style(
    flatjson: &FlatJson,
    index: Index,
    change_bars: &StructuredPreviewChangeBars,
) -> Option<Style> {
    let row = &flatjson[index];
    if row.is_closing_of_container() {
        return None;
    }
    let path = flatjson
        .build_path_to_node(PathType::DotWithTopLevelIndex, index)
        .ok()?;
    if let Some(style) = change_bars.styles.get(&path) {
        return Some(*style);
    }
    if !row.is_container() || row.is_expanded() {
        return None;
    }
    change_bars
        .styles
        .iter()
        .find(|(changed_path, _)| structured_path_is_descendant(&path, changed_path))
        .map(|(_, style)| *style)
}

fn structured_path_is_descendant(parent: &str, child: &str) -> bool {
    if parent.is_empty() {
        return !child.is_empty();
    }
    let Some(rest) = child.strip_prefix(parent) else {
        return false;
    };
    rest.starts_with('.') || rest.starts_with('[')
}

fn focused_container_pair(row: &Row, flatjson: &FlatJson, focused_row: Index) -> bool {
    if !row.is_container() {
        return false;
    }
    let pair_index = row.pair_index().unwrap();
    focused_row == pair_index || std::ptr::eq(row, &flatjson[focused_row])
}

fn trailing_comma(row: &Row, flatjson: &FlatJson, mode: Mode) -> bool {
    if mode != Mode::Line {
        return false;
    }
    let row_root = if row.is_closing_of_container() {
        &flatjson[row.pair_index().unwrap()]
    } else {
        row
    };
    row_root.parent.is_some()
        && row_root.next_sibling.is_some()
        && !(row.is_opening_of_container() && row.is_expanded())
}

struct SpanTerminal<'a> {
    spans: Vec<Span<'static>>,
    theme: &'a ResolvedTheme,
    style: JlessStyle,
}

impl<'a> SpanTerminal<'a> {
    fn new(theme: &'a ResolvedTheme) -> Self {
        Self {
            spans: Vec::new(),
            theme,
            style: JlessStyle::default(),
        }
    }

    fn into_spans(self) -> Vec<Span<'static>> {
        self.spans
    }

    fn ratatui_style(&self) -> Style {
        let mut fg = map_jless_color(self.style.fg, self.theme).unwrap_or(self.theme.text);
        let mut bg = map_jless_color(self.style.bg, self.theme);
        if self.style.dimmed {
            fg = self.theme.text_muted;
        }
        if self.style.inverted {
            let highlight = bg
                .or_else(|| map_jless_color(self.style.fg, self.theme))
                .unwrap_or(self.theme.accent);
            fg = self.theme.background.unwrap_or(Color::Black);
            bg = Some(highlight);
        }
        let mut style = Style::default().fg(fg);
        if let Some(bg) = bg {
            style = style.bg(bg);
        }
        if self.style.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    }
}

impl Write for SpanTerminal<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if !s.is_empty() {
            self.spans
                .push(Span::styled(s.to_string(), self.ratatui_style()));
        }
        Ok(())
    }
}

impl Terminal for SpanTerminal<'_> {
    fn clear_screen(&mut self) -> fmt::Result {
        Ok(())
    }

    fn clear_line(&mut self) -> fmt::Result {
        self.spans.clear();
        Ok(())
    }

    fn position_cursor(&mut self, _col: u16, _row: u16) -> fmt::Result {
        Ok(())
    }

    fn position_cursor_col(&mut self, _col: u16) -> fmt::Result {
        Ok(())
    }

    fn set_style(&mut self, style: &JlessStyle) -> fmt::Result {
        self.style = *style;
        Ok(())
    }

    fn reset_style(&mut self) -> fmt::Result {
        self.style = JlessStyle::default();
        Ok(())
    }

    fn set_fg(&mut self, color: JlessColor) -> fmt::Result {
        self.style.fg = color;
        Ok(())
    }

    fn set_bg(&mut self, color: JlessColor) -> fmt::Result {
        self.style.bg = color;
        Ok(())
    }

    fn set_inverted(&mut self, inverted: bool) -> fmt::Result {
        self.style.inverted = inverted;
        Ok(())
    }

    fn set_bold(&mut self, bold: bool) -> fmt::Result {
        self.style.bold = bold;
        Ok(())
    }

    fn set_dimmed(&mut self, dimmed: bool) -> fmt::Result {
        self.style.dimmed = dimmed;
        Ok(())
    }

    fn output(&self) -> &str {
        ""
    }

    fn clear_output(&mut self) {
        self.spans.clear();
    }
}

fn map_jless_color(color: JlessColor, theme: &ResolvedTheme) -> Option<Color> {
    match color {
        JlessColor::Default => None,
        JlessColor::C16(1) => Some(theme.error),
        JlessColor::C16(2) => Some(theme.success),
        JlessColor::C16(3) => Some(theme.warning),
        JlessColor::C16(4) | JlessColor::C16(12) => Some(theme.info),
        JlessColor::C16(5) => Some(theme.accent),
        JlessColor::C16(7) => Some(theme.text),
        JlessColor::C16(8) => Some(theme.text_muted),
        JlessColor::C16(_) => Some(theme.text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_json_tree_and_collapses_nodes() {
        let sig = StructuredPreviewSignature::new(
            StructuredPreviewKind::Json,
            "data.json",
            r#"{"a":[1,2],"b":true}"#,
        );
        let mut state = StructuredPreviewState::new(sig, r#"{"a":[1,2],"b":true}"#).unwrap();
        state.set_dimensions(80, 20);
        let expanded = flatten(&state.lines(&ResolvedTheme::default(), 80, None));
        assert!(expanded.contains("a"));
        assert!(expanded.contains("[0]: 1"));

        state.move_right();
        state.move_left();
        let collapsed = flatten(&state.lines(&ResolvedTheme::default(), 80, None));
        assert!(collapsed.contains("[…]") || collapsed.contains("[2 items]"));
    }

    #[test]
    fn renders_yaml_tree() {
        let text = "name: Oyo\nitems:\n  - one\n  - two\n";
        let sig = StructuredPreviewSignature::new(StructuredPreviewKind::Yaml, "data.yaml", text);
        let mut state = StructuredPreviewState::new(sig, text).unwrap();
        state.set_dimensions(80, 20);
        let rendered = flatten(&state.lines(&ResolvedTheme::default(), 80, None));
        assert!(rendered.contains("name"));
        assert!(rendered.contains("Oyo"));
    }

    #[test]
    fn renders_toml_tree() {
        let text = "name = 'Oyo'\n[server]\nport = 8080\n";
        let sig = StructuredPreviewSignature::new(StructuredPreviewKind::Toml, "data.toml", text);
        let mut state = StructuredPreviewState::new(sig, text).unwrap();
        state.set_dimensions(80, 20);
        let rendered = flatten(&state.lines(&ResolvedTheme::default(), 80, None));
        assert!(rendered.contains("name"));
        assert!(rendered.contains("Oyo"));
        assert!(rendered.contains("server"));
        assert!(rendered.contains("port"));
    }

    #[test]
    fn structured_change_bars_render_before_matching_rows() {
        let text = r#"{"a":1,"b":2}"#;
        let sig = StructuredPreviewSignature::new(StructuredPreviewKind::Json, "data.json", text);
        let mut state = StructuredPreviewState::new(sig, text).unwrap();
        let bars = StructuredPreviewChangeBars {
            marker: "|".to_string(),
            marker_width: 1,
            styles: std::collections::HashMap::from([(".b".to_string(), Style::default())]),
        };
        let rendered = flatten(&state.lines(&ResolvedTheme::default(), 80, Some(&bars)));
        assert!(
            rendered
                .lines()
                .any(|line| line.starts_with("| ") && line.contains("b")),
            "change bar: {rendered:?}"
        );
    }

    #[test]
    fn treats_cargo_lock_as_toml() {
        assert_eq!(
            StructuredPreviewKind::from_file_name("Cargo.lock"),
            Some(StructuredPreviewKind::Toml)
        );
        assert_eq!(
            StructuredPreviewKind::from_file_name("nested/Cargo.lock"),
            Some(StructuredPreviewKind::Toml)
        );
    }
}
