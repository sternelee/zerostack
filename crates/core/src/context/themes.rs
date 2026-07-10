use std::collections::HashMap;
use std::path::PathBuf;

use include_dir::{Dir, include_dir};

static EMBEDDED: Dir = include_dir!("$CARGO_MANIFEST_DIR/data/themes");

pub fn global_dir() -> PathBuf {
    crate::session::storage::data_dir().join("themes")
}

pub fn load() -> HashMap<String, String> {
    let mut themes: HashMap<String, String> = HashMap::new();

    for (name, content) in crate::context::load_embedded_files(&EMBEDDED, "json") {
        themes.entry(name).or_insert(content);
    }
    for (name, content) in crate::context::load_dir_files(&global_dir(), "json") {
        themes.insert(name, content);
    }
    for (name, content) in crate::context::load_dir_files(&PathBuf::from("data/themes"), "json") {
        themes.insert(name, content);
    }

    themes
}

pub fn ensure_global() -> anyhow::Result<()> {
    let dir = global_dir();
    if !dir.exists() {
        crate::context::copy_embedded_to(&EMBEDDED, &dir)?;
    }
    Ok(())
}

pub fn regen() -> anyhow::Result<()> {
    let dir = global_dir();
    crate::context::copy_embedded_to(&EMBEDDED, &dir)
}

/// Parse a theme JSON into its ColorsConfig struct.
pub fn parse_theme_colors(content: &str) -> Option<crate::config::ColorsConfig> {
    serde_json::from_str(content).ok()
}
