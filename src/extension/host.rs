//! Extension host — wasmtime component-model runtime for extensions.
//!
//! Uses the WIT world defined in `crates/extension-api/wit/extension-v0.2.0.wit`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wasmtime::component::Linker;
use wasmtime::{Config, Engine, Store};

use crate::extension::loader::Capabilities;
use crate::extension::{ExtensionId, ExtensionMeta, RegisteredCommand, RegisteredTool};

// Generate host bindings from the WIT world.
wasmtime::component::bindgen!({
    path: "crates/extension-api/wit/extension-v0.2.0.wit",
    world: "extension-world",
});

use self::zerostack::extension::{
    command_registry::CommandDefinition, extension_context::ExtensionInfo,
    tool_registry::ToolDefinition, types::ToolOutput as GuestToolOutput,
};

const GUEST_FUEL: u64 = 100_000_000;
const GUEST_MEMORY_LIMIT: usize = 64 * 1024 * 1024; // 64 MiB
const GUEST_CALL_TIMEOUT: Duration = Duration::from_secs(30);
const NAMESPACE_SEPARATOR: &str = "__";

// ── ExtensionHost ──────────────────────────────────────────────

pub(crate) struct ExtensionHost {
    engine: Engine,
    instances: HashMap<ExtensionId, LoadedExtension>,
    /// Shared session state: (session_name, terminal_title)
    session_state: Arc<Mutex<(String, String)>>,
}

struct LoadedExtension {
    store: Store<ExtGuestState>,
    instance: ExtensionWorld,
    meta: ExtensionMeta,
}

pub(crate) struct ExtGuestState {
    pub extension_id: ExtensionId,
    pub tools: Vec<RegisteredTool>,
    pub commands: HashMap<String, RegisteredCommand>,
    /// Namespace prefix for this extension's tools/commands.
    pub ns_prefix: String,
    /// Host-side context (cwd, session state, etc.) — updated per-call.
    pub host_context: ExtensionInfo,
    /// Queued prompts from trigger-prompt calls (drained after command dispatch).
    pub queued_prompts: Vec<String>,
    /// Shared session state: (session_name, terminal_title)
    pub session_state: Arc<Mutex<(String, String)>>,
    wasi_ctx: wasmtime_wasi::WasiCtx,
    wasi_table: wasmtime_wasi::ResourceTable,
}

impl ExtGuestState {
    fn new(extension_id: &str, session_state: Arc<Mutex<(String, String)>>) -> Self {
        let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
            .inherit_stdout()
            .inherit_stderr()
            .build();
        Self {
            extension_id: extension_id.to_string(),
            tools: Vec::new(),
            commands: HashMap::new(),
            ns_prefix: sanitize_id_for_namespace(extension_id),
            host_context: ExtensionInfo {
                cwd: String::new(),
                session_id: String::new(),
                model_name: String::new(),
                project_trusted: false,
            },
            queued_prompts: Vec::new(),
            session_state,
            wasi_ctx,
            wasi_table: wasmtime_wasi::ResourceTable::new(),
        }
    }
}

fn sanitize_id_for_namespace(id: &str) -> String {
    id.replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_")
}

/// Build the namespaced tool name: `{extension_id}__{tool_name}`.
/// Bare names (no `__`) are also resolved as a fallback when unambiguous.
pub(crate) fn namespaced_tool_name(ext_id: &str, tool_name: &str) -> String {
    format!("{ext_id}{NAMESPACE_SEPARATOR}{tool_name}")
}

impl wasmtime_wasi::WasiView for ExtGuestState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

// ── Host impl: tool_registry ───────────────────────────────────

impl self::zerostack::extension::tool_registry::Host for ExtGuestState {
    fn register_tool(
        &mut self,
        def: ToolDefinition,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let name: String = def.name;
        let namespaced = namespaced_tool_name(&self.extension_id, &name);

        // Reject duplicate bare name if another extension already registered it.
        self.tools.push(RegisteredTool {
            name: namespaced,
            label: def.label,
            description: def.description,
            parameters_schema: def.parameters_schema,
            prompt_snippet: def.prompt_snippet,
            prompt_guidelines: def.prompt_guidelines.into_iter().collect(),
            extension_id: self.extension_id.clone(),
        });
        Ok(())
    }

