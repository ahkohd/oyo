//! Git range picker dashboard for oy view

use crate::config::ResolvedTheme;
use crate::keybindings::{DashboardAction, Keybindings};
use crate::time_format::TimeFormatter;
use oyo_core::git::CommitEntry;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

const FOOTER_HEIGHT: u16 = 3;
const HEADER_HEIGHT: u16 = 4;
const MAX_CONTENT_WIDTH: u16 = 100;
const LAYOUT_PADDING_Y: u16 = 1;
const HEAD_REF: &str = "HEAD";
const INDEX_REF: &str = "INDEX";
const EMPTY_TREE_HASH: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardSelection {
    Uncommitted,
    Staged,
    Range { from: String, to: String },
}

#[derive(Debug, Clone)]
enum EntryKind {
    WorkingTree { files: usize },
    Staged { files: usize },
    Commit(CommitEntry),
}

#[derive(Debug, Clone, Copy)]
enum DisplayRow {
    Entry { idx: usize, detail: bool },
}

#[derive(Debug, Clone)]
struct RangeMarker {
    symbol: String,
    style: Style,
}

#[derive(Debug, Clone)]
struct DashboardEntry {
    kind: EntryKind,
}

#[derive(Debug, Clone, Copy)]
struct DashboardFooterHit {
    action: DashboardAction,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

#[derive(Debug)]
pub struct Dashboard {
    repo_root: PathBuf,
    branch: Option<String>,
    head_meta: Option<HeadMeta>,
    entries: Vec<DashboardEntry>,
    filtered: Vec<usize>,
    selected: usize,
    scroll: usize,
    filter: String,
    filter_active: bool,
    filter_cursor_visible: bool,
    filter_cursor_last_blink: Instant,
    filter_area: Option<(u16, u16, u16, u16)>,
    filter_hover: bool,
    filter_clear_hit: Option<(u16, u16, u16, u16)>,
    filter_clear_hover: bool,
    footer_action_hits: Vec<DashboardFooterHit>,
    footer_action_hover: Option<DashboardAction>,
    hovered: Option<usize>,
    pinned_from: Option<String>,
    theme: ResolvedTheme,
    primary_marker: String,
    extent_marker: String,
    last_list_area: Rect,
    time_format: TimeFormatter,
    keybindings: Keybindings,
}

#[derive(Debug)]
pub struct DashboardConfig {
    pub repo_root: PathBuf,
    pub branch: Option<String>,
    pub commits: Vec<CommitEntry>,
    pub working_files: usize,
    pub staged_files: usize,
    pub theme: ResolvedTheme,
    pub primary_marker: String,
    pub extent_marker: String,
    pub time_format: TimeFormatter,
    pub keybindings: Keybindings,
}

struct RenderLineContext<'a> {
    width: usize,
    stats_width: usize,
    detail: bool,
    hovered: bool,
    range_marker: Option<RangeMarker>,
    marker_width: usize,
    theme: &'a ResolvedTheme,
    head_meta: Option<&'a HeadMeta>,
    time_format: &'a TimeFormatter,
    now: i64,
}

#[derive(Debug, Clone)]
struct HeadMeta {
    author: String,
    author_time: Option<i64>,
}

impl Dashboard {
    pub fn new(config: DashboardConfig) -> Self {
        let mut entries = Vec::new();
        let head_meta = config.commits.first().map(|commit| HeadMeta {
            author: commit.author.clone(),
            author_time: commit.author_time,
        });
        entries.push(DashboardEntry {
            kind: EntryKind::WorkingTree {
                files: config.working_files,
            },
        });
        entries.push(DashboardEntry {
            kind: EntryKind::Staged {
                files: config.staged_files,
            },
        });
        for commit in config.commits {
            entries.push(DashboardEntry {
                kind: EntryKind::Commit(commit),
            });
        }
        let filtered = (0..entries.len()).collect();
        Self {
            repo_root: config.repo_root,
            branch: config.branch,
            head_meta,
            entries,
            filtered,
            selected: 0,
            scroll: 0,
            filter: String::new(),
            filter_active: false,
            filter_cursor_visible: true,
            filter_cursor_last_blink: Instant::now(),
            filter_area: None,
            filter_hover: false,
            filter_clear_hit: None,
            filter_clear_hover: false,
            footer_action_hits: Vec::new(),
            footer_action_hover: None,
            hovered: None,
            pinned_from: None,
            theme: config.theme,
            primary_marker: config.primary_marker,
            extent_marker: config.extent_marker,
            last_list_area: Rect::default(),
            time_format: config.time_format,
            keybindings: config.keybindings,
        }
    }

    pub fn filter_active(&self) -> bool {
        self.filter_active
    }

    pub(crate) fn keybindings_mut(&mut self) -> &mut Keybindings {
        &mut self.keybindings
    }

    pub fn start_filter(&mut self) {
        self.filter.clear();
        self.focus_filter();
        self.refresh_filter();
    }

    pub fn focus_filter(&mut self) {
        self.filter_active = true;
        self.reset_filter_cursor();
    }

    pub fn stop_filter(&mut self) {
        self.filter_active = false;
        self.filter_cursor_visible = true;
    }

    pub fn push_filter_char(&mut self, ch: char) {
        self.filter.push(ch);
        self.reset_filter_cursor();
        self.refresh_filter();
    }

