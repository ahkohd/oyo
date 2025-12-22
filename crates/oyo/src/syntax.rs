//! Syntax highlighting helpers (syntect-backed)

use crate::config::ResolvedTheme;
use ratatui::style::{Color as TuiColor, Modifier, Style};
use syntect::{
    easy::HighlightLines,
    highlighting::{
        Color, FontStyle, Style as SynStyle, StyleModifier, Theme, ThemeItem, ThemeSettings,
        ScopeSelectors,
    },
    parsing::{SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};
use std::str::FromStr;

#[derive(Clone, Copy, Debug)]
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

pub struct SyntaxEngine {
    syntax_set: SyntaxSet,
    theme: Theme,
    plain: TuiColor,
}

impl SyntaxEngine {
    pub fn new(theme: &ResolvedTheme) -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax_theme = build_theme(theme);
        Self {
            syntax_set,
            theme: syntax_theme,
            plain: theme.syntax_plain,
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

    fn syntax_for_file(&self, file_name: &str) -> &SyntaxReference {
        self.syntax_set
            .find_syntax_for_file(file_name)
            .ok()
            .flatten()
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
    }
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

fn build_theme(theme: &ResolvedTheme) -> Theme {
    let mut t = Theme::default();
    t.settings = ThemeSettings {
        foreground: Some(to_syntect(theme.syntax_plain)),
        ..ThemeSettings::default()
    };

    let mut scopes = Vec::new();
    scopes.push(theme_item("comment", theme.syntax_comment));
    scopes.push(theme_item(
        "punctuation.definition.comment, punctuation.definition.comment.begin, punctuation.definition.comment.end",
        theme.syntax_comment,
    ));
    scopes.push(theme_item("string", theme.syntax_string));
    scopes.push(theme_item("keyword", theme.syntax_keyword));
    scopes.push(theme_item("constant.numeric", theme.syntax_number));
    scopes.push(theme_item("storage.type, entity.name.type", theme.syntax_type));
    scopes.push(theme_item("entity.name.function", theme.syntax_function));
    scopes.push(theme_item("variable, variable.parameter, variable.other", theme.syntax_variable));
    scopes.push(theme_item("constant", theme.syntax_constant));
    scopes.push(theme_item("keyword.operator", theme.syntax_operator));
    scopes.push(theme_item("punctuation", theme.syntax_punctuation));

    t.scopes = scopes;
    t
}

fn theme_item(selector: &str, color: TuiColor) -> ThemeItem {
    ThemeItem {
        scope: ScopeSelectors::from_str(selector)
            .unwrap_or_else(|_| ScopeSelectors::from_str("text").unwrap()),
        style: StyleModifier {
            foreground: Some(to_syntect(color)),
            background: None,
            font_style: None,
        },
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
        _ => Color {
            r: 0xFF,
            g: 0xFF,
            b: 0xFF,
            a: 0xFF,
        },
    }
}

fn to_tui(color: Color) -> TuiColor {
    TuiColor::Rgb(color.r, color.g, color.b)
}
