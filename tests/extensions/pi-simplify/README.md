# pi-simplify

A zerostack extension that adds the `/simplify` slash command — reviews recently changed files for clarity, consistency, and maintainability improvements.

Port of the [pi extension](https://github.com/earendil-works/pi-extensions) of the same name.

## Usage

### Build

Requires `wasm32-wasip2` target:

```bash
rustup target add wasm32-wasip2
cargo build -p pi-simplify --target wasm32-wasip2 --release
```

### Install

Copy the built `.wasm` file and `extension.toml` to an extension directory:

```bash
# Global (all projects)
mkdir -p ~/.local/share/zerostack/extensions/pi-simplify
cp target/wasm32-wasip2/release/pi_simplify.wasm extension.toml \
  ~/.local/share/zerostack/extensions/pi-simplify/

# Or project-local
mkdir -p .zerostack/extensions/pi-simplify
cp target/wasm32-wasip2/release/pi_simplify.wasm extension.toml \
  .zerostack/extensions/pi-simplify/
```

### Load

```bash
# From CLI (for testing)
zerostack -E target/wasm32-wasip2/debug/pi_simplify.wasm

# Auto-discovered from standard directories (no flag needed)
zerostack
```

### Commands

| Command | Description |
|---------|-------------|
| `/simplify` | Review files changed since last commit (HEAD) |
| `/simplify --staged` | Review staged changes |
| `/simplify --ref=HEAD~3` | Review changes against a specific ref |
| `/simplify src/foo.rs src/bar.rs` | Review specific files |

## How it works

1. `/simplify` runs `git diff --name-status <ref>` to find changed files
2. Builds a structured review prompt with quality principles
3. Injects the prompt into the agent via `trigger-prompt`
4. The agent reads each file, identifies improvements, and applies them

## Extension manifest

```toml
# extension.toml
id = "pi/simplify"
name = "Simplify"
version = "0.1.0"
schema_version = 2

[extension]
entrypoint = "pi_simplify.wasm"  # relative to this directory

[capabilities]
tools = false
commands = true
```
