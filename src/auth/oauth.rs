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
