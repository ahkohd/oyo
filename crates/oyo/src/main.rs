//! Oyo CLI - terminal diff viewer TUI

mod app;
mod avatars;
mod blame;
mod color;
mod config;
mod control;
mod csv_preview;
mod dashboard;
mod forgejo;
mod gitlab;
mod input;
mod jless;
mod keybindings;
mod markdown;
mod structured_preview;
mod syntax;
#[cfg(test)]
mod test_utils;
mod time_format;
mod toasts;
mod ui;
mod views;

use crate::dashboard::{
    Dashboard, DashboardConfig, DashboardContextMenuResult, DashboardSelection,
};
use crate::input::handle_app_key;
use crate::keybindings::{DashboardAction, DashboardFilterAction, Dispatch, Keybindings};
use crate::syntax::{list_syntax_themes, SyntaxEngine};
use crate::time_format::TimeFormatter;
use crate::toasts::ToastEvent;
use anyhow::{anyhow, Context, Result};
use app::{
    review::{
        ReviewAuthor, ReviewComment, ReviewCommentFilter, ReviewProviderComment,
        ReviewPullRequestTarget, ReviewRange, ReviewRemoteOption, ReviewSide, ReviewSyncAction,
        ReviewTargetKind, ReviewTargetMetadata,
    },
    App, ViewMode,
};
use clap::{Parser, Subcommand, ValueEnum};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use oyo_core::{
    multi::{FileSide, RawFileDiff},
    DirectoryScanOptions, LineKind, MultiFileDiff, ViewLine,
};
use ratatui::prelude::*;
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, IsTerminal, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{Duration, Instant};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const INDEX_REF: &str = "INDEX";
const MAX_COALESCED_MOUSE_SCROLL_READS: usize = 4096;
const MAX_DISCRETE_MOUSE_SCROLL_ACTIONS_PER_FRAME: isize = 16;
const MAX_EXIT_INPUT_DRAIN_EVENTS: usize = 65_536;
const EXIT_INPUT_DRAIN: Duration = Duration::from_millis(100);
const MOUSE_SCROLL_FRAME: Duration = Duration::from_millis(16);
const OYO_CODE_REVIEW_SKILL: &str = include_str!("../docs/SKILL.md");
const OYO_CONTROL_SKILL: &str = include_str!("../docs/CONTROL.md");

