//! Configuration file support for oyo
//!
//! Config file location: `~/.config/oyo/config.toml` (XDG_CONFIG_HOME)
//!
//! Example config:
//! ```toml
//! [ui]
//! zen = false
//! topbar = true
//! auto_center = true
//! view_mode = "single"
//! line_wrap = false
//! scrollbar = false
//! strikethrough_deletions = false
//! gutter_signs = true
//! # [ui.split]
//! # align_lines = false
//! # align_fill = "╱"
//! primary_marker = "▶"
//! primary_marker_right = "◀"
//! extent_marker = "▌"
//! extent_marker_right = "▐"
//!
//! [ui.theme.defs]
//! oyo14 = "#A3BE8C"
//! oyo11 = "#BF616A"
//!
//! [ui.theme.theme.diffAdded]
//! dark = "oyo14"
//!
//! [ui.theme.theme.diffRemoved]
//! dark = "oyo11"
//!
//! [playback]
//! speed = 200
//! autoplay = false
//! animation = true
//! auto_step_on_enter = true
//! auto_step_blank_files = true
//!
//! [files]
//! panel_visible = true
//! counts = "active"
//! ```

use crate::color::{self, AnimationGradient};
use ratatui::style::Color;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

// ============================================================================
// Theme Configuration
// ============================================================================

/// Dark/light color pair for a theme token
#[derive(Debug, Clone, Deserialize)]
pub struct DarkLight {
    pub dark: String,
    #[serde(default)]
    #[allow(dead_code)] // Reserved for future light theme support
    pub light: Option<String>,
}

/// Theme tokens (opencode schema)
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemeTokens {
    pub text: Option<DarkLight>,
    pub text_muted: Option<DarkLight>,
    pub primary: Option<DarkLight>,
    pub secondary: Option<DarkLight>,
    pub accent: Option<DarkLight>,
    pub error: Option<DarkLight>,
    pub warning: Option<DarkLight>,
    pub success: Option<DarkLight>,
    pub info: Option<DarkLight>,
    pub background: Option<DarkLight>,
    pub background_panel: Option<DarkLight>,
    pub background_element: Option<DarkLight>,
    pub border: Option<DarkLight>,
    pub border_active: Option<DarkLight>,
    pub border_subtle: Option<DarkLight>,
    pub diff_added: Option<DarkLight>,
    pub diff_added_bg: Option<DarkLight>,
    pub diff_removed: Option<DarkLight>,
    pub diff_removed_bg: Option<DarkLight>,
    pub diff_context: Option<DarkLight>,
    pub diff_line_number: Option<DarkLight>,
    pub diff_ext_marker: Option<DarkLight>,
    pub diff_modified_bg: Option<DarkLight>,
}

/// Theme configuration (defs + tokens)
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Built-in theme name (e.g., "tokyonight")
    pub name: Option<String>,
    /// Theme mode: "dark" or "light"
    pub mode: Option<String>,
    /// Named color definitions (e.g., green1 = "#A3BE8C")
    pub defs: HashMap<String, String>,
    /// Theme tokens with dark/light values
    pub theme: ThemeTokens,
}

const BUILTIN_THEMES: &[(&str, &str)] = &[
    (
        "aura",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/aura.json")),
    ),
    (
        "ayu",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/ayu.json")),
    ),
    (
        "catppuccin",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/catppuccin.json"
        )),
    ),
    (
        "catppuccin-frappe",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/catppuccin-frappe.json"
        )),
    ),
    (
        "catppuccin-macchiato",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/catppuccin-macchiato.json"
        )),
    ),
    (
        "cobalt2",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/cobalt2.json")),
    ),
    (
        "dracula",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/dracula.json")),
    ),
    (
        "everforest",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/everforest.json"
        )),
    ),
    (
        "flexoki",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/flexoki.json")),
    ),
    (
        "github",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/github.json")),
    ),
    (
        "gruvbox",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/gruvbox.json")),
    ),
    (
        "kanagawa",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/kanagawa.json")),
    ),
    (
        "material",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/material.json")),
    ),
    (
        "monokai",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/monokai.json")),
    ),
    (
        "nightowl",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/nightowl.json")),
    ),
    (
        "nord",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/nord.json")),
    ),
    (
        "one-dark",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/one-dark.json")),
    ),
    (
        "palenight",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/palenight.json"
        )),
    ),
    (
        "rosepine",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/rosepine.json")),
    ),
    (
        "solarized",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/solarized.json"
        )),
    ),
    (
        "synthwave84",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/synthwave84.json"
        )),
    ),
    (
        "tokyonight",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/tokyonight.json"
        )),
    ),
    (
        "zenburn",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/zenburn.json")),
    ),
];

