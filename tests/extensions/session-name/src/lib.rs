//! session-name extension — v0.5.0 with all required v0.5.0 event exports.

wit_bindgen::generate!({
    world: "extension-world",
    path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../../crates/extension-api/wit"),
});

use crate::zerostack::extension::command_registry::CommandDefinition;
use crate::zerostack::extension::tool_registry::ToolDefinition;
use crate::zerostack::extension::types::{DeliverAs, ExecutionMode};
// `ToolOutput`, `ToolCallDecision`, `ToolResultPatch` come from the
// wit-bindgen prelude.

struct SessionNameExtension;

impl Guest for SessionNameExtension {
    fn init() -> Result<(), String> {
        crate::zerostack::extension::command_registry::register_command(&CommandDefinition {
            name: "name".into(),
            description: "Show or generate a session name".into(),
            argument_hint: Some("[new name]".into()),
        })
        .map_err(|e| format!("register_command failed: {e}"))?;
        crate::zerostack::extension::tool_registry::register_tool(&ToolDefinition {
            name: "set_session_name".into(),
            label: "Set Session Name".into(),
            description:
                "Set the current session name to a short, concise title (2-5 words). \
                 Call this after generating a title for the session based on the user's request."
                    .into(),
            parameters_schema: r#"{"type":"object","properties":{"name":{"type":"string","description":"Short session title, 2-5 words"}},"required":["name"]}"#.into(),
            prompt_snippet: Some("Persist a memorable session title.".into()),
            prompt_guidelines: Some(vec![
                "Call once per session, after the user's intent is clear.".into(),
                "Keep titles <= 5 words and avoid punctuation.".into(),
            ]),
            execution_mode: Some(ExecutionMode::Sequential),
            deferred: Some(false),
        })
        .map_err(|_| "register_tool failed (name conflict)".to_string())?;
        Ok(())
    }

    fn tool_execute(name: String, params_json: String) -> Result<ToolOutput, String> {
        if !name.ends_with("set_session_name") {
            return Err(format!("unknown tool: {name}"));
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&params_json).map_err(|e| format!("invalid JSON: {e}"))?;
        let session_name = parsed["name"].as_str().unwrap_or("").trim().to_string();
        if session_name.is_empty() {
            return Ok(noop_output("No name provided; session name unchanged."));
        }
        crate::zerostack::extension::session_control::set_session_name(&session_name)
            .map_err(|e| format!("failed to set session name: {e}"))?;
        let ctx = crate::zerostack::extension::extension_context::get_context();
        let cwd_short = std::path::Path::new(&ctx.cwd)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| ctx.cwd.clone());
        let title = format!("\u{2733} {session_name} - {cwd_short}");
        crate::zerostack::extension::session_control::set_terminal_title(&title);
        Ok(noop_output(&format!("Session name set to: {session_name}")))
    }

    fn on_command(name: String, args: String) -> Result<String, String> {
        if !name.ends_with("name") {
            return Ok(String::new());
        }
        let args = args.trim();
        if !args.is_empty() {
            crate::zerostack::extension::session_control::set_session_name(args)
                .map_err(|e| format!("failed to set session name: {e}"))?;
            let ctx = crate::zerostack::extension::extension_context::get_context();
            let cwd_short = std::path::Path::new(&ctx.cwd)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| ctx.cwd.clone());
            let title = format!("\u{2733} {args} - {cwd_short}");
            crate::zerostack::extension::session_control::set_terminal_title(&title);
            return Ok(format!("Session name set to: {args}"));
        }
        let existing = crate::zerostack::extension::session_control::get_session_name();
        if !existing.is_empty() {
            return Ok(format!("Current session name: {existing}"));
        }
        let prompt = "\
Please generate a short, concise session title (2-5 words) for this conversation, \
then call the `set_session_name` tool with the title. Keep the user's language. \
The title should summarize the main task or topic being discussed."
            .to_string();
        crate::zerostack::extension::trigger_prompt::trigger_prompt(&prompt, DeliverAs::FollowUp)
            .map_err(|e| format!("trigger-prompt failed: {e}"))?;
        Ok("I'll generate a session name based on our conversation. Please continue with your next message.".into())
    }

    fn on_set_session_name(name: String) -> Result<bool, String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        crate::zerostack::extension::session_control::set_session_name(trimmed)
            .map_err(|e| format!("session_control: {e}"))?;
        Ok(true)
    }

    fn session_start() -> Result<(), String> {
        let ctx = crate::zerostack::extension::extension_context::get_context();
        let cwd_short = std::path::Path::new(&ctx.cwd)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| ctx.cwd.clone());
        let existing = crate::zerostack::extension::session_control::get_session_name();
        let title = if existing.is_empty() {
            format!("\u{00b7} zerostack - {cwd_short}")
        } else {
            format!("\u{2733} {existing} - {cwd_short}")
        };
        crate::zerostack::extension::session_control::set_terminal_title(&title);
        Ok(())
    }

    fn session_shutdown() -> Result<(), String> {
        crate::zerostack::extension::session_control::set_terminal_title("zerostack");
        Ok(())
    }

    // v0.5.0 event hooks
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
    fn prepare_arguments(_name: String, args_json: String) -> Result<String, String> {
        Ok(format!("ok:{args_json}"))
    }
    fn init_async() -> Result<(), String> {
        Ok(())
    }
}

fn noop_output(content: &str) -> ToolOutput {
    ToolOutput {
        content: content.into(),
        details: "{}".into(),
        is_error: false,
        terminate: None,
        added_tool_names: None,
        is_partial: None,
    }
}

export!(SessionNameExtension);
