use std::path::PathBuf;

use crate::app::{
    is_fold_line,
    review::{ReviewRange, ReviewSide, ReviewTargetKind},
    AnimationPhase, App, ViewMode,
};
use crate::config::{
    DiffForegroundMode, DiffHighlightMode, EvoSyntaxMode, ModifiedStepMode, SyntaxMode,
};
use crate::test_utils::TestApp;
use crate::views::{
    extent_marker_text, fold_context_band, render_blame, render_diff_scrollbar, render_evolution,
    render_split, render_unified_pane, review_note_block, show_extent_marker,
    unified_pane::TRAILING_REVIEW_SPACER_ROWS,
};
use oyo_core::{AnimationFrame, LineKind, MultiFileDiff, ViewLine};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier},
    Terminal,
};
use unicode_width::UnicodeWidthStr;

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

fn make_fold_scope_app(view_mode: ViewMode) -> App {
    let mut lines = vec!["fn inner_scope() {".to_string()];
    lines.extend((1..=60).map(|line| format!("    let value_{line} = {line};")));
    lines.push("}".to_string());
    let old = format!("{}\n", lines.join("\n"));
    lines[30] = "    let value_29 = 999;".to_string();
    let new = format!("{}\n", lines.join("\n"));
    let diff = MultiFileDiff::from_file_pair(
        PathBuf::from("scope.rs"),
        PathBuf::from("scope.rs"),
        old,
        new,
    );
    let mut app = App::new(diff, view_mode, 200, false, None);
    app.syntax_mode = SyntaxMode::Off;
    app.next_step();
    app
}

fn render_full_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| crate::ui::draw(frame, app))
        .expect("draw");
    terminal.backend().buffer().clone()
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
                ViewMode::Preview => {}
            }
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

