use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import crate::views::sidebar::*;
    import crate::views::chat_view::*;
    import crate::views::input_bar::*;

    MainView = {{MainView}} {
        width: Fill, height: Fill,
        flow: Right,
        spacing: 0,

        <Sidebar> {}

        <View> {
            width: Fill, height: Fill,
            flow: Down,
            spacing: 0,

            <View> {
                width: Fill, height: 40,
                show_bg: true,
                draw_bg: { color: #1A1B26 }
                padding: {left: 16, right: 16},
                flow: Right,
                align: {y: 0.5},

                <Label> {
                    text: "zerostack",
                    draw_text: {
                        color: #CDD6F4,
                        text_style: <THEME_FONT_BOLD> { font_size: 14.0 },
                    }
                }
            }

            <ChatView> {}
            <InputBar> {}
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct MainView {
    #[live]
    ui: WidgetRef,
}

impl Widget for MainView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
