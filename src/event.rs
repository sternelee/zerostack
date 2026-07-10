use compact_str::CompactString;

pub use zerostack_core::event::{AgentEvent, BtwEvent};

#[derive(Debug, Clone)]
pub enum UserEvent {
    Key(crossterm::event::KeyEvent),
    ScrollUp,
    ScrollDown,
    Resize,
    Paste(String),
    #[allow(dead_code)]
    MouseDown {
        row: u16,
        col: u16,
    },
    #[allow(dead_code)]
    MouseDrag {
        row: u16,
        col: u16,
    },
    #[allow(dead_code)]
    MouseUp {
        row: u16,
        col: u16,
    },
    /// An interactive MCP OAuth login finished in a background task. `error` is
    /// `None` on success. Handled by the TUI loop to reconnect the server.
    #[cfg(feature = "mcp")]
    McpLoginDone {
        server: CompactString,
        error: Option<CompactString>,
    },
}
