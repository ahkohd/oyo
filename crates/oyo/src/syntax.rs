//! Syntax highlighting helpers (syntect-backed)

use crate::config::Config;
use ratatui::style::{Color as TuiColor, Modifier, Style};
use ratatui::text::Span;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
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

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SyntaxDebugStats {
    pub(crate) requests: usize,
    pub(crate) rendered_hits: usize,
    pub(crate) rendered_misses: usize,
    pub(crate) highlight_lines: usize,
    pub(crate) cached_lines: usize,
    pub(crate) warm_lines: usize,
}

#[derive(Clone, Debug)]
pub struct SyntaxCache {
    old: SyntaxStore,
    new: SyntaxStore,
    epoch: u64,
}

#[derive(Clone, Debug)]
enum SyntaxStore {
    Full(FullSyntaxCache),
    Lazy(Box<LazySyntaxCache>),
}

#[derive(Clone, Debug)]
struct FullSyntaxCache {
    lines: Vec<Vec<SyntaxSpan>>,
    rendered: Vec<Option<Vec<Span<'static>>>>,
}

#[derive(Clone, Debug)]
struct LazySyntaxCache {
    syntax_set: std::sync::Arc<SyntaxSet>,
    theme: std::sync::Arc<Theme>,
    plain: TuiColor,
    lines: Vec<String>,
    spans: Vec<Option<Vec<SyntaxSpan>>>,
    rendered: Vec<Option<Vec<Span<'static>>>>,
    checkpoints: Vec<Option<(syntect::highlighting::HighlightState, ParseState)>>,
    chunk_states: Vec<Option<ChunkProgress>>,
    warm_progress: Option<WarmProgress>,
    stride: usize,
}

const MAX_LAZY_SYNTAX_BYTES: usize = 512 * 1024;
const SYNTAX_CHECKPOINT_STRIDE: usize = 200;
#[cfg(test)]
const MAX_SYNC_SYNTAX_LINES: usize = 200;
#[cfg(not(test))]
const MAX_SYNC_SYNTAX_LINES: usize = 5_000;

#[derive(Clone, Debug)]
struct ChunkProgress {
    next_line: usize,
    state: (syntect::highlighting::HighlightState, ParseState),
}

#[derive(Clone, Debug)]
struct WarmProgress {
    next_line: usize,
    next_chunk: usize,
    target_chunk: usize,
    state: (syntect::highlighting::HighlightState, ParseState),
}

struct SyntaxDebugCounters {
    requests: AtomicUsize,
    rendered_hits: AtomicUsize,
    rendered_misses: AtomicUsize,
    highlight_lines: AtomicUsize,
    cached_lines: AtomicUsize,
    warm_lines: AtomicUsize,
}

impl Default for SyntaxDebugCounters {
    fn default() -> Self {
        Self {
            requests: AtomicUsize::new(0),
            rendered_hits: AtomicUsize::new(0),
            rendered_misses: AtomicUsize::new(0),
            highlight_lines: AtomicUsize::new(0),
            cached_lines: AtomicUsize::new(0),
            warm_lines: AtomicUsize::new(0),
        }
    }
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
    syntax_set: std::sync::Arc<SyntaxSet>,
    theme: std::sync::Arc<Theme>,
    plain: TuiColor,
}

impl SyntaxEngine {
    pub fn new(syntax_theme: &str, light_mode: bool) -> Self {
        let syntax_set = std::sync::Arc::new(two_face::syntax::extra_newlines());
        let (syntax_theme, plain) = resolve_syntax_theme(syntax_theme, light_mode);
        Self {
            syntax_set,
            theme: std::sync::Arc::new(syntax_theme),
            plain,
        }
    }

