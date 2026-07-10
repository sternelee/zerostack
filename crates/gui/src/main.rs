use std::sync::{LazyLock, Mutex};
use std::thread;

use compact_str::CompactString;
use makepad_widgets::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use zerostack_core::engine::CoreEngine;
use zerostack_core::events::{ChatMessage, CoreEvent, SessionInfo, UserAction};
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
                        action = action_rx.recv() => {
                            let Some(action) = action else { break };
                            let is_quit = matches!(action, UserAction::Quit);
                            let events = engine.handle_action(action).await;
                            for event in events {
                                let _ = gui_tx.send(event);
                            }
                            if is_quit {
                                return;
                            }
                        }
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

    pub fn send_action(&self, action: UserAction) {
        let _ = self.action_tx.send(action);
    }
}

// ─── Global State ──────────────────────────────────────────────────────────

/// Global mutable state shared between the script UI and Rust handlers.
/// Using a Mutex<...> static is the simplest bridge for Makepad's script_mod.
static GUI_STATE: LazyLock<Mutex<GuiState>> = LazyLock::new(|| {
    Mutex::new(GuiState {
        sessions: Vec::new(),
        current_session_id: String::new(),
        model: String::from("claude-sonnet"),
        provider: String::from("anthropic"),
        mode: String::from("yolo"),
        is_running: false,
        tokens_used: 0,
    })
});

struct GuiState {
    sessions: Vec<SessionInfo>,
    current_session_id: String,
    model: String,
    provider: String,
    mode: String,
    is_running: bool,
    tokens_used: u64,
}

// ─── App UI ────────────────────────────────────────────────────────────────

