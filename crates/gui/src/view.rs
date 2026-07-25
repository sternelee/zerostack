//! Root view of the zerostack GUI.
//!
//! Layout: a horizontal split with a session sidebar on the left, a chat column on the
//! right (history + bottom-pinned input). The view polls a [`GuiBridge`] on a small
//! recurring tick so streaming deltas appear in near-real-time.
//!
//! This view is intentionally minimal: it implements only one screen (sidebar / chat /
//! input), per the agreed scope for the first iteration. Tool cards, permission dialogs,
//! and slash commands can land in follow-ups as separate view components.
//!
//! [`GuiBridge`]: crate::GuiBridge
use std::collections::HashMap;
use std::time::Duration;

use compact_str::CompactString;
use gpui::{
    App, Bounds, Context, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    GlobalElementId, KeyDownEvent, LayoutId, Pixels, Render, ScrollHandle, ScrollStrategy,
    SharedString, Style, TitlebarOptions, UTF16Selection, UniformListScrollHandle, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, relative, rgb, rgba, size, uniform_list,
};
use gpui_platform::application;
use zerostack_core::events::CoreEvent;
use zerostack_core::events::SessionInfo;
use zerostack_core::events::UserAction;

use crate::GuiBridge;
use crate::markdown::{BlockKind, MarkdownBlock, MarkdownSpan, parse_markdown};
use crate::theme::dark;

/// The high-level shape of a row in the chat column. We collapse the engine's
/// `ChatMessage` (which carries the role as a string) into this enum so the view layer
/// can switch on it without re-parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
    /// Chain-of-thought deltas pumped out by the model while it thinks.
    /// Rendered as a foldable "thinking…" card so users can audit it but
    /// don't have to read it by default. We never persist this to history
    /// (engine strips it before saving), so the local `chat` is the only
    /// place we keep it.
    Reasoning,
    /// Interactive permission prompt forwarded from the engine. The card
    /// renders Allow/Deny buttons; the visible `content` field carries the
    /// formatted tool name + arg snippet.
    Permission,
}

impl Role {
    pub fn from_engine(role: &str) -> Self {
        match role {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool_call" | "tool_result" | "subagent_tool_call" => Role::Tool,
            _ => Role::System,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: Role,
    pub content: SharedString,
    /// For `Role::Permission` this carries the engine-issued ask id so the
    /// Allow/Deny buttons can route the response back. For other roles it's
    /// `None` and the renderer ignores it.
    permission_id: Option<u64>,
}

impl ChatMessage {
    pub fn user(text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::User,
            content: text.into(),
            permission_id: None,
        }
    }

    pub fn assistant(text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::Assistant,
            content: text.into(),
            permission_id: None,
        }
    }

    pub fn tool(text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::Tool,
            content: text.into(),
            permission_id: None,
        }
    }

    pub fn system(text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::System,
            content: text.into(),
            permission_id: None,
        }
    }

    pub fn permission(id: u64, text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::Permission,
            content: text.into(),
            permission_id: Some(id),
        }
    }
}

/// A sidebar "project" group: every session that was started in the same
/// working directory. The `key` is the stable, machine-friendly identifier
/// (the full path, or `""` for the ungrouped bucket); `label` is the
/// human-friendly display name (typically the path's basename); `hint` is a
/// dimmed secondary line with the full path so the user can still see where
/// the project lives when two `foo` directories collide.
#[derive(Clone, Debug)]
struct ProjectGroup {
    key: String,
    label: String,
    hint: String,
    /// Indices into [`ShellState::sidebar`] that belong to this group, in the
    /// same order they appeared in the input list (which puts the most
    /// recently modified sessions first).
    session_indices: Vec<usize>,
    /// Whether the group contains the currently active session.
    is_active: bool,
}

/// One row in the sidebar's flattened tree view. Project rows always come
/// first, followed (when expanded) by their child session rows. The
/// `group_idx` indexes into the precomputed `Vec<ProjectGroup>` built by
/// [`ShellState::build_sidebar_rows`], and `session_idx` indexes into
/// [`ShellState::sidebar`] (the underlying `Vec<SessionInfo>`).
#[derive(Clone, Debug)]
enum SidebarRow {
    Project {
        group_idx: usize,
    },
    #[allow(dead_code)]
    Session {
        /// Bookkeeping for future grouping-aware renderers (e.g. indenting
        /// child rows by group nesting depth) — not read today, but kept
        /// for symmetry with [`SidebarRow::Project`].
        group_idx: usize,
        session_idx: usize,
    },
}

/// Slash commands exposed by the engine. Kept in sync with the dispatch table in
/// `zerostack_core::engine::CoreEngine::handle_slash_command`. Each entry is
/// `(name, description, needs_arg)`. When `needs_arg` is true, selecting the
/// command (via Tab, Enter or click) only fills the input box — the user must
/// then type the argument before submitting, mirroring the TUI behavior.
/// Atomic commands (false) are submitted on Enter directly.
///
/// Only commands the engine itself handles are listed here. The TUI has the
/// broader picker in `src/ui/pickers/list.rs` (which itself only reflects the
/// commands available in `BASE_COMMANDS` plus feature flags); the GUI is a
/// thinner client. Anything missing here that the engine treats as
/// `unknown command` gets a friendly error from the engine.
const SLASH_COMMANDS: &[(&str, &str, bool)] = &[
    ("/clear", "wipe current session history", false),
    ("/new", "alias for /clear", false),
    ("/undo", "remove the last user/assistant pair", false),
    (
        "/mode",
        "switch permission mode: standard|restrictive|readonly|guarded|yolo",
        true,
    ),
    ("/model", "switch the active model", true),
    ("/provider", "switch the active LLM provider", true),
    ("/add", "attach a file to the active context", true),
    ("/drop", "detach a file from the active context", true),
    ("/drop-all", "detach every extra file", false),
    ("/rename", "rename the active session", true),
    (
        "/history",
        "show recent chat history (most recent first)",
        false,
    ),
    (
        "/reasoning",
        "toggle chain-of-thought reasoning on/off",
        false,
    ),
    ("/thinking", "alias for /reasoning", false),
    ("/sessions", "list every session known to the engine", false),
    ("/help", "list every slash command", false),
    ("/quit", "close the window and shut the engine down", false),
];

/// Return the subset of [`SLASH_COMMANDS`] whose name starts with `prefix`.
fn slash_matches(prefix: &str) -> Vec<(&'static str, &'static str, bool)> {
    SLASH_COMMANDS
        .iter()
        .copied()
        .filter(|(name, _, _)| name.starts_with(prefix))
        .collect()
}

/// State owned by the root view. Lives inside one main window. The bridge is owned
/// directly by the state; the polling tick operates on the same Entity without needing
/// to share the bridge.
pub struct ShellState {
    bridge: GuiBridge,

    sidebar: Vec<SessionInfo>,
    current_session_id: SharedString,
    /// Explorer-style expand/collapse state keyed by project path. Missing
    /// entries default to expanded so newly-discovered projects pop open on
    /// first view. Click the project header to toggle.
    sidebar_groups_expanded: HashMap<String, bool>,
    /// Free-text filter applied to the sidebar tree. Empty shows everything.
    /// Matches are case-insensitive substrings against the project's label,
    /// the project's hint (full path), the session's name, and the session
    /// model/provider. We keep it as a `SharedString` so it can be edited
    /// through the same IME-aware path the input box uses.
    sidebar_filter: SharedString,
    /// Dedicated focus handle for the sidebar search box. Distinct from the
    /// chat input focus handle so toggling into the search box doesn't move
    /// selection into the message draft.
    sidebar_search_focus: FocusHandle,
    /// Set whenever the user clicks the "Refresh" button so we can flash a
    /// tiny "syncing…" indicator; the actual fetch happens in the bridge.
    sidebar_refreshing: bool,

    chat: Vec<ChatMessage>,
    /// Index of the assistant message currently being streamed into. Lets us append
    /// text cheaply as `StreamingDelta` chunks arrive.
    streaming_assistant_idx: Option<usize>,

    /// Whether the engine is currently streaming/working. Drives the
    /// "thinking…" pill on the chat footer and the active session row
    /// badge in the sidebar.
    is_thinking: bool,
    last_error: Option<SharedString>,

    /// Lightweight header strip at the top of the chat column: shows the
    /// current model / provider pair, the active permission mode, and a
    /// rolling token counter from the most recent `StatusUpdate` event.
    status_model: SharedString,
    status_provider: SharedString,
    status_mode: SharedString,
    status_tokens: u64,

    input_text: SharedString,
    /// Character-index cursor inside `input_text`. Always satisfies
    /// `0 <= input_cursor <= input_text.chars().count()`. Rendering draws a small
    /// rectangle at this position so the user can see where the next character will
    /// land (and where backspace will remove).
    input_cursor: usize,
    input_focus: FocusHandle,
    sidebar_scroll: UniformListScrollHandle,
    /// Scroll handle for the slash-command popup. Bumped whenever the user navigates
    /// the highlight up/down so the selected row stays visible.
    slash_popup_scroll: UniformListScrollHandle,
    /// Scroll handle for the chat message list. Long chats stay inside the column
    /// instead of pushing the input box off-screen.
    chat_scroll: ScrollHandle,

    /// Last `current_session_id` we already scrolled the sidebar to. We
    /// compare against it on every paint to decide whether a fresh scroll
    /// is needed; resetting to `""` on `SessionChanged` ensures the next
    /// render finds a mismatch and re-scrolls. We never reset this on
    /// first scroll to avoid clobbering user-driven scrolling every frame.
    last_scrolled_session_id: SharedString,

    /// True while the platform IME is composing preedit text (e.g. typing
    /// pinyin before converting to a CJK character). While composing we
    /// suppress our `on_key_down` listener — both so we don't double-insert
    /// keys that the IME consumes, and so navigation keys (arrows, Esc)
    /// reach macOS's `NSTextInputClient` instead of being eaten by us. Cleared
    /// in `replace_text_in_range` (commit) and `unmark_text` (cancel).
    ime_composing: bool,
    /// UTF-16 range within `input_text` that currently holds the IME mark.
    /// Returned from `EntityInputHandler::marked_text_range` so macOS sees
    /// `hasMarkedText == YES` and routes new keystrokes through the IME.
    ime_mark_utf16: Option<std::ops::Range<usize>>,

    /// Blinking cursor state: `Some(epoch)` while a blink timer is running, `None`
    /// when nothing is animating. The epoch is bumped on focus changes so an already-
    /// running timer can reset cleanly.
    cursor_visible: bool,

    /// When the user has typed `/`, we show a small popup listing matching slash
    /// commands above the input box. The popup filters as the user keeps typing.
    /// `slash_popup_selected` is the index into the filtered list (highlights with the
    /// accent color); the popup stays open until the user dismisses it with Esc,
    /// submits, or stops typing `/` at the start of the input.
    slash_popup_visible: bool,
    slash_popup_selected: usize,

    /// Set after the very first successful focus pass. Without an explicit call to
    /// `FocusHandle::focus(window, cx)` the input box never captures keystrokes, so we
    /// perform that on the first render and then leave the user in control.
    has_focused_input: bool,

