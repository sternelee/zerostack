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
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::highlight::{self, TokenClass};
use compact_str::CompactString;
use gpui::{
    AnimationExt, App, Bounds, Context, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, FollowMode, GlobalElementId, KeyDownEvent, LayoutId, ListAlignment, ListOffset,
    ListState, Pixels, Render, ScrollHandle, ScrollStrategy, SharedString, Style, StyledText,
    TextRun, TitlebarOptions, UTF16Selection, UniformListScrollHandle, Window, WindowBounds,
    WindowOptions, div, font, list, prelude::*, px, relative, rgb, rgba, size, uniform_list,
};
use gpui_platform::application;
use zerostack_core::events::CoreEvent;
use zerostack_core::events::SessionInfo;
use zerostack_core::events::UserAction;

use crate::GuiBridge;
use crate::markdown::{BlockKind, MarkdownBlock, MarkdownSpan, parse_markdown};
use crate::theme::dark;
use crate::tool_utils;

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
    /// Structured metadata for `Role::Tool` messages so we can render foldable
    /// tool cards with name / args / status / result. When `None` the renderer
    /// falls back to a plain text bubble (legacy / pre-structured events).
    tool_meta: Option<ToolMeta>,
    /// When the message was created in this process, for hover footers.
    /// `None` for history replayed from disk.
    sent_at: Option<std::time::Instant>,
}

/// Lifecycle state of a tool invocation surfaced by the engine. Driven by
/// the [`CoreEvent::ToolCall`] and [`CoreEvent::ToolResult`] events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    /// Tool was dispatched but the engine has not yet emitted a result.
    Pending,
    /// Tool finished successfully. `result` carries the output.
    Ok,
}

#[derive(Clone, Debug)]
pub struct ToolMeta {
    pub name: SharedString,
    /// One-line summary of the primary argument (path / command / pattern).
    /// Mirrors the TUI's `format_tool_call_summary` so the GUI bubble shows
    /// the same thing users see on the CLI.
    pub args_summary: SharedString,
    pub status: ToolStatus,
    /// Output from [`CoreEvent::ToolResult`]. Empty while the tool is still
    /// running or if the result was elided due to its size.
    pub result: SharedString,
    /// Whether the user has clicked the card to reveal the result body.
    /// Defaults to `false`; toggled on click in `render_tool_item`.
    pub expanded: bool,
    /// Instant at which the tool was dispatched (for elapsed-time display).
    /// `None` for tool cards loaded from session history (pre-existing).
    pub pending_since: Option<std::time::Instant>,
}

