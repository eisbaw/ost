//! Channel moderation settings.
//!
//! Graph exposes channel moderation **beta-only**: `GET/PATCH
//! /teams/{id}/channels/{id}` with a `moderationSettings`
//! (`userNewMessageRestriction`: `everyone` | `everyoneExceptGuests` |
//! `moderators`; `replyRestriction`: `everyone` | `authorAndModerators`;
//! `allowNewMessageFromBots` / `allowNewMessageFromConnectors`). The
//! v1.0 channel PATCH covers `displayName`/`description` only, and the
//! v1.0 channel PATCH route is denied for the Teams token
//! (`ChannelSettings.ReadWrite.All`), so — like the rename/description
//! edit — the moderation write goes through the proven Teams middle
//! tier channel PATCH (`mt_channel_url` + `mt_send_json`). The read is a
//! beta Graph GET.

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::client::TeamsClient;
use super::teams::{mt_channel_url, team_thread_id};

/// Who may start new posts: `everyone` (default), `everyoneExceptGuests`,
/// or `moderators` only.
pub const POST_EVERYONE: &str = "everyone";
pub const POST_EXCEPT_GUESTS: &str = "everyoneExceptGuests";
pub const POST_MODERATORS: &str = "moderators";

/// Who may reply: `everyone` (default) or `authorAndModerators`.
pub const REPLY_EVERYONE: &str = "everyone";
pub const REPLY_AUTHOR_MODERATORS: &str = "authorAndModerators";

/// One channel's moderation settings. `None` = unknown (read failed or
/// the tenant omitted the field); posting defaults to everyone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelModeration {
    pub user_new_message_restriction: Option<String>,
    pub reply_restriction: Option<String>,
    pub allow_bots: Option<bool>,
    pub allow_connectors: Option<bool>,
}

/// Graph (beta) path of one channel. Pure so tests pin it.
pub fn moderation_path(team_id: &str, channel_id: &str) -> String {
    format!(
        "/teams/{}/channels/{}",
        team_id.trim(),
        channel_id.trim()
    )
}

/// Beta read path (`$select` keeps the answer to the one property).
/// Pure so tests pin it.
pub fn moderation_read_path(team_id: &str, channel_id: &str) -> String {
    format!("{}?$select=moderationSettings", moderation_path(team_id, channel_id))
}

