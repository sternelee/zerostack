//! Bridge between the GPUI foreground and the background [`CoreEngine`].
//!
//! The engine lives in its own OS thread with a tokio runtime, since `gpui` itself is
//! built on top of `smol` and we don't want to fight two executors. Communication is
//! strictly message-passing over `mpsc`: actions go in, [`CoreEvent`]s come out.
//!
//! [`CoreEngine`]: zerostack_core::engine::CoreEngine
use std::thread::JoinHandle;
use std::time::Duration;

use compact_str::CompactString;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use zerostack_core::engine::CoreEngine;
use zerostack_core::events::CoreEvent;
use zerostack_core::events::UserAction;
use zerostack_core::permission::SecurityMode;

/// Owns the engine thread and the two halves of the channel. Cheap to clone thanks to
/// internal `tokio::sync` mpsc, which is already `Arc`-backed.
pub struct GuiBridge {
    action_tx: UnboundedSender<UserAction>,
    event_rx: UnboundedReceiver<CoreEvent>,
    _runtime_thread: JoinHandle<()>,
}

impl GuiBridge {
    /// Launch the engine in a new OS thread backed by a single-threaded tokio runtime.
    /// Errors during engine build are surfaced through the event channel rather than
    /// being swallowed, so the GUI can display them in-line.
    pub fn launch(model: &str, provider: &str, mode: SecurityMode) -> Self {
        let (action_tx, mut action_rx) = unbounded_channel::<UserAction>();
        let (event_tx, event_rx) = unbounded_channel::<CoreEvent>();

        let m = CompactString::new(model);
        let p = CompactString::new(provider);
        let event_tx_for_thread = event_tx.clone();
        let runtime_thread = std::thread::Builder::new()
            .name("zerostack-gui-engine".into())
            .spawn(move || run_event_loop(m, p, mode, &mut action_rx, &event_tx_for_thread))
            .expect("failed to spawn engine thread");

        Self {
            action_tx,
            event_rx,
            _runtime_thread: runtime_thread,
        }
    }