    pub fn highlight(&self, content: &str, file_name: &str) -> Vec<Vec<SyntaxSpan>> {
        let syntax = self.syntax_for_file(file_name);
        let mut highlighter = HighlightLines::new(syntax, self.theme.as_ref());
        let mut out = Vec::new();

        for line in LinesWithEndings::from(content) {
            let ranges = highlighter
                .highlight_line(line, &self.syntax_set)
                .unwrap_or_default();
            out.push(ranges_to_spans(ranges, self.plain));
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

    pub fn syntax_ref(&self, file_name: &str) -> SyntaxReference {
        self.syntax_for_file(file_name).clone()
    }

    pub fn syntax_set(&self) -> std::sync::Arc<SyntaxSet> {
        self.syntax_set.clone()
    }

    pub fn theme(&self) -> &Theme {
        self.theme.as_ref()
    }

    pub fn theme_arc(&self) -> std::sync::Arc<Theme> {
        self.theme.clone()
    }

    pub fn plain(&self) -> TuiColor {
        self.plain
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
    pub fn new(
        engine: &SyntaxEngine,
        old: &str,
        new: &str,
        file_name: &str,
        force_lazy: bool,
    ) -> Self {
        let max_len = old.len().max(new.len());
        let lazy = force_lazy || max_len > MAX_LAZY_SYNTAX_BYTES;
        if lazy {
            let old = LazySyntaxCache::new(engine, old, file_name);
            let new = LazySyntaxCache::new(engine, new, file_name);
            Self {
                old: SyntaxStore::Lazy(Box::new(old)),
                new: SyntaxStore::Lazy(Box::new(new)),
                epoch: 0,
            }
        } else {
            let old = engine.highlight(old, file_name);
            let new = engine.highlight(new, file_name);
            Self {
                old: SyntaxStore::Full(FullSyntaxCache {
                    rendered: vec![None; old.len()],
                    lines: old,
                }),
                new: SyntaxStore::Full(FullSyntaxCache {
                    rendered: vec![None; new.len()],
                    lines: new,
                }),
                epoch: 0,
            }
        }
    }

    pub fn rendered_spans(
        &mut self,
        side: SyntaxSide,
        line_index: usize,
    ) -> Option<Vec<Span<'static>>> {
        syntax_debug_request();
        match side {
            SyntaxSide::Old => rendered_spans_for_store(&mut self.old, line_index),
            SyntaxSide::New => rendered_spans_for_store(&mut self.new, line_index),
        }
    }

    pub(crate) fn warm_checkpoints(&mut self, max_lines: usize) -> usize {
        if max_lines == 0 {
            return 0;
        }
        let new_pending_before = warm_pending_for_store(&self.new);
        let old_pending_before = warm_pending_for_store(&self.old);
        let mut remaining = max_lines;
        if new_pending_before || old_pending_before {
            if new_pending_before {
                remaining =
                    remaining.saturating_sub(warm_checkpoints_for_store(&mut self.new, remaining));
            }
            if old_pending_before {
                remaining =
                    remaining.saturating_sub(warm_checkpoints_for_store(&mut self.old, remaining));
            }
        } else {
            remaining =
                remaining.saturating_sub(warm_checkpoints_for_store(&mut self.new, remaining));
            remaining =
                remaining.saturating_sub(warm_checkpoints_for_store(&mut self.old, remaining));
        }
        if new_pending_before && !warm_pending_for_store(&self.new) {
            self.bump_epoch();
        }
        if old_pending_before && !warm_pending_for_store(&self.old) {
            self.bump_epoch();
        }
        max_lines - remaining
    }

    pub(crate) fn warm_pending(&self) -> bool {
        warm_pending_for_store(&self.old) || warm_pending_for_store(&self.new)
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    fn bump_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }

    pub(crate) fn set_warmup_targets(
        &mut self,
        old: Option<crate::app::WarmupRange>,
        new: Option<crate::app::WarmupRange>,
    ) {
        set_warmup_target_for_store(&mut self.old, old);
        set_warmup_target_for_store(&mut self.new, new);
    }
}

fn rendered_spans_for_store(
    store: &mut SyntaxStore,
    line_index: usize,
) -> Option<Vec<Span<'static>>> {
    match store {
        SyntaxStore::Full(cache) => {
            if line_index >= cache.lines.len() {
                return None;
            }
            if let Some(rendered) = cache.rendered.get(line_index).and_then(|v| v.as_ref()) {
                syntax_debug_rendered_hit();
                return Some(rendered.clone());
            }
            syntax_debug_rendered_miss();
            let spans = cache.lines.get(line_index)?;
            let rendered = syntax_spans_to_ratatui(spans);
            cache.rendered[line_index] = Some(rendered.clone());
            Some(rendered)
        }
        SyntaxStore::Lazy(cache) => cache.rendered_spans(line_index),
    }
}

fn warm_checkpoints_for_store(store: &mut SyntaxStore, max_lines: usize) -> usize {
    match store {
        SyntaxStore::Full(_) => 0,
        SyntaxStore::Lazy(cache) => cache.warm_checkpoints(max_lines),
    }
}

fn warm_pending_for_store(store: &SyntaxStore) -> bool {
    match store {
        SyntaxStore::Full(_) => false,
        SyntaxStore::Lazy(cache) => cache.warm_pending(),
    }
}

fn set_warmup_target_for_store(store: &mut SyntaxStore, range: Option<crate::app::WarmupRange>) {
    match store {
        SyntaxStore::Full(_) => {}
        SyntaxStore::Lazy(cache) => match range {
            Some(range) => cache.set_warmup_target(range.start, range.end),
            None => cache.clear_warmup_target(),
        },
    }
}

impl LazySyntaxCache {
    fn new(engine: &SyntaxEngine, content: &str, file_name: &str) -> Self {
        let syntax = engine.syntax_ref(file_name);
        let lines: Vec<String> = LinesWithEndings::from(content)
            .map(|line| line.to_string())
            .collect();
        let mut lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };

