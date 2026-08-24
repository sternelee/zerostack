#![deny(unsafe_code)]
#![allow(clippy::type_complexity)]

pub mod bridge;
pub mod highlight;
pub mod markdown;
pub mod scrollbar;
pub mod theme;
pub mod tool_utils;
pub mod tooltip;
pub mod tracing_init;
pub mod view;

pub use bridge::GuiBridge;
pub use tracing_init::init;
pub use view::{run, run_with_args};
pub use zerostack_core::permission::SecurityMode;
