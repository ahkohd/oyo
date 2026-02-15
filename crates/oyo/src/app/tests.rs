use super::utils::{
    allow_overscroll_state, evolution_display_metrics, max_scroll, split_display_metrics,
};
use super::*;
use crate::test_utils::{DiffSettingsGuard, TestApp};
use oyo_core::{LineKind, MultiFileDiff, StepDirection, ViewLine};
use std::sync::{Mutex, MutexGuard};

static VIEW_DEBUG_ENV_LOCK: Mutex<()> = Mutex::new(());

struct ViewDebugEnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_view: Option<std::ffi::OsString>,
    old_view_nav: Option<std::ffi::OsString>,
    old_view_file: Option<std::ffi::OsString>,
}

impl ViewDebugEnvGuard {
    fn new(path: &std::path::Path) -> Self {
        let lock = VIEW_DEBUG_ENV_LOCK.lock().unwrap();
        let old_view = std::env::var_os("OYO_DEBUG_VIEW");
        let old_view_nav = std::env::var_os("OYO_DEBUG_VIEW_NAV");
        let old_view_file = std::env::var_os("OYO_DEBUG_VIEW_FILE");
        std::env::set_var("OYO_DEBUG_VIEW", "1");
        std::env::set_var("OYO_DEBUG_VIEW_NAV", "1");
        std::env::set_var("OYO_DEBUG_VIEW_FILE", path);
        Self {
            _lock: lock,
            old_view,
            old_view_nav,
            old_view_file,
        }
    }
}

impl Drop for ViewDebugEnvGuard {
    fn drop(&mut self) {
        match &self.old_view {
            Some(val) => std::env::set_var("OYO_DEBUG_VIEW", val),
            None => std::env::remove_var("OYO_DEBUG_VIEW"),
        }
        match &self.old_view_nav {
            Some(val) => std::env::set_var("OYO_DEBUG_VIEW_NAV", val),
            None => std::env::remove_var("OYO_DEBUG_VIEW_NAV"),
        }
        match &self.old_view_file {
            Some(val) => std::env::set_var("OYO_DEBUG_VIEW_FILE", val),
            None => std::env::remove_var("OYO_DEBUG_VIEW_FILE"),
        }
    }
}

#[test]
fn test_allow_overscroll_state() {
    assert!(!allow_overscroll_state(false, false, false));
    assert!(allow_overscroll_state(false, false, true));
    assert!(!allow_overscroll_state(false, true, false));
    assert!(!allow_overscroll_state(true, false, false));
    assert!(allow_overscroll_state(true, true, false));
    assert!(allow_overscroll_state(true, true, true));
    assert!(allow_overscroll_state(true, false, true));
}

#[test]
fn test_max_scroll_normal() {
    assert_eq!(max_scroll(100, 20, false), 80);
    assert_eq!(max_scroll(50, 10, false), 40);
    assert_eq!(max_scroll(20, 20, false), 0);
    assert_eq!(max_scroll(5, 20, false), 0);
}

#[test]
fn test_max_scroll_overscroll() {
    assert_eq!(max_scroll(100, 20, true), 89);
    assert_eq!(max_scroll(50, 10, true), 44);
    assert_eq!(max_scroll(5, 20, true), 0);
    assert_eq!(max_scroll(1, 20, true), 0);
}

fn make_view_line(
    kind: LineKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    is_active: bool,
    is_primary_active: bool,
) -> ViewLine {
    ViewLine {
        content: String::new(),
        spans: vec![],
        kind,
        old_line,
        new_line,
        is_active,
        is_active_change: is_active,
        is_primary_active,
        show_hunk_extent: false,
        change_id: 0,
        hunk_index: None,
        has_changes: kind != LineKind::Context,
    }
}

#[test]
fn test_evolution_metrics_skips_deleted() {
    let view = vec![
        make_view_line(LineKind::Context, Some(1), Some(1), false, false),
        make_view_line(LineKind::Deleted, Some(2), None, false, false),
        make_view_line(LineKind::Deleted, Some(3), None, false, false),
        make_view_line(LineKind::Context, Some(4), Some(2), true, true),
    ];
    let (len, idx) = evolution_display_metrics(&view, AnimationPhase::Idle);
    assert_eq!(len, 2);
    assert_eq!(idx, Some(1));
}

