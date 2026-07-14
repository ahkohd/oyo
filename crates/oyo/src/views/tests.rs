use std::path::PathBuf;

use crate::app::{
    is_fold_line,
    review::{ReviewRange, ReviewSide, ReviewTargetKind},
    AnimationPhase, App, ReviewCommentContextMenuAction, SettingItem, ViewMode,
};
use crate::config::{
    DiffForegroundMode, DiffHighlightMode, EvoSyntaxMode, ModifiedStepMode, SyntaxMode,
};
use crate::test_utils::TestApp;
use crate::views::{
    extent_marker_text, fold_context_band, render_blame, render_diff_scrollbar, render_evolution,
    render_split, render_unified_pane, review_note_block, show_extent_marker,
    unified_pane::TRAILING_REVIEW_SPACER_ROWS, wrap_review_card_spans,
};
use oyo_core::{AnimationFrame, LineKind, MultiFileDiff, ViewLine};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier, Style},
    text::Span,
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
fn review_line_add_button_only_renders_on_file_tabs() {
    let mut app = make_app("old\n", "new\n", ViewMode::UnifiedPane);
    app.enable_review_mode();
    render_full_buffer(&mut app, 80, 24);
    let (_, y, _, _) = app.diff_view_area.unwrap();
    app.review_line_add_row = Some(y + 1);

    render_full_buffer(&mut app, 80, 24);
    assert!(app.review_line_add_hit.is_some());

    app.open_settings_tab();
    app.review_line_add_row = Some(y + 1);
    render_full_buffer(&mut app, 80, 24);
    assert!(app.review_line_add_hit.is_none());
}