/// Pull the moderation settings out of a channel resource. A missing
/// `moderationSettings` means unmoderated (all `None`), never an error.
pub fn parse_moderation_settings(channel: &Value) -> ChannelModeration {
    let ms = channel.get("moderationSettings");
    let text = |k: &str| {
        ms.and_then(|m| m.get(k))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let flag = |k: &str| ms.and_then(|m| m.get(k)).and_then(Value::as_bool);
    ChannelModeration {
        user_new_message_restriction: text("userNewMessageRestriction"),
        reply_restriction: text("replyRestriction"),
        allow_bots: flag("allowNewMessageFromBots"),
        allow_connectors: flag("allowNewMessageFromConnectors"),
    }
}

fn check_pair(what: &str, id: &str) -> Result<()> {
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

fn check_posts(v: &str) -> Result<()> {
    match v {
        POST_EVERYONE | POST_EXCEPT_GUESTS | POST_MODERATORS => Ok(()),
        _ => bail!("posts must be '{}', '{}' or '{}'", POST_EVERYONE, POST_EXCEPT_GUESTS, POST_MODERATORS),
    }
}

fn check_replies(v: &str) -> Result<()> {
    match v {
        REPLY_EVERYONE | REPLY_AUTHOR_MODERATORS => Ok(()),
        _ => bail!("replies must be '{}' or '{}'", REPLY_EVERYONE, REPLY_AUTHOR_MODERATORS),
    }
}

/// PATCH body for a moderation change: only the restrictions that
/// change, under `moderationSettings`. At least one must be `Some`
/// (validated here; blank strings are rejected before any network).
/// Pure so tests pin it.
pub fn moderation_body(posts: Option<&str>, replies: Option<&str>) -> Result<Value> {
    let posts = posts.map(str::trim).filter(|s| !s.is_empty());
    let replies = replies.map(str::trim).filter(|s| !s.is_empty());
    if let Some(p) = posts {
        check_posts(p)?;
    }
    if let Some(r) = replies {
        check_replies(r)?;
    }
    if posts.is_none() && replies.is_none() {
        bail!("nothing to update");
    }
    let mut inner = serde_json::json!({});
    if let Some(p) = posts {
        inner["userNewMessageRestriction"] = Value::String(p.to_string());
    }
    if let Some(r) = replies {
        inner["replyRestriction"] = Value::String(r.to_string());
    }
    Ok(serde_json::json!({ "moderationSettings": inner }))
}

/// Read one channel's moderation settings (beta Graph GET). Fails only
/// when the read itself fails; a missing property parses to all-`None`.
pub async fn moderation_data(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
) -> Result<ChannelModeration> {
    check_pair("team_id", team_id)?;
    check_pair("channel_id", channel_id)?;
    let v: Value = client
        .graph_beta_get(&moderation_read_path(team_id, channel_id))
        .await?
        .json()
        .await
        .context("Failed to parse channel moderation response")?;
    Ok(parse_moderation_settings(&v))
}

/// Set one channel's moderation restrictions through the Teams middle
/// tier (same PATCH transport as the rename/description edit). At
/// least one restriction must change; the service's refusal surfaces
/// to the caller.
pub async fn update_moderation_data(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
    posts: Option<&str>,
    replies: Option<&str>,
) -> Result<()> {
    check_pair("team_id", team_id)?;
    check_pair("channel_id", channel_id)?;
    let body = moderation_body(posts, replies)?;
    let team = team_thread_id(client, team_id).await?;
    let url = mt_channel_url(&client.middle_tier_url(), &team, channel_id);
    client.mt_send_json("PATCH", &url, &body, None).await?;
    Ok(())
}

/// Print one channel's moderation settings (read-only CLI).
pub async fn show_moderation(team_id: &str, channel_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let m = moderation_data(&client, team_id, channel_id).await?;
    println!("Moderation for channel {} (team {}):", channel_id.trim(), team_id.trim());
    println!(
        "  who can post:  {}",
        m.user_new_message_restriction.as_deref().unwrap_or("everyone (default)")
    );
    println!(
        "  who can reply: {}",
        m.reply_restriction.as_deref().unwrap_or("everyone (default)")
    );
    if let Some(b) = m.allow_bots {
        println!("  bots:        {}", if b { "allowed" } else { "blocked" });
    }
    if let Some(c) = m.allow_connectors {
        println!("  connectors:  {}", if c { "allowed" } else { "blocked" });
    }
    Ok(())
}

/// Set one channel's moderation restrictions, then read them back.
pub async fn set_moderation(
    team_id: &str,
    channel_id: &str,
    posts: Option<&str>,
    replies: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    update_moderation_data(&client, team_id, channel_id, posts, replies).await?;
    show_moderation(team_id, channel_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_are_pinned() {
        assert_eq!(moderation_path("t1", "c1"), "/teams/t1/channels/c1");
        assert_eq!(
            moderation_read_path("t1", "c1"),
            "/teams/t1/channels/c1?$select=moderationSettings"
        );
    }

    #[test]
    fn moderation_parses_full_shape() {
        let v = json!({"id":"c","moderationSettings":{
            "userNewMessageRestriction":"moderators",
            "replyRestriction":"authorAndModerators",
            "allowNewMessageFromBots":false,
            "allowNewMessageFromConnectors":true}});
        assert_eq!(
            parse_moderation_settings(&v),
            ChannelModeration {
                user_new_message_restriction: Some("moderators".into()),
                reply_restriction: Some("authorAndModerators".into()),
                allow_bots: Some(false),
                allow_connectors: Some(true),
            }
        );
    }

    #[test]
    fn missing_moderation_is_unmoderated_not_error() {
        assert_eq!(parse_moderation_settings(&json!({"id":"c"})), ChannelModeration::default());
        let v = json!({"moderationSettings":{"replyRestriction":"everyone"}});
        let m = parse_moderation_settings(&v);
        assert_eq!(m.reply_restriction.as_deref(), Some("everyone"));
        assert_eq!(m.user_new_message_restriction, None);
    }

    #[test]
    fn body_carries_only_changes() {
        let b = moderation_body(Some("moderators"), None).unwrap();
        assert_eq!(
            b,
            json!({"moderationSettings":{"userNewMessageRestriction":"moderators"}})
        );
        let b = moderation_body(None, Some("authorAndModerators")).unwrap();
        assert_eq!(
            b,
            json!({"moderationSettings":{"replyRestriction":"authorAndModerators"}})
        );
    }

    #[test]
    fn body_rejects_bad_values_and_empty_change() {
        assert!(moderation_body(Some("owners"), None).is_err());
        assert!(moderation_body(None, Some("nobody")).is_err());
        assert!(moderation_body(None, None).is_err());
        assert!(moderation_body(Some("  "), Some("")).is_err());
    }

    #[test]
    fn bad_ids_are_refused_before_any_request() {
        assert!(check_pair("team_id", "").is_err());
        assert!(check_pair("team_id", "a/b").is_err());
        assert!(check_pair("channel_id", "a b").is_err());
        assert!(check_pair("team_id", "abc-123").is_ok());
    }
}
