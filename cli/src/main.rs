mod client;
mod connection;
mod error;
mod event;
mod session;
mod tui;

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use agent_client_protocol::{StopReason, ToolCallContent, ToolCallStatus};
use clap::Parser;
use futures::StreamExt;
use tokio::task::LocalSet;

use client::CliClient;
use connection::Connection;
use event::{Event, PermissionResponse};
use session::Session;

#[derive(Parser)]
#[command(
    name = "corust",
    about = "Corust CLI — an ACP client for the Corust agent"
)]
struct Cli {
    /// Working directory for the session.
    #[arg(short = 'C', long, default_value = ".")]
    project_dir: PathBuf,

    /// Path to the corust-agent-acp binary.
    /// Falls back to $CORUST_ACP_BIN, then sibling binary, then PATH.
    #[arg(long)]
    server_bin: Option<String>,

    /// Non-interactive mode: execute a single prompt and exit.
    #[arg(short, long)]
    exec: Option<String>,

    /// Launch the ratatui TUI instead of the line-based REPL.
    #[arg(long)]
    tui: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();

    let cli = Cli::parse();

    let project_dir = std::fs::canonicalize(&cli.project_dir)?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let local_set = LocalSet::new();

    rt.block_on(local_set.run_until(async move {
        let (event_tx, event_rx) = futures::channel::mpsc::unbounded();
        let acp_client = CliClient::new(event_tx);

        let conn = Connection::spawn(acp_client, cli.server_bin.as_deref()).await?;
        let (session, info) = Session::start(&conn, project_dir).await?;

        if cli.tui {
            tui::run(&conn, &session, event_rx).await?;
        } else {
            let agent_label = info.agent_name.as_deref().unwrap_or("agent");
            eprintln!(
                "corust session started ({}, {})",
                agent_label, info.session_id.0
            );

            if let Some(prompt) = cli.exec {
                run_single(&conn, &session, event_rx, &prompt).await?;
            } else {
                run_repl(&conn, &session, event_rx).await?;
            }
        }

        conn.shutdown().await;
        Ok::<(), Box<dyn std::error::Error>>(())
    }))
}

/// Execute a single prompt and exit.
async fn run_single(
    conn: &Connection,
    session: &Session,
    mut event_rx: futures::channel::mpsc::UnboundedReceiver<Event>,
    prompt: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let prompt_fut = session.prompt(conn, prompt);
    tokio::pin!(prompt_fut);

    let stop_reason = loop {
        tokio::select! {
            result = &mut prompt_fut => {
                drain_events(&mut event_rx);
                break result?;
            }
            event = event_rx.next() => {
                if let Some(event) = event {
                    handle_event(event);
                }
            }
        }
    };

    println!();
    if stop_reason != StopReason::EndTurn {
        eprintln!("turn ended: {stop_reason:?}");
    }
    Ok(())
}

/// Interactive REPL loop.
async fn run_repl(
    conn: &Connection,
    session: &Session,
    mut event_rx: futures::channel::mpsc::UnboundedReceiver<Event>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        eprint!("\n\x1b[1;32m>\x1b[0m ");
        io::stderr().flush()?;

        let line = tokio::task::spawn_blocking(|| {
            let stdin = io::stdin();
            let mut buf = String::new();
            match stdin.lock().read_line(&mut buf) {
                Ok(0) => None,
                Ok(_) => Some(buf),
                Err(_) => None,
            }
        })
        .await
        .unwrap_or(None);

        let Some(line) = line else { break };
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input == "/quit" || input == "/exit" {
            break;
        }

        let prompt_fut = session.prompt(conn, input);
        tokio::pin!(prompt_fut);

        let stop_reason = loop {
            tokio::select! {
                result = &mut prompt_fut => {
                    drain_events(&mut event_rx);
                    break result?;
                }
                event = event_rx.next() => {
                    if let Some(event) = event {
                        handle_event(event);
                    }
                }
            }
        };

        println!();
        if stop_reason != StopReason::EndTurn {
            eprintln!("turn ended: {stop_reason:?}");
        }
    }

    Ok(())
}

