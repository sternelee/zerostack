# zerostack GUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract zerostack's core logic into `crates/core` and build a Makepad GUI frontend in `crates/gui`, with full feature parity to the existing crossterm TUI.

**Architecture:** Channel-based event architecture. `CoreEngine` runs on a tokio background thread, communicates with frontends via `unbounded_channel`. `crates/gui` uses `GuiBridge` to connect Makepad's event loop to the core engine. Existing `src/` TUI stays in place and will be updated to use `crates/core` imports.

**Tech Stack:** Rust 2024, Makepad (git dependency), tokio, rig 0.39, crossterm 0.29, serde, compact_str

## Global Constraints

- `cargo test` must pass after every task
- `cargo fmt` after every code change
- Feature flags (`mcp`, `subagents`, `extensions`, `memory`, `loop`, `git-worktree`, `archmd`, `advisor`, `multimodal`, `multithread`, `acp`, `status-signals`, `pdf`) must be preserved and functional
- Existing TUI must remain fully operational throughout
- `cargo install --path . --debug` must succeed after Phase 1
- `deny(unsafe_code)` must be preserved in all crates

---

## File Structure

```
crates/core/src/
├── lib.rs               # pub mod declarations, re-exports
├── events.rs            # NEW: CoreEvent, UserAction, TokenUsage, SessionInfo, InitialState
├── engine.rs            # NEW: CoreEngine struct
├── agent/               # MOVED from src/agent/
├── config/              # MOVED from src/config/
├── session/             # MOVED from src/session/
├── permission/          # MOVED from src/permission/
├── provider.rs          # MOVED from src/provider.rs
├── extension/           # MOVED from src/extension/
├── extras/              # MOVED from src/extras/
├── fs.rs                # MOVED from src/fs.rs
├── auth.rs              # MOVED from src/auth.rs
├── event.rs             # MOVED from src/event.rs (existing AgentEvent etc.)
├── logging.rs           # MOVED from src/logging.rs
├── models_catalog.rs    # MOVED from src/models_catalog.rs
├── pricing.rs           # MOVED from src/pricing.rs
├── retry.rs             # MOVED from src/retry.rs
├── sandbox.rs           # MOVED from src/sandbox.rs
└── docs.rs              # MOVED from src/docs.rs

crates/gui/src/
├── lib.rs               # pub mod declarations
├── app.rs               # MakepadApp, live_design! macro
├── bridge.rs            # GuiBridge: tokio ↔ Makepad
├── theme.rs             # Color constants, theme definitions
├── views/
│   ├── mod.rs
│   ├── main_view.rs     # MainView layout (sidebar + chat + input)
│   ├── sidebar.rs       # SessionList, NewSessionButton
│   ├── chat_view.rs     # MessageList, message rendering
│   └── input_bar.rs     # TextInput, SendButton
└── components/
    ├── mod.rs
    ├── message_bubble.rs  # UserMessage, AssistantMessage
    ├── tool_card.rs       # ToolCallCard, ToolResultCard
    └── code_block.rs      # Syntax-highlighted code block

src/
├── main.rs              # UPDATED: imports from zerostack_core, dispatches TUI/GUI
├── cli.rs               # UPDATED: --gui flag added
├── ui/                  # UPDATED: imports from zerostack_core
├── agent/               # REMOVED (moved to core)
├── config/              # REMOVED (moved to core)
├── session/             # REMOVED (moved to core)
├── permission/          # REMOVED (moved to core)
├── provider.rs          # REMOVED (moved to core)
├── extension/           # REMOVED (moved to core)
├── extras/              # REMOVED (moved to core)
├── fs.rs                # REMOVED (moved to core)
├── auth.rs              # REMOVED (moved to core)
├── event.rs             # REMOVED (moved to core)
├── logging.rs           # REMOVED (moved to core)
├── models_catalog.rs    # REMOVED (moved to core)
├── pricing.rs           # REMOVED (moved to core)
├── retry.rs             # REMOVED (moved to core)
├── sandbox.rs           # REMOVED (moved to core)
├── docs.rs              # REMOVED (moved to core)
└── tests/               # STAYS (test-only, references core crate)
```

---

## Phase 1: Core Engine Extraction

### Task 1: Create `crates/core` skeleton with event types

**Files:**
- Create: `crates/core/Cargo.toml`
- Create: `crates/core/src/lib.rs`
- Create: `crates/core/src/events.rs`
- Modify: `Cargo.toml` (workspace root)

**Interfaces:**
- Produces: `zerostack_core` crate with `events` module containing `CoreEvent`, `UserAction`, `TokenUsage`, `SessionInfo`, `InitialState`

- [ ] **Step 1: Create workspace member directory**

```bash
mkdir -p crates/core/src
```

- [ ] **Step 2: Write `crates/core/Cargo.toml`**

```toml
[package]
name = "zerostack-core"
version.workspace = true
edition.workspace = true
license.workspace = true
homepage.workspace = true
repository.workspace = true
description = "Core engine for zerostack — agent, tools, sessions, config, permissions"

[features]
default = ['loop', 'git-worktree', 'mcp', 'subagents', 'archmd', 'status-signals', 'multithread', 'extensions']
status-signals = []
loop = []
git-worktree = []
mcp = [
    "dep:rmcp",
    "rmcp?/client",
    "rmcp?/transport-child-process",
    "rmcp?/transport-streamable-http-client-reqwest",
    "rmcp?/auth",
]
acp = ["dep:agent-client-protocol", "dep:blocking"]
memory = []
subagents = []
archmd = []
multithread = ["tokio/rt-multi-thread"]
multimodal = ["rig/image"]
pdf = ["multimodal", "rig/pdf"]
advisor = []
extensions = ["dep:wasmtime", "dep:wasmtime-wasi", "zerostack-extension-api"]

[dependencies]
rig = { version = "0.39", features = ["rmcp"] }
rmcp = { version = "2.0", optional = true, default-features = false, features = [
    "client",
    "transport-child-process",
    "transport-streamable-http-client-reqwest",
] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml_ng = "0.10"
toml = "1.1"
tokio = { version = "1", features = [
    "rt",
    "macros",
    "sync",
    "time",
    "process",
    "fs",
] }
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
thiserror = "2"
futures = "0.3"
reqwest = "0.13"
dirs = "6"
compact_str = { version = "0.9", features = ["serde"] }
smallvec = "1"
regex = "1"
unicode-width = "0.2"
ignore = "0.4"
pulldown-cmark = "0.13"
include_dir = "0.7"
http = "1"
agent-client-protocol = { version = "1.0.1", optional = true }
blocking = { version = "1", optional = true }
mimalloc = { version = "0.1", default-features = false }
wasmtime = { version = "46", optional = true, default-features = false, features = ["runtime", "cranelift", "component-model"] }
wasmtime-wasi = { version = "46", optional = true, default-features = false, features = ["p2"] }
zerostack-extension-api = { path = "../extension-api", optional = true }
```