        if lines.len() == 1 && lines[0].is_empty() {
            lines[0].push_str("");
        }

        let stride = SYNTAX_CHECKPOINT_STRIDE.max(1);
        let checkpoint_len = lines.len().saturating_sub(1) / stride + 1;
        let mut checkpoints = vec![None; checkpoint_len];
        let highlighter = HighlightLines::new(&syntax, engine.theme());
        checkpoints[0] = Some(highlighter.state());

        Self {
            syntax_set: engine.syntax_set(),
            theme: engine.theme_arc(),
            plain: engine.plain(),
            spans: vec![None; lines.len()],
            rendered: vec![None; lines.len()],
            lines,
            checkpoints,
            chunk_states: vec![None; checkpoint_len],
            warm_progress: None,
            stride,
        }
    }

    fn spans(&mut self, line_index: usize) -> Option<&[SyntaxSpan]> {
        if line_index >= self.lines.len() {
            return None;
        }
        let needs_fill = self
            .spans
            .get(line_index)
            .map(|spans| spans.is_none())
            .unwrap_or(true);
        if !needs_fill {
            return self.spans.get(line_index).and_then(|s| s.as_deref());
        }

        let chunk = line_index / self.stride;
        let state = self.ensure_checkpoint(chunk)?;
        let chunk_start = chunk * self.stride;
        let chunk_end = ((chunk + 1) * self.stride).min(self.lines.len());
        let progress = self
            .chunk_states
            .get_mut(chunk)
            .and_then(|slot| slot.take());

        let (mut next_line, mut highlighter) = if let Some(progress) = progress {
            (
                progress.next_line,
                HighlightLines::from_state(self.theme.as_ref(), progress.state.0, progress.state.1),
            )
        } else {
            (
                chunk_start,
                HighlightLines::from_state(self.theme.as_ref(), state.0, state.1),
            )
        };

        if next_line < chunk_start {
            next_line = chunk_start;
        }

        let lines = &self.lines;
        let spans = &mut self.spans;
        let to_process = line_index.saturating_sub(next_line).saturating_add(1);
        if to_process > 0 {
            syntax_debug_highlight_lines(to_process);
            syntax_debug_cached_lines(to_process);
        }
        for idx in next_line..=line_index {
            let line = &lines[idx];
            let ranges = highlighter
                .highlight_line(line, &self.syntax_set)
                .unwrap_or_default();
            spans[idx] = Some(ranges_to_spans(ranges, self.plain));
        }

        let next_line = (line_index + 1).min(chunk_end);
        let state = highlighter.state();
        if next_line == chunk_end && self.checkpoints.len() > chunk + 1 {
            self.checkpoints[chunk + 1] = Some(state);
        } else if let Some(slot) = self.chunk_states.get_mut(chunk) {
            *slot = Some(ChunkProgress { next_line, state });
        }

        self.spans.get(line_index).and_then(|s| s.as_deref())
    }

