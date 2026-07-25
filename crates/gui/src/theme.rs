//! Color tokens for the zerostack GUI.
//!
//! Dark-by-default palette in the spirit of the deleted makepad shell: a near-black canvas,
//! soft user bubble, glass-y tool rows. Tokens are hex strings so the renderer can hand them
//! straight to GPUI's `rgb(...)` constructor.

pub mod dark {
    pub const APP_BG: u32 = 0x050505;
    pub const SIDEBAR_BG: u32 = 0x0A0A0C;
    pub const CHAT_BG: u32 = 0x050505;

    pub const USER_BUBBLE_BG: u32 = 0x1A1A1F;
    pub const ASST_BUBBLE_BG: u32 = 0x0E0E12;
    pub const TOOL_BUBBLE_BG: u32 = 0x131317;

    pub const INPUT_BG: u32 = 0x121216;
    pub const BUTTON_BG: u32 = 0x18181C;
    pub const BUTTON_HOVER: u32 = 0x232328;

    pub const BORDER: u32 = 0x1F1F24;
    pub const BORDER_HOVER: u32 = 0x2E2E34;

    pub const TEXT: u32 = 0xF2F2F5;
    pub const TEXT_SECONDARY: u32 = 0x9A9AA0;
    pub const TEXT_MUTED: u32 = 0x6E6E75;

    pub const ACCENT: u32 = 0xC4B5FD;
    pub const SUCCESS: u32 = 0xA6E3A1;
    pub const WARNING: u32 = 0xF9E2AF;
    pub const ERROR: u32 = 0xF38BA8;
}
