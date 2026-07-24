//! pi-simplify extension for zerostack.
//!
//! Port of the pi extension: `/simplify` command that reviews recently
//! changed files for clarity, consistency, and maintainability improvements.
//!
//! Uses `git diff --name-status` (via WASI process) to find changed files,
//! builds a structured review prompt, and injects it via `trigger-prompt`.

wit_bindgen::generate!({
    world: "extension-world",
    path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../../crates/extension-api/wit"),
});

use crate::zerostack::extension::command_registry::CommandDefinition;

struct SimplifyExtension;

impl Guest for SimplifyExtension {
    fn init() -> Result<(), String> {
        crate::zerostack::extension::command_registry::register_command(
            &CommandDefinition {
                name: "simplify".into(),
                description: "Review recently changed files for clarity, consistency, and maintainability improvements".into(),
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
}

/// Parse command arguments: `--staged`, `--ref=NAME`, file paths.
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

/// Run `git diff --name-status` and parse output into file paths.
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

/// Build the simplification review prompt.
fn build_prompt(files: &[String]) -> String {
    let file_list = files
        .iter()
        .map(|f| format!("- {}", f))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Review the following recently changed files and apply simplification improvements.\n\
\n\
## Principles\n\
\n\
- **Preserve functionality**: Never change what the code does. All existing tests must continue to pass.\n\
- **Apply project standards**: Follow any conventions from CLAUDE.md or AGENTS.md in this project.\n\
- **Enhance clarity**: Reduce unnecessary complexity and nesting, eliminate redundant code and abstractions, improve variable and function names, consolidate related logic, remove unnecessary comments that describe obvious code.\n\
- **Maintain balance**: Do not over-simplify. Avoid overly clever solutions. Prioritize readability over fewer lines.\n\
\n\
## Scope\n\
\n\
Only review and modify these files:\n\
{file_list}\n\
\n\
## Process\n\
\n\
1. Read each file listed above\n\
2. Identify concrete improvements (dead code, unclear names, redundant logic)\n\
3. Apply changes one file at a time\n\
4. After all changes, run existing tests to verify nothing is broken\n\
5. Summarize what you changed and why\n\
\n\
Do NOT add new features, change public APIs, or refactor code outside the listed files.",
    )
}

/// Run a shell command via the host and return stdout as a String.
///
/// Routes through the `host-calls::exec` host import rather than
/// `std::process::Command` because the WASI sandbox does not implement
/// `wasi:cli/process` — calling `std::process::Command::new("sh")` from a
/// wasm32-wasip2 guest yields "operation not supported on this platform".
fn run_shell(cmd: &str) -> Result<String, String> {
    let result = crate::zerostack::extension::host_calls::exec(
        "sh",
        &[String::from("-c"), String::from(cmd)],
    )
    .map_err(|e| format!("failed to run command: {e}"))?;

    if result.exit_code == 0 {
        Ok(result.stdout)
    } else {
        Err(format!(
            "command exited with code {}: {}",
            result.exit_code, result.stderr
        ))
    }
}

/// Handle the /simplify slash command.
fn handle_simplify_command(args: &str) -> Result<String, String> {
    let (explicit_files, staged, git_ref) = parse_args(args);

    let files = match get_changed_files(staged, &git_ref, &explicit_files) {
        Ok(f) => f,
        Err(e) => {
            return Ok(format!("pi-simplify: {e}"));
        }
    };

    let prompt = build_prompt(&files);

    crate::zerostack::extension::trigger_prompt::trigger_prompt(&prompt, "followUp")
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
