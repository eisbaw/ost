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
// Channel edit + delete
// ---------------------------------------------------------------------------

/// Graph path for one channel in one team (pure so tests pin it).
pub fn channel_path(team_id: &str, channel_id: &str) -> String {
    format!("/teams/{}/channels/{}", team_id.trim(), channel_id.trim())
}

/// PATCH body for a channel edit: only the fields that change. A blank
/// description clears it (Graph takes `""`); `None` leaves it alone.
pub fn update_channel_body(name: Option<&str>, description: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({});
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        body["displayName"] = serde_json::Value::String(n.to_string());
    }
    if let Some(d) = description {
        body["description"] = serde_json::Value::String(d.trim().to_string());
    }
    body
}

/// Rename a channel and/or change its description.
///
/// Graph `PATCH /teams/{team-id}/channels/{channel-id}` (204). The
/// General channel cannot be renamed; Graph answers 400 and the detail
/// surfaces to the caller. An empty change is rejected before any
/// network.
pub async fn update_channel_data(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<()> {
    check_id("team_id", team_id)?;
    check_id("channel_id", channel_id)?;
    let body = update_channel_body(name, description);
    if body.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        bail!("nothing to update");
    }
    client
        .graph_patch(&channel_path(team_id, channel_id), &body)
        .await?;
    Ok(())
}

/// Delete one channel (Graph `DELETE /teams/{team-id}/channels/{id}`,
/// 204). Teams keeps deleted channels restorable for 30 days. Empty or
/// path-breaking ids are rejected before any network.
pub async fn delete_channel_data(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
) -> Result<()> {
    check_id("team_id", team_id)?;
    check_id("channel_id", channel_id)?;
    client
        .graph_delete(&channel_path(team_id, channel_id))
        .await?;
    Ok(())
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

// ---------------------------------------------------------------------------
// Channel message reactions (om-je-parity)
// ---------------------------------------------------------------------------
// Graph setReaction/unsetReaction on channel messages — the channel
// counterpart to the chat-service `.../messages/{id}/reactions` calls,
// which return 404 on channel threads (see om-h1-listdetail-PROOF.md).
// Paths follow the v1.0 docs (`/teams/{id}/channels/{id}/messages/{id}/
// (set|unset)Reaction`, `reactionType` sent as unicode); delegated
// `ChannelMessage.Send` rides the first-party client id.
//
// NOT YET VERIFIED LIVE: auth was down when this landed (expired
// refresh token), so the unicode-vs-named `reactionType` form, the
// scope grant, and the Graph-vs-chat-service message id mapping still
// need a signed-in probe before any caller is wired. Receipts stay on
// HOLD: Graph exposes no channel read-receipt API (v1.0 chatMessage
// resource lists no receipt/read-state method), and the chat-service
// consumptionhorizons call 403s on channel threads.

/// `POST .../messages/{id}/setReaction` path. Pure so tests pin it.
pub fn channel_set_reaction_path(team_id: &str, channel_id: &str, message_id: &str) -> String {
    format!(
        "/teams/{}/channels/{}/messages/{}/setReaction",
        team_id.trim(),
        channel_id.trim(),
        message_id.trim()
    )
}

/// `POST .../messages/{id}/unsetReaction` path. Pure so tests pin it.
pub fn channel_unset_reaction_path(
    team_id: &str,
    channel_id: &str,
    message_id: &str,
) -> String {
    format!(
        "/teams/{}/channels/{}/messages/{}/unsetReaction",
        team_id.trim(),
        channel_id.trim(),
        message_id.trim()
    )
}

/// `POST .../messages/{id}/replies/{reply}/setReaction` path.
pub fn channel_reply_set_reaction_path(
    team_id: &str,
    channel_id: &str,
    message_id: &str,
    reply_id: &str,
) -> String {
    format!(
        "/teams/{}/channels/{}/messages/{}/replies/{}/setReaction",
        team_id.trim(),
        channel_id.trim(),
        message_id.trim(),
        reply_id.trim()
    )
}

/// `POST .../messages/{id}/replies/{reply}/unsetReaction` path.
pub fn channel_reply_unset_reaction_path(
    team_id: &str,
    channel_id: &str,
    message_id: &str,
    reply_id: &str,
) -> String {
    format!(
        "/teams/{}/channels/{}/messages/{}/replies/{}/unsetReaction",
        team_id.trim(),
        channel_id.trim(),
        message_id.trim(),
        reply_id.trim()
    )
}

/// `{"reactionType": <emoji>}` body. Graph takes the type as unicode
/// (v1.0 docs example), so the picker emoji itself is sent.
pub fn channel_react_body(emoji: &str) -> serde_json::Value {
    serde_json::json!({ "reactionType": emoji.trim() })
}

/// Set one picker-emoji reaction on a channel message (`reply_id` targets
/// a reply, `None` the root message). Unknown emoji and bad ids are
/// rejected before any network; success is 204 with no body.
pub async fn set_channel_reaction_data(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
    message_id: &str,
    emoji: &str,
    reply_id: Option<&str>,
) -> Result<()> {
    check_id("team_id", team_id)?;
    check_id("channel_id", channel_id)?;
    check_id("message_id", message_id)?;
    if super::chat::reaction_type_for_emoji(emoji.trim()).is_none() {
        bail!("unsupported reaction emoji: {}", emoji);
    }
    let path = match reply_id {
        Some(r) => {
            check_id("reply_id", r)?;
            channel_reply_set_reaction_path(team_id, channel_id, message_id, r)
        }
        None => channel_set_reaction_path(team_id, channel_id, message_id),
    };
    client.graph_post(&path, &channel_react_body(emoji)).await?;
    Ok(())
}

/// Unset one picker-emoji reaction on a channel message. Same arg
/// rules as [`set_channel_reaction_data`].
pub async fn unset_channel_reaction_data(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
    message_id: &str,
    emoji: &str,
    reply_id: Option<&str>,
) -> Result<()> {
    check_id("team_id", team_id)?;
    check_id("channel_id", channel_id)?;
    check_id("message_id", message_id)?;
    if super::chat::reaction_type_for_emoji(emoji.trim()).is_none() {
        bail!("unsupported reaction emoji: {}", emoji);
    }
    let path = match reply_id {
        Some(r) => {
            check_id("reply_id", r)?;
            channel_reply_unset_reaction_path(team_id, channel_id, message_id, r)
        }
        None => channel_unset_reaction_path(team_id, channel_id, message_id),
    };
    client.graph_post(&path, &channel_react_body(emoji)).await?;
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

// ---------------------------------------------------------------------------
// Team CREATE (om-jf-teamcreate): async Graph POST /teams
// ---------------------------------------------------------------------------

/// Seconds between async-operation polls.
pub const TEAM_CREATE_POLL_SECS: u64 = 3;
/// Give up polling after this many seconds (40 polls at 3s).
pub const TEAM_CREATE_TIMEOUT_SECS: u64 = 120;

/// POST path for template-based team creation.
pub fn create_team_path() -> &'static str {
    "/teams"
}

/// `teamsTemplates` binding for a plain standard team.
pub fn standard_team_template() -> &'static str {
    "https://graph.microsoft.com/v1.0/teamsTemplates('standard')"
}

