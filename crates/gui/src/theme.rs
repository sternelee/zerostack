pub mod dark {
    // App shell — near-black premium dark
    pub const APP_BG: &str = "#050505";
    pub const SIDEBAR_BG: &str = "#0A0A0C";
    pub const LEFT_RAIL_BG: &str = APP_BG;
    pub const RIGHT_RAIL_BG: &str = APP_BG;
    pub const TOP_BAR_BG: &str = APP_BG;
    pub const BOTTOM_BAR_BG: &str = APP_BG;

    // Chat
    pub const CHAT_BG: &str = APP_BG;
    pub const USER_BUBBLE_BG: &str = "#1A1A1F";
    pub const ASST_BUBBLE_BG: &str = APP_BG;
    pub const TOOL_BUBBLE_BG: &str = "#131317";
    pub const WORKED_LABEL_BG: &str = "#1A1A1F";

    // Input / controls
    pub const INPUT_BG: &str = "#121216";
    pub const BUTTON_BG: &str = "#18181C";
    pub const BUTTON_HOVER: &str = "#232328";
    pub const BUTTON_ACTIVE: &str = "#2E2E34";
    pub const CHIP_BG: &str = "#121216";

    // Borders
    pub const BORDER: &str = "#1F1F24";
    pub const BORDER_HOVER: &str = "#2E2E34";

    // Text
    pub const TEXT: &str = "#F2F2F5";
    pub const TEXT_SECONDARY: &str = "#9A9AA0";
    pub const TEXT_MUTED: &str = "#6E6E75";

    // Accents
    pub const ACCENT: &str = "#F2F2F5";
    pub const ACCENT_HOVER: &str = "#FFFFFF";
    pub const SUCCESS: &str = "#A6E3A1";
    pub const WARNING: &str = "#F9E2AF";
    pub const ERROR: &str = "#F38BA8";

    // Legacy aliases (keep existing code references working)
    pub const BG: &str = APP_BG;
    pub const SIDEBAR_BG_LEGACY: &str = SIDEBAR_BG;
    pub const USER_MSG_BG: &str = USER_BUBBLE_BG;
    pub const ASST_MSG_BG: &str = ASST_BUBBLE_BG;
    pub const CODE_BG: &str = "#131317";
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
    pub const ERROR: &str = "#E74C3C";
    pub const SUCCESS: &str = "#27AE60";
    pub const WARNING: &str = "#F39C12";
}
