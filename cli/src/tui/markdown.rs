use super::syntax::{SYNTAX_SET, THEME_NAME, THEME_SET};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting;
use syntect::util::LinesWithEndings;
use unicode_width::UnicodeWidthStr;

/// Convert markdown text to styled ratatui [`Line`]s.
pub fn render_markdown(input: &str) -> Vec<Line<'static>> {
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(input, options);

    let mut renderer = MdRenderer::new();
    for event in parser {
        renderer.handle_event(event);
    }
    renderer.finish()
}

// ---------------------------------------------------------------------------

struct MdRenderer {
    lines: Vec<Line<'static>>,
    current_spans: Vec<Span<'static>>,

    // style state
    bold: bool,
    italic: bool,
    strikethrough: bool,
    in_link: bool,
    heading_level: Option<usize>,

    // code block state
    in_code_block: bool,
    code_block_lang: String,
    code_block_buf: String,

    // list state
    list_depth: usize,
    ordered_index: Vec<u64>,

    // table state
    in_table: bool,
    table_alignments: Vec<Alignment>,
    table_rows: Vec<Vec<Vec<Span<'static>>>>,
}

impl MdRenderer {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            current_spans: Vec::new(),
            bold: false,
            italic: false,
            strikethrough: false,
            in_link: false,
            heading_level: None,
            in_code_block: false,
            code_block_lang: String::new(),
            code_block_buf: String::new(),
            list_depth: 0,
            ordered_index: Vec::new(),
            in_table: false,
            table_alignments: Vec::new(),
            table_rows: Vec::new(),
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            // --- Inline styles ---
            Event::Start(Tag::Strong) => self.bold = true,
            Event::End(TagEnd::Strong) => self.bold = false,
            Event::Start(Tag::Emphasis) => self.italic = true,
            Event::End(TagEnd::Emphasis) => self.italic = false,
            Event::Start(Tag::Strikethrough) => self.strikethrough = true,
            Event::End(TagEnd::Strikethrough) => self.strikethrough = false,
            Event::Start(Tag::Link { .. }) => self.in_link = true,
            Event::End(TagEnd::Link) => self.in_link = false,

            // --- Text ---
            Event::Text(text) => {
                if self.in_code_block {
                    self.code_block_buf.push_str(&text);
                } else {
                    self.push_styled_span(&text);
                }
            }
            Event::Code(code) => {
                // Render inline code with background, no visible backticks.
                self.current_spans.push(Span::styled(
                    code.to_string(),
                    Style::default()
                        .fg(Color::Cyan)
                        .bg(Color::Rgb(40, 40, 55)),
                ));
            }
            Event::SoftBreak => {
                self.current_spans.push(Span::raw(" "));
            }
            Event::HardBreak => self.flush_line(),

            // --- Headings ---
            Event::Start(Tag::Heading { level, .. }) => {
                self.ensure_blank_line();
                self.heading_level = Some(level as usize);
            }
            Event::End(TagEnd::Heading(_)) => {
                self.heading_level = None;
                self.flush_line();
            }

            // --- Paragraphs ---
            Event::Start(Tag::Paragraph) => {
                if !self.lines.is_empty() && self.list_depth == 0 {
                    self.lines.push(Line::from(""));
                }
            }
            Event::End(TagEnd::Paragraph) => self.flush_line(),

            // --- Code blocks ---
            Event::Start(Tag::CodeBlock(kind)) => {
                self.ensure_blank_line();
                self.in_code_block = true;
                self.code_block_buf.clear();
                self.code_block_lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        lang.split([',', ' ']).next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
            }
            Event::End(TagEnd::CodeBlock) => {
                self.in_code_block = false;
                let lang = std::mem::take(&mut self.code_block_lang);
                let code = std::mem::take(&mut self.code_block_buf);
                self.render_code_block(&code, &lang);
                self.ensure_blank_line();
            }