/// POST body for team creation: template bind + `displayName`,
/// `description` only when non-blank, caller (`owner` user id) as the
/// owning member. Pure so tests pin it.
pub fn create_team_body(
    name: &str,
    description: Option<&str>,
    owner: &str,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "template@odata.bind": standard_team_template(),
        "displayName": name,
        "members": [{
            "@odata.type": "#microsoft.graph.aadUserConversationMember",
            "roles": ["owner"],
            "user@odata.bind": format!(
                "https://graph.microsoft.com/v1.0/users('{}')",
                owner.trim()
            ),
        }],
    });
    if let Some(d) = description {
        if !d.trim().is_empty() {
            body["description"] = serde_json::Value::String(d.to_string());
        }
    }
    body
}

/// One Graph `teamsAsyncOperation` poll response (only the fields the
/// create loop reads).
#[derive(Debug, Deserialize)]
pub struct TeamsAsyncOperation {
    pub status: String,
    #[serde(rename = "targetResourceId")]
    pub target_resource_id: Option<String>,
    #[serde(rename = "targetResourceLocation")]
    pub target_resource_location: Option<String>,
    pub error: Option<serde_json::Value>,
}

/// Terminal success (`succeeded`, case-insensitive).
pub fn operation_succeeded(status: &str) -> bool {
    status.eq_ignore_ascii_case("succeeded")
}

