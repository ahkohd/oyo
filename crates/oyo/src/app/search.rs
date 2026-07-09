use super::utils::{
    apply_highlight_spans, inline_text_for_change, line_has_query, match_ranges,
    old_text_for_change,
};
use super::{AnimationPhase, App, PeekMode, ViewMode};
use crate::color;
use crate::views::{expand_tabs_in_text, TAB_WIDTH};
use oyo_core::{LineKind, ViewLine};
use ratatui::style::{Color, Modifier};
use ratatui::text::Span;
use regex::{Regex, RegexBuilder};
use unicode_width::UnicodeWidthStr;

const SEARCH_MIN_CONTRAST: f32 = 4.5;

fn point_in_rect(rect: (u16, u16, u16, u16), column: u16, row: u16) -> bool {
    let (x, y, width, height) = rect;
    column >= x && column < x.saturating_add(width) && row >= y && row < y.saturating_add(height)
}

impl App {
    fn bump_search_revision(&mut self) {
        self.search_revision = self.search_revision.wrapping_add(1);
    }

    pub(crate) fn search_revision(&self) -> u64 {
        self.search_revision
    }

    pub fn start_search(&mut self) {
        self.search_active = true;
        self.search_cursor_visible = true;
        self.search_cursor_last_blink = std::time::Instant::now();
        self.search_query.clear();
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.search_regex = None;
        self.bump_search_revision();
    }

    pub fn stop_search(&mut self) {
        self.search_active = false;
        self.bump_search_revision();
    }

    pub fn clear_search(&mut self) {
        self.search_active = false;
        self.search_cursor_visible = true;
        self.search_query.clear();
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.search_regex = None;
        self.bump_search_revision();
    }

    pub fn clear_search_text(&mut self) {
        self.search_cursor_visible = true;
        self.search_cursor_last_blink = std::time::Instant::now();
        self.search_query.clear();
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.search_regex = None;
        self.bump_search_revision();
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
        self.search_cursor_visible = true;
        self.search_cursor_last_blink = std::time::Instant::now();
        self.search_query.push(ch);
        self.search_last_target = None;
        self.update_search_regex();
        self.bump_search_revision();
    }

    pub fn pop_search_char(&mut self) {
        self.search_cursor_visible = true;
        self.search_cursor_last_blink = std::time::Instant::now();
        self.search_query.pop();
        self.search_last_target = None;
        self.update_search_regex();
        self.bump_search_revision();
    }

    pub(super) fn reset_search_for_file_switch(&mut self) {
        self.search_last_target = None;
        self.search_target = None;
        self.needs_scroll_to_search = false;
        self.bump_search_revision();
    }