- [ ] **Step 3: Write `crates/core/src/events.rs`**

```rust
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
```

- [ ] **Step 4: Write `crates/core/src/lib.rs`**

```rust
#![deny(unsafe_code)]

pub mod events;
```

- [ ] **Step 5: Add `crates/core` to workspace `Cargo.toml`**

Edit root `Cargo.toml`, add `"crates/core"` to the workspace members:

```toml
members = [".", "crates/extension-api", "crates/core", "tests/extensions/test-echo", "tests/extensions/pi-simplify"]
```

- [ ] **Step 6: Verify compilation**

```bash
cargo test -p zerostack-core
```

Expected: compiles and tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/core/ Cargo.toml
git commit -m "feat(core): add crates/core skeleton with event types"
```
---

### Task 2: Move `permission` module to core

**Files:**
- Move: `src/permission/` → `crates/core/src/permission/`
- Modify: `crates/core/src/lib.rs`

**Interfaces:**
- Produces: `zerostack_core::permission` module (same public API as before)

- [ ] **Step 1: Move the directory**

```bash
git mv src/permission crates/core/src/permission
```

- [ ] **Step 2: Update `crates/core/src/lib.rs`**

```rust
#![deny(unsafe_code)]

pub mod events;
pub mod permission;
```

- [ ] **Step 3: Verify core compiles**

```bash
cargo test -p zerostack-core
```

- [ ] **Step 4: Commit**

```bash
git add crates/core/ src/permission/
git commit -m "refactor(core): move permission module to crates/core"
```

---

### Task 3: Move `session` module to core

**Files:**
- Move: `src/session/` → `crates/core/src/session/`
- Modify: `crates/core/src/lib.rs`

- [ ] **Step 1: Move the directory**

```bash
git mv src/session crates/core/src/session
```

- [ ] **Step 2: Update `crates/core/src/lib.rs`**

```rust
pub mod session;
```

- [ ] **Step 3: Verify core compiles**

```bash
cargo test -p zerostack-core
```

- [ ] **Step 4: Commit**

```bash
git add crates/core/ src/session/
git commit -m "refactor(core): move session module to crates/core"
```

---

### Task 4: Move `config` module to core

**Files:**
- Move: `src/config/` → `crates/core/src/config/`
- Modify: `crates/core/src/lib.rs`

- [ ] **Step 1: Move the directory**

```bash
git mv src/config crates/core/src/config
```

- [ ] **Step 2: Update `crates/core/src/lib.rs`**

```rust
pub mod config;
```

- [ ] **Step 3: Fix any `crate::` imports in config files**

`crate::permission` references should now resolve within `zerostack_core`. Check `crates/core/src/config/mod.rs` and `crates/core/src/config/load.rs`.

- [ ] **Step 4: Verify core compiles**

```bash
cargo test -p zerostack-core
```

- [ ] **Step 5: Commit**

```bash
git add crates/core/ src/config/
git commit -m "refactor(core): move config module to crates/core"
```

---

### Task 5: Move utility modules to core (fs, auth, event, logging, etc.)

**Files:**
- Move: `src/fs.rs` → `crates/core/src/fs.rs`
- Move: `src/auth.rs` → `crates/core/src/auth.rs`
- Move: `src/event.rs` → `crates/core/src/event.rs`
- Move: `src/logging.rs` → `crates/core/src/logging.rs`
- Move: `src/models_catalog.rs` → `crates/core/src/models_catalog.rs`
- Move: `src/pricing.rs` → `crates/core/src/pricing.rs`
- Move: `src/retry.rs` → `crates/core/src/retry.rs`
- Move: `src/sandbox.rs` → `crates/core/src/sandbox.rs`
- Move: `src/docs.rs` → `crates/core/src/docs.rs`
- Modify: `crates/core/src/lib.rs`

- [ ] **Step 1: Move all utility files**

```bash
for f in fs auth event logging models_catalog pricing retry sandbox docs; do
  git mv "src/${f}.rs" "crates/core/src/${f}.rs"
