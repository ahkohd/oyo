use std::path::PathBuf;

use crate::app::{AnimationPhase, App, ViewMode};
use crate::config::{
    DiffForegroundMode, DiffHighlightMode, EvoSyntaxMode, ModifiedStepMode, SyntaxMode,
};
use crate::test_utils::TestApp;
use crate::views::{render_blame, render_evolution, render_split, render_unified_pane};
use oyo_core::{AnimationFrame, MultiFileDiff};
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

fn make_app(old: &str, new: &str, view_mode: ViewMode) -> TestApp {
    TestApp::new_default(|| {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("old.txt"),
            PathBuf::from("new.txt"),
            old.to_string(),
            new.to_string(),
        );
        let mut app = App::new(diff, view_mode, 200, false, None);
        app.animation_enabled = false;
        app.animation_phase = AnimationPhase::Idle;
        app.syntax_mode = SyntaxMode::Off;
        app.diff_bg = false;
        app.diff_fg = DiffForegroundMode::Theme;
        app.diff_highlight = DiffHighlightMode::Text;
        app
    })
}

fn render_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            match app.view_mode {
                ViewMode::UnifiedPane => render_unified_pane(frame, app, area),
                ViewMode::Split => render_split(frame, app, area),
                ViewMode::Evolution => render_evolution(frame, app, area),
                ViewMode::Blame => render_blame(frame, app, area),
            }
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn render_unified_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            render_unified_pane(frame, app, area);
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn buffer_text(buf: &Buffer) -> Vec<String> {
    let mut lines = Vec::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        lines.push(line);
    }
    lines
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

fn column_contains(buf: &Buffer, x: u16, needle: &str) -> bool {
    for y in 0..buf.area.height {
        if buf[(x, y)].symbol() == needle {
            return true;
        }
    }
    false
}

#[test]
fn test_unified_modified_lifecycle_render() {
    let old = "line1\nOLDSIDE\nline3\n";
    let new = "line1\nNEWSIDE\nline3\n";
    let mut app = make_app(old, new, ViewMode::UnifiedPane);

    let before = buffer_text(&render_buffer(&mut app, 80, 20)).join("\n");
    assert!(before.contains("OLDSIDE"));
    assert!(!before.contains("NEWSIDE"));

    app.next_step();
    let on_step_lines = buffer_text(&render_buffer(&mut app, 80, 20));
    assert!(
        on_step_lines
            .iter()
            .any(|line| line.contains("OLDSIDE") && line.contains("NEWSIDE")),
        "active modified line should show old + new"
    );

    app.multi_diff.current_navigator().clear_active_change();
    app.animation_phase = AnimationPhase::Idle;
    let after = buffer_text(&render_buffer(&mut app, 80, 20)).join("\n");
    assert!(!after.contains("OLDSIDE"));
    assert!(after.contains("NEWSIDE"));
}

#[test]
fn test_unified_peek_change_updates_render() {
    let old = "line1\nOLDTOKEN\nline3\n";
    let new = "line1\nNEWTOKEN\nline3\n";
    let mut app = make_app(old, new, ViewMode::UnifiedPane);
    app.unified_modified_step_mode = ModifiedStepMode::Mixed;

    app.next_step();
    let before = buffer_text(&render_buffer(&mut app, 80, 20)).join("\n");
    assert!(before.contains("OLDTOKEN"));
    assert!(before.contains("NEWTOKEN"));

    app.toggle_peek_old_change();
    let after = buffer_text(&render_buffer(&mut app, 80, 20)).join("\n");
    assert!(!after.contains("OLDTOKEN"));
    assert!(after.contains("NEWTOKEN"));
}

#[test]
fn test_split_modified_lifecycle_render() {
    let old = "line1\nOLDSPLIT\nline3\n";
    let new = "line1\nNEWSPLIT\nline3\n";
    let mut app = make_app(old, new, ViewMode::Split);

    let before = buffer_text(&render_buffer(&mut app, 100, 20)).join("\n");
    assert_eq!(count_occurrences(&before, "OLDSPLIT"), 2);
    assert!(!before.contains("NEWSPLIT"));

    app.next_step();
    app.multi_diff.current_navigator().clear_active_change();
    let after = buffer_text(&render_buffer(&mut app, 100, 20)).join("\n");
    assert_eq!(count_occurrences(&after, "OLDSPLIT"), 1);
    assert_eq!(count_occurrences(&after, "NEWSPLIT"), 1);
}

#[test]
fn test_evolution_full_preview_no_duplicate_modified_line() {
    let old = "line1\nOLDEVO\nline3\n";
    let new = "line1\nNEWEVO\nline3\n";
    let mut app = make_app(old, new, ViewMode::Evolution);
    app.syntax_mode = SyntaxMode::On;
    app.evo_syntax = EvoSyntaxMode::Full;

    app.next_hunk();
    app.next_hunk();
    let rendered = buffer_text(&render_buffer(&mut app, 80, 20)).join("\n");
    assert!(rendered.contains("NEWEVO"));
    assert!(!rendered.contains("OLDEVO"));
}