/// Terminal failure (`failed`, case-insensitive). Anything else
/// (`notStarted`/`inProgress`/unknown) keeps polling.
pub fn operation_failed(status: &str) -> bool {
    status.eq_ignore_ascii_case("failed")
}

/// Team id from a finished operation: `targetResourceId` first, else
/// the last segment of `targetResourceLocation` (`/teams('guid')` or
/// `/teams/guid` forms). `None` when neither is present.
pub fn operation_team_id(op: &TeamsAsyncOperation) -> Option<String> {
    if let Some(id) = op.target_resource_id.as_deref() {
        if !id.trim().is_empty() {
            return Some(id.trim().to_string());
        }
    }
    let loc = op.target_resource_location.as_deref()?.trim();
    if loc.is_empty() {
        return None;
    }
    // OData key form `/teams('guid')`: the id sits between quotes.
    if let Some(start) = loc.find('\'') {
        if let Some(end) = loc.rfind('\'') {
            if end > start + 1 {
                return Some(loc[start + 1..end].to_string());
            }
            return None;
        }
    }
    // Plain `/teams/guid` form: last path segment.
    let last = loc.rsplit('/').next()?.trim();
    let id = last.trim_matches(|c| c == '(' || c == ')');
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Absolute poll URL from a `Content-Location` header value: absolute
/// values pass through, relative paths expand under Graph v1.0.
pub fn operation_url(content_location: &str) -> String {
    let h = content_location.trim();
    if h.starts_with("http://") || h.starts_with("https://") {
        h.to_string()
    } else {
        format!("https://graph.microsoft.com/v1.0{}", h)
    }
}

/// Created team plus poll telemetry (`polls` operation GETs,
/// `elapsed_ms` wall time incl. POST).
pub struct TeamCreateResult {
    pub team: TeamInfo,
    pub polls: u32,
    pub elapsed_ms: u64,
}

/// Fetch one team + its channels (channel errors degrade to an empty
/// list, same as [`list_teams_data`]). `fallback_name` covers a missing
/// `displayName`.
async fn fetch_team_with_channels(
    client: &TeamsClient,
    team_id: &str,
    fallback_name: &str,
) -> Result<TeamInfo> {
    let team: Team = client
        .graph_get(&format!("/teams/{}", team_id))
        .await?
        .json()
        .await
        .context("Failed to parse created team response")?;
    let channels = match client
        .graph_get(&format!("/teams/{}/channels", team_id))
        .await
    {
        Ok(resp) => resp
            .json::<ChannelsResponse>()
            .await
            .map(|r| r.value.into_iter().map(channel_info).collect())
            .unwrap_or_default(),
        Err(e) => {
            tracing::warn!("Failed to fetch channels for new team: {:#}", e);
            Vec::new()
        }
    };
    Ok(TeamInfo {
        id: team.id,
        name: team
            .display_name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| fallback_name.trim().to_string()),
        channels,
    })
}

