use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::Instant;

use compact_str::CompactString;
use makepad_widgets::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use zerostack_core::engine::CoreEngine;
use zerostack_core::events::{ChatMessage, CoreEvent, SessionInfo, UserAction};
use zerostack_core::permission::SecurityMode;

use zerostack_gui::theme::dark;

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
                let (mut engine, mut engine_event_rx, mut ask_rx) = match CoreEngine::build_default(
                    m.as_str().into(),
                    p.as_str().into(),
                    SecurityMode::Yolo,
                )
                .await
                {
                    Ok(triple) => triple,
                    Err(e) => {
                        eprintln!("Failed to build CoreEngine: {e}");
                        let _ = gui_tx.send(CoreEvent::Error {
                            message: CompactString::new(format!("Engine init failed: {e}")),
                        });
                        return;
                    }
                };

                loop {
                    if let Some(ref mut rx) = ask_rx {
                        while let Ok(request) = rx.try_recv() {
                            engine.handle_ask_request(request);
                        }
                    }

                    tokio::select! {
                        action = action_rx.recv() => {
                            let Some(action) = action else { break };
                            let is_quit = matches!(action, UserAction::Quit);
                            let events = engine.handle_action(action).await;
                            for event in events {
                                let _ = gui_tx.send(event);
                            }
                            if is_quit { return; }
                        }
                        event = engine_event_rx.recv() => {
                            match event {
                                Some(event) => {
                                    if matches!(event, CoreEvent::MessageComplete { .. }) {
                                        engine.save_current_session();
                                    }
                                    let _ = gui_tx.send(event);
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

static GUI_STATE: LazyLock<Mutex<GuiState>> = LazyLock::new(|| {
    let (model, provider) = resolve_provider_model();
    Mutex::new(GuiState {
        sessions: Vec::new(),
        current_session_id: String::new(),
        current_session_name: String::from("New chat"),
        project_name: String::from("zerostack"),
        model,
        provider,
        mode: "yolo".to_string(),
        tokens_used: 0,
    })
});

fn resolve_provider_model() -> (String, String) {
    let (cfg, _) = zerostack_core::config::load();
    let cli = zerostack_core::cli::Cli::default();
    let provider = cli.resolve_provider(&cfg).to_string();
    let model = cli.resolve_model(&cfg).to_string();
    (model, provider)
}

struct GuiState {
    sessions: Vec<SessionInfo>,
    current_session_id: String,
    current_session_name: String,
    project_name: String,
    model: String,
    provider: String,
    mode: String,
    tokens_used: u64,
}

// ─── Chat Bubbles ──────────────────────────────────────────────────────────

const MAX_BUBBLES: usize = 20;

#[derive(Clone, Debug)]
struct ChatBubble {
    role: String,
    content: String,
}

impl ChatBubble {
    fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

// ─── Constants ─────────────────────────────────────────────────────────────

const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ─── App UI ────────────────────────────────────────────────────────────────

script_mod! {
    use mod.prelude.widgets.*

    let app = startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(1280, 840)
                window.title: "zerostack"

                body +: {
                    width: Fill, height: Fill
                    flow: Right
                    spacing: 0.0
                    show_bg: true
                    draw_bg.color: #x171719

                    // ══ Sidebar ══════════════════════════════════════
                    sidebar := RoundedView {
                        width: 260.0, height: Fill
                        show_bg: true
                        draw_bg.color: #x1F2024
                        draw_bg.border_radius: 0.0
                        flow: Down
                        spacing: 0.0

                        // Sidebar top: toggle button only
                        RoundedView {
                            width: Fill, height: Fit
                            padding: Inset { left: 10.0, right: 10.0, top: 10.0, bottom: 8.0 }
                            flow: Right
                            align: Align { y: 0.5 }
                            spacing: 8.0

                            View { width: Fill, height: 0.0 }

                            sidebar_split_btn := Button {
                                text: "▢"
                                width: 24.0, height: 24.0
                                draw_bg.color: #x00000000
                                draw_bg.radius: 6.0
                                draw_bg.color_hover: #x2E2F33
                                draw_text.color: #x71717A
                                draw_text.text_style.font_size: 12.0
                            }
                        }

                        // Navigation menu
                        nav_menu := View {
                            width: Fill, height: Fit
                            flow: Down
                            spacing: 2.0
                            padding: Inset { left: 10.0, right: 10.0, top: 4.0, bottom: 8.0 }

                            nav_new_agent := Button {
                                text: "  ＋  New Agent"
                                width: Fill, height: 32.0
                                padding: Inset { left: 10.0, right: 10.0 }
                                align: Align { x: 0.0, y: 0.5 }
                                draw_bg.color: #x00000000
                                draw_bg.radius: 8.0
                                draw_bg.color_hover: #x2A2B30
                                draw_text.color: #xF2F2F5
                                draw_text.text_style.font_size: 12.0
                            }
                            nav_search := Button {
                                text: "  ⌕  Search"
                                width: Fill, height: 32.0
                                padding: Inset { left: 10.0, right: 10.0 }
                                align: Align { x: 0.0, y: 0.5 }
                                draw_bg.color: #x00000000
                                draw_bg.radius: 8.0
                                draw_bg.color_hover: #x2A2B30
                                draw_text.color: #x98999D
                                draw_text.text_style.font_size: 12.0
                            }
                            nav_automations := Button {
                                text: "  ⚡  Automations"
                                width: Fill, height: 32.0
                                padding: Inset { left: 10.0, right: 10.0 }
                                align: Align { x: 0.0, y: 0.5 }
                                draw_bg.color: #x00000000
                                draw_bg.radius: 8.0
                                draw_bg.color_hover: #x2A2B30
                                draw_text.color: #x98999D
                                draw_text.text_style.font_size: 12.0
                            }
                            nav_customize := Button {
                                text: "  ⚙  Customize"
                                width: Fill, height: 32.0
                                padding: Inset { left: 10.0, right: 10.0 }
                                align: Align { x: 0.0, y: 0.5 }
                                draw_bg.color: #x00000000
                                draw_bg.radius: 8.0
                                draw_bg.color_hover: #x2A2B30
                                draw_text.color: #x98999D
                                draw_text.text_style.font_size: 12.0
                            }
                        }

                        // Project / Recent section header
                        project_header := View {
                            width: Fill, height: Fit
                            padding: Inset { left: 16.0, right: 10.0, top: 6.0, bottom: 4.0 }
                            flow: Right
                            align: Align { y: 0.5 }
                            spacing: 6.0

                            section_project := Label {
                                text: "natro"
                                width: Fill
                                draw_text.color: #x98999D
                                draw_text.text_style.font_size: 11.0
                            }

                            section_filter := Button {
                                text: "≣"
                                width: 22.0, height: 22.0
                                draw_bg.color: #x00000000
                                draw_bg.radius: 6.0
                                draw_bg.color_hover: #x2A2B30
                                draw_text.color: #x71717A
                                draw_text.text_style.font_size: 13.0
                            }
                            section_select_all := Button {
                                text: "☐"
                                width: 22.0, height: 22.0
                                draw_bg.color: #x00000000
                                draw_bg.radius: 6.0
                                draw_bg.color_hover: #x2A2B30
                                draw_text.color: #x71717A
                                draw_text.text_style.font_size: 12.0
                            }
                        }

                        project_sessions := View {
                            width: Fill, height: Fit
                            flow: Down
                            spacing: 2.0
                            padding: Inset { left: 12.0, right: 12.0 }

                            project_session_0 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            project_session_1 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            project_session_2 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            project_session_3 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            project_session_4 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            project_session_5 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            project_session_6 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            project_session_7 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                        }

                        // Divider
                        RoundedView {
                            width: Fill, height: 1.0
                            margin: Inset { left: 16.0, right: 16.0, top: 8.0, bottom: 8.0 }
                            show_bg: true
                            draw_bg.color: #x27272A
                        }

                        // This Mac section header
                        local_header := View {
                            width: Fill, height: Fit
                            padding: Inset { left: 16.0, right: 10.0, top: 8.0, bottom: 4.0 }
                            flow: Right
                            align: Align { y: 0.5 }
                            spacing: 6.0

                            section_local := Label {
                                text: "This Mac"
                                width: Fill
                                draw_text.color: #x98999D
                                draw_text.text_style.font_size: 11.0
                            }
                        }

                        local_sessions := View {
                            width: Fill, height: Fill
                            flow: Down
                            spacing: 2.0
                            padding: Inset { left: 12.0, right: 12.0 }
                            scroll_bars: ScrollBars {
                                scroll_bar_y.drag_scrolling: true
                            }

                            local_session_0 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            local_session_1 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            local_session_2 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            local_session_3 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            local_session_4 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            local_session_5 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            local_session_6 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                            local_session_7 := Button { width: Fill, height: Fit, visible: false, padding: Inset { left: 10.0, right: 10.0, top: 6.0, bottom: 6.0 }, align: Align { x: 0.0, y: 0.5 }, draw_bg.color: #x00000000, draw_bg.radius: 8.0, draw_bg.color_hover: #x18181B, draw_text.color: #xA1A1AA, draw_text.text_style.font_size: 12.0, text: "" }
                        }

                        // User profile footer
                        RoundedView { width: Fill, height: Fill }

                        // Divider above footer
                        RoundedView {
                            width: Fill, height: 1.0
                            margin: Inset { left: 16.0, right: 16.0, bottom: 8.0 }
                            show_bg: true
                            draw_bg.color: #x27272A
                        }

                        profile_footer := RoundedView {
                            width: Fill, height: Fit
                            show_bg: true
                            draw_bg.color: #x00000000
                            padding: Inset { left: 12.0, right: 12.0, top: 10.0, bottom: 12.0 }
                            flow: Right
                            spacing: 10.0
                            align: Align { y: 0.5 }

                            profile_avatar := Button {
                                text: "U"
                                width: 32.0, height: 32.0
                                draw_bg.color: #x27272A
                                draw_bg.radius: 9999.0
                                draw_bg.color_hover: #x3F3F46
                                draw_text.color: #xFAFAFA
                                draw_text.text_style.font_size: 12.0
                            }

                            View {
                                width: Fill, height: Fit
                                flow: Down
                                spacing: 2.0
                                align: Align { y: 0.5 }

                                profile_name := Label {
                                    text: "User"
                                    width: Fill
                                    draw_text.color: #xFAFAFA
                                    draw_text.text_style.font_size: 12.0
                                }
                                profile_plan := Label {
                                    text: "Free Plan"
                                    width: Fill
                                    draw_text.color: #xA1A1AA
                                    draw_text.text_style.font_size: 10.0
                                }
                            }

                            profile_settings := Button {
                                text: "⚙"
                                width: 28.0, height: 28.0
                                draw_bg.color: #x00000000
                                draw_bg.radius: 8.0
                                draw_bg.color_hover: #x18181B
                                draw_text.color: #x71717A
                                draw_text.text_style.font_size: 14.0
                            }
                        }
                    }

                    // Vertical divider between sidebar and chat
                    RoundedView {
                        width: 1.0, height: Fill
                        show_bg: true
                        draw_bg.color: #x27272A
                    }

                    // ══ Main Content ═════════════════════════════════
                    main_content := RoundedView {
                        width: Fill, height: Fill
                        flow: Down
                        spacing: 0.0
                        show_bg: true
                        draw_bg.color: #x171719

                        // ── Top Bar ─────────────────────────────────
                        top_bar := RoundedView {
                            width: Fill, height: 40.0
                            show_bg: true
                            draw_bg.color: #x171719
                            draw_bg.border_radius: 0.0
                                padding: Inset { left: 16.0, right: 12.0 }
                                flow: Right
                            spacing: 10.0
                            align: Align { y: 0.5 }

                            session_title := Label {
                                text: "new chat"
                                draw_text.color: #xF2F2F5
                                draw_text.text_style.font_size: 13.0
                            }

                            View { width: Fill, height: 0.0 }

                            top_model := Button {
                                text: "Composer 2.5 Fast"
                                width: Fit, height: 26.0
                                padding: Inset { left: 10.0, right: 10.0 }
                                draw_bg.color: #x2A2B30
                                draw_bg.radius: 8.0
                                draw_bg.color_hover: #x34353A
                                draw_text.color: #xD4D4D8
                                draw_text.text_style.font_size: 11.0
                            }

                            top_menu := Button {
                                text: "⋯"
                                width: 28.0, height: 26.0
                                draw_bg.color: #x00000000
                                draw_bg.radius: 8.0
                                draw_bg.color_hover: #x2A2B30
                                draw_text.color: #x98999D
                                draw_text.text_style.font_size: 14.0
                            }

                            top_maximize := Button {
                                text: "☐"
                                width: 28.0, height: 26.0
                                draw_bg.color: #x00000000
                                draw_bg.radius: 8.0
                                draw_bg.color_hover: #x2A2B30
                                draw_text.color: #x98999D
                                draw_text.text_style.font_size: 12.0
                            }
                        }

                        // Top bar divider
                        RoundedView {
                            width: Fill, height: 1.0
                            show_bg: true
                            draw_bg.color: #x33343A
                        }

                        // ── Chat Area ────────────────────────────────
                        chat_scroll := RoundedView {
                            width: Fill, height: Fill
                            flow: Down
                            padding: Inset { left: 16.0, right: 16.0, top: 8.0, bottom: 16.0 }
                            spacing: 0.0
                            scroll_bars: ScrollBars {
                                scroll_bar_y.drag_scrolling: true
                            }

                            // Welcome / empty state
                            welcome_view := View {
                                width: Fill, height: Fit
                                visible: true
                                padding: Inset { top: 140.0, bottom: 60.0 }
                                align: Align { x: 0.5, y: 0.0 }
                                flow: Down
                                spacing: 20.0

                                View {
                                    width: Fit, height: Fit
                                    align: Align { x: 0.5 }
                                    flow: Down
                                    spacing: 10.0

                                    welcome_title := Label {
                                        text: "zerostack"
                                        width: Fit
                                        draw_text.color: #xFAFAFA
                                        draw_text.text_style.font_size: 28.0
                                    }
                                    welcome_subtitle := Label {
                                        text: "What can I help you build today?"
                                        width: Fit
                                        draw_text.color: #x71717A
                                        draw_text.text_style.font_size: 14.0
                                    }
                                }

                                RoundedView {
                                    width: Fit, height: Fit
                                    align: Align { x: 0.5 }
                                    flow: Right
                                    spacing: 10.0
                                    padding: Inset { top: 16.0 }

                                    hint_0 := RoundedView {
                                        width: Fit, height: Fit
                                        show_bg: true
                                        draw_bg.color: #x141417
                                        draw_bg.border_radius: 12.0
                                        draw_bg.border_size: 1.0
                                        draw_bg.border_color: #x27272A
                                        padding: Inset { left: 14.0, right: 14.0, top: 10.0, bottom: 10.0 }
                                        hint_0_text := Label {
                                            text: "Explain this code"
                                            draw_text.color: #xA1A1AA
                                            draw_text.text_style.font_size: 12.0
                                        }
                                    }
                                    hint_1 := RoundedView {
                                        width: Fit, height: Fit
                                        show_bg: true
                                        draw_bg.color: #x141417
                                        draw_bg.border_radius: 12.0
                                        draw_bg.border_size: 1.0
                                        draw_bg.border_color: #x27272A
                                        padding: Inset { left: 14.0, right: 14.0, top: 10.0, bottom: 10.0 }
                                        hint_1_text := Label {
                                            text: "Refactor a function"
                                            draw_text.color: #xA1A1AA
                                            draw_text.text_style.font_size: 12.0
                                        }
                                    }
                                    hint_2 := RoundedView {
                                        width: Fit, height: Fit
                                        show_bg: true
                                        draw_bg.color: #x141417
                                        draw_bg.border_radius: 12.0
                                        draw_bg.border_size: 1.0
                                        draw_bg.border_color: #x27272A
                                        padding: Inset { left: 14.0, right: 14.0, top: 10.0, bottom: 10.0 }
                                        hint_2_text := Label {
                                            text: "Write a test"
                                            draw_text.color: #xA1A1AA
                                            draw_text.text_style.font_size: 12.0
                                        }
                                    }
                                }
                            }

                            // Worked-for indicator
                            worked_label := View {
                                width: Fill, height: Fit
                                visible: false
                                padding: Inset { left: 20.0, right: 16.0, top: 4.0, bottom: 8.0 }
                                align: Align { x: 0.0, y: 0.0 }

                                worked_text := Label {
                                    text: "Worked for 1s"
                                    draw_text.color: #x71717A
                                    draw_text.text_style.font_size: 11.0
                                }
                            }

                            // Message bubbles (generic slots - alignment set at runtime)
                            msg_0 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_0_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_0_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_0_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_1 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_1_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_1_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_1_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_2 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_2_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_2_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_2_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_3 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_3_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_3_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_3_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_4 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_4_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_4_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_4_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_5 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_5_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_5_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_5_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_6 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_6_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_6_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_6_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_7 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_7_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_7_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_7_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_8 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_8_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_8_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_8_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_9 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_9_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_9_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_9_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_10 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_10_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_10_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_10_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_11 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_11_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_11_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_11_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_12 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_12_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_12_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_12_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_13 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_13_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_13_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_13_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_14 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_14_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_14_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_14_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_15 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_15_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_15_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_15_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_16 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_16_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_16_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_16_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_17 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_17_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_17_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_17_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_18 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_18_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_18_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_18_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }
                            msg_19 := View {
                                width: Fill, height: Fit
                                visible: false
                                flow: Right
                                padding: Inset { left: 16.0, right: 16.0, top: 6.0, bottom: 6.0 }

                                msg_19_spacer := View {
                                    width: Fill, height: 0.0
                                    visible: false
                                }

                                msg_19_inner := RoundedView {
                                    width: Fill, height: Fit
                                    show_bg: true
                                    draw_bg.color: #x2E3039
                                    draw_bg.border_radius: 12.0
                                    padding: Inset { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }
                                    msg_19_text := Markdown {
                                        width: Fit, height: Fit
                                        draw_text.text_style.font_size: 14.0
                                        body: ""
                                    }
                                }
                            }

                            // ── Message actions row (Just now + reactions) ───
                            msg_actions := View {
                                width: Fill, height: Fit
                                visible: false
                                padding: Inset { left: 16.0, right: 16.0, top: 4.0, bottom: 8.0 }
                                flow: Right
                                align: Align { x: 1.0, y: 0.5 }
                                spacing: 14.0

                                msg_actions_time := Label {
                                    text: "Just now"
                                    draw_text.color: #x71717A
                                    draw_text.text_style.font_size: 11.0
                                }

                                msg_actions_thumb_up := Button {
                                    text: "[👍]"
                                    width: Fit, height: 22.0
                                    draw_bg.color: #x00000000
                                    draw_bg.radius: 6.0
                                    draw_bg.color_hover: #x2A2B30
                                    draw_text.color: #x98999D
                                    draw_text.text_style.font_size: 11.0
                                }
                                msg_actions_thumb_down := Button {
                                    text: "[👎]"
                                    width: Fit, height: 22.0
                                    draw_bg.color: #x00000000
                                    draw_bg.radius: 6.0
                                    draw_bg.color_hover: #x2A2B30
                                    draw_text.color: #x98999D
                                    draw_text.text_style.font_size: 11.0
                                }
                                msg_actions_branch := Button {
                                    text: "[⎇]"
                                    width: Fit, height: 22.0
                                    draw_bg.color: #x00000000
                                    draw_bg.radius: 6.0
                                    draw_bg.color_hover: #x2A2B30
                                    draw_text.color: #x98999D
                                    draw_text.text_style.font_size: 11.0
                                }
                                msg_actions_copy := Button {
                                    text: "[📋]"
                                    width: Fit, height: 22.0
                                    draw_bg.color: #x00000000
                                    draw_bg.radius: 6.0
                                    draw_bg.color_hover: #x2A2B30
                                    draw_text.color: #x98999D
                                    draw_text.text_style.font_size: 11.0
                                }
                            }
                        }

                        // ── Bottom Input Bar ─────────────────────────
                        bottom_bar := RoundedView {
                            width: Fill, height: Fit
                            show_bg: true
                            draw_bg.color: #x171719
                            padding: Inset { left: 16.0, right: 16.0, top: 8.0, bottom: 12.0 }
                            align: Align { y: 0.5 }

                            input_container := RoundedView {
                                width: Fill, height: Fit
                                show_bg: true
                                draw_bg.color: #x26272C
                                draw_bg.border_radius: 18.0
                                draw_bg.border_size: 1.0
                                draw_bg.border_color: #x33343A
                                draw_bg.color_focus: #x3F3F46
                                padding: Inset { left: 8.0, right: 8.0, top: 6.0, bottom: 6.0 }
                                flow: Right
                                spacing: 8.0
                                align: Align { y: 0.5 }

                                attach_btn := Button {
                                    text: "＋"
                                    width: 32.0, height: 32.0
                                    draw_bg.color: #x00000000
                                    draw_bg.radius: 9999.0
                                    draw_bg.color_hover: #x2E2F33
                                    draw_text.color: #x98999D
                                    draw_text.text_style.font_size: 16.0
                                }

                                input_field := TextInput {
                                    width: Fill, height: Fit
                                    empty_text: "Send follow-up"
                                    submit_on_enter: true
                                    padding: Inset { left: 6.0, right: 6.0, top: 9.0, bottom: 9.0 }
                                    draw_bg.color: #x00000000
                                    draw_text.color: #xF2F2F5
                                    draw_text.text_style.font_size: 13.0
                                    draw_text.empty_color: #x71717A
                                    draw_bg.color_focus: #x3F3F46
                                }

                                model_btn := Button {
                                    text: "Composer 2.5 Fast"
                                    width: Fit, height: 32.0
                                    padding: Inset { left: 12.0, right: 10.0 }
                                    align: Align { x: 0.0, y: 0.5 }
                                    draw_bg.color: #x00000000
                                    draw_bg.radius: 9999.0
                                    draw_bg.color_hover: #x2E2F33
                                    draw_text.color: #xC7C8CC
                                    draw_text.text_style.font_size: 11.0
                                }

                                mic_btn := Button {
                                    text: "🎤"
                                    width: 32.0, height: 32.0
                                    draw_bg.color: #xF2F2F5
                                    draw_bg.radius: 9999.0
                                    draw_bg.color_hover: #xFAFAFA
                                    draw_text.color: #x09090B
                                    draw_text.text_style.font_size: 10.0
                                }
                            }
                        }

                        // ── Status Bar (very bottom, 24px) ───────────
                        status_bar := RoundedView {
                            width: Fill, height: 24.0
                            show_bg: true
                            draw_bg.color: #x171719
                            padding: Inset { left: 16.0, right: 12.0, top: 4.0, bottom: 4.0 }
                            flow: Right
                            align: Align { y: 0.5 }
                            spacing: 8.0

                            status_project := Label {
                                text: "natro"
                                draw_text.color: #x71717A
                                draw_text.text_style.font_size: 10.0
                            }

                            View { width: Fill, height: 0.0 }

                            status_spinner := Label {
                                text: "◌"
                                draw_text.color: #x71717A
                                draw_text.text_style.font_size: 12.0
                            }
                        }
                    }

                    // ══ Right Tool Rail ══════════════════════════════
                    right_rail := RoundedView {
                        width: 40.0, height: Fill
                        show_bg: true
                        draw_bg.color: #x171719
                        flow: Down
                        padding: Inset { top: 12.0, bottom: 12.0 }
                        spacing: 6.0
                        align: Align { x: 0.5 }

                        tool_fullscreen := Button {
                            text: "⛶"
                            width: 32.0, height: 32.0
                            draw_bg.color: #x00000000
                            draw_bg.radius: 9999.0
                            draw_bg.color_hover: #x2A2B30
                            draw_text.color: #x98999D
                            draw_text.text_style.font_size: 14.0
                        }
                        tool_browser := Button {
                            text: "🌐"
                            width: 32.0, height: 32.0
                            draw_bg.color: #x00000000
                            draw_bg.radius: 9999.0
                            draw_bg.color_hover: #x2A2B30
                            draw_text.color: #x98999D
                            draw_text.text_style.font_size: 14.0
                        }
                        tool_terminal := Button {
                            text: ">_"
                            width: 32.0, height: 32.0
                            draw_bg.color: #x00000000
                            draw_bg.radius: 9999.0
                            draw_bg.color_hover: #x2A2B30
                            draw_text.color: #x98999D
                            draw_text.text_style.font_size: 11.0
                        }
                        tool_canvas := Button {
                            text: "📄"
                            width: 32.0, height: 32.0
                            draw_bg.color: #x00000000
                            draw_bg.radius: 9999.0
                            draw_bg.color_hover: #x2A2B30
                            draw_text.color: #x98999D
                            draw_text.text_style.font_size: 13.0
                        }

                        View { width: Fill, height: Fill }

                        tool_stop := Button {
                            text: "■"
                            width: 32.0, height: 32.0
                            visible: false
                            draw_bg.color: #xEF4444
                            draw_bg.radius: 9999.0
                            draw_bg.color_hover: #xFCA5A5
                            draw_text.color: #x171719
                            draw_text.text_style.font_size: 12.0
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
    chat_messages: Vec<ChatMessage>,
    #[rust]
    bubbles: Vec<ChatBubble>,
    #[rust]
    sessions: Vec<SessionInfo>,
    #[rust]
    current_session_id: String,
    #[rust]
    is_running: bool,
    #[rust]
    spinner_frame: usize,
    #[rust]
    pending_permission: Option<(u64, String, String)>,
    #[rust]
    agent_start_time: Option<Instant>,
    #[rust]
    worked_seconds: u64,
}

fn short_model_name(model: &str) -> &str {
    model
        .trim()
        .split_once('-')
        .map(|(prefix, _)| prefix)
        .unwrap_or(model)
}

fn format_session_time(created_at: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let fallback = || {
        if created_at.len() > 5 {
            created_at[..5].to_string()
        } else {
            created_at.to_string()
        }
    };

    let secs = match created_at.parse::<f64>() {
        Ok(s) => s,
        Err(_) => return fallback(),
    };
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(_) => return fallback(),
    };
    let delta = now - secs;

    if delta < 60.0 {
        "now".to_string()
    } else if delta < 3600.0 {
        format!("{}m", (delta / 60.0) as u64)
    } else if delta < 86400.0 {
        format!("{}h", (delta / 3600.0) as u64)
    } else if delta < 30.0 * 86400.0 {
        format!("{}d", (delta / 86400.0) as u64)
    } else if delta < 365.0 * 86400.0 {
        format!("{}mo", (delta / (30.0 * 86400.0)) as u64)
    } else {
        format!("{}y", (delta / (365.0 * 86400.0)) as u64)
    }
}

impl App {
    fn ensure_bridge() {
        let mut guard = BRIDGE.lock().unwrap();
        if guard.is_none() {
            let (model, provider) = resolve_provider_model();
            *guard = Some(GuiBridge::new(&model, &provider));
            if let Ok(mut state) = GUI_STATE.lock() {
                state.model = model;
                state.provider = provider;
            }
        }
    }

    // ── Input handling ──────────────────────────────────────────────────

    fn send_input_to_engine(&mut self, cx: &mut Cx) {
        let input = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(main_content),
                live_id!(bottom_bar),
                live_id!(input_container),
                live_id!(input_field),
            ],
        );
        if input.is_empty() {
            return;
        }
        let text = input.text();
        if text.is_empty() {
            return;
        }
        input.set_text(cx, "");
        input.set_key_focus(cx);

        // Hide welcome state once user starts chatting
        self.hide_welcome(cx);

        if text.starts_with('/') {
            self.handle_slash_input(cx, &text);
            return;
        }

        self.add_bubble(cx, ChatBubble::new("user", text.clone()));
        self.is_running = true;
        self.agent_start_time = Some(Instant::now());
        self.worked_seconds = 0;
        self.update_running_ui(cx);
        Self::ensure_bridge();
        if let Ok(guard) = BRIDGE.lock()
            && let Some(ref bridge) = *guard {
                bridge.send_action(UserAction::SendMessage {
                    text: text.as_str().into(),
                });
            }
    }

    fn handle_slash_input(&mut self, cx: &mut Cx, text: &str) {
        let trimmed = text.trim();

        if trimmed == "/allow" || trimmed.starts_with("/allow ") {
            if let Some((id, _, _)) = self.pending_permission.take() {
                Self::ensure_bridge();
                if let Ok(guard) = BRIDGE.lock()
                    && let Some(ref bridge) = *guard {
                        bridge.send_action(UserAction::PermissionResponse { id, allow: true });
                    }
                self.add_bubble(cx, ChatBubble::new("assistant", "✅ **Allowed**"));
            }
            return;
        }
        if trimmed == "/deny" || trimmed.starts_with("/deny ") {
            if let Some((id, _, _)) = self.pending_permission.take() {
                Self::ensure_bridge();
                if let Ok(guard) = BRIDGE.lock()
                    && let Some(ref bridge) = *guard {
                        bridge.send_action(UserAction::PermissionResponse { id, allow: false });
                    }
                self.add_bubble(cx, ChatBubble::new("assistant", "🚫 **Denied**"));
            }
            return;
        }

        self.add_bubble(cx, ChatBubble::new("user", format!("`{}`", trimmed)));
        self.is_running = true;
        self.agent_start_time = Some(Instant::now());
        self.update_running_ui(cx);
        Self::ensure_bridge();
        if let Ok(guard) = BRIDGE.lock()
            && let Some(ref bridge) = *guard {
                bridge.send_action(UserAction::RunSlashCommand {
                    command: trimmed.into(),
                });
            }
    }

    // ── Bubble rendering ────────────────────────────────────────────────

    fn hide_welcome(&self, cx: &mut Cx) {
        let welcome = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(main_content),
                live_id!(chat_scroll),
                live_id!(welcome_view),
            ],
        );
        if !welcome.is_empty() {
            welcome.set_visible(cx, false);
        }
    }

    fn add_bubble(&mut self, cx: &mut Cx, bubble: ChatBubble) {
        self.bubbles.push(bubble);
        if self.bubbles.len() > MAX_BUBBLES {
            self.bubbles.remove(0);
        }
        self.render_bubbles(cx);
    }

    fn render_bubbles(&self, cx: &mut Cx) {
        // Hide all slots first
        for i in 0..MAX_BUBBLES {
            let slot = self.bubble_slot_outer(cx, i);
            if !slot.is_empty() {
                slot.set_visible(cx, false);
            }
        }

        // Render from the end so latest messages are at the bottom
        let start = self.bubbles.len().saturating_sub(MAX_BUBBLES);
        for (slot_idx, bubble) in self.bubbles[start..].iter().enumerate() {
            let outer = self.bubble_slot_outer(cx, slot_idx);
            let inner = self.bubble_slot_inner(cx, slot_idx);
            let spacer = self.bubble_slot_spacer(cx, slot_idx);
            let text = self.bubble_slot_text(cx, slot_idx);
            if outer.is_empty() || inner.is_empty() || spacer.is_empty() || text.is_empty() {
                continue;
            }

            let (body, is_user, is_tool) = match bubble.role.as_str() {
                "user" => (bubble.content.clone(), true, false),
                "tool_call" => (format!("🔧 `{}`", bubble.content), false, true),
                "tool_result" => (format!("```\n{}\n```", bubble.content), false, true),
                _ => (bubble.content.clone(), false, false),
            };

            text.set_text(cx, &body);

            if is_user {
                spacer.set_visible(cx, false);
                let (r, g, b) = hex_to_rgb(dark::USER_BUBBLE_BG);
                self.set_view_color(cx, &inner, r, g, b, 0xFF);
            } else {
                spacer.set_visible(cx, false);
                if is_tool {
                    let (r, g, b) = hex_to_rgb(dark::TOOL_BUBBLE_BG);
                    self.set_view_color(cx, &inner, r, g, b, 0xFF);
                } else {
                    self.set_view_color(cx, &inner, 0x00, 0x00, 0x00, 0x00);
                }
            }

            outer.set_visible(cx, true);
        }
    }

    fn set_view_color(&self, cx: &mut Cx, view: &WidgetRef, r: u8, g: u8, b: u8, a: u8) {
        if let Some(mut v) = view.borrow_mut::<View>() {
            v.draw_bg.set_uniform(
                cx,
                live_id!(color),
                &[
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    a as f32 / 255.0,
                ],
            );
        }
    }

    fn bubble_slot_outer(&self, cx: &mut Cx, idx: usize) -> WidgetRef {
        const IDS: [LiveId; MAX_BUBBLES] = [
            live_id!(msg_0),
            live_id!(msg_1),
            live_id!(msg_2),
            live_id!(msg_3),
            live_id!(msg_4),
            live_id!(msg_5),
            live_id!(msg_6),
            live_id!(msg_7),
            live_id!(msg_8),
            live_id!(msg_9),
            live_id!(msg_10),
            live_id!(msg_11),
            live_id!(msg_12),
            live_id!(msg_13),
            live_id!(msg_14),
            live_id!(msg_15),
            live_id!(msg_16),
            live_id!(msg_17),
            live_id!(msg_18),
            live_id!(msg_19),
        ];
        self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(chat_scroll),
                IDS[idx],
            ],
        )
    }

    fn bubble_slot_inner(&self, cx: &mut Cx, idx: usize) -> WidgetRef {
        const IDS: [LiveId; MAX_BUBBLES] = [
            live_id!(msg_0_inner),
            live_id!(msg_1_inner),
            live_id!(msg_2_inner),
            live_id!(msg_3_inner),
            live_id!(msg_4_inner),
            live_id!(msg_5_inner),
            live_id!(msg_6_inner),
            live_id!(msg_7_inner),
            live_id!(msg_8_inner),
            live_id!(msg_9_inner),
            live_id!(msg_10_inner),
            live_id!(msg_11_inner),
            live_id!(msg_12_inner),
            live_id!(msg_13_inner),
            live_id!(msg_14_inner),
            live_id!(msg_15_inner),
            live_id!(msg_16_inner),
            live_id!(msg_17_inner),
            live_id!(msg_18_inner),
            live_id!(msg_19_inner),
        ];
        self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(chat_scroll),
                self.bubble_outer_id(idx),
                IDS[idx],
            ],
        )
    }

    fn bubble_slot_spacer(&self, cx: &mut Cx, idx: usize) -> WidgetRef {
        const IDS: [LiveId; MAX_BUBBLES] = [
            live_id!(msg_0_spacer),
            live_id!(msg_1_spacer),
            live_id!(msg_2_spacer),
            live_id!(msg_3_spacer),
            live_id!(msg_4_spacer),
            live_id!(msg_5_spacer),
            live_id!(msg_6_spacer),
            live_id!(msg_7_spacer),
            live_id!(msg_8_spacer),
            live_id!(msg_9_spacer),
            live_id!(msg_10_spacer),
            live_id!(msg_11_spacer),
            live_id!(msg_12_spacer),
            live_id!(msg_13_spacer),
            live_id!(msg_14_spacer),
            live_id!(msg_15_spacer),
            live_id!(msg_16_spacer),
            live_id!(msg_17_spacer),
            live_id!(msg_18_spacer),
            live_id!(msg_19_spacer),
        ];
        self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(chat_scroll),
                self.bubble_outer_id(idx),
                IDS[idx],
            ],
        )
    }

    fn bubble_slot_text(&self, cx: &mut Cx, idx: usize) -> WidgetRef {
        const IDS: [LiveId; MAX_BUBBLES] = [
            live_id!(msg_0_text),
            live_id!(msg_1_text),
            live_id!(msg_2_text),
            live_id!(msg_3_text),
            live_id!(msg_4_text),
            live_id!(msg_5_text),
            live_id!(msg_6_text),
            live_id!(msg_7_text),
            live_id!(msg_8_text),
            live_id!(msg_9_text),
            live_id!(msg_10_text),
            live_id!(msg_11_text),
            live_id!(msg_12_text),
            live_id!(msg_13_text),
            live_id!(msg_14_text),
            live_id!(msg_15_text),
            live_id!(msg_16_text),
            live_id!(msg_17_text),
            live_id!(msg_18_text),
            live_id!(msg_19_text),
        ];
        const INNER_IDS: [LiveId; MAX_BUBBLES] = [
            live_id!(msg_0_inner),
            live_id!(msg_1_inner),
            live_id!(msg_2_inner),
            live_id!(msg_3_inner),
            live_id!(msg_4_inner),
            live_id!(msg_5_inner),
            live_id!(msg_6_inner),
            live_id!(msg_7_inner),
            live_id!(msg_8_inner),
            live_id!(msg_9_inner),
            live_id!(msg_10_inner),
            live_id!(msg_11_inner),
            live_id!(msg_12_inner),
            live_id!(msg_13_inner),
            live_id!(msg_14_inner),
            live_id!(msg_15_inner),
            live_id!(msg_16_inner),
            live_id!(msg_17_inner),
            live_id!(msg_18_inner),
            live_id!(msg_19_inner),
        ];
        self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(chat_scroll),
                self.bubble_outer_id(idx),
                INNER_IDS[idx],
                IDS[idx],
            ],
        )
    }

    fn bubble_outer_id(&self, idx: usize) -> LiveId {
        [
            live_id!(msg_0),
            live_id!(msg_1),
            live_id!(msg_2),
            live_id!(msg_3),
            live_id!(msg_4),
            live_id!(msg_5),
            live_id!(msg_6),
            live_id!(msg_7),
            live_id!(msg_8),
            live_id!(msg_9),
            live_id!(msg_10),
            live_id!(msg_11),
            live_id!(msg_12),
            live_id!(msg_13),
            live_id!(msg_14),
            live_id!(msg_15),
            live_id!(msg_16),
            live_id!(msg_17),
            live_id!(msg_18),
            live_id!(msg_19),
        ][idx]
    }

    // ── Worked-for indicator ────────────────────────────────────────────

    fn set_worked_visible(&self, cx: &mut Cx, visible: bool, seconds: u64) {
        let worked = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(worked_label),
            ],
        );
        if worked.is_empty() {
            return;
        }
        if visible {
            let text = self.ui.widget(
                cx,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(worked_label),
                    live_id!(worked_text),
                ],
            );
            if !text.is_empty() {
                text.set_text(cx, &format!("Worked for {}s", seconds));
            }
            worked.set_visible(cx, true);
        } else {
            worked.set_visible(cx, false);
        }
    }

    // ── Message actions row (timestamp + reactions) ───────────────────

    fn show_msg_actions(&self, cx: &mut Cx, visible: bool) {
        let actions = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(main_content),
                live_id!(chat_scroll),
                live_id!(msg_actions),
            ],
        );
        if !actions.is_empty() {
            actions.set_visible(cx, visible);
        }
    }

    // ── Status bar spinner animation ─────────────────────────────────

    fn update_status_spinner(&self, cx: &mut Cx) {
        if !self.is_running {
            return;
        }
        let spinner = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(main_content),
                live_id!(status_bar),
                live_id!(status_spinner),
            ],
        );
        if !spinner.is_empty() {
            let frame = SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()];
            spinner.set_text(cx, &frame.to_string());
        }
    }

    // ── Visual updates ──────────────────────────────────────────────────

    fn update_top_bar(&self, cx: &mut Cx) {
        if let Ok(guard) = GUI_STATE.lock() {
            ui_set_text(
                cx,
                &self.ui,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(main_content),
                    live_id!(top_bar),
                    live_id!(session_title),
                ],
                &guard.current_session_name,
            );
            ui_set_text(
                cx,
                &self.ui,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(main_content),
                    live_id!(top_bar),
                    live_id!(top_model),
                ],
                short_model_name(&guard.model),
            );
            ui_set_text(
                cx,
                &self.ui,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(main_content),
                    live_id!(bottom_bar),
                    live_id!(input_container),
                    live_id!(model_btn),
                ],
                short_model_name(&guard.model),
            );
            ui_set_text(
                cx,
                &self.ui,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(sidebar),
                    live_id!(project_header),
                    live_id!(section_project),
                ],
                &guard.project_name,
            );
            ui_set_text(
                cx,
                &self.ui,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(main_content),
                    live_id!(status_bar),
                    live_id!(status_project),
                ],
                &guard.project_name,
            );
        }
    }

    fn update_mode_pill(&self, cx: &mut Cx) {
        self.update_top_bar(cx);
    }

    fn update_running_ui(&mut self, cx: &mut Cx) {
        let stop_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(right_rail),
                live_id!(tool_stop),
            ],
        );
        if !stop_btn.is_empty() {
            stop_btn.set_visible(cx, self.is_running);
        }
    }

    fn refresh_session_buttons(&self, cx: &mut Cx, sessions: &[SessionInfo]) {
        const PROJECT_IDS: [LiveId; 8] = [
            live_id!(project_session_0),
            live_id!(project_session_1),
            live_id!(project_session_2),
            live_id!(project_session_3),
            live_id!(project_session_4),
            live_id!(project_session_5),
            live_id!(project_session_6),
            live_id!(project_session_7),
        ];
        const LOCAL_IDS: [LiveId; 8] = [
            live_id!(local_session_0),
            live_id!(local_session_1),
            live_id!(local_session_2),
            live_id!(local_session_3),
            live_id!(local_session_4),
            live_id!(local_session_5),
            live_id!(local_session_6),
            live_id!(local_session_7),
        ];

        // First half to project, second half to local for demo grouping
        let split = sessions.len().min(8);
        for (idx, id) in PROJECT_IDS.iter().enumerate() {
            self.set_session_button(cx, *id, sessions.get(idx));
        }
        for (idx, id) in LOCAL_IDS.iter().enumerate() {
            self.set_session_button(cx, *id, sessions.get(split + idx));
        }
    }

    fn set_session_button(&self, cx: &mut Cx, id: LiveId, session: Option<&SessionInfo>) {
        let btn = self.ui.widget(
            cx,
            &[live_id!(main_window), live_id!(body), live_id!(sidebar), id],
        );
        if btn.is_empty() {
            return;
        }
        if let Some(s) = session {
            let is_selected = s.id.as_str() == self.current_session_id;
            let name = if s.name.is_empty() {
                format!("Session {}", &s.id[..s.id.len().min(6)])
            } else {
                s.name.to_string()
            };
            let text = if is_selected {
                format!(
                    "● {}    {}",
                    name,
                    format_session_time(s.created_at.as_str())
                )
            } else {
                format!(
                    "  {}    {}",
                    name,
                    format_session_time(s.created_at.as_str())
                )
            };
            btn.set_text(cx, &text);
            btn.set_visible(cx, true);
        } else {
            btn.set_visible(cx, false);
        }
    }

    fn toggle_search(&mut self, cx: &mut Cx) {
        let search = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(sidebar),
                live_id!(search_input),
            ],
        );
        if !search.is_empty() {
            search.set_key_focus(cx);
        }
    }

    fn filter_sessions(&self, cx: &mut Cx, query: &str) {
        let lower = query.to_lowercase();
        let filtered: Vec<&SessionInfo> = self
            .sessions
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&lower) || s.id.to_lowercase().contains(&lower)
            })
            .collect();
        // Flatten filtered list into sidebar slots for now
        const ALL_IDS: [LiveId; 16] = [
            live_id!(project_session_0),
            live_id!(project_session_1),
            live_id!(project_session_2),
            live_id!(project_session_3),
            live_id!(project_session_4),
            live_id!(project_session_5),
            live_id!(project_session_6),
            live_id!(project_session_7),
            live_id!(local_session_0),
            live_id!(local_session_1),
            live_id!(local_session_2),
            live_id!(local_session_3),
            live_id!(local_session_4),
            live_id!(local_session_5),
            live_id!(local_session_6),
            live_id!(local_session_7),
        ];
        for (idx, id) in ALL_IDS.iter().enumerate() {
            self.set_session_button(cx, *id, filtered.get(idx).copied());
        }
    }

    fn cycle_model(&self) {
        Self::ensure_bridge();
        if let Ok(guard) = BRIDGE.lock()
            && let Some(ref bridge) = *guard {
                // Placeholder: cycle through a few common models
                let next = if let Ok(state) = GUI_STATE.lock() {
                    match state.model.as_str() {
                        "gpt-4o" => "claude-sonnet-4",
                        "claude-sonnet-4" => "gemini-2.5-pro",
                        "gemini-2.5-pro" => "o3-mini",
                        _ => "gpt-4o",
                    }
                    .to_string()
                } else {
                    "gpt-4o".to_string()
                };
                bridge.send_action(UserAction::SetModel {
                    model: next.as_str().into(),
                });
            }
    }

    // ── Main loop ───────────────────────────────────────────────────────

    fn poll_and_render(&mut self, cx: &mut Cx) {
        self.frame_count += 1;

        if self.is_running {
            self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
            self.update_status_spinner(cx);
            if let Some(start) = self.agent_start_time {
                let elapsed = start.elapsed().as_secs();
                if elapsed != self.worked_seconds {
                    self.worked_seconds = elapsed;
                    self.set_worked_visible(cx, true, elapsed);
                }
            }
        }

        if self.frame_count == 1 {
            Self::ensure_bridge();
            self.update_top_bar(cx);
            self.update_mode_pill(cx);
        }

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

        let mut needs_bubble_render = false;
        let mut status_changed = false;

        for event in &events {
            match event {
                CoreEvent::StreamingDelta { text } => {
                    self.hide_welcome(cx);
                    self.show_msg_actions(cx, false);
                    if self.bubbles.last().map(|b| b.role.as_str()) != Some("assistant") {
                        self.bubbles.push(ChatBubble::new("assistant", ""));
                        if self.bubbles.len() > MAX_BUBBLES {
                            self.bubbles.remove(0);
                        }
                        // First token: show worked-for time and hide it
                        if let Some(start) = self.agent_start_time {
                            self.worked_seconds = start.elapsed().as_secs().max(1);
                            self.set_worked_visible(cx, true, self.worked_seconds);
                        }
                    }
                    if let Some(last) = self.bubbles.last_mut() {
                        last.content.push_str(text);
                    }
                    needs_bubble_render = true;
                }
                CoreEvent::ReasoningDelta { text } => {
                    self.hide_welcome(cx);
                    if self.bubbles.last().map(|b| b.role.as_str()) != Some("assistant") {
                        self.bubbles.push(ChatBubble::new("assistant", ""));
                        if self.bubbles.len() > MAX_BUBBLES {
                            self.bubbles.remove(0);
                        }
                    }
                    if let Some(last) = self.bubbles.last_mut() {
                        last.content.push_str(text);
                    }
                    needs_bubble_render = true;
                }
                CoreEvent::ToolCall { name, args } => {
                    self.hide_welcome(cx);
                    let text = format!("\n\n### 🔧 {}\n```json\n{}\n```\n", name, args);
                    if self.bubbles.last().map(|b| b.role.as_str()) != Some("assistant") {
                        self.bubbles.push(ChatBubble::new("assistant", text));
                    } else if let Some(last) = self.bubbles.last_mut() {
                        last.content.push_str(&text);
                    }
                    needs_bubble_render = true;
                }
                CoreEvent::ToolResult { name, output } => {
                    let out = if output.len() > 500 {
                        format!("{}...", &output[..500])
                    } else {
                        output.to_string()
                    };
                    let text = format!("\n```\n{}: {}\n```\n", name, out);
                    if let Some(last) = self.bubbles.last_mut() {
                        last.content.push_str(&text);
                    }
                    needs_bubble_render = true;
                }
                CoreEvent::SubagentToolCall { name, args } => {
                    let text = format!("\n\n### 🤖 {}\n```json\n{}\n```\n", name, args);
                    if self.bubbles.last().map(|b| b.role.as_str()) != Some("assistant") {
                        self.bubbles.push(ChatBubble::new("assistant", text));
                    } else if let Some(last) = self.bubbles.last_mut() {
                        last.content.push_str(&text);
                    }
                    needs_bubble_render = true;
                }
                CoreEvent::MessageComplete {
                    response,
                    input_tokens,
                    output_tokens,
                    ..
                } => {
                    if !response.is_empty() {
                        if self.bubbles.last().map(|b| b.role.as_str()) != Some("assistant") {
                            self.bubbles
                                .push(ChatBubble::new("assistant", response.to_string()));
                        } else if let Some(last) = self.bubbles.last_mut()
                            && !last.content.contains(response.as_str()) {
                                last.content.push_str(response);
                            }
                    }
                    needs_bubble_render = true;
                    self.set_worked_visible(cx, false, 0);
                    self.show_msg_actions(cx, true);

                    if let Ok(mut guard) = BRIDGE.lock()
                        && let Some(ref mut bridge) = *guard {
                            bridge.tokens_used += input_tokens + output_tokens;
                        }
                    if let Ok(mut guard) = GUI_STATE.lock() {
                        guard.tokens_used += input_tokens + output_tokens;
                    }
                    status_changed = true;
                }
                CoreEvent::AgentStarted => {
                    self.is_running = true;
                    self.agent_start_time = Some(Instant::now());
                    self.worked_seconds = 0;
                    self.update_running_ui(cx);
                }
                CoreEvent::AgentStopped => {
                    self.is_running = false;
                    self.agent_start_time = None;
                    self.set_worked_visible(cx, false, 0);
                    self.update_running_ui(cx);
                }
                CoreEvent::Retrying { attempt, max } => {
                    self.add_bubble(
                        cx,
                        ChatBubble::new(
                            "assistant",
                            format!("⏳ Retrying ({}/{})...\n\n", attempt, max),
                        ),
                    );
                }
                CoreEvent::Error { message } => {
                    if message.as_str() != "quit" {
                        self.add_bubble(
                            cx,
                            ChatBubble::new("assistant", format!("❌ **{}**", message)),
                        );
                        self.is_running = false;
                        self.agent_start_time = None;
                        self.set_worked_visible(cx, false, 0);
                        self.update_running_ui(cx);
                    }
                }
                CoreEvent::SessionListUpdated { sessions } => {
                    self.sessions = sessions.clone();
                    if let Ok(mut guard) = GUI_STATE.lock() {
                        guard.sessions = sessions.clone();
                    }
                    self.refresh_session_buttons(cx, sessions);
                }
                CoreEvent::SessionChanged { session_id } => {
                    self.current_session_id = session_id.to_string();
                    if let Ok(mut guard) = GUI_STATE.lock() {
                        guard.current_session_id = session_id.to_string();
                        if let Some(s) = self
                            .sessions
                            .iter()
                            .find(|s| s.id.as_str() == session_id.as_str())
                        {
                            guard.current_session_name = s.name.to_string();
                            if guard.current_session_name.is_empty() {
                                guard.current_session_name =
                                    format!("Session {}", &s.id[..s.id.len().min(6)]);
                            }
                        }
                    }
                    self.refresh_session_buttons(cx, &self.sessions);
                    self.update_top_bar(cx);
                }
                CoreEvent::SessionHistory { messages } => {
                    self.chat_messages = messages.clone();
                    self.bubbles.clear();
                    if messages.is_empty() {
                        let welcome = self.ui.widget(
                            cx,
                            &[
                                live_id!(main_window),
                                live_id!(body),
                                live_id!(main_content),
                                live_id!(chat_scroll),
                                live_id!(welcome_view),
                            ],
                        );
                        if !welcome.is_empty() {
                            welcome.set_visible(cx, true);
                        }
                    } else {
                        self.hide_welcome(cx);
                        for msg in messages {
                            let role = match msg.role.as_str() {
                                "user" => "user",
                                "assistant" => "assistant",
                                "tool_call" => "tool_call",
                                "tool_result" => "tool_result",
                                _ => "assistant",
                            };
                            self.bubbles
                                .push(ChatBubble::new(role, msg.content.to_string()));
                        }
                        needs_bubble_render = true;
                    }
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
                    self.add_bubble(
                        cx,
                        ChatBubble::new("assistant", format!("```\n{}\n```", text)),
                    );
                }
                CoreEvent::PermissionNeeded {
                    id,
                    tool_name,
                    args,
                } => {
                    self.pending_permission = Some((*id, tool_name.to_string(), args.clone()));
                    self.add_bubble(
                        cx,
                        ChatBubble::new(
                            "assistant",
                            format!(
                                "⚠️ **Permission needed:** `{}`\n```\n{}\n```\nType `/allow` to permit or `/deny` to reject.",
                                tool_name, args
                            ),
                        ),
                    );
                }
                CoreEvent::ConfigChanged => {
                    status_changed = true;
                }
                _ => {}
            }
        }

        if needs_bubble_render {
            self.render_bubbles(cx);
        }
        if status_changed {
            self.update_top_bar(cx);
            self.update_mode_pill(cx);
        }
    }
}

