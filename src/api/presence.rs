//! Presence API for Microsoft Teams

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

#[derive(Debug, Deserialize)]
struct PresenceResponse {
    availability: String,
    activity: String,
}

/// Get current presence status (prints to stdout).
pub async fn get_presence() -> Result<()> {
    let client = TeamsClient::new().await?;
    let info = get_presence_data(&client).await?;

    println!("\nPresence Status:");
    println!("  Availability: {}", info.availability);
    println!("  Activity: {}", info.activity);

    Ok(())
}

// ---------------------------------------------------------------------------
// Data-returning API functions for TUI integration
// ---------------------------------------------------------------------------

/// Presence info for TUI display.
#[allow(dead_code)]
pub struct PresenceInfo {
    pub availability: String,
    pub activity: String,
}

/// Fetch current presence and return structured data.
pub async fn get_presence_data(client: &TeamsClient) -> Result<PresenceInfo> {
    let resp = client.graph_get("/me/presence").await?;
    let presence: PresenceResponse = resp
        .json()
        .await
        .context("Failed to parse presence response")?;

    Ok(PresenceInfo {
        availability: presence.availability,
        activity: presence.activity,
    })
}

/// Set presence status
pub async fn set_presence(status: &str) -> Result<()> {
    let (availability, activity) = match status.to_lowercase().as_str() {
        "available" => ("Available", "Available"),
        "busy" => ("Busy", "InACall"),
        "dnd" | "donotdisturb" => ("DoNotDisturb", "Presenting"),
        "away" => ("Away", "Away"),
        "offline" => ("Offline", "OffWork"),
        other => bail!(
            "Unknown status: {}. Use: available, busy, dnd, away, offline",
            other
        ),
    };

    let client = TeamsClient::new().await?;
    let body = serde_json::json!({
        "sessionId": "teams-cli",
        "availability": availability,
        "activity": activity,
        "expirationDuration": "PT1H"
    });

    client
        .graph_post("/me/presence/setUserPreferredPresence", &body)
        .await?;

    println!("Presence set to: {}", status);
    Ok(())
}

/// Presence service audience and front door.
const PRESENCE_SCOPE: &str = "https://presence.teams.microsoft.com/.default";
const PRESENCE_URL: &str = "https://presence.teams.microsoft.com";

/// Tell the presence service this endpoint (the Trouter `epid`) is online.
///
/// Without an active endpoint the user shows as Offline, and the calling
/// service does not offer incoming calls to it: they ring for the whole
/// timeout and go to voicemail. Teams clients publish their endpoint when
/// they connect and keep it fresh while they run.
pub async fn publish_endpoint(http: &reqwest::Client, epid: &str) -> Result<()> {
    let token = crate::auth::oauth::access_token_for(PRESENCE_SCOPE).await?;
    let body = endpoint_body(epid);
    let resp = http
        .put(format!("{PRESENCE_URL}/v1/me/endpoints/"))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .context("Presence endpoint PUT failed to send")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("Presence endpoint PUT failed: HTTP {status}");
    }
    tracing::debug!("Presence endpoint published ({status})");
    Ok(())
}

fn endpoint_body(epid: &str) -> serde_json::Value {
    serde_json::json!({
        "id": epid,
        "activity": "Available",
        "availability": "Available",
        "deviceType": "Desktop",
    })
}

#[cfg(test)]
mod endpoint_tests {
    use super::endpoint_body;

    #[test]
    fn endpoint_body_names_the_trouter_endpoint() {
        let b = endpoint_body("2f951868-95ad-4039-a816-592dbfc52786");
        assert_eq!(b["id"], "2f951868-95ad-4039-a816-592dbfc52786");
        assert_eq!(b["availability"], "Available");
        assert_eq!(b["deviceType"], "Desktop");
    }
}