    /// Live reasoning / chain-of-thought buffer for the most recent assistant
    /// turn. Cleared on `AgentStarted`, appended to on `ReasoningDelta`, and
    /// rendered as a foldable ("thinking…") card above the assistant's reply.
    /// `reasoning_idx` is the position in `chat` for the reasoning row so we
    /// can mutate the same bubble in place as more deltas arrive.
    reasoning_buffer: SharedString,
    reasoning_idx: Option<usize>,

    /// Stack of pending permission asks that the engine has handed us. The
    /// keys are the platform-issued IDs the engine gave us via
    /// `PermissionNeeded`; the values are the tool name + args payload so the
    /// user can read them before deciding. We keep this map even when nothing
    /// is pending so callers can `remove` without re-allocating.
    pending_permissions: std::collections::HashMap<u64, PendingPermission>,

    /// Recent prompts the user actually sent (most recent at the back). Lets
    /// us replay the previous input on the Up arrow, matching the TUI's
    /// "history recall" behaviour. Capped so we don't grow forever.
    prompt_history: Vec<String>,
    /// Cached pointer into `prompt_history`: when the user starts editing
    /// something new (different from the most recent prompt) we mark that the
    /// history walk should start from the *oldest* entry on Up, and end on the
    /// *pending draft* on Down.
    prompt_history_cursor: Option<usize>,
}

/// One permission prompt the engine has handed us but not yet resolved. We
/// keep the raw inputs in addition to a formatted display string so the user
/// can see exactly what would be executed.
#[derive(Clone, Debug)]
struct PendingPermission {
    tool: SharedString,
    args: SharedString,
}

impl ShellState {
    fn new(bridge: GuiBridge, cx: &mut Context<Self>) -> Self {
        Self {
            bridge,
            sidebar: Vec::new(),
            current_session_id: SharedString::new(""),
            sidebar_groups_expanded: HashMap::new(),
            sidebar_filter: SharedString::new(""),
            sidebar_refreshing: false,
            chat: vec![ChatMessage::system(
                "zerostack-gui ready. Type below and press Enter to send. Use / for slash commands (e.g. /help, /model).",
            )],
            streaming_assistant_idx: None,
            is_thinking: false,
            last_error: None,
            status_model: SharedString::new(""),
            status_provider: SharedString::new(""),
            status_mode: SharedString::new("yolo"),
            status_tokens: 0,
            input_text: SharedString::new(""),
            input_cursor: 0,
            input_focus: cx.focus_handle(),
            sidebar_search_focus: cx.focus_handle(),
            sidebar_scroll: UniformListScrollHandle::new(),
            slash_popup_scroll: UniformListScrollHandle::new(),
            chat_scroll: ScrollHandle::new(),
            last_scrolled_session_id: SharedString::new(""),
            cursor_visible: true,
            slash_popup_visible: false,
            slash_popup_selected: 0,
            has_focused_input: false,
            ime_composing: false,
            ime_mark_utf16: None,
            reasoning_buffer: SharedString::new(""),
            reasoning_idx: None,
            pending_permissions: std::collections::HashMap::new(),
            prompt_history: Vec::new(),
            prompt_history_cursor: None,
        }
    }

    /// Insert a single character at the current cursor position and advance the cursor
    /// by one. Treats input as chars, not bytes, so multi-byte UTF-8 stays intact.
    /// Kept as a helper for callers that synthesise text into the input box
    /// (e.g. paste prefill or future IPC); the live keyboard path no longer
    /// routes through here so printable keystrokes stay with the
    /// IME-aware `replace_text_in_range` handler.
    #[allow(dead_code)]
    fn insert_char_at_cursor(&mut self, ch: char) {
        let mut buf = self.input_text.to_string();
        let byte_idx = buf
            .char_indices()
            .nth(self.input_cursor)
            .map(|(i, _)| i)
            .unwrap_or(buf.len());
        buf.insert(byte_idx, ch);
        self.input_text = SharedString::new(buf);
        self.input_cursor += 1;
        self.refresh_slash_popup();
    }

    /// Replace the character range `[start_char, end_char)` with `text` and
    /// advance the cursor to the end of the inserted text. Used by the IME
    /// commit path and as a building block for composition replacement once
    /// UTF-16 ranges have been converted into codepoint indices.
    fn splice_text(&mut self, start_char: usize, end_char: usize, text: &str) {
        let mut buf = self.input_text.to_string();
        let start_byte = byte_index_for_char(&buf, start_char);
        let end_byte = byte_index_for_char(&buf, end_char.max(start_char));
        buf.replace_range(start_byte..end_byte, text);
        self.input_text = SharedString::new(buf);
        self.input_cursor = start_char + text.chars().count();
    }

    /// Remove the character immediately before the cursor. No-op when cursor is at
    /// the start of the input.
    fn backspace_at_cursor(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let mut buf = self.input_text.to_string();
        let target_char_idx = self.input_cursor - 1;
        if let Some((byte_idx, _)) = buf.char_indices().nth(target_char_idx) {
            // find end of that char
            let end = buf
                .char_indices()
                .nth(target_char_idx + 1)
                .map(|(i, _)| i)
                .unwrap_or(buf.len());
            buf.replace_range(byte_idx..end, "");
            self.input_text = SharedString::new(buf);
            self.input_cursor = target_char_idx;
        }
        self.refresh_slash_popup();
    }

    /// Move the insertion cursor by `delta` characters, clamped to the input bounds.
    fn move_cursor(&mut self, delta: isize) {
        let total = self.input_text.chars().count();
        let cur = self.input_cursor as isize + delta;
        self.input_cursor = cur.clamp(0, total as isize) as usize;
    }

    /// Recompute whether the slash popup should be visible and what items match. The
    /// popup is open iff the input starts with `/` and contains no whitespace; we
    /// filter [`SLASH_COMMANDS`] by the typed prefix and clamp the highlight index.
    fn refresh_slash_popup(&mut self) {
        let s = self.input_text.as_str();
        if let Some(rest) = s.strip_prefix('/') {
            if rest.contains(char::is_whitespace) {
                self.slash_popup_visible = false;
                return;
            }
            let matches = slash_matches(s);
            self.slash_popup_visible = !matches.is_empty();
            if self.slash_popup_selected >= matches.len() {
                self.slash_popup_selected = matches.len().saturating_sub(1);
            }
        } else {
            self.slash_popup_visible = false;
            self.slash_popup_selected = 0;
        }
    }

    /// Drain any pending events from the bridge and update our local state. Called
    /// from a recurring `cx.spawn`-based timer in [`ShellState::render`].
    fn poll_bridge(&mut self, cx: &mut Context<Self>) {
        let events = self.bridge.poll();
        for ev in events {
            self.apply_event(ev, cx);
        }
    }

    fn apply_event(&mut self, ev: CoreEvent, _cx: &mut Context<Self>) {
        let prev_len = self.chat.len();
        match ev {
            CoreEvent::StreamingDelta { text } => {
                self.append_to_streaming(text);
                self.is_thinking = false;
            }
            CoreEvent::ReasoningDelta { text } => {
                // Append every chunk to a live reasoning buffer; we'll render it
                // as a foldable "thinking" card so the user can read what the
                // model actually wrestled with. No-op on empty deltas — those
                // are common during streaming toe-holds.
                if text.is_empty() {
                    self.is_thinking = true;
                } else {
                    let combined = format!("{}{}", self.reasoning_buffer.as_str(), text.as_str());
                    self.reasoning_buffer = SharedString::new(combined);
                    match self.reasoning_idx {
                        None => {
                            self.chat.push(ChatMessage {
                                role: Role::Reasoning,
                                content: self.reasoning_buffer.clone(),
                                permission_id: None,
                            });
                            self.reasoning_idx = Some(self.chat.len() - 1);
                        }
                        Some(idx) => {
                            self.chat[idx].content = self.reasoning_buffer.clone();
                        }
                    }
                    self.is_thinking = true;
                }
            }
            CoreEvent::CompletionCall { .. } => {}
            CoreEvent::ToolCall { name, .. } => {
                self.chat
                    .push(ChatMessage::tool(format!("calling {name}…")));
                self.streaming_assistant_idx = None;
            }
            CoreEvent::ToolResult { name, output } => {
                self.chat
                    .push(ChatMessage::tool(format!("{name} → {output}")));
            }
            CoreEvent::SubagentToolCall { .. } => {}
            CoreEvent::PermissionNeeded {
                id,
                tool_name,
                args,
            } => {
                // Park the ask in our pending map so the Allow / Deny buttons can
                // route the response back to the engine, and surface an
                // interactive card so the user can actually see what's being
                // asked. We refuse to auto-allow anything the engine bothered to
                // interrupt for — that defeats the whole permission system — so
                // the GUI explicitly *waits* for a human decision.
                let tool = tool_name.to_string();
                let display = format!("{tool} wants to run: {args}");
                self.pending_permissions.insert(
                    id,
                    PendingPermission {
                        tool: SharedString::new(tool),
                        args: SharedString::new(args.to_string()),
                    },
                );
                self.chat.push(ChatMessage::permission(id, display));
            }
            CoreEvent::MessageComplete { response, .. } => {
                let text = response.to_string();
                if !text.is_empty() {
                    // We may have been streaming into the placeholder row; if not,
                    // push one. Either way finalize.
                    if let Some(idx) = self.streaming_assistant_idx {
                        self.chat[idx].content = SharedString::new(text);
                    } else {
                        self.chat.push(ChatMessage::assistant(text));
                    }
                }
                self.streaming_assistant_idx = None;
                self.reasoning_idx = None;
                self.reasoning_buffer = SharedString::new("");
                self.is_thinking = false;
            }
            CoreEvent::Retrying { attempt, max } => {
                self.chat
                    .push(ChatMessage::system(format!("retrying ({attempt}/{max})…")));
            }
            CoreEvent::SessionListUpdated { sessions } => {
                self.sidebar = sessions;
                // The disk-poll / manual refresh button lights up a tiny
                // pulse so the user sees that the list just got rewritten.
                self.sidebar_refreshing = false;
            }
            CoreEvent::SessionChanged { session_id } => {
                self.current_session_id = SharedString::new(session_id.as_str());
                // Keep the sidebar's selection in view: by clearing
                // `last_scrolled_session_id` we make the next sidebar paint
                // `scroll_to_item` again so the highlight is brought back
                // into view, even when the user has been scrolling around.
                self.last_scrolled_session_id = SharedString::new("");
            }
            CoreEvent::SessionHistory { messages } => {
                self.chat = messages
                    .into_iter()
                    .map(|m| ChatMessage {
                        role: Role::from_engine(&m.role),
                        content: SharedString::new(m.content.as_str()),
                        permission_id: None,
                    })
                    .collect();
                self.streaming_assistant_idx = None;
            }
            CoreEvent::StatusUpdate {
                model,
                provider,
                tokens_used,
                mode,
            } => {
                self.status_model = SharedString::new(model.as_str());
                self.status_provider = SharedString::new(provider.as_str());
                self.status_tokens = tokens_used;
                self.status_mode = SharedString::new(mode.as_str());
            }
            CoreEvent::AgentStarted => {
                // New user turn; reset the streaming placeholder and the
                // reasoning pipeline so any prior reasoning doesn't leak into
                // the next assistant response.
                self.streaming_assistant_idx = None;
                self.reasoning_buffer = SharedString::new("");
                self.reasoning_idx = None;
                self.is_thinking = true;
            }
            CoreEvent::AgentStopped => {
                self.is_thinking = false;
                self.reasoning_idx = None;
                self.reasoning_buffer = SharedString::new("");
            }
            CoreEvent::ConfigChanged => {}
            CoreEvent::CommandOutput { text } => {
                self.chat.push(ChatMessage::system(text.to_string()));
            }
            CoreEvent::Error { message } => {
                let text = message.to_string();
                self.last_error = Some(SharedString::new(text.clone()));
                self.chat.push(ChatMessage::system(text));
                self.is_thinking = false;
            }
        }
        // Follow-tail: when new chat content arrives, jump to the last message so
        // the user sees the new message. The scrollable chat container now has the
        // status bar as child 0 and the messages as children 1..=N, so the last
        // message index is `self.chat.len()`.
        if self.chat.len() > prev_len {
            self.chat_scroll.scroll_to_item(self.chat.len());
        }
    }

