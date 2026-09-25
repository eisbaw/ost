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

// ---------------------------------------------------------------------------
// Team members + owners (om-h5-members)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MembersResponse {
    value: Vec<Member>,
}

#[derive(Debug, Deserialize)]
struct Member {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "userId")]
    user_id: Option<String>,
    email: Option<String>,
    #[serde(default)]
    roles: Vec<String>,
}

/// One team roster entry. `id` is the membership id (the DELETE
/// target), NOT the user id. Owners are members whose `roles`
/// contains `"owner"`; `is_owner` pins that rule in one place.
pub struct TeamMemberInfo {
    pub id: String,
    pub display_name: String,
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub roles: Vec<String>,
    pub is_owner: bool,
}

/// `GET/POST /teams/{id}/members` path. Pure so tests pin it.
pub fn members_path(team_id: &str) -> String {
    format!("/teams/{}/members", team_id.trim())
}

/// `DELETE /teams/{team}/members/{member}` path. Pure so tests pin it.
pub fn member_path(team_id: &str, member_id: &str) -> String {
    format!(
        "/teams/{}/members/{}",
        team_id.trim(),
        member_id.trim()
    )
}

/// `POST /teams/{id}/members` body: `user` is a user id or UPN;
/// `owner` adds the `"owner"` role (Graph rejects unknown roles).
/// Pure so tests pin it.
pub fn add_member_body(user: &str, owner: bool) -> serde_json::Value {
    let roles: Vec<&str> = if owner { vec!["owner"] } else { vec![] };
    serde_json::json!({
        "@odata.type": "#microsoft.graph.aadUserConversationMember",
        "roles": roles,
        "user@odata.bind": format!(
            "https://graph.microsoft.com/v1.0/users('{}')",
            user.trim()
        ),
    })
}

fn member_info(m: Member) -> TeamMemberInfo {
    let is_owner = m.roles.iter().any(|r| r == "owner");
    TeamMemberInfo {
        display_name: m.display_name.unwrap_or_else(|| m.id.clone()),
        id: m.id,
        user_id: m.user_id,
        email: m.email,
        roles: m.roles,
        is_owner,
    }
}

/// List one team's roster (members + owners) and return structured data.
/// Missing `displayName` falls back to the membership id; blank stays
/// blank — the Swift caller-fallback owns display names (see roster UI).
pub async fn list_team_members_data(
    client: &TeamsClient,
    team_id: &str,
) -> Result<Vec<TeamMemberInfo>> {
    check_id("team_id", team_id)?;
    let resp = client.graph_get(&members_path(team_id)).await?;
    let members: MembersResponse = resp
        .json()
        .await
        .context("Failed to parse team members response")?;
    Ok(members.value.into_iter().map(member_info).collect())
}

/// Add one user to a team (`owner` grants the owner role) and return
/// the created membership. Empty args are rejected before any network.
pub async fn add_team_member_data(
    client: &TeamsClient,
    team_id: &str,
    user: &str,
    owner: bool,
) -> Result<TeamMemberInfo> {
    check_id("team_id", team_id)?;
    if user.trim().is_empty() {
        bail!("empty user");
    }
    let resp = client
        .graph_post(&members_path(team_id), &add_member_body(user, owner))
        .await?;
    let member: Member = resp
        .json()
        .await
        .context("Failed to parse added team member response")?;
    Ok(member_info(member))
}

/// Remove one membership from a team. `member_id` is the membership
/// id from the roster, not the user id. Empty args are rejected
/// before any network.
pub async fn remove_team_member_data(
    client: &TeamsClient,
    team_id: &str,
    member_id: &str,
) -> Result<()> {
    check_id("team_id", team_id)?;
    check_id("member_id", member_id)?;
    client
        .graph_delete(&member_path(team_id, member_id))
        .await?;
    Ok(())
}

