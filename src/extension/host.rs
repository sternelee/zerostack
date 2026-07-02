//! Extension host — wasmtime component-model runtime for extensions.
//!
//! Uses the WIT world defined in `crates/extension-api/wit/extension-v0.1.0.wit`.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use wasmtime::component::{Linker, bindgen};
use wasmtime::{Config, Engine, Store};

use crate::extension::loader::Capabilities;
use crate::extension::{ExtensionId, ExtensionMeta, RegisteredCommand, RegisteredTool};

// Generate host bindings from the WIT world.
bindgen!({
    path: "crates/extension-api/wit/extension-v0.1.0.wit",
    world: "extension-world",
});

// Alias the generated world so we don't have to spell out the module path.
// The `bindgen!` macro below generates `ExtensionWorld` and the
// `zerostack::extension::*` submodules in this module's namespace.
use self::zerostack::extension::{
    command_registry::CommandDefinition, tool_registry::ToolDefinition,
    types::ToolOutput as GuestToolOutput,
};

const GUEST_FUEL: u64 = 100_000_000;
const GUEST_MEMORY_LIMIT: usize = 64 * 1024 * 1024; // 64 MiB
const GUEST_CALL_TIMEOUT: Duration = Duration::from_secs(30);

// ── ExtensionHost ──────────────────────────────────────────────

pub(crate) struct ExtensionHost {
    engine: Engine,
    instances: HashMap<ExtensionId, LoadedExtension>,
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
    wasi_ctx: wasmtime_wasi::WasiCtx,
    wasi_table: wasmtime_wasi::ResourceTable,
}

impl ExtGuestState {
    fn new(extension_id: &str) -> Self {
        let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
            .inherit_stdout()
            .inherit_stderr()
            .build();
        Self {
            extension_id: extension_id.to_string(),
            tools: Vec::new(),
            commands: HashMap::new(),
            wasi_ctx,
            wasi_table: wasmtime_wasi::ResourceTable::new(),
        }
    }
}

impl wasmtime_wasi::WasiView for ExtGuestState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

impl self::zerostack::extension::tool_registry::Host for ExtGuestState {
    fn register_tool(
        &mut self,
        def: ToolDefinition,
    ) -> Result<(), wasmtime::component::__internal::String> {
        self.tools.push(RegisteredTool {
            name: def.name.into(),
            label: def.label.into(),
            description: def.description.into(),
            parameters_schema: def.parameters_schema.into(),
            prompt_snippet: def.prompt_snippet.map(Into::into),
            prompt_guidelines: def.prompt_guidelines.into_iter().map(Into::into).collect(),
            extension_id: self.extension_id.clone(),
        });
        Ok(())
    }

    fn unregister_tool(
        &mut self,
        name: wasmtime::component::__internal::String,
    ) -> Result<(), wasmtime::component::__internal::String> {
        self.tools.retain(|t| t.name != name);
        Ok(())
    }
}

impl self::zerostack::extension::command_registry::Host for ExtGuestState {
    fn register_command(
        &mut self,
        def: CommandDefinition,
    ) -> Result<(), wasmtime::component::__internal::String> {
        self.commands.insert(
            def.name.clone().into(),
            RegisteredCommand {
                name: def.name.into(),
                description: def.description.into(),
                extension_id: self.extension_id.clone(),
            },
        );
        Ok(())
    }

    fn unregister_command(
        &mut self,
        name: wasmtime::component::__internal::String,
    ) -> Result<(), wasmtime::component::__internal::String> {
        self.commands.remove(&name);
        Ok(())
    }
}

impl self::zerostack::extension::types::Host for ExtGuestState {}

impl ExtensionHost {
    pub fn new() -> Result<Self, String> {
        let mut config = Config::default();
        config.consume_fuel(true);
        config.max_wasm_stack(512 * 1024);

        let engine = Engine::new(&config).map_err(|e| e.to_string())?;
        Ok(Self {
            engine,
            instances: HashMap::new(),
        })
    }

    /// Load an extension from a compiled .wasm component.
    pub fn load_extension(
        &mut self,
        extension_id: &str,
        wasm_path: &Path,
        _capabilities: &Capabilities,
    ) -> Result<ExtensionMeta, String> {
        let wasm_bytes =
            std::fs::read(wasm_path).map_err(|e| format!("failed to read {wasm_path:?}: {e}"))?;

        let component = wasmtime::component::Component::from_binary(&self.engine, &wasm_bytes)
            .map_err(|e| format!("failed to compile component: {e}"))?;

        let mut linker = Linker::<ExtGuestState>::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| format!("failed to add wasi imports to linker: {e}"))?;
        ExtensionWorld::add_to_linker::<ExtGuestState, wasmtime::component::HasSelf<ExtGuestState>>(
            &mut linker,
            |state: &mut ExtGuestState| state,
        )
        .map_err(|e| format!("failed to add imports to linker: {e}"))?;

        let mut store = Store::new(&self.engine, ExtGuestState::new(extension_id));
        store.set_fuel(GUEST_FUEL).map_err(|e| e.to_string())?;
        store.limiter(|state| state as &mut dyn wasmtime::ResourceLimiter);

        let instance = ExtensionWorld::instantiate(&mut store, &component, &linker)
            .map_err(|e| format!("instantiation failed: {e}"))?;

        instance
            .call_init(&mut store)
            .map_err(|e| format!("init trap: {e}"))?
            .map_err(|e| format!("extension init failed: {e}"))?;

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

    /// Execute an extension-registered tool.
    pub fn execute_tool(
        &mut self,
        extension_id: &str,
        tool_name: &str,
        params_json: &str,
    ) -> Result<GuestToolOutput, String> {
        let loaded = self
            .instances
            .get_mut(extension_id)
            .ok_or_else(|| format!("extension '{extension_id}' not loaded"))?;

        loaded
            .instance
            .call_tool_execute(&mut loaded.store, tool_name, params_json)
            .map_err(|e| format!("tool_execute trap: {e}"))?
            .map_err(|e| format!("tool_execute failed: {e}"))
    }

    /// Dispatch a registered slash command.
    pub fn dispatch_command(
        &mut self,
        command_name: &str,
        args: &str,
    ) -> Result<Option<String>, String> {
        let extension_id = self
            .instances
            .iter()
            .find(|(_, inst)| inst.store.data().commands.contains_key(command_name))
            .map(|(id, _)| id.clone());

        let Some(extension_id) = extension_id else {
            return Ok(None);
        };

        let loaded = self
            .instances
            .get_mut(&extension_id)
            .ok_or_else(|| "extension vanished".to_string())?;

        let result = loaded
            .instance
            .call_on_command(&mut loaded.store, command_name, args)
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
}

// Resource limiter implementation for the guest store.
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
