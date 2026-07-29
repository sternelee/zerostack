//! permission-gate extension for zerostack.
//!
//! Demonstrates the v0.5.0 `on-tool-call` event hook. The extension rejects
//! `bash` tool calls whose `command` arguments look like destructive
//! operations (`rm -rf /`, `mkfs`, `dd of=/dev/...`).

wit_bindgen::generate!({
    world: "extension-world",
    path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../../crates/extension-api/wit"),
});

use crate::zerostack::extension::tool_registry::ToolDefinition;
// `ToolCallDecision`, `ToolOutput`, `ToolResultPatch`, `DeliverAs`,
// `ExecutionMode` come from the wit-bindgen prelude.

struct PermissionGate;

impl Guest for PermissionGate {
    fn init() -> Result<(), String> {
        crate::zerostack::extension::tool_registry::register_tool(&ToolDefinition {
            name: "permission_audit".into(),
            label: "Permission Audit".into(),
            description:
                "Returns the list of tools the current project has been granted access to.".into(),
            parameters_schema: r#"{"type":"object","properties":{},"required":[]}"#.into(),
            prompt_snippet: Some("Reports capability grants for the active project.".into()),
            prompt_guidelines: Some(vec![
                "Use this when the user asks 'which tools can I use in this project?'.".into(),
            ]),
            execution_mode: None,
            deferred: Some(false),
        })
        .map_err(|e| format!("register-tool failed: {e}"))?;
        Ok(())
    }

    fn tool_execute(name: String, _params_json: String) -> Result<ToolOutput, String> {
        if !name.ends_with("permission_audit") {
            return Err(format!("unknown tool: {name}"));
        }
        let ctx = crate::zerostack::extension::extension_context::get_context();
        Ok(ToolOutput {
            content: format!(
                "permission-audit: project_trusted={}, has_ui={}",
                ctx.project_trusted, ctx.has_ui
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

    fn on_tool_call(
        tool_name: String,
        _call_id: String,
        input_json: String,
    ) -> Result<ToolCallDecision, String> {
        if tool_name != "bash" {
            return Ok(empty_decision());
        }
        // Look for destructive patterns in the bash args.
        let dangerous = ["rm -rf /", "rm -rf /*", "mkfs.", "dd if=", ":(){ :|:& };:"];
        for needle in dangerous {
            if input_json.contains(needle) {
                return Ok(block(needle));
            }
        }
        Ok(empty_decision())
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

    fn prepare_arguments(_name: String, args_json: String) -> Result<String, String> {
        Ok(format!("ok:{args_json}"))
    }

    fn session_start() -> Result<(), String> {
        Ok(())
    }
    fn session_shutdown() -> Result<(), String> {
        Ok(())
    }
    fn init_async() -> Result<(), String> {
        Ok(())
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
}

fn empty_decision() -> ToolCallDecision {
    ToolCallDecision {
        block: None,
        reason: None,
        new_input_json: None,
    }
}

fn block(needle: &str) -> ToolCallDecision {
    ToolCallDecision {
        block: Some(true),
        reason: Some(format!(
            "permission-gate blocked bash: matches forbidden pattern `{needle}`"
        )),
        new_input_json: None,
    }
}

export!(PermissionGate);