type TuiBackend = CrosstermBackend<Box<dyn io::Write>>;
type TuiTerminal = Terminal<TuiBackend>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseScrollTarget {
    CommandPalette,
    FileSearch,
    FilePanel,
    Step,
    Diff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingMouseScroll {
    target: MouseScrollTarget,
    delta: isize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlockedMouseScroll {
    target: MouseScrollTarget,
    direction: isize,
}

#[derive(Parser, Debug)]
#[command(name = "oy")]
#[command(author, version, about = "A terminal diff viewer")]
#[command(subcommand_precedence_over_arg = true)]
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

    /// Name for this live TUI control session
    #[arg(long)]
    session: Option<String>,

    /// Auto-play through all changes
    #[arg(long)]
    autoplay: bool,

    /// Theme mode: dark or light
    #[arg(long, value_enum, global = true)]
    theme_mode: Option<CliThemeMode>,

    /// Theme name (overrides config)
    #[arg(long, global = true)]
    theme_name: Option<String>,

    /// Extra config file to merge after the default config (repeatable)
    #[arg(long = "config", value_name = "FILE", global = true)]
    config_files: Vec<PathBuf>,

    /// Syntax theme name or .tmTheme file (overrides config)
    #[arg(long, global = true)]
    syntax_theme: Option<String>,

    /// Dump syntax scopes for a file and exit
    #[arg(long, value_name = "FILE")]
    dump_scopes: Option<PathBuf>,

    /// Enable step mode
    #[arg(long, global = true, conflicts_with = "no_step")]
    step: bool,

    /// Disable stepping (kept for compatibility; now the default)
    #[arg(long, global = true, conflicts_with = "step")]
    no_step: bool,

    /// Show working tree changes (working tree vs HEAD)
    #[arg(long, conflicts_with_all = ["staged", "range"])]
    worktree: bool,

    /// Show staged changes (index vs HEAD)
    #[arg(long, alias = "cached", conflicts_with_all = ["worktree", "range"])]
    staged: bool,

    /// Diff a git range (e.g. HEAD~1..HEAD)
    #[arg(long, value_name = "RANGE", conflicts_with_all = ["worktree", "staged"])]
    range: Option<String>,

    /// Directory for review database
    #[arg(long, value_name = "DIR", global = true)]
    review_dir: Option<PathBuf>,

    /// Disable loading/saving persisted review state
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
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// List built-in themes
    Themes,
    /// List syntax themes
    SyntaxThemes,
    /// Open History to pick a git range
    Log {
        /// Number of commits to show
        #[arg(long, default_value = "200")]
        limit: usize,
    },
    /// Show and manage saved reviews
    Review {
        /// Print JSON
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        command: Option<ReviewCommand>,
    },
    /// Control a running Oyo TUI
    #[command(visible_alias = "c")]
    Control {
        /// Name for this live TUI control session
        #[arg(short = 's', long, global = true)]
        session: Option<String>,
        #[command(subcommand)]
        command: Option<CtlCommand>,
    },
    /// Show Oyo agent skill helpers
    Skill {
        #[command(subcommand)]
        command: Option<SkillCommand>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SkillName {
    Review,
    Control,
}

impl SkillName {
    fn directory(self) -> &'static str {
        match self {
            SkillName::Review => "oyo-code-review",
            SkillName::Control => "oyo-tui-control",
        }
    }

    fn content(self) -> &'static str {
        match self {
            SkillName::Review => OYO_CODE_REVIEW_SKILL,
            SkillName::Control => OYO_CONTROL_SKILL,
        }
    }
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    /// Print a local Oyo skill path
    Path {
        /// Skill to print
        #[arg(value_enum, default_value_t = SkillName::Review)]
        skill: SkillName,
    },
}

#[derive(Debug, Subcommand)]
enum CtlCommand {
    /// List running TUI control sessions
    List {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Show session metadata
    Get {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Rename a TUI control session
    Rename {
        /// New session name
        name: String,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Show what the TUI is displaying now
    Where {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Show the loaded diff structure
    Diff {
        /// Print JSON
        #[arg(long)]
        json: bool,
        /// Include raw patch text when supported
        #[arg(long)]
        include_patch: bool,
    },
    /// Move forward
    Next {
        /// Repeat count
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
    /// Move backward
    Prev {
        /// Repeat count
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
    /// Move between hunks, or jump within the current hunk
    Hunk {
        /// next, prev, start or end
        mode: String,
        /// Repeat count for next/prev
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
    /// Select a file, or pass next/prev
    File {
        /// File path, next or prev
        target: Option<String>,
        /// Open in a topbar tab
        #[arg(long)]
        new_tab: bool,
        /// Repeat count for next/prev
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
    /// Jump to a file, line, hunk or step
    Goto {
        /// File path
        #[arg(long)]
        file: Option<String>,
        /// New-side line number
        #[arg(long, conflicts_with_all = ["old_line", "hunk", "step_number", "start", "end"])]
        new_line: Option<usize>,
        /// Old-side line number
        #[arg(long, conflicts_with_all = ["new_line", "hunk", "step_number", "start", "end"])]
        old_line: Option<usize>,
        /// Hunk number
        #[arg(long, conflicts_with_all = ["new_line", "old_line", "step_number", "start", "end"])]
        hunk: Option<usize>,
        /// Step number
        #[arg(long = "step-number", conflicts_with_all = ["new_line", "old_line", "hunk", "start", "end"])]
        step_number: Option<usize>,
        /// Jump to start
        #[arg(long, conflicts_with_all = ["new_line", "old_line", "hunk", "step_number", "end"])]
        start: bool,
        /// Jump to end
        #[arg(long, conflicts_with_all = ["new_line", "old_line", "hunk", "step_number", "start"])]
        end: bool,
    },
    /// Change the loaded review target
    Target {
        /// Commit, branch, bookmark, change ID, revset or range
        target: Option<String>,
        /// Load working tree changes
        #[arg(long, conflicts_with_all = ["staged", "target"])]
        worktree: bool,
        /// Load staged changes
        #[arg(long, conflicts_with_all = ["worktree", "target"])]
        staged: bool,
    },
    /// Set or cycle view mode
    View {
        /// unified, split, evolution, blame, preview, next or prev
        mode: String,
    },
    /// Set step mode
    Step {
        /// on, off or toggle
        mode: String,
    },
    /// Set watch mode
    Watch {
        /// on, off or toggle
        mode: String,
    },
    /// Change animation speed
    Speed {
        /// increase or decrease
        mode: String,
    },
    /// Set animation mode
    Animation {
        /// on, off or toggle
        mode: String,
    },
    /// Set line wrap mode
    Wrap {
        /// on, off or toggle
        mode: String,
    },
    /// Set syntax highlighting mode
    Syntax {
        /// on, off or toggle
        mode: String,
    },
    /// Set zen mode
    Zen {
        /// on, off or toggle
        mode: String,
    },
    /// Control the sidebar
    Sidebar {
        /// files, comments, close, toggle or focus
        mode: String,
    },
    /// Open a topbar tab
    Tab {
        /// help, pr-comments, close or file
        kind: String,
        /// File path for `file`
        file: Option<String>,
    },
    /// Play visible steps with a delay
    Play {
        /// Start step: current, start, end or a 1-based step number
        #[arg(long)]
        from: Option<String>,
        /// End step: current, start, end or a 1-based step number
        #[arg(long)]
        to: Option<String>,
        /// Delay between visible steps, for example 700ms or 1s
        #[arg(long, default_value = "700ms")]
        delay: String,
    },
    /// Stop autoplay if it is running
    Pause,
    /// Cancel queued control work
    Cancel,
    /// Run a named TUI action
    Action {
        /// Action ID, for example normal.step_down
        id: String,
        /// Repeat count
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum ReviewCommand {
    /// Show saved reviews for this workspace
    Log {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Show the review for the current target or revision
    Status {
        /// Commit, branch, bookmark, change ID, revset or range
        revision: Option<String>,
        /// Review target
        #[arg(
            short = 't',
            long = "target",
            value_name = "REV",
            conflicts_with = "revision"
        )]
        target: Option<String>,
        /// Only show unresolved comments
        #[arg(long)]
        unresolved: bool,
        /// Only show outdated comments
        #[arg(long, conflicts_with = "no_outdated")]
        outdated: bool,
        /// Hide outdated comments
        #[arg(long = "no-outdated")]
        no_outdated: bool,
        /// Only show comments with this ID, repeatable
        #[arg(long = "id")]
        ids: Vec<u64>,
        /// Only show comments changed at or after this Unix timestamp
        #[arg(long)]
        since: Option<u64>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Show and manage comments
    Comment {
        /// Saved review ID, commit, branch, bookmark, change ID, revset or range
        revision: Option<String>,
        /// Review target
        #[arg(
            short = 't',
            long = "target",
            value_name = "REV",
            conflicts_with = "revision"
        )]
        target: Option<String>,
        /// Only show unresolved comments
        #[arg(long)]
        unresolved: bool,
        /// Only show outdated comments
        #[arg(long, conflicts_with = "no_outdated")]
        outdated: bool,
        /// Hide outdated comments
        #[arg(long = "no-outdated")]
        no_outdated: bool,
        /// Filter by author name, email or username
        #[arg(long)]
        author: Option<String>,
        /// Filter by author type: human, agent or bot
        #[arg(long)]
        author_type: Option<String>,
        /// Only show comments with this ID, repeatable
        #[arg(long = "id")]
        ids: Vec<u64>,
        /// Only show comments changed at or after this Unix timestamp
        #[arg(long)]
        since: Option<u64>,
        /// Print JSON
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        command: Option<ReviewCommentCommand>,
    },
    /// Export the current target or revision review
    Export {
        /// Commit, branch, bookmark, change ID, revset or range
        revision: Option<String>,
        /// Review target
        #[arg(
            short = 't',
            long = "target",
            value_name = "REV",
            conflicts_with = "revision"
        )]
        target: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value_t = ReviewExportFormat::Markdown)]
        format: ReviewExportFormat,
        /// Write to a file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Pull provider comments into the local review
    Pull {
        /// Optional target followed by optional remote
        #[arg(num_args = 0..=2)]
        args: Vec<String>,
        /// Review target
        #[arg(short = 't', long = "target", value_name = "REV")]
        target: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Push local review comments to the provider
    Push {
        /// Optional target followed by optional remote
        #[arg(num_args = 0..=2)]
        args: Vec<String>,
        /// Review target
        #[arg(short = 't', long = "target", value_name = "REV")]
        target: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Delete the saved review for the current target or revision
    Abandon {
        /// Commit, branch, bookmark, change ID, revset or range
        revision: Option<String>,
        /// Review target
        #[arg(
            short = 't',
            long = "target",
            value_name = "REV",
            conflicts_with = "revision"
        )]
        target: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ReviewCommentCommand {
    /// Add a comment
    New {
        /// Commit, branch, bookmark, change ID, revset or range
        revision: Option<String>,
        /// Review target
        #[arg(
            short = 't',
            long = "target",
            value_name = "REV",
            conflicts_with = "revision"
        )]
        target: Option<String>,
        /// Changed file path
        #[arg(long)]
        file: String,
        /// New-side line number
        #[arg(long, conflicts_with_all = ["old_line", "file_level"])]
        new_line: Option<usize>,
        /// Old-side line number
        #[arg(long, conflicts_with_all = ["new_line", "file_level"])]
        old_line: Option<usize>,
        /// Comment on the whole file
        #[arg(long, conflicts_with_all = ["new_line", "old_line"])]
        file_level: bool,
        /// Comment body
        #[arg(long)]
        body: String,
        /// Comment author type: human, agent or bot
        #[arg(long)]
        author_type: Option<String>,
        /// Comment author name
        #[arg(long)]
        author_name: Option<String>,
        /// Comment author email
        #[arg(long)]
        author_email: Option<String>,
        /// Comment author username, or provider=username, repeatable
        #[arg(long, value_name = "USERNAME")]
        author_username: Vec<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Edit a comment body
    Edit {
        /// Optional revision followed by a comment ID, or just a comment ID
        #[arg(num_args = 1..=2)]
        args: Vec<String>,
        /// Review target
        #[arg(short = 't', long = "target", value_name = "REV")]
        target: Option<String>,
        /// Comment body
        #[arg(long)]
        body: String,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Reply to a pulled review-thread comment
    Reply {
        /// Optional revision followed by a parent comment ID, or just a parent comment ID
        #[arg(num_args = 1..=2)]
        args: Vec<String>,
        /// Review target
        #[arg(short = 't', long = "target", value_name = "REV")]
        target: Option<String>,
        /// Reply body
        #[arg(long)]
        body: String,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove a comment
    Rm {
        /// Optional revision followed by a comment ID, or just a comment ID
        #[arg(num_args = 1..=2)]
        args: Vec<String>,
        /// Review target
        #[arg(short = 't', long = "target", value_name = "REV")]
        target: Option<String>,
        /// Confirm removal
        #[arg(long)]
        yes: bool,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Mark a comment resolved
    Resolve {
        /// Optional revision followed by a comment ID, or just a comment ID
        #[arg(num_args = 1..=2)]
        args: Vec<String>,
        /// Review target
        #[arg(short = 't', long = "target", value_name = "REV")]
        target: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Mark a comment unresolved
    #[command(name = "unresolve", visible_alias = "reopen")]
    Reopen {
        /// Optional revision followed by a comment ID, or just a comment ID
        #[arg(num_args = 1..=2)]
        args: Vec<String>,
        /// Review target
        #[arg(short = 't', long = "target", value_name = "REV")]
        target: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Apply comments from a JSON file, or '-' for stdin
    Apply {
        /// Optional revision followed by a JSON file, or just a JSON file
        #[arg(num_args = 1..=2)]
        args: Vec<String>,
        /// Review target
        #[arg(short = 't', long = "target", value_name = "REV")]
        target: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ReviewExportFormat {
    Json,
    Markdown,
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
    /// Preview view - rendered Markdown or source text
    Preview,
}

impl From<CliViewMode> for ViewMode {
    fn from(mode: CliViewMode) -> Self {
        match mode {
            CliViewMode::Unified => ViewMode::UnifiedPane,
            CliViewMode::Split => ViewMode::Split,
            CliViewMode::Evolution => ViewMode::Evolution,
            CliViewMode::Blame => ViewMode::Blame,
            CliViewMode::Preview => ViewMode::Preview,
        }
    }
}

/// Represents input mode detected from arguments
#[derive(Debug)]
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
    /// jj revision or revset
    JjRevision { rev: String },
    /// One file in a non-colocated jj workspace
    JjFile { path: PathBuf },
    /// No valid input
    None,
}

#[derive(Debug)]
enum AppExit {
    Quit,
    OpenDashboard,
    Reload(InputMode, u64),
}

struct BuiltDiff {
    multi_diff: MultiFileDiff,
    branch: Option<String>,
    workspace_root: Option<PathBuf>,
}

/// Detect if we're being called as a git external diff tool
/// Git calls: oy path old-file old-hex old-mode new-file new-hex new-mode
fn detect_input_mode(paths: &[PathBuf]) -> InputMode {
    detect_input_mode_in(paths, &std::env::current_dir().unwrap_or_default())
}

fn detect_input_mode_in(paths: &[PathBuf], cwd: &Path) -> InputMode {
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
        let value = paths[0].to_string_lossy();
        let argument_path = if paths[0].is_absolute() {
            paths[0].clone()
        } else {
            cwd.join(&paths[0])
        };
        let path_exists = argument_path.exists();
        if is_jj_repo(cwd) && should_treat_as_jj_revision(path_exists, &value) {
            let rev =
                if value.as_ref() != "@" && !value.contains("..") && jj_bookmark_exists(&value) {
                    jj_bookmark_revset(&value)
                } else {
                    value.to_string()
                };
            InputMode::JjRevision { rev }
        } else if path_exists && is_jj_repo(cwd) && !oyo_core::git::is_git_repo(cwd) {
            let repo_root = jj_workspace_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
            let path = argument_path
                .canonicalize()
                .ok()
                .and_then(|path| {
                    repo_root
                        .canonicalize()
                        .ok()
                        .and_then(|root| path.strip_prefix(root).ok().map(Path::to_path_buf))
                })
                .unwrap_or_else(|| paths[0].clone());
            InputMode::JjFile { path }
        } else if !path_exists && oyo_core::git::is_git_repo(cwd) {
            git_ref_input_mode(cwd, &value).unwrap_or_else(|| InputMode::GitFile {
                path: paths[0].clone(),
            })
        } else {
            InputMode::GitFile {
                path: paths[0].clone(),
            }
        }
    } else if paths.is_empty() {
        if is_jj_repo(cwd) {
            InputMode::JjRevision {
                rev: default_jj_review_revision(),
            }
        } else {
            InputMode::GitUncommitted
        }
    } else {
        InputMode::None
    }
}

fn git_merge_base_in(root: &Path, from: &str, to: &str) -> Option<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(root)
        .arg("merge-base")
        .arg(from)
        .arg(to)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!base.is_empty()).then_some(base)
}

fn git_merge_base(from: &str, to: &str) -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    git_merge_base_in(&cwd, from, to)
}

fn parse_range(range: &str) -> Result<(String, String)> {
    if let Some((from, to)) = range.split_once("...") {
        if from.is_empty() || to.is_empty() {
            anyhow::bail!("Range must be in the form A..B or A...B");
        }
        if to.contains("..") {
            anyhow::bail!("Range must be in the form A..B or A...B");
        }
        let from = git_merge_base(from, to).unwrap_or_else(|| from.to_string());
        return Ok((from, to.to_string()));
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

fn is_jj_repo(path: &Path) -> bool {
    jj_workspace_root(path).is_some()
}

fn jj_workspace_root(path: &Path) -> Option<PathBuf> {
    let output = ProcessCommand::new("jj")
        .arg("-R")
        .arg(path)
        .arg("--no-pager")
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .arg("root")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!root.is_empty()).then(|| PathBuf::from(root))
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

fn install_panic_terminal_restore() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stderr(), PopKeyboardEnhancementFlags);
        let _ = execute!(io::stderr(), DisableMouseCapture);
        drain_queued_input_events();
        let _ = disable_raw_mode();
        let _ = execute!(io::stderr(), LeaveAlternateScreen);
        default_hook(info);
    }));
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
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn setup_image_picker() -> Option<ratatui_image::picker::Picker> {
    ratatui_image::picker::Picker::from_query_stdio().ok()
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

fn drain_queued_input_events() {
    let started = Instant::now();
    for _ in 0..MAX_EXIT_INPUT_DRAIN_EVENTS {
        if started.elapsed() >= EXIT_INPUT_DRAIN {
            break;
        }
        match event::poll(Duration::from_millis(1)) {
            Ok(true) => {
                if event::read().is_err() {
                    break;
                }
            }
            Ok(false) | Err(_) => break,
        }
    }
}

fn restore_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    execute!(terminal.backend_mut(), DisableMouseCapture)?;
    drain_queued_input_events();
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn suspend_terminal_for_child(terminal: &mut TuiTerminal) -> Result<()> {
    restore_terminal(terminal)
}

fn resume_terminal_after_child(terminal: &mut TuiTerminal) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    let _ = execute!(
        terminal.backend_mut(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
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
    app.file_panel_position = config.files.panel_position;
    app.file_count_mode = config.files.counts;
    app.auto_center = config.ui.auto_center;
    app.watch = config.ui.watch;
    app.overscroll = config.ui.overscroll;
    app.confirm_quit = config.ui.confirm_quit;
    app.topbar = config.ui.topbar;
    app.line_wrap = config.ui.line_wrap;
    app.set_fold_context_lines(config.ui.fold_context_lines);
    app.set_fold_context_mode(config.ui.fold_context);
    app.scrollbar_visible = config.ui.scrollbar;
    app.strikethrough_deletions = config.ui.strikethrough_deletions;
    app.gutter_signs = config.ui.gutter_signs;
    app.toasts_enabled = config.ui.toasts.enabled;
    app.toast_position = config.ui.toasts.position.toast_position();
    app.preview_change_bars = config.ui.diff.preview_change_bars;
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
    app.review_hooks = config.review.hooks.clone();
    app.review_actions = config.review.actions.clone();
    app.selection_actions = config.selection.actions.clone();
    app.hunk_wrap = config.navigation.wrap.hunk;
    app.step_wrap = config.navigation.wrap.step;
    app.primary_marker = config.ui.primary_marker.clone();
    app.primary_marker_right = config
        .ui
        .primary_marker_right
        .clone()
        .unwrap_or_else(|| "◀".to_string());
    app.extent_marker = config.ui.extent_marker_left().to_string();
    app.extent_marker_right = config
        .ui
        .extent_marker_right
        .clone()
        .unwrap_or_else(|| "▐".to_string());
    app.extent_marker_deleted = config.ui.extent_marker_deleted.clone();
    app.theme = config.ui.theme.resolve(light_mode);
    app.ui_theme_name = config.ui.theme.name.clone();
    app.time_format = TimeFormatter::new(&config.ui.time);
    app.theme_is_light = light_mode;
    app.start_syntax_engine_load();

    if args.step {
        app.stepping = true;
    } else if args.no_step {
        app.stepping = false;
    } else {
        app.stepping = config.ui.stepping;
    }
    if !app.stepping {
        app.enter_no_step_mode();
    }
    app.handle_file_enter();
}

fn default_review_base_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("oyo")
        .join("reviews")
}

fn oyo_skill_path(skill: SkillName) -> Result<PathBuf> {
    let path = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("oyo")
        .join("skills")
        .join(skill.directory())
        .join("SKILL.md");
    if path.parent().is_some_and(|parent| !parent.exists()) {
        fs::create_dir_all(path.parent().unwrap()).context("Failed to create skill directory")?;
    }
    if fs::read_to_string(&path).ok().as_deref() != Some(skill.content()) {
        fs::write(&path, skill.content()).context("Failed to write Oyo skill")?;
    }
    Ok(path)
}

fn resolve_review_dir(path: PathBuf, workspace_root: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        workspace_root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .join(path)
    }
}

fn review_base_dir(config: &config::Config, args: &Args, workspace_root: Option<&Path>) -> PathBuf {
    args.review_dir
        .clone()
        .or_else(|| config.review.dir.clone())
        .map(|path| resolve_review_dir(path, workspace_root))
        .unwrap_or_else(default_review_base_dir)
}

fn command_config_value(mut command: ProcessCommand, key: &str) -> Option<String> {
    let output = command.arg(key).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn strip_config_quotes(value: &str) -> String {
    let value = value.trim();
    serde_json::from_str::<String>(value).unwrap_or_else(|_| value.to_string())
}

fn username_provider(key: &str) -> Option<String> {
    if let Some(provider) = key.strip_prefix("usernames.") {
        return (!provider.is_empty()).then(|| provider.to_string());
    }
    if let Some(provider) = key.strip_prefix("user.") {
        if matches!(provider, "name" | "email" | "signingkey") {
            return None;
        }
        return Some(provider.trim_end_matches("_username").to_string());
    }
    key.strip_suffix(".user")
        .filter(|provider| !provider.is_empty())
        .map(ToString::to_string)
}

fn git_config_value(root: &Path, key: &str) -> Option<String> {
    let mut command = ProcessCommand::new("git");
    command.arg("-C").arg(root).arg("config").arg("--get");
    command_config_value(command, key)
}

fn git_usernames(root: &Path) -> BTreeMap<String, String> {
    let mut command = ProcessCommand::new("git");
    let output = command
        .arg("-C")
        .arg(root)
        .arg("config")
        .arg("--get-regexp")
        .arg("^(usernames\\.|user\\.|[A-Za-z0-9_-]+\\.user$)")
        .output()
        .ok();
    let mut usernames = BTreeMap::new();
    let Some(output) = output.filter(|output| output.status.success()) else {
        return usernames;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(key) = parts.next() else { continue };
        let Some(value) = parts.next() else { continue };
        if let Some(provider) = username_provider(key) {
            let value = strip_config_quotes(value);
            if !value.is_empty() {
                usernames.insert(provider, value);
            }
        }
    }
    usernames
}

fn jj_config_value(root: &Path, key: &str) -> Option<String> {
    let mut command = ProcessCommand::new("jj");
    command
        .arg("-R")
        .arg(root)
        .arg("--no-pager")
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .arg("config")
        .arg("get");
    command_config_value(command, key).map(|value| strip_config_quotes(&value))
}

fn jj_usernames(root: &Path) -> BTreeMap<String, String> {
    let output = ProcessCommand::new("jj")
        .arg("-R")
        .arg(root)
        .arg("--no-pager")
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .arg("config")
        .arg("list")
        .output()
        .ok();
    let mut usernames = BTreeMap::new();
    let Some(output) = output.filter(|output| output.status.success()) else {
        return usernames;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if let Some(provider) = username_provider(key.trim()) {
            let value = strip_config_quotes(value);
            if !value.is_empty() {
                usernames.insert(provider, value);
            }
        }
    }
    usernames
}

fn first_config_value(root: &Path, is_jj: bool, key: &str) -> Option<String> {
    if is_jj {
        jj_config_value(root, key).or_else(|| git_config_value(root, key))
    } else {
        git_config_value(root, key)
    }
}

fn github_avatar_url(username: &str) -> Option<String> {
    let username = username.trim().trim_start_matches('@');
    (!username.is_empty()).then(|| format!("https://github.com/{username}.png?size=64"))
}

fn review_author_for_workspace(root: Option<&Path>) -> Option<ReviewAuthor> {
    if let Ok(Some(author)) = review_author_from_env() {
        return Some(author);
    }
    let root = root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let is_jj = is_jj_repo(&root);
    let name = first_config_value(&root, is_jj, "user.name")?;
    let email = first_config_value(&root, is_jj, "user.email");
    let mut usernames = git_usernames(&root);
    if is_jj {
        usernames.extend(jj_usernames(&root));
    }
    review_author(name, email, None, usernames)
}

fn review_author(
    name: String,
    email: Option<String>,
    author_type: Option<String>,
    usernames: BTreeMap<String, String>,
) -> Option<ReviewAuthor> {
    let avatar_url = usernames
        .get("github")
        .and_then(|username| github_avatar_url(username));
    if let Some(url) = avatar_url.as_deref() {
        let _ = crate::avatars::cache_avatar_url(url);
    }
    Some(ReviewAuthor {
        name,
        email,
        author_type,
        usernames,
        avatar_url,
    })
}

fn parse_author_usernames(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut usernames = BTreeMap::new();
    for value in values {
        let (provider, username) = value
            .split_once('=')
            .map(|(provider, username)| (provider.trim(), username.trim()))
            .unwrap_or(("local", value.trim()));
        let username = username.trim_start_matches('@');
        if provider.is_empty() || username.is_empty() {
            anyhow::bail!("Pass --author-username as username or provider=username");
        }
        usernames.insert(provider.to_string(), username.to_string());
    }
    Ok(usernames)
}

fn review_author_type(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if matches!(value, "human" | "agent" | "bot") {
        Ok(Some(value.to_string()))
    } else {
        anyhow::bail!("Pass --author-type as human, agent or bot")
    }
}

fn env_text(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_author_usernames() -> Vec<String> {
    env_text("OYO_REVIEW_AUTHOR_USERNAME")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn review_author_from_env() -> Result<Option<ReviewAuthor>> {
    let name = env_text("OYO_REVIEW_AUTHOR_NAME");
    let email = env_text("OYO_REVIEW_AUTHOR_EMAIL");
    let author_type = env_text("OYO_REVIEW_AUTHOR_TYPE");
    let usernames = env_author_usernames();
    review_author_from_values(
        name.as_deref(),
        email.as_deref(),
        author_type.as_deref(),
        &usernames,
    )
}

fn review_author_from_cli(
    name: Option<&str>,
    email: Option<&str>,
    author_type: Option<&str>,
    username_values: &[String],
) -> Result<Option<ReviewAuthor>> {
    let env_name = env_text("OYO_REVIEW_AUTHOR_NAME");
    let env_email = env_text("OYO_REVIEW_AUTHOR_EMAIL");
    let env_author_type = env_text("OYO_REVIEW_AUTHOR_TYPE");
    let mut usernames = env_author_usernames();
    usernames.extend(username_values.iter().cloned());
    review_author_from_values(
        name.or(env_name.as_deref()),
        email.or(env_email.as_deref()),
        author_type.or(env_author_type.as_deref()),
        &usernames,
    )
}

fn review_author_from_values(
    name: Option<&str>,
    email: Option<&str>,
    author_type: Option<&str>,
    username_values: &[String],
) -> Result<Option<ReviewAuthor>> {
    if name.is_none() && email.is_none() && author_type.is_none() && username_values.is_empty() {
        return Ok(None);
    }
    let author_type = review_author_type(author_type)?;
    let usernames = parse_author_usernames(username_values)?;
    let name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .or_else(|| usernames.values().next().cloned())
        .or_else(|| {
            email
                .map(str::trim)
                .filter(|email| !email.is_empty())
                .map(ToString::to_string)
        })
        .or_else(|| match author_type.as_deref() {
            Some("agent") => Some("Agent".to_string()),
            Some("bot") => Some("Bot".to_string()),
            Some("human") => Some("Human".to_string()),
            _ => None,
        })
        .ok_or_else(|| anyhow!("Pass --author-name, --author-email or --author-username"))?;
    let email = email
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(ToString::to_string);
    Ok(review_author(name, email, author_type, usernames))
}

fn apply_review_storage_to_app(
    app: &mut App,
    config: &config::Config,
    args: &Args,
    workspace_root: Option<PathBuf>,
) {
    let base = review_base_dir(config, args, workspace_root.as_deref());
    app.set_review_author(review_author_for_workspace(workspace_root.as_deref()));
    app.set_review_workspace_root(workspace_root);
    app.set_review_base_dir(Some(base));
}

fn run_jj(repo_root: &Path, args: &[&str]) -> Result<String> {
    let output = ProcessCommand::new("jj")
        .current_dir(repo_root)
        .arg("-R")
        .arg(repo_root)
        .arg("--no-pager")
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .args(args)
        .output()
        .with_context(|| format!("Failed to run jj in {}", repo_root.display()))?;
    if !output.status.success() {
        anyhow::bail!(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) fn jj_working_copy_stamp(repo_root: &Path) -> Option<String> {
    run_jj(
        repo_root,
        &["log", "--no-graph", "-r", "@", "-T", "commit_id"],
    )
    .ok()
    .map(|stamp| stamp.trim().to_string())
}

fn run_jj_bytes(repo_root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = ProcessCommand::new("jj")
        .current_dir(repo_root)
        .arg("-R")
        .arg(repo_root)
        .arg("--no-pager")
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .args(args)
        .output()
        .with_context(|| format!("Failed to run jj in {}", repo_root.display()))?;
    if !output.status.success() {
        anyhow::bail!(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output.stdout)
}

fn jj_summary_paths(status: oyo_core::git::FileStatus, path: &str) -> (Option<String>, String) {
    if status != oyo_core::git::FileStatus::Renamed {
        return (None, path.to_string());
    }
    let Some(open) = path.find('{') else {
        return (None, path.to_string());
    };
    let Some(close_offset) = path[open + 1..].find('}') else {
        return (None, path.to_string());
    };
    let close = open + 1 + close_offset;
    let Some((old, new)) = path[open + 1..close].split_once(" => ") else {
        return (None, path.to_string());
    };
    let prefix = &path[..open];
    let suffix = &path[close + 1..];
    (
        Some(format!("{prefix}{old}{suffix}")),
        format!("{prefix}{new}{suffix}"),
    )
}

fn jj_diff_summary(
    repo_root: &Path,
    rev: &str,
    path: Option<&Path>,
) -> Result<(String, String, String)> {
    let path = path.map(|path| path.to_string_lossy());
    if rev.contains("..") {
        let (from, to) = parse_range(rev)?;
        let base = format!("fork_point(({from}) | ({to}))");
        let summary = if let Some(path) = path.as_deref() {
            run_jj(
                repo_root,
                &[
                    "diff",
                    "--from",
                    &base,
                    "--to",
                    &to,
                    "--summary",
                    "--",
                    path,
                ],
            )?
        } else {
            run_jj(
                repo_root,
                &["diff", "--from", &base, "--to", &to, "--summary"],
            )?
        };
        Ok((summary, base, to))
    } else {
        let summary = if let Some(path) = path.as_deref() {
            run_jj(repo_root, &["diff", "-r", rev, "--summary", "--", path])?
        } else {
            run_jj(repo_root, &["diff", "-r", rev, "--summary"])?
        };
        Ok((summary, format!("({rev})-"), rev.to_string()))
    }
}

fn jj_summary_entries(summary: &str) -> Vec<(oyo_core::git::FileStatus, Option<PathBuf>, PathBuf)> {
    summary
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (status, path) = line.split_once(' ')?;
            let status = match status.chars().next().unwrap_or('M') {
                'A' => oyo_core::git::FileStatus::Added,
                'D' => oyo_core::git::FileStatus::Deleted,
                'R' => oyo_core::git::FileStatus::Renamed,
                _ => oyo_core::git::FileStatus::Modified,
            };
            let (old_path, path) = jj_summary_paths(status, path.trim());
            Some((status, old_path.map(PathBuf::from), PathBuf::from(path)))
        })
        .collect()
}

pub(crate) fn jj_diff_file_signature(
    repo_root: &Path,
    rev: &str,
) -> Result<Vec<(oyo_core::git::FileStatus, Option<PathBuf>, PathBuf)>> {
    let (summary, _, _) = jj_diff_summary(repo_root, rev, None)?;
    Ok(jj_summary_entries(&summary))
}

type JjBatchSides = (Vec<Option<Vec<u8>>>, Vec<Option<Vec<u8>>>);
type JjGitContent = (PathBuf, String, String);

fn jj_git_content(repo_root: &Path, old_rev: &str, new_rev: &str) -> Result<JjGitContent> {
    let git_root = PathBuf::from(run_jj(repo_root, &["git", "root"])?.trim());
    if [old_rev, new_rev].into_iter().any(|rev| {
        run_jj(
            repo_root,
            &["log", "--no-graph", "-r", rev, "-T", "conflict"],
        )
        .is_ok_and(|value| value.trim() == "true")
    }) {
        anyhow::bail!("jj conflict content needs file show");
    }
    let old_commit = run_jj(
        repo_root,
        &["log", "--no-graph", "-r", old_rev, "-T", "commit_id"],
    )?
    .trim()
    .to_string();
    let new_commit = run_jj(
        repo_root,
        &["log", "--no-graph", "-r", new_rev, "-T", "commit_id"],
    )?
    .trim()
    .to_string();
    Ok((git_root, old_commit, new_commit))
}

fn jj_git_batch_sides(
    git_content: &JjGitContent,
    entries: &[(oyo_core::git::FileStatus, Option<PathBuf>, PathBuf)],
) -> Result<JjBatchSides> {
    let (git_root, old_commit, new_commit) = git_content;
    let old_specs = entries
        .iter()
        .filter(|(status, _, _)| *status != oyo_core::git::FileStatus::Added)
        .map(|(_, old_path, path)| {
            format!(
                "{old_commit}:{}",
                old_path.as_deref().unwrap_or(path).display()
            )
        })
        .collect::<Vec<_>>();
    let new_specs = entries
        .iter()
        .filter(|(status, _, _)| *status != oyo_core::git::FileStatus::Deleted)
        .map(|(_, _, path)| format!("{new_commit}:{}", path.display()))
        .collect::<Vec<_>>();
    Ok((
        oyo_core::git::get_objects_batch(git_root, &old_specs, 32 * 1024 * 1024)?,
        oyo_core::git::get_objects_batch(git_root, &new_specs, 32 * 1024 * 1024)?,
    ))
}

fn build_jj_diff(repo_root: &Path, rev: &str, path: Option<&Path>) -> Result<MultiFileDiff> {
    let (summary, old_rev, new_rev) = jj_diff_summary(repo_root, rev, path)?;
    let mut entries = jj_summary_entries(&summary);
    if entries.is_empty() {
        if let Some(path) = path {
            entries.push((
                oyo_core::git::FileStatus::Modified,
                None,
                path.to_path_buf(),
            ));
        }
    }
    let git_content = jj_git_content(repo_root, &old_rev, &new_rev).ok();
    let colocated = repo_root.join(".git").exists();
    if entries.len() > 4 {
        let stats = git_content
            .as_ref()
            .and_then(|(git_root, old_commit, new_commit)| {
                oyo_core::git::get_diff_numstat(git_root, &[format!("{old_commit}..{new_commit}")])
                    .ok()
            });
        let pending = entries
            .into_iter()
            .map(|(status, old_path, path)| {
                let (old, new) = if let Some((git_root, old_commit, new_commit)) = &git_content {
                    let old = if status == oyo_core::git::FileStatus::Added {
                        oyo_core::multi::ContentSource::Empty
                    } else {
                        oyo_core::multi::ContentSource::GitObject {
                            repo_root: git_root.clone(),
                            spec: format!(
                                "{old_commit}:{}",
                                old_path.as_deref().unwrap_or(&path).display()
                            ),
                        }
                    };
                    let new = if status == oyo_core::git::FileStatus::Deleted {
                        oyo_core::multi::ContentSource::Empty
                    } else {
                        oyo_core::multi::ContentSource::GitObject {
                            repo_root: git_root.clone(),
                            spec: format!("{new_commit}:{}", path.display()),
                        }
                    };
                    (old, new)
                } else {
                    let old = if status == oyo_core::git::FileStatus::Added {
                        oyo_core::multi::ContentSource::Empty
                    } else {
                        oyo_core::multi::ContentSource::JjFile {
                            repo_root: repo_root.to_path_buf(),
                            revision: old_rev.clone(),
                            path: old_path.clone().unwrap_or_else(|| path.clone()),
                        }
                    };
                    let new = if status == oyo_core::git::FileStatus::Deleted {
                        oyo_core::multi::ContentSource::Empty
                    } else {
                        oyo_core::multi::ContentSource::JjFile {
                            repo_root: repo_root.to_path_buf(),
                            revision: new_rev.clone(),
                            path: path.clone(),
                        }
                    };
                    (old, new)
                };
                let (insertions, deletions) = stats
                    .as_ref()
                    .and_then(|stats| stats.get(&path).copied())
                    .unwrap_or((0, 0));
                let file = oyo_core::multi::FileEntry {
                    display_name: path.display().to_string(),
                    path,
                    old_path,
                    old_source_path: None,
                    new_source_path: None,
                    status,
                    insertions,
                    deletions,
                    binary: false,
                };
                (file, old, new)
            })
            .collect();
        let mut diff = MultiFileDiff::from_pending_files(
            Some(repo_root.to_path_buf()),
            pending,
            stats.is_some(),
        );
        if colocated {
            if let Some((_, old_commit, new_commit)) = git_content {
                let new_source = if rev.trim() == "@" {
                    oyo_core::multi::BlameSource::Worktree
                } else {
                    oyo_core::multi::BlameSource::Commit(new_commit)
                };
                diff.set_blame_sources(
                    oyo_core::multi::BlameSource::Commit(old_commit),
                    new_source,
                );
            }
        } else {
            diff.set_blame_sources(
                oyo_core::multi::BlameSource::JjRev(old_rev),
                oyo_core::multi::BlameSource::JjRev(new_rev),
            );
        }
        return Ok(diff);
    }
    let mut batched = git_content
        .as_ref()
        .and_then(|content| jj_git_batch_sides(content, &entries).ok())
        .map(|(old, new)| (old.into_iter(), new.into_iter()));
    let mut files = Vec::new();
    for (status, old_path, path) in entries {
        let (old_bytes, old_too_large) = if matches!(status, oyo_core::git::FileStatus::Added) {
            (Vec::new(), false)
        } else if let Some((old, _)) = batched.as_mut() {
            match old.next().flatten() {
                Some(bytes) => (bytes, false),
                None => (Vec::new(), true),
            }
        } else {
            (
                run_jj_bytes(
                    repo_root,
                    &[
                        "file",
                        "show",
                        "-r",
                        &old_rev,
                        old_path
                            .as_deref()
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .as_ref(),
                    ],
                )?,
                false,
            )
        };
        let (new_bytes, new_too_large) = if matches!(status, oyo_core::git::FileStatus::Deleted) {
            (Vec::new(), false)
        } else if let Some((_, new)) = batched.as_mut() {
            match new.next().flatten() {
                Some(bytes) => (bytes, false),
                None => (Vec::new(), true),
            }
        } else {
            (
                run_jj_bytes(
                    repo_root,
                    &[
                        "file",
                        "show",
                        "-r",
                        &new_rev,
                        path.to_string_lossy().as_ref(),
                    ],
                )?,
                false,
            )
        };
        let old_content = String::from_utf8_lossy(&old_bytes).to_string();
        let new_content = String::from_utf8_lossy(&new_bytes).to_string();
        let binary = old_too_large
            || new_too_large
            || old_content.contains('\0')
            || new_content.contains('\0');
        files.push(RawFileDiff {
            path: path.clone(),
            old_path,
            old_source_path: None,
            new_source_path: Some(repo_root.join(path)),
            status,
            old_content,
            new_content,
            binary,
        });
    }
    let mut diff = MultiFileDiff::from_raw_files(Some(repo_root.to_path_buf()), files);
    if colocated {
        if let Some((_, old_commit, new_commit)) = git_content {
            let new_source = if rev.trim() == "@" {
                oyo_core::multi::BlameSource::Worktree
            } else {
                oyo_core::multi::BlameSource::Commit(new_commit)
            };
            diff.set_blame_sources(oyo_core::multi::BlameSource::Commit(old_commit), new_source);
        }
    } else {
        diff.set_blame_sources(
            oyo_core::multi::BlameSource::JjRev(old_rev),
            oyo_core::multi::BlameSource::JjRev(new_rev),
        );
    }
    Ok(diff)
}

fn review_hash(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn current_review_workspace() -> Result<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(root) = jj_workspace_root(&cwd) {
        return Ok(root);
    }
    if oyo_core::git::is_git_repo(&cwd) {
        return oyo_core::git::get_repo_root(&cwd).context("Failed to get git repository root");
    }
    Ok(cwd)
}

fn git_revision_input_mode(revision: &str) -> Result<InputMode> {
    if revision.contains("..") {
        let (from, to) = parse_range(revision)?;
        Ok(InputMode::GitRange { from, to })
    } else {
        Ok(InputMode::GitRange {
            from: format!("{revision}^"),
            to: revision.to_string(),
        })
    }
}

fn git_branch_ref_exists(root: &Path, branch: &str) -> bool {
    [
        format!("refs/heads/{branch}"),
        format!("refs/remotes/{branch}"),
        format!("refs/remotes/origin/{branch}"),
    ]
    .iter()
    .any(|name| {
        ProcessCommand::new("git")
            .arg("-C")
            .arg(root)
            .arg("show-ref")
            .arg("--verify")
            .arg("--quiet")
            .arg(name)
            .status()
            .is_ok_and(|status| status.success())
    })
}

fn git_ref_input_mode(cwd: &Path, value: &str) -> Option<InputMode> {
    let root = oyo_core::git::get_repo_root(cwd).ok()?;
    if git_branch_ref_exists(&root, value) {
        let base = default_git_base_ref(&root, value).unwrap_or_else(|| format!("{value}^"));
        let from = git_merge_base_in(&root, &base, value).unwrap_or(base);
        return Some(InputMode::GitRange {
            from,
            to: value.to_string(),
        });
    }
    git_commit(&root, value).and_then(|_| git_revision_input_mode(value).ok())
}

fn default_git_base_ref(root: &Path, branch: &str) -> Option<String> {
    git_output(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .filter(|base| base != branch)
    .or_else(|| {
        ["main", "master", "origin/main", "origin/master"]
            .into_iter()
            .find(|base| *base != branch && git_commit(root, base).is_some())
            .map(str::to_string)
    })
}

fn jj_bookmark_revset(bookmark: &str) -> String {
    format!("trunk()..{bookmark}")
}

fn jj_bookmarks_for_rev(rev: &str) -> Vec<String> {
    let Ok(root) = current_review_workspace() else {
        return Vec::new();
    };
    if !is_jj_repo(&root) {
        return Vec::new();
    }
    run_jj(
        &root,
        &["log", "--no-graph", "-r", rev, "-T", "bookmarks ++ \"\\n\""],
    )
    .ok()
    .and_then(|output| output.lines().next().map(str::to_string))
    .map(|line| line.split_whitespace().map(str::to_string).collect())
    .unwrap_or_default()
}

fn jj_bookmark_exists(bookmark: &str) -> bool {
    let Ok(root) = current_review_workspace() else {
        return false;
    };
    if !is_jj_repo(&root) {
        return false;
    }
    run_jj(
        &root,
        &[
            "bookmark",
            "list",
            "--template",
            "name ++ \"\\n\"",
            bookmark,
        ],
    )
    .ok()
    .is_some_and(|output| output.lines().any(|line| line.trim() == bookmark))
}

fn jj_revset_resolves(revset: &str) -> bool {
    let Ok(root) = current_review_workspace() else {
        return false;
    };
    if !is_jj_repo(&root) {
        return false;
    }
    run_jj(
        &root,
        &[
            "log",
            "--no-graph",
            "-r",
            revset,
            "-T",
            "commit_id.shortest(1) ++ \"\\n\"",
        ],
    )
    .is_ok()
}

fn should_treat_as_jj_revision(path_exists: bool, value: &str) -> bool {
    value == "@"
        || value.contains("..")
        || jj_bookmark_exists(value)
        || (!path_exists && jj_revset_resolves(value))
}

fn default_jj_review_revision() -> String {
    let bookmarks = jj_bookmarks_for_rev("@");
    match bookmarks.as_slice() {
        [bookmark] => jj_bookmark_revset(bookmark),
        _ => "@".to_string(),
    }
}

fn default_jj_review_label() -> String {
    let bookmarks = jj_bookmarks_for_rev("@");
    match bookmarks.as_slice() {
        [bookmark] => format!("@  {bookmark}"),
        _ => "@".to_string(),
    }
}

fn input_mode_for_control_target(target: control::ControlTarget, args: &Args) -> Result<InputMode> {
    match target {
        control::ControlTarget::Revision(target) => review_input_mode(Some(&target), args),
        control::ControlTarget::Worktree => Ok(InputMode::GitUncommitted),
        control::ControlTarget::Staged => Ok(InputMode::GitStaged),
    }
}

fn review_input_mode(revision: Option<&str>, args: &Args) -> Result<InputMode> {
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(revision) = revision {
        if is_jj_repo(&cwd) {
            let rev = if revision != "@" && !revision.contains("..") && jj_bookmark_exists(revision)
            {
                jj_bookmark_revset(revision)
            } else {
                revision.to_string()
            };
            return Ok(InputMode::JjRevision { rev });
        }
        if oyo_core::git::is_git_repo(&cwd) {
            if revision == "@" {
                return Ok(InputMode::GitUncommitted);
            }
            if let Some(input_mode) = git_ref_input_mode(&cwd, revision) {
                return Ok(input_mode);
            }
        }
        return git_revision_input_mode(revision);
    }
    if args.worktree {
        return Ok(InputMode::GitUncommitted);
    }
    if args.staged {
        return Ok(InputMode::GitStaged);
    }
    if let Some(range) = args.range.as_deref() {
        let (from, to) = parse_range(range)?;
        return Ok(InputMode::GitRange { from, to });
    }
    if is_jj_repo(&cwd) {
        return Ok(InputMode::JjRevision {
            rev: default_jj_review_revision(),
        });
    }
    if oyo_core::git::is_git_repo(&cwd) {
        return Ok(InputMode::GitUncommitted);
    }
    Ok(detect_input_mode(&[]))
}

fn basic_review_target_metadata(label: impl Into<String>, vcs: &str) -> ReviewTargetMetadata {
    ReviewTargetMetadata {
        label: label.into(),
        vcs: vcs.to_string(),
        jj_change_id: None,
        jj_commit_id: None,
        git_base_ref: None,
        git_head_ref: None,
        git_base_commit: None,
        git_head_commit: None,
        branch: None,
        pr_provider: None,
        pr_repo: None,
        pr_number: None,
        author: None,
        timestamp: None,
        bookmarks: None,
    }
}

fn jj_target_metadata(label: &str, rev: &str) -> Option<ReviewTargetMetadata> {
    let root = current_review_workspace().ok()?;
    if !is_jj_repo(&root) {
        return None;
    }
    let output = ProcessCommand::new("jj")
        .arg("-R")
        .arg(root)
        .arg("--no-pager")
        .arg("--config")
        .arg("signing.behavior=\"drop\"")
        .arg("log")
        .arg("--no-graph")
        .arg("-r")
        .arg(rev)
        .arg("-T")
        .arg("change_id.shortest(8) ++ \"\\t\" ++ commit_id.shortest(8) ++ \"\\t\" ++ author.email() ++ \"\\t\" ++ committer.timestamp().format(\"%Y-%m-%d %H:%M:%S\") ++ \"\\t\" ++ bookmarks ++ \"\\n\"")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let data = String::from_utf8_lossy(&output.stdout);
    let parts = data.lines().next()?.split('\t').collect::<Vec<_>>();
    (parts.len() >= 5).then(|| {
        let mut metadata = basic_review_target_metadata(label, "jj");
        metadata.jj_change_id = Some(parts[0].to_string());
        metadata.jj_commit_id = Some(parts[1].to_string());
        metadata.author = Some(parts[2].to_string());
        metadata.timestamp = Some(parts[3].to_string());
        metadata.bookmarks = (!parts[4].trim().is_empty()).then(|| parts[4].to_string());
        metadata
    })
}

fn git_commit(root: &Path, rev: &str) -> Option<String> {
    if rev == INDEX_REF {
        return None;
    }
    let spec = format!("{rev}^{{commit}}");
    git_output(root, &["rev-parse", "--verify", &spec]).ok()
}

fn git_commit_author_time(root: &Path, commit: &str) -> (Option<String>, Option<String>) {
    git_output(
        root,
        &[
            "show",
            "-s",
            "--date=format:%Y-%m-%d %H:%M:%S",
            "--format=%ae%x09%cd",
            commit,
        ],
    )
    .ok()
    .and_then(|line| {
        let (author, timestamp) = line.split_once('\t')?;
        Some((Some(author.to_string()), Some(timestamp.to_string())))
    })
    .unwrap_or((None, None))
}

fn git_target_metadata_for_input_mode(input_mode: &InputMode) -> ReviewTargetMetadata {
    let cwd = std::env::current_dir().unwrap_or_default();
    let root = oyo_core::git::get_repo_root(&cwd).unwrap_or(cwd);
    let branch = oyo_core::git::get_current_branch(&root)
        .ok()
        .filter(|branch| branch != "HEAD");
    match input_mode {
        InputMode::GitRange { from, to } => {
            let mut metadata = basic_review_target_metadata(format!("{from}..{to}"), "git");
            metadata.git_base_ref = Some(from.clone());
            metadata.git_head_ref = Some(to.clone());
            metadata.git_base_commit = git_commit(&root, from);
            metadata.git_head_commit = git_commit(&root, to);
            metadata.branch = branch
                .filter(|branch| branch == to)
                .or_else(|| (!to.contains(['^', '~', ':']) && to != INDEX_REF).then(|| to.clone()));
            if let Some(commit) = metadata.git_head_commit.as_deref() {
                let (author, timestamp) = git_commit_author_time(&root, commit);
                metadata.author = author;
                metadata.timestamp = timestamp;
            }
            metadata
        }
        InputMode::GitStaged => {
            let mut metadata = basic_review_target_metadata("staged", "git");
            metadata.git_base_ref = Some("HEAD".to_string());
            metadata.git_head_ref = Some(INDEX_REF.to_string());
            metadata.git_base_commit = git_commit(&root, "HEAD");
            metadata.branch = branch;
            metadata
        }
        InputMode::GitUncommitted => {
            let mut metadata = basic_review_target_metadata("@", "git");
            metadata.git_base_ref = Some("HEAD".to_string());
            metadata.git_base_commit = git_commit(&root, "HEAD");
            metadata.branch = branch;
            metadata
        }
        InputMode::GitFile { path } => {
            let mut metadata = basic_review_target_metadata(path.display().to_string(), "git");
            metadata.git_base_ref = Some("HEAD".to_string());
            metadata.git_base_commit = git_commit(&root, "HEAD");
            metadata.branch = branch;
            metadata
        }
        _ => basic_review_target_metadata("current target", "git"),
    }
}

fn review_target_metadata_for_input_mode(input_mode: &InputMode) -> ReviewTargetMetadata {
    match input_mode {
        InputMode::JjRevision { rev } => {
            let label = rev.clone();
            jj_target_metadata(&label, rev)
                .unwrap_or_else(|| basic_review_target_metadata(label, "jj"))
        }
        InputMode::JjFile { path } => {
            let label = path.display().to_string();
            let rev = default_jj_review_revision();
            jj_target_metadata(&label, &rev)
                .unwrap_or_else(|| basic_review_target_metadata(label, "jj"))
        }
        _ => git_target_metadata_for_input_mode(input_mode),
    }
}

fn review_target_metadata(revision: Option<&str>, args: &Args) -> ReviewTargetMetadata {
    review_input_mode(revision, args)
        .map(|mode| review_target_metadata_for_input_mode(&mode))
        .unwrap_or_else(|_| {
            let vcs = if is_jj_repo(&std::env::current_dir().unwrap_or_default()) {
                "jj"
            } else {
                "git"
            };
            basic_review_target_metadata(review_target_label(revision, args), vcs)
        })
}

fn review_target_label(revision: Option<&str>, args: &Args) -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(revision) = revision {
        return revision.to_string();
    }
    if args.worktree {
        return "worktree".to_string();
    }
    if args.staged {
        return "staged".to_string();
    }
    if let Some(range) = args.range.as_deref() {
        return range.to_string();
    }
    if is_jj_repo(&cwd) {
        return default_jj_review_label();
    }
    if oyo_core::git::is_git_repo(&cwd) {
        return "@".to_string();
    }
    "current target".to_string()
}

fn configure_review_state_for_app(
    app: &mut App,
    config: &config::Config,
    args: &Args,
    workspace_root: Option<PathBuf>,
    input_mode: &InputMode,
    revision: Option<&str>,
    create: bool,
) -> Result<()> {
    apply_review_storage_to_app(app, config, args, workspace_root);
    let target_metadata = review_target_metadata_for_input_mode(input_mode);
    app.set_review_target_metadata(Some(target_metadata.clone()));
    app.set_review_persist_enabled(!args.no_review_persist);
    app.set_review_filter_to_current_diff(matches!(
        input_mode,
        InputMode::GitUncommitted
            | InputMode::GitStaged
            | InputMode::GitFile { .. }
            | InputMode::JjFile { .. }
    ));
    if create {
        app.enable_review_mode();
    } else {
        app.load_review_mode();
    }
    if let InputMode::JjRevision { rev } = input_mode {
        if rev.contains("..") {
            let fingerprints = saved_review_fingerprints_for_jj_revset(config, args, rev)?;
            if fingerprints.len() > 1 {
                app.load_review_snapshots_into_current_target(&fingerprints);
                return Ok(());
            }
        }
    }
    if app.review_comment_count() == 0 && should_load_saved_review_fallback(input_mode) {
        if let Some(fingerprint) = saved_review_fingerprint_for_app_target(
            config,
            args,
            input_mode,
            revision,
            &target_metadata,
        )? {
            app.load_review_snapshot_into_current_target(&fingerprint);
        }
    }
    Ok(())
}

fn should_load_saved_review_fallback(input_mode: &InputMode) -> bool {
    !matches!(input_mode, InputMode::JjRevision { rev } if rev == "@")
}

fn review_app_for_target(
    config: &config::Config,
    args: &Args,
    revision: Option<&str>,
    create: bool,
) -> Result<App> {
    let input_mode = review_input_mode(revision, args)?;
    let built = build_diff_from_input_mode(&input_mode, config, args)?
        .ok_or_else(|| anyhow!("No changes found."))?;
    let mut app = App::new(
        built.multi_diff,
        ViewMode::UnifiedPane,
        config.playback.speed,
        false,
        built.branch,
    );
    configure_review_state_for_app(
        &mut app,
        config,
        args,
        built.workspace_root,
        &input_mode,
        revision,
        create,
    )?;
    Ok(app)
}

fn path_json(path: Option<PathBuf>) -> serde_json::Value {
    path.map(|path| serde_json::Value::String(path.to_string_lossy().to_string()))
        .unwrap_or(serde_json::Value::Null)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewTargetSummary {
    review_key: String,
    label: String,
    pr: Option<u64>,
}

fn review_target_summary(app: &App) -> ReviewTargetSummary {
    let metadata = app.review_target_metadata();
    let pr = metadata.and_then(|target| target.pr_number);
    let label = match (metadata, pr) {
        (Some(target), Some(number)) => {
            let noun = target
                .pr_provider
                .as_deref()
                .and_then(ReviewProviderKind::from_id)
                .unwrap_or(ReviewProviderKind::GitHub)
                .short_review_noun();
            target
                .branch
                .as_deref()
                .or(target.git_head_ref.as_deref())
                .map(|branch| format!("{noun} #{number} ({branch})"))
                .unwrap_or_else(|| format!("{noun} #{number}"))
        }
        (Some(target), None) if target.label == "@" => "working tree".to_string(),
        (Some(target), None) if target.label == "staged" => "staged changes".to_string(),
        (Some(target), None) => target.label.clone(),
        (None, None) => "current target".to_string(),
        (None, Some(_)) => unreachable!(),
    };
    ReviewTargetSummary {
        review_key: app.review_storage_key().to_string(),
        label,
        pr,
    }
}

fn review_json_with_target(app: &App, mut value: serde_json::Value) -> serde_json::Value {
    let target = review_target_summary(app);
    if let Some(object) = value.as_object_mut() {
        object.insert("reviewKey".to_string(), target.review_key.into());
        object.insert("target".to_string(), target.label.clone().into());
        object.insert("label".to_string(), target.label.into());
        object.insert("pr".to_string(), target.pr.into());
    }
    value
}

fn print_review_target_header(app: &App) {
    let color = review_cli_color_enabled();
    let label = review_target_summary(app).label;
    println!("Reviewing: {}", review_cli_paint(color, "36", &label));
}

fn review_status_json_value(app: &App, filter: &ReviewCommentFilter) -> serde_json::Value {
    let paths = app.review_paths();
    let rows = app.review_status_comment_rows_filtered(filter);
    let comments = rows
        .iter()
        .map(|row| {
            let mut item = serde_json::json!({
                "id": row.id,
                "subject": row.subject,
                "location": row.location,
                "preview": row.preview,
                "createdAt": row.created_at,
                "updatedAt": row.updated_at,
                "resolved": row.resolved,
                "outdated": row.outdated,
                "deleted": row.deleted,
            });
            if filter.since.is_some() {
                item["changeType"] = serde_json::Value::String(
                    if row.deleted {
                        "removed"
                    } else if row.created_at == row.updated_at {
                        "added"
                    } else {
                        "updated"
                    }
                    .to_string(),
                );
            }
            item
        })
        .collect::<Vec<_>>();
    let comment_count = comments.len();
    review_json_with_target(
        app,
        serde_json::json!({
            "workspaceRoot": app.review_workspace_root().unwrap_or_default(),
            "diffFingerprint": app.review_diff_fingerprint(),
            "reviewDir": path_json(paths.review_dir),
            "reviewDb": path_json(paths.db_file),
            "commentCount": comment_count,
            "comments": comments,
        }),
    )
}

fn ensure_review_filter_ids(app: &App, filter: &ReviewCommentFilter) -> Result<()> {
    if let Some(id) = app.missing_review_filter_id(filter) {
        anyhow::bail!("No comment matches id {id}.");
    }
    Ok(())
}

fn print_review_status(app: &App, json: bool, filter: &ReviewCommentFilter) -> Result<()> {
    ensure_review_filter_ids(app, filter)?;
    let rows = app.review_status_comment_rows_filtered(filter);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&review_status_json_value(app, filter))?
        );
        return Ok(());
    }
    let color = review_cli_color_enabled();
    print_review_target_header(app);
    if rows.is_empty() {
        println!("{}", review_cli_paint(color, "32", "No review comments."));
    } else {
        println!("Review comments:");
        for row in rows {
            let id = review_cli_paint(color, "1;38;5;8", &format!("#{}", row.id));
            let subject = review_cli_paint(color, "36", &review_cli_truncate(&row.subject, 34));
            let location = review_cli_paint(color, "2", &row.location);
            let preview = review_cli_truncate(&row.preview, 72);
            println!("{id} {subject} {location}  {preview}");
        }
    }
    if app.review_session_has_changes() {
        println!("{}", review_cli_paint(color, "33", "local changes"));
    }
    Ok(())
}

fn review_cli_color_enabled() -> bool {
    io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").map_or(true, |term| term != "dumb")
}

fn review_cli_paint(enabled: bool, code: &str, text: &str) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn review_cli_colored_id(color: bool, value: &str, prefix_len: usize, prefix_code: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let prefix = value.chars().take(prefix_len.max(1)).collect::<String>();
    let rest = value.chars().skip(prefix_len.max(1)).collect::<String>();
    format!(
        "{}{}",
        review_cli_paint(color, prefix_code, &prefix),
        review_cli_paint(color, "1;38;5;8", &rest)
    )
}

fn review_cli_jj_id(color: bool, value: &str, first_code: &str) -> String {
    review_cli_colored_id(color, value, 1, first_code)
}

fn shortest_unique_prefix_len(value: &str, values: &[String]) -> usize {
    (1..=value.len())
        .find(|len| {
            let prefix = &value[..*len];
            values
                .iter()
                .filter(|candidate| candidate.starts_with(prefix))
                .count()
                == 1
        })
        .unwrap_or(value.len())
}

fn review_cli_truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return "...".chars().take(max_chars).collect();
    }
    let keep = max_chars.saturating_sub(3);
    format!("{}...", value.chars().take(keep).collect::<String>())
}

fn review_cli_target_value(color: bool, target: &str) -> String {
    let range = target
        .split_once("...")
        .map(|(from, to)| (from, "...", to))
        .or_else(|| target.split_once("..").map(|(from, to)| (from, "..", to)));
    if let Some((from, sep, to)) = range {
        return format!(
            "{}{}{}",
            review_cli_paint(color, "35", from),
            review_cli_paint(color, "2", sep),
            review_cli_paint(color, "35", to)
        );
    }
    review_cli_paint(color, "36", target)
}

fn review_comment_count_label(count: usize) -> String {
    match count {
        0 => "0 comments".to_string(),
        1 => "1 comment".to_string(),
        n => format!("{n} comments"),
    }
}

fn saved_review_fingerprint(
    config: &config::Config,
    args: &Args,
    target: &str,
) -> Result<Option<String>> {
    if target.is_empty() || target.contains("..") {
        return Ok(None);
    }
    let current = review_app_for_target(config, args, None, false)
        .ok()
        .map(|app| app.review_storage_key().to_string());
    let current_metadata = review_target_metadata(None, args);
    let mut matches = review_log_entries_for_scope(config, args)?
        .into_iter()
        .filter(|entry| entry["commentCount"].as_u64().unwrap_or(0) > 0)
        .filter(|entry| {
            review_log_entry_match_ids_for_current(entry, current.as_deref(), &current_metadata)
                .iter()
                .any(|id| id.starts_with(target))
        })
        .filter_map(|entry| entry["diffFingerprint"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [] => Ok(None),
        [fingerprint] => Ok(Some(fingerprint.clone())),
        _ => anyhow::bail!("Saved review id '{target}' is ambiguous"),
    }
}

fn metadata_matches_target(saved: &ReviewTargetMetadata, target: &ReviewTargetMetadata) -> bool {
    if saved.vcs != target.vcs {
        return false;
    }
    match saved.vcs.as_str() {
        "git" => match (&saved.git_head_commit, &target.git_head_commit) {
            (Some(saved_head), Some(target_head)) if saved_head == target_head => {
                saved.git_base_commit.is_none()
                    || target.git_base_commit.is_none()
                    || saved.git_base_commit == target.git_base_commit
            }
            _ => false,
        },
        "jj" => {
            saved
                .jj_change_id
                .as_ref()
                .zip(target.jj_change_id.as_ref())
                .is_some_and(|(saved_id, target_id)| saved_id == target_id)
                || saved
                    .jj_commit_id
                    .as_ref()
                    .zip(target.jj_commit_id.as_ref())
                    .is_some_and(|(saved_id, target_id)| saved_id == target_id)
        }
        _ => false,
    }
}

fn metadata_in_current_scope(saved: &ReviewTargetMetadata, target: &ReviewTargetMetadata) -> bool {
    if saved.vcs != target.vcs {
        return false;
    }
    match saved.vcs.as_str() {
        "git" => {
            saved
                .branch
                .as_ref()
                .zip(target.branch.as_ref())
                .is_some_and(|(saved_branch, target_branch)| saved_branch == target_branch)
                || saved
                    .git_head_ref
                    .as_ref()
                    .zip(target.git_head_ref.as_ref())
                    .is_some_and(|(saved_ref, target_ref)| saved_ref == target_ref)
                || saved
                    .git_head_commit
                    .as_ref()
                    .zip(target.git_head_commit.as_ref())
                    .is_some_and(|(saved_commit, target_commit)| saved_commit == target_commit)
        }
        "jj" => {
            saved
                .jj_change_id
                .as_ref()
                .zip(target.jj_change_id.as_ref())
                .is_some_and(|(saved_id, target_id)| saved_id == target_id)
                || saved
                    .jj_commit_id
                    .as_ref()
                    .zip(target.jj_commit_id.as_ref())
                    .is_some_and(|(saved_id, target_id)| saved_id == target_id)
        }
        _ => false,
    }
}

fn review_log_entries_for_scope(
    config: &config::Config,
    args: &Args,
) -> Result<Vec<serde_json::Value>> {
    let target = review_target_metadata(None, args);
    let current_mode = review_input_mode(None, args).ok();
    let current = review_app_for_target(config, args, None, false)
        .ok()
        .map(|app| app.review_storage_key().to_string());
    let current_diff_only = matches!(
        current_mode,
        Some(
            InputMode::GitUncommitted
                | InputMode::GitStaged
                | InputMode::GitFile { .. }
                | InputMode::JjFile { .. },
        )
    );
    let jj_stack_change_ids = match current_mode.as_ref() {
        Some(InputMode::JjRevision { rev }) if rev.contains("..") => {
            jj_revset_change_ids(rev).unwrap_or_default()
        }
        _ => Vec::new(),
    };
    let entries = review_log_entries(config, args)?
        .into_iter()
        .filter(|entry| {
            current.as_deref() == entry["diffFingerprint"].as_str()
                || (!current_diff_only
                    && review_log_entry_metadata(entry)
                        .as_ref()
                        .is_some_and(|metadata| {
                            metadata_in_current_scope(metadata, &target)
                                || metadata
                                    .jj_change_id
                                    .as_ref()
                                    .is_some_and(|id| jj_stack_change_ids.contains(id))
                        }))
        })
        .collect();
    Ok(dedupe_review_log_entries(
        entries,
        current.as_deref(),
        &target,
    ))
}

fn jj_revset_change_ids(revset: &str) -> Result<Vec<String>> {
    let root = current_review_workspace()?;
    if !is_jj_repo(&root) {
        return Ok(Vec::new());
    }
    let output = run_jj(
        &root,
        &[
            "log",
            "--no-graph",
            "-r",
            revset,
            "-T",
            "change_id.shortest(8) ++ \"\\n\"",
        ],
    )?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn saved_review_fingerprints_for_jj_revset(
    config: &config::Config,
    args: &Args,
    revset: &str,
) -> Result<Vec<String>> {
    let change_ids = jj_revset_change_ids(revset)?;
    if change_ids.len() <= 1 {
        return Ok(Vec::new());
    }
    let order = change_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut latest = BTreeMap::<String, (u64, String)>::new();
    for entry in review_log_entries(config, args)? {
        if entry["commentCount"].as_u64().unwrap_or(0) == 0 {
            continue;
        }
        let Some(metadata) = review_log_entry_metadata(&entry) else {
            continue;
        };
        if metadata.vcs != "jj" {
            continue;
        }
        let Some(change_id) = metadata.jj_change_id else {
            continue;
        };
        if !order.contains_key(&change_id) {
            continue;
        }
        let Some(fingerprint) = entry["diffFingerprint"].as_str() else {
            continue;
        };
        let updated = entry["updatedAt"].as_u64().unwrap_or(0);
        let replace = latest
            .get(&change_id)
            .is_none_or(|(seen_updated, _)| updated > *seen_updated);
        if replace {
            latest.insert(change_id, (updated, fingerprint.to_string()));
        }
    }
    let mut rows = latest
        .into_iter()
        .filter_map(|(change_id, (_updated, fingerprint))| {
            order.get(&change_id).map(|index| (*index, fingerprint))
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|(index, _)| *index);
    Ok(rows
        .into_iter()
        .map(|(_, fingerprint)| fingerprint)
        .collect())
}

fn saved_review_fingerprint_for_app_target(
    config: &config::Config,
    args: &Args,
    input_mode: &InputMode,
    revision: Option<&str>,
    target_metadata: &ReviewTargetMetadata,
) -> Result<Option<String>> {
    Ok(review_log_entries(config, args)?
        .into_iter()
        .filter(|entry| entry["commentCount"].as_u64().unwrap_or(0) > 0)
        .filter(|entry| {
            review_log_entry_metadata(entry)
                .as_ref()
                .is_some_and(|metadata| {
                    if revision.is_some() {
                        metadata_matches_target(metadata, target_metadata)
                    } else if matches!(
                        input_mode,
                        InputMode::GitUncommitted
                            | InputMode::GitStaged
                            | InputMode::GitFile { .. }
                            | InputMode::JjFile { .. }
                    ) {
                        false
                    } else {
                        metadata_in_current_scope(metadata, target_metadata)
                    }
                })
        })
        .filter_map(|entry| entry["diffFingerprint"].as_str().map(str::to_string))
        .next())
}

fn saved_review_fingerprint_for_target(
    config: &config::Config,
    args: &Args,
    target: &str,
) -> Result<Option<String>> {
    let target_metadata = review_target_metadata(Some(target), args);
    Ok(review_log_entries(config, args)?
        .into_iter()
        .filter(|entry| entry["commentCount"].as_u64().unwrap_or(0) > 0)
        .filter(|entry| {
            review_log_entry_metadata(entry)
                .as_ref()
                .is_some_and(|metadata| metadata_matches_target(metadata, &target_metadata))
        })
        .filter_map(|entry| entry["diffFingerprint"].as_str().map(str::to_string))
        .next())
}

fn load_jj_revset_snapshots_for_read(
    app: &mut App,
    config: &config::Config,
    args: &Args,
    target: Option<&str>,
) -> Result<()> {
    let Some(target) = target.filter(|target| target.contains("..")) else {
        return Ok(());
    };
    let fingerprints = saved_review_fingerprints_for_jj_revset(config, args, target)?;
    if fingerprints.len() > 1 {
        app.load_review_snapshots_into_current_target(&fingerprints);
    }
    Ok(())
}

fn local_pr_review_revision(entries: &[serde_json::Value], branch: &str) -> Option<String> {
    entries.iter().find_map(|entry| {
        let metadata = review_log_entry_metadata(entry)?;
        metadata.pr_number?;
        let matches_branch = metadata.branch.as_deref() == Some(branch)
            || metadata.git_head_ref.as_deref() == Some(branch);
        if metadata.vcs != "git" || !matches_branch {
            return None;
        }
        let base = metadata.git_base_commit.or(metadata.git_base_ref)?;
        let head = metadata.git_head_ref.or(metadata.branch)?;
        Some(format!("{base}...{head}"))
    })
}

fn resolve_review_target(
    config: &config::Config,
    args: &Args,
    explicit: Option<&str>,
) -> Result<Option<String>> {
    if let Some(target) = explicit {
        return Ok(Some(target.to_string()));
    }
    if args.worktree || args.staged || args.range.is_some() {
        return Ok(None);
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    if is_jj_repo(&cwd) || !oyo_core::git::is_git_repo(&cwd) {
        return Ok(None);
    }
    let root = oyo_core::git::get_repo_root(&cwd)?;
    let Some(branch) = oyo_core::git::get_current_branch(&root)
        .ok()
        .filter(|branch| branch != "HEAD" && !branch.is_empty())
    else {
        return Ok(None);
    };
    let target = local_pr_review_revision(&review_log_entries(config, args)?, &branch);
    let Some(target) = target else {
        return Ok(None);
    };
    let valid = parse_range(&target).is_ok_and(|(base, head)| {
        git_commit(&root, &base).is_some() && git_commit(&root, &head).is_some()
    });
    if valid {
        Ok(Some(target))
    } else {
        eprintln!(
            "Notice: the saved PR review for branch '{branch}' is unavailable; using the working tree."
        );
        Ok(None)
    }
}

fn combine_review_targets(targets: &[Option<&str>]) -> Result<Option<String>> {
    let targets = targets.iter().flatten().copied().collect::<Vec<_>>();
    match targets.as_slice() {
        [] => Ok(None),
        [target] => Ok(Some((*target).to_string())),
        _ => anyhow::bail!("Pass the review target once, using -t/--target or a positional target"),
    }
}

fn review_app_for_comment_target(
    config: &config::Config,
    args: &Args,
    target: Option<&str>,
) -> Result<App> {
    let mut app = review_app_for_target(config, args, None, false)?;
    let Some(target) = target else {
        return Ok(app);
    };
    if target == "@" {
        return review_app_for_target(config, args, Some("@"), false);
    }
    if is_jj_repo(&std::env::current_dir().unwrap_or_default())
        && !target.contains("..")
        && jj_bookmark_exists(target)
    {
        return review_app_for_target(config, args, Some(target), false);
    }
    if target.contains("..") {
        let mut app = review_app_for_target(config, args, Some(target), false)?;
        load_jj_revset_snapshots_for_read(&mut app, config, args, Some(target))?;
        return Ok(app);
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    if oyo_core::git::is_git_repo(&cwd) && git_ref_input_mode(&cwd, target).is_some() {
        return review_app_for_target(config, args, Some(target), false);
    }
    if let Some(fingerprint) = saved_review_fingerprint(config, args, target)? {
        if app.load_review_by_fingerprint(&fingerprint) {
            return Ok(app);
        }
    }
    if let Some(fingerprint) = saved_review_fingerprint_for_target(config, args, target)? {
        if app.load_review_by_fingerprint(&fingerprint) {
            return Ok(app);
        }
    }
    review_app_for_target(config, args, Some(target), false)
}

fn review_comments_json_value(
    app: &App,
    filter: &ReviewCommentFilter,
) -> Result<serde_json::Value> {
    let value = serde_json::from_str(&app.review_comments_json_filtered(filter))?;
    Ok(review_json_with_target(app, value))
}

fn print_review_comments(app: &App, json: bool, filter: &ReviewCommentFilter) -> Result<()> {
    ensure_review_filter_ids(app, filter)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&review_comments_json_value(app, filter)?)?
        );
        return Ok(());
    }
    let color = review_cli_color_enabled();
    print_review_target_header(app);
    let markdown = app.review_markdown_filtered_colored(filter, color);
    if markdown.trim().is_empty() {
        println!("{}", review_cli_paint(color, "32", "No review comments."));
    } else {
        println!("{markdown}");
    }
    Ok(())
}

fn write_export_output(output: Option<&Path>, data: &str) -> Result<()> {
    if let Some(path) = output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, data)?;
    } else if data.ends_with('\n') {
        print!("{data}");
    } else {
        println!("{data}");
    }
    Ok(())
}

fn export_review(app: &App, format: ReviewExportFormat, output: Option<&Path>) -> Result<()> {
    let data = match format {
        ReviewExportFormat::Json => serde_json::to_string_pretty(&review_json_with_target(
            app,
            serde_json::from_str(&app.review_comments_json())?,
        ))?,
        ReviewExportFormat::Markdown => app.review_markdown(),
    };
    write_export_output(output, &data)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewProviderKind {
    GitHub,
    GitLab,
    Forgejo,
}

impl ReviewProviderKind {
    fn id(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
            Self::Forgejo => "forgejo",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        match id {
            "github" => Some(Self::GitHub),
            "gitlab" => Some(Self::GitLab),
            "forgejo" => Some(Self::Forgejo),
            _ => None,
        }
    }

    pub(crate) fn short_review_noun(self) -> &'static str {
        match self {
            Self::GitLab => "MR",
            Self::GitHub | Self::Forgejo => "PR",
        }
    }

    pub(crate) fn long_review_noun_title(self) -> &'static str {
        match self {
            Self::GitLab => "Merge request",
            Self::GitHub | Self::Forgejo => "Pull request",
        }
    }

    pub(crate) fn long_review_noun(self) -> &'static str {
        match self {
            Self::GitLab => "merge request",
            Self::GitHub | Self::Forgejo => "pull request",
        }
    }
}

#[derive(Debug, Clone)]
struct ReviewRemote {
    name: String,
    provider: ReviewProviderKind,
    host: String,
    repo: String,
}

#[derive(Debug, Clone)]
struct ProviderPr {
    provider: ReviewProviderKind,
    remote: String,
    host: String,
    repo: String,
    number: u64,
    title: String,
    url: String,
    base_branch: String,
    head_branch: String,
    base_commit: String,
    start_commit: Option<String>,
    head_commit: String,
}

#[derive(Debug, Clone)]
struct ProviderUser {
    login: String,
    avatar_url: Option<String>,
}

fn review_pull_request_target(pr: &ProviderPr) -> ReviewPullRequestTarget {
    ReviewPullRequestTarget {
        provider: pr.provider.id().to_string(),
        remote: pr.remote.clone(),
        repo: pr.repo.clone(),
        number: pr.number,
        title: pr.title.clone(),
    }
}

fn review_pr_target_metadata(pr: &ProviderPr) -> ReviewTargetMetadata {
    let mut metadata =
        basic_review_target_metadata(format!("{}#{}", pr.provider.id(), pr.number), "git");
    metadata.git_base_ref = Some(pr.base_branch.clone());
    metadata.git_head_ref = Some(pr.head_branch.clone());
    metadata.git_base_commit = Some(pr.base_commit.clone());
    metadata.git_head_commit = Some(pr.head_commit.clone());
    metadata.branch = Some(pr.head_branch.clone());
    metadata.pr_provider = Some(pr.provider.id().to_string());
    metadata.pr_repo = Some(pr.repo.clone());
    metadata.pr_number = Some(pr.number);
    metadata
}

#[derive(Debug, Clone, Deserialize)]
struct GhUser {
    login: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    title: String,
    url: String,
    base_ref_name: String,
    head_ref_name: String,
    base_ref_oid: String,
    head_ref_oid: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GhCommentUser {
    login: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhComment {
    id: u64,
    body: String,
    path: String,
    #[serde(default)]
    line: Option<usize>,
    #[serde(default)]
    original_line: Option<usize>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    in_reply_to_id: Option<u64>,
    user: GhCommentUser,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone)]
struct GhProviderComment {
    comment: GhComment,
    thread_id: Option<String>,
    thread_resolved: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GhReviewThreadState {
    thread_id: String,
    resolved: bool,
}

#[derive(Debug, Deserialize)]
struct GhReviewThreadsResponse {
    data: GhReviewThreadsData,
}

#[derive(Debug, Deserialize)]
struct GhReviewThreadsData {
    repository: GhReviewThreadsRepository,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhReviewThreadsRepository {
    pull_request: GhReviewThreadsPullRequest,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhReviewThreadsPullRequest {
    review_threads: GhReviewThreadConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhReviewThreadConnection {
    #[serde(default)]
    nodes: Vec<GhReviewThread>,
    page_info: GhPageInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhReviewThread {
    id: String,
    is_resolved: bool,
    comments: GhReviewThreadComments,
}

#[derive(Debug, Deserialize)]
struct GhReviewThreadComments {
    #[serde(default)]
    nodes: Vec<GhReviewThreadComment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhReviewThreadComment {
    full_database_id: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhIssueComment {
    id: u64,
    body: String,
    user: GhCommentUser,
    created_at: String,
    updated_at: String,
}

fn run_output(mut command: ProcessCommand) -> Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("command exited with status {}", output.status)
        } else {
            stderr
        };
        anyhow::bail!(message);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let mut command = ProcessCommand::new("git");
    command.arg("-C").arg(root).args(args);
    run_output(command)
}

fn git_remotes(root: &Path) -> Vec<String> {
    git_output(root, &["remote"])
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn default_review_remote(root: &Path) -> Result<String> {
    if let Ok(upstream) = git_output(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    ) {
        if let Some((remote, _)) = upstream.split_once('/') {
            if !remote.is_empty() {
                return Ok(remote.to_string());
            }
        }
    }
    let remotes = git_remotes(root);
    if remotes.iter().any(|remote| remote == "origin") {
        return Ok("origin".to_string());
    }
    remotes
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No Git remote found."))
}

fn parse_remote_url(url: &str) -> Option<(String, String)> {
    let (host, rest) = if let Some(rest) = url.strip_prefix("git@") {
        let (host, rest) = rest.split_once(':')?;
        (host, rest)
    } else if let Some(rest) = url.strip_prefix("ssh://git@") {
        rest.split_once('/')?
    } else if let Some(rest) = url.strip_prefix("https://") {
        rest.split_once('/')?
    } else {
        url.strip_prefix("http://")?.split_once('/')?
    };
    let repo = rest.trim_end_matches(".git").trim_matches('/');
    let mut parts = repo.split('/');
    parts.next()?;
    parts.next()?;
    Some((host.to_string(), repo.to_string()))
}

fn review_remote(root: &Path, remote: Option<&str>) -> Result<ReviewRemote> {
    let name = match remote {
        Some(remote) => remote.to_string(),
        None => default_review_remote(root)?,
    };
    let url = git_output(root, &["remote", "get-url", &name])?;
    let Some((host, repo)) = parse_remote_url(&url) else {
        anyhow::bail!("Unsupported review remote URL: {url}");
    };
    let provider = review_provider_for_host(&host)
        .ok_or_else(|| anyhow!("Unsupported or unauthenticated review provider: {host}"))?;
    Ok(ReviewRemote {
        name,
        provider,
        host,
        repo,
    })
}

fn review_provider_for_host(host: &str) -> Option<ReviewProviderKind> {
    match host {
        "github.com" => Some(ReviewProviderKind::GitHub),
        "gitlab.com" => Some(ReviewProviderKind::GitLab),
        "codeberg.org" => Some(ReviewProviderKind::Forgejo),
        _ => {
            let forgejo_configured = forgejo::has_forgejo_token(host);
            let gitlab_configured = !forgejo_configured && glab_host_configured(host);
            review_provider_for_configured_host(host, forgejo_configured, gitlab_configured)
        }
    }
}

fn review_provider_for_configured_host(
    host: &str,
    forgejo_configured: bool,
    gitlab_configured: bool,
) -> Option<ReviewProviderKind> {
    match host {
        "github.com" => Some(ReviewProviderKind::GitHub),
        "gitlab.com" => Some(ReviewProviderKind::GitLab),
        "codeberg.org" => Some(ReviewProviderKind::Forgejo),
        _ if forgejo_configured => Some(ReviewProviderKind::Forgejo),
        _ if gitlab_configured => Some(ReviewProviderKind::GitLab),
        _ => None,
    }
}

fn glab_host_configured(host: &str) -> bool {
    let mut command = ProcessCommand::new("glab");
    command
        .arg("auth")
        .arg("status")
        .arg("--hostname")
        .arg(host)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.status().is_ok_and(|status| status.success())
}

fn parse_sync_args(root: &Path, items: &[String]) -> Result<(Option<String>, String)> {
    let remotes = git_remotes(root);
    match items {
        [] => Ok((None, default_review_remote(root)?)),
        [one] if remotes.iter().any(|remote| remote == one) => Ok((None, one.clone())),
        [one] => Ok((Some(one.clone()), default_review_remote(root)?)),
        [target, remote] => Ok((Some(target.clone()), remote.clone())),
        _ => anyhow::bail!("Usage: oy review push [target] [remote]"),
    }
}

fn gh_json<T: for<'de> Deserialize<'de>>(args: &[&str]) -> Result<T> {
    let mut command = ProcessCommand::new("gh");
    command.args(args);
    let data = run_output(command)?;
    serde_json::from_str(&data).map_err(|error| anyhow!(error))
}

fn gh_whoami() -> Result<ProviderUser> {
    let user: GhUser = gh_json(&["api", "user"])?;
    if let Some(url) = user.avatar_url.as_deref() {
        let _ = crate::avatars::cache_avatar_url(url);
    }
    Ok(ProviderUser {
        login: user.login,
        avatar_url: user.avatar_url,
    })
}

fn gh_pr(remote: &ReviewRemote, target: Option<&str>) -> Result<ProviderPr> {
    let mut args = vec![
        "pr",
        "view",
        "--repo",
        &remote.repo,
        "--json",
        "number,title,url,baseRefName,headRefName,baseRefOid,headRefOid",
    ];
    if let Some(target) = target {
        args.insert(2, target);
    }
    let pr: GhPr = gh_json(&args).with_context(|| {
        format!(
            "No pull request found for {} in {}.",
            target.unwrap_or("the current branch"),
            remote.repo
        )
    })?;
    Ok(ProviderPr {
        provider: remote.provider,
        remote: remote.name.clone(),
        host: remote.host.clone(),
        repo: remote.repo.clone(),
        number: pr.number,
        title: pr.title,
        url: pr.url,
        base_branch: pr.base_ref_name,
        head_branch: pr.head_ref_name,
        base_commit: pr.base_ref_oid,
        start_commit: None,
        head_commit: pr.head_ref_oid,
    })
}

fn gh_comments(pr: &ProviderPr) -> Result<Vec<GhComment>> {
    let endpoint = format!("repos/{}/pulls/{}/comments", pr.repo, pr.number);
    gh_json(&["api", &endpoint, "--paginate"])
}

fn insert_review_thread_states(
    states: &mut BTreeMap<u64, GhReviewThreadState>,
    threads: Vec<GhReviewThread>,
) {
    for thread in threads {
        for comment in thread.comments.nodes {
            let comment_id = comment.full_database_id.as_ref().and_then(|id| {
                id.as_u64()
                    .or_else(|| id.as_str().and_then(|id| id.parse().ok()))
            });
            if let Some(comment_id) = comment_id {
                states.insert(
                    comment_id,
                    GhReviewThreadState {
                        thread_id: thread.id.clone(),
                        resolved: thread.is_resolved,
                    },
                );
            }
        }
    }
}

fn gh_review_thread_states(pr: &ProviderPr) -> Result<BTreeMap<u64, GhReviewThreadState>> {
    const QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $endCursor) {
        nodes {
          id
          isResolved
          comments(first: 100) { nodes { fullDatabaseId } }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;
    let (owner, name) = pr
        .repo
        .split_once('/')
        .ok_or_else(|| anyhow!("GitHub repository must use owner/name format."))?;
    let query = format!("query={QUERY}");
    let owner = format!("owner={owner}");
    let name = format!("name={name}");
    let number = format!("number={}", pr.number);
    let mut end_cursor = None;
    let mut states = BTreeMap::new();
    loop {
        let mut args = vec![
            "api",
            "graphql",
            "-f",
            query.as_str(),
            "-f",
            owner.as_str(),
            "-f",
            name.as_str(),
            "-F",
            number.as_str(),
        ];
        let cursor_arg = end_cursor
            .as_ref()
            .map(|cursor| format!("endCursor={cursor}"));
        if let Some(cursor) = cursor_arg.as_deref() {
            args.extend(["-f", cursor]);
        }
        let response: GhReviewThreadsResponse = gh_json(&args)?;
        let threads = response.data.repository.pull_request.review_threads;
        insert_review_thread_states(&mut states, threads.nodes);
        if !threads.page_info.has_next_page {
            break;
        }
        end_cursor = threads.page_info.end_cursor;
        if end_cursor.is_none() {
            anyhow::bail!("GitHub review thread pagination returned no cursor.");
        }
    }
    Ok(states)
}

fn gh_issue_comments(pr: &ProviderPr) -> Result<Vec<GhIssueComment>> {
    let endpoint = format!("repos/{}/issues/{}/comments", pr.repo, pr.number);
    gh_json(&["api", &endpoint, "--paginate"])
}

fn add_login(value: &serde_json::Value, users: &mut BTreeSet<String>) {
    if let Some(login) = value.get("login").and_then(serde_json::Value::as_str) {
        users.insert(login.to_string());
    }
}

fn github_conversation_comment_users(
    pr: &ProviderPr,
    current_login: &str,
) -> Result<BTreeSet<String>> {
    let value: serde_json::Value = gh_json(&[
        "pr",
        "view",
        &pr.number.to_string(),
        "--repo",
        &pr.repo,
        "--json",
        "author,reviewRequests,reviews",
    ])?;
    let mut users = BTreeSet::from([current_login.to_string()]);
    add_login(&value["author"], &mut users);
    if let Some(reviews) = value["reviews"].as_array() {
        for review in reviews {
            add_login(&review["author"], &mut users);
        }
    }
    if let Some(requests) = value["reviewRequests"].as_array() {
        for request in requests {
            add_login(&request["requestedReviewer"], &mut users);
        }
    }
    Ok(users)
}

fn parse_github_time(value: &str) -> u64 {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .and_then(|date| u64::try_from(date.unix_timestamp()).ok())
        .unwrap_or(0)
}

fn provider_revision(target: Option<&str>, pr: &ProviderPr) -> String {
    target
        .filter(|target| target.contains(".."))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}...{}", pr.base_branch, pr.head_branch))
}

fn github_issue_comment_to_review_comment(
    app: &App,
    pr: &ProviderPr,
    current_login: &str,
    comment: GhIssueComment,
) -> Result<ReviewComment> {
    let avatar_url = comment.user.avatar_url.clone();
    if let Some(url) = avatar_url.as_deref() {
        let _ = crate::avatars::cache_avatar_url(url);
    }
    let login = comment.user.login;
    let provider_id = pr.provider.id();
    let mut usernames = BTreeMap::new();
    usernames.insert(provider_id.to_string(), login.clone());
    let data = serde_json::json!({
        "version": 1,
        "comments": [{
            "file": pr.title.clone(),
            "kind": "pr",
            "author": {
                "name": login.clone(),
                "usernames": usernames,
                "avatarUrl": avatar_url
            },
            "canEdit": login == current_login,
            "provider": {
                "provider": provider_id,
                "remote": pr.remote.clone(),
                "repo": pr.repo.clone(),
                "prNumber": pr.number,
                "commentId": comment.id.to_string(),
                "authorUsername": login,
                "prTitle": pr.title.clone(),
                "apiKind": "issue",
                "syncState": "clean"
            },
            "createdAt": parse_github_time(&comment.created_at),
            "updatedAt": parse_github_time(&comment.updated_at),
            "body": comment.body
        }]
    });
    let comments = app
        .parse_review_comments_json_for_sync(&data.to_string())
        .map_err(|error| anyhow!(error))?;
    comments
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Provider comment did not map to this diff."))
}

fn github_comment_to_review_comment(
    app: &App,
    pr: &ProviderPr,
    current_login: &str,
    provider_comment: GhProviderComment,
) -> Result<ReviewComment> {
    let GhProviderComment {
        comment,
        thread_id,
        thread_resolved,
    } = provider_comment;
    let side = match comment.side.as_deref() {
        Some("LEFT") => Some(ReviewSide::Old),
        _ => Some(ReviewSide::New),
    };
    let line = match side {
        Some(ReviewSide::Old) => comment.original_line.or(comment.line),
        _ => comment.line.or(comment.original_line),
    }
    .ok_or_else(|| anyhow!("Skipped provider comment without a line."))?;
    let (old_range, new_range) = match side {
        Some(ReviewSide::Old) => (
            Some(ReviewRange {
                start: line,
                end: line,
            }),
            None,
        ),
        _ => (
            None,
            Some(ReviewRange {
                start: line,
                end: line,
            }),
        ),
    };
    let avatar_url = comment.user.avatar_url.clone();
    if let Some(url) = avatar_url.as_deref() {
        let _ = crate::avatars::cache_avatar_url(url);
    }
    let login = comment.user.login;
    let provider_id = pr.provider.id();
    let mut usernames = BTreeMap::new();
    usernames.insert(provider_id.to_string(), login.clone());
    let data = serde_json::json!({
        "version": 1,
        "comments": [{
            "file": comment.path,
            "kind": "line",
            "side": side.map(|side| side.as_str()),
            "oldRange": old_range,
            "newRange": new_range,
            "author": {
                "name": login.clone(),
                "usernames": usernames,
                "avatarUrl": avatar_url
            },
            "canEdit": login == current_login,
            "provider": {
                "provider": provider_id,
                "remote": pr.remote.clone(),
                "repo": pr.repo.clone(),
                "prNumber": pr.number,
                "commentId": comment.id.to_string(),
                "inReplyToId": comment.in_reply_to_id.map(|id| id.to_string()),
                "threadId": thread_id,
                "threadResolved": thread_resolved,
                "authorUsername": login,
                "prTitle": pr.title.clone(),
                "apiKind": "review",
                "syncState": "clean"
            },
            "createdAt": parse_github_time(&comment.created_at),
            "updatedAt": parse_github_time(&comment.updated_at),
            "resolved": thread_resolved.unwrap_or(false),
            "body": comment.body
        }]
    });
    let comments = app
        .parse_review_comments_json_for_sync(&data.to_string())
        .map_err(|error| anyhow!(error))?;
    comments
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Provider comment did not map to this diff."))
}

fn run_output_with_stdin(mut command: ProcessCommand, stdin_data: &str) -> Result<String> {
    use std::io::Write;
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_data.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("command exited with status {}", output.status)
        } else {
            stderr
        };
        anyhow::bail!(message);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn gh_api_json<T: for<'de> Deserialize<'de>>(
    method: &str,
    endpoint: &str,
    body: serde_json::Value,
) -> Result<T> {
    let mut command = ProcessCommand::new("gh");
    command
        .arg("api")
        .arg("-X")
        .arg(method)
        .arg(endpoint)
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("--input")
        .arg("-");
    let data = run_output_with_stdin(command, &body.to_string())?;
    serde_json::from_str(&data).map_err(|error| anyhow!(error))
}

fn gh_api_no_output(method: &str, endpoint: &str) -> Result<()> {
    let mut command = ProcessCommand::new("gh");
    command.arg("api").arg("-X").arg(method).arg(endpoint);
    run_output(command).map(|_| ())
}

fn github_side(side: Option<ReviewSide>) -> &'static str {
    match side {
        Some(ReviewSide::Old) => "LEFT",
        _ => "RIGHT",
    }
}

fn github_comment_body(pr: &ProviderPr, comment: &ReviewComment) -> Result<serde_json::Value> {
    let anchor = &comment.anchor;
    let mut body = serde_json::json!({
        "body": comment.body,
        "commit_id": pr.head_commit,
        "path": anchor.file_path,
    });
    match anchor.kind {
        ReviewTargetKind::PullRequest => {
            anyhow::bail!("Pull request comments use the issue comment endpoint");
        }
        ReviewTargetKind::File => {
            body["subject_type"] = serde_json::Value::String("file".to_string());
        }
        ReviewTargetKind::Line | ReviewTargetKind::Hunk => {
            let side = anchor.side.unwrap_or(ReviewSide::New);
            let line = match side {
                ReviewSide::Old => anchor.old_range,
                ReviewSide::New => anchor.new_range,
            }
            .or(anchor.new_range)
            .or(anchor.old_range)
            .map(|range| range.end)
            .ok_or_else(|| anyhow!("Comment {} has no line anchor", comment.id))?;
            body["side"] = serde_json::Value::String(github_side(Some(side)).to_string());
            body["line"] = serde_json::Value::Number(serde_json::Number::from(line));
        }
    }
    Ok(body)
}

fn github_create_comment(pr: &ProviderPr, comment: &ReviewComment) -> Result<GhComment> {
    let endpoint = format!("repos/{}/pulls/{}/comments", pr.repo, pr.number);
    gh_api_json("POST", &endpoint, github_comment_body(pr, comment)?)
}

fn github_create_reply(pr: &ProviderPr, parent_comment_id: &str, body: &str) -> Result<GhComment> {
    let endpoint = format!(
        "repos/{}/pulls/{}/comments/{parent_comment_id}/replies",
        pr.repo, pr.number
    );
    gh_api_json("POST", &endpoint, serde_json::json!({ "body": body }))
}

fn github_update_comment(pr: &ProviderPr, comment_id: &str, body: &str) -> Result<GhComment> {
    let endpoint = format!("repos/{}/pulls/comments/{comment_id}", pr.repo);
    gh_api_json("PATCH", &endpoint, serde_json::json!({ "body": body }))
}

fn ignore_github_delete_not_found(result: Result<()>) -> Result<()> {
    match result {
        Err(error) if error.to_string().to_ascii_lowercase().contains("http 404") => Ok(()),
        result => result,
    }
}

fn github_delete_comment(pr: &ProviderPr, comment_id: &str) -> Result<()> {
    let endpoint = format!("repos/{}/pulls/comments/{comment_id}", pr.repo);
    ignore_github_delete_not_found(gh_api_no_output("DELETE", &endpoint))
}

fn github_create_issue_comment(pr: &ProviderPr, body: &str) -> Result<GhIssueComment> {
    let endpoint = format!("repos/{}/issues/{}/comments", pr.repo, pr.number);
    gh_api_json("POST", &endpoint, serde_json::json!({ "body": body }))
}

fn github_update_issue_comment(
    pr: &ProviderPr,
    comment_id: &str,
    body: &str,
) -> Result<GhIssueComment> {
    let endpoint = format!("repos/{}/issues/comments/{comment_id}", pr.repo);
    gh_api_json("PATCH", &endpoint, serde_json::json!({ "body": body }))
}

fn github_delete_issue_comment(pr: &ProviderPr, comment_id: &str) -> Result<()> {
    let endpoint = format!("repos/{}/issues/comments/{comment_id}", pr.repo);
    ignore_github_delete_not_found(gh_api_no_output("DELETE", &endpoint))
}

fn github_set_review_thread_resolved(thread_id: &str, resolved: bool) -> Result<()> {
    let mutation = if resolved {
        "resolveReviewThread"
    } else {
        "unresolveReviewThread"
    };
    let query = format!(
        "query=mutation($threadId: ID!) {{ {mutation}(input: {{threadId: $threadId}}) {{ thread {{ id isResolved }} }} }}"
    );
    let thread_id = format!("threadId={thread_id}");
    let _: serde_json::Value = gh_json(&[
        "api",
        "graphql",
        "-f",
        query.as_str(),
        "-f",
        thread_id.as_str(),
    ])?;
    Ok(())
}

fn clean_provider_link(
    pr: &ProviderPr,
    remote_user: &str,
    comment: &GhComment,
) -> ReviewProviderComment {
    ReviewProviderComment {
        provider: pr.provider.id().to_string(),
        remote: pr.remote.clone(),
        repo: pr.repo.clone(),
        pr_number: pr.number,
        comment_id: comment.id.to_string(),
        in_reply_to_id: comment.in_reply_to_id.map(|id| id.to_string()),
        thread_id: None,
        thread_resolved: None,
        resolved_dirty: false,
        author_username: Some(remote_user.to_string()),
        pr_title: Some(pr.title.clone()),
        pr_url: Some(pr.url.clone()),
        api_kind: "review".to_string(),
        sync_state: "clean".to_string(),
    }
}

fn clean_issue_provider_link(
    pr: &ProviderPr,
    remote_user: &str,
    comment: &GhIssueComment,
) -> ReviewProviderComment {
    ReviewProviderComment {
        provider: pr.provider.id().to_string(),
        remote: pr.remote.clone(),
        repo: pr.repo.clone(),
        pr_number: pr.number,
        comment_id: comment.id.to_string(),
        in_reply_to_id: None,
        thread_id: None,
        thread_resolved: None,
        resolved_dirty: false,
        author_username: Some(remote_user.to_string()),
        pr_title: Some(pr.title.clone()),
        pr_url: Some(pr.url.clone()),
        api_kind: "issue".to_string(),
        sync_state: "clean".to_string(),
    }
}

fn sync_pr_target(target: Option<&str>) -> Option<&str> {
    target.map(|target| {
        target
            .rsplit_once("...")
            .or_else(|| target.rsplit_once(".."))
            .map(|(_, head)| head)
            .unwrap_or(target)
    })
}

trait ReviewProviderAdapter: Send + Sync {
    fn find_pr(&self, remote: &ReviewRemote, target: Option<&str>) -> Result<ProviderPr>;
    fn whoami(&self, pr: &ProviderPr) -> Result<ProviderUser>;
    fn fetch_comments(&self, pr: ProviderPr, user: ProviderUser) -> Result<ReviewPullRemoteData>;
    fn push_ops<'a>(
        &self,
        pr: &'a ProviderPr,
        user: &'a ProviderUser,
    ) -> Box<dyn ReviewProviderPushOps + 'a>;
}

struct GithubProvider;

impl ReviewProviderAdapter for GithubProvider {
    fn find_pr(&self, remote: &ReviewRemote, target: Option<&str>) -> Result<ProviderPr> {
        gh_pr(remote, target)
    }

    fn whoami(&self, _pr: &ProviderPr) -> Result<ProviderUser> {
        gh_whoami()
    }

    fn fetch_comments(&self, pr: ProviderPr, user: ProviderUser) -> Result<ReviewPullRemoteData> {
        fetch_github_comments_for_pull(pr, user)
    }

    fn push_ops<'a>(
        &self,
        pr: &'a ProviderPr,
        user: &'a ProviderUser,
    ) -> Box<dyn ReviewProviderPushOps + 'a> {
        Box::new(GithubPushOps { pr, user })
    }
}

static GITHUB_PROVIDER: GithubProvider = GithubProvider;
static GITLAB_PROVIDER: gitlab::GitlabProvider = gitlab::GitlabProvider;
static FORGEJO_PROVIDER: forgejo::ForgejoProvider = forgejo::ForgejoProvider;

fn provider_adapter(kind: ReviewProviderKind) -> Result<&'static dyn ReviewProviderAdapter> {
    match kind {
        ReviewProviderKind::GitHub => Ok(&GITHUB_PROVIDER),
        ReviewProviderKind::GitLab => Ok(&GITLAB_PROVIDER),
        ReviewProviderKind::Forgejo => Ok(&FORGEJO_PROVIDER),
    }
}

fn provider_pr(remote: &ReviewRemote, target: Option<&str>) -> Result<ProviderPr> {
    provider_adapter(remote.provider)?.find_pr(remote, target)
}

fn provider_whoami(pr: &ProviderPr) -> Result<ProviderUser> {
    provider_adapter(pr.provider)?.whoami(pr)
}

fn resolve_sync_target_parts(
    target: Option<String>,
    remote_name: String,
) -> Result<(Option<String>, ReviewRemote, ProviderPr, String)> {
    let workspace = current_review_workspace()?;
    let remote = review_remote(&workspace, Some(&remote_name))?;
    let lookup_target = sync_pr_target(target.as_deref())
        .map(str::to_string)
        .or_else(|| git_output(&workspace, &["branch", "--show-current"]).ok())
        .filter(|branch| !branch.is_empty());
    let pr = provider_pr(&remote, lookup_target.as_deref())?;
    let revision = provider_revision(target.as_deref(), &pr);
    Ok((target, remote, pr, revision))
}

fn resolve_sync_target_items(
    items: &[String],
) -> Result<(Option<String>, ReviewRemote, ProviderPr, String)> {
    let workspace = current_review_workspace()?;
    let (target, remote_name) = parse_sync_args(&workspace, items)?;
    resolve_sync_target_parts(target, remote_name)
}

fn sync_lookup_target(explicit: Option<String>, default: Option<String>) -> Option<String> {
    explicit.or_else(|| {
        default.map(|target| sync_pr_target(Some(&target)).unwrap_or(&target).to_string())
    })
}

fn resolve_sync_target(
    config: &config::Config,
    args: &Args,
    items: &[String],
    option_target: Option<&str>,
) -> Result<(Option<String>, ReviewRemote, ProviderPr, String)> {
    let workspace = current_review_workspace()?;
    let (positional_target, remote_name) = parse_sync_args(&workspace, items)?;
    let explicit = combine_review_targets(&[option_target, positional_target.as_deref()])?;
    let default = if explicit.is_none() {
        resolve_review_target(config, args, None)?
    } else {
        None
    };
    let target = sync_lookup_target(explicit, default);
    resolve_sync_target_parts(target, remote_name)
}

fn review_app_for_pr(
    config: &config::Config,
    args: &Args,
    revision: &str,
    create: bool,
) -> Result<App> {
    review_app_for_target(config, args, Some(revision), create)
}

#[derive(Debug)]
struct ReviewPullStats {
    pulled: usize,
    skipped: usize,
    changed: Vec<u64>,
}

#[derive(Debug)]
struct ReviewPushStats {
    created: usize,
    updated: usize,
    threads_updated: usize,
    deleted: usize,
    skipped: usize,
    changed: Vec<u64>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct ReviewPushChange {
    id: u64,
    provider: ReviewProviderComment,
}

#[derive(Debug)]
struct ReviewPushThreadChange {
    provider: String,
    repo: String,
    pr_number: u64,
    thread_id: String,
    resolved: bool,
    comment_states: Vec<(u64, bool)>,
}

#[derive(Debug)]
struct ReviewPushOutcome {
    created: usize,
    updated: usize,
    deleted: usize,
    skipped: usize,
    changes: Vec<ReviewPushChange>,
    thread_changes: Vec<ReviewPushThreadChange>,
    warnings: Vec<String>,
}

#[derive(Debug)]
enum ReviewPullRemoteData {
    GitHub {
        pr: ProviderPr,
        user: ProviderUser,
        provider_comments: Vec<GhProviderComment>,
        issue_comments: Vec<GhIssueComment>,
        conversation_users: BTreeSet<String>,
    },
    GitLab {
        pr: ProviderPr,
        user: ProviderUser,
        discussions: Vec<gitlab::GlDiscussion>,
    },
    Forgejo {
        pr: ProviderPr,
        user: ProviderUser,
        review_comments: Vec<forgejo::FjReviewComments>,
        issue_comments: Vec<forgejo::FjIssueComment>,
    },
}

impl ReviewPullRemoteData {
    fn pr(&self) -> &ProviderPr {
        match self {
            Self::GitHub { pr, .. } | Self::GitLab { pr, .. } | Self::Forgejo { pr, .. } => pr,
        }
    }

    fn user(&self) -> &ProviderUser {
        match self {
            Self::GitHub { user, .. } | Self::GitLab { user, .. } | Self::Forgejo { user, .. } => {
                user
            }
        }
    }
}

fn fetch_provider_comments_for_pull(
    pr: ProviderPr,
    user: ProviderUser,
) -> Result<ReviewPullRemoteData> {
    provider_adapter(pr.provider)?.fetch_comments(pr, user)
}

fn fetch_github_comments_for_pull(
    pr: ProviderPr,
    user: ProviderUser,
) -> Result<ReviewPullRemoteData> {
    let thread_states = gh_review_thread_states(&pr)?;
    let provider_comments = gh_comments(&pr)?
        .into_iter()
        .map(|comment| {
            let thread = thread_states.get(&comment.id).or_else(|| {
                comment
                    .in_reply_to_id
                    .and_then(|comment_id| thread_states.get(&comment_id))
            });
            GhProviderComment {
                thread_id: thread.map(|thread| thread.thread_id.clone()),
                thread_resolved: thread.map(|thread| thread.resolved),
                comment,
            }
        })
        .collect();
    let issue_comments = gh_issue_comments(&pr)?;
    let conversation_users = github_conversation_comment_users(&pr, &user.login)?;
    Ok(ReviewPullRemoteData::GitHub {
        pr,
        user,
        provider_comments,
        issue_comments,
        conversation_users,
    })
}

fn apply_provider_comments_to_app(
    app: &mut App,
    data: ReviewPullRemoteData,
) -> Result<ReviewPullStats> {
    let mut changed = Vec::new();
    let mut skipped = 0usize;
    match data {
        ReviewPullRemoteData::GitHub {
            pr,
            user,
            provider_comments,
            issue_comments,
            conversation_users,
        } => {
            for comment in provider_comments {
                match github_comment_to_review_comment(app, &pr, &user.login, comment) {
                    Ok(comment) => changed.push(app.upsert_provider_review_comment(comment)),
                    Err(_) => skipped += 1,
                }
            }
            for comment in issue_comments {
                if !conversation_users.contains(&comment.user.login) {
                    skipped += 1;
                    continue;
                }
                match github_issue_comment_to_review_comment(app, &pr, &user.login, comment) {
                    Ok(comment) => changed.push(app.upsert_provider_review_comment(comment)),
                    Err(_) => skipped += 1,
                }
            }
        }
        ReviewPullRemoteData::GitLab {
            pr,
            user,
            discussions,
        } => {
            for discussion in discussions {
                for comment in
                    gitlab::discussion_to_review_comments(app, &pr, &user.login, discussion)
                {
                    match comment {
                        Ok(comment) => changed.push(app.upsert_provider_review_comment(comment)),
                        Err(_) => skipped += 1,
                    }
                }
            }
        }
        ReviewPullRemoteData::Forgejo {
            pr,
            user,
            review_comments,
            issue_comments,
        } => {
            for comment in forgejo::review_comments_to_oyo(app, &pr, &user.login, review_comments) {
                match comment {
                    Ok(comment) => changed.push(app.upsert_provider_review_comment(comment)),
                    Err(_) => skipped += 1,
                }
            }
            for comment in issue_comments {
                match forgejo::issue_comment_to_oyo(app, &pr, &user.login, comment) {
                    Ok(comment) => changed.push(app.upsert_provider_review_comment(comment)),
                    Err(_) => skipped += 1,
                }
            }
        }
    }
    Ok(ReviewPullStats {
        pulled: changed.len(),
        skipped,
        changed,
    })
}

fn pull_provider_comments_into_app(
    app: &mut App,
    pr: &ProviderPr,
    user: &ProviderUser,
) -> Result<ReviewPullStats> {
    let data = fetch_provider_comments_for_pull(pr.clone(), user.clone())?;
    apply_provider_comments_to_app(app, data)
}

trait ReviewProviderPushOps {
    fn provider_label(&self) -> &'static str;
    fn create_root(&mut self, comment: &ReviewComment) -> Result<ReviewProviderComment>;
    fn create_reply(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment>;
    fn update(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment>;
    fn delete(&mut self, provider: &ReviewProviderComment) -> Result<()>;
    fn set_thread_resolved(&mut self, thread_id: &str, resolved: bool) -> Result<()>;
}

struct GithubPushOps<'a> {
    pr: &'a ProviderPr,
    user: &'a ProviderUser,
}

impl ReviewProviderPushOps for GithubPushOps<'_> {
    fn provider_label(&self) -> &'static str {
        "GitHub"
    }

    fn create_root(&mut self, comment: &ReviewComment) -> Result<ReviewProviderComment> {
        if comment.anchor.kind == ReviewTargetKind::PullRequest {
            let remote_comment = github_create_issue_comment(self.pr, &comment.body)?;
            Ok(clean_issue_provider_link(
                self.pr,
                &self.user.login,
                &remote_comment,
            ))
        } else {
            let remote_comment = github_create_comment(self.pr, comment)?;
            Ok(clean_provider_link(
                self.pr,
                &self.user.login,
                &remote_comment,
            ))
        }
    }

    fn create_reply(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment> {
        let parent_comment_id = provider
            .in_reply_to_id
            .as_deref()
            .ok_or_else(|| anyhow!("Reply is missing a parent comment id."))?;
        let remote_comment = github_create_reply(self.pr, parent_comment_id, body)?;
        Ok(clean_provider_link(
            self.pr,
            &self.user.login,
            &remote_comment,
        ))
    }

    fn update(
        &mut self,
        provider: &ReviewProviderComment,
        body: &str,
    ) -> Result<ReviewProviderComment> {
        if provider.api_kind == "issue" {
            let remote_comment = github_update_issue_comment(self.pr, &provider.comment_id, body)?;
            Ok(clean_issue_provider_link(
                self.pr,
                &self.user.login,
                &remote_comment,
            ))
        } else {
            let remote_comment = github_update_comment(self.pr, &provider.comment_id, body)?;
            Ok(clean_provider_link(
                self.pr,
                &self.user.login,
                &remote_comment,
            ))
        }
    }

    fn delete(&mut self, provider: &ReviewProviderComment) -> Result<()> {
        if provider.api_kind == "issue" {
            github_delete_issue_comment(self.pr, &provider.comment_id)
        } else {
            github_delete_comment(self.pr, &provider.comment_id)
        }
    }

    fn set_thread_resolved(&mut self, thread_id: &str, resolved: bool) -> Result<()> {
        github_set_review_thread_resolved(thread_id, resolved)
    }
}

#[cfg(test)]
fn push_review_comments_to_provider_with<R, F>(
    comments: Vec<ReviewComment>,
    pr: &ProviderPr,
    user: &ProviderUser,
    create_reply: R,
    set_thread_resolved: F,
) -> Result<ReviewPushOutcome>
where
    R: FnMut(&str, &str) -> Result<GhComment>,
    F: FnMut(&str, bool) -> Result<()>,
{
    struct GithubClosurePushOps<'a, R, F> {
        pr: &'a ProviderPr,
        user: &'a ProviderUser,
        create_reply: R,
        set_thread_resolved: F,
    }

    impl<R, F> ReviewProviderPushOps for GithubClosurePushOps<'_, R, F>
    where
        R: FnMut(&str, &str) -> Result<GhComment>,
        F: FnMut(&str, bool) -> Result<()>,
    {
        fn provider_label(&self) -> &'static str {
            "GitHub"
        }

        fn create_root(&mut self, _comment: &ReviewComment) -> Result<ReviewProviderComment> {
            unreachable!()
        }

        fn create_reply(
            &mut self,
            provider: &ReviewProviderComment,
            body: &str,
        ) -> Result<ReviewProviderComment> {
            let parent_comment_id = provider
                .in_reply_to_id
                .as_deref()
                .ok_or_else(|| anyhow!("Reply is missing a parent comment id."))?;
            let remote_comment = (self.create_reply)(parent_comment_id, body)?;
            Ok(clean_provider_link(
                self.pr,
                &self.user.login,
                &remote_comment,
            ))
        }

        fn update(
            &mut self,
            _provider: &ReviewProviderComment,
            _body: &str,
        ) -> Result<ReviewProviderComment> {
            unreachable!()
        }

        fn delete(&mut self, _provider: &ReviewProviderComment) -> Result<()> {
            unreachable!()
        }

        fn set_thread_resolved(&mut self, thread_id: &str, resolved: bool) -> Result<()> {
            (self.set_thread_resolved)(thread_id, resolved)
        }
    }

    let mut ops = GithubClosurePushOps {
        pr,
        user,
        create_reply,
        set_thread_resolved,
    };
    push_review_comments_to_provider_with_ops(comments, pr, &mut ops)
}

fn push_review_comments_to_provider(
    comments: Vec<ReviewComment>,
    pr: &ProviderPr,
    user: &ProviderUser,
) -> Result<ReviewPushOutcome> {
    let mut ops = provider_adapter(pr.provider)?.push_ops(pr, user);
    push_review_comments_to_provider_with_ops(comments, pr, ops.as_mut())
}

fn push_review_comments_to_provider_with_ops<O>(
    comments: Vec<ReviewComment>,
    pr: &ProviderPr,
    ops: &mut O,
) -> Result<ReviewPushOutcome>
where
    O: ReviewProviderPushOps + ?Sized,
{
    let mut comments = comments;
    comments.sort_by_key(|comment| {
        match (
            comment.deleted,
            comment
                .provider
                .as_ref()
                .and_then(|provider| provider.in_reply_to_id.as_ref())
                .is_some(),
        ) {
            (true, true) => 0,
            (true, false) => 1,
            _ => 2,
        }
    });

    for comment in &comments {
        let Some(provider) = comment.provider.as_ref() else {
            continue;
        };
        if !comment.can_edit
            || comment.deleted
            || provider.provider != pr.provider.id()
            || provider.repo != pr.repo
            || provider.pr_number != pr.number
            || !provider.comment_id.is_empty()
        {
            continue;
        }
        if provider.api_kind != "review"
            || provider.thread_id.is_none()
            || provider.in_reply_to_id.is_none()
        {
            anyhow::bail!(
                "Reply comment {} is not linked to a remote review thread",
                comment.id
            );
        }
    }

    let mut pending_threads = BTreeMap::new();
    let mut thread_comment_states = BTreeMap::<String, Vec<(u64, bool)>>::new();
    for comment in &comments {
        let Some(provider) = comment.provider.as_ref() else {
            continue;
        };
        if comment.deleted
            || provider.provider != pr.provider.id()
            || provider.repo != pr.repo
            || provider.pr_number != pr.number
            || provider.api_kind != "review"
        {
            continue;
        }
        let (Some(thread_id), Some(remote_resolved)) =
            (provider.thread_id.as_deref(), provider.thread_resolved)
        else {
            continue;
        };
        thread_comment_states
            .entry(thread_id.to_string())
            .or_default()
            .push((comment.id, comment.resolved));
        if comment.resolved != remote_resolved {
            if let Some(existing) = pending_threads.insert(thread_id.to_string(), comment.resolved)
            {
                if existing != comment.resolved {
                    anyhow::bail!(
                        "Review thread {thread_id} has conflicting local resolved states. Pull and retry."
                    );
                }
            }
        }
    }
    let mut created = 0usize;
    let mut updated = 0usize;
    let mut deleted = 0usize;
    let mut skipped = 0usize;
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut pending_replies = Vec::new();
    for comment in comments {
        if !comment.can_edit {
            skipped += 1;
            continue;
        }
        let provider = comment.provider.clone();
        if let Some(provider) = provider.as_ref() {
            if provider.provider != pr.provider.id()
                || provider.repo != pr.repo
                || provider.pr_number != pr.number
            {
                skipped += 1;
                continue;
            }
            if provider.comment_id.is_empty() {
                if !comment.deleted {
                    pending_replies.push((comment.id, comment.body, provider.clone()));
                }
                continue;
            }
            if comment.deleted {
                ops.delete(provider)?;
                changes.push(ReviewPushChange {
                    id: comment.id,
                    provider: provider.clone(),
                });
                deleted += 1;
                continue;
            }
            if provider.sync_state == "dirty" {
                let mut clean_provider = ops.update(provider, &comment.body)?;
                clean_provider
                    .in_reply_to_id
                    .clone_from(&provider.in_reply_to_id);
                clean_provider.thread_id.clone_from(&provider.thread_id);
                clean_provider.thread_resolved = provider.thread_resolved;
                clean_provider.resolved_dirty = provider.resolved_dirty;
                changes.push(ReviewPushChange {
                    id: comment.id,
                    provider: clean_provider,
                });
                updated += 1;
            }
            continue;
        }
        if comment.deleted {
            continue;
        }
        if comment.in_reply_to.is_some() {
            skipped += 1;
            continue;
        }
        let mut provider = ops.create_root(&comment)?;
        provider.resolved_dirty = comment.resolved;
        changes.push(ReviewPushChange {
            id: comment.id,
            provider,
        });
        created += 1;
    }

    for (id, body, provider) in pending_replies {
        let parent_comment_id = provider.in_reply_to_id.as_deref().unwrap_or_default();
        match ops.create_reply(&provider, &body) {
            Ok(mut clean_provider) => {
                clean_provider
                    .in_reply_to_id
                    .clone_from(&provider.in_reply_to_id);
                clean_provider.thread_id.clone_from(&provider.thread_id);
                clean_provider.thread_resolved = provider.thread_resolved;
                clean_provider.resolved_dirty = provider.resolved_dirty;
                changes.push(ReviewPushChange {
                    id,
                    provider: clean_provider,
                });
                created += 1;
            }
            Err(error) => {
                skipped += 1;
                warnings.push(format!(
                    "Could not reply to {} review comment {parent_comment_id}: {error}",
                    ops.provider_label()
                ));
            }
        }
    }

    let mut thread_changes = Vec::new();
    for (thread_id, resolved) in pending_threads {
        match ops.set_thread_resolved(&thread_id, resolved) {
            Ok(()) => {
                thread_changes.push(ReviewPushThreadChange {
                    provider: pr.provider.id().to_string(),
                    repo: pr.repo.clone(),
                    pr_number: pr.number,
                    comment_states: thread_comment_states.remove(&thread_id).unwrap_or_default(),
                    thread_id,
                    resolved,
                });
            }
            Err(error) => {
                skipped += 1;
                let action = if resolved { "resolve" } else { "unresolve" };
                warnings.push(format!(
                    "Could not {action} {} review thread {thread_id}: {error}",
                    ops.provider_label()
                ));
            }
        }
    }
    Ok(ReviewPushOutcome {
        created,
        updated,
        deleted,
        skipped,
        changes,
        thread_changes,
        warnings,
    })
}

fn apply_push_outcome_to_app(app: &mut App, outcome: ReviewPushOutcome) -> ReviewPushStats {
    let mut changed = Vec::new();
    let threads_updated = outcome.thread_changes.len();
    for change in outcome.changes {
        app.mark_review_comment_synced(change.id, change.provider);
        changed.push(change.id);
    }
    for change in outcome.thread_changes {
        changed.extend(app.mark_review_thread_synced(
            &change.provider,
            &change.repo,
            change.pr_number,
            &change.thread_id,
            change.resolved,
            &change.comment_states,
        ));
    }
    changed.sort_unstable();
    changed.dedup();
    ReviewPushStats {
        created: outcome.created,
        updated: outcome.updated,
        threads_updated,
        deleted: outcome.deleted,
        skipped: outcome.skipped,
        changed,
        warnings: outcome.warnings,
    }
}

fn push_app_comments_to_provider(
    app: &mut App,
    pr: &ProviderPr,
    user: &ProviderUser,
) -> Result<ReviewPushStats> {
    let outcome = push_review_comments_to_provider(app.review_comments_for_sync(), pr, user)?;
    Ok(apply_push_outcome_to_app(app, outcome))
}

enum ReviewSyncWorkerResult {
    Pull {
        action: ReviewSyncAction,
        data: Box<ReviewPullRemoteData>,
    },
    Push {
        action: ReviewSyncAction,
        provider: ReviewProviderKind,
        user: ProviderUser,
        outcome: ReviewPushOutcome,
    },
}

struct ReviewSyncWorker {
    rx: std::sync::mpsc::Receiver<Result<ReviewSyncWorkerResult>>,
}

struct ReviewPrLookupWorker {
    rx: std::sync::mpsc::Receiver<Result<ProviderPr>>,
}

fn spawn_review_pr_lookup_worker(target: Option<String>) -> ReviewPrLookupWorker {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let items = target.into_iter().collect::<Vec<_>>();
        let result = resolve_sync_target_items(&items).map(|(_, _, pr, _)| pr);
        let _ = tx.send(result);
    });
    ReviewPrLookupWorker { rx }
}

fn spawn_review_pull_worker(action: ReviewSyncAction, remote: Option<String>) -> ReviewSyncWorker {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let items = remote.into_iter().collect::<Vec<_>>();
            let (_target, _remote, pr, _revision) = resolve_sync_target_items(&items)?;
            let user = provider_whoami(&pr)?;
            let data = fetch_provider_comments_for_pull(pr, user)?;
            Ok(ReviewSyncWorkerResult::Pull {
                action,
                data: Box::new(data),
            })
        })();
        let _ = tx.send(result);
    });
    ReviewSyncWorker { rx }
}