/// List one team's roster (prints to stdout). `owners_only` keeps
/// owner-role entries; the header still names the full count.
pub async fn list_team_members(team_id: &str, owners_only: bool) -> Result<()> {
    let client = TeamsClient::new().await?;
    let members = list_team_members_data(&client, team_id).await?;
    let shown: Vec<&TeamMemberInfo> = if owners_only {
        members.iter().filter(|m| m.is_owner).collect()
    } else {
        members.iter().collect()
    };

    println!("\nTeam {} members ({}{}):", team_id.trim(), shown.len(), if owners_only { " owners" } else { "" });
    println!("{:-<60}", "");
    if shown.is_empty() {
        println!("  (none)");
        return Ok(());
    }
    for m in shown {
        let role = if m.is_owner { "owner" } else { "member" };
        let mail = m.email.as_deref().unwrap_or("-");
        println!("  {:<24} {:<7} {}  {}", m.display_name, role, m.id, mail);
    }
    Ok(())
}

/// Add one user to a team (prints to stdout).
pub async fn add_team_member(team_id: &str, user: &str, owner: bool) -> Result<()> {
    let client = TeamsClient::new().await?;
    let m = add_team_member_data(&client, team_id, user, owner).await?;
    println!(
        "Added {} as {} (membership {})",
        m.display_name,
        if m.is_owner { "owner" } else { "member" },
        m.id
    );
    Ok(())
}

/// Remove one membership from a team (prints to stdout).
pub async fn remove_team_member(team_id: &str, member_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    remove_team_member_data(&client, team_id, member_id).await?;
    println!("Removed membership {} from team {}", member_id.trim(), team_id.trim());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_guard_rejects_path_breaking() {
        for bad in ["", "   ", "a/b", "a?b", "a#b", "a b", "a\tb"] {
            assert!(check_id("team_id", bad).is_err(), "id {:?}", bad);
            assert!(check_id("member_id", bad).is_err(), "id {:?}", bad);
        }
        assert!(check_id("team_id", "550e8400-e29b-41d4-a716-446655440000").is_ok());
        assert!(check_id("team_id", "f09c6c30-1234-abcd-ef00-1234567890ab").is_ok());
        assert!(check_id("member_id", "aWQ9abcDEF123==").is_ok());
    }

    #[test]
    fn paths_pin_graph_shapes() {
        assert_eq!(members_path("  t1 "), "/teams/t1/members");
        assert_eq!(member_path("t1", "m9"), "/teams/t1/members/m9");
    }

    #[test]
    fn add_body_pins_odata_shape() {
        let member = add_member_body("user@example.com", false);
        assert_eq!(
            member["@odata.type"],
            "#microsoft.graph.aadUserConversationMember"
        );
        assert_eq!(member["roles"], serde_json::json!([]));
        assert_eq!(
            member["user@odata.bind"],
            "https://graph.microsoft.com/v1.0/users('user@example.com')"
        );
        let owner = add_member_body("  oid-1 ", true);
        assert_eq!(owner["roles"], serde_json::json!(["owner"]));
        assert_eq!(
            owner["user@odata.bind"],
            "https://graph.microsoft.com/v1.0/users('oid-1')"
        );
    }

    #[test]
    fn members_parse_owner_flag_and_fallbacks() {
        let body: MembersResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"M1","displayName":"Doe, Jane","userId":"oid-1","email":"j@x.example","roles":["owner"]},
                {"id":"M2","displayName":"","userId":"oid-2","email":null,"roles":[]},
                {"id":"M3"}]}"#,
        )
        .unwrap();
        let infos: Vec<_> = body.value.into_iter().map(member_info).collect();
        assert_eq!(infos.len(), 3);
        assert!(infos[0].is_owner);
        assert_eq!(infos[0].roles, vec!["owner"]);
        assert_eq!(infos[0].display_name, "Doe, Jane");
        assert_eq!(infos[0].user_id.as_deref(), Some("oid-1"));
        assert!(!infos[1].is_owner);
        assert_eq!(infos[1].display_name, ""); // blank kept: UI caller-fallback owns names
        assert!(infos[1].email.is_none());
        assert_eq!(infos[2].display_name, "M3"); // missing -> membership id
        assert!(!infos[2].is_owner); // missing roles -> member
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
