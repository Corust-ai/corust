use std::time::Instant;

use agent_client_protocol::{PermissionOption, ToolCallContent, ToolCallId};
use futures::channel::oneshot;
use unicode_width::UnicodeWidthStr;

use crate::event::{Event, PermissionResponse};

// ---------------------------------------------------------------------------
// Spinner
// ---------------------------------------------------------------------------

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct Spinner {
    start: Instant,
}

impl Spinner {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn reset(&mut self) {
        self.start = Instant::now();
    }

    pub fn frame(&self) -> &'static str {
        let elapsed_ms = self.start.elapsed().as_millis() as usize;
        let idx = (elapsed_ms / 80) % SPINNER_FRAMES.len();
        SPINNER_FRAMES[idx]
    }
}

// ---------------------------------------------------------------------------
// Scroll state
// ---------------------------------------------------------------------------

pub struct ScrollState {
    offset: u16,
    content_height: u16,
    viewport_height: u16,
    pub pending_auto_scroll: bool,
}

impl ScrollState {
    fn new() -> Self {
        Self {
            offset: 0,
            content_height: 0,
            viewport_height: 0,
            pending_auto_scroll: true,
        }
    }

    pub fn update_dimensions(&mut self, content_height: u16, viewport_height: u16) {
        self.content_height = content_height;
        self.viewport_height = viewport_height;

        if self.pending_auto_scroll {
            self.offset = self.max_offset();
            self.pending_auto_scroll = false;
        }
        self.clamp();
    }

    pub fn offset(&self) -> u16 {
        self.offset
    }

    pub fn scroll_up(&mut self, n: u16) {
        self.offset = self.offset.saturating_sub(n);
        self.pending_auto_scroll = false;
    }

    pub fn scroll_down(&mut self, n: u16) {
        self.offset = self.offset.saturating_add(n);
        self.clamp();
        self.pending_auto_scroll = false;
    }

    pub fn request_auto_scroll(&mut self) {
        self.pending_auto_scroll = true;
    }

    fn max_offset(&self) -> u16 {
        self.content_height.saturating_sub(self.viewport_height)
    }

    fn clamp(&mut self) {
        self.offset = self.offset.min(self.max_offset());
    }
}

// ---------------------------------------------------------------------------
// Block model
// ---------------------------------------------------------------------------

/// A single visual unit in the conversation scroll area.
#[allow(dead_code)]
pub enum Block {
    UserInput { text: String },
    AgentText { content: String, streaming: bool },
    Thinking { content: String, collapsed: bool },
    ToolCall {
        id: ToolCallId,
        title: String,
        status: String,
        locations: Vec<String>,
        output: Option<String>,
    },
    CodeBlock { lang: String, code: String },
    Diff { path: String, lines: Vec<DiffLine> },
    System { message: String },
    PermissionRequest { title: String, resolved: Option<String> },
    AgentQuestion { question: String, options: Vec<String> },
    BackgroundTask { id: String, name: String, status: TaskStatus },
    Checkpoint { path: String, content: String, restored: bool },
}

