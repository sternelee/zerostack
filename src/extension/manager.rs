//! Extension manager — orchestrates discovery, loading, and lifecycle.
//!
//! v0.5.0 changes:
//! - Project-trust gate for project-local `.zerostack/extensions/`.
//! - Version-pin check via `extension.toml.minimum_zerostack_version`.
//! - Conflict diagnostics (tool name + slash command name).
//! - Capability-enforced host imports; unsupported exports are *trapped*
//!   (extension side cannot silently call them).
//! - Provider registration queue surfaced.

use std::path::{Path, PathBuf};

use crate::extension::host::ExtensionHost;
use crate::extension::loader::{self, Capabilities, ExtensionBundle};
use crate::extension::{ExtensionMeta, LoadDiagnostics, RegisteredTool};

/// Top-level extension manager. Owns the ExtensionHost and coordinates
/// extension discovery, loading, queries, and teardown.
pub(crate) struct ExtensionManager {
    host: ExtensionHost,
    extensions: Vec<ExtensionMeta>,
    /// Bundles that failed to load (with error).
    errors: Vec<(PathBuf, String)>,
    /// Conflict + capability diagnostics emitted during load.
    diagnostics: LoadDiagnostics,
    /// Whether the host currently has a TUI to pop up dialogs.
    has_ui: bool,
    cwd: String,
    session_id: String,
    model_name: String,
    project_trusted: bool,
}

impl ExtensionManager {
    pub fn new() -> Result<Self, String> {
        let host = ExtensionHost::new()?;
        Ok(Self {
            host,
            extensions: Vec::new(),
            errors: Vec::new(),
            diagnostics: LoadDiagnostics::default(),
            has_ui: false,
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            session_id: String::new(),
            model_name: String::new(),
            project_trusted: false,
        })
    }

