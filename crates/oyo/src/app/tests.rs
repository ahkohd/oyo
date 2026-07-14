use super::utils::{
    allow_overscroll_state, evolution_display_metrics, max_scroll, split_display_metrics,
};
use super::*;
use crate::test_utils::{DiffSettingsGuard, TestApp};
use oyo_core::{LineKind, MultiFileDiff, StepDirection, ViewLine};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

static VIEW_DEBUG_ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn syntax_cache_creation_invalidates_plain_render_cache() {
    let diff = MultiFileDiff::from_file_pair(
        "test.rs".into(),
        "test.rs".into(),
        "fn main() {}\n".into(),
        "fn main() { 1; }\n".into(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    assert_eq!(app.syntax_cache_epoch(), 0);
    assert!(app.ensure_syntax_cache().is_some());
    assert_eq!(app.syntax_cache_epoch(), 1);
}

fn run_git(repo: &std::path::Path, args: &[&str]) {
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("run git")
        .success());
}

fn git_rev(repo: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read git revision");
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn finish_outdated_reconstruction(app: &mut App) {
    for _ in 0..500 {
        app.poll_outdated_reconstruction_responses();
        if !app.outdated_reconstruction_pending() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("outdated reconstruction timed out");
}

fn jj_output(repo: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("jj")
        .current_dir(repo)
        .arg("-R")
        .arg(repo)
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .args(args)
        .output()
        .expect("run jj");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn run_jj(repo: &std::path::Path, args: &[&str]) {
    let _ = jj_output(repo, args);
}

fn tracked_watch_repo(name: &str) -> std::path::PathBuf {
    let repo = std::env::temp_dir().join(format!(
        "oyo_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&repo).expect("create repo");
    run_git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), "readme\n").expect("write readme");
    std::fs::write(repo.join("other.txt"), "other\n").expect("write other");
    run_git(&repo, &["add", "."]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "initial",
        ],
    );
    repo
}

fn quit_test_app() -> App {
    App::new(
        MultiFileDiff::from_file_pair(
            "old.txt".into(),
            "new.txt".into(),
            "old\n".to_string(),
            "new\n".to_string(),
        ),
        ViewMode::UnifiedPane,
        0,
        false,
        None,
    )
}

#[test]
fn quit_confirmation_opens_and_cancel_keeps_running() {
    for code in [
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyCode::Char('n'),
        crossterm::event::KeyCode::Char('x'),
    ] {
        let mut app = quit_test_app();
        app.request_quit();
        assert!(app.quit_confirmation_active());
        assert!(!app.should_quit);

        app.handle_quit_confirmation_key(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(!app.quit_confirmation_active());
        assert!(!app.should_quit);
    }
}

#[test]
fn quit_confirmation_accepts_enter_y_and_q() {
    for code in [
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyCode::Char('y'),
        crossterm::event::KeyCode::Char('q'),
    ] {
        let mut app = quit_test_app();
        app.request_quit();
        app.handle_quit_confirmation_key(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(app.should_quit);
        assert!(!app.quit_confirmation_active());
    }
}

#[test]
fn disabled_quit_confirmation_exits_immediately() {
    let mut app = quit_test_app();
    app.confirm_quit = false;

    app.request_quit();

    assert!(app.should_quit);
    assert!(!app.quit_confirmation_active());
}

#[test]
fn command_palette_lists_and_opens_pickers() {
    let mut app = quit_test_app();
    app.enable_review_mode();
    app.start_command_palette();
    let labels = app
        .command_palette_filtered_entries()
        .into_iter()
        .map(|entry| entry.label)
        .collect::<Vec<_>>();

    assert!(labels.iter().any(|label| label == "Files..."));
    assert!(labels.iter().any(|label| label == "Comments..."));
    assert!(labels.iter().any(|label| label == "Themes..."));
    assert!(labels.iter().any(|label| label == "History..."));
    assert!(labels.iter().any(|label| label == "Toggle step mode"));
    assert!(labels.iter().any(|label| label == "Cycle view modes"));
    assert!(labels.iter().any(|label| label == "Pull request comments"));

    app.clear_command_palette_text();
    for ch in "Files...".chars() {
        app.push_command_palette_char(ch);
    }
    app.apply_command_palette_selection();
    assert!(app.file_search_active());
}

#[test]
fn comment_picker_cursor_blinks() {
    let mut app = quit_test_app();
    app.enable_review_mode();
    app.start_comment_picker();
    app.file_filter_cursor_visible = true;
    app.file_filter_cursor_last_blink = Instant::now() - Duration::from_millis(500);

    assert!(app.tick());
    assert!(!app.file_filter_cursor_visible);
}

#[test]
fn expandable_folds_preserve_step_targets_and_work_without_stepping() {
    let _guard = DiffSettingsGuard::default();
    let old = (1..=60)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut new_lines = old.lines().map(str::to_string).collect::<Vec<_>>();
    new_lines[29] = "changed line".to_string();
    let new = new_lines.join("\n");
    let make_diff = || {
        MultiFileDiff::from_file_pair(
            "fold.txt".into(),
            "fold.txt".into(),
            format!("{old}\n"),
            format!("{new}\n"),
        )
    };

    let mut app = App::new(make_diff(), ViewMode::UnifiedPane, 0, false, None);
    app.next_step();
    let folded = app.current_view_with_frame(AnimationFrame::Idle);
    assert_eq!(folded.iter().filter(|line| is_fold_line(line)).count(), 2);
    let active = folded
        .iter()
        .position(|line| line.is_primary_active || line.is_active)
        .unwrap();
    assert!(folded
        .iter()
        .enumerate()
        .filter(|(_, line)| is_fold_line(line))
        .all(|(index, _)| index.abs_diff(active) >= 4));
    app.start_search();
    for ch in "lines".chars() {
        app.push_search_char(ch);
    }
    app.search_next();
    assert_eq!(app.search_target(), None);
    app.clear_search();
    let state = app.multi_diff.current_navigator().state();
    let target = (state.current_step, state.current_hunk, state.total_steps);

    assert!(app.expand_all_context_folds());
    let state = app.multi_diff.current_navigator().state();
    assert_eq!(
        (state.current_step, state.current_hunk, state.total_steps),
        target
    );
    assert!(app
        .current_view_with_frame(AnimationFrame::Idle)
        .iter()
        .all(|line| !is_fold_line(line)));

    app.toggle_fold_context();
    app.toggle_fold_context();
    assert_eq!(app.fold_context, FoldContextMode::Expandable);
    assert_eq!(
        app.current_view_with_frame(AnimationFrame::Idle)
            .iter()
            .filter(|line| is_fold_line(line))
            .count(),
        2
    );

    let mut no_step = App::new(make_diff(), ViewMode::UnifiedPane, 0, false, None);
    no_step.toggle_stepping();
    assert_eq!(
        no_step
            .current_view_with_frame(AnimationFrame::Idle)
            .iter()
            .filter(|line| is_fold_line(line))
            .count(),
        2
    );
}

#[test]
fn full_context_watch_refresh_keeps_new_eof_change_and_comment_visible() {
    let _guard = DiffSettingsGuard::default();
    let repo = tracked_watch_repo("full_context_eof");
    let base = (1..=40)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>();
    std::fs::write(repo.join("README.md"), format!("{}\n", base.join("\n")))
        .expect("write long base");
    run_git(&repo, &["add", "README.md"]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--amend",
            "--no-edit",
            "-q",
        ],
    );

    let mut changed = base.clone();
    changed[14] = "changed line 15".to_string();
    changed.push("append one".to_string());
    std::fs::write(repo.join("README.md"), format!("{}\n", changed.join("\n")))
        .expect("write first change");
    let changes = oyo_core::git::get_uncommitted_changes(&repo).expect("changes");
    let diff = MultiFileDiff::from_git_changes(repo.clone(), changes).expect("diff");
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    app.toggle_stepping();
    app.last_viewport_height = 10;
    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    assert!(app.expand_all_context_folds());
    let expanded = app.current_view_with_frame(AnimationFrame::Idle);
    assert!(expanded.iter().all(|line| !is_fold_line(line)));
    app.scroll_offset = max_scroll(expanded.len(), app.last_viewport_height, false);

    changed.push("append two".to_string());
    std::fs::write(repo.join("README.md"), format!("{}\n", changed.join("\n")))
        .expect("append at eof");
    app.last_fs_check = Instant::now() - Duration::from_secs(2);
    assert!(app.maybe_check_file_changes());
    assert!(app.maybe_watch_refresh_changed_files());

    let refreshed = app.current_view_with_frame(AnimationFrame::Idle);
    assert!(refreshed.iter().all(|line| !is_fold_line(line)));
    let appended = refreshed
        .iter()
        .find(|line| line.content == "append two")
        .expect("newest eof line");
    assert_eq!(appended.kind, LineKind::Inserted);
    assert!(appended.has_changes);
    app.clamp_scroll(refreshed.len(), app.last_viewport_height, false);
    assert!(refreshed[app.scroll_offset..]
        .iter()
        .any(|line| line.content == "append two"));

    app.add_review_comment_from_cli(
        "README.md",
        review::ReviewTargetKind::Line,
        Some(review::ReviewSide::New),
        None,
        Some(review::ReviewRange { start: 42, end: 42 }),
        "last line".to_string(),
    )
    .unwrap();
    assert_eq!(app.review_comment_overlays_for_current_file().len(), 1);

    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn fold_context_scope_hints_are_cached_and_use_innermost_definition() {
    let mut lines = vec!["fn outer() {".to_string()];
    lines.extend((0..12).map(|idx| format!("    let outer_{idx} = {idx};")));
    lines.push("    fn inner() {".to_string());
    lines.extend((0..40).map(|idx| format!("        let inner_{idx} = {idx};")));
    lines.push("    }".to_string());
    lines.extend((0..12).map(|idx| format!("    let tail_{idx} = {idx};")));
    lines.push("}".to_string());
    let old = format!("{}\n", lines.join("\n"));
    lines[33] = "        let inner_19 = 999;".to_string();
    let new = format!("{}\n", lines.join("\n"));
    let diff = MultiFileDiff::from_file_pair("scope.rs".into(), "scope.rs".into(), old, new);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.next_step();

    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    let hints = app
        .fold_context_regions
        .iter()
        .filter_map(|region| region.scope_hint.as_deref())
        .collect::<Vec<_>>();
    assert!(hints.contains(&"fn outer() {"));
    assert!(hints.contains(&"fn inner() {"));
    app.fold_scope_caches[0].as_mut().unwrap()[0].definition = "cached_scope".to_string();

    app.invalidate_fold_context_view();
    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    assert_eq!(
        app.fold_scope_caches[0].as_ref().unwrap()[0].definition,
        "cached_scope"
    );

    app.rebuild_current_syntax_cache_after_reload();
    assert!(app.fold_scope_caches[0].is_none());
}

#[test]
fn fold_context_defaults_on_and_toggles_between_two_states() {
    let mut app = TestApp::new_default(|| {
        App::new(
            MultiFileDiff::from_file_pair(
                "fold.txt".into(),
                "fold.txt".into(),
                "old\n".to_string(),
                "new\n".to_string(),
            ),
            ViewMode::UnifiedPane,
            0,
            false,
            None,
        )
    });
    assert_eq!(app.fold_context, FoldContextMode::Expandable);
    app.toggle_fold_context();
    assert_eq!(app.fold_context, FoldContextMode::Off);
    app.toggle_fold_context();
    assert_eq!(app.fold_context, FoldContextMode::Expandable);
}

#[test]
fn diff_scrollbar_thumb_tracks_scroll_offset() {
    assert_eq!(diff_scrollbar_thumb(100, 10, 20, 0), Some((0, 2)));
    assert_eq!(diff_scrollbar_thumb(100, 10, 20, 90), Some((18, 2)));
    assert_eq!(diff_scrollbar_thumb(10, 10, 20, 0), None);
}

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
    // Feature disabled: overscroll is never allowed.
    assert!(!allow_overscroll_state(false, false, false, false));
    assert!(!allow_overscroll_state(false, true, true, false));
    assert!(!allow_overscroll_state(false, false, false, true));

    // Feature enabled: preserve existing auto-center/manual-center behavior.
    assert!(!allow_overscroll_state(true, false, false, false));
    assert!(allow_overscroll_state(true, false, false, true));
    assert!(!allow_overscroll_state(true, false, true, false));
    assert!(!allow_overscroll_state(true, true, false, false));
    assert!(allow_overscroll_state(true, true, true, false));
    assert!(allow_overscroll_state(true, true, true, true));
    assert!(allow_overscroll_state(true, true, false, true));
}

#[test]
fn selection_toolbar_waits_for_mouse_selection_finish() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "old\n".to_string(),
        "new\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 100, false, None);
    app.diff_view_area = Some((0, 0, 20, 2));
    let mut row = vec![" ".to_string(); 20];
    row[8] = "a".to_string();
    row[9] = "b".to_string();
    app.set_diff_selection_cells(vec![row.clone(), row]);

    assert!(app.start_diff_selection(8, 0));
    assert!(!app.selection_toolbar_visible());
    assert!(app.drag_diff_selection(9, 0));
    assert!(!app.selection_toolbar_visible());
    assert!(app.finish_diff_selection(9, 0));
    assert!(app.selection_toolbar_visible());

    let selection = app.diff_selection;
    app.set_selection_toolbar_hits(Vec::new());
    app.set_selection_toolbar_rect(Some((5, 0, 10, 3)));
    assert!(app.handle_selection_toolbar_click(6, 1));
    assert_eq!(app.diff_selection, selection);
    assert!(app.selection_toolbar_visible());

    assert!(app.dismiss_selection_toolbar_click(0, 1));
    assert!(app.diff_selection.is_none());
    assert!(!app.selection_toolbar_visible());

    assert!(app.start_diff_selection(8, 0));
    assert!(app.drag_diff_selection(9, 0));
    assert!(app.finish_diff_selection(9, 0));
    app.set_selection_toolbar_rect(Some((5, 0, 10, 3)));
    app.set_selection_toolbar_hits(vec![SelectionToolbarHit {
        action: SelectionToolbarAction::Cancel,
        x: 6,
        y: 1,
        width: 10,
        height: 1,
    }]);
    assert!(app.handle_selection_toolbar_click(6, 1));
    assert!(app.diff_selection.is_none());
    assert!(!app.selection_toolbar_visible());

    assert!(app.start_diff_selection(8, 0));
    assert!(app.drag_diff_selection(9, 0));
    assert!(app.finish_diff_selection(9, 0));
    app.selection_actions = vec![crate::config::SelectionActionConfig::default()];
    app.set_selection_toolbar_rect(Some((5, 0, 10, 3)));
    app.set_selection_toolbar_hits(vec![SelectionToolbarHit {
        action: SelectionToolbarAction::Custom(0),
        x: 6,
        y: 1,
        width: 10,
        height: 1,
    }]);
    assert!(app.handle_selection_toolbar_click(6, 1));
    assert!(app.diff_selection.is_none());
    assert!(!app.selection_toolbar_visible());

    assert!(app.start_keyboard_selection());
    assert!(!app.selection_toolbar_visible());
    assert!(app.show_selection_toolbar());
    assert!(app.selection_toolbar_visible());
}

#[test]
fn split_selection_comment_uses_selected_side() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "one\ntwo\nthree\n".to_string(),
        "one\nTWO\nthree\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::Split, 100, false, None);
    app.review_mode = true;
    app.diff_view_area = Some((0, 0, 100, 6));
    app.set_diff_selection_cells(vec![vec!["x".to_string(); 100]; 6]);

    assert!(app.start_diff_selection(60, 1));
    assert!(app.finish_diff_selection(61, 1));
    app.set_selection_toolbar_rect(Some((0, 0, 1, 1)));
    app.set_selection_toolbar_hits(vec![SelectionToolbarHit {
        action: SelectionToolbarAction::Comment,
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }]);

    assert!(app.handle_selection_toolbar_click(0, 0));

    let editor = app.review_editor.as_ref().expect("editor");
    assert_eq!(editor.anchor.side, Some(review::ReviewSide::New));
    assert_eq!(
        editor.anchor.new_range,
        Some(review::ReviewRange { start: 2, end: 2 })
    );
}

#[test]
fn entering_image_file_keeps_current_view_mode() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("image.png"),
        std::path::PathBuf::from("image.png"),
        String::new(),
        String::new(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 100, false, None);
    app.handle_file_enter();

    assert_eq!(app.view_mode, ViewMode::UnifiedPane);
}

#[test]
fn selecting_file_discards_review_editor() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "old\n".to_string(),
            "new\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "old\n".to_string(),
            "new\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 100, false, None);
    app.review_mode = true;
    app.review_editor = Some(review::ReviewEditorState {
        anchor: review::ReviewAnchor {
            file_index: 0,
            file_path: "a.txt".to_string(),
            kind: review::ReviewTargetKind::Line,
            side: Some(review::ReviewSide::New),
            old_range: Some(review::ReviewRange { start: 1, end: 1 }),
            new_range: Some(review::ReviewRange { start: 1, end: 1 }),
            hunk_id: None,
            display_idx_hint: Some(0),
            anchor_key: "line|a.txt|new|1".to_string(),
            snapshot: None,
        },
        text: "draft".to_string(),
        cursor: 5,
        reply: None,
    });

    app.review_reply_prefix = true;
    app.review_overflow_prefix = true;
    app.select_file(1);

    assert!(!app.review_editor_active());
    assert!(!app.review_reply_prefix);
    assert!(!app.review_overflow_prefix);
}

