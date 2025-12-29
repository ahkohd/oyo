//! Syntax highlighting helpers (syntect-backed)

use crate::config::Config;
use ratatui::style::{Color as TuiColor, Modifier, Style};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use syntect::{
    easy::HighlightLines,
    highlighting::{Color, FontStyle, Style as SynStyle, Theme, ThemeSet},
    parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxSide {
    Old,
    New,
}

#[derive(Clone, Debug)]
pub struct SyntaxSpan {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug)]
pub struct SyntaxCache {
    old: Vec<Vec<SyntaxSpan>>,
    new: Vec<Vec<SyntaxSpan>>,
}

struct EmbeddedTmTheme {
    name: &'static str,
    data: &'static [u8],
}

// Embedded tmTheme payloads for built-in UI themes.
const EMBEDDED_TMTHEMES: &[EmbeddedTmTheme] = &[
    EmbeddedTmTheme {
        name: "aura",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/aura-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "ayu",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/ayu-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "catppuccin",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/catppuccin-mocha.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "catppuccin-mocha",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/catppuccin-mocha.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "catppuccin-frappe",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/catppuccin-frappe.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "catppuccin-macchiato",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/catppuccin-macchiato.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "catppuccin-latte",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/catppuccin-latte.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "cobalt2",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/cobalt2-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "dracula",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/dracula-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "everforest",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/everforest-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "everforest-light",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/everforest-light.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "flexoki",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/flexoki-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "flexoki-light",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/flexoki-light.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "github",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/github-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "github-light",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/github-light.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "gruvbox",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/gruvbox-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "gruvbox-light",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/gruvbox-light.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "kanagawa",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/kanagawa-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "material",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/material-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "monokai",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/monokai-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "nightowl",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/nightowl-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "nightowl-light",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/nightowl-light.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "nord",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/nord-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "one-dark",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/one-dark-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "one-dark-light",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/one-dark-light.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "palenight",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/palenight-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "rosepine",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/rosepine-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "rosepine-dawn",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/rosepine-dawn.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "solarized",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/solarized-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "solarized-light",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/solarized-light.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "synthwave84",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/synthwave84-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "tokyonight",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/tokyonight-dark.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "tokyonight-day",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/tokyonight-day.tmTheme"
        )),
    },
    EmbeddedTmTheme {
        name: "zenburn",
        data: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/syntax/zenburn-dark.tmTheme"
        )),
    },
];

pub struct SyntaxEngine {
    syntax_set: SyntaxSet,
    theme: Theme,
    plain: TuiColor,
}

impl SyntaxEngine {
    pub fn new(syntax_theme: &str, light_mode: bool) -> Self {
        let syntax_set = two_face::syntax::extra_newlines();
        let (syntax_theme, plain) = resolve_syntax_theme(syntax_theme, light_mode);
        Self {
            syntax_set,
            theme: syntax_theme,
            plain,
        }
    }

    pub fn highlight(&self, content: &str, file_name: &str) -> Vec<Vec<SyntaxSpan>> {
        let syntax = self.syntax_for_file(file_name);
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let mut out = Vec::new();

        for line in LinesWithEndings::from(content) {
            let mut spans = Vec::new();
            let ranges = highlighter
                .highlight_line(line, &self.syntax_set)
                .unwrap_or_default();
            for (style, text) in ranges {
                let text = text.strip_suffix('\n').unwrap_or(text);
                let text = text.strip_suffix('\r').unwrap_or(text);
                if text.is_empty() {
                    continue;
                }
                spans.push(SyntaxSpan {
                    text: text.to_string(),
                    style: syntect_style_to_tui(style),
                });
            }
            if spans.is_empty() {
                spans.push(SyntaxSpan {
                    text: String::new(),
                    style: Style::default().fg(self.plain),
                });
            }
            out.push(spans);
        }

        // Handle empty file (no lines)
        if out.is_empty() {
            out.push(vec![SyntaxSpan {
                text: String::new(),
                style: Style::default().fg(self.plain),
            }]);
        }

        out
    }

