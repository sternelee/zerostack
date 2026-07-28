//! Extension discovery and manifest parsing.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Parsed contents of an `extension.toml` file.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub schema_version: u32,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,

    #[serde(default, alias = "extension")]
    pub extension: ExtensionSection,
    #[serde(default)]
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExtensionSection {
    /// Relative path to the compiled .wasm file.
    #[serde(default = "default_entrypoint")]
    pub entrypoint: String,
    /// Minimum zerostack version required.
    #[serde(default)]
    pub minimum_zerostack_version: Option<String>,
}

impl Default for ExtensionSection {
    fn default() -> Self {
        Self {
            entrypoint: default_entrypoint(),
            minimum_zerostack_version: None,
        }
    }
}

fn default_entrypoint() -> String {
    "extension.wasm".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Capabilities {
    #[serde(default = "Capabilities::tools_default")]
    pub tools: bool,
    #[serde(default)]
    pub commands: bool,
    #[serde(default)]
    pub lifecycle: bool,
    #[serde(default)]
    pub provider: bool,
    #[serde(default)]
    pub ui: bool,
    #[serde(default)]
    pub exec: bool,
    #[serde(default)]
    pub http: bool,
    #[serde(default)]
    pub session: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            tools: false,
            commands: false,
            lifecycle: false,
            provider: false,
            ui: false,
            exec: false,
            http: false,
            session: false,
        }
    }
}

impl Capabilities {
    fn tools_default() -> bool {
        false
    }
}

/// Discovered extension on disk.
#[derive(Debug, Clone)]
pub struct ExtensionBundle {
    /// The manifest parsed from extension.toml.
    pub manifest: ExtensionManifest,
    /// Absolute path to the extension directory (contains extension.toml).
    pub dir: PathBuf,
    /// Absolute path to the compiled .wasm file.
    pub wasm_path: PathBuf,
}

/// Parse an `extension.toml` file and validate essential fields.
pub fn parse_manifest(path: &Path) -> Result<ExtensionManifest, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read {path:?}: {e}"))?;
    let manifest: ExtensionManifest =
        toml::from_str(&content).map_err(|e| format!("Failed to parse {path:?}: {e}"))?;

    // Validate required fields.
    if manifest.id.is_empty() {
        return Err(format!("{path:?}: 'id' is required"));
    }
    if manifest.name.is_empty() {
        return Err(format!("{path:?}: 'name' is required"));
    }
    if manifest.version.is_empty() {
        return Err(format!("{path:?}: 'version' is required"));
    }

    Ok(manifest)
}

/// Discover all extension bundles in a directory.
///
/// Scans subdirectories for `extension.toml` and validates each.
pub fn discover_extensions(extensions_dir: &Path) -> Vec<ExtensionBundle> {
    let mut bundles = Vec::new();

    let Ok(entries) = std::fs::read_dir(extensions_dir) else {
        return bundles;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("extension.toml");
        if !manifest_path.exists() {
            // Also check for loose .wasm files with convention-based names.
            // For MVP, we only support extension.toml-based extensions.
            continue;
        }

        let manifest = match parse_manifest(&manifest_path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Skipping extension in {path:?}: {e}");
                continue;
            }
        };

        let wasm_path = path.join(&manifest.extension.entrypoint);
        if !wasm_path.exists() {
            tracing::warn!(
                "Extension {}: entrypoint {:?} not found, skipping",
                manifest.id,
                manifest.extension.entrypoint
            );
            continue;
        }

        bundles.push(ExtensionBundle {
            manifest,
            dir: path,
            wasm_path,
        });
    }

    bundles
}

