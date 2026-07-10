use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    UserMessage = {{UserMessage}} {
        width: Fill, height: Fit,
        flow: Right, padding: {top: 8, bottom: 8},
        <View> {
            width: Fill, height: Fit, flow: Right,
            <View> {
                width: Fit, height: Fit,
                show_bg: true,
                draw_bg: { color: #2D2E3F, border_radius: 12.0 }
                padding: {left: 16, right: 16, top: 10, bottom: 10},
                <Label> {
                    width: Fill, text: "",
                    draw_text: {
                        color: #CDD6F4,
                        text_style: <THEME_FONT_REGULAR> { font_size: 14.0 },
                    }
                    wrap: Word,
                }
            }
        }
    }

    AssistantMessage = {{AssistantMessage}} {
        width: Fill, height: Fit,
        flow: Left, padding: {top: 8, bottom: 8},
        <View> {
            width: Fill, height: Fit, flow: Down,
            <Label> {
                text: "",
                draw_text: {
                    color: #CDD6F4,
                    text_style: <THEME_FONT_REGULAR> { font_size: 14.0 },
                }
                wrap: Word,
                padding: {left: 4, right: 4, top: 4, bottom: 4},
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct UserMessage {
    #[live]
    ui: WidgetRef,
}
impl Widget for UserMessage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct AssistantMessage {
    #[live]
    ui: WidgetRef,
}
impl Widget for AssistantMessage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
