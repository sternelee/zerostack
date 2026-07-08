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
    /// Current context (updated per-session).
    cwd: String,
    session_id: String,
    model_name: String,
    project_trusted: bool,
}

impl ExtensionManager {
    /// Create a new ExtensionManager with an empty host.
    pub fn new() -> Result<Self, String> {
        let host = ExtensionHost::new()?;
        Ok(Self {
            host,
            extensions: Vec::new(),
            errors: Vec::new(),
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            session_id: String::new(),
            model_name: String::new(),
            project_trusted: false,
        })
    }

    /// Update context for all loaded extensions.
    pub fn update_context(
        &mut self,
        cwd: &str,
        session_id: &str,
        model_name: &str,
        project_trusted: bool,
    ) {
        self.cwd = cwd.to_string();
        self.session_id = session_id.to_string();
        self.model_name = model_name.to_string();
        self.project_trusted = project_trusted;
        self.host
            .update_context(cwd, session_id, model_name, project_trusted);
    }

    /// Call session_start on all loaded extensions.
    pub fn call_session_start(&mut self) {
        self.host.call_session_start();
    }

    /// Call session_shutdown on all loaded extensions.
    pub fn call_session_shutdown(&mut self) {
        self.host.call_session_shutdown();
    }

    /// Discover and load all extensions from standard directories.
    pub fn load_all(&mut self) -> Vec<&ExtensionMeta> {
        let dirs = loader::extension_dirs();
        let cwd = self.cwd.clone();
        let sid = self.session_id.clone();
        let mn = self.model_name.clone();
        let pt = self.project_trusted;

        for dir in &dirs {
            let bundles = loader::discover_extensions(dir);
            for bundle in bundles {
                self.load_bundle(bundle, &cwd, &sid, &mn, pt);
            }
        }

        self.extensions.iter().collect()
    }

    /// Load a single extension from a bundle.
    fn load_bundle(&mut self, bundle: ExtensionBundle, cwd: &str, sid: &str, mn: &str, pt: bool) {
        let extension_id = bundle.manifest.id.clone();
        let wasm_path = bundle.wasm_path.clone();

        match self.host.load_extension(
            &extension_id,
            &wasm_path,
            &bundle.manifest.capabilities,
            cwd,
            sid,
            mn,
            pt,
        ) {
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
    /// Tries to find and parse an adjacent `extension.toml` for metadata and
    /// capabilities; falls back to a sensible default.
    pub fn load_standalone(&mut self, wasm_path: &Path) -> Result<ExtensionMeta, String> {
        let extension_id = wasm_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("standalone")
            .to_string();

        // Try to find extension.toml next to the .wasm file, or in a parent dir.
        let manifest = find_manifest(wasm_path);

        let caps = manifest
            .as_ref()
            .map(|m| m.capabilities.clone())
            .unwrap_or_default();

        let meta = self.host.load_extension(
            &extension_id,
            wasm_path,
            &caps,
            &self.cwd,
            &self.session_id,
            &self.model_name,
            self.project_trusted,
        )?;

        let mut meta = meta;
        if let Some(m) = manifest {
            meta.name = m.name;
            meta.version = m.version;
            meta.description = m.description;
        }

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
        // Update context before each tool execution.
        self.host.update_context(
            &self.cwd,
            &self.session_id,
            &self.model_name,
            self.project_trusted,
        );

        let output = self.host.execute_tool(tool_name, params_json)?;
        Ok((output.content, output.details, output.is_error))
    }

    /// Drain queued prompts from all extensions (called after command dispatch).
    pub fn take_queued_prompts(&mut self) -> Vec<String> {
        self.host.take_queued_prompts()
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

/// Look for `extension.toml` adjacent to the .wasm file, or one directory up.
fn find_manifest(wasm_path: &Path) -> Option<loader::ExtensionManifest> {
    // Check same directory as .wasm.
    if let Some(dir) = wasm_path.parent() {
        let same_dir = dir.join("extension.toml");
        if same_dir.exists() {
            return loader::parse_manifest(&same_dir).ok();
        }
        // Check parent directory.
        if let Some(parent) = dir.parent() {
            let parent_manifest = parent.join("extension.toml");
            if parent_manifest.exists() {
                return loader::parse_manifest(&parent_manifest).ok();
            }
        }
    }
    None
}
