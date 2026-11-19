//! Channel moderation settings (MODERATION-FIX).
//!
//! Graph exposes channel moderation **beta-only**: `GET/PATCH
//! /teams/{id}/channels/{id}` with a `moderationSettings`
//! (`userNewMessageRestriction`: `everyone` | `everyoneExceptGuests` |
//! `moderators`; `replyRestriction`: `everyone` | `authorAndModerators`;
//! `allowNewMessageFromBots` / `allowNewMessageFromConnectors`).
//!
//! The read is a Graph beta GET (`moderationSettings`, beta-only). The
//! write is a chat-service thread-property PUT
//! (`PUT {chatService}/v1/threads/{channelThread}/properties?name=channelSettings`,
//! Skype-token auth like the web client's `setThreadPropertyChannelSettings`
//! resolver): the property is a JSON string
//! (`{"postPermissions":N,"allowReplies":N,...}`) with integer enums
//! (`postPermissions`: 0 everyone, 1 moderators, 2 everyoneExceptGuests;
//! `allowReplies`: 0 everyone, 1 authorAndModerators) — all live-proven
//! 11-19 on the trial tenant. Two transports were disproven the same
//! day: Graph beta PATCH is scope-walled for the Teams token (403, needs
//! `ChannelSettings.ReadWrite.All`), and the middle-tier channel PATCH
//! accepts a `channelSettings` envelope but silently ignores nested
//! moderation keys. The General channel (a "space thread") refuses the
//! write with 403 `AclCheckFailed`, so it is refused before any network.
//! A property that omits the bot/connector keys reads back blocked,
//! so absent keys are seeded from the beta read's effective flags
//! (per key; omitted there = allow, the tenant default) — a write
//! never flips flags it wasn't asked to change.
//! Moderators are the team's owners (implicit) plus any listed channel
//! moderators; the UI shows the owner list from the roster (read-only)
//! whenever posting is moderators-only.

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::client::TeamsClient;
use super::teams::team_thread_id;

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

/// Thread-property name carrying channel moderation on the chat
/// service (a JSON string; see [`parse_thread_settings`]).
pub const CHANNEL_SETTINGS_PROPERTY: &str = "channelSettings";

/// Thread-property keys for the bot/connector flags (1 allow, 0
/// block; live-proven, independent). A property that omits them reads
/// back blocked — so a write must seed, never drop, them.
pub const THREAD_BOTS_KEY: &str = "allowBotsPost";
pub const THREAD_CONNECTORS_KEY: &str = "allowConnectorsPost";

/// Map a `--posts` value to its `postPermissions` integer (live-proven:
/// 0 everyone, 1 moderators, 2 everyoneExceptGuests). Pure.
pub fn posts_perm(posts: &str) -> Result<u8> {
    match posts {
        POST_EVERYONE => Ok(0),
        POST_MODERATORS => Ok(1),
        POST_EXCEPT_GUESTS => Ok(2),
        _ => bail!("posts must be '{}', '{}' or '{}'", POST_EVERYONE, POST_EXCEPT_GUESTS, POST_MODERATORS),
    }
}

/// Map a `--replies` value to its `allowReplies` integer (live-proven:
/// 0 everyone, 1 authorAndModerators). Pure.
pub fn replies_perm(replies: &str) -> Result<u8> {
    match replies {
        REPLY_EVERYONE => Ok(0),
        REPLY_AUTHOR_MODERATORS => Ok(1),
        _ => bail!("replies must be '{}' or '{}'", REPLY_EVERYONE, REPLY_AUTHOR_MODERATORS),
    }
}

/// PUT target for one channel thread's moderation property. The id rides
/// raw (chat-service style, like the conversation-property URLs). Pure.
pub fn thread_settings_url(base: &str, channel_id: &str) -> String {
    format!(
        "{}/v1/threads/{}/properties?name={}",
        base.trim_end_matches('/'),
        channel_id.trim(),
        CHANNEL_SETTINGS_PROPERTY
    )
}

