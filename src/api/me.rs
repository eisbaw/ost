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

/// Fetch and display current user info from Graph /me endpoint.
pub async fn whoami() -> Result<()> {
    let client = TeamsClient::new().await?;
    let resp = client.graph_get("/me").await?;
    let me: MeResponse = resp.json().await.context("Failed to parse /me response")?;

    println!();
    println!(
        "Display Name: {}",
        me.display_name.as_deref().unwrap_or("(none)")
    );
    println!("Mail:         {}", me.mail.as_deref().unwrap_or("(none)"));
    println!(
        "UPN:          {}",
        me.user_principal_name.as_deref().unwrap_or("(none)")
    );
    println!("ID:           {}", me.id);

    Ok(())
}
