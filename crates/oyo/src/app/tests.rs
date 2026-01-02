use super::utils::{
    allow_overscroll_state, evolution_display_metrics, max_scroll, split_display_metrics,
};
use super::*;
use oyo_core::{LineKind, MultiFileDiff, StepDirection, ViewLine};

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

fn make_app_with_two_hunks() -> App {
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
}

fn make_app_with_unified_hunk() -> App {
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
}

fn make_app_with_unified_hunk_two_changes() -> App {
    let old = "one\ntwo\nthree\nfour".to_string();
    let new = "ONE\nTWO\nthree\nfour".to_string();
    let multi_diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        old,
        new,
    );
    App::new(multi_diff, ViewMode::UnifiedPane, 0, false, None)
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