    pub fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.reset_filter_cursor();
        self.refresh_filter();
    }

    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.reset_filter_cursor();
        self.refresh_filter();
    }

    pub fn tick(&mut self) -> bool {
        if !self.filter_active {
            return false;
        }
        let now = Instant::now();
        if now.duration_since(self.filter_cursor_last_blink) < Duration::from_millis(500) {
            return false;
        }
        self.filter_cursor_visible = !self.filter_cursor_visible;
        self.filter_cursor_last_blink = now;
        true
    }

    fn reset_filter_cursor(&mut self) {
        self.filter_cursor_visible = true;
        self.filter_cursor_last_blink = Instant::now();
    }

    pub fn select_selection(&mut self, selection: &DashboardSelection) {
        let selected = match selection {
            DashboardSelection::Uncommitted => {
                self.entry_position(|entry| matches!(entry.kind, EntryKind::WorkingTree { .. }))
            }
            DashboardSelection::Staged => {
                self.entry_position(|entry| matches!(entry.kind, EntryKind::Staged { .. }))
            }
            DashboardSelection::Range { from: _, to } if to == HEAD_REF => {
                self.entry_position(|entry| matches!(entry.kind, EntryKind::WorkingTree { .. }))
            }
            DashboardSelection::Range { from: _, to } if to == INDEX_REF => {
                self.entry_position(|entry| matches!(entry.kind, EntryKind::Staged { .. }))
            }
            DashboardSelection::Range { from: _, to } => {
                self.entry_position(|entry| match &entry.kind {
                    EntryKind::Commit(commit) => commit.id == *to || commit.short_id == *to,
                    _ => false,
                })
            }
        };
        let Some(selected) = selected else {
            return;
        };
        self.selected = selected;
        self.scroll = 0;
        self.pinned_from = match selection {
            DashboardSelection::Range { from, .. } => Some(from.clone()),
            _ => None,
        };
    }

    fn entry_position(&self, matches: impl Fn(&DashboardEntry) -> bool) -> Option<usize> {
        self.filtered
            .iter()
            .position(|entry_idx| self.entries.get(*entry_idx).is_some_and(&matches))
    }

    pub fn move_selection(&mut self, delta: isize, view_height: usize) {
        if self.filtered.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        let len = self.filtered.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1) as usize;
        self.selected = next;
        self.ensure_visible(view_height);
    }

    pub fn select_first(&mut self, view_height: usize) {
        self.selected = 0;
        self.scroll = 0;
        self.ensure_visible(view_height);
    }

    pub fn select_last(&mut self, view_height: usize) {
        if !self.filtered.is_empty() {
            self.selected = self.filtered.len().saturating_sub(1);
            self.ensure_visible(view_height);
        }
    }

    pub fn page_up(&mut self, view_height: usize) {
        let delta = view_height.saturating_sub(1) as isize;
        self.move_selection(-delta, view_height);
    }

    pub fn page_down(&mut self, view_height: usize) {
        let delta = view_height.saturating_sub(1) as isize;
        self.move_selection(delta, view_height);
    }

    pub fn toggle_pin(&mut self) {
        self.toggle_pin_for_idx(self.selected);
    }

    pub fn toggle_hovered_pin(&mut self) {
        if let Some(hovered) = self.hovered {
            self.toggle_pin_for_idx(hovered);
        } else {
            self.toggle_pin();
        }
    }

    fn toggle_pin_for_idx(&mut self, filtered_idx: usize) {
        let Some(entry_idx) = self.filtered.get(filtered_idx) else {
            return;
        };
        let Some(pin_id) = self.entries.get(*entry_idx).map(|entry| match &entry.kind {
            EntryKind::Commit(commit) => commit.id.clone(),
            EntryKind::WorkingTree { .. } => HEAD_REF.to_string(),
            EntryKind::Staged { .. } => INDEX_REF.to_string(),
        }) else {
            return;
        };
        if self.pinned_from.as_deref() == Some(pin_id.as_str()) {
            self.pinned_from = None;
        } else {
            self.pinned_from = Some(pin_id);
        }
    }

    pub fn clear_pin(&mut self) {
        self.pinned_from = None;
    }

    pub fn selection(&self) -> Option<DashboardSelection> {
        let entry = self.current_entry()?;
        match &entry.kind {
            EntryKind::WorkingTree { files } => {
                if let Some(from) = self.pinned_from.clone() {
                    if from == INDEX_REF {
                        return Some(DashboardSelection::Range {
                            from,
                            to: HEAD_REF.to_string(),
                        });
                    }
                    if from != HEAD_REF {
                        return Some(DashboardSelection::Range {
                            from,
                            to: HEAD_REF.to_string(),
                        });
                    }
                }
                if *files == 0 {
                    None
                } else {
                    Some(DashboardSelection::Uncommitted)
                }
            }
            EntryKind::Staged { files } => {
                if let Some(from) = self.pinned_from.clone() {
                    if from == HEAD_REF {
                        return Some(DashboardSelection::Range {
                            from,
                            to: INDEX_REF.to_string(),
                        });
                    }
                    if from == INDEX_REF {
                        if *files == 0 {
                            return None;
                        }
                        return Some(DashboardSelection::Staged);
                    }
                    if from != HEAD_REF {
                        return Some(DashboardSelection::Range {
                            from,
                            to: INDEX_REF.to_string(),
                        });
                    }
                }
                if *files == 0 {
                    None
                } else {
                    Some(DashboardSelection::Staged)
                }
            }
            EntryKind::Commit(commit) => {
                let to = commit.id.clone();
                let from = self
                    .pinned_from
                    .clone()
                    .or_else(|| commit.parents.first().cloned())
                    .unwrap_or_else(|| EMPTY_TREE_HASH.to_string());
                Some(DashboardSelection::Range { from, to })
            }
        }
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if let Some(bg) = self.theme.background {
            let block = Block::default().style(Style::default().bg(bg));
            frame.render_widget(block, area);
        }
        let content = inset_rect(centered_width(area, MAX_CONTENT_WIDTH), 0, LAYOUT_PADDING_Y);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(HEADER_HEIGHT),
                Constraint::Min(0),
                Constraint::Length(FOOTER_HEIGHT),
            ])
            .split(content);

        self.ensure_visible(chunks[1].height as usize);
        self.draw_header(frame, chunks[0]);
        self.draw_list(frame, chunks[1]);
        self.draw_footer(frame, chunks[2]);
    }

    pub fn list_height(&self, height: u16) -> usize {
        if self.last_list_area.height > 0 {
            return self.last_list_area.height as usize;
        }
        height.saturating_sub(HEADER_HEIGHT + FOOTER_HEIGHT) as usize
    }

    fn current_entry(&self) -> Option<&DashboardEntry> {
        let idx = self.filtered.get(self.selected)?;
        self.entries.get(*idx)
    }

    fn display_rows(&self) -> Vec<DisplayRow> {
        let mut rows = Vec::new();
        for (pos, entry_idx) in self.filtered.iter().enumerate() {
            rows.push(DisplayRow::Entry {
                idx: pos,
                detail: false,
            });
            if matches!(
                self.entries[*entry_idx].kind,
                EntryKind::Commit(_) | EntryKind::WorkingTree { .. } | EntryKind::Staged { .. }
            ) {
                rows.push(DisplayRow::Entry {
                    idx: pos,
                    detail: true,
                });
            }
        }
        rows
    }

    fn stats_column_width(&self) -> usize {
        let mut max_width = 0usize;
        for entry_idx in &self.filtered {
            let EntryKind::Commit(commit) = &self.entries[*entry_idx].kind else {
                continue;
            };
            let Some(stats) = commit.stats else {
                continue;
            };
            let text = format_diff_stats(stats.insertions, stats.deletions);
            max_width = max_width.max(text_width(&text));
        }
        max_width
    }

    fn refresh_filter(&mut self) {
        let query = self.filter.trim().to_ascii_lowercase();
        if query.is_empty() {
            self.filtered = (0..self.entries.len()).collect();
        } else {
            self.filtered = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(idx, entry)| {
                    if entry.matches(&query) {
                        Some(idx)
                    } else {
                        None
                    }
                })
                .collect();
        }
        if self.selected >= self.filtered.len() {
            self.selected = 0;
            self.scroll = 0;
        }
        self.hovered = None;
    }

    fn ensure_visible(&mut self, view_height: usize) {
        if view_height == 0 || self.filtered.is_empty() {
            self.scroll = 0;
            return;
        }
        let rows = self.display_rows();
        let Some(display_idx) = rows.iter().position(|row| {
            matches!(
                row,
                DisplayRow::Entry {
                    idx,
                    detail: false
                } if *idx == self.selected
            )
        }) else {
            self.scroll = 0;
            return;
        };
        if display_idx < self.scroll {
            self.scroll = display_idx;
        } else if display_idx >= self.scroll + view_height {
            self.scroll = display_idx.saturating_sub(view_height - 1);
        }
        if self.scroll >= rows.len() {
            self.scroll = rows.len().saturating_sub(1);
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let repo_name = self
            .repo_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(".");
        let branch = self
            .branch
            .clone()
            .unwrap_or_else(|| "DETACHED".to_string());
        let mut lines = vec![
            Line::from(vec![Span::styled(
                format!("{repo_name}@{branch}"),
                Style::default().fg(self.theme.text_muted),
            )]),
            Line::raw(""),
        ];
        if let Some(ref from) = self.pinned_from {
            let short_from = if from == HEAD_REF {
                HEAD_REF.to_string()
            } else if from == INDEX_REF {
                "STAGED".to_string()
            } else {
                shorten_hash(from)
            };
            let to_label = match self.current_entry().map(|entry| &entry.kind) {
                Some(EntryKind::Commit(commit)) => shorten_hash(&commit.id),
                Some(EntryKind::WorkingTree { .. }) | Some(EntryKind::Staged { .. }) => {
                    if matches!(
                        self.current_entry().map(|entry| &entry.kind),
                        Some(EntryKind::Staged { .. })
                    ) {
                        "STAGED".to_string()
                    } else {
                        HEAD_REF.to_string()
                    }
                }
                None => "select target".to_string(),
            };
            let range = if to_label == short_from {
                format!("From: {short_from} • select target")
            } else {
                format!("From: {short_from} to {to_label}")
            };
            lines.push(Line::from(Span::styled(
                truncate_text(&range, area.width.saturating_sub(2) as usize),
                Style::default()
                    .fg(self.theme.text_muted)
                    .add_modifier(Modifier::DIM),
            )));
        } else {
            let hint = format!(
                "{} to mark range start",
                self.keybindings.dashboard_keys(DashboardAction::TogglePin)
            );
            lines.push(Line::from(Span::styled(
                hint,
                Style::default()
                    .fg(self.theme.text_muted)
                    .add_modifier(Modifier::DIM),
            )));
        }
        lines.push(Line::raw(""));

        let mut header = Paragraph::new(lines).alignment(Alignment::Left);
        if let Some(bg) = self.theme.background {
            header = header.style(Style::default().bg(bg));
        }
        frame.render_widget(header, area);
    }

    fn draw_footer(&mut self, frame: &mut Frame, area: Rect) {
        let mut block = Block::default();
        if let Some(bg) = self.theme.background {
            block = block.style(Style::default().bg(bg));
        }
        frame.render_widget(block, area);
        self.footer_action_hits.clear();
        if area.height < 2 {
            self.filter_area = None;
            self.filter_clear_hit = None;
            return;
        }

        let filter_area = Rect {
            x: area.x,
            y: area.y.saturating_add(1),
            width: area.width,
            height: 1,
        };
        self.filter_area = Some((
            filter_area.x,
            filter_area.y,
            filter_area.width,
            filter_area.height,
        ));
        let mut filter = Paragraph::new(self.filter_line(filter_area.width));
        if let Some(bg) = self
            .theme
            .background_element
            .or(self.theme.background_panel)
            .or(self.theme.background)
        {
            filter = filter.style(Style::default().bg(bg));
        }
        frame.render_widget(filter, filter_area);
        if !self.filter.is_empty() && filter_area.width > 2 {
            let clear_x = filter_area
                .x
                .saturating_add(filter_area.width.saturating_sub(2));
            let clear_y = filter_area.y;
            self.filter_clear_hit = Some((clear_x, clear_y, 1, 1));
            let clear_style = if self.filter_clear_hover {
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.text_muted)
            };
            if let Some(cell) = frame.buffer_mut().cell_mut((clear_x, clear_y)) {
                cell.set_symbol("×").set_style(clear_style);
            }
        } else {
            self.filter_clear_hit = None;
            self.filter_clear_hover = false;
        }

        if area.height < 3 {
            return;
        }
        let hint_area = Rect {
            x: area.x,
            y: area.y.saturating_add(2),
            width: area.width,
            height: 1,
        };
        let mut spans = vec![Span::raw(" ")];
        let mut col = hint_area.x.saturating_add(1);
        let key_style = Style::default()
            .fg(self.theme.accent)
            .add_modifier(Modifier::BOLD);
        let label_style = Style::default()
            .fg(self.theme.text_muted)
            .add_modifier(Modifier::DIM);
        let hover_label_style = Style::default()
            .fg(self.theme.accent)
            .add_modifier(Modifier::BOLD);
        let actions = [
            (DashboardAction::Accept, "open"),
            (DashboardAction::TogglePin, "start"),
            (DashboardAction::SelectHovered, "end"),
            (DashboardAction::Quit, "quit"),
        ];
        for (idx, (action, label)) in actions.into_iter().enumerate() {
            if idx > 0 {
                spans.push(Span::raw("  "));
                col = col.saturating_add(2);
            }
            let key = self.dashboard_footer_key(action);
            let width = text_width(&format!("{key} {label}")) as u16;
            self.footer_action_hits.push(DashboardFooterHit {
                action,
                x: col,
                y: hint_area.y,
                width,
                height: 1,
            });
            let is_hover = self.footer_action_hover == Some(action);
            spans.push(Span::styled(key, key_style));
            spans.push(Span::styled(
                format!(" {label}"),
                if is_hover {
                    hover_label_style
                } else {
                    label_style
                },
            ));
            col = col.saturating_add(width);
        }
        spans.push(Span::raw(" "));
        frame.render_widget(Paragraph::new(Line::from(spans)), hint_area);
    }

    fn dashboard_footer_key(&self, action: DashboardAction) -> String {
        let keys = self.keybindings.dashboard_keys(action);
        if action == DashboardAction::Quit && keys.split(" / ").any(|key| key == "q") {
            "q".to_string()
        } else {
            keys
        }
    }

    fn filter_line(&self, width: u16) -> Line<'static> {
        if self.filter_active {
            let prompt_style = Style::default()
                .fg(self.theme.primary)
                .add_modifier(Modifier::BOLD);
            let query_width = if self.filter.is_empty() {
                width.saturating_sub(5)
            } else {
                width.saturating_sub(7)
            };
            return Line::from(vec![
                Span::raw(" "),
                Span::styled("❯ ", prompt_style),
                Span::styled(
                    truncate_text_from_start(&self.filter, query_width as usize),
                    Style::default().fg(self.theme.text),
                ),
                Span::styled(
                    if self.filter_cursor_visible {
                        "│"
                    } else {
                        " "
                    },
                    prompt_style,
                ),
            ]);
        }

        if !self.filter.is_empty() {
            let prompt_style = Style::default()
                .fg(self.theme.primary)
                .add_modifier(Modifier::BOLD);
            let query_style = if self.filter_hover {
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.text)
            };
            return Line::from(vec![
                Span::raw(" "),
                Span::styled("❯ ", prompt_style),
                Span::styled(
                    truncate_text_from_start(&self.filter, width.saturating_sub(6) as usize),
                    query_style,
                ),
            ]);
        }

        let style = if self.filter_hover {
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.text_muted)
        };
        let text = format!(
            "{} Filter",
            self.keybindings
                .dashboard_keys(DashboardAction::StartFilter)
        );
        Line::from(vec![Span::raw(" "), Span::styled(text, style)])
    }

    fn draw_list(&mut self, frame: &mut Frame, area: Rect) {
        self.last_list_area = area;
        let height = area.height as usize;
        self.ensure_visible(height);

        let mut lines = Vec::new();
        let view_width = area.width.saturating_sub(1) as usize;
        let marker_width = marker_width(&self.primary_marker, &self.extent_marker);
        let content_width = view_width.saturating_sub(marker_width + 1);
        let stats_width = self.stats_column_width();

        if self.filtered.is_empty() {
            let mut lines = vec![Line::raw(""); height];
            if height > 0 {
                let text = if self.filter.is_empty() {
                    "No commits"
                } else {
                    "No Filter Results"
                };
                let msg = truncate_text(text, content_width);
                lines[height / 2] = Line::from(Span::styled(
                    msg,
                    Style::default()
                        .fg(self.theme.text_muted)
                        .add_modifier(Modifier::DIM),
                ));
            }
            let mut list = Paragraph::new(lines).alignment(Alignment::Center);
            let mut block = Block::default();
            if let Some(bg) = self.theme.background {
                block = block.style(Style::default().bg(bg));
            }
            list = list.block(block);
            frame.render_widget(list, area);
            return;
        }

        let rows = self.display_rows();
        let pinned_filtered_idx = self.pinned_from.as_ref().and_then(|pinned_id| {
            if pinned_id == HEAD_REF {
                return self.filtered.iter().position(|entry_idx| {
                    matches!(self.entries[*entry_idx].kind, EntryKind::WorkingTree { .. })
                });
            }
            if pinned_id == INDEX_REF {
                return self.filtered.iter().position(|entry_idx| {
                    matches!(self.entries[*entry_idx].kind, EntryKind::Staged { .. })
                });
            }
            self.filtered
                .iter()
                .position(|entry_idx| match &self.entries[*entry_idx].kind {
                    EntryKind::Commit(commit) => commit.id == *pinned_id,
                    _ => false,
                })
        });
        let pinned_display_idx = pinned_filtered_idx.and_then(|idx| {
            rows.iter().position(|row| {
                matches!(
                    row,
                    DisplayRow::Entry {
                        idx: row_idx,
                        detail: false
                    } if *row_idx == idx
                )
            })
        });
        let selected_display_idx = rows.iter().position(|row| {
            matches!(
                row,
                DisplayRow::Entry {
                    idx,
                    detail: false
                } if *idx == self.selected
            )
        });

        let start = self.scroll.min(rows.len());
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let end = (start + height).min(rows.len());
        for (row_idx, row) in rows.iter().enumerate().take(end).skip(start) {
            let DisplayRow::Entry {
                idx: filtered_idx,
                detail,
            } = *row;
            let entry_idx = self.filtered[filtered_idx];
            let entry = &self.entries[entry_idx];
            let hovered = self.hovered == Some(filtered_idx);
            let range_marker = range_marker_for_row(
                row_idx,
                selected_display_idx,
                pinned_display_idx,
                detail,
                &self.theme,
                &self.primary_marker,
                &self.extent_marker,
            );
            let line = entry.render_line(RenderLineContext {
                width: content_width,
                stats_width,
                detail,
                hovered,
                range_marker,
                marker_width,
                theme: &self.theme,
                head_meta: self.head_meta.as_ref(),
                time_format: &self.time_format,
                now,
            });
            lines.push(line);
        }

        while lines.len() < height {
            lines.push(Line::raw(""));
        }

        let mut list = Paragraph::new(lines);
        let mut block = Block::default();
        if let Some(bg) = self.theme.background {
            block = block.style(Style::default().bg(bg));
        }
        list = list.block(block);
        frame.render_widget(list, area);
    }

    pub fn handle_filter_mouse_down(&mut self, x: u16, y: u16) -> bool {
        if self.hit(self.filter_clear_hit, x, y) {
            self.clear_filter();
            self.focus_filter();
            return true;
        }
        if self.hit(self.filter_area, x, y) {
            self.focus_filter();
            return true;
        }
        false
    }

    pub fn update_hover(&mut self, x: u16, y: u16) -> bool {
        let filter_hover = self.hit(self.filter_area, x, y);
        let filter_clear_hover = self.hit(self.filter_clear_hit, x, y);
        let footer_action_hover = self.footer_action_at(x, y);
        let hovered = self.hovered_at(x, y);
        if self.filter_hover == filter_hover
            && self.filter_clear_hover == filter_clear_hover
            && self.footer_action_hover == footer_action_hover
            && self.hovered == hovered
        {
            return false;
        }
        self.filter_hover = filter_hover;
        self.filter_clear_hover = filter_clear_hover;
        self.footer_action_hover = footer_action_hover;
        self.hovered = hovered;
        true
    }

    pub fn footer_action_at(&self, x: u16, y: u16) -> Option<DashboardAction> {
        self.footer_action_hits
            .iter()
            .find(|hit| self.hit(Some((hit.x, hit.y, hit.width, hit.height)), x, y))
            .map(|hit| hit.action)
    }

    pub fn select_at_mouse(&mut self, x: u16, y: u16) -> Option<bool> {
        let idx = self.hovered_at(x, y)?;
        let changed = self.selected != idx;
        self.selected = idx;
        self.ensure_visible(self.last_list_area.height as usize);
        Some(changed)
    }

    pub fn select_hovered(&mut self) -> bool {
        let Some(hovered) = self.hovered else {
            return false;
        };
        let changed = self.selected != hovered;
        self.selected = hovered;
        self.ensure_visible(self.last_list_area.height as usize);
        changed
    }

    pub fn toggle_pin_at_mouse(&mut self, x: u16, y: u16) -> bool {
        let Some(idx) = self.hovered_at(x, y) else {
            return false;
        };
        self.toggle_pin_for_idx(idx);
        true
    }

    fn hovered_at(&self, x: u16, y: u16) -> Option<usize> {
        let area = self.last_list_area;
        if area.height == 0
            || x < area.x
            || x >= area.x.saturating_add(area.width)
            || y < area.y
            || y >= area.y.saturating_add(area.height)
        {
            return None;
        }
        let rows = self.display_rows();
        let row_idx = (y - area.y) as usize + self.scroll;
        let DisplayRow::Entry { idx, .. } = *rows.get(row_idx)?;
        Some(idx)
    }

    fn hit(&self, hit: Option<(u16, u16, u16, u16)>, x: u16, y: u16) -> bool {
        hit.is_some_and(|(hit_x, hit_y, width, height)| {
            x >= hit_x
                && x < hit_x.saturating_add(width)
                && y >= hit_y
                && y < hit_y.saturating_add(height)
        })
    }
}

