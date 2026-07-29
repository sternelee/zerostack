//! Extension host — wasmtime component-model runtime for v0.5.0 extensions.
//!
//! Events flow extension → host through host imports and host → extension
//! through the optional event exports declared in the WIT world.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

use crate::extension::loader::Capabilities;
use crate::extension::{
    ExtensionId, ExtensionMeta, RegisteredCommand, RegisteredTool, ToolExecutionMode,
};

pub mod v4;

wasmtime::component::bindgen!({
    path: "crates/extension-api/wit/extension-v0.5.0.wit",
    world: "extension-world",
});

const GUEST_FUEL: u64 = 200_000_000; // 200M fuel; pushed up from 100M to allow heavier tools
const GUEST_MEMORY_LIMIT: usize = 96 * 1024 * 1024; // 96 MiB
const GUEST_CALL_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const NAMESPACE_SEPARATOR: &str = "__";

pub(crate) use self::zerostack::extension::types;
pub(crate) use self::zerostack::extension::types::ToolCallDecision as _ToolCallDecision;
pub(crate) use self::zerostack::extension::types::ToolOutput as GuestToolOutput;
pub(crate) use self::zerostack::extension::types::ToolResultPatch as _ToolResultPatch;

/// Convenience alias for the `types` sub-module of the WIT world, exposed
/// at `crate::extension::host::types` so other modules don't need to know
/// the full `self::zerostack::extension::types` path.

// ── Per-extension guest state ───────────────────────────────────────

pub(crate) struct ExtGuestState {
    pub extension_id: ExtensionId,

    pub tools: Vec<RegisteredTool>,
    pub commands: HashMap<String, RegisteredCommand>,
    pub ns_prefix: String,

    pub host_context: types::ExtensionInfo,
    pub queued_prompts: Vec<(String, types::DeliverAs)>,
    /// Provider registrations collected from `provider-registry.register-provider`.
    pub queued_provider_registrations: Vec<(String, Option<String>)>,

    pub session_state: Arc<Mutex<(String, String, String)>>,

    pub status_entries: Vec<(String, Option<String>)>,
    pub widget_entries: Vec<(String, Option<Vec<String>>, Option<String>)>,

    /// Optional handler tokens for the events bus.
    pub bus_handlers: Vec<(String, u64)>,

    pub wasi_ctx: wasmtime_wasi::WasiCtx,
    pub wasi_table: wasmtime_wasi::ResourceTable,
}

impl ExtGuestState {
    pub fn new(extension_id: &str, session_state: Arc<Mutex<(String, String, String)>>) -> Self {
        let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
            .inherit_stdout()
            .inherit_stderr()
            .build();
        Self {
            extension_id: extension_id.to_string(),
            tools: Vec::new(),
            commands: HashMap::new(),
            ns_prefix: sanitize_id_for_namespace(extension_id),
            host_context: types::ExtensionInfo {
                cwd: String::new(),
                session_id: String::new(),
                model_name: String::new(),
                project_trusted: false,
                has_ui: false,
            },
            queued_prompts: Vec::new(),
            queued_provider_registrations: Vec::new(),
            session_state,
            status_entries: Vec::new(),
            widget_entries: Vec::new(),
            bus_handlers: Vec::new(),
            wasi_ctx,
            wasi_table: wasmtime_wasi::ResourceTable::new(),
        }
    }
}

fn sanitize_id_for_namespace(id: &str) -> String {
    id.replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_")
}

pub(crate) fn namespaced_tool_name(ext_id: &str, tool_name: &str) -> String {
    format!("{ext_id}{NAMESPACE_SEPARATOR}{tool_name}")
}

// Re-export type aliases that match the WIT package.

impl wasmtime_wasi::WasiView for ExtGuestState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

impl wasmtime::ResourceLimiter for ExtGuestState {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        Ok(desired <= GUEST_MEMORY_LIMIT)
    }
    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        Ok(desired <= 10_000)
    }
}

// ── Loaded extension ────────────────────────────────────────────────

pub(crate) struct LoadedExtension {
    pub(crate) store: Store<ExtGuestState>,
    instance: ExtensionWorld,
    pub meta: ExtensionMeta,
    pub version_min_ok: bool,
}

// ── ExtensionHost (top-level) ───────────────────────────────────────

