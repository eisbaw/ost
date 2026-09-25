//! Teams message search (Graph `POST /search/query`, entity `chatMessage`).
//!
//! OstMac (om-ja-search): free-text search over the signed-in user's Teams
//! messages, ranked by Graph. No auth scope change: the existing Graph token
//! (`/.default`) carries the delegated chat scopes; a 403 surfaces as the
//! call's detail. Shape per
//! `learn.microsoft.com/graph/search-concept-chat-messages`.
//!
//! Paging is `from`/`size` (Graph caps `size` at 25): `next_from` chains the
//! next page while `more` holds.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

/// Graph's per-request hit cap for chatMessage search.
pub const SEARCH_MAX_SIZE: usize = 25;

/// One message hit: enough to render a row and jump to the bubble.
/// `chat_id` is the conversation to open — the chat thread id, or the
/// channel id for channel messages (which carry no `chatId`).
pub struct SearchHitInfo {
    pub message_id: String,
    pub chat_id: String,
    pub team_id: Option<String>,
    pub channel_id: Option<String>,
    pub sender: String,
    pub timestamp: String,
    /// Graph `summary` verbatim (plain text with `...` trims).
    pub preview: String,
    pub subject: Option<String>,
}

/// One page of hits plus the cursor for the next page.
pub struct SearchPage {
    pub hits: Vec<SearchHitInfo>,
    /// Server `total`, when reported.
    pub total: Option<i64>,
    /// Server `moreResultsAvailable`.
    pub more: bool,
}

// ---------------------------------------------------------------------------
// Wire shapes (tolerant: every field optional but the walk below)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct SearchResp {
    #[serde(default)]
    value: Vec<SearchRespValue>,
}

#[derive(Debug, Deserialize, Default)]
struct SearchRespValue {
    #[serde(rename = "hitsContainers", default)]
    hits_containers: Vec<HitsContainer>,
}

#[derive(Debug, Deserialize, Default)]
struct HitsContainer {
    #[serde(default)]
    hits: Vec<Hit>,
    total: Option<i64>,
    #[serde(rename = "moreResultsAvailable", default)]
    more: bool,
}

#[derive(Debug, Deserialize, Default)]
struct Hit {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    resource: Option<Resource>,
}

#[derive(Debug, Deserialize, Default)]
struct Resource {
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "createdDateTime", default)]
    created: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    from: Option<From>,
    #[serde(rename = "chatId", default)]
    chat_id: Option<String>,
    #[serde(rename = "channelIdentity", default)]
    channel: Option<ChannelIdent>,
}

#[derive(Debug, Deserialize, Default)]
struct From {
    #[serde(rename = "emailAddress", default)]
    email: Option<Email>,
}

