//! Global extension registry — allows the agent builder to access extension tools
//! without threading ExtensionManager through every function signature.
//!
//! Uses `std::sync::OnceLock` for one-time initialization at startup.

use std::sync::{Arc, Mutex, OnceLock};

use rig::tool::ToolDyn;

use crate::extension::manager::ExtensionManager;
use crate::extension::wrapper::ExtensionToolWrapper;

static EXTENSION_MANAGER: OnceLock<Arc<Mutex<ExtensionManager>>> = OnceLock::new();

/// Initialize the global extension registry from CLI --extension paths.
pub fn init_from_paths(extension_paths: &[std::path::PathBuf]) -> Result<Vec<String>, String> {
    let mut manager = ExtensionManager::new()?;

    let mut loaded = Vec::new();
    for path in extension_paths {
        match manager.load_standalone(path) {
            Ok(meta) => {
                tracing::info!(
                    extension_id = %meta.id,
                    tools = ?meta.tool_names,
                    "extension loaded from CLI"
                );
                loaded.push(meta.id.clone());
            }
            Err(e) => {
                tracing::error!(?path, error = %e, "failed to load extension");
                return Err(format!("failed to load {path:?}: {e}"));
            }
        }
    }

    let _ = manager.load_all();
    let _ = EXTENSION_MANAGER.set(Arc::new(Mutex::new(manager)));
    Ok(loaded)
}

/// Collect all extension tools as `Box<dyn ToolDyn>` for the agent builder.
pub fn collect_tools() -> Vec<Box<dyn ToolDyn>> {
    let Some(manager) = EXTENSION_MANAGER.get() else {
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

/// Try to dispatch a slash command to a loaded extension.
pub fn dispatch_command(name: &str, args: &str) -> Option<String> {
    EXTENSION_MANAGER.get().and_then(|manager| {
        let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
        mgr.dispatch_command(name, args).ok().flatten()
    })
}