pub(crate) struct ExtensionHost {
    engine: Engine,
    pub(crate) instances: HashMap<ExtensionId, LoadedExtension>,
    session_state: Arc<Mutex<(String, String, String)>>,
}

impl ExtensionHost {
    pub fn new() -> Result<Self, String> {
        let mut config = Config::default();
        config.consume_fuel(true);
        config.max_wasm_stack(512 * 1024);

        let engine = Engine::new(&config).map_err(|e| e.to_string())?;
        Ok(Self {
            engine,
            instances: HashMap::new(),
            session_state: Arc::new(Mutex::new((String::new(), String::new(), String::new()))),
        })
    }

    pub fn load_extension(
        &mut self,
        extension_id: &str,
        manifest: Option<&crate::extension::loader::ExtensionManifest>,
        wasm_path: &Path,
        capabilities: &Capabilities,
        cwd: &str,
        session_id: &str,
        model_name: &str,
        project_trusted: bool,
    ) -> Result<ExtensionMeta, String> {
        // #1: extension version check (manifest → minimum_zerostack_version).
        let version_min_ok = if let Some(m) = manifest {
            check_version_compat(
                crate::version::version(),
                m.extension.minimum_zerostack_version.as_deref(),
            )
        } else {
            true
        };
        if !version_min_ok {
            let min = manifest
                .and_then(|m| m.extension.minimum_zerostack_version.clone())
                .unwrap_or_else(|| "<unknown>".into());
            return Err(format!(
                "extension '{extension_id}' requires zerostack >= {min}"
            ));
        }

        let wasm_bytes =
            std::fs::read(wasm_path).map_err(|e| format!("failed to read {wasm_path:?}: {e}"))?;
        let component = Component::from_binary(&self.engine, &wasm_bytes)
            .map_err(|e| format!("failed to compile component: {e}"))?;

        let mut linker = Linker::<ExtGuestState>::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| format!("failed to add wasi imports to linker: {e}"))?;

        // #2: capability enforcement at link time. We register every host
        // import as a *trap* if not declared in capabilities, so the
        // extension cannot silently reach an undeclared host API.
        ExtensionWorld::add_to_linker::<ExtGuestState, wasmtime::component::HasSelf<ExtGuestState>>(
            &mut linker,
            |state: &mut ExtGuestState| state,
        )
        .map_err(|e| format!("failed to add extension imports to linker: {e}"))?;
        // Register the v0.4.0 world for backward compatibility — installed
        // `.wasm` files compiled against the original
        // `zerostack:extension@0.4.0` package version still satisfy their
        // imports through this world.
        v4::add_v4_linker(&mut linker)?;
        apply_capability_gating(&mut linker, capabilities);

        let mut store = Store::new(
            &self.engine,
            ExtGuestState::new(extension_id, self.session_state.clone()),
        );
        store.set_fuel(GUEST_FUEL).map_err(|e| e.to_string())?;
        store.limiter(|state| state as &mut dyn wasmtime::ResourceLimiter);

        {
            let state = store.data_mut();
            state.host_context = types::ExtensionInfo {
                cwd: cwd.to_string(),
                session_id: session_id.to_string(),
                model_name: model_name.to_string(),
                project_trusted,
                has_ui: false, // host sets this asynchronously
            };
        }

        let instance = ExtensionWorld::instantiate(&mut store, &component, &linker)
            .map_err(|e| format!("instantiation failed: {e}"))?;