#[allow(dead_code)]
pub enum DiffLine {
    Context(String),
    Add(String),
    Remove(String),
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum TaskStatus {
    Queued,
    Running,
    Done,
    Failed,
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

pub struct PendingPermission {
    pub options: Vec<PermissionOption>,
    pub respond: oneshot::Sender<PermissionResponse>,
}

pub struct StatusBar {
    pub model: String,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub turn_count: usize,
    /// Cumulative token usage across the session.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Context window: tokens used / total size.
    pub context_used: u64,
    pub context_size: u64,
    /// Cumulative cost.
    pub cost: Option<(f64, String)>,
}

/// Result of a slash command.
pub enum SlashResult {
    Handled,
}

/// Built-in slash commands.
pub const SLASH_COMMANDS: &[&str] = &[
    "/clear", "/exit", "/help", "/model", "/quit", "/status", "/undo",
];

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    // Input
    pub input: String,
    pub input_cursor: usize,

    // Conversation
    pub blocks: Vec<Block>,
    pub scroll: ScrollState,

    // State
    pub should_quit: bool,
    pub busy: bool,
    pub status: StatusBar,
    pub spinner: Spinner,
    pub pending_permission: Option<PendingPermission>,

    // History
    pub history: Vec<String>,
    pub history_cursor: Option<usize>,
    pub history_stash: String,

    // Slash completion
    pub completions: Vec<&'static str>,
    pub completion_idx: Option<usize>,

    /// Whether streaming deltas were received for the current turn.
    /// Used to avoid appending a duplicate full AgentMessage after streaming.
    pub has_received_delta: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            input_cursor: 0,
            blocks: vec![Block::System {
                message: "Welcome to corust. Type a message and press Enter.".into(),
            }],
            scroll: ScrollState::new(),
            should_quit: false,
            busy: false,
            status: StatusBar {
                model: String::new(),
                cwd: std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                git_branch: detect_git_branch(),
                turn_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                context_used: 0,
                context_size: 0,
                cost: None,
            },
            spinner: Spinner::new(),
            pending_permission: None,
            history: Vec::new(),
            history_cursor: None,
            history_stash: String::new(),
            completions: Vec::new(),
            completion_idx: None,
            has_received_delta: false,
        }
    }

    // -----------------------------------------------------------------------
    // ACP event handling
    // -----------------------------------------------------------------------

    pub fn handle_acp_event(&mut self, event: Event) {
        match event {
            Event::AgentText(text) => {
                self.has_received_delta = true;
                if let Some(Block::AgentText { content, .. }) = self.blocks.last_mut() {
                    content.push_str(&text);
                } else {
                    self.blocks.push(Block::AgentText {
                        content: text,
                        streaming: true,
                    });
                }
                self.scroll.request_auto_scroll();
            }
            Event::AgentThought(text) => {
                if let Some(Block::Thinking { content, .. }) = self.blocks.last_mut() {
                    content.push_str(&text);
                } else {
                    self.blocks.push(Block::Thinking {
                        content: text,
                        collapsed: false,
                    });
                }
                self.scroll.request_auto_scroll();
            }
            Event::ToolCallStarted(tool_call) => {
                let locations: Vec<String> = tool_call
                    .locations
                    .iter()
                    .map(|loc| {
                        if let Some(line) = loc.line {
                            format!("{}:{line}", loc.path.display())
                        } else {
                            loc.path.display().to_string()
                        }
                    })
                    .collect();
                let output = extract_text_content(&tool_call.content);
                self.blocks.push(Block::ToolCall {
                    id: tool_call.tool_call_id.clone(),
                    title: tool_call.title.clone(),
                    status: format!("{:?}", tool_call.status),
                    locations,
                    output,
                });
                extract_diff_blocks(&tool_call.content, &mut self.blocks);
                self.scroll.request_auto_scroll();
            }
            Event::ToolCallUpdated(update) => {
                let target_id = &update.tool_call_id;
                let tool_block = self.blocks.iter_mut().rev().find(|b| {
                    matches!(b, Block::ToolCall { id, .. } if id == target_id)
                });
                if let Some(Block::ToolCall {
                    title,
                    status,
                    output,
                    ..
                }) = tool_block
                {
                    if let Some(t) = &update.fields.title {
                        *title = t.clone();
                    }
                    if let Some(s) = &update.fields.status {
                        *status = format!("{s:?}");
                    }
                    if let Some(content) = &update.fields.content {
                        if let Some(text) = extract_text_content(content) {
                            *output = Some(text);
                        }
                        extract_diff_blocks(content, &mut self.blocks);
                    }
                }
                self.scroll.request_auto_scroll();
            }
            Event::PermissionRequest {
                tool_call,
                options,
                respond,
                ..
            } => {
                let title = tool_call
                    .fields
                    .title
                    .clone()
                    .unwrap_or_else(|| "Permission requested".into());
                self.blocks.push(Block::PermissionRequest {
                    title,
                    resolved: None,
                });
                self.pending_permission = Some(PendingPermission { options, respond });
                self.scroll.request_auto_scroll();
            }
            Event::SessionStarted {
                agent_name,
                session_id,
                ..
            } => {
                let label = agent_name.as_deref().unwrap_or("agent");
                self.status.model = label.to_string();
                self.blocks.push(Block::System {
                    message: format!("Session started: {label} ({})", session_id.0),
                });
                self.scroll.request_auto_scroll();
            }
            Event::UsageUpdate(usage) => {
                self.status.context_used = usage.used;
                self.status.context_size = usage.size;
                if let Some(cost) = usage.cost {
                    self.status.cost = Some((cost.amount, cost.currency));
                }
            }
            Event::Error(msg) => {
                self.blocks.push(Block::System {
                    message: format!("Error: {msg}"),
                });
                self.scroll.request_auto_scroll();
            }
        }
    }

    // -----------------------------------------------------------------------
    // Permission
    // -----------------------------------------------------------------------

    pub fn resolve_permission(&mut self, idx: usize) {
        if let Some(perm) = self.pending_permission.take() {
            let label = perm
                .options
                .get(idx)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| "cancelled".into());
            for block in self.blocks.iter_mut().rev() {
                if let Block::PermissionRequest { resolved, .. } = block {
                    *resolved = Some(label.clone());
                    break;
                }
            }
            if idx < perm.options.len() {
                let _ = perm.respond.send(PermissionResponse::Selected(idx));
            } else {
                let _ = perm.respond.send(PermissionResponse::Cancelled);
            }
        }
    }

    /// Resolve permission by shortcut key: y = first option, a = second (if exists), n = cancel.
    pub fn resolve_permission_key(&mut self, key: char) {
        match key {
            'y' => self.resolve_permission(0),
            'a' => {
                // "always" is typically the second option.
                let idx = if self
                    .pending_permission
                    .as_ref()
                    .is_some_and(|p| p.options.len() > 1)
                {
                    1
                } else {
                    0
                };
                self.resolve_permission(idx);
            }
            'n' | _ => self.cancel_permission(),
        }
    }

    pub fn cancel_permission(&mut self) {
        if let Some(perm) = self.pending_permission.take() {
            for block in self.blocks.iter_mut().rev() {
                if let Block::PermissionRequest { resolved, .. } = block {
                    *resolved = Some("denied".into());
                    break;
                }
            }
            let _ = perm.respond.send(PermissionResponse::Cancelled);
        }
    }

    // -----------------------------------------------------------------------
    // Turn lifecycle
    // -----------------------------------------------------------------------

    pub fn turn_finished(&mut self, usage: Option<agent_client_protocol::Usage>) {
        self.busy = false;
        self.has_received_delta = false;
        self.status.turn_count += 1;
        if let Some(u) = usage {
            self.status.input_tokens = u.input_tokens;
            self.status.output_tokens = u.output_tokens;
        }
        for block in self.blocks.iter_mut().rev() {
            if let Block::AgentText { streaming, .. } = block {
                *streaming = false;
                break;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Thinking toggle
    // -----------------------------------------------------------------------

    pub fn toggle_thinking(&mut self) {
        for block in self.blocks.iter_mut().rev() {
            if let Block::Thinking { collapsed, .. } = block {
                *collapsed = !*collapsed;
                break;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Multi-line input (ported from corust-agent-rs)
    // -----------------------------------------------------------------------

    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.input_cursor, c);
        self.input_cursor += c.len_utf8();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.input_cursor > 0 {
            let prev = self.input[..self.input_cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.drain(prev..self.input_cursor);
            self.input_cursor = prev;
        }
    }

    pub fn delete_at_cursor(&mut self) {
        if self.input_cursor < self.input.len() {
            let next = self.input[self.input_cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.input_cursor + i)
                .unwrap_or(self.input.len());
            self.input.drain(self.input_cursor..next);
        }
    }

    pub fn cursor_left(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor = self.input[..self.input_cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    pub fn cursor_right(&mut self) {
        if self.input_cursor < self.input.len() {
            self.input_cursor = self.input[self.input_cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.input_cursor + i)
                .unwrap_or(self.input.len());
        }
    }

    pub fn cursor_home(&mut self) {
        let line_start = self.input[..self.input_cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        self.input_cursor = line_start;
    }

    pub fn cursor_end(&mut self) {
        let line_end = self.input[self.input_cursor..]
            .find('\n')
            .map(|i| self.input_cursor + i)
            .unwrap_or(self.input.len());
        self.input_cursor = line_end;
    }

    pub fn cursor_up(&mut self) {
        let (row, col) = self.cursor_row_col();
        if row > 0 {
            self.set_cursor_row_col(row - 1, col);
        }
    }

    pub fn cursor_down(&mut self) {
        let (row, col) = self.cursor_row_col();
        let line_count = self.input_line_count();
        if row + 1 < line_count {
            self.set_cursor_row_col(row + 1, col);
        }
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
    }

    pub fn input_line_count(&self) -> usize {
        self.input.split('\n').count().max(1)
    }

    /// (row, col) from byte cursor — col in display-width units.
    pub fn cursor_row_col(&self) -> (usize, usize) {
        let before = &self.input[..self.input_cursor];
        let row = before.matches('\n').count();
        let last_line = before.rsplit('\n').next().unwrap_or(before);
        let col = UnicodeWidthStr::width(last_line);
        (row, col)
    }

    fn set_cursor_row_col(&mut self, target_row: usize, target_col: usize) {
        let mut offset = 0;
        for (i, line) in self.input.split('\n').enumerate() {
            if i == target_row {
                let mut col_width = 0;
                let mut byte_col = 0;
                for ch in line.chars() {
                    let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if col_width + w > target_col {
                        break;
                    }
                    col_width += w;
                    byte_col += ch.len_utf8();
                }
                self.input_cursor = offset + byte_col;
                return;
            }
            offset += line.len() + 1;
        }
    }

    // -----------------------------------------------------------------------
    // Submit
    // -----------------------------------------------------------------------

    pub fn submit_input(&mut self) -> Option<String> {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.history.push(text.clone());
        self.history_cursor = None;
        self.history_stash.clear();
        self.blocks.push(Block::UserInput { text: text.clone() });
        self.input.clear();
        self.input_cursor = 0;
        self.scroll.request_auto_scroll();
        Some(text)
    }

    // -----------------------------------------------------------------------
    // History
    // -----------------------------------------------------------------------

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_cursor {
            None => {
                self.history_stash = self.input.clone();
                self.history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.history_cursor = Some(idx);
        self.input = self.history[idx].clone();
        self.input_cursor = self.input.len();
    }

    pub fn history_next(&mut self) {
        let Some(idx) = self.history_cursor else { return };
        if idx + 1 >= self.history.len() {
            self.history_cursor = None;
            self.input = std::mem::take(&mut self.history_stash);
        } else {
            self.history_cursor = Some(idx + 1);
            self.input = self.history[idx + 1].clone();
        }
        self.input_cursor = self.input.len();
    }

    // -----------------------------------------------------------------------
    // Slash commands
    // -----------------------------------------------------------------------

    pub fn update_completions(&mut self) {
        if self.input.starts_with('/') && !self.input.contains(' ') {
            let prefix = &self.input;
            self.completions = SLASH_COMMANDS
                .iter()
                .filter(|cmd| cmd.starts_with(prefix))
                .copied()
                .collect();
            if self.completions.is_empty() {
                self.completion_idx = None;
            }
        } else {
            self.completions.clear();
            self.completion_idx = None;
        }
    }

    pub fn cycle_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let idx = match self.completion_idx {
            None => 0,
            Some(i) => (i + 1) % self.completions.len(),
        };
        self.completion_idx = Some(idx);
        self.input = self.completions[idx].to_string();
        self.input_cursor = self.input.len();
    }

    pub fn handle_slash_command(&mut self) -> Option<SlashResult> {
        let raw = self.input.trim().to_string();
        let (cmd, args) = match raw[1..].split_once(' ') {
            Some((c, a)) => (c, a.trim()),
            None => (&raw[1..], ""),
        };

        let result = match cmd {
            "quit" | "exit" | "q" => {
                self.should_quit = true;
                Some(SlashResult::Handled)
            }
            "clear" => {
                self.blocks.clear();
                self.blocks.push(Block::System {
                    message: "Cleared.".into(),
                });
                Some(SlashResult::Handled)
            }
            "undo" => Some(self.undo_last_edit()),
            "help" | "h" | "?" => {
                self.blocks.push(Block::System {
                    message: "/help     Show this help\n\
                              /model    Show or switch model (/model <id>)\n\
                              /status   Session info\n\
                              /clear    Clear conversation\n\
                              /undo     Undo last file edit\n\
                              /quit     Exit"
                        .into(),
                });
                self.scroll.request_auto_scroll();
                Some(SlashResult::Handled)
            }
            "status" => {
                let model = if self.status.model.is_empty() {
                    "-"
                } else {
                    &self.status.model
                };
                let branch = self
                    .status
                    .git_branch
                    .as_deref()
                    .unwrap_or("-");
                let mut info = format!(
                    "Model: {model}\n\
                     Turns: {}\n\
                     Path:  {}\n\
                     Git:   {branch}",
                    self.status.turn_count, self.status.cwd,
                );
                if self.status.input_tokens > 0 {
                    info.push_str(&format!(
                        "\nTokens: {} in / {} out",
                        self.status.input_tokens, self.status.output_tokens,
                    ));
                }
                if self.status.context_size > 0 {
                    info.push_str(&format!(
                        "\nContext: {} / {} tokens",
                        self.status.context_used, self.status.context_size,
                    ));
                }
                if let Some((amount, ref currency)) = self.status.cost {
                    info.push_str(&format!("\nCost: ${amount:.4} {currency}"));
                }
                self.blocks.push(Block::System { message: info });
                self.scroll.request_auto_scroll();
                Some(SlashResult::Handled)
            }
            "model" => {
                if args.is_empty() {
                    let model = if self.status.model.is_empty() {
                        "-"
                    } else {
                        &self.status.model
                    };
                    self.blocks.push(Block::System {
                        message: format!("Current model: {model}\nUsage: /model <model-id>"),
                    });
                } else {
                    // Model switching would need ACP support; store for now.
                    self.status.model = args.to_string();
                    self.blocks.push(Block::System {
                        message: format!("Model set to: {args}"),
                    });
                }
                self.scroll.request_auto_scroll();
                Some(SlashResult::Handled)
            }
            _ => None,
        };
        if result.is_some() {
            self.input.clear();
            self.input_cursor = 0;
            self.completions.clear();
            self.completion_idx = None;
        }
        result
    }

    fn undo_last_edit(&mut self) -> SlashResult {
        for block in self.blocks.iter_mut().rev() {
            if let Block::Checkpoint {
                path,
                content,
                restored,
            } = block
            {
                if *restored {
                    continue;
                }
                let file_path = path.clone();
                let file_content = content.clone();
                *restored = true;
                match std::fs::write(&file_path, &file_content) {
                    Ok(()) => {
                        self.blocks.push(Block::System {
                            message: format!("Restored: {file_path}"),
                        });
                        return SlashResult::Handled;
                    }
                    Err(e) => {
                        self.blocks.push(Block::System {
                            message: format!("Undo failed ({file_path}): {e}"),
                        });
                        return SlashResult::Handled;
                    }
                }
            }
        }
        self.blocks.push(Block::System {
            message: "Nothing to undo.".into(),
        });
        SlashResult::Handled
    }

    // -----------------------------------------------------------------------
    // Clipboard
    // -----------------------------------------------------------------------

    pub fn copy_last_code_block(&mut self) {
        let code = self.blocks.iter().rev().find_map(|b| {
            if let Block::CodeBlock { code, .. } = b {
                Some(code.clone())
            } else {
                None
            }
        });
        match code {
            Some(text) => match copy_to_clipboard(&text) {
                Ok(()) => self.blocks.push(Block::System {
                    message: "Copied to clipboard.".into(),
                }),
                Err(e) => self.blocks.push(Block::System {
                    message: format!("Clipboard error: {e}"),
                }),
            },
            None => {
                self.blocks.push(Block::System {
                    message: "No code block to copy.".into(),
                });
            }
        }
        self.scroll.request_auto_scroll();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
        return Ok(());
    }
    if let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
        return Ok(());
    }
    Err("no clipboard tool found (pbcopy/xclip)".into())
}

fn detect_git_branch() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn extract_text_content(content: &[ToolCallContent]) -> Option<String> {
    let mut text = String::new();
    for item in content {
        if let ToolCallContent::Content(c) = item {
            if let agent_client_protocol::ContentBlock::Text(t) = &c.content {
                text.push_str(&t.text);
            }
        }
    }
    if text.is_empty() { None } else { Some(text) }
}

fn extract_diff_blocks(content: &[ToolCallContent], blocks: &mut Vec<Block>) {
    for item in content {
        if let ToolCallContent::Diff(diff) = item {
            let path_str = diff.path.display().to_string();
            if let Some(old_text) = &diff.old_text {
                blocks.push(Block::Checkpoint {
                    path: path_str.clone(),
                    content: old_text.clone(),
                    restored: false,
                });
            }
            let mut lines = Vec::new();
            if let Some(old) = &diff.old_text {
                for line in old.lines() {
                    lines.push(DiffLine::Remove(line.to_string()));
                }
            }
            for line in diff.new_text.lines() {
                lines.push(DiffLine::Add(line.to_string()));
            }
            if !lines.is_empty() {
                blocks.push(Block::Diff { path: path_str, lines });
            }
        }
    }
}
