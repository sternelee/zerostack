# zerostack Extension System v0.5.0

zerostack extensions are wasmtime component-model modules loaded from a
manifest directory (`extension.toml` + `extension.wasm`). The WIT contract is
`zerostack:extension@0.5.0` and lives in
`crates/extension-api/wit/extension-v0.5.0.wit`.

## Lifecycle

1. **Discovery** — `~/.local/share/zerostack/extensions/` + `<cwd>/.zerostack/extensions/`
   + paths passed via `--extension`. Project-local directories are gated
   behind `project_trusted = true`.
2. **Manifest parse** — `extension.toml` declares `id`, `name`, `version`,
   `schema_version`, optional `[extension] entrypoint`,
   `[extension] minimum_zerostack_version`, and `[capabilities]`.
3. **Version-pin** — host compares `minimum_zerostack_version` against the
   running zerostack version and refuses to load any extension that
   requires a newer host.
4. **Component instantiation** — `.wasm` is parsed, fuel is set
   (`200M`), memory limit is `96 MiB`, stack limit is `512 KiB`,
   table limit is `10 000`, and `init_async`/`init` are called.
5. **Capability check** — tool/command registrations are validated
   against declared capabilities. Providers, exec, http, UI, agent-control,
   events-bus, etc. are gated at the host impl level (logged warnings
   today; full linker trap is planned for a future wasmtime).
6. **Session** — `session_start()` is broadcast; `dispatch_command` and
   `execute_tool` route through the host. `session_shutdown()` is called
   on quit/reload.

## WIT surface

### Required guest exports

+ `init() -> result<_, string>` — register tools/commands.
+ `tool-execute(name, params-json) -> result<tool-output, string>` — runs a registered tool.
+ `on-command(name, args) -> result<string, string>` — runs a registered slash command.
+ All other v0.5.0 events as no-ops.

### Optional guest exports (host invokes unconditionally; trap = "no handler")

+ `session-start`, `session-shutdown`
+ `init-async`
+ `prepare-arguments(name, args-json) -> string | "ok:<json>" | "block:<msg>" | "patch:<json>"`
+ `on-tool-call(name, call-id, input-json) -> tool-call-decision`
+ `on-tool-result(name, call-id, input-json, content, details, is-error) -> tool-result-patch`
+ `on-user-bash(command, cwd) -> string`
+ `on-set-session-name(name) -> bool`
+ `on-session-before-compact(reason) -> string`
+ `on-session-compacted(reason, summary) -> ()`
+ `on-context(messages-json) -> string`
+ `on-before-agent-start(prompt) -> string`
+ `on-input(text, source) -> string`
+ `on-message-update(message-json) -> ()`
+ `on-event(name, payload-json) -> ()` (cross-extension events)

### Host imports (extension → host)

| Interface | Surface |
| ----------- | -------- |
| `tool-registry` | `register-tool`, `unregister-tool` (ToolDefinition with `execution-mode`, `deferred`, `prompt-snippet`, `prompt-guidelines`) |
| `command-registry` | `register-command`, `unregister-command` |
| `extension-context` | `get-context` (`{cwd, session-id, model-name, project-trusted, has-ui}`) |
| `trigger-prompt` | `trigger-prompt(prompt, deliver-as)` where `deliver-as ∈ {steer, follow-up, next-turn}` |
| `session-control` | `get-session-name`, `set-session-name`, `set-terminal-title` |
| `provider-registry` | `register-provider(config)`, `unregister-provider(name)` |
| `ui-prompt` | `select`, `confirm`, `input`, `notify` |
| `ui-status` | `set-status`, `set-widget`, `set-title`, `toast` |
| `agent-control` | `send-message`, `send-user-message`, `append-entry`, `set-model`, `get-active-tools`, `set-active-tools`, `compact` |
| `exec` | `run(command, args, cwd?, timeout-ms?)` → exec-result |
| `http` | `request(method, url, headers?, body?, timeout-ms?)` → string body |
| `file-mutation-queue` | `with-lock(path, callback-id)` |
| `truncator` | `truncate-tail`, `truncate-head`, `cap-output` |
| `compaction` | `before-compact`, `after-compact` |
| `events-bus` | `publish`, `subscribe`, `unsubscribe` |
| `permissions` | `check`, `trust-project`, `set-project-trusted` |
| `resources-discover` | `discover` |
| `logger` | `log(level, target, message)` |

## Tool execution flow

```
rig::ToolDyn::call(args)
  → ExtensionToolWrapper::call
      1. JSON-Schema validate args         # rejects early with ToolError
      2. host.execute_tool(name, args, call_id)
            a. call on_tool_call           # block / patch / no-op
            b. call prepare_arguments      # arguments rewrite
            c. call tool_execute           # the actual work
            d. call on_tool_result         # patch / drop / no-op
      → ToolOutput → <content> + <details>
            (terminate / added_tool_names serialized into details JSON)
```

## Capability gating

| Capability | Manifest | WIT imports allowed |
| --- | --- | --- |
| `tools` | `[capabilities] tools = true` | `tool-registry` |
| `commands` | … | `command-registry` |
| `provider` | … | `provider-registry` |
| `ui` | … | `ui-prompt`, `ui-status` |
| `exec` | … | `exec` |
| `http` | … | `http` |
| `session` | … | `session-control`, `file-mutation-queue`, `compaction`, `resources-discover` |

Failures to declare a capability gate the relevant guest calls to either a
log warning (today) or a wasm trap (planned).

## Recipes

### Add a status-bar entry

```rust
crate::zerostack::extension::ui_status::set_status(&"my-ext", Some("ready"));
```

### Pop a confirmation

```rust
let ok = crate::zerostack::extension::ui_prompt::confirm("Delete?", "Are you sure?");
```

### Block a tool call

```rust
fn on_tool_call(name: String, _id: String, input: String) -> Result<ToolCallDecision, String> {
    if name == "bash" && input.contains("rm -rf /") {
        Ok(ToolCallDecision {
            block: Some(true),
            reason: Some("blocked: protected path".into()),
            new_input_json: None,
        })
    } else {
        Ok(ToolCallDecision { block: None, reason: None, new_input_json: None })
    }
}
```

### Inject a follow-up prompt

```rust
crate::zerostack::extension::trigger_prompt::trigger_prompt(
    "summarize the diff for me",
    DeliverAs::FollowUp,
)?;
```

### Truncate tool output

```rust
let capped = crate::zerostack::extension::truncator::cap_output(
    s,
    Some(50_000),  // max bytes
    Some(2_000),   // max lines
);
```

### Schedule end-of-loop termination

Return `ToolOutput { terminate: Some(true), .. }` from `tool_execute` and the
runner will surface `__terminate__: true` in `<details>` JSON so the agent
loop can read it and end.

## Diagnostics

`ExtensionManager.diagnostics()` exposes:

+ `tool_conflicts[(name, extensions)]` — bare-name collisions across extensions.
+ `command_conflicts[(name, extensions)]` — bare-name collisions.
+ `warnings`, `unsupported_events`

These are logged at load time and surfaced in the picker (`/foo:1`, `/foo:2`
suffix on conflict).
