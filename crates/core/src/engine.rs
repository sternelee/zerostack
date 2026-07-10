use compact_str::CompactString;

use crate::config::Config;
use crate::events::{CoreEvent, InitialState, SessionInfo, UserAction};
use crate::permission::SecurityMode;
use crate::session::Session;

pub struct CoreEngine {
    config: Config,
    sessions: Vec<Session>,
    current_session_index: Option<usize>,
    model: CompactString,
    provider: CompactString,
    mode: SecurityMode,
    permission_request_id: u64,
}

impl CoreEngine {
    pub fn new(
        config: Config,
        model: CompactString,
        provider: CompactString,
        mode: SecurityMode,
    ) -> Self {
        Self {
            config,
            sessions: Vec::new(),
            current_session_index: None,
            model,
            provider,
            mode,
            permission_request_id: 0,
        }
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
            UserAction::SendMessage { text: _ } => {
                vec![CoreEvent::Error {
                    message: CompactString::from("Agent runner not yet wired to CoreEngine"),
                }]
            }
            UserAction::CreateSession { name } => {
                let session_name = name.unwrap_or_else(|| CompactString::from("New Session"));
                let session = Session::new(
                    &self.provider,
                    &self.model,
                    self.config.resolve_context_window(
                        &self.provider,
                        &self.model,
                        &Default::default(),
                    ),
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