#[test]
fn split_search_marks_the_current_match_bold_and_wraps() {
    let old = (1..=80)
        .map(|line| {
            if matches!(line, 20 | 70) {
                format!("needle old {line}")
            } else {
                format!("line {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let new = (1..=80)
        .map(|line| {
            if matches!(line, 20 | 70) {
                format!("needle new {line}")
            } else {
                format!("line {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = make_app(&old, &new, ViewMode::Split);
    app.start_search();
    for ch in "needle".chars() {
        app.push_search_char(ch);
    }
    app.search_next();
    let first = app.search_target().unwrap();

    let buffer = render_buffer(&mut app, 100, 20);
    let active_cells = (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let cell = &buffer[(x, y)];
            cell.bg == app.theme.accent && cell.modifier.contains(Modifier::BOLD)
        })
        .count();
    assert!(active_cells > 0);

    app.search_next();
    let second = app.search_target().unwrap();
    assert_ne!(second, first);
    let buffer = render_buffer(&mut app, 100, 20);
    assert!((0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .any(|(x, y)| {
            let cell = &buffer[(x, y)];
            cell.bg == app.theme.accent && cell.modifier.contains(Modifier::BOLD)
        }));
    app.search_next();
    assert_eq!(app.search_target(), Some(first));
}

#[test]
fn evolution_search_marks_a_match_below_fold_context_active() {
    let old = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let new = (1..=80)
        .map(|line| {
            if line == 70 {
                "needle".to_string()
            } else {
                format!("line {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = make_app(&old, &new, ViewMode::Evolution);
    app.goto_last_step();
    app.start_search();
    for ch in "needle".chars() {
        app.push_search_char(ch);
    }
    app.search_next();

    let buffer = render_buffer(&mut app, 100, 20);

    assert!((0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .any(|(x, y)| {
            let cell = &buffer[(x, y)];
            cell.bg == app.theme.accent && cell.modifier.contains(Modifier::BOLD)
        }));
}

#[test]
fn fold_scope_hint_needs_room_for_text_and_ellipsis() {
    assert_eq!(super::fold_scope_hint("function", 4), None);
    assert_eq!(
        super::fold_scope_hint("function", 5).as_deref(),
        Some("   f…")
    );
}

#[test]
fn fold_context_band_drops_scope_hint_before_controls() {
    let mut app = make_fold_scope_app(ViewMode::UnifiedPane);
    let fold_line = app
        .current_view_with_frame(AnimationFrame::Idle)
        .iter()
        .find(|line| crate::app::is_fold_line(line))
        .cloned()
        .unwrap();

    let wide = fold_context_band(&app, &fold_line, 80, Some(0)).unwrap();
    let wide_text = wide
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(wide_text.ends_with("   fn inner_scope() {"));
    assert!(wide
        .spans
        .last()
        .unwrap()
        .style
        .add_modifier
        .contains(Modifier::DIM));

    let narrow = fold_context_band(&app, &fold_line, 40, Some(0)).unwrap();
    let narrow_text = narrow
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(narrow_text.ends_with('…'));
    assert!(!narrow_text.ends_with("fn inner_scope() {"));
    assert!(narrow_text.width() <= 40);
}

#[test]
fn fold_context_scope_hint_renders_in_unified_and_split_views() {
    for mode in [ViewMode::UnifiedPane, ViewMode::Split] {
        let mut app = make_fold_scope_app(mode);
        let buffer = render_buffer(&mut app, 120, 20);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            text.contains("fn inner_scope() {"),
            "missing scope in {mode:?}"
        );
    }
}

#[test]
fn diff_scrollbar_track_uses_background_without_thumb() {
    let mut app = make_app("same\n", "same\n", ViewMode::UnifiedPane);
    app.theme.background = Some(Color::Blue);
    let backend = TestBackend::new(1, 4);
    let mut terminal = Terminal::new(backend).expect("terminal");

    terminal
        .draw(|frame| render_diff_scrollbar(frame, &mut app, frame.area(), 4, 4, 0))
        .expect("draw");

    assert!(terminal
        .backend()
        .buffer()
        .content
        .iter()
        .all(|cell| cell.style().bg == Some(Color::Blue)));
    assert!(app.diff_scrollbar.is_none());
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

fn resolved_review_app() -> TestApp {
    let mut app = make_app("old\n", "new\n", ViewMode::UnifiedPane);
    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    let id = app
        .add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 1, end: 1 }),
            "check this".to_string(),
        )
        .unwrap();
    assert!(app.set_review_comment_resolved_from_cli(id, true));
    app
}

#[test]
fn goto_end_reaches_last_line_card_in_every_fold_state() {
    let old = (1..=30)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let new = format!("{old}\nLASTADDED\n");
    let mut app = make_app(&format!("{old}\n"), &new, ViewMode::UnifiedPane);
    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    let root_id = app
        .add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 31, end: 31 }),
            "CARDBODY".to_string(),
        )
        .unwrap();

    app.goto_end();
    let folded_lines = buffer_text(&render_unified_buffer(&mut app, 80, 24));
    let folded = folded_lines.join("\n");
    assert!(folded.contains("LASTADDED"), "{folded}");
    assert!(folded.contains("CARDBODY"), "{folded}");
    assert!(
        folded_lines[folded_lines.len() - 1 - TRAILING_REVIEW_SPACER_ROWS].contains("╰ ia edit")
    );

    app.toggle_fold_context();
    app.goto_end();
    let unfolded = buffer_text(&render_unified_buffer(&mut app, 80, 24)).join("\n");
    assert!(unfolded.contains("LASTADDED"), "{unfolded}");
    assert!(unfolded.contains("CARDBODY"), "{unfolded}");

    app.toggle_fold_context();
    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    assert!(app.expand_all_context_folds());
    app.goto_end();
    let expanded = buffer_text(&render_unified_buffer(&mut app, 80, 24)).join("\n");
    assert!(expanded.contains("LASTADDED"), "{expanded}");
    assert!(expanded.contains("CARDBODY"), "{expanded}");

    for index in 1..=10 {
        app.add_review_reply_from_cli(root_id, format!("REPLY_{index}"))
            .unwrap();
    }
    app.animation_enabled = true;
    assert!(app.stepping);
    app.set_fold_context_mode(crate::config::FoldContextMode::Expandable);
    assert!(app
        .current_view_with_frame(AnimationFrame::Idle)
        .iter()
        .any(is_fold_line));
    app.goto_start();
    let _ = render_full_buffer(&mut app, 80, 24);
    app.goto_end();
    let mut folded_lines = Vec::new();
    for _ in 0..4 {
        app.tick();
        folded_lines = buffer_text(&render_full_buffer(&mut app, 80, 24));
    }
    let folded = folded_lines.join("\n");
    assert!(app.scroll_to_render_end_pending());
    assert!(folded.contains("REPLY_10"), "{folded}");
    let (_, diff_y, _, diff_height) = app.diff_view_area.unwrap();
    assert!(
        folded_lines[(diff_y + diff_height - 1 - TRAILING_REVIEW_SPACER_ROWS as u16) as usize]
            .contains("╰ ik edit"),
        "{folded}"
    );
    assert!(
        !folded_lines[(diff_y + diff_height - 1) as usize].contains("ik edit"),
        "{folded}"
    );

    app.toggle_fold_context();
    app.goto_end();
    let unfolded_lines = buffer_text(&render_unified_buffer(&mut app, 80, 24));
    let unfolded = unfolded_lines.join("\n");
    assert!(unfolded.contains("REPLY_10"), "{unfolded}");
    assert!(
        unfolded_lines[unfolded_lines.len() - 1 - TRAILING_REVIEW_SPACER_ROWS]
            .contains("╰ ik edit"),
        "{unfolded}"
    );

    app.toggle_fold_context();
    let _ = app.current_view_with_frame(AnimationFrame::Idle);
    assert!(app.expand_all_context_folds());
    app.goto_end();
    let expanded_lines = buffer_text(&render_unified_buffer(&mut app, 80, 24));
    let expanded = expanded_lines.join("\n");
    assert!(expanded.contains("REPLY_10"), "{expanded}");
    assert!(
        expanded_lines[expanded_lines.len() - 1 - TRAILING_REVIEW_SPACER_ROWS]
            .contains("╰ ik edit"),
        "{expanded}"
    );

    drop(app);
    let mut mid_file = make_app(&format!("{old}\n"), &new, ViewMode::UnifiedPane);
    mid_file.set_review_persist_enabled(false);
    mid_file.enable_review_mode();
    mid_file
        .add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 15, end: 15 }),
            "middle".to_string(),
        )
        .unwrap();
    mid_file.goto_end();
    let mid_lines = buffer_text(&render_unified_buffer(&mut mid_file, 80, 16));
    let action_row = mid_lines
        .iter()
        .position(|line| line.contains("╰ ia edit"))
        .unwrap();
    assert!(mid_lines[action_row + 1].contains("line 16"));
}

#[test]
fn full_context_scroll_reaches_near_eof_card_and_trailing_pad() {
    let old = (1..=387)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let new = format!("{old}\nhi\nthis\n");
    let mut app = make_app(&format!("{old}\n"), &new, ViewMode::UnifiedPane);
    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    app.add_review_comment_from_cli(
        "new.txt",
        ReviewTargetKind::Line,
        Some(ReviewSide::New),
        None,
        Some(ReviewRange {
            start: 388,
            end: 388,
        }),
        "hello".to_string(),
    )
    .unwrap();
    app.goto_end();
    app.toggle_fold_context();

    for _ in 0..40 {
        app.mark_user_input();
        app.scroll_up();
    }
    for _ in 0..40 {
        app.mark_user_input();
        app.scroll_down();
        app.tick();
        let _ = render_full_buffer(&mut app, 80, 24);
    }
    let mut lines = Vec::new();
    for _ in 0..4 {
        app.tick();
        lines = buffer_text(&render_full_buffer(&mut app, 80, 24));
    }
    let text = lines.join("\n");
    assert!(text.contains("hello"), "{text}");
    assert!(text.contains("va resolve"), "{text}");
    assert!(text.contains("this"), "{text}");

    for _ in 0..40 {
        app.mark_user_input();
        app.scroll_up();
    }
    for _ in 0..8 {
        app.mark_user_input();
        app.scroll_half_page_down(18);
        app.tick();
        let _ = render_full_buffer(&mut app, 80, 24);
    }
    let text = (0..4)
        .map(|_| {
            app.tick();
            buffer_text(&render_full_buffer(&mut app, 80, 24)).join("\n")
        })
        .last()
        .unwrap();
    assert!(text.contains("hello"), "{text}");
    assert!(text.contains("va resolve"), "{text}");
    assert!(text.contains("this"), "{text}");

    app.goto_end();
    for _ in 0..4 {
        app.tick();
        lines = buffer_text(&render_full_buffer(&mut app, 80, 24));
    }
    let text = lines.join("\n");
    assert!(text.contains("hello"), "{text}");
    assert!(text.contains("va resolve"), "{text}");
    assert!(text.contains("this"), "{text}");
    let (_, diff_y, _, diff_height) = app.diff_view_area.unwrap();
    let footer = lines
        .iter()
        .position(|line| line.contains("╰ ia edit"))
        .unwrap();
    assert_eq!(
        (diff_y + diff_height) as usize - footer - 1,
        TRAILING_REVIEW_SPACER_ROWS + 1
    );
}

#[test]
fn resolved_review_card_dims_content_border() {
    let mut app = resolved_review_app();
    let overlay = app.review_comment_overlays_for_current_file().remove(0);
    let block = review_note_block(&mut app, &overlay, 40);

    assert_eq!(block.lines[1].spans[0].style.fg, Some(app.theme.text_muted));
}

#[test]
fn review_actions_stay_on_card_border_lines() {
    let mut app = resolved_review_app();
    let overlay = app.review_comment_overlays_for_current_file().remove(0);
    let block = review_note_block(&mut app, &overlay, 40);
    let lines = block
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    let footer = lines
        .iter()
        .find(|line| line.starts_with("╰ ia edit"))
        .unwrap();
    assert!(footer.contains("ra reply"));
    assert!(footer.contains("va unresolve"));
    assert!(footer.contains("xa delete"));
    assert!(!lines.iter().any(|line| line.starts_with('├')));
}

#[test]
fn narrow_review_action_footer_does_not_wrap_hitbox_rows() {
    let mut app = resolved_review_app();
    let overlay = app.review_comment_overlays_for_current_file().remove(0);
    let block = review_note_block(&mut app, &overlay, 20);
    let footer = block.lines.last().unwrap();

    assert!(footer.width() > 20);
    assert_eq!(super::review_note_wrap_count(&footer.spans, 20), 1);
    let fitted = super::fit_review_note_footer(footer.spans.clone(), 20);
    assert_eq!(super::spans_width(&fitted), 20);
    assert_eq!(fitted.last().unwrap().content, "╯");
}

#[test]
fn resolved_review_unresolve_action_only_highlights_on_hover() {
    let mut app = resolved_review_app();
    let overlay = app.review_comment_overlays_for_current_file().remove(0);
    let block = review_note_block(&mut app, &overlay, 40);
    let label = block
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content.contains("unresolve"))
        .unwrap();
    assert_eq!(label.style.fg, Some(app.theme.text_muted));

    app.review_preview_resolve_hover = Some(overlay.anchor_key.clone());
    let block = review_note_block(&mut app, &overlay, 40);
    let label = block
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content.contains("unresolve"))
        .unwrap();
    assert_eq!(label.style.fg, Some(app.theme.accent));
}

#[test]
fn local_reply_thread_is_flat_with_a_connector() {
    let mut app = make_app("old\n", "new\n", ViewMode::UnifiedPane);
    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    let parent_id = app
        .add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 1, end: 1 }),
            "Parent".to_string(),
        )
        .unwrap();
    app.add_review_reply_from_cli(parent_id, "Child".to_string())
        .unwrap();
    let overlays = app.review_comment_overlays_for_current_file();
    let parent = overlays
        .iter()
        .find(|overlay| !overlay.anchor_key.starts_with("reply|"))
        .unwrap();
    let parent_block = review_note_block(&mut app, parent, 40);
    assert_eq!(
        parent_block
            .lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        " │"
    );
    let reply = overlays
        .iter()
        .find(|overlay| overlay.anchor_key.starts_with("reply|"))
        .unwrap();
    let reply_block = review_note_block(&mut app, reply, 40);
    assert_eq!(reply_block.lines[0].spans[0].content, "╭ ");
}