    fn append_to_streaming(&mut self, text: CompactString) {
        if text.is_empty() {
            return;
        }
        match self.streaming_assistant_idx {
            None => {
                let mut msg = ChatMessage::assistant("".to_string());
                msg.content = SharedString::new(text.as_str());
                self.chat.push(msg);
                self.streaming_assistant_idx = Some(self.chat.len() - 1);
            }
            Some(idx) => {
                let current = &mut self.chat[idx].content;
                let combined = format!("{current}{text}");
                *current = SharedString::new(combined);
            }
        }
    }

    fn submit_input(&mut self, cx: &mut Context<Self>) {
        let text = self.input_text.to_string();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }

        // /quit is a GUI-level shortcut: there's no point rounding it through
        // the engine just for it to come back as a session-wide Quit event.
        // We shut the bridge down and close the window directly.
        if trimmed == "/quit" || trimmed == "/exit" {
            self.chat
                .push(ChatMessage::system("/quit — closing window"));
            self.bridge.shutdown();
            cx.quit();
            return;
        }

        // Slash commands don't go through the agent; they live as their own engine
        // action. Plain text is wrapped in `UserAction::SendMessage`.
        let action = if trimmed.starts_with('/') {
            UserAction::RunSlashCommand {
                command: CompactString::new(trimmed.to_string()),
            }
        } else {
            UserAction::SendMessage {
                text: CompactString::new(trimmed.to_string()),
            }
        };

        if !self.bridge.send(action) {
            self.last_error = Some(SharedString::new("engine is offline"));
            return;
        }

        // Remember this prompt so Up arrow can replay it on demand. We cap at
        // 64 entries to keep the cap on memory; older entries fall off the
        // bottom of the deque. Consecutive duplicates collapse into one slot
        // so the user doesn't have to fish through `/clear`-spam when
        // recalling.
        if self.prompt_history.last().map(String::as_str) != Some(trimmed) {
            self.prompt_history.push(trimmed.to_string());
            if self.prompt_history.len() > 64 {
                self.prompt_history.remove(0);
            }
        }
        self.prompt_history_cursor = None;