#[test]
fn test_evolution_metrics_pending_delete_visibility() {
    let view = vec![
        make_view_line(LineKind::Context, Some(1), Some(1), false, false),
        make_view_line(LineKind::PendingDelete, Some(2), None, true, true),
        make_view_line(LineKind::Context, Some(3), Some(2), false, false),
    ];

    let (len, idx) = evolution_display_metrics(&view, AnimationPhase::Idle);
    assert_eq!(len, 2);
    assert_eq!(idx, None);

    let (len, idx) = evolution_display_metrics(&view, AnimationPhase::FadeOut);
    assert_eq!(len, 3);
    assert_eq!(idx, Some(1));

    let (len, idx) = evolution_display_metrics(&view, AnimationPhase::FadeIn);
    assert_eq!(len, 3);
    assert_eq!(idx, Some(1));
}

#[test]
fn test_split_metrics_primary_dominates() {
    let view = vec![
        make_view_line(LineKind::Context, Some(1), Some(1), true, false),
        make_view_line(LineKind::Context, Some(2), Some(2), false, false),
        make_view_line(LineKind::Inserted, None, Some(3), true, true),
    ];
    let (len, idx) = split_display_metrics(&view, 0, StepDirection::Forward, false);
    assert_eq!(len, 3);
    assert_eq!(idx, Some(2));
}

#[test]
fn test_split_metrics_minimize_jump() {
    let view = vec![
        make_view_line(LineKind::Context, Some(1), Some(1), false, false),
        make_view_line(LineKind::Context, Some(2), Some(2), false, false),
        make_view_line(LineKind::Modified, Some(3), Some(3), true, true),
        make_view_line(LineKind::Context, Some(4), Some(4), false, false),
    ];
    let (_, idx) = split_display_metrics(&view, 0, StepDirection::Forward, false);
    assert_eq!(idx, Some(2));

    let (_, idx) = split_display_metrics(&view, 0, StepDirection::Backward, false);
    assert_eq!(idx, Some(2));

    let (_, idx) = split_display_metrics(&view, 10, StepDirection::Forward, false);
    assert_eq!(idx, Some(2));
}

#[test]
fn test_split_metrics_fallback_when_no_primary() {
    let view = vec![
        make_view_line(LineKind::Context, Some(1), Some(1), false, false),
        make_view_line(LineKind::Context, Some(2), Some(2), true, false),
        make_view_line(LineKind::Context, Some(3), Some(3), false, false),
    ];
    let (len, idx) = split_display_metrics(&view, 0, StepDirection::Forward, false);
    assert_eq!(len, 3);
    assert_eq!(idx, Some(1));
}

fn make_app_with_two_hunks() -> TestApp {
    TestApp::new_default(|| {
        let old_lines: Vec<String> = (1..=25).map(|i| format!("line{}", i)).collect();
        let mut new_lines = old_lines.clone();
        new_lines[1] = "line2-new".to_string();
        new_lines[19] = "line20-new".to_string();
        let old = old_lines.join("\n");
        let new = new_lines.join("\n");

        let multi_diff = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            old,
            new,
        );
        let mut app = App::new(multi_diff, ViewMode::UnifiedPane, 0, false, None);
        app.stepping = false;
        app.enter_no_step_mode();
        app
    })
}

fn make_app_with_unified_hunk() -> TestApp {
    TestApp::new_default(|| {
        let old = "one\ntwo\nthree".to_string();
        let new = "one\nTWO\nthree".to_string();
        let multi_diff = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            old,
            new,
        );
        let mut app = App::new(multi_diff, ViewMode::UnifiedPane, 0, false, None);
        app.stepping = false;
        app.enter_no_step_mode();
        app
    })
}

