use crate::extras::truncate::truncate_cjk;

const TOOL_SUMMARY_MAX: usize = 200;

fn display_value(val: &str) -> String {
    if val.len() <= TOOL_SUMMARY_MAX {
        format!("\"{}\"", val)
    } else {
        format!("\"{}\"", truncate_cjk(val, TOOL_SUMMARY_MAX, "..."))
    }
}

/// Formats a tool call showing only the primary file/command parameter.
pub fn format_tool_call_summary(name: &str, args: &serde_json::Value) -> String {
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

    let mut shown = Vec::new();
    for key in primary_keys {
        if let Some(serde_json::Value::String(val)) = obj.get(*key) {
            let display_val = if name == "bash" {
                val.clone()
            } else {
                display_value(val)
            };
            shown.push(display_val);
        }
    }

    if shown.is_empty() {
        if let Some((_, serde_json::Value::String(val))) = obj.iter().next() {
            format!("{} {}", name, display_value(val))
        } else {
            name.to_string()
        }
    } else {
        format!("{} {}", name, shown.join(" "))
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
        .map(display_value)
        .collect();
    if parts.is_empty() {
        "task".to_string()
    } else {
        format!("task {}", parts.join(" "))
    }
}