        self.chat.push(ChatMessage::user(trimmed.to_string()));
        self.input_text = SharedString::new("");
        self.input_cursor = 0;
        self.slash_popup_visible = false;
        self.slash_popup_selected = 0;
        cx.notify();
    }

    /// Walk backward through the user's prompt history. Called when the user
    /// presses the Up arrow while the cursor sits at the first row of the
    /// input box (so accidental up-presses from elsewhere don't blow away
    /// their draft). We push a synthetic "draft" entry into the cursor
    /// position when the user starts editing fresh so Down moves back to it
    /// rather than dropping them off the end of the list.
    fn recall_prev_prompt(&mut self) {
        if self.prompt_history.is_empty() {
            return;
        }
        match self.prompt_history_cursor {
            None => {
                // First Up press: stash whatever the user was currently
                // typing so a matching Down puts it back, then jump to the
                // most recent prompt.
                self.prompt_history_cursor = Some(self.prompt_history.len());
            }
            Some(0) => return,
            Some(ix) => {
                self.prompt_history_cursor = Some(ix - 1);
            }
        }
        let ix = self.prompt_history_cursor.unwrap();
        if ix >= self.prompt_history.len() {
            return;
        }
        let prompt = self.prompt_history[ix].clone();
        self.input_text = SharedString::new(prompt);
        self.input_cursor = self.input_text.chars().count();
        self.refresh_slash_popup();
    }

    /// Walk forward through the user's prompt history. Symmetric to
    /// [`ShellState::recall_prev_prompt`]: when the cursor passes the most
    /// recent entry, we hand back the in-progress draft the user stashed on
    /// the first Up press.
    fn recall_next_prompt(&mut self) {
        let Some(ix) = self.prompt_history_cursor else {
            // Down at the top of the list (or before any Up press) is a no-op;
            // the user pressed it without first going up the list.
            return;
        };
        let next = ix + 1;
        if next < self.prompt_history.len() {
            self.prompt_history_cursor = Some(next);
            let prompt = self.prompt_history[next].clone();
            self.input_text = SharedString::new(prompt);
            self.input_cursor = self.input_text.chars().count();
        } else {
            // We've walked past the newest prompt — restore the empty input
            // (or the user's in-progress draft, which is what we stashed
            // before going up).
            self.prompt_history_cursor = None;
            self.input_text = SharedString::new("");
            self.input_cursor = 0;
        }
        self.refresh_slash_popup();
    }

    /// Resolve a pending permission: send `PermissionResponse { allow }` to
    /// the engine and remove the prompt from our pending map. The chat card
    /// itself stays in place; the buttons swap to a "denied" / "allowed"
    /// label so the audit trail is visible at a glance.
    fn answer_permission(&mut self, id: u64, allow: bool, cx: &mut Context<Self>) {
        if self.pending_permissions.remove(&id).is_none() {
            return;
        }
        let _ = self
            .bridge
            .send(UserAction::PermissionResponse { id, allow });
        for msg in self.chat.iter_mut() {
            if msg.role == Role::Permission && msg.permission_id == Some(id) {
                let tool = msg
                    .permission_id
                    .map(|i| format!("#{i}"))
                    .unwrap_or_else(|| "?".to_string());
                let verdict = if allow { "allowed" } else { "denied" };
                msg.content = SharedString::new(format!("{tool} → {verdict}"));
                msg.role = Role::System;
                msg.permission_id = None;
                break;
            }
        }
        cx.notify();
    }

    /// Insert a string of one or more characters at the caret. Mirrors the
    /// `replace_text_in_range(None, text)` path the IME handler takes, but
    /// without going through the platform input client — used by
    /// Shift+Enter to drop in a newline, and (eventually) by paste handlers
    /// for preflight mutations.
    fn insert_text_at_cursor(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut buf = self.input_text.to_string();
        let byte_idx = byte_index_for_char(&buf, self.input_cursor.min(buf.chars().count()));
        buf.insert_str(byte_idx, text);
        self.input_text = SharedString::new(buf);
        self.input_cursor += text.chars().count();
        self.refresh_slash_popup();
    }

    /// `true` when the caret is on the first row of a multi-line buffer
    /// (or the buffer has no newlines at all). Used to gate the Up-arrow
    /// history recall so users can still navigate line-by-line *inside* an
    /// in-progress draft.
    fn input_cursor_at_first_line(&self) -> bool {
        match self.input_text.char_indices().find(|(_, c)| *c == '\n') {
            None => true,
            Some((byte_idx, _)) => {
                self.input_cursor
                    .min(this_char_count(self.input_text.as_str()))
                    <= self.input_text[..byte_idx].chars().count()
            }
        }
    }

    /// Mirror of [`ShellState::input_cursor_at_first_line`] for Down.
    fn input_cursor_at_last_line(&self) -> bool {
        let last_newline = self
            .input_text
            .char_indices()
            .filter(|(_, c)| *c == '\n')
            .map(|(byte_idx, _)| self.input_text[..byte_idx].chars().count())
            .last();
        match last_newline {
            None => true,
            Some(last_line_start) => self.input_cursor >= last_line_start,
        }
    }

    /// Move the caret up one line, clamped to the same column on the new
    /// line. To keep the math simple we find the count of newlines strictly
    /// before the caret; if there's at least one, we move to the same
    /// horizontal offset on the previous line.
    fn move_to_prev_line(&mut self) {
        let buf = self.input_text.to_string();
        let caret_chars = self.input_cursor.min(buf.chars().count());
        let mut before = "";
        for (char_idx, c) in buf.chars().enumerate() {
            if c == '\n' && char_idx < caret_chars {
                before = &buf[..byte_index_for_char(&buf, char_idx)];
                break;
            }
        }
        // Column on the *current* line: chars since the last newline.
        let mut col = caret_chars;
        if let Some(last) = buf[..byte_index_for_char(&buf, caret_chars)]
            .char_indices()
            .filter(|(_, c)| *c == '\n')
            .last()
        {
            col -= buf[..last.0].chars().count() + 1;
        }
        // Walk back from `before` to the previous newline (if any).
        let mut prev = before;
        let mut target_chars = 0;
        let mut target_byte = 0;
        for (b, c) in before.char_indices().rev() {
            if c == '\n' {
                prev = &before[..b];
                target_byte = b;
                break;
            }
            target_chars += 1;
        }
        let prev_chars = prev.chars().count();
        let new_col = target_chars.min(col);
        let candidate = prev_chars + new_col;
        let _ = prev;
        let _ = target_byte;
        self.input_cursor = candidate.min(this_char_count(&buf));
    }

    /// Mirror of [`ShellState::move_to_prev_line`]: move the caret down one
    /// line on the same column.
    fn move_to_next_line(&mut self) {
        let buf = self.input_text.to_string();
        let caret_chars = self.input_cursor.min(buf.chars().count());
        // Find our current column starting position: last newline before caret.
        let mut cur_line_start_chars = 0;
        for (char_idx, c) in buf.chars().enumerate() {
            if c == '\n' && char_idx < caret_chars {
                cur_line_start_chars = char_idx + 1;
            }
        }
        let col = caret_chars - cur_line_start_chars;
        // Find the next newline after caret.
        let mut next_line_start_chars: Option<usize> = None;
        let mut after = "";
        for (char_idx, c) in buf.chars().enumerate() {
            if c == '\n' && char_idx >= caret_chars {
                next_line_start_chars = Some(char_idx + 1);
                after = &buf[byte_index_for_char(&buf, char_idx + 1)..];
                break;
            }
        }
        let Some(start) = next_line_start_chars else {
            return;
        };
        // Available columns on the next line until the following newline (or EOF).
        let avail_cols = after.char_indices().take_while(|(_, c)| *c != '\n').count();
        let col = col.min(avail_cols);
        self.input_cursor = start + col;
    }

    /// Group [`ShellState::sidebar`] into per-project buckets. Sessions with no
    /// `working_dir` (legacy sessions, or shell-mode runs from `/`) collapse into
    /// a single `(no project)` bucket so they never vanish silently. Groups are
    /// sorted by the most recent `created_at` they contain so that the project
    /// the user was last working in floats to the top.
    fn build_sidebar_groups(&self) -> Vec<ProjectGroup> {
        use std::collections::HashMap;
        let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
        for (idx, s) in self.sidebar.iter().enumerate() {
            let key = s.working_dir.as_str().trim().to_string();
            buckets.entry(key).or_default().push(idx);
        }
        let needle = self.sidebar_filter.as_str().trim().to_lowercase();
        let session_matches = |s: &SessionInfo| {
            if needle.is_empty() {
                return true;
            }
            s.name.to_lowercase().contains(&needle)
                || s.model.to_lowercase().contains(&needle)
                || s.provider.to_lowercase().contains(&needle)
                || s.working_dir.to_lowercase().contains(&needle)
        };
        let project_matches = |label: &str, hint: &str| {
            needle.is_empty()
                || label.to_lowercase().contains(&needle)
                || hint.to_lowercase().contains(&needle)
        };
        let mut groups: Vec<ProjectGroup> = buckets
            .into_iter()
            .filter_map(|(key, indices)| {
                let label;
                let hint;
                if key.is_empty() {
                    label = "(no project)".to_string();
                    hint = "ungrouped".to_string();
                } else {
                    label = project_label(&key);
                    hint = key.clone();
                }
                // Filter sessions inside the bucket to those whose name / model /
                // provider / working dir match. If the project label / hint itself
                // matches we keep all sessions under it (the user clearly wants to
                // land on this project regardless of session name).
                let keep_project = project_matches(&label, &hint);
                let mut kept_indices = Vec::new();
                for &i in &indices {
                    if keep_project || session_matches(&self.sidebar[i]) {
                        kept_indices.push(i);
                    }
                }
                if kept_indices.is_empty() {
                    None
                } else {
                    let is_active = kept_indices
                        .iter()
                        .any(|&i| self.sidebar[i].id.as_str() == self.current_session_id.as_str());
                    Some(ProjectGroup {
                        key,
                        label,
                        hint,
                        session_indices: kept_indices,
                        is_active,
                    })
                }
            })
            .collect();
        // Most-recently-touched project at the top, then alphabetical on the
        // group label so the order is stable when two projects have identical
        // timestamps (TUI uses the same convention).
        groups.sort_by(|a, b| {
            let a_max = a
                .session_indices
                .iter()
                .map(|&i| self.sidebar[i].created_at.as_str())
                .max()
                .unwrap_or("");
            let b_max = b
                .session_indices
                .iter()
                .map(|&i| self.sidebar[i].created_at.as_str())
                .max()
                .unwrap_or("");
            b_max.cmp(a_max).then_with(|| a.label.cmp(&b.label))
        });
        groups
    }

    /// Flatten the projective groups into the row sequence the sidebar's
    /// `uniform_list` consumes. Each expanded group contributes 1 header row
    /// plus one row per child session; collapsed groups contribute only the
    /// header row.
    fn build_sidebar_rows(&self) -> (Vec<SidebarRow>, Vec<ProjectGroup>) {
        let groups = self.build_sidebar_groups();
        let mut rows = Vec::with_capacity(self.sidebar.len() + groups.len());
        for (group_idx, group) in groups.iter().enumerate() {
            let expanded = self
                .sidebar_groups_expanded
                .get(&group.key)
                .copied()
                .unwrap_or(true);
            rows.push(SidebarRow::Project { group_idx });
            if expanded {
                for session_idx in &group.session_indices {
                    rows.push(SidebarRow::Session {
                        group_idx,
                        session_idx: *session_idx,
                    });
                }
            }
        }
        (rows, groups)
    }

    /// Flip a project's expansion state and redraw the sidebar. Called from the
    /// project-header click handler; we don't need to recompute anything here
    /// because `build_sidebar_rows` reads from `sidebar_groups_expanded` itself.
    fn toggle_project_expanded(&mut self, key: String, cx: &mut Context<Self>) {
        let next = !self
            .sidebar_groups_expanded
            .get(&key)
            .copied()
            .unwrap_or(true);
        self.sidebar_groups_expanded.insert(key, next);
        cx.notify();
    }

    /// Drop one character from the end of the sidebar filter. Backs the
    /// Backspace key in the search box (mirrors the chat input box but
    /// shared-string-only and never IME-aware since the filter is just a
    /// substring match).
    fn backspace_sidebar_filter(&mut self) {
        let buf = self.sidebar_filter.to_string();
        let mut chars: Vec<char> = buf.chars().collect();
        if chars.pop().is_some() {
            self.sidebar_filter = SharedString::new(chars.into_iter().collect::<String>());
        }
    }

    /// Append a single printable character to the sidebar filter. We avoid the
    /// full IME-aware path because the filter is a quick local substring match;
    /// CJK support is rarely needed and the worst case is "search didn't quite
    /// find it", which is recoverable on the next keystroke.
    fn append_sidebar_filter_char(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        let mut buf = self.sidebar_filter.to_string();
        buf.push(ch);
        self.sidebar_filter = SharedString::new(buf);
    }

    /// Toggle a manual "syncing" pulse so the user knows the refresh button
    /// did something. The disk-poll loop in the bridge does the real work;
    /// the pulse stays visible until the next SessionListUpdated lands.
    fn refresh_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_refreshing = true;
        let view_entity = cx.entity().clone();
        cx.spawn(async move |_weak_entity, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(700))
                .await;
            let _ = cx.update(|cx| {
                view_entity.update(cx, |state, cx| {
                    state.sidebar_refreshing = false;
                    cx.notify();
                });
            });
        })
        .detach();
        // The poll loop in the bridge sends `SessionListUpdated` whenever the
        // disk fingerprint changes; the renderer picks that up automatically
        // via `apply_event`. We just nudge the renderer in case no new event
        // is on the way (e.g. nothing changed) so the footer count refreshes.
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (rows, groups) = self.build_sidebar_rows();
        let total_sessions_on_disk = self.sidebar.len();
        let total_visible = rows.len();
        let group_count = groups.len();
        let sidebar_scroll = self.sidebar_scroll.clone();
        let view_entity = cx.entity().clone();
        let rows_for_render = rows.clone();
        let groups_for_render = groups.clone();
        let sessions_for_render = self.sidebar.clone();
        let current_id = self.current_session_id.clone();
        let group_expanded = self.sidebar_groups_expanded.clone();
        let filter_text = self.sidebar_filter.to_string();
        let is_refreshing = self.sidebar_refreshing;
        let is_thinking = self.is_thinking;
        let any_visible = total_visible > 0;
        let filter_active = !filter_text.trim().is_empty();

        // If a brand-new `current_session_id` was just set (engine signaled
        // `SessionChanged`, or this is the very first paint of the GUI) we
        // want to scroll the sidebar into the active row before the user
        // has to hunt for it. We compare against `last_scrolled_session_id`
        // so we don't fight user's hand-picked scroll position once the
        // initial scroll settled. The flag is reset on `SessionChanged`,
        // guaranteeing that any *new* active session triggers a fresh
        // scroll-to-reveal — even if the user has been climbing up the list.
        if self.last_scrolled_session_id.as_str() != current_id.as_str() {
            let row_ix = rows_for_render.iter().position(|row| match row {
                SidebarRow::Project { .. } => false,
                SidebarRow::Session { session_idx, .. } => {
                    sessions_for_render[*session_idx].id.as_str() == current_id.as_str()
                }
            });
            if let Some(ix) = row_ix {
                sidebar_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
                // Mark this id as the one we just scrolled to. We use a
                // tick-async to dodge the `&self`-vs-`&mut self` split
                // (we're already inside a render borrow). The worst case
                // is one extra redundant scroll on the very next frame,
                // which is invisible at 60 fps.
                let view_for_async_scroll = view_entity.clone();
                let id_for_async_scroll = current_id.clone();
                cx.spawn(async move |_weak_entity, async_cx| {
                    async_cx
                        .background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                    let _ = async_cx.update(|cx| {
                        view_for_async_scroll.update(cx, |state, _cx| {
                            state.last_scrolled_session_id = id_for_async_scroll;
                        });
                    });
                })
                .detach();
            }
        }

        // The footer keeps a compact sanity-check line: how many projects /
        // sessions are currently visible. We split "on disk" from "matching
        // the current filter" so the user can see when a query is narrowing
        // the list without losing the global count.
        let footer_label = if filter_active {
            if group_count <= 1 {
                format!("{total_visible} shown · {total_sessions_on_disk} total")
            } else {
                format!(
                    "{total_visible} shown · {group_count} project(s) · {total_sessions_on_disk} total"
                )
            }
        } else if group_count <= 1 {
            format!("{total_sessions_on_disk} session(s)")
        } else {
            format!("{total_sessions_on_disk} session(s) · {group_count} project(s)")
        };

        let view_for_search = view_entity.clone();
        let view_for_refresh = view_entity.clone();
        let view_for_new = view_entity.clone();

        div()
            .flex()
            .flex_col()
            .w(px(320.0))
            .flex_shrink_0()
            .h_full()
            .bg(rgb(dark::SIDEBAR_BG))
            .border_r_1()
            .border_color(rgb(dark::BORDER))
            .child(render_sidebar_header(
                view_for_new,
                view_for_refresh,
                view_for_search,
                self.sidebar_search_focus.clone(),
                filter_text,
                is_refreshing,
            ))
            .child(
                div().flex_1().child(
                    uniform_list(
                        "session-tree",
                        rows.len(),
                        cx.processor(move |_this, range: std::ops::Range<usize>, _window, _cx| {
                            range
                                .map(|row_idx| {
                                    let row = &rows_for_render[row_idx];
                                    let view_for_click = view_entity.clone();
                                    match *row {
                                        SidebarRow::Project { group_idx } => {
                                            let group = &groups_for_render[group_idx];
                                            let expanded = group_expanded
                                                .get(&group.key)
                                                .copied()
                                                .unwrap_or(true);
                                            render_project_row(
                                                group,
                                                expanded,
                                                is_thinking,
                                                view_for_click,
                                            )
                                        }
                                        SidebarRow::Session {
                                            group_idx: _,
                                            session_idx,
                                        } => {
                                            let session = &sessions_for_render[session_idx];
                                            let is_active =
                                                session.id.as_str() == current_id.as_str();
                                            render_session_row(
                                                session,
                                                is_active,
                                                is_thinking,
                                                view_for_click,
                                            )
                                        }
                                    }
                                })
                                .collect()
                        }),
                    )
                    .size_full()
                    .track_scroll(&sidebar_scroll),
                ),
            )
            // Empty-state placeholder: when there's nothing to show (either no
            // sessions on disk, or the active filter pruned everything) we drop
            // an explanatory placeholder into the scrolling list area. We keep
            // the `uniform_list` empty so `track_scroll` stays a real handle.
            .when(!any_visible, |d| {
                d.child(
                    div()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .flex()
                        .px_4()
                        .py_8()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .items_center()
                                .child(div().text_xs().text_color(rgb(dark::TEXT_MUTED)).child(
                                    if filter_active {
                                        "no sessions match the filter"
                                    } else if total_sessions_on_disk == 0 {
                                        "no sessions yet"
                                    } else {
                                        "no rows to show"
                                    },
                                ))
                                .child(div().text_xs().text_color(rgb(dark::TEXT_MUTED)).child(
                                    if filter_active {
                                        "press Esc in the search box to clear"
                                    } else {
                                        "click + New or start a session in another shell"
                                    },
                                )),
                        ),
                )
            })
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(dark::BORDER))
                    .text_xs()
                    .text_color(rgb(dark::TEXT_MUTED))
                    .child(footer_label),
            )
    }

    fn render_chat(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chat_scroll = self.chat_scroll.clone();
        let messages = self.chat.clone();
        let view_entity = cx.entity().clone();

        // Lightweight status row inside the chat scrollable area: provider /
        // model · mode · token total · live "thinking" / "idle" pill on the
        // right. Putting it inside the same scrollable region as the messages
        // means it scrolls together with the conversation rather than sitting
        // as a fixed strip above the history.
        let status_bar = div()
            .flex()
            .items_center()
            .gap_4()
            .px_5()
            .py_3()
            .border_b_1()
            .border_color(rgb(dark::BORDER))
            .text_xs()
            .child(
                div()
                    .text_color(rgb(dark::TEXT_SECONDARY))
                    .child(format!("{} / {}", self.status_provider, self.status_model)),
            )
            .child(
                div()
                    .text_color(rgb(dark::TEXT_MUTED))
                    .child(format!("mode: {}", self.status_mode)),
            )
            .child(
                div()
                    .text_color(rgb(dark::TEXT_MUTED))
                    .child(format!("tokens: {}", self.status_tokens)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_color(if self.is_thinking {
                        rgb(dark::ACCENT)
                    } else {
                        rgb(dark::TEXT_MUTED)
                    })
                    .child(if self.is_thinking {
                        "thinking…"
                    } else {
                        "idle"
                    }),
            )
            .into_any_element();

        let message_children: Vec<gpui::AnyElement> = messages
            .iter()
            .map(|msg| render_message(msg, view_entity.clone()))
            .collect();

        div().flex_1().min_h_0().bg(rgb(dark::CHAT_BG)).child(
            div()
                .id("chat-scroll-area")
                .flex_1()
                .overflow_y_scroll()
                .track_scroll(&chat_scroll)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w_full()
                        .min_w_0()
                        .px_6()
                        .py_5()
                        .child(status_bar)
                        .children(message_children)
                        .when(messages.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .py_20()
                                    .text_color(rgb(dark::TEXT_MUTED))
                                    .child("Ask anything to start."),
                            )
                        }),
                ),
        )
    }

    fn render_input(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let view_entity = cx.entity().clone();
        let input_focus_clone = self.input_focus.clone();
        // Wrap the visual input box and an `ImeInputElement` together: the IME
        // element registers the platform input handler against `input_focus`
        // during paint, while the visible div keeps the keyboard-listener and
        // visual styling. Together they enable space-bar (via key_char) and
        // CJK IME composing text on macOS/Windows.
        div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(dark::BORDER))
            .bg(rgb(dark::APP_BG))
            .when(self.slash_popup_visible, |d| {
                d.child(self.render_slash_popup(cx))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_5()
                    .py_3()
                    .gap_4()
                    .child(div().text_xs().text_color(rgb(dark::TEXT_MUTED)).child(
                        if self.input_text.is_empty() {
                            match self.slash_popup_visible {
                                true => "↑↓ select · ↵ run or insert args · esc close".to_string(),
                                false => "Press Enter to send · / for commands".to_string(),
                            }
                        } else {
                            format!(
                                "{} chars · cursor @{}",
                                self.input_text.chars().count(),
                                self.input_cursor
                            )
                        },
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("cancel-btn")
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .bg(rgb(dark::BUTTON_BG))
                            .text_color(rgb(dark::TEXT))
                            .cursor_pointer()
                            .text_sm()
                            .when(self.is_thinking, |d| d.opacity(1.0))
                            .when(!self.is_thinking, |d| d.opacity(0.4))
                            .child(if self.is_thinking { "Cancel" } else { "—" })
                            .on_click(cx.listener(|this, _ev, _window, _cx| {
                                let _ = this.bridge.send(UserAction::CancelStream);
                            })),
                    ),
            )
            .child(
                div()
                    .id("input-box")
                    .track_focus(&self.input_focus)
                    .focus_visible(|d| d.border_color(rgb(dark::ACCENT)))
                    .bg(rgb(dark::INPUT_BG))
                    .mx_5()
                    .mb_4()
                    .mt_1()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(dark::BORDER))
                    .text_color(rgb(dark::TEXT))
                    .text_sm()
                    .min_h(px(28.0))
                    .child({
                        let (before_cursor, after_cursor) =
                            split_at_char(&self.input_text, self.input_cursor);
                        render_input_text(
                            before_cursor,
                            after_cursor,
                            self.input_text.is_empty(),
                            self.cursor_visible,
                        )
                    })
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                        let key = ev.keystroke.key.as_str();
                        let mods = &ev.keystroke.modifiers;

                        // While the IME is composing preedit text (e.g. mid-pinyin),
                        // macOS routes printable keystrokes through the IME's
                        // `NSTextInputClient` rather than `NSResponder::keyDown:`.
                        // But our muted `marked_text_range` previously told macOS
                        // we have no composition, so keys also landed here and got
                        // double-inserted on top of the IME's eventual commit. We
                        // now flag `ime_composing` from `replace_and_mark_text_in_range`
                        // and bail out of this listener while it is set, surrendering
                        // all navigation/print keys to the IME until it commits.
                        if this.ime_composing {
                            return;
                        }

                        // Global shortcuts handled regardless of slash-popup state.
                        // Ctrl-C cancels the running stream (TUI parity), Ctrl-K
                        // wipes the input box (matches TUI's `\x15` line-kill),
                        // and Ctrl-A jumps to the start of the buffer.
                        if mods.control || mods.platform {
                            match key {
                                "c" => {
                                    if this.is_thinking {
                                        let _ = this.bridge.send(UserAction::CancelStream);
                                        cx.notify();
                                    }
                                    cx.stop_propagation();
                                    return;
                                }
                                "k" => {
                                    this.input_text = SharedString::new("");
                                    this.input_cursor = 0;
                                    this.slash_popup_visible = false;
                                    this.prompt_history_cursor = None;
                                    cx.stop_propagation();
                                    cx.notify();
                                    return;
                                }
                                "a" => {
                                    this.input_cursor = 0;
                                    cx.stop_propagation();
                                    return;
                                }
                                _ => {}
                            }
                        }

                        if this.slash_popup_visible {
                            let matches = slash_matches(this.input_text.as_str());
                            match key {
                                "escape" => {
                                    this.slash_popup_visible = false;
                                    cx.stop_propagation();
                                    return;
                                }
                                "up" => {
                                    if !matches.is_empty() {
                                        let cur = this.slash_popup_selected as isize - 1;
                                        this.slash_popup_selected =
                                            cur.rem_euclid(matches.len() as isize) as usize;
                                        this.slash_popup_scroll.scroll_to_item(
                                            this.slash_popup_selected,
                                            ScrollStrategy::Nearest,
                                        );
                                        cx.notify();
                                    }
                                    cx.stop_propagation();
                                    return;
                                }
                                "down" => {
                                    if !matches.is_empty() {
                                        let cur = this.slash_popup_selected as isize + 1;
                                        this.slash_popup_selected =
                                            cur.rem_euclid(matches.len() as isize) as usize;
                                        this.slash_popup_scroll.scroll_to_item(
                                            this.slash_popup_selected,
                                            ScrollStrategy::Nearest,
                                        );
                                        cx.notify();
                                    }
                                    cx.stop_propagation();
                                    return;
                                }
                                "tab" => {
                                    if let Some((name, _, _)) =
                                        matches.get(this.slash_popup_selected)
                                    {
                                        this.input_text = SharedString::new(*name);
                                        this.input_cursor = name.chars().count();
                                        this.slash_popup_visible = false;
                                    }
                                    cx.stop_propagation();
                                    return;
                                }
                                "enter" => {
                                    if let Some((name, _, needs_arg)) =
                                        matches.get(this.slash_popup_selected)
                                    {
                                        let name_str = *name;
                                        let wants_arg = *needs_arg;
                                        this.input_text = SharedString::new(name_str);
                                        this.input_cursor = name_str.chars().count();
                                        this.slash_popup_visible = false;
                                        if !wants_arg {
                                            this.submit_input(cx);
                                        }
                                        // Either way, we handle Enter: atomic
                                        // commands submit, argument-taking ones
                                        // stay in the box. Consume the keystroke
                                        // so macOS doesn't also fire
                                        // `[inputContext handleEvent:] -> insertText:"\n"]`.
                                        cx.stop_propagation();
                                    } else {
                                        this.submit_input(cx);
                                        cx.stop_propagation();
                                    }
                                    return;
                                }
                                _ => {}
                            }
                        }
                        match key {
                            "enter" => {
                                // Plain Enter submits; Shift+Enter inserts a
                                // newline so the user can compose multi-line
                                // prompts (matching the TUI's bracketed-paste
                                // preview). Either way we consume the keystroke
                                // so macOS doesn't also fire
                                // `[inputContext handleEvent:] -> insertText:"\n"]`.
                                if mods.shift {
                                    this.insert_text_at_cursor("\n");
                                    cx.notify();
                                } else {
                                    this.submit_input(cx);
                                }
                                cx.stop_propagation();
                            }
                            "backspace" => {
                                if mods.platform || (mods.control && !this.input_text.is_empty()) {
                                    // Ctrl/Cmd-Backspace wipes the whole buffer
                                    // (matches TUI's `\x15` line-kill semantics).
                                    this.input_text = SharedString::new("");
                                    this.input_cursor = 0;
                                    this.refresh_slash_popup();
                                } else {
                                    this.backspace_at_cursor();
                                }
                                cx.stop_propagation();
                            }
                            "left" => {
                                if mods.platform || (mods.alt && !mods.shift) {
                                    this.input_cursor = 0;
                                } else {
                                    this.move_cursor(-1);
                                }
                                cx.stop_propagation();
                            }
                            "right" => {
                                if mods.platform || (mods.alt && !mods.shift) {
                                    this.input_cursor = this_char_count(this.input_text.as_str());
                                } else {
                                    this.move_cursor(1);
                                }
                                cx.stop_propagation();
                            }
                            "home" => {
                                this.input_cursor = 0;
                                cx.stop_propagation();
                            }
                            "end" => {
                                this.input_cursor = this_char_count(this.input_text.as_str());
                                cx.stop_propagation();
                            }
                            "up" => {
                                // TUI parity: when the caret sits on the first
                                // row, Up recalls the previous prompt. Inside a
                                // multi-line draft we use it for line navigation.
                                if this.input_cursor_at_first_line() {
                                    this.recall_prev_prompt();
                                } else {
                                    this.move_to_prev_line();
                                }
                                cx.stop_propagation();
                            }
                            "down" => {
                                if this.input_cursor_at_last_line() {
                                    this.recall_next_prompt();
                                } else {
                                    this.move_to_next_line();
                                }
                                cx.stop_propagation();
                            }
                            "escape" => {
                                // Esc cancels the current draft (and any open
                                // popup). Match the TUI's behavior of treating
                                // Esc as a no-op when the buffer is empty so we
                                // don't swallow focus moves system-wide.
                                this.input_text = SharedString::new("");
                                this.input_cursor = 0;
                                this.slash_popup_visible = false;
                                this.prompt_history_cursor = None;
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                        // We deliberately do NOT insert printable characters
                        // here. zed's editor pattern is to leave printable key
                        // dispatch to macOS — when the listener doesn't claim
                        // the key, the macOS shim falls through to
                        // `[inputContext handleEvent:]` which in turn calls
                        // `insertText:` on our registered `ElementInputHandler`,
                        // routing the same char through `replace_text_in_range`.
                        // That keeps the IME-aware path as the single source of
                        // truth: ASCII letters and IME-marked CJK both flow
                        // through the same handler, eliminating the previous
                        // duplication where our listener inserted ahead of the
                        // IME's commit.
                    })),
            )
            .child(ImeInputElement {
                view: view_entity,
                focus: input_focus_clone,
            })
    }

    fn render_slash_popup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = slash_matches(self.input_text.as_str());
        if matches.is_empty() {
            return div().into_any_element();
        }
        let row_count = matches.len();
        let slash_popup_scroll = self.slash_popup_scroll.clone();
        let view_entity = cx.entity().clone();
        // We can't read `self` inside the processor closure (uniform_list outlives
        // the borrow), so capture the index once.
        let current_selected = self.slash_popup_selected;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .mx_5()
            .mt_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(dark::BORDER))
            .bg(rgb(dark::BUTTON_BG))
            .max_h(px(200.0))
            .overflow_y_hidden()
            .text_sm()
            .child(
                div().h(px(180.)).child(
                    uniform_list(
                        "slash-cmd-list",
                        row_count,
                        cx.processor(move |_this, range: std::ops::Range<usize>, _window, _cx| {
                            range
                                .map(|idx| {
                                    let (name, desc, _needs_arg) = matches[idx];
                                    let view_for_click = view_entity.clone();
                                    div()
                                        .id(("slash-cmd", idx))
                                        .flex()
                                        .gap_3()
                                        .px_2()
                                        .py_1()
                                        .rounded_sm()
                                        .bg(if idx == current_selected {
                                            rgb(dark::BUTTON_HOVER)
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .border_1()
                                        .when(idx == current_selected, |d| {
                                            d.border_color(rgb(dark::ACCENT))
                                        })
                                        .when(idx != current_selected, |d| {
                                            d.border_color(rgba(0x00000000))
                                        })
                                        .child(
                                            div()
                                                .text_color(rgb(dark::TEXT))
                                                .text_sm()
                                                .min_w(px(80.0))
                                                .child(name),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            div()
                                                .text_color(rgb(dark::TEXT_MUTED))
                                                .text_sm()
                                                .child(desc),
                                        )
                                        .on_click(move |_ev, _window, cx| {
                                            view_for_click.update(cx, |state, cx| {
                                                state.input_text =
                                                    SharedString::new(name.to_string());
                                                state.input_cursor = name.chars().count();
                                                state.slash_popup_visible = false;
                                                cx.notify();
                                            });
                                        })
                                })
                                .collect()
                        }),
                    )
                    .track_scroll(&slash_popup_scroll)
                    .h_full(),
                ),
            )
            .into_any_element()
    }
}