            // --- Lists ---
            Event::Start(Tag::List(start)) => {
                if self.list_depth == 0 {
                    self.ensure_blank_line();
                }
                self.list_depth += 1;
                self.ordered_index.push(start.unwrap_or(0));
            }
            Event::End(TagEnd::List(_)) => {
                self.list_depth = self.list_depth.saturating_sub(1);
                self.ordered_index.pop();
                if self.list_depth == 0 {
                    self.ensure_blank_line();
                }
            }
            Event::Start(Tag::Item) => {
                let indent = "  ".repeat(self.list_depth.saturating_sub(1));
                if let Some(idx) = self.ordered_index.last_mut() {
                    if *idx > 0 {
                        self.current_spans.push(Span::styled(
                            format!("{indent}{idx}. "),
                            Style::default().fg(Color::Blue),
                        ));
                        *idx += 1;
                    } else {
                        self.current_spans.push(Span::styled(
                            format!("{indent}• "),
                            Style::default().fg(Color::Blue),
                        ));
                    }
                }
            }
            Event::End(TagEnd::Item) => self.flush_line(),

            // --- Blockquote ---
            Event::Start(Tag::BlockQuote(..)) => {}
            Event::End(TagEnd::BlockQuote(..)) => {}

            // --- Tables ---
            Event::Start(Tag::Table(alignments)) => {
                self.ensure_blank_line();
                self.in_table = true;
                self.table_alignments = alignments;
                self.table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                self.in_table = false;
                self.flush_table();
                self.ensure_blank_line();
            }
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                self.table_rows.push(Vec::new());
            }
            Event::End(TagEnd::TableHead) => {}
            Event::End(TagEnd::TableRow) => {}
            Event::Start(Tag::TableCell) => {
                self.current_spans.clear();
            }
            Event::End(TagEnd::TableCell) => {
                let spans = std::mem::take(&mut self.current_spans);
                if let Some(row) = self.table_rows.last_mut() {
                    row.push(spans);
                }
            }

            // --- Rule ---
            Event::Rule => {
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(40),
                    Style::default().fg(Color::DarkGray),
                )));
            }

            _ => {}
        }
    }

    fn current_style(&self) -> Style {
        let mut style = Style::default();

        if let Some(level) = self.heading_level {
            style = style.add_modifier(Modifier::BOLD);
            if level == 1 {
                style = style.add_modifier(Modifier::UNDERLINED);
            } else if level >= 3 {
                style = style.add_modifier(Modifier::ITALIC);
            }
            return style;
        }

        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.strikethrough {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.in_link {
            style = style.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
        }
        style
    }

    fn push_styled_span(&mut self, text: &str) {
        let style = self.current_style();
        if style == Style::default() {
            self.current_spans.push(Span::raw(text.to_string()));
        } else {
            self.current_spans
                .push(Span::styled(text.to_string(), style));
        }
    }

    fn ensure_blank_line(&mut self) {
        if !self.lines.is_empty()
            && self
                .lines
                .last()
                .is_some_and(|l| !l.spans.is_empty() || l.width() > 0)
        {
            self.lines.push(Line::from(""));
        }
    }

    fn flush_line(&mut self) {
        let spans = std::mem::take(&mut self.current_spans);
        if !spans.is_empty() {
            self.lines.push(Line::from(spans));
        }
    }

    fn flush_table(&mut self) {
        let rows = std::mem::take(&mut self.table_rows);
        let alignments = std::mem::take(&mut self.table_alignments);
        if rows.is_empty() {
            return;
        }

        let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut col_widths = vec![0usize; col_count];
        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                let w: usize = cell.iter().map(|s| s.content.width()).sum();
                col_widths[i] = col_widths[i].max(w);
            }
        }

        let border_style = Style::default().fg(Color::DarkGray);

        let hrule = |left: char, mid: char, right: char, fill: char| -> Line<'static> {
            let mut s = String::new();
            for (i, &w) in col_widths.iter().enumerate() {
                s.push(if i == 0 { left } else { mid });
                for _ in 0..w + 2 {
                    s.push(fill);
                }
            }
            s.push(right);
            Line::from(Span::styled(s, border_style))
        };

        self.lines.push(hrule('┌', '┬', '┐', '─'));

        let last_row = rows.len().saturating_sub(1);
        for (row_idx, row) in rows.iter().enumerate() {
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(Span::styled("│ ", border_style));

            for (col_idx, cell) in row.iter().enumerate() {
                let width = col_widths.get(col_idx).copied().unwrap_or(0);
                let text_width: usize = cell.iter().map(|s| s.content.width()).sum();
                let padding = width.saturating_sub(text_width);

                let align = alignments.get(col_idx).copied().unwrap_or(Alignment::None);
                let (pad_left, pad_right) = match align {
                    Alignment::Center => (padding / 2, padding - padding / 2),
                    Alignment::Right => (padding, 0),
                    _ => (0, padding),
                };

                if pad_left > 0 {
                    spans.push(Span::raw(" ".repeat(pad_left)));
                }
                spans.extend(cell.iter().cloned());
                if pad_right > 0 {
                    spans.push(Span::raw(" ".repeat(pad_right)));
                }
                spans.push(Span::styled(" │ ", border_style));
            }
            self.lines.push(Line::from(spans));

            if row_idx == 0 {
                self.lines.push(hrule('╞', '╪', '╡', '═'));
            } else if row_idx < last_row {
                self.lines.push(hrule('├', '┼', '┤', '─'));
            }
        }

        self.lines.push(hrule('└', '┴', '┘', '─'));
    }

    fn render_code_block(&mut self, code: &str, lang: &str) {
        let code = code.trim_end_matches('\n');
        if code.is_empty() {
            return;
        }

        let border_style = Style::default().fg(Color::DarkGray);
        let bg = Style::default().bg(Color::Rgb(30, 30, 46));
        let lineno_style = Style::default().fg(Color::DarkGray).bg(Color::Rgb(30, 30, 46));

        // Header with language label.
        let label = if lang.is_empty() { "code" } else { lang };
        self.lines.push(Line::from(Span::styled(
            format!("  ╭─ {label} "),
            border_style,
        )));

        // Highlighted code lines with line numbers.
        let highlighted = highlight_to_lines(code, lang);
        let width = highlighted.len().to_string().len();

        for (i, line) in highlighted.iter().enumerate() {
            let mut spans = vec![
                Span::styled(format!("  {:>width$} │ ", i + 1), lineno_style),
            ];
            for span in &line.spans {
                // Preserve syntax color but add code background.
                spans.push(Span::styled(
                    span.content.to_string(),
                    span.style.bg(Color::Rgb(30, 30, 46)),
                ));
            }
            // Pad with background if the line has no spans.
            if line.spans.is_empty() {
                spans.push(Span::styled(" ", bg));
            }
            self.lines.push(Line::from(spans));
        }

        // Footer.
        self.lines.push(Line::from(Span::styled(
            "  ╰───",
            border_style,
        )));
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_line();
        self.lines
    }
}

