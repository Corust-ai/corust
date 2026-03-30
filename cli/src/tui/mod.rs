pub mod app;
mod ui;

use std::future::Future;
use std::io;
use std::pin::Pin;

use crossterm::event::{Event as TermEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::channel::mpsc::UnboundedReceiver;
use futures::StreamExt;
use ratatui::DefaultTerminal;

use app::App;
use crate::connection::Connection;
use crate::error::CliError;
use crate::event::Event as AcpEvent;
use crate::session::Session;

type PromptFuture<'a> = Pin<Box<dyn Future<Output = Result<agent_client_protocol::StopReason, CliError>> + 'a>>;

/// Entry point for the TUI mode.
///
/// Takes ownership of the ACP connection handles and event stream.
pub async fn run(
    conn: &Connection,
    session: &Session,
    event_rx: UnboundedReceiver<AcpEvent>,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, conn, session, event_rx).await;
    ratatui::restore();
    result
}

/// Async event loop (TEA: Update cycle).
///
/// Uses `tokio::select!` to multiplex:
///   - Crossterm terminal events (keyboard)
///   - ACP events from the agent
///   - Prompt completion future (when busy)
async fn event_loop(
    terminal: &mut DefaultTerminal,
    conn: &Connection,
    session: &Session,
    mut event_rx: UnboundedReceiver<AcpEvent>,
) -> io::Result<()> {
    let mut app = App::new();
    let mut term_events = EventStream::new();

    // Holds the in-flight prompt future (if any).
    let mut prompt_fut: Option<PromptFuture<'_>> = None;

    loop {
        // View: render current state.
        terminal.draw(|frame| ui::draw(frame, &app))?;

        if app.should_quit {
            break;
        }

        tokio::select! {
            // Keyboard / terminal events.
            Some(Ok(term_event)) = term_events.next() => {
                if let TermEvent::Key(key) = term_event {
                    let submitted = handle_key(&mut app, key);

                    // If the user submitted text and we're not busy, start a prompt.
                    if let Some(text) = submitted {
                        if !app.busy {
                            app.busy = true;
                            prompt_fut = Some(Box::pin(async move {
                                session.prompt(conn, &text).await
                            }));
                        }
                    }
                }
            }

            // ACP events (agent text, tool calls, permissions, etc.).
            Some(acp_event) = event_rx.next() => {
                app.handle_acp_event(acp_event);
            }

            // Prompt completion.
            result = async {
                match prompt_fut.as_mut() {
                    Some(fut) => fut.await,
                    None => std::future::pending().await,
                }
            } => {
                prompt_fut = None;
                match result {
                    Ok(stop_reason) => {
                        app.turn_finished();
                        if stop_reason != agent_client_protocol::StopReason::EndTurn {
                            app.blocks.push(app::Block::System {
                                message: format!("Turn ended: {stop_reason:?}"),
                            });
                        }
                    }
                    Err(e) => {
                        app.turn_finished();
                        app.blocks.push(app::Block::System {
                            message: format!("Error: {e}"),
                        });
                    }
                }
            }
        }
    }

    Ok(())
}

/// Map key events to App mutations (TEA: Update).
///
/// Returns `Some(text)` if the user submitted input.
fn handle_key(app: &mut App, key: KeyEvent) -> Option<String> {
    // If a permission prompt is active, handle permission keys.
    if app.pending_permission.is_some() {
        match key.code {
            KeyCode::Char(c @ '0'..='9') => {
                let idx = (c as u8 - b'0') as usize;
                app.resolve_permission(idx);
            }
            KeyCode::Esc => app.cancel_permission(),
            _ => {}
        }
        return None;
    }

    match (key.modifiers, key.code) {
        // Quit
        (KeyModifiers::CONTROL, KeyCode::Char('c' | 'd')) => {
            app.should_quit = true;
            None
        }

        // Submit input
        (_, KeyCode::Enter) => app.submit_input(),

        // Text editing
        (_, KeyCode::Backspace) => { app.delete_char_before_cursor(); None }
        (_, KeyCode::Left) => { app.move_cursor_left(); None }
        (_, KeyCode::Right) => { app.move_cursor_right(); None }
        (_, KeyCode::Char(c)) => { app.insert_char(c); None }

        // Scroll
        (_, KeyCode::PageUp) => {
            app.scroll_offset = app.scroll_offset.saturating_add(5);
            None
        }
        (_, KeyCode::PageDown) => {
            app.scroll_offset = app.scroll_offset.saturating_sub(5);
            None
        }

        _ => None,
    }
}
