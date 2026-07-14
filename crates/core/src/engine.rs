use compact_str::CompactString;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::agent::runner;
use crate::agent::tools;
use crate::cli::Cli;
use crate::config::Config;
use crate::context::{self, ContextFiles};
use crate::event::AgentEvent;
use crate::events::{ChatMessage, CoreEvent, InitialState, SessionInfo, UserAction};
use crate::permission::SecurityMode;
use crate::permission::ask::{AskRequest, AskSender, UserDecision};
use crate::permission::checker::{PermCheck, PermissionChecker};
use crate::provider::{self, AnyAgent, AnyClient};
use crate::retry::RetryConfig;
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};

pub struct CoreEngine {
    config: Config,
    sessions: Vec<Session>,
    current_session_index: Option<usize>,
    model: CompactString,
    provider: CompactString,
    mode: SecurityMode,
    event_tx: mpsc::UnboundedSender<CoreEvent>,
    agent: Option<AnyAgent>,
    client: AnyClient,
    context: ContextFiles,
    cli: Cli,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    sandbox: Sandbox,
    reasoning_enabled: bool,
    /// Abort handle for the currently running agent forwarding task,
    /// so a new message or CancelStream can interrupt it.
    current_task: Option<tokio::task::JoinHandle<()>>,
    /// Token usage accumulated across all messages in this engine run.
    tokens_used: u64,
    /// Receiver for permission ask requests from the agent's tools.
    ask_rx: Option<mpsc::Receiver<AskRequest>>,
    /// Pending permission requests awaiting user response, keyed by id.
    pending_permissions: std::collections::HashMap<u64, oneshot::Sender<UserDecision>>,
    /// Next permission request id.
    next_permission_id: u64,
}

impl CoreEngine {
    /// Build a fully-configured engine with a ready-to-run agent.
    pub async fn build_default(
        model: CompactString,
        provider_name: CompactString,
        mode: SecurityMode,
    ) -> anyhow::Result<(
        Self,
        mpsc::UnboundedReceiver<CoreEvent>,
        Option<mpsc::Receiver<AskRequest>>,
    )> {
        let (cfg, _is_first_startup) = crate::config::load();
        let cli = Cli::default();
        let context = context::load(cli.resolve_no_context_files(&cfg));

        let sandbox = Sandbox::new(
            cli.resolve_sandbox(&cfg),
            &cli.resolve_sandbox_backend(&cfg),
        )
        .with_shell(&cli.resolve_shell(&cfg));
        let edit_system = cli.resolve_edit_system(&cfg);
        tools::set_edit_system(edit_system);
        tools::set_deny_repeated_reads(cfg.deny_repeated_reads.unwrap_or(true));

        let (permission, ask_tx, ask_rx) = build_permission_checker(&cli, &cfg, mode);

        let client = provider::create_client(
            provider_name.as_str(),
            cli.api_key.as_deref(),
            &cfg.custom_providers_map(),
            cfg.api_keys.as_ref(),
        )?;

        let qm_map = crate::config::quick_models_map(&cfg);
        let session = Session::new(
            &provider_name,
            &model,
            cfg.resolve_context_window(&provider_name, &model, &qm_map),
            "",
        );

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let mut engine = Self {
            config: cfg,
            sessions: vec![session],
            current_session_index: Some(0),
            model,
            provider: provider_name,
            mode,
            event_tx,
            agent: None,
            client,
            context,
            cli,
            permission,
            ask_tx,
            sandbox,
            reasoning_enabled: true,
            current_task: None,
            tokens_used: 0,
            ask_rx,
            pending_permissions: std::collections::HashMap::new(),
            next_permission_id: 0,
        };

        // Build the initial agent
        if let Err(e) = engine.rebuild_agent().await {
            return Err(e);
        }

        // Extract ask_rx so the runtime loop can select on it
        let ask_rx = engine.ask_rx.take();

        Ok((engine, event_rx, ask_rx))
    }