    pub fn collect_scopes(&self, content: &str, file_name: &str) -> BTreeMap<String, usize> {
        let syntax = self.syntax_for_file(file_name);
        let mut state = ParseState::new(syntax);
        let mut stack = ScopeStack::new();
        let mut counts = BTreeMap::new();

        for line in LinesWithEndings::from(content) {
            let ops = state.parse_line(line, &self.syntax_set).unwrap_or_default();
            for (_, op) in &ops {
                stack.apply(op).ok();
                for scope in stack.scopes.iter() {
                    *counts.entry(scope.to_string()).or_insert(0) += 1;
                }
            }
        }

        counts
    }

    pub fn syntax_name_for_file(&self, file_name: &str) -> &str {
        &self.syntax_for_file(file_name).name
    }

    pub fn scopes_for_line(
        &self,
        content: &str,
        file_name: &str,
        line_index: usize,
    ) -> Vec<String> {
        let syntax = self.syntax_for_file(file_name);
        let mut state = ParseState::new(syntax);
        let mut stack = ScopeStack::new();
        let mut scopes: BTreeSet<String> = BTreeSet::new();

        for (idx, line) in LinesWithEndings::from(content).enumerate() {
            let ops = state.parse_line(line, &self.syntax_set).unwrap_or_default();
            if idx == line_index {
                for (_, op) in ops {
                    stack.apply(&op).ok();
                    for scope in stack.scopes.iter() {
                        scopes.insert(scope.to_string());
                    }
                }
                break;
            }
            for (_, op) in ops {
                stack.apply(&op).ok();
            }
        }

        scopes.into_iter().collect()
    }

    fn syntax_for_file(&self, file_name: &str) -> &SyntaxReference {
        self.syntax_set
            .find_syntax_for_file(file_name)
            .ok()
            .flatten()
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
    }
}

fn resolve_syntax_theme(theme_name: &str, light_mode: bool) -> (Theme, TuiColor) {
    let (mut ansi_theme, ansi_plain) = load_ansi_theme();
    strip_theme_backgrounds(&mut ansi_theme);
    ensure_foreground(&mut ansi_theme, ansi_plain);

    let mut candidates = Vec::new();
    if is_explicit_variant(theme_name) {
        candidates.push(theme_name.to_string());
    } else if light_mode {
        candidates.extend(light_variants(theme_name));
        candidates.push(theme_name.to_string());
        candidates.extend(dark_variants(theme_name));
    } else {
        candidates.extend(dark_variants(theme_name));
        candidates.push(theme_name.to_string());
        candidates.extend(light_variants(theme_name));
    }

    for candidate in candidates {
        if let Some(mut theme) = load_theme_candidate(&candidate) {
            strip_theme_backgrounds(&mut theme);
            let plain = theme.settings.foreground.map(to_tui).unwrap_or(ansi_plain);
            ensure_foreground(&mut theme, plain);
            return (theme, plain);
        }
    }

    (ansi_theme, ansi_plain)
}

fn load_theme_candidate(name: &str) -> Option<Theme> {
    if name.ends_with(".tmTheme") {
        return load_tmtheme(name);
    }
    load_embedded_syntax_theme(name).or_else(|| load_tmtheme(name))
}

fn load_tmtheme(name: &str) -> Option<Theme> {
    let path = resolve_tmtheme_path(name);
    if !path.exists() {
        return None;
    }
    ThemeSet::get_theme(path).ok()
}