    pub fn set_has_ui(&mut self, has_ui: bool) {
        self.has_ui = has_ui;
        self.propagate_context();
    }

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
        self.propagate_context();
    }

    fn propagate_context(&mut self) {
        self.host.update_context(
            &self.cwd,
            &self.session_id,
            &self.model_name,
            self.project_trusted,
            self.has_ui,
        );
    }

    pub fn call_session_start(&mut self) {
        self.host.call_session_start();
    }

    pub fn call_session_shutdown(&mut self) {
        self.host.call_session_shutdown();
    }

    /// Discover and load all extensions from standard directories.
    /// Returns the metadata of newly loaded extensions.
    pub fn load_all(&mut self) -> Vec<&ExtensionMeta> {
        let dirs = loader::extension_dirs();
        let cwd = self.cwd.clone();
        let sid = self.session_id.clone();
        let mn = self.model_name.clone();
        let pt = self.project_trusted;
        let project_trusted_gate = pt; // alias for readability

        for dir in &dirs {
            let is_project_local = dir.ends_with(".zerostack/extensions");
            if is_project_local && !project_trusted_gate {
                tracing::warn!(
                    dir = %dir.display(),
                    "skipping project-local extensions: project is not trusted"
                );
                continue;
            }

            let bundles = loader::discover_extensions(dir);
            for bundle in bundles {
                self.load_bundle(bundle, &cwd, &sid, &mn, pt);
            }
        }

        // Conflict diagnostics collected after all extensions loaded.
        let cmd_conflicts = self.host.command_conflicts();
        for (name, exts) in cmd_conflicts {
            self.diagnostics.command_conflicts.push((name, exts));
        }
        let tool_conflicts = self.host.tool_conflicts();
        for (name, exts) in tool_conflicts {
            self.diagnostics.tool_conflicts.push((name, exts));
        }

        self.extensions.iter().collect()
    }

    fn load_bundle(&mut self, bundle: ExtensionBundle, cwd: &str, sid: &str, mn: &str, pt: bool) {
        let extension_id = bundle.manifest.id.clone();
        let wasm_path = bundle.wasm_path.clone();

        match self.host.load_extension(
            &extension_id,
            Some(&bundle.manifest),
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

    /// Load a single .wasm extension (used by `--extension <path>`).
    /// Tries to find an adjacent `extension.toml`; falls back to permissive
    /// defaults (tools + commands enabled).
    pub fn load_standalone(&mut self, wasm_path: &Path) -> Result<ExtensionMeta, String> {
        let extension_id = wasm_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("standalone")
            .to_string();

        let manifest = find_manifest(wasm_path);
        let caps = manifest
            .as_ref()
            .map(|m| m.capabilities.clone())
            .unwrap_or_else(|| Capabilities {
                tools: true,
                commands: true,
                ..Default::default()
            });

        let meta = self.host.load_extension(
            &extension_id,
            manifest.as_ref(),
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

    pub fn all_tools(&self) -> Vec<RegisteredTool> {
        self.host.all_tools()
    }

    /// Execute an extension-registered tool. Returns `(content, details, is_error, terminate, added_tool_names)`.
    pub fn execute_tool(
        &mut self,
        tool_name: &str,
        params_json: &str,
    ) -> Result<(String, String, bool, bool, Vec<String>), String> {
        self.propagate_context();

        let call_id = format!(
            "{}-{}-{}",
            tool_name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );

        let output = self.host.execute_tool(tool_name, params_json, &call_id)?;
        let terminate = output.terminate.unwrap_or(false);
        let added = output.added_tool_names.unwrap_or_default();
        Ok((
            output.content,
            output.details,
            output.is_error,
            terminate,
            added,
        ))
    }

    /// Drain queued prompts from all extensions (called after command dispatch).
    pub fn take_queued_prompts(
        &mut self,
    ) -> Vec<(String, crate::extension::host::types::DeliverAs)> {
        self.host.take_queued_prompts()
    }

    pub fn dispatch_command(&mut self, name: &str, args: &str) -> Result<Option<String>, String> {
        self.host.dispatch_command(name, args)
    }

    pub fn list(&self) -> &[ExtensionMeta] {
        &self.extensions
    }

    pub fn errors(&self) -> &[(PathBuf, String)] {
        &self.errors
    }

    pub fn diagnostics(&self) -> &LoadDiagnostics {
        &self.diagnostics
    }

    pub fn pending_provider_registrations(&self) -> Vec<(String, String, Option<String>)> {
        self.host.pending_provider_registrations()
    }

    pub fn drain_status_updates(&mut self) -> Vec<(String, Option<String>)> {
        self.host.drain_status_updates()
    }

    pub fn drain_widget_updates(&mut self) -> Vec<(String, Option<Vec<String>>, Option<String>)> {
        self.host.drain_widget_updates()
    }

    pub fn get_session_name(&self) -> String {
        self.host.get_session_name()
    }

    pub fn set_session_name(&mut self, name: &str) {
        self.host.set_session_name(name);
    }

    pub fn get_terminal_title(&self) -> String {
        self.host.get_terminal_title()
    }

    /// Reload: unload all extensions and re-discover from disk.
    /// Emits `session_shutdown` to existing extensions before clearing.
    pub fn reload(&mut self) -> Result<(), String> {
        self.host.call_session_shutdown();
        self.host = ExtensionHost::new()?;
        self.extensions.clear();
        self.errors.clear();
        self.diagnostics = LoadDiagnostics::default();
        self.load_all();
        Ok(())
    }
}

fn find_manifest(wasm_path: &Path) -> Option<loader::ExtensionManifest> {
    if let Some(dir) = wasm_path.parent() {
        let same = dir.join("extension.toml");
        if same.exists() {
            return loader::parse_manifest(&same).ok();
        }
        if let Some(parent) = dir.parent() {
            let par = parent.join("extension.toml");
            if par.exists() {
                return loader::parse_manifest(&par).ok();
            }
        }
    }
    None
}
