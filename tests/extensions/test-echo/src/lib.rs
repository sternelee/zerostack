//! Test echo extension — compiled to wasm32-wasip2 as a zerostack component.
//!
//! Registers a single tool, `echo`, that returns the JSON parameters it received
//! plus the current extension context.

wit_bindgen::generate!({
    world: "extension-world",
    path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../../crates/extension-api/wit"),
});

use crate::zerostack::extension::tool_registry::ToolDefinition;

struct EchoExtension;

impl Guest for EchoExtension {
    fn init() -> Result<(), String> {
        let _ = crate::zerostack::extension::tool_registry::register_tool(&ToolDefinition {
            name: "echo".into(),
            label: "Echo".into(),
            description: "Echo back the input message.".into(),
            parameters_schema: r#"{"type":"object","properties":{"message":{"type":"string","description":"Message to echo"}},"required":["message"]}"#.into(),
            prompt_snippet: None,
            prompt_guidelines: vec![],
        });
        Ok(())
    }

    fn tool_execute(_name: String, params_json: String) -> Result<ToolOutput, String> {
        let ctx = crate::zerostack::extension::extension_context::get_context();
        Ok(ToolOutput {
            content: format!(
                "echo: {params_json}\ncwd: {}\nsession: {}\nmodel: {}\ntrusted: {}",
                ctx.cwd, ctx.session_id, ctx.model_name, ctx.project_trusted,
            ),
            details: "{}".into(),
            is_error: false,
        })
    }

    fn on_command(_name: String, _args: String) -> Result<String, String> {
        Ok("".into())
    }

    fn session_start() -> Result<(), String> {
        Ok(())
    }

    fn session_shutdown() -> Result<(), String> {
        Ok(())
    }
}

export!(EchoExtension);