fn resolve_tmtheme_path(name: &str) -> PathBuf {
    let with_extension = if name
        .rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tmtheme"))
    {
        None
    } else {
        Some(format!("{name}.tmTheme"))
    };
    let path = Path::new(name);
    if path.is_absolute() || name.contains(std::path::MAIN_SEPARATOR) {
        return path.to_path_buf();
    }
    for dir in config_theme_dirs() {
        let candidate = dir.join(name);
        if candidate.exists() {
            return candidate;
        }
        if let Some(with_extension) = with_extension.as_deref() {
            let candidate = dir.join(with_extension);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    path.to_path_buf()
}

fn config_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(config_path) = Config::config_path() {
        if let Some(parent) = config_path.parent() {
            dirs.push(parent.join("themes"));
        }
    }
    if let Some(config_dir) = dirs::config_dir() {
        let themes = config_dir.join("oyo").join("themes");
        if !dirs.contains(&themes) {
            dirs.push(themes);
        }
    }
    dirs
}

pub fn list_syntax_themes() -> Vec<String> {
    let mut names = BTreeSet::new();
    for item in EMBEDDED_TMTHEMES {
        names.insert(item.name.to_string());
    }
    for theme_dir in config_theme_dirs() {
        if let Ok(entries) = fs::read_dir(theme_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("tmTheme"))
                {
                    if let Some(stem) = path.file_stem().and_then(|n| n.to_str()) {
                        names.insert(stem.to_string());
                        if let Some(base) = strip_variant_suffix(stem) {
                            names.insert(base);
                        }
                    }
                }
            }
        }
    }
    names.into_iter().collect()
}

fn load_embedded_syntax_theme(name: &str) -> Option<Theme> {
    load_embedded_tmtheme(name).or_else(|| load_two_face_theme(name))
}

fn load_embedded_tmtheme(name: &str) -> Option<Theme> {
    let needle = normalize_theme_key(name);
    for item in EMBEDDED_TMTHEMES {
        if normalize_theme_key(item.name) == needle {
            let mut cursor = Cursor::new(item.data);
            return ThemeSet::load_from_reader(&mut cursor).ok();
        }
    }
    None
}

fn load_two_face_theme(name: &str) -> Option<Theme> {
    let needle = normalize_theme_key(name);
    let embedded = two_face::theme::EmbeddedLazyThemeSet::theme_names()
        .iter()
        .copied()
        .find(|theme| normalize_theme_key(theme.as_name()) == needle)?;
    Some(two_face::theme::extra().get(embedded).clone())
}

fn load_ansi_theme() -> (Theme, TuiColor) {
    let theme = load_two_face_theme("ansi").unwrap_or_default();
    let plain = theme
        .settings
        .foreground
        .map(to_tui)
        .unwrap_or(TuiColor::White);
    (theme, plain)
}

fn strip_theme_backgrounds(theme: &mut Theme) {
    theme.settings.background = None;
    for item in &mut theme.scopes {
        item.style.background = None;
    }
}

fn ensure_foreground(theme: &mut Theme, plain: TuiColor) {
    if theme.settings.foreground.is_none() {
        theme.settings.foreground = Some(to_syntect(plain));
    }
}

fn light_variants(name: &str) -> Vec<String> {
    if name.ends_with(".tmTheme") {
        let path = Path::new(name);
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let base = path.parent().unwrap_or_else(|| Path::new(""));
            return ["-light", "_light", ".light"]
                .iter()
                .map(|suffix| base.join(format!("{stem}{suffix}.tmTheme")))
                .map(|p| p.to_string_lossy().to_string())
                .collect();
        }
        return Vec::new();
    }
    let lower = name.to_ascii_lowercase();
    let mut variants = Vec::new();
    match lower.as_str() {
        "catppuccin" => variants.push("catppuccin-latte".to_string()),
        "rosepine" => variants.push("rosepine-dawn".to_string()),
        "tokyonight" => variants.push("tokyonight-day".to_string()),
        _ => {}
    }
    variants.extend([
        format!("{name}-light"),
        format!("{name}_light"),
        format!("{name}.light"),
    ]);
    variants
}

fn dark_variants(name: &str) -> Vec<String> {
    if name.ends_with(".tmTheme") {
        let path = Path::new(name);
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let base = path.parent().unwrap_or_else(|| Path::new(""));
            return ["-dark", "_dark", ".dark"]
                .iter()
                .map(|suffix| base.join(format!("{stem}{suffix}.tmTheme")))
                .map(|p| p.to_string_lossy().to_string())
                .collect();
        }
        return Vec::new();
    }
    [
        format!("{name}-dark"),
        format!("{name}_dark"),
        format!("{name}.dark"),
    ]
    .to_vec()
}

