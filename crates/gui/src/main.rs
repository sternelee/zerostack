//! Binary entry for the zerostack GPUI frontend.
//!
//! Boots the [`ShellState`] as the only top-level view, then drives engine events in via a
//! recurring tick spawned from inside the application loop.

#![deny(unsafe_code)]

use zerostack_gui::view::run;

fn main() {
    run();
}