/// Create one standard team and wait for it (`POST /teams` answers 202
/// + `Content-Location`; the operation is polled every
/// [`TEAM_CREATE_POLL_SECS`]s until `succeeded`/`failed` or
/// [`TEAM_CREATE_TIMEOUT_SECS`]s elapse). The caller joins as owner.
/// Empty names are rejected before any network.
pub async fn create_team_data(
    client: &TeamsClient,
    name: &str,
    description: Option<&str>,
) -> Result<TeamCreateResult> {
    if name.trim().is_empty() {
        bail!("empty name");
    }
    let started = std::time::Instant::now();
    let me = super::me::whoami_data(client).await?;
    let body = create_team_body(name.trim(), description, &me.id);
    let resp = client.graph_post(create_team_path(), &body).await?;
    if resp.status() != reqwest::StatusCode::ACCEPTED {
        // Sync fallback: answer already carries the team.
        let team: Team = resp
            .json()
            .await
            .context("Failed to parse created team response")?;
        let id = team.id.clone();
        let team = fetch_team_with_channels(client, &id, name).await?;
        return Ok(TeamCreateResult {
            team,
            polls: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
    }
    let header = resp
        .headers()
        .get(reqwest::header::CONTENT_LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if header.trim().is_empty() {
        bail!("202 without Content-Location");
    }
    let url = operation_url(&header);
    let mut polls = 0u32;
    loop {
        if started.elapsed().as_secs() >= TEAM_CREATE_TIMEOUT_SECS {
            bail!(
                "team create timed out after {}s ({} polls); the team may still be creating",
                TEAM_CREATE_TIMEOUT_SECS,
                polls
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(TEAM_CREATE_POLL_SECS)).await;
        let op: TeamsAsyncOperation = client
            .graph_get_url(&url)
            .await?
            .json()
            .await
            .context("Failed to parse team operation response")?;
        polls += 1;
        if operation_succeeded(&op.status) {
            let id = operation_team_id(&op).context("operation succeeded without team id")?;
            let team = fetch_team_with_channels(client, &id, name).await?;
            return Ok(TeamCreateResult {
                team,
                polls,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
        if operation_failed(&op.status) {
            let detail = op
                .error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "failed".to_string());
            bail!("team create failed: {}", detail);
        }
    }
}

/// Middle-tier team create URL (`POST {mt}/beta/teams/create`, the web
/// client's `createTeam`). Unlike Graph `POST /teams` it needs no
/// `Team.Create` grant and answers synchronously.
pub fn mt_create_team_url(mt: &str) -> String {
    format!("{}/beta/teams/create", mt.trim_end_matches('/'))
}

/// Middle-tier team `accessType` (web client enum: None 0, Private 1,
/// Secret 2, Public 3). New teams are Private, the web client's default.
pub const TEAM_ACCESS_PRIVATE: u8 = 1;

/// Middle-tier create body as the web client builds it for a team from
/// scratch: `displayName`, `description` ("" when blank), `accessType`
/// Private, `isTenantWide` and `validationRequired` false. Pure.
pub fn mt_create_team_body(name: &str, description: Option<&str>) -> serde_json::Value {
    let desc = description.map(str::trim).filter(|d| !d.is_empty()).unwrap_or("");
    serde_json::json!({
        "displayName": name.trim(),
        "description": desc,
        "accessType": TEAM_ACCESS_PRIVATE,
        "isTenantWide": false,
        "validationRequired": false,
    })
}

/// Group id and team thread id from the middle-tier create answer. The
/// web client reads `value` and takes the group id from
/// `value.siteInfo.groupId` (`value.groupId` accepted too); the thread
/// id is `value.skypeThreadId`. `None` without a group id. Pure.
pub fn created_team_ids(v: &serde_json::Value) -> Option<(String, Option<String>)> {
    let value = v.get("value").unwrap_or(v);
    let text = |x: Option<&serde_json::Value>| {
        x.and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let group = text(value.pointer("/siteInfo/groupId")).or_else(|| text(value.get("groupId")))?;
    Some((group, text(value.get("skypeThreadId"))))
}

/// Create one standard (private) team through the Teams middle tier
/// (`POST {mt}/beta/teams/create`, synchronous answer carrying the group
/// id), then read it back with its channels from Graph. A fresh team
/// Graph does not list yet degrades to the id + name with no channels
/// (the next teams refresh fills them). The caller becomes the owner.
/// Empty names are rejected before any network. Alternative to
/// [`create_team_data`] for tokens without Graph `Team.Create`.
pub async fn create_team_via_middle_tier(
    client: &TeamsClient,
    name: &str,
    description: Option<&str>,
) -> Result<TeamCreateResult> {
    if name.trim().is_empty() {
        bail!("empty name");
    }
    let started = std::time::Instant::now();
    let url = mt_create_team_url(&client.middle_tier_url());
    let v: serde_json::Value = client
        .mt_send_json("POST", &url, &mt_create_team_body(name, description), None)
        .await?
        .json()
        .await
        .context("Failed to parse team create response")?;
    let (group_id, _thread) =
        created_team_ids(&v).context("team create answer carries no group id")?;
    let team = match fetch_team_with_channels(client, &group_id, name).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("New team not readable from Graph yet: {:#}", e);
            TeamInfo {
                id: group_id,
                name: name.trim().to_string(),
                channels: Vec::new(),
            }
        }
    };
    Ok(TeamCreateResult {
        team,
        polls: 0,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod mt_create_tests {
    use super::*;

    #[test]
    fn team_create_goes_to_middle_tier_with_web_client_body() {
        assert_eq!(
            mt_create_team_url("https://teams.microsoft.com/api/mt/emea/"),
            "https://teams.microsoft.com/api/mt/emea/beta/teams/create"
        );
        let b = mt_create_team_body("  Squad ", Some("  "));
        assert_eq!(b["displayName"], "Squad");
        assert_eq!(b["description"], "");
        assert_eq!(b["accessType"], 1);
        assert_eq!(b["isTenantWide"], false);
        assert_eq!(b["validationRequired"], false);
        assert_eq!(mt_create_team_body("S", Some(" Ship it "))["description"], "Ship it");
    }

    #[test]
    fn created_ids_come_from_site_info_or_group_id() {
        let v = serde_json::json!({"value": {"siteInfo": {"groupId": " g-1 "}, "skypeThreadId": "19:t@thread.tacv2"}});
        assert_eq!(created_team_ids(&v), Some(("g-1".into(), Some("19:t@thread.tacv2".into()))));
        let v = serde_json::json!({"value": {"groupId": "g-2"}});
        assert_eq!(created_team_ids(&v), Some(("g-2".into(), None)));
        assert_eq!(created_team_ids(&serde_json::json!({"value": {}})), None);
    }
}

/// Percent-encode one URL path segment like the web client's
/// `encodeURIComponent` (unreserved `A-Za-z0-9-_.~` kept).
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// True for a Teams thread id (`19:...@thread...`), the middle tier's
/// team and channel id form.
pub fn is_thread_id(id: &str) -> bool {
    let t = id.trim();
    t.starts_with("19:") && t.contains("@thread")
}

/// The team thread id from a Graph `team` answer (`internalId`). Pure.
pub fn team_internal_id(v: &serde_json::Value) -> Option<String> {
    v.get("internalId")
        .and_then(|s| s.as_str())
        .map(str::trim)
        .filter(|s| is_thread_id(s))
        .map(String::from)
}

/// Resolve the middle tier's team id (team thread id) from the Graph
/// group id the app lists teams by. Thread ids pass through; group ids
/// read Graph `GET /teams/{id}?$select=internalId`.
pub async fn team_thread_id(client: &TeamsClient, team_id: &str) -> Result<String> {
    if is_thread_id(team_id) {
        return Ok(team_id.trim().to_string());
    }
    check_id("team_id", team_id)?;
    let v: serde_json::Value = client
        .graph_get(&format!("/teams/{}?$select=internalId", team_id.trim()))
        .await?
        .json()
        .await
        .context("Failed to parse team response")?;
    team_internal_id(&v).context("the team's Teams id (internalId) is missing")
}

/// Middle-tier URL for one channel of one team (ids percent-encoded). Pure.
pub fn mt_channel_url(mt: &str, team_thread: &str, channel_id: &str) -> String {
    format!(
        "{}/beta/teams/{}/channels/{}",
        mt.trim_end_matches('/'),
        encode_component(team_thread.trim()),
        encode_component(channel_id.trim())
    )
}

/// DELETE body: the web client sends the channel's descriptor; this is
/// the subset known here (channel id, host team thread + group id).
/// Inferred minimal: the middle tier keys the delete on the URL.
pub fn mt_delete_channel_body(channel_id: &str, team_thread: &str, group_id: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "id": channel_id.trim(),
        "hostTeamId": team_thread.trim(),
        "isGeneral": false,
    });
    if !group_id.trim().is_empty() && !is_thread_id(group_id) {
        body["hostTeamGroupId"] = serde_json::Value::String(group_id.trim().to_string());
    }
    body
}

/// Rename a channel and/or change its description through the Teams
/// middle tier (`PATCH {mt}/beta/teams/{team}/channels/{channel}`, body
/// `displayName`/`description`, as [`update_channel_body`] builds it).
/// Alternative to [`update_channel_data`] for tokens without Graph
/// `ChannelSettings.ReadWrite.All`. An empty change is rejected before
/// any network; the service's refusal (e.g. renaming General) surfaces.
pub async fn update_channel_via_middle_tier(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<()> {
    check_id("team_id", team_id)?;
    check_id("channel_id", channel_id)?;
    let body = update_channel_body(name, description);
    if body.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        bail!("nothing to update");
    }
    let team = team_thread_id(client, team_id).await?;
    let url = mt_channel_url(&client.middle_tier_url(), &team, channel_id);
    client.mt_send_json("PATCH", &url, &body, None).await?;
    Ok(())
}

/// Delete one standard channel through the Teams middle tier (`DELETE
/// {mt}/beta/teams/{team}/channels/{channel}`); Teams keeps deleted
/// channels restorable for 30 days. Alternative to
/// [`delete_channel_data`] for tokens without Graph `Channel.Delete.All`.
/// The General channel (its id is the team's thread id) is refused before
/// any network, as are empty or path-breaking ids. Private/shared
/// channels use a separate provisioning service in the web client, not
/// wired here; the middle tier's answer surfaces as-is.
pub async fn delete_channel_via_middle_tier(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
) -> Result<()> {
    check_id("team_id", team_id)?;
    check_id("channel_id", channel_id)?;
    let team = team_thread_id(client, team_id).await?;
    if channel_id.trim() == team {
        bail!("The General channel can't be deleted");
    }
    let url = mt_channel_url(&client.middle_tier_url(), &team, channel_id);
    let body = mt_delete_channel_body(channel_id, &team, team_id);
    client.mt_send_json("DELETE", &url, &body, None).await?;
    Ok(())
}

#[cfg(test)]
mod mt_channel_tests {
    use super::*;

    #[test]
    fn channel_edit_delete_middle_tier_shapes() {
        assert!(is_thread_id("19:abc@thread.tacv2"));
        assert!(!is_thread_id("550e8400-e29b-41d4-a716-446655440000"));
        assert_eq!(
            team_internal_id(&serde_json::json!({"internalId": " 19:t@thread.tacv2 "})),
            Some("19:t@thread.tacv2".to_string())
        );
        assert_eq!(team_internal_id(&serde_json::json!({"internalId": "x"})), None);
        assert_eq!(team_internal_id(&serde_json::json!({})), None);
        assert_eq!(
            mt_channel_url("https://mt/", "19:t@thread.tacv2", "19:c@thread.tacv2"),
            "https://mt/beta/teams/19%3At%40thread.tacv2/channels/19%3Ac%40thread.tacv2"
        );
        assert_eq!(
            mt_delete_channel_body("19:c@thread.tacv2", "19:t@thread.tacv2", "group-1"),
            serde_json::json!({
                "id": "19:c@thread.tacv2",
                "hostTeamId": "19:t@thread.tacv2",
                "hostTeamGroupId": "group-1",
                "isGeneral": false,
            })
        );
        let no_group = mt_delete_channel_body("19:c@thread.tacv2", "19:t@thread.tacv2", "19:t@thread.tacv2");
        assert!(no_group.get("hostTeamGroupId").is_none());
    }
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
    fn react_paths_pin_docs_shapes() {
        assert_eq!(
            channel_set_reaction_path("t1", "19:ch@thread.tacv2", "m1"),
            "/teams/t1/channels/19:ch@thread.tacv2/messages/m1/setReaction"
        );
        assert_eq!(
            channel_unset_reaction_path("t1", "19:ch@thread.tacv2", "m1"),
            "/teams/t1/channels/19:ch@thread.tacv2/messages/m1/unsetReaction"
        );
        assert_eq!(
            channel_reply_set_reaction_path("t1", "19:ch@thread.tacv2", "m1", "r2"),
            "/teams/t1/channels/19:ch@thread.tacv2/messages/m1/replies/r2/setReaction"
        );
        assert_eq!(
            channel_reply_unset_reaction_path("t1", "19:ch@thread.tacv2", "m1", "r2"),
            "/teams/t1/channels/19:ch@thread.tacv2/messages/m1/replies/r2/unsetReaction"
        );
    }

    #[test]
    fn react_body_carries_unicode_reaction_type() {
        assert_eq!(
            channel_react_body("👍"),
            serde_json::json!({ "reactionType": "👍" })
        );
        assert_eq!(
            channel_react_body("  ❤️ "),
            serde_json::json!({ "reactionType": "❤️" })
        );
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
    fn channel_edit_delete_path_and_body() {
        assert_eq!(
            channel_path(" team-1 ", "19:abc@thread.tacv2"),
            "/teams/team-1/channels/19:abc@thread.tacv2"
        );
        assert_eq!(
            update_channel_body(Some("  Launch  "), None),
            serde_json::json!({"displayName": "Launch"})
        );
        assert_eq!(
            update_channel_body(None, Some(" Plans ")),
            serde_json::json!({"description": "Plans"})
        );
        assert_eq!(
            update_channel_body(Some("  "), Some("")),
            serde_json::json!({"description": ""})
        );
        assert_eq!(update_channel_body(Some(" "), None), serde_json::json!({}));
        assert!(check_id("channel_id", "19:abc@thread.tacv2").is_ok());
        assert!(check_id("channel_id", "19:a/b@thread.tacv2").is_err());
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

    #[test]
    fn team_create_pins_path_body_and_template() {
        assert_eq!(create_team_path(), "/teams");
        assert_eq!(
            standard_team_template(),
            "https://graph.microsoft.com/v1.0/teamsTemplates('standard')"
        );
        let body = create_team_body("  Squad  ", Some("Ship it"), "oid-1");
        assert_eq!(body["template@odata.bind"], standard_team_template());
        assert_eq!(body["displayName"], "  Squad  ");
        assert_eq!(body["description"], "Ship it");
        let members = body["members"].as_array().unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(
            members[0]["@odata.type"],
            "#microsoft.graph.aadUserConversationMember"
        );
        assert_eq!(members[0]["roles"], serde_json::json!(["owner"]));
        assert_eq!(
            members[0]["user@odata.bind"],
            "https://graph.microsoft.com/v1.0/users('oid-1')"
        );
        // Blank description is dropped, never sent as "".
        let bare = create_team_body("Squad", Some("   "), "oid-1");
        assert!(bare.get("description").is_none());
        let none = create_team_body("Squad", None, "oid-1");
        assert!(none.get("description").is_none());
    }

    #[test]
    fn team_create_poll_timing_constants() {
        assert_eq!(TEAM_CREATE_POLL_SECS, 3);
        assert_eq!(TEAM_CREATE_TIMEOUT_SECS, 120);
        assert!(TEAM_CREATE_TIMEOUT_SECS > TEAM_CREATE_POLL_SECS);
    }

    #[test]
    fn operation_status_classes_pin_terminal_states() {
        assert!(operation_succeeded("succeeded"));
        assert!(operation_succeeded("Succeeded"));
        assert!(!operation_succeeded("inProgress"));
        assert!(!operation_succeeded("failed"));
        assert!(operation_failed("failed"));
        assert!(operation_failed("Failed"));
        assert!(!operation_failed("inProgress"));
        assert!(!operation_failed("succeeded"));
        // Non-terminal polls continue.
        assert!(!operation_succeeded("notStarted"));
        assert!(!operation_failed("notStarted"));
        assert!(!operation_succeeded("inProgress"));
        assert!(!operation_failed("inProgress"));
    }

    #[test]
    fn operation_team_id_prefers_resource_id_then_location() {
        let op: TeamsAsyncOperation = serde_json::from_str(
            r#"{"status":"succeeded","targetResourceId":"guid-1",
                "targetResourceLocation":"/teams('guid-2')"}"#,
        )
        .unwrap();
        assert_eq!(operation_team_id(&op).as_deref(), Some("guid-1"));
        let loc: TeamsAsyncOperation = serde_json::from_str(
            r#"{"status":"succeeded","targetResourceLocation":"/teams('guid-9')"}"#,
        )
        .unwrap();
        assert_eq!(operation_team_id(&loc).as_deref(), Some("guid-9"));
        let bare: TeamsAsyncOperation = serde_json::from_str(
            r#"{"status":"succeeded","targetResourceLocation":"/teams/guid-7"}"#,
        )
        .unwrap();
        assert_eq!(operation_team_id(&bare).as_deref(), Some("guid-7"));
        let missing: TeamsAsyncOperation =
            serde_json::from_str(r#"{"status":"succeeded"}"#).unwrap();
        assert_eq!(operation_team_id(&missing), None);
    }

    #[test]
    fn operation_url_keeps_absolute_and_expands_relative() {
        assert_eq!(
            operation_url("https://graph.microsoft.com/v1.0/teams('t')/operations('o')"),
            "https://graph.microsoft.com/v1.0/teams('t')/operations('o')"
        );
        assert_eq!(
            operation_url("  /teams('t')/operations('o') "),
            "https://graph.microsoft.com/v1.0/teams('t')/operations('o')"
        );
    }
}
