//! zerostack btw plugin — adds /btw side-question command and btw_ask tool.
//!
//! Compiles to wasm32-unknown-unknown.
//! Exports: alloc, init, tool_execute, on_command
//! Imports: host_register_tool, host_register_command, host_exec

#![allow(static_mut_refs)]

use core::ptr::addr_of_mut;

// ── Host imports ────────────────────────────────────────────

unsafe extern "C" {
    fn host_register_tool(def_ptr: *const u8, def_len: usize) -> i32;
    fn host_register_command(def_ptr: *const u8, def_len: usize) -> i32;
    fn host_exec(cmd_ptr: *const u8, cmd_len: usize, result_ptr: *mut u8, result_max: usize)
    -> i32;
}

// ── Bump allocator ──────────────────────────────────────────

static mut BUMP: [u8; 65536] = [0; 65536];
static mut BUMP_OFF: usize = 0;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alloc(size: usize) -> *mut u8 {
    unsafe {
        let off = addr_of_mut!(BUMP_OFF).read_volatile();
        let aligned = (off + 3) & !3;
        if aligned + size > BUMP.len() {
            return core::ptr::null_mut();
        }
        addr_of_mut!(BUMP_OFF).write_volatile(aligned + size);
        addr_of_mut!(BUMP).cast::<u8>().add(aligned)
    }
}

// ── Registration ────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn init() -> i32 {
    unsafe {
        // Register the "btw_ask" tool.
        let tool_def = b"{\"name\":\"btw_ask\",\"label\":\"BTW Ask\",\"description\":\"Ask a quick side question without interrupting the main conversation. Useful for checking facts about the codebase.\",\"parameters_schema\":\"{\\\"type\\\":\\\"object\\\",\\\"properties\\\":{\\\"question\\\":{\\\"type\\\":\\\"string\\\",\\\"description\\\":\\\"The question to ask\\\"}},\\\"required\\\":[\\\"question\\\"]}\",\"prompt_snippet\":\"Ask a quick side question\",\"prompt_guidelines\":[\"Use btw_ask for quick lookups and fact-checking during a coding session.\"]}";
        host_register_tool(tool_def.as_ptr(), tool_def.len());

        // Register the "/btw" slash command.
        let cmd_def = b"{\"name\":\"btw\",\"description\":\"Ask a quick side question in parallel with the main agent\"}";
        host_register_command(cmd_def.as_ptr(), cmd_def.len());
    }
    0
}

// ── Tool execution ──────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tool_execute(
    _name_ptr: *const u8,
    _name_len: usize,
    params_ptr: *const u8,
    params_len: usize,
) -> *const u8 {
    unsafe {
        let len = if params_len > 512 { 512 } else { params_len };
        let params = core::slice::from_raw_parts(params_ptr, len);
        let s = core::str::from_utf8(params).unwrap_or("{}");
        // Extract the question from params JSON.
        let question = extract_question(s);

        // Try to answer by running a quick grep.
        let result = answer_question(&question);

        allocate_json_result(&result)
    }
}

// ── Command handler ─────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn on_command(
    _name_ptr: *const u8,
    _name_len: usize,
    args_ptr: *const u8,
    args_len: usize,
) -> *const u8 {
    unsafe {
        let len = if args_len > 2048 { 2048 } else { args_len };
        let args = core::slice::from_raw_parts(args_ptr, len);
        let question = core::str::from_utf8(args).unwrap_or("").trim();

        if question.is_empty() {
            return allocate_json_result("Usage: /btw <question> — ask a quick side question.");
        }

        let result = answer_question(question);
        allocate_json_result(&result)
    }
}

// ── Helpers ─────────────────────────────────────────────────

