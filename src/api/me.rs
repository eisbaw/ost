//! User profile endpoint (/me)

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

#[derive(Debug, Deserialize)]
struct MeResponse {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    mail: Option<String>,
    #[serde(rename = "userPrincipalName")]
    user_principal_name: Option<String>,
}

/// Fetch and display current user info from Graph /me endpoint (prints to stdout).
pub async fn whoami() -> Result<()> {
    let client = TeamsClient::new().await?;
    let info = whoami_data(&client).await?;

    println!();
    println!("Display Name: {}", info.display_name);
    println!("Mail:         {}", info.mail.as_deref().unwrap_or("(none)"));
    println!("ID:           {}", info.id);

    Ok(())
}

// ---------------------------------------------------------------------------
// Data-returning API functions for TUI integration
// ---------------------------------------------------------------------------

/// User info for TUI display.
#[allow(dead_code)]
pub struct UserInfo {
    pub display_name: String,
    pub mail: Option<String>,
    pub id: String,
}

/// Fetch current user info and return structured data.
pub async fn whoami_data(client: &TeamsClient) -> Result<UserInfo> {
    let resp = client.graph_get("/me").await?;
    let me: MeResponse = resp.json().await.context("Failed to parse /me response")?;

    Ok(UserInfo {
        display_name: me.display_name.unwrap_or_else(|| "User".to_string()),
        mail: me.mail,
        id: me.id,
    })
}