done
```

- [ ] **Step 2: Update `crates/core/src/lib.rs`**

```rust
pub mod fs;
pub mod auth;
pub mod event;
pub mod logging;
pub mod models_catalog;
pub mod pricing;
pub mod retry;
pub mod sandbox;
pub mod docs;
```

- [ ] **Step 3: Verify core compiles**

```bash
cargo test -p zerostack-core 2>&1 | tail -30
```

- [ ] **Step 4: Commit**

```bash
git add crates/core/ src/fs.rs src/auth.rs src/event.rs src/logging.rs src/models_catalog.rs src/pricing.rs src/retry.rs src/sandbox.rs src/docs.rs
git commit -m "refactor(core): move utility modules to crates/core"
```

---

### Task 6: Move `agent` module to core

**Files:**
- Move: `src/agent/` → `crates/core/src/agent/`
- Modify: `crates/core/src/lib.rs`
- Note: agent depends on provider, extras, extension — move those first if needed, or add stubs.

- [ ] **Step 1: Move the directory**

```bash
git mv src/agent crates/core/src/agent
```

- [ ] **Step 2: Update `crates/core/src/lib.rs`**

```rust
pub mod agent;
```

- [ ] **Step 3: Verify core compiles**

```bash
cargo test -p zerostack-core 2>&1 | head -50
```

Agent may reference `crate::provider`, `crate::extras`, `crate::extension`. If compilation fails, add temporary stubs in lib.rs:

```rust
// Temporary stubs for modules not yet moved
pub mod provider {
    // stub — will be moved in Task 7
}
pub mod extras {
    // stub — will be moved in Task 8
}
#[cfg(feature = "extensions")]
pub mod extension {
    // stub — will be moved in Task 8
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/core/ src/agent/
git commit -m "refactor(core): move agent module to crates/core"
```

---

### Task 7: Move `provider` to core

**Files:**
- Move: `src/provider.rs` → `crates/core/src/provider.rs`
- Modify: `crates/core/src/lib.rs`
- Remove temporary stubs if added in Task 6

- [ ] **Step 1: Move the file**

```bash
git mv src/provider.rs crates/core/src/provider.rs
```

- [ ] **Step 2: Update lib.rs**

Replace the stub with `pub mod provider;`.

- [ ] **Step 3: Verify core compiles**

```bash
cargo test -p zerostack-core 2>&1 | tail -30
```

provider.rs is ~39KB. Fix any import errors.

- [ ] **Step 4: Commit**

```bash
git add crates/core/ src/provider.rs
git commit -m "refactor(core): move provider module to crates/core"
```

---

### Task 8: Move `extension` and `extras` to core

**Files:**
- Move: `src/extension/` → `crates/core/src/extension/`
- Move: `src/extras/` → `crates/core/src/extras/`
- Modify: `crates/core/src/lib.rs`
- Remove temporary stubs.

- [ ] **Step 1: Move directories**

```bash
git mv src/extension crates/core/src/extension
git mv src/extras crates/core/src/extras
```

- [ ] **Step 2: Update lib.rs**

```rust
#[cfg(feature = "extensions")]
pub mod extension;
pub mod extras;
```

- [ ] **Step 3: Verify core compiles**

```bash
cargo test -p zerostack-core 2>&1 | tail -30
```

- [ ] **Step 4: Commit**

```bash
git add crates/core/ src/extension/ src/extras/
git commit -m "refactor(core): move extension and extras modules to crates/core"
```

---

### Task 9: Verify core crate compiles fully with all features

**Files:**
- Modify: `crates/core/Cargo.toml` (fix any missing deps)
- Modify: `crates/core/src/lib.rs` (finalize module declarations)

- [ ] **Step 1: Finalize `crates/core/src/lib.rs`**

```rust
#![deny(unsafe_code)]

pub mod agent;
pub mod auth;
pub mod config;
pub mod docs;
pub mod engine;
pub mod event;
pub mod events;
pub mod fs;
pub mod logging;
pub mod models_catalog;
pub mod permission;
pub mod pricing;
pub mod provider;
pub mod retry;
pub mod sandbox;
pub mod session;

#[cfg(feature = "extensions")]
pub mod extension;
pub mod extras;
```

- [ ] **Step 2: Run full test suite**

```bash
cargo test -p zerostack-core --all-features 2>&1 | tail -30
```

- [ ] **Step 3: Fix any remaining import errors**

Common issues: `crate::` references should now resolve within `zerostack_core`. Check for any `crate::ui::` references that should not have been moved (they should stay in `src/`).

- [ ] **Step 4: Run tests again**

```bash
cargo test -p zerostack-core --all-features
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/core/
git commit -m "fix(core): finalize module declarations and fix imports"
```

---

### Task 10: Create CoreEngine

**Files:**
- Create: `crates/core/src/engine.rs`
- Modify: `crates/core/src/lib.rs`

**Interfaces:**
- Produces: `CoreEngine` struct with `new()`, `handle_action()`, `initial_state()`, `current_session()`, `current_session_mut()`

- [ ] **Step 1: Write `crates/core/src/engine.rs`**

```rust
use compact_str::CompactString;

use crate::config::Config;
use crate::events::{CoreEvent, InitialState, SessionInfo, UserAction};
use crate::permission::SecurityMode;
use crate::session::Session;

/// The core engine manages all zerostack logic independently of any UI framework.
/// It communicates with frontends (TUI/GUI) via channels carrying CoreEvent and UserAction.
pub struct CoreEngine {
    config: Config,
    sessions: Vec<Session>,
    current_session_index: Option<usize>,
    model: CompactString,
    provider: CompactString,
    mode: SecurityMode,
    permission_request_id: u64,
}

impl CoreEngine {
    /// Create a new CoreEngine with the given configuration.
    pub fn new(
        config: Config,
        model: CompactString,
        provider: CompactString,
        mode: SecurityMode,
    ) -> Self {
        Self {
            config,
            sessions: Vec::new(),
            current_session_index: None,
            model,
            provider,
            mode,
            permission_request_id: 0,
        }
    }

    /// Get the initial state to send to the frontend on startup.
    pub fn initial_state(&self) -> InitialState {
        let sessions: Vec<SessionInfo> = self
            .sessions
            .iter()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                name: CompactString::from(s.name()),
                model: s.model.clone(),
                message_count: s.messages().len(),
                created_at: CompactString::from(s.created_at()),
            })
            .collect();

        let current_session_id = self
            .current_session_index
            .map(|i| self.sessions[i].id.clone());