    fn rendered_spans(&mut self, line_index: usize) -> Option<Vec<Span<'static>>> {
        if line_index >= self.lines.len() {
            return None;
        }
        if let Some(rendered) = self.rendered.get(line_index).and_then(|v| v.as_ref()) {
            syntax_debug_rendered_hit();
            return Some(rendered.clone());
        }
        syntax_debug_rendered_miss();
        let spans = self.spans(line_index)?;
        let rendered = syntax_spans_to_ratatui(spans);
        self.rendered[line_index] = Some(rendered.clone());
        Some(rendered)
    }

    fn warm_checkpoints(&mut self, max_lines: usize) -> usize {
        if max_lines == 0 || self.checkpoints.len() <= 1 {
            return 0;
        }
        let mut progress = self.warm_progress.take();
        if progress.is_none() {
            let Some(state) = self.checkpoints.first().and_then(|c| c.clone()) else {
                return 0;
            };
            progress = Some(WarmProgress {
                next_line: 0,
                next_chunk: 1,
                target_chunk: self.checkpoints.len().saturating_sub(1),
                state,
            });
        }
        let mut progress = progress.expect("warm progress");
        let target_chunk = progress
            .target_chunk
            .min(self.checkpoints.len().saturating_sub(1));
        if progress.next_chunk > target_chunk {
            self.warm_progress = None;
            return 0;
        }

        while progress.next_chunk <= target_chunk {
            let Some(state) = self
                .checkpoints
                .get(progress.next_chunk)
                .and_then(|c| c.clone())
            else {
                break;
            };
            progress.state = state;
            progress.next_line = progress.next_chunk.saturating_mul(self.stride);
            progress.next_chunk += 1;
        }
        if progress.next_chunk > target_chunk {
            self.warm_progress = None;
            return 0;
        }

        let mut highlighter =
            HighlightLines::from_state(self.theme.as_ref(), progress.state.0, progress.state.1);
        let mut processed = 0usize;
        let line_len = self.lines.len();

        while processed < max_lines && progress.next_chunk <= target_chunk {
            let chunk_end = (progress.next_chunk * self.stride).min(line_len);
            if progress.next_line >= chunk_end {
                let (highlight_state, parse_state) = highlighter.state();
                if let Some(slot) = self.checkpoints.get_mut(progress.next_chunk) {
                    *slot = Some((highlight_state.clone(), parse_state.clone()));
                }
                highlighter =
                    HighlightLines::from_state(self.theme.as_ref(), highlight_state, parse_state);
                progress.next_chunk += 1;
                progress.next_line = chunk_end;
                continue;
            }

            let line = &self.lines[progress.next_line];
            let _ = highlighter.highlight_line(line, &self.syntax_set);
            progress.next_line = progress.next_line.saturating_add(1);
            processed += 1;
        }

        progress.state = highlighter.state();
        if progress.next_chunk > target_chunk {
            self.warm_progress = None;
        } else {
            self.warm_progress = Some(progress);
        }
        syntax_debug_warm_lines(processed);
        processed
    }

    fn warm_pending(&self) -> bool {
        self.warm_progress.is_some()
    }

    fn clear_warmup_target(&mut self) {
        self.warm_progress = None;
    }

    fn set_warmup_target(&mut self, start: usize, end: usize) {
        if self.checkpoints.is_empty() || self.lines.is_empty() {
            return;
        }
        let last_line = self.lines.len().saturating_sub(1);
        let mut start = start.min(last_line);
        let mut end = end.min(last_line);
        if start > end {
            std::mem::swap(&mut start, &mut end);
        }
        let target_chunk = end / self.stride;
        if target_chunk >= self.checkpoints.len() {
            return;
        }
        if self.checkpoints[target_chunk].is_some() {
            self.warm_progress = None;
            return;
        }
        let mut start_chunk = start / self.stride;
        while start_chunk > 0 && self.checkpoints[start_chunk].is_none() {
            start_chunk = start_chunk.saturating_sub(1);
        }
        let Some(state) = self.checkpoints.get(start_chunk).and_then(|c| c.clone()) else {
            return;
        };
        if let Some(existing) = self.warm_progress.as_ref() {
            if existing.target_chunk == target_chunk {
                return;
            }
        }
        self.warm_progress = Some(WarmProgress {
            next_line: start_chunk * self.stride,
            next_chunk: start_chunk.saturating_add(1),
            target_chunk,
            state,
        });
    }

    fn ensure_checkpoint(
        &mut self,
        chunk: usize,
    ) -> Option<(syntect::highlighting::HighlightState, ParseState)> {
        if let Some(state) = self.checkpoints.get(chunk).and_then(|c| c.clone()) {
            return Some(state);
        }
        if self.checkpoints.is_empty() {
            return None;
        }

        let mut start_chunk = chunk;
        while start_chunk > 0
            && self
                .checkpoints
                .get(start_chunk)
                .and_then(|c| c.as_ref())
                .is_none()
        {
            start_chunk = start_chunk.saturating_sub(1);
        }
        let state = self.checkpoints.get(start_chunk).and_then(|c| c.clone())?;
        let mut line_idx = start_chunk * self.stride;
        let chunk_start = (chunk * self.stride).min(self.lines.len());
        let lines_to_process = chunk_start.saturating_sub(line_idx);
        if lines_to_process > MAX_SYNC_SYNTAX_LINES {
            self.warm_progress = Some(WarmProgress {
                next_line: line_idx,
                next_chunk: start_chunk.saturating_add(1),
                target_chunk: chunk,
                state,
            });
            return None;
        }
        let mut highlighter = HighlightLines::from_state(self.theme.as_ref(), state.0, state.1);

        for next_chunk in (start_chunk + 1)..=chunk {
            let chunk_end = (next_chunk * self.stride).min(self.lines.len());
            let checkpoint_lines = chunk_end.saturating_sub(line_idx);
            if checkpoint_lines > 0 {
                syntax_debug_highlight_lines(checkpoint_lines);
            }
            for idx in line_idx..chunk_end {
                let line = &self.lines[idx];
                let _ = highlighter.highlight_line(line, &self.syntax_set);
            }
            let state = highlighter.state();
            if let Some(slot) = self.checkpoints.get_mut(next_chunk) {
                *slot = Some(state.clone());
            }
            if next_chunk == chunk {
                return Some(state);
            }
            highlighter = HighlightLines::from_state(self.theme.as_ref(), state.0, state.1);
            line_idx = chunk_end;
        }

        self.checkpoints.get(chunk).and_then(|c| c.clone())
    }
}