// === IME (Unicode / Chinese) input plumbing =====================================
//
// gpui delivers composing text from IMEs (CJK pinyin, Japanese kana, Korean etc,
// plus macOS autocorrect) through `InputHandler` callbacks attached to a
// `FocusHandle` during the paint phase. Plain ASCII keystrokes go through our
// `on_key_down` listener above, but for IME to fire we need to also implement
// [`EntityInputHandler`] on `ShellState` and register the handler in element
// paint via [`ImeInputElement`].
//
// All UTF ranges exchanged with the platform are UTF-16 indices (matching
// `NSTextInputClient` / Win32 IMM / `GtkIMContext`). We model our cursor as a
// character (codepoint) index, so conversion happens on selection / replacement.

impl EntityInputHandler for ShellState {
    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }

    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        Some(self.input_text.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let buf = self.input_text.to_string();
        let utf16_start: usize = buf
            .chars()
            .take(self.input_cursor)
            .map(|c| c.len_utf16())
            .sum();
        let utf16_end: usize = buf
            .chars()
            .take(self.input_cursor)
            .map(|c| c.len_utf16())
            .sum();
        Some(UTF16Selection {
            range: utf16_start..utf16_end,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        // When we're composing preedit text via `replace_and_mark_text_in_range`,
        // mirror that range back to macOS so its `hasMarkedText` query returns
        // YES. This is what makes NSTextInputClient treat us as actively
        // composing — which in turn suppresses `NSResponder::keyDown:` for
        // printable keys and routes them through the IME. Without this, each
        // pinyin keystroke both inserted via our `on_key_down` listener AND
        // queued on the IME side, producing duplicated ASCII before the
        // committed Chinese character landed.
        self.ime_mark_utf16.clone()
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // The platform is asking us to drop the in-flight IME composition. We
        // don't splice anything (the marked text remains in `input_text` from
        // the last `replace_and_mark_text_in_range` call), we just stop
        // claiming composing status so future keystrokes flow back through
        // our `on_key_down` listener.
        self.ime_composing = false;
        self.ime_mark_utf16 = None;
        cx.notify();
    }

    /// Apply a text replacement to `input_text` described by a UTF-16 range,
    /// then move the cursor accordingly. Empty range means "insert at start";
    /// `None` range means "append at end". This is the IME-commit path: it
    /// clears composing state so the `on_key_down` listener resumes normal
    /// handling of subsequent keystrokes.
    fn replace_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // When the platform passes `None` it's saying "insert at the cursor";
        // interpret that with our current cursor position so the commit lands
        // where the user is editing, not at the end of the buffer.
        let (start_char, end_char) =
            utf16_range_to_char_indices(&self.input_text, range, self.input_cursor);
        self.splice_text(start_char, end_char, text);
        self.ime_composing = false;
        self.ime_mark_utf16 = None;
        self.refresh_slash_popup();
        cx.notify();
    }

    /// IME composition replace. Treat the new text as the in-progress preedit
    /// and publish it back via `marked_text_range`, so macOS sees us as
    /// composing and stops delivering printable keystrokes to our
    /// `on_key_down` listener. The fresh mark range is the UTF-16 span of the
    /// newly inserted text in the rewritten buffer.
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        _selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Same `None`-means-cursor handling as `replace_text_in_range`: when
        // the IME kicks in mid-stream (cursor not at end of buffer), we want
        // the new preedit text to land at the cursor, not blindly appended.
        let (start_char, end_char) =
            utf16_range_to_char_indices(&self.input_text, range, self.input_cursor);
        let start_byte = byte_index_for_char(&self.input_text, start_char);
        let mut buf = self.input_text.to_string();
        let end_byte_splice = byte_index_for_char(&buf, end_char.max(start_char));
        buf.replace_range(start_byte..end_byte_splice, text);
        let new_utf16_start = utf16_width_up_to(&buf, start_byte);
        let new_utf16_len = text.chars().map(char::len_utf16).sum::<usize>();
        self.input_text = SharedString::new(buf);
        self.input_cursor = start_char + text.chars().count();
        self.ime_composing = true;
        self.ime_mark_utf16 = Some(new_utf16_start..new_utf16_start + new_utf16_len);
        self.refresh_slash_popup();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// An invisible element that owns an `EntityInputHandler` registration for the
/// duration of one paint pass. Sits next to the visual input-box div so that
/// when our focus handle is active, gpui routes IME callbacks to our handler.
struct ImeInputElement {
    view: Entity<ShellState>,
    focus: FocusHandle,
}

impl IntoElement for ImeInputElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ImeInputElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        Some("zerostack-ime-input-element".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // Allocate a zero-size layout slot. We don't paint anything; the
        // element exists only so `paint` runs and registers the InputHandler.
        let mut style = Style::default();
        style.size.width = relative(0.).into();
        style.size.height = relative(0.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        _paint_state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
    }
}

fn render_message(msg: &ChatMessage, view_entity: gpui::Entity<ShellState>) -> gpui::AnyElement {
    match msg.role {
        Role::Reasoning => render_reasoning_card(msg),
        Role::Permission => render_permission_card(msg, view_entity),
        _ => render_message_text(msg),
    }
}

/// Render assistant / tool / system text as markdown.
///
/// The TUI's `markdown.rs` turns the source into a styled ANSI stream; here
/// we tokenize via `crate::markdown` and rebuild each block as a small
/// `Div` tree. Inline spans within a paragraph live in the same `flex_row`
/// container so they wrap naturally. Code blocks get a monospace, padded,
/// bordered panel; inline code gets a chip background; bold / italic /
/// strikethrough / links use native gpui text primitives.
fn render_markdown_body(text: &str) -> gpui::AnyElement {
    let blocks: Vec<MarkdownBlock> = parse_markdown(text);
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .min_w_0()
        .text_color(rgb(dark::TEXT))
        .text_sm()
        .children(blocks.into_iter().map(|block| render_markdown_block(block)))
        .into_any_element()
}

fn render_markdown_block(block: MarkdownBlock) -> gpui::AnyElement {
    match block.kind {
        BlockKind::Heading(level) => {
            let size = match level {
                1 => 22.0,
                2 => 19.0,
                3 => 17.0,
                _ => 15.0,
            };
            let top_pad = if level <= 2 { 6.0 } else { 4.0 };
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .w_full()
                .min_w_0()
                .mt(px(top_pad))
                .text_size(px(size))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(dark::TEXT))
                .children(block.spans.into_iter().map(render_markdown_span))
                .into_any_element()
        }
        BlockKind::Paragraph => div()
            .flex()
            .flex_row()
            .flex_wrap()
            .w_full()
            .min_w_0()
            .text_color(rgb(dark::TEXT))
            .children(block.spans.into_iter().map(render_markdown_span))
            .into_any_element(),
        BlockKind::CodeBlock(lang) => {
            let joined: String = block
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>()
                .trim_end_matches('\n')
                .to_string();
            let lang_label = lang.clone();
            div()
                .flex()
                .flex_col()
                .gap_1()
                .w_full()
                .min_w_0()
                .my_1()
                .p_3()
                .rounded_md()
                .bg(rgb(dark::TOOL_BUBBLE_BG))
                .border_1()
                .border_color(rgb(dark::BORDER))
                .when(lang_label.is_some(), |d| {
                    let l = lang_label.unwrap();
                    d.child(
                        div()
                            .text_xs()
                            .text_color(rgb(dark::TEXT_MUTED))
                            .child(l.to_string()),
                    )
                })
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .font_family("ui-monospace")
                        .text_xs()
                        .text_color(rgb(dark::TEXT))
                        .whitespace_normal()
                        .child(joined),
                )
                .into_any_element()
        }
        BlockKind::ListItem(marker) => {
            let prefix = match marker {
                Some(n) => format!("{n}. "),
                None => "\u{2022} ".to_string(),
            };
            div()
                .flex()
                .flex_row()
                .gap_2()
                .w_full()
                .min_w_0()
                .child(
                    div()
                        .text_color(rgb(dark::TEXT_MUTED))
                        .min_w(px(28.0))
                        .child(prefix),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .text_color(rgb(dark::TEXT))
                        .flex_1()
                        .min_w_0()
                        .children(block.spans.into_iter().map(render_markdown_span)),
                )
                .into_any_element()
        }
        BlockKind::BlockQuote => div()
            .flex()
            .flex_row()
            .gap_2()
            .w_full()
            .min_w_0()
            .pl_3()
            .border_l_2()
            .border_color(rgb(dark::ACCENT))
            .text_color(rgb(dark::TEXT_SECONDARY))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .flex_1()
                    .min_w_0()
                    .children(block.spans.into_iter().map(render_markdown_span)),
            )
            .into_any_element(),
        BlockKind::Hr => div()
            .h(px(1.0))
            .w_full()
            .bg(rgb(dark::BORDER))
            .into_any_element(),
    }
}

