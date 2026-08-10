//! Test echo extension — v0.5.0 fixtures implement every optional export
//! as a no-op; the host detects traps and treats no-op as "no event handler".

wit_bindgen::generate!({
    world: "extension-world",
    path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../../crates/extension-api/wit"),
});

// `ToolOutput`, `ToolCallDecision`, `ToolResultPatch` are in the prelude.
// `ExecutionMode`, `DeliverAs`, `CancelAction`, `LogLevel` live in `types`.
use crate::zerostack::extension::types::{CancelAction, DeliverAs, ExecutionMode, LogLevel};

struct EchoExtension;

impl Guest for EchoExtension {
    fn init() -> Result<(), String> {
        crate::zerostack::extension::tool_registry::register_tool(
            &crate::zerostack::extension::tool_registry::ToolDefinition {
                name: "echo".into(),
                label: "Echo".into(),
                description: "Echo back the input message.".into(),
                parameters_schema:
                    r#"{"type":"object","properties":{"message":{"type":"string","description":"Message to echo"}},"required":["message"]}"#
                        .into(),
                prompt_snippet: Some("Use for sanity-checking Wasm RPC.".into()),
                prompt_guidelines: Some(vec![
                    "Echo is a no-op test tool; do not use in production tasks.".into(),
                ]),
                execution_mode: Some(ExecutionMode::Parallel),
                deferred: Some(false),
            },
        )
        .map_err(|e| format!("register-tool failed: {e}"))?;
        Ok(())
    }

    fn tool_execute(_name: String, params_json: String) -> Result<ToolOutput, String> {
        let ctx = crate::zerostack::extension::extension_context::get_context();
        Ok(ToolOutput {
            content: format!(
                "echo: {params_json}\ncwd: {}\nsession: {}\nmodel: {}\ntrusted: {}\nhas-ui: {}",
                ctx.cwd, ctx.session_id, ctx.model_name, ctx.project_trusted, ctx.has_ui,
            ),
            details: "{}".into(),
            is_error: false,
            terminate: None,
            added_tool_names: None,
            is_partial: None,
        })
    }

    fn on_command(_name: String, _args: String) -> Result<String, String> {
        Ok(String::new())
    }

    fn prepare_arguments(_name: String, args_json: String) -> Result<String, String> {
        Ok(format!("ok:{args_json}"))
    }

    fn session_start() -> Result<(), String> {
        Ok(())
    }
    fn session_shutdown() -> Result<(), String> {
        Ok(())
    }

    // ── v0.5.0 event hooks — no-op defaults ──
    fn on_tool_call(
        _name: String,
        _call_id: String,
        _input_json: String,
    ) -> Result<ToolCallDecision, String> {
        Ok(ToolCallDecision {
            block: None,
            reason: None,
            new_input_json: None,
        })
    }
    fn on_tool_result(
        _name: String,
        _call_id: String,
        _input_json: String,
        _content: String,
        _details: String,
        _is_error: bool,
    ) -> Result<ToolResultPatch, String> {
        Ok(ToolResultPatch {
            content: None,
            details: None,
            is_error: None,
            drop: None,
        })
    }
    fn on_user_bash(_command: String, _cwd: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_set_session_name(_name: String) -> Result<bool, String> {
        Ok(false)
    }
    fn on_session_before_compact(_reason: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_session_compacted(_reason: String, _summary: String) -> Result<(), String> {
        Ok(())
    }
    fn on_context(_messages_json: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_before_agent_start(_prompt: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_input(_text: String, _source: String) -> Result<String, String> {
        Ok(String::new())
    }
    fn on_message_update(_message_json: String) -> Result<(), String> {
        Ok(())
    }
    fn on_event(_name: String, _payload_json: String) -> Result<(), String> {
        Ok(())
    }
    fn init_async() -> Result<(), String> {
        Ok(())
    }
}

export!(EchoExtension);
