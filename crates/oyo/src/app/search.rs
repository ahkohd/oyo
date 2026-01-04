use super::utils::{
    apply_highlight_spans, inline_text_for_change, line_has_query, match_ranges,
    old_text_for_change,
};
use super::{AnimationPhase, App, PeekMode, ViewMode};
use crate::color;
use oyo_core::{LineKind, ViewLine};
use ratatui::style::Color;
use ratatui::text::Span;
use regex::RegexBuilder;

impl App {
    pub fn start_search(&mut self) {
        self.search_active = true;
        self.search_query.clear();
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.search_regex = None;
    }

    pub fn stop_search(&mut self) {
        self.search_active = false;
    }

    pub fn clear_search(&mut self) {
        self.search_active = false;
        self.search_query.clear();
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.search_regex = None;
    }

    pub fn clear_search_text(&mut self) {
        self.search_query.clear();
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.search_regex = None;
    }

    pub fn start_goto(&mut self) {
        self.goto_active = true;
        self.goto_query.clear();
    }

    pub fn clear_goto(&mut self) {
        self.goto_active = false;
        self.goto_query.clear();
    }

    pub fn clear_goto_text(&mut self) {
        self.goto_query.clear();
    }

    pub fn push_goto_char(&mut self, ch: char) {
        self.goto_query.push(ch);
    }

    pub fn pop_goto_char(&mut self) {
        self.goto_query.pop();
    }

    pub fn goto_active(&self) -> bool {
        self.goto_active
    }

    pub fn goto_query(&self) -> &str {
        &self.goto_query
    }

    pub fn push_search_char(&mut self, ch: char) {
        self.search_query.push(ch);
        self.search_last_target = None;
        self.update_search_regex();
    }

    pub fn pop_search_char(&mut self) {
        self.search_query.pop();
        self.search_last_target = None;
        self.update_search_regex();
    }

    pub(super) fn reset_search_for_file_switch(&mut self) {
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
    }

    pub fn search_active(&self) -> bool {
        self.search_active
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    fn update_search_regex(&mut self) {
        let query = self.search_query.trim();
        if query.is_empty() {
            self.search_regex = None;
            return;
        }
        let regex = RegexBuilder::new(query)
            .case_insensitive(true)
            .build()
            .or_else(|_| {
                RegexBuilder::new(&regex::escape(query))
                    .case_insensitive(true)
                    .build()
            })
            .ok();
        self.search_regex = regex;
    }

    pub fn search_target(&self) -> Option<usize> {
        self.search_target
    }

    pub fn search_next(&mut self) {
        let matches = self.collect_search_matches();
        if matches.is_empty() {
            return;
        }
        let start = self.search_last_target.unwrap_or(self.scroll_offset);
        let target = matches
            .iter()
            .copied()
            .find(|idx| *idx > start)
            .unwrap_or(matches[0]);
        self.search_last_target = Some(target);
        self.search_target = Some(target);
        self.needs_scroll_to_search = true;
    }

    pub fn search_prev(&mut self) {
        let matches = self.collect_search_matches();
        if matches.is_empty() {
            return;
        }
        let start = self.search_last_target.unwrap_or(self.scroll_offset);
        let target = matches
            .iter()
            .copied()
            .rev()
            .find(|idx| *idx < start)
            .unwrap_or(*matches.last().unwrap());
        self.search_last_target = Some(target);
        self.search_target = Some(target);
        self.needs_scroll_to_search = true;
    }

    pub fn apply_goto(&mut self) {
        let query = self.goto_query.trim();
        if query.is_empty() {
            return;
        }

        let mut chars = query.chars();
        let first = match chars.next() {
            Some(ch) => ch,
            None => return,
        };

        match first {
            'h' | 'H' => {
                let rest = chars
                    .as_str()
                    .trim_start_matches(|c: char| c == ':' || c.is_whitespace());
                if let Ok(num) = rest.parse::<usize>() {
                    self.goto_hunk_number(num);
                }
            }
            's' | 'S' => {
                let rest = chars
                    .as_str()
                    .trim_start_matches(|c: char| c == ':' || c.is_whitespace());
                if let Ok(num) = rest.parse::<usize>() {
                    self.goto_step_number(num);
                }
            }
            _ => {
                if query.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(num) = query.parse::<usize>() {
                        self.goto_line_number(num);
                    }
                }
            }
        }
    }