fn render_markdown_span(span: MarkdownSpan) -> gpui::AnyElement {
    let mut d = div().text_color(rgb(dark::TEXT)).text_sm();
    if span.bold {
        d = d.font_weight(gpui::FontWeight::BOLD);
    }
    if span.italic {
        d = d.italic();
    }
    if span.strikethrough {
        d = d.line_through();
    }
    if span.code {
        d = d
            .font_family("ui-monospace")
            .text_xs()
            .px_1()
            .rounded_sm()
            .bg(rgb(dark::BUTTON_BG));
    }
    if span.link.is_some() {
        d = d.text_color(rgb(dark::ACCENT)).underline();
    }
    d.child(span.text.to_string()).into_any_element()
}

fn render_message_text(msg: &ChatMessage) -> gpui::AnyElement {
    let (bg, label) = match msg.role {
        Role::User => (rgb(dark::USER_BUBBLE_BG), "you"),
        Role::Assistant => (rgb(dark::ASST_BUBBLE_BG), "zerostack"),
        Role::Tool => (rgb(dark::TOOL_BUBBLE_BG), "tool"),
        Role::System => (rgba(0x00000000), "system"),
        _ => unreachable!("render_message_text handles non-special roles"),
    };

    div()
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .min_w_0()
        .bg(bg)
        .px_4()
        .py_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(dark::BORDER))
        .child(
            div()
                .text_xs()
                .text_color(rgb(dark::TEXT_MUTED))
                .child(label),
        )
        .child(render_markdown_body(msg.content.as_str()))
        .into_any_element()
}