#[derive(Debug, Deserialize, Default)]
struct Email {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    address: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ChannelIdent {
    #[serde(rename = "teamId", default)]
    team_id: Option<String>,
    #[serde(rename = "channelId", default)]
    channel_id: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.and_then(|v| {
        if v.trim().is_empty() {
            None
        } else {
            Some(v)
        }
    })
}

fn hit_info(h: Hit) -> Option<SearchHitInfo> {
    let r = h.resource?;
    let message_id = non_empty(r.id)?;
    // Chats carry `chatId`; channel messages carry only `channelIdentity`.
    let channel_id = r.channel.as_ref().and_then(|c| non_empty(c.channel_id.clone()));
    let chat_id = non_empty(r.chat_id).or(channel_id.clone())?;
    let sender = r
        .from
        .as_ref()
        .and_then(|f| f.email.as_ref())
        .and_then(|e| non_empty(e.name.clone()).or(non_empty(e.address.clone())))
        .unwrap_or_default();
    Some(SearchHitInfo {
        message_id,
        chat_id,
        team_id: r
            .channel
            .as_ref()
            .and_then(|c| non_empty(c.team_id.clone())),
        channel_id,
        sender,
        timestamp: r.created.unwrap_or_default(),
        preview: h.summary.unwrap_or_default(),
        subject: non_empty(r.subject),
    })
}

/// Parse one Graph search response into a page. Unknown shapes yield an
/// empty page (never fatal); hits without a message id or an openable
/// conversation are skipped.
pub fn parse_search_response(value: &serde_json::Value) -> SearchPage {
    let resp: SearchResp = serde_json::from_value(value.clone()).unwrap_or_default();
    let first = resp
        .value
        .into_iter()
        .flat_map(|v| v.hits_containers)
        .next();
    match first {
        None => SearchPage {
            hits: Vec::new(),
            total: None,
            more: false,
        },
        Some(c) => SearchPage {
            hits: c.hits.into_iter().filter_map(hit_info).collect(),
            total: c.total,
            more: c.more,
        },
    }
}

/// Request body for one `from`/`size` window over `query`.
pub fn search_body(query: &str, from: usize, size: usize) -> serde_json::Value {
    serde_json::json!({
        "requests": [{
            "entityTypes": ["chatMessage"],
            "query": { "queryString": query },
            "from": from,
            "size": clamp_size(size),
        }]
    })
}

/// Clamp a page size into Graph's `1..=25` window.
pub fn clamp_size(size: usize) -> usize {
    size.clamp(1, SEARCH_MAX_SIZE)
}

/// Cursor for the next page: `from + hits` while the server reports more.
/// `None` ends paging (exhausted or an empty page — never spin on it).
pub fn next_from(from: usize, page: &SearchPage) -> Option<usize> {
    if page.more && !page.hits.is_empty() {
        Some(from + page.hits.len())
    } else {
        None
    }
}

/// Search Teams messages, one `from`/`size` window. Empty queries are
/// rejected before any network.
pub async fn search_messages_data(
    client: &TeamsClient,
    query: &str,
    from: usize,
    size: usize,
) -> Result<SearchPage> {
    if query.trim().is_empty() {
        bail!("empty query");
    }
    let body = search_body(query, from, size);
    let resp = client.graph_post("/search/query", &body).await?;
    let value: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse search response")?;
    Ok(parse_search_response(&value))
}

/// Search Teams messages (prints the first window to stdout).
pub async fn search_messages(query: &str, limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let page = search_messages_data(&client, query, 0, limit).await?;

    println!("\nSearch results for {:?}:", query);
    println!("{:-<60}", "");

    if page.hits.is_empty() {
        println!("  (no messages found)");
        return Ok(());
    }

    for hit in &page.hits {
        println!("{} — {}", hit.sender, hit.timestamp);
        println!("  chat: {}", hit.chat_id);
        println!("  msg:  {}", hit.message_id);
        println!("  {}", hit.preview);
        println!();
    }

    if let Some(total) = page.total {
        println!("(showing {} of {})", page.hits.len(), total);
    }
    if page.more {
        println!("(more available — page with --from)");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat_value() -> serde_json::Value {
        json!({
            "value": [{
                "searchTerms": ["ship"],
                "hitsContainers": [{
                    "hits": [{
                        "hitId": "H1",
                        "rank": 1,
                        "summary": "...Ship it...",
                        "resource": {
                            "@odata.type": "microsoft.graph.chatMessage",
                            "id": "1758600000000",
                            "createdDateTime": "2026-09-22T09:12:05Z",
                            "subject": "",
                            "from": {"emailAddress": {"name": "Megan Harper", "address": "megan@x"}},
                            "channelIdentity": {},
                            "chatId": "19:chat1@thread.v2"
                        }
                    }],
                    "total": 1,
                    "moreResultsAvailable": false
                }]
            }]
        })
    }

    #[test]
    fn body_shape_matches_graph_contract() {
        let body = search_body("ship it", 25, 50);
        assert_eq!(
            body,
            json!({
                "requests": [{
                    "entityTypes": ["chatMessage"],
                    "query": { "queryString": "ship it" },
                    "from": 25,
                    "size": 25,
                }]
            })
        );
    }

    #[test]
    fn size_clamps_to_graph_window() {
        assert_eq!(clamp_size(0), 1);
        assert_eq!(clamp_size(10), 10);
        assert_eq!(clamp_size(25), 25);
        assert_eq!(clamp_size(500), 25);
    }

    #[test]
    fn parse_chat_hit() {
        let page = parse_search_response(&chat_value());
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.total, Some(1));
        assert!(!page.more);
        let h = &page.hits[0];
        assert_eq!(h.message_id, "1758600000000");
        assert_eq!(h.chat_id, "19:chat1@thread.v2");
        assert!(h.team_id.is_none());
        assert!(h.channel_id.is_none());
        assert_eq!(h.sender, "Megan Harper");
        assert_eq!(h.timestamp, "2026-09-22T09:12:05Z");
        assert_eq!(h.preview, "...Ship it...");
        assert!(h.subject.is_none()); // blank subject drops
    }

    #[test]
    fn parse_channel_hit_opens_by_channel() {
        let value = json!({
            "value": [{
                "hitsContainers": [{
                    "hits": [{
                        "summary": "...lane...",
                        "resource": {
                            "id": "m9",
                            "createdDateTime": "2026-09-22T09:13:05Z",
                            "from": {"emailAddress": {"address": "tom@x"}},
                            "channelIdentity": {"teamId": "t1", "channelId": "19:chan@thread.tacv2"}
                        }
                    }],
                    "total": 9,
                    "moreResultsAvailable": true
                }]
            }]
        });
        let page = parse_search_response(&value);
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.total, Some(9));
        assert!(page.more);
        let h = &page.hits[0];
        assert_eq!(h.chat_id, "19:chan@thread.tacv2");
        assert_eq!(h.team_id.as_deref(), Some("t1"));
        assert_eq!(h.sender, "tom@x"); // name falls back to address
    }