fn make_app_with_unified_hunk_two_changes() -> TestApp {
    TestApp::new_default(|| {
        let old = "one\ntwo\nthree\nfour".to_string();
        let new = "ONE\nTWO\nthree\nfour".to_string();
        let multi_diff = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            old,
            new,
        );
        App::new(multi_diff, ViewMode::UnifiedPane, 0, false, None)
    })
}

fn make_large_app(lines: usize, change_line: usize) -> App {
    let old_lines: Vec<String> = (0..lines).map(|i| format!("line{}", i)).collect();
    let mut new_lines = old_lines.clone();
    new_lines[change_line] = format!("LINE{}", change_line);
    let old = old_lines.join("\n");
    let new = new_lines.join("\n");

    let mut multi_diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        old.clone(),
        new.clone(),
    );
    let diff = MultiFileDiff::compute_diff(&old, &new);
    multi_diff.apply_diff_result(0, diff);
    multi_diff.ensure_full_navigator(0);

    let mut app = App::new(multi_diff, ViewMode::UnifiedPane, 0, false, None);
    app.stepping = false;
    app.no_step_auto_jump_on_enter = false;
    app.enter_no_step_mode();
    app
}

fn make_large_step_app(lines: usize, change_lines: &[usize]) -> App {
    let old_lines: Vec<String> = (0..lines).map(|i| format!("line{}", i)).collect();
    let mut new_lines = old_lines.clone();
    for &idx in change_lines {
        if idx < new_lines.len() {
            new_lines[idx] = format!("LINE{}", idx);
        }
    }
    let old = old_lines.join("\n");
    let new = new_lines.join("\n");

    let mut multi_diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        old.clone(),
        new.clone(),
    );
    let diff = MultiFileDiff::compute_diff(&old, &new);
    multi_diff.apply_diff_result(0, diff);
    multi_diff.ensure_full_navigator(0);

    let mut app = App::new(multi_diff, ViewMode::UnifiedPane, 0, false, None);
    app.no_step_auto_jump_on_enter = false;
    app
}

#[test]
fn test_no_step_prev_hunk_from_bottom_advances() {
    let mut app = make_app_with_two_hunks();
    let total_hunks = app.multi_diff.current_navigator().state().total_hunks;
    assert_eq!(total_hunks, 2);

    app.goto_end();
    app.prev_hunk_scroll();
    {
        let state = app.multi_diff.current_navigator().state();
        assert!(state.cursor_change.is_some());
        assert!(state.last_nav_was_hunk);
    }

    app.prev_hunk_scroll();
    let state = app.multi_diff.current_navigator().state();
    assert_eq!(state.current_hunk, 0);
}

#[test]
fn test_no_step_next_hunk_after_goto_start() {
    let mut app = make_app_with_two_hunks();
    app.goto_start();

    app.next_hunk_scroll();
    let state = app.multi_diff.current_navigator().state();
    assert_eq!(state.current_hunk, 0);
    assert!(state.cursor_change.is_some());
    assert!(state.last_nav_was_hunk);
}

#[test]
fn test_unified_hunk_jump_sets_cursor() {
    let mut app = make_app_with_unified_hunk();
    app.next_hunk_scroll();
    let state = app.multi_diff.current_navigator().state();
    assert_eq!(state.total_hunks, 1);
    assert_eq!(state.current_hunk, 0);
    assert!(state.cursor_change.is_some());
    assert!(state.last_nav_was_hunk);
}

#[test]
fn test_goto_start_clears_hunk_scope_in_no_step() {
    let mut app = make_app_with_two_hunks();
    app.next_hunk_scroll();
    app.goto_start();

    let state = app.multi_diff.current_navigator().state();
    assert!(!state.last_nav_was_hunk);
    assert!(state.cursor_change.is_none());
}

#[test]
fn test_goto_end_clears_hunk_scope_in_no_step() {
    let mut app = make_app_with_two_hunks();
    app.next_hunk_scroll();
    app.goto_end();

    let state = app.multi_diff.current_navigator().state();
    assert!(!state.last_nav_was_hunk);
    assert!(state.cursor_change.is_none());
}