        InitialState {
            sessions,
            current_session_id,
            model: self.model.clone(),
            provider: self.provider.clone(),
            mode: self.mode.to_string(),
        }
    }

    /// Get the current session, if any.
    pub fn current_session(&self) -> Option<&Session> {
        self.current_session_index.map(|i| &self.sessions[i])
    }

    /// Get the current session mutably.
    pub fn current_session_mut(&mut self) -> Option<&mut Session> {
        self.current_session_index.map(|i| &mut self.sessions[i])
    }

    /// Process a user action and return any events that should be sent to the frontend.
    pub async fn handle_action(&mut self, action: UserAction) -> Vec<CoreEvent> {
        match action {
            UserAction::SendMessage { text } => {
                // Phase 1: placeholder — agent runner will be wired in a follow-up task
                vec![CoreEvent::Error {
                    message: CompactString::from("Agent runner not yet wired to CoreEngine"),
                }]
            }
            UserAction::CreateSession { name } => {
                let session_name =
                    name.unwrap_or_else(|| CompactString::from("New Session"));
                let mut session = Session::new(
                    session_name,
                    self.model.clone(),
                    self.provider.clone(),
                    self.config.context_window(),
                );
                session.initialize();
                self.sessions.push(session);
                self.current_session_index = Some(self.sessions.len() - 1);
                self.emit_session_list_updated()
            }
            UserAction::SwitchSession { session_id } => {
                if let Some(idx) = self.sessions.iter().position(|s| s.id == session_id) {
                    self.current_session_index = Some(idx);
                    vec![CoreEvent::SessionChanged { session_id }]
                } else {
                    vec![CoreEvent::Error {
                        message: CompactString::from("Session not found"),
                    }]
                }
            }
            UserAction::DeleteSession { session_id } => {
                if let Some(idx) = self.sessions.iter().position(|s| s.id == session_id) {
                    self.sessions.remove(idx);
                    if self.current_session_index == Some(idx) {
                        self.current_session_index = if self.sessions.is_empty() {
                            None
                        } else if idx >= self.sessions.len() {
                            Some(self.sessions.len() - 1)
                        } else {
                            Some(idx)
                        };
                    } else if let Some(ref mut cur) = self.current_session_index {
                        if *cur > idx {
                            *cur -= 1;
                        }
                    }
                }
                self.emit_session_list_updated()
            }
            UserAction::RenameSession { session_id, name } => {
                if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    session.rename(&name);
                }
                self.emit_session_list_updated()
            }
            UserAction::Quit => {
                vec![CoreEvent::Error {
                    message: CompactString::from("quit"),
                }]
            }
            _ => {
                vec![CoreEvent::Error {
                    message: CompactString::from("Not yet implemented"),
                }]
            }
        }
    }

    fn emit_session_list_updated(&self) -> Vec<CoreEvent> {
        let sessions: Vec<SessionInfo> = self
            .sessions
            .iter()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                name: CompactString::from(s.name()),
                model: s.model.clone(),
                message_count: s.messages().len(),
                created_at: CompactString::from(s.created_at()),
            })
            .collect();
        vec![CoreEvent::SessionListUpdated { sessions }]
    }
}
```

Note: The CoreEngine references `Session::new()`, `Session::name()`, `Session::messages()`, `Session::created_at()`, `Session::rename()`, `Session::initialize()`, `Config::context_window()`. If these methods don't exist with these exact signatures, add accessor methods to `Session` and `Config` as needed. Check the actual field names in `session/mod.rs` — fields like `id`, `model`, `provider`, `messages`, `created_at`, `name` are used.

- [ ] **Step 2: Verify core compiles**

```bash
cargo test -p zerostack-core 2>&1 | tail -30
```

If `Session` methods like `name()`, `messages()`, `created_at()`, `rename()`, `initialize()`, or `Config::context_window()` don't exist, add them as public methods on the respective types.

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/engine.rs
git commit -m "feat(core): add CoreEngine with session management"
```

---

### Task 11: Update `src/` to use `zerostack_core` imports

**Files:**
- Modify: `Cargo.toml` (root — add zerostack-core dependency)
- Modify: `src/main.rs` (update imports)
- Modify: `src/cli.rs` (add `--gui` flag)
- Modify: `src/ui/mod.rs` (update imports)
- Modify: `src/ui/event_handler.rs` (update imports)
- Modify: `src/ui/renderer.rs` (update imports)
- Modify: all files under `src/ui/slash/` and `src/ui/pickers/` (update imports)
- Modify: `src/context/mod.rs` (update imports if needed)

- [ ] **Step 1: Add `zerostack-core` dependency to root `Cargo.toml`**

```toml
[dependencies]
zerostack-core = { path = "crates/core" }
# Keep all existing dependencies
```

- [ ] **Step 2: Add `--gui` flag to `src/cli.rs`**

Find the `Cli` struct (clap derive) and add:

```rust
/// Launch the Makepad GUI instead of the terminal TUI
#[arg(long = "gui", default_value_t = false)]
pub gui: bool,
```

- [ ] **Step 3: Update `src/main.rs` — replace `crate::` imports with `zerostack_core::`**

Replace all `use crate::` imports that reference moved modules. Key replacements:

```
crate::config → zerostack_core::config
crate::permission → zerostack_core::permission
crate::session → zerostack_core::session
crate::agent → zerostack_core::agent
crate::provider → zerostack_core::provider
crate::event → zerostack_core::event
crate::fs → zerostack_core::fs
crate::auth → zerostack_core::auth
crate::logging → zerostack_core::logging
crate::extras → zerostack_core::extras
crate::models_catalog → zerostack_core::models_catalog
crate::pricing → zerostack_core::pricing
crate::retry → zerostack_core::retry
crate::sandbox → zerostack_core::sandbox
crate::docs → zerostack_core::docs
crate::extension → zerostack_core::extension
```

Keep `crate::ui::` references as-is (UI module stays in `src/`).

- [ ] **Step 4: Update `src/ui/mod.rs` and all UI files**

Same replacement pattern for all files under `src/ui/`. Use sed:

