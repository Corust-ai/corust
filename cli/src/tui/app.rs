use agent_client_protocol::{PermissionOption, ToolCallContent, ToolCallId};
use futures::channel::oneshot;

use crate::event::{Event, PermissionResponse};

/// The TUI application model (TEA: Model).
///
/// All mutable state lives here. The `update` methods take events and
/// mutate the model; the `ui` module reads it to produce frames.
pub struct App {
    /// Current text in the input bar.
    pub input: String,
    /// Cursor byte-position within `input`.
    pub cursor: usize,
    /// Conversation blocks displayed in the scroll area.
    pub blocks: Vec<Block>,
    /// Vertical scroll offset (0 = bottom / latest).
    pub scroll_offset: u16,
    /// Whether the app should quit on the next loop iteration.
    pub should_quit: bool,
    /// Whether a prompt is currently being processed by the agent.
    pub busy: bool,
    /// Status bar info.
    pub status: StatusBar,
    /// Pending permission request (if any).
    pub pending_permission: Option<PendingPermission>,
    /// Input history for up/down recall.
    pub history: Vec<String>,
    /// Current position in history (None = editing new input).
    pub history_cursor: Option<usize>,
    /// Stashed input when browsing history.
    pub history_stash: String,
}

// ---------------------------------------------------------------------------
// Block model
// ---------------------------------------------------------------------------

/// A single visual unit in the conversation scroll area.
pub enum Block {
    /// User's submitted input.
    UserInput { text: String },

    /// Agent's streamed text response (accumulates chunks).
    AgentText { content: String, streaming: bool },

    /// Agent's internal reasoning (collapsible).
    Thinking { content: String, collapsed: bool },

    /// Tool invocation with structured output.
    ToolCall {
        id: ToolCallId,
        title: String,
        status: String,
        locations: Vec<String>,
        output: Option<String>,
    },

    /// Fenced code block extracted from agent response.
    CodeBlock { lang: String, code: String },

    /// Unified diff for file edits.
    Diff { path: String, lines: Vec<DiffLine> },

    /// System notification.
    System { message: String },

    /// Permission request (rendered inline).
    PermissionRequest { title: String, resolved: Option<String> },
}

