//! Step-through navigation for diffs

use crate::change::{Change, ChangeKind, ChangeSpan};
use crate::diff::DiffResult;
use serde::{Deserialize, Serialize};

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
}

/// Navigator for stepping through diff changes
pub struct DiffNavigator {
    /// The diff result we're navigating
    diff: DiffResult,
    /// Current step state
    state: StepState,
    /// Original content (for reconstructing views)
    old_content: String,
    /// New content (for reconstructing views)
    new_content: String,
    /// Mapping from change ID to hunk index
    change_to_hunk: std::collections::HashMap<usize, usize>,
}

impl DiffNavigator {
    pub fn new(diff: DiffResult, old_content: String, new_content: String) -> Self {
        let total_changes = diff.significant_changes.len();
        let total_hunks = diff.hunks.len();

        // Build change ID to hunk index mapping
        let mut change_to_hunk = std::collections::HashMap::new();
        for (hunk_idx, hunk) in diff.hunks.iter().enumerate() {
            for &change_id in &hunk.change_ids {
                change_to_hunk.insert(change_id, hunk_idx);
            }
        }

        Self {
            diff,
            state: StepState::new(total_changes, total_hunks),
            old_content,
            new_content,
            change_to_hunk,
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
        true
    }

    /// Get the diff result
    pub fn diff(&self) -> &DiffResult {
        &self.diff
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
            self.state.applied_changes.push(change_id);
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

        let hunk = &self.diff.hunks[self.state.current_hunk];

        // If hunk has only one change, stepping down exits the hunk
        if hunk.change_ids.len() <= 1 {
            self.state.hunk_preview_mode = false;
            // Let normal next() handle moving to next change/hunk
            return self.next();
        }

        // Keep only first change, unapply the rest
        let first_change = hunk.change_ids[0];
        let second_change = hunk.change_ids[1];

        // Remove all changes in this hunk except the first
        self.state
            .applied_changes
            .retain(|&id| !hunk.change_ids.contains(&id) || id == first_change);

        // Apply second change
        self.state.applied_changes.push(second_change);

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
        if let Some(unapplied_change_id) = self.state.applied_changes.pop() {
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
        let hunk = &self.diff.hunks[current_hunk_idx];

        // Set animating hunk for backward fade animation
        self.state.animating_hunk = Some(current_hunk_idx);

        // Unapply all changes in this hunk
        for &change_id in &hunk.change_ids {
            self.state.applied_changes.retain(|&id| id != change_id);
        }

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

    /// Jump to a specific step
    pub fn goto(&mut self, step: usize) {
        let target_step = step.min(self.state.total_steps - 1);

        // Reset to start
        self.state.current_step = 0;
        self.state.applied_changes.clear();
        self.state.active_change = None;
        self.state.cursor_change = None;
        self.state.animating_hunk = None;
        self.state.current_hunk = 0;
        self.state.last_nav_was_hunk = false; // Clear hunk nav flag on goto
        self.state.hunk_preview_mode = false; // Clear preview mode on goto
        self.state.preview_from_backward = false;

        // Apply changes up to target step
        for _ in 0..target_step {
            self.next();
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
            .any(|id| self.state.applied_changes.contains(id));

        // If current hunk has no applied changes, apply ALL changes (full preview)
        if !has_applied_in_current {
            let mut moved = false;
            for &change_id in &current_hunk.change_ids {
                if !self.state.applied_changes.contains(&change_id) {
                    self.state.applied_changes.push(change_id);
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
            if !self.state.applied_changes.contains(&change_id) {
                self.state.applied_changes.push(change_id);
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
            if !self.state.applied_changes.contains(&change_id) {
                self.state.applied_changes.push(change_id);
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
            let has_applied = hunk
                .change_ids
                .iter()
                .any(|id| self.state.applied_changes.contains(id));
            if !has_applied {
                // No movement, restore preview mode
                self.state.hunk_preview_mode = was_in_preview;
                return false;
            }
        }

        self.state.step_direction = StepDirection::Backward;

        // If we have applied changes in current hunk, unapply them
        let current_hunk_idx = self.state.current_hunk;
        let current_hunk = &self.diff.hunks[current_hunk_idx];
        let mut moved = false;

        // Unapply changes from current hunk that are applied
        for &change_id in current_hunk.change_ids.iter().rev() {
            if let Some(pos) = self
                .state
                .applied_changes
                .iter()
                .position(|&id| id == change_id)
            {
                self.state.applied_changes.remove(pos);
                self.state.current_step = self.state.current_step.saturating_sub(1);
                moved = true;
            }
        }

        // Set animating hunk for whole-hunk animation (keep pointing at the hunk
        // being removed so is_change_in_animating_hunk returns true during fade)
        self.state.animating_hunk = Some(current_hunk_idx);
        self.state.active_change = current_hunk.change_ids.first().copied();

        // Move to previous hunk if current is now empty of applied changes
        // (current_hunk tracks cursor position, animating_hunk tracks animation)
        if moved {
            // Check if we should move to previous hunk
            let still_has_applied = current_hunk
                .change_ids
                .iter()
                .any(|id| self.state.applied_changes.contains(id));
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
                self.state.applied_changes.push(change_id);
                self.state.current_step += 1;
            }
        }

        // Apply ALL changes of target hunk (full preview)
        let hunk = &self.diff.hunks[hunk_idx];
        for &change_id in &hunk.change_ids {
            self.state.applied_changes.push(change_id);
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

        let hunk = &self.diff.hunks[self.state.current_hunk];
        let first_change = match hunk.change_ids.first() {
            Some(&id) => id,
            None => return false,
        };

        // Must have at least first change applied to be "inside" hunk
        if !self.state.applied_changes.contains(&first_change) {
            return false;
        }

        // Unapply all changes in this hunk except the first
        let mut unapplied_any = false;
        for &change_id in &hunk.change_ids[1..] {
            if let Some(pos) = self
                .state
                .applied_changes
                .iter()
                .position(|&id| id == change_id)
            {
                self.state.applied_changes.remove(pos);
                self.state.current_step = self.state.current_step.saturating_sub(1);
                unapplied_any = true;
            }
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
        let has_applied = hunk
            .change_ids
            .iter()
            .any(|id| self.state.applied_changes.contains(id));
        if !has_applied {
            return false;
        }

        let last_change = hunk.change_ids.last().copied();

        // Apply all unapplied changes in this hunk
        for &change_id in &hunk.change_ids {
            if !self.state.applied_changes.contains(&change_id) {
                self.state.applied_changes.push(change_id);
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
        if let Some(hunk_idx) = self.state.animating_hunk {
            if let Some(hunk) = self.diff.hunks.get(hunk_idx) {
                return hunk.change_ids.contains(&change_id);
            }
        }
        false
    }

    /// Check if a change belongs to the current hunk (for persistent extent markers)
    fn is_change_in_current_hunk(&self, change_id: usize) -> bool {
        self.diff
            .hunks
            .get(self.state.current_hunk)
            .map(|hunk| hunk.change_ids.contains(&change_id))
            .unwrap_or(false)
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
                let applied = self.state.applied_changes.contains(&c.id);
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

        for change in &self.diff.changes {
            let is_applied = self.state.applied_changes.contains(&change.id);

            // Primary active: cursor destination (decoupled from animation target on backward)
            let is_primary_active = primary_change_id == Some(change.id);

            // Active: part of the animating hunk (for animation styling)
            let is_in_hunk = self.is_change_in_animating_hunk(change.id);
            let is_active_change = self.state.active_change == Some(change.id);
            // Active if: (1) the active_change, or (2) in animating hunk (lights up whole hunk during animation)
            let is_active = is_active_change || is_in_hunk;
            // Show extent marker if animating hunk OR (last nav was hunk AND change in current hunk)
            let show_hunk_extent = is_in_hunk
                || (self.is_change_in_current_hunk(change.id)
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
        let hunk_index = self.change_to_hunk.get(&change.id).copied();
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
        let hunk_index = self.change_to_hunk.get(&change_id).copied();
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
        &self.old_content
    }

    /// Get new content
    pub fn new_content(&self) -> &str {
        &self.new_content
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
    use crate::diff::DiffEngine;

    #[test]
    fn test_navigation() {
        let old = "foo\nbar\nbaz";
        let new = "foo\nqux\nbaz";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        assert!(active_lines.len() >= 1, "fading line should be active");

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
    fn test_markers_persist_within_hunk() {
        // Stepping within a hunk after next_hunk should preserve extent markers
        let old = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8";
        let new = "line1\nLINE2\nLINE3\nline4\nline5\nline6\nline7\nline8";

        let engine = DiffEngine::new();
        let diff = engine.diff_strings(old, new);

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());
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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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

        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
    fn test_primary_active_fallback_when_active_change_none() {
        // When active_change is None but animating_hunk is set,
        // the first line in the hunk should become primary and be active
        let old = "a\nb\nc\n";
        let new = "A\nb\nC\n"; // two changes in same hunk
        let diff = DiffEngine::new().diff_strings(old, new);
        let mut nav = DiffNavigator::new(diff, old.to_string(), new.to_string());

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
