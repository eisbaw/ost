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

// ---------------------------------------------------------------------------
// Public (joinable) team search
// ---------------------------------------------------------------------------

/// Result cap for one public-team search window.
pub const PUBLIC_TEAMS_MAX: usize = 25;

/// One joinable team from a directory search.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicTeamInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Graph `visibility` lowercased (`public`; `hiddenmembership` and
    /// `private` are filtered out before callers see them).
    pub visibility: String,
}

/// Which Graph surface answered a public-team search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicTeamsSource {
    /// `GET /groups` `$search` (word match; needs `Group.Read.All`-class
    /// read, `ConsistencyLevel: eventual`).
    Groups,
    /// `GET /teams` `startswith(displayName)` (prefix match;
    /// `Team.ReadBasic.All`), used when `/groups` is refused.
    Teams,
}

/// Percent-encode one query component (unreserved chars kept).
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Graph `/groups` search path for teams whose name matches `query`
/// (word-prefix `$search`, team-provisioned groups only). Double quotes
/// and backslashes are dropped (they would end the `$search` phrase).
/// Pure so tests pin it.
pub fn public_teams_groups_path(query: &str, limit: usize) -> String {
    let q: String = query
        .trim()
        .chars()
        .filter(|c| *c != '"' && *c != '\\')
        .collect();
    format!(
        "/groups?$search={}&$filter={}&$select=id,displayName,description,visibility&$top={}&$count=true",
        encode_component(&format!("\"displayName:{}\"", q)),
        encode_component("resourceProvisioningOptions/Any(x:x eq 'Team')"),
        limit.clamp(1, PUBLIC_TEAMS_MAX)
    )
}

/// Graph `/teams` fallback path (`startswith(displayName)`, single
/// quotes doubled per OData). Pure so tests pin it.
pub fn public_teams_list_path(query: &str, limit: usize) -> String {
    let q = query.trim().replace('\'', "''");
    format!(
        "/teams?$filter={}&$select=id,displayName,description,visibility&$top={}",
        encode_component(&format!("startswith(displayName,'{}')", q)),
        limit.clamp(1, PUBLIC_TEAMS_MAX)
    )
}

/// Parse one `/groups` or `/teams` page into joinable teams: only
/// `visibility == public` (case-insensitive) with a non-blank id; a
/// missing name falls back to the id. Unknown shapes yield no rows.
pub fn parse_public_teams(value: &serde_json::Value) -> Vec<PublicTeamInfo> {
    let Some(rows) = value.get("value").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    rows.iter()
        .filter_map(|row| {
            let id = row.get("id")?.as_str()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            let visibility = row
                .get("visibility")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if visibility != "public" || !seen.insert(id.clone()) {
                return None;
            }
            let name = row
                .get("displayName")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| id.clone());
            let description = row
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            Some(PublicTeamInfo { id, name, description, visibility })
        })
        .collect()
}

/// Search the directory for public (joinable) teams by name, one
/// window. Graph `/groups` `$search` first (word match); on any error
/// (typically 403 without group read), `/teams` `startswith` prefix
/// match. Blank queries are rejected before any network. Joined teams
/// are NOT filtered here (callers mark membership against their list);
/// join a hit with [`join_team_data`] (its id is the team id).
pub async fn search_public_teams_data(
    client: &TeamsClient,
    query: &str,
    limit: usize,
) -> Result<(PublicTeamsSource, Vec<PublicTeamInfo>)> {
    if query.trim().is_empty() {
        bail!("empty query");
    }
    let groups = async {
        let resp = client
            .graph_get_consistent(&public_teams_groups_path(query, limit))
            .await?;
        let v: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse groups search response")?;
        Ok::<_, anyhow::Error>(parse_public_teams(&v))
    };
    match groups.await {
        Ok(rows) => Ok((PublicTeamsSource::Groups, rows)),
        Err(e) => {
            tracing::debug!("groups team search failed, trying /teams: {:#}", e);
            let resp = client
                .graph_get(&public_teams_list_path(query, limit))
                .await
                .with_context(|| format!("groups search: {:#}", e))?;
            let v: serde_json::Value = resp
                .json()
                .await
                .context("Failed to parse teams list response")?;
            Ok((PublicTeamsSource::Teams, parse_public_teams(&v)))
        }
    }
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
                channels_resp
                    .value
                    .into_iter()
                    .map(|ch| ChannelInfo {
                        name: ch.display_name.unwrap_or_else(|| ch.id.clone()),
                        id: ch.id,
                        description: ch.description,
                        membership_type: ch.membership_type,
                        web_url: ch.web_url,
                    })
                    .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn public_team_search_paths_and_parse() {
        // /groups: $search phrase (quotes stripped from input), team filter,
        // clamp 1..=25, $count (required with ConsistencyLevel).
        assert_eq!(
            public_teams_groups_path(" Sup\"port ", 99),
            "/groups?$search=%22displayName%3ASupport%22\
             &$filter=resourceProvisioningOptions%2FAny%28x%3Ax%20eq%20%27Team%27%29\
             &$select=id,displayName,description,visibility&$top=25&$count=true"
        );
        // /teams fallback: OData quote doubling, then encoding.
        assert_eq!(
            public_teams_list_path("O'Brien", 0),
            "/teams?$filter=startswith%28displayName%2C%27O%27%27Brien%27%29\
             &$select=id,displayName,description,visibility&$top=1"
        );
        let v = json!({"value": [
            {"id": "t1", "displayName": "Support", "description": " Help ", "visibility": "Public"},
            {"id": "t2", "displayName": "Secret", "visibility": "Private"},
            {"id": "t3", "displayName": "Hidden", "visibility": "HiddenMembership"},
            {"id": "t1", "displayName": "Dup", "visibility": "public"},
            {"id": " ", "displayName": "Blank", "visibility": "public"},
            {"id": "t4", "visibility": "public"},
            {"displayName": "No id", "visibility": "public"}
        ]});
        let rows = parse_public_teams(&v);
        let ids: Vec<_> = rows.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t1", "t4"]);
        assert_eq!(rows[0].description.as_deref(), Some("Help"));
        assert_eq!(rows[0].visibility, "public");
        assert_eq!(rows[1].name, "t4");
        assert!(parse_public_teams(&json!({"nope": 1})).is_empty());
    }
}
