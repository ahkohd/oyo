use std::path::PathBuf;

use crate::app::{AnimationPhase, App, ViewMode};
use crate::config::{DiffForegroundMode, DiffHighlightMode, EvoSyntaxMode, SyntaxMode};
use crate::views::{render_evolution, render_single_pane, render_split};
use oyo_core::MultiFileDiff;
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

fn make_app(old: &str, new: &str, view_mode: ViewMode) -> App {
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
}

fn render_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            match app.view_mode {
                ViewMode::SinglePane => render_single_pane(frame, app, area),
                ViewMode::Split => render_split(frame, app, area),
                ViewMode::Evolution => render_evolution(frame, app, area),
            }
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

#[test]
fn test_single_modified_lifecycle_render() {
    let old = "line1\nOLDSIDE\nline3\n";
    let new = "line1\nNEWSIDE\nline3\n";
    let mut app = make_app(old, new, ViewMode::SinglePane);

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
