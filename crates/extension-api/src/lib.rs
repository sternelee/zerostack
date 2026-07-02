//! # zerostack Extension API
//!
//! This crate provides the WIT interface and helper types for building
//! zerostack extensions compiled to `wasm32-wasip2`.
//!
//! ## Quick Start (for extension authors)
//!
//! ```toml
//! # Cargo.toml
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! zerostack-extension-api = "0.1"
//! wit-bindgen = "0.58"
//! serde = "1"
//! serde_json = "1"
//!
//! [profile.release]
//! opt-level = "s"
//! lto = true
//! strip = true
//! ```
//!
//! ```rust,ignore
//! // src/lib.rs
//! wit_bindgen::generate!({
//!     world: "extension",
//!     path: zerostack_extension_api::WIT_DIR,
//! });
//!
//! use exports::zerostack::extension::extension::Guest;
//!
//! struct MyExtension;
//!
//! impl Guest for MyExtension {
//!     fn init() -> Result<(), String> {
//!         zerostack_extension_api::tool_registry::register_tool(
//!             &zerostack_extension_api::ToolDefinition {
//!                 name: "my_tool".into(),
//!                 label: "My Tool".into(),
//!                 description: "Does something useful".into(),
//!                 parameters_schema: r#"{"type":"object","properties":{}}"#.into(),
//!                 prompt_snippet: None,
//!                 prompt_guidelines: vec![],
//!             },
//!         )?;
//!         Ok(())
//!     }
//!
//!     fn tool_execute(
//!         name: String,
//!         params_json: String,
//!     ) -> Result<zerostack_extension_api::ToolOutput, String> {
//!         Ok(zerostack_extension_api::ToolOutput {
//!             content: format!("executed: {name}"),
//!             details: "{}".into(),
//!             is_error: false,
//!         })
//!     }
//! }
//!
//! export!(MyExtension);
//! ```

pub use serde;
pub use serde_json;
pub use wit_bindgen;

/// Path to the bundled WIT directory. Pass this to
/// `wit_bindgen::generate!({ path: zerostack_extension_api::WIT_DIR, ... })`
/// in your extension's crate root.
pub const WIT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/wit");

// Thin wrapper types that mirror the WIT-generated types, for use in
// host-side code and documentation.

/// Tool registration definition sent from extension to host.
pub struct ToolDefinition {
    pub name: String,
    pub label: String,
    pub description: String,
    pub parameters_schema: String,
    pub prompt_snippet: Option<String>,
    pub prompt_guidelines: Vec<String>,
}

/// Result of executing an extension tool.
pub struct ToolOutput {
    pub content: String,
    pub details: String,
    pub is_error: bool,
}

/// Extension's decision on intercepting a tool call.
pub enum ToolAction {
    Allow,
    Block(String),
    Modify(String),
}

/// Severity level for UI notifications.
pub enum NotifyLevel {
    Info,
    Warning,
    Error,
    Success,
}

/// Shell command execution result.
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// HTTP response from extension host calls.
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Snapshot of the host environment passed with every event.
pub struct ContextInfo {
    pub cwd: String,
    pub session_id: String,
    pub mode: String,
    pub has_ui: bool,
}