/// Helper: set text on a widget at path, ignoring errors silently.
fn ui_set_text(cx: &mut Cx, ui: &WidgetRef, path: &[LiveId], text: &str) {
    let w = ui.widget(cx, path);
    if !w.is_empty() {
        w.set_text(cx, text);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // ── Enter key ──
        let input = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(main_content),
                live_id!(bottom_bar),
                live_id!(input_container),
                live_id!(input_field),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<TextInputAction>(input.widget_uid()),
            TextInputAction::Returned(..)
        ) {
            self.send_input_to_engine(cx);
        }

        // ── Escape key in input → clear ──
        if matches!(
            actions.find_widget_action_cast::<TextInputAction>(input.widget_uid()),
            TextInputAction::Escaped
        ) {
            input.set_text(cx, "");
        }

        // ── New session (now bound to the nav_new_agent button) ──
        let new_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(sidebar),
                live_id!(nav_menu),
                live_id!(nav_new_agent),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(new_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            Self::ensure_bridge();
            if let Ok(guard) = BRIDGE.lock()
                && let Some(ref bridge) = *guard {
                    bridge.send_action(UserAction::CreateSession { name: None });
                }
        }

        // ── Left rail navigation ──
        let nav_new = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(nav_new_agent),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(nav_new.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            Self::ensure_bridge();
            if let Ok(guard) = BRIDGE.lock()
                && let Some(ref bridge) = *guard {
                    bridge.send_action(UserAction::CreateSession { name: None });
                }
        }

        let nav_search_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(left_rail),
                live_id!(nav_search),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(nav_search_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.toggle_search(cx);
        }

        // ── Search input changed ──
        let search = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(sidebar),
                live_id!(search_input),
            ],
        );
        if let TextInputAction::Changed(text) =
            actions.find_widget_action_cast::<TextInputAction>(search.widget_uid())
        {
            self.filter_sessions(cx, &text);
        }

        // ── Session list clicks (project) ──
        const PROJECT_BUTTON_IDS: [LiveId; 8] = [
            live_id!(project_session_0),
            live_id!(project_session_1),
            live_id!(project_session_2),
            live_id!(project_session_3),
            live_id!(project_session_4),
            live_id!(project_session_5),
            live_id!(project_session_6),
            live_id!(project_session_7),
        ];
        for (idx, btn_id) in PROJECT_BUTTON_IDS.iter().enumerate() {
            let btn = self.ui.widget(
                cx,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(sidebar),
                    *btn_id,
                ],
            );
            if !btn.is_empty()
                && matches!(
                    actions.find_widget_action_cast::<ButtonAction>(btn.widget_uid()),
                    ButtonAction::Clicked(_)
                )
            {
                if let Some(s) = self.sessions.get(idx) {
                    Self::ensure_bridge();
                    if let Ok(guard) = BRIDGE.lock()
                        && let Some(ref bridge) = *guard {
                            bridge.send_action(UserAction::SwitchSession {
                                session_id: s.id.clone(),
                            });
                        }
                }
                break;
            }
        }

        // ── Session list clicks (local) ──
        const LOCAL_BUTTON_IDS: [LiveId; 8] = [
            live_id!(local_session_0),
            live_id!(local_session_1),
            live_id!(local_session_2),
            live_id!(local_session_3),
            live_id!(local_session_4),
            live_id!(local_session_5),
            live_id!(local_session_6),
            live_id!(local_session_7),
        ];
        for (idx, btn_id) in LOCAL_BUTTON_IDS.iter().enumerate() {
            let btn = self.ui.widget(
                cx,
                &[
                    live_id!(main_window),
                    live_id!(body),
                    live_id!(sidebar),
                    *btn_id,
                ],
            );
            if !btn.is_empty()
                && matches!(
                    actions.find_widget_action_cast::<ButtonAction>(btn.widget_uid()),
                    ButtonAction::Clicked(_)
                )
            {
                let split = self.sessions.len().min(8);
                if let Some(s) = self.sessions.get(split + idx) {
                    Self::ensure_bridge();
                    if let Ok(guard) = BRIDGE.lock()
                        && let Some(ref bridge) = *guard {
                            bridge.send_action(UserAction::SwitchSession {
                                session_id: s.id.clone(),
                            });
                        }
                }
                break;
            }
        }

        // ── Mode selector is now combined into the model button ──

        // ── Model selector ──
        let model_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(main_content),
                live_id!(bottom_bar),
                live_id!(input_container),
                live_id!(model_btn),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(model_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.cycle_model();
        }

        // ── Stop button ──
        let stop_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(right_rail),
                live_id!(tool_stop),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(stop_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.is_running = false;
            self.agent_start_time = None;
            self.set_worked_visible(cx, false, 0);
            self.update_running_ui(cx);
            if let Ok(guard) = BRIDGE.lock()
                && let Some(ref bridge) = *guard {
                    bridge.send_action(UserAction::CancelStream);
                }
        }

        // ── Attach button ──
        let attach_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(main_content),
                live_id!(bottom_bar),
                live_id!(input_container),
                live_id!(attach_btn),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(attach_btn.widget_uid()),
            ButtonAction::Clicked(_)
        )
            && !input.is_empty() {
                input.set_text(cx, "/add ");
                input.set_key_focus(cx);
            }

        // ── Right rail placeholders ──
        let tool_browser_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(right_rail),
                live_id!(tool_browser),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(tool_browser_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.add_bubble(
                cx,
                ChatBubble::new("assistant", "🌐 Browser panel is not yet implemented."),
            );
        }

        let tool_terminal_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(right_rail),
                live_id!(tool_terminal),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(tool_terminal_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.add_bubble(
                cx,
                ChatBubble::new("assistant", ">_ Terminal panel is not yet implemented."),
            );
        }

        let tool_canvas_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(right_rail),
                live_id!(tool_canvas),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(tool_canvas_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.add_bubble(
                cx,
                ChatBubble::new("assistant", "▦ Canvas panel is not yet implemented."),
            );
        }

        let tool_fullscreen_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(right_rail),
                live_id!(tool_fullscreen),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(tool_fullscreen_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.add_bubble(
                cx,
                ChatBubble::new("assistant", "⛶ Fullscreen view is not yet implemented."),
            );
        }

        // ── Top model selector ──
        let top_model_btn = self.ui.widget(
            cx,
            &[
                live_id!(main_window),
                live_id!(body),
                live_id!(main_content),
                live_id!(top_bar),
                live_id!(top_model),
            ],
        );
        if matches!(
            actions.find_widget_action_cast::<ButtonAction>(top_model_btn.widget_uid()),
            ButtonAction::Clicked(_)
        ) {
            self.cycle_model();
        }
    }
}

pub fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    (r, g, b)
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
