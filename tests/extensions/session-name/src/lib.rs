//! session-name extension for zerostack.
//!
//! Registers a `/name` slash command and a `set-session-name` tool.
//! When `/name` is invoked, it injects a prompt via `trigger-prompt` asking
//! the agent to generate a concise title, then call the `set-session-name` tool.

wit_bindgen::generate!({
    world: "extension-world",
    path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../../crates/extension-api/wit"),
});

use crate::zerostack::extension::command_registry::CommandDefinition;
use crate::zerostack::extension::tool_registry::ToolDefinition;

struct SessionNameExtension;

impl Guest for SessionNameExtension {
    fn init() -> Result<(), String> {
        // Register the /name slash command.
        crate::zerostack::extension::command_registry::register_command(&CommandDefinition {
            name: "name".into(),
            description: "Show or generate a session name".into(),
            argument_hint: Some("[new name]".into()),
        })
        .map_err(|e| format!("register_command failed: {e}"))?;

        // Register the set-session-name tool (used by the agent after it generates a name).
        crate::zerostack::extension::tool_registry::register_tool(&ToolDefinition {
            name: "set_session_name".into(),
            label: "Set Session Name".into(),
            description:
                "Set the current session name to a short, concise title (2-5 words). \
                 Call this after generating a title for the session based on the user's request."
                    .into(),
            parameters_schema: r#"{"type":"object","properties":{"name":{"type":"string","description":"Short session title, 2-5 words"}},"required":["name"]}"#.into(),
            prompt_snippet: None,
            prompt_guidelines: vec![],
        })
        .map_err(|_| "register_tool failed (name conflict)".to_string())?;

        Ok(())
    }

    fn tool_execute(name: String, params_json: String) -> Result<ToolOutput, String> {
        if name.ends_with("set_session_name") {
            // Parse the name from JSON params.
            let name_value: serde_json::Value =
                serde_json::from_str(&params_json).map_err(|e| format!("invalid JSON: {e}"))?;
            let session_name = name_value["name"].as_str().unwrap_or("").trim().to_string();

            if session_name.is_empty() {
                return Ok(ToolOutput {
                    content: "No name provided; session name unchanged.".into(),
                    details: "{}".into(),
                    is_error: false,
                });
            }

            // Persist the session name.
            crate::zerostack::extension::session_control::set_session_name(&session_name)
                .map_err(|e| format!("failed to set session name: {e}"))?;

            // Update terminal title.
            let ctx = crate::zerostack::extension::extension_context::get_context();
            let cwd = std::path::Path::new(&ctx.cwd)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| ctx.cwd.clone());
            let title = format!("✳ {session_name} - {cwd}");
            crate::zerostack::extension::session_control::set_terminal_title(&title);

            return Ok(ToolOutput {
                content: format!("Session name set to: {session_name}").into(),
                details: "{}".into(),
                is_error: false,
            });
        }

        Err(format!("unknown tool: {name}"))
    }

    fn on_command(name: String, args: String) -> Result<String, String> {
        if !name.ends_with("name") {
            return Ok(String::new());
        }

        let args = args.trim();

        // If the user provided a name directly, set it.
        if !args.is_empty() {
            crate::zerostack::extension::session_control::set_session_name(args)
                .map_err(|e| format!("failed to set session name: {e}"))?;

            let ctx = crate::zerostack::extension::extension_context::get_context();
            let cwd = std::path::Path::new(&ctx.cwd)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| ctx.cwd.clone());
            let title = format!("✳ {args} - {cwd}");
            crate::zerostack::extension::session_control::set_terminal_title(&title);

            return Ok(format!("Session name set to: {args}"));
        }

        // Check if a name already exists.
        let existing = crate::zerostack::extension::session_control::get_session_name();
        if !existing.is_empty() {
            return Ok(format!("Current session name: {existing}"));
        }

        // No name yet — inject a prompt for the agent to generate one.
        let prompt = "\
Please generate a short, concise session title (2-5 words) for this conversation, \
then call the `set_session_name` tool with the title. Keep the user's language. \
The title should summarize the main task or topic being discussed."
            .to_string();

        crate::zerostack::extension::trigger_prompt::trigger_prompt(&prompt, "followUp")
            .map_err(|e| format!("trigger-prompt failed: {e}"))?;

        Ok(
            "I'll generate a session name based on our conversation. Please continue with your next message."
                .into(),
        )
    }

    fn session_start() -> Result<(), String> {
        // Update terminal title with session context on start.
        let ctx = crate::zerostack::extension::extension_context::get_context();
        let cwd = std::path::Path::new(&ctx.cwd)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| ctx.cwd.clone());

        let existing = crate::zerostack::extension::session_control::get_session_name();
        let title = if existing.is_empty() {
            format!("· zerostack - {cwd}")
        } else {
            format!("✳ {existing} - {cwd}")
        };
        crate::zerostack::extension::session_control::set_terminal_title(&title);
        Ok(())
    }

    fn session_shutdown() -> Result<(), String> {
        // Restore a clean title.
        crate::zerostack::extension::session_control::set_terminal_title("zerostack");
        Ok(())
    }
}

export!(SessionNameExtension);
