//! OAuth2 device code flow for Azure AD, plus Skype token exchange

use anyhow::{Context, Result};
use oauth2::{
    basic::{BasicClient, BasicErrorResponseType},
    AuthUrl, ClientId, DeviceAuthorizationUrl, RefreshToken, RequestTokenError, Scope,
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
    refresh_for(&crate::config::active_profile()).await
}

/// Same as [`refresh`], scoped to one account profile (multi-account:
/// per-account refresh without switching the active profile).
pub async fn refresh_for(profile: &str) -> Result<bool> {
    refresh_for_with(profile, live_main_exchange, true).await
}

/// One refresh-token grant against the token endpoint (TOKEN-REFRESH-RACE
/// seam: production uses [`live_main_exchange`]; tests inject a stub IdP
/// with single-use rotating refresh tokens, zero live traffic).
#[derive(Debug, Clone)]
pub struct RefreshedTokens {
    pub access_token: String,
    pub expires_in: Option<u64>,
    /// Rotated refresh token, when the IdP issued one.
    pub refresh_token: Option<String>,
}

/// Production grant: one AAD refresh-token exchange (Skype scope).
async fn live_main_exchange(refresh_token: String) -> Result<RefreshedTokens> {
    let auth_config = AuthConfig::default();
    let client = build_client(&auth_config)?;

    tracing::info!("Refreshing AAD token...");

    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token))
        .add_scope(Scope::new(
            "https://api.spaces.skype.com/.default".to_string(),
        ))
        .add_scope(Scope::new("offline_access".to_string()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .map_err(|e| {
            // Burned/expired/revoked refresh token: distinct, non-retryable.
            let typed_invalid = matches!(
                &e,
                RequestTokenError::ServerResponse(r)
                    if matches!(r.error(), BasicErrorResponseType::InvalidGrant)
            );
            if typed_invalid || format!("{:#}", &e).contains("invalid_grant") {
                let detail = match &e {
                    RequestTokenError::ServerResponse(r) => r
                        .error_description()
                        .map(|d| d.lines().next().unwrap_or("").to_string())
                        .filter(|d| !d.is_empty())
                        .unwrap_or_else(|| "invalid_grant".to_string()),
                    _ => "invalid_grant".to_string(),
                };
                return anyhow::Error::new(ReauthRequired { detail });
            }
            anyhow::Error::new(e).context("Failed to refresh AAD token")
        })?;

    Ok(RefreshedTokens {
        access_token: token_response.access_token().secret().to_string(),
        expires_in: token_response.expires_in().map(|d| d.as_secs()),
        refresh_token: token_response
            .refresh_token()
            .map(|rt| rt.secret().to_string()),
    })
}

/// The stored refresh token was rejected by the IdP (`invalid_grant`:
/// burned, expired, or revoked). Not retryable: the user must sign in
/// again. Callers must fail fast with a re-auth message, never retry.
#[derive(Debug, thiserror::Error)]
#[error("stored session expired (refresh token rejected: {detail}); run `teams-cli login` to sign in again")]
pub struct ReauthRequired {
    /// First line of the IdP error description (never secrets).
    pub detail: String,
}

/// True when `e` is (or wraps) [`ReauthRequired`); the message-substring
/// fallback covers grant errors already formatted as `… invalid_grant …`
/// (see `post_token_grant`), which carry no typed marker.
pub fn is_reauth_required(e: &anyhow::Error) -> bool {
    if e.downcast_ref::<ReauthRequired>().is_some() {
        return true;
    }
    format!("{:#}", e).contains("invalid_grant")
}

/// Per-profile in-process single-flight: concurrent `refresh_for` calls in
/// this process serialize here; cross-process callers serialize on the
/// profile lockfile (held across the SAME critical section, so the two
/// compose into one global exclusion per profile).
fn inflight_for(profile: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static SLOTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let map = SLOTS.get_or_init(Default::default);
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(profile.to_string())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Max time a refresher waits for another process's hold before failing
/// loud (retryable [`crate::config::ProfileBusy`], never `ReauthRequired`:
/// contention is not a burned credential). Worst legit hold is one main
/// grant plus the derived chain (Skype/Graph/IC3/recorder), each
/// HTTP-bounded; 90s covers a slow-but-alive holder, and the
/// post-timeout adoption re-read saves the common case where the winner
/// just finished while we waited.
const PROFILE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(90);

/// Acquire the profile lockfile without stalling the async runtime
/// (blocking `flock` runs on the blocking pool), waiting at most
/// [`PROFILE_LOCK_WAIT`].
async fn acquire_profile_lock(profile: &str) -> Result<crate::config::ProfileFileLock> {
    acquire_profile_lock_for(profile, PROFILE_LOCK_WAIT).await
}

/// [`acquire_profile_lock`] with an explicit wait (tests pass seconds;
/// production uses [`PROFILE_LOCK_WAIT`]).
async fn acquire_profile_lock_for(
    profile: &str,
    wait: std::time::Duration,
) -> Result<crate::config::ProfileFileLock> {
    let owned = profile.to_string();
    tokio::task::spawn_blocking(move || Config::try_lock_profile_exclusive(&owned, wait))
        .await
        .context("profile lock task failed")?
}

/// [`refresh_for`] with an injectable main grant and a derived-token switch
/// (tests pass `false`: no Skype/Graph/IC3/recorder calls, zero traffic).
///
/// Race protocol (all under per-profile in-process single-flight AND the
/// cross-process profile lockfile, held across read-exchange-write):
///  1. snapshot the stored refresh and access tokens;
///  2. re-read from disk once the locks are held; if the refresh token
///     rotated OR the access token changed, another process already
///     refreshed -> adopt it (invalidate cache) and return WITHOUT any
///     exchange. Comparing token STRINGS (not validity) is what makes
///     solo proactive refresh keep working: with no winner the re-read
///     is identical to the snapshot, so a still-valid access token does
///     NOT suppress the exchange (the missing-derived-token path in
///     `login` depends on that);
///  3. otherwise exchange exactly once;
///  4. NEVER persist on failure (no downgrade writes: a burned store stays
///     exactly as it was, so a later sign-in is not clobbered).
/// Lock contention fails loud: on [`crate::config::ProfileBusy`] the
/// waiter re-reads once (adopting a winner that just finished) and
/// otherwise returns the retryable busy error — never `ReauthRequired`.
async fn refresh_for_with<E, F>(profile: &str, exchange: E, acquire_derived: bool) -> Result<bool>
where
    E: Fn(String) -> F + Send + Sync,
    F: std::future::Future<Output = Result<RefreshedTokens>> + Send,
{
    refresh_for_with_wait(profile, exchange, acquire_derived, PROFILE_LOCK_WAIT).await
}

/// [`refresh_for_with`] with an explicit lock wait (tests pass seconds).
async fn refresh_for_with_wait<E, F>(
    profile: &str,
    exchange: E,
    acquire_derived: bool,
    lock_wait: std::time::Duration,
) -> Result<bool>
where
    E: Fn(String) -> F + Send + Sync,
    F: std::future::Future<Output = Result<RefreshedTokens>> + Send,
{
    let snapshot = Config::load_cached_for(profile)?;
    let refresh_token_str = match snapshot.get_refresh_token() {
        Some(rt) => rt,
        None => return Ok(false),
    };
    let snapshot_at = snapshot.get_access_token().map(|t| t.token);

    // (a) in-process single-flight.
    let slot = inflight_for(profile);
    let _inproc = slot.lock().await;
    // (b) cross-process exclusion, held across re-read + exchange + write.
    let _file = match acquire_profile_lock_for(profile, lock_wait).await {
        Ok(g) => g,
        Err(e) if crate::config::is_profile_busy(&e) => {
            // Contended the whole wait: adopt a winner that finished
            // meanwhile, else fail loud (retryable, not burned).
            let fresh = Config::load_for(profile)?;
            let rt_now = fresh.get_refresh_token();
            let at_now = fresh.get_access_token().map(|t| t.token);
            if rt_now.as_deref() != Some(refresh_token_str.as_str()) || at_now != snapshot_at {
                Config::invalidate_cache_for(profile);
                return Ok(rt_now.is_some());
            }
            return Err(e);
        }
        Err(e) => return Err(e),
    };

    // (c) re-read under the locks; skip the exchange when already fresh.
    // (`rt_now == None` lands here too — signed out elsewhere —
    // returning Ok(false).)
    let fresh = Config::load_for(profile)?;
    let rt_now = fresh.get_refresh_token();
    let at_now = fresh.get_access_token().map(|t| t.token);
    if rt_now.as_deref() != Some(refresh_token_str.as_str()) || at_now != snapshot_at {
        // Winner already persisted: adopt, no exchange.
        Config::invalidate_cache_for(profile);
        return Ok(rt_now.is_some());
    }
    let mut config = fresh;

    // (d) exactly one exchange; on failure persist NOTHING.
    let new_tokens = exchange(refresh_token_str).await?;

    config.set_access_token(new_tokens.access_token, new_tokens.expires_in);

    if let Some(new_rt) = new_tokens.refresh_token {
        config.set_refresh_token(new_rt);
    }

    // Derived tokens (Skype/Graph/IC3/recorder): live traffic, skipped
    // by stub-IdP tests (`acquire_derived = false`).
    if acquire_derived {
        let auth_config = AuthConfig::default();
        let client = build_client(&auth_config)?;

        // Exchange for Skype token
        let aad_token = config
            .get_access_token()
            .map(|t| t.token)
            .unwrap_or_default();
        match exchange_skype_token(&aad_token, false).await {
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
    }

    config.save_to(profile)?;
    tracing::info!("Token refresh complete");
    Ok(true)
}

/// Perform OAuth2 login flow
pub async fn login(force: bool) -> Result<()> {
    {
        let config = Config::load_cached()?;

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
                        match refresh().await {
                            Ok(true) => {
                                println!("Tokens refreshed (acquired missing derived tokens).");
                                return Ok(());
                            }
                            Err(e) if is_reauth_required(&e) => {
                                // Burned stored token, access still valid for
                                // now: single attempt, no retry; re-auth can
                                // wait until expiry (or --force now).
                                eprintln!("Stored refresh token rejected ({:#}); access token still valid. Run with --force to sign in again now.", e);
                            }
                            _ => {}
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
                            if is_reauth_required(&e) {
                                // Single attempt, no retry: the device-code
                                // flow below IS the re-auth.
                                eprintln!("Stored session expired ({:#}); signing in again.", e);
                            } else {
                                tracing::warn!(
                                    "Refresh failed, falling back to device code: {:#}",
                                    e
                                );
                            }
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
    let mut config = Config::load_cached()?;
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
    let mut config = Config::load_cached()?;
    config.clear_tokens();
    config.save()?;
    println!("Logged out.");
    Ok(())
}

/// Display current auth status
pub async fn status() -> Result<()> {
    let config = Config::load_cached()?;

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

/// TOKEN-REFRESH-RACE tests: stub IdP (rotating single-use refresh
/// tokens, in-process) + temp-dir stores (`OST_CONFIG_DIR`). ZERO live
/// traffic: `acquire_derived = false`, no HTTP anywhere in this module.
#[cfg(test)]
mod refresh_race_tests {
    use super::*;
    use crate::config::Config;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    /// True when this process is the spawned child for `entry` (env
    /// `TOKENRACE_CHILD` matches): the caller runs its body. Otherwise
    /// spawns that child (which runs ONLY this test via the harness
    /// filter, so the env swap below never shares a process with store
    /// tests like `config::tests`), asserts it passed, and returns false.
    fn in_spawned_child(entry: &str) -> bool {
        if std::env::var("TOKENRACE_CHILD").as_deref() == Ok(entry) {
            return true;
        }
        let exe = std::env::current_exe().expect("test exe");
        let out = std::process::Command::new(&exe)
            .arg("--nocapture")
            .arg(entry)
            .env("TOKENRACE_CHILD", entry)
            .output()
            .expect("spawn child");
        assert!(
            out.status.success(),
            "child {entry} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        false
    }

    /// Serializes the tests in this module within one process (they swap
    /// the process-wide `OST_CONFIG_DIR`).
    static STORE_LOCK: Mutex<()> = Mutex::new(());

    /// Temp-dir store with a unique profile. Restores the previous
    /// `OST_CONFIG_DIR` on drop. Hold `STORE_LOCK` while alive.
    struct TempStore {
        _guard: std::sync::MutexGuard<'static, ()>,
        prev: Option<std::ffi::OsString>,
        profile: String,
    }

    impl TempStore {
        fn new(tag: &str) -> Self {
            let guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("OST_CONFIG_DIR");
            let dir = std::env::temp_dir().join(format!(
                "tokenrace-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).expect("temp store dir");
            std::env::set_var("OST_CONFIG_DIR", &dir);
            let profile = format!("tokenrace-{}", tag);
            Config::invalidate_cache_for(&profile);
            Self {
                _guard: guard,
                prev,
                profile,
            }
        }

        /// Seed an expired access token + `rt` as the stored refresh token.
        fn seed(&self, rt: &str) {
            let mut cfg = Config::default();
            cfg.set_access_token("AT-OLD".to_string(), Some(0));
            cfg.set_refresh_token(rt.to_string());
            cfg.save_to(&self.profile).expect("seed store");
        }

        /// Seed a still-VALID access token + `rt` (the proactive-refresh
        /// shape: `login`'s missing-derived branch calls `refresh_for`
        /// while the access token is good).
        fn seed_valid(&self, rt: &str) {
            let mut cfg = Config::default();
            cfg.set_access_token("AT-OLD".to_string(), Some(3600));
            cfg.set_refresh_token(rt.to_string());
            cfg.save_to(&self.profile).expect("seed store");
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            Config::invalidate_cache_for(&self.profile);
            match self.prev.take() {
                Some(v) => std::env::set_var("OST_CONFIG_DIR", v),
                None => std::env::remove_var("OST_CONFIG_DIR"),
            }
        }
    }

    /// Stub IdP: single-use refresh tokens. The FIRST exchange of the
    /// current token wins; any replay of a burned token fails with
    /// `invalid_grant`. Rotating mode issues a fresh RT per win (like AAD
    /// normally does); non-rotating mode re-issues the SAME RT string but
    /// still burns it server-side after one redeem (the TOKENRACE-R2 gap
    /// (b) shape: RT unchanged on disk while the store's access token is
    /// already fresh). `delay` forces caller overlap.
    struct StubIdP {
        state: Mutex<StubState>,
        exchanges: AtomicU64,
        delay: std::time::Duration,
        rotating: bool,
    }

    struct StubState {
        current_rt: String,
        next_id: u64,
        consumed: bool,
    }

    impl StubIdP {
        fn new(rt0: &str, delay: std::time::Duration) -> Arc<Self> {
            Self::new_mode(rt0, delay, true)
        }

        fn new_mode(rt0: &str, delay: std::time::Duration, rotating: bool) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(StubState {
                    current_rt: rt0.to_string(),
                    next_id: 1,
                    consumed: false,
                }),
                exchanges: AtomicU64::new(0),
                delay,
                rotating,
            })
        }

        async fn exchange(self: &Arc<Self>, rt: String) -> Result<RefreshedTokens> {
            self.exchanges.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if rt != st.current_rt || (st.consumed && !self.rotating) {
                anyhow::bail!("token grant failed (HTTP 400): invalid_grant burned token replay");
            }
            st.consumed = true;
            let id = st.next_id;
            st.next_id += 1;
            if self.rotating {
                st.current_rt = format!("RT-{}", id);
                st.consumed = false;
            }
            Ok(RefreshedTokens {
                access_token: format!("AT-{}", id),
                expires_in: Some(3600),
                refresh_token: if self.rotating {
                    Some(st.current_rt.clone())
                } else {
                    None
                },
            })
        }
    }

    /// R0 failing-first: N concurrent refreshes must cost exactly ONE IdP
    /// exchange (single-use RT). Unfixed code: every caller exchanges
    /// (N) and last-writer-wins persists a burned token.
    #[tokio::test]
    async fn concurrent_refresh_costs_one_exchange() {
        if !in_spawned_child("refresh_race_tests::concurrent_refresh_costs_one_exchange") {
            return;
        }
        let store = TempStore::new("race");
        store.seed("RT-0");
        let stub = StubIdP::new("RT-0", std::time::Duration::from_millis(200));

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let s = stub.clone();
            let profile = store.profile.clone();
            tasks.push(tokio::spawn(async move {
                refresh_for_with(
                    &profile,
                    move |rt| {
                        let s = s.clone();
                        async move { s.exchange(rt).await }
                    },
                    false,
                )
                .await
            }));
        }
        let mut results = Vec::new();
        for t in tasks {
            results.push(t.await.expect("task"));
        }
        assert_eq!(
            stub.exchanges.load(Ordering::SeqCst),
            1,
            "single-use RT: exactly one IdP exchange for N concurrent refreshes"
        );
        assert_eq!(
            results.iter().filter(|r| matches!(r, Ok(true))).count(),
            8,
            "every caller must succeed"
        );
        // Losers adopt the winner's tokens (no burned-token persistence).
        let final_cfg = Config::load_cached_for(&store.profile).expect("reload");
        let rt = final_cfg.get_refresh_token().expect("stored rt");
        assert_eq!(rt, stub.state.lock().unwrap().current_rt);
        assert!(final_cfg
            .get_access_token()
            .is_some_and(|t| !t.is_expired()));
        assert_eq!(
            final_cfg.get_access_token().expect("at").token,
            format!("AT-1")
        );
    }

    /// Failure persists nothing: a burned stored token fails every caller
    /// with `ReauthRequired` and the store file stays byte-identical (no
    /// downgrade writes). Exchange count == N proves one attempt each and
    /// no retry loop (a storm would exceed N).
    #[tokio::test]
    async fn failed_refresh_persists_nothing() {
        if !in_spawned_child("refresh_race_tests::failed_refresh_persists_nothing") {
            return;
        }
        let store = TempStore::new("fail");
        store.seed("RT-BURNED");
        let path = Config::config_path_for(&store.profile).expect("path");
        let before = std::fs::read(&path).expect("seeded file");
        let stub = StubIdP::new("RT-ELSEWHERE", std::time::Duration::from_millis(50));

        let mut tasks = Vec::new();
        for _ in 0..4 {
            let s = stub.clone();
            let profile = store.profile.clone();
            tasks.push(tokio::spawn(async move {
                refresh_for_with(
                    &profile,
                    move |rt| {
                        let s = s.clone();
                        async move { s.exchange(rt).await }
                    },
                    false,
                )
                .await
            }));
        }
        let mut reauth = 0;
        for t in tasks {
            let r = t.await.expect("task");
            let e = r.expect_err("burned token must fail");
            assert!(is_reauth_required(&e), "must be ReauthRequired: {:#}", e);
            reauth += 1;
        }
        assert_eq!(reauth, 4);
        assert_eq!(
            stub.exchanges.load(Ordering::SeqCst),
            4,
            "one attempt per caller, no retries"
        );
        Config::invalidate_cache_for(&store.profile);
        assert_eq!(
            std::fs::read(&path).expect("file"),
            before,
            "store untouched on failure"
        );
    }

    #[test]
    fn reauth_required_predicate() {
        let typed = anyhow::Error::new(ReauthRequired { detail: "x".into() });
        assert!(is_reauth_required(&typed));
        let wrapped = typed.context("outer");
        assert!(is_reauth_required(&wrapped));
        let formatted: anyhow::Error =
            anyhow::anyhow!("token grant failed (HTTP 400): invalid_grant nope");
        assert!(is_reauth_required(&formatted));
        let other: anyhow::Error = anyhow::anyhow!("connection refused");
        assert!(!is_reauth_required(&other));
    }

    /// Child-process work for [`profile_lock_serializes_across_processes`]:
    /// one locked read-modify-write of the shared counter. True when the
    /// child env was present (work done). Never creates a `TempStore` (it
    /// would swap `OST_CONFIG_DIR` away from the shared store).
    fn do_child_increment() -> bool {
        let Ok(spec) = std::env::var("TOKENRACE_LOCK_CHILD") else {
            return false;
        };
        let (profile, counter) = spec.split_once('|').expect("spec");
        {
            let _held = Config::lock_profile_exclusive(profile).expect("child lock");
            let n: u64 = std::fs::read_to_string(counter)
                .expect("read")
                .trim()
                .parse()
                .expect("num");
            std::thread::sleep(std::time::Duration::from_millis(100));
            std::fs::write(counter, (n + 1).to_string()).expect("write");
        }
        true
    }

    /// Cross-process exclusion: N child PROCESSES (real OS processes, one
    /// shared lockfile) read-modify-write a counter under the profile lock.
    /// Without the `flock`, parallel readers all see 0 and the final count
    /// is 1; with it, exactly N.
    #[tokio::test]
    async fn profile_lock_serializes_across_processes() {
        if do_child_increment() {
            return;
        }
        if !in_spawned_child("refresh_race_tests::profile_lock_serializes_across_processes") {
            return;
        }
        let store = TempStore::new("lockx");
        let counter = std::env::temp_dir().join(format!(
            "tokenrace-counter-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::write(&counter, "0").expect("counter");
        let exe = std::env::current_exe().expect("test exe");
        let mut kids = Vec::new();
        for _ in 0..4 {
            let spec = format!("{}|{}", store.profile, counter.display());
            kids.push(
                tokio::task::spawn_blocking({
                    let exe = exe.clone();
                    move || {
                        std::process::Command::new(&exe)
                            .arg("refresh_race_tests::lock_child_entry")
                            .env("TOKENRACE_LOCK_CHILD", &spec)
                            .output()
                            .expect("spawn child")
                    }
                })
                .await
                .expect("join"),
            );
        }
        for out in &kids {
            assert!(out.status.success(), "child failed: {:?}", out);
        }
        let n: u64 = std::fs::read_to_string(&counter)
            .expect("read")
            .trim()
            .parse()
            .expect("num");
        assert_eq!(n, kids.len() as u64, "no lost updates across processes");
    }

    /// Entry point for [`profile_lock_serializes_across_processes`] child
    /// processes (matched by substring filter). With the child env set it
    /// does the locked increment; without it (plain suite run) it passes
    /// as a no-op and never spawns.
    #[test]
    fn lock_child_entry() {
        let _ = do_child_increment();
    }

    /// R2 gap (b): proactive refresh (still-valid access token, e.g.
    /// `login`'s missing-derived branch) racing a winner against a
    /// NON-rotating IdP. The re-read sees an unchanged RT with a still-valid
    /// access token, so the loser redeems the already-redeemed RT and fails
    /// spuriously on a store that is actually fresh. Fixed code: the loser
    /// adopts whenever the access token changed under it (exactly one
    /// exchange, both callers Ok).
    #[tokio::test]
    async fn proactive_refresh_adopts_after_winner_no_rotation() {
        if !in_spawned_child(
            "refresh_race_tests::proactive_refresh_adopts_after_winner_no_rotation",
        ) {
            return;
        }
        let store = TempStore::new("proactive");
        store.seed_valid("RT-0");
        let stub = StubIdP::new_mode("RT-0", std::time::Duration::from_millis(150), false);

        let s = stub.clone();
        let profile = store.profile.clone();
        let first = tokio::spawn(async move {
            refresh_for_with(
                &profile,
                move |rt| {
                    let s = s.clone();
                    async move { s.exchange(rt).await }
                },
                false,
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let s = stub.clone();
        let profile = store.profile.clone();
        let second = tokio::spawn(async move {
            refresh_for_with(
                &profile,
                move |rt| {
                    let s = s.clone();
                    async move { s.exchange(rt).await }
                },
                false,
            )
            .await
        });
        let a = first.await.expect("task");
        let b = second.await.expect("task");
        match (&a, &b) {
            (Ok(_), Ok(_)) => {}
            _ => panic!("both callers must succeed (a={a:?}, b={b:?})"),
        }
        assert_eq!(
            stub.exchanges.load(Ordering::SeqCst),
            1,
            "loser must adopt, not redeem"
        );
        let final_cfg = Config::load_cached_for(&store.profile).expect("reload");
        assert!(
            final_cfg
                .get_access_token()
                .is_some_and(|t| !t.is_expired()),
            "store must hold the winner's fresh access token"
        );
    }

    /// Corner-fix guard: a SOLO proactive refresh (still-valid access,
    /// no concurrent winner — `login`'s missing-derived-token path) must
    /// STILL exchange. The string-compare adopt rule only skips when the
    /// re-read differs from the snapshot, and with no winner they are
    /// identical.
    #[tokio::test]
    async fn proactive_solo_refresh_still_exchanges() {
        if !in_spawned_child("refresh_race_tests::proactive_solo_refresh_still_exchanges") {
            return;
        }
        let store = TempStore::new("prosol");
        store.seed_valid("RT-0");
        let stub = StubIdP::new("RT-0", std::time::Duration::ZERO);
        let s = stub.clone();
        let profile = store.profile.clone();
        match refresh_for_with(
            &profile,
            move |rt| {
                let s = s.clone();
                async move { s.exchange(rt).await }
            },
            false,
        )
        .await
        {
            Ok(true) => {}
            other => panic!("solo proactive refresh must exchange (got {other:?})"),
        }
        assert_eq!(
            stub.exchanges.load(Ordering::SeqCst),
            1,
            "no winner: exactly one exchange, not a silent adopt"
        );
    }

    /// R2 gap (c): the profile `flock` used to block with NO timeout — a
    /// process hung mid-exchange stalled every other refresher forever.
    /// Fixed code waits boundedly ([`PROFILE_LOCK_WAIT`], seconds in this
    /// test) and fails loud with retryable `ProfileBusy` — never
    /// `ReauthRequired`. (Supersedes the vendored-lane R0 characterization
    /// `profile_lock_wait_is_unbounded`, absent on this base: it proved the
    /// unbounded wait — now bounded by construction.)
    #[tokio::test]
    async fn refresh_fails_loud_when_lock_busy() {
        if !in_spawned_child("refresh_race_tests::refresh_fails_loud_when_lock_busy") {
            return;
        }
        let store = TempStore::new("lockbusy");
        store.seed("RT-0");
        let stub = StubIdP::new("RT-0", std::time::Duration::ZERO);
        let held = Config::lock_profile_exclusive(&store.profile).expect("hold lock");
        let s = stub.clone();
        let profile = store.profile.clone();
        let t0 = std::time::Instant::now();
        let r = refresh_for_with_wait(
            &profile,
            move |rt| {
                let s = s.clone();
                async move { s.exchange(rt).await }
            },
            false,
            std::time::Duration::from_secs(2),
        )
        .await;
        let waited = t0.elapsed();
        let e = r.expect_err("contended lock must fail, not hang");
        assert!(
            crate::config::is_profile_busy(&e),
            "must be typed ProfileBusy, got: {:#}",
            e
        );
        assert!(
            !is_reauth_required(&e),
            "contention is not a burned credential: {:#}",
            e
        );
        assert!(
            waited < std::time::Duration::from_secs(10),
            "wait must be bounded (took {waited:?})"
        );
        assert_eq!(
            stub.exchanges.load(Ordering::SeqCst),
            0,
            "no exchange while contended"
        );
        // Store untouched: a later retry or sign-in is not clobbered.
        let after = Config::load_for(&store.profile).expect("reload");
        assert_eq!(after.get_refresh_token().as_deref(), Some("RT-0"));
        drop(held);
    }
}
