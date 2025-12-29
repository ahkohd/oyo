//! Oyo CLI - Step-through diff viewer TUI

mod app;
mod color;
mod config;
mod dashboard;
mod syntax;
mod ui;
mod views;

use crate::dashboard::{Dashboard, DashboardConfig, DashboardSelection};
use crate::syntax::{list_syntax_themes, SyntaxEngine};
use anyhow::{Context, Result};
use app::{App, ViewMode};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use oyo_core::MultiFileDiff;
use ratatui::prelude::*;
use std::fs::OpenOptions;
use std::io::{self, IsTerminal};
#[cfg(unix)]
use std::path::PathBuf;
use std::time::Duration;

const INDEX_REF: &str = "INDEX";

#[derive(Parser, Debug)]
#[command(name = "oy")]
#[command(author, version, about = "A step-through diff viewer")]
#[command(args_conflicts_with_subcommands = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Files or directories to compare: old_file new_file
    /// Also works as a git external diff tool (git config diff.external oy)
    #[arg(num_args = 0..)]
    paths: Vec<PathBuf>,

    /// View mode: single, split, or evolution
    #[arg(short, long, default_value = "single")]
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
    /// Single pane that morphs from old to new state
    Single,
    /// Split view with synchronized stepping
    #[value(alias = "sbs")]
    Split,
    /// Evolution view - shows file morphing, deletions just disappear
    #[value(alias = "evo")]
    Evolution,
}

