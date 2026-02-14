//! Step-through navigation for diffs

use crate::change::{Change, ChangeKind, ChangeSpan};
use crate::diff::DiffResult;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Direction of the last step action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StepDirection {
    #[default]
    None,
    Forward,
    Backward,
}

/// Animation frame for phase-aware rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationFrame {
    #[default]
    Idle,
    FadeOut,
    FadeIn,
}

/// The current state of stepping through a diff
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepState {
    /// Current step index (0 = initial state, 1 = after first change applied, etc.)
    pub current_step: usize,
    /// Total number of steps (number of significant changes + 1 for initial state)
    pub total_steps: usize,
    /// IDs of changes that have been applied up to current step
    pub applied_changes: Vec<usize>,
    /// Fast membership for applied changes (kept in sync with applied_changes)
    #[serde(skip, default)]
    applied_changes_set: FxHashSet<usize>,
    /// ID of the change being highlighted/animated at current step
    pub active_change: Option<usize>,
    /// Cursor change used for non-stepping navigation (does not imply animation)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_change: Option<usize>,
    /// Hunk currently being animated (distinct from cursor position in current_hunk)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animating_hunk: Option<usize>,
    /// Direction of the last step action
    pub step_direction: StepDirection,
    /// Current hunk index (0-based)
    pub current_hunk: usize,
    /// Total number of hunks
    pub total_hunks: usize,
    /// True if the last navigation was a hunk navigation (for extent marker display)
    #[serde(default)]
    pub last_nav_was_hunk: bool,
    /// True after hunkdown (full preview mode), cleared on first step
    #[serde(default)]
    pub hunk_preview_mode: bool,
    /// True if preview was entered via hunkup (backward navigation)
    #[serde(default)]
    pub preview_from_backward: bool,
    /// Show hunk extent markers while stepping (set by UI)
    #[serde(default)]
    pub show_hunk_extent_while_stepping: bool,
}

impl StepState {
    pub fn new(total_changes: usize, total_hunks: usize) -> Self {
        Self {
            current_step: 0,
            total_steps: total_changes + 1, // +1 for initial state
            applied_changes: Vec::new(),
            applied_changes_set: FxHashSet::default(),
            active_change: None,
            cursor_change: None,
            animating_hunk: None,
            step_direction: StepDirection::None,
            current_hunk: 0,
            total_hunks,
            last_nav_was_hunk: false,
            hunk_preview_mode: false,
            preview_from_backward: false,
            show_hunk_extent_while_stepping: false,
        }
    }

    /// Check if we're at the initial state (no changes applied)
    pub fn is_at_start(&self) -> bool {
        self.current_step == 0
    }

    /// Check if we're at the final state (all changes applied)
    pub fn is_at_end(&self) -> bool {
        self.current_step >= self.total_steps - 1
    }

    /// Get progress as a percentage
    pub fn progress(&self) -> f64 {
        if self.total_steps <= 1 {
            return 100.0;
        }
        (self.current_step as f64 / (self.total_steps - 1) as f64) * 100.0
    }

    fn rebuild_applied_set(&mut self) {
        self.applied_changes_set = self.applied_changes.iter().copied().collect();
    }

    pub fn is_applied(&self, change_id: usize) -> bool {
        self.applied_changes_set.contains(&change_id)
    }

    fn push_applied(&mut self, change_id: usize) {
        if self.applied_changes_set.insert(change_id) {
            self.applied_changes.push(change_id);
        }
    }

    fn pop_applied(&mut self) -> Option<usize> {
        let change_id = self.applied_changes.pop()?;
        self.applied_changes_set.remove(&change_id);
        Some(change_id)
    }

    fn truncate_applied_to(&mut self, new_len: usize) -> usize {
        let old_len = self.applied_changes.len();
        if new_len >= old_len {
            return 0;
        }
        for change_id in &self.applied_changes[new_len..] {
            self.applied_changes_set.remove(change_id);
        }
        self.applied_changes.truncate(new_len);
        old_len - new_len
    }

    fn clear_applied(&mut self) {
        self.applied_changes.clear();
        self.applied_changes_set.clear();
    }
}

/// Navigator for stepping through diff changes
pub struct DiffNavigator {
    /// The diff result we're navigating
    diff: DiffResult,
    /// Current step state
    state: StepState,
    /// Original content (for reconstructing views)
    old_content: Arc<str>,
    /// New content (for reconstructing views)
    new_content: Arc<str>,
    /// Mapping from change ID to hunk index
    change_to_hunk: Vec<Option<usize>>,
    /// Exact mapping from change ID to hunk index (no context padding)
    change_id_to_hunk_exact: Vec<Option<usize>>,
    /// Mapping from change ID to change index in the diff
    change_to_index: Vec<Option<usize>>,
    /// Mapping from change ID to step index (significant_changes order)
    change_to_step_index: Vec<Option<usize>>,
    /// Skip building full lookup maps for large diffs
    lazy_maps: bool,
    /// Step range (start index, length) per hunk for fast hunk progress
    hunk_step_ranges: Vec<Option<HunkStepRange>>,
    /// Cached change index range per hunk (inclusive), for O(1) scope checks
    hunk_change_ranges: Vec<Option<(usize, usize)>>,
    /// Exact change index range per hunk (inclusive), no context padding
    hunk_change_ranges_exact: Vec<Option<(usize, usize)>>,
    /// Cached display indices for evolution view (None for hidden deletions)
    evo_visible_index: Option<Vec<Option<usize>>>,
    /// Cached visible line count for evolution view
    evo_visible_len: Option<usize>,
    /// Cached list of visible change indices (display index -> change index)
    evo_display_to_change: Option<Vec<usize>>,
    /// Cached nearest visible change index per change (for evo)
    evo_nearest_visible: Option<Vec<Option<usize>>>,
}

const LARGE_CONTEXT_PAD: usize = 3;

#[derive(Debug, Clone, Copy)]
struct HunkStepRange {
    start: usize,
    len: usize,
}

impl DiffNavigator {
    pub fn new(
        diff: DiffResult,
        old_content: Arc<str>,
        new_content: Arc<str>,
        lazy_maps: bool,
    ) -> Self {
        let total_changes = diff.significant_changes.len();
        let total_hunks = diff.hunks.len();

        // Build change ID lookup maps
        let mut change_to_hunk = vec![None; diff.changes.len()];
        let mut max_change_id = 0usize;
        for change in diff.changes.iter() {
            max_change_id = max_change_id.max(change.id);
        }
        let mut change_to_index = vec![None; max_change_id.saturating_add(1)];
        for (idx, change) in diff.changes.iter().enumerate() {
            if let Some(slot) = change_to_index.get_mut(change.id) {
                *slot = Some(idx);
            }
        }
        let mut change_to_step_index = vec![None; max_change_id.saturating_add(1)];
        for (idx, change_id) in diff.significant_changes.iter().enumerate() {
            if let Some(slot) = change_to_step_index.get_mut(*change_id) {
                *slot = Some(idx);
            }
        }

        let mut change_id_to_hunk_exact = vec![None; max_change_id.saturating_add(1)];
        let mut hunk_change_ranges = vec![None; diff.hunks.len()];
        let mut hunk_change_ranges_exact = vec![None; diff.hunks.len()];
        for (hunk_idx, hunk) in diff.hunks.iter().enumerate() {
            let mut min_idx = usize::MAX;
            let mut max_idx = 0usize;
            for &change_id in &hunk.change_ids {
                if let Some(slot) = change_id_to_hunk_exact.get_mut(change_id) {
                    *slot = Some(hunk.id);
                }
                if let Some(Some(idx)) = change_to_index.get(change_id) {
                    min_idx = min_idx.min(*idx);
                    max_idx = max_idx.max(*idx);
                }
            }
            if min_idx == usize::MAX {
                continue;
            }
            hunk_change_ranges_exact[hunk_idx] = Some((min_idx, max_idx));
            let start = if lazy_maps {
                min_idx.saturating_sub(LARGE_CONTEXT_PAD)
            } else {
                min_idx
            };
            let end = if lazy_maps {
                (max_idx + LARGE_CONTEXT_PAD).min(diff.changes.len().saturating_sub(1))
            } else {
                max_idx
            };
            hunk_change_ranges[hunk_idx] = Some((start, end));
            for idx in start..=end {
                if let Some(slot) = change_to_hunk.get_mut(idx) {
                    if slot.is_none() {
                        *slot = Some(hunk_idx);
                    }
                }
            }
        }

        let mut hunk_step_ranges = vec![None; diff.hunks.len()];
        for (hunk_idx, hunk) in diff.hunks.iter().enumerate() {
            if hunk.change_ids.is_empty() {
                continue;
            }
            let mut min = usize::MAX;
            let mut count = 0usize;
            for change_id in &hunk.change_ids {
                if let Some(Some(step_idx)) = change_to_step_index.get(*change_id) {
                    if *step_idx < min {
                        min = *step_idx;
                    }
                    count += 1;
                }
            }
            if count > 0 && min != usize::MAX {
                hunk_step_ranges[hunk_idx] = Some(HunkStepRange {
                    start: min,
                    len: count,
                });
            }
        }

        Self {
            diff,
            state: StepState::new(total_changes, total_hunks),
            old_content,
            new_content,
            change_to_hunk,
            change_id_to_hunk_exact,
            change_to_index,
            change_to_step_index,
            lazy_maps,
            hunk_step_ranges,
            hunk_change_ranges,
            hunk_change_ranges_exact,
            evo_visible_index: None,
            evo_visible_len: None,
            evo_display_to_change: None,
            evo_nearest_visible: None,
        }
    }

    /// Get the current step state
    pub fn state(&self) -> &StepState {
        &self.state
    }

    /// Get mutable access to step state (test-only)
    #[cfg(test)]
    pub fn state_mut(&mut self) -> &mut StepState {
        &mut self.state
    }

    /// Replace the current step state (used to restore stepping mode)
    pub fn set_state(&mut self, state: StepState) -> bool {
        if state.total_steps != self.state.total_steps
            || state.total_hunks != self.state.total_hunks
        {
            return false;
        }
        self.state = state;
        self.state.rebuild_applied_set();
        true
    }

    /// Get the diff result
    pub fn diff(&self) -> &DiffResult {
        &self.diff
    }

    fn change_visible_in_evolution(change: &Change) -> bool {
        let mut has_old = false;
        let mut has_new = false;
        for span in &change.spans {
            match span.kind {
                ChangeKind::Insert => has_new = true,
                ChangeKind::Delete => has_old = true,
                ChangeKind::Replace => {
                    has_old = true;
                    has_new = true;
                }
                ChangeKind::Equal => {}
            }
        }
        !has_old || has_new
    }

    fn change_visible_in_evolution_state(&self, change: &Change) -> bool {
        let applied = self.state.is_applied(change.id);
        let mut has_old = false;
        let mut has_new = false;
        for span in &change.spans {
            match span.kind {
                ChangeKind::Insert => has_new = true,
                ChangeKind::Delete => has_old = true,
                ChangeKind::Replace => {
                    has_old = true;
                    has_new = true;
                }
                ChangeKind::Equal => {}
            }
        }
        if has_old && !has_new {
            return !applied;
        }
        if has_new && !has_old {
            return applied;
        }
        true
    }

