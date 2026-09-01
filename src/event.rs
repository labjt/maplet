use tokio::sync::oneshot;

use crate::theme::Theme;

/// Everything the main loop reacts to, from every source.
pub enum AppEvent {
    Term(crossterm::event::Event),
    Chat(ChatEvent),
    Acp(AcpEvent),
    ThemeChanged(Theme),
    ProxyHealth(bool),
    Models(Vec<String>),
}

/// Chat-mode streaming events. `turn` guards against stale deltas after cancel.
pub enum ChatEvent {
    Delta { turn: u64, text: String },
    Done { turn: u64 },
    Error { turn: u64, message: String },
}

/// Agent-mode events, mapped from ACP session/update notifications.
pub enum AcpEvent {
    SessionReady { session_id: String },
    MessageChunk(String),
    ThoughtChunk(String),
    ToolCall(ToolCallView),
    ToolCallUpdate(ToolCallUpdateView),
    Plan(Vec<PlanEntryView>),
    PermissionRequest {
        title: String,
        options: Vec<PermissionOptionView>,
        reply: oneshot::Sender<PermissionOutcome>,
    },
    TurnEnded { stop_reason: String },
    Error(String),
    /// goose has no configured provider / auth problem — actionable, not fatal.
    AuthRequired(String),
    /// goose subprocess or ACP thread died.
    Exited(String),
}

#[derive(Debug, Clone)]
pub struct ToolCallView {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub raw_input: Option<String>,
    pub raw_output: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolCallUpdateView {
    pub id: String,
    pub title: Option<String>,
    pub status: Option<String>,
    pub raw_input: Option<String>,
    pub raw_output: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PlanEntryView {
    pub content: String,
    pub status: String, // pending | in_progress | completed
}

#[derive(Debug, Clone)]
pub struct PermissionOptionView {
    pub id: String,
    pub name: String,
    /// ACP option kind: allow_once | allow_always | reject_once | reject_always
    pub kind: String,
}

#[derive(Debug, Clone)]
pub enum PermissionOutcome {
    Selected { option_id: String },
    Cancelled,
}

/// Commands the UI sends to the ACP thread.
pub enum AcpCommand {
    Prompt { text: String },
    Cancel,
    Shutdown,
}