fn is_explicit_variant(name: &str) -> bool {
    if name.ends_with(".tmTheme") {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.ends_with("-light")
        || lower.ends_with("_light")
        || lower.ends_with(".light")
        || lower.ends_with("-dark")
        || lower.ends_with("_dark")
        || lower.ends_with(".dark")
}

fn strip_variant_suffix(stem: &str) -> Option<String> {
    let lower = stem.to_ascii_lowercase();
    for suffix in ["-light", "_light", ".light", "-dark", "_dark", ".dark"] {
        if lower.ends_with(suffix) {
            return Some(stem[..stem.len().saturating_sub(suffix.len())].to_string());
        }
    }
    None
}

fn normalize_theme_key(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

impl SyntaxCache {
    pub fn new(engine: &SyntaxEngine, old: &str, new: &str, file_name: &str) -> Self {
        let old = engine.highlight(old, file_name);
        let new = engine.highlight(new, file_name);
        Self { old, new }
    }

    pub fn spans(&self, side: SyntaxSide, line_index: usize) -> Option<&[SyntaxSpan]> {
        match side {
            SyntaxSide::Old => self.old.get(line_index).map(|v| v.as_slice()),
            SyntaxSide::New => self.new.get(line_index).map(|v| v.as_slice()),
        }
    }
}

fn syntect_style_to_tui(style: SynStyle) -> Style {
    let mut out = Style::default().fg(to_tui(style.foreground));
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

fn to_syntect(color: TuiColor) -> Color {
    match color {
        TuiColor::Rgb(r, g, b) => Color { r, g, b, a: 0xFF },
        TuiColor::Black => Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0xFF,
        },
        TuiColor::Red => Color {
            r: 205,
            g: 0,
            b: 0,
            a: 0xFF,
        },
        TuiColor::Green => Color {
            r: 0,
            g: 205,
            b: 0,
            a: 0xFF,
        },
        TuiColor::Yellow => Color {
            r: 205,
            g: 205,
            b: 0,
            a: 0xFF,
        },
        TuiColor::Blue => Color {
            r: 0,
            g: 0,
            b: 238,
            a: 0xFF,
        },
        TuiColor::Magenta => Color {
            r: 205,
            g: 0,
            b: 205,
            a: 0xFF,
        },
        TuiColor::Cyan => Color {
            r: 0,
            g: 205,
            b: 205,
            a: 0xFF,
        },
        TuiColor::Gray => Color {
            r: 229,
            g: 229,
            b: 229,
            a: 0xFF,
        },
        TuiColor::DarkGray => Color {
            r: 127,
            g: 127,
            b: 127,
            a: 0xFF,
        },
        TuiColor::LightRed => Color {
            r: 255,
            g: 0,
            b: 0,
            a: 0xFF,
        },
        TuiColor::LightGreen => Color {
            r: 0,
            g: 255,
            b: 0,
            a: 0xFF,
        },
        TuiColor::LightYellow => Color {
            r: 255,
            g: 255,
            b: 0,
            a: 0xFF,
        },
        TuiColor::LightBlue => Color {
            r: 92,
            g: 92,
            b: 255,
            a: 0xFF,
        },
        TuiColor::LightMagenta => Color {
            r: 255,
            g: 0,
            b: 255,
            a: 0xFF,
        },
        TuiColor::LightCyan => Color {
            r: 0,
            g: 255,
            b: 255,
            a: 0xFF,
        },
        TuiColor::White => Color {
            r: 255,
            g: 255,
            b: 255,
            a: 0xFF,
        },
        _ => Color {
            r: 255,
            g: 255,
            b: 255,
            a: 0xFF,
        },
    }
}

fn to_tui(color: Color) -> TuiColor {
    TuiColor::Rgb(color.r, color.g, color.b)
}
