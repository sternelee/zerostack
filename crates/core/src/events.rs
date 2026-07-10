use compact_str::CompactString;
use serde::{Deserialize, Serialize};

/// Events sent from CoreEngine to the frontend (TUI or GUI).
#[derive(Debug, Clone)]
pub enum CoreEvent {
    // === Streaming output ===
    StreamingDelta { text: CompactString },
    ReasoningDelta { text: CompactString },
    CompletionCall {
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },

    // === Tool calls ===
    ToolCall {
        name: CompactString,
        args: serde_json::Value,
    },
    ToolResult {
        name: CompactString,
        output: CompactString,
    },
    SubagentToolCall {
        name: CompactString,
        args: serde_json::Value,
    },

    // === Permissions ===
    PermissionNeeded {
        id: u64,
        tool_name: CompactString,
        args: String,
    },

    // === Message lifecycle ===
    MessageComplete {
        response: CompactString,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },
    Retrying {
        attempt: usize,
        max: usize,
    },

    // === Session management ===
    SessionListUpdated {
        sessions: Vec<SessionInfo>,
    },
    SessionChanged {
        session_id: CompactString,
    },

    // === Status ===
    StatusUpdate {
        model: CompactString,
        tokens_used: u64,
        mode: String,
    },
    ConfigChanged,

    // === System ===
    Error {
        message: CompactString,
    },
}

/// Actions sent from the frontend to CoreEngine.
#[derive(Debug, Clone)]
pub enum UserAction {
    // === Messages ===
    SendMessage { text: CompactString },
    CancelStream,

    // === Permissions ===
    PermissionResponse { id: u64, allow: bool },

    // === Sessions ===
    CreateSession { name: Option<CompactString> },
    SwitchSession { session_id: CompactString },
    DeleteSession { session_id: CompactString },
    RenameSession { session_id: CompactString, name: CompactString },

    // === Commands ===
    RunCommand { command: CompactString },

    // === Config ===
    ReloadConfig,
    SetModel { model: CompactString },

    // === Lifecycle ===
    Quit,
}

/// Lightweight session metadata for the sidebar list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: CompactString,
    pub name: CompactString,
    pub model: CompactString,
    pub message_count: usize,
    pub created_at: CompactString,
}

/// Token usage summary.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

/// Initial state sent to the frontend on startup.
#[derive(Debug, Clone)]
pub struct InitialState {
    pub sessions: Vec<SessionInfo>,
    pub current_session_id: Option<CompactString>,
    pub model: CompactString,
    pub provider: CompactString,
    pub mode: String,
}