fn ranges_to_spans(ranges: Vec<(SynStyle, &str)>, plain: TuiColor) -> Vec<SyntaxSpan> {
    let mut spans = Vec::new();
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
            style: Style::default().fg(plain),
        });
    }
    spans
}

fn syntax_spans_to_ratatui(spans: &[SyntaxSpan]) -> Vec<Span<'static>> {
    spans
        .iter()
        .map(|span| Span::styled(span.text.clone(), span.style))
        .collect()
}

fn syntax_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OYO_DEBUG_VIEW").is_some())
}

fn syntax_debug_counters() -> &'static SyntaxDebugCounters {
    static COUNTERS: OnceLock<SyntaxDebugCounters> = OnceLock::new();
    COUNTERS.get_or_init(SyntaxDebugCounters::default)
}

pub(crate) fn syntax_debug_reset() {
    if !syntax_debug_enabled() {
        return;
    }
    let counters = syntax_debug_counters();
    counters.requests.store(0, Ordering::Relaxed);
    counters.rendered_hits.store(0, Ordering::Relaxed);
    counters.rendered_misses.store(0, Ordering::Relaxed);
    counters.highlight_lines.store(0, Ordering::Relaxed);
    counters.cached_lines.store(0, Ordering::Relaxed);
}

pub(crate) fn syntax_debug_stats() -> Option<SyntaxDebugStats> {
    if !syntax_debug_enabled() {
        return None;
    }
    let counters = syntax_debug_counters();
    Some(SyntaxDebugStats {
        requests: counters.requests.load(Ordering::Relaxed),
        rendered_hits: counters.rendered_hits.load(Ordering::Relaxed),
        rendered_misses: counters.rendered_misses.load(Ordering::Relaxed),
        highlight_lines: counters.highlight_lines.load(Ordering::Relaxed),
        cached_lines: counters.cached_lines.load(Ordering::Relaxed),
        warm_lines: counters.warm_lines.swap(0, Ordering::Relaxed),
    })
}