fn spawn_review_push_worker(
    action: ReviewSyncAction,
    pr: ProviderPr,
    user: ProviderUser,
    comments: Vec<ReviewComment>,
) -> ReviewSyncWorker {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let outcome = push_review_comments_to_provider(comments, &pr, &user)?;
            Ok(ReviewSyncWorkerResult::Push {
                action,
                provider: pr.provider,
                user,
                outcome,
            })
        })();
        let _ = tx.send(result);
    });
    ReviewSyncWorker { rx }
}

fn spawn_review_push_request_worker(
    action: ReviewSyncAction,
    remote: Option<String>,
    comments: Vec<ReviewComment>,
) -> ReviewSyncWorker {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let items = remote.into_iter().collect::<Vec<_>>();
            let (_target, _remote, pr, _revision) = resolve_sync_target_items(&items)?;
            let user = provider_whoami(&pr)?;
            let outcome = push_review_comments_to_provider(comments, &pr, &user)?;
            Ok(ReviewSyncWorkerResult::Push {
                action,
                provider: pr.provider,
                user,
                outcome,
            })
        })();
        let _ = tx.send(result);
    });
    ReviewSyncWorker { rx }
}

fn review_remote_options() -> Result<Vec<ReviewRemoteOption>> {
    let workspace = current_review_workspace()?;
    let mut options = Vec::new();
    for name in git_remotes(&workspace) {
        let label = git_output(&workspace, &["remote", "get-url", &name])
            .ok()
            .and_then(|url| parse_remote_url(&url).map(|(_, repo)| repo).or(Some(url)))
            .unwrap_or_default();
        options.push(ReviewRemoteOption { name, label });
    }
    Ok(options)
}