    fn ensure_evo_visible_index(&mut self) {
        if self.evo_visible_index.is_some() {
            return;
        }
        let mut mapping = Vec::with_capacity(self.diff.changes.len());
        let mut display_to_change = Vec::new();
        let mut display_idx = 0usize;
        for (idx, change) in self.diff.changes.iter().enumerate() {
            if Self::change_visible_in_evolution(change) {
                mapping.push(Some(display_idx));
                display_to_change.push(idx);
                display_idx += 1;
            } else {
                mapping.push(None);
            }
        }
        self.evo_visible_len = Some(display_idx);
        self.evo_visible_index = Some(mapping);
        self.evo_display_to_change = Some(display_to_change);

        let mut prev_visible = vec![None; self.diff.changes.len()];
        let mut last_visible = None;
        for (idx, change) in self.diff.changes.iter().enumerate() {
            if Self::change_visible_in_evolution(change) {
                last_visible = Some(idx);
            }
            prev_visible[idx] = last_visible;
        }
        let mut next_visible = vec![None; self.diff.changes.len()];
        let mut next = None;
        for (idx, change) in self.diff.changes.iter().enumerate().rev() {
            if Self::change_visible_in_evolution(change) {
                next = Some(idx);
            }
            next_visible[idx] = next;
        }
        let mut nearest = vec![None; self.diff.changes.len()];
        for idx in 0..self.diff.changes.len() {
            match (prev_visible[idx], next_visible[idx]) {
                (Some(prev), Some(next)) => {
                    let prev_dist = idx.saturating_sub(prev);
                    let next_dist = next.saturating_sub(idx);
                    nearest[idx] = if next_dist < prev_dist {
                        Some(next)
                    } else {
                        Some(prev)
                    };
                }
                (Some(prev), None) => nearest[idx] = Some(prev),
                (None, Some(next)) => nearest[idx] = Some(next),
                (None, None) => nearest[idx] = None,
            }
        }
        self.evo_nearest_visible = Some(nearest);
    }

    pub fn evolution_display_index_for_change(&mut self, change_id: usize) -> Option<usize> {
        self.ensure_evo_visible_index();
        let idx = self.change_index_for(change_id)?;
        self.evo_visible_index
            .as_ref()
            .and_then(|mapping| mapping.get(idx).copied().flatten())
    }

    pub fn evolution_display_index_for_change_index(&mut self, change_idx: usize) -> Option<usize> {
        self.ensure_evo_visible_index();
        self.evo_visible_index
            .as_ref()
            .and_then(|mapping| mapping.get(change_idx).copied().flatten())
    }

    pub fn evolution_display_index_or_nearest(&mut self, change_id: usize) -> Option<usize> {
        if let Some(idx) = self.evolution_display_index_for_change(change_id) {
            return Some(idx);
        }
        self.ensure_evo_visible_index();
        let change_idx = self.change_index_for(change_id)?;
        let nearest = self
            .evo_nearest_visible
            .as_ref()
            .and_then(|mapping| mapping.get(change_idx).copied().flatten())?;
        self.evolution_display_index_for_change_index(nearest)
    }

    pub fn evolution_nearest_visible_change_id(&mut self, change_id: usize) -> Option<usize> {
        self.ensure_evo_visible_index();
        let change_idx = self.change_index_for(change_id)?;
        let nearest = self
            .evo_nearest_visible
            .as_ref()
            .and_then(|mapping| mapping.get(change_idx).copied().flatten())?;
        self.diff.changes.get(nearest).map(|change| change.id)
    }

    pub fn evolution_nearest_visible_change_id_dynamic(
        &self,
        change_id: usize,
        max_scan: usize,
    ) -> Option<usize> {
        let idx = self.change_index_for(change_id)?;
        if self.diff.changes.is_empty() {
            return None;
        }
        let mut offset = 0usize;
        while offset <= max_scan {
            if let Some(left) = idx.checked_sub(offset) {
                let change = &self.diff.changes[left];
                if self.change_visible_in_evolution_state(change) {
                    return Some(change.id);
                }
            }
            let right = idx + offset;
            if right < self.diff.changes.len() {
                let change = &self.diff.changes[right];
                if self.change_visible_in_evolution_state(change) {
                    return Some(change.id);
                }
            }
            offset += 1;
        }
        None
    }

    pub fn evolution_visible_len(&mut self) -> usize {
        self.ensure_evo_visible_index();
        self.evo_visible_len.unwrap_or(0)
    }

    pub fn evolution_change_range_for_display(
        &mut self,
        display_idx: usize,
        radius: usize,
    ) -> Option<(usize, usize)> {
        self.ensure_evo_visible_index();
        let visible_len = self.evo_visible_len?;
        if visible_len == 0 {
            return None;
        }
        let display_idx = display_idx.min(visible_len.saturating_sub(1));
        let start_display = display_idx.saturating_sub(radius);
        let end_display = (display_idx + radius).min(visible_len.saturating_sub(1));
        let display_to_change = self.evo_display_to_change.as_ref()?;
        let start_change = *display_to_change.get(start_display)?;
        let end_change = *display_to_change.get(end_display)?;
        Some((start_change, end_change))
    }

    /// Set a non-animated cursor for classic (no-step) navigation.
    pub fn set_cursor_hunk(&mut self, hunk_idx: usize, change_id: Option<usize>) {
        if self.state.total_hunks > 0 {
            self.state.current_hunk = hunk_idx.min(self.state.total_hunks - 1);
        }
        self.state.cursor_change = change_id;
    }

    /// Override the active cursor without changing applied steps.
    pub fn set_cursor_override(&mut self, change_id: Option<usize>) {
        self.state.cursor_change = change_id;
        self.state.active_change = None;
        self.state.step_direction = StepDirection::None;
        self.state.animating_hunk = None;
    }

    /// Set cursor change without altering current hunk.
    pub fn set_cursor_change(&mut self, change_id: Option<usize>) {
        self.state.cursor_change = change_id;
    }

    /// Clear the non-animated cursor.
    pub fn clear_cursor_change(&mut self) {
        self.state.cursor_change = None;
    }

    /// Control hunk scope markers (extent indicators).
    pub fn set_hunk_scope(&mut self, enabled: bool) {
        self.state.last_nav_was_hunk = enabled;
    }

    /// Move to the next step
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> bool {
        // Handle preview mode dissolution on first step
        if self.state.hunk_preview_mode {
            return self.dissolve_preview_for_step_down();
        }

        if self.state.is_at_end() {
            return false;
        }

        let prev_hunk = self.state.current_hunk;

        self.state.step_direction = StepDirection::Forward;
        self.state.animating_hunk = None; // Clear hunk animation for single-step

        // Get the next change to apply
        let change_idx = self.state.current_step;
        if change_idx < self.diff.significant_changes.len() {
            let change_id = self.diff.significant_changes[change_idx];
            self.state.push_applied(change_id);
            self.state.active_change = Some(change_id);

            // Update current hunk
            if let Some(hunk) = self.diff.hunk_for_change(change_id) {
                self.state.current_hunk = hunk.id;
            }
        }

        self.state.current_step += 1;

        // Clear extent markers only when leaving hunk
        if self.state.current_hunk != prev_hunk {
            self.state.last_nav_was_hunk = false;
        }

        true
    }

    /// Dissolve preview mode on step down: keep first change, apply second
    fn dissolve_preview_for_step_down(&mut self) -> bool {
        if self.state.preview_from_backward {
            self.state.hunk_preview_mode = false;
            self.state.preview_from_backward = false;
            return self.next();
        }

        let current_hunk_idx = self.state.current_hunk;
        let hunk_len = self
            .diff
            .hunks
            .get(current_hunk_idx)
            .map(|hunk| hunk.change_ids.len())
            .unwrap_or(0);

        // If hunk has only one change, stepping down exits the hunk
        if hunk_len <= 1 {
            self.state.hunk_preview_mode = false;
            // Let normal next() handle moving to next change/hunk
            return self.next();
        }

        // Keep only first change, unapply the rest
        let (first_change, second_change) = {
            let hunk = &self.diff.hunks[current_hunk_idx];
            (hunk.change_ids[0], hunk.change_ids[1])
        };

        // Remove all changes in this hunk except the first
        self.remove_applied_bulk_for_hunk(current_hunk_idx, Some(first_change));

        // Apply second change
        self.state.push_applied(second_change);

        // Update current_step to reflect actual applied changes
        self.state.current_step = self.state.applied_changes.len();

        self.state.active_change = Some(second_change);
        self.state.step_direction = StepDirection::Forward;
        self.state.hunk_preview_mode = false;
        self.state.preview_from_backward = false;
        self.state.animating_hunk = None;
        // Preserve extent markers - we're still in the hunk scope
        self.state.last_nav_was_hunk = true;

        true
    }

    /// Move to the previous step
    pub fn prev(&mut self) -> bool {
        // Handle preview mode dissolution on first step up: exit hunk entirely
        if self.state.hunk_preview_mode {
            return self.dissolve_preview_for_step_up();
        }

        if self.state.is_at_start() {
            return false;
        }

        let prev_hunk = self.state.current_hunk;

        self.state.step_direction = StepDirection::Backward;
        self.state.animating_hunk = None; // Clear hunk animation for single-step
        self.state.current_step -= 1;

        // Pop the change and set it as active for backward animation
        if let Some(unapplied_change_id) = self.state.pop_applied() {
            self.state.active_change = Some(unapplied_change_id);

            // Update current hunk based on last applied change
            if let Some(&last_applied) = self.state.applied_changes.last() {
                if let Some(hunk) = self.diff.hunk_for_change(last_applied) {
                    self.state.current_hunk = hunk.id;
                }
            } else {
                self.state.current_hunk = 0;
            }
        } else {
            self.state.active_change = None;
        }

        // Reset hunk cursor when at start; animation state cleared by CLI after animation completes
        if self.state.is_at_start() {
            self.state.current_hunk = 0;
        }

        // Clear extent markers when leaving hunk or at step 0
        if self.state.is_at_start() || self.state.current_hunk != prev_hunk {
            self.state.last_nav_was_hunk = false;
        }

        true
    }

    /// Dissolve preview mode on step up: unapply all changes in hunk and exit
    fn dissolve_preview_for_step_up(&mut self) -> bool {
        if self.state.preview_from_backward {
            self.state.hunk_preview_mode = false;
            self.state.preview_from_backward = false;
            return self.prev();
        }

        let current_hunk_idx = self.state.current_hunk;

        // Set animating hunk for backward fade animation
        self.state.animating_hunk = Some(current_hunk_idx);

        // Unapply all changes in this hunk
        self.remove_applied_bulk_for_hunk(current_hunk_idx, None);

        // Update current_step to reflect actual applied changes
        self.state.current_step = self.state.applied_changes.len();

        // Move to previous hunk if possible
        if current_hunk_idx > 0 {
            self.state.current_hunk = current_hunk_idx - 1;
        }

        self.state.step_direction = StepDirection::Backward;
        // Keep cursor logic aligned with the destination hunk (bottom-most applied change)
        self.state.active_change = self.state.applied_changes.last().copied();
        self.state.hunk_preview_mode = false;
        self.state.preview_from_backward = false;
        self.state.last_nav_was_hunk = false; // Exiting hunk

        true
    }