impl DashboardEntry {
    fn matches(&self, query: &str) -> bool {
        match &self.kind {
            EntryKind::WorkingTree { .. } | EntryKind::Staged { .. } => false,
            EntryKind::Commit(commit) => {
                let haystack = format!(
                    "{} {} {} {}",
                    commit.id, commit.short_id, commit.author, commit.summary
                )
                .to_ascii_lowercase();
                haystack.contains(query)
            }
        }
    }

    fn render_line(&self, ctx: RenderLineContext<'_>) -> Line<'static> {
        let mut spans = Vec::new();

        spans.extend(range_marker_spans(
            ctx.range_marker.clone(),
            ctx.marker_width,
        ));

        match &self.kind {
            EntryKind::WorkingTree { files } => {
                if ctx.detail {
                    let meta = ctx
                        .head_meta
                        .map(|meta| {
                            let date = ctx.time_format.format(meta.author_time, ctx.now);
                            format!("{} {}", meta.author, date)
                        })
                        .unwrap_or_else(|| "Working tree changes".to_string());
                    spans.push(Span::styled(
                        "  ",
                        Style::default().fg(ctx.theme.text_muted),
                    ));
                    spans.push(Span::styled(
                        truncate_text(&meta, ctx.width.saturating_sub(2)),
                        Style::default()
                            .fg(ctx.theme.text_muted)
                            .add_modifier(Modifier::DIM),
                    ));
                    return line_with_hover(spans, &ctx);
                }
                let label = if *files == 0 {
                    "Working tree (clean)".to_string()
                } else {
                    format!("Working tree ({files} files)")
                };
                let style = if *files == 0 {
                    Style::default().fg(ctx.theme.text_muted)
                } else {
                    Style::default().fg(ctx.theme.accent)
                };
                spans.push(Span::styled(truncate_text(&label, ctx.width), style));
            }
            EntryKind::Staged { files } => {
                if ctx.detail {
                    let meta = ctx
                        .head_meta
                        .map(|meta| {
                            let date = ctx.time_format.format(meta.author_time, ctx.now);
                            format!("{} {}", meta.author, date)
                        })
                        .unwrap_or_else(|| "Staged changes".to_string());
                    spans.push(Span::styled(
                        "  ",
                        Style::default().fg(ctx.theme.text_muted),
                    ));
                    spans.push(Span::styled(
                        truncate_text(&meta, ctx.width.saturating_sub(2)),
                        Style::default()
                            .fg(ctx.theme.text_muted)
                            .add_modifier(Modifier::DIM),
                    ));
                    return line_with_hover(spans, &ctx);
                }
                let label = if *files == 0 {
                    "Staged (clean)".to_string()
                } else {
                    format!("Staged ({files} files)")
                };
                let style = if *files == 0 {
                    Style::default().fg(ctx.theme.text_muted)
                } else {
                    Style::default().fg(ctx.theme.primary)
                };
                spans.push(Span::styled(truncate_text(&label, ctx.width), style));
            }
            EntryKind::Commit(commit) => {
                if ctx.detail {
                    let date = ctx.time_format.format(commit.author_time, ctx.now);
                    let meta = format!("{} {}", commit.author, date);
                    spans.push(Span::styled(
                        "  ",
                        Style::default().fg(ctx.theme.text_muted),
                    ));
                    spans.push(Span::styled(
                        truncate_text(&meta, ctx.width.saturating_sub(2)),
                        Style::default()
                            .fg(ctx.theme.text_muted)
                            .add_modifier(Modifier::DIM),
                    ));
                } else {
                    let mut right_text = String::new();
                    if let Some(stats) = commit.stats {
                        right_text = format_diff_stats(stats.insertions, stats.deletions);
                    }
                    let right_width = if ctx.stats_width == 0 {
                        0
                    } else {
                        ctx.stats_width.saturating_add(1)
                    };
                    let left_max = ctx.width.saturating_sub(right_width);

                    let short_width = text_width(&commit.short_id);
                    let mut summary_width = left_max.saturating_sub(short_width + 1);
                    if summary_width < 8 {
                        summary_width = left_max.saturating_sub(short_width + 1);
                    }
                    summary_width = summary_width.saturating_sub(4);

                    let summary = truncate_text(&commit.summary, summary_width);
                    let short_id = truncate_text(&commit.short_id, short_width);
                    spans.push(Span::styled(short_id, Style::default().fg(ctx.theme.info)));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(summary, Style::default().fg(ctx.theme.text)));

                    if right_width > 0 {
                        let content_used = spans_width(&spans).saturating_sub(ctx.marker_width + 1);
                        let pad = left_max.saturating_sub(content_used);
                        spans.push(Span::raw(" ".repeat(pad)));
                        let right_text = pad_to_width(&right_text, ctx.stats_width);
                        spans.push(Span::styled(
                            right_text,
                            Style::default().fg(ctx.theme.text_muted),
                        ));
                    }
                }
            }
        }

        line_with_hover(spans, &ctx)
    }
}