#[test]
fn review_line_add_rows_skip_reserved_comment_notes() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "old\n".to_string(),
        "new\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 100, false, None);
    app.review_mode = true;
    app.diff_view_area = Some((0, 10, 40, 20));
    app.set_diff_selection_cells(vec![vec!["x".to_string(); 40]; 20]);
    app.add_review_preview_box(0, 12, 37, 3, "note".to_string());

    assert_eq!(app.review_display_idx_for_screen_row(11), Some(1));
    assert_eq!(app.review_display_idx_for_screen_row(12), None);
    assert_eq!(app.review_display_idx_for_screen_row(15), Some(2));
    assert_eq!(app.review_line_add_hover_at(38, 12), (None, false));
    assert_eq!(app.review_line_add_hover_at(38, 15), (Some(15), true));
}

#[test]
fn non_file_tabs_clear_review_line_add_hover_state() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "old\n".to_string(),
        "new\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 100, false, None);
    app.ensure_topbar_tabs();
    app.open_settings_tab();
    app.review_line_add_row = Some(12);
    app.review_line_add_hover = true;

    assert!(app.update_topbar_hover(0, 0));
    assert_eq!(app.review_line_add_row, None);
    assert!(!app.review_line_add_hover);
}

#[test]
fn last_diff_hover_persists_after_mouse_leaves_in_normal_mode() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "old\n".to_string(),
        "new\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 100, false, None);
    app.diff_view_area = Some((0, 10, 40, 4));
    app.set_diff_selection_cells(vec![vec!["x".to_string(); 40]; 4]);

    app.remember_diff_line_hover(10, 10);
    assert_eq!(
        app.current_file_relative_position_label().as_deref(),
        Some("a.txt:R1")
    );
    app.remember_diff_line_hover(80, 20);
    assert_eq!(
        app.current_file_relative_position_label().as_deref(),
        Some("a.txt:R1")
    );
}

#[test]
fn rendered_unified_hover_tracks_the_exact_line_past_virtual_rows() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "one\ntwo\nthree\nfour\nfive\n".to_string(),
        "one\nTWO\nthree\nfour\nFIVE\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 100, false, None);
    app.handle_file_enter();
    let backend = ratatui::backend::TestBackend::new(100, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let local_row = app
        .review_unified_line_rows
        .iter()
        .find_map(|(row, idx)| (*idx == 4).then_some(*row))
        .unwrap();
    let (x, y, _, _) = app.diff_view_area.unwrap();

    app.remember_diff_line_hover(x.saturating_add(10), y.saturating_add(local_row as u16));

    assert_eq!(
        app.current_file_relative_position_label().as_deref(),
        Some("a.txt:R5")
    );
    assert_eq!(
        app.current_file_cursor_position_label().as_deref(),
        Some("a.txt:R2")
    );
}

#[test]
fn line_comment_key_uses_hovered_diff_row_and_split_side() {
    for (mode, side, row, display_idx) in [
        (ViewMode::UnifiedPane, None, 11, 1),
        (ViewMode::Split, Some(review::ReviewSide::Old), 10, 0),
        (ViewMode::Split, Some(review::ReviewSide::New), 10, 0),
    ] {
        let diff = MultiFileDiff::from_file_pair(
            std::path::PathBuf::from("a.txt"),
            std::path::PathBuf::from("a.txt"),
            "one\ntwo\n".to_string(),
            "ONE\nTWO\n".to_string(),
        );
        let mut app = App::new(diff, mode, 100, false, None);
        app.review_mode = true;
        app.diff_view_area = Some((0, 10, 80, 4));
        app.review_line_add_row = Some(row);
        app.review_line_add_side = side;
        if mode == ViewMode::Split {
            assert_eq!(
                app.review_side_at_screen_column(20),
                Some(review::ReviewSide::Old)
            );
            assert_eq!(
                app.review_side_at_screen_column(60),
                Some(review::ReviewSide::New)
            );
        }

        app.start_line_comment();

        let anchor = &app.review_editor.as_ref().unwrap().anchor;
        assert_eq!(anchor.display_idx_hint, Some(display_idx));
        if let Some(side) = side {
            assert_eq!(anchor.side, Some(side));
        }
    }
}