    /// Rebuild the agent with the current model/provider/context/permission state.
    async fn rebuild_agent(&mut self) -> anyhow::Result<()> {
        let temperature = crate::config::resolve_temperature(&self.cli, &self.config, &self.model);
        let extra_body = crate::config::resolve_extra_body(&self.config, &self.model);

        #[cfg(feature = "mcp")]
        let mcp_manager = connect_mcp(&self.config).await;

        let completion_model = self.client.completion_model(self.model.to_string());
        let agent = provider::build_agent(
            completion_model,
            &self.cli,
            &self.config,
            &self.context,
            self.permission.clone(),
            self.ask_tx.clone(),
            self.sandbox.clone(),
            self.reasoning_enabled,
            temperature,
            extra_body,
            #[cfg(feature = "mcp")]
            mcp_manager.as_ref(),
        )
        .await;

        self.agent = Some(agent);
        Ok(())
    }

    /// Rebuild the agent with a new provider client.
    async fn rebuild_agent_with_provider(&mut self, provider_name: &str) -> anyhow::Result<()> {
        self.client = provider::create_client(
            provider_name,
            self.cli.api_key.as_deref(),
            &self.config.custom_providers_map(),
            self.config.api_keys.as_ref(),
        )?;
        self.provider = CompactString::new(provider_name);
        self.rebuild_agent().await
    }

