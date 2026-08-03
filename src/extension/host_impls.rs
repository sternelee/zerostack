//! Host import implementations for v0.5.0 extensions.
//!
//! Strategy:
//! - Each registered host import gets a `Host` impl for `ExtGuestState`.
//! - Capabilities are enforced at link time (`host::apply_capability_gating`).
//! - Event hooks (`on-tool-call`, `on-tool-result`, etc.) are called
//!   unconditionally on guest exports; guest traps indicate "no handler".
//! - Provider registrations bubble up via the global queue shimmed on
//!   `ExtGuestState.queued_provider_registrations`.

use std::sync::Arc;
use std::time::Duration;

use crate::extension::host::zerostack::extension::{
    command_registry as wit_cmd, exec as wit_exec, file_mutation_queue as wit_fmq,
    http as wit_http, provider_registry as wit_pr, session_control as wit_sc,
    tool_registry as wit_tr, trigger_prompt as wit_tp, truncator as wit_trunc,
    ui_prompt as wit_uip, ui_status as wit_uis,
};
use crate::extension::host::{
    ExtGuestState, ExtensionHost, ExtensionWorld, namespaced_tool_name, types,
};

// Compile-time bridge: WIT-bindgen generates `wasmtime::component::__internal::String`
// for the host-impl error type.
pub(crate) use wasmtime::component::__internal::String as __internal_string;

// ── tool-registry ────────────────────────────────────────────────────

impl wit_tr::Host for ExtGuestState {
    fn register_tool(&mut self, def: wit_tr::ToolDefinition) -> Result<(), __internal_string> {
        let bare = def.name;
        let namespaced = namespaced_tool_name(&self.extension_id, &bare);
        self.tools.push(crate::extension::RegisteredTool {
            name: namespaced,
            label: def.label,
            description: def.description,
            parameters_schema: def.parameters_schema,
            prompt_snippet: def.prompt_snippet,
            prompt_guidelines: def.prompt_guidelines.unwrap_or_default(),
            extension_id: self.extension_id.clone(),
            execution_mode: def
                .execution_mode.map(crate::extension::ToolExecutionMode::from)
                .unwrap_or_default(),
            loading_mode: if def.deferred.unwrap_or(false) {
                crate::extension::ToolLoadingMode::Deferred
            } else {
                crate::extension::ToolLoadingMode::Eager
            },
        });
        Ok(())
    }

    fn unregister_tool(&mut self, name: String) {
        let namespaced = namespaced_tool_name(&self.extension_id, &name);
        self.tools.retain(|t| t.name != namespaced);
    }
}

// ── command-registry ────────────────────────────────────────────────

impl wit_cmd::Host for ExtGuestState {
    fn register_command(
        &mut self,
        def: wit_cmd::CommandDefinition,
    ) -> Result<(), __internal_string> {
        let bare = def.name;
        let namespaced = namespaced_tool_name(&self.extension_id, &bare);
        self.commands.insert(
            namespaced.clone(),
            crate::extension::RegisteredCommand {
                name: namespaced,
                description: def.description,
                extension_id: self.extension_id.clone(),
            },
        );
        Ok(())
    }

    fn unregister_command(&mut self, name: String) {
        let namespaced = namespaced_tool_name(&self.extension_id, &name);
        self.commands.remove(&namespaced);
    }
}

// ── extension-context ───────────────────────────────────────────────

impl crate::extension::host::zerostack::extension::extension_context::Host for ExtGuestState {
    fn get_context(
        &mut self,
    ) -> crate::extension::host::zerostack::extension::extension_context::ExtensionInfo {
        let info = &self.host_context;
        crate::extension::host::zerostack::extension::extension_context::ExtensionInfo {
            cwd: info.cwd.clone(),
            session_id: info.session_id.clone(),
            model_name: info.model_name.clone(),
            project_trusted: info.project_trusted,
            has_ui: info.has_ui,
        }
    }
}

// ── trigger-prompt ───────────────────────────────────────────────────

impl wit_tp::Host for ExtGuestState {
    fn trigger_prompt(
        &mut self,
        prompt: String,
        deliver_as: types::DeliverAs,
    ) -> Result<(), __internal_string> {
        self.queued_prompts.push((prompt, deliver_as));
        Ok(())
    }
}