    pub fn highlight_search_spans(
        &self,
        spans: Vec<Span<'static>>,
        text: &str,
        is_active: bool,
    ) -> Vec<Span<'static>> {
        let Some(regex) = self.search_regex.as_ref() else {
            return spans;
        };
        let ranges = match_ranges(text, regex);
        if ranges.is_empty() {
            return spans;
        }
        let highlight_bg = if is_active {
            self.theme.accent
        } else {
            color::dim_color(self.theme.accent)
        };
        let highlight_fg = if is_active {
            self.search_highlight_fg(highlight_bg)
        } else {
            None
        };
        apply_highlight_spans(spans, &ranges, highlight_bg, highlight_fg)
    }

    fn search_highlight_fg(&self, bg: Color) -> Option<Color> {
        let text = self.theme.text;
        let mut best_color = text;
        let mut best_ratio = color::contrast_ratio(bg, text).unwrap_or(0.0);
        if let Some(bg_color) = self.theme.background {
            let ratio = color::contrast_ratio(bg, bg_color).unwrap_or(0.0);
            if ratio > best_ratio {
                best_ratio = ratio;
                best_color = bg_color;
            }
        }
        if best_ratio == 0.0 {
            None
        } else {
            Some(best_color)
        }
    }

    fn collect_search_matches(&mut self) -> Vec<usize> {
        let regex = match self.search_regex.as_ref() {
            Some(regex) => regex.clone(),
            None => return Vec::new(),
        };
        let frame = self.animation_frame();
        let view = self.current_view_with_frame(frame);
        let mut matches = Vec::new();

        match self.view_mode {
            ViewMode::UnifiedPane | ViewMode::Blame => {
                for (display_idx, line) in view.iter().enumerate() {
                    let text = self.search_text_unified(line);
                    if line_has_query(&text, &regex) {
                        matches.push(display_idx);
                    }
                }
            }
            ViewMode::Evolution => {
                let mut display_idx = 0usize;
                for line in &view {
                    let visible = match line.kind {
                        LineKind::Deleted => false,
                        LineKind::PendingDelete => {
                            line.is_active && self.animation_phase != AnimationPhase::Idle
                        }
                        _ => true,
                    };
                    if !visible {
                        continue;
                    }
                    let text = self.search_text_unified(line);
                    if line_has_query(&text, &regex) {
                        matches.push(display_idx);
                    }
                    display_idx += 1;
                }
            }
            ViewMode::Split => {
                let mut old_idx = 0usize;
                let mut new_idx = 0usize;
                for line in &view {
                    if line.old_line.is_some() {
                        if let Some(text) = self.search_text_split_old(line) {
                            if line_has_query(&text, &regex) {
                                matches.push(old_idx);
                            }
                        }
                        old_idx += 1;
                    }
                    if line.new_line.is_some()
                        && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                    {
                        if let Some(text) = self.search_text_split_new(line) {
                            if line_has_query(&text, &regex) {
                                matches.push(new_idx);
                            }
                        }
                        new_idx += 1;
                    }
                }
            }
        }

        matches.sort_unstable();
        matches
    }

    fn search_text_unified(&mut self, view_line: &ViewLine) -> String {
        if let Some(mode) = self.peek_mode_for_line(view_line) {
            match mode {
                PeekMode::Old => {
                    if let Some(text) = self.peek_text_for_line(view_line) {
                        return text;
                    }
                }
                PeekMode::Modified => {
                    if let Some(text) = self.modified_only_text_for_line(view_line) {
                        return text;
                    }
                }
                PeekMode::Mixed => {
                    if let Some(change) = self
                        .multi_diff
                        .current_navigator()
                        .diff()
                        .changes
                        .get(view_line.change_id)
                    {
                        let text = inline_text_for_change(change);
                        if !text.is_empty() {
                            return text;
                        }
                    }
                }
            }
        }
        if !self.stepping && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
        {
            if let Some(change) = self
                .multi_diff
                .current_navigator()
                .diff()
                .changes
                .get(view_line.change_id)
            {
                let text = inline_text_for_change(change);
                if !text.is_empty() {
                    return text;
                }
            }
        }
        view_line.content.clone()
    }

    fn search_text_split_old(&mut self, view_line: &ViewLine) -> Option<String> {
        view_line.old_line?;
        if matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify) {
            if let Some(change) = self
                .multi_diff
                .current_navigator()
                .diff()
                .changes
                .get(view_line.change_id)
            {
                let text = old_text_for_change(change);
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        Some(view_line.content.clone())
    }

    fn search_text_split_new(&mut self, view_line: &ViewLine) -> Option<String> {
        view_line.new_line?;
        Some(view_line.content.clone())
    }

    pub fn handle_search_scroll_if_needed(&mut self, viewport_height: usize) -> bool {
        if !self.needs_scroll_to_search {
            return false;
        }
        self.needs_scroll_to_search = false;
        if let Some(idx) = self.search_target {
            if self.auto_center {
                let half_viewport = viewport_height / 2;
                self.scroll_offset = idx.saturating_sub(half_viewport);
                self.centered_once = true;
            } else {
                let margin = 3.min(viewport_height / 4);
                if idx < self.scroll_offset.saturating_add(margin) {
                    self.scroll_offset = idx.saturating_sub(margin);
                } else if idx
                    >= self
                        .scroll_offset
                        .saturating_add(viewport_height.saturating_sub(margin))
                {
                    self.scroll_offset =
                        idx.saturating_sub(viewport_height.saturating_sub(margin + 1));
                }
            }
        }
        true
    }
}
