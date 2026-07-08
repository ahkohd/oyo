//! Markdown rendering shared by previews and review comments.

use crate::{color, config::ResolvedTheme};
use image::GenericImageView;
use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use unicode_width::UnicodeWidthStr;

fn text_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(crate) struct MarkdownChangeBars {
    pub(crate) marker: String,
    pub(crate) marker_width: usize,
    pub(crate) styles: HashMap<usize, Style>,
}

impl MarkdownChangeBars {
    pub(crate) fn gutter_width(&self) -> usize {
        self.marker_width + 1
    }
}

/// Colors and derived styles used while rendering a Markdown preview.
struct MarkdownStyles {
    text: Style,
    muted: Style,
    code: Style,
    link: Style,
    rule: Style,
    list_marker: Style,
    check_done: Style,
    check_todo: Style,
    table_border: Style,
    table_head: Style,
    code_lang: Style,
    /// Per-level heading styles (index 0 = H1 .. index 5 = H6).
    headings: [Style; 6],
    /// Per-level heading marker glyphs (shrinking blocks).
    heading_marks: [&'static str; 6],
    /// Per-level heading band, tinted with the heading's own color
    /// (None on transparent/ANSI themes where blending isn't possible).
    heading_bands: [Option<Color>; 6],
    /// Depth-cycled colors for list bullet markers.
    list_colors: [Color; 4],
    /// Background chip behind inline code / highlights (None when transparent).
    inline_bg: Option<Color>,
    /// Panel background behind fenced code blocks (None on transparent themes).
    code_bg: Option<Color>,
}

impl MarkdownStyles {
    fn from_theme(theme: &ResolvedTheme) -> Self {
        let bold = Modifier::BOLD;
        let heading_colors = [
            theme.accent,
            theme.info,
            theme.success,
            theme.warning,
            theme.primary,
            theme.text_muted,
        ];
        let headings =
            std::array::from_fn(|i| Style::default().fg(heading_colors[i]).add_modifier(bold));

        // markview draws bands/chips/panels with explicit background colors, so
        // they show even on transparent themes. Derive a surface to blend from:
        // the real page background when opaque, otherwise a mode-appropriate
        // neutral inferred from the text color's luminance (light text = dark
        // theme). Blending returns None for ANSI/named colors, which degrades
        // gracefully to no background.
        let dark_mode = color::relative_luminance(theme.text)
            .map(|l| l > 0.5)
            .unwrap_or(true);
        let surface = theme
            .background
            .or(theme.background_panel)
            .or(theme.background_element)
            .unwrap_or(if dark_mode {
                Color::Rgb(0x1b, 0x1d, 0x22)
            } else {
                Color::Rgb(0xe8, 0xe9, 0xec)
            });
        let wash = |fg: Color, alpha: f32| color::blend_colors(surface, fg, alpha);

        // Per-level heading bands, tinted with each heading's own color.
        let heading_bands = std::array::from_fn(|i| wash(heading_colors[i], 0.18));
        // Inline chip reads clearly; the code panel is a subtler lift.
        let inline_bg = wash(theme.text, 0.16);
        let code_bg = wash(theme.text, 0.09);
        MarkdownStyles {
            text: Style::default().fg(theme.text),
            muted: Style::default().fg(theme.text_muted),
            code: Style::default().fg(theme.warning).bg_opt(inline_bg),
            link: Style::default()
                .fg(theme.info)
                .add_modifier(Modifier::UNDERLINED),
            rule: Style::default().fg(theme.border_subtle),
            list_marker: Style::default().fg(theme.accent).add_modifier(bold),
            check_done: Style::default().fg(theme.success).add_modifier(bold),
            check_todo: Style::default().fg(theme.text_muted),
            table_border: Style::default().fg(theme.border_subtle),
            table_head: Style::default().fg(theme.accent).add_modifier(bold),
            code_lang: Style::default()
                .fg(theme.accent)
                .add_modifier(bold)
                .bg_opt(code_bg),
            headings,
            heading_marks: ["█ ", "▊ ", "▌ ", "▎ ", "▏ ", "· "],
            heading_bands,
            list_colors: [theme.accent, theme.success, theme.info, theme.warning],
            inline_bg,
            code_bg,
        }
    }
}

/// A GitHub-style callout (`> [!NOTE]`) rendered as a colored, titled quote.
struct Callout {
    icon: &'static str,
    label: &'static str,
    color: Color,
}

impl Callout {
    fn from_kind(kind: BlockQuoteKind, theme: &ResolvedTheme) -> Self {
        match kind {
            BlockQuoteKind::Note => Callout {
                icon: "ℹ",
                label: "Note",
                color: theme.info,
            },
            BlockQuoteKind::Tip => Callout {
                icon: "✎",
                label: "Tip",
                color: theme.success,
            },
            BlockQuoteKind::Important => Callout {
                icon: "★",
                label: "Important",
                color: theme.primary,
            },
            BlockQuoteKind::Warning => Callout {
                icon: "▲",
                label: "Warning",
                color: theme.warning,
            },
            BlockQuoteKind::Caution => Callout {
                icon: "✖",
                label: "Caution",
                color: theme.error,
            },
        }
    }
}

/// One active block-quote level; the border is repeated on every wrapped line.
struct QuoteFrame {
    border: Style,
}

#[derive(Default)]
struct MarkdownList {
    next: Option<u64>,
    marker_width: usize,
}

/// A table cell holds styled spans, so inline code, emphasis, and links keep
/// their colors instead of being flattened to plain text.
type TableCell = Vec<Span<'static>>;

#[derive(Default)]
struct MarkdownTable {
    rows: Vec<Vec<TableCell>>,
    current_row: Vec<TableCell>,
    current_cell: TableCell,
    in_cell: bool,
}

struct MarkdownCodeBlock {
    lang: Option<String>,
    text: String,
}

struct MarkdownImage {
    dest_url: String,
    alt: String,
}

pub(crate) type CodeHighlighter<'a> =
    dyn FnMut(Option<&str>, &str) -> Option<Vec<Vec<Span<'static>>>> + 'a;

/// A clickable hyperlink located in content (line/column) coordinates, produced
/// by the renderer and later mapped to screen coordinates in `render_preview`.
pub(crate) struct PreviewLink {
    pub(crate) line: usize,
    pub(crate) col: usize,
    pub(crate) width: usize,
    pub(crate) url: String,
}

pub(crate) fn markdown_preview_lines<'a>(
    text: &str,
    theme: &ResolvedTheme,
    width: usize,
    base_dir: Option<&Path>,
    highlight: &'a mut CodeHighlighter<'a>,
    change_bars: Option<&'a MarkdownChangeBars>,
) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
    let styles = MarkdownStyles::from_theme(theme);
    let content_width = width
        .saturating_sub(
            change_bars
                .map(MarkdownChangeBars::gutter_width)
                .unwrap_or(0),
        )
        .max(1);
    let mut renderer = MarkdownRenderer::new(
        &styles,
        theme,
        content_width,
        base_dir,
        highlight,
        change_bars,
    );
    renderer.run(text);
    renderer.finish()
}

