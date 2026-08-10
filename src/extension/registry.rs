//! Extension registry — per-session extension manager, stored in a static
//! for access by the agent builder and slash-command dispatcher without
//! threading through every function signature.
//!
//! v0.5.0 changes:
//! - `dispatch_with_prompts` now returns queued prompts as `(text, deliver-as)`
//!   so the agent runner can route them to `steer` / `follow-up` / `next-turn`.
//! - `init_from_paths` accepts a `project_trusted` flag so the manager can
//!   gate project-local `.zerostack/extensions/` until the user opts in.
//! - New `set_has_ui` so `ui-prompt.select/confirm/input` knows whether to
//!   return empty buffers or actually pop a dialog.

use std::sync::{Arc, Mutex, OnceLock};

use rig::tool::ToolDyn;

use crate::extension::host::types::DeliverAs;
use crate::extension::manager::ExtensionManager;
use crate::extension::wrapper::ExtensionToolWrapper;

/// The per-session extension manager. Set once at startup.
static EXT_MANAGER: OnceLock<Arc<Mutex<ExtensionManager>>> = OnceLock::new();

/// Initialize the extension manager from CLI paths + auto-discovery.
/// `project_trusted` gates the project-local `.zerostack/extensions/` dir.
pub fn init_from_paths(
    extension_paths: &[std::path::PathBuf],
    project_trusted: bool,
) -> Result<(), String> {
    let mut manager = ExtensionManager::new()?;
    manager.update_context(
        &std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        "",
        "",
        project_trusted,
    );

    for path in extension_paths {
        match manager.load_standalone(path) {
            Ok(meta) => {
                tracing::info!(
                    extension_id = %meta.id,
                    tools = ?meta.tool_names,
                    "extension loaded from CLI"
                );
            }
            Err(e) => {
                tracing::error!(?path, error = %e, "failed to load extension");
                return Err(format!("failed to load {path:?}: {e}"));
            }
        }
    }

    let discovered = manager.load_all();
    if !discovered.is_empty() {
        tracing::info!(
            count = discovered.len(),
            "auto-discovered extensions loaded"
        );
    }
    for (path, error) in manager.errors().iter() {
        tracing::warn!(?path, error = %error, "extension discovery error");
    }
    for (name, exts) in &manager.diagnostics().command_conflicts {
        tracing::warn!(
            command = name,
            extensions = ?exts,
            "slash command collision: same bare name registered by multiple extensions"
        );
    }
    for (name, exts) in &manager.diagnostics().tool_conflicts {
        tracing::warn!(
            tool = name,
            extensions = ?exts,
            "tool-name collision: same bare name registered by multiple extensions"
        );
    }

    EXT_MANAGER
        .set(Arc::new(Mutex::new(manager)))
        .map_err(|_| "extension manager already initialized".to_string())?;
    Ok(())
}