/// A single line within a diff block.
pub enum DiffLine {
    Context(String),
    Add(String),
    Remove(String),
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// A pending permission request awaiting user decision.
pub struct PendingPermission {
    pub options: Vec<PermissionOption>,
    pub respond: oneshot::Sender<PermissionResponse>,
}

/// Static metadata shown in the status bar.
pub struct StatusBar {
    pub model: String,
    pub cwd: String,
    pub git_branch: Option<String>,
}

// ---------------------------------------------------------------------------
// App implementation
// ---------------------------------------------------------------------------

impl App {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            blocks: vec![Block::System {
                message: "Welcome to corust. Type a message and press Enter.".into(),
            }],
            scroll_offset: 0,
            should_quit: false,
            busy: false,
            status: StatusBar {
                model: String::new(),
                cwd: std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                git_branch: None,
            },
            pending_permission: None,
            history: Vec::new(),
            history_cursor: None,
            history_stash: String::new(),
        }
    }

    // -- ACP event handling --

    pub fn handle_acp_event(&mut self, event: Event) {
        match event {
            Event::AgentText(text) => {
                if let Some(Block::AgentText { content, .. }) = self.blocks.last_mut() {
                    content.push_str(&text);
                } else {
                    self.blocks.push(Block::AgentText {
                        content: text,
                        streaming: true,
                    });
                }
                split_code_blocks(&mut self.blocks);
                self.scroll_offset = 0;
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
                self.scroll_offset = 0;
            }
            Event::ToolCallStarted(tool_call) => {
                // Format locations as "path:line" strings.
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

                // Extract text output from content.
                let output = extract_text_content(&tool_call.content);

                self.blocks.push(Block::ToolCall {
                    id: tool_call.tool_call_id.clone(),
                    title: tool_call.title.clone(),
                    status: format!("{:?}", tool_call.status),
                    locations,
                    output,
                });

                // Extract diffs as separate Diff blocks.
                extract_diff_blocks(&tool_call.content, &mut self.blocks);

                self.scroll_offset = 0;
            }
            Event::ToolCallUpdated(update) => {
                let target_id = &update.tool_call_id;

                // Find the matching ToolCall block by ID.
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
                    if let Some(new_title) = &update.fields.title {
                        *title = new_title.clone();
                    }
                    if let Some(new_status) = &update.fields.status {
                        *status = format!("{new_status:?}");
                    }
                    if let Some(content) = &update.fields.content {
                        if let Some(text) = extract_text_content(content) {
                            *output = Some(text);
                        }
                        extract_diff_blocks(content, &mut self.blocks);
                    }
                }
                self.scroll_offset = 0;
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
                self.scroll_offset = 0;
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
                self.scroll_offset = 0;
            }
            Event::Error(msg) => {
                self.blocks.push(Block::System {
                    message: format!("Error: {msg}"),
                });
                self.scroll_offset = 0;
            }
        }
    }

    // -- Permission --

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

    pub fn cancel_permission(&mut self) {
        if let Some(perm) = self.pending_permission.take() {
            for block in self.blocks.iter_mut().rev() {
                if let Block::PermissionRequest { resolved, .. } = block {
                    *resolved = Some("cancelled".into());
                    break;
                }
            }
            let _ = perm.respond.send(PermissionResponse::Cancelled);
        }
    }

    // -- Turn lifecycle --

    pub fn turn_finished(&mut self) {
        self.busy = false;
        // Mark the last AgentText as done streaming.
        for block in self.blocks.iter_mut().rev() {
            if let Block::AgentText { streaming, .. } = block {
                *streaming = false;
                break;
            }
        }
    }

    // -- Thinking toggle --

    pub fn toggle_thinking(&mut self) {
        // Toggle the most recent Thinking block.
        for block in self.blocks.iter_mut().rev() {
            if let Block::Thinking { collapsed, .. } = block {
                *collapsed = !*collapsed;
                break;
            }
        }
    }

    // -- Input editing --

    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn delete_char_before_cursor(&mut self) {
        if self.cursor > 0 {
            let prev = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.drain(prev..self.cursor);
            self.cursor = prev;
        }
    }

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
        self.cursor = 0;
        self.scroll_offset = 0;
        Some(text)
    }

    // -- History navigation --

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_cursor {
            None => {
                // Entering history: stash current input.
                self.history_stash = self.input.clone();
                self.history.len() - 1
            }
            Some(0) => return, // already at oldest
            Some(i) => i - 1,
        };
        self.history_cursor = Some(idx);
        self.input = self.history[idx].clone();
        self.cursor = self.input.len();
    }

    pub fn history_next(&mut self) {
        let Some(idx) = self.history_cursor else { return };
        if idx + 1 >= self.history.len() {
            // Back to current input.
            self.history_cursor = None;
            self.input = std::mem::take(&mut self.history_stash);
        } else {
            self.history_cursor = Some(idx + 1);
            self.input = self.history[idx + 1].clone();
        }
        self.cursor = self.input.len();
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor = self.input[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.input.len());
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Scan the last `AgentText` block for completed fenced code blocks.
/// Split them into `AgentText` + `CodeBlock` segments, keeping any
/// incomplete fence (mid-stream) as trailing `AgentText`.
fn split_code_blocks(blocks: &mut Vec<Block>) {
    let Some(Block::AgentText { content, streaming }) = blocks.last() else { return };

    // Quick check: does the content even contain a fence?
    if !content.contains("```") {
        return;
    }

    let raw = content.clone();
    let is_streaming = *streaming;
    blocks.pop();

    let mut remaining = raw.as_str();

    while let Some(fence_pos) = remaining.find("```") {
        // Text before the opening fence.
        let before = &remaining[..fence_pos];
        if !before.trim().is_empty() {
            blocks.push(Block::AgentText {
                content: before.to_string(),
                streaming: false,
            });
        }

        let after_backticks = &remaining[fence_pos + 3..];

        // Extract language from the opening fence line.
        let lang_end = after_backticks.find('\n').unwrap_or(after_backticks.len());
        let lang = after_backticks[..lang_end].trim().to_string();

        // Start of code content (after the lang line + newline).
        let code_start_offset = fence_pos + 3 + lang_end;
        if code_start_offset >= remaining.len() {
            // Incomplete fence at very end — keep as text.
            blocks.push(Block::AgentText {
                content: remaining[fence_pos..].to_string(),
                streaming: is_streaming,
            });
            remaining = "";
            break;
        }

        let code_area = &remaining[code_start_offset + 1..]; // +1 skip newline

        if let Some(close_pos) = code_area.find("```") {
            let code = code_area[..close_pos].trim_end_matches('\n').to_string();
            blocks.push(Block::CodeBlock { lang, code });

            // Advance past the closing ```.
            let after_close = &code_area[close_pos + 3..];
            remaining = after_close.strip_prefix('\n').unwrap_or(after_close);
        } else {
            // No closing fence yet — keep everything from fence_pos as streaming text.
            blocks.push(Block::AgentText {
                content: remaining[fence_pos..].to_string(),
                streaming: is_streaming,
            });
            remaining = "";
            break;
        }
    }

    if !remaining.is_empty() {
        blocks.push(Block::AgentText {
            content: remaining.to_string(),
            streaming: is_streaming,
        });
    }
}

/// Extract plain text from ToolCallContent blocks.
fn extract_text_content(content: &[ToolCallContent]) -> Option<String> {
    let mut text = String::new();
    for item in content {
        if let ToolCallContent::Content(c) = item {
            if let agent_client_protocol::ContentBlock::Text(t) = &c.content {
                text.push_str(&t.text);
            }
        }
        if let ToolCallContent::Terminal(_) = item {
            // Terminal content is streamed separately; skip for now.
        }
    }
    if text.is_empty() { None } else { Some(text) }
}

/// Extract Diff blocks from ToolCallContent and append them to the block list.
fn extract_diff_blocks(content: &[ToolCallContent], blocks: &mut Vec<Block>) {
    for item in content {
        if let ToolCallContent::Diff(diff) = item {
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
                blocks.push(Block::Diff {
                    path: diff.path.display().to_string(),
                    lines,
                });
            }
        }
    }
}