fn extract_question(json: &str) -> String {
    // Simple JSON value extraction for "question" field.
    for part in json.split('"') {
        if part == "question" {
            continue;
        }
        if part == ":" || part == ": " || part == "," {
            continue;
        }
        // After "question":" the next quoted string is the value.
        if let Some(start) = json.find("\"question\"") {
            let after = &json[start + 10..]; // skip "question"
            let after = after.trim_start_matches(|c| c == ':' || c == ' ' || c == '"');
            if let Some(end) = after.find('"') {
                return after[..end].to_string();
            }
        }
    }
    json.to_string()
}

fn answer_question(question: &str) -> String {
    // For a real implementation, this would use host_exec to run grep/read.
    // For MVP, provide a helpful response based on keyword matching.
    let lower = question.to_lowercase();

    if lower.contains("help") || lower.contains("usage") {
        return "BTW Plugin: Use /btw <question> to ask quick side questions while the agent works.\n\
                The LLM can also call the btw_ask tool with a question parameter.\n\
                This plugin demonstrates zerostack's Wasm extension system.".to_string();
    }
    if lower.contains("btw") && (lower.contains("what") || lower.contains("how")) {
        return "BTW (By The Way) is a side-question channel. It lets you ask quick questions\n\
                in parallel with the main coding agent, without interrupting it.\n\
                Results appear inline in the TUI."
            .to_string();
    }
    if lower.contains("test") || lower.contains("plugin") {
        return "This is the btw plugin for zerostack, built with Rust + Wasm.\n\
                It registers:\n\
                - /btw slash command (for user-triggered side questions)\n\
                - btw_ask tool (for LLM-triggered side questions)\n\
                The plugin uses zerostack-extension-api and compiles to wasm32-unknown-unknown."
            .to_string();
    }

    // Try to run a grep via host_exec to give a real answer.
    let mut cmd_buf = [0u8; 1024];
    let cmd = b"sh\0-c\0grep -ri --include='*.rs' ";
    let mut pos = 0;
    copy_bytes(&mut cmd_buf, &mut pos, cmd);
    copy_bytes(&mut cmd_buf, &mut pos, b"\"");
    copy_bytes(&mut cmd_buf, &mut pos, question.as_bytes());
    copy_bytes(&mut cmd_buf, &mut pos, b"\" src/ 2>/dev/null | head -20\0");

    let mut result_buf = [0u8; 4096];
    let exit_code = unsafe {
        host_exec(
            cmd_buf.as_ptr(),
            pos,
            result_buf.as_mut_ptr(),
            result_buf.len(),
        )
    };

    if exit_code == 0 {
        let s = core::str::from_utf8(&result_buf).unwrap_or("");
        if !s.is_empty() && s.trim() != "" {
            return format!("BTW results for \"{question}\":\n{s}");
        }
    }

    format!(
        "BTW: No codebase results found for \"{question}\". Try searching for a different keyword, or use /btw help for usage info."
    )
}

fn copy_bytes(dst: &mut [u8; 1024], pos: &mut usize, src: &[u8]) {
    let end = (*pos + src.len()).min(dst.len());
    let n = end - *pos;
    dst[*pos..*pos + n].copy_from_slice(&src[..n]);
    *pos = end;
}

fn allocate_json_result(text: &str) -> *const u8 {
    let prefix = b"{\"content\":\"";
    let suffix = b"\",\"details\":\"{}\",\"is_error\":false}";
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    let total = prefix.len() + escaped.len() + suffix.len();

    let dst = unsafe { alloc(total) };
    if dst.is_null() {
        return b"{\"content\":\"error: out of memory\",\"details\":\"{}\",\"is_error\":true}"
            .as_ptr();
    }

    unsafe {
        let mut p = dst;
        core::ptr::copy_nonoverlapping(prefix.as_ptr(), p, prefix.len());
        p = p.add(prefix.len());
        core::ptr::copy_nonoverlapping(escaped.as_ptr(), p, escaped.len());
        p = p.add(escaped.len());
        core::ptr::copy_nonoverlapping(suffix.as_ptr(), p, suffix.len());
    }
    dst
}