    pub fn search_active(&self) -> bool {
        self.search_active
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub(crate) fn search_bar_visible(&self) -> bool {
        self.search_active || !self.search_query.is_empty()
    }

    pub(crate) fn set_search_bar_hits(
        &mut self,
        bar: Option<(u16, u16, u16, u16)>,
        prev: Option<(u16, u16, u16, u16)>,
        next: Option<(u16, u16, u16, u16)>,
        clear: Option<(u16, u16, u16, u16)>,
    ) {
        self.search_bar_hit = bar;
        self.search_prev_hit = prev;
        self.search_next_hit = next;
        self.search_clear_hit = clear;
        if prev.is_none() && next.is_none() && clear.is_none() {
            self.search_prev_hover = false;
            self.search_next_hover = false;
            self.search_clear_hover = false;
        }
    }

    pub(crate) fn update_search_bar_hover(&mut self, column: u16, row: u16) -> bool {
        let prev = self
            .search_prev_hit
            .is_some_and(|rect| point_in_rect(rect, column, row));
        let next = self
            .search_next_hit
            .is_some_and(|rect| point_in_rect(rect, column, row));
        let clear = self
            .search_clear_hit
            .is_some_and(|rect| point_in_rect(rect, column, row));
        if self.search_prev_hover == prev
            && self.search_next_hover == next
            && self.search_clear_hover == clear
        {
            return false;
        }
        self.search_prev_hover = prev;
        self.search_next_hover = next;
        self.search_clear_hover = clear;
        true
    }

    pub(crate) fn handle_search_bar_click(&mut self, column: u16, row: u16) -> bool {
        if self
            .search_prev_hit
            .is_some_and(|rect| point_in_rect(rect, column, row))
        {
            self.search_prev();
            return true;
        }
        if self
            .search_next_hit
            .is_some_and(|rect| point_in_rect(rect, column, row))
        {
            self.search_next();
            return true;
        }
        if self
            .search_clear_hit
            .is_some_and(|rect| point_in_rect(rect, column, row))
        {
            self.clear_search();
            return true;
        }
        self.search_bar_hit
            .is_some_and(|rect| point_in_rect(rect, column, row))
    }

    pub(crate) fn search_match_position(&mut self) -> (usize, usize) {
        let matches = self.collect_search_matches();
        let current = self
            .search_target
            .and_then(|target| matches.iter().position(|idx| *idx == target))
            .map(|idx| idx + 1)
            .unwrap_or(0);
        (current, matches.len())
    }

    pub(crate) fn search_target_screen_row(&self) -> Option<u16> {
        let target = self.search_target?;
        let (_, y, _, height) = self.diff_view_area?;
        (y..y.saturating_add(height))
            .find(|row| self.review_display_idx_for_screen_row(*row) == Some(target))
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

    pub(crate) fn set_preview_search_lines(&mut self, lines: Vec<String>) {
        if self.preview_search_lines != lines {
            self.preview_search_lines = lines;
            self.bump_search_revision();
        }
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
        self.bump_search_revision();
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
        self.bump_search_revision();
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
        let highlight_fg = self.search_highlight_fg(highlight_bg);
        let modifier = if is_active {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };
        apply_highlight_spans(spans, &ranges, highlight_bg, highlight_fg, modifier)
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
        if best_ratio < SEARCH_MIN_CONTRAST {
            let black = Color::Rgb(0, 0, 0);
            let white = Color::Rgb(255, 255, 255);
            let black_ratio = color::contrast_ratio(bg, black).unwrap_or(0.0);
            let white_ratio = color::contrast_ratio(bg, white).unwrap_or(0.0);
            if black_ratio > white_ratio {
                best_color = black;
                best_ratio = black_ratio;
            } else {
                best_color = white;
                best_ratio = white_ratio;
            }
        }
        (best_ratio > 0.0).then_some(best_color)
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
                    if super::is_fold_line(line) {
                        continue;
                    }
                    let text = self.search_text_unified(line);
                    if line_has_query(&text, &regex) {
                        matches.push(display_idx);
                    }
                }
            }
            ViewMode::Evolution => {
                let mut display_idx = 0usize;
                for line in view.iter() {
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
                    if !super::is_fold_line(line) {
                        let text = self.search_text_unified(line);
                        if line_has_query(&text, &regex) {
                            matches.push(display_idx);
                        }
                    }
                    display_idx += 1;
                }
            }
            ViewMode::Split => {
                let mut old_idx = 0usize;
                let mut new_idx = 0usize;
                for line in view.iter() {
                    let fold = super::is_fold_line(line);
                    let old_present = line.old_line.is_some() || fold;
                    let new_present = (line.new_line.is_some()
                        && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete))
                        || fold;
                    if old_present || (self.split_align_lines && new_present) {
                        if line.old_line.is_some() {
                            if let Some(text) = self.search_text_split_old(line) {
                                if line_has_query(&text, &regex) {
                                    matches.push(old_idx);
                                }
                            }
                        }
                        old_idx += 1;
                    }
                    if new_present || (self.split_align_lines && old_present) {
                        if line.new_line.is_some()
                            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                        {
                            if let Some(text) = self.search_text_split_new(line) {
                                if line_has_query(&text, &regex) {
                                    matches.push(new_idx);
                                }
                            }
                        }
                        new_idx += 1;
                    }
                }
            }
            ViewMode::Preview => {
                for (display_idx, text) in self.preview_search_lines.iter().enumerate() {
                    if line_has_query(text, &regex) {
                        matches.push(display_idx);
                    }
                }
            }
        }

        matches.sort_unstable();
        matches.dedup();
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

    fn search_target_display_range(&mut self) -> Option<(usize, usize)> {
        let target = self.search_target?;
        let regex = self.search_regex.clone()?;
        let frame = self.animation_frame();
        let view = self.current_view_with_frame(frame);

        match self.view_mode {
            ViewMode::UnifiedPane | ViewMode::Blame => view
                .get(target)
                .map(|line| self.search_text_unified(line))
                .and_then(|text| search_match_display_range(&text, &regex)),
            ViewMode::Evolution => {
                let mut display_idx = 0usize;
                for line in view.iter() {
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
                    if display_idx == target && !super::is_fold_line(line) {
                        let text = self.search_text_unified(line);
                        return search_match_display_range(&text, &regex);
                    }
                    display_idx += 1;
                }
                None
            }
            ViewMode::Split => {
                let mut old_idx = 0usize;
                let mut new_idx = 0usize;
                for line in view.iter() {
                    let fold = super::is_fold_line(line);
                    let old_present = line.old_line.is_some() || fold;
                    let new_present = (line.new_line.is_some()
                        && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete))
                        || fold;
                    if old_present || (self.split_align_lines && new_present) {
                        if old_idx == target && line.old_line.is_some() {
                            if let Some(range) = self
                                .search_text_split_old(line)
                                .and_then(|text| search_match_display_range(&text, &regex))
                            {
                                return Some(range);
                            }
                        }
                        old_idx += 1;
                    }
                    if new_present || (self.split_align_lines && old_present) {
                        if new_idx == target
                            && line.new_line.is_some()
                            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                        {
                            if let Some(range) = self
                                .search_text_split_new(line)
                                .and_then(|text| search_match_display_range(&text, &regex))
                            {
                                return Some(range);
                            }
                        }
                        new_idx += 1;
                    }
                }
                None
            }
            ViewMode::Preview => self
                .preview_search_lines
                .get(target)
                .and_then(|text| search_match_display_range(text, &regex)),
        }
    }

    fn scroll_search_match_horizontally(&mut self, viewport_width: usize) {
        if self.line_wrap || viewport_width == 0 {
            return;
        }
        let Some((start, end)) = self.search_target_display_range() else {
            return;
        };
        let margin = 3.min(viewport_width / 4);
        let usable_width = viewport_width.saturating_sub(margin.saturating_mul(2));
        if end <= viewport_width.saturating_sub(margin) {
            self.horizontal_scroll = 0;
        } else if end.saturating_sub(start) >= usable_width
            || start < self.horizontal_scroll.saturating_add(margin)
        {
            self.horizontal_scroll = start.saturating_sub(margin);
        } else if end.saturating_add(margin) > self.horizontal_scroll.saturating_add(viewport_width)
        {
            self.horizontal_scroll = end.saturating_add(margin).saturating_sub(viewport_width);
        }
    }

    pub fn handle_search_scroll_if_needed(
        &mut self,
        viewport_height: usize,
        viewport_width: usize,
    ) -> bool {
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
            self.scroll_search_match_horizontally(viewport_width);
        }
        true
    }
}