struct MarkdownRenderer<'a> {
    styles: &'a MarkdownStyles,
    theme: &'a ResolvedTheme,
    width: usize,
    base_dir: Option<PathBuf>,
    highlight: &'a mut CodeHighlighter<'a>,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    style_stack: Vec<Style>,
    lists: Vec<MarkdownList>,
    quotes: Vec<QuoteFrame>,
    link_urls: Vec<String>,
    images: Vec<MarkdownImage>,
    table: Option<MarkdownTable>,
    code_block: Option<MarkdownCodeBlock>,
    pending_marker: Option<Span<'static>>,
    /// True while emitting the body of a completed task-list item.
    done_task: bool,
    /// Clickable links collected in content coordinates.
    links: Vec<PreviewLink>,
    /// (index into `current`, url) for link spans on the line being built.
    current_link_marks: Vec<(usize, String)>,
    change_bars: Option<&'a MarkdownChangeBars>,
    line_change_styles: Vec<Option<Style>>,
    current_change_style: Option<Style>,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(
        styles: &'a MarkdownStyles,
        theme: &'a ResolvedTheme,
        width: usize,
        base_dir: Option<&Path>,
        highlight: &'a mut CodeHighlighter<'a>,
        change_bars: Option<&'a MarkdownChangeBars>,
    ) -> Self {
        MarkdownRenderer {
            styles,
            theme,
            width: width.max(1),
            base_dir: base_dir.map(Path::to_path_buf),
            highlight,
            lines: Vec::new(),
            current: Vec::new(),
            style_stack: vec![styles.text],
            lists: Vec::new(),
            quotes: Vec::new(),
            link_urls: Vec::new(),
            images: Vec::new(),
            table: None,
            code_block: None,
            pending_marker: None,
            done_task: false,
            links: Vec::new(),
            current_link_marks: Vec::new(),
            change_bars,
            line_change_styles: Vec::new(),
            current_change_style: None,
        }
    }

    fn run(&mut self, text: &str) {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_TASKLISTS);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_FOOTNOTES);
        options.insert(Options::ENABLE_GFM);
        options.insert(Options::ENABLE_DEFINITION_LIST);
        options.insert(Options::ENABLE_HEADING_ATTRIBUTES);

        // The current heading level while inside a heading (for band + level style).
        let mut heading_level: Option<u8> = None;
        let line_starts = markdown_line_starts(text);

        for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
            self.note_change_range(&line_starts, range);
            match event {
                Event::Start(tag) => match tag {
                    Tag::Paragraph => {}
                    Tag::Heading { level, .. } => {
                        self.blank();
                        let idx = (level as usize).clamp(1, 6) - 1;
                        heading_level = Some(level as u8);
                        self.current.push(Span::styled(
                            self.styles.heading_marks[idx].to_string(),
                            self.styles.headings[idx],
                        ));
                        self.style_stack.push(self.styles.headings[idx]);
                    }
                    Tag::BlockQuote(kind) => {
                        let callout = kind.map(|k| Callout::from_kind(k, self.theme));
                        let border = match &callout {
                            Some(c) => Style::default().fg(c.color).add_modifier(Modifier::BOLD),
                            None => self.styles.muted,
                        };
                        self.quotes.push(QuoteFrame { border });
                        if let Some(c) = callout {
                            let title = Style::default().fg(c.color).add_modifier(Modifier::BOLD);
                            self.current
                                .push(Span::styled(format!("{} {}", c.icon, c.label), title));
                            self.flush();
                        }
                        self.style_stack.push(self.styles.muted);
                    }
                    Tag::CodeBlock(kind) => {
                        self.blank();
                        let lang = match kind {
                            CodeBlockKind::Fenced(lang) => {
                                let lang = lang.trim();
                                (!lang.is_empty()).then(|| lang.to_string())
                            }
                            CodeBlockKind::Indented => None,
                        };
                        self.code_block = Some(MarkdownCodeBlock {
                            lang,
                            text: String::new(),
                        });
                    }
                    Tag::List(start) => {
                        // In a tight list the parent item's text arrives before
                        // its nested list; flush it onto its own line first.
                        self.flush();
                        self.lists.push(MarkdownList {
                            next: start,
                            marker_width: 0,
                        });
                    }
                    Tag::Item => {
                        // Each item's task state is independent; a TaskListMarker
                        // sets it back on if this item is a completed task.
                        self.done_task = false;
                        let depth = self.lists.len();
                        let marker = match self.lists.last().and_then(|l| l.next) {
                            Some(next) => format!("{next}. "),
                            None => markdown_bullet(depth).to_string(),
                        };
                        if let Some(list) = self.lists.last_mut() {
                            list.marker_width = text_width(&marker);
                        }
                        // Ordered markers keep the accent color; bullets cycle
                        // color by nesting depth like markview.
                        let marker_style = if self.lists.last().and_then(|l| l.next).is_some() {
                            self.styles.list_marker
                        } else {
                            let color = self.styles.list_colors[(depth.saturating_sub(1)) % 4];
                            Style::default().fg(color).add_modifier(Modifier::BOLD)
                        };
                        self.pending_marker = Some(Span::styled(marker, marker_style));
                    }
                    Tag::FootnoteDefinition(label) => {
                        self.blank();
                        self.current
                            .push(Span::styled(format!("[{label}] "), self.styles.muted));
                    }
                    Tag::DefinitionList | Tag::DefinitionListTitle => {}
                    Tag::DefinitionListDefinition => {
                        self.current
                            .push(Span::styled("  ".to_string(), self.styles.muted));
                    }
                    Tag::Table(_) => self.table = Some(MarkdownTable::default()),
                    Tag::TableHead | Tag::TableRow => {
                        if let Some(table) = self.table.as_mut() {
                            table.current_row.clear();
                        }
                    }
                    Tag::TableCell => {
                        if let Some(table) = self.table.as_mut() {
                            table.current_cell.clear();
                            table.in_cell = true;
                        }
                    }
                    Tag::Emphasis => self
                        .style_stack
                        .push(self.top_style().add_modifier(Modifier::ITALIC)),
                    Tag::Strong => self
                        .style_stack
                        .push(self.top_style().add_modifier(Modifier::BOLD)),
                    Tag::Strikethrough => self
                        .style_stack
                        .push(self.top_style().add_modifier(Modifier::CROSSED_OUT)),
                    Tag::Superscript | Tag::Subscript => self
                        .style_stack
                        .push(self.top_style().add_modifier(Modifier::DIM)),
                    Tag::Link { dest_url, .. } => {
                        self.link_urls.push(dest_url.to_string());
                        self.style_stack.push(self.styles.link);
                    }
                    Tag::Image { dest_url, .. } => {
                        self.images.push(MarkdownImage {
                            dest_url: dest_url.to_string(),
                            alt: String::new(),
                        });
                        self.style_stack.push(self.styles.link);
                    }
                    Tag::HtmlBlock | Tag::MetadataBlock(_) => {
                        self.style_stack.push(self.styles.muted)
                    }
                },
                Event::End(end) => match end {
                    TagEnd::Paragraph => {
                        self.flush();
                        self.blank();
                    }
                    TagEnd::Heading(_) => {
                        self.style_stack.pop();
                        let level = heading_level.take().unwrap_or(1);
                        self.flush_heading(level);
                        self.blank();
                    }
                    TagEnd::BlockQuote(_) => {
                        self.style_stack.pop();
                        self.flush();
                        // Drop the dangling border-only line the closing blank
                        // separator left after the quote's last content line.
                        if self.lines.last().is_some_and(markdown_line_is_quote_border) {
                            self.lines.pop();
                            self.line_change_styles.pop();
                        }
                        self.quotes.pop();
                        self.blank();
                    }
                    TagEnd::CodeBlock => {
                        if let Some(block) = self.code_block.take() {
                            self.render_code_block(block);
                        }
                        self.blank();
                    }
                    TagEnd::List(_) => {
                        self.lists.pop();
                        self.blank();
                    }
                    TagEnd::Item => {
                        self.flush();
                        self.pending_marker = None;
                        if let Some(list) = self.lists.last_mut() {
                            if let Some(next) = list.next.as_mut() {
                                *next = next.saturating_add(1);
                            }
                        }
                    }
                    TagEnd::FootnoteDefinition
                    | TagEnd::DefinitionList
                    | TagEnd::DefinitionListTitle
                    | TagEnd::DefinitionListDefinition => {
                        self.flush();
                    }
                    TagEnd::Table => {
                        if let Some(table) = self.table.take() {
                            self.render_table(table);
                            self.blank();
                        }
                    }
                    TagEnd::TableHead | TagEnd::TableRow => {
                        if let Some(table) = self.table.as_mut() {
                            table.rows.push(std::mem::take(&mut table.current_row));
                        }
                    }
                    TagEnd::TableCell => {
                        if let Some(table) = self.table.as_mut() {
                            let cell = trim_cell_spans(std::mem::take(&mut table.current_cell));
                            table.current_row.push(cell);
                            table.in_cell = false;
                        }
                    }
                    TagEnd::Emphasis
                    | TagEnd::Strong
                    | TagEnd::Strikethrough
                    | TagEnd::Superscript
                    | TagEnd::Subscript
                    | TagEnd::HtmlBlock
                    | TagEnd::MetadataBlock(_) => {
                        self.style_stack.pop();
                    }
                    TagEnd::Link => {
                        self.style_stack.pop();
                        // Push the URL suffix while the link is still active so
                        // it becomes part of the same clickable region.
                        if let Some(url) = self.link_urls.last().cloned() {
                            self.push_text(&format!(" ({url})"), self.styles.muted);
                        }
                        self.link_urls.pop();
                    }
                    TagEnd::Image => {
                        self.style_stack.pop();
                        if let Some(image) = self.images.pop() {
                            self.render_image(image);
                        }
                    }
                },
                Event::Text(text) => {
                    if let Some(image) = self.images.last_mut() {
                        image.alt.push_str(text.as_ref());
                    } else if let Some(block) = self.code_block.as_mut() {
                        block.text.push_str(text.as_ref());
                    } else {
                        let style = self.top_style();
                        self.push_text(text.as_ref(), style);
                    }
                }
                Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                    if let Some(image) = self.images.last_mut() {
                        image.alt.push_str(text.as_ref());
                    } else {
                        self.push_inline_code(text.as_ref());
                    }
                }
                Event::Html(text) | Event::InlineHtml(text) => {
                    self.push_text(text.as_ref(), self.styles.muted)
                }
                Event::FootnoteReference(label) => {
                    self.push_text(&format!("[{label}]"), self.styles.muted)
                }
                Event::SoftBreak => {
                    if let Some(image) = self.images.last_mut() {
                        image.alt.push(' ');
                    } else {
                        let style = self.top_style();
                        self.push_text(" ", style);
                    }
                }
                Event::HardBreak => self.flush(),
                Event::Rule => {
                    self.blank();
                    // A centered diamond ornament on a full-width rule.
                    let mid = self.width / 2;
                    let left = "─".repeat(mid);
                    let right = "─".repeat(self.width.saturating_sub(mid + 1));
                    self.push_current_line(Line::from(Span::styled(
                        format!("{left}◇{right}"),
                        self.styles.rule,
                    )));
                    self.blank();
                }
                Event::TaskListMarker(done) => {
                    // Completed items get dimmed + struck-through body text.
                    self.done_task = done;
                    let (glyph, style) = if done {
                        ("▣ ", self.styles.check_done)
                    } else {
                        ("▢ ", self.styles.check_todo)
                    };
                    if self.pending_marker.is_some() {
                        if let Some(list) = self.lists.last_mut() {
                            list.marker_width = text_width(glyph);
                        }
                        self.pending_marker = Some(Span::styled(glyph.to_string(), style));
                    } else {
                        self.push_text(glyph, style);
                    }
                }
            }
        }
    }

    fn finish(mut self) -> (Vec<Line<'static>>, Vec<PreviewLink>) {
        self.flush();
        while self.lines.last().is_some_and(markdown_line_is_blank) {
            self.lines.pop();
            self.line_change_styles.pop();
        }
        if self.lines.is_empty() {
            self.push_line(Line::from(""), None);
        }
        self.add_change_gutters();
        (self.lines, self.links)
    }

    fn top_style(&self) -> Style {
        self.style_stack.last().copied().unwrap_or_default()
    }

    fn note_change_range(&mut self, line_starts: &[usize], range: std::ops::Range<usize>) {
        let Some(change_bars) = self.change_bars else {
            return;
        };
        if range.is_empty() {
            return;
        }
        let start = markdown_line_for_byte(line_starts, range.start);
        let end = markdown_line_for_byte(line_starts, range.end.saturating_sub(1));
        for line in start..=end {
            if let Some(style) = change_bars.styles.get(&line) {
                self.current_change_style.get_or_insert(*style);
            }
        }
    }

    fn push_line(&mut self, line: Line<'static>, style: Option<Style>) {
        self.lines.push(line);
        self.line_change_styles.push(style);
    }

    fn push_current_line(&mut self, line: Line<'static>) {
        let style = self.current_change_style.take();
        self.push_line(line, style);
    }

    fn add_change_gutters(&mut self) {
        let Some(change_bars) = self.change_bars else {
            return;
        };
        for (idx, line) in self.lines.iter_mut().enumerate() {
            let style = self.line_change_styles.get(idx).copied().flatten();
            let marker = match style {
                Some(style) => Span::styled(change_bars.marker.clone(), style),
                None => Span::raw(" ".repeat(change_bars.marker_width)),
            };
            let mut spans = vec![marker, Span::raw(" ")];
            spans.extend(std::mem::take(&mut line.spans));
            line.spans = spans;
        }
        let gutter = change_bars.gutter_width();
        for link in &mut self.links {
            link.col = link.col.saturating_add(gutter);
        }
    }

    /// Leading spans repeated at the start of every visual line: block-quote
    /// borders followed by the list marker (or hanging-indent padding).
    fn line_prefix(&mut self) -> Vec<Span<'static>> {
        let mut prefix = Vec::new();
        for quote in &self.quotes {
            prefix.push(Span::styled("▎ ".to_string(), quote.border));
        }
        let depth = self.lists.len();
        if depth > 0 {
            let indent = 2 * (depth - 1);
            if let Some(marker) = self.pending_marker.take() {
                if indent > 0 {
                    prefix.push(Span::styled(" ".repeat(indent), self.styles.text));
                }
                prefix.push(marker);
            } else {
                let cont = indent + self.lists.last().map_or(0, |l| l.marker_width);
                if cont > 0 {
                    prefix.push(Span::styled(" ".repeat(cont), self.styles.text));
                }
            }
        }
        prefix
    }

    fn flush(&mut self) {
        if self.current.is_empty() {
            self.current_link_marks.clear();
            return;
        }
        let line_idx = self.lines.len();
        let prefix = self.line_prefix();
        let prefix_w: usize = prefix.iter().map(|s| text_width(&s.content)).sum();
        if !self.current_link_marks.is_empty() {
            let mut col = prefix_w;
            let mut cols = Vec::with_capacity(self.current.len());
            for span in &self.current {
                cols.push(col);
                col += text_width(&span.content);
            }
            let span_w = |i: usize| self.current.get(i).map_or(0, |s| text_width(&s.content));
            // Coalesce consecutive same-url spans into one clickable region.
            let mut marks = std::mem::take(&mut self.current_link_marks)
                .into_iter()
                .peekable();
            while let Some((idx, url)) = marks.next() {
                let Some(&start) = cols.get(idx) else {
                    continue;
                };
                let mut end = start + span_w(idx);
                while let Some((nidx, nurl)) = marks.peek() {
                    if *nurl == url && cols.get(*nidx) == Some(&end) {
                        end += span_w(*nidx);
                        marks.next();
                    } else {
                        break;
                    }
                }
                self.links.push(PreviewLink {
                    line: line_idx,
                    col: start,
                    width: end - start,
                    url,
                });
            }
        }
        let mut line = prefix;
        line.append(&mut self.current);
        self.push_current_line(Line::from(line));
    }

    fn push_prefixed_line(&mut self, spans: &mut Vec<Span<'static>>, style: Option<Style>) {
        let mut line = self.line_prefix();
        line.append(spans);
        self.push_line(Line::from(line), style);
    }

    /// Emit the current heading line on a full-width band tinted with the
    /// heading's own color (markview-style colored heading bars).
    fn flush_heading(&mut self, level: u8) {
        // Heading links aren't tracked as clickable; drop any pending marks.
        self.current_link_marks.clear();
        if self.current.is_empty() {
            return;
        }
        let idx = (level as usize).clamp(1, 6) - 1;
        let mut spans = self.line_prefix();
        spans.append(&mut self.current);
        if let Some(band) = self.styles.heading_bands[idx] {
            for span in &mut spans {
                span.style = span.style.bg(band);
            }
            let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
            if used < self.width {
                spans.push(Span::styled(
                    " ".repeat(self.width - used),
                    Style::default().bg(band),
                ));
            }
        }
        self.push_current_line(Line::from(spans));
    }

    fn blank(&mut self) {
        if self.quotes.is_empty() {
            if !self.lines.last().is_some_and(markdown_line_is_blank) {
                self.push_line(Line::from(""), None);
            }
        } else {
            // Continue the quote border through the blank separator line.
            let prefix: Vec<Span<'static>> = self
                .quotes
                .iter()
                .map(|q| Span::styled("▎".to_string(), q.border))
                .collect();
            self.push_line(Line::from(prefix), None);
        }
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if let Some(table) = self.table.as_mut() {
            let cell = text.replace('\n', " ");
            if !cell.is_empty() {
                table.current_cell.push(Span::styled(cell, style));
            }
            return;
        }
        let style = self.done_style(style);
        let linking = self.link_urls.last().cloned();
        for (idx, part) in text.split('\n').enumerate() {
            if idx > 0 {
                self.flush();
            }
            if !part.is_empty() {
                self.current.push(Span::styled(part.to_string(), style));
                if let Some(url) = &linking {
                    self.current_link_marks
                        .push((self.current.len() - 1, url.clone()));
                }
            }
        }
    }

    /// Fade and strike a style when inside a completed task-list item.
    fn done_style(&self, style: Style) -> Style {
        if self.done_task {
            style.add_modifier(Modifier::DIM | Modifier::CROSSED_OUT)
        } else {
            style
        }
    }

    /// Inline code / math as a padded background "chip" (` code `), matching
    /// markview. In table cells the padding is dropped to keep columns tight,
    /// but the code keeps its color/background.
    fn push_inline_code(&mut self, text: &str) {
        if let Some(table) = self.table.as_mut() {
            let cell = text.replace('\n', " ");
            if !cell.is_empty() {
                table
                    .current_cell
                    .push(Span::styled(cell, self.styles.code));
            }
            return;
        }
        let chip = if self.styles.inline_bg.is_some() {
            format!(" {} ", text.replace('\n', " "))
        } else {
            text.replace('\n', " ")
        };
        let style = self.done_style(self.styles.code);
        self.current.push(Span::styled(chip, style));
    }

    fn render_code_block(&mut self, block: MarkdownCodeBlock) {
        let highlighted = (self.highlight)(block.lang.as_deref(), &block.text);
        let bg = self.styles.code_bg;
        let code_lines: Vec<&str> = block.text.lines().collect();

        // Shrink-wrap the panel to its content (like display: inline-block):
        // the wider of the longest code line and the language label, rather than
        // stretching to the viewport. PAD columns of inner padding each side, a
        // minimum width (≈45% of the viewport) so short snippets still read as a
        // panel, and a blank padded row below for vertical breathing room.
        const PAD: usize = 2;
        let min_width = (self.width * 45 / 100).min(self.width);
        let label = block.lang.as_deref().unwrap_or("code");
        let code_max = code_lines.iter().map(|l| text_width(l)).max().unwrap_or(0);
        let width = (PAD + code_max + PAD)
            .max(PAD + text_width(label) + PAD)
            .max(min_width)
            .min(self.width);
        let change_style = self.current_change_style.take();
        let pad_style = Style::default().bg_opt(bg);
        let pad = |n: usize| Span::styled(" ".repeat(n), pad_style);

        // Header row: language label near the right edge, with a one-column
        // trailing space before the panel border.
        let header = vec![
            pad(width.saturating_sub(text_width(label) + 1)),
            Span::styled(format!("{label} "), self.styles.code_lang),
        ];
        self.push_line(Line::from(header), change_style);

        for (idx, raw) in code_lines.iter().enumerate() {
            let mut spans = vec![pad(PAD)];
            match highlighted.as_ref().and_then(|rows| rows.get(idx)) {
                Some(row) => {
                    for span in row {
                        let mut style = span.style;
                        if let Some(bg) = bg {
                            style = style.bg(bg);
                        }
                        spans.push(Span::styled(span.content.to_string(), style));
                    }
                }
                None => {
                    let mut style = Style::default().fg(self.styles.code.fg.unwrap_or_default());
                    if let Some(bg) = bg {
                        style = style.bg(bg);
                    }
                    spans.push(Span::styled(raw.to_string(), style));
                }
            }
            pad_row(&mut spans, width, bg);
            self.push_line(Line::from(spans), change_style);
        }
        self.push_line(
            Line::from(Span::styled(" ".repeat(width), pad_style)),
            change_style,
        );
    }

    fn render_image(&mut self, image: MarkdownImage) {
        if self.table.is_some() {
            let label = image_fallback_label(&image.alt, &image.dest_url);
            self.push_text(&label, self.styles.muted);
            return;
        }
        self.flush();
        let change_style = self.current_change_style.take();
        let Some(path) = self.local_image_path(&image.dest_url) else {
            let mut line = vec![Span::styled(
                image_fallback_label(&image.alt, &image.dest_url),
                self.styles.muted,
            )];
            self.push_prefixed_line(&mut line, change_style);
            return;
        };
        let bg = self
            .theme
            .background
            .or(self.theme.background_panel)
            .unwrap_or(Color::Black);
        let Some(image_lines) = image_preview_lines(&path, self.width, 20, bg) else {
            let mut line = vec![Span::styled(
                image_fallback_label(&image.alt, &image.dest_url),
                self.styles.muted,
            )];
            self.push_prefixed_line(&mut line, change_style);
            return;
        };
        let alt = image.alt.trim();
        if !alt.is_empty() {
            let mut caption = vec![Span::styled(format!("image: {alt}"), self.styles.muted)];
            self.push_prefixed_line(&mut caption, change_style);
        }
        for line in image_lines {
            let mut spans = line.spans;
            self.push_prefixed_line(&mut spans, change_style);
        }
        self.blank();
    }

    fn local_image_path(&self, url: &str) -> Option<PathBuf> {
        if url.is_empty() || url.starts_with('#') || url.starts_with("data:") || url.contains("://")
        {
            return None;
        }
        let path = url
            .split(['?', '#'])
            .next()
            .filter(|path| !path.is_empty())?;
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            self.base_dir.as_ref()?.join(path)
        };
        path.is_file().then_some(path)
    }

    fn render_table(&mut self, table: MarkdownTable) {
        let change_style = self.current_change_style.take();
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let mut widths = vec![0usize; cols];
        for row in &table.rows {
            for (idx, cell) in row.iter().enumerate() {
                widths[idx] = widths[idx].max(cell_width(cell));
            }
        }
        for width in &mut widths {
            *width = (*width).max(1);
        }

        let border = self.styles.table_border;
        let empty: TableCell = Vec::new();
        self.push_line(
            Line::from(Span::styled(table_border("╭", "┬", "╮", &widths), border)),
            change_style,
        );
        for (row_idx, row) in table.rows.iter().enumerate() {
            let is_head = row_idx == 0;
            let mut spans = vec![Span::styled("│".to_string(), border)];
            for (col, width) in widths.iter().enumerate().take(cols) {
                let cell = row.get(col).unwrap_or(&empty);
                spans.push(Span::styled(" ".to_string(), self.styles.text));
                for span in cell {
                    let style = self.table_cell_style(span.style, is_head);
                    spans.push(Span::styled(span.content.to_string(), style));
                }
                let pad = width.saturating_sub(cell_width(cell));
                spans.push(Span::styled(
                    format!("{} ", " ".repeat(pad)),
                    self.styles.text,
                ));
                spans.push(Span::styled("│".to_string(), border));
            }
            self.push_line(Line::from(spans), change_style);
            if is_head && table.rows.len() > 1 {
                self.push_line(
                    Line::from(Span::styled(table_border("├", "┼", "┤", &widths), border)),
                    change_style,
                );
            }
        }
        self.push_line(
            Line::from(Span::styled(table_border("╰", "┴", "╯", &widths), border)),
            change_style,
        );
    }

    /// Style a table cell span. Header cells make plain body text accent-bold
    /// (like a header) while leaving inline code / links their own color, and
    /// bolden special spans so the header row still reads as a header.
    fn table_cell_style(&self, span: Style, is_head: bool) -> Style {
        if !is_head {
            return span;
        }
        if span.fg == self.styles.text.fg {
            self.styles.table_head.add_modifier(span.add_modifier)
        } else {
            span.add_modifier(Modifier::BOLD)
        }
    }
}