    /// Clear animation state (called after animation completes or one-frame render)
    /// For backward steps, keeps cursor on last applied change (destination)
    pub fn clear_active_change(&mut self) {
        if self.state.step_direction == StepDirection::Backward {
            self.state.active_change = self.state.applied_changes.last().copied();
        } else {
            self.state.active_change = None;
        }
        self.state.animating_hunk = None;
        self.state.step_direction = StepDirection::None;
    }

    fn remove_applied_bulk_for_hunk(&mut self, hunk_idx: usize, keep: Option<usize>) -> usize {
        let (diff, change_to_step_index, state) =
            (&self.diff, &self.change_to_step_index, &mut self.state);
        let Some(hunk) = diff.hunks.get(hunk_idx) else {
            return 0;
        };

        let mut min_index: Option<usize> = None;
        let mut keep_index: Option<usize> = None;

        for &change_id in &hunk.change_ids {
            if Some(change_id) == keep {
                keep_index = change_to_step_index.get(change_id).copied().flatten();
                continue;
            }
            if !state.is_applied(change_id) {
                continue;
            }
            if let Some(step_idx) = change_to_step_index.get(change_id).copied().flatten() {
                min_index = Some(min_index.map_or(step_idx, |min| min.min(step_idx)));
            }
        }

        let new_len = if let Some(keep_id) = keep {
            let Some(step_idx) = keep_index else {
                return 0;
            };
            if !state.is_applied(keep_id) {
                return 0;
            }
            step_idx + 1
        } else {
            let Some(step_idx) = min_index else {
                return 0;
            };
            step_idx
        };

        state.truncate_applied_to(new_len)
    }

    /// Jump to a specific step
    pub fn goto(&mut self, step: usize) {
        let target_step = step.min(self.state.total_steps - 1);

        // Reset to start
        self.state.current_step = 0;
        self.state.clear_applied();
        self.state.active_change = None;
        self.state.cursor_change = None;
        self.state.animating_hunk = None;
        self.state.current_hunk = 0;
        self.state.last_nav_was_hunk = false; // Clear hunk nav flag on goto
        self.state.hunk_preview_mode = false; // Clear preview mode on goto
        self.state.preview_from_backward = false;

        if self.lazy_maps {
            if target_step > 0 {
                let end = target_step.min(self.diff.significant_changes.len());
                self.state.applied_changes = self.diff.significant_changes[..end].to_vec();
                self.state.rebuild_applied_set();
                self.state.current_step = end;
                self.state.active_change = self.state.applied_changes.last().copied();
                self.state.step_direction = StepDirection::Forward;
            } else {
                self.state.step_direction = StepDirection::None;
            }
        } else {
            // Apply changes up to target step
            for _ in 0..target_step {
                self.next();
            }
        }

        // Update which hunk we're in
        self.update_current_hunk();
    }

    /// Go to the start
    pub fn goto_start(&mut self) {
        self.goto(0);
    }

    /// Go to the end
    pub fn goto_end(&mut self) {
        self.goto(self.state.total_steps - 1);
    }

    // ==================== Hunk Navigation ====================

    /// Move to the next hunk, applying ALL changes (full preview mode).
    /// If current hunk is not started, applies all its changes with cursor at top.
    /// If current hunk is partially/fully applied, completes it and moves to next hunk.
    /// Returns true if moved, false if no movement possible
    pub fn next_hunk(&mut self) -> bool {
        if self.diff.hunks.is_empty() {
            return false;
        }

        // Preserve preview mode in case we return false without doing anything
        let was_in_preview = self.state.hunk_preview_mode;

        self.state.step_direction = StepDirection::Forward;
        self.state.hunk_preview_mode = false; // Will be set to true after applying
        self.state.preview_from_backward = false;

        let current_hunk = &self.diff.hunks[self.state.current_hunk];
        let has_applied_in_current = current_hunk
            .change_ids
            .iter()
            .any(|id| self.state.is_applied(*id));

        // If current hunk has no applied changes, apply ALL changes (full preview)
        if !has_applied_in_current {
            let mut moved = false;
            for &change_id in &current_hunk.change_ids {
                if !self.state.is_applied(change_id) {
                    self.state.push_applied(change_id);
                    self.state.current_step += 1;
                    moved = true;
                }
            }

            self.state.animating_hunk = Some(self.state.current_hunk);
            self.state.active_change = current_hunk.change_ids.first().copied();

            if moved {
                self.state.last_nav_was_hunk = true;
                self.state.hunk_preview_mode = true;
                self.state.preview_from_backward = false;
            }

            return moved;
        }

        // Current hunk has applied changes - complete it first, exit preview mode
        self.state.hunk_preview_mode = false;
        self.state.preview_from_backward = false;
        let mut completed_any = false;
        for &change_id in &current_hunk.change_ids {
            if !self.state.is_applied(change_id) {
                self.state.push_applied(change_id);
                self.state.current_step += 1;
                completed_any = true;
            }
        }

        // Move to next hunk
        let next_hunk_idx = self.state.current_hunk + 1;
        if next_hunk_idx >= self.diff.hunks.len() {
            // No next hunk - if we completed current hunk, update state; otherwise return false
            if completed_any {
                self.state.animating_hunk = Some(self.state.current_hunk);
                self.state.active_change = current_hunk.change_ids.last().copied();
                self.state.last_nav_was_hunk = true;
                return true;
            }
            // No movement, restore preview mode
            self.state.hunk_preview_mode = was_in_preview;
            return false;
        }

        let hunk = &self.diff.hunks[next_hunk_idx];

        // Apply ALL changes of next hunk (full preview)
        let mut moved = false;
        for &change_id in &hunk.change_ids {
            if !self.state.is_applied(change_id) {
                self.state.push_applied(change_id);
                self.state.current_step += 1;
                moved = true;
            }
        }

        self.state.animating_hunk = Some(next_hunk_idx);
        self.state.active_change = hunk.change_ids.first().copied();
        self.state.current_hunk = next_hunk_idx;

        if moved {
            self.state.last_nav_was_hunk = true;
            self.state.hunk_preview_mode = true;
            self.state.preview_from_backward = false;
        }

        moved
    }

    /// Move to the previous hunk, unapplying changes
    /// Returns true if moved, false if nothing to unapply
    pub fn prev_hunk(&mut self) -> bool {
        if self.diff.hunks.is_empty() {
            return false;
        }

        // Preserve preview mode in case we return false without doing anything
        let was_in_preview = self.state.hunk_preview_mode;

        // Clear preview mode
        self.state.hunk_preview_mode = false;
        self.state.preview_from_backward = false;

        // On hunk 0, only proceed if there are applied changes to unapply
        if self.state.current_hunk == 0 {
            let hunk = &self.diff.hunks[0];
            let has_applied = hunk.change_ids.iter().any(|id| self.state.is_applied(*id));
            if !has_applied {
                // No movement, restore preview mode
                self.state.hunk_preview_mode = was_in_preview;
                return false;
            }
        }

        self.state.step_direction = StepDirection::Backward;

        // If we have applied changes in current hunk, unapply them
        let current_hunk_idx = self.state.current_hunk;
        let mut moved = false;

        // Unapply changes from current hunk that are applied
        let removed = self.remove_applied_bulk_for_hunk(current_hunk_idx, None);
        if removed > 0 {
            self.state.current_step = self.state.applied_changes.len();
            moved = true;
        }

        // Set animating hunk for whole-hunk animation (keep pointing at the hunk
        // being removed so is_change_in_animating_hunk returns true during fade)
        self.state.animating_hunk = Some(current_hunk_idx);
        self.state.active_change = self
            .diff
            .hunks
            .get(current_hunk_idx)
            .and_then(|hunk| hunk.change_ids.first().copied());

        // Move to previous hunk if current is now empty of applied changes
        // (current_hunk tracks cursor position, animating_hunk tracks animation)
        if moved {
            // Check if we should move to previous hunk
            let still_has_applied = self
                .diff
                .hunks
                .get(current_hunk_idx)
                .map(|hunk| hunk.change_ids.iter().any(|id| self.state.is_applied(*id)))
                .unwrap_or(false);
            if !still_has_applied && self.state.current_hunk > 0 {
                self.state.current_hunk -= 1;
            }
        } else if self.state.current_hunk > 0 {
            // Nothing in current hunk was applied, try previous
            self.state.current_hunk -= 1;
            return self.prev_hunk();
        }

        // Don't overwrite animating_hunk here - let animation complete first.
        // current_hunk already tracks cursor position for status display.

        // Animation state cleared by CLI after animation completes

        // Enter preview mode when we land in a previous hunk
        if moved {
            let entered_prev_hunk = self.state.current_hunk != current_hunk_idx;
            if entered_prev_hunk {
                self.state.hunk_preview_mode = true;
                self.state.preview_from_backward = true;
                self.state.last_nav_was_hunk = true;
            } else {
                // Set or clear extent markers based on whether we landed at step 0
                self.state.last_nav_was_hunk = !self.state.is_at_start();
            }
        }

        moved
    }

    /// Go to a specific hunk (0-indexed)
    /// Applies all changes through target hunk (full preview mode).
    /// Cursor lands at top of target hunk.
    pub fn goto_hunk(&mut self, hunk_idx: usize) {
        if hunk_idx >= self.diff.hunks.len() {
            return;
        }

        // Reset to start
        self.goto_start();

        // Apply all changes for hunks before target
        for idx in 0..hunk_idx {
            let hunk = &self.diff.hunks[idx];
            for &change_id in &hunk.change_ids {
                self.state.push_applied(change_id);
                self.state.current_step += 1;
            }
        }

        // Apply ALL changes of target hunk (full preview)
        let hunk = &self.diff.hunks[hunk_idx];
        for &change_id in &hunk.change_ids {
            self.state.push_applied(change_id);
            self.state.current_step += 1;
        }

        self.state.current_hunk = hunk_idx;
        self.state.animating_hunk = Some(hunk_idx);
        self.state.active_change = hunk.change_ids.first().copied();
        self.state.step_direction = StepDirection::Forward;
        self.state.last_nav_was_hunk = true;
        self.state.hunk_preview_mode = true;
        self.state.preview_from_backward = false;
    }

    /// Jump to first change of current hunk, unapplying all but first
    /// Returns true if moved, false if not inside a hunk or already at start
    pub fn goto_hunk_start(&mut self) -> bool {
        if self.diff.hunks.is_empty() {
            return false;
        }

        let current_hunk_idx = self.state.current_hunk;
        let first_change = match self
            .diff
            .hunks
            .get(current_hunk_idx)
            .and_then(|hunk| hunk.change_ids.first().copied())
        {
            Some(id) => id,
            None => return false,
        };

        // Must have at least first change applied to be "inside" hunk
        if !self.state.is_applied(first_change) {
            return false;
        }

        // Unapply all changes in this hunk except the first
        let removed = self.remove_applied_bulk_for_hunk(current_hunk_idx, Some(first_change));
        let unapplied_any = removed > 0;
        if removed > 0 {
            self.state.current_step = self.state.applied_changes.len();
        }

        // No-op if already at start (nothing unapplied and cursor on first)
        if !unapplied_any && self.state.active_change == Some(first_change) {
            return false;
        }

        self.state.active_change = Some(first_change);
        self.state.hunk_preview_mode = false;
        self.state.preview_from_backward = false;
        self.state.last_nav_was_hunk = true;
        true
    }

