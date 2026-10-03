//! File + people search (Graph drive search + `/users` `$search`).
//!
//! OstMac (om-jb-filesearch): free-text search over the signed-in user's
//! OneDrive files and the directory. No auth scope change: the existing
//! Graph token (`/.default`) carries delegated Files + User scopes; a 403
//! surfaces as the call's detail. Shapes per
//! `learn.microsoft.com/graph/api/driveitem-search` and
//! `learn.microsoft.com/graph/search-query-parameter`.
//!
//! Both searches are single-window (`$top`, capped at 25): no paging
//! cursors, so the Swift store is search/retry/clear with no loadMore.
//! Rows reuse [`SharedFile`] and [`TeamMemberInfo`] — the palette renders
//! the same shapes as the Shared tab and the roster.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;
use super::files::SharedFile;
use super::teams::TeamMemberInfo;

/// Per-request result cap for both searches.
pub const FIND_MAX_LIMIT: usize = 25;

/// Clamp a result limit into Graph's `1..=25` window.
pub fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, FIND_MAX_LIMIT)
}

/// Minimal query encoder: keeps unreserved chars, percent-encodes the
/// rest (spaces -> %20, etc). Graph single-quote escaping (`''`) is
/// applied by the drive path builder before this runs.
fn encode_query(s: &str) -> String {
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

// ---------------------------------------------------------------------------
// File search: GET /me/drive/root/search(q='{q}')?$top=N
// ---------------------------------------------------------------------------

/// Graph path for one OneDrive filename/content search window.
/// Single quotes in the query double up (Graph escaping) before URL
/// encoding. Pure so tests pin it.
pub fn drive_search_path(query: &str, limit: usize) -> String {
    let escaped = query.trim().replace('\'', "''");
    format!(
        "/me/drive/root/search(q='{}')?$top={}",
        encode_query(&escaped),
        clamp_limit(limit)
    )
}

#[derive(Debug, Deserialize, Default)]
struct DriveSearchResponse {
    #[serde(default)]
    value: Vec<DriveSearchItem>,
}

#[derive(Debug, Deserialize, Default)]
struct DriveSearchItem {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(rename = "webUrl", default)]
    web_url: Option<String>,
    #[serde(rename = "@microsoft.graph.downloadUrl", default)]
    download_url: Option<String>,
    #[serde(rename = "createdDateTime", default)]
    created: Option<String>,
    #[serde(rename = "lastModifiedDateTime", default)]
    modified: Option<String>,
    #[serde(default)]
    file: Option<SearchFileFacet>,
    #[serde(default)]
    folder: Option<serde_json::Value>,
    #[serde(rename = "parentReference", default)]
    parent: Option<SearchParentRef>,
}

#[derive(Debug, Deserialize, Default)]
struct SearchFileFacet {
    #[serde(rename = "mimeType", default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct SearchParentRef {
    #[serde(rename = "driveId", default)]
    drive_id: Option<String>,
}

fn shared_from_hit(item: DriveSearchItem) -> Option<SharedFile> {
    let id = item.id.filter(|s| !s.trim().is_empty())?;
    Some(SharedFile {
        id,
        name: item.name.unwrap_or_else(|| "[unnamed]".to_string()),
        size: item.size.unwrap_or(0),
        mime: item.file.and_then(|f| f.mime_type),
        web_url: item.web_url,
        download_url: item.download_url,
        drive_id: item.parent.and_then(|p| p.drive_id),
        created: item.created,
        modified: item.modified,
        sender: None,
        is_folder: item.folder.is_some(),
        // NOTE: the vendored copy also sets `attachment_id: None` here
        // (unledgered om-inline-docs eTag mining, not part of this PR).
        share_url: None,
    })
}

/// Parse one drive-search response into shared files. Unknown shapes
/// yield no files (never fatal); items without an id are skipped.
pub fn parse_drive_search_response(value: &serde_json::Value) -> Vec<SharedFile> {
    let resp: DriveSearchResponse =
        serde_json::from_value(value.clone()).unwrap_or_default();
    resp.value.into_iter().filter_map(shared_from_hit).collect()
}

/// Search the signed-in user's OneDrive, one `$top` window. Empty
/// queries are rejected before any network.
pub async fn search_files_data(
    client: &TeamsClient,
    query: &str,
    limit: usize,
) -> Result<Vec<SharedFile>> {
    if query.trim().is_empty() {
        bail!("empty query");
    }
    let resp = client.graph_get(&drive_search_path(query, limit)).await?;
    let value: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse drive search response")?;
    Ok(parse_drive_search_response(&value))
}

