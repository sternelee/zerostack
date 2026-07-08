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
            tools: true,
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
        true
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

/// Returns standard extension directories to scan.
pub fn extension_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Global extensions.
    if let Some(data_dir) = dirs::data_dir() {
        dirs.push(data_dir.join("zerostack").join("extensions"));
    }

    // Project-local extensions.
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".zerostack").join("extensions"));
    }

    dirs
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
