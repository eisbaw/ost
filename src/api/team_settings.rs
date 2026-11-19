//! Team privacy switch: visibility on the backing M365 group.
//!
//! A team IS its group (same id): `GET /groups/{id}?$select=visibility`
//! reads it, `PATCH /groups/{id}` flips it. Private = invite-only,
//! Public = anyone in the org can join.

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::client::TeamsClient;

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

/// Graph path of the backing group (the team id IS the group id).
/// Pure so tests pin it.
pub fn visibility_path(team_id: &str) -> String {
    format!("/groups/{}", team_id.trim())
}

/// Read path (`$select` keeps the answer to the one property).
/// Pure so tests pin it.
pub fn visibility_read_path(team_id: &str) -> String {
    format!("{}?$select=visibility", visibility_path(team_id))
}

/// Pull `visibility` out of a group resource, lowercased for stable
/// compare (`Private`/`Public`/…). Absent or blank reads as `None`.
pub fn parse_visibility(group: &Value) -> Option<String> {
    group
        .get("visibility")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
}

/// PATCH body for a visibility flip. Pure so tests pin it.
pub fn visibility_body(private: bool) -> Value {
    serde_json::json!({ "visibility": if private { "Private" } else { "Public" } })
}

/// Read one team's privacy (its group's visibility). `None` = unknown
/// (read failed or the tenant omitted the field).
pub async fn team_visibility_data(client: &TeamsClient, team_id: &str) -> Result<Option<String>> {
    check_team_id(team_id)?;
    let v: Value = client
        .graph_get(&visibility_read_path(team_id))
        .await?
        .json()
        .await
        .context("Failed to parse group visibility response")?;
    Ok(parse_visibility(&v))
}

/// Flip one team's privacy. The service's refusal surfaces to the
/// caller; read the visibility back to confirm.
pub async fn update_team_visibility_data(
    client: &TeamsClient,
    team_id: &str,
    private: bool,
) -> Result<()> {
    check_team_id(team_id)?;
    client
        .graph_patch(&visibility_path(team_id), &visibility_body(private))
        .await?;
    Ok(())
}

/// Print one team's privacy (read-only CLI).
pub async fn show_visibility(team_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let v = team_visibility_data(&client, team_id).await?;
    println!(
        "Team {} is {}.",
        team_id.trim(),
        match v.as_deref() {
            Some("private") => "Private (invite-only)",
            Some("public") => "Public (anyone in the org can join)",
            Some(other) => other,
            None => "unknown (visibility unreadable)",
        }
    );
    Ok(())
}

/// Flip one team's privacy, then read it back.
pub async fn set_visibility(team_id: &str, private: bool) -> Result<()> {
    let client = TeamsClient::new().await?;
    update_team_visibility_data(&client, team_id, private).await?;
    show_visibility(team_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bad_ids_are_refused() {
        assert!(check_team_id("").is_err());
        assert!(check_team_id("a b").is_err());
        assert!(check_team_id("abc-123").is_ok());
    }

    #[test]
    fn visibility_paths_are_pinned() {
        assert_eq!(visibility_path("t1"), "/groups/t1");
        assert_eq!(visibility_read_path("t1"), "/groups/t1?$select=visibility");
    }

    #[test]
    fn visibility_parses_case_insensitively() {
        assert_eq!(parse_visibility(&json!({"visibility":"Private"})).as_deref(), Some("private"));
        assert_eq!(parse_visibility(&json!({"visibility":"Public"})).as_deref(), Some("public"));
        assert_eq!(parse_visibility(&json!({})), None);
        assert_eq!(parse_visibility(&json!({"visibility":""})), None);
    }

    #[test]
    fn visibility_body_flips() {
        assert_eq!(visibility_body(true), json!({"visibility":"Private"}));
        assert_eq!(visibility_body(false), json!({"visibility":"Public"}));
    }
}
