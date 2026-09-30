//! Teams tags the signed-in user belongs to (for tag @-mentions).
//! Graph `/teams/{id}/tags` needs TeamworkTag.Read, which the Teams web
//! client token does not carry (403). The web client reads tags from the
//! CSA service instead (`teams/users/me/teams/tagCards`, worker
//! `getTeamsTagCards`), so this does too.
//!
//! Proof level: the URL and its `tagType`/`pageSize` query come from the
//! web client. Not yet confirmed against a live tenant: the `tagType`
//! value `Team`, the response shape, and whether the list is only the
//! tags the user belongs to. The parser is therefore strict about shape
//! (an unrecognised answer is an error, never an empty "no tags" list)
//! and honours a membership flag when a card carries one.

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::client::TeamsClient;

/// CSA service root (skype-token bearer, as `csa_get` expects).
pub const CSA_BASE: &str = "https://teams.microsoft.com/api/csa/api/v1";

/// `pageSize` the web client asks for on this list.
pub const TAG_CARDS_PAGE: usize = 500;
/// Tag kind asked for. Inferred from the web client's team tag enum.
pub const TAG_TYPE: &str = "Team";

/// `GET` URL for the signed-in user's tag cards across teams. Pure.
pub fn my_tag_cards_url(csa: &str) -> String {
    format!(
        "{}/teams/users/me/teams/tagCards?tagType={}&pageSize={}",
        csa.trim_end_matches('/'),
        TAG_TYPE,
        TAG_CARDS_PAGE
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCardInfo {
    pub team_id: String,
    pub tag_id: String,
    pub name: String,
}

fn s(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .map(str::trim)
        .find(|x| !x.is_empty())
        .unwrap_or("")
        .to_string()
}

/// A card that says it is not the user's is dropped; no flag = kept.
fn is_members_card(card: &Value) -> bool {
    ["isMember", "isCurrentUserMember", "isUserMember"]
        .iter()
        .filter_map(|k| card.get(*k).and_then(|x| x.as_bool()))
        .all(|b| b)
}

/// Parse a tag-cards answer. Recognised containers: `teamTagCards`
/// (`[{teamId, tagCards:[…]}]`), a top-level `tagCards`, or a bare array.
/// Anything else, or a non-empty container with no nameable card, is an
/// error: an answer this parser cannot read must never look like "the
/// user has no tags". Pure.
pub fn parse_tag_cards(v: &Value) -> Result<Vec<TagCardInfo>> {
    let mut groups: Vec<(String, &Vec<Value>)> = Vec::new();
    if let Some(teams) = v.get("teamTagCards").and_then(|x| x.as_array()) {
        for t in teams {
            let cards = t
                .get("tagCards")
                .and_then(|x| x.as_array())
                .context("tag answer: a team entry has no tagCards list")?;
            groups.push((s(t, &["teamId", "id"]), cards));
        }
    } else if let Some(cards) = v.get("tagCards").and_then(|x| x.as_array()) {
        groups.push((s(v, &["teamId"]), cards));
    } else if let Some(cards) = v.as_array() {
        groups.push((String::new(), cards));
    } else {
        bail!("tag answer has no tagCards list (unrecognised shape)");
    }
    let (mut out, mut seen_cards) = (Vec::new(), 0usize);
    for (team, cards) in groups {
        for c in cards {
            seen_cards += 1;
            let name = s(c, &["displayName", "name", "tagName"]);
            if name.is_empty() || !is_members_card(c) {
                continue;
            }
            let team_id = match s(c, &["teamId"]) {
                t if t.is_empty() => team.clone(),
                t => t,
            };
            out.push(TagCardInfo { team_id, tag_id: s(c, &["id", "tagId"]), name });
        }
    }
    if seen_cards > 0 && out.is_empty() && !has_membership_flag(v) {
        bail!("tag answer has cards but none with a name (unrecognised shape)");
    }
    Ok(out)
}

fn has_membership_flag(v: &Value) -> bool {
    let mut cards: Vec<&Value> = Vec::new();
    if let Some(t) = v.get("teamTagCards").and_then(|x| x.as_array()) {
        for e in t {
            cards.extend(e.get("tagCards").and_then(|x| x.as_array()).into_iter().flatten());
        }
    } else if let Some(c) = v.get("tagCards").and_then(|x| x.as_array()).or_else(|| v.as_array()) {
        cards.extend(c);
    }
    cards.iter().any(|c| {
        ["isMember", "isCurrentUserMember", "isUserMember"].iter().any(|k| c.get(*k).is_some())
    })
}

/// The signed-in user's tags across teams, from the CSA service. A read:
/// no side effects. Errors (no token, HTTP failure, bad JSON, unreadable
/// shape) propagate.
pub async fn my_tag_cards_data(client: &TeamsClient) -> Result<Vec<TagCardInfo>> {
    let url = my_tag_cards_url(CSA_BASE);
    let v: Value = client
        .csa_get(&url)
        .await?
        .json()
        .await
        .context("Failed to parse tag cards response")?;
    parse_tag_cards(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_is_the_web_clients_tag_cards_read() {
        assert_eq!(
            my_tag_cards_url("https://teams.microsoft.com/api/csa/api/v1/"),
            "https://teams.microsoft.com/api/csa/api/v1/teams/users/me/teams/tagCards?tagType=Team&pageSize=500"
        );
    }

    #[test]
    fn parses_team_grouped_and_flat_shapes() {
        let grouped = json!({"teamTagCards": [
            {"teamId": "t1", "teamName": "Ops", "tagCards": [
                {"id": "g1", "displayName": "Designers"},
                {"id": "g2", "displayName": "  "},
                {"id": "g3", "name": "Leads", "isMember": false},
                {"id": "g4", "name": "On call", "isMember": true}]}]});
        let got = parse_tag_cards(&grouped).unwrap();
        assert_eq!(
            got,
            vec![
                TagCardInfo { team_id: "t1".into(), tag_id: "g1".into(), name: "Designers".into() },
                TagCardInfo { team_id: "t1".into(), tag_id: "g4".into(), name: "On call".into() },
            ]
        );
        let flat = json!({"tagCards": [{"tagId": "x", "tagName": "QA", "teamId": "t9"}]});
        assert_eq!(parse_tag_cards(&flat).unwrap()[0].team_id, "t9");
        assert_eq!(parse_tag_cards(&json!([{"displayName": "A"}])).unwrap().len(), 1);
    }

    #[test]
    fn empty_lists_are_a_real_none_but_unreadable_answers_are_errors() {
        assert!(parse_tag_cards(&json!({"teamTagCards": []})).unwrap().is_empty());
        assert!(parse_tag_cards(&json!({"tagCards": []})).unwrap().is_empty());
        // Unknown shape, error object, or nameless cards must not read as "no tags".
        assert!(parse_tag_cards(&json!({"error": {"code": "Forbidden"}})).is_err());
        assert!(parse_tag_cards(&json!({"teamTagCards": [{"teamId": "t"}]})).is_err());
        assert!(parse_tag_cards(&json!({"tagCards": [{"id": "1"}, {"id": "2"}]})).is_err());
        assert!(parse_tag_cards(&json!("nope")).is_err());
    }
}