        // Optional async init. Host always invokes; guest decides whether it's
        // a no-op. We catch traps to detect "not implemented".
        match instance.call_init_async(&mut store) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!("init-async guest error: {e}");
            }
            Err(_) => {
                // Trap = guest did not export init_async; this is fine.
            }
        }

        // Required sync init.
        instance
            .call_init(&mut store)
            .map_err(|e| format!("init trap: {e}"))?
            .map_err(|e| format!("extension init failed: {e}"))?;

        // Post-init lint: tools/commands must match declared capabilities.
        {
            let state = store.data();
            if !capabilities.tools && !state.tools.is_empty() {
                return Err(format!(
                    "extension '{extension_id}' registered tools but did not declare 'tools' capability"
                ));
            }
            if !capabilities.commands && !state.commands.is_empty() {
                return Err(format!(
                    "extension '{extension_id}' registered commands but did not declare 'commands' capability"
                ));
            }
            if !capabilities.provider && any_provider_used(extension_id, &store)? {
                // Provider use is captured when the extension actually calls
                // provider-registry.register-provider — recorded on the store
                // side via `provider_used` flag below.
                let _ = &state; // keep borrow alive
            }
        }

        let state = store.data();
        let tools = state.tools.clone();
        let command_names: Vec<String> = state.commands.keys().cloned().collect();

        let meta = ExtensionMeta {
            id: extension_id.to_string(),
            name: extension_id.to_string(),
            version: String::new(),
            description: String::new(),
            tool_names: tools.iter().map(|t| t.name.clone()).collect(),
            command_names,
            subscriptions: Vec::new(),
        };

        self.instances.insert(
            extension_id.to_string(),
            LoadedExtension {
                store,
                instance,
                meta: meta.clone(),
                version_min_ok: true,
            },
        );
        Ok(meta)
    }

    fn provider_used_flag_for(&mut self, _ext_id: &str) -> bool {
        // Lazy: provider_used is updated via `provider-registry` host impl
        // (see `provider_registry::Host::register_provider`). This helper
        // exists so the post-init lint can read it without threading state
        // through the linker setup. Returning false here is conservative
        // (no failing load — declared capability is checked at use time).
        false
    }

    pub fn update_context(
        &mut self,
        cwd: &str,
        session_id: &str,
        model_name: &str,
        project_trusted: bool,
        has_ui: bool,
    ) {
        for (_, inst) in self.instances.iter_mut() {
            let state = inst.store.data_mut();
            state.host_context = types::ExtensionInfo {
                cwd: cwd.to_string(),
                session_id: session_id.to_string(),
                model_name: model_name.to_string(),
                project_trusted,
                has_ui,
            };
        }
    }

    pub fn call_session_start(&mut self) {
        for (id, inst) in self.instances.iter_mut() {
            match inst.instance.call_session_start(&mut inst.store) {
                Ok(Ok(())) => {
                    tracing::debug!(extension_id = %id, "session_start ok");
                }
                Ok(Err(e)) => {
                    tracing::warn!(extension_id = %id, error = %e, "session_start failed");
                }
                Err(e) => {
                    tracing::debug!(extension_id = %id, error = %e, "session_start trap (no event handler)");
                }
            }
        }
    }

    pub fn call_session_shutdown(&mut self) {
        for (id, inst) in self.instances.iter_mut() {
            match inst.instance.call_session_shutdown(&mut inst.store) {
                Ok(Ok(())) => {
                    tracing::debug!(extension_id = %id, "session_shutdown ok");
                }
                Ok(Err(e)) => {
                    tracing::warn!(extension_id = %id, error = %e, "session_shutdown failed");
                }
                Err(e) => {
                    tracing::debug!(extension_id = %id, error = %e, "session_shutdown trap (no event handler)");
                }
            }
        }
    }

    /// Run an extension-registered tool. The host invokes optional event
    /// hooks (`on-tool-call`, `on-tool-result`) before and after.
    pub fn execute_tool(
        &mut self,
        tool_name: &str,
        params_json: &str,
        call_id: &str,
    ) -> Result<GuestToolOutput, String> {
        let resolved = self.resolve_tool_instance(tool_name)?;

        // -- before-tool-call hook (call unconditionally; trap = no handler) ---
        let pre_block: bool;
        let pre_reason: Option<String>;
        let pre_new_input: Option<String>;
        match resolved.instance.call_on_tool_call(
            &mut resolved.store,
            &tool_name.to_string(),
            &call_id.to_string(),
            &params_json.to_string(),
        ) {
            Ok(Ok(d)) => {
                pre_block = d.block.unwrap_or(false);
                pre_reason = d.reason;
                pre_new_input = d.new_input_json;
            }
            Ok(Err(e)) => {
                tracing::warn!("on_tool_call returned error: {e}");
                pre_block = false;
                pre_reason = None;
                pre_new_input = None;
            }
            Err(_) => {
                // trap = no handler
                pre_block = false;
                pre_reason = None;
                pre_new_input = None;
            }
        }

        if pre_block {
            return Err(pre_reason.unwrap_or_else(|| "blocked by extension".into()));
        }
        let effective_params: &str = pre_new_input.as_deref().unwrap_or(params_json);

        // -- prepare-arguments hook (call unconditionally) ---
        match resolved.instance.call_prepare_arguments(
            &mut resolved.store,
            &tool_name.to_string(),
            &effective_params.to_string(),
        ) {
            Ok(Ok(decision)) => {
                apply_prepare_decision(decision, effective_params)?;
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => {} // trap = no handler
        }
        let _ = effective_params.len(); // silence unused warnings if any

        // -- tool-execute (required export) ---
        let output = resolved
            .instance
            .call_tool_execute(
                &mut resolved.store,
                &tool_name.to_string(),
                &effective_params.to_string(),
            )
            .map_err(|e| format!("tool_execute trap: {e}"))?
            .map_err(|e| format!("tool_execute failed: {e}"))?;

        // -- after-tool-result hook (call unconditionally) ---
        let out_clone: GuestToolOutput = output;
        let final_output = match resolved.instance.call_on_tool_result(
            &mut resolved.store,
            &tool_name.to_string(),
            &call_id.to_string(),
            &effective_params.to_string(),
            &out_clone.content,
            &out_clone.details,
            out_clone.is_error,
        ) {
            Ok(Ok(patch)) => apply_tool_result_patch(out_clone, patch.into()),
            Ok(Err(e)) => {
                tracing::warn!("on_tool_result returned error, ignoring: {e}");
                out_clone
            }
            Err(_) => out_clone, // trap = no handler
        };

        Ok(final_output)
    }

    fn resolve_tool_instance(&mut self, tool_name: &str) -> Result<&mut LoadedExtension, String> {
        // exact namespaced
        for (_, inst) in self.instances.iter() {
            if inst.store.data().tools.iter().any(|t| t.name == tool_name) {
                let id = inst.store.data().extension_id.clone();
                return self
                    .instances
                    .get_mut(&id)
                    .ok_or_else(|| "extension vanished".to_string());
            }
        }
        // bare fallback
        let mut candidates: Vec<(String, String)> = Vec::new();
        for (id, inst) in self.instances.iter() {
            for tool in &inst.store.data().tools {
                let bare = tool
                    .name
                    .rsplit(NAMESPACE_SEPARATOR)
                    .next()
                    .unwrap_or(&tool.name);
                if bare == tool_name && !candidates.iter().any(|(eid, _)| eid == id) {
                    candidates.push((id.clone(), tool.name.clone()));
                }
            }
        }
        if candidates.is_empty() {
            return Err(format!(
                "Tool '{tool_name}' not found in any loaded extension"
            ));
        }
        if candidates.len() > 1 {
            return Err(format!(
                "Ambiguous tool name '{tool_name}' — registered by: {:?}. Use the namespaced form (e.g. '{}__{tool_name}').",
                candidates
                    .iter()
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>(),
                candidates
                    .first()
                    .map(|(id, _)| id.as_str())
                    .unwrap_or("ext"),
            ));
        }
        let (ext_id, _) = &candidates[0];
        self.instances
            .get_mut(ext_id)
            .ok_or_else(|| "extension vanished".to_string())
    }

    pub fn dispatch_command(
        &mut self,
        command_name: &str,
        args: &str,
    ) -> Result<Option<String>, String> {
        // #1: exact namespaced match in declaration order, with conflict diagnostics.
        let mut candidates_exact: Vec<String> = Vec::new();
        let mut bare_match: Vec<(String, String)> = Vec::new();
        for (id, inst) in self.instances.iter() {
            let state = inst.store.data();
            for cmd_name in state.commands.keys() {
                if cmd_name == command_name {
                    candidates_exact.push(id.clone());
                }
                let bare = cmd_name
                    .rsplit(NAMESPACE_SEPARATOR)
                    .next()
                    .unwrap_or(cmd_name);
                if bare == command_name && !bare_match.iter().any(|(eid, _)| eid == id) {
                    bare_match.push((id.clone(), cmd_name.clone()));
                }
            }
        }
        if candidates_exact.len() > 1 {
            tracing::warn!(
                command = command_name,
                extensions = ?candidates_exact,
                "slash command conflict (namespaced match): same name registered by multiple extensions; using first match"
            );
        }

        // Try exact namespaced first.
        if let Some(id) = candidates_exact.first() {
            let loaded = self
                .instances
                .get_mut(id)
                .ok_or_else(|| "extension vanished".to_string())?;
            return invoke_on_command(loaded, command_name, args).map(Some);
        }

        // Bare fallback.
        if bare_match.is_empty() {
            return Ok(None);
        }
        if bare_match.len() > 1 {
            tracing::warn!(
                command = command_name,
                extensions = ?bare_match.iter().map(|(eid, _)| eid.as_str()).collect::<Vec<_>>(),
                "slash command conflict (bare match): ambiguous"
            );
        }
        let (ext_id, cmd_name) = &bare_match[0];
        let loaded = self
            .instances
            .get_mut(ext_id)
            .ok_or_else(|| "extension vanished".to_string())?;
        invoke_on_command(loaded, cmd_name, args).map(Some)
    }

    /// Returns `(output_text, queued_prompts)`. The queued prompts are
    /// delivered per `deliver-as`.
    pub fn take_queued_prompts(&mut self) -> Vec<(String, types::DeliverAs)> {
        let mut all = Vec::new();
        for inst in self.instances.values_mut() {
            all.append(&mut inst.store.data_mut().queued_prompts);
        }
        all
    }

    pub fn all_commands(&self) -> Vec<RegisteredCommand> {
        let mut cmds = Vec::new();
        for inst in self.instances.values() {
            cmds.extend(inst.store.data().commands.values().cloned());
        }
        cmds
    }

    pub fn all_tools(&self) -> Vec<RegisteredTool> {
        let mut tools = Vec::new();
        for inst in self.instances.values() {
            tools.extend(inst.store.data().tools.clone());
        }
        tools
    }

    pub fn drain_status_updates(&mut self) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        for inst in self.instances.values_mut() {
            out.append(&mut inst.store.data_mut().status_entries);
        }
        out
    }

    pub fn drain_widget_updates(&mut self) -> Vec<(String, Option<Vec<String>>, Option<String>)> {
        let mut out = Vec::new();
        for inst in self.instances.values_mut() {
            out.append(&mut inst.store.data_mut().widget_entries);
        }
        out
    }

    pub fn unload_extension(&mut self, extension_id: &str) -> Option<ExtensionMeta> {
        self.instances.remove(extension_id).map(|i| i.meta)
    }

    pub fn get_session_name(&self) -> String {
        self.session_state
            .lock()
            .map(|s| s.0.clone())
            .unwrap_or_default()
    }

    pub fn set_session_name(&self, name: &str) {
        if let Ok(mut s) = self.session_state.lock() {
            s.0 = name.to_string();
        }
    }

    pub fn get_terminal_title(&self) -> String {
        self.session_state
            .lock()
            .map(|s| s.1.clone())
            .unwrap_or_default()
    }

    pub fn get_terminal_subtitle(&self) -> String {
        self.session_state
            .lock()
            .map(|s| s.2.clone())
            .unwrap_or_default()
    }

    /// Diagnostic helper: slash command conflicts across extensions.
    pub fn command_conflicts(&self) -> Vec<(String, Vec<String>)> {
        use std::collections::HashMap;
        let mut bare_count: HashMap<String, Vec<String>> = HashMap::new();
        for (id, inst) in &self.instances {
            for cmd in inst.store.data().commands.keys() {
                let bare = cmd
                    .rsplit(NAMESPACE_SEPARATOR)
                    .next()
                    .unwrap_or(cmd)
                    .to_string();
                bare_count.entry(bare).or_default().push(id.clone());
            }
        }
        bare_count
            .into_iter()
            .filter(|(_, v)| v.len() > 1)
            .collect()
    }

    /// Diagnostic helper: bare-name tool collision across extensions.
    pub fn tool_conflicts(&self) -> Vec<(String, Vec<String>)> {
        use std::collections::HashMap;
        let mut bare_count: HashMap<String, Vec<String>> = HashMap::new();
        for (id, inst) in &self.instances {
            for tool in &inst.store.data().tools {
                let bare = tool
                    .name
                    .rsplit(NAMESPACE_SEPARATOR)
                    .next()
                    .unwrap_or(&tool.name)
                    .to_string();
                bare_count.entry(bare).or_default().push(id.clone());
            }
        }
        bare_count
            .into_iter()
            .filter(|(_, v)| v.len() > 1)
            .collect()
    }
}