pub fn builtin_theme_names() -> Vec<&'static str> {
    BUILTIN_THEMES.iter().map(|(name, _)| *name).collect()
}

impl ThemeConfig {
    /// Check if config specifies light mode
    pub fn is_light_mode(&self) -> bool {
        self.mode
            .as_ref()
            .map(|m| m.eq_ignore_ascii_case("light"))
            .unwrap_or(false)
    }

    fn resolved_config(&self) -> ThemeConfig {
        let mut base = self
            .name
            .as_deref()
            .and_then(ThemeConfig::builtin)
            .unwrap_or_default();

        base.name = self.name.clone();
        if self.mode.is_some() {
            base.mode = self.mode.clone();
        }
        base.defs.extend(self.defs.clone());
        merge_theme_tokens(&mut base.theme, &self.theme);
        base
    }

    fn builtin(name: &str) -> Option<ThemeConfig> {
        let key = name.to_ascii_lowercase();
        let json = BUILTIN_THEMES
            .iter()
            .find(|(theme_name, _)| *theme_name == key)
            .map(|(_, json)| *json)?;
        let mut config: ThemeConfig =
            serde_json::from_str(json).expect("builtin theme JSON should parse");
        config.name = Some(key);
        Some(config)
    }
}

/// Resolved theme — all ratatui Colors ready to use
#[derive(Debug, Clone)]
pub struct ResolvedTheme {
    // Core UI
    pub text: Color,
    pub text_muted: Color,
    pub primary: Color,
    pub accent: Color,

    // Status
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub info: Color,

    // Backgrounds (None = transparent)
    pub background: Option<Color>,
    pub background_panel: Option<Color>,
    pub background_element: Option<Color>,

    // Borders
    #[allow(dead_code)]
    pub border: Color,
    pub border_active: Color,
    pub border_subtle: Color,

    // Diff
    pub diff_context: Color,
    pub diff_line_number: Color,
    pub diff_ext_marker: Color,
    pub diff_added_bg: Option<Color>,
    pub diff_removed_bg: Option<Color>,
    pub diff_modified_bg: Option<Color>,

    // Animation gradients (derived from diff colors)
    pub insert: AnimationGradient,
    pub delete: AnimationGradient,
    pub modify: AnimationGradient,
}

impl ResolvedTheme {
    /// Get dimmed version of insert color for inactive spans
    pub fn insert_dim(&self) -> Color {
        color::dim_color_from_gradient(&self.insert)
    }

    /// Get base insert color (for animation start/end)
    pub fn insert_base(&self) -> Color {
        let rgb = color::hsl_to_rgb(self.insert.base);
        Color::Rgb(rgb.r, rgb.g, rgb.b)
    }

    /// Get dimmed version of delete color for inactive spans
    pub fn delete_dim(&self) -> Color {
        color::dim_color_from_gradient(&self.delete)
    }

    /// Get base delete color (for animation start/end)
    pub fn delete_base(&self) -> Color {
        let rgb = color::hsl_to_rgb(self.delete.base);
        Color::Rgb(rgb.r, rgb.g, rgb.b)
    }

    /// Get dimmed version of modify color for inactive spans
    pub fn modify_dim(&self) -> Color {
        color::dim_color_from_gradient(&self.modify)
    }

    /// Get base modify color (for animation start/end)
    pub fn modify_base(&self) -> Color {
        let rgb = color::hsl_to_rgb(self.modify.base);
        Color::Rgb(rgb.r, rgb.g, rgb.b)
    }

