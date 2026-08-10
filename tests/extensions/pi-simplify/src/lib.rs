//! pi-simplify extension — v0.5.0 with all required v0.5.0 event exports.

wit_bindgen::generate!({
    world: "extension-world",
    path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../../crates/extension-api/wit"),
});

use crate::zerostack::extension::command_registry::CommandDefinition;
use crate::zerostack::extension::types::DeliverAs;
// `ToolOutput`, `ToolCallDecision`, `ToolResultPatch` come from the
// wit-bindgen prelude.

struct SimplifyExtension;

impl Guest for SimplifyExtension {
    fn init() -> Result<(), String> {
        crate::zerostack::extension::command_registry::register_command(
            &CommandDefinition {
                name: "simplify".into(),
                description:
                    "Review recently changed files for clarity, consistency, and maintainability improvements"
                        .into(),
                argument_hint: Some("[files...] [--staged] [--ref=HEAD]".into()),
            },
        )
        .map_err(|e| format!("register_command failed: {e}"))?;
        Ok(())
    }

    fn tool_execute(_name: String, _params_json: String) -> Result<ToolOutput, String> {
        Err("pi-simplify has no tools — use /simplify".into())
    }

    fn on_command(name: String, args: String) -> Result<String, String> {
        if !name.ends_with("simplify") {
            return Ok(String::new());
        }
        handle_simplify_command(&args)
    }

    fn session_start() -> Result<(), String> {
        Ok(())
    }
    fn session_shutdown() -> Result<(), String> {
        Ok(())
    }
    fn prepare_arguments(_name: String, args_json: String) -> Result<String, String> {
        Ok(format!("ok:{args_json}"))
    }
    fn init_async() -> Result<(), String> {
        Ok(())
    }

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
}

fn parse_args(args: &str) -> (Vec<String>, bool, String) {
    let mut files = Vec::new();
    let mut staged = false;
    let mut git_ref = String::from("HEAD");
    for token in args.split_whitespace() {
        if token == "--staged" {
            staged = true;
        } else if let Some(val) = token.strip_prefix("--ref=") {
            git_ref = val.to_string();
        } else {
            files.push(token.to_string());
        }
    }
    (files, staged, git_ref)
}

fn get_changed_files(
    staged: bool,
    git_ref: &str,
    explicit_files: &[String],
) -> Result<Vec<String>, String> {
    if !explicit_files.is_empty() {
        return Ok(explicit_files.to_vec());
    }
    let mut cmd = String::from("git diff --name-status");
    if staged {
        cmd.push_str(" --cached");
    } else {
        cmd.push(' ');
        cmd.push_str(git_ref);
    }
    let output = run_shell(&cmd)?;
    if output.trim().is_empty() {
        return Err(
            "No changed files found. Specify file paths or make some changes first.".into(),
        );
    }
    let mut files = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let path = if parts.len() >= 3 && (line.starts_with('R') || line.starts_with('C')) {
                parts[2]
            } else {
                parts[1]
            };
            files.push(path.to_string());
        }
    }
    if files.is_empty() {
        return Err(
            "No changed files found. Specify file paths or make some changes first.".into(),
        );
    }
    Ok(files)
}

fn build_prompt(files: &[String]) -> String {
    let file_list = files
        .iter()
        .map(|f| format!("- {}", f))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Review the following recently changed files and apply simplification improvements.\n\n\
## Principles\n\n\
- **Preserve functionality**: Never change what the code does. All existing tests must continue to pass.\n\
- **Apply project standards**: Follow any conventions from CLAUDE.md or AGENTS.md in this project.\n\
- **Enhance clarity**: Reduce unnecessary complexity and nesting, eliminate redundant code and abstractions, improve variable and function names, consolidate related logic, remove unnecessary comments that describe obvious code.\n\
- **Maintain balance**: Do not over-simplify. Avoid overly clever solutions. Prioritize readability over fewer lines.\n\n\
## Scope\n\nOnly review and modify these files:\n{file_list}\n\n\
## Process\n\n1. Read each file listed above\n\
2. Identify concrete improvements\n\
3. Apply changes one file at a time\n\
4. After all changes, run existing tests\n\
5. Summarize what you changed and why\n\n\
Do NOT add new features or refactor outside the listed files."
    )
}

fn run_shell(cmd: &str) -> Result<String, String> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map(|output| {
            if output.status.success() {
                String::from_utf8_lossy(&output.stdout).into_owned()
            } else {
                String::new()
            }
        })
        .map_err(|e| format!("failed to run command: {e}"))
}

fn handle_simplify_command(args: &str) -> Result<String, String> {
    let (explicit_files, staged, git_ref) = parse_args(args);
    let files = match get_changed_files(staged, &git_ref, &explicit_files) {
        Ok(f) => f,
        Err(e) => return Ok(format!("pi-simplify: {e}")),
    };
    let prompt = build_prompt(&files);
    crate::zerostack::extension::trigger_prompt::trigger_prompt(&prompt, DeliverAs::NextTurn)
        .map_err(|e| format!("trigger-prompt failed: {e}"))?;
    Ok(format!(
        "pi-simplify: reviewing {} file(s):\n{}",
        files.len(),
        files
            .iter()
            .map(|f| format!("  - {}", f))
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

export!(SimplifyExtension);
