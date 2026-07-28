#![deny(unsafe_code)]
#![allow(clippy::type_complexity)]

pub mod bridge;
pub mod markdown;
pub mod theme;
pub mod tool_utils;
pub mod tracing_init;
pub mod view;

pub use bridge::GuiBridge;
pub use tracing_init::init;
pub use view::{ChatMessage, Role, ShellState};
