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

    // Pill-shaped chips used in the input-bar status row, sidebar quick
    // actions, and the welcome-page shortcut cards. Subtle border + dark
    // fill so they read as inert metadata even at small sizes; the hover
    // step is gentle so a row of them sits quietly rather than competing
    // for attention.
    pub const CHIP_BG: u32 = 0x131318;
    pub const CHIP_HOVER: u32 = 0x1E1E24;
    pub const CHIP_BORDER: u32 = 0x26262C;
    pub const CHIP_ACCENT_BG: u32 = 0x1F1A2E;

    // The sidebar's small `--` and `>` glyphs etc. share a single muted
    // tone so they don't pull focus from the session names. Tied to
    // `BORDER_HOVER` so a hover on the row tints both at once.
    pub const ICON_MUTED: u32 = 0x6E6E75;

    pub const BORDER: u32 = 0x1F1F24;
    pub const BORDER_HOVER: u32 = 0x2E2E34;

    pub const TEXT: u32 = 0xF2F2F5;
    pub const TEXT_SECONDARY: u32 = 0x9A9AA0;
    pub const TEXT_MUTED: u32 = 0x6E6E75;

    pub const ACCENT: u32 = 0xC4B5FD;
    pub const ACCENT_DEEP: u32 = 0x2A2440;
    pub const SUCCESS: u32 = 0xA6E3A1;
    pub const WARNING: u32 = 0xF9E2AF;
    pub const ERROR: u32 = 0xF38BA8;
}
