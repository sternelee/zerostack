//! Extension manager — orchestrates discovery, loading, and lifecycle.

use std::path::{Path, PathBuf};

use crate::extension::host::ExtensionHost;
use crate::extension::loader::{self, ExtensionBundle};
use crate::extension::{ExtensionMeta, RegisteredTool};

/// Top-level extension manager. Owns the ExtensionHost and coordinates
/// extension discovery, loading, queries, and teardown.
pub(crate) struct ExtensionManager {
    host: ExtensionHost,
    /// Metadata for loaded extensions by id.
    extensions: Vec<ExtensionMeta>,
    /// Bundles that failed to load (with error).
    errors: Vec<(PathBuf, String)>,
}

impl ExtensionManager {
    /// Create a new ExtensionManager with an empty host.
    pub fn new() -> Result<Self, String> {
        let host = ExtensionHost::new()?;
        Ok(Self {
            host,
            extensions: Vec::new(),
            errors: Vec::new(),
        })
    }

    /// Discover and load all extensions from standard directories.
    pub fn load_all(&mut self) -> Vec<&ExtensionMeta> {
        let dirs = loader::extension_dirs();

        for dir in &dirs {
            let bundles = loader::discover_extensions(dir);
            for bundle in bundles {
                self.load_bundle(bundle);
            }
        }

        self.extensions.iter().collect()
    }

    /// Load a single extension from a bundle.
    fn load_bundle(&mut self, bundle: ExtensionBundle) {
        let extension_id = bundle.manifest.id.clone();
        let wasm_path = bundle.wasm_path.clone();

        match self
            .host
            .load_extension(&extension_id, &wasm_path, &bundle.manifest.capabilities)
        {
            Ok(mut meta) => {
                meta.name = bundle.manifest.name.clone();
                meta.version = bundle.manifest.version.clone();
                meta.description = bundle.manifest.description.clone();
                tracing::info!(
                    extension_id = %extension_id,
                    "extension loaded"
                );
                self.extensions.push(meta);
            }
            Err(e) => {
                tracing::error!(
                    extension_id = %extension_id,
                    error = %e,
                    "failed to load extension"
                );
                self.errors.push((wasm_path, e));
            }
        }
    }

    /// Load a extension from an explicit .wasm path (for CLI --extension flag).
    pub fn load_standalone(&mut self, wasm_path: &Path) -> Result<ExtensionMeta, String> {
        let extension_id = wasm_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("standalone")
            .to_string();

        let default_caps = loader::Capabilities {
            tools: true,
            ..Default::default()
        };

        let meta = self
            .host
            .load_extension(&extension_id, wasm_path, &default_caps)?;

        self.extensions.push(meta.clone());
        Ok(meta)
    }

    /// Get all tool definitions from loaded extensions.
    pub fn all_tools(&self) -> Vec<RegisteredTool> {
        self.host.all_tools()
    }

    /// Execute a extension tool by name.
    pub fn execute_tool(
        &mut self,
        tool_name: &str,
        params_json: &str,
    ) -> Result<(String, String, bool), String> {
        let extension_id = self
            .extensions
            .iter()
            .find(|p| p.tool_names.contains(&tool_name.to_string()))
            .map(|p| p.id.clone())
            .ok_or_else(|| format!("Tool '{tool_name}' not found in any loaded extension"))?;

        let output = self
            .host
            .execute_tool(&extension_id, tool_name, params_json)?;
        Ok((output.content, output.details, output.is_error))
    }

    /// Dispatch a slash command to the extension that registered it.
    /// Returns Some(output) if a extension handled the command, None otherwise.
    pub fn dispatch_command(&mut self, name: &str, args: &str) -> Result<Option<String>, String> {
        self.host.dispatch_command(name, args)
    }

    pub fn list(&self) -> &[ExtensionMeta] {
        &self.extensions
    }

    pub fn errors(&self) -> &[(PathBuf, String)] {
        &self.errors
    }
}
