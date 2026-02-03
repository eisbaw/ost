//! Authenticated HTTP client for Teams APIs
//!
//! Wraps reqwest::Client with automatic token injection and refresh.

use anyhow::{bail, Context, Result};

use crate::auth::TokenStore;
use crate::config::Config;

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
const DEFAULT_CHAT_SERVICE: &str = "https://amer.ng.msg.teams.microsoft.com";
const CHATSVCAGG: &str = "https://chatsvcagg.teams.microsoft.com";

/// Authenticated client that handles both Graph (AAD) and Teams (Skype) APIs.
pub struct TeamsClient {
    http: reqwest::Client,
    config: Config,
}

impl TeamsClient {
    /// Load config and build client. Attempts token refresh if AAD token is expired.
    pub async fn new() -> Result<Self> {
        let mut config = Config::load()?;

        // Auto-refresh if any token is expired but refresh token exists
        let needs_refresh = config.get_access_token().map_or(true, |t| t.is_expired())
            || config.get_graph_token().map_or(true, |t| t.is_expired());
        if needs_refresh {
            if config.get_refresh_token().is_some() {
                tracing::info!("Tokens missing or expired, refreshing...");
                match crate::auth::oauth::refresh().await {
                    Ok(true) => {
                        config = Config::load()?;
                        tracing::info!("Token refreshed");
                    }
                    Ok(false) => {
                        bail!("No refresh token available. Run 'teams-cli login'.");
                    }
                    Err(e) => {
                        bail!("Token refresh failed: {:#}. Run 'teams-cli login'.", e);
                    }
                }
            } else {
                bail!("Token expired and no refresh token. Run 'teams-cli login'.");
            }
        }

        Ok(Self {
            http: reqwest::Client::new(),
            config,
        })
    }

    fn graph_token(&self) -> Result<String> {
        let token = self
            .config
            .get_graph_token()
            .context("No Graph token. Run 'teams-cli login' first.")?;
        if token.is_expired() {
            bail!("Graph token expired. Run 'teams-cli login'.");
        }
        Ok(token.token)
    }

    fn skype_token(&self) -> Result<String> {
        let token = self
            .config
            .get_skype_token()
            .context("No Skype token. Run 'teams-cli login' first.")?;
        if token.is_expired() {
            bail!("Skype token expired. Run 'teams-cli login'.");
        }
        Ok(token.token)
    }

    /// GET request to Microsoft Graph API (bearer auth with Graph token).
    pub async fn graph_get(&self, path: &str) -> Result<reqwest::Response> {
        let token = self.graph_token()?;
        let url = format!("{}{}", GRAPH_BASE, path);
        tracing::debug!("Graph GET {}", url);

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .with_context(|| format!("Graph GET {} failed", url))?;

        check_response(resp, &url).await
    }

    /// POST request to Microsoft Graph API (bearer auth with Graph token).
    pub async fn graph_post(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response> {
        let token = self.graph_token()?;
        let url = format!("{}{}", GRAPH_BASE, path);
        tracing::debug!("Graph POST {}", url);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("Graph POST {} failed", url))?;

        check_response(resp, &url).await
    }

    /// GET request to Teams/Skype API (X-SkypeToken header).
    pub async fn teams_get(&self, url: &str) -> Result<reqwest::Response> {
        let token = self.skype_token()?;
        tracing::debug!("Teams GET {}", url);

        let resp = self
            .http
            .get(url)
            .header("X-SkypeToken", &token)
            .send()
            .await
            .with_context(|| format!("Teams GET {} failed", url))?;

        check_response(resp, url).await
    }

    /// POST request to Teams/Skype API (X-SkypeToken header).
    pub async fn teams_post(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response> {
        let token = self.skype_token()?;
        tracing::debug!("Teams POST {}", url);

        let resp = self
            .http
            .post(url)
            .header("X-SkypeToken", &token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("Teams POST {} failed", url))?;

        check_response(resp, url).await
    }

    /// Chat service base URL from region_gtms, falling back to default.
    pub fn chat_service_url(&self) -> String {
        self.config
            .get_region_gtms()
            .and_then(|v| {
                v.get("chatService")
                    .and_then(|s| s.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| DEFAULT_CHAT_SERVICE.to_string())
    }

    /// Chat service aggregator URL from region_gtms, falling back to default.
    pub fn chatsvcagg_url(&self) -> String {
        self.config
            .get_region_gtms()
            .and_then(|v| {
                v.get("chatServiceAggregator")
                    .and_then(|s| s.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| CHATSVCAGG.to_string())
    }

    /// GET using `Authorization: Bearer {skype_token}` with client version header (CSA/AFD endpoint).
    pub async fn csa_get(&self, url: &str) -> Result<reqwest::Response> {
        let token = self.skype_token()?;
        tracing::debug!("CSA GET {}", url);

        let resp = self
            .http
            .get(url)
            .bearer_auth(&token)
            .header("x-ms-client-version", "1416/1.0.0.2024050301")
            .send()
            .await
            .with_context(|| format!("CSA GET {} failed", url))?;

        check_response(resp, url).await
    }

    /// GET using `Authentication: skypetoken=...` header (native chat API).
    pub async fn chat_get(&self, url: &str) -> Result<reqwest::Response> {
        let token = self.skype_token()?;
        tracing::debug!("Chat GET {}", url);

        let resp = self
            .http
            .get(url)
            .header("Authentication", format!("skypetoken={}", token))
            .send()
            .await
            .with_context(|| format!("Chat GET {} failed", url))?;

        check_response(resp, url).await
    }

    /// POST using `Authentication: skypetoken=...` header (native chat API).
    pub async fn chat_post(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response> {
        let token = self.skype_token()?;
        tracing::debug!("Chat POST {}", url);

        let resp = self
            .http
            .post(url)
            .header("Authentication", format!("skypetoken={}", token))
            .json(body)
            .send()
            .await
            .with_context(|| format!("Chat POST {} failed", url))?;

        check_response(resp, url).await
    }
}

/// Check HTTP response status code and return a clear error on failure.
async fn check_response(resp: reqwest::Response, url: &str) -> Result<reqwest::Response> {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!(
            "401 Unauthorized for {}. Token may be invalid -- run 'teams-cli login'.",
            url
        );
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("HTTP {} for {}: {}", status.as_u16(), url, body);
    }
    Ok(resp)
}