#[test]
fn settings_view_renders_live_values_and_mouse_activates_rows() {
    let mut app = make_app("old\n", "new\n", ViewMode::UnifiedPane);
    let dir = std::env::temp_dir().join(format!("oyo-settings-view-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    app.settings_config_path_override = Some(dir.join("config.toml"));
    app.open_settings_tab();

    let lines = buffer_text(&render_full_buffer(&mut app, 90, 32));
    let text = lines.join("\n");
    assert!(text.contains("Settings"), "{text}");
    assert!(text.contains("unsaved changes"), "{text}");
    assert!(text.contains("View mode"), "{text}");
    assert!(text.contains("Line wrap"), "{text}");
    assert!(text.contains("General"), "{text}");
    let view_line = lines
        .iter()
        .find(|line| line.contains("View mode"))
        .unwrap();
    let fold_line = lines
        .iter()
        .find(|line| line.contains("Fold context"))
        .unwrap();
    assert!(view_line.contains("‹ unified ›"), "{view_line}");
    assert!(!view_line.trim_start().starts_with("> "), "{view_line}");
    assert!(!fold_line.contains('‹'), "{fold_line}");
    let value_start = |line: &str, value: &str| {
        let start = line.find(value).unwrap();
        UnicodeWidthStr::width(&line[..start])
    };
    assert_eq!(
        value_start(view_line, "unified"),
        value_start(fold_line, "expandable")
    );
    let view_hint = value_start(view_line, "cycle");
    let fold_hint = value_start(fold_line, "collapse");
    assert_eq!(view_hint, fold_hint);
    assert_eq!(
        fold_hint - value_start(fold_line, "expandable"),
        UnicodeWidthStr::width("change syntax theme…") + 2
    );
    let selected_start = value_start(view_line, "unified");
    app.settings_selection = SettingItem::FoldContext as usize;
    let lines = buffer_text(&render_full_buffer(&mut app, 90, 32));
    let view_line = lines
        .iter()
        .find(|line| line.contains("View mode"))
        .unwrap();
    assert_eq!(value_start(view_line, "unified"), selected_start);
    let first_hit = app.settings_hits[0];
    assert!(!app.handle_settings_click(first_hit.x, first_hit.y - 1));

    app.settings_selection = SettingItem::Syntax as usize;
    let lines = buffer_text(&render_full_buffer(&mut app, 90, 32));
    let text = lines.join("\n");
    let diff_row = lines
        .iter()
        .position(|line| line.rsplit('▕').next().unwrap_or(line).trim() == "Diff")
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        lines[diff_row - 1]
            .rsplit('▕')
            .next()
            .unwrap_or(&lines[diff_row - 1])
            .trim()
            .is_empty(),
        "{text}"
    );

    app.settings_selection = SettingItem::FilePanelPosition as usize;
    let lines = buffer_text(&render_full_buffer(&mut app, 90, 32));
    let panel_side_line = lines
        .iter()
        .find(|line| line.contains("File panel side"))
        .unwrap();
    assert!(panel_side_line.contains("‹ left ›"), "{panel_side_line}");
    assert!(app.file_panel_rect.unwrap().0 < app.diff_view_area.unwrap().0);
    assert!(app.settings_hits.iter().any(|hit| {
        hit.target == crate::app::SettingsTarget::Item(SettingItem::FilePanelPosition)
    }));
    app.adjust_selected_setting(true);
    let text = buffer_text(&render_full_buffer(&mut app, 90, 32)).join("\n");
    assert!(text.contains("‹ right ›"), "{text}");
    assert!(app.file_panel_rect.unwrap().0 > app.diff_view_area.unwrap().0);

    app.settings_selection = SettingItem::Theme as usize;
    let lines = buffer_text(&render_full_buffer(&mut app, 90, 32));
    let theme_line = lines
        .iter()
        .find(|line| line.contains("Colour theme"))
        .unwrap();
    assert!(theme_line.contains("change theme…"), "{theme_line}");
    assert!(!theme_line.contains('‹'), "{theme_line}");

    app.settings_selection = SettingItem::ALL.len();
    let lines = buffer_text(&render_full_buffer(&mut app, 90, 32));
    let theme_line = lines
        .iter()
        .find(|line| line.contains("Colour theme"))
        .unwrap();
    assert!(theme_line.contains("default"), "{theme_line}");
    assert!(!theme_line.contains("change theme…"), "{theme_line}");
    let theme_hit = app
        .settings_hits
        .iter()
        .find(|hit| hit.target == crate::app::SettingsTarget::Item(SettingItem::Theme))
        .copied()
        .unwrap();
    assert!(app.update_settings_hover(theme_hit.x, theme_hit.y));
    assert_eq!(
        app.settings_selected_target(),
        crate::app::SettingsTarget::Item(SettingItem::Theme)
    );
    let text = buffer_text(&render_full_buffer(&mut app, 90, 32)).join("\n");
    assert!(text.contains("change theme…"), "{text}");
    assert!(app.update_settings_hover(0, 0));
    let text = buffer_text(&render_full_buffer(&mut app, 90, 32)).join("\n");
    assert!(text.contains("change theme…"), "{text}");
    assert!(app.handle_settings_click(theme_hit.x, theme_hit.y));
    assert!(app.theme_picker_active());
    app.stop_theme_picker();
    app.settings_hover = None;

    app.settings_selection = SettingItem::ALL.len();
    let text = buffer_text(&render_full_buffer(&mut app, 90, 32)).join("\n");
    assert!(text.contains("default"), "{text}");
    assert!(!text.contains("change theme…"), "{text}");

    app.settings_selection = SettingItem::SyntaxTheme as usize;
    let text = buffer_text(&render_full_buffer(&mut app, 90, 32)).join("\n");
    assert!(text.contains("change syntax theme…"), "{text}");
    let syntax_hit = app
        .settings_hits
        .iter()
        .find(|hit| hit.target == crate::app::SettingsTarget::Item(SettingItem::SyntaxTheme))
        .copied()
        .unwrap();
    assert_eq!(
        syntax_hit.target,
        crate::app::SettingsTarget::Item(SettingItem::SyntaxTheme)
    );
    app.activate_selected_setting();
    assert!(app.theme_picker_active());
    app.stop_theme_picker();
    app.settings_hover = None;

    app.settings_selection = SettingItem::ALL.len() + 2;
    let buffer = render_full_buffer(&mut app, 140, 32);
    let lines = buffer_text(&buffer);
    let text = lines.join("\n");
    assert!(text.contains("Appearance"), "{text}");
    assert!(text.contains("  Save  "), "{text}");
    assert!(text.contains("  Revert  "), "{text}");
    assert!(text.contains("  Reset to defaults  "), "{text}");
    assert!(!text.contains("[ Save ]"), "{text}");
    assert!(app.settings_scroll > 0);
    let button_hit = |target| {
        app.settings_hits
            .iter()
            .find(|hit| hit.target == target)
            .copied()
            .unwrap()
    };
    let save = button_hit(crate::app::SettingsTarget::Save);
    let revert = button_hit(crate::app::SettingsTarget::Revert);
    let reset = button_hit(crate::app::SettingsTarget::ResetDefaults);
    let theme = button_hit(crate::app::SettingsTarget::Item(SettingItem::Theme));
    let theme_line = &lines[theme.y as usize];
    let hint_x =
        UnicodeWidthStr::width(&theme_line[..theme_line.find("open colour theme picker").unwrap()]);
    let reset_line = &lines[reset.y as usize];
    let reset_text_x =
        UnicodeWidthStr::width(&reset_line[..reset_line.find("Reset to defaults").unwrap()]);
    assert_eq!(reset_text_x, hint_x);
    assert_eq!(save.y.saturating_sub(theme.y), 3);
    assert_eq!(revert.x.saturating_sub(save.x + save.width), 3);
    assert_eq!(reset.x.saturating_sub(revert.x + revert.width), 3);
    assert_eq!(buffer[(reset.x, reset.y)].bg, app.theme.error);
    for target in [
        crate::app::SettingsTarget::Save,
        crate::app::SettingsTarget::Revert,
        crate::app::SettingsTarget::ResetDefaults,
    ] {
        assert!(app.settings_hits.iter().any(|hit| hit.target == target));
    }
    app.settings_selection = SettingItem::ALL.len();
    let buffer = render_full_buffer(&mut app, 140, 32);
    assert_eq!(buffer[(reset.x, reset.y)].fg, app.theme.error);
    assert!(app.update_settings_hover(reset.x, reset.y));
    let buffer = render_full_buffer(&mut app, 140, 32);
    assert_eq!(buffer[(reset.x, reset.y)].bg, app.theme.error);
    assert!(app.handle_settings_click(reset.x, reset.y));
    assert!(app.settings_reset_confirmation_active());
    app.cancel_settings_reset_confirmation();

    app.settings_selection = SettingItem::ALL.len() + 2;
    render_full_buffer(&mut app, 80, 24);
    let reset = app
        .settings_hits
        .iter()
        .find(|hit| hit.target == crate::app::SettingsTarget::ResetDefaults)
        .copied()
        .unwrap();
    let (diff_x, _, diff_width, _) = app.diff_view_area.unwrap();
    assert!(reset.x + reset.width <= diff_x + diff_width - 2);

    app.settings_selection = 2;
    render_full_buffer(&mut app, 80, 24);
    let hit = app
        .settings_hits
        .iter()
        .find(|hit| hit.target == crate::app::SettingsTarget::Item(SettingItem::LineWrap))
        .copied()
        .unwrap();
    assert!(app.handle_settings_click(hit.x, hit.y));
    assert!(app.line_wrap);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn settings_dirty_value_uses_warning_without_moving_the_column() {
    let mut app = make_app("old\n", "new\n", ViewMode::UnifiedPane);
    let dir = std::env::temp_dir().join(format!("oyo-settings-dirty-row-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    app.settings_config_path_override = Some(dir.join("config.toml"));
    app.open_settings_tab();
    assert!(app.save_settings());
    app.settings_selection = SettingItem::ViewMode as usize;
    render_full_buffer(&mut app, 90, 32);
    let scrollbar_hit = app
        .settings_hits
        .iter()
        .find(|hit| hit.target == crate::app::SettingsTarget::Item(SettingItem::Scrollbar))
        .copied()
        .unwrap();
    assert!(app.update_settings_hover(scrollbar_hit.x, scrollbar_hit.y));
    assert_eq!(
        app.settings_selected_target(),
        crate::app::SettingsTarget::Item(SettingItem::Scrollbar)
    );

    let value_cell = |buffer: &Buffer, row_label: &str, value: &str| {
        let lines = buffer_text(buffer);
        let (y, line) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains(row_label))
            .unwrap();
        let byte = line.find(value).unwrap();
        let x = UnicodeWidthStr::width(&line[..byte]);
        (x as u16, y as u16)
    };

    let clean = render_full_buffer(&mut app, 90, 32);
    let clean_value = if app.scrollbar_visible { "on" } else { "off" };
    let clean_chevrons = format!("‹ {clean_value} ›");
    let clean_cell = value_cell(&clean, "Scrollbar", &clean_chevrons);
    assert_eq!(clean[clean_cell].fg, app.theme.accent);
    let lines = buffer_text(&clean);
    let view_line = lines
        .iter()
        .find(|line| line.contains("View mode"))
        .unwrap();
    assert!(!view_line.contains('‹'), "{view_line}");

    app.scrollbar_visible = !app.scrollbar_visible;
    let dirty = render_full_buffer(&mut app, 90, 32);
    let dirty_value = if app.scrollbar_visible { "on" } else { "off" };
    let dirty_chevrons = format!("‹ {dirty_value} ›");
    let dirty_cell = value_cell(&dirty, "Scrollbar", &dirty_chevrons);
    assert_eq!(dirty[dirty_cell].fg, app.theme.warning);
    assert_eq!(dirty_cell.0, clean_cell.0);

    let line_wrap = if app.line_wrap { "on" } else { "off" };
    let line_wrap_cell = value_cell(&dirty, "Line wrap", line_wrap);
    assert_eq!(dirty[line_wrap_cell].fg, app.theme.text);
    let line_wrap_hint = value_cell(&dirty, "Line wrap", "wrap long");
    assert_eq!(dirty[line_wrap_hint].fg, app.theme.text_muted);

    assert!(app.save_settings());
    let saved = render_full_buffer(&mut app, 90, 32);
    let saved_cell = value_cell(&saved, "Scrollbar", &dirty_chevrons);
    assert_ne!(saved[saved_cell].fg, app.theme.warning);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn review_card_wraps_words_across_styles() {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let lines = wrap_review_card_spans(
        vec![
            Span::styled("lorem", bold),
            Span::raw(" "),
            Span::raw("ipsum"),
        ],
        8,
    );
    let text = lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(text, ["lorem", "ipsum"]);
    assert_eq!(lines[0][0].style, bold);
    assert_eq!(lines[1][0].style, Style::default());
}

#[test]
fn review_card_splits_only_overlong_words() {
    let lines = wrap_review_card_spans(vec![Span::raw("abcdefghijk")], 4);
    let text = lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(text, ["abcd", "efgh", "ijk"]);
    assert!(lines.iter().all(|line| super::spans_width(line) <= 4));
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

    app.review_preview_resolve_hover = Some(overlay.id);
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
fn shared_anchor_thread_hover_and_flash_are_isolated_by_comment_id() {
    let mut app = resolved_review_app();
    let overlay = app.review_comment_overlays_for_current_file().remove(0);
    let mut sibling = overlay.clone();
    sibling.id = overlay.id + 1;
    assert_eq!(sibling.anchor_key, overlay.anchor_key);

    app.review_preview_hover = Some(overlay.anchor_key.clone());
    app.review_preview_hover_id = Some(overlay.id);
    let selected = review_note_block(&mut app, &overlay, 60);
    let sibling_block = review_note_block(&mut app, &sibling, 60);
    assert_eq!(selected.lines[0].spans[0].style.fg, Some(app.theme.accent));
    assert_ne!(
        sibling_block.lines[0].spans[0].style.fg,
        Some(app.theme.accent)
    );

    let checks = [
        ("edit", ReviewCommentContextMenuAction::Body),
        ("reply", ReviewCommentContextMenuAction::Id),
        ("unresolve", ReviewCommentContextMenuAction::FileLine),
        ("delete", ReviewCommentContextMenuAction::Url),
        ("…", ReviewCommentContextMenuAction::MarkdownQuote),
    ];
    for (label, action) in checks {
        app.review_preview_edit_hover = None;
        app.review_preview_reply_hover = None;
        app.review_preview_resolve_hover = None;
        app.review_preview_delete_hover = None;
        app.review_preview_overflow_hover = None;
        match action {
            ReviewCommentContextMenuAction::Body => {
                app.review_preview_edit_hover = Some(overlay.id)
            }
            ReviewCommentContextMenuAction::Id => app.review_preview_reply_hover = Some(overlay.id),
            ReviewCommentContextMenuAction::FileLine => {
                app.review_preview_resolve_hover = Some(overlay.id)
            }
            ReviewCommentContextMenuAction::Url => {
                app.review_preview_delete_hover = Some(overlay.id)
            }
            ReviewCommentContextMenuAction::MarkdownQuote => {
                app.review_preview_overflow_hover = Some(overlay.id)
            }
        }
        let selected = review_note_block(&mut app, &overlay, 60);
        let sibling_block = review_note_block(&mut app, &sibling, 60);
        let selected_style = selected
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains(label))
            .unwrap()
            .style;
        let sibling_style = sibling_block
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains(label))
            .unwrap()
            .style;
        assert_ne!(selected_style, sibling_style, "{label}");
    }

    app.review_preview_hover = None;
    app.review_preview_hover_id = None;
    assert!(app.open_review_comment(0));
    let selected = review_note_block(&mut app, &overlay, 60);
    let sibling_block = review_note_block(&mut app, &sibling, 60);
    assert_eq!(selected.lines[0].spans[0].style.fg, Some(app.theme.accent));
    assert_ne!(
        sibling_block.lines[0].spans[0].style.fg,
        Some(app.theme.accent)
    );
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