fn search_match_display_range(text: &str, regex: &Regex) -> Option<(usize, usize)> {
    let matched = regex.find(text)?;
    let expanded = expand_tabs_in_text(&text[..matched.end()], TAB_WIDTH);
    let end = UnicodeWidthStr::width(expanded.as_str());
    let start =
        UnicodeWidthStr::width(expand_tabs_in_text(&text[..matched.start()], TAB_WIDTH).as_str());
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oyo_core::{AnimationFrame, MultiFileDiff};
    use ratatui::style::Style;

    #[test]
    fn search_highlights_clear_contrast_floor_for_active_and_inactive_matches() {
        let diff = MultiFileDiff::from_file_pair(
            "test.rs".into(),
            "test.rs".into(),
            "old\n".into(),
            "needle\n".into(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.theme.accent = Color::Rgb(120, 120, 120);
        app.theme.text = Color::Rgb(125, 125, 125);
        app.theme.background = Some(Color::Rgb(115, 115, 115));
        app.start_search();
        for ch in "needle".chars() {
            app.push_search_char(ch);
        }

        let syntax_fg = Color::Rgb(118, 118, 118);
        let highlight = |active| {
            app.highlight_search_spans(
                vec![Span::styled("needle", Style::default().fg(syntax_fg))],
                "needle",
                active,
            )[0]
            .style
        };
        let inactive = highlight(false);
        let active = highlight(true);

        assert_ne!(active.bg, inactive.bg);
        assert!(active.add_modifier.contains(Modifier::BOLD));
        assert!(!inactive.add_modifier.contains(Modifier::BOLD));
        for style in [inactive, active] {
            assert!(
                color::contrast_ratio(style.fg.unwrap(), style.bg.unwrap()).unwrap()
                    >= SEARCH_MIN_CONTRAST
            );
            assert_ne!(style.fg, Some(syntax_fg));
        }
    }

    #[test]
    fn search_state_changes_invalidate_the_view_cache() {
        let diff = MultiFileDiff::from_file_pair(
            "README.md".into(),
            "README.md".into(),
            "old\n".into(),
            "use one\nuse two\n".into(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.goto_last_step();
        app.start_search();
        for ch in "use".chars() {
            app.push_search_char(ch);
        }
        let query_key = app.view_cache_key(AnimationFrame::Idle, false, 0);

        app.search_next();
        let target_key = app.view_cache_key(AnimationFrame::Idle, false, 0);
        assert_ne!(target_key, query_key);

        app.clear_search();
        let clear_key = app.view_cache_key(AnimationFrame::Idle, false, 0);
        assert_ne!(clear_key, target_key);
    }

    #[test]
    fn search_navigation_scrolls_horizontally_to_each_match() {
        for view_mode in [ViewMode::UnifiedPane, ViewMode::Split] {
            let diff = MultiFileDiff::from_file_pair(
                "test.rs".into(),
                "test.rs".into(),
                "old\n".into(),
                format!("short needle\n{}needle\n", "x".repeat(60)),
            );
            let mut app = App::new(diff, view_mode, 0, false, None);
            app.goto_last_step();
            app.start_search();
            for ch in "needle".chars() {
                app.push_search_char(ch);
            }

            app.search_next();
            assert!(app.handle_search_scroll_if_needed(10, 20));
            let first_scroll = app.horizontal_scroll;

            app.search_next();
            assert!(app.handle_search_scroll_if_needed(10, 20));
            let second_scroll = app.horizontal_scroll;
            assert!(first_scroll == 0 || second_scroll == 0);
            let right_scroll = first_scroll.max(second_scroll);
            assert!(right_scroll <= 60);
            assert!(66 <= right_scroll + 20);

            app.search_next();
            assert!(app.handle_search_scroll_if_needed(10, 20));
            assert_eq!(app.horizontal_scroll, first_scroll);
        }
    }

    #[test]
    fn search_next_and_previous_move_and_wrap_the_active_match() {
        let diff = MultiFileDiff::from_file_pair(
            "test.rs".into(),
            "test.rs".into(),
            "old\n".into(),
            "needle\nother\nneedle\n".into(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.goto_last_step();
        app.start_search();
        for ch in "needle".chars() {
            app.push_search_char(ch);
        }

        app.search_next();
        let first = app.search_target().unwrap();
        app.search_next();
        let second = app.search_target().unwrap();
        assert_ne!(second, first);
        app.search_next();
        assert_eq!(app.search_target(), Some(first));
        app.search_prev();
        assert_eq!(app.search_target(), Some(second));
    }
}