#[test]
fn test_no_step_b_e_jump_within_hunk() {
    let mut app = make_app_with_two_hunks();
    app.next_hunk_scroll();

    let state = app.multi_diff.current_navigator().state();
    let current_hunk = state.current_hunk;

    app.goto_hunk_end_scroll();
    let end_state = app.multi_diff.current_navigator().state();
    assert_eq!(end_state.current_hunk, current_hunk);
    assert!(end_state.cursor_change.is_some());

    app.goto_hunk_start_scroll();
    let start_state = app.multi_diff.current_navigator().state();
    assert_eq!(start_state.current_hunk, current_hunk);
    assert!(start_state.cursor_change.is_some());
}

#[test]
fn test_toggle_stepping_restores_no_step_cursor_scope() {
    let mut app = make_app_with_two_hunks();
    app.next_hunk_scroll();

    let before = app.multi_diff.current_navigator().state().clone();
    assert!(before.last_nav_was_hunk);
    assert!(before.cursor_change.is_some());

    app.toggle_stepping();
    assert!(app.stepping);
    app.toggle_stepping();

    let after = app.multi_diff.current_navigator().state();
    assert_eq!(after.current_hunk, before.current_hunk);
    assert_eq!(after.cursor_change, before.cursor_change);
    assert!(after.last_nav_was_hunk);
}

#[test]
fn test_hunk_step_info_counts_applied_changes() {
    let mut app = make_app_with_unified_hunk_two_changes();
    assert_eq!(app.hunk_step_info(), Some((0, 2)));

    app.next_step();
    assert_eq!(app.hunk_step_info(), Some((1, 2)));

    app.next_step();
    assert_eq!(app.hunk_step_info(), Some((2, 2)));
}

#[test]
fn test_no_step_snapshot_restores_cursor_or_jumps() {
    let _guard = DiffSettingsGuard::default();
    let old_lines: Vec<String> = (1..=25).map(|i| format!("line{}", i)).collect();
    let mut new_lines = old_lines.clone();
    new_lines[1] = "line2-new".to_string();
    new_lines[19] = "line20-new".to_string();
    let old = old_lines.join("\n");
    let new = new_lines.join("\n");

    let multi_diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        old,
        new,
    );
    let mut app = App::new(multi_diff, ViewMode::UnifiedPane, 0, false, None);
    app.stepping = false;
    app.no_step_auto_jump_on_enter = true;
    app.enter_no_step_mode();

    let idx = app.multi_diff.selected_index;
    app.save_no_step_state_snapshot(idx);
    app.multi_diff.current_navigator().clear_cursor_change();
    app.multi_diff.current_navigator().set_hunk_scope(false);

    assert!(app.restore_no_step_state_snapshot(idx));
    let cursor_id = app
        .multi_diff
        .current_navigator()
        .state()
        .cursor_change
        .expect("cursor change expected");
    assert!(cursor_id > 0);
}

#[test]
fn test_no_step_cursor_stable_through_file_cycles() {
    let _guard = DiffSettingsGuard::default();
    let old_lines: Vec<String> = (1..=25).map(|i| format!("line{}", i)).collect();
    let mut new_lines = old_lines.clone();
    new_lines[1] = "line2-new".to_string();
    new_lines[19] = "line20-new".to_string();
    let old = old_lines.join("\n");
    let new = new_lines.join("\n");

    let multi = MultiFileDiff::from_file_pairs(vec![
        (std::path::PathBuf::from("a.txt"), old.clone(), new.clone()),
        (std::path::PathBuf::from("b.txt"), old.clone(), new.clone()),
        (std::path::PathBuf::from("c.txt"), old.clone(), new.clone()),
    ]);
    let mut app = App::new(multi, ViewMode::UnifiedPane, 0, false, None);
    app.stepping = false;
    app.no_step_auto_jump_on_enter = true;
    app.enter_no_step_mode();

    app.goto_hunk_start_scroll();
    let first_cursor = app.multi_diff.current_navigator().state().cursor_change;

    app.next_file();
    app.next_file();
    app.prev_file();
    app.prev_file();

    let cursor_after = app.multi_diff.current_navigator().state().cursor_change;

    assert_eq!(first_cursor, cursor_after);
}