    /// Get dimmed version of warning for autoplay flash
    pub fn warning_dim(&self) -> Color {
        color::dim_color(self.warning)
    }
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        ThemeConfig::default().resolve(false)
    }
}

impl ThemeConfig {
    /// Resolve theme config to concrete colors
    /// If light_mode is true, prefers .light values, falls back to .dark
    pub fn resolve(&self, light_mode: bool) -> ResolvedTheme {
        let merged = self.resolved_config();
        let defs = &merged.defs;
        let tokens = &merged.theme;

        // Helper to resolve a token with fallback
        // In light mode: try .light first, fall back to .dark
        let resolve = |token: &Option<DarkLight>, fallback: Color| -> Color {
            token
                .as_ref()
                .and_then(|dl| {
                    if light_mode {
                        // Try light first, fallback to dark
                        dl.light
                            .as_ref()
                            .and_then(|v| color::resolve_color(v, defs))
                            .or_else(|| color::resolve_color(&dl.dark, defs))
                    } else {
                        color::resolve_color(&dl.dark, defs)
                    }
                })
                .unwrap_or(fallback)
        };

        // Helper for optional background colors (None = transparent)
        let resolve_bg = |token: &Option<DarkLight>| -> Option<Color> {
            token.as_ref().and_then(|dl| {
                let value_str = if light_mode {
                    dl.light.as_ref().unwrap_or(&dl.dark)
                } else {
                    &dl.dark
                };
                let value = value_str.trim().to_lowercase();
                if value == "transparent" || value == "none" {
                    None
                } else {
                    color::resolve_color(value_str, defs)
                }
            })
        };

        // Resolve diff colors first (needed for gradients)
        let diff_added = resolve(&tokens.diff_added, Color::Green);
        let diff_removed = resolve(&tokens.diff_removed, Color::Red);
        let warning = resolve(&tokens.warning, Color::Yellow);

        let background = resolve_bg(&tokens.background);
        let background_panel = resolve_bg(&tokens.background_panel);
        let background_element = resolve_bg(&tokens.background_element);
        let base_bg = background.or(background_panel).or(background_element);

        let diff_added_bg = resolve_bg(&tokens.diff_added_bg)
            .or_else(|| base_bg.and_then(|bg| color::blend_colors(bg, diff_added, 0.18)));
        let diff_removed_bg = resolve_bg(&tokens.diff_removed_bg)
            .or_else(|| base_bg.and_then(|bg| color::blend_colors(bg, diff_removed, 0.18)));
        let diff_modified_bg = resolve_bg(&tokens.diff_modified_bg)
            .or_else(|| base_bg.and_then(|bg| color::blend_colors(bg, warning, 0.16)));

        ResolvedTheme {
            // Core UI - ANSI defaults for terminal palette compatibility
            text: resolve(&tokens.text, Color::Reset),
            text_muted: resolve(&tokens.text_muted, Color::DarkGray),
            primary: resolve(&tokens.primary, Color::Cyan),
            accent: resolve(&tokens.accent, Color::Cyan),

            // Status
            error: resolve(&tokens.error, Color::Red),
            warning,
            success: resolve(&tokens.success, Color::Green),
            info: resolve(&tokens.info, Color::Blue),

            // Backgrounds - transparent by default
            background,
            background_panel,
            background_element,

            // Borders
            border: resolve(&tokens.border, Color::DarkGray),
            border_active: resolve(&tokens.border_active, Color::Gray),
            border_subtle: resolve(&tokens.border_subtle, Color::DarkGray),

            // Diff
            diff_context: resolve(&tokens.diff_context, Color::Reset),
            diff_line_number: resolve(&tokens.diff_line_number, Color::DarkGray),
            diff_ext_marker: resolve(&tokens.diff_ext_marker, Color::DarkGray),
            diff_added_bg,
            diff_removed_bg,
            diff_modified_bg,

            // Animation gradients derived from diff colors
            insert: color::gradient_from_color(diff_added),
            delete: color::gradient_from_color(diff_removed),
            modify: color::gradient_from_color(warning),
        }
    }
}