fn line_with_hover(mut spans: Vec<Span<'static>>, ctx: &RenderLineContext<'_>) -> Line<'static> {
    if ctx.hovered {
        let bg = ctx.theme.background_element.or(ctx.theme.background_panel);
        for span in spans.iter_mut().skip(2) {
            span.style = span
                .style
                .fg(if ctx.detail {
                    ctx.theme.text
                } else {
                    ctx.theme.accent
                })
                .add_modifier(Modifier::BOLD);
            if let Some(bg) = bg {
                span.style = span.style.bg(bg);
            }
        }
    }
    Line::from(spans)
}

fn truncate_text(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    if text_width(text) <= max_width {
        return text.to_string();
    }
    let suffix_len = max_width.saturating_sub(3);
    let mut acc = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthStr::width(ch.to_string().as_str());
        if width + ch_width > suffix_len {
            break;
        }
        acc.push(ch);
        width += ch_width;
    }
    format!("{acc}…")
}

fn truncate_text_from_start(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text_width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    let suffix_width = max_width.saturating_sub(3);
    let mut acc = String::new();
    let mut width = 0usize;
    for ch in text.chars().rev() {
        let ch_width = UnicodeWidthStr::width(ch.to_string().as_str());
        if width + ch_width > suffix_width {
            break;
        }
        acc.push(ch);
        width += ch_width;
    }
    let suffix: String = acc.chars().rev().collect();
    format!("...{suffix}")
}