#[test]
fn binary_image_empty_state_points_to_preview() {
    let diff = MultiFileDiff::from_file_pair_bytes(
        PathBuf::from("image.png"),
        vec![0xff, 0x00],
        vec![0xff, 0x01],
    );
    let mut app = App::new(diff, ViewMode::UnifiedPane, 200, false, None);
    let text = buffer_text(&render_buffer(&mut app, 40, 5)).join("\n");

    assert!(text.contains("ctrl-p preview"), "empty state: {text}");
    assert!(!text.contains("preview disabled"), "empty state: {text}");

    app.set_review_persist_enabled(false);
    app.enable_review_mode();
    let text = buffer_text(&render_buffer(&mut app, 40, 5)).join("\n");
    assert!(text.contains("m comment"), "empty state: {text}");
    let (comment_x, comment_y, comment_width, _) =
        app.review_file_comment_hit.expect("comment action hitbox");
    assert!(app.handle_review_file_comment_click(comment_x + comment_width / 2, comment_y));
    assert!(app.review_editor_active());
    app.review_cancel_editor();

    app.diff_view_area = Some((0, 0, 40, 5));
    app.set_diff_selection_cells(vec![vec!["x".to_string(); 40]; 5]);
    assert_eq!(app.review_line_add_hover_at(38, 2), (None, false));
    let (x, y, width, _) = app.binary_preview_hit.expect("preview action hitbox");
    assert!(app.handle_binary_preview_click(x + width / 2, y));
    assert_eq!(app.view_mode, ViewMode::Preview);
}

