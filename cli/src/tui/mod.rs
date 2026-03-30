pub mod app;
mod markdown;
mod syntax;
mod ui;

use std::future::Future;
use std::io;
use std::pin::Pin;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as TermEvent, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, supports_keyboard_enhancement,
};
use futures::channel::mpsc::UnboundedReceiver;
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::App;
use crate::connection::Connection;
use crate::error::CliError;
use crate::event::Event as AcpEvent;
use crate::session::Session;

type PromptFuture<'a> = Pin<
    Box<
        dyn Future<
                Output = Result<
                    (agent_client_protocol::StopReason, Option<agent_client_protocol::Usage>),
                    CliError,
                >,
            > + 'a,
    >,
>;

// ---------------------------------------------------------------------------
// Terminal RAII guard
// ---------------------------------------------------------------------------

struct TerminalGuard {
    enhanced_keys: bool,
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let mut stdout = io::stdout();
        if self.enhanced_keys {
            let _ = crossterm::execute!(
                stdout,
                LeaveAlternateScreen,
                DisableMouseCapture,
                PopKeyboardEnhancementFlags
            );
        } else {
            let _ = crossterm::execute!(stdout, LeaveAlternateScreen, DisableMouseCapture);
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run(
    conn: &Connection,
    session: &Session,
    event_rx: UnboundedReceiver<AcpEvent>,
) -> io::Result<()> {
    // Setup terminal with mouse + enhanced keyboard.
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();

    let enhanced_keys = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced_keys {
        crossterm::execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    } else {
        crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let _guard = TerminalGuard { enhanced_keys };

    let result = event_loop(&mut terminal, conn, session, event_rx).await;

    // Disarm guard — explicit cleanup.
    std::mem::forget(_guard);

    crossterm::terminal::disable_raw_mode()?;
    if enhanced_keys {
        crossterm::execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            PopKeyboardEnhancementFlags
        )?;
    } else {
        crossterm::execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
    }
    terminal.show_cursor()?;

    result
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    conn: &Connection,
    session: &Session,
    mut event_rx: UnboundedReceiver<AcpEvent>,
) -> io::Result<()> {
    let mut app = App::new();
    let mut term_events = EventStream::new();
    let mut prompt_fut: Option<PromptFuture<'_>> = None;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(80));

    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if app.should_quit {
            break;
        }

        tokio::select! {
            Some(Ok(term_event)) = term_events.next() => {
                // Scroll events (mouse, PageUp/Down, Shift+arrows).
                if handle_scroll(&term_event, &mut app) {
                    continue;
                }

                if let TermEvent::Key(key) = term_event {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match handle_key(&mut app, key) {
                        KeyAction::Submit(text) if !app.busy => {
                            app.busy = true;
                            app.spinner.reset();
                            app.has_received_delta = false;
                            prompt_fut = Some(Box::pin(async move {
                                session.prompt(conn, &text).await
                            }));
                        }
                        KeyAction::CancelTurn => {
                            prompt_fut = None;
                            app.turn_finished(None);
                            app.blocks.push(app::Block::System {
                                message: "Cancelled.".into(),
                            });
                        }
                        _ => {}
                    }
                }
            }

            Some(acp_event) = event_rx.next() => {
                app.handle_acp_event(acp_event);
            }

            result = async {
                match prompt_fut.as_mut() {
                    Some(fut) => fut.await,
                    None => std::future::pending().await,
                }
            } => {
                prompt_fut = None;
                match result {
                    Ok((stop_reason, usage)) => {
                        app.turn_finished(usage);
                        if stop_reason != agent_client_protocol::StopReason::EndTurn {
                            app.blocks.push(app::Block::System {
                                message: format!("Turn ended: {stop_reason:?}"),
                            });
                        }
                    }
                    Err(e) => {
                        app.turn_finished(None);
                        app.blocks.push(app::Block::System {
                            message: format!("Error: {e}"),
                        });
                    }
                }
            }

            // Tick drives spinner animation.
            _ = tick.tick() => {}
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Scroll handling (mouse + keyboard)
// ---------------------------------------------------------------------------

fn handle_scroll(event: &TermEvent, app: &mut App) -> bool {
    match event {
        TermEvent::Key(KeyEvent {
            code: KeyCode::PageUp,
            ..
        }) => {
            app.scroll.scroll_up(10);
            true
        }
        TermEvent::Key(KeyEvent {
            code: KeyCode::PageDown,
            ..
        }) => {
            app.scroll.scroll_down(10);
            true
        }
        TermEvent::Key(KeyEvent {
            code: KeyCode::Up,
            modifiers: KeyModifiers::SHIFT,
            ..
        }) => {
            app.scroll.scroll_up(1);
            true
        }
        TermEvent::Key(KeyEvent {
            code: KeyCode::Down,
            modifiers: KeyModifiers::SHIFT,
            ..
        }) => {
            app.scroll.scroll_down(1);
            true
        }
        TermEvent::Mouse(me) => match me.kind {
            MouseEventKind::ScrollUp => {
                app.scroll.scroll_up(3);
                true
            }
            MouseEventKind::ScrollDown => {
                app.scroll.scroll_down(3);
                true
            }
            _ => false,
        },
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

enum KeyAction {
    None,
    Submit(String),
    CancelTurn,
}

fn handle_key(app: &mut App, key: KeyEvent) -> KeyAction {
    // Permission mode: y/n/a or digit keys.
    if app.pending_permission.is_some() {
        match key.code {
            KeyCode::Char('y') => app.resolve_permission_key('y'),
            KeyCode::Char('a') => app.resolve_permission_key('a'),
            KeyCode::Char('n') => app.resolve_permission_key('n'),
            KeyCode::Char(c @ '0'..='9') => {
                app.resolve_permission((c as u8 - b'0') as usize);
            }
            KeyCode::Esc => app.cancel_permission(),
            _ => {}
        }
        return KeyAction::None;
    }

    match (key.modifiers, key.code) {
        // Quit / Cancel
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            if app.busy {
                KeyAction::CancelTurn
            } else {
                app.should_quit = true;
                KeyAction::None
            }
        }
        (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
            app.should_quit = true;
            KeyAction::None
        }

        // Ctrl+U: clear input
        (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
            app.clear_input();
            app.update_completions();
            KeyAction::None
        }

        // Ctrl+Y: copy last code block
        (KeyModifiers::CONTROL, KeyCode::Char('y')) => {
            app.copy_last_code_block();
            KeyAction::None
        }

        // Tab: slash completion or thinking toggle
        (_, KeyCode::Tab) => {
            if !app.completions.is_empty() {
                app.cycle_completion();
            } else {
                app.toggle_thinking();
            }
            KeyAction::None
        }

        // Multiline: Shift+Enter or Ctrl+J inserts newline
        (KeyModifiers::SHIFT, KeyCode::Enter) => {
            app.insert_newline();
            KeyAction::None
        }
        (KeyModifiers::CONTROL, KeyCode::Char('j')) => {
            app.insert_newline();
            KeyAction::None
        }

        // Enter: submit
        (_, KeyCode::Enter) => {
            if app.input.starts_with('/') {
                if app.handle_slash_command().is_some() {
                    return KeyAction::None;
                }
            }
            match app.submit_input() {
                Some(text) => KeyAction::Submit(text),
                None => KeyAction::None,
            }
        }

        // Input history (only for single-line input — Up/Down in multiline moves cursor)
        (_, KeyCode::Up) if app.input_line_count() <= 1 => {
            app.history_prev();
            KeyAction::None
        }
        (_, KeyCode::Down) if app.input_line_count() <= 1 => {
            app.history_next();
            KeyAction::None
        }

        // Cursor movement in multiline
        (_, KeyCode::Up) => { app.cursor_up(); KeyAction::None }
        (_, KeyCode::Down) => { app.cursor_down(); KeyAction::None }

        // Editing
        (_, KeyCode::Backspace) => {
            app.backspace();
            app.update_completions();
            KeyAction::None
        }
        (_, KeyCode::Delete) => {
            app.delete_at_cursor();
            KeyAction::None
        }
        (_, KeyCode::Left) => { app.cursor_left(); KeyAction::None }
        (_, KeyCode::Right) => { app.cursor_right(); KeyAction::None }
        (_, KeyCode::Home) => { app.cursor_home(); KeyAction::None }
        (_, KeyCode::End) => { app.cursor_end(); KeyAction::None }

        (_, KeyCode::Char(c)) => {
            app.insert_char(c);
            app.update_completions();
            KeyAction::None
        }

        _ => KeyAction::None,
    }
}
