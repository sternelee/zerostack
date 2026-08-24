//! Color tokens for the zerostack GUI.
//!
//! Dark graphite palette in the spirit of Waku / Claude Desktop: neutral
//! surfaces, color reserved for meaning (status, accent). The accent stays
//! zerostack violet; everything else is a neutral ramp.
//!
//! Tokens are either `u32` hex (for `rgb(...)`) or `(u8,u8,u8,u8)` tuples
//! (for `rgba(...)`) so the renderer can hand them straight to GPUI.
//!
//! The ramp (dark):
//!   canvas  #1A1A1A  app background / transcript background
//!   raised  #232323  user bubble, hover cards
//!   inset   #151515  code blocks, tool detail, deepest surfaces
//!   borders are white-with-alpha hairlines, like native macOS.

pub mod dark {
    // ── Surfaces ─────────────────────────────────────────────
    pub const APP_BG: u32 = 0x1A1A1A;
    pub const SIDEBAR_BG: u32 = 0x1A1A1A;
    pub const CHAT_BG: u32 = 0x1A1A1A;

    /// User bubble / hover cards / raised controls.
    pub const RAISED: u32 = 0x232323;
    /// Code blocks, tool detail, deepest inset surfaces.
    pub const INSET: u32 = 0x151515;
    /// Composer / input background.
    pub const COMPOSER: u32 = 0x212121;

    // Legacy aliases (kept so call sites read naturally; same values).
    pub const USER_BUBBLE_BG: u32 = RAISED;
    pub const ASST_BUBBLE_BG: u32 = CHAT_BG;
    pub const TOOL_BUBBLE_BG: u32 = INSET;
    pub const INPUT_BG: u32 = COMPOSER;
    pub const BUTTON_BG: u32 = RAISED;

    // ── Hairlines & fills (alpha-based, native feel) ─────────
    /// Soft fill: system pill, table header, card washes. ~5% white.
    pub const OVERLAY: u32 = 0xE5EAF413;
    /// Hover fills, icon-button hover. ~9% white.
    pub const OVERLAY_STRONG: u32 = 0xE5EAF417;
    /// Hairline dividers. ~7% white.
    pub const BORDER: u32 = 0xE5EAF412;
    /// Card outlines, stronger dividers. ~14% white.
    pub const BORDER_STRONG: u32 = 0xE5EAF424;

    // ── Chips (kept from the first pass; harmonized with the ramp) ──
    pub const CHIP_BG: u32 = 0x202024;
    pub const CHIP_HOVER: u32 = 0x2A2A2E;
    pub const CHIP_BORDER: u32 = 0xE5EAF416;
    pub const CHIP_ACCENT_BG: u32 = 0x2A2440;

    // Legacy aliases.
    pub const BUTTON_HOVER: u32 = RAISED;
    pub const BORDER_HOVER: u32 = 0xE5EAF424;

    pub const ICON_MUTED: u32 = 0x575757;

    // ── Text ramp ────────────────────────────────────────────
    pub const TEXT: u32 = 0xE2E2E2;
    pub const TEXT_SECONDARY: u32 = 0xA3A3A3;
    pub const TEXT_TERTIARY: u32 = 0x7D7D7D;
    pub const TEXT_GHOST: u32 = 0x575757;

    // Legacy aliases.
    pub const TEXT_MUTED: u32 = TEXT_TERTIARY;

    // ── Accent & status ──────────────────────────────────────
    pub const ACCENT: u32 = 0xC4B5FD;
    pub const ACCENT_DEEP: u32 = 0x2A2440;
    pub const SUCCESS: u32 = 0x62C987;
    pub const WARNING: u32 = 0xE0B36A;
    pub const ERROR: u32 = 0xE2726A;
    /// Red-tinted wash for destructive hovers (stop button). ~10% red.
    pub const DANGER_SOFT: u32 = 0xE2726A1A;

    /// Primary pill fill (send button): light so the dark arrow pops.
    pub const INVERSE: u32 = 0xE7E9EC;
    /// Glyph on the primary pill.
    pub const ON_INVERSE: u32 = 0x17181C;
    /// Favorite-star amber.
    pub const FAVORITE: u32 = 0xEAB308;

    /// Inline code text (warm tan, restrained).
    pub const CODE_TEXT: u32 = 0xE0A882;
    /// Inline code wash background. ~8% white.
    pub const CODE_WASH: u32 = 0xE5EAF414;

    // ── Syntax token colors (restrained: three hues + muted) ──
    pub const TOKEN_KEYWORD: u32 = 0xC98BC0;
    pub const TOKEN_LITERAL: u32 = 0xD9A05B;
    pub const TOKEN_STRING: u32 = 0x94C08A;
    pub const TOKEN_TYPE: u32 = 0x8FB8D9;
}

/// Convenience: turn an alpha pair into a border color arg for `border_color`,
/// matching what the old solid-hex borders did but with proper hairlines.
/// Kept as documentation of intent; call sites use `rgba(...)` directly.
#[allow(dead_code)]
pub fn border() -> u32 {
    dark::BORDER
}