fn markdown_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn markdown_line_for_byte(starts: &[usize], byte: usize) -> usize {
    match starts.binary_search(&byte) {
        Ok(index) => index + 1,
        Err(index) => index.max(1),
    }
}

fn image_fallback_label(alt: &str, url: &str) -> String {
    let alt = alt.trim();
    if alt.is_empty() {
        format!("image: {url}")
    } else {
        format!("image: {alt} ({url})")
    }
}

fn image_preview_lines(
    path: &Path,
    max_width: usize,
    max_rows: usize,
    bg: Color,
) -> Option<Vec<Line<'static>>> {
    let image = image::ImageReader::open(path).ok()?.decode().ok()?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || max_width == 0 || max_rows == 0 {
        return None;
    }
    let max_cols = max_width.clamp(1, 80) as u32;
    let max_pixel_rows = (max_rows as u32).saturating_mul(2).max(1);
    let scale = (max_cols as f32 / width as f32)
        .min(max_pixel_rows as f32 / height as f32)
        .min(1.0);
    let target_width = ((width as f32 * scale).round() as u32).max(1);
    let target_height = ((height as f32 * scale).round() as u32).max(1);
    let resized = image
        .resize(
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        )
        .to_rgba8();
    let mut lines = Vec::new();
    for y in (0..target_height).step_by(2) {
        let mut spans = Vec::with_capacity(target_width as usize);
        for x in 0..target_width {
            let top = *resized.get_pixel(x, y);
            let bottom = if y + 1 < target_height {
                *resized.get_pixel(x, y + 1)
            } else {
                image::Rgba([0, 0, 0, 0])
            };
            spans.push(Span::styled(
                "▀".to_string(),
                Style::default()
                    .fg(rgba_over_bg(top, bg))
                    .bg(rgba_over_bg(bottom, bg)),
            ));
        }
        lines.push(Line::from(spans));
    }
    Some(lines)
}

