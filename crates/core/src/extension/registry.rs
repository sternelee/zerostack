//! Extension registry — per-session extension manager, stored in a static
//! for access by the agent builder and slash-command dispatcher without
//! threading through every function signature.
//!
//! Set once at startup from CLI `--extension` flags, cleared on shutdown.

use std::sync::{Arc, Mutex, OnceLock};

use rig::tool::ToolDyn;

use crate::extension::manager::ExtensionManager;
use crate::extension::wrapper::ExtensionToolWrapper;

/// The per-session extension manager. Set once at startup.
static EXT_MANAGER: OnceLock<Arc<Mutex<ExtensionManager>>> = OnceLock::new();

/// Create an extension manager from CLI `--extension` paths, then discover and
/// load extensions from standard directories. Stores the result in the static
/// registry for later access.
///
/// Errors are returned eagerly for explicit CLI paths; auto-discovered
/// extensions log warnings but do not fail startup.
pub fn init_from_paths(extension_paths: &[std::path::PathBuf]) -> Result<(), String> {
    let mut manager = ExtensionManager::new()?;

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

    EXT_MANAGER
        .set(Arc::new(Mutex::new(manager)))
        .map_err(|_| "extension manager already initialized".to_string())?;
    Ok(())
}

/// Collect all extension tools as `Box<dyn ToolDyn>` from the registry.
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

/// Dispatch a slash command and return (output_text, queued_prompts).
pub fn dispatch_with_prompts(name: &str, args: &str) -> (Option<String>, Vec<String>) {
    let Some(manager) = EXT_MANAGER.get() else {
        return (None, Vec::new());
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    let output = mgr.dispatch_command(name, args).ok().flatten();
    let prompts = mgr.take_queued_prompts();
    (output, prompts)
}

/// Try to dispatch a slash command to a loaded extension (legacy API).
pub fn dispatch_command(name: &str, args: &str) -> Option<String> {
    dispatch_with_prompts(name, args).0
}

/// Get the current session name from the extension manager.
pub fn get_session_name() -> String {
    let Some(manager) = EXT_MANAGER.get() else {
        return String::new();
    };
    let mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.get_session_name()
}

/// Set the session name via the extension manager.
pub fn set_session_name(name: &str) {
    let Some(manager) = EXT_MANAGER.get() else {
        return;
    };
    let mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.set_session_name(name);
}

/// Update context on all loaded extensions.
pub fn update_context(cwd: &str, session_id: &str, model_name: &str, project_trusted: bool) {
    let Some(manager) = EXT_MANAGER.get() else {
        return;
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.update_context(cwd, session_id, model_name, project_trusted);
}

/// Call session_start on all loaded extensions.
pub fn call_session_start() {
    let Some(manager) = EXT_MANAGER.get() else {
        return;
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.call_session_start();
}

/// Call session_shutdown on all loaded extensions.
pub fn call_session_shutdown() {
    let Some(manager) = EXT_MANAGER.get() else {
        return;
    };
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.call_session_shutdown();
}

/// Get slash-command names registered by loaded extensions (without "/" prefix).
pub fn extension_command_names() -> Vec<String> {
    let Some(manager) = EXT_MANAGER.get() else {
        return Vec::new();
    };
    let mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.list()
        .iter()
        .flat_map(|meta| meta.command_names.iter().cloned())
        .map(|name| format!("/{}", name.rsplit("__").next().unwrap_or(&name)))
        .collect()
}