fn column_contains(buf: &Buffer, x: u16, needle: &str) -> bool {
    for y in 0..buf.area.height {
        if buf[(x, y)].symbol() == needle {
            return true;
        }
    }
    false
}

fn marker_view_line(kind: LineKind, has_changes: bool) -> ViewLine {
    ViewLine {
        content: String::new(),
        spans: vec![],
        kind,
        old_line: Some(1),
        new_line: Some(1),
        is_active: false,
        is_active_change: false,
        is_primary_active: false,
        show_hunk_extent: false,
        change_id: 0,
        hunk_index: Some(0),
        has_changes,
    }
}

#[test]
fn deleted_extent_marker_uses_dashed_bar() {
    let deleted = marker_view_line(LineKind::Deleted, true);
    let inserted = marker_view_line(LineKind::Inserted, true);

    assert_eq!(extent_marker_text("▌", "D", &deleted), "D");
    assert_eq!(extent_marker_text("▌", "D", &inserted), "▌");
}

#[test]
fn no_step_extent_marker_shows_changed_hunk_lines() {
    let mut app = make_app("old\n", "new\n", ViewMode::UnifiedPane);
    app.stepping = false;

    let changed = marker_view_line(LineKind::Modified, true);
    assert!(show_extent_marker(&app, &changed));

    let context = marker_view_line(LineKind::Context, false);
    assert!(!show_extent_marker(&app, &context));
    app.diff_extent_marker_context = true;
    assert!(show_extent_marker(&app, &context));
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
