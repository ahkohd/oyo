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
//! primary_marker = "▶"
//! primary_marker_right = "◀"
//! extent_marker = "▌"
//! extent_marker_right = "▐"
//!
//! [playback]
//! speed = 200
//! autoplay = false
//! animation = false
//! auto_step_on_enter = false
//!
//! [files]
//! panel_visible = true
//! ```

use serde::Deserialize;
use std::path::PathBuf;

/// UI configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Start in zen mode (minimal UI)
    pub zen: bool,
    /// Auto-center on active change after stepping (like vim's zz)
    pub auto_center: bool,
    /// Default view mode: "single", "side-by-side", or "evolution"
    pub view_mode: Option<String>,
    /// Enable line wrapping (default: false, uses horizontal scroll instead)
    pub line_wrap: bool,
    /// Show scrollbar (default: false)
    pub scrollbar: bool,
    /// Marker for primary active line (left pane / single pane)
    pub primary_marker: String,
    /// Marker for right pane primary line (defaults to ◀)
    pub primary_marker_right: Option<String>,
    /// Marker for hunk extent lines (left pane / single pane)
    pub extent_marker: String,
    /// Marker for right pane extent lines (defaults to ▐)
    pub extent_marker_right: Option<String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            zen: false,
            auto_center: true,
            view_mode: None,
            line_wrap: false,
            scrollbar: false,
            primary_marker: "▶".to_string(),
            primary_marker_right: None,
            extent_marker: "▌".to_string(),
            extent_marker_right: None,
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
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            speed: 200,
            autoplay: false,
            animation: false,
            animation_duration: 150,
            auto_step_on_enter: false,
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
            "side-by-side" | "sbs" | "split" => Some(crate::app::ViewMode::SideBySide),
            "evolution" | "evo" => Some(crate::app::ViewMode::Evolution),
            _ => None,
        })
    }
}
