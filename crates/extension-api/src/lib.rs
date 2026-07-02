//! # zerostack Extension API
//!
//! This crate provides the WIT interface for building zerostack extensions
//! compiled to `wasm32-wasip2`.
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
//! serde = { version = "1", features = ["derive"] }
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
//!     world: "extension-world",
//!     path: zerostack_extension_api::WIT_DIR,
//! });
//!
//! use crate::zerostack::extension::tool_registry::ToolDefinition;
//! use crate::zerostack::extension::types::ToolOutput;
//! use exports::zerostack::extension::extension_world::Guest;
//!
//! struct MyExtension;
//!
//! impl Guest for MyExtension {
//!     fn init() -> Result<(), String> {
//!         crate::zerostack::extension::tool_registry::register_tool(&ToolDefinition {
//!             name: "my_tool".into(),
//!             label: "My Tool".into(),
//!             description: "Does something useful".into(),
//!             parameters_schema: r#"{"type":"object","properties":{}}"#.into(),
//!             prompt_snippet: None,
//!             prompt_guidelines: vec![],
//!         });
//!         Ok(())
//!     }
//!
//!     fn tool_execute(
//!         _name: String,
//!         _params_json: String,
//!     ) -> Result<ToolOutput, String> {
//!         Ok(ToolOutput {
//!             content: "done".into(),
//!             details: "{}".into(),
//!             is_error: false,
//!         })
//!     }
//!
//!     fn on_command(_name: String, _args: String) -> Result<String, String> {
//!         Ok("".into())
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
