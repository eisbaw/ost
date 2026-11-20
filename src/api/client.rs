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

    /// PATCH request to Microsoft Graph API (Bearer [REDACTED] with Graph token).
    /// OstMac whiteboard: Fluid board file rename via PATCH.
    pub async fn graph_patch(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response> {
        let token = self.graph_token()?;
        let url = format!("{}{}", GRAPH_BASE, path);
        tracing::debug!("Graph PATCH {}", url);

        let resp = self
            .http
            .patch(&url)
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("Graph PATCH {} failed", url))?;

        check_response(resp, &url).await
    }

    /// DELETE request to Microsoft Graph API (Bearer [REDACTED] with Graph token).
    /// OstMac whiteboard: board file delete via DELETE.
    pub async fn graph_delete(&self, path: &str) -> Result<reqwest::Response> {
        let token = self.graph_token()?;
        let url = format!("{}{}", GRAPH_BASE, path);
        tracing::debug!("Graph DELETE {}", url);

        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .with_context(|| format!("Graph DELETE {} failed", url))?;

        check_response(resp, &url).await
    }

    /// Whiteboard legacy-REST root (`.../api/`). Production is always
    /// the Microsoft host; unit tests may point it at a local fake
    /// (`whiteboardBase` key, same seam as `graphBase`).
    /// WHITEBOARD-BUILD.
    pub(crate) fn whiteboard_base(&self) -> String {
        #[cfg(test)]
        if let Some(b) = self
            .config
            .get_region_gtms()
            .and_then(|v| v.get("whiteboardBase").and_then(|s| s.as_str()).map(String::from))
        {
            return b;
        }
        super::whiteboard::WHITEBOARD_BASE.to_string()
    }

    /// Bearer token attempted against the whiteboard + SPO surfaces:
    /// the Teams AAD token. The mapped scope is the whiteboard/SPO
    /// resource (`.../.default`, `{site}/Files.ReadWrite.All`), which
    /// this token may lack — the server's 401/403 then becomes the
    /// Swift live-gate reason, never a silent failure. Phase-2 P1/P4
    /// (trial tenant) decides whether a separate grant is needed.
    /// WHITEBOARD-BUILD.
    fn whiteboard_token(&self) -> Result<String> {
        let aad = self
            .config
            .get_access_token()
            .context("No AAD token. Run 'teams-cli login' first.")?;
        if aad.is_expired() {
            bail!("AAD token expired. Run 'teams-cli login'.");
        }
        Ok(aad.token)
    }

    /// One JSON request against the whiteboard legacy REST base
    /// (`Authorization: Bearer`, optional JSON body). `path` is a
    /// `whiteboard::wb_*_path` value (relative, may carry a query).
    /// WHITEBOARD-BUILD.
    pub async fn wb_send_json(
        &self,
        verb: &'static str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response> {
        let method = reqwest::Method::from_bytes(verb.as_bytes())
            .with_context(|| format!("bad method {}", verb))?;
        let token = self.whiteboard_token()?;
        let url = format!("{}{}", self.whiteboard_base(), path);
        tracing::debug!("WB {} {}", verb, url);
        let mut req = self.http.request(method, &url).bearer_auth(&token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("WB {} {} failed", verb, url))?;
        check_response(resp, &url).await
    }

    /// GET against the whiteboard legacy REST base. WHITEBOARD-BUILD.
    pub async fn wb_get(&self, path: &str) -> Result<reqwest::Response> {
        self.wb_send_json("GET", path, None).await
    }

    /// POST (JSON) against the whiteboard legacy REST base.
    /// WHITEBOARD-BUILD.
    pub async fn wb_post(&self, path: &str, body: &serde_json::Value) -> Result<reqwest::Response> {
        self.wb_send_json("POST", path, Some(body)).await
    }

    /// PUT (JSON) against the whiteboard legacy REST base.
    /// WHITEBOARD-BUILD.
    pub async fn wb_put(&self, path: &str, body: Option<&serde_json::Value>) -> Result<reqwest::Response> {
        self.wb_send_json("PUT", path, body).await
    }

    /// PATCH (JSON) against the whiteboard legacy REST base.
    /// WHITEBOARD-BUILD.
    pub async fn wb_patch(&self, path: &str, body: &serde_json::Value) -> Result<reqwest::Response> {
        self.wb_send_json("PATCH", path, Some(body)).await
    }

    /// DELETE against the whiteboard legacy REST base.
    /// WHITEBOARD-BUILD.
    pub async fn wb_delete(&self, path: &str) -> Result<reqwest::Response> {
        self.wb_send_json("DELETE", path, None).await
    }

    /// GET one legacy content page with the read-version headers
    /// (`X-MinimumReadVersion`, `X-MaximumReadVersion`,
    /// `X-WriteVersion`, verbatim from `getBoardContents`).
    /// WHITEBOARD-BUILD.
    pub async fn wb_get_versioned(
        &self,
        path: &str,
        min_read: &str,
        max_read: &str,
        write: &str,
    ) -> Result<reqwest::Response> {
        let token = self.whiteboard_token()?;
        let url = format!("{}{}", self.whiteboard_base(), path);
        tracing::debug!("WB GET {}", url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .header("X-MinimumReadVersion", min_read.to_string())
            .header("X-MaximumReadVersion", max_read.to_string())
            .header("X-WriteVersion", write.to_string())
            .send()
            .await
            .with_context(|| format!("WB GET {} failed", url))?;
        check_response(resp, &url).await
    }

    /// One SharePoint/ODSP request (absolute `url`, Bearer AAD attempt
    /// + `X-Partner-Id: WB`, verbatim from
    /// `SharepointServiceInternal`). Optional JSON body; raw bytes go
    /// through [`Self::spo_post_bytes`]. WHITEBOARD-BUILD.
    pub async fn spo_send_json(
        &self,
        verb: &'static str,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response> {
        let method = reqwest::Method::from_bytes(verb.as_bytes())
            .with_context(|| format!("bad method {}", verb))?;
        let token = self.whiteboard_token()?;
        tracing::debug!("SPO {} {}", verb, url);
        let mut req = self
            .http
            .request(method, url)
            .bearer_auth(&token)
            .header("X-Partner-Id", "WB");
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("SPO {} {} failed", verb, url))?;
        check_response(resp, url).await
    }

    /// GET one absolute SharePoint/ODSP URL. WHITEBOARD-BUILD.
    pub async fn spo_get(&self, url: &str) -> Result<reqwest::Response> {
        self.spo_send_json("GET", url, None).await
    }

    /// DELETE one absolute SharePoint/ODSP URL. WHITEBOARD-BUILD.
    pub async fn spo_delete(&self, url: &str) -> Result<reqwest::Response> {
        self.spo_send_json("DELETE", url, None).await
    }

    /// POST raw bytes (the hand-built joinSession/snapshot multipart)
    /// to one absolute ODSP URL with an explicit content type.
    /// WHITEBOARD-BUILD.
    pub async fn spo_post_bytes(
        &self,
        url: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<reqwest::Response> {
        let token = self.whiteboard_token()?;
        tracing::debug!("SPO POST {} ({} bytes)", url, bytes.len());
        let resp = self
            .http
            .post(url)
            .bearer_auth(&token)
            .header("X-Partner-Id", "WB")
            .header(reqwest::header::CONTENT_TYPE, content_type.to_string())
            .body(bytes)
            .send()
            .await
            .with_context(|| format!("SPO POST {} failed", url))?;
        check_response(resp, url).await
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