    /// Drain all events currently queued on the channel.
    pub fn poll(&mut self) -> Vec<CoreEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        events
    }

    /// Send a user action to the engine. Returns `false` if the receiver is gone.
    pub fn send(&self, action: UserAction) -> bool {
        self.action_tx.send(action).is_ok()
    }

    /// Tell the engine to quit. Drain one short tick so the final event lands on the
    /// channel before the thread exits.
    pub fn shutdown(&self) {
        let _ = self.action_tx.send(UserAction::Quit);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_event_loop(
    model: CompactString,
    provider: CompactString,
    mode: SecurityMode,
    action_rx: &mut UnboundedReceiver<UserAction>,
    event_tx: &UnboundedSender<CoreEvent>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = event_tx.send(CoreEvent::Error {
                message: CompactString::new(format!("tokio runtime build failed: {e}")),
            });
            return;
        }
    };

    rt.block_on(async move {
        // Push a placeholder status update before we touch the engine. This makes
        // the status bar populate even if engine construction below fails — the
        // user sees *something* (the chosen provider/model) instead of an empty
        // window.
        let _ = event_tx.send(CoreEvent::StatusUpdate {
            model: model.clone(),
            provider: provider.clone(),
            tokens_used: 0,
            mode: mode.to_string(),
        });

        let (mut engine, mut engine_event_rx, mut ask_rx) =
            match CoreEngine::build_default(model, provider, mode).await {
                Ok(triple) => triple,
                Err(e) => {
                    let _ = event_tx.send(CoreEvent::Error {
                        message: CompactString::new(format!("engine init failed: {e}")),
                    });
                    return;
                }
            };

        // Push the initial state so the GUI can render the sidebar / status bar.
        let initial_state = engine.initial_state();
        let _ = event_tx.send(CoreEvent::StatusUpdate {
            model: initial_state.model.clone(),
            provider: initial_state.provider.clone(),
            tokens_used: 0,
            mode: initial_state.mode.clone(),
        });

        // Merge in-memory engine state with on-disk sessions so the sidebar shows
        // every chat asset the user has stored. Cap at 100 to keep startup latency
        // predictable for users with large session histories.
        let disk_sessions =
            zerostack_core::session::storage::find_recent_sessions(100).unwrap_or_default();
        let mut all_sessions = initial_state.sessions.clone();
        for s in disk_sessions {
            if !all_sessions.iter().any(|info| info.id == s.id) {
                all_sessions.push(zerostack_core::events::SessionInfo {
                    id: s.id,
                    name: s.name,
                    model: s.model,
                    provider: s.provider,
                    message_count: s.messages.len(),
                    created_at: s.created_at,
                    working_dir: s.working_dir,
                    last_message: s
                        .messages
                        .last()
                        .map(|m| zerostack_core::engine::preview_text(&m.content))
                        .unwrap_or_default(),
                });
            }
        }
        let _ = event_tx.send(CoreEvent::SessionListUpdated {
            sessions: all_sessions,
        });
        if let Some(id) = initial_state.current_session_id {
            let _ = event_tx.send(CoreEvent::SessionChanged { session_id: id });
        }

        // Background disk poll: every couple of seconds, re-scan the on-disk
        // session directory and forward any *new* sessions to the GUI. This is
        // what makes the GUI's sidebar reflect CLI/TUI sessions started in
        // parallel — without it the sidebar would only see sessions this
        // engine instance knows about. The poll keeps a string fingerprint of
        // the disk listing so we only emit when something actually changed
        // (otherwise we'd wake up the renderer every 5 seconds for nothing).
        let poll_tx = event_tx.clone();
        tokio::spawn(async move {
            let mut last_fingerprint: Option<String> = None;
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
            // Skip the immediate first tick so we don't double-fetch what the
            // startup sync above just produced.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let Ok(disk) = zerostack_core::session::storage::find_recent_sessions(100) else {
                    continue;
                };
                let fingerprint = disk
                    .iter()
                    .map(|s| {
                        format!(
                            "{}|{}|{}",
                            s.id.as_str(),
                            s.messages.len(),
                            s.created_at.as_str()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if Some(&fingerprint) == last_fingerprint.as_ref() {
                    continue;
                }
                last_fingerprint = Some(fingerprint);
                let sessions: Vec<zerostack_core::events::SessionInfo> = disk
                    .into_iter()
                    .map(|s| zerostack_core::events::SessionInfo {
                        id: s.id,
                        name: s.name,
                        model: s.model,
                        provider: s.provider,
                        message_count: s.messages.len(),
                        created_at: s.created_at,
                        working_dir: s.working_dir,
                        last_message: s
                            .messages
                            .last()
                            .map(|m| m.content.clone())
                            .unwrap_or_default(),
                    })
                    .collect();
                let _ = poll_tx.send(CoreEvent::SessionListUpdated { sessions });
            }
        });

        loop {
            if let Some(ref mut rx) = ask_rx {
                while let Ok(request) = rx.try_recv() {
                    engine.handle_ask_request(request);
                }
            }

            tokio::select! {
                biased;
                action = action_rx.recv() => {
                    let Some(action) = action else { break };
                    let is_quit = matches!(action, UserAction::Quit);
                    let events = engine.handle_action(action).await;
                    for ev in events {
                        let _ = event_tx.send(ev);
                    }
                    if is_quit {
                        return;
                    }
                }
                event = engine_event_rx.recv() => {
                    match event {
                        Some(event) => {
                            if matches!(event, CoreEvent::MessageComplete { .. }) {
                                engine.save_current_session();
                            }
                            let _ = event_tx.send(event);
                        }
                        None => break,
                    }
                }
                ask = async {
                    match &mut ask_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(request) = ask {
                        engine.handle_ask_request(request);
                    }
                }
            }
        }
    });
}
