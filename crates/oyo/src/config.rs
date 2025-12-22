//! Configuration file support for oyo
//!
//! Config file location: `~/.config/oyo/config.toml` (XDG_CONFIG_HOME)
//!
//! Example config:
//! ```toml
//! [ui]
//! zen = false
//! auto_center = true
//! view_mode = "single"
//! line_wrap = false
//! scrollbar = false
//! strikethrough_deletions = false
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
//! animation = false
//! auto_step_on_enter = true
//! auto_step_blank_files = true
//!
//! [files]
//! panel_visible = true
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
    pub syntax_plain: Option<DarkLight>,
    pub syntax_keyword: Option<DarkLight>,
    pub syntax_string: Option<DarkLight>,
    pub syntax_number: Option<DarkLight>,
    pub syntax_comment: Option<DarkLight>,
    pub syntax_type: Option<DarkLight>,
    pub syntax_function: Option<DarkLight>,
    pub syntax_variable: Option<DarkLight>,
    pub syntax_constant: Option<DarkLight>,
    pub syntax_operator: Option<DarkLight>,
    pub syntax_punctuation: Option<DarkLight>,
    pub background: Option<DarkLight>,
    pub background_panel: Option<DarkLight>,
    pub background_element: Option<DarkLight>,
    pub border: Option<DarkLight>,
    pub border_active: Option<DarkLight>,
    pub border_subtle: Option<DarkLight>,
    pub diff_added: Option<DarkLight>,
    pub diff_removed: Option<DarkLight>,
    pub diff_context: Option<DarkLight>,
    pub diff_line_number: Option<DarkLight>,
    pub diff_ext_marker: Option<DarkLight>,
}

/// Theme configuration (defs + tokens)
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Theme mode: "dark" or "light"
    pub mode: Option<String>,
    /// Named color definitions (e.g., green1 = "#A3BE8C")
    pub defs: HashMap<String, String>,
    /// Theme tokens with dark/light values
    pub theme: ThemeTokens,
}

impl ThemeConfig {
    /// Check if config specifies light mode
    pub fn is_light_mode(&self) -> bool {
        self.mode
            .as_ref()
            .map(|m| m.eq_ignore_ascii_case("light"))
            .unwrap_or(false)
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

    // Syntax
    pub syntax_plain: Color,
    pub syntax_keyword: Color,
    pub syntax_string: Color,
    pub syntax_number: Color,
    pub syntax_comment: Color,
    pub syntax_type: Color,
    pub syntax_function: Color,
    pub syntax_variable: Color,
    pub syntax_constant: Color,
    pub syntax_operator: Color,
    pub syntax_punctuation: Color,

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
        let defs = &self.defs;
        let tokens = &self.theme;

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

            // Syntax (fallbacks align to existing UI colors)
            syntax_plain: resolve(&tokens.syntax_plain, Color::Reset),
            syntax_keyword: resolve(&tokens.syntax_keyword, resolve(&tokens.accent, Color::Cyan)),
            syntax_string: resolve(&tokens.syntax_string, resolve(&tokens.success, Color::Green)),
            syntax_number: resolve(&tokens.syntax_number, resolve(&tokens.warning, Color::Yellow)),
            syntax_comment: resolve(&tokens.syntax_comment, resolve(&tokens.text_muted, Color::DarkGray)),
            syntax_type: resolve(&tokens.syntax_type, resolve(&tokens.primary, Color::Cyan)),
            syntax_function: resolve(&tokens.syntax_function, resolve(&tokens.info, Color::Blue)),
            syntax_variable: resolve(&tokens.syntax_variable, resolve(&tokens.error, Color::Red)),
            syntax_constant: resolve(&tokens.syntax_constant, resolve(&tokens.secondary, Color::Cyan)),
            syntax_operator: resolve(&tokens.syntax_operator, resolve(&tokens.text, Color::Reset)),
            syntax_punctuation: resolve(&tokens.syntax_punctuation, resolve(&tokens.text_muted, Color::DarkGray)),

            // Backgrounds - transparent by default
            background: resolve_bg(&tokens.background),
            background_panel: resolve_bg(&tokens.background_panel),
            background_element: resolve_bg(&tokens.background_element),

            // Borders
            border: resolve(&tokens.border, Color::DarkGray),
            border_active: resolve(&tokens.border_active, Color::Gray),
            border_subtle: resolve(&tokens.border_subtle, Color::DarkGray),

            // Diff
            diff_context: resolve(&tokens.diff_context, Color::Reset),
            diff_line_number: resolve(&tokens.diff_line_number, Color::DarkGray),
            diff_ext_marker: resolve(&tokens.diff_ext_marker, Color::DarkGray),

            // Animation gradients derived from diff colors
            insert: color::gradient_from_color(diff_added),
            delete: color::gradient_from_color(diff_removed),
            modify: color::gradient_from_color(warning),
        }
    }
}

/// UI configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Start in zen mode (minimal UI)
    pub zen: bool,
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
    /// Syntax highlighting: "auto", "on", or "off"
    pub syntax: SyntaxMode,
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
            auto_center: true,
            view_mode: None,
            line_wrap: false,
            scrollbar: false,
            strikethrough_deletions: false,
            syntax: SyntaxMode::Auto,
            stepping: true,
            primary_marker: "▶".to_string(),
            primary_marker_right: None,
            extent_marker: "▌".to_string(),
            extent_marker_right: None,
            theme: ThemeConfig::default(),
        }
    }
}

/// Syntax highlighting mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyntaxMode {
    Auto,
    On,
    Off,
}

impl Default for SyntaxMode {
    fn default() -> Self {
        SyntaxMode::Auto
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
            animation: false,
            animation_duration: 150,
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
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            panel_visible: true,
        }
    }
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