```bash
# Replace crate:: imports with zerostack_core:: for moved modules
# Do NOT replace crate::ui:: references
for f in src/ui/mod.rs src/ui/event_handler.rs src/ui/renderer.rs src/ui/statusline.rs src/ui/slash/*.rs src/ui/pickers/*.rs src/context/mod.rs; do
  sed -i '' 's/use crate::config::/use zerostack_core::config::/g' "$f"
  sed -i '' 's/use crate::permission::/use zerostack_core::permission::/g' "$f"
  sed -i '' 's/use crate::session::/use zerostack_core::session::/g' "$f"
  sed -i '' 's/use crate::agent::/use zerostack_core::agent::/g' "$f"
  sed -i '' 's/use crate::provider::/use zerostack_core::provider::/g' "$f"
  sed -i '' 's/use crate::event::/use zerostack_core::event::/g' "$f"
  sed -i '' 's/use crate::fs::/use zerostack_core::fs::/g' "$f"
  sed -i '' 's/use crate::auth::/use zerostack_core::auth::/g' "$f"
  sed -i '' 's/use crate::logging::/use zerostack_core::logging::/g' "$f"
  sed -i '' 's/use crate::extras::/use zerostack_core::extras::/g' "$f"
  sed -i '' 's/use crate::models_catalog::/use zerostack_core::models_catalog::/g' "$f"
  sed -i '' 's/use crate::pricing::/use zerostack_core::pricing::/g' "$f"
  sed -i '' 's/use crate::retry::/use zerostack_core::retry::/g' "$f"
  sed -i '' 's/use crate::sandbox::/use zerostack_core::sandbox::/g' "$f"
  sed -i '' 's/use crate::docs::/use zerostack_core::docs::/g' "$f"
  sed -i '' 's/use crate::extension::/use zerostack_core::extension::/g' "$f"
done
```

Also handle `crate::` references in inline code (not just imports):
```bash
for f in src/ui/mod.rs src/ui/event_handler.rs src/ui/renderer.rs; do
  sed -i '' 's/crate::config::/zerostack_core::config::/g' "$f"
  sed -i '' 's/crate::permission::/zerostack_core::permission::/g' "$f"
  sed -i '' 's/crate::session::/zerostack_core::session::/g' "$f"
  sed -i '' 's/crate::agent::/zerostack_core::agent::/g' "$f"
  sed -i '' 's/crate::provider::/zerostack_core::provider::/g' "$f"
  sed -i '' 's/crate::event::/zerostack_core::event::/g' "$f"
  sed -i '' 's/crate::fs::/zerostack_core::fs::/g' "$f"
  sed -i '' 's/crate::extras::/zerostack_core::extras::/g' "$f"
  sed -i '' 's/crate::extension::/zerostack_core::extension::/g' "$f"
done
```

- [ ] **Step 5: Update `src/main.rs` — handle `crate::` references in inline code**

After the sed replacements, manually review `src/main.rs` for remaining `crate::` references. The `crate::cli::` and `crate::ui::` references should stay. All other `crate::module::` references should be replaced with `zerostack_core::module::`.

- [ ] **Step 6: Verify compilation**

```bash
cargo test 2>&1 | tail -30
```

- [ ] **Step 7: Verify TUI still works**

```bash
cargo install --path . --debug
zerostack --help
```

Expected: shows `--gui` flag in help output.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: update src/ to use zerostack_core imports, add --gui flag"
```

---

## Phase 2: Makepad GUI

### Task 12: Create `crates/gui` skeleton

**Files:**
- Create: `crates/gui/Cargo.toml`
- Create: `crates/gui/src/lib.rs`
- Create: `crates/gui/src/app.rs`
- Modify: `Cargo.toml` (workspace root)

**Interfaces:**
- Produces: `zerostack_gui` crate with Makepad app skeleton

- [ ] **Step 1: Create directories**

```bash
mkdir -p crates/gui/src/views crates/gui/src/components
```

- [ ] **Step 2: Write `crates/gui/Cargo.toml`**

```toml
[package]
name = "zerostack-gui"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "Makepad GUI frontend for zerostack"

[dependencies]
zerostack-core = { path = "../core" }
makepad-widgets = { git = "https://github.com/makepad/makepad", branch = "rik" }
```

Note: The Makepad branch name may need updating. Check the current makepad repo for the correct branch.

- [ ] **Step 3: Write `crates/gui/src/lib.rs`**

```rust
#![deny(unsafe_code)]