fn handle_review_pull_command(
    items: &[String],
    target: Option<&str>,
    json: bool,
    config: &config::Config,
    args: &Args,
) -> Result<()> {
    let (_target, _remote, pr, revision) = resolve_sync_target(config, args, items, target)?;
    let mut app = review_app_for_pr(config, args, &revision, true)?;
    app.set_review_target_metadata(Some(review_pr_target_metadata(&pr)));
    let user = provider_whoami(&pr)?;
    let stats = pull_provider_comments_into_app(&mut app, &pr, &user)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&review_json_with_target(
                &app,
                serde_json::json!({
                    "ok": true,
                    "provider": pr.provider.id(),
                    "repo": pr.repo,
                    "url": pr.url,
                    "revision": revision,
                    "baseCommit": pr.base_commit,
                    "headCommit": pr.head_commit,
                    "pulled": stats.pulled,
                    "skipped": stats.skipped,
                    "changedComments": stats.changed,
                }),
            ))?
        );
    } else {
        print_review_target_header(&app);
        println!("Pulled {} comments from {}.", stats.pulled, pr.url);
        if stats.skipped > 0 {
            println!(
                "Skipped {} comments that do not map to this diff.",
                stats.skipped
            );
        }
    }
    Ok(())
}

fn handle_review_push_command(
    items: &[String],
    target: Option<&str>,
    json: bool,
    config: &config::Config,
    args: &Args,
) -> Result<()> {
    let (_target, _remote, pr, revision) = resolve_sync_target(config, args, items, target)?;
    let mut app = review_app_for_pr(config, args, &revision, false)?;
    app.set_review_target_metadata(Some(review_pr_target_metadata(&pr)));
    let user = provider_whoami(&pr)?;
    let stats = push_app_comments_to_provider(&mut app, &pr, &user)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&review_json_with_target(
                &app,
                serde_json::json!({
                    "ok": true,
                    "provider": pr.provider.id(),
                    "repo": pr.repo,
                    "url": pr.url,
                    "revision": revision,
                    "baseCommit": pr.base_commit,
                    "headCommit": pr.head_commit,
                    "created": stats.created,
                    "updated": stats.updated,
                    "threadsUpdated": stats.threads_updated,
                    "deleted": stats.deleted,
                    "skipped": stats.skipped,
                    "changedComments": stats.changed,
                    "warnings": stats.warnings,
                }),
            ))?
        );
    } else {
        print_review_target_header(&app);
        println!(
            "Pushed {} created, {} updated and {} deleted comments, plus {} thread updates, to {}.",
            stats.created, stats.updated, stats.deleted, stats.threads_updated, pr.url
        );
        if stats.skipped > 0 {
            println!("Skipped {} comments or thread updates.", stats.skipped);
        }
        for warning in stats.warnings {
            eprintln!("Warning: {warning}");
        }
    }
    Ok(())
}