    fn unregister_tool(
        &mut self,
        name: wasmtime::component::__internal::String,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let namespaced = namespaced_tool_name(&self.extension_id, &name);
        self.tools.retain(|t| t.name != namespaced);
        Ok(())
    }
}

// ── Host impl: command_registry ──────────────────────────────────

impl self::zerostack::extension::command_registry::Host for ExtGuestState {
    fn register_command(
        &mut self,
        def: CommandDefinition,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let name: String = def.name;
        let namespaced = namespaced_tool_name(&self.extension_id, &name);
        self.commands.insert(
            namespaced.clone(),
            RegisteredCommand {
                name: namespaced,
                description: def.description,
                extension_id: self.extension_id.clone(),
            },
        );
        Ok(())
    }

    fn unregister_command(
        &mut self,
        name: wasmtime::component::__internal::String,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let namespaced = namespaced_tool_name(&self.extension_id, &name);
        self.commands.remove(&namespaced);
        Ok(())
    }
}

// ── Host impl: extension_context ────────────────────────────────

impl self::zerostack::extension::extension_context::Host for ExtGuestState {
    fn get_context(&mut self) -> self::zerostack::extension::extension_context::ExtensionInfo {
        self.host_context.clone()
    }
}

// ── Host impl: trigger_prompt ──────────────────────────────────

impl self::zerostack::extension::trigger_prompt::Host for ExtGuestState {
    fn trigger_prompt(
        &mut self,
        prompt: wasmtime::component::__internal::String,
        _deliver_as: wasmtime::component::__internal::String,
    ) -> Result<(), wasmtime::component::__internal::String> {
        self.queued_prompts.push(prompt);
        Ok(())
    }
}

impl self::zerostack::extension::types::Host for ExtGuestState {}

// ── Host impl: session_control ────────────────────────────────

impl self::zerostack::extension::session_control::Host for ExtGuestState {
    fn get_session_name(&mut self) -> wasmtime::component::__internal::String {
        self.session_state
            .lock()
            .map(|s| s.0.clone())
            .unwrap_or_default()
    }

    fn set_session_name(
        &mut self,
        name: wasmtime::component::__internal::String,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let name: String = name;
        if let Ok(mut s) = self.session_state.lock() {
            s.0 = name;
        }
        Ok(())
    }

    fn set_terminal_title(&mut self, title: wasmtime::component::__internal::String) {
        let title: String = title;
        // Save the title so we can restore it later.
        if let Ok(mut s) = self.session_state.lock() {
            s.1 = title.clone();
        }
        // ANSI OSC escape sequence for terminal title.
        print!("\x1b]0;{title}\x07");
    }
}

// ── ExtensionHost ──────────────────────────────────────────────

impl ExtensionHost {
    pub fn new() -> Result<Self, String> {
        let mut config = Config::default();
        config.consume_fuel(true);
        config.max_wasm_stack(512 * 1024);

        let engine = Engine::new(&config).map_err(|e| e.to_string())?;
        Ok(Self {
            engine,
            instances: HashMap::new(),
            session_state: Arc::new(Mutex::new((String::new(), String::new()))),
        })
    }

    /// Load an extension from a compiled .wasm component.
    /// Only links host imports declared in `capabilities`.
    pub fn load_extension(
        &mut self,
        extension_id: &str,
        wasm_path: &Path,
        capabilities: &Capabilities,
        cwd: &str,
        session_id: &str,
        model_name: &str,
        project_trusted: bool,
    ) -> Result<ExtensionMeta, String> {
        let wasm_bytes =
            std::fs::read(wasm_path).map_err(|e| format!("failed to read {wasm_path:?}: {e}"))?;

        let component = wasmtime::component::Component::from_binary(&self.engine, &wasm_bytes)
            .map_err(|e| format!("failed to compile component: {e}"))?;

        let mut linker = Linker::<ExtGuestState>::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| format!("failed to add wasi imports to linker: {e}"))?;