    pub fn initial_state(&self) -> InitialState {
        let sessions: Vec<SessionInfo> = self
            .sessions
            .iter()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                name: s.name.clone(),
                model: s.model.clone(),
                provider: s.provider.clone(),
                message_count: s.messages.len(),
                created_at: s.created_at.clone(),
            })
            .collect();

        let current_session_id = self
            .current_session_index
            .map(|i| self.sessions[i].id.clone());

        InitialState {
            sessions,
            current_session_id,
            model: self.model.clone(),
            provider: self.provider.clone(),
            mode: self.mode.to_string(),
        }
    }

    pub fn current_session(&self) -> Option<&Session> {
        self.current_session_index.map(|i| &self.sessions[i])
    }

    pub fn current_session_mut(&mut self) -> Option<&mut Session> {
        self.current_session_index.map(|i| &mut self.sessions[i])
    }

    pub async fn handle_action(&mut self, action: UserAction) -> Vec<CoreEvent> {
        match action {
            UserAction::SendMessage { text } => self.handle_send_message(text).await,
            UserAction::CancelStream => {
                if let Some(handle) = self.current_task.take() {
                    handle.abort();
                }
                let _ = self.event_tx.send(CoreEvent::AgentStopped);
                vec![]
            }
            UserAction::CreateSession { name } => {
                let session_name = name.unwrap_or_else(|| CompactString::from("New Session"));
                let qm_map = crate::config::quick_models_map(&self.config);
                let session = Session::new(
                    &self.provider,
                    &self.model,
                    self.config
                        .resolve_context_window(&self.provider, &self.model, &qm_map),
                    &session_name,
                );
                self.sessions.push(session);
                self.current_session_index = Some(self.sessions.len() - 1);
                let mut events = self.emit_session_list_updated();
                events.extend(self.emit_session_history());
                events
            }
            UserAction::SwitchSession { session_id } => {
                if let Some(idx) = self.sessions.iter().position(|s| s.id == session_id) {
                    self.current_session_index = Some(idx);
                    let mut events = vec![CoreEvent::SessionChanged { session_id }];
                    events.extend(self.emit_session_history());
                    events
                } else {
                    vec![CoreEvent::Error {
                        message: CompactString::from("Session not found"),
                    }]
                }
            }
            UserAction::DeleteSession { session_id } => {
                if let Some(idx) = self.sessions.iter().position(|s| s.id == session_id) {
                    self.sessions.remove(idx);
                    if self.current_session_index == Some(idx) {
                        self.current_session_index = if self.sessions.is_empty() {
                            None
                        } else if idx >= self.sessions.len() {
                            Some(self.sessions.len() - 1)
                        } else {
                            Some(idx)
                        };
                    } else if let Some(ref mut cur) = self.current_session_index {
                        if *cur > idx {
                            *cur -= 1;
                        }
                    }
                }
                let mut events = self.emit_session_list_updated();
                events.extend(self.emit_session_history());
                events
            }
            UserAction::RenameSession { session_id, name } => {
                if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    session.name = name;
                }
                self.emit_session_list_updated()
            }
            UserAction::ClearSession => {
                if let Some(idx) = self.current_session_index {
                    let session = &mut self.sessions[idx];
                    session.messages.clear();
                    session.total_estimated_tokens = 0;
                    session.reset_calibration();
                    session.compactions.clear();
                    self.context.chain_declined.clear();
                }
                self.emit_session_history()
            }
            UserAction::UndoLastExchange => {
                if let Some(idx) = self.current_session_index {
                    let session = &mut self.sessions[idx];
                    undo_last_exchange(session);
                }
                self.emit_session_history()
            }
            UserAction::SetModel { model } => {
                self.model = model.clone();
                if let Some(idx) = self.current_session_index {
                    let qm_map = crate::config::quick_models_map(&self.config);
                    self.sessions[idx].model = model.clone();
                    self.sessions[idx].update_context_window(self.config.resolve_context_window(
                        &self.provider,
                        &model,
                        &qm_map,
                    ));
                }
                match self.rebuild_agent().await {
                    Ok(()) => vec![self.emit_status_update()],
                    Err(e) => vec![CoreEvent::Error {
                        message: CompactString::new(format!("Failed to switch model: {e}")),
                    }],
                }
            }
            UserAction::SetProvider { provider } => {
                // Default the model to something valid for the new provider
                if let Some((default_model, _costs)) =
                    provider::default_model_for_provider(&provider, &self.config)
                {
                    self.model = CompactString::new(&default_model);
                    if let Some(idx) = self.current_session_index {
                        self.sessions[idx].model = self.model.clone();
                    }
                }
                match self.rebuild_agent_with_provider(&provider).await {
                    Ok(()) => {
                        if let Some(idx) = self.current_session_index {
                            let qm_map = crate::config::quick_models_map(&self.config);
                            self.sessions[idx].provider = self.provider.clone();
                            self.sessions[idx].update_context_window(
                                self.config.resolve_context_window(
                                    &self.provider,
                                    &self.model,
                                    &qm_map,
                                ),
                            );
                        }
                        vec![self.emit_status_update()]
                    }
                    Err(e) => vec![CoreEvent::Error {
                        message: CompactString::new(format!("Failed to switch provider: {e}")),
                    }],
                }
            }
            UserAction::SetMode { mode } => {
                let new_mode = SecurityMode::from_str(&mode).unwrap_or(SecurityMode::Standard);
                self.mode = new_mode;
                if let Some(ref perm) = self.permission {
                    perm.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .set_mode(new_mode);
                }
                vec![self.emit_status_update()]
            }
            UserAction::AddFile { path } => {
                let path = resolve_path(&path);
                if !path.exists() || !path.is_file() {
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!("file not found: {}", path.display())),
                    }];
                }
                let canonical = path.canonicalize().unwrap_or(path);
                if self.context.extra_files.contains(&canonical) {
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!(
                            "already added: {}",
                            canonical.display()
                        )),
                    }];
                }
                let size = std::fs::metadata(&canonical).map(|m| m.len()).unwrap_or(0);
                self.context.extra_files.push(canonical.clone());
                if let Err(e) = self.rebuild_agent().await {
                    return vec![CoreEvent::Error {
                        message: CompactString::new(format!("rebuild failed: {e}")),
                    }];
                }
                vec![CoreEvent::CommandOutput {
                    text: CompactString::from(format!(
                        "added: {} ({}B)",
                        canonical.display(),
                        size
                    )),
                }]
            }
            UserAction::DropFile { path } => {
                let path = resolve_path(&path);
                let canonical = path.canonicalize().unwrap_or(path);
                if let Some(i) = self
                    .context
                    .extra_files
                    .iter()
                    .position(|f| *f == canonical)
                {
                    self.context.extra_files.remove(i);
                    if let Err(e) = self.rebuild_agent().await {
                        return vec![CoreEvent::Error {
                            message: CompactString::new(format!("rebuild failed: {e}")),
                        }];
                    }
                    vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!("dropped: {}", canonical.display())),
                    }]
                } else {
                    vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!(
                            "not in context: {}",
                            canonical.display()
                        )),
                    }]
                }
            }
            UserAction::DropAllFiles => {
                let count = self.context.extra_files.len();
                self.context.extra_files.clear();
                if count > 0 {
                    if let Err(e) = self.rebuild_agent().await {
                        return vec![CoreEvent::Error {
                            message: CompactString::new(format!("rebuild failed: {e}")),
                        }];
                    }
                }
                vec![CoreEvent::CommandOutput {
                    text: CompactString::from(format!("dropped {} file(s)", count)),
                }]
            }
            UserAction::RunSlashCommand { command } => self.handle_slash_command(&command).await,
            UserAction::ReloadConfig => {
                let (cfg, _) = crate::config::load();
                self.config = cfg;
                if let Err(e) = self.rebuild_agent().await {
                    return vec![CoreEvent::Error {
                        message: CompactString::new(format!("rebuild failed: {e}")),
                    }];
                }
                vec![CoreEvent::ConfigChanged, self.emit_status_update()]
            }
            UserAction::PermissionResponse { id, allow } => {
                if let Some(reply) = self.pending_permissions.remove(&id) {
                    let decision = if allow {
                        UserDecision::AllowOnce
                    } else {
                        UserDecision::Deny
                    };
                    let _ = reply.send(decision);
                }
                vec![]
            }
            UserAction::Quit => {
                vec![CoreEvent::Error {
                    message: CompactString::from("quit"),
                }]
            }
            _ => {
                vec![CoreEvent::Error {
                    message: CompactString::from("Not yet implemented"),
                }]
            }
        }
    }

    /// Handle a SendMessage: build history, spawn the agent runner, and
    /// forward AgentEvents as CoreEvents via the event channel.
    async fn handle_send_message(&mut self, text: CompactString) -> Vec<CoreEvent> {
        // Cancel any currently running task
        if let Some(handle) = self.current_task.take() {
            handle.abort();
        }

        // Ensure we have a session
        if self.current_session_index.is_none() {
            let qm_map = crate::config::quick_models_map(&self.config);
            let session = Session::new(
                &self.provider,
                &self.model,
                self.config
                    .resolve_context_window(&self.provider, &self.model, &qm_map),
                "",
            );
            self.sessions.push(session);
            self.current_session_index = Some(self.sessions.len() - 1);
        }

        let session_idx = self.current_session_index.unwrap();

        // Add user message to session
        {
            let session = &mut self.sessions[session_idx];
            session.add_message(MessageRole::User, text.as_str());
        }

        // Build history from session
        let history = runner::convert_history(&self.sessions[session_idx]);

        // Get the agent (clone since spawn_runner consumes)
        let agent = match &self.agent {
            Some(a) => a.clone(),
            None => {
                return vec![CoreEvent::Error {
                    message: CompactString::from("Agent not built"),
                }];
            }
        };

        let retry_config: RetryConfig = self.config.retry.clone();

        // Spawn the agent runner
        let mut agent_runner = agent.spawn_runner(text.to_string(), history, retry_config).await;

        // Signal that the agent has started
        let _ = self.event_tx.send(CoreEvent::AgentStarted);

        // Spawn a forwarding task that converts AgentEvent -> CoreEvent
        let event_tx = self.event_tx.clone();
        let session_id = self.sessions[session_idx].id.clone();

        let handle = tokio::spawn(async move {
            let mut full_response = String::new();
            while let Some(agent_event) = agent_runner.event_rx.recv().await {
                match agent_event {
                    AgentEvent::Token(t) => {
                        full_response.push_str(&t);
                        let _ = event_tx.send(CoreEvent::StreamingDelta { text: t });
                    }
                    AgentEvent::Reasoning(t) => {
                        let _ = event_tx.send(CoreEvent::ReasoningDelta { text: t });
                    }
                    AgentEvent::ToolCall { name, args } => {
                        let _ = event_tx.send(CoreEvent::ToolCall { name, args });
                    }
                    AgentEvent::ToolResult { name, output } => {
                        let _ = event_tx.send(CoreEvent::ToolResult { name, output });
                    }
                    AgentEvent::SubagentToolCall { name, args } => {
                        let _ = event_tx.send(CoreEvent::SubagentToolCall { name, args });
                    }
                    AgentEvent::Error(msg) => {
                        let _ = event_tx.send(CoreEvent::Error { message: msg });
                    }
                    AgentEvent::Retrying { attempt, max } => {
                        let _ = event_tx.send(CoreEvent::Retrying { attempt, max });
                    }
                    AgentEvent::CompletionCall {
                        input_tokens,
                        output_tokens,
                        cached_input_tokens,
                        cache_creation_input_tokens,
                    } => {
                        let _ = event_tx.send(CoreEvent::CompletionCall {
                            input_tokens,
                            output_tokens,
                            cached_input_tokens,
                            cache_creation_input_tokens,
                        });
                    }
                    AgentEvent::Done {
                        response,
                        input_tokens,
                        output_tokens,
                        cached_input_tokens,
                        cache_creation_input_tokens,
                    } => {
                        // Save assistant response to session
                        // (The forwarding task can't access the engine's sessions,
                        // so the final response is sent and the GUI/engine loop
                        // can persist it. For now we send the event.)
                        let final_response = if response.is_empty() {
                            CompactString::from(full_response.as_str())
                        } else {
                            response
                        };
                        let _ = event_tx.send(CoreEvent::MessageComplete {
                            response: final_response,
                            input_tokens,
                            output_tokens,
                            cached_input_tokens,
                            cache_creation_input_tokens,
                        });
                        let _ = event_tx.send(CoreEvent::AgentStopped);
                        let _ = event_tx.send(CoreEvent::SessionChanged {
                            session_id: session_id.clone(),
                        });
                        return;
                    }
                }
            }
            // Stream ended without Done
            let _ = event_tx.send(CoreEvent::MessageComplete {
                response: CompactString::new(""),
                input_tokens: 0,
                output_tokens: 0,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
            });
            let _ = event_tx.send(CoreEvent::AgentStopped);
        });

        self.current_task = Some(handle);

        // Return empty - events come asynchronously via the channel
        vec![]
    }

    /// Handle slash commands that the GUI can dispatch.
    async fn handle_slash_command(&mut self, command: &str) -> Vec<CoreEvent> {
        let parts: Vec<&str> = command.trim().splitn(3, ' ').collect();
        if parts.is_empty() {
            return vec![];
        }
        let cmd = parts[0];

        match cmd {
            "/help" => {
                vec![CoreEvent::CommandOutput {
                    text: CompactString::from(
                        "Commands: /clear /undo /mode /model /provider /add /drop /sessions /quit",
                    ),
                }]
            }
            "/clear" | "/new" => {
                if let Some(idx) = self.current_session_index {
                    let session = &mut self.sessions[idx];
                    session.messages.clear();
                    session.total_estimated_tokens = 0;
                    session.reset_calibration();
                    session.compactions.clear();
                    self.context.chain_declined.clear();
                }
                self.emit_session_history()
            }
            "/undo" => {
                if let Some(idx) = self.current_session_index {
                    let session = &mut self.sessions[idx];
                    let removed = undo_last_exchange(session);
                    if removed == 0 {
                        return vec![CoreEvent::CommandOutput {
                            text: CompactString::from("nothing to undo"),
                        }];
                    }
                }
                self.emit_session_history()
            }
            "/mode" => {
                if parts.len() < 2 {
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!("current mode: {}", self.mode)),
                    }];
                }
                let mode_str = parts[1];
                match SecurityMode::from_str(mode_str) {
                    Some(new_mode) => {
                        self.mode = new_mode;
                        if let Some(ref perm) = self.permission {
                            perm.lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .set_mode(new_mode);
                        }
                        vec![self.emit_status_update()]
                    }
                    None => vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!(
                            "unknown mode: '{}'. Valid: standard, restrictive, readonly, guarded, yolo",
                            mode_str
                        )),
                    }],
                }
            }
            "/model" => {
                if parts.len() < 2 {
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!("current model: {}", self.model)),
                    }];
                }
                let new_model = CompactString::from(parts[1].trim());
                self.model = new_model.clone();
                if let Some(idx) = self.current_session_index {
                    let qm_map = crate::config::quick_models_map(&self.config);
                    self.sessions[idx].model = new_model.clone();
                    self.sessions[idx].update_context_window(self.config.resolve_context_window(
                        &self.provider,
                        &new_model,
                        &qm_map,
                    ));
                }
                match self.rebuild_agent().await {
                    Ok(()) => vec![self.emit_status_update()],
                    Err(e) => vec![CoreEvent::Error {
                        message: CompactString::new(format!("Failed to switch model: {e}")),
                    }],
                }
            }
            "/provider" => {
                if parts.len() < 2 {
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!("current provider: {}", self.provider)),
                    }];
                }
                let new_provider = parts[1].trim();
                // Default the model for the new provider
                if let Some((default_model, _)) =
                    provider::default_model_for_provider(new_provider, &self.config)
                {
                    self.model = CompactString::new(&default_model);
                    if let Some(idx) = self.current_session_index {
                        self.sessions[idx].model = self.model.clone();
                    }
                }
                match self.rebuild_agent_with_provider(new_provider).await {
                    Ok(()) => {
                        if let Some(idx) = self.current_session_index {
                            let qm_map = crate::config::quick_models_map(&self.config);
                            self.sessions[idx].provider = self.provider.clone();
                            self.sessions[idx].update_context_window(
                                self.config.resolve_context_window(
                                    &self.provider,
                                    &self.model,
                                    &qm_map,
                                ),
                            );
                        }
                        vec![self.emit_status_update()]
                    }
                    Err(e) => vec![CoreEvent::Error {
                        message: CompactString::new(format!("Failed to switch provider: {e}")),
                    }],
                }
            }
            "/add" => {
                if parts.len() < 2 {
                    let files: Vec<String> = self
                        .context
                        .extra_files
                        .iter()
                        .map(|f| {
                            let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                            format!("  {} ({}B)", f.display(), size)
                        })
                        .collect();
                    let text = if files.is_empty() {
                        "no files added (use /add <path>)".to_string()
                    } else {
                        format!("added files:\n{}", files.join("\n"))
                    };
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from(text),
                    }];
                }
                let path = resolve_path(parts[1]);
                if !path.exists() || !path.is_file() {
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!("file not found: {}", path.display())),
                    }];
                }
                let canonical = path.canonicalize().unwrap_or(path);
                if self.context.extra_files.contains(&canonical) {
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!(
                            "already added: {}",
                            canonical.display()
                        )),
                    }];
                }
                let size = std::fs::metadata(&canonical).map(|m| m.len()).unwrap_or(0);
                self.context.extra_files.push(canonical.clone());
                if let Err(e) = self.rebuild_agent().await {
                    return vec![CoreEvent::Error {
                        message: CompactString::new(format!("rebuild failed: {e}")),
                    }];
                }
                vec![CoreEvent::CommandOutput {
                    text: CompactString::from(format!(
                        "added: {} ({}B)",
                        canonical.display(),
                        size
                    )),
                }]
            }
            "/drop" | "/drop-all" => {
                if cmd == "/drop-all" || (cmd == "/drop" && parts.len() < 2) {
                    let count = self.context.extra_files.len();
                    self.context.extra_files.clear();
                    if count > 0 {
                        let _ = self.rebuild_agent().await;
                    }
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!("dropped {} file(s)", count)),
                    }];
                }
                let path = resolve_path(parts[1]);
                let canonical = path.canonicalize().unwrap_or(path);
                if let Some(i) = self
                    .context
                    .extra_files
                    .iter()
                    .position(|f| *f == canonical)
                {
                    self.context.extra_files.remove(i);
                    let _ = self.rebuild_agent().await;
                    vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!("dropped: {}", canonical.display())),
                    }]
                } else {
                    vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!(
                            "not in context: {}",
                            canonical.display()
                        )),
                    }]
                }
            }
            "/sessions" => {
                let lines: Vec<String> = self
                    .sessions
                    .iter()
                    .map(|s| {
                        let name_part = if s.name.is_empty() {
                            String::new()
                        } else {
                            format!("  [{}]", s.name)
                        };
                        format!(
                            "  {}  {}msgs  {}{}",
                            &s.id[..8.min(s.id.len())],
                            s.messages.len(),
                            s.model,
                            name_part
                        )
                    })
                    .collect();
                vec![CoreEvent::CommandOutput {
                    text: CompactString::from(format!(
                        "sessions ({}):\n{}",
                        self.sessions.len(),
                        lines.join("\n")
                    )),
                }]
            }
            "/rename" => {
                if parts.len() < 2 {
                    return vec![CoreEvent::CommandOutput {
                        text: CompactString::from("usage: /rename <name>"),
                    }];
                }
                let new_name = parts[1..].join(" ");
                if let Some(idx) = self.current_session_index {
                    self.sessions[idx].name = CompactString::from(new_name.clone());
                }
                let mut events = self.emit_session_list_updated();
                events.push(CoreEvent::CommandOutput {
                    text: CompactString::from(format!("session renamed to \"{}\"", new_name)),
                });
                events
            }
            "/history" => match crate::session::chat_history::load_history() {
                Ok(entries) => {
                    if entries.is_empty() {
                        vec![CoreEvent::CommandOutput {
                            text: CompactString::from("no chat history"),
                        }]
                    } else {
                        let lines: Vec<String> = entries
                            .iter()
                            .rev()
                            .take(10)
                            .rev()
                            .map(|e| {
                                let preview: String = e.content.chars().take(80).collect();
                                format!("  {}", preview)
                            })
                            .collect();
                        vec![CoreEvent::CommandOutput {
                            text: CompactString::from(format!(
                                "global chat history ({} entries):\n{}",
                                entries.len(),
                                lines.join("\n")
                            )),
                        }]
                    }
                }
                Err(e) => vec![CoreEvent::CommandOutput {
                    text: CompactString::from(format!("failed to load chat history: {}", e)),
                }],
            },
            "/reasoning" | "/thinking" => {
                self.reasoning_enabled = !self.reasoning_enabled;
                match self.rebuild_agent().await {
                    Ok(()) => vec![CoreEvent::CommandOutput {
                        text: CompactString::from(format!(
                            "reasoning: {}",
                            if self.reasoning_enabled { "on" } else { "off" }
                        )),
                    }],
                    Err(e) => vec![CoreEvent::Error {
                        message: CompactString::new(format!("rebuild failed: {e}")),
                    }],
                }
            }
            _ => {
                vec![CoreEvent::CommandOutput {
                    text: CompactString::from(format!("unknown command: {}", cmd)),
                }]
            }
        }
    }

    /// Save the current session to disk.
    pub fn save_current_session(&self) {
        if let Some(session) = self.current_session() {
            let _ = crate::session::storage::save_session(session);
        }
    }

    /// Handle an incoming permission ask request from the agent's tools.
    /// Stores the oneshot reply and emits a PermissionNeeded event.
    pub fn handle_ask_request(&mut self, request: AskRequest) {
        let id = self.next_permission_id;
        self.next_permission_id += 1;
        let tool_name = request.tool.clone();
        let args = request.input.clone();
        self.pending_permissions.insert(id, request.reply);
        let _ = self.event_tx.send(CoreEvent::PermissionNeeded {
            id,
            tool_name,
            args,
        });
    }

    fn emit_session_list_updated(&self) -> Vec<CoreEvent> {
        let sessions: Vec<SessionInfo> = self
            .sessions
            .iter()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                name: s.name.clone(),
                model: s.model.clone(),
                provider: s.provider.clone(),
                message_count: s.messages.len(),
                created_at: s.created_at.clone(),
            })
            .collect();
        vec![CoreEvent::SessionListUpdated { sessions }]
    }

    fn emit_session_history(&self) -> Vec<CoreEvent> {
        let messages = if let Some(idx) = self.current_session_index {
            self.sessions[idx]
                .messages
                .iter()
                .map(|m| ChatMessage {
                    role: match m.role {
                        MessageRole::User => "user".to_string(),
                        MessageRole::Assistant => "assistant".to_string(),
                        MessageRole::System => "system".to_string(),
                        MessageRole::ToolCall => "tool_call".to_string(),
                        MessageRole::ToolResult => "tool_result".to_string(),
                        MessageRole::SubagentToolCall => "subagent_tool_call".to_string(),
                    },
                    content: m.content.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        vec![CoreEvent::SessionHistory { messages }]
    }

    fn emit_status_update(&self) -> CoreEvent {
        CoreEvent::StatusUpdate {
            model: self.model.clone(),
            provider: self.provider.clone(),
            tokens_used: self.tokens_used,
            mode: self.mode.to_string(),
        }
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Build a permission checker.
fn build_permission_checker(
    cli: &Cli,
    cfg: &Config,
    mode: SecurityMode,
) -> (
    Option<PermCheck>,
    Option<AskSender>,
    Option<mpsc::Receiver<AskRequest>>,
) {
    let no_tools = cli.resolve_no_tools(cfg);
    if no_tools || cli.dangerously_skip_permissions {
        return (None, None, None);
    }

    let perm_config = cfg.build_permission_config();
    let permission_modes = cfg.permission_modes.clone();
    let checker = PermissionChecker::new(&perm_config, mode, None, permission_modes);
    let perm: PermCheck = std::sync::Arc::new(std::sync::Mutex::new(checker));

    let (ask_tx, ask_rx) = mpsc::channel(64);
    (Some(perm), Some(ask_tx), Some(ask_rx))
}

/// Connect configured MCP servers.
#[cfg(feature = "mcp")]
async fn connect_mcp(cfg: &Config) -> Option<crate::extras::mcp::McpClientManager> {
    let servers = cfg.mcp_servers.as_ref()?;
    if servers.is_empty() {
        return None;
    }
    let manager = crate::extras::mcp::McpClientManager::connect_all(servers).await;
    for notice in &manager.notices {
        tracing::info!("{}", notice);
    }
    Some(manager)
}

/// Resolve a relative path against the current working directory.
fn resolve_path(s: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(s);
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(p)
    }
}

/// Remove the last user+assistant exchange from the session.
fn undo_last_exchange(session: &mut Session) -> usize {
    let mut removed = 0;
    // Remove trailing assistant message(s)
    while let Some(last) = session.messages.last() {
        if last.role == MessageRole::Assistant {
            session.messages.pop();
            removed += 1;
        } else {
            break;
        }
    }
    // Remove the trailing user message
    if let Some(last) = session.messages.last() {
        if last.role == MessageRole::User {
            session.messages.pop();
            removed += 1;
        }
    }
    removed
}
