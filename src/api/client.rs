//! Authenticated HTTP client for Teams APIs
//!
//! Wraps reqwest::Client with automatic token injection and refresh.

use anyhow::{bail, Context, Result};

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::auth::TokenStore;
use crate::config::Config;

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
const DEFAULT_CHAT_SERVICE: &str = "https://amer.ng.msg.teams.microsoft.com";
const CHATSVCAGG: &str = "https://chatsvcagg.teams.microsoft.com";
const DEFAULT_MIDDLE_TIER: &str = "https://teams.microsoft.com/api/mt/amer";

/// Connect deadline for every request the client makes: a dead network
/// fails fast instead of hanging a section load forever.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Default whole-request deadline for API calls. Long transfers opt out
/// via [`TeamsClient::with_timeout`] / [`TeamsClient::graph_get_download`].
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Request observer: `(method, url, status, elapsed_ms)`, status 0 means
/// a transport failure or timeout. Called once per `TeamsClient` request
/// after the response head (or the failure) arrives. A host installing
/// this must not log the raw URL (ids, query) verbatim.
pub type RequestObserver = fn(&str, &str, u16, u64);
static OBSERVER: OnceLock<RequestObserver> = OnceLock::new();

/// Install the process-wide request observer (first call wins).
pub fn set_request_observer(f: RequestObserver) {
    let _ = OBSERVER.set(f);
}

/// Send with an optional whole-request deadline, reporting to the
/// observer.
trait SendObserved {
    fn send_observed(
        self,
        method: &'static str,
        url: &str,
        timeout: Option<Duration>,
    ) -> impl std::future::Future<Output = reqwest::Result<reqwest::Response>> + Send;
}

impl SendObserved for reqwest::RequestBuilder {
    fn send_observed(
        self,
        method: &'static str,
        url: &str,
        timeout: Option<Duration>,
    ) -> impl std::future::Future<Output = reqwest::Result<reqwest::Response>> + Send {
        let rb = match timeout {
            Some(t) => self.timeout(t),
            None => self,
        };
        let url = url.to_string();
        async move {
            let t0 = Instant::now();
            let r = rb.send().await;
            if let Some(f) = OBSERVER.get() {
                let status = r.as_ref().map(|x| x.status().as_u16()).unwrap_or(0);
                f(method, &url, status, t0.elapsed().as_millis() as u64);
            }
            r
        }
    }
}

/// Authenticated client that handles both Graph (AAD) and Teams (Skype) APIs.
pub struct TeamsClient {
    http: reqwest::Client,
    config: Config,
    /// Whole-request deadline for API calls (`None` = connect only).
    timeout: Option<Duration>,
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

        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Ok(Self {
            http,
            config,
            timeout: Some(DEFAULT_REQUEST_TIMEOUT),
        })
    }

    /// Per-client deadline override: `None` = connect timeout only (long
    /// transfers), `Some(d)` = whole-request deadline `d`.
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// [`graph_get`](Self::graph_get) without the whole-request deadline:
    /// content downloads stream for as long as they need.
    pub async fn graph_get_download(&self, path: &str) -> Result<reqwest::Response> {
        let token = self.graph_token()?;
        let url = format!("{}{}", GRAPH_BASE, path);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send_observed("GET", &url, None)
            .await
            .with_context(|| format!("Graph GET {} failed", url))?;
        check_response(resp, &url).await
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
            .send_observed("GET", &url, self.timeout)
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
            .send_observed("POST", &url, self.timeout)
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
            .send_observed("GET", url, self.timeout)
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
            .send_observed("POST", url, self.timeout)
            .await
            .with_context(|| format!("Teams POST {} failed", url))?;

        check_response(resp, url).await
    }

    /// Apps-platform middle tier base (`region_gtms.middleTier`),
    /// falling back to the default region.
    pub fn middle_tier_url(&self) -> String {
        self.config
            .get_region_gtms()
            .and_then(|v| v.get("middleTier").and_then(|s| s.as_str()).map(String::from))
            .unwrap_or_else(|| DEFAULT_MIDDLE_TIER.to_string())
    }

    /// Middle-tier auth: the Teams AAD token as Bearer plus X-Skypetoken
    /// (the shell's `mtAuthWithSkypeXTokenResource`).
    fn mt_request(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let aad = self
            .config
            .get_access_token()
            .context("No AAD token. Run 'teams-cli login' first.")?;
        if aad.is_expired() {
            bail!("AAD token expired. Run 'teams-cli login'.");
        }
        Ok(req
            .bearer_auth(&aad.token)
            .header("X-Skypetoken", self.skype_token()?)
            .header("x-ms-client-type", "desktop"))
    }

    /// GET against the apps-platform middle tier.
    pub async fn mt_get(&self, url: &str) -> Result<reqwest::Response> {
        tracing::debug!("MT GET {}", url);
        let resp = self
            .mt_request(self.http.get(url))?
            .send_observed("GET", url, self.timeout)
            .await
            .with_context(|| format!("MT GET {} failed", url))?;
        check_response(resp, url).await
    }

    /// POST (JSON) against the apps-platform middle tier (read-only
    /// queries: entitlement views, batched definitions).
    pub async fn mt_post(&self, url: &str, body: &serde_json::Value) -> Result<reqwest::Response> {
        tracing::debug!("MT POST {}", url);
        let resp = self
            .mt_request(self.http.post(url))?
            .json(body)
            .send_observed("POST", url, self.timeout)
            .await
            .with_context(|| format!("MT POST {} failed", url))?;
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
            .send_observed("GET", url, self.timeout)
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
            .send_observed("GET", url, self.timeout)
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
            .send_observed("POST", url, self.timeout)
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