fn shorten_hash(hash: &str) -> String {
    hash.chars().take(8).collect()
}

fn pad_to_width(text: &str, width: usize) -> String {
    let text_width = text_width(text);
    if text_width >= width {
        return text.to_string();
    }
    let mut padded = String::with_capacity(width);
    padded.push_str(text);
    padded.push_str(&" ".repeat(width - text_width));
    padded
}

fn text_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

#[allow(clippy::manual_is_multiple_of)]
fn format_number(value: usize) -> String {
    let s = value.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    for (idx, ch) in chars.iter().enumerate() {
        out.push(*ch);
        let remaining = len - idx - 1;
        if remaining > 0 && remaining % 3 == 0 {
            out.push(',');
        }
    }
    out
}

fn format_diff_stats(insertions: usize, deletions: usize) -> String {
    format!(
        "+{} -{}",
        format_number(insertions),
        format_number(deletions)
    )
}

fn centered_width(area: Rect, max_width: u16) -> Rect {
    let width = area.width.min(max_width);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    Rect {
        x,
        y: area.y,
        width,
        height: area.height,
    }
}

fn inset_rect(area: Rect, pad_x: u16, pad_y: u16) -> Rect {
    let width = area.width.saturating_sub(pad_x * 2);
    let height = area.height.saturating_sub(pad_y * 2);
    Rect {
        x: area.x + pad_x,
        y: area.y + pad_y,
        width,
        height,
    }
}