fn rgba_over_bg(pixel: image::Rgba<u8>, bg: Color) -> Color {
    let [r, g, b, a] = pixel.0;
    if a == 255 {
        return Color::Rgb(r, g, b);
    }
    let (br, bgc, bb) = color_rgb(bg).unwrap_or((0, 0, 0));
    if a == 0 {
        return Color::Rgb(br, bgc, bb);
    }
    let alpha = a as f32 / 255.0;
    Color::Rgb(
        ((r as f32 * alpha) + (br as f32 * (1.0 - alpha))).round() as u8,
        ((g as f32 * alpha) + (bgc as f32 * (1.0 - alpha))).round() as u8,
        ((b as f32 * alpha) + (bb as f32 * (1.0 - alpha))).round() as u8,
    )
}

fn color_rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((255, 0, 0)),
        Color::Green => Some((0, 255, 0)),
        Color::Yellow => Some((255, 255, 0)),
        Color::Blue => Some((0, 0, 255)),
        Color::Magenta => Some((255, 0, 255)),
        Color::Cyan => Some((0, 255, 255)),
        Color::Gray => Some((128, 128, 128)),
        Color::DarkGray => Some((64, 64, 64)),
        Color::LightRed => Some((255, 128, 128)),
        Color::LightGreen => Some((128, 255, 128)),
        Color::LightYellow => Some((255, 255, 128)),
        Color::LightBlue => Some((128, 128, 255)),
        Color::LightMagenta => Some((255, 128, 255)),
        Color::LightCyan => Some((128, 255, 255)),
        Color::White => Some((255, 255, 255)),
        _ => None,
    }
}