fn review_log_entries(config: &config::Config, args: &Args) -> Result<Vec<serde_json::Value>> {
    let workspace = current_review_workspace()?;
    let base = review_base_dir(config, args, Some(&workspace));
    let root = base.join(review_hash(&workspace.to_string_lossy()));
    let db_path = root.join("review.db");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open(&db_path)?;
    let mut stmt = conn.prepare(
        "SELECT r.review_key, r.updated_at, r.target_json, COUNT(c.id)
         FROM reviews r
         LEFT JOIN comments c ON c.review_key = r.review_key
         GROUP BY r.review_key, r.updated_at, r.target_json
         ORDER BY r.updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "workspaceRoot": workspace.to_string_lossy().to_string(),
            "reviewDir": root.to_string_lossy().to_string(),
            "reviewDb": db_path.to_string_lossy().to_string(),
            "diffFingerprint": row.get::<_, String>(0)?,
            "reviewKey": row.get::<_, String>(0)?,
            "updatedAt": row.get::<_, i64>(1)?.max(0) as u64,
            "target": row.get::<_, Option<String>>(2)?.and_then(|json| serde_json::from_str::<ReviewTargetMetadata>(&json).ok()),
            "commentCount": row.get::<_, i64>(3)?.max(0) as u64,
        }))
    })?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

fn review_log_entry_metadata(entry: &serde_json::Value) -> Option<ReviewTargetMetadata> {
    serde_json::from_value::<ReviewTargetMetadata>(entry["target"].clone()).ok()
}

fn review_log_comment_count(entry: &serde_json::Value) -> u64 {
    entry["commentCount"].as_u64().unwrap_or(0)
}

fn review_log_dedupe_key(
    entry: &serde_json::Value,
    current: Option<&str>,
    current_metadata: &ReviewTargetMetadata,
) -> String {
    let fingerprint = entry["diffFingerprint"].as_str().unwrap_or_default();
    if review_log_entry_matches_current(entry, current, current_metadata) {
        return format!(
            "current:{}",
            review_target_metadata_display_id(current_metadata, fingerprint)
        );
    }
    if let Some(metadata) = review_log_entry_metadata(entry) {
        let id = match metadata.vcs.as_str() {
            "git" => metadata
                .git_head_commit
                .or(metadata.git_base_commit)
                .or(metadata.branch)
                .or(metadata.git_head_ref),
            "jj" => metadata.jj_change_id.or(metadata.jj_commit_id),
            _ => None,
        };
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            return format!("{}:{id}", metadata.vcs);
        }
    }
    format!("fingerprint:{fingerprint}")
}

fn dedupe_review_log_entries(
    entries: Vec<serde_json::Value>,
    current: Option<&str>,
    current_metadata: &ReviewTargetMetadata,
) -> Vec<serde_json::Value> {
    let mut out = Vec::<serde_json::Value>::new();
    let mut seen = BTreeMap::<String, usize>::new();
    for entry in entries {
        let key = review_log_dedupe_key(&entry, current, current_metadata);
        if let Some(idx) = seen.get(&key).copied() {
            if review_log_comment_count(&out[idx]) == 0 && review_log_comment_count(&entry) > 0 {
                out[idx] = entry;
            }
        } else {
            seen.insert(key, out.len());
            out.push(entry);
        }
    }
    out
}

fn review_target_metadata_display_id(metadata: &ReviewTargetMetadata, fallback: &str) -> String {
    match metadata.vcs.as_str() {
        "git" => metadata
            .git_head_commit
            .clone()
            .or(metadata.git_base_commit.clone()),
        "jj" => metadata
            .jj_change_id
            .clone()
            .or(metadata.jj_commit_id.clone()),
        _ => None,
    }
    .unwrap_or_else(|| fallback.to_string())
}

fn review_target_metadata_match_ids(
    metadata: &ReviewTargetMetadata,
    fallback: &str,
) -> Vec<String> {
    let mut ids = match metadata.vcs.as_str() {
        "git" => vec![
            metadata.git_head_commit.clone(),
            metadata.git_base_commit.clone(),
        ],
        "jj" => vec![metadata.jj_change_id.clone(), metadata.jj_commit_id.clone()],
        _ => Vec::new(),
    }
    .into_iter()
    .flatten()
    .filter(|id| !id.is_empty())
    .collect::<Vec<_>>();
    if !fallback.is_empty() {
        ids.push(fallback.to_string());
    }
    ids.sort();
    ids.dedup();
    ids
}

fn review_log_entry_display_id(entry: &serde_json::Value) -> String {
    let fingerprint = entry["diffFingerprint"].as_str().unwrap_or_default();
    review_log_entry_metadata(entry)
        .map(|metadata| review_target_metadata_display_id(&metadata, fingerprint))
        .unwrap_or_else(|| fingerprint.to_string())
}

fn review_log_entry_matches_current(
    entry: &serde_json::Value,
    current: Option<&str>,
    current_metadata: &ReviewTargetMetadata,
) -> bool {
    let fingerprint = entry["diffFingerprint"].as_str().unwrap_or_default();
    current == Some(fingerprint)
        || review_log_entry_metadata(entry)
            .as_ref()
            .is_some_and(|metadata| metadata_in_current_scope(metadata, current_metadata))
}

fn review_log_entry_display_id_for_current(
    entry: &serde_json::Value,
    current: Option<&str>,
    current_metadata: &ReviewTargetMetadata,
) -> String {
    let fingerprint = entry["diffFingerprint"].as_str().unwrap_or_default();
    if review_log_entry_matches_current(entry, current, current_metadata) {
        review_target_metadata_display_id(current_metadata, fingerprint)
    } else {
        review_log_entry_display_id(entry)
    }
}

fn review_log_entry_match_ids_for_current(
    entry: &serde_json::Value,
    current: Option<&str>,
    current_metadata: &ReviewTargetMetadata,
) -> Vec<String> {
    let fingerprint = entry["diffFingerprint"].as_str().unwrap_or_default();
    if review_log_entry_matches_current(entry, current, current_metadata) {
        review_target_metadata_match_ids(current_metadata, fingerprint)
    } else {
        review_log_entry_metadata(entry)
            .map(|metadata| review_target_metadata_match_ids(&metadata, fingerprint))
            .unwrap_or_else(|| vec![fingerprint.to_string()])
    }
}

fn print_review_log(config: &config::Config, args: &Args, json: bool) -> Result<()> {
    let entries = review_log_entries_for_scope(config, args)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        let color = review_cli_color_enabled();
        println!("{}", review_cli_paint(color, "32", "No saved reviews."));
        return Ok(());
    }
    let current = review_app_for_target(config, args, None, false)
        .ok()
        .map(|app| app.review_storage_key().to_string());
    let current_metadata = review_target_metadata(None, args);
    let color = review_cli_color_enabled();
    let display_ids = entries
        .iter()
        .filter(|entry| entry["commentCount"].as_u64().unwrap_or(0) > 0)
        .map(|entry| {
            review_log_entry_display_id_for_current(entry, current.as_deref(), &current_metadata)
        })
        .collect::<Vec<_>>();
    let jj_commit_ids = entries
        .iter()
        .filter(|entry| entry["commentCount"].as_u64().unwrap_or(0) > 0)
        .filter_map(|entry| {
            let fingerprint = entry["diffFingerprint"].as_str().unwrap_or_default();
            if current.as_deref() == Some(fingerprint) {
                current_metadata.jj_commit_id.clone()
            } else {
                review_log_entry_metadata(entry).and_then(|metadata| metadata.jj_commit_id)
            }
        })
        .collect::<Vec<_>>();
    let mut printed = false;
    let mut current_marked = false;
    for entry in entries {
        let count = entry["commentCount"].as_u64().unwrap_or(0) as usize;
        if count == 0 {
            continue;
        }
        let fingerprint = entry["diffFingerprint"].as_str().unwrap_or_default();
        let matches_current =
            review_log_entry_matches_current(&entry, current.as_deref(), &current_metadata);
        let current_target = matches_current && !current_marked;
        current_marked |= matches_current;
        let marker = if current_target { "@" } else { "○" };
        let marker = review_cli_paint(
            color,
            if current_target { "1;38;5;2" } else { "38;5;2" },
            marker,
        );
        let metadata = if current_target {
            Some(current_metadata.clone())
        } else {
            review_log_entry_metadata(&entry)
        };
        match metadata {
            Some(metadata) if metadata.vcs == "jj" => {
                let change_id = metadata.jj_change_id.unwrap_or(metadata.label.clone());
                let commit_id = metadata.jj_commit_id.unwrap_or_default();
                let mut line = format!(
                    "{marker}  {}",
                    review_cli_jj_id(
                        color,
                        &change_id,
                        if current_target {
                            "1;38;5;13"
                        } else {
                            "1;38;5;5"
                        },
                    )
                );
                if let Some(author) = metadata.author.filter(|value| !value.trim().is_empty()) {
                    line.push(' ');
                    line.push_str(&review_cli_paint(color, "38;5;3", &author));
                }
                if let Some(timestamp) = metadata.timestamp.filter(|value| !value.trim().is_empty())
                {
                    line.push(' ');
                    line.push_str(&review_cli_paint(
                        color,
                        if current_target { "38;5;14" } else { "38;5;6" },
                        &timestamp,
                    ));
                }
                if let Some(bookmarks) = metadata.bookmarks.filter(|value| !value.trim().is_empty())
                {
                    line.push(' ');
                    line.push_str(&review_cli_paint(color, "38;5;5", &bookmarks));
                }
                if !commit_id.is_empty() {
                    line.push(' ');
                    let prefix_len = shortest_unique_prefix_len(&commit_id, &jj_commit_ids).min(8);
                    line.push_str(&review_cli_colored_id(
                        color,
                        &commit_id,
                        prefix_len,
                        if current_target {
                            "1;38;5;12"
                        } else {
                            "1;38;5;4"
                        },
                    ));
                }
                println!("{line}");
            }
            Some(metadata) if metadata.vcs == "git" => {
                let id = metadata
                    .git_head_commit
                    .clone()
                    .or(metadata.git_base_commit.clone())
                    .unwrap_or_else(|| fingerprint.to_string());
                let short = id.chars().take(8).collect::<String>();
                let prefix_len = shortest_unique_prefix_len(&id, &display_ids).min(8);
                let mut line = format!(
                    "{marker}  {}",
                    review_cli_colored_id(
                        color,
                        &short,
                        prefix_len,
                        if current_target {
                            "1;38;5;12"
                        } else {
                            "1;38;5;4"
                        },
                    )
                );
                if let Some(author) = metadata.author.filter(|value| !value.trim().is_empty()) {
                    line.push(' ');
                    line.push_str(&review_cli_paint(color, "38;5;3", &author));
                }
                if let Some(timestamp) = metadata.timestamp.filter(|value| !value.trim().is_empty())
                {
                    line.push(' ');
                    line.push_str(&review_cli_paint(
                        color,
                        if current_target { "38;5;14" } else { "38;5;6" },
                        &timestamp,
                    ));
                }
                let label = metadata
                    .pr_number
                    .map(|number| {
                        format!(
                            "{}#{}",
                            metadata.pr_provider.as_deref().unwrap_or("pr"),
                            number
                        )
                    })
                    .or(metadata.branch)
                    .or(metadata.git_head_ref);
                if let Some(label) = label.filter(|value| !value.trim().is_empty()) {
                    line.push(' ');
                    line.push_str(&review_cli_paint(color, "38;5;5", &label));
                }
                println!("{line}");
            }
            _ => {
                let label = if current_target {
                    review_cli_target_value(color, &current_metadata.label)
                } else {
                    let short = fingerprint.chars().take(8).collect::<String>();
                    let prefix_len = shortest_unique_prefix_len(fingerprint, &display_ids).min(8);
                    review_cli_colored_id(color, &short, prefix_len, "1;38;5;12")
                };
                println!("{marker}  {label}");
            }
        }
        println!(
            "{}  {}",
            review_cli_paint(color, "38;5;6", "│"),
            review_cli_paint(color, "33", &review_comment_count_label(count))
        );
        printed = true;
    }
    if printed {
        println!("~");
    } else {
        println!("{}", review_cli_paint(color, "32", "No saved reviews."));
    }
    Ok(())
}

fn mutation_json(app: &App, changed: Vec<u64>) -> Result<()> {
    let paths = app.review_paths();
    println!(
        "{}",
        serde_json::to_string_pretty(&review_json_with_target(
            app,
            serde_json::json!({
                "ok": true,
                "workspaceRoot": app.review_workspace_root().unwrap_or_default(),
                "diffFingerprint": app.review_diff_fingerprint(),
                "reviewDir": path_json(paths.review_dir),
                "reviewDb": path_json(paths.db_file),
                "commentCount": app.review_comment_count(),
                "changedComments": changed,
            }),
        ))?
    );
    Ok(())
}

fn abandon_json(app: &App, removed: bool) -> Result<()> {
    let paths = app.review_paths();
    println!(
        "{}",
        serde_json::to_string_pretty(&review_json_with_target(
            app,
            serde_json::json!({
                "ok": true,
                "abandoned": removed,
                "workspaceRoot": app.review_workspace_root().unwrap_or_default(),
                "diffFingerprint": app.review_diff_fingerprint(),
                "reviewDir": path_json(paths.review_dir),
                "reviewDb": path_json(paths.db_file),
                "commentCount": app.review_comment_count(),
            }),
        ))?
    );
    Ok(())
}

fn parse_comment_id_args(
    items: &[String],
    option_target: Option<&str>,
) -> Result<(Option<String>, u64)> {
    match items {
        [id] => Ok((option_target.map(str::to_string), id.parse()?)),
        [revision, id] => Ok((
            combine_review_targets(&[option_target, Some(revision)])?,
            id.parse()?,
        )),
        _ => anyhow::bail!("Pass a comment ID, with an optional review target"),
    }
}

fn handle_review_comment_command(
    command: &Option<ReviewCommentCommand>,
    outer_target: Option<&str>,
    default_json: bool,
    filter: &ReviewCommentFilter,
    config: &config::Config,
    args: &Args,
) -> Result<()> {
    match command {
        None => {
            let target = resolve_review_target(config, args, outer_target)?;
            let app = review_app_for_comment_target(config, args, target.as_deref())?;
            print_review_comments(&app, default_json, filter)
        }
        Some(ReviewCommentCommand::New {
            revision,
            target,
            file,
            new_line,
            old_line,
            file_level,
            body,
            author_type,
            author_name,
            author_email,
            author_username,
            json,
        }) => {
            let explicit =
                combine_review_targets(&[outer_target, target.as_deref(), revision.as_deref()])?;
            let target = resolve_review_target(config, args, explicit.as_deref())?;
            let mut app = review_app_for_target(config, args, target.as_deref(), true)?;
            if let Some(author) = review_author_from_cli(
                author_name.as_deref(),
                author_email.as_deref(),
                author_type.as_deref(),
                author_username,
            )? {
                app.set_review_author(Some(author));
            }
            let (kind, side, old_range, new_range) = if *file_level {
                (ReviewTargetKind::File, None, None, None)
            } else if let Some(line) = new_line {
                (
                    ReviewTargetKind::Line,
                    Some(ReviewSide::New),
                    None,
                    Some(ReviewRange {
                        start: *line,
                        end: *line,
                    }),
                )
            } else if let Some(line) = old_line {
                (
                    ReviewTargetKind::Line,
                    Some(ReviewSide::Old),
                    Some(ReviewRange {
                        start: *line,
                        end: *line,
                    }),
                    None,
                )
            } else {
                anyhow::bail!("Pass --new-line, --old-line or --file-level");
            };
            let id = app
                .add_review_comment_from_cli(file, kind, side, old_range, new_range, body.clone())
                .map_err(|error| anyhow!(error))?;
            if *json {
                mutation_json(&app, vec![id])
            } else {
                print_review_target_header(&app);
                println!("Added comment {id}.");
                Ok(())
            }
        }
        Some(ReviewCommentCommand::Edit {
            args: items,
            target,
            body,
            json,
        }) => {
            let explicit = combine_review_targets(&[outer_target, target.as_deref()])?;
            let (target, comment_id) = parse_comment_id_args(items, explicit.as_deref())?;
            let target = resolve_review_target(config, args, target.as_deref())?;
            let mut app = review_app_for_comment_target(config, args, target.as_deref())?;
            if !app.edit_review_comment_from_cli(comment_id, body.clone()) {
                anyhow::bail!("No comment matches id {comment_id}");
            }
            if *json {
                mutation_json(&app, vec![comment_id])
            } else {
                print_review_target_header(&app);
                println!("Edited comment {comment_id}.");
                Ok(())
            }
        }
        Some(ReviewCommentCommand::Reply {
            args: items,
            target,
            body,
            json,
        }) => {
            let explicit = combine_review_targets(&[outer_target, target.as_deref()])?;
            let (target, parent_id) = parse_comment_id_args(items, explicit.as_deref())?;
            let target = resolve_review_target(config, args, target.as_deref())?;
            let mut app = review_app_for_comment_target(config, args, target.as_deref())?;
            let id = app
                .add_review_reply_from_cli(parent_id, body.clone())
                .map_err(|error| anyhow!(error))?;
            if *json {
                mutation_json(&app, vec![id])
            } else {
                print_review_target_header(&app);
                println!("Replied to comment {parent_id} with comment {id}.");
                Ok(())
            }
        }
        Some(ReviewCommentCommand::Rm {
            args: items,
            target,
            yes,
            json,
        }) => {
            let explicit = combine_review_targets(&[outer_target, target.as_deref()])?;
            let (target, comment_id) = parse_comment_id_args(items, explicit.as_deref())?;
            let target = resolve_review_target(config, args, target.as_deref())?;
            let mut app = review_app_for_comment_target(config, args, target.as_deref())?;
            let Some(deleted_ids) = app.review_comment_delete_ids(comment_id) else {
                anyhow::bail!("No comment matches id {comment_id}");
            };
            let reply_count = deleted_ids.len().saturating_sub(1);
            let replies = if reply_count == 1 { "reply" } else { "replies" };
            if !yes {
                if reply_count > 0 {
                    anyhow::bail!(
                        "Pass --yes to remove this comment and its {reply_count} {replies}"
                    );
                }
                anyhow::bail!("Pass --yes to remove a comment");
            }
            if !app.remove_review_comment_from_cli(comment_id) {
                anyhow::bail!("No comment matches id {comment_id}");
            }
            if *json {
                mutation_json(&app, deleted_ids)
            } else {
                print_review_target_header(&app);
                if reply_count > 0 {
                    println!("Removed comment {comment_id} and {reply_count} {replies}.");
                } else {
                    println!("Removed comment {comment_id}.");
                }
                Ok(())
            }
        }
        Some(ReviewCommentCommand::Resolve {
            args: items,
            target,
            json,
        }) => {
            let explicit = combine_review_targets(&[outer_target, target.as_deref()])?;
            let (target, comment_id) = parse_comment_id_args(items, explicit.as_deref())?;
            let target = resolve_review_target(config, args, target.as_deref())?;
            let mut app = review_app_for_comment_target(config, args, target.as_deref())?;
            if app.review_comment_is_reply_id(comment_id) {
                anyhow::bail!("Replies can't be resolved - resolve the parent comment.");
            }
            if !app.set_review_comment_resolved_from_cli(comment_id, true) {
                anyhow::bail!("No comment matches id {comment_id}");
            }
            if *json {
                mutation_json(&app, vec![comment_id])
            } else {
                print_review_target_header(&app);
                println!("Resolved comment {comment_id}.");
                Ok(())
            }
        }
        Some(ReviewCommentCommand::Reopen {
            args: items,
            target,
            json,
        }) => {
            let explicit = combine_review_targets(&[outer_target, target.as_deref()])?;
            let (target, comment_id) = parse_comment_id_args(items, explicit.as_deref())?;
            let target = resolve_review_target(config, args, target.as_deref())?;
            let mut app = review_app_for_comment_target(config, args, target.as_deref())?;
            if app.review_comment_is_reply_id(comment_id) {
                anyhow::bail!("Replies can't be resolved - resolve the parent comment.");
            }
            if !app.set_review_comment_resolved_from_cli(comment_id, false) {
                anyhow::bail!("No comment matches id {comment_id}");
            }
            if *json {
                mutation_json(&app, vec![comment_id])
            } else {
                print_review_target_header(&app);
                println!("Reopened comment {comment_id}.");
                Ok(())
            }
        }
        Some(ReviewCommentCommand::Apply {
            args: items,
            target,
            json,
        }) => {
            let (positional_target, input) = match items.as_slice() {
                [input] => (None, input.as_str()),
                [revision, input] => (Some(revision.as_str()), input.as_str()),
                _ => anyhow::bail!("Usage: oy review comment apply [revision] <file|->"),
            };
            let explicit =
                combine_review_targets(&[outer_target, target.as_deref(), positional_target])?;
            let target = resolve_review_target(config, args, explicit.as_deref())?;
            let mut data = String::new();
            if input == "-" {
                io::stdin().read_to_string(&mut data)?;
            } else {
                data = std::fs::read_to_string(input)
                    .with_context(|| format!("Failed to read comments file: {input}"))?;
            }
            let mut app = review_app_for_target(config, args, target.as_deref(), true)?;
            let ids = app
                .apply_review_comments_from_cli(&data)
                .map_err(|error| anyhow!(error))?;
            if *json {
                mutation_json(&app, ids)
            } else {
                print_review_target_header(&app);
                println!("Applied {} comments.", ids.len());
                Ok(())
            }
        }
    }
}

fn print_ctl_sessions(sessions: &[control::ControlSessionInfo], json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "sessions": sessions }))?
        );
        return Ok(());
    }
    if sessions.is_empty() {
        println!("No Oyo sessions running.");
        return Ok(());
    }
    println!("{:<20} {:<36} TARGET", "SESSION", "WORKSPACE");
    for session in sessions {
        println!(
            "{:<20} {:<36} {}",
            session.name,
            review_cli_truncate(&session.workspace.display().to_string(), 36),
            session.target
        );
    }
    Ok(())
}

fn print_ctl_response(response: &control::ControlResponse, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(response)?);
    } else if let Some(message) = response.message.as_deref() {
        println!("{message}");
    }
    Ok(())
}

fn ctl_request(command: &CtlCommand) -> Option<control::ControlRequest> {
    match command {
        CtlCommand::List { .. } => None,
        CtlCommand::Get { .. } => Some(control::ControlRequest::Get),
        CtlCommand::Rename { name, .. } => {
            Some(control::ControlRequest::Rename { name: name.clone() })
        }
        CtlCommand::Where { .. } => Some(control::ControlRequest::Where),
        CtlCommand::Diff { include_patch, .. } => Some(control::ControlRequest::Diff {
            include_patch: *include_patch,
        }),
        CtlCommand::Next { count } => Some(control::ControlRequest::Next { count: *count }),
        CtlCommand::Prev { count } => Some(control::ControlRequest::Prev { count: *count }),
        CtlCommand::Hunk { mode, count } => Some(control::ControlRequest::Hunk {
            mode: mode.clone(),
            count: *count,
        }),
        CtlCommand::File {
            target,
            new_tab,
            count,
        } => Some(control::ControlRequest::File {
            target: target.clone(),
            new_tab: *new_tab,
            count: *count,
        }),
        CtlCommand::Goto {
            file,
            new_line,
            old_line,
            hunk,
            step_number,
            start,
            end,
        } => Some(control::ControlRequest::Goto {
            file: file.clone(),
            new_line: *new_line,
            old_line: *old_line,
            hunk: *hunk,
            step: *step_number,
            start: *start,
            end: *end,
        }),
        CtlCommand::Target {
            target,
            worktree,
            staged,
        } => Some(control::ControlRequest::Target {
            target: target.clone(),
            worktree: *worktree,
            staged: *staged,
        }),
        CtlCommand::View { mode } => Some(control::ControlRequest::View { mode: mode.clone() }),
        CtlCommand::Step { mode } => Some(control::ControlRequest::Step { mode: mode.clone() }),
        CtlCommand::Watch { mode } => Some(control::ControlRequest::Watch { mode: mode.clone() }),
        CtlCommand::Speed { mode } => Some(control::ControlRequest::Speed { mode: mode.clone() }),
        CtlCommand::Animation { mode } => {
            Some(control::ControlRequest::Animation { mode: mode.clone() })
        }
        CtlCommand::Wrap { mode } => Some(control::ControlRequest::Wrap { mode: mode.clone() }),
        CtlCommand::Syntax { mode } => Some(control::ControlRequest::Syntax { mode: mode.clone() }),
        CtlCommand::Zen { mode } => Some(control::ControlRequest::Zen { mode: mode.clone() }),
        CtlCommand::Sidebar { mode } => {
            Some(control::ControlRequest::Sidebar { mode: mode.clone() })
        }
        CtlCommand::Tab { kind, file } => Some(control::ControlRequest::Tab {
            kind: kind.clone(),
            file: file.clone(),
        }),
        CtlCommand::Play { .. } => None,
        CtlCommand::Pause => Some(control::ControlRequest::Pause),
        CtlCommand::Cancel => Some(control::ControlRequest::Cancel),
        CtlCommand::Action { id, count } => Some(control::ControlRequest::Action {
            id: id.clone(),
            count: *count,
        }),
    }
}

fn ctl_json(command: &CtlCommand) -> bool {
    match command {
        CtlCommand::List { json }
        | CtlCommand::Get { json }
        | CtlCommand::Rename { json, .. }
        | CtlCommand::Where { json }
        | CtlCommand::Diff { json, .. } => *json,
        _ => false,
    }
}

fn parse_ctl_delay(value: &str) -> Result<Duration> {
    let value = value.trim();
    if let Some(ms) = value.strip_suffix("ms") {
        return Ok(Duration::from_millis(ms.trim().parse()?));
    }
    if let Some(seconds) = value.strip_suffix('s') {
        let seconds = seconds.trim().parse::<f64>()?;
        if !seconds.is_finite() || seconds < 0.0 {
            anyhow::bail!("Delay must be a positive duration");
        }
        return Ok(Duration::from_secs_f64(seconds));
    }
    Ok(Duration::from_millis(value.parse()?))
}