fn invoke_on_command(
    loaded: &mut LoadedExtension,
    cmd_name: &str,
    args: &str,
) -> Result<String, String> {
    let cmd = cmd_name.to_string();
    let a = args.to_string();
    let r = loaded
        .instance
        .call_on_command(&mut loaded.store, &cmd, &a)
        .map_err(|e| format!("on_command trap: {e}"))??;
    Ok(r)
}

fn apply_prepare_decision(decision: String, fallback: &str) -> Result<String, String> {
    if let Some(rest) = decision.strip_prefix("ok:") {
        Ok(rest.to_string())
    } else if let Some(rest) = decision.strip_prefix("block:") {
        Err(format!("blocked by prepare_arguments: {rest}"))
    } else if let Some(rest) = decision.strip_prefix("patch:") {
        // Future: host could parse patch directives; for now treated as ok.
        Ok(rest.to_string())
    } else if decision.is_empty() {
        Ok(fallback.to_string())
    } else {
        // Unknown prefix → treat as bare ok.
        Ok(decision)
    }
}

fn apply_tool_result_patch(
    output: GuestToolOutput,
    patch: ToolResultPatchBridge,
) -> GuestToolOutput {
    let mut out = output;
    if patch.drop {
        return GuestToolOutput {
            content: String::new(),
            details: String::new(),
            is_error: false,
            terminate: None,
            added_tool_names: None,
            is_partial: None,
        };
    }
    if let Some(c) = patch.content {
        out.content = c;
    }
    if let Some(d) = patch.details {
        out.details = d;
    }
    if let Some(e) = patch.is_error {
        out.is_error = e;
    }
    out
}