#[test]
fn rendered_split_hover_targets_the_line_under_each_pane() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "one\ntwo\nthree\nfour\n".to_string(),
        "ONE\ntwo\nTHREE\nfour\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::Split, 100, false, None);
    app.review_mode = true;
    let backend = ratatui::backend::TestBackend::new(120, 28);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();

    let (x, _, width, _) = app.diff_view_area.unwrap();
    let old_row = app
        .review_split_line_rows
        .iter()
        .find_map(|(row, side, idx)| {
            (*side == review::ReviewSide::Old && *idx == 2).then_some(*row)
        })
        .unwrap();
    assert!(app.update_topbar_hover(x.saturating_add(10), old_row));
    assert_eq!(app.review_line_add_side, Some(review::ReviewSide::Old));
    app.start_line_comment();
    let anchor = &app.review_editor.as_ref().unwrap().anchor;
    assert_eq!(anchor.side, Some(review::ReviewSide::Old));
    assert_eq!(anchor.old_range.unwrap().start, 3);

    app.review_cancel_editor();
    let new_row = app
        .review_split_line_rows
        .iter()
        .find_map(|(row, side, idx)| {
            (*side == review::ReviewSide::New && *idx == 2).then_some(*row)
        })
        .unwrap();
    assert!(app.update_topbar_hover(x.saturating_add(width.saturating_mul(3) / 4), new_row,));
    assert_eq!(app.review_line_add_side, Some(review::ReviewSide::New));
    app.start_line_comment();
    let anchor = &app.review_editor.as_ref().unwrap().anchor;
    assert_eq!(anchor.side, Some(review::ReviewSide::New));
    assert_eq!(anchor.new_range.unwrap().start, 3);
}

#[test]
fn review_line_add_click_opens_line_comment() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "old\n".to_string(),
        "new\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 100, false, None);
    app.review_mode = true;
    app.diff_view_area = Some((0, 0, 40, 4));
    app.set_diff_selection_cells(vec![vec!["x".to_string(); 40]]);

    let (row, hover) = app.review_line_add_hover_at(38, 0);
    assert_eq!(row, Some(0));
    assert!(hover);

    app.review_line_add_hit = Some(ReviewLineAddHit {
        x: 37,
        y: 0,
        width: 3,
        height: 1,
        row: 0,
    });
    assert!(app.start_diff_selection(8, 0));
    assert!(app.finish_diff_selection(9, 0));
    assert!(app.selection_toolbar_visible());
    assert!(app.handle_review_line_add_click(38, 0));
    assert!(app.review_editor_active());
    assert!(app.diff_selection.is_none());
    assert!(!app.selection_toolbar_visible());

    app.review_insert_char('a');
    app.review_save_editor();
    assert_eq!(app.review_comment_count(), 1);
    app.review_preview_hover = Some(app.review_comments[0].anchor.anchor_key.clone());
    assert!(app.remove_hovered_review_comment());
    assert_eq!(app.review_comment_count(), 0);
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

#[test]
fn test_right_file_panel_resize_uses_left_edge() {
    let multi_diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "old".to_string(),
        "new".to_string(),
    );
    let mut app = App::new(multi_diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_panel_position = FilePanelPosition::Right;
    app.file_panel_rect = Some((70, 0, 30, 20));

    assert!(app.start_file_panel_resize(70, 5));
    assert!(app.drag_file_panel_resize(65, 100));
    assert_eq!(app.file_panel_width, 35);
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

#[test]
fn goto_line_keeps_cursor_on_target_when_centered() {
    let mut app = make_large_app(120, 60);
    app.last_viewport_height = 20;

    app.goto_line_number(61);

    let cursor = app.multi_diff.current_navigator().state().cursor_change;
    let view = app.current_view_with_frame(AnimationFrame::Idle);
    let cursor_line = cursor.and_then(|id| view.iter().find(|line| line.change_id == id));
    assert_eq!(cursor_line.and_then(|line| line.new_line), Some(61));
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

fn topbar_tab(id: usize, file_index: usize) -> TopbarTab {
    TopbarTab {
        id,
        content: TopbarTabContent::File(file_index),
        view_mode: ViewMode::UnifiedPane,
        step_view_mode: ViewMode::UnifiedPane,
        stepping: true,
        scroll_offset: 0,
        horizontal_scroll: 0,
        preview_rendered: true,
        navigator_state: None,
    }
}

fn topbar_files(app: &App) -> Vec<usize> {
    app.topbar_tabs
        .iter()
        .filter_map(|tab| match tab.content {
            TopbarTabContent::File(index) => Some(index),
            TopbarTabContent::Help
            | TopbarTabContent::Settings
            | TopbarTabContent::PrComments
            | TopbarTabContent::OutdatedComments => None,
        })
        .collect()
}

fn topbar_ids(app: &App) -> Vec<usize> {
    app.topbar_tabs.iter().map(|tab| tab.id).collect()
}

#[test]
fn disabled_toasts_do_not_enqueue_notifications() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.toasts_enabled = false;

    app.toggle_line_wrap();

    assert_eq!(app.toast_engine.queue_len(), 0);
}

#[test]
fn diff_view_scrolls_horizontally_with_mouse() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "short\n".to_string(),
        "very very very long line\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.diff_view_area = Some((0, 1, 10, 10));
    app.set_current_max_line_width(40);

    assert!(app.mouse_over_diff_view(5, 5));
    assert!(app.scroll_diff_horizontally(1));
    assert_eq!(app.horizontal_scroll, 4);
    assert!(app.scroll_diff_horizontally(-1));
    assert_eq!(app.horizontal_scroll, 0);

    app.diff_view_area = Some((0, 1, 200, 10));
    assert!(app.scroll_diff_horizontally(1));
    assert_eq!(app.horizontal_scroll, 4);
}

#[test]
fn topbar_tabs_scroll_horizontally_without_scrollbar_state() {
    let diff = MultiFileDiff::from_file_pairs(
        (0..4)
            .map(|idx| {
                (
                    std::path::PathBuf::from(format!("file-{idx}.txt")),
                    "old\n".to_string(),
                    "new\n".to_string(),
                )
            })
            .collect(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.new_topbar_tab();
    app.new_topbar_tab();

    assert!(app.scroll_topbar_tabs(1));
    assert_eq!(app.topbar_tab_scroll, 1);
    assert!(app.scroll_topbar_tabs(-1));
    assert_eq!(app.topbar_tab_scroll, 0);
}

#[test]
fn topbar_overflow_buttons_scroll_tabs() {
    let diff = MultiFileDiff::from_file_pairs(
        (0..4)
            .map(|idx| {
                (
                    std::path::PathBuf::from(format!("file-{idx}.txt")),
                    "old\n".to_string(),
                    "new\n".to_string(),
                )
            })
            .collect(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.new_topbar_tab();
    app.new_topbar_tab();
    app.topbar_scroll_right_hit = Some((10, 0, 1, 1));

    assert!(app.update_topbar_hover(10, 0));
    assert!(app.topbar_scroll_right_hover);
    assert!(app.handle_topbar_mouse_down(10, 0));
    assert_eq!(app.topbar_tab_scroll, 1);

    app.topbar_scroll_left_hit = Some((0, 0, 1, 1));
    assert!(app.handle_topbar_mouse_down(0, 0));
    assert_eq!(app.topbar_tab_scroll, 0);
}

#[test]
fn status_bar_mode_click_cycles_views() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.status_mode_hit = Some((0, 9, 9, 1));

    assert!(app.handle_status_bar_mouse_down(1, 9, false));
    assert_eq!(app.view_mode, ViewMode::Split);
    assert!(app.handle_status_bar_mouse_down(1, 9, true));
    assert_eq!(app.view_mode, ViewMode::UnifiedPane);
}

#[test]
fn right_click_status_mode_opens_mode_menu() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.status_mode_hit = Some((0, 9, 9, 1));

    assert!(app.open_status_mode_menu(1, 9));
    assert_eq!(
        app.status_mode_menu.map(|menu| (menu.x, menu.y)),
        Some((1, 9))
    );
}

#[test]
fn status_mode_menu_click_sets_view_mode() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.status_mode_menu = Some(StatusModeMenu { x: 1, y: 9 });
    app.status_mode_menu_hits.push(StatusModeMenuHit {
        mode: ViewMode::Preview,
        x: 2,
        y: 4,
        width: 12,
        height: 1,
    });

    assert!(app.handle_status_mode_menu_click(3, 4));
    assert_eq!(app.view_mode, ViewMode::Preview);
    assert!(app.status_mode_menu.is_none());
}

#[test]
fn topbar_preview_toggle_hover_tracks_hitbox() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("README.md"),
        std::path::PathBuf::from("README.md"),
        "old\n".to_string(),
        "new\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::Preview, 0, false, None);
    app.preview_toggle_hit = Some((10, 0, 8, 1));

    assert!(app.update_topbar_hover(12, 0));
    assert!(app.preview_toggle_hover);
    assert!(app.update_topbar_hover(1, 0));
    assert!(!app.preview_toggle_hover);
}

#[test]
fn file_filter_click_keeps_existing_query() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_filter = "abc".to_string();
    app.file_filter_area = Some((0, 0, 20, 3));

    assert!(app.handle_file_list_click(1, 1, false));
    assert!(app.file_filter_active);
    assert_eq!(app.file_filter, "abc");
}

#[test]
fn file_filter_click_out_blurs_filter() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_filter_active = true;
    app.file_list_area = Some((0, 0, 20, 5));

    assert!(app.handle_file_list_click(30, 1, false));
    assert!(!app.file_filter_active);
}

#[test]
fn file_filter_clear_button_clears_filter() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_filter = "abc".to_string();
    app.file_filter_clear_hit = Some((10, 2, 1, 1));
    app.file_panel_rect = Some((0, 0, 20, 10));

    assert!(app.update_topbar_hover(10, 2));
    assert!(app.file_filter_clear_hover);
    assert!(app.handle_file_list_click(10, 2, false));
    assert!(app.file_filter.is_empty());
}