    /// Jump to last change of current hunk, applying all changes in hunk
    /// Returns true if moved, false if not inside a hunk or already at end
    pub fn goto_hunk_end(&mut self) -> bool {
        if self.diff.hunks.is_empty() {
            return false;
        }

        let hunk = &self.diff.hunks[self.state.current_hunk];
        let has_applied = hunk.change_ids.iter().any(|id| self.state.is_applied(*id));
        if !has_applied {
            return false;
        }

        let last_change = hunk.change_ids.last().copied();

        // Apply all unapplied changes in this hunk
        for &change_id in &hunk.change_ids {
            if !self.state.is_applied(change_id) {
                self.state.push_applied(change_id);
                self.state.current_step += 1;
            }
        }

        // No-op if already at end (cursor on last)
        if self.state.active_change == last_change {
            return false;
        }

        self.state.active_change = last_change;
        self.state.hunk_preview_mode = false;
        self.state.preview_from_backward = false;
        self.state.last_nav_was_hunk = true;
        true
    }

    /// Update current hunk based on applied changes
    pub fn update_current_hunk(&mut self) {
        if self.diff.hunks.is_empty() {
            return;
        }

        // Find which hunk contains the most recently applied change
        if let Some(&last_applied) = self.state.applied_changes.last() {
            for (idx, hunk) in self.diff.hunks.iter().enumerate() {
                if hunk.change_ids.contains(&last_applied) {
                    self.state.current_hunk = idx;
                    return;
                }
            }
        }

        // If no changes applied, we're at hunk 0
        self.state.current_hunk = 0;
    }

    /// Get the current hunk
    pub fn current_hunk(&self) -> Option<&crate::diff::Hunk> {
        self.diff.hunks.get(self.state.current_hunk)
    }

    /// Get all hunks
    pub fn hunks(&self) -> &[crate::diff::Hunk] {
        &self.diff.hunks
    }

    pub fn set_show_hunk_extent_while_stepping(&mut self, enabled: bool) {
        self.state.show_hunk_extent_while_stepping = enabled;
    }

    // ==================== End Hunk Navigation ====================

    /// Check if a change belongs to the hunk currently being animated
    fn is_change_in_animating_hunk(&self, change_id: usize) -> bool {
        self.state
            .animating_hunk
            .and_then(|hunk_idx| {
                self.hunk_index_for_change(change_id)
                    .map(|id| id == hunk_idx)
            })
            .unwrap_or(false)
    }

    fn hunk_index_for_change(&self, change_id: usize) -> Option<usize> {
        let idx = self.change_to_index.get(change_id).copied().flatten()?;
        self.change_to_hunk.get(idx).copied().flatten()
    }

    fn hunk_index_for_change_exact(&self, change_id: usize) -> Option<usize> {
        self.change_id_to_hunk_exact
            .get(change_id)
            .copied()
            .flatten()
    }

    fn hunk_change_index_range(&self, hunk_idx: usize) -> Option<(usize, usize)> {
        self.hunk_change_ranges.get(hunk_idx).copied().flatten()
    }

    fn hunk_change_index_range_exact(&self, hunk_idx: usize) -> Option<(usize, usize)> {
        self.hunk_change_ranges_exact
            .get(hunk_idx)
            .copied()
            .flatten()
    }

    fn change_index(&self, change_id: usize) -> Option<usize> {
        self.change_to_index.get(change_id).copied().flatten()
    }

    pub fn hunk_index_for_change_id(&self, change_id: usize) -> Option<usize> {
        self.hunk_index_for_change(change_id)
    }

    pub fn hunk_index_for_change_id_exact(&self, change_id: usize) -> Option<usize> {
        self.hunk_index_for_change_exact(change_id)
    }

    pub fn change_index_for(&self, change_id: usize) -> Option<usize> {
        self.change_index(change_id)
    }

    pub fn hunk_step_range(&self, hunk_idx: usize) -> Option<(usize, usize)> {
        self.hunk_step_ranges
            .get(hunk_idx)
            .copied()
            .flatten()
            .map(|range| (range.start, range.len))
    }

    /// Get the currently active change
    pub fn active_change(&self) -> Option<&Change> {
        self.state
            .active_change
            .and_then(|id| self.diff.changes.iter().find(|c| c.id == id))
    }

    /// Get all changes with their application status
    pub fn changes_with_status(&self) -> Vec<(&Change, bool, bool)> {
        self.diff
            .changes
            .iter()
            .filter(|c| c.has_changes())
            .map(|c| {
                let applied = self.state.is_applied(c.id);
                let active = self.state.active_change == Some(c.id);
                (c, applied, active)
            })
            .collect()
    }

    /// Reconstruct the content at the current step
    /// Returns lines with their change status (uses Idle frame for backwards compatibility)
    pub fn current_view(&self) -> Vec<ViewLine> {
        self.current_view_with_frame(AnimationFrame::Idle)
    }

    /// Phase-aware view for word-level animation
    /// CLI should pass its current animation phase for proper fade animations
    pub fn current_view_with_frame(&self, frame: AnimationFrame) -> Vec<ViewLine> {
        self.view_for_changes(self.diff.changes.iter(), frame)
    }

    pub fn view_line_for_change(
        &self,
        frame: AnimationFrame,
        change_id: usize,
    ) -> Option<ViewLine> {
        let change = self.diff.changes.iter().find(|c| c.id == change_id)?;
        let is_applied = self.state.is_applied(change_id);
        let is_in_hunk = self.is_change_in_animating_hunk(change_id);
        let is_active_change = self.state.active_change == Some(change_id);
        let is_active = is_active_change || is_in_hunk;
        let has_changes = change.has_changes();
        let scope_hunk = if self.state.last_nav_was_hunk {
            self.state
                .cursor_change
                .and_then(|id| self.hunk_index_for_change_exact(id))
                .unwrap_or(self.state.current_hunk)
        } else {
            self.state.current_hunk
        };
        let in_scope = if self.state.last_nav_was_hunk {
            let scope_range = if has_changes {
                self.hunk_change_index_range_exact(scope_hunk)
            } else {
                self.hunk_change_index_range(scope_hunk)
            };
            let idx = self.change_to_index.get(change_id).copied().flatten();
            match (scope_range, idx) {
                (Some((start, end)), Some(idx)) => idx >= start && idx <= end,
                _ => {
                    if has_changes {
                        self.hunk_index_for_change_exact(change_id) == Some(scope_hunk)
                    } else {
                        self.hunk_index_for_change(change_id) == Some(scope_hunk)
                    }
                }
            }
        } else {
            self.hunk_index_for_change(change_id) == Some(scope_hunk)
        };
        let show_hunk_extent = is_in_hunk
            || (in_scope
                && (self.state.last_nav_was_hunk || self.state.show_hunk_extent_while_stepping));

        let primary_change_id = if self.state.cursor_change.is_some()
            && self.state.active_change.is_none()
            && self.state.step_direction == StepDirection::None
        {
            self.state.cursor_change
        } else if self.state.step_direction == StepDirection::Backward {
            self.state
                .applied_changes
                .last()
                .copied()
                .or(self.state.active_change)
        } else {
            self.state.active_change
        };

        let is_primary_active =
            primary_change_id == Some(change_id) || (primary_change_id.is_none() && is_in_hunk);

        if change.spans.len() > 1 {
            self.build_word_level_line(
                change,
                is_applied,
                is_active,
                is_active_change,
                is_primary_active,
                show_hunk_extent,
                frame,
            )
        } else {
            let span = change.spans.first()?;
            self.build_single_span_line(
                span,
                change_id,
                is_applied,
                is_active,
                is_active_change,
                is_primary_active,
                show_hunk_extent,
                frame,
            )
        }
    }

    pub fn current_view_for_hunk(
        &self,
        frame: AnimationFrame,
        hunk_idx: usize,
        context_lines: usize,
    ) -> Vec<ViewLine> {
        if self.diff.changes.is_empty() {
            return Vec::new();
        }
        let Some(hunk) = self.diff.hunks.get(hunk_idx) else {
            return self.current_view_with_frame(frame);
        };
        let mut min_idx = None;
        let mut max_idx = None;
        for change_id in &hunk.change_ids {
            if let Some(idx) = self.change_index(*change_id) {
                min_idx = Some(min_idx.map_or(idx, |v: usize| v.min(idx)));
                max_idx = Some(max_idx.map_or(idx, |v: usize| v.max(idx)));
            }
        }
        let Some(min_idx) = min_idx else {
            return self.current_view_with_frame(frame);
        };
        let Some(max_idx) = max_idx else {
            return self.current_view_with_frame(frame);
        };
        let start = min_idx.saturating_sub(context_lines);
        let end = (max_idx + context_lines).min(self.diff.changes.len().saturating_sub(1));
        self.view_for_changes(self.diff.changes[start..=end].iter(), frame)
    }

    pub fn current_view_for_change_window(
        &self,
        frame: AnimationFrame,
        change_id: usize,
        radius: usize,
    ) -> Vec<ViewLine> {
        let Some(idx) = self.change_index(change_id) else {
            return self.current_view_with_frame(frame);
        };
        if self.diff.changes.is_empty() {
            return Vec::new();
        }
        let start = idx.saturating_sub(radius);
        let end = (idx + radius).min(self.diff.changes.len().saturating_sub(1));
        self.view_for_changes(self.diff.changes[start..=end].iter(), frame)
    }

    pub fn current_view_for_change_range(
        &self,
        frame: AnimationFrame,
        start: usize,
        end: usize,
    ) -> Vec<ViewLine> {
        if self.diff.changes.is_empty() {
            return Vec::new();
        }
        let start = start.min(self.diff.changes.len().saturating_sub(1));
        let end = end.min(self.diff.changes.len().saturating_sub(1));
        if start > end {
            return Vec::new();
        }
        self.view_for_changes(self.diff.changes[start..=end].iter(), frame)
    }

