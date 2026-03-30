use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as UiBlock, Borders, Paragraph, Wrap};

use super::app::{App, Block, DiffLine};

/// Render the full UI (TEA: View).
///
/// Three zones top-to-bottom:
///   1. Status bar  (1 line)
///   2. Scroll area (flex)
///   3. Input bar   (3 lines)
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1),  // status bar
        Constraint::Min(1),    // scroll area
        Constraint::Length(3), // input bar
    ])
    .split(frame.area());

    draw_status_bar(frame, app, chunks[0]);
    draw_scroll_area(frame, app, chunks[1]);
    draw_input_bar(frame, app, chunks[2]);
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let mut spans = vec![];

    // Busy indicator
    if app.busy {
        spans.push(Span::styled(
            "● ",
            Style::default().fg(Color::Yellow),
        ));
    } else {
        spans.push(Span::styled(
            "○ ",
            Style::default().fg(Color::Green),
        ));
    }

    if !app.status.model.is_empty() {
        spans.push(Span::styled(
            &app.status.model,
            Style::default().fg(Color::Cyan),
        ));
        spans.push(Span::raw(" │ "));
    }

    spans.push(Span::styled(
        &app.status.cwd,
        Style::default().fg(Color::Blue),
    ));

    if let Some(branch) = &app.status.git_branch {
        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(
            branch,
            Style::default().fg(Color::Magenta),
        ));
    }

    let bar = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(Color::DarkGray));
    frame.render_widget(bar, area);
}

// ---------------------------------------------------------------------------
// Scroll area
// ---------------------------------------------------------------------------

fn draw_scroll_area(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let mut lines: Vec<Line> = Vec::new();

    for block in &app.blocks {
        render_block(block, app, &mut lines);
        lines.push(Line::from("")); // blank separator
    }

    // Auto-scroll to bottom by default.
    let visible_height = area.height as usize;
    let total = lines.len();
    let scroll = if app.scroll_offset == 0 {
        total.saturating_sub(visible_height)
    } else {
        total
            .saturating_sub(visible_height)
            .saturating_sub(app.scroll_offset as usize)
    };

    let paragraph = Paragraph::new(lines)
        .scroll((scroll as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Render a single Block into lines.
fn render_block<'a>(block: &'a Block, app: &'a App, lines: &mut Vec<Line<'a>>) {
    match block {
        Block::UserInput { text } => {
            lines.push(Line::from(vec![
                Span::styled(
                    "> ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(text.as_str()),
            ]));
        }

        Block::AgentText { content, streaming } => {
            for line in content.lines() {
                lines.push(Line::from(Span::raw(line)));
            }
            if *streaming {
                lines.push(Line::from(Span::styled(
                    "▍",
                    Style::default().fg(Color::Cyan),
                )));
            }
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
                        line,
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
                Span::raw(title.as_str()),
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
                let total_lines = out.lines().count();
                if total_lines > 10 {
                    lines.push(Line::from(Span::styled(
                        format!("  ... ({} more lines)", total_lines - 10),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }

        Block::Diff { path, lines: diff_lines } => {
            lines.push(Line::from(vec![
                Span::styled("[edit] ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    path.as_str(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            for dl in diff_lines {
                match dl {
                    DiffLine::Add(text) => {
                        lines.push(Line::from(Span::styled(
                            format!("  + {text}"),
                            Style::default().fg(Color::Green),
                        )));
                    }
                    DiffLine::Remove(text) => {
                        lines.push(Line::from(Span::styled(
                            format!("  - {text}"),
                            Style::default().fg(Color::Red),
                        )));
                    }
                    DiffLine::Context(text) => {
                        lines.push(Line::from(Span::styled(
                            format!("    {text}"),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
            }
        }

        Block::PermissionRequest { title, resolved } => {
            if let Some(outcome) = resolved {
                lines.push(Line::from(vec![
                    Span::styled("[permission] ", Style::default().fg(Color::Yellow)),
                    Span::raw(title.as_str()),
                    Span::styled(
                        format!(" -> {outcome}"),
                        Style::default().fg(Color::Green),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        "[permission] ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(title.as_str()),
                ]));
                if let Some(perm) = &app.pending_permission {
                    for (i, opt) in perm.options.iter().enumerate() {
                        lines.push(Line::from(Span::styled(
                            format!("  [{}] {} ({:?})", i, opt.name, opt.kind),
                            Style::default().fg(Color::Yellow),
                        )));
                    }
                }
            }
        }

        Block::System { message } => {
            lines.push(Line::from(Span::styled(
                message.as_str(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
    }
}

// ---------------------------------------------------------------------------
// Input bar
// ---------------------------------------------------------------------------

fn draw_input_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let prefix = if app.busy { "… " } else { "> " };
    let input_line = Line::from(vec![
        Span::styled(
            prefix,
            Style::default()
                .fg(if app.busy { Color::Yellow } else { Color::Green })
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(&app.input),
    ]);

    let input_widget = Paragraph::new(input_line)
        .block(UiBlock::default().borders(Borders::TOP));

    frame.render_widget(input_widget, area);

    // Place the visible cursor (only when not busy).
    if !app.busy {
        let cursor_x = area.x + 2 + app.input[..app.cursor].chars().count() as u16;
        let cursor_y = area.y + 1; // +1 for the TOP border
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}