fn merge_theme_tokens(base: &mut ThemeTokens, overlay: &ThemeTokens) {
    if overlay.text.is_some() {
        base.text = overlay.text.clone();
    }
    if overlay.text_muted.is_some() {
        base.text_muted = overlay.text_muted.clone();
    }
    if overlay.primary.is_some() {
        base.primary = overlay.primary.clone();
    }
    if overlay.secondary.is_some() {
        base.secondary = overlay.secondary.clone();
    }
    if overlay.accent.is_some() {
        base.accent = overlay.accent.clone();
    }
    if overlay.error.is_some() {
        base.error = overlay.error.clone();
    }
    if overlay.warning.is_some() {
        base.warning = overlay.warning.clone();
    }
    if overlay.success.is_some() {
        base.success = overlay.success.clone();
    }
    if overlay.info.is_some() {
        base.info = overlay.info.clone();
    }
    if overlay.background.is_some() {
        base.background = overlay.background.clone();
    }
    if overlay.background_panel.is_some() {
        base.background_panel = overlay.background_panel.clone();
    }
    if overlay.background_element.is_some() {
        base.background_element = overlay.background_element.clone();
    }
    if overlay.border.is_some() {
        base.border = overlay.border.clone();
    }
    if overlay.border_active.is_some() {
        base.border_active = overlay.border_active.clone();
    }
    if overlay.border_subtle.is_some() {
        base.border_subtle = overlay.border_subtle.clone();
    }
    if overlay.diff_added.is_some() {
        base.diff_added = overlay.diff_added.clone();
    }
    if overlay.diff_added_bg.is_some() {
        base.diff_added_bg = overlay.diff_added_bg.clone();
    }
    if overlay.diff_removed.is_some() {
        base.diff_removed = overlay.diff_removed.clone();
    }
    if overlay.diff_removed_bg.is_some() {
        base.diff_removed_bg = overlay.diff_removed_bg.clone();
    }
    if overlay.diff_context.is_some() {
        base.diff_context = overlay.diff_context.clone();
    }
    if overlay.diff_line_number.is_some() {
        base.diff_line_number = overlay.diff_line_number.clone();
    }
    if overlay.diff_ext_marker.is_some() {
        base.diff_ext_marker = overlay.diff_ext_marker.clone();
    }
    if overlay.diff_modified_bg.is_some() {
        base.diff_modified_bg = overlay.diff_modified_bg.clone();
    }
}

/// UI configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Start in zen mode (minimal UI)
    pub zen: bool,
    /// Show top bar in diff view
    pub topbar: bool,
    /// Auto-center on active change after stepping (like vim's zz)
    pub auto_center: bool,
    /// Default view mode: "single", "split", or "evolution"
    pub view_mode: Option<String>,
    /// Enable line wrapping (default: false, uses horizontal scroll instead)
    pub line_wrap: bool,
    /// Show scrollbar (default: false)
    pub scrollbar: bool,
    /// Show strikethrough on deleted text
    pub strikethrough_deletions: bool,
    /// Show +/- sign column in the gutter (single/evolution)
    pub gutter_signs: bool,
    /// Syntax highlighting configuration
    pub syntax: SyntaxConfig,
    /// Single-pane view settings
    pub single: SingleViewConfig,
    /// Split view settings
    pub split: SplitViewConfig,
    /// Evolution view settings
    pub evo: EvoViewConfig,
    /// Diff styling settings
    pub diff: DiffConfig,
    /// Enable stepping (default: true). If false, shows all changes (no-step behavior)
    pub stepping: bool,
    /// Marker for primary active line (left pane / single pane)
    pub primary_marker: String,
    /// Marker for right pane primary line (defaults to ◀)
    pub primary_marker_right: Option<String>,
    /// Marker for hunk extent lines (left pane / single pane)
    pub extent_marker: String,
    /// Marker for right pane extent lines (defaults to ▐)
    pub extent_marker_right: Option<String>,
    /// Theme configuration
    pub theme: ThemeConfig,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            zen: false,
            topbar: true,
            auto_center: true,
            view_mode: None,
            line_wrap: false,
            scrollbar: false,
            strikethrough_deletions: false,
            gutter_signs: true,
            syntax: SyntaxConfig::default(),
            single: SingleViewConfig::default(),
            split: SplitViewConfig::default(),
            evo: EvoViewConfig::default(),
            diff: DiffConfig::default(),
            stepping: true,
            primary_marker: "▶".to_string(),
            primary_marker_right: None,
            extent_marker: "▌".to_string(),
            extent_marker_right: None,
            theme: ThemeConfig::default(),
        }
    }
}