    fn view_for_changes<'a, I>(&self, changes: I, frame: AnimationFrame) -> Vec<ViewLine>
    where
        I: IntoIterator<Item = &'a Change>,
    {
        let mut lines = Vec::new();

        // Primary cursor destination: last applied change on backward, active_change on forward
        // Fallback to active_change at step 0 so cursor stays on fading line
        let primary_change_id = if self.state.cursor_change.is_some()
            && self.state.active_change.is_none()
            && self.state.step_direction == StepDirection::None
        {
            self.state.cursor_change
        } else if self.state.step_direction == StepDirection::Backward {
            self.state
                .applied_changes
                .last()
                .copied()
                .or(self.state.active_change)
        } else {
            self.state.active_change
        };

        // Track if we've assigned a primary active line (for fallback when primary_change_id is None)
        let mut primary_assigned = false;
        let scope_hunk = if self.state.last_nav_was_hunk {
            self.state
                .cursor_change
                .and_then(|id| self.hunk_index_for_change_exact(id))
                .unwrap_or(self.state.current_hunk)
        } else {
            self.state.current_hunk
        };
        let scope_range_exact = if self.state.last_nav_was_hunk {
            self.hunk_change_index_range_exact(scope_hunk)
        } else {
            None
        };
        let scope_range_padded = if self.state.last_nav_was_hunk {
            self.hunk_change_index_range(scope_hunk)
        } else {
            None
        };

        for change in changes {
            let is_applied = self.state.is_applied(change.id);
            let has_changes = change.has_changes();
            let use_exact = self.state.last_nav_was_hunk && has_changes;

            // Primary active: cursor destination (decoupled from animation target on backward)
            let is_primary_active = primary_change_id == Some(change.id);

            // Active: part of the animating hunk (for animation styling)
            let is_in_hunk = self.is_change_in_animating_hunk(change.id);
            let is_active_change = self.state.active_change == Some(change.id);
            // Active if: (1) the active_change, or (2) in animating hunk (lights up whole hunk during animation)
            let is_active = is_active_change || is_in_hunk;
            // Show extent marker if animating hunk OR (last nav was hunk AND change in current hunk)
            let scope_range = if use_exact {
                scope_range_exact
            } else {
                scope_range_padded
            };
            let change_idx =
                scope_range.and_then(|_| self.change_to_index.get(change.id).copied().flatten());
            let in_scope = if let (Some((start, end)), Some(idx)) = (scope_range, change_idx) {
                idx >= start && idx <= end
            } else if use_exact {
                self.hunk_index_for_change_exact(change.id) == Some(scope_hunk)
            } else {
                self.hunk_index_for_change(change.id) == Some(scope_hunk)
            };
            let show_hunk_extent = is_in_hunk
                || (in_scope
                    && (self.state.last_nav_was_hunk
                        || self.state.show_hunk_extent_while_stepping));

            // Fallback: if primary_change_id is None but we're in an animating hunk,
            // first active line becomes primary
            let is_primary_active = is_primary_active
                || (!primary_assigned && is_in_hunk && primary_change_id.is_none());

            if is_primary_active {
                primary_assigned = true;
            }

            // Check if this is a word-level diff (multiple spans in one change that represents a line)
            let is_word_level = change.spans.len() > 1;

            if is_word_level {
                // Combine all spans into a single line
                let line = self.build_word_level_line(
                    change,
                    is_applied,
                    is_active,
                    is_active_change,
                    is_primary_active,
                    show_hunk_extent,
                    frame,
                );
                if let Some(l) = line {
                    lines.push(l);
                }
            } else {
                // Single span - handle as before
                if let Some(span) = change.spans.first() {
                    if let Some(line) = self.build_single_span_line(
                        span,
                        change.id,
                        is_applied,
                        is_active,
                        is_active_change,
                        is_primary_active,
                        show_hunk_extent,
                        frame,
                    ) {
                        lines.push(line);
                    }
                }
            }
        }

        lines
    }