/// Search OneDrive files (prints one window to stdout).
pub async fn search_files(query: &str, limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let files = search_files_data(&client, query, limit).await?;

    println!("\nFile results for {:?}:", query);
    println!("{:-<60}", "");
    if files.is_empty() {
        println!("  (no files found)");
        return Ok(());
    }
    for f in &files {
        println!("{}{}", f.name, if f.is_folder { "/" } else { "" });
        if let Some(url) = f.web_url.as_deref() {
            println!("  {}", url);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// People search: GET /users?$search="displayName:{q}" (+ ConsistencyLevel)
// ---------------------------------------------------------------------------

/// Graph path for one directory search window. Double quotes in the
/// query are dropped (they would break the `$search` phrase); the rest
/// is URL-encoded. Pure so tests pin it.
pub fn people_search_path(query: &str, limit: usize) -> String {
    let phrase: String = query.trim().replace('"', "");
    format!(
        "/users?$search=\"displayName:{}\"&$select=id,displayName,mail,userPrincipalName&$top={}",
        encode_query(&phrase),
        clamp_limit(limit)
    )
}

#[derive(Debug, Deserialize, Default)]
struct PeopleSearchResponse {
    #[serde(default)]
    value: Vec<PeopleSearchUser>,
}

#[derive(Debug, Deserialize, Default)]
struct PeopleSearchUser {
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
    #[serde(default)]
    mail: Option<String>,
    #[serde(rename = "userPrincipalName", default)]
    upn: Option<String>,
}

fn member_from_user(u: PeopleSearchUser) -> Option<TeamMemberInfo> {
    let id = u.id.filter(|s| !s.trim().is_empty())?;
    let email = u.mail.filter(|s| !s.trim().is_empty()).or(u.upn);
    Some(TeamMemberInfo {
        display_name: u.display_name.unwrap_or_else(|| id.clone()),
        user_id: Some(id.clone()),
        id,
        email,
        roles: Vec::new(),
        is_owner: false,
    })
}

/// Parse one `/users` `$search` response into member rows (roles empty:
/// directory hits carry no team role). Unknown shapes yield no rows
/// (never fatal); users without an id are skipped.
pub fn parse_people_search_response(value: &serde_json::Value) -> Vec<TeamMemberInfo> {
    let resp: PeopleSearchResponse =
        serde_json::from_value(value.clone()).unwrap_or_default();
    resp.value.into_iter().filter_map(member_from_user).collect()
}

/// Search the directory, one `$top` window. Empty queries are rejected
/// before any network. `$search` requires `ConsistencyLevel: eventual`
/// (see `graph_get_consistent`); without it Graph 400s.
pub async fn search_people_data(
    client: &TeamsClient,
    query: &str,
    limit: usize,
) -> Result<Vec<TeamMemberInfo>> {
    if query.trim().is_empty() {
        bail!("empty query");
    }
    let resp = client
        .graph_get_consistent(&people_search_path(query, limit))
        .await?;
    let value: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse people search response")?;
    Ok(parse_people_search_response(&value))
}

/// Search the directory (prints one window to stdout).
pub async fn search_people(query: &str, limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let people = search_people_data(&client, query, limit).await?;

    println!("\nPeople results for {:?}:", query);
    println!("{:-<60}", "");
    if people.is_empty() {
        println!("  (no people found)");
        return Ok(());
    }
    for p in &people {
        println!(
            "{} — {}",
            p.display_name,
            p.email.as_deref().unwrap_or("(no email)")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn limit_clamps_to_graph_window() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(10), 10);
        assert_eq!(clamp_limit(25), 25);
        assert_eq!(clamp_limit(500), 25);
    }

    #[test]
    fn drive_path_shape_matches_graph_contract() {
        assert_eq!(
            drive_search_path("quarterly plan", 50),
            "/me/drive/root/search(q='quarterly%20plan')?$top=25"
        );
    }

    #[test]
    fn drive_path_escapes_single_quotes() {
        assert_eq!(
            drive_search_path("tom's", 10),
            "/me/drive/root/search(q='tom%27%27s')?$top=10"
        );
    }

    #[test]
    fn people_path_shape_matches_graph_contract() {
        assert_eq!(
            people_search_path("ava", 50),
            "/users?$search=\"displayName:ava\"&$select=id,displayName,mail,userPrincipalName&$top=25"
        );
    }

    #[test]
    fn people_path_drops_embedded_quotes() {
        assert_eq!(
            people_search_path("a\"b", 10),
            "/users?$search=\"displayName:ab\"&$select=id,displayName,mail,userPrincipalName&$top=10"
        );
    }

    #[test]
    fn parse_drive_hits_files_and_folders() {
        let value = json!({
            "value": [
                {"id": "f1", "name": "plan.md", "size": 48211,
                 "webUrl": "https://x/plan",
                 "file": {"mimeType": "text/markdown"},
                 "parentReference": {"driveId": "d1"}},
                {"id": "d9", "name": "Design", "folder": {}},
                {"name": "no-id-skipped"},
            ]
        });
        let files = parse_drive_search_response(&value);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "plan.md");
        assert_eq!(files[0].size, 48211);
        assert_eq!(files[0].drive_id.as_deref(), Some("d1"));
        assert!(!files[0].is_folder);
        assert!(files[1].is_folder);
    }

    #[test]
    fn parse_drive_tolerates_unknown_shapes() {
        for raw in [
            json!({}),
            json!({"value": []}),
            json!(null),
            json!("bogus"),
        ] {
            assert!(parse_drive_search_response(&raw).is_empty());
        }
    }

    #[test]
    fn parse_people_prefers_mail_over_upn() {
        let value = json!({
            "value": [
                {"id": "u1", "displayName": "Ava Lindqvist",
                 "mail": "ava@x", "userPrincipalName": "ava.upn@x"},
                {"id": "u2", "displayName": "Tom Becker",
                 "mail": null, "userPrincipalName": "tom@x"},
                {"displayName": "no-id-skipped"},
            ]
        });
        let people = parse_people_search_response(&value);
        assert_eq!(people.len(), 2);
        assert_eq!(people[0].email.as_deref(), Some("ava@x"));
        assert_eq!(people[0].user_id.as_deref(), Some("u1"));
        assert!(people[0].roles.is_empty());
        assert!(!people[0].is_owner);
        assert_eq!(people[1].email.as_deref(), Some("tom@x"));
    }

    #[test]
    fn parse_people_tolerates_unknown_shapes() {
        for raw in [
            json!({}),
            json!({"value": []}),
            json!(null),
            json!("bogus"),
        ] {
            assert!(parse_people_search_response(&raw).is_empty());
        }
    }
}