/// Split view configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SplitViewConfig {
    /// Insert blank rows for missing lines to keep panes vertically aligned
    pub align_lines: bool,
    /// Fill character for aligned blank rows (empty = no marker)
    pub align_fill: String,
}

impl Default for SplitViewConfig {
    fn default() -> Self {
        Self {
            align_lines: false,
            align_fill: "╱".to_string(),
        }
    }
}

/// Single-pane configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SingleViewConfig {
    /// How modified lines render while stepping: "mixed" or "modified"
    pub modified_step_mode: ModifiedStepMode,
}

impl Default for SingleViewConfig {
    fn default() -> Self {
        Self {
            modified_step_mode: ModifiedStepMode::Mixed,
        }
    }
}

/// Evolution view configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EvoViewConfig {
    /// Syntax scope in evolution view: "context" or "full"
    pub syntax: EvoSyntaxMode,
}

impl Default for EvoViewConfig {
    fn default() -> Self {
        Self {
            syntax: EvoSyntaxMode::Context,
        }
    }
}

/// Diff styling configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DiffConfig {
    /// Diff background (full-line) toggle
    #[serde(default = "diff_bg_default")]
    pub bg: bool,
    /// Diff foreground mode: "theme" or "syntax"
    #[serde(default = "diff_fg_default")]
    pub fg: DiffForegroundMode,
    /// Inline diff highlight mode: "word", "text", or "none"
    #[serde(default = "diff_highlight_default")]
    pub highlight: DiffHighlightMode,
    /// Extent marker color mode: "neutral" or "diff"
    #[serde(default = "diff_extent_marker_default")]
    pub extent_marker: DiffExtentMarkerMode,
    /// Extent marker scope: "progress" or "hunk"
    #[serde(default = "diff_extent_marker_scope_default")]
    pub extent_marker_scope: DiffExtentMarkerScope,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            bg: diff_bg_default(),
            fg: diff_fg_default(),
            highlight: diff_highlight_default(),
            extent_marker: diff_extent_marker_default(),
            extent_marker_scope: diff_extent_marker_scope_default(),
        }
    }
}

fn diff_bg_default() -> bool {
    false
}

fn diff_fg_default() -> DiffForegroundMode {
    DiffForegroundMode::Theme
}

fn diff_highlight_default() -> DiffHighlightMode {
    DiffHighlightMode::Text
}

fn diff_extent_marker_default() -> DiffExtentMarkerMode {
    DiffExtentMarkerMode::Neutral
}

fn diff_extent_marker_scope_default() -> DiffExtentMarkerScope {
    DiffExtentMarkerScope::Progress
}
/// Evolution view syntax scope
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EvoSyntaxMode {
    /// Syntax highlight only non-diff context lines
    #[default]
    Context,
    /// Syntax highlight all non-active lines (including diffs)
    Full,
}

/// Single-pane modified line rendering mode
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModifiedStepMode {
    #[default]
    Mixed,
    Modified,
}

/// Diff foreground rendering mode
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiffForegroundMode {
    #[default]
    Theme,
    Syntax,
}

/// Inline diff highlight mode
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiffHighlightMode {
    #[default]
    Text,
    Word,
    None,
}

/// Extent marker color mode
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiffExtentMarkerMode {
    #[default]
    Neutral,
    Diff,
}

/// Extent marker scope
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiffExtentMarkerScope {
    #[default]
    Progress,
    Hunk,
}

