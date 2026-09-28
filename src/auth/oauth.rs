//! OAuth2 device code flow for Azure AD, plus Skype token exchange

use anyhow::{Context, Result};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, DeviceAuthorizationUrl, RefreshToken, Scope,
    StandardDeviceAuthorizationResponse, TokenResponse, TokenUrl,
};

use super::skype::exchange_skype_token;
use super::{AuthConfig, TokenStore};
use crate::config::Config;

/// Build the OAuth2 client from an AuthConfig
fn build_client(auth_config: &AuthConfig) -> Result<BasicClient> {
    let auth_url = AuthUrl::new(format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/authorize",
        auth_config.tenant
    ))?;
    let token_url = TokenUrl::new(format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        auth_config.tenant
    ))?;
    let device_url = DeviceAuthorizationUrl::new(format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/devicecode",
        auth_config.tenant
    ))?;

    Ok(BasicClient::new(
        ClientId::new(auth_config.client_id.to_string()),
        None,
        auth_url,
        Some(token_url),
    )
    .set_device_authorization_url(device_url))
}

/// Acquire an IC3 token by exchanging the refresh token with IC3 scope.
async fn acquire_ic3_token(
    client: &BasicClient,
    refresh_token_str: &str,
) -> Result<(String, Option<u64>)> {
    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token_str.to_string()))
        .add_scope(Scope::new(
            "https://ic3.teams.office.com/.default".to_string(),
        ))
        .add_scope(Scope::new("offline_access".to_string()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .context("Failed to acquire IC3 token")?;

    Ok((
        token_response.access_token().secret().to_string(),
        token_response.expires_in().map(|d| d.as_secs()),
    ))
}

/// Acquire a recorder service AAD token (audience: 4580fd1d-e5a3-4f56-9ad1-aab0e3bf8f76).
async fn acquire_recorder_token(
    client: &BasicClient,
    refresh_token_str: &str,
) -> Result<(String, Option<u64>)> {
    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token_str.to_string()))
        .add_scope(Scope::new(
            "4580fd1d-e5a3-4f56-9ad1-aab0e3bf8f76/.default".to_string(),
        ))
        .add_scope(Scope::new("offline_access".to_string()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .context("Failed to acquire recorder token")?;

    Ok((
        token_response.access_token().secret().to_string(),
        token_response.expires_in().map(|d| d.as_secs()),
    ))
}

/// Acquire a Graph API token by exchanging the refresh token with Graph scope.
async fn acquire_graph_token(
    client: &BasicClient,
    refresh_token_str: &str,
) -> Result<(String, Option<u64>)> {
    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token_str.to_string()))
        .add_scope(Scope::new(
            "https://graph.microsoft.com/.default".to_string(),
        ))
        .add_scope(Scope::new("offline_access".to_string()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .context("Failed to acquire Graph token")?;

    Ok((
        token_response.access_token().secret().to_string(),
        token_response.expires_in().map(|d| d.as_secs()),
    ))
}

/// Refresh the AAD access token using a stored refresh_token, then
/// re-exchange for a Skype token. Returns Ok(true) if refresh succeeded.
pub async fn refresh() -> Result<bool> {
    let mut config = Config::load()?;
    let refresh_token_str = match config.get_refresh_token() {
        Some(rt) => rt,
        None => return Ok(false),
    };

    let auth_config = AuthConfig::default();
    let client = build_client(&auth_config)?;

    tracing::info!("Refreshing AAD token...");

    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token_str))
        .add_scope(Scope::new(
            "https://api.spaces.skype.com/.default".to_string(),
        ))
        .add_scope(Scope::new("offline_access".to_string()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .context("Failed to refresh AAD token")?;

    config.set_access_token(
        token_response.access_token().secret().to_string(),
        token_response.expires_in().map(|d| d.as_secs()),
    );

    if let Some(new_rt) = token_response.refresh_token() {
        config.set_refresh_token(new_rt.secret().to_string());
    }

    // Exchange for Skype token
    let aad_token = token_response.access_token().secret();
    match exchange_skype_token(aad_token, false).await {
        Ok((skype_tok, expires_in, region_gtms)) => {
            config.set_skype_token(skype_tok, expires_in);
            if let Some(gtms) = region_gtms {
                config.set_region_gtms(gtms);
            }
            tracing::info!("Skype token refreshed");
        }
        Err(e) => {
            tracing::warn!("Skype token exchange failed during refresh: {:#}", e);
        }
    }

    // Acquire Graph API token (separate audience)
    let rt_for_graph = config.get_refresh_token().unwrap_or_default();
    if !rt_for_graph.is_empty() {
        match acquire_graph_token(&client, &rt_for_graph).await {
            Ok((graph_tok, expires_in)) => {
                config.set_graph_token(graph_tok, expires_in);
                tracing::info!("Graph token acquired");
            }
            Err(e) => {
                tracing::warn!("Graph token acquisition failed: {:#}", e);
            }
        }
    }

    // Acquire IC3 token (for Trouter WebSocket auth)
    let rt_for_ic3 = config.get_refresh_token().unwrap_or_default();
    if !rt_for_ic3.is_empty() {
        match acquire_ic3_token(&client, &rt_for_ic3).await {
            Ok((ic3_tok, expires_in)) => {
                config.set_ic3_token(ic3_tok, expires_in);
                tracing::info!("IC3 token acquired");
            }
            Err(e) => {
                tracing::warn!("IC3 token acquisition failed: {:#}", e);
            }
        }
    }

    // Acquire recorder service token (for call recording)
    let rt_for_recorder = config.get_refresh_token().unwrap_or_default();
    if !rt_for_recorder.is_empty() {
        match acquire_recorder_token(&client, &rt_for_recorder).await {
            Ok((rec_tok, expires_in)) => {
                config.set_recorder_token(rec_tok, expires_in);
                tracing::info!("Recorder token acquired");
            }
            Err(e) => {
                tracing::warn!("Recorder token acquisition failed: {:#}", e);
            }
        }
    }

    config.save()?;
    tracing::info!("Token refresh complete");
    Ok(true)
}

/// Perform OAuth2 login flow
pub async fn login(force: bool) -> Result<()> {
    {
        let config = Config::load()?;

        // Check for existing valid token
        if !force {
            if let Some(token) = config.get_access_token() {
                if !token.is_expired() {
                    // Check if any derived tokens are missing; if so, refresh to acquire them
                    let missing_tokens =
                        config.get_recorder_token().is_none() || config.get_ic3_token().is_none();
                    if missing_tokens && config.get_refresh_token().is_some() {
                        tracing::info!(
                            "AAD token valid but some derived tokens missing, refreshing..."
                        );
                        if let Ok(true) = refresh().await {
                            println!("Tokens refreshed (acquired missing derived tokens).");
                            return Ok(());
                        }
                    }
                    println!(
                        "Already logged in (AAD token valid). Use --force to re-authenticate."
                    );
                    return Ok(());
                }
                // Try refresh before falling through to device code
                if config.get_refresh_token().is_some() {
                    tracing::info!("AAD token expired, attempting refresh...");
                    match refresh().await {
                        Ok(true) => {
                            println!("Token refreshed successfully.");
                            return Ok(());
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!("Refresh failed, falling back to device code: {:#}", e);
                        }
                    }
                }
            }
        }
    }

    let auth_config = AuthConfig::default();
    let client = build_client(&auth_config)?;

    // Use device code flow for CLI
    tracing::info!("Initiating device code flow...");

    let device_auth_response: StandardDeviceAuthorizationResponse = client
        .exchange_device_code()?
        .add_scope(Scope::new(
            "https://api.spaces.skype.com/.default".to_string(),
        ))
        .add_scope(Scope::new("offline_access".to_string()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .context("Failed to request device code")?;

    let verification_url = device_auth_response.verification_uri().as_str();
    let user_code = device_auth_response.user_code().secret();

    println!();
    println!("To sign in, visit: {}", verification_url);
    println!("Enter code:        {}", user_code);
    println!();

    // Poll for token
    tracing::info!("Waiting for authentication...");

    let token_response = client
        .exchange_device_access_token(&device_auth_response)
        .request_async(oauth2::reqwest::async_http_client, tokio::time::sleep, None)
        .await
        .context("Failed to exchange device code for token")?;

    // Save AAD tokens (single load-mutate-save)
    let mut config = Config::load()?;
    config.set_access_token(
        token_response.access_token().secret().to_string(),
        token_response.expires_in().map(|d| d.as_secs()),
    );

    if let Some(refresh_token) = token_response.refresh_token() {
        config.set_refresh_token(refresh_token.secret().to_string());
    }

    // Exchange for Skype token
    let aad_token = token_response.access_token().secret();
    let mut skype_ok = false;
    match exchange_skype_token(aad_token, false).await {
        Ok((skype_tok, expires_in, region_gtms)) => {
            config.set_skype_token(skype_tok, expires_in);
            if let Some(gtms) = region_gtms {
                config.set_region_gtms(gtms);
            }
            skype_ok = true;
        }
        Err(e) => {
            tracing::warn!("Skype token exchange failed: {:#}", e);
            eprintln!("Warning: Skype token exchange failed; some operations may not work.");
        }
    }

    // Acquire Graph API token (separate audience from Skype token)
    let mut graph_ok = false;
    if let Some(ref rt) = config.get_refresh_token() {
        let auth_config = AuthConfig::default();
        let client = build_client(&auth_config)?;
        match acquire_graph_token(&client, rt).await {
            Ok((graph_tok, expires_in)) => {
                config.set_graph_token(graph_tok, expires_in);
                graph_ok = true;
            }
            Err(e) => {
                tracing::warn!("Graph token acquisition failed: {:#}", e);
                eprintln!("Warning: Graph token acquisition failed; whoami/chats may not work.");
            }
        }
    }

    // Acquire IC3 token (for Trouter WebSocket auth)
    let mut ic3_ok = false;
    if let Some(ref rt) = config.get_refresh_token() {
        let auth_config = AuthConfig::default();
        let client = build_client(&auth_config)?;
        match acquire_ic3_token(&client, rt).await {
            Ok((ic3_tok, expires_in)) => {
                config.set_ic3_token(ic3_tok, expires_in);
                ic3_ok = true;
            }
            Err(e) => {
                tracing::warn!("IC3 token acquisition failed: {:#}", e);
                eprintln!("Warning: IC3 token acquisition failed; trouter may not work.");
            }
        }
    }

    // Acquire recorder service token (for call recording)
    let mut recorder_ok = false;
    if let Some(ref rt) = config.get_refresh_token() {
        let auth_config = AuthConfig::default();
        let client = build_client(&auth_config)?;
        match acquire_recorder_token(&client, rt).await {
            Ok((rec_tok, expires_in)) => {
                config.set_recorder_token(rec_tok, expires_in);
                recorder_ok = true;
            }
            Err(e) => {
                tracing::warn!("Recorder token acquisition failed: {:#}", e);
                eprintln!("Warning: Recorder token acquisition failed; recording may not work.");
            }
        }
    }

    config.save()?;
    if skype_ok && graph_ok && ic3_ok && recorder_ok {
        println!("Login successful.");
    } else {
        println!(
            "Login partially successful (missing: {}).",
            [
                (!skype_ok).then_some("Skype"),
                (!graph_ok).then_some("Graph"),
                (!ic3_ok).then_some("IC3"),
                (!recorder_ok).then_some("Recorder")
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
        );
    }
    Ok(())
}

/// Clear stored credentials
pub async fn logout() -> Result<()> {
    let mut config = Config::load()?;
    config.clear_tokens();
    config.save()?;
    println!("Logged out.");
    Ok(())
}

/// Display current auth status
pub async fn status() -> Result<()> {
    let config = Config::load()?;

    // AAD token status
    match config.get_access_token() {
        Some(token) if !token.is_expired() => {
            println!("AAD token:   valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("AAD token:   expired");
        }
        None => {
            println!("AAD token:   none");
        }
    }

    // Refresh token
    match config.get_refresh_token() {
        Some(_) => println!("Refresh tok: present"),
        None => println!("Refresh tok: none"),
    }

    // Graph token status
    match config.get_graph_token() {
        Some(token) if !token.is_expired() => {
            println!("Graph token: valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("Graph token: expired");
        }
        None => {
            println!("Graph token: none");
        }
    }

    // IC3 token status
    match config.get_ic3_token() {
        Some(token) if !token.is_expired() => {
            println!("IC3 token:   valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("IC3 token:   expired");
        }
        None => {
            println!("IC3 token:   none");
        }
    }

    // Recorder token status
    match config.get_recorder_token() {
        Some(token) if !token.is_expired() => {
            println!("Recorder tk: valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("Recorder tk: expired");
        }
        None => {
            println!("Recorder tk: none");
        }
    }

    // Skype token status
    match config.get_skype_token() {
        Some(token) if !token.is_expired() => {
            println!("Skype token: valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("Skype token: expired");
        }
        None => {
            println!("Skype token: none");
        }
    }

    // Region GTMs
    if config.region_gtms.is_some() {
        println!("Region GTMs: present");
    } else {
        println!("Region GTMs: none");
    }

    if config.get_access_token().is_none() {
        println!("\nRun 'teams-cli login' to authenticate.");
    }

    Ok(())
}

// MARK: - Token broker (any scope + nested app auth)
//
// A TeamsJS host answers `authentication.getAuthToken` (the app's
// `webApplicationInfo.resource`) and MSAL.js nested-app-auth `GetToken`
// requests. Both are refresh-token grants on the stored Teams refresh
// token: plain for a resource/scope, brokered (`brk_client_id`) for a
// nested app's own client id. Tokens are cached in memory per
// (client, scopes) and never logged.

/// One minted access token. `Debug` never prints the token.
#[derive(Clone)]
pub struct TokenGrant {
    pub access_token: String,
    pub expires_in: Option<u64>,
    pub scope: Option<String>,
}

impl std::fmt::Debug for TokenGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenGrant")
            .field("access_token", &format_args!("<{} chars>", self.access_token.len()))
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish()
    }
}

/// AAD v2 token endpoint for a tenant (`common`, a tenant id, …).
pub fn token_endpoint(tenant: &str) -> String {
    format!("https://login.microsoftonline.com/{}/oauth2/v2.0/token", tenant)
}

/// A resource (`https://x.sharepoint.com`, `api://host/guid`, a bare
/// app id) becomes `<resource>/.default`; anything that already names a
/// scope (`…/.default`, `https://graph.microsoft.com/User.Read`,
/// `openid`) is kept. Space-separated lists are normalized per item,
/// deduplicated, in order.
pub fn normalize_scopes(input: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in input.split_whitespace() {
        let s = raw.trim_end_matches('/');
        let scope = if s.ends_with("/.default") || !s.contains(['/', ':', '-']) {
            // Already `.default`, or an OIDC scope (openid, profile, …).
            s.to_string()
        } else if let Some(rest) = s.strip_prefix("api://") {
            if rest.is_empty() { continue; }
            // api://host/guid is a resource; api://host/guid/scope a scope
            // only when the last segment is not a GUID-like id.
            let last = rest.rsplit('/').next().unwrap_or(rest);
            if rest.contains('/') && !looks_like_id(last) {
                s.to_string()
            } else {
                format!("{}/.default", s)
            }
        } else if let Ok(u) = url::Url::parse(s) {
            if matches!(u.scheme(), "https" | "http") && (u.path().is_empty() || u.path() == "/") {
                format!("{}/.default", s)
            } else {
                s.to_string()
            }
        } else if looks_like_id(s) {
            format!("{}/.default", s)
        } else {
            s.to_string()
        };
        if !out.contains(&scope) {
            out.push(scope);
        }
    }
    out.join(" ")
}

fn looks_like_id(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Refresh-token grant form for `scopes` on `client_id`.
pub fn scope_grant_form(client_id: &str, refresh_token: &str, scopes: &str) -> Vec<(String, String)> {
    vec![
        ("client_id".into(), client_id.into()),
        ("grant_type".into(), "refresh_token".into()),
        ("refresh_token".into(), refresh_token.into()),
        ("scope".into(), format!("{} offline_access", normalize_scopes(scopes))),
    ]
}

/// Brokered (nested app auth) redirect for an app origin:
/// `https://tasks.example.com` → `brk-multihub://tasks.example.com`.
pub fn naa_redirect_uri(origin: &str) -> Option<String> {
    let u = url::Url::parse(origin).ok()?;
    let host = u.host_str()?;
    Some(match u.port() {
        Some(p) => format!("brk-multihub://{}:{}", host, p),
        None => format!("brk-multihub://{}", host),
    })
}

/// The hub's own registered redirect (Teams client `1fec8e78`, native
/// public client), sent as `brk_redirect_uri` in brokered grants.
pub const HUB_REDIRECT_URI: &str = "https://login.microsoftonline.com/common/oauth2/nativeclient";

/// Brokered refresh-token grant: the hub (`broker_client_id`, our
/// Teams client) mints a token for a nested app's own client id.
/// `redirect_uri` is the nested app's `brk-multihub://<host>` and
/// `brk_redirect_uri` the hub's own redirect. Verified live (Planner's
/// client): the hub redirect plus NO `Origin` header succeeds; the
/// app's brk-multihub URI as `brk_redirect_uri` fails AADSTS50011, and
/// any `Origin` fails AADSTS9002326 (a native client's refresh token
/// cannot be redeemed cross-origin).
pub fn naa_grant_form(
    broker_client_id: &str,
    nested_client_id: &str,
    refresh_token: &str,
    scopes: &str,
    redirect_uri: &str,
) -> Vec<(String, String)> {
    vec![
        ("client_id".into(), nested_client_id.into()),
        ("grant_type".into(), "refresh_token".into()),
        ("refresh_token".into(), refresh_token.into()),
        ("scope".into(), format!("{} offline_access", normalize_scopes(scopes))),
        ("brk_client_id".into(), broker_client_id.into()),
        ("brk_redirect_uri".into(), HUB_REDIRECT_URI.into()),
        ("redirect_uri".into(), redirect_uri.into()),
    ]
}

/// POSTs one token grant. Errors carry the AAD error code and the first
/// line of its description only (never request or response secrets).
pub async fn post_token_grant(
    http: &reqwest::Client,
    url: &str,
    form: &[(String, String)],
    origin: Option<&str>,
) -> Result<TokenGrant> {
    let mut req = http.post(url).form(form);
    if let Some(o) = origin {
        req = req.header("Origin", o);
    }
    let resp = req.send().await.context("token endpoint unreachable")?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        let code = v.get("error").and_then(|e| e.as_str()).unwrap_or("unknown_error");
        let desc = v
            .get("error_description")
            .and_then(|e| e.as_str())
            .and_then(|d| d.lines().next())
            .unwrap_or("");
        anyhow::bail!("token grant failed (HTTP {}): {} {}", status.as_u16(), code, desc);
    }
    let access_token = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .context("token response has no access_token")?
        .to_string();
    Ok(TokenGrant {
        access_token,
        expires_in: v.get("expires_in").and_then(|e| e.as_u64().or_else(|| e.as_str()?.parse().ok())),
        scope: v.get("scope").and_then(|s| s.as_str()).map(String::from),
    })
}

type GrantCache = std::sync::Mutex<std::collections::HashMap<String, (TokenGrant, std::time::Instant)>>;

fn grant_cache() -> &'static GrantCache {
    static CACHE: std::sync::OnceLock<GrantCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Cached grant still valid for at least 5 minutes, with its remaining
/// lifetime as `expires_in`.
fn cached_grant(key: &str) -> Option<TokenGrant> {
    let cache = grant_cache().lock().unwrap_or_else(|e| e.into_inner());
    let (g, until) = cache.get(key)?;
    let left = until.checked_duration_since(std::time::Instant::now())?;
    if left.as_secs() < 300 {
        return None;
    }
    Some(TokenGrant { expires_in: Some(left.as_secs()), ..g.clone() })
}

fn store_grant(key: String, g: &TokenGrant) {
    let life = std::time::Duration::from_secs(g.expires_in.unwrap_or(3600));
    grant_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, (g.clone(), std::time::Instant::now() + life));
}

/// Drops every cached grant (sign-out).
pub fn clear_grants() {
    grant_cache().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

fn stored_refresh_token() -> Result<String> {
    Config::load()?.get_refresh_token().context("No refresh token. Sign in again.")
}

fn broker_http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(reqwest::Client::new)
}

/// Access token for any resource or scope (`getAuthToken`), minted with
/// the stored Teams refresh token on the Teams client id.
pub async fn token_for_scope(scopes: &str) -> Result<TokenGrant> {
    let scopes = normalize_scopes(scopes);
    anyhow::ensure!(!scopes.is_empty(), "no scope");
    let auth = AuthConfig::default();
    let key = format!("{}|{}", auth.client_id, scopes);
    if let Some(g) = cached_grant(&key) {
        return Ok(g);
    }
    let rt = stored_refresh_token()?;
    let form = scope_grant_form(auth.client_id, &rt, &scopes);
    let g = post_token_grant(broker_http(), &token_endpoint(auth.tenant), &form, None).await?;
    store_grant(key, &g);
    Ok(g)
}

/// Nested app auth token: `client_id` is the nested app's AAD client,
/// `origin` its page origin (redirect `brk-multihub://<host>`). The
/// grant is sent without an `Origin` header (see [`naa_grant_form`]).
pub async fn naa_token_for(client_id: &str, scopes: &str, origin: &str) -> Result<TokenGrant> {
    let scopes = normalize_scopes(scopes);
    anyhow::ensure!(!scopes.is_empty(), "no scope");
    anyhow::ensure!(!client_id.trim().is_empty(), "no client id");
    let redirect = naa_redirect_uri(origin).context("bad app origin")?;
    let auth = AuthConfig::default();
    let key = format!("{}|{}|{}", client_id, redirect, scopes);
    if let Some(g) = cached_grant(&key) {
        return Ok(g);
    }
    let rt = stored_refresh_token()?;
    let form = naa_grant_form(auth.client_id, client_id, &rt, &scopes, &redirect);
    let g = post_token_grant(broker_http(), &token_endpoint(auth.tenant), &form, None).await?;
    store_grant(key, &g);
    Ok(g)
}

#[cfg(test)]
mod broker_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn scopes_normalize() {
        assert_eq!(normalize_scopes("https://contoso.sharepoint.com"), "https://contoso.sharepoint.com/.default");
        assert_eq!(normalize_scopes("https://contoso.sharepoint.com/"), "https://contoso.sharepoint.com/.default");
        assert_eq!(
            normalize_scopes("api://tasks.example.com/11111111-2222-3333-4444-555555555555"),
            "api://tasks.example.com/11111111-2222-3333-4444-555555555555/.default"
        );
        assert_eq!(normalize_scopes("api://tasks.example.com/abc/access_as_user"), "api://tasks.example.com/abc/access_as_user");
        assert_eq!(
            normalize_scopes("75f31797-37c9-498e-8dc9-53c16a36afca"),
            "75f31797-37c9-498e-8dc9-53c16a36afca/.default"
        );
        assert_eq!(
            normalize_scopes("https://graph.microsoft.com/User.Read openid openid https://graph.microsoft.com/.default"),
            "https://graph.microsoft.com/User.Read openid https://graph.microsoft.com/.default"
        );
        assert_eq!(normalize_scopes("  "), "");
    }

    #[test]
    fn grant_forms() {
        let f = scope_grant_form("hub", "RT", "https://x.sharepoint.com");
        assert!(f.contains(&("scope".into(), "https://x.sharepoint.com/.default offline_access".into())));
        assert!(f.contains(&("client_id".into(), "hub".into())));
        assert_eq!(naa_redirect_uri("https://tasks.example.com/teamsui/x").as_deref(), Some("brk-multihub://tasks.example.com"));
        assert_eq!(naa_redirect_uri("http://localhost:8080").as_deref(), Some("brk-multihub://localhost:8080"));
        assert_eq!(naa_redirect_uri("not a url"), None);
        let n = naa_grant_form("hub", "nested", "RT", "User.Read", "brk-multihub://a.example.com");
        assert!(n.contains(&("client_id".into(), "nested".into())));
        assert!(n.contains(&("brk_client_id".into(), "hub".into())));
        assert!(n.contains(&("redirect_uri".into(), "brk-multihub://a.example.com".into())));
        assert!(n.contains(&("brk_redirect_uri".into(), HUB_REDIRECT_URI.into())));
        let dbg = format!("{:?}", TokenGrant { access_token: "SECRET".into(), expires_in: Some(1), scope: None });
        assert!(!dbg.contains("SECRET"));
    }

    /// One-shot HTTP stub: records the raw request, answers `resp`.
    async fn stub(resp: String) -> (String, Arc<Mutex<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}/common/oauth2/v2.0/token", listener.local_addr().expect("addr"));
        let seen = Arc::new(Mutex::new(String::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                let n = sock.read(&mut tmp).await.unwrap_or(0);
                if n == 0 { break; }
                buf.extend_from_slice(&tmp[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text[..end]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len { break; }
                }
            }
            *log.lock().unwrap() = String::from_utf8_lossy(&buf).to_string();
            sock.write_all(resp.as_bytes()).await.ok();
            sock.shutdown().await.ok();
        });
        (url, seen)
    }

    fn http_json(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            body.len(),
            body
        )
    }

    #[tokio::test]
    async fn naa_grant_posts_brokered_form_and_parses() {
        let (url, seen) = stub(http_json(
            "200 OK",
            r#"{"token_type":"Bearer","scope":"User.Read","expires_in":"3599","access_token":"AT-1"}"#,
        ))
        .await;
        let form = naa_grant_form("hub", "nested", "RT", "User.Read", "brk-multihub://a.example.com");
        let g = post_token_grant(&reqwest::Client::new(), &url, &form, None)
            .await
            .expect("grant");
        assert_eq!(g.access_token, "AT-1");
        assert_eq!(g.expires_in, Some(3599));
        let req = seen.lock().unwrap().clone();
        assert!(req.starts_with("POST /common/oauth2/v2.0/token"));
        assert!(!req.to_ascii_lowercase().contains("\norigin:"), "brokered grants carry no Origin");
        assert!(req.contains("brk_client_id=hub"));
        assert!(req.contains("client_id=nested"));
        assert!(req.contains("grant_type=refresh_token"));
        assert!(req.contains("redirect_uri=brk-multihub%3A%2F%2Fa.example.com"));
        assert!(req.contains("brk_redirect_uri=https%3A%2F%2Flogin.microsoftonline.com%2Fcommon%2Foauth2%2Fnativeclient"));
    }

    #[tokio::test]
    async fn grant_error_reports_code_not_secrets() {
        let (url, _) = stub(http_json(
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"AADSTS70000: bad grant.\r\nTrace ID: x"}"#,
        ))
        .await;
        let form = scope_grant_form("hub", "RT-SECRET", "https://x.sharepoint.com");
        let e = post_token_grant(&reqwest::Client::new(), &url, &form, None).await.unwrap_err();
        let msg = format!("{:#}", e);
        assert!(msg.contains("invalid_grant") && msg.contains("AADSTS70000"));
        assert!(!msg.contains("RT-SECRET") && !msg.contains("Trace ID"));
    }

    #[test]
    fn grant_cache_expiry_and_clear() {
        let g = TokenGrant { access_token: "t".into(), expires_in: Some(3600), scope: None };
        store_grant("c|s1".into(), &g);
        store_grant("c|s2".into(), &TokenGrant { expires_in: Some(60), ..g.clone() });
        assert!(cached_grant("c|s1").is_some());
        assert!(cached_grant("c|s2").is_none(), "under 5 min left = refetch");
        clear_grants();
        assert!(cached_grant("c|s1").is_none());
    }
}