pub mod app;
pub mod bridge;
pub mod theme;
pub mod views;
pub mod components;
```

- [ ] **Step 4: Write `crates/gui/src/app.rs`**

```rust
use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    App = {{App}} {
        ui: <Window> {
            window: { title: "zerostack" },
            body = <View> {
                flow: Down,
                width: Fill, height: Fill,
                padding: 0,
                spacing: 0,

                <Label> {
                    text: "zerostack GUI",
                    draw_text: {
                        color: #fff,
                        text_style: <THEME_FONT_REGULAR> { font_size: 16.0 },
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] bridge: Option<crate::bridge::GuiBridge>,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

app_main!(App);
```

- [ ] **Step 5: Add `crates/gui` to workspace**

```toml
members = [".", "crates/extension-api", "crates/core", "crates/gui", "tests/extensions/test-echo", "tests/extensions/pi-simplify"]
```

- [ ] **Step 6: Verify GUI crate compiles**

```bash
cargo build -p zerostack-gui 2>&1 | tail -20
```

- [ ] **Step 7: Commit**

```bash
git add crates/gui/ Cargo.toml
git commit -m "feat(gui): add crates/gui skeleton with Makepad"
```

---

### Task 13: Implement GuiBridge

**Files:**
- Create: `crates/gui/src/bridge.rs`

**Interfaces:**
- Consumes: `zerostack_core::events::{CoreEvent, UserAction}`, `zerostack_core::config::Config`, `zerostack_core::engine::CoreEngine`, `zerostack_core::permission::SecurityMode`
- Produces: `GuiBridge` struct with `new()`, `send_action()`, `poll_events()`

- [ ] **Step 1: Write `crates/gui/src/bridge.rs`**

```rust
use std::thread;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use zerostack_core::config::Config;
use zerostack_core::engine::CoreEngine;
use zerostack_core::events::{CoreEvent, UserAction};
use zerostack_core::permission::SecurityMode;

/// Bridges the Makepad main thread and the tokio background thread.
/// All communication goes through unbounded channels.
pub struct GuiBridge {
    action_tx: UnboundedSender<UserAction>,
    event_rx: UnboundedReceiver<CoreEvent>,
    _runtime_thread: thread::JoinHandle<()>,
}

impl GuiBridge {
    pub fn new(config: Config, model: String, provider: String, mode: SecurityMode) -> Self {
        let (action_tx, mut action_rx) = unbounded_channel::<UserAction>();
        let (event_tx, event_rx) = unbounded_channel::<CoreEvent>();

        let runtime_thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build tokio runtime");

            rt.block_on(async move {
                let mut engine = CoreEngine::new(config, model.into(), provider.into(), mode);

                while let Some(action) = action_rx.recv().await {
                    let events = engine.handle_action(action).await;
                    for event in events {
                        if matches!(&event, CoreEvent::Error { message } if message.as_str() == "quit") {
                            let _ = event_tx.send(event);
                            return;
                        }
                        let _ = event_tx.send(event);
                    }
                }
            });
        });

        Self { action_tx, event_rx, _runtime_thread: runtime_thread }
    }

    pub fn send_action(&self, action: UserAction) {
        let _ = self.action_tx.send(action);
    }

    pub fn poll_events(&mut self) -> Vec<CoreEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        events
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo build -p zerostack-gui 2>&1 | tail -20
```

- [ ] **Step 3: Commit**

```bash
git add crates/gui/src/bridge.rs
git commit -m "feat(gui): implement GuiBridge for tokio-Makepad communication"
```

---

### Task 14: Implement theme module

**Files:**
- Create: `crates/gui/src/theme.rs`

- [ ] **Step 1: Write `crates/gui/src/theme.rs`**

```rust
/// Dark theme colors matching zerostack's existing TUI color scheme.
pub mod dark {
    pub const BG: &str = "#1A1B26";
    pub const SIDEBAR_BG: &str = "#1F2030";
    pub const USER_MSG_BG: &str = "#2D2E3F";
    pub const ASST_MSG_BG: &str = "#1A1B26";
    pub const CODE_BG: &str = "#0D0E1A";
    pub const ACCENT: &str = "#7C6FF0";
    pub const BORDER: &str = "#2D2E3F";
    pub const TEXT: &str = "#CDD6F4";
    pub const TEXT_SECONDARY: &str = "#6C7086";
    pub const ERROR: &str = "#F38BA8";
    pub const SUCCESS: &str = "#A6E3A1";
    pub const WARNING: &str = "#FAB387";
}

/// Light theme colors.
pub mod light {
    pub const BG: &str = "#FFFFFF";
    pub const SIDEBAR_BG: &str = "#F7F7F8";
    pub const USER_MSG_BG: &str = "#F0F0F0";
    pub const ASST_MSG_BG: &str = "#FFFFFF";
    pub const CODE_BG: &str = "#F5F5F5";
    pub const ACCENT: &str = "#6C5CE7";
    pub const BORDER: &str = "#E5E5E5";
    pub const TEXT: &str = "#1A1A2E";
    pub const TEXT_SECONDARY: &str = "#8E8EA0";
    pub const ERROR: &str = "#E74C3C";
    pub const SUCCESS: &str = "#27AE60";
    pub const WARNING: &str = "#F39C12";
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/gui/src/theme.rs
git commit -m "feat(gui): add theme color constants"
```

---

### Task 15: Implement MainView layout

**Files:**
- Create: `crates/gui/src/views/mod.rs`
- Create: `crates/gui/src/views/main_view.rs`
- Modify: `crates/gui/src/app.rs`

- [ ] **Step 1: Write `crates/gui/src/views/mod.rs`**

```rust
pub mod main_view;
pub mod sidebar;
pub mod chat_view;
pub mod input_bar;
```

- [ ] **Step 2: Write `crates/gui/src/views/main_view.rs`**

```rust
use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    MainView = {{MainView}} {
        width: Fill, height: Fill,
        flow: Right,
        spacing: 0,

        // Left sidebar (220px)
        <View> {
            width: 220, height: Fill,
            show_bg: true,
            draw_bg: { color: #1F2030 }
            flow: Down,
            padding: {top: 8, bottom: 8},

            <Label> {
                text: "Sessions",
                draw_text: {
                    color: #6C7086,
                    text_style: <THEME_FONT_REGULAR> { font_size: 11.0 },
                }
                padding: {left: 12, bottom: 8},
            }
        }

        // Right panel
        <View> {
            width: Fill, height: Fill,
            flow: Down,
            spacing: 0,

            // Header
            <View> {
                width: Fill, height: 40,
                show_bg: true,
                draw_bg: { color: #1A1B26 }
                padding: {left: 16, right: 16},
                flow: Right,
                align: {y: 0.5},

                <Label> {
                    text: "zerostack",
                    draw_text: {
                        color: #CDD6F4,
                        text_style: <THEME_FONT_BOLD> { font_size: 14.0 },
                    }
                }
            }

            // Chat area
            <View> {
                width: Fill, height: Fill,
                show_bg: true,
                draw_bg: { color: #1A1B26 }
                padding: 16,
                flow: Down,
                spacing: 12,

                <Label> {
                    text: "Welcome to zerostack GUI",
                    draw_text: {
                        color: #CDD6F4,
                        text_style: <THEME_FONT_REGULAR> { font_size: 14.0 },
                    }
                }
            }

            // Input bar
            <View> {
                width: Fill, height: 60,
                show_bg: true,
                draw_bg: { color: #1F2030 }
                padding: {left: 16, right: 16, top: 12, bottom: 12},
                flow: Right,
                spacing: 8,
                align: {y: 0.5},

                <TextInput> {
                    width: Fill, height: 36,
                    empty_message: "Type a message...",
                    draw_bg: { color: #2D2E3F, border_radius: 8.0 }
                }

                <Button> {
                    text: "Send",
                    draw_bg: { color: #7C6FF0, border_radius: 8.0 }
                }
            }

            // Status bar
            <View> {
                width: Fill, height: 28,
                show_bg: true,
                draw_bg: { color: #1F2030 }
                padding: {left: 16, right: 16},
                flow: Right,
                spacing: 16,
                align: {y: 0.5},

                <Label> {
                    text: "claude-sonnet",
                    draw_text: {
                        color: #6C7086,
                        text_style: <THEME_FONT_REGULAR> { font_size: 11.0 },
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct MainView {
    #[live] ui: WidgetRef,
}

impl Widget for MainView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
```

- [ ] **Step 3: Update `crates/gui/src/app.rs` to use MainView**

```rust
live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import crate::views::main_view::*;

    App = {{App}} {
        ui: <Window> {
            window: { title: "zerostack" },
            body = <MainView> {}
        }
    }
}
```

- [ ] **Step 4: Verify compilation**

```bash
cargo build -p zerostack-gui 2>&1 | tail -20
```

- [ ] **Step 5: Commit**

```bash
git add crates/gui/src/views/ crates/gui/src/app.rs
git commit -m "feat(gui): implement MainView layout with sidebar, chat, input, status bar"
```

---

### Task 16: Implement sidebar, chat view, input bar, and components

**Files:**
- Create: `crates/gui/src/views/sidebar.rs`
- Create: `crates/gui/src/views/chat_view.rs`
- Create: `crates/gui/src/views/input_bar.rs`
- Create: `crates/gui/src/components/mod.rs`
- Create: `crates/gui/src/components/message_bubble.rs`
- Create: `crates/gui/src/components/tool_card.rs`
- Create: `crates/gui/src/components/code_block.rs`
- Modify: `crates/gui/src/views/main_view.rs` (use new components)

- [ ] **Step 1: Write `crates/gui/src/views/sidebar.rs`**

```rust
use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    Sidebar = {{Sidebar}} {
        width: 220, height: Fill,
        flow: Down,
        show_bg: true,
        draw_bg: { color: #1F2030 }

        <View> {
            width: Fill, height: 40,
            padding: {left: 12, right: 12},
            flow: Right,
            align: {y: 0.5},
            spacing: 8,

            <Label> {
                text: "Sessions",
                draw_text: {
                    color: #6C7086,
                    text_style: <THEME_FONT_BOLD> { font_size: 11.0 },
                }
            }

            <View> { width: Fill, height: 0 }

            <Button> {
                text: "+",
                draw_bg: { color: #7C6FF0, border_radius: 4.0 }
                draw_text: {
                    color: #fff,
                    text_style: <THEME_FONT_BOLD> { font_size: 14.0 },
                }
                width: 28, height: 28,
            }
        }

        <ScrollView> {
            width: Fill, height: Fill,
            flow: Down,
            padding: {left: 4, right: 4},
            <View> { width: Fill, height: Fit, flow: Down, spacing: 2 }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct Sidebar {
    #[live] ui: WidgetRef,
}

impl Widget for Sidebar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
```

- [ ] **Step 2: Write `crates/gui/src/views/chat_view.rs`**

```rust
use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import crate::components::message_bubble::*;

    ChatView = {{ChatView}} {
        width: Fill, height: Fill,
        flow: Down,
        show_bg: true,
        draw_bg: { color: #1A1B26 }

        <ScrollView> {
            width: Fill, height: Fill,
            flow: Down,
            padding: {left: 16, right: 16, top: 16, bottom: 16},
            spacing: 4,

            <View> { width: Fill, height: Fit, flow: Down, spacing: 4 }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct ChatView {
    #[live] ui: WidgetRef,
}

impl Widget for ChatView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
```

- [ ] **Step 3: Write `crates/gui/src/views/input_bar.rs`**

```rust
use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    InputBar = {{InputBar}} {
        width: Fill, height: Fit,
        flow: Down,
        show_bg: true,
        draw_bg: { color: #1F2030 }

        <View> {
            width: Fill, height: 60,
            padding: {left: 16, right: 16, top: 12, bottom: 12},
            flow: Right,
            spacing: 8,
            align: {y: 0.5},

            <TextInput> {
                width: Fill, height: 36,
                empty_message: "Type a message... (Enter to send)",
                draw_bg: { color: #2D2E3F, border_radius: 8.0 }
                draw_text: {
                    color: #CDD6F4,
                    text_style: <THEME_FONT_REGULAR> { font_size: 14.0 },
                }
            }

            <Button> {
                text: "Send",
                width: 72, height: 36,
                draw_bg: { color: #7C6FF0, border_radius: 8.0, color_hover: #8B7FF7 }
                draw_text: {
                    color: #fff,
                    text_style: <THEME_FONT_BOLD> { font_size: 13.0 },
                }
            }
        }

        <View> {
            width: Fill, height: 28,
            padding: {left: 16, right: 16},
            flow: Right,
            spacing: 16,
            align: {y: 0.5},

            <Label> {
                text: "claude-sonnet",
                draw_text: {
                    color: #6C7086,
                    text_style: <THEME_FONT_REGULAR> { font_size: 11.0 },
                }
            }

            <View> { width: Fill, height: 0 }

            <Label> {
                text: "0 tokens",
                draw_text: {
                    color: #6C7086,
                    text_style: <THEME_FONT_REGULAR> { font_size: 11.0 },
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct InputBar {
    #[live] ui: WidgetRef,
}

impl Widget for InputBar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
```

- [ ] **Step 4: Write `crates/gui/src/components/message_bubble.rs`**

```rust
use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    UserMessage = {{UserMessage}} {
        width: Fill, height: Fit,
        flow: Right,
        padding: {top: 8, bottom: 8},

        <View> {
            width: Fill, height: Fit, flow: Right,
            <View> {
                width: Fit, height: Fit,
                show_bg: true,
                draw_bg: { color: #2D2E3F, border_radius: 12.0 }
                padding: {left: 16, right: 16, top: 10, bottom: 10},
                <Label> {
                    width: Fill,
                    text: "",
                    draw_text: {
                        color: #CDD6F4,
                        text_style: <THEME_FONT_REGULAR> { font_size: 14.0 },
                    }
                    wrap: Word,
                }
            }
        }
    }

    AssistantMessage = {{AssistantMessage}} {
        width: Fill, height: Fit,
        flow: Left,
        padding: {top: 8, bottom: 8},

        <View> {
            width: Fill, height: Fit, flow: Down,
            <Label> {
                text: "",
                draw_text: {
                    color: #CDD6F4,
                    text_style: <THEME_FONT_REGULAR> { font_size: 14.0 },
                }
                wrap: Word,
                padding: {left: 4, right: 4, top: 4, bottom: 4},
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct UserMessage { #[live] ui: WidgetRef; }
impl Widget for UserMessage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}

#[derive(Live, LiveHook)]
pub struct AssistantMessage { #[live] ui: WidgetRef; }
impl Widget for AssistantMessage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
```

- [ ] **Step 5: Write `crates/gui/src/components/tool_card.rs`**

```rust
use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    ToolCallCard = {{ToolCallCard}} {
        width: Fill, height: Fit,
        show_bg: true,
        draw_bg: {
            color: #1F2030, border_radius: 8.0,
            border_width: 1.0, border_color: #2D2E3F,
        }
        padding: {left: 12, right: 12, top: 8, bottom: 8},
        flow: Down, spacing: 4,

        <Label> {
            text: "🔧 tool_name",
            draw_text: {
                color: #7C6FF0,
                text_style: <THEME_FONT_BOLD> { font_size: 12.0 },
            }
        }
    }

    ToolResultCard = {{ToolResultCard}} {
        width: Fill, height: Fit,
        show_bg: true,
        draw_bg: {
            color: #1A1B26, border_radius: 8.0,
            border_width: 1.0, border_color: #2D2E3F,
        }
        padding: {left: 12, right: 12, top: 8, bottom: 8},
        flow: Down, spacing: 4,

        <Label> {
            text: "",
            draw_text: {
                color: #6C7086,
                text_style: <THEME_FONT_REGULAR> { font_size: 12.0 },
            }
            wrap: Word,
        }
    }
}

#[derive(Live, LiveHook)]
pub struct ToolCallCard { #[live] ui: WidgetRef; }
impl Widget for ToolCallCard {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}

#[derive(Live, LiveHook)]
pub struct ToolResultCard { #[live] ui: WidgetRef; }
impl Widget for ToolResultCard {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
```

- [ ] **Step 6: Write `crates/gui/src/components/code_block.rs`**

```rust
use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    CodeBlock = {{CodeBlock}} {
        width: Fill, height: Fit,
        show_bg: true,
        draw_bg: { color: #0D0E1A, border_radius: 8.0 }
        padding: {left: 16, right: 16, top: 12, bottom: 12},
        flow: Down, spacing: 4,

        <Label> {
            text: "",
            draw_text: {
                color: #CDD6F4,
                text_style: { font_size: 13.0 },
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct CodeBlock { #[live] ui: WidgetRef; }
impl Widget for CodeBlock {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
```

- [ ] **Step 7: Write `crates/gui/src/components/mod.rs`**

```rust
pub mod message_bubble;
pub mod tool_card;
pub mod code_block;
```

- [ ] **Step 8: Update `crates/gui/src/views/main_view.rs` to use new components**

Replace the placeholder sidebar, chat, and input sections with `<Sidebar> {}`, `<ChatView> {}`, and `<InputBar> {}`. Add imports:

```rust
import crate::views::sidebar::*;
import crate::views::chat_view::*;
import crate::views::input_bar::*;
```

- [ ] **Step 9: Verify compilation**

```bash
cargo build -p zerostack-gui 2>&1 | tail -20
```

- [ ] **Step 10: Commit**

```bash
git add crates/gui/src/
git commit -m "feat(gui): implement all GUI components (sidebar, chat, input, messages, tools, code)"
```

---

### Task 17: CLI integration — wire `--gui` flag

**Files:**
- Modify: `src/main.rs` (add GUI launch path)

- [ ] **Step 1: Add GUI launch in `src/main.rs`**

After the CLI parsing, before the TUI/headless branching, add:

```rust
if cli.gui {
    #[cfg(feature = "gui")]
    {
        // The Makepad app_main! macro handles the entry point.
        // We invoke it from the zerostack-gui crate.
        zerostack_gui::app::run_gui(cfg, model, provider, mode);
        return Ok(());
    }
    #[cfg(not(feature = "gui"))]
    {
        eprintln!("GUI support is not enabled. Rebuild with --features gui");
        std::process::exit(1);
    }
}
```

- [ ] **Step 2: Add `gui` feature flag to root `Cargo.toml`**

```toml
[features]
gui = ["zerostack-gui"]
```

And add the dependency:

```toml
[dependencies]
zerostack-gui = { path = "crates/gui", optional = true }
```

- [ ] **Step 3: Add `run_gui` function to `crates/gui/src/app.rs`**

```rust
pub fn run_gui(config: zerostack_core::config::Config, model: String, provider: String, mode: zerostack_core::permission::SecurityMode) {
    // The app_main! macro already provides the entry point.
    // We need to store the config for the App struct to use.
    std::env::set_var("ZEROSTACK_GUI_MODEL", &model);
    std::env::set_var("ZEROSTACK_GUI_PROVIDER", &provider);
    // app_main! handles the rest
}
```

Actually, Makepad's `app_main!` takes over the main function. For now, we need a different approach: the GUI binary should be a separate entry point. The simplest approach is to make `zerostack-gui` a binary crate, or add a binary target.

Simpler approach: Add a `[[bin]]` target in the gui crate's Cargo.toml:

```toml
[[bin]]
name = "zerostack-gui"
path = "src/main.rs"
```

And create `crates/gui/src/main.rs`:

```rust
fn main() {
    zerostack_gui::app::run();
}
```

Then the `--gui` flag in the main CLI invokes the separate binary:

```rust
if cli.gui {
    std::process::Command::new("zerostack-gui").spawn()?;
    return Ok(());
}
```

Or, keep it simple: just have the user run `cargo run -p zerostack-gui` or `zerostack-gui` directly. The `--gui` flag in the main CLI is a convenience that spawns the GUI binary.

- [ ] **Step 4: Verify compilation**

```bash
cargo build --all 2>&1 | tail -20
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(gui): add CLI integration for --gui flag"
```

---

## Phase 3: Polish & Testing

### Task 18: Run full test suite

- [ ] **Step 1: Run all tests**

```bash
cargo test --all-features 2>&1 | tail -30
```

- [ ] **Step 2: Fix any failing tests**

- [ ] **Step 3: Run `cargo fmt`**

```bash
cargo fmt
```

- [ ] **Step 4: Verify TUI still works**

```bash
cargo install --path . --debug
zerostack --help
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: finalize tests, fmt, verify TUI"
```