script_mod! {
    use mod.prelude.widgets.*

    let app = startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(1200, 800)
                window.title: "zerostack"

                body +: {
                    width: Fill, height: Fill
                    flow: Right
                    spacing: 0.0
                    show_bg: true
                    draw_bg.color: #1A1B26

                    // ── Sidebar ──────────────────────────────────────
                    sidebar := View {
                        width: 240.0, height: Fill
                        show_bg: true
                        draw_bg.color: #1F2030
                        flow: Down
                        padding: Inset { top: 8.0, bottom: 8.0 }
                        spacing: 4.0

                        // Sidebar header
                        View {
                            width: Fill, height: Fit
                            padding: Inset { left: 12.0, right: 12.0, top: 4.0, bottom: 8.0 }
                            flow: Right
                            align: Align { y: 0.5 }
                            Label {
                                text: "Sessions"
                                draw_text.color: #6C7086
                                draw_text.text_style.font_size: 11.0
                            }
                            View { width: Fill, height: 0.0 }
                            new_session_btn := Button {
                                text: "+ New"
                                width: Fit, height: 24.0
                                draw_bg.color: #7C6FF0
                                draw_bg.radius: 4.0
                                draw_text.color: #FFFFFF
                                draw_text.text_style.font_size: 11.0
                            }
                        }

                        // Session list
                        session_list := PortalList {
                            width: Fill, height: Fill
                            flow: Down
                            spacing: 2.0
                            drag_scrolling: true

                            SessionItem := View {
                                width: Fill, height: Fit
                                padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                flow: Down
                                spacing: 2.0
                                show_bg: true
                                draw_bg.color: #1F2030
                                session_name := Label {
                                    text: "Session"
                                    draw_text.color: #CDD6F4
                                    draw_text.text_style.font_size: 12.0
                                }
                                session_meta := Label {
                                    text: ""
                                    draw_text.color: #6C7086
                                    draw_text.text_style.font_size: 10.0
                                }
                            }
                        }
                    }

                    // ── Main Content ─────────────────────────────────
                    View {
                        width: Fill, height: Fill
                        flow: Down
                        spacing: 0.0

                        // ── Toolbar ───────────────────────────────────
                        View {
                            width: Fill, height: 44.0
                            show_bg: true
                            draw_bg.color: #1A1B26
                            padding: Inset { left: 16.0, right: 16.0 }
                            flow: Right
                            spacing: 8.0
                            align: Align { y: 0.5 }

                            Label {
                                text: "zerostack"
                                draw_text.color: #CDD6F4
                                draw_text.text_style.font_size: 14.0
                            }

                            View { width: Fill, height: 0.0 }

                            // Mode button
                            mode_btn := Button {
                                text: "YOLO"
                                width: Fit, height: 28.0
                                draw_bg.color: #2D2E3F
                                draw_bg.radius: 4.0
                                draw_text.color: #CDD6F4
                                draw_text.text_style.font_size: 11.0
                            }

                            // Clear button
                            clear_btn := Button {
                                text: "Clear"
                                width: Fit, height: 28.0
                                draw_bg.color: #2D2E3F
                                draw_bg.radius: 4.0
                                draw_text.color: #CDD6F4
                                draw_text.text_style.font_size: 11.0
                            }

                            // Cancel button
                            cancel_btn := Button {
                                text: "Stop"
                                width: Fit, height: 28.0
                                draw_bg.color: #F38BA8
                                draw_bg.radius: 4.0
                                draw_text.color: #FFFFFF
                                draw_text.text_style.font_size: 11.0
                                visible: false
                            }
                        }

                        // ── Chat Area ─────────────────────────────────
                        chat_area := View {
                            width: Fill, height: Fill
                            flow: Down
                            spacing: 0.0
                            padding: Inset { left: 16.0, right: 16.0, top: 12.0, bottom: 12.0 }

                            chat_text := Markdown {
                                width: Fill, height: Fit
                                selectable: true
                                body: "# Welcome to zerostack GUI\n\nType a message below to get started.\n\n**Commands:** `/help`, `/mode yolo`, `/model`, `/provider`, `/add`, `/clear`, `/undo`\n\nPress **Enter** to send, **Shift+Enter** for newline."
                            }
                        }

                        // ── Input Bar ─────────────────────────────────
                        View {
                            width: Fill, height: Fit
                            show_bg: true
                            draw_bg.color: #1F2030
                            flow: Down
                            spacing: 0.0

                            View {
                                width: Fill, height: 64.0
                                padding: Inset { left: 16.0, right: 16.0, top: 12.0, bottom: 12.0 }
                                flow: Right
                                spacing: 8.0
                                align: Align { y: 0.5 }

                                input_field := TextInput {
                                    width: Fill, height: 40.0
                                    empty_text: "Type a message... (Enter to send, / for commands)"
                                    submit_on_enter: true
                                    draw_bg.color: #2D2E3F
                                    draw_bg.radius: 8.0
                                    draw_text.color: #CDD6F4
                                    draw_text.text_style.font_size: 14.0
                                }

                                send_btn := Button {
                                    text: "Send"
                                    width: 72.0, height: 40.0
                                    draw_bg.color: #7C6FF0
                                    draw_bg.radius: 8.0
                                    draw_text.color: #FFFFFF
                                    draw_text.text_style.font_size: 14.0
                                }
                            }

                            // ── Status Bar ──────────────────────────────
                            View {
                                width: Fill, height: 28.0
                                padding: Inset { left: 16.0, right: 16.0 }
                                flow: Right
                                spacing: 16.0
                                align: Align { y: 0.5 }
                                show_bg: true
                                draw_bg.color: #1A1B26

                                status_provider := Label {
                                    text: "anthropic"
                                    draw_text.color: #6C7086
                                    draw_text.text_style.font_size: 11.0
                                }
                                status_model := Label {
                                    text: "claude-sonnet"
                                    draw_text.color: #6C7086
                                    draw_text.text_style.font_size: 11.0
                                }
                                status_mode := Label {
                                    text: "yolo"
                                    draw_text.color: #6C7086
                                    draw_text.text_style.font_size: 11.0
                                }
                                View { width: Fill, height: 0.0 }
                                status_running := Label {
                                    text: ""
                                    draw_text.color: #F9E2AF
                                    draw_text.text_style.font_size: 11.0
                                }
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
    #[rust]
    chat_messages: Vec<ChatMessage>,
    #[rust]
    sessions: Vec<SessionInfo>,
    #[rust]
    current_session_id: String,
    #[rust]
    is_running: bool,
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

        // Check for slash commands
        if text.starts_with('/') {
            self.accumulated_text
                .push_str(&format!("```\n{}\n```\n\n", text));
            self.redraw_chat(cx);
            Self::ensure_bridge();
            if let Ok(guard) = BRIDGE.lock() {
                if let Some(ref bridge) = *guard {
                    bridge.send_action(UserAction::RunSlashCommand {
                        command: text.as_str().into(),
                    });
                }
            }
            return;
        }

        // Regular message
        self.accumulated_text
            .push_str(&format!("## You\n\n{}\n\n## Assistant\n\n", text));
        self.redraw_chat(cx);
        self.is_running = true;
        self.update_running_indicator(cx);
        Self::ensure_bridge();
        if let Ok(guard) = BRIDGE.lock() {
            if let Some(ref bridge) = *guard {
                bridge.send_action(UserAction::SendMessage {
                    text: text.as_str().into(),
                });
            }
        }
    }

    fn redraw_chat(&self, cx: &mut Cx) {
        let chat = self.ui.widget(
            cx,
            &[live_id!(main_window), live_id!(body), live_id!(chat_text)],
        );
        if !chat.is_empty() {
            chat.set_text(cx, &self.accumulated_text);
        }
    }

    fn update_running_indicator(&mut self, cx: &mut Cx) {
        // Show/hide cancel button
        let cancel_btn = self.ui.widget(
            cx,
            &[live_id!(main_window), live_id!(body), live_id!(cancel_btn)],
        );
        if !cancel_btn.is_empty() {
            cancel_btn.set_visible(cx, self.is_running);
        }

        // Update running label
        let running = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(status_running),
            ],
        );
        if !running.is_empty() {
            let text = if self.is_running {
                "● thinking..."
            } else {
                ""
            };
            running.set_text(cx, text);
        }
    }

    fn update_status_bar(&self, cx: &mut Cx) {
        if let Ok(guard) = GUI_STATE.lock() {
            let provider = self.ui.widget(
                cx,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(status_provider),
                ],
            );
            if !provider.is_empty() {
                provider.set_text(cx, &guard.provider);
            }

            let model = self.ui.widget(
                cx,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(status_model),
                ],
            );
            if !model.is_empty() {
                model.set_text(cx, &guard.model);
            }

            let mode = self.ui.widget(
                cx,
                &[live_id!(main_window), live_id!(body), live_id!(status_mode)],
            );
            if !mode.is_empty() {
                mode.set_text(cx, &guard.mode);
            }

            let tokens = self.ui.widget(
                cx,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(status_tokens),
                ],
            );
            if !tokens.is_empty() {
                tokens.set_text(cx, &format!("{} tokens", guard.tokens_used));
            }
        }
    }

    fn update_mode_button(&self, cx: &mut Cx) {
        let mode_btn = self.ui.widget(
            cx,
            &[live_id!(main_window), live_id!(body), live_id!(mode_btn)],
        );
        if !mode_btn.is_empty() {
            if let Ok(guard) = GUI_STATE.lock() {
                mode_btn.set_text(cx, &guard.mode.to_uppercase());
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
        let mut status_changed = false;

        for event in &events {
            match event {
                CoreEvent::StreamingDelta { text } => {
                    self.accumulated_text.push_str(text);
                    needs_redraw = true;
                }
                CoreEvent::ReasoningDelta { text } => {
                    self.accumulated_text.push_str(&format!("*{}*", text));
                    needs_redraw = true;
                }
                CoreEvent::ToolCall { name, args } => {
                    self.accumulated_text
                        .push_str(&format!("\n\n## 🔧 {}\n```json\n{}\n```\n", name, args));
                    needs_redraw = true;
                }
                CoreEvent::ToolResult { name, output } => {
                    let out = if output.len() > 500 {
                        format!("{}...", &output[..500])
                    } else {
                        output.to_string()
                    };
                    self.accumulated_text
                        .push_str(&format!("\n```\n{}: {}\n```\n", name, out));
                    needs_redraw = true;
                }
                CoreEvent::MessageComplete {
                    response,
                    input_tokens,
                    output_tokens,
                    ..
                } => {
                    // Append final response if not already streamed
                    if !response.is_empty() && !self.accumulated_text.contains(response.as_str()) {
                        self.accumulated_text.push_str(response);
                        needs_redraw = true;
                    }
                    // Update token count
                    if let Ok(mut guard) = BRIDGE.lock() {
                        if let Some(ref mut bridge) = *guard {
                            bridge.tokens_used += input_tokens + output_tokens;
                        }
                    }
                    if let Ok(mut guard) = GUI_STATE.lock() {
                        guard.tokens_used += input_tokens + output_tokens;
                    }
                    status_changed = true;
                }
                CoreEvent::AgentStarted => {
                    self.is_running = true;
                    self.update_running_indicator(cx);
                }
                CoreEvent::AgentStopped => {
                    self.is_running = false;
                    self.update_running_indicator(cx);
                    self.accumulated_text.push_str("\n\n---\n\n");
                    needs_redraw = true;
                }
                CoreEvent::Retrying { attempt, max } => {
                    self.accumulated_text
                        .push_str(&format!("\n\n⏳ Retrying ({}/{})...\n", attempt, max));
                    needs_redraw = true;
                }
                CoreEvent::Error { message } => {
                    if message.as_str() != "quit" {
                        self.accumulated_text
                            .push_str(&format!("\n\n❌ **Error:** {}\n", message));
                        needs_redraw = true;
                        self.is_running = false;
                        self.update_running_indicator(cx);
                    }
                }
                CoreEvent::SessionListUpdated { sessions } => {
                    self.sessions = sessions.clone();
                    if let Ok(mut guard) = GUI_STATE.lock() {
                        guard.sessions = sessions.clone();
                    }
                }
                CoreEvent::SessionChanged { session_id } => {
                    self.current_session_id = session_id.to_string();
                    if let Ok(mut guard) = GUI_STATE.lock() {
                        guard.current_session_id = session_id.to_string();
                    }
                }
                CoreEvent::SessionHistory { messages } => {
                    // Rebuild chat display from session history
                    self.chat_messages = messages.clone();
                    self.accumulated_text.clear();
                    for msg in messages {
                        match msg.role.as_str() {
                            "user" => {
                                self.accumulated_text
                                    .push_str(&format!("## You\n\n{}\n\n", msg.content));
                            }
                            "assistant" => {
                                self.accumulated_text
                                    .push_str(&format!("## Assistant\n\n{}\n\n", msg.content));
                            }
                            "tool_call" => {
                                self.accumulated_text
                                    .push_str(&format!("🔧 {}\n\n", msg.content));
                            }
                            "tool_result" => {
                                self.accumulated_text
                                    .push_str(&format!("```\n{}\n```\n\n", msg.content));
                            }
                            _ => {}
                        }
                    }
                    needs_redraw = true;
                }
                CoreEvent::StatusUpdate {
                    model,
                    provider,
                    tokens_used,
                    mode,
                } => {
                    if let Ok(mut guard) = GUI_STATE.lock() {
                        guard.model = model.to_string();
                        guard.provider = provider.to_string();
                        guard.mode = mode.to_string();
                        guard.tokens_used = *tokens_used;
                    }
                    status_changed = true;
                }
                CoreEvent::CommandOutput { text } => {
                    self.accumulated_text
                        .push_str(&format!("```\n{}\n```\n\n", text));
                    needs_redraw = true;
                }
                CoreEvent::ConfigChanged => {
                    status_changed = true;
                }
                _ => {}
            }
        }

        if needs_redraw {
            self.redraw_chat(cx);
        }
        if status_changed {
            self.update_status_bar(cx);
            self.update_mode_button(cx);
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Send button
        let send_btn = self.ui.widget(
            cx,
            &[live_id!(main_window), live_id!(body), live_id!(send_btn)],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(send_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.send_input_to_engine(cx);
        }

        // Enter key in input
        let input = self.ui.widget(
            cx,
            &[live_id!(main_window), live_id!(body), live_id!(input_field)],
        );
        if matches!(
            actions.find_widget_action_cast::<TextInputAction>(input.widget_uid()),
            TextInputAction::Returned(..)
        ) {
            self.send_input_to_engine(cx);
        }

        // New session button
        let new_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(new_session_btn),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(new_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            Self::ensure_bridge();
            if let Ok(guard) = BRIDGE.lock() {
                if let Some(ref bridge) = *guard {
                    bridge.send_action(UserAction::CreateSession { name: None });
                }
            }
        }

        // Clear button
        let clear_btn = self.ui.widget(
            cx,
            &[live_id!(main_window), live_id!(body), live_id!(clear_btn)],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(clear_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.accumulated_text.clear();
            self.redraw_chat(cx);
            Self::ensure_bridge();
            if let Ok(guard) = BRIDGE.lock() {
                if let Some(ref bridge) = *guard {
                    bridge.send_action(UserAction::ClearSession);
                }
            }
        }

        // Cancel button
        let cancel_btn = self.ui.widget(
            cx,
            &[live_id!(main_window), live_id!(body), live_id!(cancel_btn)],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(cancel_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.is_running = false;
            self.update_running_indicator(cx);
            if let Ok(guard) = BRIDGE.lock() {
                if let Some(ref bridge) = *guard {
                    bridge.send_action(UserAction::CancelStream);
                }
            }
        }

        // Mode button - cycles through modes
        let mode_btn = self.ui.widget(
            cx,
            &[live_id!(main_window), live_id!(body), live_id!(mode_btn)],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(mode_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            let next_mode = if let Ok(guard) = GUI_STATE.lock() {
                match guard.mode.as_str() {
                    "yolo" => "standard",
                    "standard" => "guarded",
                    "guarded" => "restrictive",
                    "restrictive" => "readonly",
                    _ => "yolo",
                }
                .to_string()
            } else {
                "yolo".to_string()
            };
            Self::ensure_bridge();
            if let Ok(guard) = BRIDGE.lock() {
                if let Some(ref bridge) = *guard {
                    bridge.send_action(UserAction::SetMode {
                        mode: next_mode.as_str().into(),
                    });
                }
            }
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