#[test]
fn ctrl_click_file_list_opens_new_tab() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "a\n".to_string(),
            "aa\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_list_area = Some((0, 0, 20, 5));
    app.file_list_rows = vec![Some(1)];

    assert!(app.handle_file_list_click(1, 1, true));

    assert_eq!(topbar_files(&app), vec![0, 1]);
    assert_eq!(app.active_topbar_content(), Some(TopbarTabContent::File(1)));
}

#[test]
fn right_click_file_list_opens_context_menu_without_selecting() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "a\n".to_string(),
            "aa\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_list_area = Some((0, 0, 20, 5));
    app.file_list_rows = vec![Some(1)];

    assert!(app.open_file_context_menu(1, 1));
    assert_eq!(app.file_context_menu.map(|menu| menu.file_index), Some(1));
    assert_eq!(app.multi_diff.selected_index, 0);
}

#[test]
fn file_context_menu_click_opens_new_tab() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "a\n".to_string(),
            "aa\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_list_area = Some((0, 0, 20, 5));
    app.file_list_rows = vec![Some(1)];
    assert!(app.open_file_context_menu(1, 1));
    app.file_context_menu_hits.push(FileContextMenuHit {
        action: FileContextMenuAction::OpenInNewTab,
        x: 2,
        y: 2,
        width: 20,
        height: 1,
    });

    assert!(app.handle_file_context_menu_click(3, 2));

    assert_eq!(topbar_files(&app), vec![0, 1]);
    assert_eq!(app.active_topbar_content(), Some(TopbarTabContent::File(1)));
    assert!(app.file_context_menu.is_none());
}

#[test]
fn file_list_hover_tracks_row_hitbox() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "a\n".to_string(),
            "aa\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_list_area = Some((0, 1, 20, 5));
    app.file_list_rows = vec![None, Some(1)];

    app.file_panel_rect = Some((0, 0, 20, 10));

    assert!(app.update_topbar_hover(2, 3));
    assert_eq!(app.file_list_hover, Some(1));
    assert!(app.file_panel_hover);
    assert!(app.update_topbar_hover(25, 3));
    assert_eq!(app.file_list_hover, None);
    assert!(!app.file_panel_hover);
}

#[test]
fn file_panel_root_hover_and_click_open_oy_view() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "a\n".to_string(),
            "aa\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_panel_mode = FilePanelMode::Comments;
    app.file_panel_root_hit = Some((1, 0, 8, 1));
    app.file_panel_rect = Some((0, 0, 20, 10));

    assert!(app.update_topbar_hover(2, 0));
    assert!(app.file_panel_root_hover);
    assert!(app.handle_file_list_click(2, 0, false));
    assert!(app.open_dashboard);
    assert_eq!(app.file_panel_mode, FilePanelMode::Comments);
}

#[test]
fn sidebar_tab_unseen_markers_clear_on_switch() {
    let diff = MultiFileDiff::from_file_pair(
        "a.txt".into(),
        "a.txt".into(),
        "old\n".to_string(),
        "new\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.comments_tab_unseen = true;
    app.show_comments_sidebar();
    assert!(!app.comments_tab_unseen);

    app.files_tab_unseen = true;
    assert!(app.show_files_sidebar());
    assert!(!app.files_tab_unseen);

    app.comments_tab_unseen = true;
    app.file_panel_mode_toggle_hover = true;
    app.toggle_file_panel_mode();
    assert!(!app.comments_tab_unseen);
    assert!(!app.file_panel_mode_toggle_hover);
    app.files_tab_unseen = true;
    app.file_panel_mode_toggle_hover = true;
    app.toggle_file_panel_mode();
    assert!(!app.files_tab_unseen);
    assert!(!app.file_panel_mode_toggle_hover);
}

#[test]
fn selecting_visible_file_does_not_recentre_sidebar() {
    let pairs = (0..50)
        .map(|idx| {
            (
                std::path::PathBuf::from(format!("file-{idx}.txt")),
                "old\n".to_string(),
                "new\n".to_string(),
            )
        })
        .collect();
    let diff = MultiFileDiff::from_file_pairs(pairs);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_list_area = Some((0, 0, 30, 43));

    app.select_file(39);

    assert_eq!(app.file_list_scroll, 0);
}

#[test]
fn file_list_scroll_counts_group_rows() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a/one.txt"),
            "old\n".to_string(),
            "new\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b/two.txt"),
            "old\n".to_string(),
            "new\n".to_string(),
        ),
    ]);
    let app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    let indices = app.filtered_file_indices();

    assert_eq!(app.file_list_total_rows(&indices), 5);
}

#[test]
fn theme_picker_previews_and_restores_on_cancel() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.apply_ui_theme("aura");

    app.start_theme_picker();
    app.push_theme_picker_char('n');
    app.push_theme_picker_char('o');
    app.push_theme_picker_char('r');

    assert!(app.theme_picker_active());
    assert_eq!(app.ui_theme_name.as_deref(), Some("nord"));
    app.stop_theme_picker();
    assert_eq!(app.ui_theme_name.as_deref(), Some("aura"));
}

#[test]
fn theme_picker_accept_keeps_previewed_theme() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.apply_ui_theme("aura");

    app.start_theme_picker();
    app.push_theme_picker_char('n');
    app.push_theme_picker_char('o');
    app.push_theme_picker_char('r');
    app.apply_theme_picker_selection();

    assert!(!app.theme_picker_active());
    assert_eq!(app.ui_theme_name.as_deref(), Some("nord"));
}

#[test]
fn single_file_can_show_sidebar() {
    let diff = MultiFileDiff::from_file_pair(
        std::path::PathBuf::from("a.txt"),
        std::path::PathBuf::from("a.txt"),
        "a\n".to_string(),
        "aa\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.file_panel_visible = false;

    assert!(app.show_files_sidebar());
    assert!(app.file_panel_visible);
    assert_eq!(app.file_panel_mode, FilePanelMode::Files);
}

#[test]
fn topbar_sidebar_toggle_button_toggles_file_panel() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "a\n".to_string(),
            "aa\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.topbar_sidebar_toggle_hit = Some((0, 0, 3, 1));

    assert!(app.update_topbar_hover(1, 0));
    assert!(app.topbar_sidebar_toggle_hover);
    assert!(app.handle_topbar_mouse_down(1, 0));
    assert!(!app.file_panel_visible);
    assert!(app.handle_topbar_mouse_down(1, 0));
    assert!(app.file_panel_visible);
}

#[test]
fn empty_help_tab_closes_back_to_file_placeholder() {
    let diff = MultiFileDiff::from_file_pairs(Vec::new());
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.open_help_tab();
    let tab_id = app.active_topbar_tab.unwrap();
    let file_tab_id = app
        .topbar_tabs
        .iter()
        .find(|tab| tab.content == TopbarTabContent::File(0))
        .unwrap()
        .id;
    assert!(!app.topbar_close_allowed(file_tab_id));
    app.close_topbar_tab(file_tab_id);
    assert_eq!(app.topbar_tabs.len(), 2);
    app.topbar_tab_hits = vec![TopbarTabHit {
        tab_id,
        row: 0,
        start_col: 0,
        end_col: 8,
        close_col: Some(6),
    }];

    assert!(app.update_topbar_hover(6, 0));
    assert_eq!(app.topbar_hover_close, Some(tab_id));
    assert!(app.handle_topbar_mouse_down(6, 0));
    assert_eq!(app.topbar_tabs.len(), 1);
    assert_eq!(app.active_topbar_content(), Some(TopbarTabContent::File(0)));
    assert_eq!(app.view_mode, ViewMode::UnifiedPane);
}

#[test]
fn topbar_close_keeps_last_tab() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "a\n".to_string(),
            "aa\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.topbar_tabs = vec![topbar_tab(1, 0), topbar_tab(2, 1)];
    app.active_topbar_tab = Some(2);
    app.multi_diff.select_file(1);
    app.topbar_tab_hits = vec![TopbarTabHit {
        tab_id: 2,
        row: 0,
        start_col: 0,
        end_col: 7,
        close_col: Some(5),
    }];

    assert!(app.update_topbar_hover(5, 0));
    assert_eq!(app.topbar_hover_close, Some(2));
    assert!(app.update_topbar_hover(1, 0));
    assert_eq!(app.topbar_hover_close, None);

    assert!(app.handle_topbar_mouse_down(5, 0));
    assert_eq!(topbar_files(&app), vec![0]);
    assert_eq!(app.active_topbar_tab, Some(1));
    assert_eq!(app.multi_diff.selected_index, 0);

    app.topbar_tab_hits = vec![TopbarTabHit {
        tab_id: 1,
        row: 0,
        start_col: 0,
        end_col: 7,
        close_col: Some(5),
    }];
    assert!(app.handle_topbar_mouse_down(5, 0));
    assert_eq!(topbar_files(&app), vec![0]);
}

#[test]
fn topbar_select_replaces_active_tab_file() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "a\n".to_string(),
            "aa\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);

    app.select_file(1);
    assert_eq!(topbar_files(&app), vec![1]);

    app.new_topbar_tab();
    assert_eq!(topbar_files(&app), vec![1, 1]);
    app.select_file(0);
    assert_eq!(topbar_files(&app), vec![1, 0]);
}

#[test]
fn help_opens_as_preview_tab_for_empty_diff() {
    let diff = MultiFileDiff::from_file_pairs(Vec::new());
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);

    app.open_help_tab();

    assert!(!app.show_help);
    assert_eq!(app.view_mode, ViewMode::Preview);
    assert_eq!(app.active_topbar_content(), Some(TopbarTabContent::Help));
    assert_eq!(app.topbar_tabs.len(), 2);
    assert!(app
        .topbar_tabs
        .iter()
        .any(|tab| tab.content == TopbarTabContent::File(0)));

    app.ensure_topbar_tabs();
    assert_eq!(app.active_topbar_content(), Some(TopbarTabContent::Help));
}

#[test]
fn help_opens_as_preview_tab() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.md"),
            "# old\n".to_string(),
            "# new\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);

    app.open_help_tab();
    assert_eq!(app.view_mode, ViewMode::Preview);
    assert_eq!(app.active_topbar_content(), Some(TopbarTabContent::Help));
    assert_eq!(app.topbar_tabs.len(), 2);

    app.open_help_tab();
    assert_eq!(app.topbar_tabs.len(), 2);

    app.select_file(1);
    assert_eq!(app.active_topbar_content(), Some(TopbarTabContent::File(1)));
}