#[test]
fn test_evolution_deleted_active_fallback_marker() {
    let old = "line1\nDEL\nline3\n";
    let new = "line1\nline3\n";
    let mut app = make_app(old, new, ViewMode::Evolution);
    app.next_step(); // apply deletion
    app.animation_phase = AnimationPhase::Idle;

    let rendered = buffer_text(&render_buffer(&mut app, 60, 10)).join("\n");
    assert!(
        rendered.contains("▶"),
        "cursor marker should remain visible when deleted line is hidden"
    );
}

#[test]
fn test_evolution_window_cache_scroll_offset() {
    let old = (0..600).map(|i| format!("line {i}\n")).collect::<String>();
    let new = (0..600)
        .filter(|i| *i >= 50)
        .map(|i| format!("line {i} new\n"))
        .collect::<String>();
    let mut app = make_app(&old, &new, ViewMode::Evolution);
    app.last_viewport_height = 10;
    app.auto_center = false;
    app.needs_scroll_to_active = false;
    app.stepping = false;
    app.scroll_offset = 400;
    let span = app.last_viewport_height.max(20).saturating_mul(4).max(200);
    let scroll_offset = app.scroll_offset;
    let (_window_start, display_start) = {
        let nav = app.multi_diff.current_navigator();
        let window_start = scroll_offset.min(nav.diff().changes.len().saturating_sub(1).max(span));
        let display_start = nav
            .evolution_display_index_for_change_index(window_start)
            .unwrap_or(0);
        assert!(
            display_start < window_start,
            "expected evolution display start to differ from raw change index"
        );
        (window_start, display_start)
    };

    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    let start_first = app.view_window_start();
    assert_eq!(
        start_first, display_start,
        "window start should use evolution display index"
    );

    app.scroll_offset = start_first + 5;
    let view = app.current_view_with_frame(AnimationFrame::Idle);
    let start_second = app.view_window_start();
    let render_scroll = app.render_scroll_offset();

    assert_eq!(
        start_first, start_second,
        "cached view should preserve window start"
    );
    assert!(
        render_scroll < view.len(),
        "render scroll should stay inside windowed view"
    );
}

#[test]
fn test_unified_wrap_hunk_hint_overflow_places_above() {
    let long = "LONGINSERT_LONGINSERT_LONGINSERT_LONGINSERT";
    let old = "";
    let new = format!("{long}\nshort\n");
    let mut app = make_app(old, &new, ViewMode::UnifiedPane);
    app.line_wrap = true;

    for _ in 0..5 {
        if app.last_step_hint_text().is_some() {
            break;
        }
        app.next_step();
    }
    assert!(
        app.last_step_hint_text().is_some(),
        "should reach last-step hint state"
    );

    let lines = buffer_text(&render_buffer(&mut app, 20, 4));
    let hint_idx = lines
        .iter()
        .position(|line| line.contains("Last step"))
        .expect("virtual hint should render");
    let long_idx = lines
        .iter()
        .position(|line| line.contains("LONGINSERT"))
        .expect("insert line should render");
    assert!(
        hint_idx < long_idx,
        "wrapped overflow should place hint above the hunk"
    );
}

#[test]
fn test_unified_wrap_end_scroll_no_bounce() {
    let long = "LONGINSERT_LONGINSERT_LONGINSERT_LONGINSERT";
    let mut new = String::new();
    for idx in 0..40 {
        new.push_str(&format!("{long} {idx}\n"));
    }
    let mut app = make_app("", &new, ViewMode::UnifiedPane);
    app.line_wrap = true;
    app.auto_center = false;
    app.needs_scroll_to_active = false;
    app.no_step_auto_jump_on_enter = false;
    app.stepping = false;
    app.enter_no_step_mode();
    app.last_viewport_height = 4;
    app.scroll_offset = usize::MAX;

    let first = buffer_text(&render_buffer(&mut app, 20, 4));
    let max_scroll = app.scroll_offset;
    assert!(max_scroll > 0, "expected content to be scrollable");

    app.scroll_down();
    let second = buffer_text(&render_buffer(&mut app, 20, 4));

    assert_eq!(
        app.scroll_offset, max_scroll,
        "scroll offset should clamp at end"
    );
    assert_eq!(first, second, "render should not bounce at end");
}

#[test]
fn test_blame_end_scroll_no_bounce() {
    let long = "LONGINSERT_LONGINSERT_LONGINSERT_LONGINSERT";
    let mut new = String::new();
    for idx in 0..40 {
        new.push_str(&format!("{long} {idx}\n"));
    }
    let mut app = make_app("", &new, ViewMode::Blame);
    app.blame_enabled = true;
    app.line_wrap = false;
    app.auto_center = false;
    app.needs_scroll_to_active = false;
    app.no_step_auto_jump_on_enter = false;
    app.stepping = false;
    app.enter_no_step_mode();
    app.last_viewport_height = 4;
    app.scroll_offset = usize::MAX;

    let first = buffer_text(&render_buffer(&mut app, 30, 4));
    let max_scroll = app.scroll_offset;
    assert!(max_scroll > 0, "expected content to be scrollable");

    app.scroll_down();
    let second = buffer_text(&render_buffer(&mut app, 30, 4));

    assert_eq!(
        app.scroll_offset, max_scroll,
        "scroll offset should clamp at end"
    );
    assert_eq!(first, second, "render should not bounce at end");
}