/// Total display width of a table cell's spans.
fn cell_width(cell: &[Span<'_>]) -> usize {
    cell.iter().map(|s| text_width(&s.content)).sum()
}

/// Trim leading/trailing whitespace across a cell's styled spans.
fn trim_cell_spans(mut cell: TableCell) -> TableCell {
    while let Some(first) = cell.first_mut() {
        let trimmed = first.content.trim_start();
        if trimmed.is_empty() {
            cell.remove(0);
        } else {
            if trimmed.len() != first.content.len() {
                first.content = trimmed.to_string().into();
            }
            break;
        }
    }
    while let Some(last) = cell.last_mut() {
        let trimmed = last.content.trim_end();
        if trimmed.is_empty() {
            cell.pop();
        } else {
            if trimmed.len() != last.content.len() {
                last.content = trimmed.to_string().into();
            }
            break;
        }
    }
    cell
}

/// Depth-aware bullet marker for unordered list items (1-based depth).
fn markdown_bullet(depth: usize) -> &'static str {
    match depth {
        0 | 1 => "● ",
        2 => "○ ",
        3 => "◆ ",
        _ => "▸ ",
    }
}

/// Pad a row of spans with a trailing background block out to `width`.
fn pad_row(spans: &mut Vec<Span<'static>>, width: usize, bg: Option<Color>) {
    let Some(bg) = bg else { return };
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    if used < width {
        spans.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(bg),
        ));
    }
}

fn markdown_line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

/// True when a line contains only block-quote border glyphs and whitespace
/// (the continuation border drawn on blank lines inside a quote).
pub(crate) fn markdown_line_is_quote_border(line: &Line<'_>) -> bool {
    let mut saw_border = false;
    for span in &line.spans {
        for ch in span.content.chars() {
            match ch {
                '▎' => saw_border = true,
                c if c.is_whitespace() => {}
                _ => return false,
            }
        }
    }
    saw_border
}

fn table_border(left: &str, middle: &str, right: &str, widths: &[usize]) -> String {
    let mut line = String::from(left);
    for (idx, width) in widths.iter().enumerate() {
        if idx > 0 {
            line.push_str(middle);
        }
        line.push_str(&"─".repeat(width.saturating_add(2)));
    }
    line.push_str(right);
    line
}

/// Small extension so background colors that are `Option<Color>` (transparent
/// themes) can be applied fluently while building styles.
trait StyleBgOpt {
    fn bg_opt(self, color: Option<Color>) -> Self;
}

impl StyleBgOpt for Style {
    fn bg_opt(self, color: Option<Color>) -> Self {
        match color {
            Some(color) => self.bg(color),
            None => self,
        }
    }
}
