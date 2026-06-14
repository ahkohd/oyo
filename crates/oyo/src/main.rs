//! Oyo CLI - Step-through diff viewer TUI

mod app;
mod blame;
mod color;
mod config;
mod dashboard;
mod input;
mod keybindings;
mod syntax;
#[cfg(test)]
mod test_utils;
mod time_format;
mod ui;
mod views;

use crate::dashboard::{Dashboard, DashboardConfig, DashboardSelection};
use crate::input::handle_app_key;
use crate::keybindings::{DashboardAction, DashboardFilterAction, Dispatch, Keybindings};
use crate::syntax::{list_syntax_themes, SyntaxEngine};
use crate::time_format::TimeFormatter;
use anyhow::{anyhow, Context, Result};
use app::{App, ViewMode};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use oyo_core::{multi::FileSide, DirectoryScanOptions, LineKind, MultiFileDiff, ViewLine};
use ratatui::prelude::*;
use std::fs::OpenOptions;
use std::io::{self, IsTerminal};
use std::path::{Component, Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::Duration;

const INDEX_REF: &str = "INDEX";

type TuiBackend = CrosstermBackend<Box<dyn io::Write>>;
type TuiTerminal = Terminal<TuiBackend>;

#[derive(Parser, Debug)]
#[command(name = "oy")]
#[command(author, version, about = "A step-through diff viewer")]
#[command(args_conflicts_with_subcommands = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Files or directories to compare: old_file new_file
    /// Single file compares against HEAD (like git diff)
    /// Also works as a git external diff tool (git config diff.external oy)
    #[arg(num_args = 0..)]
    paths: Vec<PathBuf>,

    /// View mode: unified, split, or evolution
    #[arg(short, long, default_value = "unified")]
    view: CliViewMode,

    /// Animation speed in milliseconds
    #[arg(short, long, default_value = "200")]
    speed: u64,

    /// Auto-play through all changes
    #[arg(long)]
    autoplay: bool,

    /// Theme mode: dark or light
    #[arg(long, value_enum, global = true)]
    theme_mode: Option<CliThemeMode>,

    /// Theme name (overrides config)
    #[arg(long, global = true)]
    theme_name: Option<String>,

    /// Syntax theme name or .tmTheme file (overrides config)
    #[arg(long, global = true)]
    syntax_theme: Option<String>,

    /// Dump syntax scopes for a file and exit
    #[arg(long, value_name = "FILE")]
    dump_scopes: Option<PathBuf>,

    /// Disable stepping (no-step diff view)
    #[arg(long, global = true)]
    no_step: bool,

    /// Show staged changes (index vs HEAD)
    #[arg(long, alias = "cached", conflicts_with = "range")]
    staged: bool,

    /// Diff a git range (e.g. HEAD~1..HEAD)
    #[arg(long, value_name = "RANGE", conflicts_with = "staged")]
    range: Option<String>,

    /// Write review comments to this file on quit
    #[arg(long, value_name = "FILE", global = true)]
    review_output_file: Option<PathBuf>,

    /// Do not print review comments to stdout (requires --review-output-file)
    #[arg(long, requires = "review_output_file", global = true)]
    no_print_review: bool,

    /// Disable loading/saving persisted review session comments
    #[arg(long, global = true)]
    no_review_persist: bool,

    /// Respect git ignore files during directory scans
    #[arg(long, global = true, conflicts_with = "no_git_ignore")]
    git_ignore: bool,

    /// Do not respect git ignore files during directory scans
    #[arg(long, global = true, conflicts_with = "git_ignore")]
    no_git_ignore: bool,

    /// Glob patterns to exclude during directory scans (pipe-separated, repeatable)
    #[arg(long, value_name = "GLOBS", global = true)]
    ignore_glob: Vec<String>,

    /// Clear saved review session state for the current diff on startup
    #[arg(long, global = true)]
    clear_review_session: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List built-in themes
    Themes,
    /// List syntax themes
    SyntaxThemes,
    /// Open the git range picker dashboard
    View {
        /// Number of commits to show
        #[arg(long, default_value = "200")]
        limit: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum CliThemeMode {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum CliViewMode {
    /// Unified pane that morphs from old to new state
    Unified,
    /// Split view with synchronized stepping
    #[value(alias = "sbs")]
    Split,
    /// Evolution view - shows file morphing, deletions just disappear
    #[value(alias = "evo")]
    Evolution,
    /// Blame view - per-line blame gutter
    Blame,
}

impl From<CliViewMode> for ViewMode {
    fn from(mode: CliViewMode) -> Self {
        match mode {
            CliViewMode::Unified => ViewMode::UnifiedPane,
            CliViewMode::Split => ViewMode::Split,
            CliViewMode::Evolution => ViewMode::Evolution,
            CliViewMode::Blame => ViewMode::Blame,
        }
    }
}

/// Represents input mode detected from arguments
enum InputMode {
    /// Git external diff: path old-file old-hex old-mode new-file new-hex new-mode
    GitExternal {
        display_path: PathBuf,
        old_file: PathBuf,
        new_file: PathBuf,
    },
    /// Two files or directories to compare
    TwoPaths {
        old_path: PathBuf,
        new_path: PathBuf,
    },
    /// Single file compared against HEAD
    GitFile { path: PathBuf },
    /// No args - try git uncommitted changes in current directory
    GitUncommitted,
    /// Staged changes (index vs HEAD)
    GitStaged,
    /// Git range
    GitRange { from: String, to: String },
    /// No valid input
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppExit {
    Quit,
    OpenDashboard,
}

/// Detect if we're being called as a git external diff tool
/// Git calls: oy path old-file old-hex old-mode new-file new-hex new-mode
fn detect_input_mode(paths: &[PathBuf]) -> InputMode {
    if paths.len() == 7 {
        // Git external diff format
        let display_path = paths[0].clone();
        let old_file = paths[1].clone();
        let new_file = paths[4].clone();
        InputMode::GitExternal {
            display_path,
            old_file,
            new_file,
        }
    } else if paths.len() >= 2 {
        InputMode::TwoPaths {
            old_path: paths[0].clone(),
            new_path: paths[1].clone(),
        }
    } else if paths.len() == 1 {
        InputMode::GitFile {
            path: paths[0].clone(),
        }
    } else if paths.is_empty() {
        // No args - try git uncommitted changes
        InputMode::GitUncommitted
    } else {
        InputMode::None
    }
}

fn parse_range(range: &str) -> Result<(String, String)> {
    if let Some((from, to)) = range.split_once("...") {
        if from.is_empty() || to.is_empty() {
            anyhow::bail!("Range must be in the form A..B or A...B");
        }
        if to.contains("..") {
            anyhow::bail!("Range must be in the form A..B or A...B");
        }
        return Ok((from.to_string(), to.to_string()));
    }
    if let Some((from, to)) = range.split_once("..") {
        if from.is_empty() || to.is_empty() {
            anyhow::bail!("Range must be in the form A..B or A...B");
        }
        if to.contains("..") {
            anyhow::bail!("Range must be in the form A..B or A...B");
        }
        return Ok((from.to_string(), to.to_string()));
    }
    anyhow::bail!("Range must be in the form A..B or A...B");
}

fn split_ignore_globs(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| value.split('|'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn looks_like_jj_external_diff_dirs(old_path: &Path, new_path: &Path) -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let old_abs = if old_path.is_absolute() {
        old_path.to_path_buf()
    } else {
        cwd.join(old_path)
    };
    let new_abs = if new_path.is_absolute() {
        new_path.to_path_buf()
    } else {
        cwd.join(new_path)
    };

    if old_abs.file_name().and_then(|name| name.to_str()) != Some("left") {
        return false;
    }
    if new_abs.file_name().and_then(|name| name.to_str()) != Some("right") {
        return false;
    }
    let Some(old_parent) = old_abs.parent() else {
        return false;
    };
    if new_abs.parent() != Some(old_parent) {
        return false;
    }

    old_parent
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("jj-diff-"))
        .unwrap_or(false)
}

fn is_jj_diff_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("jj-diff-"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, after_name) = stat.rsplit_once(") ")?;
    let mut fields = after_name.split_whitespace();
    fields.next()?;
    fields.next()?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn parent_pid(_pid: u32) -> Option<u32> {
    None
}

#[cfg(target_os = "linux")]
fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(not(target_os = "linux"))]
fn process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

fn infer_external_diff_workspace_root() -> Option<PathBuf> {
    let mut pid = parent_pid(std::process::id())?;
    for _ in 0..8 {
        if let Some(cwd) = process_cwd(pid) {
            if cwd.is_dir() && !is_jj_diff_dir(&cwd) {
                return Some(cwd);
            }
        }
        pid = parent_pid(pid)?;
    }
    None
}

fn directory_scan_options(
    config: &config::Config,
    args: &Args,
    old_path: &Path,
    new_path: &Path,
) -> DirectoryScanOptions {
    let vcs_external_diff = looks_like_jj_external_diff_dirs(old_path, new_path);
    let mut git_ignore = match config.files.scan.git_ignore {
        config::GitIgnoreMode::Auto => !vcs_external_diff,
        config::GitIgnoreMode::On => true,
        config::GitIgnoreMode::Off => false,
    };
    if args.git_ignore {
        git_ignore = true;
    }
    if args.no_git_ignore {
        git_ignore = false;
    }

    let mut ignore_globs = config.files.scan.ignore_globs.clone();
    ignore_globs.extend(split_ignore_globs(&args.ignore_glob));
    DirectoryScanOptions {
        git_ignore,
        ignore_globs,
    }
}

fn setup_terminal() -> Result<TuiTerminal> {
    enable_raw_mode()?;
    let mut stdout: Box<dyn io::Write> = if io::stdout().is_terminal() {
        Box::new(io::stdout())
    } else {
        match OpenOptions::new().read(true).write(true).open("/dev/tty") {
            Ok(file) => Box::new(file),
            Err(_) => Box::new(io::stdout()),
        }
    };
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorSide {
    Old,
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EditorFocus {
    side: EditorSide,
    line: usize,
}

struct EditorTarget {
    path: PathBuf,
    line: Option<usize>,
    cwd: Option<PathBuf>,
    refresh_after_edit: bool,
}

fn resolve_editor_command(config: &config::EditorConfig) -> String {
    fn non_empty(value: Option<String>) -> Option<String> {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    non_empty(config.command.clone())
        .or_else(|| non_empty(std::env::var("VISUAL").ok()))
        .or_else(|| non_empty(std::env::var("EDITOR").ok()))
        .unwrap_or_else(|| "vi".to_string())
}

fn current_editor_target(app: &mut App, needs_line: bool) -> Result<Option<EditorTarget>> {
    let focus = if needs_line {
        current_editor_focus(app)
    } else {
        None
    };
    let side = focus.map(|focus| focus.side).unwrap_or(EditorSide::New);
    let line = focus.map(|focus| focus.line);

    let file_index = app.multi_diff.selected_index;
    let file = match app.multi_diff.current_file() {
        Some(file) => file.clone(),
        None => return Ok(None),
    };
    let display_path = match side {
        EditorSide::Old => file.old_path.clone().unwrap_or_else(|| file.path.clone()),
        EditorSide::New => file.path.clone(),
    };

    let file_side = match side {
        EditorSide::Old => FileSide::Old,
        EditorSide::New => FileSide::New,
    };
    if let Some(path) = app.multi_diff.existing_source_path(file_index, file_side) {
        return Ok(Some(EditorTarget {
            path,
            line,
            cwd: app.multi_diff.repo_root().map(Path::to_path_buf),
            refresh_after_edit: true,
        }));
    }

    let Some((old_content, new_content)) = app.multi_diff.file_contents(file_index) else {
        return Ok(None);
    };
    let content = match side {
        EditorSide::Old => old_content,
        EditorSide::New => new_content,
    };
    let path = write_editor_snapshot(&display_path, side, content)?;
    Ok(Some(EditorTarget {
        path,
        line,
        cwd: None,
        refresh_after_edit: false,
    }))
}

fn editor_needs_line(config: &config::EditorConfig) -> bool {
    if config.open_at_line {
        return true;
    }
    config
        .args
        .as_ref()
        .map(|args| args.iter().any(|arg| arg.contains("{line}")))
        .unwrap_or(false)
}

fn render_editor_template(template: &str, line: Option<usize>, path: &Path) -> String {
    let file = path.to_string_lossy();
    let line = line.unwrap_or(1).to_string();
    template.replace("{file}", &file).replace("{line}", &line)
}

fn render_editor_args(
    config: &config::EditorConfig,
    line: Option<usize>,
    path: &Path,
) -> Vec<String> {
    if let Some(args) = &config.args {
        return args
            .iter()
            .map(|arg| render_editor_template(arg, line, path))
            .collect();
    }

    let mut args = Vec::new();
    if config.open_at_line {
        if let Some(line) = line {
            args.push(format!("+{}", line));
        }
    }
    args.push(path.to_string_lossy().into_owned());
    args
}

fn snapshot_rel_path(path: &Path) -> PathBuf {
    let rel = path.components().filter_map(|component| match component {
        Component::Normal(part) => Some(part),
        _ => None,
    });
    let mut out = PathBuf::new();
    for part in rel {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        out.push("file");
    }
    out
}

fn write_editor_snapshot(display_path: &Path, side: EditorSide, content: &str) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let side_dir = match side {
        EditorSide::Old => "old",
        EditorSide::New => "new",
    };
    let mut path = std::env::temp_dir()
        .join("oy-editor")
        .join(format!("{}-{nanos}", std::process::id()))
        .join(side_dir);
    path.push(snapshot_rel_path(display_path));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(true);
        let _ = std::fs::set_permissions(&path, permissions);
    }
    Ok(path)
}

fn current_editor_focus(app: &mut App) -> Option<EditorFocus> {
    let frame = app.animation_frame();
    let view = app.current_view_with_frame(frame);
    if app.stepping {
        return primary_editor_focus(&view)
            .or_else(|| active_editor_focus(&view))
            .or_else(|| visible_editor_focus(app, &view));
    }

    let hunk_cursor = {
        let state = app.multi_diff.current_navigator().state();
        state.last_nav_was_hunk && state.cursor_change.is_some()
    };
    if hunk_cursor {
        primary_editor_focus(&view).or_else(|| visible_editor_focus(app, &view))
    } else {
        visible_editor_focus(app, &view).or_else(|| primary_editor_focus(&view))
    }
}

fn primary_editor_focus(view: &[ViewLine]) -> Option<EditorFocus> {
    view.iter()
        .find(|line| line.is_primary_active)
        .and_then(editor_focus_for_line)
}

fn active_editor_focus(view: &[ViewLine]) -> Option<EditorFocus> {
    view.iter()
        .find(|line| line.is_active)
        .and_then(editor_focus_for_line)
}

fn editor_focus_for_line(line: &ViewLine) -> Option<EditorFocus> {
    if let Some(line_number) = line.new_line.filter(|line| *line > 0) {
        return Some(EditorFocus {
            side: EditorSide::New,
            line: line_number,
        });
    }
    line.old_line
        .filter(|line| *line > 0)
        .map(|line_number| EditorFocus {
            side: EditorSide::Old,
            line: line_number,
        })
}

fn visible_editor_focus(app: &App, view: &[ViewLine]) -> Option<EditorFocus> {
    let target = app.render_scroll_offset();
    match app.view_mode {
        ViewMode::Split => visible_split_editor_focus(app, view, target),
        ViewMode::Evolution => view
            .iter()
            .filter(|line| !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete))
            .enumerate()
            .skip_while(|(idx, _)| *idx < target)
            .find_map(|(_, line)| editor_focus_for_line(line)),
        _ => view.iter().skip(target).find_map(editor_focus_for_line),
    }
}

fn visible_split_editor_focus(app: &App, view: &[ViewLine], target: usize) -> Option<EditorFocus> {
    let mut old_idx = 0usize;
    let mut new_idx = 0usize;
    let mut old_match = None;
    let mut new_match = None;

    for line in view {
        let fold_line = crate::app::is_fold_line(line);
        let old_present = line.old_line.is_some() || fold_line;
        let new_present = (line.new_line.is_some()
            && !matches!(line.kind, LineKind::Deleted | LineKind::PendingDelete))
            || fold_line;

        if old_present || (app.split_align_lines && new_present) {
            if old_idx >= target && old_match.is_none() {
                old_match = line
                    .old_line
                    .filter(|line| *line > 0)
                    .map(|line_number| EditorFocus {
                        side: EditorSide::Old,
                        line: line_number,
                    });
            }
            old_idx += 1;
        }
        if new_present || (app.split_align_lines && old_present) {
            if new_idx >= target && new_match.is_none() {
                new_match = line
                    .new_line
                    .filter(|line| *line > 0)
                    .map(|line_number| EditorFocus {
                        side: EditorSide::New,
                        line: line_number,
                    })
                    .or_else(|| editor_focus_for_line(line));
            }
            new_idx += 1;
        }
        if new_match.is_some() {
            break;
        }
    }

    new_match.or(old_match)
}

fn suspend_terminal_for_child(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn resume_terminal_after_child(terminal: &mut TuiTerminal) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.clear()?;
    Ok(())
}

fn run_editor_command(
    command: &str,
    args: &[String],
    cwd: Option<&Path>,
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(windows)]
    let mut child = {
        let mut parts = command.split_whitespace();
        let exe = parts.next().unwrap_or(command);
        let mut cmd = ProcessCommand::new(exe);
        cmd.args(parts);
        cmd
    };

    #[cfg(not(windows))]
    let mut child = {
        let mut cmd = ProcessCommand::new("sh");
        cmd.arg("-c")
            .arg(format!("exec {} \"$@\"", command))
            .arg("oy-editor");
        cmd
    };

    child.args(args);
    if let Some(cwd) = cwd {
        child.current_dir(cwd);
    }
    child.status()
}

fn open_current_file_in_editor(
    terminal: &mut TuiTerminal,
    app: &mut App,
    config: &config::EditorConfig,
) -> Result<()> {
    let Some(target) = current_editor_target(app, editor_needs_line(config))? else {
        return Ok(());
    };
    let command = resolve_editor_command(config);
    let args = render_editor_args(config, target.line, &target.path);

    suspend_terminal_for_child(terminal)?;
    let editor_result = run_editor_command(&command, &args, target.cwd.as_deref());
    let resume_result = resume_terminal_after_child(terminal);
    resume_result?;

    if editor_result.is_ok() && target.refresh_after_edit {
        app.refresh_current_file();
    }
    Ok(())
}

fn apply_config_to_app(app: &mut App, config: &config::Config, args: &Args, light_mode: bool) {
    let mut keybinding_warnings = Vec::new();
    app.keybindings =
        Keybindings::from_config_with_warnings(&config.keybindings, &mut keybinding_warnings);
    for warning in keybinding_warnings {
        eprintln!("Warning: {warning}");
    }

    app.zen_mode = config.ui.zen;
    app.animation_enabled = config.playback.animation;
    app.animation_duration = config.playback.animation_duration;
    app.file_panel_visible = config.files.panel_visible;
    app.file_panel_width = config.files.panel_width;
    app.file_count_mode = config.files.counts;
    app.auto_center = config.ui.auto_center;
    app.overscroll = config.ui.overscroll;
    app.topbar = config.ui.topbar;
    app.line_wrap = config.ui.line_wrap;
    app.set_fold_context_mode(config.ui.fold_context);
    app.scrollbar_visible = config.ui.scrollbar;
    app.strikethrough_deletions = config.ui.strikethrough_deletions;
    app.gutter_signs = config.ui.gutter_signs;
    app.diff_bg = config.ui.diff.bg;
    app.diff_fg = config.ui.diff.fg;
    app.diff_highlight = config.ui.diff.highlight;
    app.diff_defer = config.ui.diff.defer;
    app.diff_idle_ms = config.ui.diff.idle_ms;
    app.diff_extent_marker = config.ui.diff.extent_marker;
    app.diff_extent_marker_scope = config.ui.diff.extent_marker_scope;
    app.diff_extent_marker_context = config.ui.diff.extent_marker_context;
    app.blame_enabled = config.ui.blame.enabled;
    app.blame_mode = config.ui.blame.mode;
    app.blame_hunk_hint_enabled = config.ui.blame.hunk_hint;
    app.blame_hunk_hint_enabled = config.ui.blame.hunk_hint;
    app.syntax_mode = config.ui.syntax.mode;
    app.syntax_theme = config.ui.syntax.theme.clone();
    app.syntax_warmup_active_lines = config.ui.syntax.warmup.active_lines;
    app.syntax_warmup_pending_lines = config.ui.syntax.warmup.pending_lines;
    app.syntax_warmup_idle_lines = config.ui.syntax.warmup.idle_lines;
    app.syntax_warmup_debounce_ms = config.ui.syntax.warmup.debounce_ms;
    app.unified_modified_step_mode = config.ui.unified.modified_step_mode;
    app.split_align_lines = config.ui.split.align_lines;
    app.split_align_fill = config.ui.split.align_fill.clone();
    app.evo_syntax = config.ui.evo.syntax;
    app.auto_step_on_enter = config.playback.auto_step_on_enter;
    app.auto_step_blank_files = config.playback.auto_step_blank_files;
    app.no_step_auto_jump_on_enter = config.no_step.auto_jump_on_enter;
    app.review_mention_file_scope = config.comments.mentions.file_scope;
    app.review_mention_finder = config.comments.mentions.finder;
    app.hunk_wrap = config.navigation.wrap.hunk;
    app.step_wrap = config.navigation.wrap.step;
    app.primary_marker = config.ui.primary_marker.clone();
    app.primary_marker_right = config
        .ui
        .primary_marker_right
        .clone()
        .unwrap_or_else(|| "◀".to_string());
    app.extent_marker = config.ui.extent_marker.clone();
    app.extent_marker_right = config
        .ui
        .extent_marker_right
        .clone()
        .unwrap_or_else(|| "▐".to_string());
    app.theme = config.ui.theme.resolve(light_mode);
    app.time_format = TimeFormatter::new(&config.ui.time);
    app.theme_is_light = light_mode;

    if args.no_step {
        app.stepping = false;
    } else {
        app.stepping = config.ui.stepping;
    }
    if !app.stepping {
        app.enter_no_step_mode();
    }
    app.handle_file_enter();
}

fn emit_review_output(
    review_output: Option<String>,
    review_output_file: Option<&PathBuf>,
    print_review: bool,
) -> Result<()> {
    let output = review_output.unwrap_or_default();

    if let Some(path) = review_output_file {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "Failed to create review output directory: {}",
                        parent.display()
                    )
                })?;
            }
        }
        std::fs::write(path, &output)
            .with_context(|| format!("Failed to write review output file: {}", path.display()))?;
    }

    if print_review && !output.trim().is_empty() {
        println!("{output}");
    }

    Ok(())
}