/// One channel thread's `channelSettings`, parsed from the property's
/// JSON string. Unknown extra keys ride along in `rest` so a write
/// preserves them. Pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadChannelSettings {
    pub post_permissions: u8,
    pub allow_replies: u8,
    pub rest: serde_json::Map<String, Value>,
}

/// Parse the property's JSON string; a missing/blank property means
/// defaults (a fresh channel). Malformed JSON bails (fail closed).
/// Pure so tests pin it.
pub fn parse_thread_settings(raw: Option<&str>) -> Result<ThreadChannelSettings> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty());
    let mut map = match raw {
        None => serde_json::Map::new(),
        Some(s) => serde_json::from_str::<Value>(s)
            .with_context(|| "channel thread settings are not valid JSON")?
            .as_object()
            .cloned()
            .with_context(|| "channel thread settings are not a JSON object")?,
    };
    let post_permissions = map.remove("postPermissions").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
    let allow_replies = map.remove("allowReplies").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
    Ok(ThreadChannelSettings { post_permissions, allow_replies, rest: map })
}

/// Render back to the property's JSON string (known fields first, then
/// preserved extras). Pure so tests pin it.
pub fn render_thread_settings(s: &ThreadChannelSettings) -> String {
    let mut map = serde_json::Map::with_capacity(s.rest.len() + 2);
    map.insert("postPermissions".to_string(), Value::from(s.post_permissions));
    map.insert("allowReplies".to_string(), Value::from(s.allow_replies));
    for (k, v) in &s.rest {
        map.insert(k.clone(), v.clone());
    }
    Value::Object(map).to_string()
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

/// Read one channel thread's `channelSettings` property (chat
/// service). A missing property parses to defaults (see
/// [`parse_thread_settings`]); the fetch itself must succeed.
pub async fn thread_settings_data(
    client: &TeamsClient,
    channel_id: &str,
) -> Result<ThreadChannelSettings> {
    check_pair("channel_id", channel_id)?;
    let v: Value = client
        .chat_get(&thread_settings_thread_url(&client.chat_service_url(), channel_id))
        .await?
        .json()
        .await
        .context("Failed to parse channel thread response")?;
    parse_thread_settings(
        v.get("properties")
            .and_then(|p| p.get(CHANNEL_SETTINGS_PROPERTY))
            .and_then(Value::as_str),
    )
}

/// GET target for one channel thread (chat service). Pure.
pub fn thread_settings_thread_url(base: &str, channel_id: &str) -> String {
    format!("{}/v1/threads/{}", base.trim_end_matches('/'), channel_id.trim())
}

/// Seed bot/connector keys the thread property omits. A property
/// without them reads back blocked (live-proven: a minimal PUT flips
/// bots/connectors allowed -> blocked), so absent keys are seeded from
/// the beta read's effective flags — per key, independently — and a
/// beta read that omits them (never set) seeds allow, the tenant
/// default. Keys already present are never touched (no extra read).
/// A failed beta read refuses the write: failing closed beats a
/// silent flag flip.
async fn seed_absent_bot_keys(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
    current: &mut ThreadChannelSettings,
) -> Result<()> {
    if current.rest.contains_key(THREAD_BOTS_KEY) && current.rest.contains_key(THREAD_CONNECTORS_KEY) {
        return Ok(());
    }
    let beta = moderation_data(client, team_id, channel_id)
        .await
        .context("couldn't read the channel's bot/connector flags, refusing the write")?;
    if !current.rest.contains_key(THREAD_BOTS_KEY) {
        current.rest.insert(
            THREAD_BOTS_KEY.to_string(),
            Value::from(u8::from(beta.allow_bots.unwrap_or(true))),
        );
    }
    if !current.rest.contains_key(THREAD_CONNECTORS_KEY) {
        current.rest.insert(
            THREAD_CONNECTORS_KEY.to_string(),
            Value::from(u8::from(beta.allow_connectors.unwrap_or(true))),
        );
    }
    Ok(())
}

/// Set one channel's moderation restrictions through the chat-service
/// thread property (read-modify-write, so bot/connector/pin flags are
/// preserved). At least one restriction must change; the General channel
/// (the team's own thread) is refused before any write — the service
/// ACLs it with 403 `AclCheckFailed`, live-proven. Any other service
/// refusal surfaces to the caller.
pub async fn update_moderation_data(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
    posts: Option<&str>,
    replies: Option<&str>,
) -> Result<()> {
    check_pair("team_id", team_id)?;
    check_pair("channel_id", channel_id)?;
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
    let team = team_thread_id(client, team_id).await?;
    if channel_id.trim() == team {
        bail!("The General channel's moderation can't be changed (the service allows it only on standard channels)");
    }
    let mut current = thread_settings_data(client, channel_id).await?;
    seed_absent_bot_keys(client, team_id, channel_id, &mut current).await?;
    if let Some(p) = posts {
        current.post_permissions = posts_perm(p)?;
    }
    if let Some(r) = replies {
        current.allow_replies = replies_perm(r)?;
    }
    let body = serde_json::json!({ CHANNEL_SETTINGS_PROPERTY: render_thread_settings(&current) });
    client
        .chat_put(&thread_settings_url(&client.chat_service_url(), channel_id), &body)
        .await?;
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
    fn bad_ids_are_refused_before_any_request() {
        assert!(check_pair("team_id", "").is_err());
        assert!(check_pair("team_id", "a/b").is_err());
        assert!(check_pair("channel_id", "a b").is_err());
        assert!(check_pair("team_id", "abc-123").is_ok());
    }

    #[test]
    fn perms_map_to_live_proven_ints() {
        assert_eq!(posts_perm("everyone").unwrap(), 0);
        assert_eq!(posts_perm("moderators").unwrap(), 1);
        assert_eq!(posts_perm("everyoneExceptGuests").unwrap(), 2);
        assert!(posts_perm("owners").is_err());
        assert_eq!(replies_perm("everyone").unwrap(), 0);
        assert_eq!(replies_perm("authorAndModerators").unwrap(), 1);
        assert!(replies_perm("nobody").is_err());
    }

    #[test]
    fn thread_urls_are_pinned() {
        assert_eq!(
            thread_settings_thread_url("https://h/", "19:c@thread.tacv2"),
            "https://h/v1/threads/19:c@thread.tacv2"
        );
        assert_eq!(
            thread_settings_url("https://h/", "19:c@thread.tacv2"),
            "https://h/v1/threads/19:c@thread.tacv2/properties?name=channelSettings"
        );
    }

    #[test]
    fn thread_settings_parse_render_round_trip() {
        let raw = "{\"postPermissions\":1,\"allowReplies\":1,\"allowPinPosts\":7,\"allowBotsPost\":1,\"allowConnectorsPost\":0}";
        let s = parse_thread_settings(Some(raw)).unwrap();
        assert_eq!(s.post_permissions, 1);
        assert_eq!(s.allow_replies, 1);
        let back: Value = serde_json::from_str(&render_thread_settings(&s)).unwrap();
        assert_eq!(
            back,
            json!({"postPermissions":1,"allowReplies":1,"allowPinPosts":7,"allowBotsPost":1,"allowConnectorsPost":0})
        );
    }

    #[test]
    fn thread_settings_missing_is_defaults_not_error() {
        for raw in [None, Some(""), Some("   ")] {
            let s = parse_thread_settings(raw).unwrap();
            assert_eq!(s.post_permissions, 0);
            assert_eq!(s.allow_replies, 0);
            assert!(s.rest.is_empty());
        }
        assert!(parse_thread_settings(Some("{oops")).is_err());
        assert!(parse_thread_settings(Some("[1,2]")).is_err());
    }
}