#[test]
fn preview_render_toggle_is_per_tab() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.md"),
            "# old\n".to_string(),
            "# new\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.md"),
            "# b\n".to_string(),
            "# bb\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::Preview, 0, false, None);
    app.toggle_preview_rendered();
    assert!(!app.active_preview_rendered());

    app.new_topbar_tab();
    assert!(!app.active_preview_rendered());
    app.select_file(1);
    assert!(app.active_preview_rendered());
}

#[test]
fn topbar_tabs_keep_view_and_step_state() {
    let diff = MultiFileDiff::from_file_pairs(vec![(
        std::path::PathBuf::from("a.txt"),
        "a\nb\n".to_string(),
        "aa\nbb\n".to_string(),
    )]);
    let mut app = App::new(diff, ViewMode::Split, 0, false, None);
    app.stepping = false;
    app.multi_diff.current_navigator().goto(1);
    app.new_topbar_tab();
    let first = app.topbar_tabs[0].id;
    let second = app.topbar_tabs[1].id;

    app.view_mode = ViewMode::UnifiedPane;
    app.stepping = true;
    app.multi_diff.current_navigator().goto(0);

    app.select_topbar_tab(first);
    assert_eq!(app.view_mode, ViewMode::Split);
    assert!(!app.stepping);
    assert_eq!(app.multi_diff.current_navigator().state().current_step, 1);

    app.select_topbar_tab(second);
    assert_eq!(app.view_mode, ViewMode::UnifiedPane);
    assert!(app.stepping);
    assert_eq!(app.multi_diff.current_navigator().state().current_step, 0);
}

