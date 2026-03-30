use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block as UiBlock, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Block, DiffLine, TaskStatus};
use super::markdown::render_markdown;

/// Maximum input area height (including border).
const MAX_INPUT_HEIGHT: u16 = 10;

// ---------------------------------------------------------------------------
// Main draw
// ---------------------------------------------------------------------------

pub fn draw(frame: &mut Frame, app: &mut App) {
    let input_content_lines = app.input_line_count() as u16;
    let input_height = (input_content_lines + 2).min(MAX_INPUT_HEIGHT);

    let [status_area, chat_area, input_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(input_height),
    ])
    .areas(frame.area());

    draw_status_bar(frame, app, status_area);
    draw_chat(frame, app, chat_area);
    draw_input(frame, app, input_area);
}

// ---------------------------------------------------------------------------
// Status bar (pill-style segments + spinner)
// ---------------------------------------------------------------------------

fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let dim = Style::default().fg(Color::DarkGray);
    let sep = Span::styled(" · ", dim);

    let name = if app.status.model.is_empty() {
        "corust"
    } else {
        &app.status.model
    };

    let mut spans: Vec<Span> = vec![
        Span::styled(
            name.to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
    ];

    // Cwd
    let short_cwd = std::path::Path::new(&app.status.cwd)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| app.status.cwd.clone());
    spans.push(sep.clone());
    spans.push(Span::styled(short_cwd, Style::default().fg(Color::White)));

    // Git branch
    if let Some(branch) = &app.status.git_branch {
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!(" {branch}"),
            Style::default().fg(Color::Magenta),
        ));
    }

    // Turn count
    spans.push(sep.clone());
    spans.push(Span::styled(
        format!("{} turns", app.status.turn_count),
        dim,
    ));

    // Token usage
    if app.status.input_tokens > 0 || app.status.output_tokens > 0 {
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!(
                "{}↓ {}↑",
                format_tokens(app.status.input_tokens),
                format_tokens(app.status.output_tokens),
            ),
            dim,
        ));
    }

    // Context window
    if app.status.context_size > 0 {
        let pct = (app.status.context_used as f64 / app.status.context_size as f64 * 100.0) as u64;
        let ctx_color = if pct > 80 {
            Color::Red
        } else if pct > 60 {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        spans.push(sep.clone());
        spans.push(Span::styled(format!("ctx {pct}%"), Style::default().fg(ctx_color)));
    }

    // Cost
    if let Some((amount, ref currency)) = app.status.cost {
        spans.push(sep.clone());
        spans.push(Span::styled(format!("${amount:.4} {currency}"), dim));
    }

    // Mode indicator
    if app.busy {
        spans.push(Span::styled("  ", dim));
        spans.push(Span::styled(
            format!("{} thinking", app.spinner.frame()),
            Style::default().fg(Color::Yellow),
        ));
    } else if app.pending_permission.is_some() {
        spans.push(Span::styled("  ", dim));
        spans.push(Span::styled(
            "⚠ approval needed",
            Style::default().fg(Color::Yellow),
        ));
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

/// Format token counts in human-readable form (e.g. 1.2k, 45.3k).
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------
// Chat area (with pre-wrapping + proper scroll)
// ---------------------------------------------------------------------------

fn draw_chat(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let mut logical_lines: Vec<Line<'static>> = Vec::new();

    // Collect permission options before iterating blocks to avoid borrow conflict.
    let perm_options: Vec<(String, String)> = app
        .pending_permission
        .as_ref()
        .map(|p| {
            p.options
                .iter()
                .map(|o| (o.name.clone(), format!("{:?}", o.kind)))
                .collect()
        })
        .unwrap_or_default();

    for block in &app.blocks {
        render_block(block, &perm_options, &mut logical_lines);
        logical_lines.push(Line::from(""));
    }

    // Pre-wrap all lines for accurate scroll.
    let width = area.width as usize;
    let lines = wrap_all_lines(logical_lines, width);

    app.scroll
        .update_dimensions(lines.len() as u16, area.height);

    let chat = Paragraph::new(Text::from(lines))
        .scroll((app.scroll.offset(), 0));

    frame.render_widget(chat, area);
}

// ---------------------------------------------------------------------------
// Block rendering
// ---------------------------------------------------------------------------

fn render_block(block: &Block, perm_options: &[(String, String)], lines: &mut Vec<Line<'static>>) {
    match block {
        Block::UserInput { text } => {
            let user_style = Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD);
            for (i, line) in text.lines().enumerate() {
                let prefix = if i == 0 { "❯ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(prefix.to_string(), user_style),
                    Span::styled(line.to_string(), user_style),
                ]));
            }
        }

        Block::AgentText { content, streaming } => {
            let md_lines = render_markdown(content);
            lines.extend(md_lines);
            if *streaming {
                lines.push(Line::from(Span::styled(
                    "▍",
                    Style::default().fg(Color::Cyan),
                )));
            }
        }

        Block::CodeBlock { lang, code } => {
            // Standalone code blocks — render through markdown for consistent styling.
            let fenced = format!("```{lang}\n{code}\n```");
            let md_lines = render_markdown(&fenced);
            lines.extend(md_lines);
        }

        Block::Thinking { content, collapsed } => {
            if *collapsed {
                let line_count = content.lines().count();
                lines.push(Line::from(vec![
                    Span::styled(
                        "▶ [thinking] ",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                    Span::styled(
                        format!("({line_count} lines, Tab to expand)"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            } else {
                lines.push(Line::from(Span::styled(
                    "▼ [thinking]",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
                for line in content.lines() {
                    lines.push(Line::from(Span::styled(
                        line.to_string(),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
        }

        Block::ToolCall {
            title,
            status,
            locations,
            output,
            ..
        } => {
            lines.push(Line::from(vec![
                Span::styled("[tool] ", Style::default().fg(Color::Cyan)),
                Span::raw(title.to_string()),
                Span::styled(
                    format!(" ({status})"),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
            for loc in locations {
                lines.push(Line::from(Span::styled(
                    format!("  {loc}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            if let Some(out) = output {
                for line in out.lines().take(10) {
                    lines.push(Line::from(Span::styled(
                        format!("  {line}"),
                        Style::default().fg(Color::White),
                    )));
                }
                let total = out.lines().count();
                if total > 10 {
                    lines.push(Line::from(Span::styled(
                        format!("  … ({} more lines)", total - 10),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }

        Block::Diff { path, lines: diff_lines } => {
            lines.push(Line::from(vec![
                Span::styled("[edit] ".to_string(), Style::default().fg(Color::Cyan)),
                Span::styled(
                    path.to_string(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            for dl in diff_lines {
                match dl {
                    DiffLine::Add(text) => lines.push(Line::from(Span::styled(
                        format!("  + {text}"),
                        Style::default().fg(Color::Green),
                    ))),
                    DiffLine::Remove(text) => lines.push(Line::from(Span::styled(
                        format!("  - {text}"),
                        Style::default().fg(Color::Red),
                    ))),
                    DiffLine::Context(text) => lines.push(Line::from(Span::styled(
                        format!("    {text}"),
                        Style::default().fg(Color::DarkGray),
                    ))),
                }
            }
        }

        Block::PermissionRequest { title, resolved } => {
            if let Some(outcome) = resolved {
                lines.push(Line::from(vec![
                    Span::styled("[permission] ".to_string(), Style::default().fg(Color::Yellow)),
                    Span::raw(title.to_string()),
                    Span::styled(
                        format!(" → {outcome}"),
                        Style::default().fg(Color::Green),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        "[permission] ".to_string(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(title.to_string()),
                ]));
                for (i, (name, kind)) in perm_options.iter().enumerate() {
                    lines.push(Line::from(Span::styled(
                        format!("  [{i}] {name} ({kind})"),
                        Style::default().fg(Color::Yellow),
                    )));
                }
            }
        }

        Block::AgentQuestion { question, options } => {
            lines.push(Line::from(vec![
                Span::styled(
                    "[question] ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(question.to_string()),
            ]));
            for (i, opt) in options.iter().enumerate() {
                lines.push(Line::from(Span::styled(
                    format!("  [{i}] {opt}"),
                    Style::default().fg(Color::Magenta),
                )));
            }
        }

        Block::BackgroundTask { name, status, .. } => {
            let (icon, color) = match status {
                TaskStatus::Queued => ("◌", Color::DarkGray),
                TaskStatus::Running => ("◐", Color::Yellow),
                TaskStatus::Done => ("●", Color::Green),
                TaskStatus::Failed => ("✗", Color::Red),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{icon} "), Style::default().fg(color)),
                Span::styled("[task] ", Style::default().fg(Color::Cyan)),
                Span::raw(name.to_string()),
                Span::styled(format!(" ({status:?})"), Style::default().fg(color)),
            ]));
        }

        Block::Checkpoint { path, restored, .. } => {
            if *restored {
                lines.push(Line::from(Span::styled(
                    format!("[checkpoint] {path} (restored)"),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }

        Block::System { message } => {
            for line in message.lines() {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pre-wrapping (ported from corust-agent-rs)
// ---------------------------------------------------------------------------

fn wrap_all_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return lines;
    }
    let mut result = Vec::with_capacity(lines.len());
    for line in lines {
        if line.width() <= max_width {
            result.push(line);
        } else {
            wrap_line_into(&line, max_width, &mut result);
        }
    }
    result
}

fn wrap_line_into(line: &Line<'static>, max_width: usize, out: &mut Vec<Line<'static>>) {
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_width: usize = 0;
    let mut buf = String::new();
    let mut buf_style = Style::default();

    for span in &line.spans {
        let style = span.style;
        let text = span.content.as_ref();

        if !buf.is_empty() && buf_style != style {
            current_spans.push(Span::styled(std::mem::take(&mut buf), buf_style));
        }
        buf_style = style;

        for ch in text.chars() {
            let ch_width = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]) as &str);

            if current_width + ch_width > max_width
                && (!current_spans.is_empty() || !buf.is_empty())
            {
                if !buf.is_empty() {
                    current_spans.push(Span::styled(std::mem::take(&mut buf), buf_style));
                }
                out.push(Line::from(std::mem::take(&mut current_spans)));
                current_width = 0;
            }

            buf.push(ch);
            current_width += ch_width;
        }
    }

    if !buf.is_empty() {
        current_spans.push(Span::styled(buf, buf_style));
    }
    if !current_spans.is_empty() {
        out.push(Line::from(current_spans));
    }
}

// ---------------------------------------------------------------------------
// Input area (multiline + dynamic height + completion)
// ---------------------------------------------------------------------------

fn draw_input(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    // Approval mode: dedicated styled bar.
    if app.pending_permission.is_some() {
        draw_approval_input(frame, area);
        return;
    }

    let (title, border_style) = if app.busy {
        (
            "Waiting… (Ctrl+C to interrupt)",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        (
            "Enter ↵ send • Shift+Enter ↵ newline",
            Style::default().fg(Color::Cyan),
        )
    };

    // Build input text with optional completion ghost.
    let mut input_text = app.input.clone();
    let ghost = completion_ghost(app);
    if let Some(g) = &ghost {
        input_text.push_str(g);
    }

    let input_widget = Paragraph::new(input_text.as_str()).block(
        UiBlock::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title),
    );
    frame.render_widget(input_widget, area);

    // Cursor
    if !app.busy {
        let (row, col) = app.cursor_row_col();
        let cursor_x = area.x + 1 + col as u16;
        let cursor_y = area.y + 1 + row as u16;
        if cursor_y < area.y + area.height.saturating_sub(1) {
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

/// Dedicated approval input bar with styled y/n/a buttons.
fn draw_approval_input(frame: &mut Frame, area: ratatui::layout::Rect) {
    let input = Paragraph::new(Line::from(vec![
        Span::styled(
            " [y] ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("yes  "),
        Span::styled(
            " [n] ",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("no  "),
        Span::styled(
            " [a] ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("always  "),
        Span::styled(
            " [Esc] ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("cancel"),
    ]))
    .block(
        UiBlock::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title("⚠ Permission required"),
    );
    frame.render_widget(input, area);
}

fn completion_ghost(app: &App) -> Option<String> {
    if let Some(idx) = app.completion_idx {
        if let Some(cmd) = app.completions.get(idx) {
            if cmd.len() > app.input.len() {
                return Some(cmd[app.input.len()..].to_string());
            }
        }
    } else if app.completions.len() == 1 && app.completions[0].len() > app.input.len() {
        return Some(app.completions[0][app.input.len()..].to_string());
    }
    None
}