impl From<CliViewMode> for ViewMode {
    fn from(mode: CliViewMode) -> Self {
        match mode {
            CliViewMode::Single => ViewMode::SinglePane,
            CliViewMode::Split => ViewMode::Split,
            CliViewMode::Evolution => ViewMode::Evolution,
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
    /// No args - try git uncommitted changes in current directory
    GitUncommitted,
    /// Staged changes (index vs HEAD)
    GitStaged,
    /// Git range
    GitRange { from: String, to: String },
    /// No valid input
    None,
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

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Box<dyn io::Write>>>> {
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

fn apply_config_to_app(app: &mut App, config: &config::Config, args: &Args, light_mode: bool) {
    app.zen_mode = config.ui.zen;
    app.animation_enabled = config.playback.animation;
    app.animation_duration = config.playback.animation_duration;
    app.file_panel_visible = config.files.panel_visible;
    app.file_count_mode = config.files.counts;
    app.auto_center = config.ui.auto_center;
    app.topbar = config.ui.topbar;
    app.line_wrap = config.ui.line_wrap;
    app.scrollbar_visible = config.ui.scrollbar;
    app.strikethrough_deletions = config.ui.strikethrough_deletions;
    app.gutter_signs = config.ui.gutter_signs;
    app.diff_bg = config.ui.diff.bg;
    app.diff_fg = config.ui.diff.fg;
    app.diff_highlight = config.ui.diff.highlight;
    app.diff_extent_marker = config.ui.diff.extent_marker;
    app.diff_extent_marker_scope = config.ui.diff.extent_marker_scope;
    app.syntax_mode = config.ui.syntax.mode;
    app.syntax_theme = config.ui.syntax.theme.clone();
    app.single_modified_step_mode = config.ui.single.modified_step_mode;
    app.split_align_lines = config.ui.split.align_lines;
    app.split_align_fill = config.ui.split.align_fill.clone();
    app.evo_syntax = config.ui.evo.syntax;
    app.auto_step_on_enter = config.playback.auto_step_on_enter;
    app.auto_step_blank_files = config.playback.auto_step_blank_files;
    app.no_step_auto_jump_on_enter = config.no_step.auto_jump_on_enter;
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

fn build_diff_from_input_mode(
    input_mode: InputMode,
) -> Result<Option<(MultiFileDiff, Option<String>)>> {
    let (multi_diff, git_branch) = match input_mode {
        InputMode::GitExternal {
            display_path,
            old_file,
            new_file,
        } => {
            let old_content = if old_file.to_string_lossy() == "/dev/null" {
                String::new()
            } else {
                std::fs::read_to_string(&old_file)
                    .context(format!("Failed to read old file: {}", old_file.display()))?
            };

            let new_content = if new_file.to_string_lossy() == "/dev/null" {
                String::new()
            } else {
                std::fs::read_to_string(&new_file)
                    .context(format!("Failed to read new file: {}", new_file.display()))?
            };

            let branch =
                oyo_core::git::get_current_branch(&std::env::current_dir().unwrap_or_default())
                    .ok();

            let diff = MultiFileDiff::from_file_pair(
                display_path.clone(),
                display_path,
                old_content,
                new_content,
            );
            (diff, branch)
        }
        InputMode::TwoPaths { old_path, new_path } => {
            let diff = if old_path.is_dir() && new_path.is_dir() {
                MultiFileDiff::from_directories(&old_path, &new_path)
                    .context("Failed to create diff from directories")?
            } else {
                let old_content = std::fs::read_to_string(&old_path)
                    .context(format!("Failed to read: {}", old_path.display()))?;
                let new_content = std::fs::read_to_string(&new_path)
                    .context(format!("Failed to read: {}", new_path.display()))?;

                MultiFileDiff::from_file_pair(old_path, new_path, old_content, new_content)
            };
            (diff, None)
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
                let changes = oyo_core::git::get_changes_between(&repo_root, &from, &to)
                    .context("Failed to get range changes")?;
                if changes.is_empty() {
                    return Ok(None);
                }
                let diff =
                    MultiFileDiff::from_git_range(repo_root.clone(), changes.clone(), from, to)
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
                 \n\
                 Or run from a git repository to diff uncommitted changes."
            );
        }
    };

    Ok(Some((multi_diff, git_branch)))
}

#[cfg(test)]
mod tests {
    use super::parse_range;

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

    // Compute theme mode: CLI overrides config, default to dark
    let light_mode = match args.theme_mode {
        Some(CliThemeMode::Light) => true,
        Some(CliThemeMode::Dark) => false,
        None => config.ui.theme.is_light_mode(),
    };

    if let Some(limit) = view_limit {
        let cwd = std::env::current_dir().unwrap_or_default();
        if !oyo_core::git::is_git_repo(&cwd) {
            anyhow::bail!("Not in a git repository.");
        }

        let repo_root =
            oyo_core::git::get_repo_root(&cwd).context("Failed to get git repository root")?;
        let branch = oyo_core::git::get_current_branch(&repo_root).ok();
        let commits = oyo_core::git::get_recent_commits(&repo_root, limit)
            .context("Failed to get commits")?;
        let working_changes = oyo_core::git::get_uncommitted_changes(&repo_root)
            .context("Failed to get uncommitted changes")?;
        let staged_changes = oyo_core::git::get_staged_changes(&repo_root)
            .context("Failed to get staged changes")?;

        let theme = config.ui.theme.resolve(light_mode);
        let mut dashboard = Dashboard::new(DashboardConfig {
            repo_root: repo_root.clone(),
            branch: branch.clone(),
            commits,
            working_files: working_changes.len(),
            staged_files: staged_changes.len(),
            theme,
            primary_marker: config.ui.primary_marker.clone(),
            extent_marker: config.ui.extent_marker.clone(),
        });

        let mut terminal = setup_terminal()?;
        let selection = run_dashboard(&mut terminal, &mut dashboard)?;
        if selection.is_none() {
            disable_raw_mode()?;
            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            )?;
            terminal.show_cursor()?;
            return Ok(());
        }

        let input_mode = match selection.unwrap() {
            DashboardSelection::Uncommitted => InputMode::GitUncommitted,
            DashboardSelection::Staged => InputMode::GitStaged,
            DashboardSelection::Range { from, to } => InputMode::GitRange { from, to },
        };

        let empty_message = match &input_mode {
            InputMode::GitUncommitted => Some("No uncommitted changes found.".to_string()),
            InputMode::GitStaged => Some("No staged changes found.".to_string()),
            InputMode::GitRange { from, to } => {
                Some(format!("No changes in range {}..{}.", from, to))
            }
            _ => Some("No changes found.".to_string()),
        };
        let (multi_diff, git_branch) = match build_diff_from_input_mode(input_mode)? {
            Some(result) => result,
            None => {
                disable_raw_mode()?;
                execute!(
                    terminal.backend_mut(),
                    LeaveAlternateScreen,
                    DisableMouseCapture
                )?;
                terminal.show_cursor()?;
                if let Some(message) = empty_message {
                    println!("{message}");
                }
                return Ok(());
            }
        };

        if multi_diff.file_count() == 0 {
            disable_raw_mode()?;
            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            )?;
            terminal.show_cursor()?;
            println!("No changes found.");
            return Ok(());
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

        let result = run_app(&mut terminal, &mut app);

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        if let Err(err) = result {
            eprintln!("Error: {}", err);
            return Err(err);
        }
        return Ok(());
    }

    let input_mode = if args.paths.len() == 7 {
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
    let (multi_diff, git_branch) = match build_diff_from_input_mode(input_mode)? {
        Some(result) => result,
        None => {
            if let Some(message) = empty_message {
                println!("{message}");
            }
            return Ok(());
        }
    };

    if multi_diff.file_count() == 0 {
        println!("No changes found.");
        return Ok(());
    }

    // Setup terminal
    let mut terminal = setup_terminal()?;

    // Determine view mode (CLI overrides config)
    let view_mode: ViewMode = args.view.into();
    let view_mode = config.parse_view_mode().unwrap_or(view_mode);

    // Determine speed (CLI default is 200, config can override)
    let speed = if args.speed != 200 {
        args.speed
    } else {
        config.playback.speed
    };

    // Autoplay: CLI flag or config
    let autoplay = args.autoplay || config.playback.autoplay;

    // Create app
    let mut app = App::new(multi_diff, view_mode, speed, autoplay, git_branch);

    // Apply additional config settings
    app.zen_mode = config.ui.zen;
    app.animation_enabled = config.playback.animation;
    app.animation_duration = config.playback.animation_duration;
    app.file_panel_visible = config.files.panel_visible;
    app.file_count_mode = config.files.counts;
    app.auto_center = config.ui.auto_center;
    app.topbar = config.ui.topbar;
    app.line_wrap = config.ui.line_wrap;
    app.scrollbar_visible = config.ui.scrollbar;
    app.strikethrough_deletions = config.ui.strikethrough_deletions;
    app.gutter_signs = config.ui.gutter_signs;
    app.diff_bg = config.ui.diff.bg;
    app.diff_fg = config.ui.diff.fg;
    app.diff_highlight = config.ui.diff.highlight;
    app.diff_extent_marker = config.ui.diff.extent_marker;
    app.diff_extent_marker_scope = config.ui.diff.extent_marker_scope;
    app.syntax_mode = config.ui.syntax.mode;
    app.syntax_theme = config.ui.syntax.theme.clone();
    app.single_modified_step_mode = config.ui.single.modified_step_mode;
    app.split_align_lines = config.ui.split.align_lines;
    app.split_align_fill = config.ui.split.align_fill.clone();
    app.evo_syntax = config.ui.evo.syntax;
    app.auto_step_on_enter = config.playback.auto_step_on_enter;
    app.auto_step_blank_files = config.playback.auto_step_blank_files;
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

    apply_config_to_app(&mut app, &config, &args, light_mode);

    // Run event loop
    let result = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = result {
        eprintln!("Error: {}", err);
        return Err(err);
    }

    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let tick_rate = Duration::from_millis(16);

    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // Clear active change after render (one-frame extent marker display when animation disabled)
        if app.clear_active_on_next_render {
            app.multi_diff.current_navigator().clear_active_change();
            app.clear_active_on_next_render = false;
        }

        if event::poll(tick_rate)? {
            match event::read()? {
                Event::Mouse(me) => {
                    if app.show_help || app.show_path_popup {
                        continue;
                    }
                    app.reset_count();
                    match me.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if app.handle_file_list_click(me.column, me.row) {
                                continue;
                            }
                        }
                        MouseEventKind::ScrollUp => {
                            if app.file_list_focused {
                                app.prev_file();
                            } else if app.stepping {
                                app.prev_step();
                            } else {
                                app.scroll_up();
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            if app.file_list_focused {
                                app.next_file();
                            } else if app.stepping {
                                app.next_step();
                            } else {
                                app.scroll_down();
                            }
                        }
                        _ => {}
                    }
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.show_help {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                                app.toggle_help();
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                app.help_scroll_down();
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.help_scroll_up();
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.file_filter_active {
                        match key.code {
                            KeyCode::Esc | KeyCode::Enter => {
                                app.stop_file_filter();
                            }
                            KeyCode::Backspace => {
                                app.pop_file_filter_char();
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.clear_file_filter();
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                app.push_file_filter_char(c);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.goto_active() {
                        match key.code {
                            KeyCode::Esc => {
                                app.clear_goto();
                            }
                            KeyCode::Enter => {
                                app.apply_goto();
                                app.clear_goto();
                            }
                            KeyCode::Backspace => {
                                if app.goto_query().is_empty() {
                                    app.clear_goto();
                                } else {
                                    app.pop_goto_char();
                                }
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.clear_goto_text();
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                app.push_goto_char(c);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.search_active() {
                        match key.code {
                            KeyCode::Esc => {
                                app.clear_search();
                            }
                            KeyCode::Enter => {
                                app.stop_search();
                                app.search_next();
                            }
                            KeyCode::Backspace => {
                                if app.search_query().is_empty() {
                                    app.clear_search();
                                } else {
                                    app.pop_search_char();
                                }
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.clear_search_text();
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                app.push_search_char(c);
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.pending_g_prefix {
                        let is_plain_g = matches!(key.code, KeyCode::Char('g'))
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::ALT);
                        if is_plain_g {
                            app.pending_g_prefix = false;
                            app.reset_count();
                            app.goto_start();
                            continue;
                        }
                        app.pending_g_prefix = false;
                    }
                    if matches!(key.code, KeyCode::Esc)
                        && !app.show_help
                        && !app.show_path_popup
                        && (app.search_active()
                            || !app.search_query().is_empty()
                            || app.goto_active()
                            || !app.goto_query().is_empty())
                    {
                        app.reset_count();
                        app.clear_search();
                        app.clear_goto();
                        continue;
                    }

                    match key.code {
                        // Digit keys for vim-style counts (e.g., 10j, 5l)
                        KeyCode::Char(c @ '0'..='9') => {
                            // Don't treat '0' as count if no pending count (it's a command)
                            if c == '0' && app.pending_count.is_none() {
                                // '0' without pending count = go to start of line (like vim)
                                app.scroll_to_line_start();
                            } else {
                                app.push_count_digit(c as u8 - b'0');
                            }
                        }
                        // $ = go to end of line (horizontal scroll to end, like vim)
                        KeyCode::Char('$') => {
                            app.reset_count();
                            app.scroll_to_line_end();
                        }
                        KeyCode::Char('q') | KeyCode::Esc => {
                            app.reset_count();
                            if app.show_help {
                                app.show_help = false;
                            } else if app.show_path_popup {
                                app.show_path_popup = false;
                            } else {
                                return Ok(());
                            }
                        }
                        // Step navigation (supports count)
                        KeyCode::Down | KeyCode::Char('j') => {
                            let count = app.take_count();
                            for _ in 0..count {
                                if app.file_list_focused {
                                    app.next_file();
                                } else if app.stepping {
                                    app.next_step();
                                } else {
                                    app.scroll_down();
                                }
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            let count = app.take_count();
                            for _ in 0..count {
                                if app.file_list_focused {
                                    app.prev_file();
                                } else if app.stepping {
                                    app.prev_step();
                                } else {
                                    app.scroll_up();
                                }
                            }
                        }
                        // Hunk navigation (h/l and arrow keys, supports count)
                        KeyCode::Right | KeyCode::Char('l') => {
                            if app.stepping {
                                let count = app.take_count();
                                for _ in 0..count {
                                    app.next_hunk();
                                }
                            } else {
                                // Scroll-only navigation in no-step mode
                                app.next_hunk_scroll();
                            }
                        }
                        KeyCode::Left | KeyCode::Char('h') => {
                            if app.stepping {
                                let count = app.take_count();
                                for _ in 0..count {
                                    app.prev_hunk();
                                }
                            } else {
                                // Scroll-only navigation in no-step mode
                                app.prev_hunk_scroll();
                            }
                        }
                        // Jump to begin/end of current hunk
                        KeyCode::Char('b') => {
                            app.reset_count();
                            if app.stepping {
                                app.goto_hunk_start();
                            } else {
                                app.goto_hunk_start_scroll();
                            }
                        }
                        KeyCode::Char('e') => {
                            app.reset_count();
                            if app.stepping {
                                app.goto_hunk_end();
                            } else {
                                app.goto_hunk_end_scroll();
                            }
                        }
                        // Peek old without stepping (single view)
                        KeyCode::Char('p') => {
                            app.reset_count();
                            if app.stepping {
                                app.toggle_peek_old_change();
                            }
                        }
                        KeyCode::Char('P') => {
                            app.reset_count();
                            if app.stepping {
                                app.toggle_peek_old_hunk();
                            }
                        }
                        // Yank to clipboard
                        KeyCode::Char('y') => {
                            app.reset_count();
                            app.yank_current_change();
                        }
                        KeyCode::Char('Y') => {
                            app.reset_count();
                            app.yank_current_hunk();
                        }
                        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.reset_count();
                            // Toggle file path popup
                            app.toggle_path_popup();
                        }
                        KeyCode::Home => {
                            app.reset_count();
                            app.goto_start();
                        }
                        KeyCode::Char('g') => {
                            app.reset_count();
                            app.pending_g_prefix = true;
                        }
                        KeyCode::End | KeyCode::Char('G') => {
                            app.reset_count();
                            app.goto_end();
                        }
                        KeyCode::Char('<') => {
                            app.reset_count();
                            if app.stepping {
                                app.goto_first_step();
                            } else {
                                app.goto_first_hunk_scroll();
                            }
                        }
                        KeyCode::Char('>') => {
                            app.reset_count();
                            if app.stepping {
                                app.goto_last_step();
                            } else {
                                app.goto_last_hunk_scroll();
                            }
                        }
                        // File navigation (supports count)
                        KeyCode::Char('[') | KeyCode::BackTab => {
                            let count = app.take_count();
                            for _ in 0..count {
                                app.prev_file();
                            }
                        }
                        KeyCode::Char(']') => {
                            let count = app.take_count();
                            for _ in 0..count {
                                app.next_file();
                            }
                        }
                        // General controls
                        KeyCode::Char(' ') => {
                            app.reset_count();
                            if app.stepping {
                                app.toggle_autoplay();
                            }
                        }
                        KeyCode::Char('B') => {
                            app.reset_count();
                            if app.stepping {
                                app.toggle_autoplay_reverse();
                            }
                        }
                        KeyCode::Tab => {
                            app.reset_count();
                            app.toggle_view_mode();
                        }
                        // Scroll navigation (supports count)
                        KeyCode::Char('K') => {
                            let count = app.take_count();
                            for _ in 0..count {
                                app.scroll_up();
                            }
                        }
                        KeyCode::Char('J') => {
                            let count = app.take_count();
                            for _ in 0..count {
                                app.scroll_down();
                            }
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.reset_count();
                            if let Ok((_, rows)) = crossterm::terminal::size() {
                                let viewport_height = rows.saturating_sub(6) as usize;
                                app.scroll_half_page_up(viewport_height);
                            }
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.reset_count();
                            if let Ok((_, rows)) = crossterm::terminal::size() {
                                let viewport_height = rows.saturating_sub(6) as usize;
                                app.scroll_half_page_down(viewport_height);
                            }
                        }
                        KeyCode::Enter => {
                            app.reset_count();
                            // Switch focus between file list and diff view
                            if app.is_multi_file() {
                                app.file_list_focused = !app.file_list_focused;
                                if !app.file_list_focused {
                                    app.stop_file_filter();
                                }
                            }
                        }
                        KeyCode::Char('+') | KeyCode::Char('=') => {
                            app.reset_count();
                            app.increase_speed();
                        }
                        KeyCode::Char('-') => {
                            app.reset_count();
                            app.decrease_speed();
                        }
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.reset_count();
                            // Toggle file list focus (Ctrl+A)
                            if app.is_multi_file() {
                                app.file_list_focused = !app.file_list_focused;
                                if !app.file_list_focused {
                                    app.stop_file_filter();
                                }
                            }
                        }
                        KeyCode::Char('a') => {
                            app.reset_count();
                            // Toggle animation mode
                            app.toggle_animation();
                        }
                        KeyCode::Char('w') => {
                            app.reset_count();
                            // Toggle line wrap
                            app.toggle_line_wrap();
                        }
                        KeyCode::Char('t') => {
                            app.reset_count();
                            // Toggle syntax highlighting mode
                            app.toggle_syntax();
                        }
                        KeyCode::Char('E') => {
                            app.reset_count();
                            if app.view_mode == ViewMode::Evolution {
                                app.toggle_evo_syntax();
                            }
                        }
                        KeyCode::Char('s') => {
                            app.reset_count();
                            // Toggle stepping state
                            app.toggle_stepping();
                        }
                        KeyCode::Char('S') => {
                            app.reset_count();
                            // Toggle strikethrough for deletions
                            app.toggle_strikethrough_deletions();
                        }
                        KeyCode::Char('H') => {
                            // Scroll left (horizontal)
                            let count = app.take_count();
                            for _ in 0..count {
                                app.scroll_left();
                            }
                        }
                        KeyCode::Char('L') => {
                            // Scroll right (horizontal)
                            let count = app.take_count();
                            for _ in 0..count {
                                app.scroll_right();
                            }
                        }
                        KeyCode::Char('z') => {
                            app.reset_count();
                            // Center on active change (like Vim's zz)
                            if let Ok((_, rows)) = crossterm::terminal::size() {
                                let viewport_height = rows.saturating_sub(4) as usize;
                                app.center_on_active(viewport_height);
                            }
                        }
                        KeyCode::Char('Z') => {
                            app.reset_count();
                            // Toggle zen mode
                            app.toggle_zen();
                        }
                        KeyCode::Char('r') => {
                            app.replay_step();
                        }
                        KeyCode::Char('R') => {
                            app.reset_count();
                            if app.file_list_focused {
                                // Refresh all files from git
                                app.refresh_all_files();
                            } else {
                                // Refresh current file from disk
                                app.refresh_current_file();
                            }
                        }
                        KeyCode::Char('f') => {
                            app.reset_count();
                            // Toggle file panel visibility
                            if app.is_multi_file() {
                                app.toggle_file_panel();
                            }
                        }
                        KeyCode::Char('/') => {
                            app.reset_count();
                            if app.file_list_focused {
                                app.start_file_filter();
                            } else {
                                app.start_search();
                            }
                        }
                        KeyCode::Char(':') => {
                            app.reset_count();
                            if !app.file_list_focused {
                                app.start_goto();
                            }
                        }
                        KeyCode::Char('n') => {
                            app.reset_count();
                            app.search_next();
                        }
                        KeyCode::Char('N') => {
                            app.reset_count();
                            app.search_prev();
                        }
                        KeyCode::Char('?') => {
                            app.reset_count();
                            // Toggle help popover
                            app.toggle_help();
                        }
                        _ => {
                            app.reset_count();
                        }
                    }
                }
                _ => {}
            }
        }

        // Handle autoplay
        app.tick();

        if app.should_quit {
            return Ok(());
        }
    }
}

fn run_dashboard<B: Backend>(
    terminal: &mut Terminal<B>,
    dashboard: &mut Dashboard,
) -> Result<Option<DashboardSelection>> {
    let tick_rate = Duration::from_millis(16);

    loop {
        terminal.draw(|f| dashboard.draw(f))?;

        if event::poll(tick_rate)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let list_height = dashboard.list_height(terminal.size()?.height);
                    if dashboard.filter_active() {
                        match key.code {
                            KeyCode::Esc => {
                                dashboard.stop_filter();
                            }
                            KeyCode::Enter => {
                                if let Some(selection) = dashboard.selection() {
                                    return Ok(Some(selection));
                                }
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                dashboard.clear_filter();
                            }
                            KeyCode::Backspace => {
                                dashboard.pop_filter_char();
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                dashboard.move_selection(1, list_height);
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                dashboard.move_selection(-1, list_height);
                            }
                            KeyCode::PageDown => {
                                dashboard.page_down(list_height);
                            }
                            KeyCode::PageUp => {
                                dashboard.page_up(list_height);
                            }
                            KeyCode::Char('g') | KeyCode::Home => {
                                dashboard.select_first(list_height);
                            }
                            KeyCode::Char('G') | KeyCode::End => {
                                dashboard.select_last(list_height);
                            }
                            KeyCode::Char(ch) => {
                                dashboard.push_filter_char(ch);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
                        KeyCode::Char('/') => {
                            dashboard.start_filter();
                        }
                        KeyCode::Char('r') => {
                            dashboard.clear_pin();
                        }
                        KeyCode::Char(' ') => {
                            dashboard.toggle_pin();
                        }
                        KeyCode::Enter => {
                            if let Some(selection) = dashboard.selection() {
                                return Ok(Some(selection));
                            }
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            dashboard.move_selection(1, list_height);
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            dashboard.move_selection(-1, list_height);
                        }
                        KeyCode::PageDown => {
                            dashboard.page_down(list_height);
                        }
                        KeyCode::PageUp => {
                            dashboard.page_up(list_height);
                        }
                        KeyCode::Char('g') | KeyCode::Home => {
                            dashboard.select_first(list_height);
                        }
                        KeyCode::Char('G') | KeyCode::End => {
                            dashboard.select_last(list_height);
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    let list_height = dashboard.list_height(terminal.size()?.height);
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
