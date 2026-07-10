use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    Sidebar = {{Sidebar}} {
        width: 220, height: Fill,
        flow: Down,
        show_bg: true,
        draw_bg: { color: #1F2030 }

        <View> {
            width: Fill, height: 40,
            padding: {left: 12, right: 12},
            flow: Right, align: {y: 0.5}, spacing: 8,

            <Label> {
                text: "Sessions",
                draw_text: {
                    color: #6C7086,
                    text_style: <THEME_FONT_BOLD> { font_size: 11.0 },
                }
            }
            <View> { width: Fill, height: 0 }
            <Button> {
                text: "+", width: 28, height: 28,
                draw_bg: { color: #7C6FF0, border_radius: 4.0 }
                draw_text: {
                    color: #fff,
                    text_style: <THEME_FONT_BOLD> { font_size: 14.0 },
                }
            }
        }

        <ScrollView> {
            width: Fill, height: Fill,
            flow: Down,
            padding: {left: 4, right: 4},
            <View> { width: Fill, height: Fit, flow: Down, spacing: 2 }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct Sidebar {
    #[live]
    ui: WidgetRef,
}

impl Widget for Sidebar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
