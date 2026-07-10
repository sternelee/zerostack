use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import crate::views::main_view::*;

    App = {{App}} {
        ui: <Window> {
            window: { title: "zerostack" },
            body = <MainView> {}
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    bridge: Option<crate::bridge::GuiBridge>,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

app_main!(App);