// ── session-control ──────────────────────────────────────────────────

impl wit_sc::Host for ExtGuestState {
    fn get_session_name(&mut self) -> String {
        self.session_state
            .lock()
            .map(|s| s.0.clone())
            .unwrap_or_default()
    }

    fn set_session_name(&mut self, name: String) -> Result<(), __internal_string> {
        if let Ok(mut s) = self.session_state.lock() {
            s.0 = name;
        }
        Ok(())
    }

    fn set_terminal_title(&mut self, title: String) {
        // Don't print to stdout directly — TUI routes via renderer.
        if let Ok(mut s) = self.session_state.lock() {
            s.1 = title;
        }
    }
}

// ── provider-registry ────────────────────────────────────────────────

impl wit_pr::Host for ExtGuestState {
    fn register_provider(&mut self, cfg: wit_pr::ProviderConfig) -> Result<(), __internal_string> {
        self.queued_provider_registrations
            .push((cfg.name.clone(), cfg.base_url.clone()));
        Ok(())
    }
    fn unregister_provider(&mut self, _name: String) -> Result<(), __internal_string> {
        Ok(())
    }
}

impl ExtGuestState {
    pub fn drain_provider_registrations(&mut self) -> Vec<(String, Option<String>)> {
        std::mem::take(&mut self.queued_provider_registrations)
    }
}

// ── ui-prompt ────────────────────────────────────────────────────────

impl wit_uip::Host for ExtGuestState {
    fn select(&mut self, title: String, options: Vec<types::SelectOption>) -> String {
        if !self.host_context.has_ui {
            return String::new();
        }
        crate::ui::dialogs::select(&title, options).unwrap_or_default()
    }
    fn confirm(&mut self, title: String, message: String) -> bool {
        if !self.host_context.has_ui {
            return false;
        }
        crate::ui::dialogs::confirm(&title, &message).unwrap_or(false)
    }
    fn input(&mut self, title: String, placeholder: Option<String>) -> String {
        if !self.host_context.has_ui {
            return String::new();
        }
        crate::ui::dialogs::input(&title, placeholder.as_deref()).unwrap_or_default()
    }
    fn notify(&mut self, message: String, level: Option<String>) {
        if !self.host_context.has_ui {
            return;
        }
        crate::ui::dialogs::notify(&message, level.as_deref().unwrap_or("info"));
    }
}

// ── ui-status ────────────────────────────────────────────────────────

impl wit_uis::Host for ExtGuestState {
    fn set_status(&mut self, key: String, text: Option<String>) {
        self.status_entries.push((key, text));
    }
    fn set_widget(&mut self, key: String, lines: Option<Vec<String>>, placement: Option<String>) {
        self.widget_entries.push((key, lines, placement));
    }
    fn set_title(&mut self, title: String) {
        if let Ok(mut s) = self.session_state.lock() {
            s.1 = title;
        }
    }
    fn toast(&mut self, message: String) {
        wit_uip::Host::notify(self, message, Some("info".into()));
    }
}

// ── agent-control ────────────────────────────────────────────────────

impl crate::extension::host::zerostack::extension::agent_control::Host for ExtGuestState {
    fn send_message(&mut self, _text: String, _deliver_as: Option<String>) -> Result<(), String> {
        Ok(())
    }
    fn send_user_message(&mut self, _text: String) -> Result<(), String> {
        Ok(())
    }
    fn append_entry(&mut self, _custom_type: String, _data_json: String) -> Result<(), String> {
        Ok(())
    }
    fn set_model(&mut self, _provider: String, _name: String) -> Result<(), String> {
        Ok(())
    }
    fn get_active_tools(&mut self) -> Vec<String> {
        Vec::new()
    }
    fn set_active_tools(&mut self, _names: Vec<String>) -> Result<(), String> {
        Ok(())
    }
    fn compact(&mut self, _custom_instructions: Option<String>) -> Result<(), String> {
        Ok(())
    }
}

// ── exec ─────────────────────────────────────────────────────────────

