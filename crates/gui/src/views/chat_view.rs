use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    ChatView = {{ChatView}} {
        width: Fill, height: Fill,
        flow: Down,
        show_bg: true,
        draw_bg: { color: #1A1B26 }

        <ScrollView> {
            width: Fill, height: Fill,
            flow: Down,
            padding: {left: 16, right: 16, top: 16, bottom: 16},
            spacing: 4,
            <View> { width: Fill, height: Fit, flow: Down, spacing: 4 }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct ChatView {
    #[live]
    ui: WidgetRef,
}

impl Widget for ChatView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