#[test]
fn test_blame_large_file_end_scroll_no_empty_state() {
    let long = "LONGINSERT_LONGINSERT_LONGINSERT_LONGINSERT";
    let mut new = String::new();
    for idx in 0..200 {
        new.push_str(&format!("{long} {idx}\n"));
    }
    let mut app = TestApp::new_with_guard(32, || {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("old.txt"),
            PathBuf::from("new.txt"),
            String::new(),
            new,
        );
        let mut app = App::new(diff, ViewMode::Blame, 200, false, None);
        app.animation_enabled = false;
        app.animation_phase = AnimationPhase::Idle;
        app.syntax_mode = SyntaxMode::Off;
        app.diff_bg = false;
        app.diff_fg = DiffForegroundMode::Theme;
        app.diff_highlight = DiffHighlightMode::Text;
        app
    });
    app.blame_enabled = true;
    app.line_wrap = false;
    app.auto_center = false;
    app.needs_scroll_to_active = false;
    app.no_step_auto_jump_on_enter = false;
    app.stepping = false;
    app.enter_no_step_mode();
    app.last_viewport_height = 4;
    app.scroll_offset = usize::MAX;

    let view = app.current_view_with_frame(AnimationFrame::Idle);
    let mut extra_rows = vec![0; view.len()];
    if let Some(last) = extra_rows.last_mut() {
        *last = 2;
    }
    app.blame_extra_rows = Some(extra_rows);

    let first_buf = render_unified_buffer(&mut app, 30, 4);
    let first_text = buffer_text(&first_buf).join("\n");
    assert!(
        !first_text.contains("No content at this step"),
        "expected content at end of blame view"
    );
    let max_scroll = app.scroll_offset;

    app.scroll_down();
    let second_buf = render_unified_buffer(&mut app, 30, 4);
    let second_text = buffer_text(&second_buf).join("\n");

    assert_eq!(
        app.scroll_offset, max_scroll,
        "scroll offset should clamp at end"
    );
    assert_eq!(first_text, second_text, "render should not bounce at end");
}

#[test]
fn test_split_wrap_hunk_hint_overflow_places_above() {
    let long = "LONGINSERT".repeat(12);
    let old = "";
    let new = format!("{long}\nshort\n");
    let mut app = make_app(old, &new, ViewMode::Split);
    app.line_wrap = true;
    app.split_align_lines = true;

    for _ in 0..5 {
        if app.last_step_hint_text().is_some() {
            break;
        }
        app.next_step();
    }
    assert!(
        app.last_step_hint_text().is_some(),
        "should reach last-step hint state"
    );
    app.multi_diff.current_navigator().clear_active_change();

    let lines = buffer_text(&render_buffer(&mut app, 60, 4));
    let hint_idx = lines
        .iter()
        .position(|line| line.contains("Last step"))
        .expect("virtual hint should render");
    let long_idx = lines
        .iter()
        .position(|line| line.contains("LONGINSERT"))
        .expect("insert line should render");
    assert!(
        hint_idx < long_idx,
        "wrapped overflow should place hint above the hunk"
    );
}

#[test]
fn test_extent_markers_clear_at_start() {
    let old = "line1\nOLD_A\nOLD_B\nline4\n";
    let new = "line1\nNEW_A\nNEW_B\nline4\n";
    let mut app = make_app(old, new, ViewMode::UnifiedPane);
    app.extent_marker = "E".to_string();

    let before_buf = render_buffer(&mut app, 80, 10);
    assert!(
        !column_contains(&before_buf, 0, "E"),
        "extent markers should be hidden at step 0"
    );

    app.next_hunk();
    let in_hunk_buf = render_buffer(&mut app, 80, 10);
    assert!(
        column_contains(&in_hunk_buf, 0, "E"),
        "extent markers should show inside a hunk"
    );

    app.prev_step();
    app.multi_diff.current_navigator().clear_active_change();
    app.animation_phase = AnimationPhase::Idle;
    let after_buf = render_buffer(&mut app, 80, 10);
    assert!(
        !column_contains(&after_buf, 0, "E"),
        "extent markers should clear after hunk-out"
    );
}

#[test]
fn test_extent_markers_skip_context_by_default() {
    let old = "CTX\nOLD1\nOLD2\n";
    let new = "CTX\nNEW1\nNEW2\n";
    let mut app = make_app(old, new, ViewMode::UnifiedPane);
    app.extent_marker = "E".to_string();

    app.next_hunk();
    let buf = render_buffer(&mut app, 40, 8);
    assert!(
        column_contains(&buf, 0, "E"),
        "extent markers should show for changed lines"
    );

    let lines = buffer_text(&buf);
    let ctx_row = lines
        .iter()
        .position(|line| line.contains("CTX"))
        .expect("context line should render");
    assert_ne!(
        buf[(0, ctx_row as u16)].symbol(),
        "E",
        "context lines should not show extent markers by default"
    );
}