#[test]
fn test_windowed_view_tracks_scroll_offset_in_no_step_large_file() {
    let _guard = DiffSettingsGuard::new(64);
    let mut app = make_large_app(600, 320);
    app.last_viewport_height = 25;
    app.scroll_offset = 250;

    let view = app.current_view_with_frame(AnimationFrame::Idle);

    assert!(app.view_windowed());
    let start = app.view_window_start();
    assert!(start <= app.scroll_offset);
    assert_eq!(app.render_scroll_offset(), app.scroll_offset - start);

    let span = app.last_viewport_height.max(20).saturating_mul(4).max(200);
    assert!(view.len() <= span.saturating_add(1));
}

#[test]
fn test_step_jump_waits_for_view_rebuild_before_scroll() {
    let _guard = DiffSettingsGuard::new(64);
    let change_lines: Vec<usize> = (0..600).collect();
    let mut app = make_large_step_app(600, &change_lines);
    app.view_mode = ViewMode::Split;
    app.split_align_lines = true;
    app.last_viewport_height = 25;

    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    app.defer_view_build_for_jump();
    app.goto_last_step();
    assert!(app.needs_scroll_to_active);

    app.ensure_active_visible_if_needed(app.last_viewport_height);
    assert!(
        app.needs_scroll_to_active,
        "deferred view should keep active scroll pending"
    );

    app.ensure_active_visible_if_needed(app.last_viewport_height);
    assert!(!app.needs_scroll_to_active);
    let state = app.multi_diff.current_navigator().state().clone();
    let window_start = app.view_window_start();
    let pending = app.view_build_pending();
    let scroll_offset = app.scroll_offset;
    assert!(
        scroll_offset > 0,
        "scroll_offset={} window_start={} pending={} active_change={:?} current_step={} step_dir={:?}",
        scroll_offset,
        window_start,
        pending,
        state.active_change,
        state.current_step,
        state.step_direction
    );
    assert!(window_start > 0);
}

#[test]
fn test_no_step_end_scroll_does_not_shift_window() {
    let _guard = DiffSettingsGuard::new(64);
    let mut app = make_large_app(600, 320);
    app.last_viewport_height = 72;
    let total_len = app.multi_diff.current_navigator().diff().changes.len();
    let max = max_scroll(total_len, app.last_viewport_height, app.allow_overscroll());
    app.scroll_offset = max;

    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    let start = app.view_window_start();

    app.scroll_down();
    let _ = app.current_view_with_frame(AnimationFrame::Idle);

    assert_eq!(app.scroll_offset, max);
    assert_eq!(app.view_window_start(), start);
    assert_eq!(app.render_scroll_offset(), app.scroll_offset - start);
}

#[test]
fn test_no_step_goto_end_preserves_hunk_scope() {
    let _guard = DiffSettingsGuard::new(64);
    let mut app = make_large_app(600, 599);
    app.view_mode = ViewMode::Split;
    app.split_align_lines = true;
    app.last_viewport_height = 25;

    app.goto_last_hunk_scroll();
    let view = app.current_view_with_frame(AnimationFrame::Idle);
    let state = app.multi_diff.current_navigator().state();
    assert!(state.last_nav_was_hunk);
    assert!(view.iter().any(|line| line.show_hunk_extent));

    app.goto_end();
    let view = app.current_view_with_frame(AnimationFrame::Idle);
    let state = app.multi_diff.current_navigator().state();
    assert!(state.last_nav_was_hunk);
    assert!(view.iter().any(|line| line.show_hunk_extent));
}

#[test]
fn test_no_step_goto_end_updates_hunk_scope_after_scroll() {
    let _guard = DiffSettingsGuard::new(64);
    let mut app = make_large_step_app(600, &[10, 590]);
    app.stepping = false;
    app.no_step_auto_jump_on_enter = false;
    app.enter_no_step_mode();
    app.view_mode = ViewMode::Split;
    app.split_align_lines = true;
    app.last_viewport_height = 25;

    app.goto_hunk_index_scroll(0);
    app.goto_end();
    let view = app.current_view_with_frame(AnimationFrame::Idle);
    let state = app.multi_diff.current_navigator().state();
    assert!(state.last_nav_was_hunk);
    assert!(view.iter().any(|line| line.show_hunk_extent));
}

