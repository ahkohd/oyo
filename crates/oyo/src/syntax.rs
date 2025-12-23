//! Syntax highlighting helpers (syntect-backed)

use crate::config::ResolvedTheme;
use ratatui::style::{Color as TuiColor, Modifier, Style};
use std::collections::BTreeSet;
use std::str::FromStr;
use syntect::{
    easy::HighlightLines,
    highlighting::{
        Color, FontStyle, ScopeSelectors, Style as SynStyle, StyleModifier, Theme, ThemeItem,
        ThemeSettings,
    },
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
    let mut t = Theme {
        settings: ThemeSettings {
            foreground: Some(to_syntect(theme.syntax_plain)),
            ..ThemeSettings::default()
        },
        ..Theme::default()
    };

    let scopes = vec![
        theme_item("comment", theme.syntax_comment),
        theme_item(
            "punctuation.definition.comment, punctuation.definition.comment.begin, punctuation.definition.comment.end",
            theme.syntax_comment,
        ),
        theme_item("string", theme.syntax_string),
        theme_item(
            "keyword, keyword.declaration, keyword.control, keyword.other, storage.modifier",
            theme.syntax_keyword,
        ),
        theme_item("constant.numeric", theme.syntax_number),
        theme_item(
            "meta.annotation, meta.attribute, entity.other.attribute-name, variable.annotation",
            theme.syntax_attribute,
        ),
        theme_item(
            "meta.struct, meta.enum, meta.trait, meta.type, meta.generic",
            theme.syntax_type,
        ),
        theme_item(
            "storage.type, entity.name.type, entity.name.type.struct, entity.name.type.enum, entity.name.type.trait, entity.name.type.alias, entity.name.type.interface, entity.name.namespace, support.type, support.namespace",
            theme.syntax_type,
        ),
        theme_item(
            "entity.name.function, entity.name.function.method, support.function",
            theme.syntax_function,
        ),
        theme_item(
            "entity.name.function.macro, support.function.macro",
            theme.syntax_macro,
        ),
        theme_item(
            "variable, variable.parameter, variable.other, variable.other.member, variable.other.constant, variable.language",
            theme.syntax_variable,
        ),
        theme_item(
            "constant, constant.language, constant.character, constant.other",
            theme.syntax_constant,
        ),
        theme_item(
            "support.type.builtin, support.constant.builtin, support.function.builtin",
            theme.syntax_builtin,
        ),
        theme_item(
            "keyword.operator, keyword.operator.word, keyword.operator.symbol, operator",
            theme.syntax_operator,
        ),
        theme_item("punctuation", theme.syntax_punctuation),
    ];

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