#[test]
fn topbar_drag_reorders_tabs() {
    let diff = MultiFileDiff::from_file_pairs(vec![
        (
            std::path::PathBuf::from("a.txt"),
            "a\n".to_string(),
            "aa\n".to_string(),
        ),
        (
            std::path::PathBuf::from("b.txt"),
            "b\n".to_string(),
            "bb\n".to_string(),
        ),
        (
            std::path::PathBuf::from("c.txt"),
            "c\n".to_string(),
            "cc\n".to_string(),
        ),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.topbar_tabs = vec![topbar_tab(1, 0), topbar_tab(2, 1), topbar_tab(3, 2)];
    app.active_topbar_tab = Some(1);
    app.topbar_tab_hits = vec![
        TopbarTabHit {
            tab_id: 1,
            row: 0,
            start_col: 0,
            end_col: 6,
            close_col: Some(4),
        },
        TopbarTabHit {
            tab_id: 2,
            row: 0,
            start_col: 7,
            end_col: 13,
            close_col: Some(11),
        },
        TopbarTabHit {
            tab_id: 3,
            row: 0,
            start_col: 14,
            end_col: 20,
            close_col: Some(18),
        },
    ];

    assert!(app.handle_topbar_mouse_down(1, 0));
    assert_eq!(app.topbar_drag_target, None);
    assert!(app.drag_topbar_tab(1, 0));
    assert_eq!(app.topbar_drag_target, None);
    assert!(app.drag_topbar_tab(15, 0));
    assert_eq!(app.topbar_drag_target, Some(2));
    app.topbar_tab_hits[2].start_col = 15;
    app.topbar_tab_hits[2].end_col = 21;
    assert!(app.drag_topbar_tab(14, 0));
    assert_eq!(app.topbar_drag_target, Some(2));
    assert!(app.finish_topbar_drag());
    assert_eq!(topbar_ids(&app), vec![2, 1, 3]);
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
    app.set_fold_context_mode(FoldContextMode::Off);
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
    app.set_fold_context_mode(FoldContextMode::Off);
    app.view_mode = ViewMode::Split;
    app.split_align_lines = true;
    app.last_viewport_height = 25;

    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    app.defer_view_build_for_jump();
    app.goto_last_step();
    assert!(app.needs_scroll_to_active);

    app.ensure_active_visible_if_needed(app.last_viewport_height, 80);
    assert!(
        app.needs_scroll_to_active,
        "deferred view should keep active scroll pending"
    );

    app.ensure_active_visible_if_needed(app.last_viewport_height, 80);
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
    app.set_fold_context_mode(FoldContextMode::Off);
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
    app.set_fold_context_mode(FoldContextMode::Off);
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
    app.set_fold_context_mode(FoldContextMode::Off);
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
    app.set_fold_context_mode(FoldContextMode::Off);
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
    app.set_fold_context_mode(FoldContextMode::Off);
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
fn test_refresh_current_file_keeps_no_step_state() {
    let _guard = DiffSettingsGuard::default();
    let old = "line1\nline2\nline3\n".to_string();
    let new = "line1\nLINE2\nline3\n".to_string();

    let path = std::env::temp_dir().join(format!(
        "oyo_refresh_state_test_{}_{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(&path, &new).expect("write test file");

    let diff = MultiFileDiff::from_file_pair(path.clone(), path.clone(), old, new);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.stepping = false;
    app.no_step_auto_jump_on_enter = false;
    app.enter_no_step_mode();
    app.scroll_offset = 1;
    app.horizontal_scroll = 4;

    app.refresh_current_file();

    let state = app.multi_diff.current_navigator().state().clone();
    assert_eq!(state.step_direction, StepDirection::None);
    assert!(state.active_change.is_none());
    assert_eq!(app.scroll_offset, 1);
    assert_eq!(app.horizontal_scroll, 4);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_refresh_current_file_preserves_no_step_hunk_scope() {
    let _guard = DiffSettingsGuard::default();
    let old_lines: Vec<String> = (1..=25).map(|i| format!("line{}", i)).collect();
    let mut new_lines = old_lines.clone();
    new_lines[1] = "line2-new".to_string();
    new_lines[19] = "line20-new".to_string();
    let old = old_lines.join("\n");
    let new = new_lines.join("\n");

    let path = std::env::temp_dir().join(format!(
        "oyo_refresh_hunk_test_{}_{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(&path, &new).expect("write test file");

    let diff = MultiFileDiff::from_file_pair(path.clone(), path.clone(), old, new);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.stepping = false;
    app.no_step_auto_jump_on_enter = false;
    app.enter_no_step_mode();
    app.goto_last_hunk_scroll();

    let before = app.multi_diff.current_navigator().state().clone();
    assert!(before.last_nav_was_hunk);
    assert!(before.cursor_change.is_some());

    app.refresh_current_file();

    let (after, cursor_in_hunk) = {
        let nav = app.multi_diff.current_navigator();
        let state = nav.state().clone();
        let in_hunk = state
            .cursor_change
            .map(|id| {
                nav.diff()
                    .hunks
                    .get(state.current_hunk)
                    .map(|hunk| hunk.change_ids.contains(&id))
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        (state, in_hunk)
    };

    assert!(after.last_nav_was_hunk);
    assert_eq!(after.current_hunk, before.current_hunk);
    assert!(after.cursor_change.is_some());
    assert!(cursor_in_hunk, "cursor should remain within selected hunk");

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_files_changed_indicator_detects_disk_modification_and_clears_on_refresh() {
    let _guard = DiffSettingsGuard::default();
    let initial = "line1\n";
    let path = std::env::temp_dir().join(format!(
        "oyo_changed_indicator_test_{}_{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(&path, initial).expect("write test file");

    let diff = MultiFileDiff::from_file_pair(
        path.clone(),
        path.clone(),
        "old\n".to_string(),
        initial.to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.watch = false;

    app.last_fs_check = Instant::now() - Duration::from_secs(2);
    app.maybe_check_file_changes();
    assert!(!app.files_changed_on_disk);
    assert!(!app.file_changed_on_disk(0));

    std::fs::write(&path, "line1 changed on disk\n").expect("update test file");
    app.last_fs_check = Instant::now() - Duration::from_secs(2);
    app.maybe_check_file_changes();
    assert!(app.files_changed_on_disk);
    assert!(app.file_changed_on_disk(0));

    app.refresh_current_file();
    assert!(!app.files_changed_on_disk);
    assert!(!app.file_changed_on_disk(0));

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_empty_git_diff_no_step_does_not_panic() {
    let _guard = DiffSettingsGuard::default();
    let diff = MultiFileDiff::from_git_changes(std::env::temp_dir(), Vec::new()).expect("diff");
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);

    app.stepping = false;
    app.enter_no_step_mode();
    app.toggle_stepping();

    assert_eq!(app.multi_diff.file_count(), 0);
    assert!(!app.syntax_enabled());
    assert!(!app.tick());
    assert_eq!(app.stats(), (0, 0));
}

#[test]
fn test_tick_watch_refreshes_changed_files_on_disk() {
    let _guard = DiffSettingsGuard::default();
    let path = std::env::temp_dir().join(format!(
        "oyo_watch_test_{}_{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(&path, "new\n").expect("write test file");

    let diff = MultiFileDiff::from_file_pair_with_sources(
        path.clone(),
        b"old\n".to_vec(),
        b"new\n".to_vec(),
        None,
        Some(path.clone()),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    assert!(app.watch);
    let diff_bg = ratatui::style::Color::Rgb(1, 2, 3);
    app.theme.diff_added_bg = Some(diff_bg);
    app.theme.diff_removed_bg = Some(diff_bg);
    app.theme.diff_modified_bg = Some(diff_bg);
    app.diff_bg = true;
    app.stepping = false;
    app.no_step_auto_jump_on_enter = false;
    app.enter_no_step_mode();
    assert!(app.ensure_syntax_cache().is_some());
    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    assert!(app.view_cache.is_some());

    std::fs::write(&path, "new changed on disk\n").expect("update test file");
    app.last_fs_check = Instant::now() - Duration::from_secs(2);

    assert!(app.tick());
    assert!(!app.files_changed_on_disk);
    assert!(app.file_changed_on_disk(0));
    assert!(app.files_changed_indicator_active());
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some("new changed on disk\n")
    );
    assert!(!app.multi_diff.current_navigator().diff().hunks.is_empty());
    assert!(app.syntax_caches[0].is_some());
    assert!(app.view_cache.is_none());
    assert!(app
        .syntax_spans_for_line(crate::syntax::SyntaxSide::New, Some(1))
        .is_some());

    let backend = ratatui::backend::TestBackend::new(80, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| crate::views::render_unified_pane(frame, &mut app, frame.area()))
        .expect("render refreshed diff");
    let buffer = terminal.backend().buffer();
    let mut has_diff_bg = false;
    let mut has_gutter_decoration = false;
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            has_diff_bg |= cell.bg == diff_bg;
            has_gutter_decoration |= matches!(cell.symbol(), "+" | "~" | "┃");
        }
    }
    assert!(has_diff_bg);
    assert!(has_gutter_decoration);

    app.file_recently_changed_until[0] = Some(Instant::now() - Duration::from_millis(1));
    assert!(app.expire_recent_file_changes(Instant::now()));
    assert!(!app.file_changed_on_disk(0));
    assert!(!app.files_changed_indicator_active());

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_tick_watch_adds_new_untracked_file() {
    let _guard = DiffSettingsGuard::default();
    let repo = std::env::temp_dir().join(format!(
        "oyo_watch_repo_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&repo).expect("create repo");
    std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(&repo)
        .status()
        .expect("git init");

    let changes = oyo_core::git::get_uncommitted_changes(&repo).expect("changes");
    let diff = MultiFileDiff::from_git_changes(repo.clone(), changes).expect("diff");
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    assert_eq!(app.multi_diff.file_count(), 0);

    std::fs::write(repo.join("new.txt"), "new\n").expect("write new file");
    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);

    assert!(app.maybe_watch_refresh_git_files());
    assert_eq!(app.multi_diff.file_count(), 1);
    assert_eq!(app.current_file_path(), "new.txt");
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some("new\n")
    );
    assert!(app.syntax_caches[0].is_some());
    assert!(app.view_cache.is_none());

    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn live_refresh_moves_deleted_comment_to_outdated_and_restores_it() {
    let _guard = DiffSettingsGuard::default();
    let path = std::env::temp_dir().join(format!(
        "oyo_live_review_refresh_{}_{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let other_path = path.with_extension("other.txt");
    std::fs::write(&path, "base\ntarget\n").expect("write target");
    std::fs::write(&other_path, "base\nother target\n").expect("write other target");
    let diff = MultiFileDiff::from_raw_files(
        None,
        vec![
            oyo_core::multi::RawFileDiff {
                path: "live.txt".into(),
                old_path: None,
                old_source_path: None,
                new_source_path: Some(path.clone()),
                status: oyo_core::git::FileStatus::Modified,
                old_content: "base\n".to_string(),
                new_content: "base\ntarget\n".to_string(),
                binary: false,
            },
            oyo_core::multi::RawFileDiff {
                path: "other.txt".into(),
                old_path: None,
                old_source_path: None,
                new_source_path: Some(other_path.clone()),
                status: oyo_core::git::FileStatus::Modified,
                old_content: "base\n".to_string(),
                new_content: "base\nother target\n".to_string(),
                binary: false,
            },
        ],
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    app.add_review_comment_from_cli(
        "live.txt",
        review::ReviewTargetKind::Line,
        Some(review::ReviewSide::New),
        None,
        Some(review::ReviewRange { start: 2, end: 2 }),
        "keep this".to_string(),
    )
    .unwrap();
    app.add_review_comment_from_cli(
        "other.txt",
        review::ReviewTargetKind::Line,
        Some(review::ReviewSide::New),
        None,
        Some(review::ReviewRange { start: 2, end: 2 }),
        "keep this too".to_string(),
    )
    .unwrap();

    std::fs::write(&path, "base\n").expect("delete target");
    std::fs::write(&other_path, "base\n").expect("delete other target");
    app.refresh_current_file();

    assert!(app.review_comments[0].outdated);
    assert!(!app.review_comments[1].outdated);
    assert!(app.maybe_watch_refresh_changed_files());
    assert!(app.review_comments[1].outdated);
    assert!(app.open_review_comment(0));
    assert_eq!(
        app.outdated_diff_title().as_deref(),
        Some("Outdated: live.txt")
    );
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some("base\ntarget\n")
    );
    app.select_file(0);

    std::fs::write(&path, "base\ntarget\n").expect("restore target");
    app.refresh_current_file();
    assert!(!app.review_comments[0].outdated);
    assert!(!app.review_comments[0].reanchored);

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(other_path);
}

#[test]
fn outdated_comment_reconstruction_swaps_and_restores_live_git_diff() {
    let _guard = DiffSettingsGuard::default();
    let repo = std::env::temp_dir().join(format!(
        "oyo_outdated_reconstruction_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["config", "user.name", "Test"]);
    run_git(&repo, &["config", "user.email", "test@example.com"]);
    run_git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("a.txt"), "one\nold\n").unwrap();
    std::fs::write(repo.join("b.txt"), "one\nold\n").unwrap();
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-qm", "base"]);
    let base = git_rev(&repo);
    std::fs::write(repo.join("a.txt"), "one\nhistorical\n").unwrap();
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-qm", "historical"]);
    let historical = git_rev(&repo);
    std::fs::write(repo.join("a.txt"), "one\nlive\n").unwrap();
    std::fs::write(repo.join("b.txt"), "one\nlive\n").unwrap();
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-qm", "live"]);
    let live = git_rev(&repo);
    let changes = oyo_core::git::get_changes_between(&repo, &historical, &live).unwrap();
    let diff =
        MultiFileDiff::from_git_range(repo.clone(), changes, historical.clone(), live).unwrap();
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    app.add_review_comment_from_cli(
        "a.txt",
        review::ReviewTargetKind::Line,
        Some(review::ReviewSide::New),
        None,
        Some(review::ReviewRange { start: 2, end: 2 }),
        "historical note".to_string(),
    )
    .unwrap();
    let target = review::ReviewAnchorSnapshotTarget {
        vcs: "git".to_string(),
        jj_change_id: None,
        jj_commit_id: None,
        git_base_commit: Some(base),
        git_head_commit: Some(historical),
    };
    app.review_comments[0].outdated = true;
    let snapshot = app.review_comments[0].anchor.snapshot.as_mut().unwrap();
    snapshot.line_text = "historical".to_string();
    snapshot.target = Some(target.clone());
    let mut second = app.review_comments[0].clone();
    second.id = second.id.saturating_add(1);
    second.body = "second historical note".to_string();
    app.review_comments.push(second);

    app.file_panel_mode = FilePanelMode::Comments;
    app.file_list_focused = true;
    app.file_list_area = Some((0, 0, 30, 10));
    app.file_list_rows = vec![Some(0)];
    assert!(app.handle_file_list_click(1, 1, false));
    assert!(app.outdated_reconstruction_pending());
    let spinner_backend = ratatui::backend::TestBackend::new(120, 28);
    let mut spinner_terminal = ratatui::Terminal::new(spinner_backend).unwrap();
    spinner_terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let spinner_buffer = spinner_terminal.backend().buffer();
    let spinner_text = (0..spinner_buffer.area.height)
        .flat_map(|y| (0..spinner_buffer.area.width).map(move |x| spinner_buffer[(x, y)].symbol()))
        .collect::<String>();
    assert!(spinner_text.contains("Reconstructing..."), "{spinner_text}");
    finish_outdated_reconstruction(&mut app);
    assert_eq!(
        app.outdated_diff_title().as_deref(),
        Some("Outdated: a.txt")
    );
    assert_eq!(app.multi_diff.file_count(), 1);
    assert!(!app.file_list_focused);
    assert_eq!(app.file_panel_mode, FilePanelMode::Comments);
    assert!(!app.needs_scroll_to_active);
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some("one\nhistorical\n")
    );
    let backend = ratatui::backend::TestBackend::new(120, 28);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rendered = (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| buffer[(x, y)].symbol()))
        .collect::<String>();
    assert!(rendered.contains("historical"), "{rendered}");
    assert!(rendered.contains("historical note"), "{rendered}");
    assert!(app
        .review_comment_overlays_for_current_file()
        .iter()
        .any(|overlay| overlay.id == app.review_comments[0].id));

    assert!(app.open_review_comment(1));
    assert!(app.outdated_reconstruction_pending());
    app.select_file(1);
    assert!(!app.outdated_reconstruction_pending());
    for _ in 0..500 {
        app.poll_outdated_reconstruction_responses();
        if matches!(
            app.outdated_reconstruction_cache
                .get(&app.review_comments[1].id),
            Some(review::OutdatedReconstructionState::Ready(_))
        ) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.current_file_path(), "b.txt");
    assert!(app.outdated_diff_title().is_none());
    assert!(app.open_review_comment(1));
    assert!(!app.outdated_reconstruction_pending());
    assert_eq!(
        app.outdated_diff_title().as_deref(),
        Some("Outdated: a.txt")
    );
    let refreshed_live = MultiFileDiff::from_raw_files(
        Some(repo.clone()),
        vec![
            oyo_core::multi::RawFileDiff {
                path: "a.txt".into(),
                old_path: None,
                old_source_path: None,
                new_source_path: None,
                status: oyo_core::git::FileStatus::Modified,
                old_content: "one\nhistorical\n".to_string(),
                new_content: "one\nrefreshed a\n".to_string(),
                binary: false,
            },
            oyo_core::multi::RawFileDiff {
                path: "b.txt".into(),
                old_path: None,
                old_source_path: None,
                new_source_path: None,
                status: oyo_core::git::FileStatus::Modified,
                old_content: "one\nold\n".to_string(),
                new_content: "one\nrefreshed b\n".to_string(),
                binary: false,
            },
        ],
    );
    app.replace_multi_diff(refreshed_live);
    assert_eq!(
        app.outdated_diff_title().as_deref(),
        Some("Outdated: a.txt")
    );
    app.select_file(1);
    assert!(app.outdated_diff_title().is_none());
    assert_eq!(app.multi_diff.file_count(), 2);
    assert_eq!(app.current_file_path(), "b.txt");
    assert_eq!(
        app.multi_diff.file_contents(1).map(|(_, new)| new),
        Some("one\nrefreshed b\n")
    );

    let mut fallback = app.review_comments[0].clone();
    fallback.id = fallback.id.saturating_add(100);
    let stale_base = target.git_base_commit.clone();
    fallback.anchor.snapshot.as_mut().unwrap().target = Some(review::ReviewAnchorSnapshotTarget {
        vcs: "git".to_string(),
        jj_change_id: None,
        jj_commit_id: None,
        git_base_commit: stale_base.clone(),
        git_head_commit: stale_base,
    });
    app.review_comments.push(fallback);
    assert!(app.open_review_comment(2));
    finish_outdated_reconstruction(&mut app);
    assert!(app.active_outdated_comments_view());
    assert!(app.outdated_diff_title().is_none());

    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn outdated_comment_reconstruction_repairs_jj_evolog_and_falls_back() {
    let _guard = DiffSettingsGuard::default();
    if std::process::Command::new("jj")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let repo = std::env::temp_dir().join(format!(
        "oyo_outdated_jj_reconstruction_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&repo).unwrap();
    let init = std::process::Command::new("jj")
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .args(["git", "init", "--no-colocate"])
        .arg(&repo)
        .output()
        .unwrap();
    assert!(init.status.success());
    run_jj(&repo, &["config", "set", "--repo", "user.name", "Test"]);
    run_jj(
        &repo,
        &["config", "set", "--repo", "user.email", "test@example.com"],
    );
    std::fs::write(repo.join("a.txt"), "one\nbase\n").unwrap();
    run_jj(&repo, &["commit", "-m", "base"]);
    let base_commit = jj_output(&repo, &["log", "--no-graph", "-r", "@-", "-T", "commit_id"])
        .trim()
        .to_string();
    std::fs::write(repo.join("a.txt"), "one\nhistorical\n").unwrap();
    let historical_commit = jj_output(&repo, &["log", "--no-graph", "-r", "@", "-T", "commit_id"])
        .trim()
        .to_string();
    std::fs::write(repo.join("a.txt"), "one\nlive\n").unwrap();
    let current_change = jj_output(
        &repo,
        &[
            "log",
            "--no-graph",
            "-r",
            "@",
            "-T",
            "change_id.shortest(8)",
        ],
    )
    .trim()
    .to_string();
    let diff = crate::build_jj_diff(&repo, "@", None).unwrap();
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    app.add_review_comment_from_cli(
        "a.txt",
        review::ReviewTargetKind::Line,
        Some(review::ReviewSide::New),
        None,
        Some(review::ReviewRange { start: 2, end: 2 }),
        "historical jj note".to_string(),
    )
    .unwrap();
    app.review_comments[0].outdated = true;
    let snapshot = app.review_comments[0].anchor.snapshot.as_mut().unwrap();
    snapshot.line_text = "historical".to_string();
    snapshot.target = Some(review::ReviewAnchorSnapshotTarget {
        vcs: "jj".to_string(),
        jj_change_id: Some(current_change.clone()),
        jj_commit_id: Some(historical_commit),
        git_base_commit: None,
        git_head_commit: None,
    });

    assert!(app.open_review_comment(0));
    finish_outdated_reconstruction(&mut app);
    assert_eq!(
        app.outdated_diff_title().as_deref(),
        Some("Outdated: a.txt")
    );
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some("one\nhistorical\n")
    );
    app.select_file(0);
    assert!(app.outdated_diff_title().is_none());
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some("one\nlive\n")
    );

    let mut stale = app.review_comments[0].clone();
    stale.id = stale.id.saturating_add(100);
    let stale_snapshot = stale.anchor.snapshot.as_mut().unwrap();
    stale_snapshot.target = Some(review::ReviewAnchorSnapshotTarget {
        vcs: "jj".to_string(),
        jj_change_id: Some(current_change),
        jj_commit_id: Some(base_commit.clone()),
        git_base_commit: None,
        git_head_commit: None,
    });
    app.review_comments.push(stale);
    assert!(app.open_review_comment(1));
    finish_outdated_reconstruction(&mut app);
    assert_eq!(
        app.outdated_diff_title().as_deref(),
        Some("Outdated: a.txt")
    );
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some("one\nhistorical\n")
    );
    app.select_file(0);

    let mut missing = app.review_comments[0].clone();
    missing.id = missing.id.saturating_add(200);
    missing.anchor.snapshot.as_mut().unwrap().target = Some(review::ReviewAnchorSnapshotTarget {
        vcs: "jj".to_string(),
        jj_change_id: Some("missing-change".to_string()),
        jj_commit_id: Some(base_commit),
        git_base_commit: None,
        git_head_commit: None,
    });
    app.review_comments.push(missing);
    assert!(app.open_review_comment(2));
    finish_outdated_reconstruction(&mut app);
    assert!(app.active_outdated_comments_view());
    assert!(app.outdated_diff_title().is_none());

    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn outdated_reconstruction_preloads_from_hover_comments_and_idle() {
    let diff = MultiFileDiff::from_file_pair(
        "a.txt".into(),
        "a.txt".into(),
        "old\n".to_string(),
        "new\n".to_string(),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    app.add_review_comment_from_cli(
        "a.txt",
        review::ReviewTargetKind::Line,
        Some(review::ReviewSide::New),
        None,
        Some(review::ReviewRange { start: 1, end: 1 }),
        "note".to_string(),
    )
    .unwrap();
    app.review_comments[0].outdated = true;
    let id = app.review_comments[0].id;

    app.review_preview_hover_id = Some(id);
    app.maybe_preload_hovered_outdated_reconstruction();
    assert!(app.outdated_reconstruction_cache.contains_key(&id));

    app.clear_outdated_reconstruction_cache();
    app.show_comments_sidebar();
    assert!(app.outdated_reconstruction_cache.contains_key(&id));

    app.clear_outdated_reconstruction_cache();
    app.last_outdated_reconstruction_idle_enqueue = Instant::now() - Duration::from_secs(1);
    app.maybe_preload_idle_outdated_reconstruction();
    assert!(app.outdated_reconstruction_cache.contains_key(&id));
}

#[test]
fn test_jj_watch_tracks_empty_start_additions_and_removals() {
    let _guard = DiffSettingsGuard::default();
    if std::process::Command::new("jj")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let repo = std::env::temp_dir().join(format!(
        "oyo_jj_watch_repo_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let init = std::process::Command::new("jj")
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .args(["git", "init"])
        .arg(&repo)
        .output()
        .expect("jj init");
    assert!(init.status.success());
    run_jj(&repo, &["config", "set", "--repo", "user.name", "Test"]);
    run_jj(
        &repo,
        &["config", "set", "--repo", "user.email", "test@example.com"],
    );
    std::fs::write(repo.join("README.md"), "base\n").expect("write base");
    run_jj(&repo, &["commit", "-m", "base"]);

    let diff = crate::build_jj_diff(&repo, "@", None).expect("initial jj diff");
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, Some("@".into()));
    app.set_review_persist_enabled(false);
    let (initial_change_id, initial_commit_id) = crate::jj_revision_ids(&repo, "@").unwrap();
    let mut metadata = crate::basic_review_target_metadata("@", "jj");
    metadata.jj_change_id = Some(initial_change_id);
    metadata.jj_commit_id = Some(initial_commit_id.clone());
    app.set_review_target_metadata(Some(metadata));
    app.set_jj_watch_target(repo.clone(), "@".into());
    assert_eq!(app.multi_diff.file_count(), 0);
    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);
    assert!(!app.maybe_watch_refresh_jj_files());

    std::fs::write(repo.join("README.md"), "base\nbefore\n").expect("change tracked file");
    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);
    assert!(app.tick());
    let (_, amended_commit_id) = crate::jj_revision_ids(&repo, "@").unwrap();
    assert_ne!(amended_commit_id, initial_commit_id);
    assert_eq!(
        app.review_target_metadata()
            .and_then(|metadata| metadata.jj_commit_id.as_deref()),
        Some(amended_commit_id.as_str())
    );
    assert_eq!(app.current_file_path(), "README.md");
    assert_eq!(app.stats(), (1, 0));
    assert!(!app.multi_diff.current_navigator().diff().hunks.is_empty());
    assert!(app.file_recently_changed_until[0].is_some());
    let existing_flash = Instant::now() + Duration::from_secs(1);
    app.file_recently_changed_until[0] = Some(existing_flash);
    app.scroll_offset = 3;
    app.horizontal_scroll = 2;

    std::fs::write(repo.join("new.txt"), "new\n").expect("add new file");
    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);
    assert!(app.maybe_watch_refresh_jj_files());
    assert_eq!(app.multi_diff.file_count(), 2);
    assert_eq!(app.current_file_path(), "README.md");
    assert_eq!(app.scroll_offset, 3);
    assert_eq!(app.horizontal_scroll, 2);
    let readme_index = app
        .multi_diff
        .files
        .iter()
        .position(|file| file.path == std::path::Path::new("README.md"))
        .unwrap();
    let new_index = app
        .multi_diff
        .files
        .iter()
        .position(|file| file.path == std::path::Path::new("new.txt"))
        .unwrap();
    assert_eq!(
        app.file_recently_changed_until[readme_index],
        Some(existing_flash)
    );
    assert!(app.file_recently_changed_until[new_index].is_some());

    std::fs::write(repo.join("new.txt"), "new\nmore\n").expect("re-edit non-current file");
    app.last_fs_check = Instant::now() - Duration::from_secs(2);
    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);
    assert!(app.tick());
    assert_eq!(app.multi_diff.files[new_index].insertions, 2);
    assert_eq!(app.multi_diff.files[new_index].deletions, 0);

    app.file_recently_changed_until[readme_index] = None;
    std::fs::write(repo.join("README.md"), "base\nbefore\nduring\n").expect("re-edit tracked file");
    app.last_fs_check = Instant::now() - Duration::from_secs(2);
    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);
    assert!(app.tick());
    assert_eq!(app.stats(), (2, 0));
    assert!(!app.multi_diff.current_navigator().diff().hunks.is_empty());
    assert!(app.file_recently_changed_until[readme_index].is_some());

    std::fs::remove_file(repo.join("new.txt")).expect("remove new file");
    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);
    assert!(app.maybe_watch_refresh_jj_files());
    assert_eq!(app.multi_diff.file_count(), 1);
    assert_eq!(app.current_file_path(), "README.md");

    std::fs::write(repo.join("manual.txt"), "manual\n").expect("add manual file");
    app.refresh_all_files();
    assert!(app
        .multi_diff
        .files
        .iter()
        .any(|file| file.path == std::path::Path::new("manual.txt")));

    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn test_watch_adds_newly_modified_tracked_file() {
    let _guard = DiffSettingsGuard::default();
    let repo = tracked_watch_repo("watch_tracked");
    std::fs::write(repo.join("other.txt"), "other changed\n").expect("modify other");
    let changes = oyo_core::git::get_uncommitted_changes(&repo).expect("changes");
    let diff = MultiFileDiff::from_git_changes(repo.clone(), changes).expect("diff");
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    assert_eq!(app.multi_diff.file_count(), 1);
    app.show_comments_sidebar();

    std::fs::write(repo.join("README.md"), "readme changed\n").expect("modify readme");
    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);

    assert!(app.maybe_watch_refresh_git_files());
    assert_eq!(app.multi_diff.file_count(), 2);
    assert!(app
        .multi_diff
        .files
        .iter()
        .any(|file| file.path == std::path::Path::new("README.md")));
    assert!(app.files_tab_unseen);
    assert!(app.show_files_sidebar());
    assert!(!app.files_tab_unseen);

    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);
    assert!(!app.maybe_watch_refresh_git_files());
    assert!(!app.files_tab_unseen);

    std::fs::write(repo.join("third.txt"), "third\n").expect("modify third");
    app.last_change_list_watch_check = Instant::now() - Duration::from_secs(2);
    assert!(app.maybe_watch_refresh_git_files());
    assert!(!app.files_tab_unseen);

    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn test_palette_refresh_all_adds_newly_modified_tracked_file() {
    let _guard = DiffSettingsGuard::default();
    let repo = tracked_watch_repo("palette_refresh_tracked");
    std::fs::write(repo.join("other.txt"), "other changed\n").expect("modify other");
    let changes = oyo_core::git::get_uncommitted_changes(&repo).expect("changes");
    let diff = MultiFileDiff::from_git_changes(repo.clone(), changes).expect("diff");
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    assert_eq!(app.multi_diff.file_count(), 1);

    std::fs::write(repo.join("README.md"), "readme changed\n").expect("modify readme");
    app.start_command_palette();
    for ch in "refresh all files".chars() {
        app.push_command_palette_char(ch);
    }
    app.apply_command_palette_selection();

    assert_eq!(app.multi_diff.file_count(), 2);
    assert!(app
        .multi_diff
        .files
        .iter()
        .any(|file| file.path == std::path::Path::new("README.md")));

    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn test_tick_watch_refreshes_non_current_changed_file() {
    let _guard = DiffSettingsGuard::default();
    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path_a = std::env::temp_dir().join(format!(
        "oyo_watch_a_{}_{}.txt",
        std::process::id(),
        now_nanos
    ));
    let path_b = std::env::temp_dir().join(format!(
        "oyo_watch_b_{}_{}.txt",
        std::process::id(),
        now_nanos
    ));
    std::fs::write(&path_a, "A\n").expect("write file a");
    std::fs::write(&path_b, "B\n").expect("write file b");

    let diff = MultiFileDiff::from_file_pairs(vec![
        (path_a.clone(), "old A\n".to_string(), "A\n".to_string()),
        (path_b.clone(), "old B\n".to_string(), "B\n".to_string()),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);

    std::fs::write(&path_a, "A changed\n").expect("update file a");
    std::fs::write(&path_b, "B changed\n").expect("update file b");
    app.last_fs_check = Instant::now() - Duration::from_secs(2);

    assert!(app.tick());
    assert!(!app.files_changed_on_disk);
    assert!(app.file_changed_on_disk(0));
    assert!(app.file_changed_on_disk(1));
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some("A changed\n")
    );
    assert_eq!(
        app.multi_diff.file_contents(1).map(|(_, new)| new),
        Some("B changed\n")
    );

    let _ = std::fs::remove_file(path_a);
    let _ = std::fs::remove_file(path_b);
}

#[test]
fn test_refresh_current_file_keeps_changed_indicator_if_other_file_is_modified() {
    let _guard = DiffSettingsGuard::default();
    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path_a = std::env::temp_dir().join(format!(
        "oyo_changed_a_{}_{}.txt",
        std::process::id(),
        now_nanos
    ));
    let path_b = std::env::temp_dir().join(format!(
        "oyo_changed_b_{}_{}.txt",
        std::process::id(),
        now_nanos
    ));

    std::fs::write(&path_a, "A\n").expect("write file a");
    std::fs::write(&path_b, "B\n").expect("write file b");

    let diff = MultiFileDiff::from_file_pairs(vec![
        (path_a.clone(), "old A\n".to_string(), "A\n".to_string()),
        (path_b.clone(), "old B\n".to_string(), "B\n".to_string()),
    ]);
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);

    std::fs::write(&path_a, "A changed on disk\n").expect("update file a");
    std::fs::write(&path_b, "B changed on disk\n").expect("update file b");

    app.last_fs_check = Instant::now() - Duration::from_secs(2);
    app.maybe_check_file_changes();
    assert!(app.files_changed_on_disk);
    assert!(app.file_changed_on_disk(0));
    assert!(app.file_changed_on_disk(1));

    app.multi_diff.select_file(0);
    app.refresh_current_file();
    assert!(
        app.files_changed_on_disk,
        "indicator should stay on while another file remains changed"
    );
    assert!(!app.file_changed_on_disk(0));
    assert!(app.file_changed_on_disk(1));

    app.multi_diff.select_file(1);
    app.refresh_current_file();
    assert!(!app.files_changed_on_disk);
    assert!(!app.file_changed_on_disk(0));
    assert!(!app.file_changed_on_disk(1));

    let _ = std::fs::remove_file(path_a);
    let _ = std::fs::remove_file(path_b);
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
fn test_refresh_current_file_queues_deferred_diff_and_restores_decorations() {
    let _guard = DiffSettingsGuard::new(32);
    let path = std::env::temp_dir().join(format!(
        "oyo_refresh_deferred_{}_{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(&path, "new\n").expect("write file");
    let diff = MultiFileDiff::from_file_pair_with_sources(
        path.clone(),
        b"old\n".to_vec(),
        b"new\n".to_vec(),
        None,
        Some(path.clone()),
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
    app.stepping = false;
    app.no_step_auto_jump_on_enter = false;
    app.enter_no_step_mode();
    let revision = app.diff_revision();

    std::fs::write(
        &path,
        "new line 1\nnew line 2\nnew line 3\nnew line 4\nnew line 5\n",
    )
    .expect("update file");
    app.refresh_current_file();

    assert!(app.diff_revision() > revision);
    assert_eq!(app.diff_inflight, Some(0));
    assert!(matches!(
        app.multi_diff.current_file_diff_status(),
        DiffStatus::Computing
    ));

    let final_content =
        "final line 1\nfinal line 2\nfinal line 3\nfinal line 4\nfinal line 5\nfinal line 6\n";
    std::fs::write(&path, final_content).expect("update file again");
    app.refresh_current_file();
    assert!(app.diff_queue.contains(&0));

    let mut ready = false;
    for _ in 0..200 {
        app.poll_diff_responses();
        if matches!(app.multi_diff.current_file_diff_status(), DiffStatus::Ready) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(ready, "refreshed diff worker did not finish");
    assert!(!app.multi_diff.current_navigator_is_placeholder());
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some(final_content)
    );
    let state = app.multi_diff.current_navigator().state().clone();
    assert!(state.is_at_end());
    assert!(!app.multi_diff.current_navigator().diff().hunks.is_empty());

    std::fs::write(
        &path,
        "large again 1\nlarge again 2\nlarge again 3\nlarge again 4\n",
    )
    .expect("start another deferred refresh");
    app.refresh_current_file();
    assert_eq!(app.diff_inflight, Some(0));

    let immediate_content = "tiny\n";
    std::fs::write(&path, immediate_content).expect("replace with immediate diff");
    app.refresh_current_file();
    assert!(matches!(
        app.multi_diff.current_file_diff_status(),
        DiffStatus::Ready
    ));
    assert!(!app.diff_queue.contains(&0));
    for _ in 0..200 {
        app.poll_diff_responses();
        if app.diff_inflight.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let expected = MultiFileDiff::compute_diff("old\n", immediate_content);
    assert_eq!(
        app.multi_diff.file_contents(0).map(|(_, new)| new),
        Some(immediate_content)
    );
    assert_eq!(app.multi_diff.files[0].insertions, expected.insertions);
    assert_eq!(app.multi_diff.files[0].deletions, expected.deletions);
    assert!(!app.multi_diff.current_navigator().diff().hunks.is_empty());

    let _ = std::fs::remove_file(path);
}

#[test]
fn content_worker_replaces_loading_placeholder_without_moving_scroll() {
    let root = std::env::temp_dir().join(format!(
        "oyo-content-worker-{}-{:?}",
        std::process::id(),
        Instant::now()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("old.txt"), "old\n").unwrap();
    std::fs::write(root.join("new.txt"), "new\n").unwrap();
    let file = oyo_core::multi::FileEntry {
        path: "file.txt".into(),
        old_path: None,
        old_source_path: None,
        new_source_path: None,
        display_name: "file.txt".to_string(),
        status: oyo_core::git::FileStatus::Modified,
        insertions: 1,
        deletions: 1,
        binary: false,
    };
    let diff = MultiFileDiff::from_pending_files(
        Some(root.clone()),
        vec![(
            file,
            oyo_core::multi::ContentSource::File(root.join("old.txt")),
            oyo_core::multi::ContentSource::File(root.join("new.txt")),
        )],
        true,
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 100, false, None);
    app.scroll_offset = 7;
    assert_eq!(app.multi_diff.diff_status(0), DiffStatus::Loading);
    assert!(app.start_content_loading());
    assert_eq!(app.content_loading_count(), 1);
    for _ in 0..100 {
        if app.poll_content_responses() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(app.content_loading_count(), 0);
    assert_eq!(app.multi_diff.diff_status(0), DiffStatus::Ready);
    assert_eq!(app.multi_diff.file_contents(0), Some(("old\n", "new\n")));
    assert_eq!(app.scroll_offset, 7);
    assert_eq!(app.stats(), (1, 1));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn replacing_diff_restarts_pending_content_loading() {
    let root = std::env::temp_dir().join(format!(
        "oyo-content-replace-{}-{:?}",
        std::process::id(),
        Instant::now()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("old.txt"), "old\n").unwrap();
    std::fs::write(root.join("new.txt"), "new\n").unwrap();
    let entries = (0..2)
        .map(|idx| {
            let file = oyo_core::multi::FileEntry {
                path: format!("file-{idx}.txt").into(),
                old_path: None,
                old_source_path: None,
                new_source_path: None,
                display_name: format!("file-{idx}.txt"),
                status: oyo_core::git::FileStatus::Modified,
                insertions: 1,
                deletions: 1,
                binary: false,
            };
            (
                file,
                oyo_core::multi::ContentSource::File(root.join("old.txt")),
                oyo_core::multi::ContentSource::File(root.join("new.txt")),
            )
        })
        .collect();
    let initial = MultiFileDiff::from_file_pair(
        "file-0.txt".into(),
        "file-0.txt".into(),
        "before\n".into(),
        "after\n".into(),
    );
    let mut app = App::new(initial, ViewMode::UnifiedPane, 100, false, None);
    let replacement = MultiFileDiff::from_pending_files(Some(root.clone()), entries, true);

    app.replace_multi_diff(replacement);

    assert_eq!(app.multi_diff.diff_status(0), DiffStatus::Ready);
    assert_eq!(app.multi_diff.diff_status(1), DiffStatus::Loading);
    assert_eq!(app.content_loading_count(), 1);
    for _ in 0..100 {
        if app.poll_content_responses() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(app.multi_diff.diff_status(1), DiffStatus::Ready);
    assert_eq!(app.content_loading_count(), 0);
    let _ = std::fs::remove_dir_all(root);
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
