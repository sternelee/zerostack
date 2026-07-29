//! Extension system — Wasm-based extensibility for zerostack.
//!
//! ## Architecture
//!
//! ```text
//! ExtensionManager
//!   ├── Loader: discovers extension.toml + .wasm files
//!   ├── ExtensionHost: wasmtime Engine + instances
//!   ├── Host impls: WIT host import surface
//!   └── ExtensionToolWrapper: adapts guest tool → rig::tool::ToolDyn
//! ```

#![allow(dead_code)]
// wasmtime::component::bindgen! generates unsafe blocks internally.
#![allow(unsafe_code)]

pub(crate) mod host;
mod host_impls;
mod loader;
pub(crate) mod manager;
pub(crate) mod registry;
mod wrapper;

#[cfg(test)]
mod tests;

/// Unique identifier for an extension instance.
pub type ExtensionId = String;

/// How an extension tool should be invoked: parallel with sibling tools or
/// sequentially to itself (stateful tools).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Parallel,
    Sequential,
}

impl Default for ToolExecutionMode {
    fn default() -> Self {
        Self::Parallel
    }
}

/// Whether a tool is a "deferred" tool. The model may opt into requesting
/// the tool by name when the relevant conversation requires it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolLoadingMode {
    /// Always exposed to the model.
    #[default]
    Eager,
    /// Only exposed when `added_tool_names` returns this name.
    Deferred,
}

/// Stores metadata for a single loaded extension.
#[derive(Debug, Clone)]
pub struct ExtensionMeta {
    pub id: ExtensionId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub tool_names: Vec<String>,
    pub command_names: Vec<String>,
    pub subscriptions: Vec<String>,
}

/// A tool registered by an extension.
#[derive(Debug, Clone)]
pub struct RegisteredTool {
    pub name: String,
    pub label: String,
    pub description: String,
    pub parameters_schema: String,
    pub prompt_snippet: Option<String>,
    pub prompt_guidelines: Vec<String>,
    pub extension_id: ExtensionId,
    pub execution_mode: ToolExecutionMode,
    pub loading_mode: ToolLoadingMode,
}

/// A slash command registered by an extension.
#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    pub name: String,
    pub description: String,
    pub extension_id: ExtensionId,
}

/// Conflict diagnostics emitted at load time (used by picker + warnings).
#[derive(Debug, Clone, Default)]
pub struct LoadDiagnostics {
    pub tool_conflicts: Vec<(String, Vec<String>)>,
    pub command_conflicts: Vec<(String, Vec<String>)>,
    pub unsupported_events: Vec<String>,
    pub warnings: Vec<String>,
}