/// Syntax highlighting mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyntaxMode {
    #[default]
    On,
    Off,
}

/// Syntax highlighting configuration
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "SyntaxConfigDef")]
pub struct SyntaxConfig {
    pub mode: SyntaxMode,
    pub theme: String,
}

impl Default for SyntaxConfig {
    fn default() -> Self {
        Self {
            mode: SyntaxMode::On,
            theme: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SyntaxConfigDef {
    Mode(SyntaxMode),
    Detailed {
        #[serde(default)]
        mode: SyntaxMode,
        theme: Option<String>,
    },
}

impl From<SyntaxConfigDef> for SyntaxConfig {
    fn from(def: SyntaxConfigDef) -> Self {
        match def {
            SyntaxConfigDef::Mode(mode) => Self {
                mode,
                ..Self::default()
            },
            SyntaxConfigDef::Detailed { mode, theme } => Self {
                mode,
                theme: theme.unwrap_or_default(),
            },
        }
    }
}

/// Playback configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PlaybackConfig {
    /// Autoplay speed in milliseconds (delay between steps)
    pub speed: u64,
    /// Start with autoplay enabled
    pub autoplay: bool,
    /// Enable step animations (fade in/out effects)
    pub animation: bool,
    /// Animation duration in milliseconds (how long fade effects take)
    pub animation_duration: u64,
    /// Auto-step to first change when entering a file at step 0
    pub auto_step_on_enter: bool,
    /// Auto-step when file would be blank at step 0 (new files)
    pub auto_step_blank_files: bool,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            speed: 200,
            autoplay: false,
            animation: true,
            animation_duration: 120,
            auto_step_on_enter: true,
            auto_step_blank_files: true,
        }
    }
}

/// Files panel configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct FilesConfig {
    /// Show file panel by default in multi-file mode
    pub panel_visible: bool,
    /// When to show per-file +/- counts in the file panel
    pub counts: FileCountMode,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            panel_visible: true,
            counts: FileCountMode::Active,
        }
    }
}

/// File list counts display behavior
#[derive(Debug, Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum FileCountMode {
    #[default]
    Active,
    Focused,
    All,
    Off,
}

/// Root configuration
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub playback: PlaybackConfig,
    pub files: FilesConfig,
}

impl Config {
    /// Get all possible config file paths in priority order
    fn config_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // 1. XDG_CONFIG_HOME (if set)
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            paths.push(PathBuf::from(xdg).join("oyo").join("config.toml"));
        }

        // 2. ~/.config/oyo/config.toml (XDG default, works on all platforms)
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".config").join("oyo").join("config.toml"));
        }

        // 3. Platform-specific config dir (~/Library/Application Support on macOS)
        if let Some(config_dir) = dirs::config_dir() {
            let platform_path = config_dir.join("oyo").join("config.toml");
            // Avoid duplicate if it's the same as ~/.config
            if !paths.contains(&platform_path) {
                paths.push(platform_path);
            }
        }

        paths
    }

    /// Get the first existing config file path
    pub fn config_path() -> Option<PathBuf> {
        Self::config_paths().into_iter().find(|p| p.exists())
    }

    /// Load config from XDG config path
    /// Returns default config if file doesn't exist or can't be parsed
    pub fn load() -> Self {
        Self::config_path()
            .and_then(|path| std::fs::read_to_string(&path).ok())
            .and_then(|content| {
                toml::from_str(&content)
                    .map_err(|e| {
                        eprintln!("Warning: Failed to parse config: {}", e);
                        e
                    })
                    .ok()
            })
            .unwrap_or_default()
    }

    /// Parse view mode string to ViewMode enum
    pub fn parse_view_mode(&self) -> Option<crate::app::ViewMode> {
        self.ui.view_mode.as_ref().and_then(|s| match s.as_str() {
            "single" => Some(crate::app::ViewMode::SinglePane),
            "split" | "sbs" => Some(crate::app::ViewMode::Split),
            "evolution" | "evo" => Some(crate::app::ViewMode::Evolution),
            _ => None,
        })
    }
}
