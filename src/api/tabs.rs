//! Channel tabs via Microsoft Graph (read-only).
//!
//! `GET /teams/{team}/channels/{channel}/tabs` lists the pinned tabs
//! (Posts, Files, Notes, website tabs, ...). Callers pass only the channel
//! id; the owning team resolves via a joinedTeams scan.
//! Auth: existing Graph token, no scope widening. No tab content is
//! fetched here — only link-out targets (IDs + URLs) are returned.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

// -- Wire types --

#[derive(Debug, Deserialize)]
struct TabsResponse {
    value: Vec<WireTab>,
}

#[derive(Debug, Deserialize)]
struct WireTab {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "webUrl")]
    web_url: Option<String>,
    #[serde(rename = "teamsAppId")]
    teams_app_id: Option<String>,
    /// v1.0 has no `teamsAppId`; `$expand=teamsApp` carries the app id
    /// (live tabs came back with no app id at all otherwise).
    #[serde(rename = "teamsApp")]
    teams_app: Option<WireTeamsApp>,
    configuration: Option<WireTabConfig>,
}

#[derive(Debug, Deserialize)]
struct WireTeamsApp {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireTabConfig {
    #[serde(rename = "contentUrl")]
    content_url: Option<String>,
    #[serde(rename = "websiteUrl")]
    website_url: Option<String>,
    #[serde(rename = "entityId")]
    entity_id: Option<String>,
}

// -- Public model --

/// One pinned channel tab: identity + link-out targets only.
/// Well-known tabs (Posts/Files/Notes) are identified by name/app id;
/// anything with a URL can open in the browser. No content renderers.
pub struct TabInfo {
    pub id: String,
    pub name: String,
    pub app_id: Option<String>,
    pub content_url: Option<String>,
    pub website_url: Option<String>,
    /// Tab configuration `entityId` (native TeamsJS hosting context).
    pub entity_id: Option<String>,
}

fn tab_from_wire(tab: WireTab) -> TabInfo {
    let name = tab.display_name.unwrap_or_else(|| tab.id.clone());
    let (content_url, website_url, entity_id) = match tab.configuration {
        Some(c) => (c.content_url, c.website_url.or(tab.web_url), c.entity_id),
        None => (None, tab.web_url, None),
    };
    TabInfo {
        id: tab.id,
        name,
        app_id: tab.teams_app_id.or_else(|| tab.teams_app.and_then(|a| a.id)).filter(|s| !s.is_empty()),
        content_url,
        website_url,
        entity_id,
    }
}

// -- Data-returning API function --

/// List a channel's pinned tabs. Empty `channel_id` bails pre-network;
/// unknown channels (no joined team contains them) bail after the scan.
pub async fn list_tabs_data(client: &TeamsClient, channel_id: &str) -> Result<Vec<TabInfo>> {
    let channel_id = channel_id.trim();
    if channel_id.is_empty() {
        bail!("empty channel_id");
    }
    let team_id = find_team_for_channel(client, channel_id).await?;
    let path = format!("/teams/{}/channels/{}/tabs?$expand=teamsApp", team_id, channel_id);
    let resp = client.graph_get(&path).await?;
    let parsed: TabsResponse = resp.json().await.context("Failed to parse tabs response")?;
    Ok(parsed.value.into_iter().map(tab_from_wire).collect())
}

async fn find_team_for_channel(client: &TeamsClient, channel_id: &str) -> Result<String> {
    let teams = crate::api::list_teams_data(client).await?;
    for team in &teams {
        if team.channels.iter().any(|c| c.id == channel_id) {
            return Ok(team.id.clone());
        }
    }
    bail!("No joined team contains channel {}", channel_id)
}

// -- CLI entry point (prints to stdout) --

/// List a channel's tabs (prints to stdout).
pub async fn list_tabs(channel_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let tabs = list_tabs_data(&client, channel_id).await?;

    println!("\nChannel Tabs:");
    println!("{:-<60}", "");
    print_tabs(&tabs);
    Ok(())
}

fn print_tabs(tabs: &[TabInfo]) {
    if tabs.is_empty() {
        println!("  (no tabs found)");
        return;
    }
    for t in tabs {
        println!("  {:<30} {}", t.name, t.id);
        if let Some(ref u) = t.website_url.as_ref().or(t.content_url.as_ref()) {
            println!("    {}", u);
        }
    }
}

// -- Tabs-all dump (single-auth, machine-splittable sections) --

/// Section header for the tabs-all dump: `=== team | channel | id`.
/// Team/channel names never contain `|` from Graph; consumers split on it.
pub fn tabs_all_section_header(team: &str, channel: &str, channel_id: &str) -> String {
    format!("=== {} | {} | {}", team, channel, channel_id)
}

/// Dump every joined channel's tabs (one auth, read-only). Same per-tab
/// lines as `list_tabs`; sections split on `=== ` headers. One slow call
/// (~1 Graph round-trip per channel) beats N CLI spawns (N auth setups).
pub async fn list_tabs_all() -> Result<()> {
    let client = TeamsClient::new().await?;
    let teams = crate::api::list_teams_data(&client).await?;
    for team in &teams {
        for channel in &team.channels {
            println!(
                "{}",
                tabs_all_section_header(&team.name, &channel.name, &channel.id)
            );
            let path = format!("/teams/{}/channels/{}/tabs?$expand=teamsApp", team.id, channel.id);
            let resp = client.graph_get(&path).await?;
            let parsed: TabsResponse =
                resp.json().await.context("Failed to parse tabs response")?;
            print_tabs(
                &parsed
                    .value
                    .into_iter()
                    .map(tab_from_wire)
                    .collect::<Vec<_>>(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_parses_builtin_and_web_tabs() {
        let parsed: TabsResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"t-posts","displayName":"Posts"},
                {"id":"t-files","displayName":"Files",
                 "teamsAppId":"com.microsoft.teamspace.tab.files.sharepoint"},
                {"id":"t-notes","displayName":"Notes",
                 "webUrl":"https://example.sharepoint.com/notes",
                 "configuration":{"websiteUrl":"https://example.com/notebook"}},
                {"id":"t-web","displayName":"Dashboard",
                 "configuration":{"contentUrl":"https://example.com/app"}},
                {"id":"t-bare"}]}"#,
        )
        .unwrap();
        assert_eq!(parsed.value.len(), 5);
        let tabs: Vec<TabInfo> = parsed.value.into_iter().map(tab_from_wire).collect();
        assert_eq!(tabs[0].name, "Posts");
        assert!(tabs[0].website_url.is_none());
        assert_eq!(
            tabs[1].app_id.as_deref(),
            Some("com.microsoft.teamspace.tab.files.sharepoint")
        );
        // configuration.websiteUrl wins over top-level webUrl.
        assert_eq!(
            tabs[2].website_url.as_deref(),
            Some("https://example.com/notebook")
        );
        assert_eq!(
            tabs[3].content_url.as_deref(),
            Some("https://example.com/app")
        );
        // Missing displayName falls back to the tab id.
        assert_eq!(tabs[4].name, "t-bare");
    }

    #[test]
    fn section_header_round_trips_pipes() {
        assert_eq!(
            tabs_all_section_header("IS OPS", "General", "19:abc@thread.skype"),
            "=== IS OPS | General | 19:abc@thread.skype"
        );
    }

    #[test]
    fn web_url_fallback_without_configuration() {
        let parsed: TabsResponse = serde_json::from_str(
            r#"{"value":[{"id":"t1","displayName":"Wiki",
                "webUrl":"https://example.com/wiki"}]}"#,
        )
        .unwrap();
        let tabs: Vec<TabInfo> = parsed.value.into_iter().map(tab_from_wire).collect();
        assert_eq!(
            tabs[0].website_url.as_deref(),
            Some("https://example.com/wiki")
        );
    }

    #[test]
    fn app_id_from_expanded_teams_app() {
        let parsed: TabsResponse = serde_json::from_str(
            r#"{"value":[{"id":"t1","displayName":"Plan","teamsApp":{"id":"com.microsoft.teamspace.tab.planner"},
               "configuration":{"contentUrl":"https://tasks.office.com/x","entityId":"e1"}},
              {"id":"t2","displayName":"Old","teamsAppId":"app-2","teamsApp":{"id":"ignored"}}]}"#,
        )
        .unwrap();
        let tabs: Vec<TabInfo> = parsed.value.into_iter().map(tab_from_wire).collect();
        assert_eq!(tabs[0].app_id.as_deref(), Some("com.microsoft.teamspace.tab.planner"));
        assert_eq!(tabs[1].app_id.as_deref(), Some("app-2"));
    }
}
