# session-name

A zerostack extension that adds the `/name` slash command and a `set_session_name` tool — auto-generates concise, meaningful session titles and updates the terminal title in real time.

Inspired by [pi-session-name](https://github.com/ttttmr/pi-session-name).

## What it does

- Register a `/name` slash command with three modes:
  - `/name My Title` — set the session name directly
  - `/name` (with existing name) — show the current session name
  - `/name` (no name yet) — inject a naming prompt so the agent generates one
- Register a `set_session_name` tool that the agent calls when it decides on a title
- Update the terminal window title on every change:
  - `· zerostack - <cwd>` — while no session name is set
  - `✳ <session name> - <cwd>` — after a session name is set
- Restore the terminal title to `zerostack` on session shutdown

## Behavior details

1. On `session-start`, the extension sets the terminal title to `· zerostack - <cwd>` (or `✳ <name> - <cwd>` if a name already exists).
2. When `/name` is invoked without arguments and no name exists, a prompt is injected via `trigger-prompt` asking the agent to generate a short (2–5 word) title and call `set_session_name`.
3. The agent responds with a title, calls the tool, and the name is persisted to the session file.
4. Future `/name` invocations will show the existing name.
5. On `session-shutdown`, the terminal title is restored to `zerostack`.

## Usage

### Build

Requires `wasm32-wasip2` target:

```bash
rustup target add wasm32-wasip2
cargo build -p session-name --target wasm32-wasip2 --release
```

### Install

Copy the built `.wasm` file and `extension.toml` to an extension directory:

```bash
# Global (all projects)
mkdir -p ~/.local/share/zerostack/extensions/session-name
cp target/wasm32-wasip2/release/session_name.wasm extension.toml \
  ~/.local/share/zerostack/extensions/session-name/

# Or project-local
mkdir -p .zerostack/extensions/session-name
cp target/wasm32-wasip2/release/session_name.wasm extension.toml \
  .zerostack/extensions/session-name/
```

### Load

```bash
# From CLI (for testing)
zerostack -E target/wasm32-wasip2/debug/session_name.wasm

# Auto-discovered from standard directories (no flag needed)
zerostack
```

### Commands & Tools

| Name | Type | Description |
|------|------|-------------|
| `/name` | Slash command | Show, set, or auto-generate a session name |
| `set_session_name` | Agent tool | Set the current session name (2–5 words) |

### Examples

```
/name                                  # Show current name or trigger auto-generation
/name Debugging the auth middleware     # Set name directly
```

When the agent is prompted to name the session, it will call:

```
set_session_name({ "name": "Auth middleware debugging" })
```

## How it works

1. The extension registers a `/name` slash command and a `set_session_name` tool via the WIT `command-registry` and `tool-registry` host imports.
2. When `/name` is invoked without a name set, it calls `trigger-prompt` to inject a naming request into the agent input queue.
3. The agent processes the prompt, generates a title, and calls `set_session_name`.
4. The tool handler calls `session-control::set_session_name()` to persist the name and `session-control::set_terminal_title()` to update the terminal.
5. The host syncs the session name back to the session JSON file on each agent turn completion.

## Extension manifest

```toml
# extension.toml
id = "zerostack/session-name"
name = "Session Name"
version = "0.1.0"
schema_version = 2

[extension]
entrypoint = "target/wasm32-wasip2/debug/session_name.wasm"

[capabilities]
tools = true
commands = true
```

## Development

```bash
# Build
cargo build -p session-name --target wasm32-wasip2

# Run tests (from repository root)
cargo test --features extensions
```

## License

[GPL-3.0](../../LICENSE)
