//! add-dir extension for zerostack.
//!
//! Adds external directories to the session so their `AGENTS.md`,
//! `CLAUDE.md`, and (via the host preamble walker) skills are included in
//! the agent's system prompt on every turn. Equivalent of `pi-add-dir`
//! adapted to the zerostack extension contract.
//!
//! Commands exposed:
//!   - `/add-dir [<path>]`     — add a directory (absolute or relative to cwd)
//!   - `/remove-dir [<path>]`  — remove a directory (lists current dirs if no arg)
//!   - `/dirs`                 — list currently added directories
//!
//! Tools exposed (for the agent):
//!   - `add_directory({path})`
//!   - `remove_directory({path})`
//!   - `list_directories()`

wit_bindgen::generate!({
    world: "extension-world",
    path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../../crates/extension-api/wit"),
});

use crate::zerostack::extension::command_registry::CommandDefinition;
use crate::zerostack::extension::external_dirs as ext_dirs;
use crate::zerostack::extension::tool_registry::ToolDefinition;
use crate::zerostack::extension::types::ExecutionMode;

struct AddDirExtension;

impl Guest for AddDirExtension {
    fn init() -> Result<(), String> {
        // Slash commands.
        for def in [
            CommandDefinition {
                name: "add-dir".into(),
                description: "Add an external directory to the session's context".into(),
                argument_hint: Some("<path>".into()),
            },
            CommandDefinition {
                name: "remove-dir".into(),
                description: "Remove a previously-added external directory".into(),
                argument_hint: Some("[path]".into()),
            },
            CommandDefinition {
                name: "dirs".into(),
                description: "List currently-added external directories".into(),
                argument_hint: None,
            },
        ] {
            crate::zerostack::extension::command_registry::register_command(&def)
                .map_err(|e| format!("register_command({}) failed: {e}", def.name))?;
        }

        // Agent-callable tools.
        for def in [
            ToolDefinition {
                name: "add_directory".into(),
                label: "Add Directory".into(),
                description:
                    "Add an external directory to the session. The directory's AGENTS.md \
                     and CLAUDE.md will be loaded into every turn's system prompt so the \
                     agent understands both projects at once. Path may be absolute or \
                     relative to the current working directory."
                        .into(),
                parameters_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"Directory path to add"}},"required":["path"]}"#.into(),
                prompt_snippet: None,
                prompt_guidelines: None,
                execution_mode: Some(ExecutionMode::Parallel),
                deferred: Some(false),
            },
            ToolDefinition {
                name: "remove_directory".into(),
                label: "Remove Directory".into(),
                description:
                    "Remove a previously-added external directory from the session. \
                     Errors if the path is not currently in the list."
                        .into(),
                parameters_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"Directory path to remove"}},"required":["path"]}"#.into(),
                prompt_snippet: None,
                prompt_guidelines: None,
                execution_mode: Some(ExecutionMode::Parallel),
                deferred: Some(false),
            },
            ToolDefinition {
                name: "list_directories".into(),
                label: "List External Directories".into(),
                description:
                    "List all external directories currently in this session. \
                     Returns an empty list if none have been added."
                        .into(),
                parameters_schema: r#"{"type":"object","properties":{}}"#.into(),
                prompt_snippet: None,
                prompt_guidelines: None,
                execution_mode: Some(ExecutionMode::Parallel),
                deferred: Some(false),
            },
        ] {
            crate::zerostack::extension::tool_registry::register_tool(&def)
                .map_err(|_| format!("register_tool({}) failed (name conflict)", def.name))?;
        }

        Ok(())
    }

    fn tool_execute(name: String, params_json: String) -> Result<ToolOutput, String> {
        // Tool names reach us namespaced as `zerostack/add-dir__add_directory`
        // (and similar). Match by suffix so the bare-name resolution path
        // works without false positives.
        let path_arg = |p: &str| -> Result<String, String> {
            let v: serde_json::Value =
                serde_json::from_str(p).map_err(|e| format!("invalid JSON: {e}"))?;
            v.get("path")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| "missing required field `path`".to_string())
        };

        if name.ends_with("add_directory") {
            let path = path_arg(&params_json)?;
            ext_dirs::add_dir(&path)?;
            Ok(ToolOutput {
                content: format!("directory added: {path}"),
                details: serde_json::json!({ "path": path }).to_string(),
                is_error: false,
                terminate: None,
                added_tool_names: None,
                is_partial: None,
            })
        } else if name.ends_with("remove_directory") {
            let path = path_arg(&params_json)?;
            ext_dirs::remove_dir(&path)?;
            Ok(ToolOutput {
                content: format!("directory removed: {path}"),
                details: serde_json::json!({ "path": path }).to_string(),
                is_error: false,
                terminate: None,
                added_tool_names: None,
                is_partial: None,
            })
        } else if name.ends_with("list_directories") {
            let dirs = ext_dirs::list_dirs();
            let content = if dirs.is_empty() {
                "no external directories in this session.".to_string()
            } else {
                format!(
                    "{} external director{}:\n{}",
                    dirs.len(),
                    if dirs.len() == 1 { "y" } else { "ies" },
                    dirs.iter()
                        .map(|d| format!("  - {d}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
            Ok(ToolOutput {
                content,
                details: serde_json::json!({ "dirs": &dirs }).to_string(),
                is_error: false,
                terminate: None,
                added_tool_names: None,
                is_partial: None,
            })
        } else {
            Err(format!("unknown tool: {name}"))
        }
    }

    fn on_command(name: String, args: String) -> Result<String, String> {
        if name.ends_with("add-dir") {
            return handle_add_dir(&args);
        }
        if name.ends_with("remove-dir") {
            return handle_remove_dir(&args);
        }
        if name.ends_with("dirs") {
            return handle_dirs();
        }
        Ok(String::new())
    }

    fn session_start() -> Result<(), String> {
        Ok(())
    }

    fn session_shutdown() -> Result<(), String> {
        Ok(())
    }

    // ── v0.5.0 event hooks — no-op defaults ──
    fn prepare_arguments(_name: String, args_json: String) -> Result<String, String> {
        Ok(format!("ok:{args_json}"))
    }
    fn on_tool_call(
        _name: String,
        _call_id: String,
        _input_json: String,
    ) -> Result<ToolCallDecision, String> {
        Ok(ToolCallDecision {
            block: None,
            reason: None,
            new_input_json: None,
        })
    }
    fn on_tool_result(
        _name: String,
        _call_id: String,
        _input_json: String,
        _content: String,
        _details: String,
        _is_error: bool,
    ) -> Result<ToolResultPatch, String> {
        Ok(ToolResultPatch {
            content: None,
            details: None,
            is_error: None,
            drop: None,
        })
    }
    fn on_user_bash(_command: String, _cwd: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_set_session_name(_name: String) -> Result<bool, String> {
        Ok(false)
    }
    fn on_session_before_compact(_reason: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_session_compacted(_reason: String, _summary: String) -> Result<(), String> {
        Ok(())
    }
    fn on_context(_messages_json: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_before_agent_start(_prompt: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_input(_text: String, _source: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_message_update(_message_json: String) -> Result<(), String> {
        Ok(())
    }
    fn on_event(_name: String, _payload_json: String) -> Result<(), String> {
        Ok(())
    }
    fn init_async() -> Result<(), String> {
        Ok(())
    }
}

// ── command handlers ─────────────────────────────────────────

fn handle_add_dir(args: &str) -> Result<String, String> {
    let args = args.trim();
    if args.is_empty() {
        // Smart suggestion: scan cwd for sibling directories and Cargo
        // workspace members. Lightweight — we are inside a sandbox so we
        // don't reach anywhere else.
        let suggestions = suggest_external_dirs();
        if suggestions.is_empty() {
            return Ok("usage: /add-dir <path>".into());
        }
        let mut out = format!(
            "usage: /add-dir <path>\n\nsuggestions ({}):",
            suggestions.len()
        );
        for s in &suggestions {
            out.push_str(&format!("\n  - {s}"));
        }
        return Ok(out);
    }

    ext_dirs::add_dir(args)?;
    Ok(format!("directory added: {args}"))
}

fn handle_remove_dir(args: &str) -> Result<String, String> {
    let args = args.trim();
    if args.is_empty() {
        let dirs = ext_dirs::list_dirs();
        if dirs.is_empty() {
            return Ok("no external directories in this session (nothing to remove)".into());
        }
        let mut out = "usage: /remove-dir <path>\n\ncurrent directories:".to_string();
        for d in &dirs {
            out.push_str(&format!("\n  - {d}"));
        }
        return Ok(out);
    }

    ext_dirs::remove_dir(args)?;
    Ok(format!("directory removed: {args}"))
}

fn handle_dirs() -> Result<String, String> {
    let dirs = ext_dirs::list_dirs();
    if dirs.is_empty() {
        return Ok(
            "no external directories in this session. use /add-dir <path> to add one.".into(),
        );
    }
    let mut out = format!(
        "{} external director{}:",
        dirs.len(),
        if dirs.len() == 1 { "y" } else { "ies" }
    );
    for d in &dirs {
        out.push_str(&format!("\n  - {d}"));
    }
    Ok(out)
}

/// Smart suggestions for `/add-dir` with no args. Behaviour is a small,
/// self-contained subset of `pi-add-dir`'s relevance-scoring pass — we are
/// inside a sandbox so we only inspect cwd's immediate surroundings and
/// surface the ones most likely to be useful.
fn suggest_external_dirs() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(_) => return out,
    };

    // 1. Cargo workspace members (path = "." is the current crate, skip it).
    if let Ok(text) = std::fs::read_to_string(cwd.join("Cargo.toml")) {
        let mut in_ws = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed == "[workspace]" {
                in_ws = true;
                continue;
            }
            if trimmed.starts_with('[') && in_ws {
                in_ws = false;
            }
            if in_ws
                && let Some(rest) = trimmed.strip_prefix("members")
                && let Some(value) = rest.split('=').nth(1)
            {
                for tok in value.split(',') {
                    let tok = tok.trim().trim_matches('"').trim_matches('\'');
                    if !tok.is_empty() && tok != "." {
                        let p = cwd.join(tok).canonicalize().ok();
                        if let Some(p) = p {
                            push_unique(&mut out, &p.to_string_lossy());
                        }
                    }
                }
            }
        }
    }

    // 2. Sibling directories next to cwd (one level up), excluding the
    //    cwd basename itself. Heuristic: prefer directories containing
    //    AGENTS.md, CLAUDE.md, or a `Cargo.toml` — those are likely leads
    //    another agent run has a stake in.
    if let Some(parent) = cwd.parent() {
        let my_basename = cwd.file_name();
        let entries: Vec<_> = match std::fs::read_dir(parent) {
            Ok(e) => e.flatten().collect(),
            Err(_) => Vec::new(),
        };
        for entry in entries {
            let p = entry.path();
            if !p.is_dir() || p.file_name() == my_basename {
                continue;
            }
            let has_context = p.join("AGENTS.md").exists()
                || p.join("CLAUDE.md").exists()
                || p.join("Cargo.toml").exists();
            if has_context && let Ok(canon) = p.canonicalize() {
                push_unique(&mut out, &canon.to_string_lossy());
            }
        }
    }

    out
}

fn push_unique(vec: &mut Vec<String>, s: &str) {
    if !vec.iter().any(|x| x == s) {
        vec.push(s.to_string());
    }
}

export!(AddDirExtension);
