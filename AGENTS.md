# Repository Guidelines

## Project Overview

**zerostack** is a minimal coding agent CLI written in Rust. It provides an interactive TUI (built with `crossterm`) for conversational coding with LLM providers (OpenAI, Anthropic, Gemini, Ollama, OpenRouter, custom OpenAI-compatible gateways). Supports tools (read/write/edit/bash/grep), MCP servers, Wasm extensions, subagents, memory, git worktrees, and headless (`-p`/`--loop`) operation.

## Architecture & Data Flow

```
CLI (clap) → main.rs
  ├─ config::load()           # TOML/YAML config from data dir
  ├─ provider::build_agent()  # Model client + agent with tools
  ├─ extension::registry::init_from_paths()  # Wasm extensions
  └─ ui::run_interactive()    # TUI event loop
       ├─ event_handler       # AgentEvent routing
       ├─ slash/              # Slash command dispatch
       ├─ permission_handler  # Ask/deny UI
       └─ input/              # Text editor + pickers
```

**Agents** are `rig::agent::Agent<M>` built via `agent::builder::build_agent_inner()`. The builder assembles a system preamble (AGENTS.md, ARCHITECTURE.md, skills, memory), attaches tools (read, write, edit, bash, grep, find, list_dir, todo), plus optional MCP tools and extension tools.

**Sessions** (`session::Session`) hold message history as `Vec<SessionMessage>`, persisted as JSON files in `~/.local/share/zerostack/sessions/`.

**The TUI** (`ui::mod::run_interactive`) runs a single-threaded event loop: user input → slash command dispatch or agent run → stream tokens/events → display.

## Key Directories

| Directory | Purpose |
|-----------|---------|
| `src/agent/` | Agent building, tool definitions, runner loop |
| `src/ui/` | TUI: renderer, event loop, slash commands, pickers, input |
| `src/config/` | Config loading (TOML/YAML), config struct, types |
| `src/session/` | Session state, storage (JSON), chat history |
| `src/provider.rs` | LLM provider client construction (OpenAI, Anthropic, etc.) |
| `src/permission/` | Permission checker, pattern matching, ask flow |
| `src/extension/` | Wasm extension host (wasmtime), loader, registry, manager |
| `src/extras/` | Feature-gated modules: mcp, subagents, memory, git_worktree, loop, advisor, archmd, etc. |
| `src/context/` | Context files (AGENTS.md, prompts, themes, skills) |
| `src/tests/` | Integration and unit tests (flat module) |
| `docs/` | User-facing documentation |
| `scripts/` | Build/utility scripts |
| `packaging/` | AUR, Homebrew, Conda packaging |
| `crates/extension-api/` | Extension WIT interface crate for Wasm extension authors |
| `tests/extensions/` | Example/test Wasm extensions (test-echo, pi-simplify) |

## Development Commands

```
# Check compilation (via tests)
cargo test

# Format code (always do this)
cargo fmt

# Install dev binary
cargo install --path . --debug

# Run a specific test
cargo test test_name

# Build Wasm extensions
cargo build -p test-echo --target wasm32-wasip2
cargo build -p pi-simplify --target wasm32-wasip2

# Build release (intentional, for distribution)
cargo build --release
```

**NEVER**: `cargo build` (use `cargo install --path . --debug`), `cargo check` (use `cargo test`), `--release` during development.

## Code Conventions

### Formatting & Naming
- `cargo fmt` (standard Rust style)
- Module names: `snake_case` (e.g., `git_worktree`, `shell_mode`)
- Struct/enum: `CamelCase` (e.g., `SlashCtx`, `MessageRole`, `AgentEvent`)
- Functions: `snake_case` (e.g., `build_agent_inner`, `handle_slash`)
- Constants: `SCREAMING_SNAKE_CASE` (e.g., `TOOL_RESULT_SAVE_THRESHOLD`)
- Prefer `CompactString` over `String` for persistent text (session messages, event payloads)
- Prefer `SmallVec` over `Vec` for small fixed-size collections

### Error Handling
- `anyhow::Result` for application-level errors
- `String` as error type for extension host (Wasm boundary)
- `tracing` for logging (never `println!`/`eprintln!` in library code)
- Log levels: `debug` for agent stream events, `info` for startup/extension loading, `warn` for discovery errors, `error` for failures

### Async Patterns
- `tokio` runtime, single-threaded by default (`flavor = "current_thread"`), multi-threaded via `multithread` feature
- Uses `tokio::sync::mpsc` for agent events, user events
- `rig` (0.39) for LLM provider interactions — streaming responses via `StreamingChat`
- Agent runner uses `spawn_blocking` for sync Wasm extension tool calls

