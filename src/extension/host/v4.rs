//! v0.4.0 host bindings for legacy extensions.
//!
//! Components compiled against `zerostack:extension@0.4.0` (the original WIT
//! that lived at `wit/extension-v0.2.0.wit`) import interfaces at the `0.4.0`
//! package version. New v0.5.0 components import at `0.5.0`. wasmtime's
//! linker matches each component's imports against the registered world
//! versions, so we register both worlds to support installed extensions that
//! haven't been recompiled.
//!
//! v0.4.0 has these host imports (delegated to the same store as v0.5.0):
//!   - `tool-registry`, `command-registry`
//!   - `extension-context`, `trigger-prompt`, `session-control`

use crate::extension::host::{
    ExtGuestState, GuestToolOutput, NAMESPACE_SEPARATOR, namespaced_tool_name,
};

wasmtime::component::bindgen!({
    path: "crates/extension-api/wit/v0.4.0/extension.wit",
    world: "extension-world",
});

const V4_NS_PREFIX: &str = "v4__";

struct V4GuestStateShim<'a> {
    inner: &'a mut ExtGuestState,
}

impl<'a> V4GuestStateShim<'a> {
    fn namespaced(&self, name: &str) -> String {
        format!("{V4_NS_PREFIX}{name}")
    }
}

impl self::zerostack::extension::types::Host for ExtGuestState {}

pub(crate) fn add_v4_linker(
    linker: &mut wasmtime::component::Linker<ExtGuestState>,
) -> Result<(), String> {
    self::ExtensionWorld::add_to_linker::<ExtGuestState, wasmtime::component::HasSelf<ExtGuestState>>(
        linker,
        |state| state,
    )
    .map_err(|e| format!("failed to add v0.4.0 world to linker: {e}"))
}

// ── Host imports (v0.4.0) ───────────────────────────────────────────

impl self::zerostack::extension::tool_registry::Host for ExtGuestState {
    fn register_tool(
        &mut self,
        def: self::zerostack::extension::tool_registry::ToolDefinition,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let bare = def.name;
        // Legacy tools are namespaced under `v4__<name>` so they cannot
        // collide with v0.5.0 tools registered by newer extensions.
        let namespaced = format!("{V4_NS_PREFIX}{bare}");
        self.tools.push(crate::extension::RegisteredTool {
            name: namespaced.clone(),
            label: def.label,
            description: def.description,
            parameters_schema: def.parameters_schema,
            prompt_snippet: def.prompt_snippet,
            prompt_guidelines: def.prompt_guidelines,
            extension_id: format!("{}/v4", self.extension_id),
            execution_mode: crate::extension::ToolExecutionMode::Parallel,
            loading_mode: crate::extension::ToolLoadingMode::Eager,
        });
        tracing::info!(legacy_tool = %namespaced, "registered tool via v0.4.0 world");
        Ok(())
    }

    fn unregister_tool(
        &mut self,
        name: wasmtime::component::__internal::String,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let namespaced = format!("{V4_NS_PREFIX}{name}");
        self.tools.retain(|t| t.name != namespaced);
        Ok(())
    }
}

impl self::zerostack::extension::command_registry::Host for ExtGuestState {
    fn register_command(
        &mut self,
        def: self::zerostack::extension::command_registry::CommandDefinition,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let bare = def.name;
        let namespaced = format!("{V4_NS_PREFIX}{bare}");
        self.commands.insert(
            namespaced.clone(),
            crate::extension::RegisteredCommand {
                name: namespaced,
                description: def.description,
                extension_id: format!("{}/v4", self.extension_id),
            },
        );
        tracing::info!(legacy_cmd = %bare, "registered slash command via v0.4.0 world");
        Ok(())
    }

    fn unregister_command(
        &mut self,
        name: wasmtime::component::__internal::String,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let namespaced = format!("{V4_NS_PREFIX}{name}");
        self.commands.remove(&namespaced);
        Ok(())
    }
}

impl self::zerostack::extension::trigger_prompt::Host for ExtGuestState {
    fn trigger_prompt(
        &mut self,
        prompt: wasmtime::component::__internal::String,
        deliver_as: wasmtime::component::__internal::String,
    ) -> Result<(), wasmtime::component::__internal::String> {
        let mapped = match deliver_as.as_str() {
            "steer" => crate::extension::host::types::DeliverAs::Steer,
            "nextTurn" | "next-turn" => crate::extension::host::types::DeliverAs::NextTurn,
            _ => crate::extension::host::types::DeliverAs::FollowUp,
        };
        self.queued_prompts.push((prompt, mapped));
        Ok(())
    }
}

impl self::zerostack::extension::extension_context::Host for ExtGuestState {
    fn get_context(&mut self) -> self::zerostack::extension::extension_context::ExtensionInfo {
        let info = &self.host_context;
        self::zerostack::extension::extension_context::ExtensionInfo {
            cwd: info.cwd.clone(),
            session_id: info.session_id.clone(),
            model_name: info.model_name.clone(),
            project_trusted: info.project_trusted,
        }
    }
}

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
        if let Ok(mut s) = self.session_state.lock() {
            s.0 = name;
        }
        Ok(())
    }

    fn set_terminal_title(&mut self, title: wasmtime::component::__internal::String) {
        // Same as v0.5.0: store on session_state, don't write to stdout.
        if let Ok(mut s) = self.session_state.lock() {
            s.1 = title;
        }
    }
}

// Suppress unused warnings for items declared via the inner bindgen.
#[allow(dead_code)]
fn _v4_force_export_use<'a>(
    _e: &ExtensionWorld,
    _ext: &'a mut ExtGuestState,
    _t: &'a GuestToolOutput,
) {
    let _ = NAMESPACE_SEPARATOR;
    let _ = namespaced_tool_name;
}
