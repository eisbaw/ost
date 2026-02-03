//! Microsoft Graph API: joined teams and channels

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

#[derive(Debug, Deserialize)]
struct TeamsResponse {
    value: Vec<Team>,
}

#[derive(Debug, Deserialize)]
struct Team {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChannelsResponse {
    value: Vec<Channel>,
}

#[derive(Debug, Deserialize)]
struct Channel {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

pub async fn list_teams() -> Result<()> {
    let client = TeamsClient::new().await?;

    tracing::info!("Fetching joined teams...");
    let resp = client.graph_get("/me/joinedTeams").await?;
    let teams: TeamsResponse = resp
        .json()
        .await
        .context("Failed to parse joinedTeams response")?;

    println!("\nTeams and Channels:");
    println!("{:-<60}", "");

    if teams.value.is_empty() {
        println!("  (no teams found)");
        return Ok(());
    }

    for team in &teams.value {
        let team_name = team.display_name.as_deref().unwrap_or(&team.id);
        tracing::debug!("Fetching channels for team: {} ({})", team_name, team.id);

        let path = format!("/teams/{}/channels", team.id);
        let resp = client
            .graph_get(&path)
            .await
            .with_context(|| format!("Failed to fetch channels for team {}", team_name))?;
        let channels: ChannelsResponse = resp
            .json()
            .await
            .context("Failed to parse channels response")?;

        println!("Team: {} ({} channels)", team_name, channels.value.len());
        for ch in &channels.value {
            let ch_name = ch.display_name.as_deref().unwrap_or(&ch.id);
            println!("  {:<30} {}", ch_name, ch.id);
        }
        println!();
    }

    Ok(())
}
