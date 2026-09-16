use std::time::Duration;

use anyhow::{bail, Context, Result};
use maple_proxy::{Config as MapleProxyConfig, Pcr0Environment};
use tokio::net::TcpListener;

use crate::config::Config;

const DEFAULT_BACKEND_URL: &str = "https://enclave.trymaple.ai";

/// Handle to the in-process maple-proxy. All enclave attestation and E2EE
/// lives behind this; maplet only ever speaks loopback OpenAI-compat HTTP.
#[derive(Debug, Clone)]
pub struct ProxyHandle {
    pub port: u16,
}

impl ProxyHandle {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// Bind (ephemeral port by default), spawn the axum server, and wait until
/// /health answers so callers can assume the proxy is usable.
pub async fn start(config: &Config, api_key: &str) -> Result<ProxyHandle> {
    let pcr0 = match config.proxy.pcr0_environment.as_str() {
        "production" => Pcr0Environment::Production,
        "development" => Pcr0Environment::Development,
        other => bail!("invalid proxy.pcr0_environment {other:?} (production|development)"),
    };
    let backend_url = config
        .proxy
        .backend_url
        .clone()
        .unwrap_or_else(|| DEFAULT_BACKEND_URL.to_string());

    let proxy_config = MapleProxyConfig::new("127.0.0.1".into(), config.proxy.port, backend_url)
        .with_pcr0_environment(pcr0)
        .with_api_key(api_key.to_string());

    let app = maple_proxy::create_app(proxy_config);
    let listener = TcpListener::bind(("127.0.0.1", config.proxy.port))
        .await
        .context("binding maple-proxy listener")?;
    let port = listener.local_addr()?.port();
    tracing::info!(port, "embedded maple-proxy listening");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("embedded maple-proxy exited: {e}");
        }
    });

    let handle = ProxyHandle { port };
    crate::breadcrumb::mark("proxy: bound, attesting enclave");
    wait_healthy(&handle).await?;
    crate::breadcrumb::mark("proxy: healthy");
    Ok(handle)
}

async fn wait_healthy(handle: &ProxyHandle) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/health", handle.base_url());
    for _ in 0..50 {
        match client.get(&url).timeout(Duration::from_secs(2)).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    bail!("embedded maple-proxy did not become healthy on {url}")
}