        // Link all extension-world imports via the world-level linker.
        // Capability violations (e.g. registering tools without declaring
        // the `tools` capability) are caught after `init()` below.
        ExtensionWorld::add_to_linker::<
            ExtGuestState,
            wasmtime::component::HasSelf<ExtGuestState>,
        >(&mut linker, |state: &mut ExtGuestState| state)
        .map_err(|e| format!("failed to add extension imports to linker: {e}"))?;

        let mut store = Store::new(
            &self.engine,
            ExtGuestState::new(extension_id, self.session_state.clone()),
        );
        store.set_fuel(GUEST_FUEL).map_err(|e| e.to_string())?;
        store.limiter(|state| state as &mut dyn wasmtime::ResourceLimiter);

        // Set initial context.
        {
            let state = store.data_mut();
            state.host_context = ExtensionInfo {
                cwd: cwd.to_string(),
                session_id: session_id.to_string(),
                model_name: model_name.to_string(),
                project_trusted,
            };
        }

        let instance = ExtensionWorld::instantiate(&mut store, &component, &linker)
            .map_err(|e| format!("instantiation failed: {e}"))?;

        instance
            .call_init(&mut store)
            .map_err(|e| format!("init trap: {e}"))?
            .map_err(|e| format!("extension init failed: {e}"))?;

