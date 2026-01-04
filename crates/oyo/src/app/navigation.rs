use super::utils::{
    copy_to_clipboard, inline_text_for_change, is_fold_line, modified_only_text_for_change,
    old_text_for_change,
};
use super::{
    AnimationPhase, App, HunkBounds, HunkEdge, HunkEdgeHint, HunkStart, PeekMode, PeekScope,
    PeekState, StepEdge, StepEdgeHint, ViewMode,
};
use crate::config::{HunkWrapMode, ModifiedStepMode, StepWrapMode};
use oyo_core::{AnimationFrame, ChangeKind, LineKind, StepState, ViewLine};
use std::time::{Duration, Instant};

const STEP_EDGE_HINT_MS: u64 = 700;

impl App {
    pub fn toggle_peek_old_change(&mut self) {
        self.cycle_peek_change();
    }

    pub fn toggle_peek_old_hunk(&mut self) {
        self.toggle_peek_hunk();
    }

    fn clear_peek(&mut self) {
        self.peek_state = None;
    }

    fn cycle_peek_change(&mut self) {
        if !self.stepping {
            return;
        }
        let base = self.base_modified_view_mode();
        let current = match self.peek_state {
            Some(PeekState {
                scope: PeekScope::Change,
                mode,
            }) => mode,
            _ => base,
        };
        let next = match current {
            PeekMode::Modified => PeekMode::Old,
            PeekMode::Old => PeekMode::Mixed,
            PeekMode::Mixed => PeekMode::Modified,
        };
        if next == base {
            self.peek_state = None;
        } else {
            self.peek_state = Some(PeekState {
                scope: PeekScope::Change,
                mode: next,
            });
        }
    }

    fn toggle_peek_hunk(&mut self) {
        if !self.stepping {
            return;
        }
        let next = PeekState {
            scope: PeekScope::Hunk,
            mode: PeekMode::Old,
        };
        if self.peek_state == Some(next) {
            self.peek_state = None;
        } else {
            self.peek_state = Some(next);
        }
    }

    fn base_modified_view_mode(&self) -> PeekMode {
        if self.unified_modified_step_mode == ModifiedStepMode::Modified {
            PeekMode::Modified
        } else {
            PeekMode::Mixed
        }
    }

    pub fn is_peek_override_for_line(&mut self, view_line: &ViewLine) -> bool {
        if !self.stepping {
            return false;
        }
        let Some(peek) = self.peek_state else {
            return false;
        };
        match peek.scope {
            PeekScope::Change => view_line.is_primary_active,
            PeekScope::Hunk => {
                let current_hunk = self.multi_diff.current_navigator().state().current_hunk;
                view_line.hunk_index == Some(current_hunk)
            }
        }
    }

