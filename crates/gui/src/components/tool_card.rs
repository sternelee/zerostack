use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    ToolCallCard = {{ToolCallCard}} {
        width: Fill, height: Fit,
        show_bg: true,
        draw_bg: {
            color: #1F2030, border_radius: 8.0,
            border_width: 1.0, border_color: #2D2E3F,
        }
        padding: {left: 12, right: 12, top: 8, bottom: 8},
        flow: Down, spacing: 4,
        <Label> {
            text: "🔧 tool_name",
            draw_text: {
                color: #7C6FF0,
                text_style: <THEME_FONT_BOLD> { font_size: 12.0 },
            }
        }
    }

    ToolResultCard = {{ToolResultCard}} {
        width: Fill, height: Fit,
        show_bg: true,
        draw_bg: {
            color: #1A1B26, border_radius: 8.0,
            border_width: 1.0, border_color: #2D2E3F,
        }
        padding: {left: 12, right: 12, top: 8, bottom: 8},
        flow: Down, spacing: 4,
        <Label> {
            text: "",
            draw_text: {
                color: #6C7086,
                text_style: <THEME_FONT_REGULAR> { font_size: 12.0 },
            }
            wrap: Word,
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct ToolCallCard {
    #[live]
    ui: WidgetRef,
}
impl Widget for ToolCallCard {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct ToolResultCard {
    #[live]
    ui: WidgetRef,
}
impl Widget for ToolResultCard {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ui.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ui.draw_walk(cx, scope, walk)
    }
}
