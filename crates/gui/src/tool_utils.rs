//! Formatting helpers for tool calls / tool results surfaced by the engine.

/// Compact one-line summary of a tool call, mirroring the formatter used by
/// the TUI (`src/ui/utils.rs`) so the GUI and TUI show the same primary
/// argument. The result is short enough to fit on one line of a tool
/// bubble without scrolling.
pub(crate) fn format_tool_call_summary(name: &str, args: &serde_json::Value) -> String {
    let obj = match args {
        serde_json::Value::Object(map) => map,
        _ => return name.to_string(),
    };

    if name == "task" {
        return format_task_summary(obj);
    }

    let primary_keys: &[&str] = match name {
        "read" | "write" | "edit" | "list_dir" => &["path"],
        "grep" => &["pattern", "path"],
        "find_files" => &["pattern"],
        "bash" => &["command"],
        _ => &[],
    };

    let mut shown: Vec<String> = Vec::new();
    for key in primary_keys {
        if let Some(serde_json::Value::String(val)) = obj.get(*key) {
            shown.push(display_value(val));
        }
    }

    if shown.is_empty() {
        match obj.iter().next() {
            Some((_, serde_json::Value::String(val))) => {
                format!("{} {}", name, display_value(val))
            }
            _ => name.to_string(),
        }
    } else {
        format!("{} {}", name, shown.join(" "))
    }
}

fn display_value(val: &str) -> String {
    if val.len() > 80 {
        format!("{}…", &val[..77])
    } else {
        val.to_string()
    }
}

fn format_task_summary(obj: &serde_json::Map<String, serde_json::Value>) -> String {
    let prompts = match obj.get("prompts") {
        Some(serde_json::Value::Array(arr)) => arr,
        _ => return "task".to_string(),
    };
    let parts: Vec<String> = prompts
        .iter()
        .filter_map(|v| v.as_str())
        .map(str::to_string)
        .collect();
    if parts.is_empty() {
        "task".to_string()
    } else {
        format!("task {}", parts.join(" "))
    }
}

/// Trim a tool result down to the first `max_chars` characters with an
/// ellipsis when truncated. The frontend renders the full content if the
/// user expands the card; this helper only feeds the collapsed preview.
pub(crate) fn preview_tool_result(output: &str, max_chars: usize) -> String {
    if output.len() <= max_chars {
        return output.to_string();
    }
    let truncated = &output[..max_chars];
    // Trim to the last full char so we don't slice mid-codepoint.
    let mut cut = max_chars;
    while cut > 0 && !truncated.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut s = truncated[..cut].to_string();
    // Strip trailing newline so the ellipsis doesn't sit on its own line.
    while s.ends_with('\n') || s.ends_with(' ') {
        s.pop();
    }
    s.push('…');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bash_summary_extracts_command() {
        let s = format_tool_call_summary("bash", &json!({"command": "ls -la"}));
        assert_eq!(s, "bash ls -la");
    }

    #[test]
    fn read_summary_extracts_path() {
        let s = format_tool_call_summary("read", &json!({"path": "/tmp/x.rs"}));
        assert_eq!(s, "read /tmp/x.rs");
    }

    #[test]
    fn unknown_tool_falls_back_to_name() {
        let s = format_tool_call_summary("custom_tool", &json!({"foo": "bar"}));
        assert_eq!(s, "custom_tool bar");
    }

    #[test]
    fn preview_truncates_with_ellipsis() {
        assert_eq!(preview_tool_result("abcdef", 3), "abc…");
    }

    #[test]
    fn preview_returns_full_when_short() {
        assert_eq!(preview_tool_result("abc", 10), "abc");
    }

    #[test]
    fn preview_strips_trailing_whitespace_before_ellipsis() {
        // Input is 6 chars, max 4 — slice to 4 ("foo "), strip the trailing
        // space so the ellipsis sits next to "foo".
        assert_eq!(preview_tool_result("foo bar", 4), "foo…");
    }
}