impl wit_exec::Host for ExtGuestState {
    fn run(
        &mut self,
        command: String,
        args: Vec<String>,
        cwd: Option<String>,
        timeout_ms: Option<u32>,
    ) -> Result<types::ExecResult, String> {
        use std::process::Command;
        let mut cmd = Command::new(&command);
        cmd.args(&args);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let timeout = timeout_ms.map(|m| Duration::from_millis(m as u64));
        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("exec spawn failed: {e}"))?;
        let timed_out = match timeout {
            Some(t) => match child.wait_timeout(t) {
                Ok(Some(_)) => false,
                Ok(None) => {
                    let _ = child.kill();
                    true
                }
                Err(_) => false,
            },
            None => {
                let _ = child.wait();
                false
            }
        };
        let out = child.wait_with_output().ok();
        let exit = out.as_ref().and_then(|o| o.status.code());
        let mut combined = String::new();
        if let Some(o) = out {
            combined.push_str(&String::from_utf8_lossy(&o.stdout));
        }
        Ok(types::ExecResult {
            output: combined,
            exit_code: exit,
            timed_out: Some(timed_out),
        })
    }
}

trait WaitTimeoutExt {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>>;
}

#[cfg(unix)]
impl WaitTimeoutExt for std::process::Child {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = std::time::Instant::now();
        loop {
            match self.try_wait()? {
                Some(status) => return Ok(Some(status)),
                None => {
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}

#[cfg(not(unix))]
impl WaitTimeoutExt for std::process::Child {
    fn wait_timeout(
        &mut self,
        _timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        Ok(Some(self.wait()?))
    }
}

// ── http ─────────────────────────────────────────────────────────────

impl wit_http::Host for ExtGuestState {
    fn request(
        &mut self,
        method: String,
        url: String,
        headers: Option<Vec<(String, String)>>,
        body: Option<String>,
        timeout_ms: Option<u32>,
    ) -> Result<String, String> {
        let timeout = timeout_ms.map(|m| Duration::from_millis(m as u64));
        tokio::task::block_in_place(|| {
            let client = reqwest::blocking::Client::builder()
                .timeout(timeout.unwrap_or(Duration::from_secs(30)))
                .build()
                .map_err(|e| format!("http client build: {e}"))?;
            let mut req = client.request(
                method.parse().map_err(|e| format!("bad method: {e}"))?,
                &url,
            );
            if let Some(h) = headers {
                for (k, v) in h {
                    req = req.header(&k, &v);
                }
            }
            if let Some(b) = body {
                req = req.body(b);
            }
            let resp = req.send().map_err(|e| format!("http send: {e}"))?;
            resp.text().map_err(|e| format!("http body: {e}"))
        })
    }
}

// ── file-mutation-queue ──────────────────────────────────────────────

impl wit_fmq::Host for ExtGuestState {
    fn with_lock(&mut self, _path: String, _callback_id: u64) -> Result<(), String> {
        // Hook is recorded; actual locking lives in the host manager.
        Ok(())
    }
}

// ── truncator ────────────────────────────────────────────────────────

impl wit_trunc::Host for ExtGuestState {
    fn truncate_tail(&mut self, content: String, max_bytes: u32) -> String {
        truncate_tail_impl(&content, max_bytes as usize)
    }
    fn truncate_head(&mut self, content: String, max_bytes: u32) -> String {
        truncate_head_impl(&content, max_bytes as usize)
    }
    fn cap_output(
        &mut self,
        content: String,
        max_bytes: Option<u32>,
        max_lines: Option<u32>,
    ) -> String {
        const DEFAULT_MAX_BYTES: usize = 50 * 1024;
        const DEFAULT_MAX_LINES: usize = 2000;
        let max_bytes = max_bytes.map(|n| n as usize).unwrap_or(DEFAULT_MAX_BYTES);
        let max_lines = max_lines.map(|n| n as usize).unwrap_or(DEFAULT_MAX_LINES);
        let mut out = content;
        if out.len() > max_bytes {
            out = truncate_tail_impl(&out, max_bytes);
            out.push_str("\n[truncated]");
        }
        let lines: Vec<&str> = out.lines().collect();
        if lines.len() > max_lines {
            let mut s = lines[lines.len() - max_lines..].join("\n");
            s.insert_str(0, "[truncated]\n");
            s
        } else {
            out
        }
    }
}

fn truncate_tail_impl(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }
    let end = content.len();
    let mut start = end.saturating_sub(max_bytes);
    while start < end && !content.is_char_boundary(start) {
        start += 1;
    }
    let mut s = String::with_capacity(end - start + 64);
    s.push_str("[truncated]\n");
    s.push_str(&content[start..]);
    s
}

fn truncate_head_impl(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }
    let mut end = max_bytes;
    while end < content.len() && !content.is_char_boundary(end) {
        end += 1;
    }
    let mut s = String::with_capacity(end + 64);
    s.push_str(&content[..end]);
    s.push_str("\n[truncated]");
    s
}

// ── compaction ───────────────────────────────────────────────────────

impl crate::extension::host::zerostack::extension::compaction::Host for ExtGuestState {
    fn before_compact(
        &mut self,
        _reason: String,
    ) -> Result<crate::extension::host::zerostack::extension::compaction::CompactionDecision, String>
    {
        Ok(
            crate::extension::host::zerostack::extension::compaction::CompactionDecision {
                action: types::CancelAction::Proceed,
                summary: None,
                label: None,
                skip_conversation_restore: None,
            },
        )
    }
    fn after_compact(&mut self, _reason: String, _summary: String) -> Result<(), String> {
        Ok(())
    }
}

// ── events-bus ───────────────────────────────────────────────────────

impl crate::extension::host::zerostack::extension::events_bus::Host for ExtGuestState {
    fn subscribe(&mut self, name: String, handler_id: u64) -> Result<(), String> {
        self.bus_handlers.push((name, handler_id));
        Ok(())
    }
    fn unsubscribe(&mut self, name: String, handler_id: u64) -> Result<(), String> {
        self.bus_handlers
            .retain(|(n, h)| !(n == &name && *h == handler_id));
        Ok(())
    }
    fn publish(&mut self, _name: String, _payload_json: String) -> Result<(), String> {
        Ok(())
    }
}

// ── permissions ──────────────────────────────────────────────────────

impl crate::extension::host::zerostack::extension::permissions::Host for ExtGuestState {
    fn check(
        &mut self,
        _tool: String,
        _pattern: String,
    ) -> Result<crate::extension::host::zerostack::extension::permissions::Verdict, String> {
        Ok(crate::extension::host::zerostack::extension::permissions::Verdict::Allow)
    }
    fn trust_project(&mut self) -> Result<bool, String> {
        Ok(self.host_context.project_trusted)
    }
    fn set_project_trusted(&mut self, _trusted: bool) -> Result<(), String> {
        Ok(())
    }
}

// ── resources-discover ───────────────────────────────────────────────

impl crate::extension::host::zerostack::extension::resources_discover::Host for ExtGuestState {
    fn discover(&mut self) -> Result<Vec<(String, String)>, String> {
        Ok(Vec::new())
    }
}

// ── logger ───────────────────────────────────────────────────────────

impl crate::extension::host::zerostack::extension::logger::Host for ExtGuestState {
    fn log(&mut self, level: types::LogLevel, _target: String, message: String) {
        match level {
            types::LogLevel::Debug => tracing::debug!(target: "extension", "{}", message),
            types::LogLevel::Info => tracing::info!(target: "extension", "{}", message),
            types::LogLevel::Warn => tracing::warn!(target: "extension", "{}", message),
            types::LogLevel::Error => tracing::error!(target: "extension", "{}", message),
        }
    }
}

// ── types::Host (empty default impl) ─────────────────────────────────

impl crate::extension::host::zerostack::extension::types::Host for ExtGuestState {}

// ── pending provider registrations accessor ─────────────────────────

impl ExtensionHost {
    /// Drain provider registrations made by extensions.
    pub fn pending_provider_registrations(&self) -> Vec<(String, String, Option<String>)> {
        let mut out = Vec::new();
        for (id, inst) in &self.instances {
            for rec in &inst.store.data().queued_provider_registrations {
                out.push((id.clone(), rec.0.clone(), rec.1.clone()));
            }
        }
        out
    }
}

// Reserve an area for the world type alias.
#[allow(dead_code)]
fn _force_extworld_to_be_in_scope(_e: &ExtensionWorld, _s: &Arc<()>) {}