#[test]
fn test_no_step_hunk_scope_shows_extent_in_windowed_view() {
    let _guard = DiffSettingsGuard::new(64);
    let mut app = make_large_app(600, 320);
    app.last_viewport_height = 25;

    app.next_hunk_scroll();
    let view = app.current_view_with_frame(AnimationFrame::Idle);

    let state = app.multi_diff.current_navigator().state();
    assert!(state.last_nav_was_hunk);
    assert!(state.cursor_change.is_some());
    assert!(app.view_windowed());
    assert!(view.iter().any(|line| line.show_hunk_extent));
}

#[test]
fn test_step_hunk_nav_clears_view_build_defer_in_large_file() {
    let _guard = DiffSettingsGuard::new(64);
    let mut app = make_large_step_app(600, &[50, 450]);
    app.last_viewport_height = 25;

    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    app.next_hunk();
    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    app.defer_view_build_for_jump();
    assert!(app.view_build_defer);
    let hunk_before = app.multi_diff.current_navigator().state().current_hunk;
    app.next_hunk();
    let hunk_after = app.multi_diff.current_navigator().state().current_hunk;
    assert_ne!(hunk_before, hunk_after);
    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    assert!(
        !app.view_build_pending(),
        "hunk nav should rebuild immediately without pending view"
    );

    app.defer_view_build_for_jump();
    assert!(app.view_build_defer);
    app.prev_hunk();
    let hunk_back = app.multi_diff.current_navigator().state().current_hunk;
    assert_ne!(hunk_after, hunk_back);
    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    assert!(
        !app.view_build_pending(),
        "reverse hunk nav should rebuild immediately without pending view"
    );
}

#[test]
fn test_view_nav_logging_emits_entry() {
    let _guard = DiffSettingsGuard::default();
    let path = std::env::temp_dir().join(format!("oyo_view_nav_test_{}.log", std::process::id()));
    let _guard = ViewDebugEnvGuard::new(&path);
    let _ = std::fs::remove_file(&path);

    let old = "line1\nline2\nline3\n";
    let new = "line1\nLINE2\nline3\n";
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        old.to_string(),
        new.to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);

    app.next_step();

    let log = std::fs::read_to_string(&path).expect("read nav log");
    assert!(log.contains("OYO_VIEW_NAV"), "missing nav log header");
    assert!(log.contains("action=step_down"), "missing step_down action");
    assert!(log.contains("moved=true"), "expected moved=true for step");
}

#[test]
fn test_diff_worker_upgrades_deferred_diff_and_updates_counts() {
    let _guard = DiffSettingsGuard::new(32);
    let old = "line1\nline2\nline3\nline4\nline5\nline6\n";
    let new = "line1\nLINE2\nline3\nline4\nline5\nline6\n";
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        old.to_string(),
        new.to_string(),
    );
    assert_eq!(diff.diff_status(0), DiffStatus::Deferred);

    let expected = MultiFileDiff::compute_diff(old, new);

    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.stepping = false;
    app.no_step_auto_jump_on_enter = false;
    app.enter_no_step_mode();

    let _ = app.multi_diff.current_navigator();
    assert!(app.multi_diff.current_navigator_is_placeholder());

    app.queue_current_file_diff();

    let mut ready = false;
    for _ in 0..200 {
        app.poll_diff_responses();
        if matches!(app.multi_diff.current_file_diff_status(), DiffStatus::Ready) {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert!(ready, "diff worker did not finish");
    assert!(!app.multi_diff.current_navigator_is_placeholder());
    let file = &app.multi_diff.files[0];
    assert_eq!(file.insertions, expected.insertions);
    assert_eq!(file.deletions, expected.deletions);
}
