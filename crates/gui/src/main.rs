use std::sync::Mutex;
use std::thread;

use makepad_widgets::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use zerostack_core::config;
use zerostack_core::engine::CoreEngine;
use zerostack_core::events::{CoreEvent, UserAction};
use zerostack_core::permission::SecurityMode;

app_main!(App);

// ─── Global Bridge ─────────────────────────────────────────────────────────

static BRIDGE: Mutex<Option<GuiBridge>> = Mutex::new(None);
static PENDING_SEND: Mutex<Option<String>> = Mutex::new(None);

pub struct GuiBridge {
    pub action_tx: UnboundedSender<UserAction>,
    pub event_rx: UnboundedReceiver<CoreEvent>,
    pub tokens_used: u64,
    _runtime_thread: thread::JoinHandle<()>,
}

impl GuiBridge {
    pub fn new(model: &str, provider: &str) -> Self {
        let (action_tx, mut action_rx) = unbounded_channel();
        let (event_tx, event_rx) = unbounded_channel();
        let m = model.to_string();
        let p = provider.to_string();

        let runtime_thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");

            rt.block_on(async move {
                let (cfg, _) = config::load();
                let mut engine = CoreEngine::new(
                    cfg,
                    m.as_str().into(),
                    p.as_str().into(),
                    SecurityMode::Yolo,
                );

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
            tokens_used: 0,
            _runtime_thread: runtime_thread,
        }
    }

    pub fn poll(&mut self) -> Vec<CoreEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        events
    }
}

// ─── App ───────────────────────────────────────────────────────────────────

