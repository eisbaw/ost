//! Microsoft Graph API: joined teams and channels

use anyhow::{bail, Context, Result};
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
    description: Option<String>,
    #[serde(rename = "membershipType")]
    membership_type: Option<String>,
    #[serde(rename = "webUrl")]
    web_url: Option<String>,
}

/// List joined teams and channels (prints to stdout).
pub async fn list_teams() -> Result<()> {
    let client = TeamsClient::new().await?;
    let teams = list_teams_data(&client).await?;

    println!("\nTeams and Channels:");
    println!("{:-<60}", "");

    if teams.is_empty() {
        println!("  (no teams found)");
        return Ok(());
    }

    for team in &teams {
        println!("Team: {} ({} channels)", team.name, team.channels.len());
        for ch in &team.channels {
            println!("  {:<30} {}", ch.name, ch.id);
        }
        println!();
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Data-returning API functions for TUI integration
// ---------------------------------------------------------------------------

/// Team metadata for TUI display.
pub struct TeamInfo {
    pub id: String,
    pub name: String,
    pub channels: Vec<ChannelInfo>,
}

/// Channel metadata for TUI display.
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    /// Graph `description` (absent on old/unset channels).
    pub description: Option<String>,
    /// Graph `membershipType` (`standard`/`private`/`shared`).
    pub membership_type: Option<String>,
    /// Graph `webUrl` (open-in-browser deep link).
    pub web_url: Option<String>,
}

/// Graph path for creating a channel in one team (pure so tests pin it).
pub fn create_channel_path(team_id: &str) -> String {
    format!("/teams/{}/channels", team_id)
}

/// POST body for channel creation: `displayName` plus `description` only
/// when non-blank (pure so tests pin it).
pub fn create_channel_body(name: &str, description: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({ "displayName": name });
    if let Some(d) = description {
        if !d.trim().is_empty() {
            body["description"] = serde_json::Value::String(d.to_string());
        }
    }
    body
}

/// Reject ids that would break out of the Graph path segment.
fn check_id(what: &str, id: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("empty {}", what);
    }
    if id.contains('/')
        || id.contains('?')
        || id.contains('#')
        || id.chars().any(|c| c.is_whitespace())
    {
        bail!("{} must not contain '/', '?', '#' or whitespace", what);
    }
    Ok(())
}

fn channel_info(ch: Channel) -> ChannelInfo {
    ChannelInfo {
        name: ch.display_name.unwrap_or_else(|| ch.id.clone()),
        id: ch.id,
        description: ch.description,
        membership_type: ch.membership_type,
        web_url: ch.web_url,
    }
}

/// Join one team by id: self-enroll via `POST /teams/{id}/members`.
/// Resolves the caller's own user id from Graph /me, then adds self as
/// a plain member (`aadUserConversationMember`, no roles). Returns the
/// trimmed team id on success.
///
/// Join-by-code (6-char Teams invite codes) is NOT Graph — codes redeem
/// only through the undocumented teams.microsoft.com web API, so codes
/// are out of scope here; callers pass the team id (GUID).
pub async fn join_team_data(client: &TeamsClient, team_id: &str) -> Result<String> {
    let id = team_id.trim();
    anyhow::ensure!(!id.is_empty(), "empty team_id");
    anyhow::ensure!(
        !id.contains(['/', '?', '#']),
        "invalid team_id (path separator)"
    );
    let me = super::me::whoami_data(client).await?;
    let body = serde_json::json!({
        "@odata.type": "#microsoft.graph.aadUserConversationMember",
        "roles": [],
        "user@odata.bind": format!("https://graph.microsoft.com/v1.0/users('{}')", me.id),
    });
    let path = format!("/teams/{}/members", id);
    client.graph_post(&path, &body).await?;
    Ok(id.to_string())
}

/// List joined teams with their channels and return structured data.
pub async fn list_teams_data(client: &TeamsClient) -> Result<Vec<TeamInfo>> {
    tracing::debug!("Fetching joined teams...");
    let resp = client.graph_get("/me/joinedTeams").await?;
    let teams: TeamsResponse = resp
        .json()
        .await
        .context("Failed to parse joinedTeams response")?;

    let mut result = Vec::new();

    for team in &teams.value {
        let team_name = team.display_name.as_deref().unwrap_or(&team.id).to_string();
        tracing::debug!("Fetching channels for team: {} ({})", team_name, team.id);

        let path = format!("/teams/{}/channels", team.id);
        let channels = match client.graph_get(&path).await {
            Ok(resp) => {
                let channels_resp: ChannelsResponse = resp
                    .json()
                    .await
                    .context("Failed to parse channels response")?;
                channels_resp.value.into_iter().map(channel_info).collect()
            }
            Err(e) => {
                tracing::warn!("Failed to fetch channels for {}: {:#}", team_name, e);
                Vec::new()
            }
        };

        result.push(TeamInfo {
            id: team.id.clone(),
            name: team_name,
            channels,
        });
    }

    Ok(result)
}

/// Create one standard channel in a team and return it.
///
/// Graph `POST /teams/{team-id}/channels` (`Teamwork.Create` on the
/// first-party client id; a 403 surfaces as the call's detail). Empty
/// names and path-breaking team ids are rejected before any network.
pub async fn create_channel_data(
    client: &TeamsClient,
    team_id: &str,
    name: &str,
    description: Option<&str>,
) -> Result<ChannelInfo> {
    check_id("team_id", team_id)?;
    if name.trim().is_empty() {
        bail!("empty name");
    }
    let path = create_channel_path(team_id);
    let body = create_channel_body(name, description);
    let resp = client.graph_post(&path, &body).await?;
    let channel: Channel = resp
        .json()
        .await
        .context("Failed to parse created channel response")?;
    Ok(channel_info(channel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_guard_rejects_path_breaking() {
        for bad in ["", "   ", "a/b", "a?b", "a#b", "a b"] {
            assert!(check_id("team_id", bad).is_err(), "id {:?}", bad);
        }
        assert!(check_id("team_id", "550e8400-e29b-41d4-a716-446655440000").is_ok());
    }

    #[test]
    fn create_path_names_team_channels() {
        assert_eq!(
            create_channel_path("team-1"),
            "/teams/team-1/channels"
        );
    }

    #[test]
    fn create_body_carries_display_name_and_optional_description() {
        let with = create_channel_body("General 2", Some("Second room"));
        assert_eq!(with["displayName"], "General 2");
        assert_eq!(with["description"], "Second room");
        let bare = create_channel_body("Random", None);
        assert_eq!(bare["displayName"], "Random");
        assert!(bare.get("description").is_none());
        let blank = create_channel_body("Random", Some("   "));
        assert!(blank.get("description").is_none());
    }

    #[test]
    fn created_channel_parses_with_name_fallback() {
        let named: Channel = serde_json::from_str(
            r#"{"id":"19:new@thread.tacv2","displayName":"New room"}"#,
        )
        .unwrap();
        let info = channel_info(named);
        assert_eq!(info.id, "19:new@thread.tacv2");
        assert_eq!(info.name, "New room");
        let unnamed: Channel =
            serde_json::from_str(r#"{"id":"19:anon@thread.tacv2"}"#).unwrap();
        assert_eq!(channel_info(unnamed).name, "19:anon@thread.tacv2");
    }
}