/// Reasoning bubble: model output gathered from `ReasoningDelta` events and
/// folded into a single placeholder row. Render as a muted / italic card with
/// a left accent bar so the user can scan past it without losing context
/// (matching how the TUI's statusline prefixes `▌thinking…`).
fn render_reasoning_card(msg: &ChatMessage) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_4()
        .py_2()
        .rounded_md()
        .border_l_2()
        .border_color(rgb(dark::ACCENT))
        .bg(rgba(0x00000000))
        .child(
            div()
                .text_xs()
                .text_color(rgb(dark::TEXT_MUTED))
                .child("thinking…"),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(dark::TEXT_SECONDARY))
                .child(msg.content.clone()),
        )
        .into_any_element()
}

/// Permission card: surfaces the tool the engine wants to call and the
/// arguments it would use, then offers Allow / Deny buttons. The handlers
/// flip the card's role to a system row after the user decides so the audit
/// trail stays in-place. We always show both buttons; the engine treats any
/// un-answered ask as blocking, so ambiguity is worse than an explicit
/// deny.
fn render_permission_card(
    msg: &ChatMessage,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    let ask_id = msg.permission_id.unwrap_or(0);
    let allow_id = ElementId::Name(format!("perm-allow-{ask_id}").into());
    let deny_id = ElementId::Name(format!("perm-deny-{ask_id}").into());
    let view_for_allow = view_entity.clone();
    let view_for_deny = view_entity.clone();

    div()
        .flex()
        .flex_col()
        .gap_2()
        .bg(rgb(dark::TOOL_BUBBLE_BG))
        .px_4()
        .py_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(dark::ACCENT))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(dark::TEXT))
                        .child("permission requested"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(dark::TEXT_MUTED))
                        .child(format!("#{ask_id}")),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(dark::TEXT))
                .child(msg.content.clone()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id(allow_id)
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(dark::ACCENT))
                        .text_color(rgb(dark::APP_BG))
                        .cursor_pointer()
                        .text_sm()
                        .child("Allow")
                        .on_click(move |_ev, _window, cx| {
                            view_for_allow.update(cx, |state, cx| {
                                state.answer_permission(ask_id, true, cx);
                            });
                        }),
                )
                .child(
                    div()
                        .id(deny_id)
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(dark::BUTTON_BG))
                        .text_color(rgb(dark::TEXT))
                        .cursor_pointer()
                        .text_sm()
                        .child("Deny")
                        .on_click(move |_ev, _window, cx| {
                            view_for_deny.update(cx, |state, cx| {
                                state.answer_permission(ask_id, false, cx);
                            });
                        }),
                ),
        )
        .into_any_element()
}

/// Best-effort "human name" for a working directory. Falls back gracefully:
///   - empty input → `(no project)` (handled by callers via a hint field)
///   - bare slash (`/`) or no path component (e.g. `"/"`) → `(root)`
///   - any other path → `Path::file_name()` converted to UTF-8, falling
///     back to the full string if the bytes are not valid UTF-8 (Windows
///     paths in cross-platform runs use lossy decoding in the engine, but
///     at the GUI boundary we tolerate whatever landed in `SessionInfo`).
/// Header (title bar + search input + action buttons) for the sidebar. Kept as
/// a free function so the closures inside can lazily capture all the click
/// entities we need without entangling the sidebar renderer's lifetime.
fn render_sidebar_header(
    view_for_new: gpui::Entity<ShellState>,
    view_for_refresh: gpui::Entity<ShellState>,
    view_for_search: gpui::Entity<ShellState>,
    focus_handle: FocusHandle,
    filter_text: String,
    is_refreshing: bool,
) -> gpui::AnyElement {
    let filter_text_for_render = filter_text.clone();
    let placeholder = filter_text.trim().is_empty();
    let search_id = ElementId::Name("sidebar-search-input".into());

    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_3()
        .border_b_1()
        .border_color(rgb(dark::BORDER))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(dark::TEXT))
                        .child("PROJECTS"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id(ElementId::Name("sidebar-refresh".into()))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(dark::BUTTON_BG))
                                .text_color(if is_refreshing {
                                    rgb(dark::ACCENT)
                                } else {
                                    rgb(dark::TEXT)
                                })
                                .cursor_pointer()
                                .text_xs()
                                .child(if is_refreshing { "syncing…" } else { "↻" })
                                .hover(|this| this.bg(rgb(dark::BUTTON_HOVER)))
                                .on_click({
                                    let view_entity = view_for_refresh.clone();
                                    move |_ev, _window, cx| {
                                        view_entity.update(cx, |state, cx| {
                                            state.refresh_sidebar(cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .id(ElementId::Name("sidebar-new".into()))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(dark::ACCENT))
                                .text_color(rgb(dark::APP_BG))
                                .cursor_pointer()
                                .text_xs()
                                .child("+ New")
                                .hover(|this| this.bg(rgb(dark::BUTTON_HOVER)))
                                .on_click({
                                    let view_entity = view_for_new.clone();
                                    move |_ev, _window, cx| {
                                        view_entity.update(cx, |state, cx| {
                                            let _ = state
                                                .bridge
                                                .send(UserAction::CreateSession { name: None });
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                ),
        )
        .child(
            // The search field itself. We render it as a focused track like a
            // mini text input — clicking focuses it, typing keys mutates the
            // filter via `view_for_search`. Substring-only, never IME-aware
            // (the chat input is the canonical path for Japanese / Chinese).
            div()
                .id(search_id)
                .track_focus(&focus_handle)
                .focus_visible(|d| d.border_color(rgb(dark::ACCENT)))
                .border_1()
                .border_color(rgb(dark::BORDER))
                .rounded_md()
                .px_2()
                .py_1()
                .bg(rgb(dark::INPUT_BG))
                .text_xs()
                .text_color(if placeholder {
                    rgb(dark::TEXT_MUTED)
                } else {
                    rgb(dark::TEXT)
                })
                .cursor_text()
                .child(if placeholder {
                    SharedString::new("⌕  filter sessions / paths")
                } else {
                    SharedString::new(filter_text_for_render.clone())
                })
                .on_click({
                    let view_entity = view_for_search.clone();
                    let focus = focus_handle.clone();
                    move |_ev, window, cx| {
                        // Direct focus on click so the user can type without
                        // an extra Tab. The window reference here is the
                        // standard `&mut Window` arg gpui passes to on_click.
                        focus.focus(window, cx);
                        let _ = view_entity; // silence unused
                    }
                })
                .on_key_down({
                    let view_entity = view_for_search.clone();
                    move |ev: &gpui::KeyDownEvent, _window, cx| {
                        let key = ev.keystroke.key.as_str();
                        let mods = &ev.keystroke.modifiers;
                        view_entity.update(cx, |state, cx| {
                            if key == "escape" || key == "backspace" {
                                if mods.shift
                                    || (mods.control || mods.platform)
                                    || state.sidebar_filter.is_empty()
                                {
                                    state.sidebar_filter = SharedString::new("");
                                } else {
                                    state.backspace_sidebar_filter();
                                }
                            } else if let Some(chars) = ev.keystroke.key_char.as_ref() {
                                for ch in chars.chars() {
                                    if !ch.is_control() {
                                        state.append_sidebar_filter_char(ch);
                                    }
                                }
                            } else if key.chars().count() == 1 {
                                if let Some(first) = key.chars().next() {
                                    if !first.is_control() {
                                        state.append_sidebar_filter_char(first);
                                    }
                                }
                            }
                            cx.notify();
                        });
                        // Don't let the handler leak into the chat input.
                        cx.stop_propagation();
                    }
                }),
        )
        .into_any_element()
}

fn project_label(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return "(no project)".to_string();
    }
    let p = std::path::Path::new(trimmed);
    // `file_name()` returns None for things like `/`, `.`, `..`.
    match p.file_name() {
        Some(name) => name.to_string_lossy().trim().to_string(),
        None => "(root)".to_string(),
    }
}

/// Render a single project-header row in the sidebar tree. Shows a chevron
/// (`▶` / `▼`) plus a tiny folder glyph, the project label, a dimmed full-
/// path hint, and the session count on the right. Clicking anywhere on the
/// row toggles expansion. When the expanded state is being read inside a
/// `uniform_list` closure we have to lift everything into owned values; a
/// single `String` per field keeps that simple.
fn render_project_row(
    group: &ProjectGroup,
    expanded: bool,
    is_thinking: bool,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    let view_entity = view_entity.clone();
    let key = group.key.clone();
    let active = group.is_active;
    let key_for_click = key.clone();
    let chevron = if expanded { "▼" } else { "▶" };
    let key_for_id = key.clone();
    // `ElementId` doesn't have a `(&str, &str)` `From` impl — fall back to a
    // stringified name derived from the (stable) project key.
    let project_id = ElementId::Name(format!("project-row:{key_for_id}").into());
    // Folder glyph becomes a tiny accent bubble when at least one session
    // inside this group is currently being driven by the agent — gives the
    // user a per-project "something is alive here" cue without scanning
    // every child row.
    let folder_glyph = "📁";
    let thinking_dot_color = if is_thinking {
        rgb(dark::ACCENT)
    } else {
        rgba(0x00000000)
    };

    div()
        .id(project_id)
        .flex()
        .items_center()
        .w_full()
        .gap_2()
        .px_3()
        .py_2()
        .mx_1()
        .cursor_pointer()
        .bg(if active {
            rgb(dark::BUTTON_HOVER)
        } else {
            rgba(0x00000000)
        })
        .border_l_2()
        .border_color(if active {
            rgb(dark::ACCENT)
        } else {
            rgba(0x00000000)
        })
        .hover(|this| this.bg(rgb(dark::BUTTON_BG)))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(dark::TEXT_MUTED))
                        .child(chevron),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if active {
                            rgb(dark::ACCENT)
                        } else {
                            rgb(dark::TEXT_SECONDARY)
                        })
                        .child(folder_glyph),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_0()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(dark::TEXT))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .truncate()
                        .child(SharedString::new(group.label.clone())),
                )
                .when(!group.hint.is_empty() && group.hint != group.label, |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(rgb(dark::TEXT_MUTED))
                            .truncate()
                            .child(SharedString::new(group.hint.clone())),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(dark::TEXT_MUTED))
                        .child(format!("{}", group.session_indices.len())),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .size_2p5()
                        .rounded_full()
                        .bg(thinking_dot_color),
                ),
        )
        .on_click(move |_ev, _window, cx| {
            view_entity.update(cx, |state, cx| {
                state.toggle_project_expanded(key_for_click.clone(), cx);
            });
        })
        .into_any_element()
}

