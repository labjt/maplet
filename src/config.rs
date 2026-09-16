use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Default model: tool-calling capable, and what Maple's own agent mode uses.
/// Maple retires model ids periodically, so this is a starting point rather
/// than a guarantee — `maplet models` lists what the account can actually use.
pub const DEFAULT_MODEL: &str = "glm-5-3";

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub chat: ChatConfig,
    #[serde(default)]
    pub agent: AgentConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    /// Path to a file containing the Maple API key (should be chmod 600).
    pub key_file: Option<PathBuf>,
    /// Inline key. Discouraged; key_file or $MAPLE_API_KEY preferred.
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProxyConfig {
    /// 0 = ephemeral port (default). Fixed port only if you want to share the
    /// proxy with other local tools.
    pub port: u16,
    pub pcr0_environment: String,
    pub backend_url: Option<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self { port: 0, pcr0_environment: "production".into(), backend_url: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ChatConfig {
    pub model: String,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self { model: DEFAULT_MODEL.into() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgentConfig {
    pub model: String,
    pub goose_path: Option<PathBuf>,
    /// Default working directory for agent sessions ("." = where maplet runs).
    pub cwd: PathBuf,
    /// Goose builtin extensions to enable (`goose acp --with-builtin ...`).
    /// "developer" provides shell + file tools; empty disables builtins.
    pub builtins: Vec<String>,
    #[serde(rename = "mcp_servers")]
    pub mcp_servers: Vec<McpServerDecl>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.into(),
            goose_path: None,
            cwd: PathBuf::from("."),
            builtins: vec!["developer".into()],
            mcp_servers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerDecl {
    pub name: String,
    /// "stdio" or "http" (goose supports streamable HTTP, not SSE).
    #[serde(default = "default_transport")]
    pub transport: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

fn default_transport() -> String {
    "stdio".into()
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("maple-tui")
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("maple-tui")
}

pub fn state_dir() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| PathBuf::from(".")).join("maple-tui")
}

fn default_key_file() -> PathBuf {
    config_dir().join("api_key")
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_dir().join("config.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// Resolution order: $MAPLE_API_KEY > key_file > inline api_key.
    pub fn resolve_api_key(&self) -> Result<String> {
        if let Ok(key) = std::env::var("MAPLE_API_KEY") {
            let key = key.trim().to_string();
            if !key.is_empty() {
                return Ok(key);
            }
        }
        let key_file = self.auth.key_file.clone().map(expand_tilde).unwrap_or_else(default_key_file);
        if key_file.exists() {
            let meta = fs::metadata(&key_file)?;
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                tracing::warn!(
                    "{} is group/world readable (mode {:o}); consider chmod 600",
                    key_file.display(),
                    mode
                );
            }
            let key = fs::read_to_string(&key_file)
                .with_context(|| format!("reading {}", key_file.display()))?
                .trim()
                .to_string();
            if !key.is_empty() {
                return Ok(key);
            }
        }
        if let Some(key) = &self.auth.api_key {
            if !key.trim().is_empty() {
                return Ok(key.trim().to_string());
            }
        }
        bail!(
            "no Maple API key found. Set $MAPLE_API_KEY, or write your key to {} (chmod 600). \
             Keys are created at https://trymaple.ai (Pro/Team/Max plans).",
            key_file.display()
        )
    }
}

pub fn expand_tilde(p: PathBuf) -> PathBuf {
    if let Ok(stripped) = p.strip_prefix("~") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    p
}
