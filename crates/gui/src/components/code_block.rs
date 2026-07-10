use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    CodeBlock = {{CodeBlock}} {
        width: Fill, height: Fit,
        show_bg: true,
        draw_bg: { color: #0D0E1A, border_radius: 8.0 }
        padding: {left: 16, right: 16, top: 12, bottom: 12},
        flow: Down, spacing: 4,
        <Label> {
            text: "",
            draw_text: {
                color: #CDD6F4,
                text_style: { font_size: 13.0 },
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct CodeBlock {
    #[live]
    ui: WidgetRef,
}
impl Widget for CodeBlock {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