    #[test]
    fn parse_skips_unopenable_hits() {
        let value = json!({
            "value": [{
                "hitsContainers": [{
                    "hits": [
                        {"summary": "no resource"},
                        {"resource": {"chatId": "c1"}}, // no message id
                        {"resource": {"id": "m2"}},     // no conversation
                        {"resource": {"id": "m3", "chatId": "c3"}},
                    ],
                    "total": 4,
                    "moreResultsAvailable": false
                }]
            }]
        });
        let page = parse_search_response(&value);
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].message_id, "m3");
        assert_eq!(page.hits[0].sender, ""); // missing sender stays empty
    }

    #[test]
    fn parse_tolerates_unknown_shapes() {
        for raw in [
            json!({}),
            json!({"value": []}),
            json!({"value": [{"hitsContainers": []}]}),
            json!({"value": [{"hitsContainers": [{"hits": []}]}]}),
            json!(null),
            json!("bogus"),
        ] {
            let page = parse_search_response(&raw);
            assert!(page.hits.is_empty());
            assert!(page.total.is_none());
            assert!(!page.more);
        }
    }

    #[test]
    fn paging_cursor_chains_while_more() {
        let more = SearchPage {
            hits: vec![SearchHitInfo {
                message_id: "m".into(),
                chat_id: "c".into(),
                team_id: None,
                channel_id: None,
                sender: String::new(),
                timestamp: String::new(),
                preview: String::new(),
                subject: None,
            }],
            total: Some(9),
            more: true,
        };
        assert_eq!(next_from(0, &more), Some(1));
        assert_eq!(next_from(25, &more), Some(26));
        let done = SearchPage {
            hits: more.hits,
            total: Some(9),
            more: false,
        };
        assert_eq!(next_from(0, &done), None);
        let empty_more = SearchPage {
            hits: Vec::new(),
            total: None,
            more: true,
        };
        assert_eq!(next_from(0, &empty_more), None); // empty page ends paging
    }
}