/// Drain remaining buffered events.
fn drain_events(rx: &mut futures::channel::mpsc::UnboundedReceiver<Event>) {
    while let Ok(event) = rx.try_recv() {
        handle_event(event);
    }
}

/// Render a single event to the terminal (simple text REPL, no TUI).
fn handle_event(event: Event) {
    match event {
        Event::AgentText(text) => {
            print!("{text}");
            let _ = io::stdout().flush();
        }
        Event::AgentThought(text) => {
            eprint!("\x1b[2m{text}\x1b[0m");
            let _ = io::stderr().flush();
        }
        Event::ToolCallStarted(tool_call) => {
            eprintln!(
                "\n\x1b[36m[{:?}] {}\x1b[0m",
                tool_call.kind, tool_call.title
            );
            // Show affected file locations
            for loc in &tool_call.locations {
                if let Some(line) = loc.line {
                    eprintln!(
                        "  \x1b[2m{}\x1b[0m:\x1b[33m{line}\x1b[0m",
                        loc.path.display()
                    );
                } else {
                    eprintln!("  \x1b[2m{}\x1b[0m", loc.path.display());
                }
            }
            // Show content (diffs, text, etc.)
            render_tool_content(&tool_call.content);
        }
        Event::ToolCallUpdated(update) => {
            let status = update.fields.status.unwrap_or(ToolCallStatus::InProgress);
            if let Some(title) = &update.fields.title {
                eprintln!("  \x1b[33m↳ {title} ({status:?})\x1b[0m");
            } else {
                eprintln!("  \x1b[33m↳ {status:?}\x1b[0m");
            }
            if let Some(content) = &update.fields.content {
                render_tool_content(content);
            }
        }
        Event::PermissionRequest {
            tool_call,
            options,
            respond,
            ..
        } => {
            eprintln!("\n\x1b[1;33m⚠ Permission requested\x1b[0m");
            if let Some(title) = &tool_call.fields.title {
                eprintln!("  {title}");
            }
            if let Some(content) = &tool_call.fields.content {
                render_tool_content(content);
            }
            for (i, opt) in options.iter().enumerate() {
                eprintln!("  [{i}] {} ({:?})", opt.name, opt.kind);
            }
            eprint!("  Choose [0]: ");
            let _ = io::stderr().flush();

            let mut choice = String::new();
            let _ = io::stdin().read_line(&mut choice);
            let idx: usize = choice.trim().parse().unwrap_or(0);

            if idx < options.len() {
                let _ = respond.send(PermissionResponse::Selected(idx));
            } else {
                let _ = respond.send(PermissionResponse::Cancelled);
            }
        }
        Event::SessionStarted {
            session_id,
            agent_name,
            ..
        } => {
            let label = agent_name.as_deref().unwrap_or("agent");
            eprintln!("\x1b[2m[session {label} {}]\x1b[0m", session_id.0);
        }
        Event::Error(msg) => {
            eprintln!("\x1b[1;31merror:\x1b[0m {msg}");
        }
    }
}

/// Render tool call content blocks (diffs, text, terminals).
fn render_tool_content(content: &[ToolCallContent]) {
    for item in content {
        match item {
            ToolCallContent::Diff(diff) => {
                eprintln!("  \x1b[1m--- {}\x1b[0m", diff.path.display());
                if let Some(old) = &diff.old_text {
                    for line in old.lines().take(5) {
                        eprintln!("  \x1b[31m- {line}\x1b[0m");
                    }
                }
                for line in diff.new_text.lines().take(5) {
                    eprintln!("  \x1b[32m+ {line}\x1b[0m");
                }
            }
            ToolCallContent::Content(c) => {
                if let agent_client_protocol::ContentBlock::Text(text) = &c.content {
                    for line in text.text.lines() {
                        eprintln!("  {line}");
                    }
                }
            }
            ToolCallContent::Terminal(_) => {
                eprintln!("  \x1b[2m(terminal output)\x1b[0m");
            }
            _ => {}
        }
    }
}
