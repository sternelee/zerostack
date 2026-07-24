# add-dir

A zerostack extension that adds external directories to the session so their `AGENTS.md`, `CLAUDE.md`, and skills are loaded into the agent's system prompt on every turn — so the agent understands both projects at once.

Inspired by [pi-add-dir](https://github.com/itisbryan/pi-add-dir).

## What it does

- Register three slash commands:
  - `/add-dir <path>` — add a directory (absolute or relative to cwd)
  - `/add-dir` — show smart suggestions (Cargo workspace members, sibling projects with context files)
  - `/remove-dir [path]` — remove a directory (lists current dirs if no path)
  - `/dirs` — list currently-added directories
- Register three agent tools:
  - `add_directory({path})` — adds a directory
  - `remove_directory({path})` — removes a directory
  - `list_directories()` — lists current dirs
- Uses the `external-dirs` WIT host import to ask the host to canonicalise,
  deduplicate, and walk the registered directories on every agent turn.
- The host aggregates these directories and includes each dir's `AGENTS.md`
  / `CLAUDE.md` (and `ARCHITECTURE.md` with the `archmd` feature) under a
  "External directory context" header in the system prompt.

## Behavior details

1. On `init`, registers three commands and three tools via `command-registry` and `tool-registry`.
2. `/add-dir <path>` → `external_dirs.add_dir(path)` — host canonicalises and stores.
3. `/remove-dir <path>` → `external_dirs.remove_dir(path)` — host errors if path was not added.
4. `/add-dir` (no args) returns a smart-suggestion list (Cargo workspace members, sibling directories containing AGENTS.md / CLAUDE.md / Cargo.toml).
5. `/dirs` returns the host's current canonical list.
6. The agent can call `list_directories`, `add_directory`, `remove_directory` directly.

## Usage

### Build

Requires `wasm32-wasip2` target:

```bash
rustup target add wasm32-wasip2
cargo build -p add-dir --target wasm32-wasip2 --release
```

### Install

Copy the built `.wasm` file and `extension.toml` to an extension directory:

```bash
# Global (all projects)
mkdir -p ~/.local/share/zerostack/extensions/add-dir
cp target/wasm32-wasip2/release/add_dir.wasm extension.toml \
  ~/.local/share/zerostack/extensions/add-dir/

# Or project-local
mkdir -p .zerostack/extensions/add-dir
cp target/wasm32-wasip2/release/add_dir.wasm extension.toml \
  .zerostack/extensions/add-dir/
```

### Load

```bash
# From CLI (for testing)
zerostack -E target/wasm32-wasip2/debug/add_dir.wasm

# Auto-discovered from standard directories (no flag needed)
zerostack
```

### Examples

```
/add-dir /Users/me/other-project
/add-dir ../shared-library
/add-dir                       # Show suggestions
/remove-dir /Users/me/other-project
/dirs
```

When the agent decides to add a directory, it will call:

```
add_directory({ "path": "/Users/me/libs/core" })
```

## How it works

```
┌─────────────────────────────────────────┐
│  zerostack session                       │
│                                         │
│  /add-dir /other-project                │
│     │                                   │
│     ├─► extension calls                 │
│     │   external_dirs.add_dir("/path")  │
│     │                                   │
│     ├─► host canonicalises + stores     │
│     │   in ExtGuestState.external_dirs  │
│     │                                   │
│     ├─► next agent turn,                │
│     │   build_preamble() walks the      │
│     │   aggregated external_dirs list:   │
│     │     - <dir>/AGENTS.md             │
│     │     - <dir>/CLAUDE.md             │
│     │     [archmd] <dir>/ARCHITECTURE.md│
│     │                                   │
│     └─► "External directory context"    │
│         block appended to system prompt │
└─────────────────────────────────────────┘
```

### What's persisted

Each session file (`~/.local/share/zerostack/sessions/<id>.json`) has an
`external_dirs` field that records the directories added during the session.
This is informational today; the live state is owned by the host's
`ExtensionHost::external_dirs()` so a hot restart picks up wherever the
loaded extensions are configured.

## Limitations

| Works                                               | Limitation                                                                                                  |
| --------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| AGENTS.md / CLAUDE.md loaded into system prompt     | Skills from external dirs are not auto-registered (host skills system is separate, planned for a follow-up) |
| Agent can read/edit/write any path                  | `ctx.cwd` is read-only — external dirs work via absolute paths                                              |
| List, add, remove via slash commands or agent tools | Bulk suggestion heuristics are a subset of pi-add-dir's full scanner                                        |

## Extension manifest

```toml
# extension.toml
id = "zerostack/add-dir"
name = "Add Directory"
version = "0.1.0"
schema_version = 2

[extension]
entrypoint = "target/wasm32-wasip2/debug/add_dir.wasm"

[capabilities]
tools = true
commands = true
```

## Development

```bash
# Build
cargo build -p add-dir --target wasm32-wasip2

# Run tests (from repository root)
cargo test --features extensions
```

## License

[GPL-3.0](../../LICENSE)