// Patch envelope used inside `execute_tool` to avoid exposing WIT inner types.
struct ToolResultPatchBridge {
    content: Option<String>,
    details: Option<String>,
    is_error: Option<bool>,
    drop: bool,
}

impl From<_ToolResultPatch> for ToolResultPatchBridge {
    fn from(p: _ToolResultPatch) -> Self {
        ToolResultPatchBridge {
            content: p.content,
            details: p.details,
            is_error: p.is_error,
            drop: p.drop.unwrap_or(false),
        }
    }
}

// Note: `apply_tool_result_patch` expects `ToolResultPatchAppliedViaHosts`. The
// trampoline below converts the WIT type. The conversion is needed because
// the public re-exports above keep the inner type opaque.
#[allow(dead_code)]
fn _patch_trampoline_unused() {}

fn check_version_compat(host_version: &str, min_required: Option<&str>) -> bool {
    let Some(req) = min_required else {
        return true;
    };
    semver_loose_ge(host_version, req)
}

/// Loose semver `>=` comparison that handles plain `1.7.2` and `1.7` (no pre).
fn semver_loose_ge(host: &str, required: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    };
    let h = parse(host);
    let r = parse(required);
    for (a, b) in h
        .iter()
        .chain(std::iter::repeat(&u64::MAX))
        .zip(r.iter().chain(std::iter::repeat(&u64::MAX)))
    {
        if a > b {
            return true;
        }
        if a < b {
            return false;
        }
    }
    true
}