// ---------------------------------------------------------------------------
// Syntax highlighting
// ---------------------------------------------------------------------------

fn highlight_to_lines(code: &str, lang: &str) -> Vec<Line<'static>> {
    if !lang.is_empty()
        && let Some(syntax) = SYNTAX_SET.find_syntax_by_token(lang)
        && let Some(theme) = THEME_SET.themes.get(THEME_NAME)
    {
        return highlight_with_syntect(code, syntax, theme);
    }

    code.lines()
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(Color::White),
            ))
        })
        .collect()
}

fn highlight_with_syntect(
    code: &str,
    syntax: &syntect::parsing::SyntaxReference,
    theme: &highlighting::Theme,
) -> Vec<Line<'static>> {
    let mut h = HighlightLines::new(syntax, theme);
    let mut result = Vec::new();

    for line in LinesWithEndings::from(code) {
        let Ok(ranges) = h.highlight_line(line, &SYNTAX_SET) else {
            result.push(Line::from(Span::raw(line.trim_end().to_string())));
            continue;
        };

        let spans: Vec<Span<'static>> = ranges
            .into_iter()
            .map(|(style, text)| {
                let text = text.trim_end_matches('\n').trim_end_matches('\r');
                Span::styled(text.to_string(), convert_syntect_style(style))
            })
            .collect();

        result.push(Line::from(spans));
    }

    result
}

fn convert_syntect_style(syn: highlighting::Style) -> Style {
    let mut style = Style::default();

    let fg = syn.foreground;
    if fg.a == 0xFF {
        style = style.fg(Color::Rgb(fg.r, fg.g, fg.b));
    } else if fg.a == 0x00 {
        style = style.fg(Color::Indexed(fg.r));
    }

    if syn.font_style.contains(highlighting::FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if syn.font_style.contains(highlighting::FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }

    style
}