/// Returns standard extension directories to scan, in increasing order of
/// specificity (most generic first, most specific last) so neither accidentally
/// shadows the other. The paths cover the three conventional homes for a
/// user's Wasm extensions:
///
/// 1. **Global data dir** (XDG_DATA_HOME on Linux, `~/Library/Application
///    Support` on macOS) under `zerostack/extensions/`. The data dir is the
///    natural home for *stateful* installs — same path the TUI's
///    `src/ui/pickers/list.rs::extension_dirs` reads from.
/// 2. **Global config dir** (XDG_CONFIG_HOME on Linux — typically
///    `~/.config/zerostack/extensions/`), the same XDG location the user
///    already has for running on Linux. On macOS this aliases the data dir,
///    which is fine — XDG-style config dirs are the more familiar surface
///    for Linux users installing a binary from a package manager.
/// 3. **Project-local `<cwd>/.zerostack/extensions/`**, so a repo can ship a
///    workspace-bundled Wasm artifact under version control without a
///    `--extension` flag.
/// 4. **In-tree `tests/extensions/`**, the workspace's own metadata-free
///    Wasm artifacts that show up when developing zerostack itself. We walk
///    up the cwd looking for a `Cargo.toml` whose workspace lists `tests/
///    extensions/...` so this works whether the user is in `crates/gui/`,
///    at the workspace root, or deeper.
///
/// The returned list is in priority order — earlier entries *win* over
/// later entries when two directories share an extension id (see
/// `ExtensionManager::load_all`, which iterates this list and skips ids
/// already seen). Roughly: project-local overrides first, then per-user
/// XDG-style paths, then platform fallback globals; the in-tree
/// `tests/extensions/` directory sits at the bottom because its
/// `extension.toml`s often reference `target/wasm32-wasip2/...` paths
/// that only exist after a developer runs `cargo build --target
/// wasm32-wasip2` — first to load wins, and a checkout sitting at a
/// higher priority would silently shadow the user's manually-installed
/// bundles.
pub fn extension_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // 1. Explicit override via env var — handy when the user wants a
    //    single catch-all directory without splitting across XDG paths.
    //    Inserted *first* so a manual install in `ZS_EXTENSIONS_DIR`
    //    shadows the auto-discovered defaults, mirroring the convention
    //    used by `ZS_DATA_DIR` / `ZS_CONFIG_DIR` in
    //    `src/session/storage.rs`.
    if let Some(raw) = std::env::var_os("ZS_EXTENSIONS_DIR") {
        let expanded = expand_tilde(&raw.to_string_lossy());
        dirs.push(PathBuf::from(expanded));
    }

    // 2. Project-local `.zerostack/extensions/`, sitting next to the
    //    user's `Cargo.toml` (or whatever they're working on). This is
    //    pinned to the cwd at startup — projects that ship their own
    //    pinned extension versions just commit them next to the
    //    workspace.
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".zerostack").join("extensions"));
    }

    // 3. The conventional `~/.config/zerostack/extensions/` slot. On
    //    Linux this *is* `dirs::config_dir()`, but we probe it
    //    explicitly so a user who manually created
    //    `$HOME/.config/zerostack/extensions/` on macOS — where the
    //    `dirs` crate maps `config_dir` to `~/Library/Application
    //    Support` — gets their manually-installed directory scanned
    //    too. Higher priority than the platform globals because users
    //    who bothered to create it usually want it to override the OS
    //    default location.
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".config").join("zerostack").join("extensions"));
    }

    // 4. The `dirs` config-dir XDG slot. On Linux this is the same as
    //    step 3, but we keep it for compatibility with users who set
    //    `$XDG_CONFIG_HOME` to redirect away from `$HOME/.config`,
    //    and on macOS it maps to `~/Library/Application Support`
    //    (the same path as step 5 — harmless duplicate, dedupe covers
    //    it).
    if let Some(config_dir) = dirs::config_dir() {
        dirs.push(config_dir.join("zerostack").join("extensions"));
    }

    // 5. The platform "data" path. On macOS this is
    //    `~/Library/Application Support/zerostack/extensions/`, the
    //    canonical install location the `.app` bundles target; on Linux
    //    this resolves to `~/.local/share/zerostack/extensions/`.
    //    Above the in-tree checkout so a user override anywhere in
    //    steps 1–4 automatically wins, but lower than the explicit
    //    `~/.config/...` probe so users who created that directory
    //    intentionally see their manual install honoured.
    if let Some(data_dir) = dirs::data_dir() {
        dirs.push(data_dir.join("zerostack").join("extensions"));
    }

    // 6. In-tree `tests/extensions/` when developing from inside a
    //    workspace checkout. Sits at the *lowest* priority because its
    //    examples typically point at `target/wasm32-wasip2/...` paths
    //    that exist only after `cargo build --target wasm32-wasip2` —
    //    a developer editing that target dir for testing is fine to
    //    be shadowed by their manual install.
    if let Some(in_tree) = find_in_tree_tests_extensions() {
        dirs.push(in_tree);
    }

    dirs
}

/// Expand a leading `~` or `~/` in a path string to the user's home
/// directory. No-op when the path doesn't start with `~` or `HOME` is
/// unset. `~user/...` is left for the OS to resolve later — we don't
/// enumerate other users for portability.
fn expand_tilde(raw: &str) -> String {
    if !raw.starts_with('~') {
        return raw.to_string();
    }
    let home = match std::env::var_os("HOME") {
        Some(h) => h.to_string_lossy().into_owned(),
        None => return raw.to_string(),
    };
    if raw == "~" {
        return home;
    }
    let rest = &raw[1..];
    if rest.starts_with('/') || rest.is_empty() {
        return format!("{home}{rest}");
    }
    raw.to_string()
}

/// Walk up the current directory looking for a `tests/extensions/` folder that
/// sits beside a `Cargo.toml`. Returns `None` when the cwd isn't inside the
/// zerostack workspace — handy for users who reinstall the binary elsewhere
/// or build with `cargo install`.
fn find_in_tree_tests_extensions() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut cur: std::path::PathBuf = cwd;
    loop {
        let manifest = cur.join("Cargo.toml");
        // The workspace root has both a `Cargo.toml` and a top-level
        // `tests/extensions/` directory. If both are present, we've found
        // the right anchor.
        let in_tree = cur.join("tests").join("extensions");
        if manifest.is_file() && in_tree.is_dir() {
            return Some(in_tree);
        }
        if !cur.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_manifest_minimal() {
        let toml = r#"
id = "test/hello"
name = "Hello Extension"
version = "0.1.0"
schema_version = 1
"#;
        let m: ExtensionManifest = toml::from_str(toml).unwrap();
        assert_eq!(m.id, "test/hello");
        assert_eq!(m.name, "Hello Extension");
        assert_eq!(m.extension.entrypoint, "extension.wasm");
        assert!(!m.capabilities.tools);
    }

    #[test]
    fn test_parse_manifest_full() {
        let toml = r#"
id = "test/full"
name = "Full Extension"
version = "1.0.0"
schema_version = 1
authors = ["Alice"]
description = "A full-featured extension"
repository = "https://github.com/test/full"
license = "MIT"
keywords = ["demo"]

[extension]
entrypoint = "target/wasm32-wasip2/release/extension.wasm"
minimum_zerostack_version = "1.6.0"

[capabilities]
tools = true
commands = true
lifecycle = true
ui = true
"#;
        let m: ExtensionManifest = toml::from_str(toml).unwrap();
        assert_eq!(m.id, "test/full");
        assert_eq!(
            m.extension.entrypoint,
            "target/wasm32-wasip2/release/extension.wasm"
        );
        assert!(m.capabilities.tools);
        assert!(m.capabilities.commands);
        assert!(m.capabilities.lifecycle);
        assert!(!m.capabilities.exec);
        assert!(!m.capabilities.session);
    }
}