impl ChatMessage {
    pub fn user(text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::User,
            content: text.into(),
            permission_id: None,
            tool_meta: None,
            sent_at: Some(std::time::Instant::now()),
        }
    }

    pub fn assistant(text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::Assistant,
            content: text.into(),
            permission_id: None,
            tool_meta: None,
            sent_at: Some(std::time::Instant::now()),
        }
    }

    pub fn tool(text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::Tool,
            content: text.into(),
            permission_id: None,
            tool_meta: None,
            sent_at: Some(std::time::Instant::now()),
        }
    }

    pub fn system(text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::System,
            content: text.into(),
            permission_id: None,
            tool_meta: None,
            sent_at: Some(std::time::Instant::now()),
        }
    }

    /// Build a reasoning row that accumulates chain-of-thought deltas. The
    /// renderer folds it behind a "thinking…" header; users can expand it.
    pub fn reasoning(text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::Reasoning,
            content: text.into(),
            permission_id: None,
            tool_meta: None,
            sent_at: Some(std::time::Instant::now()),
        }
    }

    pub fn permission(id: u64, text: impl Into<SharedString>) -> Self {
        Self {
            role: Role::Permission,
            content: text.into(),
            permission_id: Some(id),
            tool_meta: None,
            sent_at: Some(std::time::Instant::now()),
        }
    }

    /// Build a structured tool card used to render foldable tool bubbles.
    pub fn tool_card(name: impl Into<SharedString>, args_summary: impl Into<SharedString>) -> Self {
        Self {
            role: Role::Tool,
            content: SharedString::new(""),
            permission_id: None,
            tool_meta: Some(ToolMeta {
                name: name.into(),
                args_summary: args_summary.into(),
                status: ToolStatus::Pending,
                result: SharedString::new(""),
                expanded: false,
                pending_since: Some(std::time::Instant::now()),
            }),
            sent_at: Some(std::time::Instant::now()),
        }
    }

    /// Try to read the structured tool metadata. Returns `None` for any
    /// role other than `Role::Tool`, or for tool bubbles that were created
    /// before the structured form was introduced.
    pub fn tool_meta(&self) -> Option<&ToolMeta> {
        self.tool_meta.as_ref()
    }

    /// Mark the tool as finished and attach the engine output. A no-op on
    /// non-tool messages and on tool bubbles that lack structured metadata.
    pub fn complete_tool(&mut self, output: impl Into<SharedString>) {
        if let Some(meta) = self.tool_meta.as_mut() {
            meta.status = ToolStatus::Ok;
            meta.result = output.into();
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
/// The static `SLASH_COMMANDS` table only mirrors what the engine itself
/// handles; commands registered by Wasm extensions end up in the picker
/// alongside these (appended by [`slash_matches`]). The TUI does the much
/// more dramatic split in `src/ui/pickers/list.rs` (BASE_COMMANDS plus a
/// long `available_commands()` ladder per feature flag) — here we keep the
/// engine-handled commands static because the GUI doesn't rerun the engine
/// on each keystroke and the engine still owns the dispatch.
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

/// Read the slash commands currently registered by Wasm extensions.
///
/// Mirrors `crate::extension::registry::extension_command_names` from
/// `src/ui/pickers/list.rs` and `crates/core/src/extension/registry.rs`,
/// which the TUI also calls. The GUI uses its own copy because it lives
/// in a different binary (`zerostack-gui`) that doesn't link against the
/// CLI crate, and the registry is a `OnceLock<Arc<Mutex<…>>>` — we just
/// need to forward the names that other code paths also see.
///
/// Returns an empty `Vec` when the engine feature for extensions is off
/// (the GUI binary ships with `extensions` enabled by default but a
/// `--no-default-features` build or a re-export without it shouldn't
/// break the picker).
fn extension_slash_entries() -> Vec<(String, String, bool)> {
    #[cfg(feature = "extensions")]
    {
        zerostack_core::extension::registry::extension_command_names()
            .into_iter()
            .map(|name| {
                (
                    name.clone(),
                    "extension command".to_string(),
                    // Treat extension commands as atomic from the GUI's
                    // perspective: the user types the arg directly into the
                    // input box (matching how `/model foo` already works
                    // for engine-handled commands). Some extensions install
                    // arg-less commands and some install commands that
                    // *need* body text; we don't introspect either, so
                    // both behaviours round-trip through the same
                    // `submit_input` fall-through.
                    false,
                )
            })
            .collect()
    }
    #[cfg(not(feature = "extensions"))]
    {
        Vec::new()
    }
}

/// Return the subset of `[SLASH_COMMANDS] + extension commands` whose name
/// starts with `prefix`, or — when `prefix` is the literal command prefix of
/// an arg-requiring command followed by a single space — the available
/// argument choices for that command. Returning the docs-only "atomic" flag
/// so the key handler treats each choice as immediately actionable (Tab
/// inserts the chosen value into the buffer; Enter inserts + submits),
/// avoiding an extra keystroke versus the TUI's "type then press enter"
/// flow.
///
/// Returns `Vec<(String, String, bool)>` instead of static refs so we can
/// heap-allocate the extension names alongside the static engine-handled
/// ones — the popup renderer doesn't care and `SharedString::new(name)`
/// clones cheaply.
fn slash_matches(prefix: &str) -> Vec<(String, String, bool)> {
    // Recognise the boundary where the user just selected an arg-needing
    // command and typed a space — this is where we switch to a chooser. Each
    // choice item's name is just the bare value (`anthropic`) and the key
    // handler rebuilds the full `/provider anthropic` string from the
    // arg-prefix; the description is shown as the choice label.
    if let Some(arg_prefix) = strip_command_arg_prefix(prefix) {
        let choices: &[(&str, &str)] = match arg_prefix {
            "/provider" => PROVIDER_CHOICES,
            "/mode" => MODE_CHOICES,
            _ => &[],
        };
        return choices
            .iter()
            .map(|(value, desc)| (value.to_string(), desc.to_string(), false))
            .collect();
    }
    let mut out: Vec<(String, String, bool)> = SLASH_COMMANDS
        .iter()
        .map(|(name, desc, needs_arg)| (name.to_string(), desc.to_string(), *needs_arg))
        .filter(|(name, _, _)| name.starts_with(prefix))
        .collect();
    for (name, desc, atomic) in extension_slash_entries() {
        if name.starts_with(prefix) && !out.iter().any(|(existing, _, _)| existing == &name) {
            out.push((name, desc, atomic));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Return the arg-needing slash command whose name precedes the first space
/// in `prefix`. The result is `Some("/provider")` for inputs like
/// "/provider "/provider an"; `None` otherwise. Leading slashes are kept so
/// the caller can compare against `SLASH_COMMANDS` directly.
fn strip_command_arg_prefix(input: &str) -> Option<&'static str> {
    if !input.starts_with('/') {
        return None;
    }
    let head = input.split_whitespace().next().unwrap_or(input);
    match head {
        "/provider" => Some("/provider"),
        "/mode" => Some("/mode"),
        _ => None,
    }
}

/// Provider options offered when `/provider<space>` is typed. Mirrors the
/// providers routed by `AnyClient::provider_name` in `crates/core/src/provider.rs`.
const PROVIDER_CHOICES: &[(&str, &str)] = &[
    ("anthropic", "Anthropic Claude"),
    ("openai", "OpenAI / compatible"),
    ("gemini", "Google Gemini"),
    ("openrouter", "OpenRouter"),
    ("ollama", "Local Ollama"),
];

/// Permission-mode options offered when `/mode<space>` is typed. Mirrors the
/// `SecurityMode` enum in `crates/core/src/permission/mod.rs` (the engine
/// reports the current mode as one of these strings).
const MODE_CHOICES: &[(&str, &str)] = &[
    ("yolo", "auto-run any tool call"),
    ("standard", "ask outside the cwd"),
    ("restrictive", "ask every tool call"),
    ("readonly", "deny writes & commands"),
    ("guarded", "deny outside a trust list"),
];

#[cfg(test)]
mod picker_tests {
    use super::*;

    #[test]
    fn commands_picker_filters_by_prefix() {
        let v = slash_matches("/");
        assert!(v.iter().any(|(n, _, _)| n == "/clear"));
    }

    #[test]
    fn provider_picker_offers_arg_chooser() {
        let v = slash_matches("/provider ");
        let collected: Vec<String> = v.iter().map(|(n, _, _)| n.clone()).collect();
        assert_eq!(
            collected,
            vec![
                "anthropic".to_string(),
                "openai".to_string(),
                "gemini".to_string(),
                "openrouter".to_string(),
                "ollama".to_string(),
            ]
        );
    }

    #[test]
    fn mode_picker_offers_arg_chooser() {
        let v = slash_matches("/mode ");
        assert!(v.iter().any(|(n, _, _)| n == "yolo"));
        assert_eq!(v.len(), 5);
    }

    #[test]
    fn commands_with_picker_for_unrelated_input_returns_empty() {
        let v = slash_matches("/xyz ");
        assert!(v.is_empty());
    }
}

// Close-confirm tests live behind a manual run — the modal flow needs a
// real GPUI window and a real platform close event to exercise
// end-to-end (latch flips, modal opens on `on_window_should_close`,
// Esc cancels, Quit arms the latch, second close exits). Covering it
// here would require pulling in gpui::TestAppContext plus a real
// AnyWindowHandle to feed `Window::on_window_should_close`, which
// crosses into "running a process" territory rather than a unit test.
// The `[rollback]` checks below live in the manual QA path: install
// with `cargo install --path . --debug`, run, click the red dot, hit
// Esc inside the modal, hit Quit, repeat.

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
    /// Focus handle for the model picker's search field. The field grabs
    /// focus when the panel opens so typing filters immediately; Up/Down/
    /// Enter/Esc are claimed by the input-box listener (higher priority).
    model_picker_search_focus: FocusHandle,
    /// Scroll handle for the model picker's list; keyboard navigation scrolls
    /// the highlighted row into view.
    model_picker_scroll: ScrollHandle,
    /// Overlay-scrollbar state for the model picker's list.
    model_picker_scrollbar: std::rc::Rc<crate::scrollbar::ScrollbarState>,
    /// Scroll handle for the file picker's list (same purpose).
    file_picker_scroll: ScrollHandle,
    /// Overlay-scrollbar state for the file picker's list.
    file_picker_scrollbar: std::rc::Rc<crate::scrollbar::ScrollbarState>,
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
    /// Virtualized list state for the chat transcript. Bottom-aligned so the
    /// tail stays in view; rows are measured lazily and only the visible
    /// window is laid out each frame (long sessions stay cheap).
    chat_list: ListState,
    /// Number of rows the virtualized list was last synced to; used to detect
    /// append-only growth (streaming) vs a full reset (session switch).
    chat_list_rows: usize,
    /// Cached flattened rows for the current chat, rebuilt in `render_chat`.
    /// The `list` render closure is `'static`, so it reads rows through the
    /// entity rather than borrowing the view.
    chat_rows: Vec<ChatRow>,
    /// Overlay-scrollbar state for the chat list (AppKit-style reveal/fade).
    chat_scrollbar: std::rc::Rc<crate::scrollbar::ScrollbarState>,
    /// Whether new content should auto-scroll the chat to the bottom. Defaults
    /// `true` (follow-tail on first session), flips to `false` whenever the user
    /// actively scrolls away — either via the mouse wheel, PageUp/Shift+Up, or
    /// Cmd/Ctrl+Home to jump to the top. Re-engages when the user scrolls back
    /// down to the bottom (wheel down, PageDown, Cmd/Ctrl+End) so an incoming
    /// agent reply doesn't strand them mid-history.
    chat_follow_tail: bool,

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
    /// Whether each reasoning card is expanded (keyed by chat index). While a
    /// turn is live the card defaults to expanded; once settled it folds to a
    /// one-line "Thought for Ns" header.
    reasoning_expanded: std::collections::HashMap<usize, bool>,
    /// Whether each tool-activity run is expanded (keyed by the first chat
    /// index of the run). Defaults to expanded while the turn is live.
    activity_expanded: std::collections::HashMap<usize, bool>,
    /// Turn fold: when a turn finishes with tool activity, everything before
    /// the final text answer folds into one "Worked for Ns" divider.
    /// `turn_fold_after` is the chat index just before the final answer;
    /// `turn_fold_expanded` maps that index to whether the work is shown.
    turn_fold_after: Option<usize>,
    turn_fold_expanded: std::collections::HashMap<usize, bool>,
    /// Elapsed seconds of the turn that produced `turn_fold_after`, for the
    /// "Worked for Ns" label.
    turn_fold_elapsed: f32,
    /// When the current turn started (for "Working for 9s" elapsed display).
    turn_started_at: Option<std::time::Instant>,
    /// Whether the current turn has produced any tool / reasoning activity
    /// (so a quick answer doesn't get a pointless fold divider).
    turn_had_activity: bool,
    /// Cache of parsed markdown blocks per chat index, invalidated when the
    /// source text changes. `(content_len_at_parse, blocks)`.
    md_cache: std::collections::HashMap<usize, (usize, std::sync::Arc<Vec<MarkdownBlock>>)>,
    /// Message whose footer copy button is currently showing the "copied ✓"
    /// state (message index, when it was pressed).
    copied_message: Option<(usize, std::time::Instant)>,
    /// Code block currently showing a "copied ✓" state on its copy button.
    /// Keyed by the code block's (`message index`, `block index`) so multiple
    /// blocks in one message can each have their own feedback.
    copied_code: Option<((usize, usize), std::time::Instant)>,
    /// Open message context menu (right-click on a message row): the message
    /// index plus the window position where it should appear.
    msg_menu: Option<MsgMenuState>,
    /// Esc two-step stop: first Esc while streaming arms this for 3s (the
    /// send button shows "Esc"), second Esc cancels. Mirrors Waku's
    /// `escape_stop_armed` confirmation.
    escape_stop_armed: bool,

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

    /// True while the close-confirmation modal is on screen. The platform
    /// window-close callback flips this on (and returns `false` from
    /// `Window::on_window_should_close` so the window stays open until the
    /// user resolves the dialog). `/quit` and `/exit` slash commands bypass
    /// the modal by design — those are deliberate actions, not an
    /// accidental close-button tap.
    close_confirm_visible: bool,
    /// Latched once the user has clicked "Quit" in the confirm modal. The
    /// window-close callback checks this on the next platform close event
    /// and returns `true` so the close goes through. Without this latch, the
    /// modal would re-open indefinitely on each cx.quit() round.
    close_confirm_armed: bool,
    /// Focus target for the confirm modal. Distinct from the input and
    /// sidebar handles so we can hand focus to it on open and recover it on
    /// dismiss without confusing the existing focus paths.
    close_confirm_focus: FocusHandle,

    // === Input-bar picker state ===
    /// Cached quick-model entries from the resolved config, sorted by
    /// display name. Loaded once at startup; the chip-row click handler
    /// toggles `model_picker_visible` to surface this list above the input
    /// box. The entries correspond to what `/model <provider/model>`
    /// accepts — calling `UserAction::SetModel { model }` flips the agent
    /// without us needing to manage the swap in the GUI directly.
    quick_models: Vec<QuickModelEntry>,
    /// Listing of the current working directory, refreshed once at startup
    /// (cheap `read_dir` cap of 64 entries). Drives the `+` chip's file
    /// picker; clicking a row fires `UserAction::AddFile { path }` which
    /// the engine resolves absolutely and pushes into `context.extra_files`.
    cwd_files: Vec<String>,
    /// Files currently injected into the agent context.
    context_files: Vec<String>,
    model_picker_visible: bool,
    model_picker_selected: usize,
    /// Free-text filter for the model picker. Empty shows every cached
    /// quick model; otherwise rows are matched case-insensitively against
    /// name / provider / `provider/model`.
    model_picker_query: SharedString,
    file_picker_visible: bool,
    file_picker_selected: usize,
    /// Free-text filter for the file picker. Empty shows the full cwd list;
    /// otherwise rows are matched case-insensitively against the path.
    file_picker_query: SharedString,
    /// Focus handle for the file picker's search field (mirrors the model
    /// picker's search box).
    file_picker_search_focus: FocusHandle,
    /// Whether the permission-mode menu (from the composer's mode chip) is
    /// open. Selecting a row sends `/mode <name>` through the engine.
    mode_picker_visible: bool,
    /// Whether the MCP management panel (from the `+ MCP` chip) is open.
    mcp_picker_visible: bool,
    /// Whether the settings panel is open (quick-model management).
    settings_visible: bool,
    /// Snapshot of configured quick models, reloaded when the panel opens.
    settings_models: Vec<(String, zerostack_core::config::types::QuickModelConfig)>,
    /// Add-model form fields.
    settings_new_name: SharedString,
    settings_new_provider: SharedString,
    settings_new_model: SharedString,
    /// Last settings action feedback line ("saved"/"removed"/error).
    settings_feedback: SharedString,
    /// Working copy of the config for the settings panel. Loaded on open,
    /// mutated by the toggles/inputs, written back with `save_config` +
    /// `ReloadConfig` on Save.
    settings_cfg: Option<zerostack_core::config::Config>,
    /// Which settings panel section is scrolled into view context (0=general,
    /// 1=limits, 2=permissions) — used to keep the list stable.
    settings_section: usize,
    /// Open rename-session dialog: target session id + its old name. The
    /// input buffer is edited in the dialog and committed with Enter.
    rename_target: Option<(String, String)>,
    rename_buffer: SharedString,
    /// Last reported MCP server statuses, from `CoreEvent::McpStatus`. Empty
    /// until the first `QueryMcp` round-trips.
    mcp_servers: Vec<zerostack_core::events::McpServerStatus>,
    /// True while an MCP status refresh is in flight (spinner on the chip).
    mcp_refreshing: bool,
}

/// One row in the model picker popup. Each entry corresponds to a single
/// `QuickModelConfig` key in the resolved config — we surface the friendly
/// name in the popup and pass the underlying `provider/model` to
/// `UserAction::SetModel` so the engine keeps the canonical id form.
#[derive(Clone, Debug)]
struct QuickModelEntry {
    /// Key from `cfg.quick_models` (e.g. `"deepseek-v4-pro"`). Shown as
    /// the primary label so the user knows which quick model they're
    /// picking.
    name: String,
    /// Provider id (e.g. `"deepseek"`). Used for the secondary label.
    provider: String,
    /// `provider/model`form  that the engine accepts as `SetModel`'s
    /// argument. Built from the config but kept separate so we never have
    /// to split/re-join at click time.
    model_arg: String,
}

/// One permission prompt the engine has handed us but not yet resolved. We
/// keep the id → state mapping so repeated asks for the same id are resolved
/// against the same entry.
#[derive(Clone, Debug)]
struct PendingPermission {}

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
            model_picker_search_focus: cx.focus_handle(),
            model_picker_scroll: ScrollHandle::new(),
            model_picker_scrollbar: crate::scrollbar::ScrollbarState::new(),
            file_picker_scroll: ScrollHandle::new(),
            file_picker_scrollbar: crate::scrollbar::ScrollbarState::new(),
            sidebar_scroll: UniformListScrollHandle::new(),
            slash_popup_scroll: UniformListScrollHandle::new(),
            chat_list: ListState::new(0, ListAlignment::Bottom, px(2048.0)),
            chat_list_rows: 0,
            chat_rows: Vec::new(),
            chat_scrollbar: crate::scrollbar::ScrollbarState::new(),
            chat_follow_tail: true,
            last_scrolled_session_id: SharedString::new(""),
            cursor_visible: true,
            slash_popup_visible: false,
            slash_popup_selected: 0,
            has_focused_input: false,
            ime_composing: false,
            ime_mark_utf16: None,
            reasoning_buffer: SharedString::new(""),
            reasoning_idx: None,
            reasoning_expanded: std::collections::HashMap::new(),
            activity_expanded: std::collections::HashMap::new(),
            turn_fold_after: None,
            turn_fold_expanded: std::collections::HashMap::new(),
            turn_fold_elapsed: 0.0,
            turn_started_at: None,
            turn_had_activity: false,
            md_cache: std::collections::HashMap::new(),
            copied_message: None,
            copied_code: None,
            msg_menu: None,
            escape_stop_armed: false,
            pending_permissions: std::collections::HashMap::new(),
            prompt_history: Vec::new(),
            prompt_history_cursor: None,
            close_confirm_visible: false,
            close_confirm_armed: false,
            close_confirm_focus: cx.focus_handle(),
            quick_models: load_quick_models(),
            cwd_files: load_cwd_files(),
            context_files: Vec::new(),
            model_picker_visible: false,
            model_picker_selected: 0,
            model_picker_query: SharedString::new(""),
            file_picker_visible: false,
            file_picker_selected: 0,
            file_picker_query: SharedString::new(""),
            file_picker_search_focus: cx.focus_handle(),
            mode_picker_visible: false,
            mcp_picker_visible: false,
            mcp_servers: Vec::new(),
            mcp_refreshing: false,
            settings_visible: false,
            settings_models: Vec::new(),
            settings_new_name: SharedString::new(""),
            settings_new_provider: SharedString::new(""),
            settings_new_model: SharedString::new(""),
            settings_feedback: SharedString::new(""),
            settings_cfg: None,
            settings_section: 0,
            rename_target: None,
            rename_buffer: SharedString::new(""),
        }
    }

    /// Build the modal that blocks accidental window-close. Used from
    /// [`Render::render`] when [`ShellState::close_confirm_visible`] is set.
    /// The overlay occludes the rest of the UI (`occlude()`) so clicks
    /// outside its card are swallowed and dispatched as "cancel", and Esc
    /// inside it does the same thing.
    fn render_close_confirm_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.close_confirm_focus.clone();

        // The overlay itself. `.absolute()` + `.inset_0` + `.size_full()`
        // stretches it to fill the root (which is the only positioned
        // ancestor in this tree, so absolute resolves to the window bounds).
        // The translucent background visually blocks the chat while keeping
        // context visible behind it, like the native macOS alert sheet.
        div()
            .absolute()
            .inset_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            .occlude()
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                // Esc on the modal = cancel. We check this BEFORE stop_propagation
                // so the chat column wrapper's key handler doesn't see Esc twice;
                // the listener below also returns early when the modal is gone,
                // so Esc while it's hidden simply falls through to the input box.
                if this.close_confirm_visible && ev.keystroke.key == "escape" {
                    this.cancel_close_confirm(window, cx);
                    cx.stop_propagation();
                }
            }))
            // Translucent backdrop click acts as cancel. Real cancel
            // buttons are inside the card; this matches the macOS sheet
            // idiom (click outside = dismiss) without leaving the user
            // stuck on a modal they can't get out of.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.close_confirm_visible {
                        this.cancel_close_confirm(window, cx);
                    }
                }),
            )
            // The card itself. Centering already happened on the overlay,
            // so this just needs to be a non-stretching child.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(rgb(dark::RAISED))
                    .border_1()
                    .border_color(rgba(dark::BORDER_STRONG))
                    .rounded(px(13.0))
                    .shadow_lg()
                    .p_6()
                    .w(px(440.))
                    .on_mouse_down(
                        // Clicks inside the card must not bubble up to the
                        // backdrop-cancel listener; otherwise clicking on a
                        // button would dismiss the modal *before* the
                        // button's on_click could fire.
                        gpui::MouseButton::Left,
                        |_, _, cx| {
                            cx.stop_propagation();
                        },
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(dark::TEXT))
                            .child("Quit zerostack?"),
                    )
                    .child(div().text_sm().text_color(rgb(dark::TEXT_SECONDARY)).child(
                        "Any streaming reply or pending tool call will be cancelled. \
                                 Type /quit next time if you actually meant it.",
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .mt_2()
                            .child(
                                // Cancel button: dismisses the modal. Using
                                // `on_click` (single press) here matches what a
                                // "Cancel" button does in every native dialog.
                                // The `.id(...)` is required: it promotes the
                                // `Div` into a `Stateful<Div>` so the
                                // `StatefulInteractiveElement::on_click` method
                                // is in scope. Without that trait qualifier the
                                // compiler can't find `.on_click` on plain `Div`.
                                div()
                                    .id("close-confirm-cancel")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(7.))
                                    .border_1()
                                    .border_color(rgba(dark::BORDER))
                                    .bg(rgba(0x00000000))
                                    .text_color(rgb(dark::TEXT))
                                    .text_sm()
                                    .cursor_pointer()
                                    .hover(|element| element.bg(rgba(dark::OVERLAY)))
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.cancel_close_confirm(_w, cx);
                                    }))
                                    .child("Cancel"),
                            )
                            .child(
                                // Quit button: arms the latch and asks the
                                // platform to quit. The next close round
                                // hits on_window_should_close with
                                // `close_confirm_armed = true` => return true,
                                // and the window actually closes. Same
                                // `.id("...")` requirement as the Cancel button
                                // above for the same trait-resolution reason.
                                div()
                                    .id("close-confirm-quit")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(7.))
                                    .border_1()
                                    .border_color(rgba(0x00000000))
                                    .bg(rgb(dark::INVERSE))
                                    .text_color(rgb(dark::ON_INVERSE))
                                    .text_sm()
                                    .cursor_pointer()
                                    .hover(|element| element.opacity(0.9))
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.confirm_close_quit(cx);
                                    }))
                                    .child("Quit"),
                            ),
                    ),
            )
    }

    /// Called by [`Window::on_window_should_close`] when the platform asks
    /// the window to close. Returns `true` to let the close happen, `false`
    /// to keep the window alive and present the modal.
    fn handle_window_should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.close_confirm_armed {
            // User already confirmed in the modal — let the close through.
            // We don't reset `close_confirm_visible` here so the modal stays
            // drawn until the next paint after the window destruct, but it
            // doesn't matter: the app is exiting.
            return true;
        }
        // First close attempt (or repeated accidental taps): show the
        // modal and reject the close so the window stays alive.
        self.open_close_confirm(window, cx);
        false
    }

    /// Open the modal and hand focus to it. Used both from the platform
    /// close callback and from future entry points (e.g. keyboard shortcut).
    fn open_close_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_confirm_visible {
            return;
        }
        self.close_confirm_visible = true;
        window.focus(&self.close_confirm_focus, cx);
        cx.notify();
    }

    /// "Cancel" path: dismiss the modal without quitting. Used by the
    /// Cancel button, Esc, and backdrop-click.
    fn cancel_close_confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.close_confirm_visible {
            return;
        }
        self.close_confirm_visible = false;
        // Hand focus back to the input box so the user can keep typing
        // without an extra click.
        self.input_focus.focus(_window, cx);
        cx.notify();
    }

    /// "Quit" path: latch the armed flag so the next `Window::on_window_should_close`
    /// call returns `true`, then ask gpui to shut down. We also tear down the
    /// bridge here so an in-flight `/quit` invocation matches the slash-command
    /// behaviour used by [`ShellState::apply_event`] when the user types
    /// `/quit` directly.
    fn confirm_close_quit(&mut self, cx: &mut Context<Self>) {
        self.close_confirm_armed = true;
        self.bridge.shutdown();
        cx.quit();
    }

    /// Rename-session dialog: an overlay with a text field prefilled with the
    /// current name. Enter commits `RenameSession`; Esc or outside-click
    /// cancels. Uses plain key_char capture (the dialog is modal, so the
    /// IME-aware composer handler isn't in play here).
    fn render_rename_dialog(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let _view_entity = cx.entity().clone();
        let rename_buffer = self.rename_buffer.clone();
        let old_name = self
            .rename_target
            .as_ref()
            .map(|(_, old)| old.clone())
            .unwrap_or_default();

        let input_field = div()
            .id("rename-input")
            .h(px(32.0))
            .px(px(10.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(rgba(dark::BORDER_STRONG))
            .bg(rgb(dark::COMPOSER))
            .flex()
            .items_center()
            .cursor_text()
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.0))
                    .text_color(if rename_buffer.is_empty() {
                        rgb(dark::TEXT_GHOST)
                    } else {
                        rgb(dark::TEXT)
                    })
                    .child(if rename_buffer.is_empty() {
                        SharedString::from("session name")
                    } else {
                        rename_buffer.clone()
                    }),
            )
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                let key = ev.keystroke.key.as_str();
                let mods = &ev.keystroke.modifiers;
                if key == "enter" {
                    let name = this.rename_buffer.trim().to_string();
                    if !name.is_empty()
                        && let Some((sid, _)) = this.rename_target.clone()
                    {
                        let _ = this.bridge.send(UserAction::RenameSession {
                            session_id: CompactString::new(sid.as_str()),
                            name: CompactString::new(name.as_str()),
                        });
                    }
                    this.rename_target = None;
                    this.rename_buffer = SharedString::new("");
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if key == "escape" {
                    this.rename_target = None;
                    this.rename_buffer = SharedString::new("");
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if key == "backspace" {
                    let mut s = this.rename_buffer.to_string();
                    if mods.platform || mods.control {
                        s.clear();
                    } else {
                        s.pop();
                    }
                    this.rename_buffer = SharedString::new(s);
                } else if let Some(chars) = ev.keystroke.key_char.as_ref() {
                    let mut s = this.rename_buffer.to_string();
                    s.push_str(chars);
                    this.rename_buffer = SharedString::new(s);
                }
                cx.notify();
                cx.stop_propagation();
            }));

        div()
            .absolute()
            .inset_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            .occlude()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _ev, _window, cx| {
                    this.rename_target = None;
                    this.rename_buffer = SharedString::new("");
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(rgb(dark::RAISED))
                    .border_1()
                    .border_color(rgba(dark::BORDER_STRONG))
                    .rounded(px(13.0))
                    .shadow_lg()
                    .p_6()
                    .w(px(360.))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(dark::TEXT))
                            .child("Rename session"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(dark::TEXT_SECONDARY))
                            .child(SharedString::from(format!("Current name: {old_name}"))),
                    )
                    .child(input_field)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(
                                div()
                                    .id("rename-cancel")
                                    .px_3()
                                    .py_1p5()
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(rgba(dark::BORDER))
                                    .text_color(rgb(dark::TEXT))
                                    .text_sm()
                                    .cursor_pointer()
                                    .hover(|element| element.bg(rgba(dark::OVERLAY)))
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.rename_target = None;
                                        this.rename_buffer = SharedString::new("");
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("rename-commit")
                                    .px_3()
                                    .py_1p5()
                                    .rounded(px(6.0))
                                    .bg(rgb(dark::INVERSE))
                                    .text_color(rgb(dark::ON_INVERSE))
                                    .text_sm()
                                    .cursor_pointer()
                                    .hover(|element| element.opacity(0.9))
                                    .child("Save")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        let name = this.rename_buffer.trim().to_string();
                                        if !name.is_empty()
                                            && let Some((sid, _)) = this.rename_target.clone()
                                        {
                                            let _ = this.bridge.send(UserAction::RenameSession {
                                                session_id: CompactString::new(sid.as_str()),
                                                name: CompactString::new(name.as_str()),
                                            });
                                        }
                                        this.rename_target = None;
                                        this.rename_buffer = SharedString::new("");
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
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
        if !s.starts_with('/') {
            self.slash_popup_visible = false;
            self.slash_popup_selected = 0;
            return;
        }
        // Two trigger modes:
        //   1. pure `/` followed by command characters → commands picker;
        //   2. `/provider<space>`, `/mode<space>`, ... → argument chooser.
        // Anything else falls out of the picker entirely.
        if s.contains(char::is_whitespace) && strip_command_arg_prefix(s).is_none() {
            self.slash_popup_visible = false;
            return;
        }
        let matches = slash_matches(s);
        self.slash_popup_visible = !matches.is_empty();
        if self.slash_popup_selected >= matches.len() {
            self.slash_popup_selected = matches.len().saturating_sub(1);
        }
    }

    /// Drain any pending events from the bridge and update our local state. Called
    /// from a recurring `cx.spawn`-based timer in [`ShellState::render`].
    fn poll_bridge(&mut self, cx: &mut Context<Self>) {
        self.sweep_copied_marker();
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
                self.turn_had_activity = true;
                if text.is_empty() {
                    self.is_thinking = true;
                } else {
                    let combined = format!("{}{}", self.reasoning_buffer.as_str(), text.as_str());
                    self.reasoning_buffer = SharedString::new(combined);
                    match self.reasoning_idx {
                        None => {
                            self.chat
                                .push(ChatMessage::reasoning(self.reasoning_buffer.clone()));
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
            CoreEvent::ToolCall { name, args } => {
                // Push a structured tool card so the renderer can show name +
                // args summary + status pill; we update it in place when the
                // matching ToolResult shows up.
                self.turn_had_activity = true;
                let summary = tool_utils::format_tool_call_summary(&name, &args);
                self.chat
                    .push(ChatMessage::tool_card(name.to_string(), summary));
                self.streaming_assistant_idx = None;
            }
            CoreEvent::ToolResult { name, output } => {
                // Find the most recent pending tool card with the same name and
                // flip it to Ok state — keeps the timeline tight instead of
                // producing two adjacent bubbles ("calling foo…" then "foo →").
                let mut updated = false;
                for msg in self.chat.iter_mut().rev() {
                    if msg.role == Role::Tool
                        && let Some(meta) = msg.tool_meta.as_ref()
                        && meta.name.as_ref() == name.as_str()
                        && meta.status == ToolStatus::Pending
                    {
                        msg.complete_tool(output.to_string());
                        updated = true;
                        break;
                    }
                }
                if !updated {
                    // Engine emitted a result without a matching call (rare,
                    // e.g. retry / replay). Drop a plain fall-back so the user
                    // still sees the output.
                    self.chat
                        .push(ChatMessage::tool(format!("{name} → {output}")));
                }
            }
            CoreEvent::SubagentToolCall { name, args } => {
                self.turn_had_activity = true;
                let summary = tool_utils::format_tool_call_summary(&name, &args);
                self.chat
                    .push(ChatMessage::tool_card(name.to_string(), summary));
            }
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
                self.pending_permissions.insert(id, PendingPermission {});
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
                // Turn fold: if this turn did real work (tools / reasoning)
                // and took >= 2s, remember where the work ended so the
                // renderer can fold it behind a "Worked for Ns" divider.
                // The fold index must be the *answer's* index: the placeholder
                // index if we streamed into it, else the freshly-pushed row.
                if self.turn_had_activity
                    && let Some(started) = self.turn_started_at
                    && started.elapsed() >= Duration::from_secs(2)
                {
                    self.turn_fold_after = self
                        .streaming_assistant_idx
                        .or_else(|| self.chat.len().checked_sub(1));
                    self.turn_fold_elapsed = started.elapsed().as_secs_f32();
                }
                self.streaming_assistant_idx = None;
                self.reasoning_idx = None;
                self.reasoning_buffer = SharedString::new("");
                self.is_thinking = false;
                self.escape_stop_armed = false;
                self.turn_started_at = None;
                self.turn_had_activity = false;
                self.invalidate_md_cache();
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
                        tool_meta: None,
                        sent_at: None, // replayed from disk; no hover timestamp
                    })
                    .collect();
                self.streaming_assistant_idx = None;
                self.reasoning_idx = None;
                self.reasoning_buffer = SharedString::new("");
                self.turn_fold_after = None;
                self.turn_had_activity = false;
                self.turn_started_at = None;
                self.md_cache.clear();
                self.activity_expanded.clear();
                self.reasoning_expanded.clear();
                self.turn_fold_expanded.clear();
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
            CoreEvent::ContextFilesUpdated { files } => {
                self.context_files = files.into_iter().map(|path| path.to_string()).collect();
            }
            CoreEvent::McpStatus { servers } => {
                self.mcp_servers = servers;
                self.mcp_refreshing = false;
            }
            CoreEvent::McpLoginStarted { server, auth_url } => {
                // Open the authorization URL in the system browser and keep
                // the panel open so the user sees the in-flight state.
                _cx.open_url(auth_url.as_str());
                self.chat.push(ChatMessage::system(format!(
                    "OAuth login for '{server}' — open the browser to authorize, then return here"
                )));
                self.mcp_refreshing = false;
            }
            CoreEvent::McpLoginDone { server, error } => {
                match error {
                    Some(err) => self.chat.push(ChatMessage::system(format!(
                        "OAuth login for '{server}' failed: {err}"
                    ))),
                    None => self.chat.push(ChatMessage::system(format!(
                        "OAuth login for '{server}' complete — server reconnected"
                    ))),
                }
                // Refresh the panel so the row flips to connected.
                self.refresh_mcp(_cx);
            }
            CoreEvent::AgentStarted => {
                // New user turn; reset the streaming placeholder and the
                // reasoning pipeline so any prior reasoning doesn't leak into
                // the next assistant response.
                self.streaming_assistant_idx = None;
                self.reasoning_buffer = SharedString::new("");
                self.reasoning_idx = None;
                self.is_thinking = true;
                self.turn_started_at = Some(Instant::now());
                self.turn_had_activity = false;
                self.turn_fold_after = None;
                self.turn_fold_elapsed = 0.0;
            }
            CoreEvent::AgentStopped => {
                self.is_thinking = false;
                self.reasoning_idx = None;
                self.reasoning_buffer = SharedString::new("");
                self.turn_started_at = None;
                self.turn_had_activity = false;
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
            CoreEvent::BtwStarted { id } => {
                self.chat
                    .push(ChatMessage::system(format!("[btw #{id}] thinking...")));
            }
            CoreEvent::BtwComplete {
                id,
                response,
                input_tokens,
                output_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
            } => {
                self.chat
                    .push(ChatMessage::system(format!("[btw #{id}] answer:")));
                self.chat.push(ChatMessage::assistant(response.to_string()));
                let total_in = input_tokens + cached_input_tokens + cache_creation_input_tokens;
                if total_in > 0 || output_tokens > 0 {
                    self.status_tokens = self
                        .status_tokens
                        .saturating_add(total_in)
                        .saturating_add(output_tokens);
                }
            }
            CoreEvent::BtwError { id, message } => {
                self.chat
                    .push(ChatMessage::system(format!("[btw #{id}] error: {message}")));
            }
        }
        // Follow-tail: jump to the bottom only if the user has stayed scrolled
        // to the bottom. If they walked up to read history, leave their scroll
        // position alone — a streaming reply should not drag them away.
        if self.chat.len() > prev_len {
            self.maybe_follow_tail();
        }
    }

    /// Snap the chat viewport to the bottom and re-arm follow-tail, but only if
    /// we've been holding the tail the whole time. Called from every code path
    /// that appends new chat content (new message, streaming delta, tool/perm
    /// card) so a reply in progress keeps the new text in view.
    fn maybe_follow_tail(&mut self) {
        if self.chat_follow_tail {
            self.chat_list.scroll_to_end();
            self.chat_follow_tail = true;
        }
    }

    /// True if the chat viewport is currently parked at the bottom. `None`
    /// means the list hasn't laid out yet or isn't scrollable.
    fn is_chat_at_bottom(&self) -> bool {
        self.chat_list.is_scrolled_to_end().unwrap_or(true)
    }

    /// Adjust the chat scroll offset by a pixel delta and re-derive follow-tail
    /// from the new position. Positive `dy` scrolls the viewport down (newer
    /// messages come into view); negative goes back up into history.
    fn scroll_chat_by(&mut self, dy: f32) {
        self.chat_list.scroll_by(px(dy));
        self.chat_follow_tail = self.is_chat_at_bottom();
    }

    /// True while the copy button for message `idx` should show its "copied ✓"
    /// state (within the 2s feedback window).
    fn is_copied(&self, idx: usize) -> bool {
        self.copied_message
            .map(|(i, at)| i == idx && at.elapsed() < Duration::from_secs(2))
            .unwrap_or(false)
    }

    /// Sweep expired "copied ✓" markers during the poll tick.
    fn sweep_copied_marker(&mut self) {
        if let Some((_, at)) = self.copied_message
            && at.elapsed() >= Duration::from_secs(2)
        {
            self.copied_message = None;
        }
        if let Some((_, at)) = self.copied_code
            && at.elapsed() >= Duration::from_secs(2)
        {
            self.copied_code = None;
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
                // Streaming content changed; drop the cached parse for this
                // message so the next render reparses (cheap for one message).
                self.md_cache.remove(&idx);
            }
        }
        // Token-level follow-tail: a reply unfolding in chunks needs to reveal
        // each new token if (and only if) the user was already parked at the
        // bottom. Without this, watch the bottom-edge tick while streaming.
        self.maybe_follow_tail();
    }

    /// Drop every cached markdown parse. Called whenever the chat array is
    /// replaced wholesale (session switch) or a turn finalizes.
    fn invalidate_md_cache(&mut self) {
        self.md_cache.clear();
    }

    /// Parse-or-reuse markdown blocks for the message at `idx`. The cache key
    /// includes the source length so streaming appends (which invalidate the
    /// entry) don't serve stale parses. Kept bounded to avoid unbounded growth
    /// on very long sessions.
    fn blocks_for(&mut self, idx: usize) -> Arc<Vec<MarkdownBlock>> {
        let msg = &self.chat[idx];
        let source = msg.content.as_str();
        let len = source.len();
        if let Some((cached_len, blocks)) = self.md_cache.get(&idx)
            && *cached_len == len
        {
            return blocks.clone();
        }
        let blocks = Arc::new(parse_markdown(source));
        if self.md_cache.len() >= 200 {
            self.md_cache.clear();
        }
        self.md_cache.insert(idx, (len, blocks.clone()));
        blocks
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

        // Echo the user's input into chat *before* dispatching extensions,
        // matching the TUI's behavior (it writes the user's command into the
        // context before dispatch — extension commands don't go through the
        // engine via `bridge.send`, so the user echo has to land in chat
        // here to keep the transcript readable. Engine-handled commands
        // emit their own echo via `SessionListUpdated` / event re-render
        // and skip the inline path.
        //
        // Start the turn timer here (it's also armed on `AgentStarted`) so a
        // "Working for 9s" row appears even before the first delta lands.
        self.turn_started_at.get_or_insert_with(Instant::now);
        self.turn_had_activity = false;

        // Wasm extensions register their own slash commands through
        // `extension::registry`. The engine itself doesn't dispatch those —
        // `handle_slash_command` only knows its hard-coded list and returns
        // "unknown command" otherwise — so we mirror what the TUI does in
        // `src/ui/slash/mod.rs::_ => { extension catch-all }`: when the
        // command is one of our extension names, run it locally, push any
        // output into the chat as a system message, and forward any
        // extension-queued follow-up prompts to the engine as regular
        // `SendMessage`'s. Engine-handled commands still go down the
        // `RunSlashCommand` path below.
        #[cfg(feature = "extensions")]
        if trimmed.starts_with('/') {
            // `parts[0]` is the bare command (with the leading slash); we
            // match against the names the registry exposes, then strip the
            // slash before dispatching because the Wasm bindings expect
            // bare names.
            let head = trimmed.split_whitespace().next().unwrap_or(trimmed);
            let known: Vec<String> = zerostack_core::extension::registry::extension_command_names();
            if known.iter().any(|n| n == head) {
                let cmd_name = head.strip_prefix('/').unwrap_or(head);
                let full_args = if trimmed.len() > head.len() {
                    trimmed[head.len()..].trim().to_string()
                } else {
                    String::new()
                };
                self.push_chat_message_user(trimmed.to_string());
                self.record_prompt_history(trimmed);
                self.reset_input_after_submit();
                // Run the Wasm extension command synchronously. Output is
                // Optional text; queued prompts is what we need to push
                // back into the agent loop. The TUI uses `ctx.pending_inputs`
                // for the same purpose; here we wrap into `SendMessage`.
                let (output, queued) = zerostack_core::extension::registry::dispatch_with_prompts(
                    cmd_name, &full_args,
                );
                if let Some(text) = output {
                    let trimmed_text = text.trim();
                    if !trimmed_text.is_empty() {
                        self.chat
                            .push(ChatMessage::assistant(trimmed_text.to_string()));
                    }
                }
                for prompt in queued {
                    if prompt.trim().is_empty() {
                        continue;
                    }
                    let _ = self.bridge.send(UserAction::SendMessage {
                        text: CompactString::new(prompt),
                    });
                }
                // Keep `last_error` in sync — extensions shouldn't set it,
                // but if the engine is offline and the user hits Enter the
                // dispatcher still needs to reflect that.
                cx.notify();
                return;
            }
        }

        // Slash commands don't go through the agent; they live as their own engine
        // action. Plain text is wrapped in `UserAction::SendMessage`.
        let action = if trimmed.starts_with('/') {
            UserAction::RunSlashCommand {
                command: CompactString::new(trimmed),
            }
        } else {
            UserAction::SendMessage {
                text: CompactString::new(trimmed),
            }
        };

        if !self.bridge.send(action) {
            self.last_error = Some(SharedString::new("engine is offline"));
            return;
        }

        self.record_prompt_history(trimmed);
        self.push_chat_message_user(trimmed.to_string());
        self.reset_input_after_submit();
        cx.notify();
    }

    /// Side-effect helper inside [`ShellState::submit_input`]: stash the
    /// echoed prompt in the up-arrow recall history, capped so the deque
    /// doesn't grow forever. Pulled out as a free helper because both the
    /// extension branch and the engine branch need to do it.
    fn record_prompt_history(&mut self, trimmed: &str) {
        if self.prompt_history.last().map(String::as_str) != Some(trimmed) {
            self.prompt_history.push(trimmed.to_string());
            if self.prompt_history.len() > 64 {
                self.prompt_history.remove(0);
            }
        }
        self.prompt_history_cursor = None;
    }

    /// Side-effect helper in submit_input: append the user's message to
    /// chat history so the transcript keeps a clean echo. The TUI writes
    /// this in `Renderer::write`; we reuse the same plumbing the bridge
    /// emits after a successful round-trip, except extensions don't drive
    /// the bridge, so we draw it ourselves.
    fn push_chat_message_user(&mut self, body: String) {
        self.chat.push(ChatMessage::user(body));
    }

    /// Side-effect helper in submit_input: clear the input buffer, reset
    /// the slash popup, and bring the cursor home. Used by both branches
    /// of submit_input; nothing here is bridge-specific.
    fn reset_input_after_submit(&mut self) {
        self.input_text = SharedString::new("");
        self.input_cursor = 0;
        self.slash_popup_visible = false;
        self.slash_popup_selected = 0;
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
            .next_back();
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
            .rfind(|(_, c)| *c == '\n')
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
            cx.update(|cx| {
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
        let _group_count = groups.len();
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
                    async_cx.update(|cx| {
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
        let view_for_search = view_entity.clone();
        let view_for_refresh = view_entity.clone();
        let view_for_new = view_entity.clone();

        div()
            .flex()
            .flex_col()
            .w(px(280.0))
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
                                .child(div().text_xs().text_color(rgb(dark::TEXT_GHOST)).child(
                                    if filter_active {
                                        "⌕"
                                    } else if total_sessions_on_disk == 0 {
                                        "▧"
                                    } else {
                                        "…"
                                    },
                                ))
                                .child(div().text_xs().text_color(rgb(dark::TEXT_TERTIARY)).child(
                                    if filter_active {
                                        "no sessions match the filter"
                                    } else if total_sessions_on_disk == 0 {
                                        "no sessions yet"
                                    } else {
                                        "no rows to show"
                                    },
                                ))
                                .child(div().text_xs().text_color(rgb(dark::TEXT_GHOST)).child(
                                    if filter_active {
                                        "press Esc in the search box to clear"
                                    } else {
                                        "start a chat below to create one"
                                    },
                                )),
                        ),
                )
            })
    }

    /// Render an empty-state welcome page inside the chat column: brand mark,
    /// tagline, a quick-reference card listing the keyboard shortcuts we wired
    /// up, and a hint about the most recent sessions so a returning user can
    /// jump back in without scrolling through the sidebar tree. Recent rows
    /// are hidden when the session list is empty.
    fn render_welcome(&self, view_entity: gpui::Entity<ShellState>) -> gpui::AnyElement {
        let welcome_header = div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_3()
            .child(
                div()
                    .text_2xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(dark::TEXT))
                    .child("zerostack"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(dark::TEXT_SECONDARY))
                    .whitespace_normal()
                    .child(SharedString::from(
                        "Minimal coding agent. Type below to start, or pick a recent session.",
                    )),
            );

        let shortcuts: &[(&str, &str, &str)] = &[
            (
                "/",
                "slash menu",
                "pick a command without typing the full name",
            ),
            (
                "Ctrl/Cmd+L",
                "new session",
                "wipe the current history and start fresh",
            ),
            ("Ctrl/Cmd+R", "search", "focus the sidebar search box"),
            (
                "Ctrl/Cmd+K",
                "command palette",
                "open the slash picker with / pre-filled",
            ),
            (
                "Ctrl/Cmd+J",
                "toggle focus",
                "jump between input and sidebar search",
            ),
            ("Shift+Enter", "newline", "compose multi-line prompts"),
        ];

        let shortcut_rows: Vec<gpui::AnyElement> = shortcuts
            .iter()
            .map(|(key, label, desc)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .child(
                        // Chip-style key indicator: a small monospace-style
                        // pill that holds the keyboard shortcut. Wide enough
                        // for "Ctrl/Cmd+K" without overflowing; visually
                        // anchors the row so the user can scan the column
                        // for the binding they need.
                        div()
                            .min_w(px(120.0))
                            .text_xs()
                            .px_2p5()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(dark::CHIP_BG))
                            .border_1()
                            .border_color(rgb(dark::CHIP_BORDER))
                            .text_color(rgb(dark::TEXT))
                            .child(key.to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(dark::TEXT))
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(dark::TEXT_TERTIARY))
                            .text_sm()
                            .whitespace_normal()
                            .child(desc.to_string()),
                    )
                    .into_any_element()
            })
            .collect();

        // Keep the divider off the last row so the card has a clean edge.
        let shortcuts_card = {
            let mut rows = shortcut_rows;
            if let Some(last) = rows.last_mut() {
                // `.border_b_1()` overlays on the parent divider; the card's
                // own border still closes the bottom, so stripping the inner
                // border on the last row is cosmetic but cleaner.
                let _ = last; // placeholder so we can mutate later.
            }
            div()
                .flex()
                .flex_col()
                .gap_0()
                .p_3()
                .my_4()
                .rounded(px(13.0))
                .bg(rgb(dark::RAISED))
                .border_1()
                .border_color(rgba(dark::BORDER))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(dark::TEXT_TERTIARY))
                        .pb_2()
                        .child("QUICK KEYBOARD SHORTCUTS"),
                )
                .children(rows)
        };

        // Recent sessions: top 3 by recency. Each row is clickable and routes
        // a UserAction::SwitchSession through the bridge.
        let recent_rows: Vec<gpui::AnyElement> = self
            .sidebar
            .iter()
            .filter(|s| !s.id.is_empty())
            .take(3)
            .map(|s| {
                let view_for_click = view_entity.clone();
                let id = s.id.to_string();
                let name: SharedString = if s.name.is_empty() {
                    SharedString::new(&s.id)
                } else {
                    SharedString::new(&s.name)
                };
                let last: SharedString = if s.last_message.is_empty() {
                    SharedString::new("no messages yet")
                } else {
                    SharedString::new(&s.last_message)
                };
                let model_line = SharedString::new(format!(
                    "{} · {}",
                    if s.provider.is_empty() {
                        "—"
                    } else {
                        &s.provider
                    },
                    if s.model.is_empty() { "—" } else { &s.model },
                ));
                let id_for_click = id.clone();
                div()
                    .id(ElementId::Name(
                        format!("welcome-recent-{id_for_click}").into(),
                    ))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded(px(11.0))
                    .bg(rgb(dark::RAISED))
                    .border_1()
                    .border_color(rgba(dark::BORDER))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgba(dark::OVERLAY)))
                    .on_click(move |_ev, _window, cx| {
                        view_for_click.update(cx, |state, cx| {
                            let _ = state.bridge.send(UserAction::SwitchSession {
                                session_id: CompactString::from(id_for_click.as_str()),
                            });
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(dark::TEXT))
                            .child(name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(dark::TEXT_TERTIARY))
                            .child(model_line),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(dark::TEXT_SECONDARY))
                            .whitespace_normal()
                            .child(last),
                    )
                    .into_any_element()
            })
            .collect();

        let recent_card = if recent_rows.is_empty() {
            div().into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .my_4()
                .rounded(px(13.0))
                .border_1()
                .border_color(rgba(dark::BORDER))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(dark::TEXT_TERTIARY))
                        .child("RECENT SESSIONS"),
                )
                .children(recent_rows)
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .px_8()
            .py_10()
            .w_full()
            .max_w(px(640.0))
            .mx_auto()
            .child(welcome_header)
            .child(shortcuts_card)
            .child(recent_card)
            .into_any_element()
    }

    fn render_chat(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let view_entity = cx.entity().clone();
        let chat_list = self.chat_list.clone();

        // Re-derive follow-tail from the current scroll position before painting.
        // This catches both cases without a gesture (e.g. a session switch that
        // resized the chat to be shorter than the viewport — `is_chat_at_bottom`
        // would always read true there, so re-engaging is harmless) and the
        // case where the user wheels back down to the bottom of the conversation
        // without committing a follow-tail-affecting event. Polling here makes
        // the state a function of where the viewport currently sits, which is
        // the property `maybe_follow_tail` keys off when new content arrives.
        self.chat_follow_tail = self.is_chat_at_bottom();

        let streaming_idx = self.streaming_assistant_idx;
        let is_active_stream = self.is_thinking;
        let fold_after = self.turn_fold_after;
        let fold_expanded = fold_after
            .and_then(|idx| self.turn_fold_expanded.get(&idx).copied())
            .unwrap_or(false);
        let fold_elapsed = self.turn_fold_elapsed;

        // Flatten the message list into render rows. Activity (tool runs +
        // reasoning) between a user message and the final answer is buffered
        // and either emitted as rows or swallowed behind a "Worked for Ns"
        // fold divider, mirroring Waku's turn-fold.
        let mut rows: Vec<ChatRow> = Vec::new();
        let mut run_start: Option<usize> = None;
        let flush_run = |rows: &mut Vec<ChatRow>, start: Option<usize>, end: usize| {
            if let Some(start) = start {
                rows.push(ChatRow::ToolRun { start, end });
            }
        };
        let mut pending_activity: Vec<ChatRow> = Vec::new();
        let emit_pending = |rows: &mut Vec<ChatRow>, pending: &mut Vec<ChatRow>| {
            rows.append(pending);
        };

        let n = self.chat.len();
        let mut i = 0usize;
        while i < n {
            let msg = &self.chat[i];
            match msg.role {
                Role::User => {
                    flush_run(&mut pending_activity, run_start.take(), i);
                    emit_pending(&mut rows, &mut pending_activity);
                    rows.push(ChatRow::User(i));
                }
                Role::Assistant => {
                    flush_run(&mut pending_activity, run_start.take(), i);
                    let is_streaming = is_active_stream && Some(i) == streaming_idx && i == n - 1;
                    if Some(i) == fold_after {
                        if fold_expanded {
                            // Expanded fold: show the divider, then the work.
                            rows.push(ChatRow::TurnFold {
                                answer_idx: i,
                                elapsed: fold_elapsed,
                            });
                            emit_pending(&mut rows, &mut pending_activity);
                        } else {
                            // Collapsed fold: drop the buffered activity, show
                            // only the divider (the toggle affordance).
                            pending_activity.clear();
                            rows.push(ChatRow::TurnFold {
                                answer_idx: i,
                                elapsed: fold_elapsed,
                            });
                        }
                    } else {
                        emit_pending(&mut rows, &mut pending_activity);
                    }
                    rows.push(ChatRow::Assistant(i, is_streaming));
                }
                Role::Tool => {
                    if msg.tool_meta.is_some() {
                        if run_start.is_none() {
                            run_start = Some(i);
                        }
                    } else {
                        flush_run(&mut pending_activity, run_start.take(), i);
                        pending_activity.push(ChatRow::ToolText(i));
                    }
                }
                Role::Reasoning => {
                    flush_run(&mut pending_activity, run_start.take(), i);
                    pending_activity.push(ChatRow::Reasoning(i));
                }
                Role::System | Role::Permission => {
                    flush_run(&mut pending_activity, run_start.take(), i);
                    emit_pending(&mut rows, &mut pending_activity);
                    rows.push(if msg.role == Role::System {
                        ChatRow::System(i)
                    } else {
                        ChatRow::Permission(i)
                    });
                }
            }
            i += 1;
        }
        flush_run(&mut pending_activity, run_start.take(), n);
        emit_pending(&mut rows, &mut pending_activity);

        // Working indicator: while the agent is busy but nothing has been
        // emitted yet (no streaming text, no activity rows), show a pinned
        // "Working for Ns" row after the user's message.
        let has_live_activity = rows.iter().any(|r| {
            matches!(
                r,
                ChatRow::ToolRun { .. } | ChatRow::Reasoning(_) | ChatRow::Assistant(_, true)
            )
        });
        let last_is_user = matches!(rows.last(), Some(ChatRow::User(_)));
        if is_active_stream && !has_live_activity && last_is_user {
            let elapsed = self
                .turn_started_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            rows.push(ChatRow::Working(elapsed));
        }

        // Sync the virtualized list with the row set. Row order is append-only
        // while streaming, so splice new rows onto the end; on a wholesale
        // change (session switch, fold toggle) reset everything.
        let row_count = rows.len();
        if row_count != self.chat_list_rows {
            if row_count > self.chat_list_rows {
                chat_list.splice(
                    self.chat_list_rows..self.chat_list_rows,
                    row_count - self.chat_list_rows,
                );
            } else {
                chat_list.reset(row_count);
            }
            self.chat_list_rows = row_count;
        }
        // Streaming rows change height as text lands; re-measure the last row
        // so the tail grows instead of clipping.
        if self.streaming_assistant_idx.is_some() && row_count > 0 {
            chat_list.remeasure_items(row_count - 1..row_count);
        }
        self.chat_rows = rows;
        chat_list.set_follow_mode(FollowMode::Tail);
        if self.chat_follow_tail {
            chat_list.scroll_to_end();
        }

        let rows_for_entity = view_entity.downgrade();
        let empty = self.chat.is_empty();
        let welcome = self.render_welcome(view_entity.clone());
        let chat_scrollbar = self.chat_scrollbar.clone();

        // The outer wrapper here is a flex *column* so the inner
        // `chat-scroll-area` div can use `flex_1()` to claim the wrapper's
        // remaining height. Without `flex` on this wrapper, the inner div falls
        // back to content-fit height and `overflow_y_scroll` has nothing to
        // clip — so wheel events and PageUp/Down would hit a zero-budget
        // scroll region. This was the silent regression that broke scrolling.
        div()
            .flex_1()
            .min_h_0()
            .bg(rgb(dark::CHAT_BG))
            .flex()
            .flex_col()
            .child(
                div()
                    .id("chat-scroll-area")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_x_hidden()
                    .relative()
                    .when(empty, |d| {
                        d.child(
                            div().flex_1().child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .w_full()
                                    .min_w_0()
                                    .px_6()
                                    .py_5()
                                    .child(welcome),
                            ),
                        )
                    })
                    .when(!empty, |d| {
                        d.child(
                            list(chat_list, move |index, _window, cx| {
                                rows_for_entity
                                    .upgrade()
                                    .map(|entity| {
                                        entity.update(cx, |this, cx| {
                                            this.render_chat_row_index(index, cx)
                                        })
                                    })
                                    .unwrap_or_else(|| div().into_any_element())
                            })
                            .size_full(),
                        )
                    })
                    .child(crate::scrollbar::vertical(&self.chat_list, &chat_scrollbar))
                    .when(!self.chat_follow_tail, |d| {
                        // Waku-style scroll-to-bottom FAB: shows whenever the
                        // reader has scrolled away from the tail, re-pins on
                        // click.
                        let view_entity_for_fab = view_entity.clone();
                        d.child(
                            div()
                                .id("chat-scroll-to-bottom")
                                .absolute()
                                .bottom(px(8.0))
                                .left(px(0.0))
                                .right(px(0.0))
                                .flex()
                                .justify_center()
                                .child(
                                    div()
                                        .id("chat-scroll-to-bottom-btn")
                                        .size(px(32.0))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(rgba(dark::BORDER_STRONG))
                                        .bg(rgb(dark::COMPOSER))
                                        .shadow_md()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|element| element.bg(rgb(dark::RAISED)))
                                        .child(div().text_color(rgb(dark::TEXT)).child("↓"))
                                        .on_click(move |_ev, _window, cx| {
                                            view_entity_for_fab.update(cx, |state, cx| {
                                                state.chat_list.scroll_to_end();
                                                state.chat_follow_tail = true;
                                                state.maybe_follow_tail();
                                                cx.notify();
                                            });
                                        }),
                                ),
                        )
                    })
                    .when(self.msg_menu.is_some(), |d| {
                        d.child(self.render_msg_menu(cx))
                    }),
            )
    }

    /// Render the row at virtual index `index` (used by the `list` closure,
    /// which is `'static` and cannot borrow the view).
    fn render_chat_row_index(&mut self, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        // Clone the row out of the cache so we can take `&mut self` for
        // markdown-block caching below without fighting the borrow.
        let Some(row) = self.chat_rows.get(index).cloned() else {
            return div().into_any_element();
        };
        let view_entity = cx.entity().clone();
        self.render_chat_row(&row, view_entity)
    }

    /// Render the open message context menu (right-click), positioned
    /// absolutely at the recorded click point. Closes on outside click or Esc.
    fn render_msg_menu(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(menu) = &self.msg_menu else {
            return div().into_any_element();
        };
        let msg = &self.chat[menu.msg_idx];
        let view_entity = cx.entity().clone();
        let msg_idx = menu.msg_idx;
        let content = msg.content.to_string();

        let mut items: Vec<gpui::AnyElement> = Vec::new();
        // Copy message.
        items.push(menu_item(
            "⧉",
            "Copy message",
            view_entity.clone(),
            move |state, cx| {
                state.copied_message = Some((msg_idx, Instant::now()));
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(content.clone()));
                state.msg_menu = None;
                cx.notify();
            },
        ));
        // Copy to composer: pull the message text back into the input box.
        // Most useful for user messages (reuse a past prompt), so we offer it
        // for user + system rows.
        let msg_role = msg.role.clone();
        if matches!(msg_role, Role::User | Role::System) {
            let content_for_composer = msg.content.to_string();
            items.push(menu_item(
                "✎",
                "Copy to composer",
                view_entity.clone(),
                move |state, cx| {
                    state.input_text = SharedString::new(content_for_composer.clone());
                    state.input_cursor = state.input_text.chars().count();
                    state.msg_menu = None;
                    cx.notify();
                },
            ));
        }
        // Revert to here: undo every exchange after this user message (the
        // engine's `/undo` walks back one user+assistant pair; we replay it
        // until the transcript ends at this message).
        if msg.role == Role::User {
            items.push(menu_item(
                "↩",
                "Revert to here",
                view_entity.clone(),
                move |state, cx| {
                    // Count exchanges strictly after this message, then undo
                    // that many times via the engine.
                    let target = msg_idx;
                    let mut count = 0usize;
                    for (i, m) in state.chat.iter().enumerate() {
                        if i > target && m.role == Role::User {
                            count += 1;
                        }
                    }
                    for _ in 0..count {
                        let _ = state.bridge.send(UserAction::UndoLastExchange);
                    }
                    if count == 0 {
                        let _ = state.bridge.send(UserAction::UndoLastExchange);
                    }
                    state.msg_menu = None;
                    cx.notify();
                },
            ));
        }
        // Copy code (assistant messages that contain fenced code).
        let code: String = extract_fenced_code(msg.content.as_str());
        if !code.is_empty() {
            items.push(menu_item(
                "⎙",
                "Copy code",
                view_entity.clone(),
                move |state, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(code.clone()));
                    state.msg_menu = None;
                    cx.notify();
                },
            ));
        }

        div()
            .absolute()
            .left(px(menu.x))
            .top(px(menu.y))
            .flex()
            .flex_col()
            .min_w(px(180.0))
            .py(px(4.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(rgba(dark::BORDER_STRONG))
            .bg(rgb(dark::RAISED))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out({
                let view_entity = view_entity.clone();
                move |_ev, _window, cx| {
                    view_entity.update(cx, |state, cx| {
                        state.msg_menu = None;
                        cx.notify();
                    });
                }
            })
            .children(items)
            .into_any_element()
    }

    /// Render one flattened chat row. Activity runs / reasoning / turn folds
    /// carry their expansion state from the corresponding maps; user and
    /// assistant rows get hover footers with timestamp + copy.
    fn render_chat_row(
        &mut self,
        row: &ChatRow,
        view_entity: gpui::Entity<ShellState>,
    ) -> gpui::AnyElement {
        match row {
            ChatRow::User(idx) => {
                let copied = self.is_copied(*idx);
                wrap_msg_menu(
                    render_user_msg(&self.chat[*idx], *idx, view_entity.clone(), copied),
                    *idx,
                    view_entity,
                )
            }
            ChatRow::Assistant(idx, is_streaming) => {
                let copied = self.is_copied(*idx);
                // Parse via the cache (blocks_for) so long sessions don't
                // reparse every message on every frame; only the streaming
                // message is invalidated as it grows.
                let blocks = self.blocks_for(*idx);
                // Which code block in this message is showing "copied ✓"
                // right now (if any).
                let copied_code = self
                    .copied_code
                    .and_then(|((msg_idx, block_idx), _)| (msg_idx == *idx).then_some(block_idx))
                    .map(|block_idx| (*idx, block_idx));
                wrap_msg_menu(
                    render_assistant_msg(
                        &self.chat[*idx],
                        *idx,
                        *is_streaming,
                        view_entity.clone(),
                        copied,
                        blocks,
                        copied_code,
                    ),
                    *idx,
                    view_entity,
                )
            }
            ChatRow::System(idx) => render_system_msg(&self.chat[*idx]),
            ChatRow::Permission(idx) => render_permission_card(&self.chat[*idx], view_entity),
            ChatRow::ToolText(idx) => render_tool_text(&self.chat[*idx]),
            ChatRow::Reasoning(idx) => {
                let is_live = self.is_thinking && Some(*idx) == self.reasoning_idx;
                let expanded = self.reasoning_expanded.get(idx).copied().unwrap_or(is_live);
                render_reasoning_row(&self.chat[*idx], *idx, expanded, is_live, view_entity)
            }
            ChatRow::ToolRun { start, end } => {
                let any_pending = (*start..*end).any(|i| {
                    matches!(
                        self.chat[i].tool_meta().map(|m| &m.status),
                        Some(ToolStatus::Pending)
                    )
                });
                let is_live = self.is_thinking && any_pending;
                let expanded = self
                    .activity_expanded
                    .get(start)
                    .copied()
                    .unwrap_or(is_live);
                render_tool_run(
                    &self.chat[*start..*end],
                    *start,
                    *end,
                    expanded,
                    is_live,
                    view_entity,
                )
            }
            ChatRow::TurnFold {
                answer_idx,
                elapsed,
            } => {
                let expanded = self
                    .turn_fold_expanded
                    .get(answer_idx)
                    .copied()
                    .unwrap_or(false);
                render_turn_fold(*answer_idx, *elapsed, expanded, view_entity)
            }
            ChatRow::Working(elapsed) => render_working_row(*elapsed),
        }
    }

    fn render_input(&mut self, cx: &mut Context<Self>, window: &mut Window) -> impl IntoElement {
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
            .border_color(rgba(dark::BORDER))
            .bg(rgb(dark::APP_BG))
            .when(self.slash_popup_visible, |d| {
                d.child(self.render_slash_popup(cx))
            })
            .when(self.model_picker_visible, |d| {
                d.child(self.render_model_picker(cx))
            })
            .when(self.mode_picker_visible, |d| {
                d.child(self.render_mode_picker(cx))
            })
            .when(self.mcp_picker_visible, |d| {
                d.child(self.render_mcp_picker(cx))
            })
            .when(self.settings_visible, |d| d.child(self.render_settings(cx)))
            .when(self.file_picker_visible, |d| {
                d.child(self.render_file_picker(cx))
            })
            .when(!self.context_files.is_empty(), |d| {
                d.child(self.render_context_files(cx))
            })
            // Composer card: rounded surface holding the text input and the
            // controls row (model chip, status, send). Mirrors Waku's
            // composer — the card owns the rounded/raised look, the inner
            // input is chrome-less so focus shows as the caret only.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .mx_5()
                    .mb_4()
                    .mt_1()
                    .px_2()
                    .py_2()
                    .rounded(px(13.0))
                    .border_1()
                    .border_color(rgba(dark::BORDER))
                    .bg(rgb(dark::COMPOSER))
                    .child(
                        div()
                            .id("input-box")
                            .track_focus(&self.input_focus)
                            .focus_visible(|d| d.border_color(rgb(dark::ACCENT)))
                            .px_4()
                            .pt_2()
                            .pb_1()
                            .text_color(rgb(dark::TEXT))
                            .text_size(px(13.5))
                            .line_height(px(22.0))
                            .min_h(px(24.0))
                            .child({
                                let (before_cursor, after_cursor) =
                                    split_at_char(&self.input_text, self.input_cursor);
                                render_input_text(
                                    before_cursor,
                                    after_cursor,
                                    self.input_text.is_empty(),
                                    // Caret only while the composer is focused;
                                    // a blinking block on a defocused window is
                                    // noise (and users report it as a bug).
                                    self.cursor_visible && self.input_focus.is_focused(window),
                                )
                            })
                            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                                let key = ev.keystroke.key.as_str();
                                let mods = &ev.keystroke.modifiers;

                                // Global modifier-combo shortcuts fire regardless of
                                // what's in the buffer. We only intercept when the
                                // platform modifier (Cmd on macOS, Ctrl elsewhere) is
                                // held, so plain text typing still works as before.
                                if (mods.platform || mods.control) && !mods.alt {
                                    match key.to_ascii_lowercase().as_str() {
                                        "," => {
                                            // Cmd/Ctrl+, — open the settings panel
                                            // (macOS convention).
                                            this.open_settings(cx);
                                            cx.stop_propagation();
                                            return;
                                        }
                                        "l" => {
                                            // Ctrl/Cmd+L — start a fresh session
                                            // (mirrors the TUI's `/clear`). Send
                                            // through the bridge so the engine owns
                                            // the lifecycle.
                                            let _ = this.bridge.send(UserAction::ClearSession);
                                            this.input_text = SharedString::new("");
                                            this.input_cursor = 0;
                                            this.refresh_slash_popup();
                                            cx.notify();
                                            cx.stop_propagation();
                                            return;
                                        }
                                        "r" => {
                                            // Ctrl/Cmd+R — focus the sidebar search
                                            // box so the user can immediately start
                                            // filtering by name or path.
                                            this.sidebar_search_focus.focus(window, cx);
                                            cx.notify();
                                            cx.stop_propagation();
                                            return;
                                        }
                                        "j" => {
                                            // Ctrl/Cmd+J — toggle focus between the
                                            // chat input and the sidebar search so
                                            // the user can jump between the two
                                            // without taking their hand off the
                                            // keyboard.
                                            if this.sidebar_search_focus.is_focused(window) {
                                                this.input_focus.focus(window, cx);
                                            } else {
                                                this.sidebar_search_focus.focus(window, cx);
                                            }
                                            cx.notify();
                                            cx.stop_propagation();
                                            return;
                                        }
                                        "k" => {
                                            // Ctrl/Cmd+K — focus the input and
                                            // prefill "/" so the slash picker pops
                                            // up without an extra keystroke. Same
                                            // idea as the TUI's command palette.
                                            this.input_text = SharedString::new("/");
                                            this.input_cursor = 1;
                                            this.refresh_slash_popup();
                                            this.input_focus.focus(window, cx);
                                            cx.notify();
                                            cx.stop_propagation();
                                            return;
                                        }
                                        _ => {}
                                    }
                                }

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

                                // Model picker has higher priority than the input box and the slash
                                // popup: when it's open we want Up/Down/Enter to navigate
                                // the model list, and Esc to dismiss. We always
                                // `stop_propagation` so the underlying input text
                                // doesn't see the keystroke once a selection is made.
                                if this.model_picker_visible {
                                    if this.handle_model_picker_key(key, mods) {
                                        cx.notify();
                                        cx.stop_propagation();
                                        return;
                                    }
                                    // Backspace with an empty filter closes nothing; it
                                    // just stays (the search field edits the query).
                                    if key == "backspace" {
                                        cx.stop_propagation();
                                        return;
                                    }
                                }

                                // File picker mirrors the model picker: same keymap.
                                if this.file_picker_visible {
                                    if this.handle_file_picker_key(key, mods) {
                                        cx.notify();
                                        cx.stop_propagation();
                                        return;
                                    }
                                    if key == "backspace" {
                                        cx.stop_propagation();
                                        return;
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
                                                let name_str = name.clone();
                                                if let Some(arg_prefix) = strip_command_arg_prefix(
                                                    this.input_text.as_str(),
                                                ) {
                                                    // We're already inside an arg
                                                    // chooser (e.g. `/provider<space>`).
                                                    // Build the full slash command from
                                                    // the picked value and submit it.
                                                    let full = format!("{arg_prefix} {name_str}");
                                                    this.input_text = SharedString::new(full);
                                                    this.input_cursor =
                                                        this.input_text.chars().count();
                                                    this.slash_popup_visible = false;
                                                    this.submit_input(cx);
                                                } else {
                                                    // Step into the chooser for the
                                                    // arg-needing command. The popup
                                                    // will refresh into its argument
                                                    // list on the next call.
                                                    let mut buf = name_str;
                                                    buf.push(' ');
                                                    this.input_text = SharedString::new(buf);
                                                    this.input_cursor =
                                                        this.input_text.chars().count();
                                                    this.refresh_slash_popup();
                                                }
                                            }
                                            cx.stop_propagation();
                                            return;
                                        }
                                        "enter" => {
                                            if let Some((name, _, _)) =
                                                matches.get(this.slash_popup_selected)
                                            {
                                                let name_str = name.clone();
                                                if let Some(arg_prefix) = strip_command_arg_prefix(
                                                    this.input_text.as_str(),
                                                ) {
                                                    let full = format!("{arg_prefix} {name_str}");
                                                    this.input_text = SharedString::new(full);
                                                    this.input_cursor =
                                                        this.input_text.chars().count();
                                                    this.slash_popup_visible = false;
                                                    this.submit_input(cx);
                                                } else {
                                                    this.input_text = SharedString::new(name_str);
                                                    this.input_cursor =
                                                        this.input_text.chars().count();
                                                    this.slash_popup_visible = false;
                                                    this.submit_input(cx);
                                                }
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
                                        if mods.platform
                                            || (mods.control && !this.input_text.is_empty())
                                        {
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
                                            this.input_cursor =
                                                this_char_count(this.input_text.as_str());
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
                                        this.input_cursor =
                                            this_char_count(this.input_text.as_str());
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
                                        // Esc while streaming: first press
                                        // arms a 3s confirmation (send button
                                        // shows "Esc"), second press cancels.
                                        // When idle, Esc just clears the draft
                                        // like the TUI.
                                        if this.is_thinking {
                                            if this.escape_stop_armed {
                                                let _ = this.bridge.send(UserAction::CancelStream);
                                                this.escape_stop_armed = false;
                                            } else {
                                                this.escape_stop_armed = true;
                                                // Resolve the entity from the
                                                // listener's own `cx` (the closure
                                                // must not capture outer locals).
                                                let view = cx.entity().clone();
                                                cx.spawn(async move |_weak, async_cx| {
                                                    async_cx
                                                        .background_executor()
                                                        .timer(Duration::from_secs(3))
                                                        .await;
                                                    async_cx.update(|cx| {
                                                        view.update(cx, |state, cx| {
                                                            state.escape_stop_armed = false;
                                                            cx.notify();
                                                        });
                                                    });
                                                })
                                                .detach();
                                            }
                                            cx.stop_propagation();
                                            return;
                                        }
                                        // Esc cancels the current draft (and any open
                                        // popup). Match the TUI's behavior of treating
                                        // Esc as a no-op when the buffer is empty so we
                                        // don't swallow focus moves system-wide.
                                        this.input_text = SharedString::new("");
                                        this.input_cursor = 0;
                                        this.slash_popup_visible = false;
                                        this.msg_menu = None;
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
                    // Controls row (model chip / mode / status / send) inside the
                    // composer card.
                    .child(self.render_input_chip_row(cx)),
            )
            .child(ImeInputElement {
                view: view_entity,
                focus: input_focus_clone,
            })
    }

    /// Persistent, removable context chips above the composer. The engine
    /// remains the source of truth: a click requests removal and the row only
    /// changes after `ContextFilesUpdated` comes back.
    fn render_context_files(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let chips = self.context_files.iter().map(|path| {
            let full_path = path.clone();
            let label = std::path::Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path)
                .to_string();
            let view_for_remove = view_entity.clone();
            div()
                .id(ElementId::Name(format!("context-file-{path}").into()))
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded(px(6.0))
                .border_1()
                .border_color(rgba(dark::BORDER))
                .bg(rgb(dark::CHIP_BG))
                .text_xs()
                .text_color(rgb(dark::TEXT_SECONDARY))
                .tooltip(crate::tooltip::Tooltip::text(full_path.clone()))
                .child("▧")
                .child(label)
                .child(div().ml_1().text_color(rgb(dark::TEXT_GHOST)).child("×"))
                .hover(|d| {
                    d.bg(rgba(dark::OVERLAY))
                        .border_color(rgba(dark::BORDER_STRONG))
                })
                .on_click(move |_ev, _window, cx| {
                    view_for_remove.update(cx, |state, _cx| {
                        let _ = state.bridge.send(UserAction::DropFile {
                            path: CompactString::new(full_path.as_str()),
                        });
                    });
                })
        });

        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .mx_5()
            .mt_2()
            .children(chips)
            .child(
                div()
                    .id("context-clear-all")
                    .px_2()
                    .py_1()
                    .rounded(px(6.0))
                    .text_xs()
                    .text_color(rgb(dark::TEXT_GHOST))
                    .cursor_pointer()
                    .hover(|element| element.bg(rgba(dark::OVERLAY)).text_color(rgb(dark::ERROR)))
                    .tooltip(crate::tooltip::Tooltip::text("Drop all context files"))
                    .child("clear all")
                    .on_click({
                        let view_entity = view_entity.clone();
                        move |_ev, _window, cx| {
                            view_entity.update(cx, |state, _cx| {
                                let _ = state.bridge.send(UserAction::DropAllFiles);
                            });
                        }
                    }),
            )
            .into_any_element()
    }

    /// Render the slim chip row that lives below the input box. The chips
    /// mirror the reference design: a leading `+` action, the current
    /// provider/model based on resolved config, autonomy/mode, MCP status,
    /// idle counter, help, and a trailing submit arrow. Each chip is a
    /// pseudo-button — the `+`, `+ MCP`, `?`, and `↗` chips accept clicks
    /// for future expansion, but the metadata chips carry status only.
    ///
    /// When the agent is actively streaming, the status chip swaps to a
    /// red "Cancel" pill that fires `UserAction::CancelStream` — same
    /// code path the old `cancel-btn` toggle used.
    fn render_input_chip_row(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let model_label = if self.status_model.is_empty() {
            SharedString::new("model")
        } else {
            // provider/model. The provider tag is upcased to match the chip
            // style in the reference design.
            let provider = self.status_provider.to_uppercase();
            SharedString::new(format!("{provider} · {}", self.status_model.as_str()))
        };
        let mode_label = if self.status_mode.is_empty() {
            SharedString::new("yolo")
        } else {
            self.status_mode.clone()
        };
        let idle_label: SharedString = if self.is_thinking {
            SharedString::new("thinking…")
        } else {
            SharedString::new("idle")
        };

        // Capture picker-open state for the trigger chips so they get the
        // accent fill when their popup is showing — the user gets a tight
        // visual link between "I see this chip depressed" and "the dropdown
        // is open above me".
        let file_chip_active = self.file_picker_visible;
        let model_chip_active = self.model_picker_visible;

        div()
            .flex()
            .items_center()
            .gap_1p5()
            .mt_1()
            .px_2()
            .pb_1()
            .text_size(px(11.5))
            .line_height(px(14.0))
            .child(input_chip_trigger(
                "input-chip-add",
                "+",
                file_chip_active,
                view_entity.clone(),
                |state, window, cx| {
                    // Toggle the file picker; close the model picker if it
                    // was open so only one dropdown shows at a time.
                    let was_open = state.file_picker_visible;
                    state.file_picker_visible = !was_open;
                    if state.file_picker_visible {
                        state.model_picker_visible = false;
                        state.cwd_files = load_cwd_files();
                        state.file_picker_selected = 0;
                        state.file_picker_query = SharedString::new("");
                        // Hand the search field focus so typing filters
                        // immediately (same as the model picker).
                        state.file_picker_search_focus.focus(window, cx);
                    }
                    cx.notify();
                },
            ))
            .child(input_chip_trigger(
                "input-chip-model",
                model_label.as_str(),
                model_chip_active,
                view_entity.clone(),
                |state, window, cx| {
                    let was_open = state.model_picker_visible;
                    state.model_picker_visible = !was_open;
                    if state.model_picker_visible {
                        state.file_picker_visible = false;
                        state.model_picker_selected = 0;
                        state.model_picker_query = SharedString::new("");
                        // Hand the search field focus so typing filters
                        // immediately (Waku's picker does the same).
                        state.model_picker_search_focus.focus(window, cx);
                    }
                    cx.notify();
                },
            ))
            .child(input_chip_trigger(
                "input-chip-mode",
                mode_label.as_str(),
                self.mode_picker_visible,
                view_entity.clone(),
                |state, _, cx| {
                    // Toggle the mode menu; close the other pickers so only
                    // one dropdown shows at a time.
                    let was_open = state.mode_picker_visible;
                    state.mode_picker_visible = !was_open;
                    if state.mode_picker_visible {
                        state.file_picker_visible = false;
                        state.model_picker_visible = false;
                    }
                    cx.notify();
                },
            ))
            .child(input_chip_trigger(
                "input-chip-mcp",
                "+ MCP",
                self.mcp_picker_visible,
                view_entity.clone(),
                |state, _, cx| {
                    // Toggle the MCP panel; close the other pickers so only
                    // one dropdown shows at a time. Refresh status on open.
                    let was_open = state.mcp_picker_visible;
                    state.mcp_picker_visible = !was_open;
                    if state.mcp_picker_visible {
                        state.model_picker_visible = false;
                        state.file_picker_visible = false;
                        state.mode_picker_visible = false;
                        state.refresh_mcp(cx);
                    }
                    cx.notify();
                },
            ))
            .child(input_chip_status(idle_label.as_str(), self.is_thinking))
            .child(div().flex_1())
            .child(input_chip_send(
                self.input_text.is_empty(),
                self.is_thinking,
                self.escape_stop_armed,
                view_entity,
            ))
            .into_any_element()
    }

    /// Render the dropdown above the input box listing the cached
    /// `QuickModelEntry` rows. Clicking a row fires
    /// `UserAction::SetModel { model }` with the canonical `provider/model`
    /// string the engine expects, and the picker auto-closes. Lines up
    /// vertically with the input column (same `mx_5()` inset) so the user
    /// sees the trigger chip and its options in the same column.
    fn render_model_picker(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entries: Vec<QuickModelEntry> = self.filtered_quick_models();
        let view_entity = cx.entity().clone();
        let query = self.model_picker_query.clone();
        let highlight = self
            .model_picker_selected
            .min(entries.len().saturating_sub(1));
        let current_model = self.status_model.clone();

        // Search field: a chrome-less box that owns text input while the
        // panel is open. Typing filters; Up/Down/Enter/Esc are claimed by the
        // input listener above (they stop_propagation before reaching us).
        let search_box = div()
            .id("model-picker-search")
            .track_focus(&self.model_picker_search_focus)
            .focus_visible(|d| d.border_color(rgb(dark::ACCENT)))
            .h(px(34.0))
            .px(px(10.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(rgba(dark::BORDER))
            .bg(rgb(dark::RAISED))
            .flex()
            .items_center()
            .gap_2()
            .cursor_text()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _ev, window, cx| {
                    this.model_picker_search_focus.focus(window, cx);
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                let key = ev.keystroke.key.as_str();
                let mods = &ev.keystroke.modifiers;
                // Arrow/enter/esc navigation is shared with the input-box
                // listener so the picker behaves the same from either focus.
                if this.handle_model_picker_key(key, mods) {
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if key == "backspace" {
                    // Remove one char from the filter (or clear it all with
                    // Cmd/Ctrl+Backspace).
                    if mods.platform || mods.control {
                        this.model_picker_query = SharedString::new("");
                    } else {
                        let mut updated = this.model_picker_query.to_string();
                        updated.pop();
                        this.model_picker_query = SharedString::new(updated);
                    }
                    this.model_picker_selected = 0;
                } else if let Some(chars) = ev.keystroke.key_char.as_ref() {
                    if !chars.chars().all(|ch| ch.is_control()) {
                        let mut updated = this.model_picker_query.to_string();
                        updated.push_str(chars);
                        this.model_picker_query = SharedString::new(updated);
                    }
                    this.model_picker_selected = 0;
                }
                cx.notify();
                cx.stop_propagation();
            }))
            .child(div().text_color(rgb(dark::TEXT_SECONDARY)).child("⌕"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(if query.is_empty() {
                        rgb(dark::TEXT_GHOST)
                    } else {
                        rgb(dark::TEXT)
                    })
                    .child(if query.is_empty() {
                        SharedString::from("Search models…")
                    } else {
                        query.clone()
                    }),
            );

        let mut rows: Vec<gpui::AnyElement> = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let is_selected = entry.model_arg == current_model.as_str();
                let is_highlighted = idx == highlight;
                let view_for_click = view_entity.clone();
                let model_arg_for_click = entry.model_arg.clone();
                let name = entry.name.clone();
                let provider = entry.provider.clone();
                let subtitle = entry.model_arg.clone();
                div()
                    .id(ElementId::Name(format!("model-row-{idx}").into()))
                    .h(px(58.0))
                    .px(px(12.0))
                    .rounded(px(9.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if is_highlighted {
                        rgb(dark::ACCENT)
                    } else {
                        rgba(0x00000000)
                    })
                    .bg(if is_selected {
                        rgba(dark::OVERLAY_STRONG)
                    } else if is_highlighted {
                        rgba(dark::OVERLAY)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|element| element.bg(rgba(dark::OVERLAY)))
                    .active(|element| element.opacity(0.85))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(dark::TEXT))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .text_color(rgb(dark::TEXT_TERTIARY))
                                    .child(SharedString::from(format!("{provider} · {subtitle}"))),
                            ),
                    )
                    .on_click(move |_ev, _window, cx| {
                        view_for_click.update(cx, |state, cx| {
                            let _ = state.bridge.send(UserAction::SetModel {
                                model: CompactString::new(model_arg_for_click.as_str()),
                            });
                            state.model_picker_visible = false;
                            state.model_picker_query = SharedString::new("");
                            cx.notify();
                        });
                    })
                    .into_any_element()
            })
            .collect();

        if rows.is_empty() {
            rows.push(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.5))
                    .text_color(rgb(dark::TEXT_GHOST))
                    .child("No models found")
                    .into_any_element(),
            );
        }

        // Panel: 460×390 raised card, border_strong outline, shadow. Anchored
        // above the composer (it is a flex child of the input column, so it
        // stacks above the card naturally). Fixed height so the inner list
        // scrolls instead of stretching the window.
        div()
            .flex()
            .flex_col()
            .mx_5()
            .mb_1()
            .w(px(460.0))
            .h(px(390.0))
            .relative()
            .occlude()
            .on_mouse_down_out({
                let view_entity = view_entity.clone();
                move |_ev, _window, cx| {
                    view_entity.update(cx, |state, cx| {
                        state.model_picker_visible = false;
                        state.model_picker_query = SharedString::new("");
                        cx.notify();
                    });
                }
            })
            .mb_1()
            .rounded(px(13.0))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(dark::BORDER_STRONG))
            .bg(rgb(dark::RAISED))
            .shadow_lg()
            .child(
                div()
                    .w_full()
                    .h(px(52.0))
                    .px(px(12.0))
                    .pt(px(10.0))
                    .pb(px(8.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(search_box),
            )
            .child(
                div()
                    .id("model-picker-list")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.model_picker_scroll)
                    .p(px(9.0))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .child(crate::scrollbar::vertical(
                &self.model_picker_scroll,
                &self.model_picker_scrollbar,
            ))
            .into_any_element()
    }

    /// Permission-mode menu, opened from the composer's mode chip. Rows are
    /// the engine's `SecurityMode` values; selecting one sends `/mode <name>`
    /// (the engine owns the switch and reports the new mode back via
    /// `StatusUpdate`). Small raised card, same visual language as the model
    /// picker.
    fn render_mode_picker(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        const MODES: &[(&str, &str)] = &[
            ("standard", "ask before every action"),
            ("restrictive", "deny by default, allow explicitly"),
            ("readonly", "no mutations at all"),
            ("guarded", "auto-allow safe commands"),
            ("yolo", "run everything without asking"),
        ];
        let current_mode = self.status_mode.clone();
        let view_entity = cx.entity().clone();
        let rows: Vec<gpui::AnyElement> = MODES
            .iter()
            .map(|(name, desc)| {
                let is_current = current_mode.eq_ignore_ascii_case(name);
                let name_owned = name.to_string();
                let view_for_click = view_entity.clone();
                div()
                    .id(ElementId::Name(format!("mode-row-{name}").into()))
                    .h(px(40.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgba(0x00000000))
                    .bg(if is_current {
                        rgba(dark::OVERLAY_STRONG)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|element| element.bg(rgba(dark::OVERLAY)))
                    .active(|element| element.opacity(0.85))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .font_weight(if is_current {
                                        gpui::FontWeight::SEMIBOLD
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .text_color(rgb(dark::TEXT))
                                    .child(name_owned.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(10.5))
                                    .text_color(rgb(dark::TEXT_TERTIARY))
                                    .child(desc.to_string()),
                            ),
                    )
                    .when(is_current, |d| {
                        d.child(div().text_color(rgb(dark::SUCCESS)).child("✓"))
                    })
                    .on_click(move |_ev, _window, cx| {
                        view_for_click.update(cx, |state, cx| {
                            // Use the dedicated SetMode action instead of the
                            // /mode slash command — the engine updates the
                            // permission checker and status directly.
                            let _ = state.bridge.send(UserAction::SetMode {
                                mode: CompactString::new(name_owned.as_str()),
                            });
                            state.mode_picker_visible = false;
                            cx.notify();
                        });
                    })
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .mx_5()
            .mb_1()
            .rounded(px(13.0))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(dark::BORDER_STRONG))
            .bg(rgb(dark::RAISED))
            .shadow_lg()
            .p(px(6.0))
            .occlude()
            .on_mouse_down_out({
                let view_entity = view_entity.clone();
                move |_ev, _window, cx| {
                    view_entity.update(cx, |state, cx| {
                        state.mode_picker_visible = false;
                        cx.notify();
                    });
                }
            })
            .children(rows)
            .into_any_element()
    }

    /// MCP management panel, opened from the `+ MCP` composer chip. Lists
    /// every configured server with its connection state and tool count,
    /// with a refresh control. Clicking a connected server sends a small
    /// status update through the engine (via the same `/mcp <server>` path
    /// the TUI uses, surfaced as a system message).
    fn render_mcp_picker(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let refreshing = self.mcp_refreshing;
        let servers = self.mcp_servers.clone();

        let rows: Vec<gpui::AnyElement> = if servers.is_empty() {
            vec![
                div()
                    .h(px(72.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.5))
                    .text_color(rgb(dark::TEXT_GHOST))
                    .child(if refreshing {
                        "Querying MCP servers…"
                    } else {
                        "No MCP servers configured"
                    })
                    .into_any_element(),
            ]
        } else {
            servers
                .iter()
                .map(|server| {
                    let name_owned = server.name.to_string();
                    let view_for_click = view_entity.clone();
                    let needs_login = server.needs_oauth && !server.connected;
                    let (dot_color, status_label) = if server.connected {
                        (rgb(dark::SUCCESS), "connected")
                    } else if server.needs_oauth {
                        (rgb(dark::WARNING), "needs OAuth login")
                    } else {
                        (rgb(dark::ERROR), "not connected")
                    };
                    let tool_line = match (server.connected, server.tool_count) {
                        (true, Some(n)) => format!("{n} tool(s)"),
                        (true, None) => "connected".to_string(),
                        _ => String::new(),
                    };
                    div()
                        .id(ElementId::Name(
                            format!("mcp-row-{}", server.name.as_str()).into(),
                        ))
                        .h(px(48.0))
                        .px(px(12.0))
                        .rounded(px(8.0))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .border_1()
                        .border_color(rgba(0x00000000))
                        .bg(rgba(0x00000000))
                        .hover(|element| element.bg(rgba(dark::OVERLAY)))
                        .active(|element| element.opacity(0.85))
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(dot_color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(dark::TEXT))
                                        .child(name_owned.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(10.5))
                                        .text_color(rgb(dark::TEXT_TERTIARY))
                                        .child(if tool_line.is_empty() {
                                            SharedString::from(status_label)
                                        } else {
                                            SharedString::from(format!(
                                                "{status_label} · {tool_line}"
                                            ))
                                        }),
                                ),
                        )
                        // OAuth login button: only when the server needs it.
                        .when(needs_login, |row| {
                            let view_for_login = view_entity.clone();
                            let server_for_login = name_owned.clone();
                            row.child(
                                div()
                                    .id(ElementId::Name(
                                        format!("mcp-login-{}", server_for_login).into(),
                                    ))
                                    .px_2p5()
                                    .py_1()
                                    .rounded(px(6.0))
                                    .bg(rgb(dark::INVERSE))
                                    .text_color(rgb(dark::ON_INVERSE))
                                    .text_size(px(11.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .cursor_pointer()
                                    .hover(|element| element.opacity(0.9))
                                    .child("Login")
                                    .tooltip(crate::tooltip::Tooltip::text(
                                        "Start OAuth login for this server",
                                    ))
                                    .on_click(move |_ev, _window, cx| {
                                        view_for_login.update(cx, |state, cx| {
                                            let _ = state.bridge.send(UserAction::LoginMcp {
                                                server: CompactString::new(
                                                    server_for_login.as_str(),
                                                ),
                                            });
                                            cx.notify();
                                        });
                                    }),
                            )
                        })
                        .on_click(move |_ev, _window, cx| {
                            view_for_click.update(cx, |state, cx| {
                                // Surface the server's details in the chat via
                                // the same path the TUI uses.
                                let _ = state.bridge.send(UserAction::RunSlashCommand {
                                    command: CompactString::new(format!("/mcp {}", name_owned)),
                                });
                                state.mcp_picker_visible = false;
                                cx.notify();
                            });
                        })
                        .into_any_element()
                })
                .collect()
        };

        div()
            .flex()
            .flex_col()
            .mx_5()
            .mb_1()
            .rounded(px(13.0))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(dark::BORDER_STRONG))
            .bg(rgb(dark::RAISED))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out({
                let view_entity = view_entity.clone();
                move |_ev, _window, cx| {
                    view_entity.update(cx, |state, cx| {
                        state.mcp_picker_visible = false;
                        cx.notify();
                    });
                }
            })
            .child(
                div()
                    .w_full()
                    .h(px(44.0))
                    .px(px(12.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgba(dark::BORDER))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(dark::TEXT_TERTIARY))
                            .child("MCP SERVERS"),
                    )
                    .child(
                        div()
                            .id("mcp-refresh")
                            .px_2()
                            .py_1()
                            .rounded(px(6.0))
                            .text_size(px(11.0))
                            .text_color(if refreshing {
                                rgb(dark::ACCENT)
                            } else {
                                rgb(dark::TEXT_SECONDARY)
                            })
                            .cursor_pointer()
                            .hover(|element| element.bg(rgba(dark::OVERLAY)))
                            .tooltip(crate::tooltip::Tooltip::text("Refresh MCP status"))
                            .child(if refreshing { "⟳" } else { "↻" })
                            .on_click({
                                let view_entity = view_entity.clone();
                                move |_ev, _window, cx| {
                                    view_entity.update(cx, |state, cx| {
                                        state.refresh_mcp(cx);
                                    });
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .id("mcp-picker-list")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(6.0))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .into_any_element()
    }

    /// Settings panel: manage quick models (add / delete), persisted to the
    /// config file. Opened with Cmd/Ctrl+, or from a future sidebar entry.
    /// Changes call the engine's `ReloadConfig` so the running session picks
    /// them up without a restart.
    fn render_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let models = self.settings_models.clone();
        let feedback = self.settings_feedback.clone();
        // Existing quick models, each with a delete affordance.
        let model_rows: Vec<gpui::AnyElement> = models
            .iter()
            .map(|(name, qmc)| {
                let name_owned = name.clone();
                let provider = qmc.provider.to_string();
                let model = qmc.model.to_string();
                let view_for_delete = view_entity.clone();
                div()
                    .id(ElementId::Name(format!("settings-model-{name}").into()))
                    .h(px(36.0))
                    .px(px(10.0))
                    .rounded(px(7.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .hover(|element| element.bg(rgba(dark::OVERLAY)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(dark::TEXT))
                            .child(name_owned.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(dark::TEXT_TERTIARY))
                            .child(format!("{provider} · {model}")),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(format!("settings-del-{name}").into()))
                            .px_1p5()
                            .rounded(px(5.0))
                            .text_size(px(11.0))
                            .text_color(rgb(dark::TEXT_GHOST))
                            .cursor_pointer()
                            .hover(|element| {
                                element.bg(rgba(dark::OVERLAY)).text_color(rgb(dark::ERROR))
                            })
                            .tooltip(crate::tooltip::Tooltip::text("Delete this quick model"))
                            .child("✕")
                            .on_click(move |_ev, _window, cx| {
                                view_for_delete.update(cx, |state, cx| {
                                    match zerostack_core::config::load::remove_quick_model(
                                        &name_owned,
                                    ) {
                                        Ok(true) => {
                                            state.settings_feedback = SharedString::new(format!(
                                                "removed '{name_owned}'"
                                            ));
                                        }
                                        Ok(false) => {
                                            state.settings_feedback = SharedString::new(format!(
                                                "'{name_owned}' not found"
                                            ));
                                        }
                                        Err(e) => {
                                            state.settings_feedback =
                                                SharedString::new(format!("remove failed: {e}"));
                                        }
                                    }
                                    state.reload_settings_models();
                                    state.reload_config_after_settings();
                                    cx.notify();
                                });
                            }),
                    )
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .mx_5()
            .mb_1()
            .w(px(480.0))
            .h(px(420.0))
            .relative()
            .occlude()
            .on_mouse_down_out({
                let view_entity = view_entity.clone();
                move |_ev, _window, cx| {
                    view_entity.update(cx, |state, cx| {
                        state.settings_visible = false;
                        cx.notify();
                    });
                }
            })
            .rounded(px(13.0))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(dark::BORDER_STRONG))
            .bg(rgb(dark::RAISED))
            .shadow_lg()
            .child(
                div()
                    .w_full()
                    .h(px(44.0))
                    .px(px(12.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgba(dark::BORDER))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(10.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(dark::TEXT_TERTIARY))
                            .child("SETTINGS — QUICK MODELS"),
                    )
                    .when(!feedback.is_empty(), |header| {
                        header.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(dark::SUCCESS))
                                .child(feedback.clone()),
                        )
                    })
                    // Reasoning toggle: flips the engine's chain-of-thought
                    // flag via the same `/reasoning` slash command the TUI uses.
                    .child(
                        div()
                            .id("settings-reasoning")
                            .px_2()
                            .py_1()
                            .rounded(px(6.0))
                            .text_size(px(10.5))
                            .text_color(rgb(dark::TEXT_SECONDARY))
                            .cursor_pointer()
                            .hover(|element| element.bg(rgba(dark::OVERLAY)))
                            .tooltip(crate::tooltip::Tooltip::text(
                                "Toggle chain-of-thought reasoning",
                            ))
                            .child("reasoning: toggle")
                            .on_click({
                                let view_entity = view_entity.clone();
                                move |_ev, _window, cx| {
                                    view_entity.update(cx, |state, cx| {
                                        let _ = state.bridge.send(UserAction::RunSlashCommand {
                                            command: CompactString::new("/reasoning"),
                                        });
                                        cx.notify();
                                    });
                                }
                            }),
                    ),
            )
            // Add-model form.
            .child(self.render_settings_form(cx))
            .child(self.render_provider_switch(cx))
            .child(self.render_config_section(cx))
            .child(self.render_limits_section(cx))
            .child(self.render_permission_section(cx))
            .child(self.render_editor_section(cx))
            .child(self.render_settings_save(cx))
            .child(
                div()
                    .id("settings-model-list")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(8.0))
                    .flex()
                    .flex_col()
                    .children(model_rows),
            )
            .into_any_element()
    }

    /// Add-model form inside the settings panel: name / provider / model
    /// fields with a save button. Writes via `save_quick_model` and reloads.
    fn render_settings_form(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let new_name = self.settings_new_name.clone();
        let new_provider = self.settings_new_provider.clone();
        let new_model = self.settings_new_model.clone();

        // A tiny chrome-less text field: click to focus, key_char appends.
        fn field(
            id: &'static str,
            value: &SharedString,
            placeholder: &str,
            view_entity: gpui::Entity<ShellState>,
            setter: impl Fn(&mut ShellState, String) + 'static,
        ) -> gpui::AnyElement {
            div()
                .id(ElementId::Name(id.into()))
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(rgba(dark::BORDER))
                .bg(rgb(dark::COMPOSER))
                .flex()
                .items_center()
                .cursor_text()
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(if value.is_empty() {
                            rgb(dark::TEXT_GHOST)
                        } else {
                            rgb(dark::TEXT)
                        })
                        .child(if value.is_empty() {
                            SharedString::from(placeholder)
                        } else {
                            value.clone()
                        }),
                )
                .on_key_down(move |ev: &gpui::KeyDownEvent, _window, cx| {
                    let key = ev.keystroke.key.as_str();
                    let mods = &ev.keystroke.modifiers;
                    view_entity.update(cx, |state, cx| {
                        let current = match id {
                            "settings-name" => state.settings_new_name.to_string(),
                            "settings-provider" => state.settings_new_provider.to_string(),
                            _ => state.settings_new_model.to_string(),
                        };
                        let updated = if key == "backspace" {
                            let mut s = current;
                            if mods.platform || mods.control {
                                s.clear();
                            } else {
                                s.pop();
                            }
                            s
                        } else if let Some(chars) = ev.keystroke.key_char.as_ref() {
                            let mut s = current;
                            s.push_str(chars);
                            s
                        } else {
                            return;
                        };
                        setter(state, updated);
                        cx.notify();
                    });
                })
                .into_any_element()
        }

        let name_field = field(
            "settings-name",
            &new_name,
            "name",
            view_entity.clone(),
            |state, v| state.settings_new_name = SharedString::new(v),
        );
        let provider_field = field(
            "settings-provider",
            &new_provider,
            "provider",
            view_entity.clone(),
            |state, v| state.settings_new_provider = SharedString::new(v),
        );
        let model_field = field(
            "settings-model",
            &new_model,
            "model",
            view_entity.clone(),
            |state, v| state.settings_new_model = SharedString::new(v),
        );
        let view_for_save = view_entity.clone();

        div()
            .w_full()
            .flex_none()
            .px(px(12.0))
            .py(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgba(dark::BORDER))
            .child(name_field)
            .child(provider_field)
            .child(model_field)
            .child(
                div()
                    .id("settings-save")
                    .px_2p5()
                    .py_1()
                    .rounded(px(6.0))
                    .bg(rgb(dark::INVERSE))
                    .text_color(rgb(dark::ON_INVERSE))
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|element| element.opacity(0.9))
                    .tooltip(crate::tooltip::Tooltip::text("Add quick model"))
                    .child("Add")
                    .on_click(move |_ev, _window, cx| {
                        view_for_save.update(cx, |state, cx| {
                            let name = state.settings_new_name.trim().to_string();
                            let provider = state.settings_new_provider.trim().to_string();
                            let model = state.settings_new_model.trim().to_string();
                            if name.is_empty() || provider.is_empty() || model.is_empty() {
                                state.settings_feedback =
                                    SharedString::new("name, provider and model are required");
                                cx.notify();
                                return;
                            }
                            match zerostack_core::config::load::save_quick_model(
                                &name, &provider, &model, 0.0, 0.0,
                            ) {
                                Ok(()) => {
                                    state.settings_feedback =
                                        SharedString::new(format!("added '{name}'"));
                                    state.settings_new_name = SharedString::new("");
                                    state.settings_new_provider = SharedString::new("");
                                    state.settings_new_model = SharedString::new("");
                                    state.reload_settings_models();
                                    state.reload_config_after_settings();
                                }
                                Err(e) => {
                                    state.settings_feedback =
                                        SharedString::new(format!("save failed: {e}"));
                                }
                            }
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    }

    /// Provider quick-switch inside the settings panel: the built-in
    /// providers plus any custom ones from config. Clicking one sends
    /// `SetProvider` (the engine rebuilds the agent with that provider).
    fn render_provider_switch(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let current_provider = self.status_provider.clone();
        let mut providers: Vec<String> = vec![
            "anthropic".into(),
            "openai".into(),
            "gemini".into(),
            "openrouter".into(),
            "ollama".into(),
        ];
        let (cfg, _) = zerostack_core::config::load();
        if let Some(custom) = cfg.custom_providers {
            let mut names: Vec<String> = custom.keys().cloned().collect();
            names.sort();
            providers.extend(names);
        }
        providers.sort();
        providers.dedup();

        let rows: Vec<gpui::AnyElement> = providers
            .iter()
            .map(|provider| {
                let is_current = current_provider.eq_ignore_ascii_case(provider);
                let provider_owned = provider.clone();
                let view_for_click = view_entity.clone();
                div()
                    .id(ElementId::Name(
                        format!("settings-provider-{provider}").into(),
                    ))
                    .h(px(30.0))
                    .px(px(10.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .bg(if is_current {
                        rgba(dark::OVERLAY_STRONG)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|element| element.bg(rgba(dark::OVERLAY)))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_size(px(11.5))
                            .font_weight(if is_current {
                                gpui::FontWeight::SEMIBOLD
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .text_color(rgb(dark::TEXT))
                            .child(provider_owned.clone()),
                    )
                    .when(is_current, |d| {
                        d.child(div().text_color(rgb(dark::SUCCESS)).child("✓"))
                    })
                    .on_click(move |_ev, _window, cx| {
                        view_for_click.update(cx, |state, cx| {
                            let _ = state.bridge.send(UserAction::SetProvider {
                                provider: CompactString::new(provider_owned.as_str()),
                            });
                            cx.notify();
                        });
                    })
                    .into_any_element()
            })
            .collect();

        div()
            .w_full()
            .flex_none()
            .px(px(12.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap_0p5()
            .border_b_1()
            .border_color(rgba(dark::BORDER))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(dark::TEXT_TERTIARY))
                    .child("PROVIDER"),
            )
            .child(div().flex().flex_row().flex_wrap().gap_1().children(rows))
            .into_any_element()
    }

    /// General config toggles (from `zerostack_core::config::Config`), driven
    /// by the settings panel's working copy. Every row mutates `settings_cfg`;
    /// Save writes it back.
    fn render_config_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let Some(cfg) = &self.settings_cfg else {
            return div().into_any_element();
        };

        // (label, current value, setter)
        let toggles: Vec<(&str, bool, fn(&mut zerostack_core::config::Config, bool))> = vec![
            (
                "Show reasoning",
                cfg.show_reasoning.unwrap_or(true),
                |c, v| c.show_reasoning = Some(v),
            ),
            (
                "Show tool details",
                cfg.show_tool_details.is_some(),
                |c, v| {
                    c.show_tool_details = if v { Some(Default::default()) } else { None };
                },
            ),
            (
                "Compact long sessions",
                cfg.compact_enabled.unwrap_or(true),
                |c, v| c.compact_enabled = Some(v),
            ),
            (
                "Deny repeated reads",
                cfg.deny_repeated_reads.unwrap_or(true),
                |c, v| c.deny_repeated_reads = Some(v),
            ),
            (
                "Show cost always",
                cfg.show_cost_always.unwrap_or(false),
                |c, v| c.show_cost_always = Some(v),
            ),
            (
                "Always show welcome",
                cfg.always_show_welcome.unwrap_or(false),
                |c, v| c.always_show_welcome = Some(v),
            ),
            (
                "Auto-update prompts",
                cfg.auto_update_prompts.unwrap_or(false),
                |c, v| c.auto_update_prompts = Some(v),
            ),
        ];

        let rows: Vec<gpui::AnyElement> = toggles
            .into_iter()
            .map(|(label, enabled, setter)| {
                let view = view_entity.clone();
                Self::settings_toggle_row(label, enabled, view, move |state, v| {
                    if let Some(cfg) = state.settings_cfg.as_mut() {
                        setter(cfg, v);
                    }
                })
            })
            .collect();

        div()
            .w_full()
            .flex_none()
            .px(px(12.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap_0p5()
            .border_b_1()
            .border_color(rgba(dark::BORDER))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(dark::TEXT_TERTIARY))
                    .child("GENERAL"),
            )
            .children(rows)
            .into_any_element()
    }

    /// LIMITS section: numeric tool/turn caps, edited via −/+ steppers.
    fn render_limits_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let Some(cfg) = &self.settings_cfg else {
            return div().into_any_element();
        };

        let view = view_entity.clone();
        let max_tokens = cfg.max_tokens.unwrap_or(0);
        let turns = cfg.max_agent_turns.unwrap_or(0) as u64;
        let bash_lines = cfg.max_bash_output_lines.unwrap_or(0);
        let read_lines = cfg.max_read_lines.unwrap_or(0);
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        rows.push(Self::settings_number_row(
            "Max tokens per response",
            max_tokens,
            0,
            1_000_000,
            1000,
            view.clone(),
            |state, v| {
                if let Some(cfg) = state.settings_cfg.as_mut() {
                    cfg.max_tokens = (v > 0).then_some(v);
                }
            },
        ));
        rows.push(Self::settings_number_row(
            "Max agent turns",
            turns,
            0,
            1000,
            1,
            view.clone(),
            |state, v| {
                if let Some(cfg) = state.settings_cfg.as_mut() {
                    cfg.max_agent_turns = (v > 0).then_some(v as usize);
                }
            },
        ));
        rows.push(Self::settings_number_row(
            "Max bash output lines",
            bash_lines,
            0,
            100_000,
            100,
            view.clone(),
            |state, v| {
                if let Some(cfg) = state.settings_cfg.as_mut() {
                    cfg.max_bash_output_lines = (v > 0).then_some(v);
                }
            },
        ));
        rows.push(Self::settings_number_row(
            "Max read lines",
            read_lines,
            0,
            1_000_000,
            100,
            view,
            |state, v| {
                if let Some(cfg) = state.settings_cfg.as_mut() {
                    cfg.max_read_lines = (v > 0).then_some(v);
                }
            },
        ));
        div()
            .w_full()
            .flex_none()
            .px(px(12.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap_0p5()
            .border_b_1()
            .border_color(rgba(dark::BORDER))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(dark::TEXT_TERTIARY))
                    .child("LIMITS"),
            )
            .children(rows)
            .into_any_element()
    }

    /// PERMISSION section: sandbox, default permission mode, yolo.
    fn render_permission_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let Some(cfg) = &self.settings_cfg else {
            return div().into_any_element();
        };

        let mode = cfg
            .default_permission_mode
            .clone()
            .unwrap_or_else(|| "standard".to_string());
        let modes: Vec<&str> = vec!["standard", "restrictive", "readonly", "guarded", "yolo"];
        let sandbox_view = view_entity.clone();
        let yolo_view = view_entity.clone();
        let mode_view = view_entity.clone();

        let sandbox_row = Self::settings_toggle_row(
            "Sandbox commands",
            cfg.sandbox.unwrap_or(false),
            sandbox_view,
            move |state, v| {
                if let Some(cfg) = state.settings_cfg.as_mut() {
                    cfg.sandbox = Some(v);
                }
            },
        );
        let yolo_row = Self::settings_toggle_row(
            "YOLO (skip all prompts)",
            cfg.yolo.unwrap_or(false),
            yolo_view,
            move |state, v| {
                if let Some(cfg) = state.settings_cfg.as_mut() {
                    cfg.yolo = Some(v);
                }
            },
        );
        let mode_row = Self::settings_choice_row(
            "Default permission mode",
            &modes,
            &mode,
            mode_view,
            move |state, v| {
                if let Some(cfg) = state.settings_cfg.as_mut() {
                    cfg.default_permission_mode = Some(v);
                }
            },
        );

        div()
            .w_full()
            .flex_none()
            .px(px(12.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap_0p5()
            .border_b_1()
            .border_color(rgba(dark::BORDER))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(dark::TEXT_TERTIARY))
                    .child("PERMISSIONS"),
            )
            .child(sandbox_row)
            .child(yolo_row)
            .child(mode_row)
            .into_any_element()
    }

    /// EDITOR section: shell / editor / edit system choices.
    fn render_editor_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let Some(cfg) = &self.settings_cfg else {
            return div().into_any_element();
        };

        let shell = cfg.shell.clone().unwrap_or_else(|| "default".to_string());
        let shell_opts: Vec<&str> = vec!["default", "bash", "zsh", "fish"];
        let edit_system = cfg
            .edit_system
            .map(|e| e.to_string())
            .unwrap_or_else(|| "similarity".to_string());
        let edit_opts: Vec<&str> = vec!["similarity", "hashedit"];
        let shell_view = view_entity.clone();
        let edit_view = view_entity.clone();

        let shell_row =
            Self::settings_choice_row("Shell", &shell_opts, &shell, shell_view, move |state, v| {
                if let Some(cfg) = state.settings_cfg.as_mut() {
                    cfg.shell = if v == "default" { None } else { Some(v) };
                }
            });
        let edit_row = Self::settings_choice_row(
            "Edit system",
            &edit_opts,
            &edit_system,
            edit_view,
            move |state, v| {
                if let Some(cfg) = state.settings_cfg.as_mut() {
                    cfg.edit_system = Some(if v == "hashedit" {
                        zerostack_core::config::types::EditSystem::Hashedit
                    } else {
                        zerostack_core::config::types::EditSystem::Similarity
                    });
                }
            },
        );

        div()
            .w_full()
            .flex_none()
            .px(px(12.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap_0p5()
            .border_b_1()
            .border_color(rgba(dark::BORDER))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(dark::TEXT_TERTIARY))
                    .child("EDITOR"),
            )
            .child(shell_row)
            .child(edit_row)
            .into_any_element()
    }

    /// Save button + feedback row pinned at the bottom of the settings panel.
    fn render_settings_save(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view_entity = cx.entity().clone();
        let feedback = self.settings_feedback.clone();
        div()
            .w_full()
            .flex_none()
            .px(px(12.0))
            .py(px(8.0))
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgba(dark::BORDER))
            .child(
                div()
                    .flex_1()
                    .text_size(px(10.5))
                    .text_color(if feedback.is_empty() {
                        rgb(dark::TEXT_TERTIARY)
                    } else if feedback.contains("failed") {
                        rgb(dark::ERROR)
                    } else {
                        rgb(dark::SUCCESS)
                    })
                    .child(if feedback.is_empty() {
                        SharedString::from("changes apply after Save")
                    } else {
                        feedback.clone()
                    }),
            )
            .child(
                div()
                    .id("settings-save-all")
                    .px_3()
                    .py_1p5()
                    .rounded(px(7.0))
                    .bg(rgb(dark::INVERSE))
                    .text_color(rgb(dark::ON_INVERSE))
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .hover(|element| element.opacity(0.9))
                    .tooltip(crate::tooltip::Tooltip::text("Save all settings"))
                    .child("Save")
                    .on_click({
                        let view_entity = view_entity.clone();
                        move |_ev, _window, cx| {
                            view_entity.update(cx, |state, cx| {
                                state.save_settings();
                                cx.notify();
                            });
                        }
                    }),
            )
            .into_any_element()
    }

    /// Reload the quick-model snapshot from disk after a settings change.
    fn reload_settings_models(&mut self) {
        let (cfg, _) = zerostack_core::config::load();
        self.settings_models = cfg
            .quick_models
            .as_ref()
            .map(|qm| qm.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        self.settings_models.sort_by(|a, b| a.0.cmp(&b.0));
    }

    /// Tell the engine to reload config so settings changes take effect.
    fn reload_config_after_settings(&self) {
        let _ = self.bridge.send(UserAction::ReloadConfig);
    }

    /// Open the settings panel and load current quick models.
    fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_visible = true;
        self.reload_settings_models();
        let (cfg, _) = zerostack_core::config::load();
        self.settings_cfg = Some(cfg);
        self.settings_feedback = SharedString::new("");
        cx.notify();
    }

    /// Save the settings panel's working config copy back to disk and ask the
    /// engine to reload it.
    fn save_settings(&mut self) {
        let Some(cfg) = self.settings_cfg.take() else {
            return;
        };
        match zerostack_core::config::load::save_config(&cfg) {
            Ok(()) => {
                self.settings_feedback = SharedString::new("saved — reloading…");
                let _ = self.bridge.send(UserAction::ReloadConfig);
            }
            Err(e) => {
                self.settings_feedback = SharedString::new(format!("save failed: {e}"));
                self.settings_cfg = Some(cfg);
            }
        }
    }

    /// A numeric-adjust row: `−` / `+` buttons around the current value.
    /// `min`/`max` clamp the result; the setter stores it in the working copy.
    fn settings_number_row(
        label: &str,
        value: u64,
        min: u64,
        max: u64,
        step: u64,
        view_entity: gpui::Entity<ShellState>,
        on_change: impl Fn(&mut ShellState, u64) + 'static,
    ) -> gpui::AnyElement {
        let label_owned = label.to_string();
        let current = value.to_string();
        let on_change = std::rc::Rc::new(on_change);
        let minus = std::rc::Rc::clone(&on_change);
        let plus = std::rc::Rc::clone(&on_change);
        let minus_next = value.saturating_sub(step).max(min);
        let plus_next = value.saturating_add(step).min(max);
        div()
            .id(ElementId::Name(format!("settings-number-{label}").into()))
            .h(px(30.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(rgb(dark::TEXT))
                    .child(label_owned),
            )
            .child(settings_step_btn(
                "-",
                minus_next,
                view_entity.clone(),
                move |s, v| minus(s, v),
            ))
            .child(
                div()
                    .min_w(px(40.0))
                    .text_center()
                    .text_size(px(11.5))
                    .text_color(rgb(dark::TEXT_SECONDARY))
                    .child(current),
            )
            .child(settings_step_btn(
                "+",
                plus_next,
                view_entity.clone(),
                move |s, v| plus(s, v),
            ))
            .into_any_element()
    }

    /// A choice row: clicking cycles through the given options (used for
    /// permission mode / edit system pickers).
    fn settings_choice_row(
        label: &str,
        options: &[&str],
        current: &str,
        view_entity: gpui::Entity<ShellState>,
        on_change: impl Fn(&mut ShellState, String) + 'static,
    ) -> gpui::AnyElement {
        let label_owned = label.to_string();
        let options_owned: Vec<String> = options.iter().map(|o| o.to_string()).collect();
        let current_owned = current.to_string();
        div()
            .id(ElementId::Name(format!("settings-choice-{label}").into()))
            .h(px(30.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .hover(|element| element.bg(rgba(dark::OVERLAY)))
            .tooltip(crate::tooltip::Tooltip::text(format!(
                "Click to change: {}",
                options_owned.join(" / ")
            )))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(rgb(dark::TEXT))
                    .child(label_owned),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(dark::TEXT_SECONDARY))
                    .child(current_owned.clone()),
            )
            .child(div().text_color(rgb(dark::TEXT_GHOST)).child("▸"))
            .on_click(move |_ev, _window, cx| {
                view_entity.update(cx, |state, cx| {
                    let idx = options_owned
                        .iter()
                        .position(|o| o == &current_owned)
                        .unwrap_or(0);
                    let next = &options_owned[(idx + 1) % options_owned.len()];
                    on_change(state, next.clone());
                    cx.notify();
                });
            })
            .into_any_element()
    }

    /// A boolean toggle row for a config flag: shows the flag name and its
    /// current on/off, clicking flips it in the working copy.
    fn settings_toggle_row(
        label: &str,
        enabled: bool,
        view_entity: gpui::Entity<ShellState>,
        on_toggle: impl Fn(&mut ShellState, bool) + 'static,
    ) -> gpui::AnyElement {
        let label_owned = label.to_string();
        div()
            .id(ElementId::Name(format!("settings-toggle-{label}").into()))
            .h(px(30.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .hover(|element| element.bg(rgba(dark::OVERLAY)))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(rgb(dark::TEXT))
                    .child(label_owned.clone()),
            )
            .child(
                div()
                    .w(px(30.0))
                    .h(px(16.0))
                    .rounded_full()
                    .bg(if enabled {
                        rgb(dark::ACCENT)
                    } else {
                        rgba(dark::OVERLAY_STRONG)
                    })
                    .flex()
                    .items_center()
                    .px(px(2.0))
                    .justify_center()
                    .child(
                        div()
                            .w(px(12.0))
                            .h(px(12.0))
                            .rounded_full()
                            .bg(rgb(dark::APP_BG)),
                    ),
            )
            .on_click(move |_ev, _window, cx| {
                view_entity.update(cx, |state, cx| {
                    on_toggle(state, !enabled);
                    cx.notify();
                });
            })
            .into_any_element()
    }

    /// Render the dropdown above the input box listing files in the
    /// current working directory. Clicking a row fires
    /// `UserAction::AddFile { path }` (the engine resolves to absolute +
    /// canonical), and the picker auto-closes. The list cap (64) plus
    /// ignored `.git`/`target`/etc. directories keeps the surface short;
    /// users needing the long tail route through `/add <path>`.
    fn render_file_picker(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entries: Vec<String> = self.filtered_cwd_files();
        let view_entity = cx.entity().clone();
        let query = self.file_picker_query.clone();
        let highlight = self
            .file_picker_selected
            .min(entries.len().saturating_sub(1));

        // Search field, mirroring the model picker's.
        let search_box = div()
            .id("file-picker-search")
            .track_focus(&self.file_picker_search_focus)
            .focus_visible(|d| d.border_color(rgb(dark::ACCENT)))
            .h(px(34.0))
            .px(px(10.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(rgba(dark::BORDER))
            .bg(rgb(dark::RAISED))
            .flex()
            .items_center()
            .gap_2()
            .cursor_text()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _ev, window, cx| {
                    this.file_picker_search_focus.focus(window, cx);
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                let key = ev.keystroke.key.as_str();
                let mods = &ev.keystroke.modifiers;
                if this.handle_file_picker_key(key, mods) {
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if key == "backspace" {
                    if mods.platform || mods.control {
                        this.file_picker_query = SharedString::new("");
                    } else {
                        let mut updated = this.file_picker_query.to_string();
                        updated.pop();
                        this.file_picker_query = SharedString::new(updated);
                    }
                    this.file_picker_selected = 0;
                } else if let Some(chars) = ev.keystroke.key_char.as_ref() {
                    if !chars.chars().all(|ch| ch.is_control()) {
                        let mut updated = this.file_picker_query.to_string();
                        updated.push_str(chars);
                        this.file_picker_query = SharedString::new(updated);
                    }
                    this.file_picker_selected = 0;
                }
                cx.notify();
                cx.stop_propagation();
            }))
            .child(div().text_color(rgb(dark::TEXT_SECONDARY)).child("⌕"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(if query.is_empty() {
                        rgb(dark::TEXT_GHOST)
                    } else {
                        rgb(dark::TEXT)
                    })
                    .child(if query.is_empty() {
                        SharedString::from("Search files…")
                    } else {
                        query.clone()
                    }),
            );

        let mut rows: Vec<gpui::AnyElement> = entries
            .iter()
            .enumerate()
            .map(|(idx, path)| {
                let is_highlighted = idx == highlight;
                let view_for_click = view_entity.clone();
                let path_for_click = path.clone();
                let display = path_to_display(path);
                div()
                    .id(ElementId::Name(format!("file-row-{idx}").into()))
                    .h(px(48.0))
                    .px(px(12.0))
                    .rounded(px(9.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if is_highlighted {
                        rgb(dark::ACCENT)
                    } else {
                        rgba(0x00000000)
                    })
                    .bg(if is_highlighted {
                        rgba(dark::OVERLAY)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|element| element.bg(rgba(dark::OVERLAY)))
                    .active(|element| element.opacity(0.85))
                    .child(div().text_color(rgb(dark::TEXT_TERTIARY)).child("▧"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(rgb(dark::TEXT))
                            .child(display),
                    )
                    .on_click(move |_ev, _window, cx| {
                        view_for_click.update(cx, |state, cx| {
                            let _ = state.bridge.send(UserAction::AddFile {
                                path: CompactString::new(path_for_click.as_str()),
                            });
                            state.file_picker_visible = false;
                            state.file_picker_query = SharedString::new("");
                            cx.notify();
                        });
                    })
                    .into_any_element()
            })
            .collect();

        if rows.is_empty() {
            rows.push(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.5))
                    .text_color(rgb(dark::TEXT_GHOST))
                    .child("No files found")
                    .into_any_element(),
            );
        }

        // Panel: same raised-card language as the model picker. Fixed height
        // so the inner list scrolls.
        div()
            .flex()
            .flex_col()
            .mx_5()
            .mb_1()
            .w(px(360.0))
            .h(px(340.0))
            .relative()
            .rounded(px(13.0))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(dark::BORDER_STRONG))
            .bg(rgb(dark::RAISED))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out({
                let view_entity = view_entity.clone();
                move |_ev, _window, cx| {
                    view_entity.update(cx, |state, cx| {
                        state.file_picker_visible = false;
                        state.file_picker_query = SharedString::new("");
                        cx.notify();
                    });
                }
            })
            .child(
                div()
                    .w_full()
                    .h(px(52.0))
                    .px(px(12.0))
                    .pt(px(10.0))
                    .pb(px(8.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(search_box),
            )
            .child(
                div()
                    .id("file-picker-list")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.file_picker_scroll)
                    .p(px(9.0))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .child(crate::scrollbar::vertical(
                &self.file_picker_scroll,
                &self.file_picker_scrollbar,
            ))
            .into_any_element()
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
            .gap_0()
            .mx_5()
            .mt_2()
            .rounded(px(13.0))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(dark::BORDER_STRONG))
            .bg(rgb(dark::RAISED))
            .shadow_lg()
            .max_h(px(300.0))
            .text_sm()
            .child(
                div().h(px(250.)).child(
                    uniform_list(
                        "slash-cmd-list",
                        row_count,
                        cx.processor(move |_this, range: std::ops::Range<usize>, _window, _cx| {
                            range
                                .map(|idx| {
                                    // `matches[idx]` would try to move the
                                    // String pair out of the captured Vec,
                                    // which has no mutable handle inside
                                    // this `Fn`/`FnMut` processor closure.
                                    // Clone the tuple so each row holds its
                                    // own owned strings, and the popup
                                    // closure can use them freely below.
                                    let (name, desc, _needs_arg) = matches[idx].clone();
                                    let view_for_click = view_entity.clone();
                                    // Two more copies for the click callback
                                    // closure: that closure outlives this
                                    // `map` step so we can't borrow `name`
                                    // across the iteration boundary, and
                                    // `.child(name)` on the row consumes
                                    // its argument below.
                                    let name_for_click = name.clone();
                                    div()
                                        .id(("slash-cmd", idx))
                                        .h(px(38.0))
                                        .px(px(11.0))
                                        .mx(px(4.0))
                                        .my(px(1.0))
                                        .rounded(px(8.0))
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .border_1()
                                        .border_color(if idx == current_selected {
                                            rgb(dark::ACCENT)
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .bg(if idx == current_selected {
                                            rgba(dark::OVERLAY)
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .hover(|element| element.bg(rgba(dark::OVERLAY)))
                                        .child(
                                            div()
                                                .text_color(if idx == current_selected {
                                                    rgb(dark::ACCENT)
                                                } else {
                                                    rgb(dark::TEXT)
                                                })
                                                .text_sm()
                                                .min_w(px(80.0))
                                                .child(name),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            div()
                                                .text_color(rgb(dark::TEXT_TERTIARY))
                                                .text_sm()
                                                .child(desc),
                                        )
                                        .on_click(move |_ev, _window, cx| {
                                            view_for_click.update(cx, |state, cx| {
                                                state.input_text =
                                                    SharedString::new(&name_for_click);
                                                state.input_cursor = name_for_click.chars().count();
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

/// Render the leading `+` chip on the input row. Mirrors the
/// sidebar's `+ New Session` logic so the user has one logical
/// "start fresh" affordance. We accept the entity as an
/// Status pill: shows "thinking…" while the agent is streaming
/// (click-to-cancel), or the soft "idle" chip otherwise. The
/// "cancel" path mirrors the previous standalone Cancel button,
/// so existing Ctrl-C / Cancel-click shortcuts still work the
/// same way.
fn input_chip_status(label: &str, is_thinking: bool) -> gpui::AnyElement {
    let (bg, accent) = if is_thinking {
        (rgb(dark::CHIP_ACCENT_BG), rgb(dark::ACCENT))
    } else {
        (rgb(dark::CHIP_BG), rgb(dark::TEXT_SECONDARY))
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2p5()
        .py_1()
        .rounded_md()
        .bg(bg)
        .border_1()
        .border_color(if is_thinking {
            rgb(dark::ACCENT_DEEP)
        } else {
            rgb(dark::CHIP_BORDER)
        })
        .text_color(accent)
        .child(label.to_string())
        .into_any_element()
}

/// Trailing pill on the composer controls row (Waku-style): a 26×26
/// `rounded_full` button. Idle with a draft → light `inverse` fill with a dark
/// up-arrow (click submits); idle with an empty input → muted ghost arrow;
/// busy → a stop glyph that cancels the running turn (red wash on hover).
fn input_chip_send(
    has_input: bool,
    is_thinking: bool,
    escape_stop_armed: bool,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    let has_draft = has_input && !is_thinking;
    // Armed stop: the button literally reads "Esc" as a confirm hint.
    let (bg, fg, glyph) = if is_thinking && escape_stop_armed {
        (
            rgba(dark::DANGER_SOFT),
            rgb(dark::TEXT),
            SharedString::from("Esc"),
        )
    } else if is_thinking {
        (
            rgba(dark::OVERLAY_STRONG),
            rgb(dark::TEXT),
            SharedString::from("■"),
        )
    } else if has_draft {
        (
            rgb(dark::INVERSE),
            rgb(dark::ON_INVERSE),
            SharedString::from("↑"),
        )
    } else {
        (
            rgba(dark::OVERLAY_STRONG),
            rgb(dark::TEXT_GHOST),
            SharedString::from("↑"),
        )
    };
    let mut chip = div()
        .id("input-chip-send")
        .flex()
        .items_center()
        .justify_center()
        .w(px(26.0))
        .h(px(26.0))
        .rounded_full()
        .bg(bg)
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(fg)
        .tooltip(crate::tooltip::Tooltip::text(if is_thinking {
            "Stop"
        } else {
            "Send (Enter)"
        }))
        .child(glyph);
    if is_thinking {
        // Stop: cancels the stream. Red-tinted wash on hover.
        chip = chip
            .cursor_pointer()
            .hover(|element| element.bg(rgba(dark::DANGER_SOFT)))
            .on_click({
                let view_entity = view_entity.clone();
                move |_ev, _window, cx| {
                    view_entity.update(cx, |state, cx| {
                        let _ = state.bridge.send(UserAction::CancelStream);
                        cx.notify();
                    });
                }
            });
    } else if has_draft {
        chip = chip
            .cursor_pointer()
            .hover(|element| element.opacity(0.9))
            .active(|element| element.opacity(0.8))
            .on_click({
                let view_entity = view_entity.clone();
                move |_ev, _window, cx| {
                    view_entity.update(cx, |state, cx| {
                        state.submit_input(cx);
                    });
                }
            });
    }
    chip.into_any_element()
}

/// Render a small pill chip that opens (or dismisses) one of the input-bar
/// pickers. Same shape as the plain metadata chips, but with hover/active
/// state driven by `active` — when true, the chip bg flips to the accent
/// tint and the text picks up the accent hue so the user sees an obvious
/// "this dropdown is currently open" feedback. The trailing chevron is
/// left to the caller; we don't bake one in so the helper covers both
/// the `+` row and the model row uniformly.
fn input_chip_trigger<F>(
    id: &'static str,
    label: &str,
    active: bool,
    view_entity: gpui::Entity<ShellState>,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut ShellState, &mut gpui::Window, &mut gpui::Context<ShellState>) + 'static,
{
    let (bg, fg, border) = if active {
        (
            rgb(dark::CHIP_ACCENT_BG),
            rgb(dark::ACCENT),
            rgb(dark::ACCENT_DEEP),
        )
    } else {
        (
            rgb(dark::CHIP_BG),
            rgb(dark::TEXT_SECONDARY),
            rgb(dark::CHIP_BORDER),
        )
    };
    div()
        .id(ElementId::Name(id.into()))
        .flex()
        .items_center()
        .gap_1()
        .px_2p5()
        .py_1()
        .rounded_md()
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_xs()
        .text_color(fg)
        .cursor_pointer()
        .hover(|this| this.bg(rgb(dark::CHIP_HOVER)))
        .child(label.to_string())
        .on_click({
            let view_entity = view_entity.clone();
            move |_ev, window, cx| {
                view_entity.update(cx, |state, cx| {
                    on_click(state, window, cx);
                });
            }
        })
        .into_any_element()
}

/// Load the quick-model list from the resolved config. The engine has
/// already loaded `cfg` from `~/.config/zerostack/config.toml` and merged
/// defaults, so we just point at the same source. We sort by the friendly
/// name (`key`) so the picker popup presents a stable, predictable
/// order — `cfg.quick_models` is a `HashMap`, so iteration order is
/// otherwise non-deterministic across runs.
fn load_quick_models() -> Vec<QuickModelEntry> {
    let (cfg, _is_first) = zerostack_core::config::load();
    let mut entries: Vec<QuickModelEntry> = cfg
        .quick_models
        .as_ref()
        .map(|qm| {
            qm.iter()
                .map(|(key, qmc)| QuickModelEntry {
                    name: key.to_string(),
                    provider: qmc.provider.to_string(),
                    model_arg: qmc.model.to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

impl ShellState {
    /// Keep the highlighted model row in view. Fixed 58px rows make the
    /// target offset a simple multiplication; the scroll handle clamps it.
    fn model_picker_scroll_to_selected(&mut self) {
        let row = self.model_picker_selected as f32 * 58.0;
        let viewport = self
            .model_picker_scroll
            .bounds()
            .size
            .height
            .as_f32()
            .max(1.0);
        let max = self.model_picker_scroll.max_offset().y.as_f32().max(0.0);
        let y = (row - viewport / 2.0).clamp(0.0, max);
        self.model_picker_scroll.set_offset(gpui::Point {
            x: px(0.0),
            y: px(y),
        });
    }

    /// Same for the file picker (48px rows).
    fn file_picker_scroll_to_selected(&mut self) {
        let row = self.file_picker_selected as f32 * 48.0;
        let viewport = self
            .file_picker_scroll
            .bounds()
            .size
            .height
            .as_f32()
            .max(1.0);
        let max = self.file_picker_scroll.max_offset().y.as_f32().max(0.0);
        let y = (row - viewport / 2.0).clamp(0.0, max);
        self.file_picker_scroll.set_offset(gpui::Point {
            x: px(0.0),
            y: px(y),
        });
    }

    /// Models matching the picker's free-text filter, case-insensitive, over
    /// name / provider / `provider/model`. Empty query returns everything.
    /// Ask the engine for fresh MCP server status and mark the request as
    /// in-flight so the panel can show a spinner.
    fn refresh_mcp(&mut self, cx: &mut Context<Self>) {
        if !self.bridge.send(UserAction::QueryMcp) {
            self.last_error = Some(SharedString::new("engine is offline"));
            return;
        }
        self.mcp_refreshing = true;
        cx.notify();
    }

    fn filtered_quick_models(&self) -> Vec<QuickModelEntry> {
        let query = self.model_picker_query.trim();
        if query.is_empty() {
            return self.quick_models.clone();
        }
        let query = query.to_ascii_lowercase();
        self.quick_models
            .iter()
            .filter(|entry| {
                entry.name.to_ascii_lowercase().contains(&query)
                    || entry.provider.to_ascii_lowercase().contains(&query)
                    || entry.model_arg.to_ascii_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    /// Shared keyboard navigation for the model picker. Returns `true` when
    /// the key was consumed (caller should stop propagation). Used by both
    /// the input-box listener and the search field's own listener so the
    /// arrow/enter/esc keys behave identically whether the search box or the
    /// composer holds focus.
    fn handle_model_picker_key(&mut self, key: &str, mods: &gpui::Modifiers) -> bool {
        match key {
            "up" => {
                let len = self.filtered_quick_models().len();
                if len > 0 {
                    let cur = self.model_picker_selected as isize - 1;
                    self.model_picker_selected = cur.rem_euclid(len as isize) as usize;
                    self.model_picker_scroll_to_selected();
                }
                true
            }
            "down" => {
                let len = self.filtered_quick_models().len();
                if len > 0 {
                    let cur = self.model_picker_selected as isize + 1;
                    self.model_picker_selected = cur.rem_euclid(len as isize) as usize;
                    self.model_picker_scroll_to_selected();
                }
                true
            }
            "enter" => {
                let matches = self.filtered_quick_models();
                if let Some(entry) = matches.get(self.model_picker_selected).cloned() {
                    let _ = self.bridge.send(UserAction::SetModel {
                        model: CompactString::new(entry.model_arg.as_str()),
                    });
                    self.model_picker_visible = false;
                    self.model_picker_query = SharedString::new("");
                }
                true
            }
            "escape" => {
                self.model_picker_visible = false;
                self.model_picker_query = SharedString::new("");
                true
            }
            "backspace" if mods.platform || mods.control => {
                self.model_picker_query = SharedString::new("");
                true
            }
            _ => false,
        }
    }

    /// Files matching the picker's free-text filter, case-insensitive
    /// substring over the display path. Empty query returns everything.
    fn filtered_cwd_files(&self) -> Vec<String> {
        let query = self.file_picker_query.trim();
        if query.is_empty() {
            return self.cwd_files.clone();
        }
        let query = query.to_ascii_lowercase();
        self.cwd_files
            .iter()
            .filter(|path| {
                path.to_ascii_lowercase().contains(&query)
                    || path_to_display(path).to_ascii_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    /// Shared keyboard navigation for the file picker. Returns `true` when
    /// the key was consumed (caller should stop propagation). Mirrors
    /// [`ShellState::handle_model_picker_key`].
    fn handle_file_picker_key(&mut self, key: &str, mods: &gpui::Modifiers) -> bool {
        match key {
            "up" => {
                let len = self.filtered_cwd_files().len();
                if len > 0 {
                    let cur = self.file_picker_selected as isize - 1;
                    self.file_picker_selected = cur.rem_euclid(len as isize) as usize;
                    self.file_picker_scroll_to_selected();
                }
                true
            }
            "down" => {
                let len = self.filtered_cwd_files().len();
                if len > 0 {
                    let cur = self.file_picker_selected as isize + 1;
                    self.file_picker_selected = cur.rem_euclid(len as isize) as usize;
                    self.file_picker_scroll_to_selected();
                }
                true
            }
            "enter" => {
                let matches = self.filtered_cwd_files();
                if let Some(path) = matches.get(self.file_picker_selected).cloned() {
                    let _ = self.bridge.send(UserAction::AddFile {
                        path: CompactString::new(path.as_str()),
                    });
                    self.file_picker_visible = false;
                    self.file_picker_query = SharedString::new("");
                }
                true
            }
            "escape" => {
                self.file_picker_visible = false;
                self.file_picker_query = SharedString::new("");
                true
            }
            "backspace" if mods.platform || mods.control => {
                self.file_picker_query = SharedString::new("");
                true
            }
            _ => false,
        }
    }
}

/// List of files in the current working directory. Refreshed once at
/// startup. We deliberately cap to a small number and skip common
/// noise directories (`.git`, hidden dirs) — the picker is a quick
/// "attach this file" affordance, not a project-wide browser, so the
/// first ~64 names the user sees is what matters. Hidden screenshot/
/// tool-cache folders can always be reached through the `/add` slash
/// command for the long tail.
fn load_cwd_files() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(".") else {
        return names;
    };
    for entry in entries.flatten() {
        if names.len() >= 64 {
            break;
        }
        let raw = entry.file_name();
        let name = raw.to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        // Skip directories that are known noise targets — `.git`,
        // `target`, `node_modules` style — when the user is in a fresh
        // checkout they don't want these crowding the picker.
        if matches!(
            name.as_str(),
            "target" | "node_modules" | "dist" | "build" | "__pycache__"
        ) {
            continue;
        }
        names.push(name);
    }
    names.sort();
    names
}

/// Render a path string for the file picker: truncate the directory prefix
/// so the row stays roughly on-screen for any depth (the picker scrolls if
/// it's longer than the row, but keeping it under ~48 columns means the
/// user doesn't need to scroll horizontally inside a row).
fn path_to_display(path: &str) -> String {
    if path.len() <= 48 {
        return path.to_string();
    }
    // Keep the trailing 30 chars which usually hold the basename and a hint
    // about its parent: `…/parent/very_long_filename.rs`.
    let head_budget = 16;
    let tail_budget = 30;
    let ellipsis = "…/";
    let head = &path[..head_budget.min(path.len())];
    let tail_start = path.len().saturating_sub(tail_budget);
    format!("{head}{ellipsis}{}", &path[tail_start..])
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

/// Open right-click context menu for a message row.
#[derive(Clone, Debug)]
struct MsgMenuState {
    msg_idx: usize,
    /// Window-space position for the menu's top-left corner.
    x: f32,
    y: f32,
}

/// One flattened row of the chat transcript. The renderer builds these from
/// the raw message list so activity runs, reasoning folds and turn folds can
/// be interleaved with plain messages.
#[derive(Clone, Debug)]
enum ChatRow {
    User(usize),
    /// (index, is_streaming)
    Assistant(usize, bool),
    System(usize),
    Permission(usize),
    /// Plain tool message without structured metadata.
    ToolText(usize),
    /// Reasoning fold at index.
    Reasoning(usize),
    /// Consecutive structured tool messages [start, end).
    ToolRun {
        start: usize,
        end: usize,
    },
    /// "Worked for Ns" divider; `answer_idx` is the settled answer below it.
    TurnFold {
        answer_idx: usize,
        elapsed: f32,
    },
    /// Pinned "Working for Ns" row while the agent is busy.
    Working(f32),
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

/// Humanized time for the hover footer: "9:05 AM" today, "Yesterday 5:00 PM"
/// otherwise, "May 12, 1:12 PM" within the same year, "Aug 4 2024, 11:00 AM"
/// beyond that.
fn format_message_time(when: std::time::Instant) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let then = now.saturating_sub(when.elapsed().as_secs() as i64);
    format_unix_time(then, now)
}

/// Format a unix timestamp relative to `now` (both in seconds).
fn format_unix_time(then: i64, now: i64) -> String {
    // Twelve-hour clock helper.
    fn hm(secs_of_day: i64) -> String {
        let h24 = ((secs_of_day / 3600) % 24 + 24) % 24;
        let m = (secs_of_day / 60) % 60;
        let period = if h24 >= 12 { "PM" } else { "AM" };
        let h12 = if h24 % 12 == 0 { 12 } else { h24 % 12 };
        format!("{h12}:{m:02} {period}")
    }

    const DAY: i64 = 86400;
    let (now_day, _now_tod) = (now.div_euclid(DAY), now.rem_euclid(DAY));
    let (then_day, then_tod) = (then.div_euclid(DAY), then.rem_euclid(DAY));
    if then_day == now_day {
        return hm(then_tod);
    }
    // "Yesterday" for calendar-day differences, weekday within 6 days, else date.
    let days = now_day - then_day;
    if days == 1 {
        return format!("Yesterday {}", hm(then_tod));
    }
    if days <= 7 && days > 1 {
        let weekday = [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ];
        // Day 0 = Thursday 1970-01-01.
        let idx = (then_day.rem_euclid(7) as usize + 3) % 7;
        return format!("{} {}", weekday[idx], hm(then_tod));
    }
    // Month / day (year when the date differs from the current year).
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // Approximate calendar from day count (good enough for footers; drift of a
    // few days near year boundaries is acceptable for a hover timestamp).
    let mut year = 1970i64;
    let mut rem = then_day;
    let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    loop {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days_in_year = if leap { 366 } else { 365 };
        if rem < days_in_year {
            break;
        }
        rem -= days_in_year;
        year += 1;
    }
    let mut month = 0usize;
    let mut day_of_month = rem;
    for (i, md) in month_days.iter().enumerate() {
        let days_in_month = if i == 1 && year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            29
        } else {
            *md
        };
        if day_of_month < days_in_month {
            month = i;
            break;
        }
        day_of_month -= days_in_month;
    }
    let cur_year = {
        let mut y = 1970i64;
        let mut r = now_day;
        loop {
            let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
            let d = if leap { 366 } else { 365 };
            if r < d {
                break;
            }
            r -= d;
            y += 1;
        }
        y
    };
    let date = format!(
        "{} {}{}",
        months[month],
        day_of_month + 1,
        ordinal_suffix(day_of_month + 1)
    );
    if year == cur_year {
        format!("{date}, {}", hm(then_tod))
    } else {
        format!("{date} {year}, {}", hm(then_tod))
    }
}

fn ordinal_suffix(n: i64) -> &'static str {
    match n % 100 {
        11..=13 => "th",
        _ => match n % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        },
    }
}

/// Wrap a message row so a right-click opens the context menu at the pointer.
fn wrap_msg_menu(
    inner: gpui::AnyElement,
    msg_idx: usize,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    div()
        .w_full()
        .min_w_0()
        .on_mouse_down(
            gpui::MouseButton::Right,
            move |ev: &gpui::MouseDownEvent, _window, cx| {
                view_entity.update(cx, |state, cx| {
                    state.msg_menu = Some(MsgMenuState {
                        msg_idx,
                        x: ev.position.x.as_f32(),
                        y: ev.position.y.as_f32(),
                    });
                    cx.notify();
                });
            },
        )
        .child(inner)
        .into_any_element()
}

/// One row of the message context menu.
fn menu_item<F>(
    glyph: &'static str,
    label: &'static str,
    view_entity: gpui::Entity<ShellState>,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut ShellState, &mut gpui::Context<ShellState>) + 'static,
{
    div()
        .id(ElementId::Name(format!("msg-menu-{label}").into()))
        .h(px(30.0))
        .px(px(9.0))
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .hover(|element| element.bg(rgba(dark::OVERLAY)))
        .child(
            div()
                .w(px(16.0))
                .text_color(rgb(dark::TEXT_TERTIARY))
                .child(glyph),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(dark::TEXT))
                .child(label),
        )
        .on_click(move |_ev, _window, cx| {
            view_entity.update(cx, |state, cx| {
                on_click(state, cx);
            });
        })
        .into_any_element()
}

/// Concatenate every fenced code block in `text`, separated by blank lines
/// (the "Copy code" context-menu action). Returns empty when there are none.
fn extract_fenced_code(text: &str) -> String {
    let mut blocks: Vec<String> = Vec::new();
    let mut in_block = false;
    let mut buf = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if in_block {
                blocks.push(std::mem::take(&mut buf));
                in_block = false;
            } else {
                in_block = true;
            }
            continue;
        }
        if in_block {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if in_block {
        blocks.push(buf);
    }
    blocks
        .iter()
        .map(|b| b.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Hover footer shared by user / assistant rows: timestamp + copy button,
/// revealed on row hover via a group. `align_right` right-aligns (user rows).
fn render_message_footer(
    msg: &ChatMessage,
    idx: usize,
    align_right: bool,
    view_entity: gpui::Entity<ShellState>,
    copied: bool,
) -> gpui::AnyElement {
    let group = SharedString::from(format!("msg-row-{idx}"));
    let time = msg.sent_at.map(format_message_time);
    let copy_text = msg.content.to_string();
    let footer_color = rgb(dark::TEXT_GHOST);

    let mut footer = div()
        .w_full()
        .h(px(27.0))
        .flex()
        .items_center()
        .gap_1()
        .invisible()
        .group_hover(group.clone(), |element| element.visible());
    if align_right {
        footer = footer.justify_end();
    } else {
        footer = footer.ml(px(-6.0));
    }
    let mut items: Vec<gpui::AnyElement> = Vec::new();

    if !align_right {
        // Assistant: [copy][time]
        items.push(copy_button(
            idx,
            copy_text.clone(),
            copied,
            view_entity.clone(),
        ));
    }
    if let Some(time) = time {
        items.push(
            div()
                .h(px(27.0))
                .px(px(4.0))
                .flex()
                .items_center()
                .text_size(px(11.5))
                .line_height(px(14.0))
                .text_color(footer_color)
                .child(time)
                .into_any_element(),
        );
    }
    if align_right {
        items.push(copy_button(idx, copy_text, copied, view_entity));
    }

    footer.children(items).into_any_element()
}

/// 27x27 copy icon-button; swaps to a check for 2s after a click.
fn copy_button(
    idx: usize,
    text: String,
    copied: bool,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    let glyph = if copied { "✓" } else { "⧉" };
    let color = if copied {
        rgb(dark::SUCCESS)
    } else {
        rgb(dark::TEXT_GHOST)
    };
    div()
        .id(ElementId::Name(format!("copy-msg-{idx}").into()))
        .w(px(27.0))
        .h(px(27.0))
        .rounded(px(8.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .text_color(color)
        .cursor_pointer()
        .hover(|element| element.bg(rgba(dark::OVERLAY_STRONG)))
        .tooltip(crate::tooltip::Tooltip::text(if copied {
            "Copied"
        } else {
            "Copy message"
        }))
        .child(glyph)
        .on_click(move |_ev, _window, cx| {
            view_entity.update(cx, |state, cx| {
                state.copied_message = Some((idx, Instant::now()));
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                cx.notify();
            });
        })
        .into_any_element()
}

/// User message: right-aligned raised bubble (Waku-style), max 540px wide.
fn render_user_msg(
    msg: &ChatMessage,
    idx: usize,
    view_entity: gpui::Entity<ShellState>,
    copied: bool,
) -> gpui::AnyElement {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .items_end()
        .group(SharedString::from(format!("msg-row-{idx}")))
        .child(
            div()
                .max_w(px(540.0))
                .rounded(px(12.0))
                .bg(rgb(dark::RAISED))
                .px(px(12.0))
                .py(px(8.0))
                .text_size(px(14.0))
                .line_height(px(20.0))
                .text_color(rgb(dark::TEXT))
                .whitespace_normal()
                .child(msg.content.to_string()),
        )
        .child(render_message_footer(msg, idx, true, view_entity, copied))
        .into_any_element()
}

/// Assistant message: flat, full-width markdown (no bubble), with a hover
/// footer and a streaming caret while chunks are still landing. `blocks` is
/// the (cached) parse of `msg.content`.
fn render_assistant_msg(
    msg: &ChatMessage,
    idx: usize,
    is_streaming: bool,
    view_entity: gpui::Entity<ShellState>,
    copied: bool,
    blocks: Arc<Vec<MarkdownBlock>>,
    copied_code: Option<(usize, usize)>,
) -> gpui::AnyElement {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .py(px(4.0))
        .gap_1()
        .group(SharedString::from(format!("msg-row-{idx}")))
        .child(render_markdown_blocks_copied(
            &blocks,
            idx,
            copied_code,
            view_entity.clone(),
        ))
        .when(is_streaming, |d| {
            d.child(
                div()
                    .w(px(8.0))
                    .h(px(16.0))
                    .bg(rgb(dark::ACCENT))
                    .rounded_sm()
                    .mt_1(),
            )
        })
        .child(render_message_footer(msg, idx, false, view_entity, copied))
        .into_any_element()
}

/// System message: centered overlay pill, muted.
fn render_system_msg(msg: &ChatMessage) -> gpui::AnyElement {
    div()
        .w_full()
        .flex()
        .justify_center()
        .child(
            div()
                .px(px(10.0))
                .py(px(4.0))
                .rounded_full()
                .bg(rgba(dark::OVERLAY))
                .text_size(px(11.0))
                .line_height(px(16.0))
                .text_color(rgb(dark::TEXT_TERTIARY))
                .child(msg.content.to_string()),
        )
        .into_any_element()
}

/// Plain tool message (no structured meta): a compact mono line.
fn render_tool_text(msg: &ChatMessage) -> gpui::AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .text_size(px(11.5))
        .line_height(px(16.0))
        .font_family("ui-monospace")
        .text_color(rgb(dark::TEXT_SECONDARY))
        .child(msg.content.to_string())
        .into_any_element()
}

/// Working indicator: three chasing dots + "Working for Ns".
fn render_working_row(elapsed: f32) -> gpui::AnyElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(11.5))
        .line_height(px(16.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(dark::TEXT_TERTIARY))
        .child(working_dots())
        .child(SharedString::from(format!(
            "Working for {}",
            format_elapsed(elapsed)
        )))
        .into_any_element()
}

/// Three small pulsing dots (respects reduce-motion via gpui animation).
fn working_dots() -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .children((0..3).map(|i| {
            div()
                .w(px(4.5))
                .h(px(4.5))
                .rounded_full()
                .bg(rgb(dark::TEXT_TERTIARY))
                .with_animation(
                    format!("working-dot-{i}"),
                    gpui::Animation::new(Duration::from_millis(1400))
                        .repeat()
                        .with_easing(gpui::pulsating_between(0.25, 1.0)),
                    move |element, delta| {
                        let phase = ((delta * 2.0 + i as f32 * 0.18).fract() as f64) as f32;
                        element.opacity(0.25 + 0.75 * phase)
                    },
                )
                .into_any_element()
        }))
        .into_any_element()
}

/// Compact duration label: "9s", "1m 12s", "1h 2m".
fn format_elapsed(secs: f32) -> String {
    let total = secs.max(0.0) as u64;
    if total < 60 {
        return format!("{total}s");
    }
    let m = total / 60;
    let s = total % 60;
    if m < 60 {
        return format!("{m}m {s}s");
    }
    let h = m / 60;
    let rm = m % 60;
    format!("{h}h {rm}m")
}

/// Reasoning row: folds to a one-line "Thinking…" / "Thought for Ns" header
/// when collapsed; expands to muted markdown.
fn render_reasoning_row(
    msg: &ChatMessage,
    idx: usize,
    expanded: bool,
    is_live: bool,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    let header_label = if is_live {
        "Thinking…".to_string()
    } else if let Some(sent) = msg.sent_at {
        format!(
            "Thought for {}",
            format_elapsed(sent.elapsed().as_secs_f32())
        )
    } else {
        "Thinking…".to_string()
    };
    let chevron = if expanded { "▾" } else { "▸" };
    let header = div()
        .id(ElementId::Name(format!("reasoning-toggle-{idx}").into()))
        .flex()
        .items_center()
        .gap_1p5()
        .text_size(px(11.5))
        .line_height(px(16.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(dark::TEXT_TERTIARY))
        .cursor_pointer()
        .hover(|element| element.text_color(rgb(dark::TEXT_SECONDARY)))
        .child(if is_live {
            pulse_dot(format!("reasoning-pulse-{idx}"), 5.0, dark::ACCENT)
        } else {
            div().w(px(5.0)).into_any_element()
        })
        .child(header_label)
        .child(div().text_color(rgb(dark::TEXT_GHOST)).child(chevron))
        .on_click(move |_ev, _window, cx| {
            view_entity.update(cx, |state, cx| {
                let next = !state.reasoning_expanded.get(&idx).copied().unwrap_or(true);
                state.reasoning_expanded.insert(idx, next);
                cx.notify();
            });
        });

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .child(header)
        .when(expanded, |d| {
            d.child(
                div()
                    .w_full()
                    .min_w_0()
                    .pl_3()
                    .border_l_2()
                    .border_color(rgba(dark::BORDER_STRONG))
                    .text_color(rgb(dark::TEXT_TERTIARY))
                    .child(render_markdown_body(msg.content.as_str())),
            )
        })
        .into_any_element()
}

/// A 5px accent pulse dot (running indicators).
fn pulse_dot(id: impl Into<SharedString>, size: f32, color: u32) -> gpui::AnyElement {
    div()
        .w(px(size))
        .h(px(size))
        .rounded_full()
        .bg(rgb(color))
        .with_animation(
            id.into(),
            gpui::Animation::new(Duration::from_millis(1600))
                .repeat()
                .with_easing(gpui::pulsating_between(0.3, 1.0)),
            |element, delta| element.opacity(delta),
        )
        .into_any_element()
}

/// Tool-activity run: a collapsible cluster of tool invocations between a
/// user message and the answer. Collapsed shows a one-line summary
/// ("bash · read 2 files") + chevron; expanded shows per-item rows with
/// running/ok status and click-to-reveal output.
fn render_tool_run(
    msgs: &[ChatMessage],
    start: usize,
    _end: usize,
    expanded: bool,
    is_live: bool,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    let summary = tool_run_summary(msgs);
    let chevron = if expanded { "▾" } else { "▸" };
    let running = is_live;
    // The on_click closure below captures `view_entity`; clone before moving
    // it in so the per-item loop can still use the original.
    let view_for_toggle = view_entity.clone();
    let header = div()
        .id(ElementId::Name(format!("tool-run-{start}").into()))
        .flex()
        .items_center()
        .gap_1p5()
        .text_size(px(11.5))
        .line_height(px(14.0))
        .text_color(rgb(dark::TEXT_TERTIARY))
        .cursor_pointer()
        .hover(|element| element.text_color(rgb(dark::TEXT_SECONDARY)))
        .when(running, |element| {
            element.child(pulse_dot(
                format!("tool-run-pulse-{start}"),
                5.0,
                dark::ACCENT,
            ))
        })
        .child(div().font_weight(gpui::FontWeight::MEDIUM).child(summary))
        .child(div().text_color(rgb(dark::TEXT_GHOST)).child(chevron))
        .on_click(move |_ev, _window, cx| {
            view_for_toggle.update(cx, |state, cx| {
                let next = !state.activity_expanded.get(&start).copied().unwrap_or(true);
                state.activity_expanded.insert(start, next);
                cx.notify();
            });
        });

    let mut children: Vec<gpui::AnyElement> = Vec::new();
    children.push(header.into_any_element());
    if expanded {
        for (offset, msg) in msgs.iter().enumerate() {
            children.push(render_tool_item(msg, start + offset, view_entity.clone()));
        }
    }
    let _ = view_entity;

    div()
        .w_full()
        .flex()
        .flex_col()
        .pl(px(2.0))
        .children(children)
        .into_any_element()
}

/// One tool invocation inside a run: icon + name/args + status glyph.
fn render_tool_item(
    msg: &ChatMessage,
    idx: usize,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    let Some(meta) = msg.tool_meta() else {
        return render_tool_text(msg);
    };
    let status = &meta.status;
    let (status_glyph, status_color) = match status {
        ToolStatus::Pending => (
            pulse_dot(format!("tool-pulse-{idx}"), 5.0, dark::ACCENT),
            rgb(dark::ACCENT),
        ),
        ToolStatus::Ok => (
            div()
                .text_color(rgb(dark::TEXT_GHOST))
                .child("✓")
                .into_any_element(),
            rgb(dark::TEXT_GHOST),
        ),
    };
    let _ = status_color;

    let has_detail = !meta.result.is_empty();
    // Capture the tool name for the toggle closure (the closure outlives
    // `msg`, so we need an owned string, not a borrow).
    let toggle_name = meta.name.to_string();
    let body: gpui::AnyElement = if has_detail && meta.expanded {
        div()
            .ml(px(21.0))
            .mr(px(4.0))
            .min_w_0()
            .mt(px(2.0))
            .mb(px(4.0))
            .p(px(8.0))
            .rounded(px(7.0))
            .bg(rgb(dark::INSET))
            .border_1()
            .border_color(rgba(dark::BORDER))
            .flex()
            .flex_col()
            .gap_1()
            .font_family("ui-monospace")
            .text_size(px(10.5))
            .line_height(px(16.0))
            .text_color(rgb(dark::TEXT_SECONDARY))
            .whitespace_normal()
            .child(meta.result.to_string())
            .into_any_element()
    } else {
        div().into_any_element()
    };

    let row = div()
        .id(ElementId::Name(format!("tool-item-{idx}").into()))
        .min_h(px(24.0))
        .px(px(4.0))
        .py(px(2.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(11.5))
        .line_height(px(14.0))
        .when(has_detail, |element| {
            element
                .cursor_pointer()
                .hover(|element| element.bg(rgba(dark::OVERLAY)))
        })
        .child(status_glyph)
        .child(
            div()
                .flex_none()
                .max_w(px(300.0))
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(dark::TEXT_SECONDARY))
                        .child(meta.name.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(dark::TEXT_GHOST))
                        .child(meta.args_summary.to_string()),
                ),
        )
        // Collapsed preview: a one-line peek at the tool output.
        .when(has_detail && !meta.expanded, |element| {
            element.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.0))
                    .text_color(rgb(dark::TEXT_GHOST))
                    .child(tool_utils::preview_tool_result(meta.result.as_str(), 120)),
            )
        });
    let row = if has_detail {
        row.on_click(move |_ev, _window, cx| {
            view_entity.update(cx, |state, cx| {
                for m in state.chat.iter_mut().rev() {
                    if m.role == Role::Tool
                        && let Some(meta) = m.tool_meta.as_mut()
                        && meta.name.as_ref() == toggle_name.as_str()
                    {
                        meta.expanded = !meta.expanded;
                        cx.notify();
                        return;
                    }
                }
            });
        })
    } else {
        row
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .child(row)
        .child(body)
        .into_any_element()
}

/// "Read 2 files · Ran bash" style summary for a tool run.
fn tool_run_summary(msgs: &[ChatMessage]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut any_pending = false;
    for msg in msgs {
        if let Some(meta) = msg.tool_meta() {
            if meta.status == ToolStatus::Pending {
                any_pending = true;
            }
            let name = meta.name.to_string();
            if let Some((_, c)) = counts.iter_mut().find(|(n, _)| *n == name) {
                *c += 1;
            } else {
                counts.push((name, 1));
            }
        }
    }
    let verb = if any_pending { "Running" } else { "Ran" };
    let parts: Vec<String> = counts
        .into_iter()
        .map(|(name, count)| {
            if count > 1 {
                format!("{verb} {name} ×{count}")
            } else {
                format!("{verb} {name}")
            }
        })
        .collect();
    if parts.is_empty() {
        "Tool activity".to_string()
    } else {
        parts.join(" · ")
    }
}

/// Centered "Worked for Ns" divider folding a settled turn's activity.
fn render_turn_fold(
    answer_idx: usize,
    elapsed: f32,
    expanded: bool,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    let label = format!("Worked for {}", format_elapsed(elapsed));
    let chevron = if expanded { "▾" } else { "▸" };
    div()
        .w_full()
        .flex()
        .items_center()
        .gap_3()
        .py_1()
        .child(div().h(px(1.0)).flex_1().bg(rgba(dark::BORDER)))
        .child(
            div()
                .id(ElementId::Name(format!("turn-fold-{answer_idx}").into()))
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(11.5))
                .line_height(px(16.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(rgb(dark::TEXT_TERTIARY))
                .cursor_pointer()
                .hover(|element| element.text_color(rgb(dark::TEXT_SECONDARY)))
                .child(label)
                .child(div().text_color(rgb(dark::TEXT_GHOST)).child(chevron))
                .on_click(move |_ev, _window, cx| {
                    view_entity.update(cx, |state, cx| {
                        let next = !state
                            .turn_fold_expanded
                            .get(&answer_idx)
                            .copied()
                            .unwrap_or(false);
                        state.turn_fold_expanded.insert(answer_idx, next);
                        cx.notify();
                    });
                }),
        )
        .child(div().h(px(1.0)).flex_1().bg(rgba(dark::BORDER)))
        .into_any_element()
}

/// Render assistant / tool / system text as markdown.
///
/// The TUI's `markdown.rs` turns the source into a styled ANSI stream; here
/// we tokenize via `crate::markdown` and rebuild each block as a small
/// `Div` tree. Span styling uses native gpui text primitives, but to
/// guarantee word wrapping we render each paragraph as a single `Div` with
/// `whitespace_normal`: with one child text node, gpui's text layout
/// behaves predictably across rows. A paragraph splits back into a
/// `flex_row` only when it contains inline code / links / explicit
/// formatting, so chip-like spans (inline code, links) can keep their
/// background.
fn render_markdown_body(text: &str) -> gpui::AnyElement {
    let blocks = Arc::new(parse_markdown(text));
    render_markdown_blocks(&blocks)
}

/// Render pre-parsed markdown blocks (cached by [`ShellState::blocks_for`]).
fn render_markdown_blocks(blocks: &[MarkdownBlock]) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .min_w_0()
        .text_color(rgb(dark::TEXT))
        .text_sm()
        .children(blocks.iter().cloned().map(render_markdown_block))
        .into_any_element()
}

/// Markdown blocks whose code blocks carry per-block copy buttons. `msg_idx`
/// keys the copied-feedback state on [`ShellState::copied_code`]; `view_entity`
/// is captured by the buttons' click handlers.
fn render_markdown_blocks_copied(
    blocks: &[MarkdownBlock],
    msg_idx: usize,
    copied_code: Option<(usize, usize)>,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .min_w_0()
        .text_color(rgb(dark::TEXT))
        .text_sm()
        .children(
            blocks
                .iter()
                .cloned()
                .enumerate()
                .map(|(block_idx, block)| {
                    render_markdown_block_copied(
                        block,
                        msg_idx,
                        block_idx,
                        copied_code,
                        view_entity.clone(),
                    )
                }),
        )
        .into_any_element()
}

/// Like [`render_markdown_block`] but code blocks get a copy button in their
/// header bar. `copied_code` is the `(msg_idx, block_idx)` currently showing
/// its "copied ✓" feedback, if any.
fn render_markdown_block_copied(
    block: MarkdownBlock,
    msg_idx: usize,
    block_idx: usize,
    copied_code: Option<(usize, usize)>,
    view_entity: gpui::Entity<ShellState>,
) -> gpui::AnyElement {
    match block.kind {
        BlockKind::CodeBlock(lang) => {
            let joined: String = block
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>()
                .trim_end_matches('\n')
                .to_string();
            render_code_block(
                &joined,
                lang,
                Some((msg_idx, block_idx)),
                copied_code == Some((msg_idx, block_idx)),
                Some(view_entity),
            )
        }
        _ => render_markdown_block(block),
    }
}

/// `TextRun`s that tile `code` exactly, colored by the lexer. Every run shares
/// one font (mono, 12px), so the shaped width of a line is identical with or
/// without highlighting — the property that makes coloring safe to defer
/// while a block is still streaming.
fn code_runs(code: &str, lang: highlight::Lang, plain: u32) -> Vec<TextRun> {
    let token_color = |class: TokenClass| -> u32 {
        match class {
            TokenClass::Keyword => dark::TOKEN_KEYWORD,
            TokenClass::Literal => dark::TOKEN_LITERAL,
            TokenClass::String => dark::TOKEN_STRING,
            TokenClass::Comment => dark::TEXT_GHOST,
            TokenClass::Number => dark::TOKEN_LITERAL,
            TokenClass::Type => dark::TOKEN_TYPE,
            TokenClass::Function => dark::TOKEN_TYPE,
            TokenClass::Meta => dark::TEXT_TERTIARY,
            TokenClass::Added => dark::SUCCESS,
            TokenClass::Removed => dark::ERROR,
        }
    };

    let mut font = font("ui-monospace");
    font.weight = gpui::FontWeight::NORMAL;
    let mut runs: Vec<TextRun> = Vec::new();
    let push = |runs: &mut Vec<TextRun>, len: usize, color: u32| {
        if len == 0 {
            return;
        }
        let color: gpui::Hsla = rgb(color).into();
        match runs.last_mut() {
            Some(last) if last.color == color => last.len += len,
            _ => runs.push(TextRun {
                len,
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }),
        }
    };

    let tokenized = highlight::tokenize(lang, code);
    let lines = code.split('\n').collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let tokens = tokenized.get(index).map(Vec::as_slice).unwrap_or_default();
        let mut cursor = 0;
        for token in tokens {
            push(&mut runs, token.range.start.saturating_sub(cursor), plain);
            push(&mut runs, token.range.len(), token_color(token.class));
            cursor = token.range.end;
        }
        push(&mut runs, line.len().saturating_sub(cursor), plain);
        if index + 1 < lines.len() {
            // The '\n' separator must belong to a run or shaping rejects them.
            push(&mut runs, 1, plain);
        }
    }
    runs
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
            let joined: String = block.spans.iter().map(|s| s.text.as_str()).collect();
            div()
                .w_full()
                .min_w_0()
                .mt(px(top_pad))
                .text_size(px(size))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(dark::TEXT))
                .whitespace_normal()
                .child(joined)
                .into_any_element()
        }
        BlockKind::Paragraph => {
            let has_styled = block
                .spans
                .iter()
                .any(|s| s.bold || s.italic || s.strikethrough || s.code || s.link.is_some());
            if !has_styled {
                // Plain paragraph: a single text node gives reliable wrapping.
                let joined: String = block.spans.iter().map(|s| s.text.as_str()).collect();
                div()
                    .w_full()
                    .min_w_0()
                    .text_color(rgb(dark::TEXT))
                    .whitespace_normal()
                    .child(joined)
                    .into_any_element()
            } else {
                // Mixed-style paragraph: split into runs of consecutive spans
                // sharing the same flags. Each run becomes a single flex item
                // inside the wrapping row so a long plain run can wrap while
                // bold / italic / code / link chips stay atomic.
                render_styled_paragraph(block.spans)
            }
        }
        BlockKind::CodeBlock(lang) => {
            let joined: String = block
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>()
                .trim_end_matches('\n')
                .to_string();
            render_code_block(&joined, lang, None, false, None)
        }
        BlockKind::Table(header, rows) => {
            let mut table = div()
                .w_full()
                .min_w_0()
                .my_1()
                .rounded_md()
                .overflow_hidden()
                .border_1()
                .border_color(rgba(dark::BORDER))
                .flex()
                .flex_col();
            let col_count = header.len().max(1);
            // Header row.
            let mut header_row = div()
                .flex()
                .flex_row()
                .bg(rgba(dark::OVERLAY))
                .border_b_1()
                .border_color(rgba(dark::BORDER));
            for (ci, cell) in header.iter().enumerate() {
                let cell_html = render_table_cell(cell.clone());
                header_row = header_row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(9.0))
                        .py(px(6.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(dark::TEXT))
                        .child(cell_html),
                );
                if ci + 1 < col_count {
                    header_row =
                        header_row.child(div().w(px(1.0)).self_stretch().bg(rgba(dark::BORDER)));
                }
            }
            table = table.child(header_row);
            // Body rows.
            for (ri, row) in rows.iter().enumerate() {
                let mut row_div = div().flex().flex_row().when(ri + 1 < rows.len(), |d| {
                    d.border_b_1().border_color(rgba(dark::BORDER))
                });
                for (ci, cell) in row.iter().enumerate() {
                    let cell_html = render_table_cell(cell.clone());
                    row_div = row_div.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(9.0))
                            .py(px(6.0))
                            .text_size(px(12.5))
                            .line_height(px(19.0))
                            .child(cell_html),
                    );
                    if ci + 1 < col_count {
                        row_div =
                            row_div.child(div().w(px(1.0)).self_stretch().bg(rgba(dark::BORDER)));
                    }
                }
                table = table.child(row_div);
            }
            table.into_any_element()
        }
        BlockKind::ListItem(marker) => {
            let prefix = match marker {
                Some(n) => format!("{n}. "),
                None => "\u{2022} ".to_string(),
            };
            // A task checkbox appears as the first span with `task = Some(..)`;
            // render a small box instead of the bullet for it.
            let task = block.spans.iter().find_map(|s| s.task);
            let has_styled = block
                .spans
                .iter()
                .any(|s| s.bold || s.italic || s.strikethrough || s.code || s.link.is_some());
            let body_spans: Vec<MarkdownSpan> = block
                .spans
                .iter()
                .filter(|s| s.task.is_none())
                .cloned()
                .collect();
            let body: gpui::AnyElement = if has_styled {
                render_styled_paragraph(body_spans)
            } else {
                let joined: String = body_spans.iter().map(|s| s.text.as_str()).collect();
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(rgb(dark::TEXT))
                    .whitespace_normal()
                    .child(joined)
                    .into_any_element()
            };
            let marker_el: gpui::AnyElement = if let Some(checked) = task {
                div()
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_shrink_0()
                    .mt(px(2.0))
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(if checked {
                        rgb(dark::ACCENT)
                    } else {
                        rgba(dark::BORDER_STRONG)
                    })
                    .bg(if checked {
                        rgb(dark::ACCENT)
                    } else {
                        rgba(0x00000000)
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.0))
                    .text_color(if checked {
                        rgb(dark::INSET)
                    } else {
                        rgba(0x00000000)
                    })
                    .child(if checked { "✓" } else { "" })
                    .into_any_element()
            } else {
                div()
                    .text_color(rgb(dark::TEXT_TERTIARY))
                    .min_w(px(28.0))
                    .flex_shrink_0()
                    .child(prefix)
                    .into_any_element()
            };
            div()
                .flex()
                .flex_row()
                .gap_2()
                .w_full()
                .min_w_0()
                .child(marker_el)
                .child(body)
                .into_any_element()
        }
        BlockKind::BlockQuote => {
            let has_styled = block
                .spans
                .iter()
                .any(|s| s.bold || s.italic || s.strikethrough || s.code || s.link.is_some());
            let body: gpui::AnyElement = if has_styled {
                render_styled_paragraph(block.spans)
            } else {
                let joined: String = block.spans.iter().map(|s| s.text.as_str()).collect();
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(rgb(dark::TEXT_SECONDARY))
                    .whitespace_normal()
                    .child(joined)
                    .into_any_element()
            };
            div()
                .flex()
                .flex_row()
                .gap_2()
                .w_full()
                .min_w_0()
                .pl_3()
                .border_l_2()
                .border_color(rgb(dark::ACCENT))
                .text_color(rgb(dark::TEXT_SECONDARY))
                .child(body)
                .into_any_element()
        }
        BlockKind::Hr => div()
            .h(px(1.0))
            .w_full()
            .bg(rgb(dark::BORDER))
            .into_any_element(),
    }
}

/// Render a fenced code block: a 24px header bar (language tag + optional
/// copy button) over an inset card with the code body. `copy_key` is
/// `(msg_idx, block_idx)` when the caller wants a copy button; `copied` flips
/// the button to its "✓" feedback state.
fn render_code_block(
    joined: &str,
    lang: Option<CompactString>,
    copy_key: Option<(usize, usize)>,
    copied: bool,
    view_entity: Option<gpui::Entity<ShellState>>,
) -> gpui::AnyElement {
    let lang_label = lang.clone();
    let code_body: gpui::AnyElement = if let Some(lang) = lang
        && let Some(hl) = highlight::lang_for_tag(lang.as_str())
    {
        // Syntax-highlighted: tiled `TextRun`s on one mono font, so a
        // line measures identically with or without coloring (streaming
        // safe). Gaps keep the default foreground.
        let runs = code_runs(joined, hl, dark::TEXT_SECONDARY);
        StyledText::new(SharedString::from(joined.to_string()))
            .with_runs(runs)
            .into_any_element()
    } else {
        div()
            .w_full()
            .px(px(10.0))
            .py(px(8.0))
            .font_family("ui-monospace")
            .text_xs()
            .text_color(rgb(dark::TEXT_SECONDARY))
            .whitespace_normal()
            .child(joined.to_string())
            .into_any_element()
    };

    let copy_button: gpui::AnyElement =
        if let (Some((msg_idx, block_idx)), Some(view)) = (copy_key, view_entity) {
            let code = joined.to_string();
            let glyph = if copied { "✓" } else { "⧉" };
            let color = if copied {
                rgb(dark::SUCCESS)
            } else {
                rgb(dark::TEXT_GHOST)
            };
            div()
                .id(ElementId::Name(
                    format!("code-copy-{msg_idx}-{block_idx}").into(),
                ))
                .w(px(20.0))
                .h(px(20.0))
                .rounded(px(5.0))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .text_color(color)
                .cursor_pointer()
                .hover(|element| element.bg(rgba(dark::OVERLAY_STRONG)))
                .tooltip(crate::tooltip::Tooltip::text(if copied {
                    "Copied"
                } else {
                    "Copy code"
                }))
                .child(glyph)
                .on_click(move |_ev, _window, cx| {
                    view.update(cx, |state, cx| {
                        state.copied_code = Some(((msg_idx, block_idx), Instant::now()));
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(code.clone()));
                        cx.notify();
                    });
                })
                .into_any_element()
        } else {
            div().into_any_element()
        };

    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .my_1()
        .rounded_md()
        .overflow_hidden()
        .border_1()
        .border_color(rgba(dark::BORDER))
        .bg(rgb(dark::INSET))
        // Header bar: language tag on the left, copy button on the right.
        .child(
            div()
                .w_full()
                .h(px(24.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(rgba(dark::BORDER))
                .text_size(px(10.0))
                .line_height(px(14.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(rgb(dark::TEXT_GHOST))
                .child(
                    lang_label
                        .unwrap_or_else(|| CompactString::from(""))
                        .to_string(),
                )
                .child(copy_button),
        )
        .child(
            div()
                .w_full()
                .px(px(10.0))
                .py(px(8.0))
                .font_family("ui-monospace")
                .text_xs()
                .whitespace_normal()
                .child(code_body),
        )
        .into_any_element()
}

/// Render a single table cell's inline spans.
fn render_table_cell(spans: Vec<MarkdownSpan>) -> gpui::AnyElement {
    let has_styled = spans
        .iter()
        .any(|s| s.bold || s.italic || s.strikethrough || s.code || s.link.is_some());
    if has_styled {
        render_styled_paragraph(spans)
    } else {
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        div()
            .w_full()
            .min_w_0()
            .whitespace_normal()
            .child(joined)
            .into_any_element()
    }
}

/// Render an inline-styled paragraph by collapsing consecutive spans that
/// share the same flag bits into a single run. Plain runs become a single
/// `Div` with whitespace_normal so wrapping works; styled runs become a chip
/// using [`render_styled_span_run`] so bold / italic / code / link chips stay
/// atomic and never split a word in two.
fn render_styled_paragraph(spans: Vec<MarkdownSpan>) -> gpui::AnyElement {
    // Group consecutive spans that match in the "styling fingerprint" so
    // that bold+italic_*_code runs are merged into one chip. This avoids
    // the "five divs for a single bold sentence" fragmentation that keeps
    // individual flex items from shrinking nicely.
    let grouped = group_runs(spans);
    let children: Vec<gpui::AnyElement> = grouped
        .into_iter()
        .map(|(styled, text)| render_styled_span_run(styled, text))
        .collect();
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .w_full()
        .min_w_0()
        .text_color(rgb(dark::TEXT))
        .children(children)
        .into_any_element()
}

/// Coalesce consecutive spans whose (bold, italic, strike, code, link_url)
/// fingerprint matches into a single (flags, joined_text) pair.
fn group_runs(spans: Vec<MarkdownSpan>) -> Vec<(MarkdownFlags, String)> {
    let mut out: Vec<(MarkdownFlags, String)> = Vec::new();
    for span in spans {
        let flags = MarkdownFlags {
            bold: span.bold,
            italic: span.italic,
            strikethrough: span.strikethrough,
            code: span.code,
            link: span.link.clone(),
        };
        if let Some((existing_flags, text)) = out.last_mut()
            && existing_flags == &flags
        {
            text.push_str(span.text.as_str());
            continue;
        }
        out.push((flags, span.text.to_string()));
    }
    out
}

#[derive(Clone, PartialEq, Eq)]
struct MarkdownFlags {
    bold: bool,
    italic: bool,
    strikethrough: bool,
    code: bool,
    link: Option<CompactString>,
}

fn render_styled_span_run(flags: MarkdownFlags, text: String) -> gpui::AnyElement {
    // min_w_0 + flex_shrink lets a long, unbreakable run still shrink and
    // wrap inside the parent flex_row. Without these, the run's content
    // width imposes a floor that pushes the whole line past the bubble.
    let mut d = div()
        .text_color(rgb(dark::TEXT))
        .text_sm()
        .min_w_0()
        .flex_shrink(1.0)
        .whitespace_normal();
    if flags.bold {
        d = d.font_weight(gpui::FontWeight::BOLD);
    }
    if flags.italic {
        d = d.italic();
    }
    if flags.strikethrough {
        d = d.line_through();
    }
    if flags.code {
        d = d
            .font_family("ui-monospace")
            .text_xs()
            .px_1p5()
            .rounded_sm()
            .bg(rgba(dark::CODE_WASH))
            .text_color(rgb(dark::CODE_TEXT));
    }
    if flags.link.is_some() {
        d = d.text_color(rgb(dark::ACCENT)).underline();
    }
    d.child(text).into_any_element()
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
        .bg(rgb(dark::RAISED))
        .px_4()
        .py_3()
        .rounded(px(12.0))
        .border_1()
        .border_color(rgba(dark::BORDER_STRONG))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(dark::TEXT))
                        .child("permission requested"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(dark::TEXT_TERTIARY))
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
                        .rounded(px(7.))
                        .bg(rgb(dark::INVERSE))
                        .text_color(rgb(dark::ON_INVERSE))
                        .cursor_pointer()
                        .hover(|element| element.opacity(0.9))
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
                        .rounded(px(7.))
                        .border_1()
                        .border_color(rgba(dark::BORDER))
                        .bg(rgba(0x00000000))
                        .text_color(rgb(dark::TEXT))
                        .cursor_pointer()
                        .hover(|element| element.bg(rgba(dark::OVERLAY)))
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
///
/// Header (title bar + search input + action buttons) for the sidebar. Kept as
/// a free function so the closures inside can lazily capture all the click
/// entities we need without entangling the sidebar renderer's lifetime./// The `−` / `+` button used by [`ShellState::settings_number_row`].
fn settings_step_btn(
    glyph: &'static str,
    next: u64,
    view_entity: gpui::Entity<ShellState>,
    on_change: impl Fn(&mut ShellState, u64) + 'static,
) -> gpui::AnyElement {
    div()
        .id(ElementId::Name(format!("settings-step-{glyph}").into()))
        .w(px(22.0))
        .h(px(22.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .text_color(rgb(dark::TEXT_SECONDARY))
        .cursor_pointer()
        .hover(|element| element.bg(rgba(dark::OVERLAY)).text_color(rgb(dark::TEXT)))
        .child(glyph)
        .on_click(move |_ev, _window, cx| {
            view_entity.update(cx, |state, cx| {
                on_change(state, next);
                cx.notify();
            });
        })
        .into_any_element()
}

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
                                .rounded(px(6.0))
                                .bg(rgba(0x00000000))
                                .text_color(if is_refreshing {
                                    rgb(dark::ACCENT)
                                } else {
                                    rgb(dark::TEXT_SECONDARY)
                                })
                                .cursor_pointer()
                                .text_xs()
                                .child(if is_refreshing { "syncing…" } else { "↻" })
                                .tooltip(crate::tooltip::Tooltip::text("Refresh sessions"))
                                .hover(|this| this.bg(rgba(dark::OVERLAY)))
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
                                .rounded(px(6.0))
                                .bg(rgb(dark::INVERSE))
                                .text_color(rgb(dark::ON_INVERSE))
                                .cursor_pointer()
                                .text_xs()
                                .child("+ New")
                                .tooltip(crate::tooltip::Tooltip::text("New session"))
                                .hover(|this| this.opacity(0.9))
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
                .border_color(rgba(dark::BORDER))
                .rounded(px(7.0))
                .px_2()
                .py_1()
                .bg(rgb(dark::COMPOSER))
                .text_xs()
                .text_color(if placeholder {
                    rgb(dark::TEXT_GHOST)
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
                            } else if key.chars().count() == 1
                                && let Some(first) = key.chars().next()
                                && !first.is_control()
                            {
                                state.append_sidebar_filter_char(first);
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
            rgba(dark::OVERLAY_STRONG)
        } else {
            rgba(0x00000000)
        })
        .border_l_2()
        .border_color(if active {
            rgb(dark::ACCENT)
        } else {
            rgba(0x00000000)
        })
        .hover(|this| this.bg(rgba(dark::OVERLAY)))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(dark::TEXT_TERTIARY))
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
                            .text_color(rgb(dark::TEXT_TERTIARY))
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
                        .text_color(rgb(dark::TEXT_TERTIARY))
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
    let name_owned = session.name.clone();

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
        rgb(dark::TEXT_TERTIARY)
    };

    let view_for_delete = view_entity.clone();
    let id_for_delete = id_owned.clone();
    let view_for_rename = view_entity.clone();
    let id_for_rename = id_owned.clone();
    let name_for_rename = name_owned.clone();

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
            rgba(dark::OVERLAY_STRONG)
        } else {
            rgba(0x00000000)
        })
        .border_l_2()
        .border_color(if is_active {
            rgb(dark::ACCENT)
        } else {
            rgba(0x00000000)
        })
        .hover(|this| this.bg(rgba(dark::OVERLAY)))
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
                })
                // Hover actions: delete and rename for non-active rows
                .when(!is_active, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_0p5()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .id(ElementId::Name(
                                        format!("session-rename:{}", id_for_rename.as_str()).into(),
                                    ))
                                    .px_1()
                                    .rounded_sm()
                                    .text_xs()
                                    .text_color(rgb(dark::TEXT_GHOST))
                                    .cursor_pointer()
                                    .hover(|this| {
                                        this.bg(rgba(dark::OVERLAY)).text_color(rgb(dark::ACCENT))
                                    })
                                    .tooltip(crate::tooltip::Tooltip::text("Rename session"))
                                    .child("✎")
                                    .on_click({
                                        let view = view_for_rename.clone();
                                        let sid = id_for_rename.clone();
                                        let name_to_edit = name_for_rename.clone();
                                        move |_ev, _window, cx| {
                                            view.update(cx, |state, cx| {
                                                // Open a real rename dialog prefilled
                                                // with the current name.
                                                state.rename_target = Some((
                                                    sid.to_string(),
                                                    name_to_edit.to_string(),
                                                ));
                                                state.rename_buffer =
                                                    SharedString::new(name_to_edit.as_str());
                                                cx.notify();
                                            });
                                        }
                                    }),
                            )
                            .child(
                                div()
                                    .id(ElementId::Name(
                                        format!("session-delete:{}", id_for_delete.as_str()).into(),
                                    ))
                                    .px_1()
                                    .rounded_sm()
                                    .text_xs()
                                    .text_color(rgb(dark::TEXT_GHOST))
                                    .cursor_pointer()
                                    .hover(|this| {
                                        this.bg(rgba(dark::OVERLAY)).text_color(rgb(dark::ERROR))
                                    })
                                    .tooltip(crate::tooltip::Tooltip::text("Delete session"))
                                    .child("✕")
                                    .on_click({
                                        let view = view_for_delete.clone();
                                        let sid = id_for_delete.clone();
                                        move |_ev, _window, cx| {
                                            view.update(cx, |state, _cx| {
                                                let _ =
                                                    state.bridge.send(UserAction::DeleteSession {
                                                        session_id: sid.clone(),
                                                    });
                                            });
                                        }
                                    }),
                            ),
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
            (SharedString::new(a), SharedString::new(b))
        }
        None => (s.clone(), SharedString::new("")),
    }
}

/// Render the input lines (one row per `\n`-separated segment) with a
/// block caret on the row containing the cursor. Shift+Enter introduces
/// newlines so users can compose multi-line prompts; matching the TUI's
/// bracketed-paste preview. Each row is a single Text element with
/// `whitespace_normal` set so wrapping behaves predictably when a row
/// pushes past the box width.
fn render_input_text(
    before: SharedString,
    after: SharedString,
    is_empty: bool,
    cursor_visible: bool,
) -> gpui::AnyElement {
    if is_empty {
        // Empty input still needs the caret at position 0: render the
        // placeholder text with a blinking cursor block right after it.
        let cursor_block = div()
            .w(px(1.5))
            .h(px(18.))
            .my(px(2.))
            .bg(if cursor_visible {
                rgb(dark::ACCENT)
            } else {
                rgba(0x00000000)
            })
            .rounded_sm()
            .flex_shrink_0();
        return div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0()
            .child(
                div()
                    .text_color(rgb(dark::TEXT_GHOST))
                    .child(SharedString::new("Do anything…")),
            )
            .child(cursor_block)
            .into_any_element();
    }

    let before_str = before.to_string();
    let after_str = after.to_string();
    let cursor_block = div()
        .w(px(1.5))
        .h(px(18.))
        .my(px(2.))
        .bg(if cursor_visible {
            rgb(dark::ACCENT)
        } else {
            rgba(0x00000000)
        })
        .rounded_sm()
        .flex_shrink_0();

    // Split on `\n`. The cursor sits between `before` and `after`: the
    // "cursor line" is the last line of `before_str`, with the cursor block
    // before `after_str`'s first line.
    let before_lines: Vec<&str> = before_str.split('\n').collect();
    let after_lines: Vec<&str> = after_str.split('\n').collect();
    let before_count = before_lines.len();

    let mut rows: Vec<gpui::AnyElement> = Vec::new();

    // Lines fully above the cursor.
    for line in before_lines.iter().take(before_count.saturating_sub(1)) {
        rows.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_0()
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .text_color(rgb(dark::TEXT))
                        .text_sm()
                        .child(line.to_string()),
                )
                .into_any_element(),
        );
    }

    // Cursor line.
    let last_before = before_lines.last().copied().unwrap_or("").to_string();
    let first_after = after_lines.first().copied().unwrap_or("").to_string();
    rows.push(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0()
            .child(
                div()
                    .whitespace_normal()
                    .text_color(rgb(dark::TEXT))
                    .text_sm()
                    .child(last_before),
            )
            .child(cursor_block)
            .child(
                div()
                    .whitespace_normal()
                    .text_color(rgb(dark::TEXT))
                    .text_sm()
                    .child(first_after),
            )
            .into_any_element(),
    );

    // Lines fully below the cursor (skip the first since it's been merged
    // into the cursor row above).
    for line in after_lines.iter().skip(1) {
        rows.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_0()
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .text_color(rgb(dark::TEXT))
                        .text_sm()
                        .child(line.to_string()),
                )
                .into_any_element(),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap_0()
        .children(rows)
        .into_any_element()
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
            cx.update(|cx| {
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
            cx.update(|cx| {
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
            .overflow_x_hidden()
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .flex()
                    .flex_col()
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                        // The chat column wrapper is the common parent of both
                        // the chat scroll area and the input box, so events
                        // bubbling from either child reach this listener. The
                        // input's own listener handles most printable keys and
                        // calls `stop_propagation` only for keys it owns; the
                        // few that fall through (PageUp/Down, Shift+Up/Down,
                        // Cmd/Ctrl+Home/End) are exactly the scroll-navigation
                        // gestures we want to wire up here. Putting the handler
                        // here also avoids fighting with the sidebar's own
                        // listener — sidebar focus doesn't bubble into this
                        // column, which is the behavior we want.
                        let key = ev.keystroke.key.as_str();
                        let mods = &ev.keystroke.modifiers;
                        let viewport_h = this
                            .chat_list
                            .viewport_bounds()
                            .size
                            .height
                            .as_f32()
                            .max(1.0);
                        let bump = viewport_h * 0.9;
                        let page_h = viewport_h - 32.0;
                        let handled = match key {
                            "pageup" => {
                                this.scroll_chat_by(-page_h);
                                true
                            }
                            "pagedown" => {
                                this.scroll_chat_by(page_h);
                                true
                            }
                            "up" if mods.shift => {
                                this.scroll_chat_by(-bump);
                                true
                            }
                            "down" if mods.shift => {
                                this.scroll_chat_by(bump);
                                true
                            }
                            "home" if mods.platform || mods.control => {
                                this.chat_list.scroll_to(ListOffset {
                                    item_ix: 0,
                                    offset_in_item: px(0.0),
                                });
                                this.chat_follow_tail = false;
                                true
                            }
                            "end" if mods.platform || mods.control => {
                                this.chat_list.scroll_to_end();
                                this.chat_follow_tail = true;
                                true
                            }
                            _ => false,
                        };
                        if handled {
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .child(self.render_chat(cx))
                    .child(self.render_input(cx, window)),
            )
            // Close-confirmation modal. Even though it's a flex sibling
            // here, `.absolute() + inset_0()` takes it out of normal flow
            // and stretches it across the root (which is the only
            // positioned ancestor). Inserted as the last child so it sits
            // visually on top of the chat and sidebar when shown.
            .when(self.close_confirm_visible, |d| {
                d.child(self.render_close_confirm_overlay(cx))
            })
            .when(self.rename_target.is_some(), |d| {
                d.child(self.render_rename_dialog(cx))
            })
    }
}

/// Program entry: build the engine on a background thread, drain its events into the
/// root view from a 30Hz tick, quit cleanly when the last window closes.
pub fn run() {
    let (model, provider) = resolve_provider_model();
    run_inner(
        &model,
        &provider,
        zerostack_core::permission::SecurityMode::Yolo,
    );
}

/// Program entry with explicit model / provider / security-mode overrides.
/// Called from the main zerostack binary when `--gui` is passed alongside
/// `--model`, `--provider`, `--yolo`, `--restrictive`, etc.
pub fn run_with_args(model: &str, provider: &str, mode: zerostack_core::permission::SecurityMode) {
    run_inner(model, provider, mode);
}

fn run_inner(model: &str, provider: &str, mode: zerostack_core::permission::SecurityMode) {
    // Initialise the tracing subscriber so the engine thread's logs land on
    // stderr. The GUI owns stdout (it's the drawing surface on macOS), so
    // investigating "why didn't my extension load?" requires
    // `RUST_LOG=info zerostack-gui 2>&1`. The bridge runs in its own OS
    // thread, but the tracing subscriber writes to the *process* stderr
    // (a global writer), so we register it before launching anything else.
    crate::tracing_init::init();
    eprintln!(
        "zerostack-gui starting up; run with RUST_LOG=info to surface \
         extension discovery logs on stderr"
    );
    tracing::info!("zerostack-gui starting up");

    let bridge = GuiBridge::launch(model, provider, mode);

    // Initialise the Wasm extension registry before any UI state is built.
    // We pass an empty path list so `init_from_paths` only picks up
    // auto-discovered extensions from the search directories — namely:
    //  - `ZS_EXTENSIONS_DIR` if set (with `~` expanded to `$HOME`);
    //  - `dirs::data_dir() / zerostack / extensions` (`~/Library/Application
    //    Support/zerostack/extensions/` on macOS);
    //  - `dirs::config_dir() / zerostack / extensions` (the same path on
    //    macOS since `dirs` aliases the two, but distinct on Linux XDG);
    //  - `$HOME/.config/zerostack/extensions/` directly — necessary on
    //    macOS, where users sometimes create this themselves, since the
    //    `dirs` crate won't surface it via `data_dir` / `config_dir`;
    //  - `<cwd>/.zerostack/extensions/`.
    //
    // Users can also point at a single catch-all directory with
    // `ZS_EXTENSIONS_DIR=~/my-exts zerostack-gui`; that env var prepends
    // to the scan list and `~` is expanded to `$HOME`.
    //
    // Errors are logged but not fatal: a broken extension in the user's
    // home dir shouldn't keep the GUI from booting. The Wasm host's own
    // `load_all()` collects per-path problems into `manager.errors()`
    // and emits a `tracing::warn!` per entry, so by the time control
    // returns we either have a coherent what-loaded-and-what-didn't
    // state, or the failing path is in the log.
    //
    // We don't expose `--extension` CLI flags for the GUI yet; mirroring
    // the TUI would require adding a clap layer in
    // `crates/gui/src/main.rs`, which is more surface area than this slim
    // mapper warrants. The TUI's matching call site lives in
    // `src/startup.rs::initialise_extensions`.
    #[cfg(feature = "extensions")]
    match zerostack_core::extension::registry::init_from_paths(&[] as &[std::path::PathBuf]) {
        Err(e) => {
            eprintln!("[extensions] init failed: {e}");
            tracing::warn!(error = %e, "extension registry init failed; picker will still show engine commands");
        }
        Ok(()) => {
            let names = zerostack_core::extension::registry::extension_command_names();
            // Belt-and-suspenders: `tracing::info!` covers the case where
            // the subscriber writes through `tracing-subscriber`'s env
            // filter, but real users often run the GUI as an `.app` and
            // miss env-filter wiring. We unconditionally mirror to stderr
            // so a "did anything load?" answer is visible even on macOS
            // where stdout is owned by the drawing surface.
            eprintln!(
                "[extensions] discovered {} command(s): {:?}",
                names.len(),
                names
            );
            if !names.is_empty() {
                tracing::info!(
                    count = names.len(),
                    ?names,
                    "extensions loaded into GUI slash picker"
                );
            }
        }
    }

    application().run(move |cx: &mut App| {
        let bridge_for_state = bridge;
        let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);

        let view = cx.new(move |cx| ShellState::new(bridge_for_state, cx));
        start_poll_loop(view.clone(), cx);
        spawn_cursor_blink(view.clone(), cx);

        cx.on_window_closed(|cx, _window_id| {
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
            move |window, cx| {
                // Intercept the OS close-button tap (and equivalents: "Quit
                // Zerostack" from the system menu, Cmd+Q on platforms that
                // forward it as a window close). The returned bool decides
                // whether the window actually destroys: false keeps it
                // alive and shows the modal; true lets the close through.
                let view_for_close = view.clone();
                window.on_window_should_close(cx, move |window, cx| {
                    // `Window::on_window_should_close` runs inside a deadline-
                    // sensitive platform hook, so we don't want to panic or
                    // hold a borrow across app work. `Entity::update` has two
                    // overloads with different generic orders — one returns
                    // `R` directly, the other returns `Result<R>` — so
                    // pinning the path via turbofish is brittle. Instead we
                    // ferry the answer out through a captured local, which
                    // works regardless of which overload Rust picks: a
                    // released entity leaves us with the `true` default,
                    // which equals "let the close go through".
                    let mut allow_close = true;
                    view_for_close.update(cx, |state, cx| {
                        allow_close = state.handle_window_should_close(window, cx);
                    });
                    allow_close
                });
                view.clone()
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

/// Resolve model and provider from config, matching the TUI's defaults.
pub fn resolve_provider_model() -> (String, String) {
    let (cfg, _is_first) = zerostack_core::config::load();
    let cli = zerostack_core::cli::Cli::default();
    (
        cli.resolve_model(&cfg).to_string(),
        cli.resolve_provider(&cfg).to_string(),
    )
}
