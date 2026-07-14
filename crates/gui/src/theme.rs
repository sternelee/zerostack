// shadcn/ui-inspired zinc dark palette
pub mod dark {
    // App shell
    pub const APP_BG: &str = "#09090B"; // zinc-950
    pub const SIDEBAR_BG: &str = "#0C0C0F"; // zinc-925
    pub const LEFT_RAIL_BG: &str = APP_BG;
    pub const RIGHT_RAIL_BG: &str = APP_BG;
    pub const TOP_BAR_BG: &str = APP_BG;
    pub const BOTTOM_BAR_BG: &str = APP_BG;

    // Chat
    pub const CHAT_BG: &str = APP_BG;
    pub const USER_BUBBLE_BG: &str = "#18181B"; // zinc-900
    pub const ASST_BUBBLE_BG: &str = APP_BG;
    pub const TOOL_BUBBLE_BG: &str = "#141417"; // zinc-925 variant
    pub const WORKED_LABEL_BG: &str = "#18181B";

    // Input / controls
    pub const INPUT_BG: &str = "#141417"; // zinc-925
    pub const BUTTON_BG: &str = "#18181B"; // zinc-900
    pub const BUTTON_HOVER: &str = "#27272A"; // zinc-800
    pub const BUTTON_ACTIVE: &str = "#3F3F46"; // zinc-700
    pub const CHIP_BG: &str = "#141417";

    // Borders
    pub const BORDER: &str = "#27272A"; // zinc-800
    pub const BORDER_HOVER: &str = "#3F3F46";

    // Text
    pub const TEXT: &str = "#FAFAFA"; // zinc-50
    pub const TEXT_SECONDARY: &str = "#A1A1AA"; // zinc-400
    pub const TEXT_MUTED: &str = "#71717A"; // zinc-500

    // Accents
    pub const ACCENT: &str = "#FAFAFA";
    pub const ACCENT_HOVER: &str = "#FFFFFF";
    pub const SUCCESS: &str = "#22C55E"; // green-500
    pub const WARNING: &str = "#F59E0B"; // amber-500
    pub const ERROR: &str = "#EF4444"; // red-500
    pub const ERROR_HOVER: &str = "#FCA5A5"; // red-300

    // Legacy aliases
    pub const BG: &str = APP_BG;
    pub const SIDEBAR_BG_LEGACY: &str = SIDEBAR_BG;
    pub const USER_MSG_BG: &str = USER_BUBBLE_BG;
    pub const ASST_MSG_BG: &str = ASST_BUBBLE_BG;
    pub const CODE_BG: &str = "#141417";
}

pub mod light {
    pub const BG: &str = "#FFFFFF";
    pub const SIDEBAR_BG: &str = "#F7F7F8";
    pub const USER_MSG_BG: &str = "#F0F0F0";
    pub const ASST_MSG_BG: &str = "#FFFFFF";
    pub const CODE_BG: &str = "#F5F5F5";
    pub const ACCENT: &str = "#6C5CE7";
    pub const BORDER: &str = "#E5E5E5";
    pub const TEXT: &str = "#1A1A2E";
    pub const TEXT_SECONDARY: &str = "#8E8EA0";
    pub const ERROR: &str = "#EF4444";
    pub const SUCCESS: &str = "#22C55E";
    pub const WARNING: &str = "#F59E0B";
}