pub fn set_has_ui(has_ui: bool) {
    let Some(manager) = EXT_MANAGER.get() else {
        return;
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.set_has_ui(has_ui);
}

pub fn collect_tools() -> Vec<Box<dyn ToolDyn>> {
    let Some(manager) = EXT_MANAGER.get() else {
        return Vec::new();
    };

    let mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    let tools = mgr.all_tools();
    if tools.is_empty() {
        return Vec::new();
    }
    tools
        .into_iter()
        .map(|def| Box::new(ExtensionToolWrapper::new(def, manager.clone())) as Box<dyn ToolDyn>)
        .collect()
}

/// Dispatch a slash command. Returns `(output_text, queued_prompts)`.
/// Queued prompts carry `deliver-as` semantics (steer/follow-up/next-turn).
pub fn dispatch_with_prompts(name: &str, args: &str) -> (Option<String>, Vec<(String, DeliverAs)>) {
    let Some(manager) = EXT_MANAGER.get() else {
        return (None, Vec::new());
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    let output = mgr.dispatch_command(name, args).ok().flatten();
    let prompts = mgr.take_queued_prompts();
    (output, prompts)
}

/// Legacy single-shot dispatch.
pub fn dispatch_command(name: &str, args: &str) -> Option<String> {
    dispatch_with_prompts(name, args).0
}

/// Slash command names registered by extensions, returned with `/` prefix.
/// Conflicts get a `:N` suffix so the user can disambiguate.
pub fn extension_command_names_with_conflicts() -> Vec<String> {
    let Some(manager) = EXT_MANAGER.get() else {
        return Vec::new();
    };
    let mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    let mut by_bare: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for meta in mgr.list() {
        for cmd in &meta.command_names {
            let bare = cmd.rsplit("__").next().unwrap_or(cmd).to_string();
            by_bare.entry(bare).or_default().push(cmd.clone());
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (bare, namespaced_list) in by_bare {
        if namespaced_list.len() == 1 {
            out.push(format!("/{}", bare));
        } else {
            for (i, _) in namespaced_list.iter().enumerate() {
                out.push(format!("/{}:{}", bare, i + 1));
            }
        }
    }
    out.sort();
    out
}

/// Back-compat alias used by old callers (returns bare names with `/` prefix).
pub fn extension_command_names() -> Vec<String> {
    extension_command_names_with_conflicts()
}

pub fn get_session_name() -> String {
    let Some(manager) = EXT_MANAGER.get() else {
        return String::new();
    };
    let mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.get_session_name()
}

pub fn set_session_name(name: &str) {
    let Some(manager) = EXT_MANAGER.get() else {
        return;
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.set_session_name(name);
}

pub fn get_terminal_title() -> String {
    let Some(manager) = EXT_MANAGER.get() else {
        return String::new();
    };
    let mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.get_terminal_title()
}

pub fn update_context(cwd: &str, session_id: &str, model_name: &str, project_trusted: bool) {
    let Some(manager) = EXT_MANAGER.get() else {
        return;
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.update_context(cwd, session_id, model_name, project_trusted);
}

pub fn call_session_start() {
    let Some(manager) = EXT_MANAGER.get() else {
        return;
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.call_session_start();
}

pub fn call_session_shutdown() {
    let Some(manager) = EXT_MANAGER.get() else {
        return;
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.call_session_shutdown();
}

/// Drain status-bar updates emitted by extensions during the previous turn.
/// Returns `(key, text | None)` pairs (None clears the entry).
pub fn drain_status_updates() -> Vec<(String, Option<String>)> {
    let Some(manager) = EXT_MANAGER.get() else {
        return Vec::new();
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.drain_status_updates()
}

/// Drain widget updates emitted by extensions during the previous turn.
pub fn drain_widget_updates() -> Vec<(String, Option<Vec<String>>, Option<String>)> {
    let Some(manager) = EXT_MANAGER.get() else {
        return Vec::new();
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.drain_widget_updates()
}

/// Reload all extensions (used by `/reload`).
pub fn reload() -> Result<(), String> {
    let manager = EXT_MANAGER
        .get()
        .ok_or_else(|| "extension manager not initialized".to_string())?;
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.reload()
}

/// Build a Markdown block describing registered extension tools so the
/// agent's preamble reflects extensions. Each tool contributes one
/// `prompt_snippet` line and (if non-empty) a bullet list of
/// `prompt_guidelines`.
pub fn extension_preamble_block() -> String {
    let Some(manager) = EXT_MANAGER.get() else {
        return String::new();
    };
    let mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    let mut block = String::new();
    block.push_str("## Extension-provided tools\n\n");
    let mut any = false;
    for tool in mgr.all_tools() {
        any = true;
        block.push_str(&format!("- **{}** — {}\n", tool.name, tool.description));
        if let Some(snippet) = tool.prompt_snippet.as_deref() {
            block.push_str(&format!("  when to use: {snippet}\n"));
        }
        for g in &tool.prompt_guidelines {
            block.push_str(&format!("  - {g}\n"));
        }
    }
    if !any {
        return String::new();
    }
    block
}