fn syntax_debug_request() {
    if !syntax_debug_enabled() {
        return;
    }
    let counters = syntax_debug_counters();
    counters.requests.fetch_add(1, Ordering::Relaxed);
}

fn syntax_debug_rendered_hit() {
    if !syntax_debug_enabled() {
        return;
    }
    let counters = syntax_debug_counters();
    counters.rendered_hits.fetch_add(1, Ordering::Relaxed);
}

fn syntax_debug_rendered_miss() {
    if !syntax_debug_enabled() {
        return;
    }
    let counters = syntax_debug_counters();
    counters.rendered_misses.fetch_add(1, Ordering::Relaxed);
}

fn syntax_debug_highlight_lines(count: usize) {
    if count == 0 || !syntax_debug_enabled() {
        return;
    }
    let counters = syntax_debug_counters();
    counters.highlight_lines.fetch_add(count, Ordering::Relaxed);
}

fn syntax_debug_cached_lines(count: usize) {
    if count == 0 || !syntax_debug_enabled() {
        return;
    }
    let counters = syntax_debug_counters();
    counters.cached_lines.fetch_add(count, Ordering::Relaxed);
}

fn syntax_debug_warm_lines(count: usize) {
    if count == 0 || !syntax_debug_enabled() {
        return;
    }
    let counters = syntax_debug_counters();
    counters.warm_lines.fetch_add(count, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazy_cache_only_fills_requested_lines() {
        let engine = SyntaxEngine::new("aura", false);
        let content = "alpha\nbeta\ngamma\n";
        let mut cache = LazySyntaxCache::new(&engine, content, "sample.rs");

        assert!(cache.spans[0].is_none());
        let _ = cache.spans(0).expect("expected spans for line 0");
        assert!(cache.spans[0].is_some());
        assert!(cache.spans[1].is_none());
    }

    #[test]
    fn lazy_cache_advances_within_chunk() {
        let engine = SyntaxEngine::new("aura", false);
        let content = "one\ntwo\nthree\nfour\nfive\n";
        let mut cache = LazySyntaxCache::new(&engine, content, "sample.rs");

        let _ = cache.spans(0).expect("expected spans for line 0");
        let _ = cache.spans(3).expect("expected spans for line 3");

        assert!(cache.spans[1].is_some());
        assert!(cache.spans[2].is_some());
        assert!(cache.spans[3].is_some());
    }

    #[test]
    fn lazy_cache_checkpointing_skips_intermediate_spans() {
        let engine = SyntaxEngine::new("aura", false);
        let mut content = String::new();
        for idx in 0..500 {
            content.push_str(&format!("line {idx}\n"));
        }
        let mut cache = LazySyntaxCache::new(&engine, &content, "sample.rs");

        let _ = cache.warm_checkpoints(1_000);
        let _ = cache.spans(450).expect("expected spans for line 450");

        assert!(cache.spans[0].is_none());
        assert!(cache.spans[199].is_none());
        assert!(cache.spans[399].is_none());
        assert!(cache.spans[400].is_some());
    }

    #[test]
    fn lazy_cache_rendered_spans_cache_single_line() {
        let engine = SyntaxEngine::new("aura", false);
        let content = "alpha\nbeta\ngamma\n";
        let mut cache = LazySyntaxCache::new(&engine, content, "sample.rs");

        assert!(cache.rendered[0].is_none());
        let spans = cache.rendered_spans(0).expect("expected rendered spans");
        assert!(!spans.is_empty());
        assert!(cache.rendered[0].is_some());
        assert!(cache.rendered[1].is_none());
    }

    #[test]
    fn lazy_cache_warmup_fills_checkpoints_without_spans() {
        let engine = SyntaxEngine::new("aura", false);
        let mut content = String::new();
        for idx in 0..450 {
            content.push_str(&format!("line {idx}\n"));
        }
        let mut cache = LazySyntaxCache::new(&engine, &content, "sample.rs");

        assert!(cache.checkpoints.get(1).and_then(|c| c.as_ref()).is_none());
        let processed = cache.warm_checkpoints(300);
        assert!(processed > 0);
        assert!(cache.checkpoints.get(1).and_then(|c| c.as_ref()).is_some());
        assert!(cache.spans[0].is_none());
        assert!(cache.rendered[0].is_none());
    }

    #[test]
    fn lazy_cache_warmup_progresses_across_calls() {
        let engine = SyntaxEngine::new("aura", false);
        let mut content = String::new();
        for idx in 0..260 {
            content.push_str(&format!("line {idx}\n"));
        }
        let mut cache = LazySyntaxCache::new(&engine, &content, "sample.rs");

        let processed = cache.warm_checkpoints(50);
        assert!(processed > 0);
        assert!(cache.checkpoints.get(1).and_then(|c| c.as_ref()).is_none());

        let _ = cache.warm_checkpoints(200);
        assert!(cache.checkpoints.get(1).and_then(|c| c.as_ref()).is_some());
    }

    #[test]
    fn lazy_cache_defers_large_checkpoint_and_warms() {
        let engine = SyntaxEngine::new("aura", false);
        let mut content = String::new();
        for idx in 0..600 {
            content.push_str(&format!("line {idx}\n"));
        }
        let mut cache = LazySyntaxCache::new(&engine, &content, "sample.rs");

        assert!(cache.spans(450).is_none());
        for _ in 0..20 {
            let _ = cache.warm_checkpoints(200);
        }
        assert!(cache.spans(450).is_some());
    }

    #[test]
    fn lazy_cache_warmup_target_prioritizes_range() {
        let engine = SyntaxEngine::new("aura", false);
        let mut content = String::new();
        for idx in 0..800 {
            content.push_str(&format!("line {idx}\n"));
        }
        let mut cache = LazySyntaxCache::new(&engine, &content, "sample.rs");

        cache.set_warmup_target(600, 650);
        for _ in 0..20 {
            let _ = cache.warm_checkpoints(200);
        }

        assert!(cache.spans(650).is_some());
    }

    #[test]
    fn syntax_cache_warmup_prefers_pending_store() {
        let engine = SyntaxEngine::new("aura", false);
        let mut old_content = String::new();
        let mut new_content = String::new();
        for idx in 0..800 {
            old_content.push_str(&format!("old {idx}\n"));
            new_content.push_str(&format!("new {idx}\n"));
        }
        let mut cache = SyntaxCache::new(&engine, &old_content, &new_content, "sample.rs", true);
        cache.set_warmup_targets(
            None,
            Some(crate::app::WarmupRange {
                start: 600,
                end: 650,
            }),
        );

        match (&cache.old, &cache.new) {
            (SyntaxStore::Lazy(old), SyntaxStore::Lazy(new)) => {
                assert!(old.warm_progress.is_none());
                assert!(new.warm_progress.is_some());
            }
            _ => panic!("expected lazy caches"),
        }

        let processed = cache.warm_checkpoints(50);
        assert!(processed > 0);

        match (&cache.old, &cache.new) {
            (SyntaxStore::Lazy(old), SyntaxStore::Lazy(new)) => {
                assert!(
                    old.warm_progress.is_none(),
                    "old store should not start warming when only new is pending"
                );
                assert!(new.warm_progress.is_some());
            }
            _ => panic!("expected lazy caches"),
        }
    }

    #[test]
    fn syntax_cache_epoch_bumps_when_warmup_completes() {
        let engine = SyntaxEngine::new("aura", false);
        let mut content = String::new();
        for idx in 0..800 {
            content.push_str(&format!("line {idx}\n"));
        }
        let mut cache = SyntaxCache::new(&engine, &content, &content, "sample.rs", true);
        cache.set_warmup_targets(
            None,
            Some(crate::app::WarmupRange {
                start: 600,
                end: 650,
            }),
        );

        assert!(cache.warm_pending());
        let epoch_before = cache.epoch();

        for _ in 0..20 {
            cache.warm_checkpoints(500);
            if !cache.warm_pending() {
                break;
            }
        }

        assert!(!cache.warm_pending());
        assert!(
            cache.epoch() > epoch_before,
            "epoch should advance when warmup completes"
        );
    }
}