        // Verify the extension didn't use imports it didn't declare.
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
            },
        );
        Ok(meta)
    }

    /// Update context for all loaded extensions (e.g. on model change).
    pub fn update_context(
        &mut self,
        cwd: &str,
        session_id: &str,
        model_name: &str,
        project_trusted: bool,
    ) {
        for (_, inst) in self.instances.iter_mut() {
            let state = inst.store.data_mut();
            state.host_context = ExtensionInfo {
                cwd: cwd.to_string(),
                session_id: session_id.to_string(),
                model_name: model_name.to_string(),
                project_trusted,
            };
        }
    }

    /// Call session_start on all extensions that export it. Errors are logged, not propagated.
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
                    tracing::warn!(extension_id = %id, error = %e, "session_start trap");
                }
            }
        }
    }

    /// Call session_shutdown on all extensions that export it. Best-effort.
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
                    tracing::warn!(extension_id = %id, error = %e, "session_shutdown trap");
                }
            }
        }
    }

    /// Execute an extension-registered tool. Accepts both namespaced
    /// (`ext__tool`) and bare (`tool`) names. Bare names only resolve
    /// when exactly one extension registered that name.
    pub fn execute_tool(
        &mut self,
        tool_name: &str,
        params_json: &str,
    ) -> Result<GuestToolOutput, String> {
        let loaded = self.resolve_tool_instance(tool_name)?;
        let tool_name_clone = tool_name.to_string();
        let params = params_json.to_string();

        loaded
            .instance
            .call_tool_execute(&mut loaded.store, &tool_name_clone, &params)
            .map_err(|e| format!("tool_execute trap: {e}"))?
            .map_err(|e| format!("tool_execute failed: {e}"))
    }

    /// Find the instance that owns `tool_name` (namespaced or bare).
    fn resolve_tool_instance(&mut self, tool_name: &str) -> Result<&mut LoadedExtension, String> {
        // First try exact namespaced match.
        for (_, inst) in self.instances.iter() {
            let state = inst.store.data();
            if state.tools.iter().any(|t| t.name == tool_name) {
                let id = state.extension_id.clone();
                return self
                    .instances
                    .get_mut(&id)
                    .ok_or_else(|| "extension vanished".to_string());
            }
        }

        // Try bare name: find all extensions that registered a tool with that bare name.
        // We need to check against the bare name (before namespacing).
        let mut candidates: Vec<(String, String)> = Vec::new(); // (ext_id, namespaced_name)
        for (id, inst) in self.instances.iter() {
            let state = inst.store.data();
            for tool in &state.tools {
                // Strip namespace prefix to get the bare name.
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
            let names: Vec<_> = candidates.iter().map(|(id, _)| id.as_str()).collect();
            return Err(format!(
                "Ambiguous tool name '{tool_name}' — registered by extensions: {names:?}. Use the namespaced form (e.g. '{}__{tool_name}') instead.",
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

    /// Dispatch a registered slash command. Same namespacing rules as tools.
    pub fn dispatch_command(
        &mut self,
        command_name: &str,
        args: &str,
    ) -> Result<Option<String>, String> {
        // Try exact namespaced match first.
        for (_, inst) in self.instances.iter() {
            if inst.store.data().commands.contains_key(command_name) {
                let id = inst.store.data().extension_id.clone();
                let loaded = self
                    .instances
                    .get_mut(&id)
                    .ok_or_else(|| "extension vanished".to_string())?;
                let cmd = command_name.to_string();
                let args_s = args.to_string();
                let result = loaded
                    .instance
                    .call_on_command(&mut loaded.store, &cmd, &args_s)
                    .map_err(|e| format!("on_command trap: {e}"))??;
                return Ok(Some(result));
            }
        }

        // Try bare match (must be unambiguous).
        let mut candidates: Vec<(String, String)> = Vec::new();
        for (id, inst) in self.instances.iter() {
            let state = inst.store.data();
            for cmd_name in state.commands.keys() {
                let bare = cmd_name
                    .rsplit(NAMESPACE_SEPARATOR)
                    .next()
                    .unwrap_or(cmd_name);
                if bare == command_name && !candidates.iter().any(|(eid, _)| eid == id) {
                    candidates.push((id.clone(), cmd_name.clone()));
                }
            }
        }

        if candidates.is_empty() {
            return Ok(None);
        }

        let (ext_id, cmd_name) = &candidates[0];
        // If multiple extensions registered the same bare name, it's ambiguous.
        // We still execute the first one rather than erroring, since slash commands
        // are user-triggered and the first-registered semantics match tool behavior.
        let loaded = self
            .instances
            .get_mut(ext_id)
            .ok_or_else(|| "extension vanished".to_string())?;
        let args_s = args.to_string();
        let result = loaded
            .instance
            .call_on_command(&mut loaded.store, cmd_name, &args_s)
            .map_err(|e| format!("on_command trap: {e}"))??;
        Ok(Some(result))
    }

    pub fn all_commands(&self) -> Vec<RegisteredCommand> {
        let mut cmds = Vec::new();
        for inst in self.instances.values() {
            cmds.extend(inst.store.data().commands.values().cloned());
        }
        cmds
    }

    /// Drain queued prompts from all extensions (called after command dispatch).
    pub fn take_queued_prompts(&mut self) -> Vec<String> {
        let mut prompts = Vec::new();
        for inst in self.instances.values_mut() {
            prompts.append(&mut inst.store.data_mut().queued_prompts);
        }
        prompts
    }

    pub fn unload_extension(&mut self, extension_id: &str) -> Option<ExtensionMeta> {
        self.instances.remove(extension_id).map(|i| i.meta)
    }

    pub fn all_tools(&self) -> Vec<RegisteredTool> {
        let mut tools = Vec::new();
        for inst in self.instances.values() {
            tools.extend(inst.store.data().tools.clone());
        }
        tools
    }

    /// Get the current session name.
    pub fn get_session_name(&self) -> String {
        self.session_state
            .lock()
            .map(|s| s.0.clone())
            .unwrap_or_default()
    }

    /// Set the session name (persisted externally).
    pub fn set_session_name(&self, name: &str) {
        if let Ok(mut s) = self.session_state.lock() {
            s.0 = name.to_string();
        }
    }

    /// Get the saved terminal title.
    pub fn get_terminal_title(&self) -> String {
        self.session_state
            .lock()
            .map(|s| s.1.clone())
            .unwrap_or_default()
    }
}

// ── Resource limiter ──────────────────────────────────────────

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
