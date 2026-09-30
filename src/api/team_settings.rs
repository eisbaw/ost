//! Team member permissions and the General channel id.
//!
//! Two read-only Graph GETs decide whether a non-owner may edit or
//! delete channels and which channel is the team's General:
//! `GET /teams/{id}` (`memberSettings.allowDeleteChannels`,
//! `allowCreateUpdateChannels`; full team resource, scope
//! Group.Read.All) and `GET /teams/{id}/primaryChannel` (scope
//! Channel.ReadBasic.All). Each read stands alone: one failing leaves
//! its fields `None` (unknown), never a guess.

use anyhow::{bail, Result};
use serde_json::Value;

use super::client::TeamsClient;

/// What the two reads found. `None` = unknown (read failed or the
/// field was absent).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamMemberSettings {
    pub allow_delete_channels: Option<bool>,
    pub allow_create_update_channels: Option<bool>,
    /// Id of the team's General (primary) channel.
    pub primary_channel_id: Option<String>,
}

/// Graph path of the full team resource (pure so tests pin it).
pub fn team_path(team_id: &str) -> String {
    format!("/teams/{}", team_id)
}

/// Graph path of the team's primary (General) channel.
pub fn primary_channel_path(team_id: &str) -> String {
    format!("/teams/{}/primaryChannel", team_id)
}

/// Pull `memberSettings.allowDeleteChannels` /
/// `allowCreateUpdateChannels` out of a team resource.
pub fn parse_member_settings(team: &Value) -> (Option<bool>, Option<bool>) {
    let ms = team.get("memberSettings");
    let flag = |k: &str| ms.and_then(|m| m.get(k)).and_then(Value::as_bool);
    (
        flag("allowDeleteChannels"),
        flag("allowCreateUpdateChannels"),
    )
}

/// Pull the channel id out of a primaryChannel response.
pub fn parse_primary_channel_id(ch: &Value) -> Option<String> {
    ch.get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn check_team_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("empty team_id");
    }
    if id.contains('/') || id.contains('?') || id.contains('#') || id.chars().any(|c| c.is_whitespace())
    {
        bail!("team_id must not contain '/', '?', '#' or whitespace");
    }
    Ok(())
}

/// Read the team's member permissions and General channel id. Fails
/// only when both reads fail (the first error is returned).
pub async fn team_settings_data(client: &TeamsClient, team_id: &str) -> Result<TeamMemberSettings> {
    check_team_id(team_id)?;
    let mut out = TeamMemberSettings::default();
    let mut first_err = None;
    let mut ok = 0;
    match client.graph_get(&team_path(team_id)).await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(v) => {
                let (d, e) = parse_member_settings(&v);
                out.allow_delete_channels = d;
                out.allow_create_update_channels = e;
                ok += 1;
            }
            Err(e) => first_err = Some(anyhow::Error::new(e)),
        },
        Err(e) => first_err = Some(e),
    }
    match client.graph_get(&primary_channel_path(team_id)).await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(v) => {
                out.primary_channel_id = parse_primary_channel_id(&v);
                ok += 1;
            }
            Err(e) => {
                first_err.get_or_insert(anyhow::Error::new(e));
            }
        },
        Err(e) => {
            first_err.get_or_insert(e);
        }
    }
    if ok == 0 {
        return Err(first_err.unwrap_or_else(|| anyhow::anyhow!("team settings unavailable")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_are_pinned() {
        assert_eq!(team_path("t1"), "/teams/t1");
        assert_eq!(primary_channel_path("t1"), "/teams/t1/primaryChannel");
    }

    #[test]
    fn member_settings_parse_both_flags() {
        let v = json!({"id":"t","memberSettings":{"allowCreateUpdateChannels":true,"allowDeleteChannels":false}});
        assert_eq!(parse_member_settings(&v), (Some(false), Some(true)));
    }

    #[test]
    fn missing_member_settings_are_unknown_not_false() {
        assert_eq!(parse_member_settings(&json!({"id":"t"})), (None, None));
        let v = json!({"memberSettings":{"allowDeleteChannels":true}});
        assert_eq!(parse_member_settings(&v), (Some(true), None));
    }

    #[test]
    fn primary_channel_id_parses_and_rejects_blank() {
        assert_eq!(
            parse_primary_channel_id(&json!({"id":"19:g@thread.tacv2"})).as_deref(),
            Some("19:g@thread.tacv2")
        );
        assert_eq!(parse_primary_channel_id(&json!({"id":""})), None);
        assert_eq!(parse_primary_channel_id(&json!({})), None);
    }

    #[test]
    fn bad_team_ids_are_refused_before_any_request() {
        assert!(check_team_id("").is_err());
        assert!(check_team_id("a/b").is_err());
        assert!(check_team_id("a b").is_err());
        assert!(check_team_id("abc-123").is_ok());
    }
}
