use std::sync::Mutex;
use std::thread;

use compact_str::CompactString;
use makepad_widgets::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use zerostack_core::engine::CoreEngine;
use zerostack_core::events::{CoreEvent, UserAction};
use zerostack_core::permission::SecurityMode;

app_main!(App);

// ─── Global Bridge ─────────────────────────────────────────────────────────

static BRIDGE: Mutex<Option<GuiBridge>> = Mutex::new(None);

pub struct GuiBridge {
    pub action_tx: UnboundedSender<UserAction>,
    pub event_rx: UnboundedReceiver<CoreEvent>,
    pub tokens_used: u64,
    _runtime_thread: thread::JoinHandle<()>,
}

impl GuiBridge {
    pub fn new(model: &str, provider: &str) -> Self {
        let (action_tx, mut action_rx) = unbounded_channel();
        let (gui_tx, gui_rx) = unbounded_channel::<CoreEvent>();
        let m = model.to_string();
        let p = provider.to_string();

        let runtime_thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");

            rt.block_on(async move {
                let (mut engine, mut engine_event_rx) = match CoreEngine::build_default(
                    m.as_str().into(),
                    p.as_str().into(),
                    SecurityMode::Yolo,
                )
                .await
                {
                    Ok(pair) => pair,
                    Err(e) => {
                        eprintln!("Failed to build CoreEngine: {e}");
                        let _ = gui_tx.send(CoreEvent::Error {
                            message: CompactString::new(format!("Engine init failed: {e}")),
                        });
                        return;
                    }
                };

                loop {
                    tokio::select! {
                        // Action from GUI
                        action = action_rx.recv() => {
                            let Some(action) = action else { break };
                            let is_quit = matches!(action, UserAction::Quit);
                            let events = engine.handle_action(action).await;
                            // For synchronous actions (session mgmt), forward
                            // returned events to the GUI.
                            for event in events {
                                let _ = gui_tx.send(event);
                            }
                            if is_quit {
                                return;
                            }
                        }
                        // Agent event from engine (async, for SendMessage)
                        event = engine_event_rx.recv() => {
                            match event {
                                Some(event) => { let _ = gui_tx.send(event); }
                                None => break,
                            }
                        }
                    }
                }
            });
        });

        Self {
            action_tx,
            event_rx: gui_rx,
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
                        padding: Inset { top: 8.0, bottom: 8.0 }
                        spacing: 4.0

                        View {
                            width: Fill, height: Fit
                            padding: Inset { left: 12.0, right: 12.0 }
                            flow: Right
                            align: Align { y: 0.5 }
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
                            padding: Inset { left: 16.0, right: 16.0 }
                            flow: Right
                            align: Align { y: 0.5 }
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
                            padding: Inset { left: 16.0, right: 16.0, top: 12.0, bottom: 12.0 }

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
                                padding: Inset { left: 16.0, right: 16.0, top: 12.0, bottom: 12.0 }
                                flow: Right
                                spacing: 8.0
                                align: Align { y: 0.5 }

                                input_field := TextInput {
                                    width: Fill, height: 36.0
                                    empty_text: "Type a message... (Enter to send)"
                                    submit_on_enter: true
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
                                }
                            }

                            View {
                                width: Fill, height: 28.0
                                padding: Inset { left: 16.0, right: 16.0 }
                                flow: Right
                                spacing: 16.0
                                align: Align { y: 0.5 }
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
    #[rust]
    accumulated_text: String,
}

impl App {
    fn ensure_bridge() {
        let mut guard = BRIDGE.lock().unwrap();
        if guard.is_none() {
            *guard = Some(GuiBridge::new("claude-sonnet", "anthropic"));
        }
    }

    fn send_input_to_engine(&mut self, cx: &mut Cx) {
        let input_path = &[live_id!(main_window), live_id!(body), live_id!(input_field)];
        let input = self.ui.widget(cx, input_path);
        if input.is_empty() {
            return;
        }
        let text = input.text();
        if text.is_empty() {
            return;
        }
        input.set_text(cx, "");
        input.set_key_focus(cx);
        // Show user message and clear for new response
        self.accumulated_text
            .push_str(&format!("## You\n\n{}\n\n## Assistant\n\n", text));
        Self::ensure_bridge();
        if let Ok(guard) = BRIDGE.lock() {
            if let Some(ref bridge) = *guard {
                let _ = bridge.action_tx.send(UserAction::SendMessage {
                    text: text.as_str().into(),
                });
            }
        }
    }

    fn poll_and_render(&mut self, cx: &mut Cx) {
        self.frame_count += 1;
        if self.frame_count == 1 {
            Self::ensure_bridge();
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

        let mut needs_redraw = false;
        for event in &events {
            match event {
                CoreEvent::StreamingDelta { text } => {
                    self.accumulated_text.push_str(text);
                    needs_redraw = true;
                }
                CoreEvent::ReasoningDelta { text } => {
                    self.accumulated_text.push_str(text);
                    needs_redraw = true;
                }
                CoreEvent::ToolCall { name, args } => {
                    self.accumulated_text
                        .push_str(&format!("\n\n## 🔧 {}\n```\n{}\n```\n", name, args));
                    needs_redraw = true;
                }
                CoreEvent::ToolResult { name, output } => {
                    let out = if output.len() > 500 {
                        format!("{}...", &output[..500])
                    } else {
                        output.to_string()
                    };
                    self.accumulated_text
                        .push_str(&format!("\n``\u{200b}`\n{}: {}\n``\u{200b}`\n", name, out));
                    needs_redraw = true;
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
                    // Update token count in status bar
                    let status = self.ui.widget(
                        cx,
                        &[
                            live_id!(main_window),
                            live_id!(body),
                            live_id!(status_tokens),
                        ],
                    );
                    if !status.is_empty() {
                        if let Ok(guard) = BRIDGE.lock() {
                            if let Some(ref bridge) = *guard {
                                status.set_text(cx, &format!("{} tokens", bridge.tokens_used));
                            }
                        }
                    }
                }
                CoreEvent::Error { message } => {
                    if message.as_str() != "quit" {
                        self.accumulated_text
                            .push_str(&format!("\n\n❌ {}\n", message));
                        needs_redraw = true;
                    }
                }
                _ => {}
            }
        }

        if needs_redraw {
            let chat = self.ui.widget(
                cx,
                &[live_id!(main_window), live_id!(body), live_id!(chat_text)],
            );
            if !chat.is_empty() {
                chat.set_text(cx, &self.accumulated_text);
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let send_path = &[live_id!(main_window), live_id!(body), live_id!(send_btn)];
        let send_btn = self.ui.widget(cx, send_path);
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(send_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.send_input_to_engine(cx);
        }

        let input_path = &[live_id!(main_window), live_id!(body), live_id!(input_field)];
        let input = self.ui.widget(cx, input_path);
        if matches!(
            actions.find_widget_action_cast::<TextInputAction>(input.widget_uid()),
            TextInputAction::Returned(..)
        ) {
            self.send_input_to_engine(cx);
        }
    }
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