/// Render a child session row beneath an expanded project header. The
/// session's name is the primary label; the secondary line shows the
/// provider/model and message count so users can scan at a glance. Click
/// switches the active session. When `is_thinking` is true and matches the
/// active session we render a tiny animated-style "thinking" pill so the
/// user can tell from a glance that the agent is still streaming.
fn render_session_row(
    session: &SessionInfo,
    is_active: bool,
    is_thinking: bool,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    let view_entity = view_entity.clone();
    let id_owned = session.id.clone();

    let name: SharedString = if session.name.is_empty() {
        SharedString::new("(untitled)")
    } else {
        SharedString::new(session.name.as_str())
    };

    let last_message: SharedString = if session.last_message.is_empty() {
        SharedString::new("no messages yet")
    } else {
        SharedString::new(session.last_message.as_str())
    };

    let primary_color = if is_active {
        rgb(dark::TEXT)
    } else {
        rgb(dark::TEXT_SECONDARY)
    };

    let secondary_color = if is_active {
        rgb(dark::TEXT_SECONDARY)
    } else {
        rgb(dark::TEXT_MUTED)
    };

    div()
        .id(ElementId::Name(
            format!("session-row:{}", session.id.as_str()).into(),
        ))
        .flex()
        .flex_col()
        .w_full()
        .gap_0()
        .px_3()
        .pl_5()
        .py_1p5()
        .mx_1()
        .rounded_sm()
        .cursor_pointer()
        .bg(if is_active {
            rgb(dark::BUTTON_HOVER)
        } else {
            rgba(0x00000000)
        })
        .border_l_2()
        .border_color(if is_active {
            rgb(dark::ACCENT)
        } else {
            rgba(0x00000000)
        })
        .hover(|this| this.bg(rgb(dark::BUTTON_BG)))
        .child(
            div()
                .flex()
                .flex_row()
                .w_full()
                .items_center()
                .gap_2()
                .overflow_x_hidden()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .truncate()
                        .text_color(primary_color)
                        .child(name.clone()),
                )
                .when(is_active && is_thinking, |d| {
                    // Subtle "thinking" pill next to the active row.
                    // Pairs with the chat column's footer indicator so the
                    // user can see engine status from either pane.
                    d.child(
                        div()
                            .flex_shrink_0()
                            .px_1p5()
                            .py_0p5()
                            .rounded_sm()
                            .bg(rgb(dark::ACCENT))
                            .text_xs()
                            .text_color(rgb(dark::APP_BG))
                            .child("thinking"),
                    )
                }),
        )
        .child(
            div()
                .text_xs()
                .text_color(secondary_color)
                .truncate()
                .child(last_message.clone()),
        )
        .on_click(move |_ev, _window, cx| {
            view_entity.update(cx, |state, _cx| {
                let _ = state.bridge.send(UserAction::SwitchSession {
                    session_id: id_owned.clone(),
                });
            });
        })
        .into_any_element()
}

/// Convert a UTF-16 range (the unit the platform IME issues ranges in) over
/// `s` into a `(start, end)` pair of character (codepoint) indices. We pair
/// that with `cursor_chars` so that `None` (the platform's "insert at cursor"
/// sentinel) is properly interpreted as `cursor_chars..cursor_chars` rather
/// than at the end of the buffer. The indices are clipped/ordered so the
/// result is always a valid range.
fn utf16_range_to_char_indices(
    s: &str,
    range: Option<std::ops::Range<usize>>,
    cursor_chars: usize,
) -> (usize, usize) {
    let total = s.chars().count();
    let r = match range {
        Some(r) => r,
        None => return (cursor_chars.min(total), cursor_chars.min(total)),
    };
    let mut start_utf16 = r.start;
    let mut end_utf16 = r.end;
    let mut start_chars = 0usize;
    let mut end_chars = 0usize;
    let mut start_done = false;
    let mut end_done = false;
    for c in s.chars() {
        let cu = c.len_utf16();
        if !start_done {
            if start_utf16 >= cu {
                start_utf16 -= cu;
                start_chars += 1;
            } else {
                start_done = true;
            }
        }
        if !end_done {
            if end_utf16 >= cu {
                end_utf16 -= cu;
                end_chars += 1;
            } else {
                end_done = true;
            }
        }
        if start_done && end_done {
            break;
        }
    }
    let s_clip = start_chars.min(total);
    let e_clip = end_chars.min(total);
    (s_clip.min(e_clip), s_clip.max(e_clip))
}

/// Map a character (codepoint) index into `s` to the byte offset where the
/// character begins. Out-of-range indices clamp to `s.len()`.
fn byte_index_for_char(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

/// Count UTF-16 units in `s` strictly before byte offset `byte_end`. Useful
/// when we need to publish a UTF-16 range that points into a buffer we
/// already mutated.
fn utf16_width_up_to(s: &str, byte_end: usize) -> usize {
    let byte_end = byte_end.min(s.len());
    s[..byte_end].chars().map(char::len_utf16).sum()
}

/// Char-count helper that lets the key-handler listener read the buffer
/// length without a method call to `self`. We use this for things like
/// jumping to EOL.
fn this_char_count(s: &str) -> usize {
    s.chars().count()
}

/// Split a `SharedString` at the Nth character index. `chars_before` is clamped to
/// `s.chars().count()`; the second chunk is always a valid UTF-8 string.
fn split_at_char(s: &SharedString, chars_before: usize) -> (SharedString, SharedString) {
    let buf = s.to_string();
    let total = buf.chars().count();
    let idx = chars_before.min(total);
    match buf.char_indices().nth(idx) {
        Some((byte_idx, _)) => {
            let (a, b) = buf.split_at(byte_idx);
            (
                SharedString::new(a.to_string()),
                SharedString::new(b.to_string()),
            )
        }
        None => (s.clone(), SharedString::new("")),
    }
}

/// Render the input line as `prefix` + optional block cursor + `suffix`. When
/// `is_empty` we additionally draw the placeholder as a muted child; we render the
/// accent cursor over it as a styled bar. The cursor is omitted when `cursor_visible`
/// is `false` (during the off half of the blink).
fn render_input_text(
    prefix: SharedString,
    suffix: SharedString,
    is_empty: bool,
    cursor_visible: bool,
) -> gpui::Div {
    let mut inner = div().flex().items_center().gap_0();
    if is_empty {
        inner = inner.child(
            div()
                .text_color(rgb(dark::TEXT_MUTED))
                .child(SharedString::new("Ask zerostack…")),
        );
    } else {
        inner = inner.child(
            div()
                .flex()
                .items_center()
                .gap_0()
                .child(prefix)
                .child(
                    div()
                        .w(px(2.))
                        .h(px(16.))
                        .my(px(2.))
                        .bg(if cursor_visible {
                            rgb(dark::ACCENT)
                        } else {
                            rgba(0x00000000)
                        })
                        .rounded_sm(),
                )
                .child(suffix),
        );
    }
    inner
}

/// Spawn a 500ms blink timer that toggles [`ShellState::cursor_visible`]. The timer
/// runs continuously while the entity is alive; render is gated on the field, so the
/// cost when the cursor is "off" is just a `false -> true` flip followed by a notify.
fn spawn_cursor_blink(view: gpui::Entity<ShellState>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            // Entity may have been dropped between notifications; we ignore the
            // result since the timer is best-effort and self-recovering.
            let _ = cx.update(|cx| {
                view.update(cx, |state, cx| {
                    state.cursor_visible = !state.cursor_visible;
                    cx.notify();
                });
            });
        }
    })
    .detach();
}

/// Spawn a recurring poll task. We purposely use a coarse 33ms cycle (≈30Hz): faster
/// than human-perceptible for streaming deltas, and far cheaper than a render loop.
fn start_poll_loop(view: gpui::Entity<ShellState>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(33))
                .await;
            // `update` on AsyncApp takes `&mut self` and returns the closure's
            // result directly. We ignore it; if the entity has been torn down we
            // likely can't do anything useful from a poll loop anyway.
            let _ = cx.update(|cx| {
                view.update(cx, |state, cx| {
                    state.poll_bridge(cx);
                    cx.notify();
                });
            });
        }
    })
    .detach();
}

impl Render for ShellState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Focus the input box on the very first paint so the user can start typing
        // without an extra click. Subsequent renders let focus flow naturally from
        // clicks on the field.
        if !self.has_focused_input {
            self.input_focus.focus(window, cx);
            self.has_focused_input = true;
        }
        // Drain engine events on every render so streaming deltas flow through
        // without waiting for the next timer tick.
        self.poll_bridge(cx);

        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(rgb(dark::APP_BG))
            .text_color(rgb(dark::TEXT))
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.render_chat(cx))
                    .child(self.render_input(cx)),
            )
    }
}

/// Program entry: build the engine on a background thread, drain its events into the
/// root view from a 30Hz tick, quit cleanly when the last window closes.
pub fn run() {
    let (model, provider) = resolve_provider_model();
    let bridge = GuiBridge::launch(
        &model,
        &provider,
        zerostack_core::permission::SecurityMode::Yolo,
    );

    application().run(move |cx: &mut App| {
        let bridge_for_state = bridge;
        let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);

        let view = cx.new(move |cx| ShellState::new(bridge_for_state, cx));
        start_poll_loop(view.clone(), cx);
        spawn_cursor_blink(view.clone(), cx);

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::new("zerostack-gui")),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                ..Default::default()
            },
            |_window, _cx| view.clone(),
        )
        .unwrap();
        cx.activate(true);
    });
}

/// Program entry: build the engine on a background thread, drain its events into the
/// root view from a 30Hz tick, quit cleanly when the last window closes.
pub fn resolve_provider_model() -> (String, String) {
    let (cfg, _is_first) = zerostack_core::config::load();
    let cli = zerostack_core::cli::Cli::default();
    (
        cli.resolve_model(&cfg).to_string(),
        cli.resolve_provider(&cfg).to_string(),
    )
}
