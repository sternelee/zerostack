//! Extension system — Wasm-based extensibility for zerostack.
//!
//! ## Architecture
//!
//! ```text
//! ExtensionManager
//!   ├── Loader: discovers extension.toml + .wasm files
//!   ├── ExtensionHost: wasmtime Engine + instances
//!   └── ExtensionToolWrapper: adapts guest tool → rig::tool::ToolDyn
//! ```

#![allow(dead_code)]

mod host;
mod loader;
mod manager;
pub(crate) mod registry;
mod wrapper;

#[cfg(test)]
mod tests;

/// Unique identifier for an extension instance.
pub type ExtensionId = String;

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
}

/// A slash command registered by an extension.
#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    pub name: String,
    pub description: String,
    pub extension_id: ExtensionId,
}
