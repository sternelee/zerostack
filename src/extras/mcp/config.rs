use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerConfig {
    Command {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        /// Timeout for the connection/handshake with the server, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_timeout_secs: Option<u64>,
        /// Timeout for individual tool calls, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_timeout_secs: Option<u64>,
        /// Number of times a failed connection attempt is retried.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_retries: Option<u32>,
    },
    Url {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<OAuthConfig>,
        /// Timeout for the connection/handshake with the server, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_timeout_secs: Option<u64>,
        /// Timeout for individual tool calls, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_timeout_secs: Option<u64>,
        /// Number of times a failed connection attempt is retried.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_retries: Option<u32>,
    },
}

pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;
pub const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 20;
pub const DEFAULT_CONNECT_RETRIES: u32 = 1;

impl McpServerConfig {
    fn timeout_fields(&self) -> (Option<u64>, Option<u64>, Option<u32>) {
        match self {
            McpServerConfig::Command {
                connect_timeout_secs,
                tool_timeout_secs,
                connect_retries,
                ..
            }
            | McpServerConfig::Url {
                connect_timeout_secs,
                tool_timeout_secs,
                connect_retries,
                ..
            } => (*connect_timeout_secs, *tool_timeout_secs, *connect_retries),
        }
    }

    /// Timeout for establishing the connection and MCP handshake.
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(
            self.timeout_fields()
                .0
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS),
        )
    }

    /// Timeout for individual tool calls and tool listing.
    pub fn tool_timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_fields().1.unwrap_or(DEFAULT_TOOL_TIMEOUT_SECS))
    }

    /// Number of times a failed connection attempt is retried.
    pub fn connect_retries(&self) -> u32 {
        self.timeout_fields().2.unwrap_or(DEFAULT_CONNECT_RETRIES)
    }
}

/// OAuth settings for a URL-based MCP server.
///
/// Accepts either a bare `true` (enable with all defaults: dynamic client
/// registration, no extra scopes) or an object with explicit fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OAuthConfig {
    Enabled(bool),
    Settings(OAuthSettings),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthSettings {
    /// OAuth scopes to request. Empty means none are requested explicitly.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Pre-registered client id. When absent, dynamic client registration is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Loopback port for the redirect URI. Defaults to [`DEFAULT_REDIRECT_PORT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_port: Option<u16>,
}

pub const DEFAULT_REDIRECT_PORT: u16 = 8970;

impl OAuthConfig {
    /// Returns the resolved settings if OAuth is enabled, or `None` if disabled.
    pub fn settings(&self) -> Option<OAuthSettings> {
        match self {
            OAuthConfig::Enabled(false) => None,
            OAuthConfig::Enabled(true) => Some(OAuthSettings::default()),
            OAuthConfig::Settings(s) => Some(s.clone()),
        }
    }
}

impl OAuthSettings {
    pub fn redirect_port(&self) -> u16 {
        self.redirect_port.unwrap_or(DEFAULT_REDIRECT_PORT)
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.redirect_port())
    }
}

/// Whether a `--custom-mcp*` server name is usable as an `mcp__<server>__<tool>`
/// allowlist key: non-empty, no whitespace, no `=` or `__` separators.
pub fn is_valid_server_name(name: &str) -> bool {
    !name.is_empty() && !name.chars().any(|c| c.is_whitespace() || c == '=') && !name.contains("__")
}

/// Parse one `--custom-mcp NAME=command args...` entry into a stdio server
/// config. Shell-style quoting is honored (`shell-words`), so
/// `NAME=npx -y "pkg name"` splits into `["-y", "pkg name"]`.
pub fn parse_custom_mcp(entry: &str) -> anyhow::Result<(String, McpServerConfig)> {
    let (name, spec) = entry
        .split_once('=')
        .map(|(n, s)| (n.trim(), s.trim()))
        .ok_or_else(|| anyhow::anyhow!("--custom-mcp '{entry}': expected NAME=COMMAND ..."))?;
    if !is_valid_server_name(name) {
        anyhow::bail!("--custom-mcp '{entry}': invalid server name '{name}'");
    }
    let words = shell_words::split(spec)
        .map_err(|e| anyhow::anyhow!("--custom-mcp '{entry}': cannot parse command: {e}"))?;
    let (command, args) = words.split_first().ok_or_else(|| {
        anyhow::anyhow!("--custom-mcp '{entry}': command is empty (expected NAME=COMMAND ...)")
    })?;
    Ok((
        name.to_string(),
        McpServerConfig::Command {
            command: command.clone(),
            args: args.to_vec(),
            env: HashMap::new(),
            connect_timeout_secs: None,
            tool_timeout_secs: None,
            connect_retries: None,
        },
    ))
}

/// Parse one `--custom-mcp-http NAME=https://...` entry into an HTTP server
/// config. Full options (headers, oauth, timeouts) stay in the config file.
pub fn parse_custom_mcp_http(entry: &str) -> anyhow::Result<(String, McpServerConfig)> {
    let (name, url) = entry
        .split_once('=')
        .map(|(n, s)| (n.trim(), s.trim()))
        .ok_or_else(|| anyhow::anyhow!("--custom-mcp-http '{entry}': expected NAME=URL"))?;
    if !is_valid_server_name(name) {
        anyhow::bail!("--custom-mcp-http '{entry}': invalid server name '{name}'");
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        anyhow::bail!("--custom-mcp-http '{entry}': URL must start with http:// or https://");
    }
    Ok((
        name.to_string(),
        McpServerConfig::Url {
            url: url.to_string(),
            headers: HashMap::new(),
            oauth: None,
            connect_timeout_secs: None,
            tool_timeout_secs: None,
            connect_retries: None,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_server_names() {
        assert!(is_valid_server_name("fs"));
        assert!(is_valid_server_name("my-server_1"));
        assert!(!is_valid_server_name(""));
        assert!(!is_valid_server_name("has space"));
        assert!(!is_valid_server_name("has=eq"));
        assert!(!is_valid_server_name("has__sep"));
    }

    #[test]
    fn parse_stdio_simple() {
        let (name, cfg) = parse_custom_mcp("fs=npx -y server-fs .").unwrap();
        assert_eq!(name, "fs");
        let McpServerConfig::Command { command, args, .. } = &cfg else {
            panic!("expected Command, got {cfg:?}");
        };
        assert_eq!(command, "npx");
        assert_eq!(args, &vec!["-y", "server-fs", "."]);
    }

    #[test]
    fn parse_stdio_honors_quotes() {
        let (_, cfg) = parse_custom_mcp("s=cmd \"two words\"").unwrap();
        let McpServerConfig::Command { args, .. } = &cfg else {
            panic!("expected Command, got {cfg:?}");
        };
        assert_eq!(args, &vec!["two words"]);
    }

    #[test]
    fn parse_stdio_errors() {
        assert!(parse_custom_mcp("no-equals").is_err());
        assert!(parse_custom_mcp("fs=").is_err());
        assert!(parse_custom_mcp("bad name=cmd").is_err());
        assert!(parse_custom_mcp("bad__name=cmd").is_err());
    }

    #[test]
    fn parse_http_ok() {
        let (name, cfg) = parse_custom_mcp_http("exa=https://mcp.exa.ai/mcp").unwrap();
        assert_eq!(name, "exa");
        assert!(matches!(cfg, McpServerConfig::Url { .. }));
    }

    #[test]
    fn parse_http_errors() {
        assert!(parse_custom_mcp_http("no-equals").is_err());
        assert!(parse_custom_mcp_http("s=ftp://x").is_err());
        assert!(parse_custom_mcp_http("s=npx foo").is_err());
    }
}