    /// Compute whether to show new content based on animation frame and direction.
    /// Used by both word-level and single-span line builders for consistent animation.
    fn compute_show_new(&self, is_applied: bool, frame: AnimationFrame) -> bool {
        // Guard: if direction is None, fall back to final state
        if self.state.step_direction == StepDirection::None {
            return is_applied;
        }
        match frame {
            AnimationFrame::Idle => is_applied,
            // Forward + FadeOut = show old (false), Backward + FadeOut = show new (true)
            AnimationFrame::FadeOut => self.state.step_direction == StepDirection::Backward,
            // Forward + FadeIn = show new (true), Backward + FadeIn = show old (false)
            AnimationFrame::FadeIn => self.state.step_direction != StepDirection::Backward,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_word_level_line(
        &self,
        change: &Change,
        is_applied: bool,
        is_active: bool,
        is_active_change: bool,
        is_primary_active: bool,
        show_hunk_extent: bool,
        frame: AnimationFrame,
    ) -> Option<ViewLine> {
        let first_span = change.spans.first()?;
        let old_line = first_span.old_line;
        let new_line = first_span.new_line;

        // Pre-scan to classify: does this change have old content, new content, or both?
        // Replace counts as both since it has old text and new_text.
        let has_old = change
            .spans
            .iter()
            .any(|s| matches!(s.kind, ChangeKind::Delete | ChangeKind::Replace));
        let has_new = change
            .spans
            .iter()
            .any(|s| matches!(s.kind, ChangeKind::Insert | ChangeKind::Replace));

        // Build spans for the view line
        let mut view_spans = Vec::new();
        let mut content = String::new();

        for span in &change.spans {
            // Phase-aware content and styling for active changes
            let (span_kind, text) = if is_active {
                // Determine show_new based on frame and change type:
                // - Idle: always snap to real applied state (no "phantom" content)
                // - FadeOut/FadeIn:
                //   - Mixed (has_old && has_new): phase-swap old/new
                //   - Insert-only: always show new (visible both phases)
                //   - Delete-only: always show old (visible both phases)
                let show_new = match frame {
                    AnimationFrame::Idle => self.compute_show_new(is_applied, frame),
                    _ => {
                        if has_old && has_new {
                            self.compute_show_new(is_applied, frame)
                        } else if has_new {
                            true // Insert-only: visible during animation
                        } else {
                            false // Delete-only: visible during animation
                        }
                    }
                };

                match span.kind {
                    ChangeKind::Equal => (ViewSpanKind::Equal, span.text.clone()),
                    ChangeKind::Delete => {
                        if show_new {
                            continue; // Hide deletions when showing new state
                        } else {
                            (ViewSpanKind::PendingDelete, span.text.clone())
                        }
                    }
                    ChangeKind::Insert => {
                        if show_new {
                            (ViewSpanKind::PendingInsert, span.text.clone())
                        } else {
                            continue; // Hide insertions when showing old state
                        }
                    }
                    ChangeKind::Replace => {
                        if show_new {
                            (
                                ViewSpanKind::PendingInsert,
                                span.new_text.clone().unwrap_or_else(|| span.text.clone()),
                            )
                        } else {
                            (ViewSpanKind::PendingDelete, span.text.clone())
                        }
                    }
                }
            } else if is_applied {
                // Applied but not active - show final state
                let kind = match span.kind {
                    ChangeKind::Equal => ViewSpanKind::Equal,
                    ChangeKind::Delete => ViewSpanKind::Deleted,
                    ChangeKind::Insert => ViewSpanKind::Inserted,
                    ChangeKind::Replace => ViewSpanKind::Inserted,
                };
                let text = match span.kind {
                    ChangeKind::Delete => {
                        continue; // Don't include deleted text in the final content
                    }
                    ChangeKind::Replace => {
                        span.new_text.clone().unwrap_or_else(|| span.text.clone())
                    }
                    _ => span.text.clone(),
                };
                (kind, text)
            } else {
                // Not applied, not active - show original state
                match span.kind {
                    ChangeKind::Insert => {
                        continue; // Don't show pending inserts
                    }
                    _ => (ViewSpanKind::Equal, span.text.clone()),
                }
            };

            content.push_str(&text);
            view_spans.push(ViewSpan {
                text,
                kind: span_kind,
            });
        }

        // Defensive guard: don't emit blank lines if all spans were filtered
        if view_spans.is_empty() {
            return None;
        }

        // Line kind - keep PendingModify for active word-level lines
        // (evolution view filters on LineKind::PendingDelete, so this prevents drops)
        let line_kind = if is_active {
            LineKind::PendingModify
        } else if is_applied {
            LineKind::Modified
        } else {
            LineKind::Context
        };

        // Populate hunk metadata
        let hunk_index = self.hunk_index_for_change(change.id);
        let has_changes = change.has_changes();

        Some(ViewLine {
            content,
            spans: view_spans,
            kind: line_kind,
            old_line,
            new_line,
            is_active,
            is_active_change,
            is_primary_active,
            show_hunk_extent,
            change_id: change.id,
            hunk_index,
            has_changes,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_single_span_line(
        &self,
        span: &ChangeSpan,
        change_id: usize,
        is_applied: bool,
        is_active: bool,
        is_active_change: bool,
        is_primary_active: bool,
        show_hunk_extent: bool,
        frame: AnimationFrame,
    ) -> Option<ViewLine> {
        let view_span_kind;
        let line_kind;
        let content;

        match span.kind {
            ChangeKind::Equal => {
                view_span_kind = ViewSpanKind::Equal;
                line_kind = LineKind::Context;
                content = span.text.clone();
            }
            ChangeKind::Delete => {
                // IMPORTANT: Check is_active FIRST so deletions animate before disappearing
                if is_active {
                    view_span_kind = ViewSpanKind::PendingDelete;
                    line_kind = LineKind::PendingDelete;
                    content = span.text.clone();
                } else if is_applied {
                    view_span_kind = ViewSpanKind::Deleted;
                    line_kind = LineKind::Deleted;
                    content = span.text.clone();
                } else {
                    view_span_kind = ViewSpanKind::Equal;
                    line_kind = LineKind::Context;
                    content = span.text.clone();
                }
            }
            ChangeKind::Insert => {
                // Check is_active first for animation
                if is_active {
                    view_span_kind = ViewSpanKind::PendingInsert;
                    line_kind = LineKind::PendingInsert;
                    content = span.text.clone();
                } else if is_applied {
                    view_span_kind = ViewSpanKind::Inserted;
                    line_kind = LineKind::Inserted;
                    content = span.text.clone();
                } else {
                    return None; // Don't show unapplied inserts
                }
            }
            ChangeKind::Replace => {
                // Phase-aware Replace: show old during FadeOut, new during FadeIn
                if is_active {
                    let show_new = self.compute_show_new(is_applied, frame);
                    if show_new {
                        view_span_kind = ViewSpanKind::PendingInsert;
                        content = span.new_text.clone().unwrap_or_else(|| span.text.clone());
                    } else {
                        view_span_kind = ViewSpanKind::PendingDelete;
                        content = span.text.clone();
                    }
                    line_kind = LineKind::PendingModify;
                } else if is_applied {
                    view_span_kind = ViewSpanKind::Inserted;
                    line_kind = LineKind::Modified;
                    content = span.new_text.clone().unwrap_or_else(|| span.text.clone());
                } else {
                    view_span_kind = ViewSpanKind::Equal;
                    line_kind = LineKind::Context;
                    content = span.text.clone();
                }
            }
        }

        // Populate hunk metadata
        let hunk_index = self.hunk_index_for_change(change_id);
        let has_changes = !matches!(span.kind, ChangeKind::Equal);

        Some(ViewLine {
            content: content.clone(),
            spans: vec![ViewSpan {
                text: content,
                kind: view_span_kind,
            }],
            kind: line_kind,
            old_line: span.old_line,
            new_line: span.new_line,
            is_active,
            is_active_change,
            is_primary_active,
            show_hunk_extent,
            change_id,
            hunk_index,
            has_changes,
        })
    }

    /// Get old content
    pub fn old_content(&self) -> &str {
        self.old_content.as_ref()
    }

    /// Get new content
    pub fn new_content(&self) -> &str {
        self.new_content.as_ref()
    }
}

/// A styled span within a view line
#[derive(Debug, Clone)]
pub struct ViewSpan {
    pub text: String,
    pub kind: ViewSpanKind,
}

/// The kind of span styling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewSpanKind {
    Equal,
    Inserted,
    Deleted,
    PendingInsert,
    PendingDelete,
}

/// A line in the current view with its status
#[derive(Debug, Clone)]
pub struct ViewLine {
    /// Full content of the line
    pub content: String,
    /// Individual styled spans (for word-level highlighting)
    pub spans: Vec<ViewSpan>,
    /// Overall line kind
    pub kind: LineKind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    /// Part of the active hunk (for animation styling)
    pub is_active: bool,
    /// The active change itself (not just part of a hunk preview)
    pub is_active_change: bool,
    /// The primary focus line within the hunk (for gutter marker)
    pub is_primary_active: bool,
    /// Show extent marker (true only during hunk navigation)
    pub show_hunk_extent: bool,
    /// ID of the change this line belongs to
    pub change_id: usize,
    /// Index of the hunk this line belongs to
    pub hunk_index: Option<usize>,
    /// True if the underlying change contains any non-equal spans
    pub has_changes: bool,
}

/// The kind of line in the view
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Unchanged context line
    Context,
    /// Line was inserted
    Inserted,
    /// Line was deleted
    Deleted,
    /// Line was modified
    Modified,
    /// Line is about to be deleted (active animation)
    PendingDelete,
    /// Line is about to be inserted (active animation)
    PendingInsert,
    /// Line is about to be modified (active animation)
    PendingModify,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::{Change, ChangeKind, ChangeSpan};
    use crate::diff::DiffEngine;
    use crate::diff::{DiffResult, Hunk};
    use std::sync::Arc;

    fn build_manual_diff(
        changes: Vec<Change>,
        significant_changes: Vec<usize>,
        hunks: Vec<Hunk>,
    ) -> DiffResult {
        let mut insertions = 0usize;
        let mut deletions = 0usize;
        for change in &changes {
            for span in &change.spans {
                match span.kind {
                    ChangeKind::Insert => insertions += 1,
                    ChangeKind::Delete => deletions += 1,
                    ChangeKind::Replace => {
                        insertions += 1;
                        deletions += 1;
                    }
                    ChangeKind::Equal => {}
                }
            }
        }
        DiffResult {
            changes,
            significant_changes,
            hunks,
            insertions,
            deletions,
        }
    }

    fn make_equal_change(id: usize) -> Change {
        let line = id + 1;
        Change::single(
            id,
            ChangeSpan::equal(format!("line{}", id)).with_lines(Some(line), Some(line)),
        )
    }

    fn make_insert_change(id: usize) -> Change {
        let line = id + 1;
        Change::single(
            id,
            ChangeSpan::insert(format!("ins{}", id)).with_lines(None, Some(line)),
        )
    }

    fn assert_applied_is_prefix(nav: &DiffNavigator) {
        let applied = &nav.state().applied_changes;
        let sig = &nav.diff.significant_changes;
        assert!(
            applied.len() <= sig.len(),
            "applied changes should not exceed significant changes"
        );
        assert_eq!(
            &sig[..applied.len()],
            applied.as_slice(),
            "applied changes should remain a prefix of significant_changes"
        );
    }

    #[test]
    fn test_navigation() {
        let old = "foo\nbar\nbaz";
        let new = "foo\nqux\nbaz";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        assert!(nav.state().is_at_start());
        assert!(!nav.state().is_at_end());

        nav.next();
        assert!(!nav.state().is_at_start());

        nav.goto_end();
        assert!(nav.state().is_at_end());

        nav.prev();
        assert!(!nav.state().is_at_end());

        nav.goto_start();
        assert!(nav.state().is_at_start());
    }

    #[test]
    fn test_progress() {
        let old = "a\nb\nc\nd";
        let new = "a\nB\nC\nd";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        assert_eq!(nav.state().progress(), 0.0);

        nav.goto_end();
        assert_eq!(nav.state().progress(), 100.0);
    }

    #[test]
    fn test_word_level_view() {
        let old = "const foo = 4";
        let new = "const bar = 5";

        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // At start, should show original line
        let view = nav.current_view();
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].content, "const foo = 4");

        // After applying change, should show new line
        nav.next();
        let view = nav.current_view();
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].content, "const bar = 5");
    }

    #[test]
    fn test_prev_hunk_animation_state() {
        // Setup: file with 2 hunks (changes separated by >3 unchanged lines)
        // Hunk proximity threshold is 3, so we need at least 4 unchanged lines between changes
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nK\nl";
        //              ^ hunk 0 (line 2)              ^ hunk 1 (line 11)

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);

        // Verify we have 2 hunks
        assert!(
            diff.hunks.len() >= 2,
            "Expected at least 2 hunks, got {}. Adjust fixture gap.",
            diff.hunks.len()
        );

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply both hunks
        nav.next_hunk();
        nav.next_hunk();
        assert_eq!(nav.state().current_hunk, 1);

        // Step back one hunk
        nav.prev_hunk();

        // animating_hunk should point to hunk 1 (the one being removed)
        assert_eq!(
            nav.state().animating_hunk,
            Some(1),
            "animating_hunk should stay on the hunk being removed for fade animation"
        );
        // current_hunk should have moved to 0 (cursor position)
        assert_eq!(
            nav.state().current_hunk,
            0,
            "current_hunk should move to destination for status display"
        );

        // View should mark hunk 1 changes as active
        let view = nav.current_view();
        let active_lines: Vec<_> = view.iter().filter(|l| l.is_active).collect();
        assert!(
            !active_lines.is_empty(),
            "Hunk changes should be marked active during fade"
        );

        // After clearing, animating_hunk should be None
        nav.clear_active_change();
        assert_eq!(
            nav.state().animating_hunk,
            None,
            "animating_hunk should be cleared after animation completes"
        );
    }

    #[test]
    fn test_prev_to_start_preserves_animation_state() {
        // Single hunk - stepping back lands on step 0
        // Animation state persists for fade-out, cleared by CLI after animation completes
        let old = "a\nb\nc";
        let new = "a\nB\nc";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply first change (no hunk preview)
        nav.next();
        assert_eq!(nav.state().current_step, 1);

        // Step back to start
        nav.prev();
        assert!(nav.state().is_at_start());

        // Animation state preserved for fade-out rendering
        assert!(
            nav.state().active_change.is_some(),
            "active_change preserved for fade-out"
        );
        assert_eq!(
            nav.state().step_direction,
            StepDirection::Backward,
            "step_direction should be Backward"
        );

        // CLI calls clear_active_change() after animation completes
        nav.clear_active_change();
        assert_eq!(nav.state().active_change, None);
        assert_eq!(nav.state().animating_hunk, None);
        assert_eq!(nav.state().step_direction, StepDirection::None);
    }

    #[test]
    fn test_prev_hunk_from_hunk_0_unapplies_changes() {
        // prev_hunk should unapply hunk 0 when it has applied changes
        let old = "a\nb\nc";
        let new = "a\nB\nc";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply first hunk
        nav.next_hunk();
        assert_eq!(nav.state().current_step, 1);
        assert_eq!(nav.state().current_hunk, 0);

        // prev_hunk from hunk 0 should work (unapply the hunk)
        let moved = nav.prev_hunk();
        assert!(
            moved,
            "prev_hunk should succeed when hunk 0 has applied changes"
        );
        assert!(nav.state().is_at_start());
        assert_eq!(nav.state().current_step, 0);

        // animating_hunk should be set for extent markers
        assert_eq!(
            nav.state().animating_hunk,
            Some(0),
            "animating_hunk should point to hunk 0 for fade animation"
        );
        assert_eq!(nav.state().step_direction, StepDirection::Backward);

        // Calling prev_hunk again should return false (nothing to unapply)
        let moved_again = nav.prev_hunk();
        assert!(
            !moved_again,
            "prev_hunk should fail when hunk 0 has no applied changes"
        );
    }

    #[test]
    fn test_backward_primary_marker_on_destination() {
        // Two hunks with >3 lines separation to ensure distinct hunks
        // Stepping back from hunk 1 should put primary on hunk 0's change (destination)
        let old = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8";
        let new = "line1\nLINE2\nline3\nline4\nline5\nline6\nLINE7\nline8";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        assert!(diff.hunks.len() >= 2, "Fixture must produce 2 hunks");

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply both hunks
        nav.next_hunk(); // hunk 0 (LINE2)
        nav.next_hunk(); // hunk 1 (LINE7)
        assert_eq!(nav.state().current_step, 2);

        // Step back from hunk 1
        nav.prev_hunk();

        let view = nav.current_view_with_frame(AnimationFrame::FadeOut);

        // Find the lines
        let primary_lines: Vec<_> = view.iter().filter(|l| l.is_primary_active).collect();
        let active_lines: Vec<_> = view.iter().filter(|l| l.is_active).collect();

        // Exactly one primary line
        assert_eq!(primary_lines.len(), 1, "exactly one primary line");

        // Fading hunk should have is_active lines
        assert!(!active_lines.is_empty(), "fading line should be active");

        // Primary is on destination (hunk 0 = LINE2), not fading line (hunk 1 = LINE7)
        let primary = primary_lines[0];
        assert!(
            primary.content.contains("LINE2"),
            "primary marker should be on destination (hunk 0)"
        );
        assert!(
            !primary.content.contains("LINE7"),
            "primary marker should not be on fading line (hunk 1)"
        );
    }

    #[test]
    fn test_forward_primary_marker_on_first_change() {
        // Multi-change hunk: next_hunk should put cursor on first change (top of hunk)
        // Hunk 0 has 2 consecutive changes so cursor position matters
        let old = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8";
        let new = "line1\nLINE2\nLINE3\nline4\nline5\nline6\nline7\nline8";
        //                 ^ first change  ^ second change (same hunk due to proximity)

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        assert!(
            !diff.hunks.is_empty(),
            "Fixture must produce at least 1 hunk"
        );
        assert!(
            diff.hunks[0].change_ids.len() >= 2,
            "Hunk 0 must have at least 2 changes for this test"
        );

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply hunk 0
        nav.next_hunk();

        // Use FadeIn to see new content (LINE2/LINE3)
        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);

        let primary_lines: Vec<_> = view.iter().filter(|l| l.is_primary_active).collect();
        assert_eq!(primary_lines.len(), 1, "exactly one primary line");

        // Primary should be on LINE2 (first change), not LINE3 (last change)
        let primary = primary_lines[0];
        assert!(
            primary.content.contains("LINE2"),
            "primary marker should be on first change (LINE2), got: {}",
            primary.content
        );
    }

    #[test]
    fn test_step_down_after_next_hunk() {
        // After next_hunk lands at first change, step down should move to second change
        let old = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8";
        let new = "line1\nLINE2\nLINE3\nline4\nline5\nline6\nline7\nline8";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        assert!(
            diff.hunks[0].change_ids.len() >= 2,
            "Hunk 0 must have at least 2 changes"
        );

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // next_hunk applies ALL changes (full preview), cursor at first
        nav.next_hunk();
        assert_eq!(
            nav.state().current_step,
            2,
            "Should apply all 2 changes after next_hunk"
        );
        assert!(nav.state().hunk_preview_mode, "Should be in preview mode");

        // Step down dissolves preview: keeps first, applies second, cursor on second
        let moved = nav.next();
        assert!(moved, "next() should succeed");
        assert_eq!(
            nav.state().current_step,
            2,
            "Should still be at step 2 after dissolve"
        );
        assert!(!nav.state().hunk_preview_mode, "Should exit preview mode");

        // Verify cursor is now on LINE3 (second change)
        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);
        let primary = view.iter().find(|l| l.is_primary_active);
        assert!(primary.is_some(), "Should have primary line");
        assert!(
            primary.unwrap().content.contains("LINE3"),
            "Primary should be on LINE3 after stepping"
        );
    }

    #[test]
    fn test_next_hunk_completes_current_then_lands_on_next() {
        // Two hunks separated by >3 lines. next_hunk on hunk 0 applies all,
        // then next_hunk moves to hunk 1 with full preview.
        let old = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8";
        let new = "line1\nLINE2\nLINE3\nline4\nline5\nline6\nLINE7\nLINE8";
        //         hunk 0: LINE2, LINE3          hunk 1: LINE7, LINE8

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        assert!(diff.hunks.len() >= 2, "Must have 2 hunks");

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // First next_hunk: apply all changes in hunk 0 (full preview)
        nav.next_hunk();
        assert_eq!(nav.state().current_hunk, 0);
        assert_eq!(
            nav.state().current_step,
            2,
            "Should apply all 2 changes in hunk 0"
        );
        assert!(nav.state().hunk_preview_mode);

        // Second next_hunk: move to hunk 1 with full preview
        nav.next_hunk();
        assert_eq!(nav.state().current_hunk, 1, "Should be in hunk 1");

        // All of hunk 0 (2 changes) + all of hunk 1 (2 changes) = 4 total
        assert_eq!(nav.state().current_step, 4, "Should have applied 4 changes");
        assert!(nav.state().hunk_preview_mode);

        // Cursor should be on LINE7 (first change of hunk 1)
        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);
        let primary = view.iter().find(|l| l.is_primary_active);
        assert!(primary.is_some());
        assert!(
            primary.unwrap().content.contains("LINE7"),
            "Cursor should be on LINE7"
        );
    }

    #[test]
    fn test_next_hunk_on_last_hunk_stays_at_end() {
        // Single hunk with 2 changes. Calling next_hunk applies all changes.
        // Calling next_hunk again returns false (no next hunk).
        let old = "line1\nline2\nline3";
        let new = "line1\nLINE2\nLINE3";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        assert_eq!(diff.hunks.len(), 1, "Must have exactly 1 hunk");
        assert_eq!(
            diff.hunks[0].change_ids.len(),
            2,
            "Hunk must have 2 changes"
        );

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // First next_hunk: apply all changes (full preview)
        let moved1 = nav.next_hunk();
        assert!(moved1);
        assert_eq!(nav.state().current_step, 2, "Should apply all 2 changes");
        assert!(nav.state().hunk_preview_mode);

        // Second next_hunk: no next hunk, returns false
        let moved2 = nav.next_hunk();
        assert!(!moved2, "Should return false when no next hunk");
        assert_eq!(nav.state().current_step, 2, "Should still be at step 2");
    }

    #[test]
    fn test_hunk_change_range_cached_for_large_hunk() {
        let old = "a\nb\nc\nd\ne";
        let new = "A\nB\nC\nD\nE";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        assert_eq!(diff.hunks.len(), 1, "single hunk for full replace");

        let nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), true);
        let range = nav.hunk_change_index_range(0);
        assert!(range.is_some(), "expected cached range for hunk 0");
        let (start, end) = range.unwrap();
        assert!(start <= end, "range should be valid");
    }

    #[test]
    fn test_no_step_scope_prefers_exact_mapping_for_changes() {
        let changes = (0..8)
            .map(|id| {
                if id == 2 || id == 5 {
                    make_insert_change(id)
                } else {
                    make_equal_change(id)
                }
            })
            .collect::<Vec<_>>();
        let hunks = vec![
            Hunk {
                id: 0,
                change_ids: vec![2],
                old_start: None,
                new_start: Some(3),
                insertions: 1,
                deletions: 0,
            },
            Hunk {
                id: 1,
                change_ids: vec![5],
                old_start: None,
                new_start: Some(6),
                insertions: 1,
                deletions: 0,
            },
        ];
        let diff = build_manual_diff(changes, vec![2, 5], hunks);
        let mut nav = DiffNavigator::new(diff, Arc::from(""), Arc::from(""), true);
        nav.goto_end();

        assert_eq!(
            nav.hunk_index_for_change_id(5),
            Some(0),
            "fixture should overlap padded range"
        );
        assert_eq!(
            nav.hunk_index_for_change_id_exact(5),
            Some(1),
            "exact mapping should point to hunk 1"
        );

        nav.set_cursor_hunk(1, Some(5));
        nav.set_hunk_scope(true);

        let view = nav.current_view_with_frame(AnimationFrame::Idle);
        let line_hunk_1 = view.iter().find(|l| l.change_id == 5).unwrap();
        let line_hunk_0 = view.iter().find(|l| l.change_id == 2).unwrap();

        assert!(
            line_hunk_1.show_hunk_extent,
            "change line in scope hunk should show extent"
        );
        assert!(
            !line_hunk_0.show_hunk_extent,
            "change line outside scope hunk should not show extent"
        );
    }

    #[test]
    fn test_no_step_scope_includes_context_lines() {
        let changes = (0..10)
            .map(|id| {
                if id == 4 {
                    make_insert_change(id)
                } else {
                    make_equal_change(id)
                }
            })
            .collect::<Vec<_>>();
        let hunks = vec![Hunk {
            id: 0,
            change_ids: vec![4],
            old_start: None,
            new_start: Some(5),
            insertions: 1,
            deletions: 0,
        }];
        let diff = build_manual_diff(changes, vec![4], hunks);
        let mut nav = DiffNavigator::new(diff, Arc::from(""), Arc::from(""), true);
        nav.goto_end();

        nav.set_cursor_hunk(0, Some(4));
        nav.set_hunk_scope(true);

        let view = nav.current_view_with_frame(AnimationFrame::Idle);
        let scope_range = nav
            .hunk_change_index_range(0)
            .expect("expected cached padded range");

        for line in view.iter().filter(|l| !l.has_changes) {
            let idx = nav.change_index_for(line.change_id).unwrap();
            let in_range = idx >= scope_range.0 && idx <= scope_range.1;
            assert_eq!(
                line.show_hunk_extent, in_range,
                "context line {} scope mismatch",
                line.change_id
            );
        }
    }

    #[test]
    fn test_markers_persist_within_hunk() {
        // Stepping within a hunk after next_hunk should preserve extent markers
        let old = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8";
        let new = "line1\nLINE2\nLINE3\nline4\nline5\nline6\nline7\nline8";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        nav.next_hunk();
        assert!(
            nav.state().last_nav_was_hunk,
            "last_nav_was_hunk should be true after next_hunk"
        );

        // Step within hunk (still in hunk 0)
        nav.next();
        assert!(
            nav.state().last_nav_was_hunk,
            "last_nav_was_hunk should persist within hunk"
        );
    }

    #[test]
    fn test_markers_clear_on_hunk_exit() {
        // Stepping into a different hunk should clear extent markers
        let old = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8";
        let new = "line1\nLINE2\nline3\nline4\nline5\nline6\nLINE7\nline8";
        //         hunk 0: LINE2                  hunk 1: LINE7

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        assert!(diff.hunks.len() >= 2, "Must have 2 hunks");

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply hunk 0 via next_hunk
        nav.next_hunk();
        assert!(nav.state().last_nav_was_hunk);

        // Step into hunk 1 via next() - should clear markers
        nav.next();
        assert!(
            !nav.state().last_nav_was_hunk,
            "Markers should clear when stepping into different hunk"
        );
    }

    #[test]
    fn test_active_change_flag_in_hunk_preview() {
        // Single hunk with 2 changes: hunk preview should mark all lines active,
        // but only the first change is the active change.
        let old = "line1\nline2\nline3\n";
        let new = "LINE1\nLINE2\nline3\n";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        assert_eq!(diff.hunks.len(), 1, "Expected a single hunk");
        assert!(
            diff.hunks[0].change_ids.len() >= 2,
            "Expected 2 changes in the hunk"
        );

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);
        nav.next_hunk();

        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);
        let active_changes: Vec<_> = view.iter().filter(|l| l.is_active_change).collect();
        let active_lines: Vec<_> = view.iter().filter(|l| l.is_active).collect();

        assert_eq!(active_changes.len(), 1, "Only one active change expected");
        assert!(
            active_lines.len() >= 2,
            "All hunk lines should be active during preview"
        );
        assert!(
            active_changes[0].is_primary_active,
            "Active change should be primary"
        );
        assert!(
            active_changes[0].content.contains("LINE1"),
            "Active change should be the first change in the hunk"
        );
    }

    #[test]
    fn test_word_level_phase_aware_mixed_change() {
        // Mixed change: has both old (foo, 4) and new (bar, 5) content
        // Should swap old/new at phase boundary
        let old = "const foo = 4";
        let new = "const bar = 5";

        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply the change (makes it active)
        nav.next();
        assert!(nav.state().active_change.is_some());

        // FadeOut (forward): should show OLD content
        let view = nav.current_view_with_frame(AnimationFrame::FadeOut);
        assert_eq!(view.len(), 1);
        assert_eq!(
            view[0].content, "const foo = 4",
            "FadeOut should show old content for mixed word-level change"
        );

        // FadeIn (forward): should show NEW content
        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);
        assert_eq!(view.len(), 1);
        assert_eq!(
            view[0].content, "const bar = 5",
            "FadeIn should show new content for mixed word-level change"
        );

        // Idle: should show applied (new) content
        let view = nav.current_view_with_frame(AnimationFrame::Idle);
        assert_eq!(view.len(), 1);
        assert_eq!(
            view[0].content, "const bar = 5",
            "Idle should show applied (new) content"
        );
    }

    #[test]
    fn test_word_level_insert_only_visible_both_phases() {
        // Insert-only change: should be visible across both FadeOut and FadeIn
        let old = "hello";
        let new = "hello world";

        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply the change
        nav.next();

        // FadeOut: insert-only should still be visible (not hidden)
        let view = nav.current_view_with_frame(AnimationFrame::FadeOut);
        assert_eq!(view.len(), 1);
        assert!(
            view[0].content.contains("world"),
            "Insert-only change should be visible during FadeOut, got: {}",
            view[0].content
        );

        // FadeIn: should also be visible
        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);
        assert_eq!(view.len(), 1);
        assert!(
            view[0].content.contains("world"),
            "Insert-only change should be visible during FadeIn, got: {}",
            view[0].content
        );
    }

    #[test]
    fn test_word_level_delete_only_visible_both_phases() {
        // Delete-only change: should be visible across both FadeOut and FadeIn
        let old = "hello world";
        let new = "hello";

        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply the change
        nav.next();

        // FadeOut: delete-only should still be visible (showing old content being deleted)
        let view = nav.current_view_with_frame(AnimationFrame::FadeOut);
        assert_eq!(view.len(), 1);
        assert!(
            view[0].content.contains("world"),
            "Delete-only change should show deleted content during FadeOut, got: {}",
            view[0].content
        );

        // FadeIn: should also show the deletion
        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);
        assert_eq!(view.len(), 1);
        assert!(
            view[0].content.contains("world"),
            "Delete-only change should show deleted content during FadeIn, got: {}",
            view[0].content
        );
    }

    #[test]
    fn test_word_level_insert_only_idle_respects_applied_state() {
        // Insert-only change: Idle frame should snap to real applied state
        // (no "phantom" inserts when animations are disabled)
        let old = "hello";
        let new = "hello world";

        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply then unapply
        nav.next();
        nav.prev();

        // Idle should NOT contain "world" (it's unapplied)
        let view = nav.current_view_with_frame(AnimationFrame::Idle);
        assert_eq!(view.len(), 1);
        assert!(
            !view[0].content.contains("world"),
            "Idle should not show unapplied insert, got: {}",
            view[0].content
        );
    }

    #[test]
    fn test_word_level_phase_aware_backward_with_multiple_changes() {
        // To properly test backward animation, we need multiple changes
        // so stepping back doesn't land on step 0 (which clears animation state)
        let old = "aaa\nconst foo = 4\nccc\nddd\neee\nfff\nggg\nconst bar = 8\niii\njjj";
        let new = "aaa\nconst bbb = 5\nccc\nddd\neee\nfff\nggg\nconst qux = 9\niii\njjj";
        //              ^ change 1 (word-level)              ^ change 2 (word-level)

        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(old, new);

        // Verify we have 2 changes
        assert!(
            diff.significant_changes.len() >= 2,
            "Expected at least 2 changes, got {}",
            diff.significant_changes.len()
        );

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply both changes
        nav.next(); // step 1: first change applied
        nav.next(); // step 2: second change applied
        assert_eq!(nav.state().current_step, 2);

        // Step back from step 2 to step 1 (second change is now active for backward animation)
        nav.prev();
        assert_eq!(nav.state().current_step, 1);
        assert_eq!(nav.state().step_direction, StepDirection::Backward);
        assert!(nav.state().active_change.is_some());

        // Find the line that's active (should be line 8, word-level change being un-applied)
        let view = nav.current_view_with_frame(AnimationFrame::FadeOut);
        let active_line = view.iter().find(|l| l.is_active);
        assert!(active_line.is_some(), "Should have an active line");

        // Backward + FadeOut: should show NEW content (the content being removed)
        // For word-level, this means "const qux = 9"
        let active = active_line.unwrap();
        assert_eq!(
            active.content, "const qux = 9",
            "Backward FadeOut should show new content (being removed)"
        );

        // Backward + FadeIn: should show OLD content (the content being restored)
        // For word-level, this means "const bar = 8"
        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);
        let active_line = view.iter().find(|l| l.is_active);
        assert!(active_line.is_some(), "Should have an active line");
        let active = active_line.unwrap();
        assert_eq!(
            active.content, "const bar = 8",
            "Backward FadeIn should show old content (being restored)"
        );
    }

    #[test]
    fn test_word_level_insert_only_backward_visible_both_phases() {
        // Insert-only word-level change should stay visible during backward animation
        // Test: "hello" -> "hello world" is insert-only (only adds " world")
        // We need 2 changes to avoid landing on step 0
        let old = "aaa\nhello\nccc\nddd\neee\nfff\nggg\nfoo\niii\njjj";
        let new = "aaa\nhello world\nccc\nddd\neee\nfff\nggg\nfoo bar\niii\njjj";
        //              ^ insert-only (add " world")       ^ insert-only (add " bar")

        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(old, new);

        assert!(
            diff.significant_changes.len() >= 2,
            "Need 2+ changes to avoid landing on step 0, got {}",
            diff.significant_changes.len()
        );

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Apply both changes, then step back
        nav.next();
        nav.next();
        nav.prev();

        assert_eq!(nav.state().step_direction, StepDirection::Backward);
        assert!(nav.state().active_change.is_some());

        // FadeOut: insert-only should still be visible (shows the inserted content)
        let view = nav.current_view_with_frame(AnimationFrame::FadeOut);
        let active_line = view.iter().find(|l| l.is_active);
        assert!(
            active_line.is_some(),
            "Should have an active line during FadeOut"
        );
        let active = active_line.unwrap();
        assert!(
            active.content.contains("bar"),
            "Backward FadeOut should show insert-only content, got: {}",
            active.content
        );

        // FadeIn: should also show the inserted content (visible both phases)
        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);
        let active_line = view.iter().find(|l| l.is_active);
        assert!(
            active_line.is_some(),
            "Should have an active line during FadeIn"
        );
        let active = active_line.unwrap();
        assert!(
            active.content.contains("bar"),
            "Backward FadeIn should show insert-only content, got: {}",
            active.content
        );
    }

    #[test]
    fn test_word_level_active_line_kind_pending_modify() {
        // Active word-level lines should have LineKind::PendingModify
        // (evolution view filters on LineKind::PendingDelete, so this prevents drops)
        let old = "const foo = 4";
        let new = "const bar = 5";

        let engine = DiffEngine::new().with_word_level(true);
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        nav.next(); // active change

        let view = nav.current_view_with_frame(AnimationFrame::FadeOut);
        assert_eq!(view.len(), 1);
        assert_eq!(
            view[0].kind,
            LineKind::PendingModify,
            "Active word-level line should have PendingModify kind during FadeOut"
        );

        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);
        assert_eq!(view.len(), 1);
        assert_eq!(
            view[0].kind,
            LineKind::PendingModify,
            "Active word-level line should have PendingModify kind during FadeIn"
        );
    }

    #[test]
    fn test_primary_active_unique_when_active_change_set() {
        // When active_change is set, exactly one line should be is_primary_active
        // and that line must also be is_active
        let old = "a\nb\nc\n";
        let new = "a\nB\nc\n";
        let diff = DiffEngine::new().diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        nav.next();
        let view = nav.current_view_with_frame(AnimationFrame::FadeIn);

        let primary: Vec<_> = view.iter().filter(|l| l.is_primary_active).collect();
        assert_eq!(
            primary.len(),
            1,
            "Exactly one line should be primary active"
        );
        assert!(
            primary[0].is_active,
            "Primary active line must also be is_active"
        );
    }

    #[test]
    fn test_hunk_extent_not_primary() {
        // Multi-line hunk: all lines should be active during animation,
        // but only one should be is_primary_active (for gutter marker)
        let old = "a\nb\nc\nd\n";
        let new = "A\nb\nC\nd\n"; // A and C form one hunk (b is unchanged but within proximity)
        let diff = DiffEngine::new().diff_strings(old, new);

        assert_eq!(
            diff.hunks.len(),
            1,
            "Fixture should produce a single hunk; adjust the unchanged gap if this fails"
        );

        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        nav.next_hunk();
        let view = nav.current_view_with_frame(AnimationFrame::FadeOut);

        // During animation, whole hunk lights up as active
        let active = view.iter().filter(|l| l.is_active).count();
        let extent = view.iter().filter(|l| l.show_hunk_extent).count();
        let primary = view.iter().filter(|l| l.is_primary_active).count();

        assert!(
            active > 1,
            "Multiple lines should be active during animation, got {}",
            active
        );
        assert!(
            extent > 1,
            "Multiple lines should show extent markers, got {}",
            extent
        );
        assert_eq!(primary, 1, "Exactly one line should be primary active");
    }

    #[test]
    fn test_hunk_extent_while_stepping() {
        // When stepping (not hunk-nav), extent markers should still show
        // if explicitly enabled by the UI.
        let old = "one\ntwo\nthree\nfour\n";
        let new = "ONE\nTWO\nthree\nfour\n";
        let diff = DiffEngine::new().diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        nav.next();
        nav.set_show_hunk_extent_while_stepping(true);
        let view = nav.current_view_with_frame(AnimationFrame::Idle);

        let extent = view.iter().filter(|l| l.show_hunk_extent).count();
        assert!(
            extent > 0,
            "Extent markers should show while stepping when enabled"
        );
    }

    #[test]
    fn test_applied_changes_stay_prefix_during_hunk_ops() {
        let old = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12\n";
        let new = "l1\nL2\nL3\nl4\nl5\nl6\nl7\nl8\nL9\nl10\nl11\nl12\n";
        let diff = DiffEngine::new().diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        assert_applied_is_prefix(&nav);

        nav.next_hunk();
        assert_applied_is_prefix(&nav);

        nav.next();
        assert_applied_is_prefix(&nav);

        nav.next_hunk();
        assert_applied_is_prefix(&nav);

        nav.prev_hunk();
        assert_applied_is_prefix(&nav);
    }

    #[test]
    fn test_applied_prefix_after_preview_step_down() {
        // One hunk with multiple changes so preview mode is used.
        let old = "a\nb\nc\n";
        let new = "A\nB\nc\n";
        let diff = DiffEngine::new().diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        nav.next_hunk();
        assert!(nav.state().hunk_preview_mode);
        assert_applied_is_prefix(&nav);

        nav.next(); // dissolve preview for step down
        assert!(!nav.state().hunk_preview_mode);
        assert_applied_is_prefix(&nav);
    }

    #[test]
    fn test_applied_prefix_after_preview_step_up() {
        // Two hunks so stepping up from preview exits to previous hunk.
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "A\nb\nc\nd\ne\nf\nG\nh\n";
        let diff = DiffEngine::new().diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        nav.next_hunk();
        assert!(nav.state().hunk_preview_mode);
        assert_applied_is_prefix(&nav);

        nav.prev(); // dissolve preview for step up
        assert!(!nav.state().hunk_preview_mode);
        assert_applied_is_prefix(&nav);
    }

    #[test]
    fn test_primary_active_fallback_when_active_change_none() {
        // When active_change is None but animating_hunk is set,
        // the first line in the hunk should become primary and be active
        let old = "a\nb\nc\n";
        let new = "A\nb\nC\n"; // two changes in same hunk
        let diff = DiffEngine::new().diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, Arc::from(old), Arc::from(new), false);

        // Force animating hunk without active_change
        nav.state_mut().animating_hunk = Some(0);
        nav.state_mut().active_change = None;
        nav.state_mut().step_direction = StepDirection::Forward;

        let view = nav.current_view_with_frame(AnimationFrame::FadeOut);

        let primary: Vec<_> = view.iter().filter(|l| l.is_primary_active).collect();
        assert_eq!(
            primary.len(),
            1,
            "Exactly one line should be primary active"
        );
        assert!(
            primary[0].is_active,
            "Primary active line must also be is_active"
        );

        // Verify it's the first active line in the view
        let first_active_idx = view.iter().position(|l| l.is_active).unwrap();
        let first_primary_idx = view.iter().position(|l| l.is_primary_active).unwrap();
        assert_eq!(
            first_active_idx, first_primary_idx,
            "First active line should be the primary line"
        );
    }
}