    pub fn peek_mode_for_line(&mut self, view_line: &ViewLine) -> Option<PeekMode> {
        if !self.stepping {
            return None;
        }
        if let Some(peek) = self.peek_state {
            match peek.scope {
                PeekScope::Change => {
                    if view_line.is_primary_active {
                        return Some(peek.mode);
                    }
                }
                PeekScope::Hunk => {
                    let current_hunk = self.multi_diff.current_navigator().state().current_hunk;
                    if view_line.hunk_index == Some(current_hunk) {
                        return Some(PeekMode::Old);
                    }
                }
            }
            return None;
        }
        if view_line.is_primary_active
            && matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify)
        {
            return Some(self.base_modified_view_mode());
        }
        None
    }

    pub fn yank_current_change(&mut self) {
        let frame = self.animation_frame();
        let view_lines = self.current_view_with_frame(frame);
        let Some(line) = view_lines.iter().find(|line| line.is_primary_active) else {
            return;
        };
        if let Some(text) = self.text_for_yank(line) {
            copy_to_clipboard(&text);
        }
    }

    pub fn yank_current_hunk(&mut self) {
        let frame = self.animation_frame();
        let view_lines = self.current_view_with_frame(frame);
        let current_hunk = self.multi_diff.current_navigator().state().current_hunk;
        let mut lines: Vec<String> = Vec::new();
        for line in view_lines
            .iter()
            .filter(|line| line.hunk_index == Some(current_hunk))
        {
            if let Some(text) = self.text_for_yank(line) {
                lines.push(text);
            }
        }
        if lines.is_empty() {
            return;
        }
        copy_to_clipboard(&lines.join("\n"));
    }

    fn text_for_yank(&mut self, view_line: &ViewLine) -> Option<String> {
        if let Some(mode) = self.peek_mode_for_line(view_line) {
            match mode {
                PeekMode::Old => {
                    if let Some(text) = self.peek_text_for_line(view_line) {
                        return Some(text);
                    }
                }
                PeekMode::Modified => {
                    if let Some(text) = self.modified_only_text_for_line(view_line) {
                        return Some(text);
                    }
                }
                PeekMode::Mixed => {
                    let change = self
                        .multi_diff
                        .current_navigator()
                        .diff()
                        .changes
                        .get(view_line.change_id);
                    if let Some(change) = change {
                        let text = inline_text_for_change(change);
                        if !text.is_empty() {
                            return Some(text);
                        }
                    }
                }
            }
        }
        Some(view_line.content.clone())
    }

    pub(super) fn peek_text_for_line(&mut self, view_line: &ViewLine) -> Option<String> {
        if !matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify) {
            return None;
        }
        let change = self
            .multi_diff
            .current_navigator()
            .diff()
            .changes
            .get(view_line.change_id)?;
        let text = old_text_for_change(change);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub(super) fn modified_only_text_for_line(&mut self, view_line: &ViewLine) -> Option<String> {
        if !matches!(view_line.kind, LineKind::Modified | LineKind::PendingModify) {
            return None;
        }
        let change = self
            .multi_diff
            .current_navigator()
            .diff()
            .changes
            .get(view_line.change_id)?;
        let text = modified_only_text_for_change(change);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub fn state(&mut self) -> StepState {
        self.multi_diff.current_navigator().state().clone()
    }

    pub fn next_step(&mut self) {
        self.step_forward();
    }

    pub fn prev_step(&mut self) {
        self.step_backward();
    }

    pub fn replay_step(&mut self) {
        if !self.stepping {
            return;
        }
        let count = self.take_count();
        if self.animation_phase != AnimationPhase::Idle || self.snap_frame.is_some() {
            return;
        }
        let current_step = self.multi_diff.current_navigator().state().current_step;
        if current_step == 0 {
            return;
        }
        let back_steps = count.min(current_step);
        if back_steps == 0 {
            return;
        }
        self.autoplay = false;
        self.autoplay_remaining = None;
        let target_step = current_step.saturating_sub(back_steps);
        self.clear_peek();
        self.snap_frame = None;
        self.snap_frame_started_at = None;
        self.clear_active_on_next_render = false;
        self.multi_diff.current_navigator().goto(target_step);
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
        self.autoplay = true;
        self.autoplay_reverse = false;
        self.autoplay_remaining = Some(back_steps);
        self.last_autoplay_tick = Instant::now();
    }

    pub(super) fn clear_step_edge_hint(&mut self) {
        self.step_edge_hint = None;
    }

    pub(super) fn clear_hunk_edge_hint(&mut self) {
        self.hunk_edge_hint = None;
    }

    pub(crate) fn step_edge_hint_for_change(&self, change_id: usize) -> Option<&'static str> {
        let hint = self.step_edge_hint?;
        if Instant::now() > hint.until {
            return None;
        }
        if hint.change_id == Some(change_id) {
            Some(match hint.edge {
                StepEdge::Start => "No more steps",
                StepEdge::End => "No more steps",
            })
        } else {
            None
        }
    }

    pub(crate) fn hunk_edge_hint_text(&self) -> Option<&'static str> {
        let hint = self.hunk_edge_hint?;
        if Instant::now() > hint.until {
            return None;
        }
        Some(match hint.edge {
            HunkEdge::First => "First hunk",
            HunkEdge::Last => "Last hunk",
        })
    }

    pub(crate) fn hunk_hint_overflow(
        &mut self,
        hunk_idx: usize,
        viewport_height: usize,
    ) -> Option<(bool, bool)> {
        let bounds = match self.view_mode {
            ViewMode::Split => {
                let (old_bounds, new_bounds) = self.compute_hunk_bounds_split();
                let old = old_bounds.get(hunk_idx).copied().flatten();
                let new = new_bounds.get(hunk_idx).copied().flatten();
                self.pick_split_bounds(old, new)
            }
            _ => self
                .compute_hunk_bounds_unified()
                .get(hunk_idx)
                .copied()
                .flatten(),
        }?;

        let visible_start = self.scroll_offset;
        let visible_end = self
            .scroll_offset
            .saturating_add(viewport_height.saturating_sub(1));
        let overflow_above = bounds.start.idx < visible_start;
        let overflow_below = bounds.end.idx > visible_end;
        Some((overflow_above, overflow_below))
    }

    pub(crate) fn last_step_hint_text(&mut self) -> Option<&'static str> {
        if !self.stepping {
            return None;
        }
        let state = self.multi_diff.current_navigator().state();
        if state.total_steps < 2 {
            return None;
        }
        let remaining = state
            .total_steps
            .saturating_sub(1)
            .saturating_sub(state.current_step);
        if remaining != 1 {
            return None;
        }
        Some("Last step next")
    }

    fn trigger_hunk_edge_hint(&mut self, edge: HunkEdge) {
        self.hunk_edge_hint = Some(HunkEdgeHint {
            edge,
            until: Instant::now() + Duration::from_millis(STEP_EDGE_HINT_MS),
        });
    }

    fn trigger_step_edge_hint(&mut self, edge: StepEdge) {
        let state = self.multi_diff.current_navigator().state();
        let change_id = match edge {
            StepEdge::End => state
                .applied_changes
                .last()
                .copied()
                .or(state.active_change),
            StepEdge::Start => state
                .applied_changes
                .first()
                .copied()
                .or(state.active_change),
        };
        self.step_edge_hint = Some(StepEdgeHint {
            change_id,
            edge,
            until: Instant::now() + Duration::from_millis(STEP_EDGE_HINT_MS),
        });
    }

    pub(super) fn step_forward(&mut self) -> bool {
        self.clear_peek();
        self.clear_hunk_edge_hint();
        self.clear_blame_hunk_hint();
        self.clear_blame_step_hint();
        self.snap_frame = None;
        self.snap_frame_started_at = None;
        if self.multi_diff.current_navigator().next() {
            self.clear_step_edge_hint();
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
            self.refresh_blame_toggle_hint();
            true
        } else {
            match self.step_wrap {
                StepWrapMode::File => {
                    if self.next_file_wrapped() {
                        self.goto_first_step();
                        return true;
                    }
                }
                StepWrapMode::Step => {
                    self.goto_first_step();
                    return true;
                }
                StepWrapMode::None => {}
            }
            self.trigger_step_edge_hint(StepEdge::End);
            false
        }
    }

    pub(super) fn step_backward(&mut self) -> bool {
        self.clear_peek();
        self.clear_hunk_edge_hint();
        self.clear_blame_hunk_hint();
        self.clear_blame_step_hint();
        self.snap_frame = None;
        self.snap_frame_started_at = None;
        if !self.animation_enabled {
            self.snap_frame = Some(AnimationFrame::FadeOut);
            self.snap_frame_started_at = Some(Instant::now());
            self.clear_active_on_next_render = false;
        }
        if self.multi_diff.current_navigator().prev() {
            self.clear_step_edge_hint();
            if self.animation_enabled {
                self.start_animation();
            } else if self.snap_frame.is_none() {
                self.clear_active_on_next_render = true;
            }
            self.needs_scroll_to_active = true;
            self.refresh_blame_toggle_hint();
            true
        } else {
            match self.step_wrap {
                StepWrapMode::File => {
                    if self.prev_file_wrapped() {
                        self.goto_last_step();
                        return true;
                    }
                }
                StepWrapMode::Step => {
                    self.goto_last_step();
                    return true;
                }
                StepWrapMode::None => {}
            }
            self.trigger_step_edge_hint(StepEdge::Start);
            false
        }
    }

    /// Compute hunk starts for unified/evolution view (display index + change id).
    fn compute_hunk_starts_unified(&mut self) -> Vec<Option<HunkStart>> {
        let view = self.current_view_with_frame(AnimationFrame::Idle);
        let (_, total_hunks) = self.hunk_info();

        let mut hunk_starts = vec![None; total_hunks];
        let mut display_idx = 0;

        for line in &view {
            let is_visible = match self.view_mode {
                ViewMode::Evolution => {
                    !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                }
                _ => true,
            };
            if !is_visible {
                continue;
            }
            if let Some(hidx) = line.hunk_index {
                if hidx < total_hunks && hunk_starts[hidx].is_none() {
                    hunk_starts[hidx] = Some(HunkStart {
                        idx: display_idx,
                        change_id: Some(line.change_id),
                    });
                }
            }
            display_idx += 1;
        }
        hunk_starts
    }

    /// Compute hunk bounds for unified/evolution view (display start/end + change id).
    fn compute_hunk_bounds_unified(&mut self) -> Vec<Option<HunkBounds>> {
        let view = self.current_view_with_frame(AnimationFrame::Idle);
        let (_, total_hunks) = self.hunk_info();

        let mut bounds: Vec<Option<HunkBounds>> = vec![None; total_hunks];
        let mut display_idx = 0;

        for line in &view {
            let is_visible = match self.view_mode {
                ViewMode::Evolution => {
                    !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                }
                _ => true,
            };
            if !is_visible {
                continue;
            }
            if let Some(hidx) = line.hunk_index {
                if hidx < total_hunks {
                    let start = HunkStart {
                        idx: display_idx,
                        change_id: Some(line.change_id),
                    };
                    if let Some(existing) = bounds[hidx] {
                        bounds[hidx] = Some(HunkBounds {
                            start: existing.start,
                            end: start,
                        });
                    } else {
                        bounds[hidx] = Some(HunkBounds { start, end: start });
                    }
                }
            }
            display_idx += 1;
        }
        bounds
    }

    /// Compute hunk starts for split view (per-pane display index + change id).
    fn compute_hunk_starts_split(&mut self) -> (Vec<Option<HunkStart>>, Vec<Option<HunkStart>>) {
        let view = self.current_view_with_frame(AnimationFrame::Idle);
        let (_, total_hunks) = self.hunk_info();

        let mut old_starts = vec![None; total_hunks];
        let mut new_starts = vec![None; total_hunks];
        let mut old_idx = 0usize;
        let mut new_idx = 0usize;

        for line in &view {
            let fold_line = is_fold_line(line);
            if line.old_line.is_some() || fold_line {
                if let Some(hidx) = line.hunk_index {
                    if hidx < total_hunks && old_starts[hidx].is_none() {
                        old_starts[hidx] = Some(HunkStart {
                            idx: old_idx,
                            change_id: Some(line.change_id),
                        });
                    }
                }
                old_idx += 1;
            }
            if line.new_line.is_some() || fold_line {
                if let Some(hidx) = line.hunk_index {
                    if hidx < total_hunks && new_starts[hidx].is_none() {
                        new_starts[hidx] = Some(HunkStart {
                            idx: new_idx,
                            change_id: Some(line.change_id),
                        });
                    }
                }
                new_idx += 1;
            }
        }

        (old_starts, new_starts)
    }

    /// Compute hunk bounds for split view (per-pane display start/end + change id).
    fn compute_hunk_bounds_split(&mut self) -> (Vec<Option<HunkBounds>>, Vec<Option<HunkBounds>>) {
        let view = self.current_view_with_frame(AnimationFrame::Idle);
        let (_, total_hunks) = self.hunk_info();

        let mut old_bounds: Vec<Option<HunkBounds>> = vec![None; total_hunks];
        let mut new_bounds: Vec<Option<HunkBounds>> = vec![None; total_hunks];
        let mut old_idx = 0usize;
        let mut new_idx = 0usize;

        for line in &view {
            let fold_line = is_fold_line(line);
            if line.old_line.is_some() || fold_line {
                if let Some(hidx) = line.hunk_index {
                    if hidx < total_hunks {
                        let start = HunkStart {
                            idx: old_idx,
                            change_id: Some(line.change_id),
                        };
                        if let Some(existing) = old_bounds[hidx] {
                            old_bounds[hidx] = Some(HunkBounds {
                                start: existing.start,
                                end: start,
                            });
                        } else {
                            old_bounds[hidx] = Some(HunkBounds { start, end: start });
                        }
                    }
                }
                old_idx += 1;
            }
            if line.new_line.is_some() || fold_line {
                if let Some(hidx) = line.hunk_index {
                    if hidx < total_hunks {
                        let start = HunkStart {
                            idx: new_idx,
                            change_id: Some(line.change_id),
                        };
                        if let Some(existing) = new_bounds[hidx] {
                            new_bounds[hidx] = Some(HunkBounds {
                                start: existing.start,
                                end: start,
                            });
                        } else {
                            new_bounds[hidx] = Some(HunkBounds { start, end: start });
                        }
                    }
                }
                new_idx += 1;
            }
        }

        (old_bounds, new_bounds)
    }

    fn pick_split_start(
        &self,
        old: Option<HunkStart>,
        new: Option<HunkStart>,
    ) -> Option<HunkStart> {
        match (old, new) {
            (Some(o), Some(n)) => {
                let old_dist = (o.idx as isize - self.scroll_offset as isize).abs();
                let new_dist = (n.idx as isize - self.scroll_offset as isize).abs();
                if old_dist < new_dist {
                    Some(o)
                } else {
                    Some(n)
                }
            }
            (Some(o), None) => Some(o),
            (None, Some(n)) => Some(n),
            (None, None) => None,
        }
    }

    fn pick_split_index(&self, old: Option<usize>, new: Option<usize>) -> Option<usize> {
        match (old, new) {
            (Some(o), Some(n)) => {
                let old_dist = (o as isize - self.scroll_offset as isize).abs();
                let new_dist = (n as isize - self.scroll_offset as isize).abs();
                if old_dist < new_dist {
                    Some(o)
                } else {
                    Some(n)
                }
            }
            (Some(o), None) => Some(o),
            (None, Some(n)) => Some(n),
            (None, None) => None,
        }
    }

    fn pick_split_bounds(
        &self,
        old: Option<HunkBounds>,
        new: Option<HunkBounds>,
    ) -> Option<HunkBounds> {
        match (old, new) {
            (Some(o), Some(n)) => {
                let old_dist = (o.start.idx as isize - self.scroll_offset as isize).abs();
                let new_dist = (n.start.idx as isize - self.scroll_offset as isize).abs();
                if old_dist < new_dist {
                    Some(o)
                } else {
                    Some(n)
                }
            }
            (Some(o), None) => Some(o),
            (None, Some(n)) => Some(n),
            (None, None) => None,
        }
    }

    fn next_hunk_from_starts(
        &self,
        starts: &[Option<HunkStart>],
        inclusive: bool,
    ) -> Option<(usize, HunkStart)> {
        let current_hunk_idx = starts
            .iter()
            .enumerate()
            .filter_map(|(idx, start)| start.map(|s| (idx, s.idx)))
            .filter(|&(_, start)| {
                if inclusive {
                    start <= self.scroll_offset
                } else {
                    start < self.scroll_offset
                }
            })
            .map(|(idx, _)| idx)
            .last();

        let mut target_idx = match current_hunk_idx {
            Some(curr) => curr + 1,
            None => 0,
        };

        while target_idx < starts.len() {
            if let Some(start) = starts[target_idx] {
                return Some((target_idx, start));
            }
            target_idx += 1;
        }
        None
    }

    fn next_hunk_from_index(
        &self,
        starts: &[Option<HunkStart>],
        current_hunk: usize,
    ) -> Option<(usize, HunkStart)> {
        let mut target_idx = current_hunk.saturating_add(1);
        while target_idx < starts.len() {
            if let Some(start) = starts[target_idx] {
                return Some((target_idx, start));
            }
            target_idx += 1;
        }
        None
    }

    fn first_hunk_from_starts(&self, starts: &[Option<HunkStart>]) -> Option<(usize, HunkStart)> {
        starts
            .iter()
            .enumerate()
            .find_map(|(idx, start)| start.map(|s| (idx, s)))
    }

    fn last_hunk_from_starts(&self, starts: &[Option<HunkStart>]) -> Option<(usize, HunkStart)> {
        starts
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, start)| start.map(|s| (idx, s)))
    }

    fn unified_hunk_fallback(&self, starts: &[Option<HunkStart>]) -> Option<(usize, HunkStart)> {
        let mut only: Option<(usize, HunkStart)> = None;
        for (idx, start) in starts.iter().enumerate() {
            if let Some(start) = start {
                if only.is_some() {
                    return None;
                }
                only = Some((idx, *start));
            }
        }
        only
    }

    fn prev_hunk_from_starts(&self, starts: &[Option<HunkStart>]) -> Option<(usize, HunkStart)> {
        starts
            .iter()
            .enumerate()
            .filter_map(|(idx, start)| start.map(|s| (idx, s)))
            .filter(|&(_, start)| start.idx < self.scroll_offset)
            .last()
    }

    fn prev_hunk_from_index(
        &self,
        starts: &[Option<HunkStart>],
        current_hunk: usize,
    ) -> Option<(usize, HunkStart)> {
        if starts.is_empty() {
            return None;
        }
        let mut idx = current_hunk.min(starts.len() - 1);
        while idx > 0 {
            idx -= 1;
            if let Some(start) = starts[idx] {
                return Some((idx, start));
            }
        }
        None
    }

    fn current_hunk_from_bounds(&self, bounds: &[Option<HunkBounds>]) -> Option<usize> {
        bounds.iter().enumerate().rev().find_map(|(idx, bound)| {
            bound.and_then(|b| (b.start.idx <= self.scroll_offset).then_some(idx))
        })
    }

    fn first_available_hunk(bounds: &[Option<HunkBounds>]) -> Option<(usize, HunkBounds)> {
        bounds
            .iter()
            .enumerate()
            .find_map(|(idx, bound)| bound.map(|b| (idx, b)))
    }

    pub(super) fn set_cursor_for_current_scroll(&mut self) {
        let view = self.current_view_with_frame(AnimationFrame::Idle);
        let mut display_idx = 0usize;
        let mut cursor_line = None;

        for line in &view {
            let visible = match self.view_mode {
                ViewMode::Evolution => {
                    !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                }
                _ => true,
            };
            if !visible {
                continue;
            }
            if display_idx >= self.scroll_offset {
                cursor_line = Some(line);
                break;
            }
            display_idx += 1;
        }

        if let Some(line) = cursor_line {
            if let Some(hidx) = line.hunk_index {
                self.multi_diff
                    .current_navigator()
                    .set_cursor_hunk(hidx, Some(line.change_id));
            } else {
                self.multi_diff
                    .current_navigator()
                    .set_cursor_change(Some(line.change_id));
            }
        } else {
            self.multi_diff.current_navigator().clear_cursor_change();
        }
    }

    /// Scroll to the next hunk (no-step mode)
    pub fn next_hunk_scroll(&mut self) {
        self.clear_blame_hunk_hint();
        let auto_center = self.auto_center;
        let (current_hunk, cursor_set) = {
            let state = self.multi_diff.current_navigator().state();
            (state.current_hunk, state.cursor_change.is_some())
        };
        let in_hunk_scope = self
            .multi_diff
            .current_navigator()
            .state()
            .last_nav_was_hunk;
        let use_cursor = auto_center && cursor_set && in_hunk_scope;
        let inclusive = in_hunk_scope;
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_starts, new_starts) = self.compute_hunk_starts_split();
                let effective: Vec<Option<HunkStart>> = old_starts
                    .into_iter()
                    .zip(new_starts)
                    .map(|(old, new)| self.pick_split_start(old, new))
                    .collect();
                let mut target = if use_cursor && current_hunk < effective.len() {
                    self.next_hunk_from_index(&effective, current_hunk)
                } else {
                    self.next_hunk_from_starts(&effective, inclusive)
                };
                if target.is_none() {
                    target = self.unified_hunk_fallback(&effective);
                }
                if target.is_none() && matches!(self.hunk_wrap, HunkWrapMode::Hunk) {
                    target = self.first_hunk_from_starts(&effective);
                }
                target
            }
            _ => {
                let hunk_starts = self.compute_hunk_starts_unified();
                let mut target = if use_cursor && current_hunk < hunk_starts.len() {
                    self.next_hunk_from_index(&hunk_starts, current_hunk)
                } else {
                    self.next_hunk_from_starts(&hunk_starts, inclusive)
                };
                if target.is_none() {
                    target = self.unified_hunk_fallback(&hunk_starts);
                }
                if target.is_none() && matches!(self.hunk_wrap, HunkWrapMode::Hunk) {
                    target = self.first_hunk_from_starts(&hunk_starts);
                }
                target
            }
        };

        if let Some((hidx, start)) = target {
            self.scroll_offset = start.idx;
            self.centered_once = false;
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, start.change_id);
            self.multi_diff.current_navigator().set_hunk_scope(true);
            if self.auto_center {
                self.needs_scroll_to_active = true;
            }
            self.clear_hunk_edge_hint();
            self.set_blame_hunk_hint();
            self.refresh_blame_toggle_hint();
        } else if matches!(self.hunk_wrap, HunkWrapMode::File) {
            if self.wrap_to_file_hunk(true, false) {
                self.clear_hunk_edge_hint();
            } else {
                self.trigger_hunk_edge_hint(HunkEdge::Last);
            }
        } else {
            self.trigger_hunk_edge_hint(HunkEdge::Last);
        }
    }

    /// Scroll to the previous hunk (no-step mode)
    pub fn prev_hunk_scroll(&mut self) {
        self.clear_blame_hunk_hint();
        let auto_center = self.auto_center;
        let (current_hunk, cursor_set) = {
            let state = self.multi_diff.current_navigator().state();
            (state.current_hunk, state.cursor_change.is_some())
        };
        let in_hunk_scope = self
            .multi_diff
            .current_navigator()
            .state()
            .last_nav_was_hunk;
        let use_cursor = auto_center && cursor_set && in_hunk_scope;
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_starts, new_starts) = self.compute_hunk_starts_split();
                let effective: Vec<Option<HunkStart>> = old_starts
                    .into_iter()
                    .zip(new_starts)
                    .map(|(old, new)| self.pick_split_start(old, new))
                    .collect();
                let mut target = if use_cursor && current_hunk < effective.len() {
                    self.prev_hunk_from_index(&effective, current_hunk)
                } else {
                    self.prev_hunk_from_starts(&effective)
                };
                if target.is_none() {
                    target = self.unified_hunk_fallback(&effective);
                }
                if target.is_none() && matches!(self.hunk_wrap, HunkWrapMode::Hunk) {
                    target = self.last_hunk_from_starts(&effective);
                }
                target
            }
            _ => {
                let hunk_starts = self.compute_hunk_starts_unified();
                let mut target = if use_cursor && current_hunk < hunk_starts.len() {
                    self.prev_hunk_from_index(&hunk_starts, current_hunk)
                } else {
                    self.prev_hunk_from_starts(&hunk_starts)
                };
                if target.is_none() {
                    target = self.unified_hunk_fallback(&hunk_starts);
                }
                if target.is_none() && matches!(self.hunk_wrap, HunkWrapMode::Hunk) {
                    target = self.last_hunk_from_starts(&hunk_starts);
                }
                target
            }
        };

        if let Some((hidx, start)) = target {
            self.scroll_offset = start.idx;
            self.centered_once = false;
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, start.change_id);
            self.multi_diff.current_navigator().set_hunk_scope(true);
            if self.auto_center {
                self.needs_scroll_to_active = true;
            }
            self.clear_hunk_edge_hint();
            self.set_blame_hunk_hint();
            self.refresh_blame_toggle_hint();
        } else if matches!(self.hunk_wrap, HunkWrapMode::File) {
            if self.wrap_to_file_hunk(false, false) {
                self.clear_hunk_edge_hint();
            } else {
                self.trigger_hunk_edge_hint(HunkEdge::First);
            }
        } else {
            self.trigger_hunk_edge_hint(HunkEdge::First);
        }
    }

    /// Move to the next hunk (group of related changes)
    pub fn next_hunk(&mut self) {
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        if self.multi_diff.current_navigator().next_hunk() {
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
            self.clear_hunk_edge_hint();
            self.set_blame_hunk_hint();
            self.refresh_blame_toggle_hint();
        } else {
            match self.hunk_wrap {
                HunkWrapMode::Hunk => {
                    let total = self.multi_diff.current_navigator().state().total_hunks;
                    if total > 0 {
                        self.goto_hunk_index(0);
                        self.clear_hunk_edge_hint();
                    } else {
                        self.trigger_hunk_edge_hint(HunkEdge::Last);
                    }
                }
                HunkWrapMode::File => {
                    if self.wrap_to_file_hunk(true, true) {
                        self.clear_hunk_edge_hint();
                    } else {
                        self.trigger_hunk_edge_hint(HunkEdge::Last);
                    }
                }
                HunkWrapMode::None => {
                    self.trigger_hunk_edge_hint(HunkEdge::Last);
                }
            }
        }
    }

    /// Move to the previous hunk (group of related changes)
    pub fn prev_hunk(&mut self) {
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        if self.multi_diff.current_navigator().prev_hunk() {
            if self.animation_enabled {
                self.start_animation();
            } else {
                self.clear_active_on_next_render = true;
            }
            self.needs_scroll_to_active = true;
            self.clear_hunk_edge_hint();
            self.set_blame_hunk_hint();
            self.refresh_blame_toggle_hint();
        } else {
            match self.hunk_wrap {
                HunkWrapMode::Hunk => {
                    let total = self.multi_diff.current_navigator().state().total_hunks;
                    if total > 0 {
                        self.goto_hunk_index(total.saturating_sub(1));
                        self.clear_hunk_edge_hint();
                    } else {
                        self.trigger_hunk_edge_hint(HunkEdge::First);
                    }
                }
                HunkWrapMode::File => {
                    if self.wrap_to_file_hunk(false, true) {
                        self.clear_hunk_edge_hint();
                    } else {
                        self.trigger_hunk_edge_hint(HunkEdge::First);
                    }
                }
                HunkWrapMode::None => {
                    self.trigger_hunk_edge_hint(HunkEdge::First);
                }
            }
        }
    }

    /// Get current hunk info (current hunk index, total hunks)
    pub fn hunk_info(&mut self) -> (usize, usize) {
        let state = self.multi_diff.current_navigator().state();
        (state.current_hunk + 1, state.total_hunks) // 1-indexed for display
    }

    pub fn hunk_step_info(&mut self) -> Option<(usize, usize)> {
        let nav = self.multi_diff.current_navigator();
        let state = nav.state();
        let hunk = nav.current_hunk()?;
        let total = hunk.change_ids.len();
        if total == 0 {
            return None;
        }
        let mut applied = 0usize;
        for id in &hunk.change_ids {
            if state.applied_changes.contains(id) {
                applied += 1;
            }
        }
        Some((applied, total))
    }

    pub fn pending_insert_only_in_current_hunk(&mut self) -> usize {
        let nav = self.multi_diff.current_navigator();
        let state = nav.state();
        let hunk = match nav.current_hunk() {
            Some(hunk) => hunk,
            None => return 0,
        };

        let cursor_id = state
            .cursor_change
            .or(state.active_change)
            .or_else(|| state.applied_changes.last().copied());
        let cursor_id = match cursor_id {
            Some(id) => id,
            None => return 0,
        };
        let cursor_idx = match hunk.change_ids.iter().position(|id| *id == cursor_id) {
            Some(idx) => idx,
            None => return 0,
        };
        let get_change = |id| nav.diff().changes.iter().find(|c| c.id == id);
        let is_insert_only = |change: &oyo_core::Change| {
            change
                .spans
                .iter()
                .all(|span| span.kind == ChangeKind::Insert)
        };
        let cursor_change = match get_change(cursor_id) {
            Some(change) => change,
            None => return 0,
        };
        if !is_insert_only(cursor_change) {
            return 0;
        }

        let mut pending = 0usize;
        for change_id in hunk.change_ids.iter().skip(cursor_idx + 1) {
            let change = match get_change(*change_id) {
                Some(change) => change,
                None => continue,
            };
            if !is_insert_only(change) {
                break;
            }
            if state.applied_changes.contains(change_id) {
                continue;
            }
            pending += 1;
        }

        pending
    }

    /// Jump to first change of current hunk
    pub fn goto_hunk_start(&mut self) {
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        if self.multi_diff.current_navigator().goto_hunk_start() {
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
            self.set_blame_hunk_hint();
            self.refresh_blame_toggle_hint();
        }
    }

    /// Jump to the start of the current hunk (no-step mode)
    pub fn goto_hunk_start_scroll(&mut self) {
        self.clear_blame_hunk_hint();
        let (current_hunk, in_hunk_scope) = {
            let state = self.multi_diff.current_navigator().state();
            (
                state.current_hunk,
                state.last_nav_was_hunk && state.cursor_change.is_some(),
            )
        };
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_bounds, new_bounds) = self.compute_hunk_bounds_split();
                let effective: Vec<Option<HunkBounds>> = old_bounds
                    .into_iter()
                    .zip(new_bounds)
                    .map(|(old, new)| self.pick_split_bounds(old, new))
                    .collect();
                let hidx = if in_hunk_scope {
                    Some(current_hunk)
                } else {
                    self.current_hunk_from_bounds(&effective)
                };
                hidx.and_then(|idx| effective.get(idx).copied().flatten().map(|b| (idx, b)))
                    .or_else(|| Self::first_available_hunk(&effective))
            }
            _ => {
                let bounds = self.compute_hunk_bounds_unified();
                let hidx = if in_hunk_scope {
                    Some(current_hunk)
                } else {
                    self.current_hunk_from_bounds(&bounds)
                };
                hidx.and_then(|idx| bounds.get(idx).copied().flatten().map(|b| (idx, b)))
                    .or_else(|| Self::first_available_hunk(&bounds))
            }
        };

        if let Some((hidx, bound)) = target {
            self.scroll_offset = bound.start.idx;
            self.centered_once = false;
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, bound.start.change_id);
            self.multi_diff.current_navigator().set_hunk_scope(true);
            if self.auto_center {
                self.needs_scroll_to_active = true;
            }
            self.set_blame_hunk_hint();
            self.refresh_blame_toggle_hint();
        }
    }

    /// Jump to last change of current hunk
    pub fn goto_hunk_end(&mut self) {
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        if self.multi_diff.current_navigator().goto_hunk_end() {
            if self.animation_enabled {
                self.start_animation();
            }
            self.needs_scroll_to_active = true;
            self.set_blame_hunk_hint();
            self.refresh_blame_toggle_hint();
        }
    }

    /// Jump to the end of the current hunk (no-step mode)
    pub fn goto_hunk_end_scroll(&mut self) {
        self.clear_blame_hunk_hint();
        let (current_hunk, in_hunk_scope) = {
            let state = self.multi_diff.current_navigator().state();
            (
                state.current_hunk,
                state.last_nav_was_hunk && state.cursor_change.is_some(),
            )
        };
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_bounds, new_bounds) = self.compute_hunk_bounds_split();
                let effective: Vec<Option<HunkBounds>> = old_bounds
                    .into_iter()
                    .zip(new_bounds)
                    .map(|(old, new)| self.pick_split_bounds(old, new))
                    .collect();
                let hidx = if in_hunk_scope {
                    Some(current_hunk)
                } else {
                    self.current_hunk_from_bounds(&effective)
                };
                hidx.and_then(|idx| effective.get(idx).copied().flatten().map(|b| (idx, b)))
                    .or_else(|| Self::first_available_hunk(&effective))
            }
            _ => {
                let bounds = self.compute_hunk_bounds_unified();
                let hidx = if in_hunk_scope {
                    Some(current_hunk)
                } else {
                    self.current_hunk_from_bounds(&bounds)
                };
                hidx.and_then(|idx| bounds.get(idx).copied().flatten().map(|b| (idx, b)))
                    .or_else(|| Self::first_available_hunk(&bounds))
            }
        };

        if let Some((hidx, bound)) = target {
            self.scroll_offset = bound.end.idx;
            self.centered_once = false;
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, bound.end.change_id);
            self.multi_diff.current_navigator().set_hunk_scope(true);
            if self.auto_center {
                self.needs_scroll_to_active = true;
            }
            self.set_blame_hunk_hint();
            self.refresh_blame_toggle_hint();
        }
    }

    /// Enter no-step mode without changing scroll position.
    pub fn enter_no_step_mode(&mut self) {
        // Evolution mode requires stepping, so switch to Unified view
        if self.view_mode == ViewMode::Evolution {
            self.view_mode = ViewMode::UnifiedPane;
        }

        self.peek_state = None;
        self.multi_diff.current_navigator().goto_end();
        self.multi_diff.current_navigator().clear_active_change();
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.needs_scroll_to_active = false;
        let index = self.multi_diff.selected_index;
        if !self.restore_no_step_state_snapshot(index) {
            if self.no_step_auto_jump_on_enter && !self.no_step_visited[index] {
                self.goto_hunk_index_scroll(0);
            } else {
                self.set_cursor_for_current_scroll();
                self.multi_diff.current_navigator().set_hunk_scope(false);
            }
        }
        self.no_step_visited[index] = true;
    }

    pub fn toggle_stepping(&mut self) {
        let current_index = self.multi_diff.selected_index;
        if self.stepping {
            // Turning OFF stepping: snapshot state and scroll, then enter no-step.
            self.save_scroll_position_for(current_index);
            self.save_step_state_snapshot(current_index);
            self.step_peek_state = self.peek_state.take();
            self.step_view_mode = self.view_mode;
            self.stepping = false;
            self.clear_step_edge_hint();
            self.clear_hunk_edge_hint();
            self.clear_blame_step_hint();
            self.clear_blame_hunk_hint();
            if !self.no_step_visited[current_index] {
                self.scroll_offsets_no_step[current_index] = self.scroll_offset;
                self.horizontal_scrolls_no_step[current_index] = self.horizontal_scroll;
            }
            self.restore_scroll_position_for(current_index);
            self.enter_no_step_mode();
        } else {
            // Turning ON stepping: restore snapshot and scroll.
            self.save_no_step_state_snapshot(current_index);
            self.save_scroll_position_for(current_index);
            self.stepping = true;
            self.clear_step_edge_hint();
            self.clear_hunk_edge_hint();
            self.clear_blame_step_hint();
            self.clear_blame_hunk_hint();
            self.peek_state = self.step_peek_state.take();
            self.view_mode = self.step_view_mode;
            if !self.restore_step_state_snapshot(current_index) {
                self.goto_start();
            }
            self.restore_scroll_position_for(current_index);
            self.animation_phase = AnimationPhase::Idle;
            self.animation_progress = 1.0;
            self.needs_scroll_to_active = false;
        }
    }

    pub fn goto_start(&mut self) {
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        if !self.stepping {
            self.scroll_offset = 0;
            self.centered_once = false;
            self.needs_scroll_to_active = false;
            self.multi_diff.current_navigator().clear_cursor_change();
            self.multi_diff.current_navigator().set_hunk_scope(false);
            return;
        }
        self.multi_diff.current_navigator().goto_start();
        self.scroll_offset = 0;
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
        self.refresh_blame_toggle_hint();
    }

    pub fn goto_end(&mut self) {
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        if !self.stepping {
            self.scroll_offset = usize::MAX;
            self.centered_once = false;
            self.needs_scroll_to_active = false;
            self.multi_diff.current_navigator().clear_cursor_change();
            self.multi_diff.current_navigator().set_hunk_scope(false);
            return;
        }
        self.multi_diff.current_navigator().goto_end();
        self.scroll_offset = usize::MAX; // Will be clamped to bottom
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        // Don't set needs_scroll_to_active - we want to stay at bottom
        self.refresh_blame_toggle_hint();
    }

    pub fn goto_first_step(&mut self) {
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        self.multi_diff.current_navigator().goto(1);
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
        self.refresh_blame_toggle_hint();
    }

    pub fn goto_last_step(&mut self) {
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        self.multi_diff.current_navigator().goto_end();
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
        self.refresh_blame_toggle_hint();
    }

    pub(super) fn goto_step_number(&mut self, step_number: usize) {
        if !self.stepping {
            return;
        }
        let total_steps = self.multi_diff.current_navigator().state().total_steps;
        if total_steps == 0 {
            return;
        }
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        let clamped = step_number.max(1).min(total_steps);
        let target_step = clamped.saturating_sub(1);
        self.multi_diff.current_navigator().goto(target_step);
        self.animation_phase = AnimationPhase::Idle;
        self.animation_progress = 1.0;
        self.centered_once = false;
        self.needs_scroll_to_active = true;
        self.refresh_blame_toggle_hint();
    }

    pub(super) fn goto_hunk_number(&mut self, hunk_number: usize) {
        let total_hunks = self.multi_diff.current_navigator().state().total_hunks;
        if total_hunks == 0 {
            return;
        }
        let clamped = hunk_number.max(1).min(total_hunks);
        let hunk_idx = clamped - 1;
        if self.stepping {
            self.goto_hunk_index(hunk_idx);
        } else {
            self.goto_hunk_index_scroll(hunk_idx);
        }
    }

    pub(super) fn goto_hunk_index(&mut self, hunk_idx: usize) {
        self.clear_peek();
        self.clear_blame_step_hint();
        self.clear_blame_hunk_hint();
        self.multi_diff.current_navigator().goto_hunk(hunk_idx);
        if self.animation_enabled {
            self.start_animation();
        } else {
            self.clear_active_on_next_render = true;
        }
        self.needs_scroll_to_active = true;
        self.set_blame_hunk_hint();
        self.refresh_blame_toggle_hint();
    }

    pub fn goto_first_hunk_scroll(&mut self) {
        let total = self.multi_diff.current_navigator().state().total_hunks;
        if total == 0 {
            return;
        }
        self.goto_hunk_index_scroll(0);
    }

    pub fn goto_last_hunk_scroll(&mut self) {
        let total = self.multi_diff.current_navigator().state().total_hunks;
        if total == 0 {
            return;
        }
        self.goto_hunk_index_scroll(total.saturating_sub(1));
    }

    pub(super) fn goto_hunk_index_scroll(&mut self, hunk_idx: usize) {
        self.clear_blame_hunk_hint();
        let target = match self.view_mode {
            ViewMode::Split => {
                let (old_bounds, new_bounds) = self.compute_hunk_bounds_split();
                let old = old_bounds.get(hunk_idx).copied().flatten();
                let new = new_bounds.get(hunk_idx).copied().flatten();
                self.pick_split_bounds(old, new).map(|b| (hunk_idx, b))
            }
            _ => {
                let bounds = self.compute_hunk_bounds_unified();
                bounds
                    .get(hunk_idx)
                    .copied()
                    .flatten()
                    .map(|b| (hunk_idx, b))
            }
        };

        if let Some((hidx, bound)) = target {
            self.scroll_offset = bound.start.idx;
            self.centered_once = false;
            self.multi_diff
                .current_navigator()
                .set_cursor_hunk(hidx, bound.start.change_id);
            self.multi_diff.current_navigator().set_hunk_scope(true);
            if self.auto_center {
                self.needs_scroll_to_active = true;
            }
            self.set_blame_hunk_hint();
            self.refresh_blame_toggle_hint();
        }
    }

    pub(super) fn goto_line_number(&mut self, line_number: usize) {
        self.clear_peek();
        let view = self.current_view_with_frame(AnimationFrame::Idle);
        let target_idx = match self.view_mode {
            ViewMode::Split => {
                let mut old_idx = 0usize;
                let mut new_idx = 0usize;
                let mut old_last = None;
                let mut new_last = None;
                let mut old_max_line = 0usize;
                let mut new_max_line = 0usize;
                let mut old_match = None;
                let mut new_match = None;
                for line in &view {
                    let fold_line = is_fold_line(line);
                    if let Some(old_line) = line.old_line {
                        old_max_line = old_max_line.max(old_line);
                        if old_line == line_number {
                            old_match = Some(old_idx);
                        }
                        old_idx += 1;
                        old_last = Some(old_idx - 1);
                    } else if fold_line {
                        old_idx += 1;
                        old_last = Some(old_idx - 1);
                    }
                    if let Some(new_line) = line.new_line {
                        new_max_line = new_max_line.max(new_line);
                        if new_line == line_number {
                            new_match = Some(new_idx);
                        }
                        new_idx += 1;
                        new_last = Some(new_idx - 1);
                    } else if fold_line {
                        new_idx += 1;
                        new_last = Some(new_idx - 1);
                    }
                }
                if line_number == 0 {
                    let first_old = if old_idx > 0 { Some(0) } else { None };
                    let first_new = if new_idx > 0 { Some(0) } else { None };
                    self.pick_split_index(first_old, first_new)
                } else {
                    let max_line = old_max_line.max(new_max_line);
                    if max_line > 0 && line_number > max_line {
                        if old_max_line > new_max_line {
                            old_last
                        } else if new_max_line > old_max_line {
                            new_last
                        } else {
                            self.pick_split_index(old_last, new_last)
                        }
                    } else {
                        self.pick_split_index(old_match, new_match)
                    }
                }
            }
            ViewMode::Evolution => {
                let mut target = None;
                let mut last_idx = None;
                let mut max_line = 0usize;
                for (display_idx, line) in view
                    .iter()
                    .filter(|line| {
                        !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete)
                    })
                    .enumerate()
                {
                    let line_num = line.new_line.or(line.old_line);
                    if let Some(num) = line_num {
                        max_line = max_line.max(num);
                    }
                    if line_num == Some(line_number) {
                        target = Some(display_idx);
                        break;
                    }
                    last_idx = Some(display_idx);
                }
                if line_number == 0 {
                    last_idx.map(|_| 0)
                } else if max_line > 0 && line_number > max_line {
                    last_idx
                } else {
                    target
                }
            }
            _ => {
                let mut target = None;
                let mut last_idx = None;
                let mut max_line = 0usize;
                for (display_idx, line) in view.iter().enumerate() {
                    let line_num = line.old_line.or(line.new_line);
                    if let Some(num) = line_num {
                        max_line = max_line.max(num);
                    }
                    if line_num == Some(line_number) {
                        target = Some(display_idx);
                        break;
                    }
                    last_idx = Some(display_idx);
                }
                if line_number == 0 {
                    last_idx.map(|_| 0)
                } else if max_line > 0 && line_number > max_line {
                    last_idx
                } else {
                    target
                }
            }
        };

        if let Some(idx) = target_idx {
            let viewport_height = self.last_viewport_height.max(1);
            if self.auto_center {
                let half_viewport = viewport_height / 2;
                self.scroll_offset = idx.saturating_sub(half_viewport);
                self.centered_once = true;
            } else {
                self.scroll_offset = idx;
                self.centered_once = false;
            }
            self.needs_scroll_to_active = false;
            self.multi_diff.current_navigator().set_hunk_scope(false);
            if !self.stepping {
                self.set_cursor_for_current_scroll();
            }
        }
    }

    pub fn toggle_view_mode(&mut self) {
        let allow_blame = self.blame_enabled;
        if !self.stepping {
            // In no-step mode, skip Evolution view as it requires stepping
            self.view_mode = match self.view_mode {
                ViewMode::UnifiedPane => ViewMode::Split,
                ViewMode::Split => {
                    if allow_blame {
                        ViewMode::Blame
                    } else {
                        ViewMode::UnifiedPane
                    }
                }
                ViewMode::Blame => ViewMode::UnifiedPane,
                ViewMode::Evolution => ViewMode::UnifiedPane,
            };
        } else if allow_blame {
            self.view_mode = self.view_mode.next();
        } else {
            self.view_mode = match self.view_mode {
                ViewMode::UnifiedPane => ViewMode::Split,
                ViewMode::Split => ViewMode::Evolution,
                ViewMode::Evolution => ViewMode::UnifiedPane,
                ViewMode::Blame => ViewMode::UnifiedPane,
            };
        }
    }

    pub fn set_view_mode(&mut self, target: ViewMode) {
        if target == ViewMode::Blame && !self.blame_enabled {
            return;
        }
        if target == ViewMode::Evolution && !self.stepping {
            self.step_view_mode = ViewMode::Evolution;
            self.toggle_stepping();
            return;
        }

        if !self.stepping {
            self.view_mode = match target {
                ViewMode::Evolution => ViewMode::UnifiedPane,
                other => other,
            };
            self.step_view_mode = self.view_mode;
            return;
        }

        self.view_mode = target;
    }

    pub fn toggle_view_mode_reverse(&mut self) {
        let allow_blame = self.blame_enabled;
        if !self.stepping {
            // In no-step mode, skip Evolution view as it requires stepping
            self.view_mode = match self.view_mode {
                ViewMode::UnifiedPane => {
                    if allow_blame {
                        ViewMode::Blame
                    } else {
                        ViewMode::Split
                    }
                }
                ViewMode::Split => ViewMode::UnifiedPane,
                ViewMode::Blame => ViewMode::Split,
                ViewMode::Evolution => ViewMode::UnifiedPane,
            };
        } else if allow_blame {
            self.view_mode = self.view_mode.prev();
        } else {
            self.view_mode = match self.view_mode {
                ViewMode::UnifiedPane => ViewMode::Evolution,
                ViewMode::Split => ViewMode::UnifiedPane,
                ViewMode::Evolution => ViewMode::Split,
                ViewMode::Blame => ViewMode::UnifiedPane,
            };
        }
    }
}
