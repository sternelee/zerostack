use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let app = startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(1024, 700)
                window.title: "zerostack"

                body <View> {
                    width: Fill, height: Fill
                    flow: Right
                    spacing: 0.0
                    show_bg: true
                    draw_bg.color: #1A1B26

                    // Sidebar
                    <RoundedView> {
                        width: 220.0, height: Fill
                        draw_bg.color: #1F2030
                        draw_bg.radius: 0.0
                        flow: Down
                        padding: {top: 8.0, bottom: 8.0}
                        spacing: 4.0

                        <View> {
                            width: Fill, height: Fit
                            padding: {left: 12.0, right: 12.0, top: 4.0, bottom: 4.0}
                            flow: Right
                            align: {y: 0.5}

                            <Label> {
                                text: "Sessions"
                                draw_text.color: #6C7086
                                draw_text.text_style.font_size: 11.0
                            }

                            <View> { width: Fill, height: 0.0 }

                            new_session_btn := <Button> {
                                text: "+"
                                width: 28.0, height: 28.0
                                draw_bg.color: #7C6FF0
                                draw_bg.radius: 4.0
                                on_click: || {
                                    log("New session")
                                }
                            }
                        }

                        session_list := <PortalList> {
                            width: Fill, height: Fill
                        }
                    }

                    // Main content
                    <View> {
                        width: Fill, height: Fill
                        flow: Down
                        spacing: 0.0

                        // Header
                        <View> {
                            width: Fill, height: 40.0
                            show_bg: true
                            draw_bg.color: #1A1B26
                            padding: {left: 16.0, right: 16.0}
                            flow: Right
                            align: {y: 0.5}

                            <Label> {
                                text: "zerostack"
                                draw_text.color: #CDD6F4
                                draw_text.text_style.font_size: 14.0
                            }
                        }

                        // Chat area
                        <PortalList> {
                            width: Fill, height: Fill
                            padding: {left: 16.0, right: 16.0, top: 12.0, bottom: 12.0}

                            chat_content := <View> {
                                width: Fill, height: Fit
                                flow: Down
                                spacing: 8.0

                                <Label> {
                                    text: "Welcome to zerostack GUI"
                                    draw_text.color: #6C7086
                                    draw_text.text_style.font_size: 13.0
                                }
                            }
                        }

                        // Input bar
                        <View> {
                            width: Fill, height: Fit
                            show_bg: true
                            draw_bg.color: #1F2030
                            flow: Down
                            spacing: 0.0

                            <View> {
                                width: Fill, height: 60.0
                                padding: {left: 16.0, right: 16.0, top: 12.0, bottom: 12.0}
                                flow: Right
                                spacing: 8.0
                                align: {y: 0.5}

                                input_field := <TextInput> {
                                    width: Fill, height: 36.0
                                    empty_text: "Type a message... (Enter to send)"
                                    draw_bg.color: #2D2E3F
                                    draw_bg.radius: 8.0
                                    draw_text.color: #CDD6F4
                                    draw_text.text_style.font_size: 14.0
                                }

                                send_btn := <Button> {
                                    text: "Send"
                                    width: 72.0, height: 36.0
                                    draw_bg.color: #7C6FF0
                                    draw_bg.radius: 8.0
                                    draw_text.color: #FFFFFF
                                    draw_text.text_style.font_size: 13.0
                                    on_click: || {
                                        let text = input_field.text()
                                        if text != "" {
                                            log("Send: " + text)
                                            input_field.set_text("")
                                            input_field.set_key_focus()
                                        }
                                    }
                                }
                            }

                            // Status bar
                            <View> {
                                width: Fill, height: 28.0
                                padding: {left: 16.0, right: 16.0}
                                flow: Right
                                spacing: 16.0
                                align: {y: 0.5}
                                show_bg: true
                                draw_bg.color: #1A1B26

                                status_model := <Label> {
                                    text: "claude-sonnet"
                                    draw_text.color: #6C7086
                                    draw_text.text_style.font_size: 11.0
                                }

                                <View> { width: Fill, height: 0.0 }

                                status_tokens := <Label> {
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
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
