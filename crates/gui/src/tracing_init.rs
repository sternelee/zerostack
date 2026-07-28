// Initialise a process-wide tracing subscriber that writes to stderr. We
// call this from `view::run` before the engine thread spawns so any
// tracing events from the Wasm registry, the GUI poll loop, or the
// bridge's tokio runtime all show up on the user's terminal. The GUI
// itself uses stdout as the os-drawing surface on macOS, so logs go to
// stderr explicitly.

use std::io::IsTerminal;
use std::sync::Once;

static INIT: Once = Once::new();

pub fn init() {
    INIT.call_once(|| {
        use tracing_subscriber::EnvFilter;
        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        // tracing-subscriber's fmt subscriber owns its writer; we keep
        // the inner stderr writer direct so it works inside a static
        // lazy init.
        let is_terminal = std::io::stderr().is_terminal();
        let builder = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(std::io::stderr)
            .with_ansi(is_terminal);
        let _ = builder.try_init();
    });
}