fn range_marker_spans(marker: Option<RangeMarker>, marker_width: usize) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(marker) = marker {
        let text = pad_to_width(&marker.symbol, marker_width);
        spans.push(Span::styled(text, marker.style));
    } else {
        spans.push(Span::raw(" ".repeat(marker_width.max(1))));
    }
    spans.push(Span::raw(" "));
    spans
}

fn range_marker_for_row(
    row_idx: usize,
    selected_idx: Option<usize>,
    pinned_idx: Option<usize>,
    detail: bool,
    theme: &ResolvedTheme,
    primary_marker: &str,
    extent_marker: &str,
) -> Option<RangeMarker> {
    let extent_style = Style::default()
        .fg(theme.diff_ext_marker)
        .add_modifier(Modifier::DIM);
    let extent_symbol = extent_marker.to_string();
    let range_active =
        matches!((pinned_idx, selected_idx), (Some(pinned), Some(selected)) if pinned != selected);
    let range_down =
        matches!((pinned_idx, selected_idx), (Some(pinned), Some(selected)) if pinned < selected);
    if let Some(selected_idx) = selected_idx {
        if row_idx == selected_idx && !detail {
            let is_pinned = pinned_idx == Some(selected_idx);
            return Some(RangeMarker {
                symbol: primary_marker.to_string(),
                style: if is_pinned {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(theme.primary)
                        .add_modifier(Modifier::BOLD)
                },
            });
        }
        if range_active && detail && row_idx == selected_idx + 1 {
            if !range_down {
                return Some(RangeMarker {
                    symbol: extent_symbol.clone(),
                    style: extent_style,
                });
            }
            return None;
        }
    }
    if let Some(pinned_idx) = pinned_idx {
        if row_idx == pinned_idx && !detail {
            return Some(RangeMarker {
                symbol: primary_marker.to_string(),
                style: Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            });
        }
        if let Some(selected_idx) = selected_idx {
            let (min_idx, max_idx) = if pinned_idx <= selected_idx {
                (pinned_idx, selected_idx)
            } else {
                (selected_idx, pinned_idx)
            };
            if row_idx > min_idx && row_idx < max_idx {
                return Some(RangeMarker {
                    symbol: extent_symbol,
                    style: extent_style,
                });
            }
            if range_active && range_down && detail && row_idx == pinned_idx + 1 {
                return Some(RangeMarker {
                    symbol: extent_symbol,
                    style: extent_style,
                });
            }
        }
    }
    None
}

