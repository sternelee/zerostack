use std::thread;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use zerostack_core::config::Config;
use zerostack_core::engine::CoreEngine;
use zerostack_core::events::{CoreEvent, UserAction};
use zerostack_core::permission::SecurityMode;

pub struct GuiBridge {
    action_tx: UnboundedSender<UserAction>,
    event_rx: UnboundedReceiver<CoreEvent>,
    _runtime_thread: thread::JoinHandle<()>,
}

impl GuiBridge {
    pub fn new(config: Config, model: String, provider: String, mode: SecurityMode) -> Self {
        let (action_tx, mut action_rx) = unbounded_channel::<UserAction>();
        let (event_tx, event_rx) = unbounded_channel::<CoreEvent>();

        let runtime_thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build tokio runtime");

            rt.block_on(async move {
                let mut engine = CoreEngine::new(config, model.into(), provider.into(), mode);

                while let Some(action) = action_rx.recv().await {
                    let events = engine.handle_action(action).await;
                    for event in events {
                        if matches!(&event, CoreEvent::Error { message } if message.as_str() == "quit")
                        {
                            let _ = event_tx.send(event);
                            return;
                        }
                        let _ = event_tx.send(event);
                    }
                }
            });
        });

        Self {
            action_tx,
            event_rx,
            _runtime_thread: runtime_thread,
        }
    }

    pub fn send_action(&self, action: UserAction) {
        let _ = self.action_tx.send(action);
    }

    pub fn poll_events(&mut self) -> Vec<CoreEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        events
    }
}