fn build_diff_from_input_mode(
    input_mode: &InputMode,
    config: &config::Config,
    args: &Args,
) -> Result<Option<(MultiFileDiff, Option<String>)>> {
    let (multi_diff, git_branch) = match input_mode {
        InputMode::GitExternal {
            display_path,
            old_file,
            new_file,
        } => {
            let old_bytes = if old_file.to_string_lossy() == "/dev/null" {
                Vec::new()
            } else {
                std::fs::read(old_file)
                    .context(format!("Failed to read old file: {}", old_file.display()))?
            };

            let new_bytes = if new_file.to_string_lossy() == "/dev/null" {
                Vec::new()
            } else {
                std::fs::read(new_file)
                    .context(format!("Failed to read new file: {}", new_file.display()))?
            };

            let branch =
                oyo_core::git::get_current_branch(&std::env::current_dir().unwrap_or_default())
                    .ok();

            let cwd = std::env::current_dir().unwrap_or_default();
            let new_source = cwd.join(display_path);
            let diff = MultiFileDiff::from_file_pair_with_sources(
                display_path.clone(),
                old_bytes,
                new_bytes,
                None,
                Some(new_source),
            );
            (diff, branch)
        }
        InputMode::TwoPaths { old_path, new_path } => {
            let diff = if old_path.is_dir() && new_path.is_dir() {
                let scan_options = directory_scan_options(config, args, old_path, new_path);
                let mut diff =
                    MultiFileDiff::from_directories_with_options(old_path, new_path, &scan_options)
                        .context("Failed to create diff from directories")?;
                if looks_like_jj_external_diff_dirs(old_path, new_path) {
                    if let Some(root) = infer_external_diff_workspace_root() {
                        diff.set_source_roots(root.clone(), root);
                    } else {
                        diff.clear_source_roots();
                    }
                }
                diff
            } else {
                let old_bytes = if old_path.to_string_lossy() == "/dev/null" {
                    Vec::new()
                } else {
                    std::fs::read(old_path)
                        .context(format!("Failed to read: {}", old_path.display()))?
                };
                let new_bytes = if new_path.to_string_lossy() == "/dev/null" {
                    Vec::new()
                } else {
                    std::fs::read(new_path)
                        .context(format!("Failed to read: {}", new_path.display()))?
                };

                let old_source =
                    (old_path.to_string_lossy() != "/dev/null").then(|| old_path.clone());
                let new_source =
                    (new_path.to_string_lossy() != "/dev/null").then(|| new_path.clone());
                MultiFileDiff::from_file_pair_with_sources(
                    new_path.clone(),
                    old_bytes,
                    new_bytes,
                    old_source,
                    new_source,
                )
            };
            (diff, None)
        }
        InputMode::GitFile { path } => {
            let cwd = std::env::current_dir().unwrap_or_default();
            if !oyo_core::git::is_git_repo(&cwd) {
                anyhow::bail!(
                    "Not in a git repository.\n\
                     \n\
                     Usage: oy <file>\n\
                     \n\
                     Or use: oy <old_file> <new_file>"
                );
            }

            let repo_root =
                oyo_core::git::get_repo_root(&cwd).context("Failed to get git repository root")?;
            let abs_path = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            if abs_path.exists() && abs_path.is_dir() {
                anyhow::bail!("Expected a file path: {}", path.display());
            }

            let rel_path = abs_path.strip_prefix(&repo_root).with_context(|| {
                format!("Path is outside the git repository: {}", path.display())
            })?;

            let head_exists =
                oyo_core::git::get_file_at_commit_size(&repo_root, "HEAD", rel_path).is_some();
            let work_exists = abs_path.exists();
            if !head_exists && !work_exists {
                anyhow::bail!("File not found in HEAD or working tree: {}", path.display());
            }

            let old_bytes = if head_exists {
                oyo_core::git::get_head_content_bytes(&repo_root, rel_path)
                    .context("Failed to read file from HEAD")?
            } else {
                Vec::new()
            };
            let new_bytes = if work_exists {
                std::fs::read(&abs_path)
                    .context(format!("Failed to read: {}", abs_path.display()))?
            } else {
                Vec::new()
            };

            let diff = MultiFileDiff::from_file_pair_with_sources(
                rel_path.to_path_buf(),
                old_bytes,
                new_bytes,
                None,
                Some(abs_path),
            );
            let branch = oyo_core::git::get_current_branch(&repo_root).ok();
            (diff, branch)
        }
        InputMode::GitUncommitted => {
            let cwd = std::env::current_dir().unwrap_or_default();
            if !oyo_core::git::is_git_repo(&cwd) {
                anyhow::bail!(
                    "Not in a git repository.\n\
                     \n\
                     Usage: oy <old_file> <new_file>\n\
                     \n\
                     Or run from a git repository to diff uncommitted changes."
                );
            }

            let repo_root =
                oyo_core::git::get_repo_root(&cwd).context("Failed to get git repository root")?;
            let changes = oyo_core::git::get_uncommitted_changes(&repo_root)
                .context("Failed to get uncommitted changes")?;
            if changes.is_empty() {
                return Ok(None);
            }
            let branch = oyo_core::git::get_current_branch(&repo_root).ok();
            let diff = MultiFileDiff::from_git_changes(repo_root, changes)
                .context("Failed to create diff from git changes")?;
            (diff, branch)
        }
        InputMode::GitStaged => {
            let cwd = std::env::current_dir().unwrap_or_default();
            if !oyo_core::git::is_git_repo(&cwd) {
                anyhow::bail!(
                    "Not in a git repository.\n\
                     \n\
                     Usage: oy --staged\n\
                     \n\
                     Or run from a git repository."
                );
            }

            let repo_root =
                oyo_core::git::get_repo_root(&cwd).context("Failed to get git repository root")?;
            let changes = oyo_core::git::get_staged_changes(&repo_root)
                .context("Failed to get staged changes")?;
            if changes.is_empty() {
                return Ok(None);
            }
            let branch = oyo_core::git::get_current_branch(&repo_root).ok();
            let diff = MultiFileDiff::from_git_staged(repo_root, changes)
                .context("Failed to create diff from staged changes")?;
            (diff, branch)
        }
        InputMode::GitRange { from, to } => {
            let cwd = std::env::current_dir().unwrap_or_default();
            if !oyo_core::git::is_git_repo(&cwd) {
                anyhow::bail!(
                    "Not in a git repository.\n\
                     \n\
                     Usage: oy --range A..B\n\
                     \n\
                     Or run from a git repository."
                );
            }

            let repo_root =
                oyo_core::git::get_repo_root(&cwd).context("Failed to get git repository root")?;
            let is_index_from = from == INDEX_REF;
            let is_index_to = to == INDEX_REF;
            let (changes, diff) = if is_index_from || is_index_to {
                let (commit, to_index) = if is_index_to {
                    (from.clone(), true)
                } else {
                    (to.clone(), false)
                };
                let reverse = !to_index;
                let changes =
                    oyo_core::git::get_changes_between_index(&repo_root, &commit, reverse)
                        .context("Failed to get index range changes")?;
                if changes.is_empty() {
                    return Ok(None);
                }
                let diff = MultiFileDiff::from_git_index_range(
                    repo_root.clone(),
                    changes.clone(),
                    commit,
                    to_index,
                )
                .context("Failed to create diff from index range")?;
                (changes, diff)
            } else {
                let changes = oyo_core::git::get_changes_between(&repo_root, from, to)
                    .context("Failed to get range changes")?;
                if changes.is_empty() {
                    return Ok(None);
                }
                let diff = MultiFileDiff::from_git_range(
                    repo_root.clone(),
                    changes.clone(),
                    from.clone(),
                    to.clone(),
                )
                .context("Failed to create diff from range")?;
                (changes, diff)
            };
            if changes.is_empty() {
                return Ok(None);
            }
            let branch = oyo_core::git::get_current_branch(&repo_root).ok();
            (diff, branch)
        }
        InputMode::None => {
            anyhow::bail!(
                "Usage: oy <old_file> <new_file>\n\
                 Usage: oy <file>\n\
                 \n\
                 Or run from a git repository to diff uncommitted changes."
            );
        }
    };

    Ok(Some((multi_diff, git_branch)))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let view_limit = match args.command {
        Some(Command::Themes) => {
            for name in config::list_ui_themes() {
                println!("{name}");
            }
            return Ok(());
        }
        Some(Command::SyntaxThemes) => {
            for name in list_syntax_themes() {
                println!("{name}");
            }
            return Ok(());
        }
        Some(Command::View { limit }) => Some(limit),
        None => None,
    };
    let mut config = config::Config::load();
    if let Some(path) = args.dump_scopes.as_deref() {
        if let Some(name) = args.theme_name.as_deref() {
            config.ui.theme.name = Some(name.to_string());
        }
        if let Some(name) = args.syntax_theme.as_deref() {
            config.ui.syntax.theme = name.to_string();
        }
        let light_mode = match args.theme_mode {
            Some(CliThemeMode::Light) => true,
            Some(CliThemeMode::Dark) => false,
            None => config.ui.theme.is_light_mode(),
        };
        let content =
            std::fs::read_to_string(path).context(format!("Failed to read: {}", path.display()))?;
        let file_name = path.to_string_lossy();
        let engine = SyntaxEngine::new(&config.ui.syntax.theme, light_mode);
        println!("syntax: {}", engine.syntax_name_for_file(&file_name));
        let mut entries: Vec<(String, usize)> = engine
            .collect_scopes(&content, &file_name)
            .into_iter()
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (scope, count) in entries {
            println!("{count}\t{scope}");
        }
        return Ok(());
    }

    if let Some(name) = args.theme_name.as_deref() {
        config.ui.theme.name = Some(name.to_string());
    }
    if let Some(name) = args.syntax_theme.as_deref() {
        config.ui.syntax.theme = name.to_string();
    }
    if config.ui.syntax.theme.trim().is_empty() {
        if let Some(name) = config.ui.theme.name.clone() {
            config.ui.syntax.theme = name;
        } else {
            config.ui.syntax.theme = "ansi".to_string();
        }
    }
    MultiFileDiff::set_diff_max_bytes(config.ui.diff.max_bytes);
    MultiFileDiff::set_full_context_max_bytes(config.ui.diff.full_context_max_bytes);
    MultiFileDiff::set_diff_defer(config.ui.diff.defer);

    // Compute theme mode: CLI overrides config, default to dark
    let light_mode = match args.theme_mode {
        Some(CliThemeMode::Light) => true,
        Some(CliThemeMode::Dark) => false,
        None => config.ui.theme.is_light_mode(),
    };

    if let Some(limit) = view_limit {
        let mut terminal = setup_terminal()?;
        let mut input_mode = match run_commit_picker(&mut terminal, &config, light_mode, limit)? {
            Some(mode) => mode,
            None => {
                disable_raw_mode()?;
                execute!(
                    terminal.backend_mut(),
                    LeaveAlternateScreen,
                    DisableMouseCapture
                )?;
                terminal.show_cursor()?;
                return Ok(());
            }
        };

        let mut exit_message: Option<String> = None;
        let mut review_output: Option<String> = None;
        loop {
            let empty_message = match &input_mode {
                InputMode::GitUncommitted => Some("No uncommitted changes found.".to_string()),
                InputMode::GitStaged => Some("No staged changes found.".to_string()),
                InputMode::GitRange { from, to } => {
                    Some(format!("No changes in range {}..{}.", from, to))
                }
                _ => Some("No changes found.".to_string()),
            };
            let (multi_diff, git_branch) =
                match build_diff_from_input_mode(&input_mode, &config, &args)? {
                    Some(result) => result,
                    None => {
                        exit_message = empty_message;
                        break;
                    }
                };

            if multi_diff.file_count() == 0 {
                exit_message = Some("No changes found.".to_string());
                break;
            }

            let view_mode: ViewMode = args.view.into();
            let view_mode = config.parse_view_mode().unwrap_or(view_mode);
            let speed = if args.speed != 200 {
                args.speed
            } else {
                config.playback.speed
            };
            let autoplay = args.autoplay || config.playback.autoplay;

            let mut app = App::new(multi_diff, view_mode, speed, autoplay, git_branch);
            apply_config_to_app(&mut app, &config, &args, light_mode);
            app.set_review_persist_enabled(!args.no_review_persist);
            app.set_review_clear_session_on_start(args.clear_review_session);
            app.enable_review_mode();

            let exit = run_app(&mut terminal, &mut app, &config.editor)?;
            if review_output.is_none() {
                review_output = app.take_review_submission_output();
            }
            match exit {
                AppExit::Quit => break,
                AppExit::OpenDashboard => {
                    let Some(mode) = run_commit_picker(&mut terminal, &config, light_mode, limit)?
                    else {
                        break;
                    };
                    input_mode = mode;
                }
            }
        }

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;
        emit_review_output(
            review_output,
            args.review_output_file.as_ref(),
            !args.no_print_review,
        )?;
        if let Some(message) = exit_message {
            println!("{message}");
        }
        return Ok(());
    }

    let mut input_mode = if args.paths.len() == 7 {
        detect_input_mode(&args.paths)
    } else if args.staged || args.range.is_some() {
        if !args.paths.is_empty() {
            anyhow::bail!("--staged/--range cannot be used with file paths");
        }
        if args.staged && args.range.is_some() {
            anyhow::bail!("--staged and --range are mutually exclusive");
        }
        if let Some(range) = args.range.as_deref() {
            let (from, to) = parse_range(range)?;
            InputMode::GitRange { from, to }
        } else {
            InputMode::GitStaged
        }
    } else {
        detect_input_mode(&args.paths)
    };

    let empty_message = match &input_mode {
        InputMode::GitUncommitted => Some("No uncommitted changes found.".to_string()),
        InputMode::GitStaged => Some("No staged changes found.".to_string()),
        InputMode::GitRange { from, to } => Some(format!("No changes in range {}..{}.", from, to)),
        _ => Some("No changes found.".to_string()),
    };
    let prefetched = match build_diff_from_input_mode(&input_mode, &config, &args)? {
        Some(result) => result,
        None => {
            if let Some(message) = empty_message {
                println!("{message}");
            }
            return Ok(());
        }
    };
    if prefetched.0.file_count() == 0 {
        println!("No changes found.");
        return Ok(());
    }

    let mut terminal = setup_terminal()?;
    let dashboard_limit = view_limit.unwrap_or(200);

    let mut exit_message: Option<String> = None;
    let mut review_output: Option<String> = None;
    let mut pending_diff = Some(prefetched);
    loop {
        let empty_message = match &input_mode {
            InputMode::GitUncommitted => Some("No uncommitted changes found.".to_string()),
            InputMode::GitStaged => Some("No staged changes found.".to_string()),
            InputMode::GitRange { from, to } => {
                Some(format!("No changes in range {}..{}.", from, to))
            }
            _ => Some("No changes found.".to_string()),
        };
        let (multi_diff, git_branch) = if let Some(result) = pending_diff.take() {
            result
        } else {
            match build_diff_from_input_mode(&input_mode, &config, &args)? {
                Some(result) => result,
                None => {
                    exit_message = empty_message;
                    break;
                }
            }
        };

        if multi_diff.file_count() == 0 {
            exit_message = Some("No changes found.".to_string());
            break;
        }

        let view_mode: ViewMode = args.view.into();
        let view_mode = config.parse_view_mode().unwrap_or(view_mode);
        let speed = if args.speed != 200 {
            args.speed
        } else {
            config.playback.speed
        };
        let autoplay = args.autoplay || config.playback.autoplay;

        let mut app = App::new(multi_diff, view_mode, speed, autoplay, git_branch);
        apply_config_to_app(&mut app, &config, &args, light_mode);
        app.set_review_persist_enabled(!args.no_review_persist);
        app.set_review_clear_session_on_start(args.clear_review_session);
        app.enable_review_mode();

        let exit = run_app(&mut terminal, &mut app, &config.editor)?;
        if review_output.is_none() {
            review_output = app.take_review_submission_output();
        }
        match exit {
            AppExit::Quit => break,
            AppExit::OpenDashboard => {
                let Some(mode) =
                    run_commit_picker(&mut terminal, &config, light_mode, dashboard_limit)?
                else {
                    break;
                };
                input_mode = mode;
                pending_diff = None;
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    emit_review_output(
        review_output,
        args.review_output_file.as_ref(),
        !args.no_print_review,
    )?;
    if let Some(message) = exit_message {
        println!("{message}");
    }

    Ok(())
}

fn run_app(
    terminal: &mut TuiTerminal,
    app: &mut App,
    editor_config: &config::EditorConfig,
) -> Result<AppExit> {
    let mut pending_event: Option<Event> = None;
    let mut needs_draw = true;

    loop {
        if needs_draw {
            terminal
                .draw(|f| ui::draw(f, app))
                .map_err(|e| anyhow!("{e}"))?;
            needs_draw = false;

            // Clear active change after render (one-frame extent marker display when animation disabled)
            if app.clear_active_on_next_render {
                app.multi_diff.current_navigator().clear_active_change();
                app.clear_active_on_next_render = false;
                needs_draw = true;
            }
            if needs_draw {
                continue;
            }
        }

        let event = if let Some(event) = pending_event.take() {
            Some(event)
        } else if event::poll(app.redraw_interval())? {
            Some(event::read()?)
        } else {
            None
        };

        if let Some(event) = event {
            app.mark_user_input();
            needs_draw = true;
            match event {
                Event::Mouse(me) => {
                    if app.show_help || app.show_path_popup {
                        continue;
                    }
                    app.reset_count();
                    if app.command_palette_active() {
                        match me.kind {
                            MouseEventKind::ScrollUp => {
                                app.move_command_palette_selection(-1);
                            }
                            MouseEventKind::ScrollDown => {
                                app.move_command_palette_selection(1);
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                app.handle_command_palette_click(me.column, me.row);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.file_search_active() {
                        match me.kind {
                            MouseEventKind::ScrollUp => {
                                app.move_file_search_selection(-1);
                            }
                            MouseEventKind::ScrollDown => {
                                app.move_file_search_selection(1);
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                app.handle_file_search_click(me.column, me.row);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    match me.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if app.start_file_panel_resize(me.column, me.row) {
                                continue;
                            }
                            if app.handle_review_preview_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_file_list_click(me.column, me.row) {
                                continue;
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if let Ok((cols, _)) = crossterm::terminal::size() {
                                if app.drag_file_panel_resize(me.column, cols) {
                                    continue;
                                }
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            app.end_file_panel_resize();
                        }
                        MouseEventKind::ScrollUp => {
                            if app.mouse_over_file_panel(me.column, me.row) {
                                app.prev_file();
                            } else if app.stepping && app.current_file_diff_ready() {
                                app.prev_step();
                            } else {
                                app.scroll_up();
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            if app.mouse_over_file_panel(me.column, me.row) {
                                app.next_file();
                            } else if app.stepping && app.current_file_diff_ready() {
                                app.next_step();
                            } else {
                                app.scroll_down();
                            }
                        }
                        _ => {}
                    }
                }
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    handle_app_key(app, key, &mut pending_event, terminal, editor_config)?;
                }
                _ => {}
            }
        }

        if app.tick() {
            needs_draw = true;
        }

        if app.open_dashboard {
            app.open_dashboard = false;
            return Ok(AppExit::OpenDashboard);
        }
        if app.should_quit {
            return Ok(AppExit::Quit);
        }
    }
}

fn coalesce_key_repeats(
    first: KeyEvent,
    pending_event: &mut Option<Event>,
) -> std::io::Result<usize> {
    let mut count = 1usize;
    let same_key = |next: &KeyEvent| next.code == first.code && next.modifiers == first.modifiers;
    while event::poll(Duration::from_millis(0))? {
        let next = event::read()?;
        match next {
            Event::Key(key)
                if same_key(&key)
                    && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                count += 1;
            }
            _ => {
                *pending_event = Some(next);
                break;
            }
        }
    }
    Ok(count)
}

fn run_dashboard<B: Backend>(
    terminal: &mut Terminal<B>,
    dashboard: &mut Dashboard,
) -> Result<Option<DashboardSelection>> {
    let tick_rate = Duration::from_millis(250);
    let mut needs_draw = true;

    loop {
        if needs_draw {
            terminal
                .draw(|f| dashboard.draw(f))
                .map_err(|e| anyhow!("{e}"))?;
            needs_draw = false;
        }

        if event::poll(tick_rate)? {
            needs_draw = true;
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let list_height =
                        dashboard.list_height(terminal.size().map_err(|e| anyhow!("{e}"))?.height);
                    if dashboard.filter_active() {
                        match dashboard.keybindings_mut().dashboard_filter(key) {
                            Dispatch::Matched(DashboardFilterAction::Cancel) => {
                                dashboard.stop_filter();
                            }
                            Dispatch::Matched(DashboardFilterAction::Accept) => {
                                if let Some(selection) = dashboard.selection() {
                                    return Ok(Some(selection));
                                }
                            }
                            Dispatch::Matched(DashboardFilterAction::Clear) => {
                                dashboard.clear_filter();
                            }
                            Dispatch::Matched(DashboardFilterAction::Backspace) => {
                                dashboard.pop_filter_char();
                            }
                            Dispatch::Matched(DashboardFilterAction::SelectNext) => {
                                dashboard.move_selection(1, list_height);
                            }
                            Dispatch::Matched(DashboardFilterAction::SelectPrev) => {
                                dashboard.move_selection(-1, list_height);
                            }
                            Dispatch::Matched(DashboardFilterAction::PageDown) => {
                                dashboard.page_down(list_height);
                            }
                            Dispatch::Matched(DashboardFilterAction::PageUp) => {
                                dashboard.page_up(list_height);
                            }
                            Dispatch::Matched(DashboardFilterAction::SelectFirst) => {
                                dashboard.select_first(list_height);
                            }
                            Dispatch::Matched(DashboardFilterAction::SelectLast) => {
                                dashboard.select_last(list_height);
                            }
                            Dispatch::Pending => {}
                            Dispatch::Unmatched => {
                                if let Some(ch) = printable_dashboard_char(key) {
                                    dashboard.push_filter_char(ch);
                                }
                            }
                        }
                        continue;
                    }
                    match dashboard.keybindings_mut().dashboard(key) {
                        Dispatch::Matched(DashboardAction::Quit) => return Ok(None),
                        Dispatch::Matched(DashboardAction::StartFilter) => {
                            dashboard.start_filter();
                        }
                        Dispatch::Matched(DashboardAction::ClearPin) => {
                            dashboard.clear_pin();
                        }
                        Dispatch::Matched(DashboardAction::TogglePin) => {
                            dashboard.toggle_pin();
                        }
                        Dispatch::Matched(DashboardAction::Accept) => {
                            if let Some(selection) = dashboard.selection() {
                                return Ok(Some(selection));
                            }
                        }
                        Dispatch::Matched(DashboardAction::SelectNext) => {
                            dashboard.move_selection(1, list_height);
                        }
                        Dispatch::Matched(DashboardAction::SelectPrev) => {
                            dashboard.move_selection(-1, list_height);
                        }
                        Dispatch::Matched(DashboardAction::PageDown) => {
                            dashboard.page_down(list_height);
                        }
                        Dispatch::Matched(DashboardAction::PageUp) => {
                            dashboard.page_up(list_height);
                        }
                        Dispatch::Matched(DashboardAction::SelectFirst) => {
                            dashboard.select_first(list_height);
                        }
                        Dispatch::Matched(DashboardAction::SelectLast) => {
                            dashboard.select_last(list_height);
                        }
                        Dispatch::Pending | Dispatch::Unmatched => {}
                    }
                }
                Event::Mouse(mouse) => {
                    let list_height =
                        dashboard.list_height(terminal.size().map_err(|e| anyhow!("{e}"))?.height);
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            dashboard.move_selection(-3, list_height);
                        }
                        MouseEventKind::ScrollDown => {
                            dashboard.move_selection(3, list_height);
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            let changed = dashboard.select_at_mouse(mouse.row);
                            if !changed {
                                if let Some(selection) = dashboard.selection() {
                                    return Ok(Some(selection));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

fn printable_dashboard_char(key: KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(ch)
        }
        _ => None,
    }
}

fn run_commit_picker<B: Backend>(
    terminal: &mut Terminal<B>,
    config: &config::Config,
    light_mode: bool,
    limit: usize,
) -> Result<Option<InputMode>> {
    let cwd = std::env::current_dir().unwrap_or_default();
    if !oyo_core::git::is_git_repo(&cwd) {
        anyhow::bail!("Not in a git repository.");
    }

    let repo_root =
        oyo_core::git::get_repo_root(&cwd).context("Failed to get git repository root")?;
    let branch = oyo_core::git::get_current_branch(&repo_root).ok();
    let commits =
        oyo_core::git::get_recent_commits(&repo_root, limit).context("Failed to get commits")?;
    let working_changes = oyo_core::git::get_uncommitted_changes(&repo_root)
        .context("Failed to get uncommitted changes")?;
    let staged_changes =
        oyo_core::git::get_staged_changes(&repo_root).context("Failed to get staged changes")?;

    let theme = config.ui.theme.resolve(light_mode);
    let time_format = TimeFormatter::new(&config.ui.time);
    let mut dashboard = Dashboard::new(DashboardConfig {
        repo_root,
        branch,
        commits,
        working_files: working_changes.len(),
        staged_files: staged_changes.len(),
        theme,
        primary_marker: config.ui.primary_marker.clone(),
        extent_marker: config.ui.extent_marker.clone(),
        time_format,
        keybindings: Keybindings::from_config(&config.keybindings),
    });

    let selection = run_dashboard(terminal, &mut dashboard)?;
    let input_mode = match selection {
        None => return Ok(None),
        Some(DashboardSelection::Uncommitted) => InputMode::GitUncommitted,
        Some(DashboardSelection::Staged) => InputMode::GitStaged,
        Some(DashboardSelection::Range { from, to }) => InputMode::GitRange { from, to },
    };

    Ok(Some(input_mode))
}

#[cfg(test)]
mod tests {
    use super::{config, detect_input_mode, parse_range, render_editor_args, InputMode};
    use std::path::{Path, PathBuf};

    #[test]
    fn parse_range_accepts_double_dot() {
        let (from, to) = parse_range("HEAD~1..HEAD").unwrap();
        assert_eq!(from, "HEAD~1");
        assert_eq!(to, "HEAD");
    }

    #[test]
    fn parse_range_accepts_triple_dot() {
        let (from, to) = parse_range("main...feature").unwrap();
        assert_eq!(from, "main");
        assert_eq!(to, "feature");
    }

    #[test]
    fn parse_range_rejects_empty_bounds() {
        assert!(parse_range("..HEAD").is_err());
        assert!(parse_range("HEAD..").is_err());
        assert!(parse_range("...HEAD").is_err());
        assert!(parse_range("HEAD...").is_err());
    }

    #[test]
    fn parse_range_rejects_extra_separators() {
        assert!(parse_range("A..B..C").is_err());
        assert!(parse_range("A...B..C").is_err());
    }

    #[test]
    fn parse_range_rejects_missing_separator() {
        assert!(parse_range("HEAD").is_err());
    }

    #[test]
    fn detect_input_mode_single_path() {
        let paths = vec![PathBuf::from("main.rs")];
        match detect_input_mode(&paths) {
            InputMode::GitFile { path } => assert_eq!(path, PathBuf::from("main.rs")),
            _ => panic!("unexpected input mode"),
        }
    }

    #[test]
    fn editor_default_args_open_at_line() {
        let config = config::EditorConfig::default();
        let args = render_editor_args(&config, Some(42), Path::new("src/main.rs"));
        assert_eq!(args, vec!["+42", "src/main.rs"]);
    }

    #[test]
    fn editor_template_args_replace_file_and_line() {
        let config = config::EditorConfig {
            command: Some("code".to_string()),
            args: Some(vec!["--goto".to_string(), "{file}:{line}".to_string()]),
            open_at_line: true,
        };
        let args = render_editor_args(&config, Some(42), Path::new("src/main.rs"));
        assert_eq!(args, vec!["--goto", "src/main.rs:42"]);
    }
}
