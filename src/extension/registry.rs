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

/// Try to dispatch a slash command to a loaded extension.
pub fn dispatch_command(name: &str, args: &str) -> Option<String> {
    EXT_MANAGER.get().and_then(|manager| {
        let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
        mgr.dispatch_command(name, args).ok().flatten()
    })
}