script_mod! {
    use mod.prelude.widgets.*

    let app = startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(1024, 700)
                window.title: "zerostack"

                body +: {
                    width: Fill, height: Fill
                    flow: Right
                    spacing: 0.0
                    show_bg: true
                    draw_bg.color: #1A1B26

                    // ── Sidebar ──────────────────────────────────────
                    RoundedView {
                        width: 220.0, height: Fill
                        draw_bg.color: #1F2030
                        draw_bg.radius: 0.0
                        flow: Down
                        padding: {top: 8.0, bottom: 8.0}
                        spacing: 4.0

                        View {
                            width: Fill, height: Fit
                            padding: {left: 12.0, right: 12.0, top: 4.0, bottom: 4.0}
                            flow: Right
                            align: {y: 0.5}
                            Label {
                                text: "Sessions"
                                draw_text.color: #6C7086
                                draw_text.text_style.font_size: 11.0
                            }
                            View { width: Fill, height: 0.0 }
                            Button {
                                text: "+"
                                width: 28.0, height: 28.0
                                draw_bg.color: #7C6FF0
                                draw_bg.radius: 4.0
                            }
                        }
                    }

                    // ── Main Content ─────────────────────────────────
                    View {
                        width: Fill, height: Fill
                        flow: Down
                        spacing: 0.0

                        View {
                            width: Fill, height: 40.0
                            show_bg: true
                            draw_bg.color: #1A1B26
                            padding: {left: 16.0, right: 16.0}
                            flow: Right
                            align: {y: 0.5}
                            Label {
                                text: "zerostack"
                                draw_text.color: #CDD6F4
                                draw_text.text_style.font_size: 14.0
                            }
                            View { width: Fill, height: 0.0 }
                            header_model := Label {
                                text: "claude-sonnet"
                                draw_text.color: #6C7086
                                draw_text.text_style.font_size: 12.0
                            }
                        }

                        chat_area := View {
                            width: Fill, height: Fill
                            flow: Down
                            spacing: 4.0
                            padding: {left: 16.0, right: 16.0, top: 12.0, bottom: 12.0}

                            chat_text := Markdown {
                                width: Fill, height: Fit
                                selectable: false
                                body: "# Welcome to zerostack GUI\n\nType a message below to get started."
                            }
                        }

                        // ── Input Bar ─────────────────────────────
                        View {
                            width: Fill, height: Fit
                            show_bg: true
                            draw_bg.color: #1F2030
                            flow: Down
                            spacing: 0.0

                            View {
                                width: Fill, height: 60.0
                                padding: {left: 16.0, right: 16.0, top: 12.0, bottom: 12.0}
                                flow: Right
                                spacing: 8.0
                                align: {y: 0.5}

                                input_field := TextInput {
                                    width: Fill, height: 36.0
                                    empty_text: "Type a message... (Enter to send)"
                                    draw_bg.color: #2D2E3F
                                    draw_bg.radius: 8.0
                                    draw_text.color: #CDD6F4
                                    draw_text.text_style.font_size: 14.0
                                }

                                send_btn := Button {
                                    text: "Send"
                                    width: 72.0, height: 36.0
                                    draw_bg.color: #7C6FF0
                                    draw_bg.radius: 8.0
                                    draw_text.color: #FFFFFF
                                    draw_text.text_style.font_size: 13.0
                                    on_click: || {
                                        let text = input_field.text()
                                        if text != "" {
                                            crate::app::set_pending(text)
                                            input_field.set_text("")
                                            input_field.set_key_focus()
                                        }
                                    }
                                }
                            }

                            View {
                                width: Fill, height: 28.0
                                padding: {left: 16.0, right: 16.0}
                                flow: Right
                                spacing: 16.0
                                align: {y: 0.5}
                                show_bg: true
                                draw_bg.color: #1A1B26

                                status_model := Label {
                                    text: "claude-sonnet"
                                    draw_text.color: #6C7086
                                    draw_text.text_style.font_size: 11.0
                                }
                                View { width: Fill, height: 0.0 }
                                status_tokens := Label {
                                    text: "0 tokens"
                                    draw_text.color: #6C7086
                                    draw_text.text_style.font_size: 11.0
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    app
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    frame_count: u64,
}

impl App {
    fn ensure_bridge() {
        let mut guard = BRIDGE.lock().unwrap();
        if guard.is_none() {
            *guard = Some(GuiBridge::new("claude-sonnet", "anthropic"));
        }
    }

    fn poll_and_render(&mut self, cx: &mut Cx) {
        self.frame_count += 1;
        if self.frame_count == 1 {
            Self::ensure_bridge();
        }

        // Send pending message
        if let Ok(mut pending) = PENDING_SEND.lock() {
            if let Some(text) = pending.take() {
                if let Ok(guard) = BRIDGE.lock() {
                    if let Some(ref bridge) = *guard {
                        let _ = bridge.action_tx.send(UserAction::SendMessage {
                            text: text.as_str().into(),
                        });
                    }
                }
            }
        }

        // Poll events
        let events = {
            let mut guard = BRIDGE.lock().unwrap();
            match guard.as_mut() {
                Some(bridge) => bridge.poll(),
                None => return,
            }
        };

        if events.is_empty() {
            return;
        }

        // Build display
        let mut lines = Vec::new();
        for event in &events {
            match event {
                CoreEvent::StreamingDelta { text } => {
                    lines.push(text.to_string());
                }
                CoreEvent::ToolCall { name, args } => {
                    lines.push(format!("\n## 🔧 {}\n{}", name, args));
                }
                CoreEvent::ToolResult { name, output } => {
                    let out = if output.len() > 500 {
                        format!("{}...", &output[..500])
                    } else {
                        output.to_string()
                    };
                    lines.push(format!("\n```\n{}: {}\n```\n", name, out));
                }
                CoreEvent::MessageComplete {
                    input_tokens,
                    output_tokens,
                    ..
                } => {
                    if let Ok(mut guard) = BRIDGE.lock() {
                        if let Some(ref mut bridge) = *guard {
                            bridge.tokens_used += input_tokens + output_tokens;
                        }
                    }
                }
                CoreEvent::Error { message } => {
                    if message.as_str() != "quit" {
                        lines.push(format!("\n❌ {}", message));
                    }
                }
                _ => {}
            }
        }

        if lines.is_empty() {
            return;
        }

        let display = lines.join("\n");
        let path = &[live_id!(main_window), live_id!(body), live_id!(chat_text)];

        let chat = self.ui.widget(cx, path);
        if !chat.is_empty() {
            chat.set_text(cx, &display);
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::NextFrame(_)) {
            self.poll_and_render(cx);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

// ─── Script helpers ────────────────────────────────────────────────────────

pub fn set_pending(text: String) {
    if let Ok(mut guard) = PENDING_SEND.lock() {
        *guard = Some(text);
    }
}
