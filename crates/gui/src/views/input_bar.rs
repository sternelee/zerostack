use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    InputBar = {{InputBar}} {
        width: Fill, height: Fit,
        flow: Down,
        show_bg: true,
        draw_bg: { color: #1F2030 }

        <View> {
            width: Fill, height: 60,
            padding: {left: 16, right: 16, top: 12, bottom: 12},
            flow: Right, spacing: 8, align: {y: 0.5},

            <TextInput> {
                width: Fill, height: 36,
                empty_message: "Type a message... (Enter to send)",
                draw_bg: { color: #2D2E3F, border_radius: 8.0 }
                draw_text: {
                    color: #CDD6F4,
                    text_style: <THEME_FONT_REGULAR> { font_size: 14.0 },
                }
            }
            <Button> {
                text: "Send", width: 72, height: 36,
                draw_bg: { color: #7C6FF0, border_radius: 8.0 }
                draw_text: {
                    color: #fff,
                    text_style: <THEME_FONT_BOLD> { font_size: 13.0 },
                }
            }
        }

        <View> {
            width: Fill, height: 28,
            padding: {left: 16, right: 16},
            flow: Right, spacing: 16, align: {y: 0.5},

            <Label> {
                text: "claude-sonnet",
                draw_text: {
                    color: #6C7086,
                    text_style: <THEME_FONT_REGULAR> { font_size: 11.0 },
                }
            }
            <View> { width: Fill, height: 0 }
            <Label> {
                text: "0 tokens",
                draw_text: {
                    color: #6C7086,
                    text_style: <THEME_FONT_REGULAR> { font_size: 11.0 },
                }
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct InputBar {
    #[live]
    ui: WidgetRef,
}

impl Widget for InputBar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