fn handle_ctl_command(command: &Command, args: &Args) -> Result<()> {
    let Command::Control { command, session } = command else {
        return Ok(());
    };
    let default_command = CtlCommand::List { json: false };
    let command = command.as_ref().unwrap_or(&default_command);
    let json = ctl_json(command);
    if matches!(command, CtlCommand::List { .. }) {
        return print_ctl_sessions(&control::list_sessions(), json);
    }
    let workspace = current_review_workspace().ok();
    let session_name = session.as_deref().or(args.session.as_deref());
    let session = control::resolve_session(session_name, workspace.as_deref())?;
    if let CtlCommand::Play { from, to, delay } = command {
        let delay_ms = parse_ctl_delay(delay)?
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let response = control::send_request(
            &session,
            &control::ControlRequest::Play {
                from: from.clone(),
                to: to.clone(),
                delay_ms,
            },
        )?;
        return print_ctl_response(&response, false);
    }
    let request = ctl_request(command).expect("control request");
    let response = control::send_request(&session, &request)?;
    print_ctl_response(&response, json)
}

fn review_outdated_filter(outdated: bool, no_outdated: bool) -> Option<bool> {
    if outdated {
        Some(true)
    } else if no_outdated {
        Some(false)
    } else {
        None
    }
}

fn handle_review_command(command: &Command, config: &config::Config, args: &Args) -> Result<()> {
    let Command::Review { json, command } = command else {
        return Ok(());
    };
    match command {
        None => print_review_log(config, args, *json),
        Some(ReviewCommand::Log { json }) => print_review_log(config, args, *json),
        Some(ReviewCommand::Status {
            revision,
            target,
            unresolved,
            outdated,
            no_outdated,
            ids,
            since,
            json,
        }) => {
            let explicit = combine_review_targets(&[revision.as_deref(), target.as_deref()])?;
            let target = resolve_review_target(config, args, explicit.as_deref())?;
            let mut app = review_app_for_target(config, args, target.as_deref(), false)?;
            load_jj_revset_snapshots_for_read(&mut app, config, args, target.as_deref())?;
            let filter = ReviewCommentFilter {
                unresolved: *unresolved,
                outdated: review_outdated_filter(*outdated, *no_outdated),
                ids: ids.clone(),
                since: *since,
                ..ReviewCommentFilter::default()
            };
            print_review_status(&app, *json, &filter)
        }
        Some(ReviewCommand::Comment {
            revision,
            target,
            unresolved,
            outdated,
            no_outdated,
            author,
            author_type,
            ids,
            since,
            json,
            command,
        }) => {
            let filter = ReviewCommentFilter {
                unresolved: *unresolved,
                outdated: review_outdated_filter(*outdated, *no_outdated),
                author: author.clone(),
                author_type: review_author_type(author_type.as_deref())?,
                since: *since,
                ids: ids.clone(),
            };
            let target = combine_review_targets(&[revision.as_deref(), target.as_deref()])?;
            handle_review_comment_command(command, target.as_deref(), *json, &filter, config, args)
        }
        Some(ReviewCommand::Export {
            revision,
            target,
            format,
            output,
        }) => {
            let explicit = combine_review_targets(&[revision.as_deref(), target.as_deref()])?;
            let target = resolve_review_target(config, args, explicit.as_deref())?;
            let mut app = review_app_for_target(config, args, target.as_deref(), false)?;
            load_jj_revset_snapshots_for_read(&mut app, config, args, target.as_deref())?;
            export_review(&app, *format, output.as_deref())
        }
        Some(ReviewCommand::Pull {
            args: items,
            target,
            json,
        }) => handle_review_pull_command(items, target.as_deref(), *json, config, args),
        Some(ReviewCommand::Push {
            args: items,
            target,
            json,
        }) => handle_review_push_command(items, target.as_deref(), *json, config, args),
        Some(ReviewCommand::Abandon {
            revision,
            target,
            json,
        }) => {
            let explicit = combine_review_targets(&[revision.as_deref(), target.as_deref()])?;
            let target = resolve_review_target(config, args, explicit.as_deref())?;
            let mut app = review_app_for_target(config, args, target.as_deref(), false)?;
            let removed = app.abandon_review_from_cli();
            if *json {
                abandon_json(&app, removed)
            } else if removed {
                print_review_target_header(&app);
                println!("Abandoned review.");
                Ok(())
            } else {
                print_review_target_header(&app);
                println!("No saved review.");
                Ok(())
            }
        }
    }
}

