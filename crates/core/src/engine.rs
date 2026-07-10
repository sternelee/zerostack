use compact_str::CompactString;
use tokio::sync::mpsc;

use crate::agent::runner;
use crate::agent::tools;
use crate::cli::Cli;
use crate::config::Config;
use crate::context::{self, ContextFiles};
use crate::event::AgentEvent;
use crate::events::{CoreEvent, InitialState, SessionInfo, UserAction};
use crate::permission::SecurityMode;
use crate::permission::ask::{AskRequest, AskSender};
use crate::permission::checker::{PermCheck, PermissionChecker};
use crate::provider::{self, AnyAgent};
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
    context: ContextFiles,
    cli: Cli,
    /// Abort handle for the currently running agent forwarding task,
    /// so a new message or CancelStream can interrupt it.
    current_task: Option<tokio::task::JoinHandle<()>>,
}

impl CoreEngine {
    /// Build a fully-configured engine with a ready-to-run agent.
    /// This performs all the setup that `src/main.rs` does, but simplified
    /// for the GUI use case (default Cli, yolo mode, no MCP prompts).
    pub async fn build_default(
        model: CompactString,
        provider_name: CompactString,
        mode: SecurityMode,
    ) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<CoreEvent>)> {
        let (cfg, _is_first_startup) = crate::config::load();
        let cli = Cli::default();
        let context = context::load(cli.resolve_no_context_files(&cfg));

        // Sandbox + edit system
        let sandbox = Sandbox::new(
            cli.resolve_sandbox(&cfg),
            &cli.resolve_sandbox_backend(&cfg),
        )
        .with_shell(&cli.resolve_shell(&cfg));
        let edit_system = cli.resolve_edit_system(&cfg);
        tools::set_edit_system(edit_system);
        tools::set_deny_repeated_reads(cfg.deny_repeated_reads.unwrap_or(true));

        // Permission checker
        let (permission, ask_tx, _ask_rx) = build_permission_checker(&cli, &cfg, mode);

        // Build provider client + agent
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

        let temperature = crate::config::resolve_temperature(&cli, &cfg, &model);
        let extra_body = crate::config::resolve_extra_body(&cfg, &model);

        #[cfg(feature = "mcp")]
        let mcp_manager = connect_mcp(&cfg).await;

        let completion_model = client.completion_model(model.to_string());
        let agent = provider::build_agent(
            completion_model,
            &cli,
            &cfg,
            &context,
            permission,
            ask_tx,
            sandbox,
            true, // reasoning_enabled
            temperature,
            extra_body,
            #[cfg(feature = "mcp")]
            mcp_manager.as_ref(),
        )
        .await;

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let engine = Self {
            config: cfg,
            sessions: vec![session],
            current_session_index: Some(0),
            model,
            provider: provider_name,
            mode,
            event_tx,
            agent: Some(agent),
            context,
            cli,
            current_task: None,
        };

        Ok((engine, event_rx))
    }

    pub fn initial_state(&self) -> InitialState {
        let sessions: Vec<SessionInfo> = self
            .sessions
            .iter()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                name: s.name.clone(),
                model: s.model.clone(),
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
                self.emit_session_list_updated()
            }
            UserAction::SwitchSession { session_id } => {
                if let Some(idx) = self.sessions.iter().position(|s| s.id == session_id) {
                    self.current_session_index = Some(idx);
                    vec![CoreEvent::SessionChanged { session_id }]
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
                self.emit_session_list_updated()
            }
            UserAction::RenameSession { session_id, name } => {
                if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    session.name = name;
                }
                self.emit_session_list_updated()
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
        let mut agent_runner = agent.spawn_runner(text.to_string(), history, retry_config);

        // Spawn a forwarding task that converts AgentEvent -> CoreEvent
        let event_tx = self.event_tx.clone();
        let session_id = self.sessions[session_idx].id.clone();

        let handle = tokio::spawn(async move {
            while let Some(agent_event) = agent_runner.event_rx.recv().await {
                match agent_event {
                    AgentEvent::Token(t) => {
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
                        let _ = event_tx.send(CoreEvent::MessageComplete {
                            response: response.clone(),
                            input_tokens,
                            output_tokens,
                            cached_input_tokens,
                            cache_creation_input_tokens,
                        });
                        // Tag the session id so the frontend knows which session
                        let _ = event_tx.send(CoreEvent::SessionChanged {
                            session_id: session_id.clone(),
                        });
                        return;
                    }
                }
            }
            // Stream ended without Done - send an empty MessageComplete
            let _ = event_tx.send(CoreEvent::MessageComplete {
                response: CompactString::new(""),
                input_tokens: 0,
                output_tokens: 0,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
            });
        });

        self.current_task = Some(handle);

        // Return empty - events come asynchronously via the channel
        vec![]
    }

    fn emit_session_list_updated(&self) -> Vec<CoreEvent> {
        let sessions: Vec<SessionInfo> = self
            .sessions
            .iter()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                name: s.name.clone(),
                model: s.model.clone(),
                message_count: s.messages.len(),
                created_at: s.created_at.clone(),
            })
            .collect();
        vec![CoreEvent::SessionListUpdated { sessions }]
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Build a permission checker, mirroring `build_permission_checker` in
/// `src/main.rs` but accessible from the core crate.
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