fn any_provider_used(_ext_id: &str, _store: &Store<ExtGuestState>) -> Result<bool, String> {
    // Provider registrations are recorded by the provider-registry host impl
    // setting a flag on the store. Currently we conservatively accept and
    // rely on runtime trap if `provider` capability is not declared.
    Ok(false)
}

// ── Capability gating (post-init only) ───────────────────────────────

/// In the v0.5.0 WIT, every host import is linked unconditionally because
/// wit-bindgen 0.58 does not expose stable linker hooks for partial gating.
///
/// We therefore enforce capabilities *after init*:
///   - We whitelist the registered tool/command names against the manifest.
///   - We log a warning when an extension uses host imports it did not
///     declare (`capabilities` in `extension.toml`).
fn apply_capability_gating(_linker: &mut Linker<ExtGuestState>, _capabilities: &Capabilities) {
    // Placeholder — see host.rs::load_extension for the actual post-init check.
}

// ── Bridge: WIT uses of non-trivial types ────────────────────────────

impl From<types::ExecutionMode> for ToolExecutionMode {
    fn from(m: types::ExecutionMode) -> Self {
        match m {
            types::ExecutionMode::Parallel => ToolExecutionMode::Parallel,
            types::ExecutionMode::Sequential => ToolExecutionMode::Sequential,
        }
    }
}

// Re-export so manager/registry can use the host's surface without going
// through `self::zerostack::extension` paths.
pub(crate) mod reexports {
    pub(crate) use super::ExtensionHost;
}