fn build_diff_from_input_mode(
    input_mode: &InputMode,
    config: &config::Config,
    args: &Args,
) -> Result<Option<BuiltDiff>> {
    let (mut multi_diff, git_branch, workspace_root) = match input_mode {
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
            (diff, branch, Some(cwd))
        }
        InputMode::TwoPaths { old_path, new_path } => {
            let mut workspace_root = None;
            let diff = if old_path.is_dir() && new_path.is_dir() {
                let scan_options = directory_scan_options(config, args, old_path, new_path);
                let mut diff =
                    MultiFileDiff::from_directories_with_options(old_path, new_path, &scan_options)
                        .context("Failed to create diff from directories")?;
                if looks_like_jj_external_diff_dirs(old_path, new_path) {
                    if let Some(root) = infer_external_diff_workspace_root() {
                        workspace_root = Some(root.clone());
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
            (diff, None, workspace_root)
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
            (diff, branch, Some(repo_root))
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
            let branch = oyo_core::git::get_current_branch(&repo_root).ok();
            let diff = if changes.len() > 4 {
                MultiFileDiff::from_git_changes_lazy(repo_root.clone(), changes)
            } else {
                MultiFileDiff::from_git_changes(repo_root.clone(), changes)
            }
            .context("Failed to create diff from git changes")?;
            (diff, branch, Some(repo_root))
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
            let branch = oyo_core::git::get_current_branch(&repo_root).ok();
            let diff = if changes.len() > 4 {
                MultiFileDiff::from_git_staged_lazy(repo_root.clone(), changes)
            } else {
                MultiFileDiff::from_git_staged(repo_root.clone(), changes)
            }
            .context("Failed to create diff from staged changes")?;
            (diff, branch, Some(repo_root))
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
            let (_changes, diff) = if is_index_from || is_index_to {
                let (commit, to_index) = if is_index_to {
                    (from.clone(), true)
                } else {
                    (to.clone(), false)
                };
                let reverse = !to_index;
                let changes =
                    oyo_core::git::get_changes_between_index(&repo_root, &commit, reverse)
                        .context("Failed to get index range changes")?;
                let diff = if changes.len() > 4 {
                    MultiFileDiff::from_git_index_range_lazy(
                        repo_root.clone(),
                        changes.clone(),
                        commit,
                        to_index,
                    )
                } else {
                    MultiFileDiff::from_git_index_range(
                        repo_root.clone(),
                        changes.clone(),
                        commit,
                        to_index,
                    )
                }
                .context("Failed to create diff from index range")?;
                (changes, diff)
            } else {
                let changes = oyo_core::git::get_changes_between(&repo_root, from, to)
                    .context("Failed to get range changes")?;
                let diff = if changes.len() > 4 {
                    MultiFileDiff::from_git_range_lazy(
                        repo_root.clone(),
                        changes.clone(),
                        from.clone(),
                        to.clone(),
                    )
                } else {
                    MultiFileDiff::from_git_range(
                        repo_root.clone(),
                        changes.clone(),
                        from.clone(),
                        to.clone(),
                    )
                }
                .context("Failed to create diff from range")?;
                (changes, diff)
            };
            let branch = oyo_core::git::get_current_branch(&repo_root).ok();
            (diff, branch, Some(repo_root))
        }
        InputMode::JjRevision { rev } => {
            let cwd = std::env::current_dir().unwrap_or_default();
            let repo_root = jj_workspace_root(&cwd).context("Not in a jj workspace")?;
            let diff = build_jj_diff(&repo_root, rev, None)?;
            (diff, Some(rev.clone()), Some(repo_root))
        }
        InputMode::JjFile { path } => {
            let cwd = std::env::current_dir().unwrap_or_default();
            let repo_root = jj_workspace_root(&cwd).context("Not in a jj workspace")?;
            let rev = default_jj_review_revision();
            let diff = build_jj_diff(&repo_root, &rev, Some(path))?;
            (diff, Some(rev), Some(repo_root))
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

    if let Some(request) = multi_diff.take_pending_content_for(multi_diff.selected_index) {
        let content = app::load_content_request_sync(request);
        multi_diff.apply_prepared_content(multi_diff.selected_index, content);
        multi_diff.ensure_full_navigator(multi_diff.selected_index);
    }

    Ok(Some(BuiltDiff {
        multi_diff,
        branch: git_branch,
        workspace_root,
    }))
}

fn main() {
    if let Err(error) = run() {
        let mut message = error.to_string();
        while let Some(stripped) = message.strip_prefix("Error: ") {
            message = stripped.to_string();
        }
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    install_panic_terminal_restore();
    let args = Args::parse();
    let view_limit = match &args.command {
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
        Some(Command::Log { limit }) => Some(*limit),
        Some(Command::Skill { .. })
        | Some(Command::Control { .. })
        | Some(Command::Review { .. })
        | None => None,
    };
    let mut config = if args.config_files.is_empty() {
        config::Config::load()
    } else {
        config::Config::load_with_extra(&args.config_files).map_err(|e| anyhow!(e))?
    };
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

    if let Some(Command::Skill { command }) = args.command.as_ref() {
        let default_command = SkillCommand::Path {
            skill: SkillName::Review,
        };
        match command.as_ref().unwrap_or(&default_command) {
            SkillCommand::Path { skill } => println!("{}", oyo_skill_path(*skill)?.display()),
        }
        return Ok(());
    }

    if let Some(command @ Command::Control { .. }) = args.command.as_ref() {
        return handle_ctl_command(command, &args);
    }

    if let Some(command @ Command::Review { .. }) = args.command.as_ref() {
        return handle_review_command(command, &config, &args);
    }

    if let Some(limit) = view_limit {
        let mut terminal = setup_terminal()?;
        let image_picker = setup_image_picker();
        let mut input_mode =
            match run_commit_picker(&mut terminal, &config, light_mode, limit, None, None)? {
                Some(mode) => mode,
                None => {
                    restore_terminal(&mut terminal)?;
                    return Ok(());
                }
            };

        let mut exit_message: Option<String> = None;
        let mut review_hook_warnings = Vec::new();
        let mut runtime_theme: Option<(config::ResolvedTheme, Option<String>)> = None;
        let mut control_last_applied_seq = 0;
        loop {
            let empty_message = match &input_mode {
                InputMode::GitUncommitted => Some("No uncommitted changes found.".to_string()),
                InputMode::GitStaged => Some("No staged changes found.".to_string()),
                InputMode::GitRange { from, to } => {
                    Some(format!("No changes in range {}..{}.", from, to))
                }
                _ => Some("No changes found.".to_string()),
            };
            let built = match build_diff_from_input_mode(&input_mode, &config, &args) {
                Ok(Some(result)) => result,
                Ok(None) => {
                    exit_message = empty_message;
                    break;
                }
                Err(error) => {
                    restore_terminal(&mut terminal)?;
                    return Err(error);
                }
            };

            let view_mode: ViewMode = args.view.into();
            let view_mode = config.parse_view_mode().unwrap_or(view_mode);
            let speed = if args.speed != 200 {
                args.speed
            } else {
                config.playback.speed
            };
            let autoplay = args.autoplay || config.playback.autoplay;

            let mut app = App::new(built.multi_diff, view_mode, speed, autoplay, built.branch);
            if let (InputMode::JjRevision { rev }, Some(repo_root)) =
                (&input_mode, built.workspace_root.as_ref())
            {
                app.set_jj_watch_target(repo_root.clone(), rev.clone());
            }
            if let Some(picker) = image_picker.as_ref() {
                app.set_image_picker(picker.clone());
            }
            app.no_changes_message = empty_message.clone();
            apply_config_to_app(&mut app, &config, &args, light_mode);
            if let Some((theme, name)) = &runtime_theme {
                app.theme = theme.clone();
                app.ui_theme_name = name.clone();
            }
            configure_review_state_for_app(
                &mut app,
                &config,
                &args,
                built.workspace_root,
                &input_mode,
                None,
                true,
            )?;
            app.start_content_loading();

            let exit = run_app(
                &mut terminal,
                &mut app,
                &config,
                &args,
                control_last_applied_seq,
            )?;
            review_hook_warnings.extend(app.take_review_hook_warnings());
            runtime_theme = Some((app.theme.clone(), app.ui_theme_name.clone()));
            match exit {
                AppExit::Quit => break,
                AppExit::Reload(mode, seq) => {
                    input_mode = mode;
                    control_last_applied_seq = seq;
                }
                AppExit::OpenDashboard => {
                    let Some(mode) = run_commit_picker(
                        &mut terminal,
                        &config,
                        light_mode,
                        limit,
                        Some(&input_mode),
                        runtime_theme.as_ref().map(|(theme, _)| theme),
                    )?
                    else {
                        break;
                    };
                    input_mode = mode;
                }
            }
        }

        restore_terminal(&mut terminal)?;
        for warning in review_hook_warnings {
            eprintln!("Warning: {warning}");
        }
        if let Some(message) = exit_message {
            println!("{message}");
        }
        return Ok(());
    }

    let mut input_mode = if args.paths.len() == 7 {
        detect_input_mode(&args.paths)
    } else if args.worktree || args.staged || args.range.is_some() {
        if !args.paths.is_empty() {
            anyhow::bail!("--worktree/--staged/--range cannot be used with file paths");
        }
        if let Some(range) = args.range.as_deref() {
            let (from, to) = parse_range(range)?;
            InputMode::GitRange { from, to }
        } else if args.staged {
            InputMode::GitStaged
        } else {
            InputMode::GitUncommitted
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
    let prefetched = match build_diff_from_input_mode(&input_mode, &config, &args) {
        Ok(Some(result)) => result,
        Ok(None) => {
            if let Some(message) = empty_message {
                println!("{message}");
            }
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let mut terminal = setup_terminal()?;
    let image_picker = setup_image_picker();
    let dashboard_limit = view_limit.unwrap_or(200);

    let mut exit_message: Option<String> = None;
    let mut review_hook_warnings = Vec::new();
    let mut runtime_theme: Option<(config::ResolvedTheme, Option<String>)> = None;
    let mut pending_diff = Some(prefetched);
    let mut control_last_applied_seq = 0;
    loop {
        let empty_message = match &input_mode {
            InputMode::GitUncommitted => Some("No uncommitted changes found.".to_string()),
            InputMode::GitStaged => Some("No staged changes found.".to_string()),
            InputMode::GitRange { from, to } => {
                Some(format!("No changes in range {}..{}.", from, to))
            }
            _ => Some("No changes found.".to_string()),
        };
        let built = if let Some(result) = pending_diff.take() {
            result
        } else {
            match build_diff_from_input_mode(&input_mode, &config, &args) {
                Ok(Some(result)) => result,
                Ok(None) => {
                    exit_message = empty_message;
                    break;
                }
                Err(error) => {
                    restore_terminal(&mut terminal)?;
                    return Err(error);
                }
            }
        };

        let view_mode: ViewMode = args.view.into();
        let view_mode = config.parse_view_mode().unwrap_or(view_mode);
        let speed = if args.speed != 200 {
            args.speed
        } else {
            config.playback.speed
        };
        let autoplay = args.autoplay || config.playback.autoplay;

        let mut app = App::new(built.multi_diff, view_mode, speed, autoplay, built.branch);
        if let (InputMode::JjRevision { rev }, Some(repo_root)) =
            (&input_mode, built.workspace_root.as_ref())
        {
            app.set_jj_watch_target(repo_root.clone(), rev.clone());
        }
        if let Some(picker) = image_picker.as_ref() {
            app.set_image_picker(picker.clone());
        }
        app.no_changes_message = empty_message.clone();
        apply_config_to_app(&mut app, &config, &args, light_mode);
        if let Some((theme, name)) = &runtime_theme {
            app.theme = theme.clone();
            app.ui_theme_name = name.clone();
        }
        configure_review_state_for_app(
            &mut app,
            &config,
            &args,
            built.workspace_root,
            &input_mode,
            None,
            true,
        )?;
        app.start_content_loading();

        let exit = run_app(
            &mut terminal,
            &mut app,
            &config,
            &args,
            control_last_applied_seq,
        )?;
        review_hook_warnings.extend(app.take_review_hook_warnings());
        runtime_theme = Some((app.theme.clone(), app.ui_theme_name.clone()));
        match exit {
            AppExit::Quit => break,
            AppExit::Reload(mode, seq) => {
                input_mode = mode;
                pending_diff = None;
                control_last_applied_seq = seq;
            }
            AppExit::OpenDashboard => {
                let Some(mode) = run_commit_picker(
                    &mut terminal,
                    &config,
                    light_mode,
                    dashboard_limit,
                    Some(&input_mode),
                    runtime_theme.as_ref().map(|(theme, _)| theme),
                )?
                else {
                    break;
                };
                input_mode = mode;
                pending_diff = None;
            }
        }
    }

    restore_terminal(&mut terminal)?;
    for warning in review_hook_warnings {
        eprintln!("Warning: {warning}");
    }
    if let Some(message) = exit_message {
        println!("{message}");
    }

    Ok(())
}

fn run_app(
    terminal: &mut TuiTerminal,
    app: &mut App,
    config: &config::Config,
    args: &Args,
    control_last_applied_seq: u64,
) -> Result<AppExit> {
    let editor_config = &config.editor;
    let mut pending_event: Option<Event> = None;
    let mut needs_draw = true;
    let mut scroll_draw_pending = false;
    let mut last_scroll_draw = Instant::now() - MOUSE_SCROLL_FRAME;
    let mut pending_mouse_scroll: Option<PendingMouseScroll> = None;
    let mut blocked_mouse_scroll: Option<BlockedMouseScroll> = None;
    let mut review_sync_worker: Option<ReviewSyncWorker> = None;
    let mut review_sync_pull_stats: Option<ReviewPullStats> = None;
    let mut review_pr_lookup_worker = app
        .review_pull_request_lookup_needed()
        .then(|| spawn_review_pr_lookup_worker(app.review_pull_request_lookup_target()));
    let workspace = app
        .review_workspace_root()
        .map(PathBuf::from)
        .or_else(|| current_review_workspace().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let mut control_session = match control::ControlSession::start(
        &workspace,
        &app.control_target_label(),
        args.session.as_deref(),
        control_last_applied_seq,
    ) {
        Ok(session) => Some(session),
        Err(error) => {
            app.notify(ToastEvent::SelectionActionFailed(format!(
                "Control disabled: {error}"
            )));
            None
        }
    };
    app.set_control_session_name(
        control_session
            .as_ref()
            .map(|session| session.name().to_string()),
    );

    loop {
        if scroll_draw_pending && last_scroll_draw.elapsed() >= MOUSE_SCROLL_FRAME {
            needs_draw = true;
        }
        if needs_draw {
            let scroll_before_draw = app.scroll_offset;
            let applied_mouse_scroll = pending_mouse_scroll;
            if apply_pending_mouse_scroll(app, &mut pending_mouse_scroll) {
                scroll_draw_pending = false;
            }
            terminal
                .draw(|f| ui::draw(f, app))
                .map_err(|e| anyhow!("{e}"))?;
            needs_draw = false;
            last_scroll_draw = Instant::now();
            update_mouse_scroll_block(
                &mut blocked_mouse_scroll,
                applied_mouse_scroll,
                scroll_before_draw,
                app.scroll_offset,
            );

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

        let poll_timeout = if scroll_draw_pending {
            MOUSE_SCROLL_FRAME
                .checked_sub(last_scroll_draw.elapsed())
                .unwrap_or_default()
        } else {
            app.redraw_interval()
        };
        let event = if let Some(event) = pending_event.take() {
            Some(event)
        } else if event::poll(poll_timeout)? {
            Some(event::read()?)
        } else {
            None
        };

        if let Some(event) = event {
            if let Some(session) = control_session.as_mut() {
                session.preempt(app);
            }
            app.mark_user_input();
            needs_draw = true;
            if !is_mouse_scroll_event(&event) {
                blocked_mouse_scroll = None;
                if apply_pending_mouse_scroll(app, &mut pending_mouse_scroll) {
                    scroll_draw_pending = false;
                }
            }
            match event {
                Event::Mouse(me) => {
                    if app.show_help || app.show_path_popup {
                        continue;
                    }
                    app.reset_count();
                    app.remember_diff_line_hover(me.column, me.row);
                    if let Some(button) = toast_mouse_button(me.kind) {
                        if app.handle_toast_click(me.column, me.row, button) {
                            continue;
                        }
                    }
                    if app.status_mode_menu_open() {
                        match me.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                app.handle_status_mode_menu_click(me.column, me.row);
                                continue;
                            }
                            MouseEventKind::Down(MouseButton::Right) => {
                                if !app.open_status_mode_menu(me.column, me.row) {
                                    app.close_status_mode_menu();
                                }
                                continue;
                            }
                            MouseEventKind::Moved => {
                                if !app.update_status_mode_menu_hover(me.column, me.row) {
                                    needs_draw = false;
                                }
                                continue;
                            }
                            MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                            | MouseEventKind::ScrollLeft
                            | MouseEventKind::ScrollRight => {
                                app.close_status_mode_menu();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    if app.review_comment_context_menu.is_some() {
                        match me.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                app.handle_review_comment_context_menu_click(me.column, me.row);
                                continue;
                            }
                            MouseEventKind::Moved => {
                                if !app.update_review_comment_context_menu_hover(me.column, me.row)
                                {
                                    needs_draw = false;
                                }
                                continue;
                            }
                            MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                            | MouseEventKind::ScrollLeft
                            | MouseEventKind::ScrollRight => {
                                app.close_review_comment_context_menu();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    if app.file_context_menu.is_some() {
                        match me.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                app.handle_file_context_menu_click(me.column, me.row);
                                continue;
                            }
                            MouseEventKind::Down(MouseButton::Right) => {
                                if !app.open_file_context_menu(me.column, me.row) {
                                    app.close_file_context_menu();
                                }
                                continue;
                            }
                            MouseEventKind::Moved => {
                                if !app.update_file_context_menu_hover(me.column, me.row) {
                                    needs_draw = false;
                                }
                                continue;
                            }
                            MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                            | MouseEventKind::ScrollLeft
                            | MouseEventKind::ScrollRight => {
                                app.close_file_context_menu();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    if app.command_palette_active() {
                        match me.kind {
                            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                                if queue_mouse_scroll(
                                    app,
                                    MouseScrollTarget::CommandPalette,
                                    me.kind,
                                    &mut pending_event,
                                    &mut pending_mouse_scroll,
                                    &mut blocked_mouse_scroll,
                                    last_scroll_draw,
                                )? {
                                    schedule_mouse_scroll_draw(
                                        &mut needs_draw,
                                        &mut scroll_draw_pending,
                                        last_scroll_draw,
                                    );
                                } else {
                                    needs_draw = false;
                                    scroll_draw_pending = false;
                                }
                            }
                            MouseEventKind::Down(MouseButton::Left)
                                if !app.handle_command_palette_click(me.column, me.row) =>
                            {
                                app.stop_command_palette();
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.file_search_active() {
                        match me.kind {
                            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                                if queue_mouse_scroll(
                                    app,
                                    MouseScrollTarget::FileSearch,
                                    me.kind,
                                    &mut pending_event,
                                    &mut pending_mouse_scroll,
                                    &mut blocked_mouse_scroll,
                                    last_scroll_draw,
                                )? {
                                    schedule_mouse_scroll_draw(
                                        &mut needs_draw,
                                        &mut scroll_draw_pending,
                                        last_scroll_draw,
                                    );
                                } else {
                                    needs_draw = false;
                                    scroll_draw_pending = false;
                                }
                            }
                            MouseEventKind::Down(MouseButton::Left)
                                if !app.handle_file_search_click(me.column, me.row) =>
                            {
                                app.stop_file_search();
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.comment_picker_active() {
                        match me.kind {
                            MouseEventKind::ScrollUp => app.move_comment_picker_selection(-1),
                            MouseEventKind::ScrollDown => app.move_comment_picker_selection(1),
                            MouseEventKind::Down(MouseButton::Left)
                                if !app.handle_comment_picker_click(me.column, me.row) =>
                            {
                                app.stop_comment_picker();
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.theme_picker_active() {
                        match me.kind {
                            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                                if matches!(me.kind, MouseEventKind::ScrollUp) {
                                    app.move_theme_picker_selection(-1);
                                } else {
                                    app.move_theme_picker_selection(1);
                                }
                            }
                            MouseEventKind::Down(MouseButton::Left)
                                if !app.handle_theme_picker_click(me.column, me.row) =>
                            {
                                app.stop_theme_picker();
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.session_rename_active() {
                        continue;
                    }
                    if app.review_remote_picker_active() {
                        match me.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                app.handle_review_remote_picker_click(me.column, me.row);
                            }
                            MouseEventKind::Moved
                                if !app.update_review_remote_picker_hover(me.column, me.row) =>
                            {
                                needs_draw = false;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.quit_confirmation_active() {
                        match me.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                app.handle_quit_confirmation_click(me.column, me.row);
                            }
                            MouseEventKind::Moved
                                if !app
                                    .update_review_delete_confirmation_hover(me.column, me.row) =>
                            {
                                needs_draw = false;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if app.review_delete_confirmation_active() {
                        match me.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                app.handle_review_delete_confirmation_click(me.column, me.row);
                            }
                            MouseEventKind::Moved
                                if !app
                                    .update_review_delete_confirmation_hover(me.column, me.row) =>
                            {
                                needs_draw = false;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    match me.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if app.handle_search_bar_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_no_changes_dashboard_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_no_changes_quit_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_selection_toolbar_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_review_editor_toolbar_click(me.column, me.row) {
                                continue;
                            }
                            if app.dismiss_selection_toolbar_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_status_comments_mouse_down(me.column, me.row) {
                                continue;
                            }
                            if app.handle_status_file_mouse_down(me.column, me.row) {
                                continue;
                            }
                            if app.handle_status_bar_mouse_down(
                                me.column,
                                me.row,
                                me.modifiers.contains(KeyModifiers::CONTROL),
                            ) {
                                continue;
                            }
                            if app.handle_topbar_mouse_down(me.column, me.row) {
                                continue;
                            }
                            if app.start_file_panel_resize(me.column, me.row) {
                                continue;
                            }
                            if app.start_file_panel_scrollbar_drag(me.column, me.row) {
                                continue;
                            }
                            if app.start_diff_scrollbar_drag(me.column, me.row) {
                                continue;
                            }
                            if app.handle_fold_context_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_review_line_add_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_pr_comment_view_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_review_preview_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_review_file_comment_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_preview_link_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_binary_preview_click(me.column, me.row) {
                                continue;
                            }
                            if app.handle_structured_preview_click(me.column, me.row) {
                                continue;
                            }
                            if app.start_diff_selection(me.column, me.row) {
                                continue;
                            }
                            if app.handle_file_list_click(
                                me.column,
                                me.row,
                                me.modifiers.contains(KeyModifiers::CONTROL),
                            ) {
                                continue;
                            }
                        }
                        MouseEventKind::Down(MouseButton::Right)
                            if app.open_status_mode_menu(me.column, me.row) =>
                        {
                            continue;
                        }
                        MouseEventKind::Down(MouseButton::Right)
                            if app.open_file_context_menu(me.column, me.row) =>
                        {
                            continue;
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if app.drag_topbar_tab(me.column, me.row) {
                                continue;
                            }
                            if app.drag_file_panel_scrollbar(me.row) {
                                continue;
                            }
                            if app.drag_diff_scrollbar(me.row) {
                                continue;
                            }
                            if app.drag_diff_selection(me.column, me.row) {
                                continue;
                            }
                            if let Ok((cols, _)) = crossterm::terminal::size() {
                                if app.drag_file_panel_resize(me.column, cols) {
                                    continue;
                                }
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            if app.finish_topbar_drag() {
                                continue;
                            }
                            if app.finish_file_panel_scrollbar_drag() {
                                continue;
                            }
                            if app.finish_diff_scrollbar_drag() {
                                continue;
                            }
                            if app.finish_diff_selection(me.column, me.row) {
                                continue;
                            }
                            app.end_file_panel_resize();
                        }
                        MouseEventKind::Moved => {
                            let search_changed = app.update_search_bar_hover(me.column, me.row);
                            let topbar_changed = app.update_topbar_hover(me.column, me.row);
                            if !search_changed && !topbar_changed {
                                needs_draw = false;
                            }
                        }
                        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                            if app.mouse_over_selection_toolbar(me.column, me.row) =>
                        {
                            let delta = if matches!(me.kind, MouseEventKind::ScrollLeft) {
                                -1
                            } else {
                                1
                            };
                            needs_draw = app.scroll_selection_toolbar_actions(delta);
                        }
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                            if app.mouse_over_selection_toolbar(me.column, me.row) =>
                        {
                            let delta = if matches!(me.kind, MouseEventKind::ScrollUp) {
                                -1
                            } else {
                                1
                            };
                            needs_draw = app.scroll_selection_toolbar_actions(delta);
                        }
                        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                            if app.mouse_over_review_editor_toolbar(me.column, me.row) =>
                        {
                            let delta = if matches!(me.kind, MouseEventKind::ScrollLeft) {
                                -1
                            } else {
                                1
                            };
                            needs_draw = app.scroll_review_editor_toolbar(delta);
                        }
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                            if app.mouse_over_review_editor_toolbar(me.column, me.row) =>
                        {
                            let delta = if matches!(me.kind, MouseEventKind::ScrollUp) {
                                -1
                            } else {
                                1
                            };
                            needs_draw = app.scroll_review_editor_toolbar(delta);
                        }
                        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                            if app.mouse_over_topbar(me.column, me.row) =>
                        {
                            let delta = if matches!(me.kind, MouseEventKind::ScrollLeft) {
                                -1
                            } else {
                                1
                            };
                            needs_draw = app.scroll_topbar_tabs(delta);
                        }
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                            if app.mouse_over_topbar(me.column, me.row) =>
                        {
                            let delta = if matches!(me.kind, MouseEventKind::ScrollUp) {
                                -1
                            } else {
                                1
                            };
                            needs_draw = app.scroll_topbar_tabs(delta);
                        }
                        kind if app.mouse_over_diff_view(me.column, me.row)
                            && mouse_horizontal_scroll_delta(kind, me.modifiers).is_some() =>
                        {
                            needs_draw = app.scroll_diff_horizontally(
                                mouse_horizontal_scroll_delta(kind, me.modifiers).unwrap_or(0),
                            );
                        }
                        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                            let delta =
                                mouse_horizontal_scroll_delta(me.kind, me.modifiers).unwrap_or(0);
                            needs_draw = app.scroll_diff_horizontally(delta);
                        }
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                            let target = if app.mouse_over_file_panel(me.column, me.row) {
                                MouseScrollTarget::FilePanel
                            } else if app.view_mode != ViewMode::Preview
                                && app.stepping
                                && app.current_file_diff_ready()
                            {
                                MouseScrollTarget::Step
                            } else {
                                MouseScrollTarget::Diff
                            };
                            if queue_mouse_scroll(
                                app,
                                target,
                                me.kind,
                                &mut pending_event,
                                &mut pending_mouse_scroll,
                                &mut blocked_mouse_scroll,
                                last_scroll_draw,
                            )? {
                                schedule_mouse_scroll_draw(
                                    &mut needs_draw,
                                    &mut scroll_draw_pending,
                                    last_scroll_draw,
                                );
                            } else {
                                needs_draw = false;
                                scroll_draw_pending = false;
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
        } else if scroll_draw_pending {
            needs_draw = true;
        }

        if app.tick() && !(scroll_draw_pending && last_scroll_draw.elapsed() < MOUSE_SCROLL_FRAME) {
            needs_draw = true;
        }
        if let Some(name) = app.take_pending_session_rename() {
            if let Some(session) = control_session.as_mut() {
                match session.rename(&name) {
                    Ok(info) => {
                        app.set_control_session_name(Some(info.name));
                        app.notify(ToastEvent::SessionRenamed);
                    }
                    Err(error) => app.notify(ToastEvent::SelectionActionFailed(error.to_string())),
                }
            }
            needs_draw = true;
        }
        if let Some(session) = control_session.as_mut() {
            let poll = session.poll(app);
            if poll.redraw {
                needs_draw = true;
            }
            if let Some((target, seq)) = poll.target {
                match input_mode_for_control_target(target, args) {
                    Ok(mode) => match build_diff_from_input_mode(&mode, config, args)? {
                        Some(_) => return Ok(AppExit::Reload(mode, seq)),
                        None => app.notify(ToastEvent::SelectionActionFailed(
                            "Target has no changes".to_string(),
                        )),
                    },
                    Err(error) => app.notify(ToastEvent::SelectionActionFailed(format!(
                        "Target failed: {error}"
                    ))),
                }
                needs_draw = true;
            }
        }

        let pr_lookup_result =
            review_pr_lookup_worker
                .as_ref()
                .and_then(|worker| match worker.rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err(anyhow!("Review request lookup stopped.")))
                    }
                });
        if let Some(result) = pr_lookup_result {
            review_pr_lookup_worker = None;
            if let Ok(pr) = result {
                app.set_review_pull_request_target(Some(review_pull_request_target(&pr)));
                needs_draw = true;
            }
        }

        let worker_result =
            review_sync_worker
                .as_ref()
                .and_then(|worker| match worker.rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err(anyhow!("Review sync worker stopped.")))
                    }
                });
        if let Some(result) = worker_result {
            review_sync_worker = None;
            match result {
                Ok(ReviewSyncWorkerResult::Pull { action, data }) => {
                    let pr = data.pr().clone();
                    let user = data.user().clone();
                    app.set_review_author_provider_avatar(
                        pr.provider.id(),
                        &user.login,
                        user.avatar_url.clone(),
                    );
                    app.set_review_pull_request_target(Some(review_pull_request_target(&pr)));
                    app.set_review_target_metadata(Some(review_pr_target_metadata(&pr)));
                    match apply_provider_comments_to_app(app, *data) {
                        Ok(pull) if action == ReviewSyncAction::Sync => {
                            review_sync_pull_stats = Some(pull);
                            review_sync_worker = Some(spawn_review_push_worker(
                                action,
                                pr,
                                user,
                                app.review_comments_for_sync(),
                            ));
                        }
                        Ok(pull) => {
                            app.mark_review_session_clean();
                            app.set_review_sync_status(None);
                            app.notify(ToastEvent::SelectionActionStarted(format!(
                                "Pulled {} comments",
                                pull.pulled
                            )));
                        }
                        Err(error) => {
                            app.set_review_sync_status(None);
                            app.notify(ToastEvent::SelectionActionFailed(format!(
                                "Pull failed: {error}"
                            )));
                        }
                    }
                }
                Ok(ReviewSyncWorkerResult::Push {
                    action,
                    provider,
                    user,
                    outcome,
                }) => {
                    app.set_review_author_provider_avatar(
                        provider.id(),
                        &user.login,
                        user.avatar_url.clone(),
                    );
                    let push = apply_push_outcome_to_app(app, outcome);
                    if push.warnings.is_empty() {
                        app.mark_review_session_clean();
                    }
                    app.set_review_sync_status(None);
                    if action == ReviewSyncAction::Sync {
                        let pull = review_sync_pull_stats.take().unwrap_or(ReviewPullStats {
                            pulled: 0,
                            skipped: 0,
                            changed: Vec::new(),
                        });
                        app.notify(ToastEvent::SelectionActionStarted(format!(
                            "Synced: pulled {}, created {}, updated {}, deleted {}, threads {}",
                            pull.pulled,
                            push.created,
                            push.updated,
                            push.deleted,
                            push.threads_updated
                        )));
                    } else {
                        app.notify(ToastEvent::SelectionActionStarted(format!(
                            "Pushed: created {}, updated {}, deleted {}, threads {}",
                            push.created, push.updated, push.deleted, push.threads_updated
                        )));
                    }
                    for warning in push.warnings {
                        app.notify(ToastEvent::SelectionActionFailed(warning));
                    }
                }
                Err(error) => {
                    review_sync_pull_stats = None;
                    let action = app.review_sync_status().unwrap_or(ReviewSyncAction::Sync);
                    app.set_review_sync_status(None);
                    let label = match action {
                        ReviewSyncAction::Sync => "Sync",
                        ReviewSyncAction::Pull => "Pull",
                        ReviewSyncAction::Push => "Push",
                    };
                    app.notify(ToastEvent::SelectionActionFailed(format!(
                        "{label} failed: {error}"
                    )));
                }
            }
            needs_draw = true;
        }

        if let Some(request) = app.take_review_sync_requested() {
            if request.remote.is_none() {
                match review_remote_options() {
                    Ok(remotes) if remotes.is_empty() => app.notify(
                        ToastEvent::SelectionActionFailed("No Git remotes found".to_string()),
                    ),
                    Ok(remotes) if remotes.len() == 1 => app.request_review_sync_action(
                        request.action,
                        remotes.first().map(|remote| remote.name.clone()),
                    ),
                    Ok(remotes) => app.open_review_remote_picker(request.action, remotes),
                    Err(error) => app.notify(ToastEvent::SelectionActionFailed(format!(
                        "Sync failed: {error}"
                    ))),
                }
            } else if review_sync_worker.is_some() {
                app.notify(ToastEvent::SelectionActionFailed(
                    "Review sync is already running".to_string(),
                ));
            } else {
                app.set_review_sync_status(Some(request.action));
                review_sync_pull_stats = None;
                review_sync_worker = Some(match request.action {
                    ReviewSyncAction::Push => spawn_review_push_request_worker(
                        request.action,
                        request.remote,
                        app.review_comments_for_sync(),
                    ),
                    ReviewSyncAction::Pull | ReviewSyncAction::Sync => {
                        spawn_review_pull_worker(request.action, request.remote)
                    }
                });
            }
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

fn toast_mouse_button(kind: MouseEventKind) -> Option<ratatui_comfy_toaster::ToastMouseButton> {
    match kind {
        MouseEventKind::Down(MouseButton::Left) => {
            Some(ratatui_comfy_toaster::ToastMouseButton::Left)
        }
        MouseEventKind::Down(MouseButton::Right) => {
            Some(ratatui_comfy_toaster::ToastMouseButton::Right)
        }
        _ => None,
    }
}

fn schedule_mouse_scroll_draw(
    needs_draw: &mut bool,
    scroll_draw_pending: &mut bool,
    last_scroll_draw: Instant,
) {
    *scroll_draw_pending = true;
    *needs_draw = last_scroll_draw.elapsed() >= MOUSE_SCROLL_FRAME;
}

fn is_mouse_scroll_event(event: &Event) -> bool {
    matches!(event, Event::Mouse(mouse) if mouse_scroll_delta(mouse.kind).is_some()
        || mouse_horizontal_scroll_delta(mouse.kind, mouse.modifiers).is_some())
}

fn mouse_horizontal_scroll_delta(kind: MouseEventKind, modifiers: KeyModifiers) -> Option<isize> {
    match kind {
        MouseEventKind::ScrollLeft => Some(-1),
        MouseEventKind::ScrollRight => Some(1),
        MouseEventKind::ScrollUp if modifiers.contains(KeyModifiers::SHIFT) => Some(-1),
        MouseEventKind::ScrollDown if modifiers.contains(KeyModifiers::SHIFT) => Some(1),
        _ => None,
    }
}

fn mouse_scroll_delta(kind: MouseEventKind) -> Option<isize> {
    match kind {
        MouseEventKind::ScrollUp => Some(-1),
        MouseEventKind::ScrollDown => Some(1),
        _ => None,
    }
}

fn collect_mouse_scroll_delta(
    kind: MouseEventKind,
    pending_event: &mut Option<Event>,
    read_until: Instant,
) -> std::io::Result<isize> {
    let mut reads = 1usize;
    let mut delta = mouse_scroll_delta(kind).unwrap_or(0);
    let direction = delta.signum();
    while reads < MAX_COALESCED_MOUSE_SCROLL_READS {
        let timeout = read_until
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        if !event::poll(timeout)? {
            break;
        }
        let next = event::read()?;
        match next {
            Event::Mouse(mouse) => {
                if let Some(next_delta) = mouse_scroll_delta(mouse.kind) {
                    if next_delta.signum() != direction {
                        *pending_event = Some(Event::Mouse(mouse));
                        break;
                    }
                    reads = reads.saturating_add(1);
                    delta = delta.saturating_add(next_delta);
                } else {
                    *pending_event = Some(Event::Mouse(mouse));
                    break;
                }
            }
            _ => {
                *pending_event = Some(next);
                break;
            }
        }
    }
    Ok(delta)
}

fn mouse_scroll_action_cap(target: MouseScrollTarget) -> Option<isize> {
    match target {
        MouseScrollTarget::Diff => None,
        MouseScrollTarget::CommandPalette
        | MouseScrollTarget::FileSearch
        | MouseScrollTarget::FilePanel
        | MouseScrollTarget::Step => Some(MAX_DISCRETE_MOUSE_SCROLL_ACTIONS_PER_FRAME),
    }
}

fn clamp_mouse_scroll_delta(target: MouseScrollTarget, delta: isize) -> isize {
    if let Some(cap) = mouse_scroll_action_cap(target) {
        delta.clamp(-cap, cap)
    } else {
        delta
    }
}

fn blocks_mouse_scroll(
    blocked: Option<BlockedMouseScroll>,
    target: MouseScrollTarget,
    delta: isize,
) -> bool {
    delta != 0
        && blocked.is_some_and(|block| block.target == target && block.direction == delta.signum())
}

fn update_mouse_scroll_block(
    blocked: &mut Option<BlockedMouseScroll>,
    applied: Option<PendingMouseScroll>,
    scroll_before: usize,
    scroll_after: usize,
) {
    let Some(scroll) = applied else {
        return;
    };
    if scroll.target != MouseScrollTarget::Diff || scroll.delta == 0 {
        return;
    }
    if scroll_before == scroll_after {
        *blocked = Some(BlockedMouseScroll {
            target: scroll.target,
            direction: scroll.delta.signum(),
        });
    } else {
        *blocked = None;
    }
}

fn push_pending_mouse_scroll(
    pending: &mut Option<PendingMouseScroll>,
    target: MouseScrollTarget,
    delta: isize,
) -> Option<PendingMouseScroll> {
    if delta == 0 {
        return None;
    }
    let Some(current) = pending.as_mut() else {
        *pending = Some(PendingMouseScroll {
            target,
            delta: clamp_mouse_scroll_delta(target, delta),
        });
        return None;
    };
    if current.target != target {
        let old = pending.take();
        *pending = Some(PendingMouseScroll {
            target,
            delta: clamp_mouse_scroll_delta(target, delta),
        });
        return old;
    }
    if current.delta.signum() != delta.signum() {
        current.delta = clamp_mouse_scroll_delta(target, delta);
    } else {
        current.delta = clamp_mouse_scroll_delta(target, current.delta.saturating_add(delta));
    }
    None
}

fn queue_mouse_scroll(
    app: &mut App,
    target: MouseScrollTarget,
    kind: MouseEventKind,
    pending_event: &mut Option<Event>,
    pending: &mut Option<PendingMouseScroll>,
    blocked: &mut Option<BlockedMouseScroll>,
    last_scroll_draw: Instant,
) -> std::io::Result<bool> {
    let delta =
        collect_mouse_scroll_delta(kind, pending_event, last_scroll_draw + MOUSE_SCROLL_FRAME)?;
    if blocks_mouse_scroll(*blocked, target, delta) {
        return Ok(false);
    }
    if delta != 0 {
        *blocked = None;
    }
    if let Some(scroll) = push_pending_mouse_scroll(pending, target, delta) {
        apply_mouse_scroll(app, scroll);
    }
    Ok(true)
}

fn apply_pending_mouse_scroll(app: &mut App, pending: &mut Option<PendingMouseScroll>) -> bool {
    let Some(scroll) = pending.take() else {
        return false;
    };
    apply_mouse_scroll(app, scroll);
    true
}

fn apply_mouse_scroll(app: &mut App, scroll: PendingMouseScroll) {
    let delta = scroll.delta;
    if delta == 0 {
        return;
    }
    match scroll.target {
        MouseScrollTarget::CommandPalette => {
            if app.command_palette_active() {
                app.move_command_palette_selection(delta);
            }
        }
        MouseScrollTarget::FileSearch => {
            if app.file_search_active() {
                app.move_file_search_selection(delta);
            }
        }
        MouseScrollTarget::FilePanel | MouseScrollTarget::Step | MouseScrollTarget::Diff => {
            app.clear_diff_selection();
            for _ in 0..delta.unsigned_abs() {
                match scroll.target {
                    MouseScrollTarget::FilePanel if delta < 0 => app.scroll_file_panel_up(),
                    MouseScrollTarget::FilePanel => app.scroll_file_panel_down(),
                    MouseScrollTarget::Step
                        if delta < 0 && app.stepping && app.current_file_diff_ready() =>
                    {
                        app.prev_step();
                    }
                    MouseScrollTarget::Step if app.stepping && app.current_file_diff_ready() => {
                        app.next_step();
                    }
                    MouseScrollTarget::Diff if delta < 0 => app.scroll_up(),
                    MouseScrollTarget::Diff => app.scroll_down(),
                    _ => {}
                }
            }
        }
    }
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

        if !event::poll(tick_rate)? {
            needs_draw |= dashboard.tick();
            continue;
        }

        needs_draw = true;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let list_height =
                    dashboard.list_height(terminal.size().map_err(|e| anyhow!("{e}"))?.height);
                if dashboard.context_menu_open() {
                    dashboard.close_context_menu();
                    if key.code == KeyCode::Esc {
                        continue;
                    }
                }
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
                        dashboard.toggle_hovered_pin();
                    }
                    Dispatch::Matched(DashboardAction::SelectHovered) => {
                        dashboard.select_hovered();
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
                if dashboard.context_menu_open() {
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            match dashboard.handle_context_menu_click(mouse.column, mouse.row) {
                                Some(DashboardContextMenuResult::Open(selection)) => {
                                    return Ok(Some(selection));
                                }
                                Some(DashboardContextMenuResult::Handled) => {}
                                None => {}
                            }
                            continue;
                        }
                        MouseEventKind::Down(MouseButton::Right) => {
                            if !dashboard.open_context_menu(mouse.column, mouse.row) {
                                dashboard.close_context_menu();
                            }
                            continue;
                        }
                        MouseEventKind::Moved => {
                            if !dashboard.update_context_menu_hover(mouse.column, mouse.row) {
                                needs_draw = false;
                            }
                            continue;
                        }
                        MouseEventKind::ScrollUp
                        | MouseEventKind::ScrollDown
                        | MouseEventKind::ScrollLeft
                        | MouseEventKind::ScrollRight => {
                            dashboard.close_context_menu();
                            continue;
                        }
                        _ => {}
                    }
                }
                dashboard.update_hover(mouse.column, mouse.row);
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        dashboard.move_selection(-3, list_height);
                        dashboard.update_hover(mouse.column, mouse.row);
                    }
                    MouseEventKind::ScrollDown => {
                        dashboard.move_selection(3, list_height);
                        dashboard.update_hover(mouse.column, mouse.row);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        if dashboard.handle_filter_mouse_down(mouse.column, mouse.row) {
                            continue;
                        }
                        if let Some(action) = dashboard.footer_action_at(mouse.column, mouse.row) {
                            match action {
                                DashboardAction::Quit => return Ok(None),
                                DashboardAction::Accept => {
                                    if let Some(selection) = dashboard.selection() {
                                        return Ok(Some(selection));
                                    }
                                }
                                DashboardAction::TogglePin => dashboard.toggle_hovered_pin(),
                                DashboardAction::SelectHovered => {
                                    dashboard.select_hovered();
                                }
                                DashboardAction::ClearPin => dashboard.clear_pin(),
                                _ => {}
                            }
                            continue;
                        }
                        if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                            dashboard.toggle_pin_at_mouse(mouse.column, mouse.row);
                            continue;
                        }
                        if dashboard.select_at_mouse(mouse.column, mouse.row).is_some() {
                            if let Some(selection) = dashboard.selection() {
                                return Ok(Some(selection));
                            }
                        }
                    }
                    MouseEventKind::Down(MouseButton::Right) => {
                        dashboard.open_context_menu(mouse.column, mouse.row);
                    }
                    MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {}
                    _ => {}
                }
            }
            _ => {}
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

fn dashboard_selection_for_input(input_mode: &InputMode) -> Option<DashboardSelection> {
    match input_mode {
        InputMode::GitUncommitted => Some(DashboardSelection::Uncommitted),
        InputMode::GitStaged => Some(DashboardSelection::Staged),
        InputMode::GitRange { from, to } => Some(DashboardSelection::Range {
            from: from.clone(),
            to: to.clone(),
        }),
        _ => None,
    }
}

fn run_commit_picker<B: Backend>(
    terminal: &mut Terminal<B>,
    config: &config::Config,
    light_mode: bool,
    limit: usize,
    initial: Option<&InputMode>,
    theme_override: Option<&config::ResolvedTheme>,
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

    let theme = theme_override
        .cloned()
        .unwrap_or_else(|| config.ui.theme.resolve(light_mode));
    let time_format = TimeFormatter::new(&config.ui.time);
    let mut dashboard = Dashboard::new(DashboardConfig {
        repo_root,
        branch,
        commits,
        working_files: working_changes.len(),
        staged_files: staged_changes.len(),
        theme,
        primary_marker: config.ui.primary_marker.clone(),
        extent_marker: config.ui.extent_marker_left().to_string(),
        time_format,
        keybindings: Keybindings::from_config(&config.keybindings),
    });
    if let Some(selection) = initial.and_then(dashboard_selection_for_input) {
        dashboard.select_selection(&selection);
    }

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
    use super::{
        blocks_mouse_scroll, build_jj_diff, config, dedupe_review_log_entries, detect_input_mode,
        detect_input_mode_in, git_ref_input_mode, github_comment_to_review_comment, gitlab,
        ignore_github_delete_not_found, insert_review_thread_states, jj_bookmark_revset,
        jj_summary_paths, local_pr_review_revision, mouse_horizontal_scroll_delta, parse_range,
        parse_remote_url, push_pending_mouse_scroll, push_review_comments_to_provider_with,
        push_review_comments_to_provider_with_ops, render_editor_args, review_author_from_cli,
        review_comments_json_value, review_pr_target_metadata, review_provider_for_configured_host,
        review_status_json_value, review_target_summary, should_load_saved_review_fallback,
        sync_lookup_target, update_mouse_scroll_block, Args, BlockedMouseScroll, Command,
        GhComment, GhCommentUser, GhProviderComment, GhReviewThreadsResponse, InputMode,
        MouseScrollTarget, PendingMouseScroll, ProviderPr, ProviderUser, ReviewCommand,
        ReviewCommentCommand, ReviewProviderKind, ReviewProviderPushOps, ReviewTargetMetadata,
        MAX_DISCRETE_MOUSE_SCROLL_ACTIONS_PER_FRAME,
    };
    use crate::app::{
        review::{
            ReviewAnchor, ReviewComment, ReviewCommentFilter, ReviewProviderComment, ReviewRange,
            ReviewSide, ReviewTargetKind,
        },
        App, ViewMode,
    };
    use anyhow::{anyhow, Result};
    use clap::Parser;
    use crossterm::event::{KeyModifiers, MouseEventKind};
    use oyo_core::MultiFileDiff;
    use std::path::{Path, PathBuf};
    use std::process::Command as ProcessCommand;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "oyo-main-test-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn test_jj(repo: &Path, args: &[&str]) {
        let output = ProcessCommand::new("jj")
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
    }

    fn test_provider_pr() -> ProviderPr {
        ProviderPr {
            provider: ReviewProviderKind::GitHub,
            remote: "origin".to_string(),
            host: "github.com".to_string(),
            repo: "owner/repo".to_string(),
            number: 7,
            title: "PR".to_string(),
            url: "https://github.com/owner/repo/pull/7".to_string(),
            base_branch: "main".to_string(),
            head_branch: "feature".to_string(),
            base_commit: "base".to_string(),
            start_commit: None,
            head_commit: "head".to_string(),
        }
    }

    fn test_gitlab_pr() -> ProviderPr {
        ProviderPr {
            provider: ReviewProviderKind::GitLab,
            remote: "origin".to_string(),
            host: "gitlab.com".to_string(),
            repo: "group/sub/repo".to_string(),
            number: 1,
            title: "MR".to_string(),
            url: "https://gitlab.com/group/sub/repo/-/merge_requests/1".to_string(),
            base_branch: "main".to_string(),
            head_branch: "feature".to_string(),
            base_commit: "base".to_string(),
            start_commit: Some("start".to_string()),
            head_commit: "head".to_string(),
        }
    }

    fn sync_comment(
        id: u64,
        api_kind: &str,
        thread_id: Option<&str>,
        remote_resolved: Option<bool>,
        resolved: bool,
    ) -> ReviewComment {
        ReviewComment {
            id,
            anchor: ReviewAnchor {
                file_index: 0,
                file_path: "file.rs".to_string(),
                kind: ReviewTargetKind::Line,
                side: Some(ReviewSide::New),
                old_range: None,
                new_range: Some(ReviewRange { start: 1, end: 1 }),
                hunk_id: None,
                display_idx_hint: None,
                anchor_key: format!("line-{id}"),
                snapshot: None,
            },
            body: format!("comment {id}"),
            author: None,
            can_edit: false,
            resolved,
            outdated: false,
            reanchored: false,
            deleted: false,
            provider: Some(ReviewProviderComment {
                provider: "github".to_string(),
                remote: "origin".to_string(),
                repo: "owner/repo".to_string(),
                pr_number: 7,
                comment_id: id.to_string(),
                in_reply_to_id: None,
                thread_id: thread_id.map(str::to_string),
                thread_resolved: remote_resolved,
                resolved_dirty: false,
                author_username: Some("reviewer".to_string()),
                pr_title: Some("PR".to_string()),
                pr_url: Some("https://github.com/owner/repo/pull/7".to_string()),
                api_kind: api_kind.to_string(),
                sync_state: "clean".to_string(),
            }),
            in_reply_to: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn configured_self_hosted_review_providers_are_detected() {
        assert_eq!(
            review_provider_for_configured_host("source.example.org", true, false),
            Some(ReviewProviderKind::Forgejo)
        );
        assert_eq!(
            review_provider_for_configured_host("git.example.org", false, true),
            Some(ReviewProviderKind::GitLab)
        );
        assert_eq!(
            review_provider_for_configured_host("git.example.org", false, false),
            None
        );
    }

    #[test]
    fn review_provider_uses_pr_or_mr_nouns() {
        assert_eq!(ReviewProviderKind::GitHub.short_review_noun(), "PR");
        assert_eq!(
            ReviewProviderKind::GitHub.long_review_noun(),
            "pull request"
        );
        assert_eq!(ReviewProviderKind::Forgejo.short_review_noun(), "PR");
        assert_eq!(
            ReviewProviderKind::Forgejo.long_review_noun(),
            "pull request"
        );
        assert_eq!(ReviewProviderKind::GitLab.short_review_noun(), "MR");
        assert_eq!(
            ReviewProviderKind::GitLab.long_review_noun(),
            "merge request"
        );
        assert_eq!(
            ReviewProviderKind::GitHub.long_review_noun_title(),
            "Pull request"
        );
        assert_eq!(
            ReviewProviderKind::GitLab.long_review_noun_title(),
            "Merge request"
        );
    }

    #[test]
    fn gitlab_remote_url_preserves_nested_group_path() {
        assert_eq!(
            parse_remote_url("git@gitlab.com:group/subgroup/repo.git"),
            Some(("gitlab.com".to_string(), "group/subgroup/repo".to_string()))
        );
    }

    #[test]
    fn github_delete_treats_not_found_as_already_deleted() {
        assert!(ignore_github_delete_not_found(Err(anyhow!("gh: Not Found (HTTP 404)"))).is_ok());
        assert!(ignore_github_delete_not_found(Err(anyhow!(
            "gh: Internal Server Error (HTTP 500)"
        )))
        .is_err());
    }

    #[test]
    fn github_review_thread_graphql_maps_comment_ids() {
        let response: GhReviewThreadsResponse = serde_json::from_value(serde_json::json!({
            "data": {
                "repository": {
                    "pullRequest": {
                        "reviewThreads": {
                            "nodes": [{
                                "id": "PRRT_thread",
                                "isResolved": true,
                                "comments": { "nodes": [{ "fullDatabaseId": "42" }] }
                            }],
                            "pageInfo": { "hasNextPage": false, "endCursor": null }
                        }
                    }
                }
            }
        }))
        .unwrap();
        let mut states = std::collections::BTreeMap::new();
        insert_review_thread_states(
            &mut states,
            response.data.repository.pull_request.review_threads.nodes,
        );

        assert_eq!(states[&42].thread_id, "PRRT_thread");
        assert!(states[&42].resolved);
    }

    #[test]
    fn github_review_comment_imports_thread_and_resolved_state() {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("file.rs"),
            PathBuf::from("file.rs"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        let comment = github_comment_to_review_comment(
            &app,
            &test_provider_pr(),
            "me",
            GhProviderComment {
                comment: GhComment {
                    id: 42,
                    body: "review".to_string(),
                    path: "file.rs".to_string(),
                    line: Some(1),
                    original_line: Some(1),
                    side: Some("RIGHT".to_string()),
                    in_reply_to_id: Some(41),
                    user: GhCommentUser {
                        login: "reviewer".to_string(),
                        avatar_url: None,
                    },
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    updated_at: "2026-01-01T00:00:00Z".to_string(),
                },
                thread_id: Some("PRRT_thread".to_string()),
                thread_resolved: Some(true),
            },
        )
        .unwrap();

        assert!(comment.resolved);
        let provider = comment.provider.unwrap();
        assert_eq!(provider.thread_id.as_deref(), Some("PRRT_thread"));
        assert_eq!(provider.thread_resolved, Some(true));
        assert_eq!(provider.in_reply_to_id.as_deref(), Some("41"));
        assert_eq!(provider.api_kind, "review");
    }

    #[test]
    fn gitlab_discussion_mapping_inherits_root_position_for_replies() {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("app.py"),
            PathBuf::from("app.py"),
            "one\ntwo\nthree\n".to_string(),
            "one\ntwo changed\nthree\n".to_string(),
        );
        let app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        let discussion: gitlab::GlDiscussion = serde_json::from_value(serde_json::json!({
            "id": "disc-1",
            "resolved": true,
            "notes": [
                {
                    "id": 101,
                    "body": "root",
                    "system": false,
                    "author": { "username": "reviewer", "name": "Reviewer" },
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "position": { "new_path": "app.py", "old_path": "app.py", "old_line": 2, "new_line": 2 },
                    "resolved": true
                },
                {
                    "id": 102,
                    "body": "reply",
                    "system": false,
                    "author": { "username": "me", "name": "Me" },
                    "created_at": "2026-01-01T00:01:00Z",
                    "updated_at": "2026-01-01T00:01:00Z",
                    "position": null,
                    "resolved": true
                }
            ]
        }))
        .unwrap();

        let comments =
            gitlab::discussion_to_review_comments(&app, &test_gitlab_pr(), "me", discussion)
                .into_iter()
                .collect::<Result<Vec<_>>>()
                .unwrap();

        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].anchor.file_path, "app.py");
        assert_eq!(
            comments[0].anchor.old_range,
            Some(ReviewRange { start: 2, end: 2 })
        );
        assert_eq!(
            comments[1].anchor.new_range,
            Some(ReviewRange { start: 2, end: 2 })
        );
        assert!(comments.iter().all(|comment| comment.resolved));
        let root = comments[0].provider.as_ref().unwrap();
        let reply = comments[1].provider.as_ref().unwrap();
        assert_eq!(root.provider, "gitlab");
        assert_eq!(root.repo, "group/sub/repo");
        assert_eq!(root.thread_id.as_deref(), Some("disc-1"));
        assert_eq!(root.thread_resolved, Some(true));
        assert_eq!(root.in_reply_to_id, None);
        assert_eq!(reply.comment_id, "102");
        assert_eq!(reply.in_reply_to_id.as_deref(), Some("101"));
        assert_eq!(reply.thread_id.as_deref(), Some("disc-1"));
        assert_eq!(reply.api_kind, "review");
        assert_eq!(
            comments[0]
                .author
                .as_ref()
                .and_then(|author| author.usernames.get("gitlab"))
                .map(String::as_str),
            Some("reviewer")
        );
    }

    #[test]
    fn gitlab_positionless_discussion_maps_to_merge_request_comment() {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("app.py"),
            PathBuf::from("app.py"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        let discussion: gitlab::GlDiscussion = serde_json::from_value(serde_json::json!({
            "id": "disc-general",
            "notes": [{
                "id": 201,
                "body": "General feedback",
                "system": false,
                "author": { "username": "reviewer", "name": "Reviewer" },
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "position": null
            }]
        }))
        .unwrap();

        let comments =
            gitlab::discussion_to_review_comments(&app, &test_gitlab_pr(), "me", discussion)
                .into_iter()
                .collect::<Result<Vec<_>>>()
                .unwrap();

        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].anchor.kind, ReviewTargetKind::PullRequest);
        let provider = comments[0].provider.as_ref().unwrap();
        assert_eq!(provider.api_kind, "issue");
        assert_eq!(provider.thread_id.as_deref(), Some("disc-general"));
        assert_eq!(provider.comment_id, "201");
    }

    #[test]
    fn sync_enriches_context_line_with_old_and_new_ranges() {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("app.py"),
            PathBuf::from("app.py"),
            "same\n".to_string(),
            "same\nadded\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.add_review_comment_from_cli(
            "app.py",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 1, end: 1 }),
            "context".to_string(),
        )
        .unwrap();

        let comments = app.review_comments_for_sync();
        assert_eq!(
            comments[0].anchor.old_range,
            Some(ReviewRange { start: 1, end: 1 })
        );
        assert_eq!(
            comments[0].anchor.new_range,
            Some(ReviewRange { start: 1, end: 1 })
        );
    }

    #[test]
    fn gitlab_positioned_body_uses_json_position_and_diff_refs() {
        let comment = sync_comment(9, "review", None, None, false);
        let body = gitlab::positioned_discussion_body(&test_gitlab_pr(), &comment).unwrap();
        assert_eq!(body["body"], "comment 9");
        assert_eq!(body["position"]["position_type"], "text");
        assert_eq!(body["position"]["base_sha"], "base");
        assert_eq!(body["position"]["start_sha"], "start");
        assert_eq!(body["position"]["head_sha"], "head");
        assert_eq!(body["position"]["new_path"], "file.rs");
        assert_eq!(body["position"]["old_path"], "file.rs");
        assert_eq!(body["position"]["new_line"], 1);
        assert!(body["position"].get("old_line").is_none());

        let mut context = comment.clone();
        context.anchor.old_range = Some(ReviewRange { start: 1, end: 1 });
        let context_body = gitlab::positioned_discussion_body(&test_gitlab_pr(), &context).unwrap();
        assert_eq!(context_body["position"]["old_line"], 1);
        assert_eq!(context_body["position"]["new_line"], 1);

        let mut hunk = context.clone();
        hunk.anchor.kind = ReviewTargetKind::Hunk;
        hunk.anchor.side = None;
        let hunk_body = gitlab::positioned_discussion_body(&test_gitlab_pr(), &hunk).unwrap();
        assert!(hunk_body["position"].get("old_line").is_none());
        assert_eq!(hunk_body["position"]["new_line"], 1);

        let mut removed = comment.clone();
        removed.anchor.side = Some(ReviewSide::Old);
        removed.anchor.old_range = Some(ReviewRange { start: 1, end: 1 });
        removed.anchor.new_range = None;
        let removed_body = gitlab::positioned_discussion_body(&test_gitlab_pr(), &removed).unwrap();
        assert_eq!(removed_body["position"]["old_line"], 1);
        assert!(removed_body["position"].get("new_line").is_none());

        assert_eq!(
            gitlab::percent_encode("group/sub repo/repo"),
            "group%2Fsub%20repo%2Frepo"
        );

        let mut wrong_side = comment;
        wrong_side.anchor.side = Some(ReviewSide::Old);
        assert!(gitlab::positioned_discussion_body(&test_gitlab_pr(), &wrong_side).is_err());
    }

    #[test]
    fn gitlab_positioned_response_validation_rejects_general_note() {
        let discussion: gitlab::GlDiscussion = serde_json::from_value(serde_json::json!({
            "id": "disc-1",
            "notes": [{
                "id": 101,
                "body": "root",
                "system": false,
                "author": { "username": "me" },
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "position": null
            }]
        }))
        .unwrap();

        assert!(gitlab::ensure_positioned_discussion(&discussion)
            .unwrap_err()
            .to_string()
            .contains("unpositioned"));
    }

    #[test]
    fn provider_push_dispatch_covers_create_reply_resolve_edit_and_delete() {
        struct MockOps {
            calls: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        }

        impl ReviewProviderPushOps for MockOps {
            fn provider_label(&self) -> &'static str {
                "Mock"
            }

            fn create_root(&mut self, comment: &ReviewComment) -> Result<ReviewProviderComment> {
                self.calls
                    .borrow_mut()
                    .push(format!("create:{}", comment.id));
                let mut provider = comment.provider.clone().unwrap_or_else(|| {
                    let mut provider = sync_comment(
                        100 + comment.id,
                        "review",
                        Some("new-thread"),
                        Some(false),
                        false,
                    )
                    .provider
                    .unwrap();
                    provider.provider = "gitlab".to_string();
                    provider.repo = "group/sub/repo".to_string();
                    provider.pr_number = 1;
                    provider
                });
                provider.comment_id = format!("new-{}", comment.id);
                provider.sync_state = "clean".to_string();
                Ok(provider)
            }

            fn create_reply(
                &mut self,
                provider: &ReviewProviderComment,
                body: &str,
            ) -> Result<ReviewProviderComment> {
                self.calls.borrow_mut().push(format!(
                    "reply:{}:{body}",
                    provider.in_reply_to_id.as_deref().unwrap_or_default()
                ));
                let mut clean = provider.clone();
                clean.comment_id = "reply-remote".to_string();
                clean.sync_state = "clean".to_string();
                Ok(clean)
            }

            fn update(
                &mut self,
                provider: &ReviewProviderComment,
                body: &str,
            ) -> Result<ReviewProviderComment> {
                self.calls
                    .borrow_mut()
                    .push(format!("update:{}:{body}", provider.comment_id));
                let mut clean = provider.clone();
                clean.sync_state = "clean".to_string();
                Ok(clean)
            }

            fn delete(&mut self, provider: &ReviewProviderComment) -> Result<()> {
                self.calls
                    .borrow_mut()
                    .push(format!("delete:{}", provider.comment_id));
                Ok(())
            }

            fn set_thread_resolved(&mut self, thread_id: &str, resolved: bool) -> Result<()> {
                self.calls
                    .borrow_mut()
                    .push(format!("resolve:{thread_id}:{resolved}"));
                Ok(())
            }
        }

        let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut root = sync_comment(1, "review", None, None, false);
        root.provider = None;
        root.can_edit = true;

        let mut reply = sync_comment(2, "review", Some("thread-1"), Some(false), false);
        reply.can_edit = true;
        reply.body = "reply body".to_string();
        {
            let provider = reply.provider.as_mut().unwrap();
            provider.provider = "gitlab".to_string();
            provider.repo = "group/sub/repo".to_string();
            provider.pr_number = 1;
            provider.comment_id.clear();
            provider.in_reply_to_id = Some("10".to_string());
            provider.sync_state = "dirty".to_string();
        }

        let mut update = sync_comment(3, "review", Some("thread-2"), Some(false), false);
        update.can_edit = true;
        update.body = "updated body".to_string();
        {
            let provider = update.provider.as_mut().unwrap();
            provider.provider = "gitlab".to_string();
            provider.repo = "group/sub/repo".to_string();
            provider.pr_number = 1;
            provider.comment_id = "20".to_string();
            provider.sync_state = "dirty".to_string();
        }

        let mut delete = sync_comment(4, "review", Some("thread-3"), Some(false), false);
        delete.can_edit = true;
        delete.deleted = true;
        {
            let provider = delete.provider.as_mut().unwrap();
            provider.provider = "gitlab".to_string();
            provider.repo = "group/sub/repo".to_string();
            provider.pr_number = 1;
            provider.comment_id = "30".to_string();
        }

        let mut resolve = sync_comment(5, "review", Some("thread-4"), Some(false), true);
        resolve.can_edit = true;
        {
            let provider = resolve.provider.as_mut().unwrap();
            provider.provider = "gitlab".to_string();
            provider.repo = "group/sub/repo".to_string();
            provider.pr_number = 1;
        }

        let mut ops = MockOps {
            calls: calls.clone(),
        };
        let outcome = push_review_comments_to_provider_with_ops(
            vec![root, reply, update, delete, resolve],
            &test_gitlab_pr(),
            &mut ops,
        )
        .unwrap();
        let calls = calls.borrow().clone();

        assert!(calls.contains(&"create:1".to_string()));
        assert!(calls.contains(&"reply:10:reply body".to_string()));
        assert!(calls.contains(&"update:20:updated body".to_string()));
        assert!(calls.contains(&"delete:30".to_string()));
        assert!(calls.contains(&"resolve:thread-4:true".to_string()));
        assert_eq!(outcome.created, 2);
        assert_eq!(outcome.updated, 1);
        assert_eq!(outcome.deleted, 1);
        assert_eq!(outcome.thread_changes.len(), 1);
    }

    #[test]
    fn github_push_creates_review_thread_reply_and_marks_it_clean() {
        let mut reply = sync_comment(2, "review", Some("PRRT_thread"), Some(false), false);
        reply.can_edit = true;
        reply.body = "Thread reply".to_string();
        let provider = reply.provider.as_mut().unwrap();
        provider.comment_id.clear();
        provider.in_reply_to_id = Some("42".to_string());
        provider.sync_state = "dirty".to_string();
        let mut calls = Vec::new();

        let outcome = push_review_comments_to_provider_with(
            vec![reply],
            &test_provider_pr(),
            &ProviderUser {
                login: "me".to_string(),
                avatar_url: None,
            },
            |parent_id, body| {
                calls.push((parent_id.to_string(), body.to_string()));
                Ok(GhComment {
                    id: 99,
                    body: body.to_string(),
                    path: "file.rs".to_string(),
                    line: Some(1),
                    original_line: None,
                    side: Some("RIGHT".to_string()),
                    in_reply_to_id: Some(42),
                    user: GhCommentUser {
                        login: "me".to_string(),
                        avatar_url: None,
                    },
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    updated_at: "2026-01-01T00:00:00Z".to_string(),
                })
            },
            |_thread_id, _resolved| unreachable!(),
        )
        .unwrap();

        assert_eq!(calls, vec![("42".to_string(), "Thread reply".to_string())]);
        assert_eq!(outcome.created, 1);
        let provider = &outcome.changes[0].provider;
        assert_eq!(provider.comment_id, "99");
        assert_eq!(provider.in_reply_to_id.as_deref(), Some("42"));
        assert_eq!(provider.thread_id.as_deref(), Some("PRRT_thread"));
        assert_eq!(provider.sync_state, "clean");
    }

    #[test]
    fn github_reply_failures_keep_successful_reply_bookkeeping() {
        let make_reply = |id, parent_id: &str| {
            let mut reply = sync_comment(id, "review", Some("PRRT_thread"), Some(false), false);
            reply.can_edit = true;
            let provider = reply.provider.as_mut().unwrap();
            provider.comment_id.clear();
            provider.in_reply_to_id = Some(parent_id.to_string());
            provider.sync_state = "dirty".to_string();
            reply
        };
        let mut calls = Vec::new();

        let outcome = push_review_comments_to_provider_with(
            vec![make_reply(2, "42"), make_reply(3, "43")],
            &test_provider_pr(),
            &ProviderUser {
                login: "me".to_string(),
                avatar_url: None,
            },
            |parent_id, body| {
                calls.push(parent_id.to_string());
                if parent_id == "43" {
                    anyhow::bail!("parent deleted")
                }
                Ok(GhComment {
                    id: 99,
                    body: body.to_string(),
                    path: "file.rs".to_string(),
                    line: Some(1),
                    original_line: None,
                    side: Some("RIGHT".to_string()),
                    in_reply_to_id: Some(42),
                    user: GhCommentUser {
                        login: "me".to_string(),
                        avatar_url: None,
                    },
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    updated_at: "2026-01-01T00:00:00Z".to_string(),
                })
            },
            |_thread_id, _resolved| unreachable!(),
        )
        .unwrap();

        assert_eq!(calls, vec!["42", "43"]);
        assert_eq!(outcome.created, 1);
        assert_eq!(outcome.changes.len(), 1);
        assert_eq!(outcome.changes[0].id, 2);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("parent deleted"));
    }

    #[test]
    fn malformed_reply_is_rejected_before_remote_mutation() {
        let mut reply = sync_comment(2, "review", None, None, false);
        reply.can_edit = true;
        let provider = reply.provider.as_mut().unwrap();
        provider.comment_id.clear();
        provider.in_reply_to_id = Some("42".to_string());
        let mut calls = 0usize;

        let error = push_review_comments_to_provider_with(
            vec![reply],
            &test_provider_pr(),
            &ProviderUser {
                login: "me".to_string(),
                avatar_url: None,
            },
            |_parent_id, _body| {
                calls += 1;
                unreachable!()
            },
            |_thread_id, _resolved| unreachable!(),
        )
        .unwrap_err();

        assert_eq!(calls, 0);
        assert!(error
            .to_string()
            .contains("not linked to a remote review thread"));
    }

    #[test]
    fn local_reply_children_are_not_pushed_to_provider() {
        let mut reply = sync_comment(2, "review", None, None, false);
        reply.can_edit = true;
        reply.provider = None;
        reply.in_reply_to = Some(1);

        let outcome = push_review_comments_to_provider_with(
            vec![reply],
            &test_provider_pr(),
            &ProviderUser {
                login: "me".to_string(),
                avatar_url: None,
            },
            |_parent_id, _body| unreachable!(),
            |_thread_id, _resolved| unreachable!(),
        )
        .unwrap();

        assert_eq!(outcome.created, 0);
        assert_eq!(outcome.skipped, 1);
        assert!(outcome.changes.is_empty());
    }

    #[test]
    fn github_push_resolves_each_review_thread_once_and_skips_other_comments() {
        let mut local = sync_comment(4, "review", None, None, true);
        local.provider = None;
        let comments = vec![
            sync_comment(1, "review", Some("PRRT_thread"), Some(false), true),
            sync_comment(2, "review", Some("PRRT_thread"), Some(false), true),
            sync_comment(3, "issue", None, None, true),
            sync_comment(5, "review", Some("PRRT_clean"), Some(true), true),
            local,
        ];
        let mut calls = Vec::new();
        let outcome = push_review_comments_to_provider_with(
            comments,
            &test_provider_pr(),
            &ProviderUser {
                login: "me".to_string(),
                avatar_url: None,
            },
            |_parent_id, _body| unreachable!(),
            |thread_id, resolved| {
                calls.push((thread_id.to_string(), resolved));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(calls, vec![("PRRT_thread".to_string(), true)]);
        assert_eq!(outcome.updated, 0);
        assert_eq!(outcome.thread_changes.len(), 1);
    }

    #[test]
    fn github_thread_permission_error_is_a_push_warning() {
        let outcome = push_review_comments_to_provider_with(
            vec![sync_comment(
                1,
                "review",
                Some("PRRT_thread"),
                Some(false),
                true,
            )],
            &test_provider_pr(),
            &ProviderUser {
                login: "me".to_string(),
                avatar_url: None,
            },
            |_parent_id, _body| unreachable!(),
            |_thread_id, _resolved| anyhow::bail!("forbidden"),
        )
        .unwrap();

        assert_eq!(outcome.thread_changes.len(), 0);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("forbidden"));
    }

    #[test]
    fn github_push_unresolves_review_thread() {
        let mut calls = Vec::new();
        let outcome = push_review_comments_to_provider_with(
            vec![sync_comment(
                1,
                "review",
                Some("PRRT_thread"),
                Some(true),
                false,
            )],
            &test_provider_pr(),
            &ProviderUser {
                login: "me".to_string(),
                avatar_url: None,
            },
            |_parent_id, _body| unreachable!(),
            |thread_id, resolved| {
                calls.push((thread_id.to_string(), resolved));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(calls, vec![("PRRT_thread".to_string(), false)]);
        assert_eq!(outcome.thread_changes.len(), 1);
    }

    #[test]
    fn git_branch_arg_maps_to_branch_range() {
        let root = temp_path("branch-range");
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&str]| {
            let status = ProcessCommand::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        run(&["init", "-q"]);
        run(&["config", "user.name", "Reviewer"]);
        run(&["config", "user.email", "reviewer@example.com"]);
        std::fs::write(root.join("file.txt"), "old\n").unwrap();
        run(&["add", "file.txt"]);
        run(&["-c", "commit.gpgsign=false", "commit", "-qm", "initial"]);
        run(&["checkout", "-qb", "feature"]);
        let base = ProcessCommand::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD~0"])
            .output()
            .unwrap();
        let base = String::from_utf8_lossy(&base.stdout).trim().to_string();

        let mode = git_ref_input_mode(&root, "feature").unwrap();

        assert!(matches!(
            mode,
            InputMode::GitRange { ref from, ref to } if from == &base && to == "feature"
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_pr_review_target_uses_newest_matching_branch_review() {
        let entries = vec![
            serde_json::json!({
                "reviewKey": "git:branch:new",
                "target": {
                    "label": "github#7",
                    "vcs": "git",
                    "gitBaseRef": "main",
                    "gitHeadRef": "feature",
                    "gitBaseCommit": "base-oid",
                    "gitHeadCommit": "head-oid",
                    "branch": "feature",
                    "prProvider": "github",
                    "prRepo": "owner/repo",
                    "prNumber": 7
                }
            }),
            serde_json::json!({
                "reviewKey": "git:branch:old",
                "target": {
                    "label": "github#6",
                    "vcs": "git",
                    "gitBaseRef": "old-main",
                    "gitHeadRef": "feature",
                    "branch": "feature",
                    "prNumber": 6
                }
            }),
        ];

        assert_eq!(
            local_pr_review_revision(&entries, "feature").as_deref(),
            Some("base-oid...feature")
        );
        assert_eq!(local_pr_review_revision(&entries, "other"), None);
    }

    #[test]
    fn sync_default_uses_saved_head_but_explicit_range_stays_exact() {
        assert_eq!(
            sync_lookup_target(None, Some("base...feature".to_string())).as_deref(),
            Some("feature")
        );
        assert_eq!(
            sync_lookup_target(
                Some("other-base...feature".to_string()),
                Some("base...feature".to_string()),
            )
            .as_deref(),
            Some("other-base...feature")
        );
    }

    #[test]
    fn review_target_option_parses_before_or_after_comment_subcommand() {
        let before =
            Args::try_parse_from(["oy", "review", "comment", "-t", "feature", "resolve", "1"])
                .unwrap();
        let after =
            Args::try_parse_from(["oy", "review", "comment", "resolve", "-t", "feature", "1"])
                .unwrap();

        let target = |args: Args| match args.command.unwrap() {
            Command::Review {
                command:
                    Some(ReviewCommand::Comment {
                        target: outer,
                        command: Some(ReviewCommentCommand::Resolve { target: inner, .. }),
                        ..
                    }),
                ..
            } => outer.or(inner),
            _ => panic!("unexpected review command"),
        };
        assert_eq!(target(before).as_deref(), Some("feature"));
        assert_eq!(target(after).as_deref(), Some("feature"));
    }

    #[test]
    fn review_reply_command_parses_parent_body_target_and_json() {
        let args = Args::try_parse_from([
            "oy", "review", "comment", "reply", "42", "--body", "Fixed", "--target", "feature",
            "--json",
        ])
        .unwrap();

        match args.command.unwrap() {
            Command::Review {
                command:
                    Some(ReviewCommand::Comment {
                        command:
                            Some(ReviewCommentCommand::Reply {
                                args,
                                target,
                                body,
                                json,
                            }),
                        ..
                    }),
                ..
            } => {
                assert_eq!(args, vec!["42"]);
                assert_eq!(target.as_deref(), Some("feature"));
                assert_eq!(body, "Fixed");
                assert!(json);
            }
            _ => panic!("unexpected review command"),
        }
    }

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
    fn jj_rename_summary_paths_expand_braces() {
        let (old, new) =
            jj_summary_paths(oyo_core::git::FileStatus::Renamed, "src/{old.rs => new.rs}");
        assert_eq!(old.as_deref(), Some("src/old.rs"));
        assert_eq!(new, "src/new.rs");
    }

    #[test]
    fn jj_merge_ranges_use_endpoint_tree_diff() {
        if ProcessCommand::new("jj").arg("--version").output().is_err() {
            return;
        }
        let repo = temp_path("jj-merge-range");
        std::fs::create_dir_all(&repo).unwrap();
        let init = ProcessCommand::new("jj")
            .arg("--config")
            .arg("signing.behavior=\"drop\"")
            .args(["git", "init"])
            .arg(&repo)
            .output()
            .unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        test_jj(&repo, &["config", "set", "--repo", "user.name", "Test"]);
        test_jj(
            &repo,
            &["config", "set", "--repo", "user.email", "test@example.com"],
        );
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        test_jj(&repo, &["commit", "-m", "base"]);
        test_jj(&repo, &["bookmark", "create", "main", "-r", "@-"]);
        test_jj(
            &repo,
            &[
                "config",
                "set",
                "--repo",
                "revset-aliases.'trunk()'",
                "main",
            ],
        );
        test_jj(&repo, &["new", "main"]);
        std::fs::write(repo.join("feature.txt"), "feature\n").unwrap();
        test_jj(&repo, &["commit", "-m", "feature"]);
        test_jj(&repo, &["bookmark", "create", "feature", "-r", "@-"]);
        test_jj(&repo, &["new", "main"]);
        std::fs::write(repo.join("main.txt"), "main\n").unwrap();
        test_jj(&repo, &["commit", "-m", "main"]);
        test_jj(&repo, &["bookmark", "set", "main", "-r", "@-"]);

        let diverged = build_jj_diff(&repo, "trunk()..feature", None).unwrap();
        assert!(diverged
            .files
            .iter()
            .any(|file| file.path == Path::new("feature.txt")));
        assert!(!diverged
            .files
            .iter()
            .any(|file| file.path == Path::new("main.txt")));

        test_jj(&repo, &["new", "feature", "main"]);
        std::fs::write(repo.join("merge.txt"), "merge\n").unwrap();
        test_jj(&repo, &["commit", "-m", "merge"]);
        test_jj(&repo, &["bookmark", "set", "feature", "-r", "@-"]);
        test_jj(&repo, &["edit", "feature"]);

        let old_range = ProcessCommand::new("jj")
            .current_dir(&repo)
            .arg("-R")
            .arg(&repo)
            .args(["diff", "-r", "trunk()..feature", "--summary"])
            .output()
            .unwrap();
        assert!(!old_range.status.success());

        let range = build_jj_diff(&repo, "trunk()..feature", None).unwrap();
        let explicit_range = build_jj_diff(&repo, "main..feature", None).unwrap();
        assert_eq!(jj_bookmark_revset("feature"), "trunk()..feature");
        assert_eq!(
            range
                .files
                .iter()
                .map(|file| file.path.as_path())
                .collect::<Vec<_>>(),
            explicit_range
                .files
                .iter()
                .map(|file| file.path.as_path())
                .collect::<Vec<_>>()
        );
        assert!(range
            .files
            .iter()
            .any(|file| file.path == Path::new("feature.txt")));
        assert!(range
            .files
            .iter()
            .any(|file| file.path == Path::new("merge.txt")));
        assert!(!range
            .files
            .iter()
            .any(|file| file.path == Path::new("main.txt")));
        assert!(matches!(
            range.blame_sources(),
            Some((
                oyo_core::multi::BlameSource::Commit(_),
                oyo_core::multi::BlameSource::Commit(_)
            ))
        ));
        let working_copy = build_jj_diff(&repo, "@", None).unwrap();
        assert!(matches!(
            working_copy.blame_sources(),
            Some((
                oyo_core::multi::BlameSource::Commit(_),
                oyo_core::multi::BlameSource::Worktree
            ))
        ));

        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn non_colocated_jj_diff_uses_jj_blame_sources() {
        if ProcessCommand::new("jj").arg("--version").output().is_err() {
            return;
        }
        let repo = temp_path("jj-native-blame");
        std::fs::create_dir_all(&repo).unwrap();
        let init = ProcessCommand::new("jj")
            .arg("--config")
            .arg("signing.behavior=\"drop\"")
            .args(["git", "init", "--no-colocate"])
            .arg(&repo)
            .output()
            .unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        test_jj(&repo, &["config", "set", "--repo", "user.name", "Test"]);
        test_jj(
            &repo,
            &["config", "set", "--repo", "user.email", "test@example.com"],
        );
        std::fs::write(repo.join("file.txt"), "committed\n").unwrap();
        std::fs::write(repo.join("clean.txt"), "clean\n").unwrap();
        test_jj(&repo, &["commit", "-m", "base"]);
        std::fs::write(repo.join("file.txt"), "committed\nchanged\n").unwrap();

        let mode = detect_input_mode_in(&[PathBuf::from("file.txt")], &repo);
        assert!(matches!(
            mode,
            InputMode::JjFile { path } if path == Path::new("file.txt")
        ));
        let diff = build_jj_diff(&repo, "@", Some(Path::new("file.txt"))).unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path, Path::new("file.txt"));
        let clean = build_jj_diff(&repo, "@", Some(Path::new("clean.txt"))).unwrap();
        assert_eq!(clean.files.len(), 1);
        assert_eq!(clean.files[0].path, Path::new("clean.txt"));
        assert_eq!(
            (clean.files[0].insertions, clean.files[0].deletions),
            (0, 0)
        );

        assert_eq!(
            diff.blame_sources(),
            Some((
                oyo_core::multi::BlameSource::JjRev("(@)-".to_string()),
                oyo_core::multi::BlameSource::JjRev("@".to_string()),
            ))
        );
        let entries = crate::blame::blame_range(
            &repo,
            Path::new("file.txt"),
            1,
            2,
            &oyo_core::multi::BlameSource::JjRev("@".to_string()),
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert!(!entries[0].1.author.is_empty());
        assert_eq!(entries[0].1.summary, "base");
        assert!(!entries[0].1.uncommitted);
        assert!(entries[1].1.uncommitted);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn jj_at_does_not_load_saved_fingerprint_fallback() {
        assert!(!should_load_saved_review_fallback(&InputMode::JjRevision {
            rev: "@".to_string(),
        }));
        assert!(should_load_saved_review_fallback(&InputMode::JjRevision {
            rev: "trunk()..feature".to_string(),
        }));
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
    fn mouse_scroll_preserves_diff_delta_and_reverses_pending_delta() {
        let mut pending = None;

        assert_eq!(
            push_pending_mouse_scroll(&mut pending, MouseScrollTarget::Diff, 100),
            None
        );
        assert_eq!(
            push_pending_mouse_scroll(&mut pending, MouseScrollTarget::Diff, 50),
            None
        );
        assert_eq!(
            pending,
            Some(PendingMouseScroll {
                target: MouseScrollTarget::Diff,
                delta: 150,
            })
        );

        assert_eq!(
            push_pending_mouse_scroll(&mut pending, MouseScrollTarget::Diff, -3),
            None
        );
        assert_eq!(
            pending,
            Some(PendingMouseScroll {
                target: MouseScrollTarget::Diff,
                delta: -3,
            })
        );
    }

    #[test]
    fn mouse_scroll_flushes_when_target_changes() {
        let mut pending = Some(PendingMouseScroll {
            target: MouseScrollTarget::Diff,
            delta: 5,
        });

        assert_eq!(
            push_pending_mouse_scroll(&mut pending, MouseScrollTarget::Step, -100),
            Some(PendingMouseScroll {
                target: MouseScrollTarget::Diff,
                delta: 5,
            })
        );
        assert_eq!(
            pending,
            Some(PendingMouseScroll {
                target: MouseScrollTarget::Step,
                delta: -MAX_DISCRETE_MOUSE_SCROLL_ACTIONS_PER_FRAME,
            })
        );
    }

    #[test]
    fn mouse_scroll_blocks_same_direction_at_edge() {
        let mut blocked = None;

        update_mouse_scroll_block(
            &mut blocked,
            Some(PendingMouseScroll {
                target: MouseScrollTarget::Diff,
                delta: 42,
            }),
            10,
            10,
        );

        assert_eq!(
            blocked,
            Some(BlockedMouseScroll {
                target: MouseScrollTarget::Diff,
                direction: 1,
            })
        );
        assert!(blocks_mouse_scroll(blocked, MouseScrollTarget::Diff, 1));
        assert!(!blocks_mouse_scroll(blocked, MouseScrollTarget::Diff, -1));
    }

    #[test]
    fn horizontal_mouse_scroll_supports_native_and_shift_wheel() {
        assert_eq!(
            mouse_horizontal_scroll_delta(MouseEventKind::ScrollLeft, KeyModifiers::empty()),
            Some(-1)
        );
        assert_eq!(
            mouse_horizontal_scroll_delta(MouseEventKind::ScrollRight, KeyModifiers::empty()),
            Some(1)
        );
        assert_eq!(
            mouse_horizontal_scroll_delta(MouseEventKind::ScrollUp, KeyModifiers::SHIFT),
            Some(-1)
        );
        assert_eq!(
            mouse_horizontal_scroll_delta(MouseEventKind::ScrollDown, KeyModifiers::SHIFT),
            Some(1)
        );
        assert_eq!(
            mouse_horizontal_scroll_delta(MouseEventKind::ScrollDown, KeyModifiers::empty()),
            None
        );
    }

    #[test]
    fn mouse_scroll_does_not_block_after_movement() {
        let mut blocked = Some(BlockedMouseScroll {
            target: MouseScrollTarget::Diff,
            direction: 1,
        });

        update_mouse_scroll_block(
            &mut blocked,
            Some(PendingMouseScroll {
                target: MouseScrollTarget::Diff,
                delta: 42,
            }),
            10,
            20,
        );

        assert_eq!(blocked, None);
    }

    #[test]
    fn cli_author_accepts_usernames() {
        let usernames = vec!["@helper".to_string(), "forge=oyo-agent".to_string()];
        let author = review_author_from_cli(Some("Oyo agent"), None, Some("agent"), &usernames)
            .unwrap()
            .unwrap();

        assert_eq!(author.name, "Oyo agent");
        assert_eq!(author.author_type.as_deref(), Some("agent"));
        assert_eq!(author.usernames["local"], "helper");
        assert_eq!(author.usernames["forge"], "oyo-agent");
    }

    #[test]
    fn review_status_json_comment_count_uses_filtered_count() {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("old.txt"),
            PathBuf::from("new.txt"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_persist_enabled(false);
        app.enable_review_mode();
        app.set_review_target_metadata(Some(review_pr_target_metadata(&test_provider_pr())));
        app.add_review_comment_from_cli(
            "new.txt",
            ReviewTargetKind::Line,
            Some(ReviewSide::New),
            None,
            Some(ReviewRange { start: 1, end: 1 }),
            "open".to_string(),
        )
        .unwrap();
        let resolved = app
            .add_review_comment_from_cli(
                "new.txt",
                ReviewTargetKind::Line,
                Some(ReviewSide::New),
                None,
                Some(ReviewRange { start: 1, end: 1 }),
                "done".to_string(),
            )
            .unwrap();
        assert!(app.set_review_comment_resolved_from_cli(resolved, true));

        let value = review_status_json_value(
            &app,
            &ReviewCommentFilter {
                unresolved: true,
                ..ReviewCommentFilter::default()
            },
        );

        assert_eq!(value["commentCount"], 1);
        assert_eq!(value["comments"].as_array().unwrap().len(), 1);
        assert_eq!(value["pr"], 7);
        assert_eq!(value["label"], "PR #7 (feature)");
        assert_eq!(value["target"], value["label"]);
        assert!(value["reviewKey"]
            .as_str()
            .is_some_and(|key| !key.is_empty()));
        let comments = review_comments_json_value(&app, &ReviewCommentFilter::default()).unwrap();
        assert_eq!(comments["reviewKey"], value["reviewKey"]);
        assert_eq!(comments["label"], value["label"]);
        assert_eq!(comments["pr"], value["pr"]);
    }

    #[test]
    fn review_target_summary_uses_merge_request_noun() {
        let diff = MultiFileDiff::from_file_pair(
            PathBuf::from("old.txt"),
            PathBuf::from("new.txt"),
            "old\n".to_string(),
            "new\n".to_string(),
        );
        let mut app = App::new(diff, ViewMode::UnifiedPane, 0, false, None);
        app.set_review_target_metadata(Some(review_pr_target_metadata(&test_gitlab_pr())));

        assert_eq!(review_target_summary(&app).label, "MR #1 (feature)");
    }

    #[test]
    fn review_log_dedupes_current_git_snapshots() {
        let current = ReviewTargetMetadata {
            label: "feature".to_string(),
            vcs: "git".to_string(),
            jj_change_id: None,
            jj_commit_id: None,
            git_base_ref: Some("main".to_string()),
            git_head_ref: Some("feature".to_string()),
            git_base_commit: Some("base".to_string()),
            git_head_commit: Some("head".to_string()),
            branch: Some("feature".to_string()),
            pr_provider: None,
            pr_repo: None,
            pr_number: None,
            author: None,
            timestamp: None,
            bookmarks: None,
        };
        let entries = vec![
            serde_json::json!({
                "diffFingerprint": "newer",
                "target": {
                    "label": "feature",
                    "vcs": "git",
                    "gitBaseCommit": "head",
                    "branch": "feature"
                },
                "commentCount": 4
            }),
            serde_json::json!({
                "diffFingerprint": "older",
                "target": {
                    "label": "feature",
                    "vcs": "git",
                    "gitHeadCommit": "head",
                    "branch": "feature"
                },
                "commentCount": 4
            }),
        ];

        let deduped = dedupe_review_log_entries(entries, None, &current);

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0]["diffFingerprint"], "newer");
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
