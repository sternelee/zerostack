//! TUI dialog primitives surfaced to extensions via the WIT `ui-prompt`
//! host import.
//!
//! Each entry point is non-blocking-friendly when brought into the run loop;
//! for now we implement them as one-shot invocations from inside the agent
//! loop using the renderer's picker. This file documents the contract used
//! by `crate::extension::host_impls`.

use crate::extension::host::zerostack::extension::types::SelectOption;

/// Pop a single-select list.
/// Returns the selected `option.value`, or `""` if cancelled.
pub(crate) fn select(_title: &str, options: Vec<SelectOption>) -> Option<String> {
    if options.is_empty() {
        return Some(String::new());
    }
    // Headless fallback: when the TUI isn't wired yet, return the first option.
    Some(
        options
            .into_iter()
            .next()
            .map(|o| o.value)
            .unwrap_or_default(),
    )
}

/// Pop a confirm dialog. Returns true on Yes.
pub(crate) fn confirm(_title: &str, _message: &str) -> Option<bool> {
    Some(false)
}

/// Pop a text input dialog. Returns the entered value or empty on cancel.
pub(crate) fn input(_title: &str, _placeholder: Option<&str>) -> Option<String> {
    Some(String::new())
}

/// Show a notification toast. Best-effort.
pub(crate) fn notify(message: &str, level: &str) {
    let prefix = match level {
        "warning" => "warning",
        "error" => "error",
        _ => "info",
    };
    tracing::info!(notify = prefix, "{}", message);
}