### Feature Gating (`#[cfg(feature = "...")]`)
- Optional features: `mcp`, `subagents`, `memory`, `loop`, `git-worktree`, `extensions`, `archmd`, `advisor`, `multimodal`, `acp`, `status-signals`
- Feature-gated code uses `#[cfg(feature = "...")]` blocks around imports, struct fields, and match arms
- Two `ensure_agent` copies (with/without `mcp` feature) exist in `event_handler.rs`

### State Management
- `OnceLock<Arc<Mutex<T>>>` for global extension registry
- `LazyLock` for test artifacts (preferred over `OnceLock` when initializer is known at declaration)
- `Arc<AtomicBool>` for shared boolean flags (e.g., `is_running`)
- Session state passed by `&mut` through the TUI event loop

### TUI Patterns
- `SlashCtx<'a>` bundles all mutable references needed by slash commands
- Slash commands are dispatched in `ui::slash::mod::handle_slash()`
- New slash commands: add handler function, add to `BASE_COMMANDS` in `src/ui/pickers/list.rs`, implement in `src/ui/slash/`
- `InputEditor::load_text()` to set input buffer content

## Important Files

| File | Role |
|------|------|
| `src/main.rs` | Entry point, CLI parsing, provider selection, TUI/headless branching |
| `src/cli.rs` | Clap argument definitions |
| `src/agent/builder.rs` | `build_agent_inner()` — agent construction with tools, preamble |
| `src/agent/runner.rs` | `spawn_agent()` — agent event loop, `run_print()` — headless mode |
| `src/config/mod.rs` | `Config` struct (~200 fields), config loading, resolution helpers |
| `src/config/load.rs` | Config file loading (TOML/YAML), first-run defaults |
| `src/provider.rs` | `build_agent()`, `AnyAgent`, client construction for all providers |
| `src/session/mod.rs` | `Session` struct, message management, compaction, calibration |
| `src/ui/mod.rs` | `run_interactive()` — main TUI loop (~2700 lines) |
| `src/event.rs` | `AgentEvent`, `BtwEvent`, `UserEvent` enums |
| `src/extension/host.rs` | wasmtime component-model runtime, WIT host impls |
| `src/extension/registry.rs` | Global extension registry (OnceLock), tool/command dispatch |
| `crates/extension-api/wit/extension-v0.2.0.wit` | WIT interface for Wasm extension authors |
| `Cargo.toml` | Workspace config, feature flags, dependency versions |
| `AGENTS.md` | Build constraints (this repo's own agent instructions) |
| `docs/CONFIG.md` | User-facing config documentation |
| `docs/COMMANDS.md` | Slash command reference |

## Runtime/Tooling Preferences

- **Rust edition**: 2024
- **Package manager**: Cargo (workspace with 4 members)
- **Global allocator**: `mimalloc`
- **LLM framework**: `rig` 0.39 (streaming agents, tool definitions)
- **TUI library**: `crossterm` 0.29
- **Wasm runtime**: `wasmtime` 46 (component model, WASI p2)
- **Dev profile**: `opt-level = 1`, `debug = false` (fast iteration)
- **Release profile**: `opt-level = "z"`, `lto = "thin"`, `strip = true`

## Testing & QA

- **Framework**: `cargo test` (standard Rust test harness)
- **Test location**: `src/tests/` (flat module, one file per subsystem)
- **Integration tests**: `src/extension/tests.rs` (Wasm extensions, feature-gated)
- **Test patterns**: Use `#[cfg(test)]` mod in source files for unit tests, separate files in `src/tests/` for subsystem tests
- **Write tests** for all new non-TUI code
- **Run before claiming done**: `cargo test` (all 589+ tests must pass)
- Extension tests require `wasm32-wasip2` target and `--features extensions`

## Extension System

- **WIT contract**: `crates/extension-api/wit/extension-v0.2.0.wit` (v0.3.0 with trigger-prompt)
- **Capabilities**: `tools`, `commands`, `lifecycle`, `provider`, `ui`, `exec`, `http`, `session`
- **Defaults**: `tools: true`, `commands: true`, others `false`
- **Tool namespacing**: `ext_id__tool_name` (double underscore separator)
- **Command registration**: Extension calls `register_command()` in `init()`, host namespaces as `ext_id__command_name`
- **Bare name resolution**: Both tools and commands support bare names when unambiguous
- **Context**: `get_context()` returns `{cwd, session_id, model_name, project_trusted}`
- **Lifecycle**: `session_start()`, `session_shutdown()` optional exports
- **Trigger prompt**: `trigger-prompt(prompt, deliver_as)` injects prompt into agent input