fn marker_width(primary: &str, extent: &str) -> usize {
    let primary_width = text_width(primary);
    let extent_width = text_width(extent);
    primary_width.max(extent_width).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResolvedTheme;
    use oyo_core::git::CommitEntry;

    fn commit(id: &str, parent: &str) -> CommitEntry {
        CommitEntry {
            id: id.to_string(),
            short_id: id.chars().take(7).collect(),
            parents: vec![parent.to_string()],
            author: "a".to_string(),
            author_time: None,
            summary: "s".to_string(),
            stats: None,
        }
    }

    fn dashboard() -> Dashboard {
        Dashboard::new(DashboardConfig {
            repo_root: PathBuf::from("/tmp/oyo"),
            branch: Some("main".to_string()),
            commits: vec![
                commit("bbbbbbbb", "aaaaaaaa"),
                commit("aaaaaaaa", EMPTY_TREE_HASH),
            ],
            working_files: 1,
            staged_files: 1,
            theme: ResolvedTheme::default(),
            primary_marker: ">".to_string(),
            extent_marker: "|".to_string(),
            time_format: TimeFormatter::default(),
            keybindings: Keybindings::default(),
        })
    }

    #[test]
    fn select_selection_restores_range() {
        let mut dashboard = dashboard();
        let selection = DashboardSelection::Range {
            from: "aaaaaaaa".to_string(),
            to: "bbbbbbbb".to_string(),
        };

        dashboard.select_selection(&selection);

        assert_eq!(dashboard.selection(), Some(selection));
    }

    #[test]
    fn select_selection_restores_staged() {
        let mut dashboard = dashboard();

        dashboard.select_selection(&DashboardSelection::Staged);

        assert_eq!(dashboard.selection(), Some(DashboardSelection::Staged));
    }

    #[test]
    fn hover_tracks_visible_dashboard_rows() {
        let mut dashboard = dashboard();
        dashboard.last_list_area = Rect::new(10, 5, 40, 4);

        assert!(dashboard.update_hover(12, 5));
        assert_eq!(dashboard.hovered, Some(0));
        assert!(dashboard.update_hover(12, 7));
        assert_eq!(dashboard.hovered, Some(1));
        assert!(dashboard.update_hover(2, 7));
        assert_eq!(dashboard.hovered, None);
    }

    #[test]
    fn ctrl_click_toggles_range_start_without_selecting() {
        let mut dashboard = dashboard();
        dashboard.last_list_area = Rect::new(10, 5, 40, 6);

        assert!(dashboard.toggle_pin_at_mouse(12, 9));
        assert_eq!(dashboard.selected, 0);
        assert_eq!(dashboard.pinned_from.as_deref(), Some("bbbbbbbb"));

        assert!(dashboard.toggle_pin_at_mouse(12, 9));
        assert_eq!(dashboard.pinned_from, None);
    }

    #[test]
    fn space_toggles_hovered_range_start() {
        let mut dashboard = dashboard();
        dashboard.last_list_area = Rect::new(10, 5, 40, 6);
        dashboard.update_hover(12, 9);

        dashboard.toggle_hovered_pin();

        assert_eq!(dashboard.selected, 0);
        assert_eq!(dashboard.pinned_from.as_deref(), Some("bbbbbbbb"));
    }

    #[test]
    fn shift_space_selects_hovered_range_end() {
        let mut dashboard = dashboard();
        dashboard.last_list_area = Rect::new(10, 5, 40, 6);
        dashboard.update_hover(12, 9);

        assert!(dashboard.select_hovered());

        assert_eq!(dashboard.selected, 2);
        assert_eq!(dashboard.pinned_from, None);
    }

    #[test]
    fn hover_keeps_range_marker_style() {
        let theme = ResolvedTheme::default();
        let entry = DashboardEntry {
            kind: EntryKind::Commit(commit("bbbbbbbb", "aaaaaaaa")),
        };
        let line = entry.render_line(RenderLineContext {
            width: 40,
            stats_width: 0,
            detail: false,
            hovered: true,
            range_marker: Some(RangeMarker {
                symbol: "|".to_string(),
                style: Style::default().fg(theme.diff_ext_marker),
            }),
            marker_width: 1,
            theme: &theme,
            head_meta: None,
            time_format: &TimeFormatter::default(),
            now: 0,
        });

        assert_eq!(line.spans[0].style.fg, Some(theme.diff_ext_marker));
    }

    #[test]
    fn footer_actions_are_hoverable() {
        let mut dashboard = dashboard();
        dashboard.footer_action_hits.push(DashboardFooterHit {
            action: DashboardAction::Accept,
            x: 1,
            y: 2,
            width: 10,
            height: 1,
        });

        assert!(dashboard.update_hover(5, 2));
        assert_eq!(dashboard.footer_action_hover, Some(DashboardAction::Accept));
        assert_eq!(
            dashboard.footer_action_at(5, 2),
            Some(DashboardAction::Accept)
        );
    }

    #[test]
    fn filter_clicks_focus_and_clear_like_sidebar() {
        let mut dashboard = dashboard();
        dashboard.filter_area = Some((0, 10, 20, 1));
        dashboard.filter_clear_hit = Some((18, 10, 1, 1));
        dashboard.filter = "abc".to_string();

        assert!(dashboard.handle_filter_mouse_down(1, 10));
        assert!(dashboard.filter_active());
        assert_eq!(dashboard.filter, "abc");

        assert!(dashboard.handle_filter_mouse_down(18, 10));
        assert!(dashboard.filter_active());
        assert!(dashboard.filter.is_empty());
    }
}
