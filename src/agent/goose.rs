use std::path::PathBuf;

use agent_client_protocol::{AcpAgent, AcpAgentConfig};
use anyhow::{bail, Result};

use crate::config::{Config, McpServerDecl};

/// Everything needed to spawn `goose acp` pointed at the embedded proxy.
#[derive(Clone)]
pub struct GooseSpawn {
    pub binary: PathBuf,
    pub model: String,
    pub proxy_base_url: String,
    pub api_key: String,
    pub builtins: Vec<String>,
    pub mcp_servers: Vec<McpServerDecl>,
    /// Ask before every tool use. goose's own default (smart_approve) has been
    /// seen approving a shell `rm` on its own, so maplet sets the mode
    /// explicitly rather than inheriting whatever the user's goose config says.
    pub approve: bool,
}

impl GooseSpawn {
    pub fn from_config(config: &Config, proxy_base_url: String, api_key: String) -> Result<Self> {
        let binary = locate_goose(config)?;
        Ok(Self {
            binary,
            model: config.agent.model.clone(),
            proxy_base_url,
            api_key,
            builtins: config.agent.builtins.clone(),
            mcp_servers: config.agent.mcp_servers.clone(),
            approve: true,
        })
    }

    pub fn agent(&self) -> AcpAgent {
        let mut config = AcpAgentConfig::new(&self.binary).arg("acp");
        if !self.builtins.is_empty() {
            config = config.arg("--with-builtin").arg(self.builtins.join(","));
        }
        // Zed's pattern: provider config via env, scoped to this subprocess.
        config = config
            .env("GOOSE_MODE", if self.approve { "approve" } else { "auto" })
            .env("GOOSE_PROVIDER", "openai")
            .env("GOOSE_MODEL", &self.model)
            .env("OPENAI_HOST", &self.proxy_base_url)
            .env("OPENAI_API_KEY", &self.api_key);
        AcpAgent::new(config)
            .with_debug(|line, dir| tracing::debug!(target: "goose", "{dir:?}: {line}"))
    }
}

fn locate_goose(config: &Config) -> Result<PathBuf> {
    if let Some(path) = &config.agent.goose_path {
        let path = crate::config::expand_tilde(path.clone());
        if path.exists() {
            return Ok(path);
        }
        bail!("agent.goose_path {} does not exist", path.display());
    }
    if let Some(home) = dirs::home_dir() {
        let local = home.join(".local/bin/goose");
        if local.exists() {
            return Ok(local);
        }
    }
    if let Ok(path) = which("goose") {
        return Ok(path);
    }
    bail!(
        "goose not found. Install it: curl -fsSL \
         https://github.com/aaif-goose/goose/releases/download/stable/download_cli.sh | \
         CONFIGURE=false bash"
    )
}

fn which(name: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("{name} not on PATH")
}
