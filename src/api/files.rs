//! Shared files in chats and channels via Microsoft Graph driveItems.
//!
//! Channels: `GET /teams/{team}/channels/{channel}/filesFolder` returns the
//! SharePoint folder driveItem, then `GET /drives/{d}/items/{i}/children`
//! lists files. Chats (1:1/group): files live in the sender's OneDrive
//! "Microsoft Teams Chat Files" folder with no filesFolder endpoint, so the
//! list is mined from Graph chat messages: `reference` attachments carry a
//! SharePoint `contentUrl` resolved via `GET /shares/{id}/driveItem`.
//!
//! Auth: existing Graph token (Teams client `/.default`). No scope widening:
//! the Teams desktop app registration already grants Files.Read/Write for
//! delegated flows. Upload is small-file PUT only (<4 MB); larger files need
//! an upload session (not implemented, rejected with a clear error).

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

const CHAT_FILES_FOLDER: &str = "Microsoft Teams Chat Files";
const MAX_SIMPLE_UPLOAD: u64 = 4 * 1024 * 1024;

// -- Wire types --

#[derive(Debug, Deserialize)]
struct DriveChildrenResponse {
    value: Vec<DriveItem>,
}

#[derive(Debug, Deserialize)]
struct DriveItem {
    id: String,
    name: Option<String>,
    size: Option<u64>,
    #[serde(rename = "eTag")]
    etag: Option<String>,
    #[serde(rename = "webUrl")]
    web_url: Option<String>,
    #[serde(rename = "webDavUrl")]
    web_dav_url: Option<String>,
    #[serde(rename = "@microsoft.graph.downloadUrl")]
    download_url: Option<String>,
    #[serde(rename = "createdDateTime")]
    created: Option<String>,
    #[serde(rename = "lastModifiedDateTime")]
    modified: Option<String>,
    file: Option<FileFacet>,
    folder: Option<serde_json::Value>,
    #[serde(rename = "parentReference")]
    parent: Option<ParentRef>,
}

#[derive(Debug, Deserialize)]
struct FileFacet {
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ParentRef {
    #[serde(rename = "driveId")]
    drive_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessagesResponse {
    value: Vec<GraphChatMessage>,
}

#[derive(Debug, Deserialize)]
struct GraphChatMessage {
    #[serde(default)]
    from: Option<MessageFrom>,
    #[serde(default)]
    attachments: Vec<GraphAttachment>,
}

#[derive(Debug, Deserialize)]
struct MessageFrom {
    #[serde(default)]
    user: Option<MessageUser>,
}

#[derive(Debug, Deserialize)]
struct MessageUser {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphAttachment {
    #[serde(rename = "contentType")]
    content_type: Option<String>,
    #[serde(rename = "contentUrl")]
    content_url: Option<String>,
    name: Option<String>,
}

// -- Public model --

/// One shared file (driveItem projection for list/upload/download).
pub struct SharedFile {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub mime: Option<String>,
    pub web_url: Option<String>,
    // Kept: surfaced to API consumers (CLI prints id/drive/size/type/from/url).
    #[allow(dead_code)]
    pub download_url: Option<String>,
    pub drive_id: Option<String>,
    #[allow(dead_code)]
    pub created: Option<String>,
    #[allow(dead_code)]
    pub modified: Option<String>,
    /// Display name of the chat sender (chat path only; None for channels).
    pub sender: Option<String>,
}

fn shared_from_item(item: DriveItem, sender: Option<String>) -> SharedFile {
    SharedFile {
        id: item.id,
        name: item.name.unwrap_or_else(|| "[unnamed]".to_string()),
        size: item.size.unwrap_or(0),
        mime: item.file.and_then(|f| f.mime_type),
        web_url: item.web_url,
        download_url: item.download_url,
        drive_id: item.parent.and_then(|p| p.drive_id),
        created: item.created,
        modified: item.modified,
        sender,
    }
}

/// Encode a SharePoint sharing URL as a Graph shares id (`u!` + base64url).
/// See https://learn.microsoft.com/graph/api/shares-get
pub fn encode_share_id(url: &str) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(url.as_bytes());
    format!("u!{}", b64)
}

/// Minimal path-segment encoder for drive `:/path:/content` addresses.
/// Keeps unreserved chars, encodes the rest (spaces -> %20, etc).
fn encode_segment(s: &str) -> String {
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

/// First GUID (`8-4-4-4-12` hex) in an eTag, the file-attachment id.
/// driveItem eTags embed the attachment GUID (Graph chatMessage-post docs).
fn guid_from_etag(etag: &str) -> Option<String> {
    let bytes = etag.as_bytes();
    if bytes.len() < 36 {
        return None;
    }
    let is_hex = |b: u8| b.is_ascii_hexdigit();
    for start in 0..=bytes.len() - 36 {
        let w = &bytes[start..start + 36];
        if w[8] == b'-' && w[13] == b'-' && w[18] == b'-' && w[23] == b'-'
            && w[..8].iter().all(|&b| is_hex(b))
            && w[9..13].iter().all(|&b| is_hex(b))
            && w[14..18].iter().all(|&b| is_hex(b))
            && w[19..23].iter().all(|&b| is_hex(b))
            && w[24..36].iter().all(|&b| is_hex(b))
        {
            return Some(etag[start..start + 36].to_string());
        }
    }
    None
}

fn sender_of(msg: &GraphChatMessage) -> Option<String> {
    msg.from
        .as_ref()
        .and_then(|f| f.user.as_ref())
        .and_then(|u| u.display_name.clone())
}

// -- List --

/// List shared files for a chat id or a channel id.
///
/// Tries the chat-messages/attachments path first (Graph
/// `/me/chats/{id}/messages` + shares resolution), then falls back to the
/// channel filesFolder path (team scan + `/drives/.../children`). Folders
/// are skipped; only file driveItems are returned. Deduplicated by item id.
pub async fn list_chat_files_data(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
) -> Result<Vec<SharedFile>> {
    if let Ok(files) = list_via_chat_messages(client, chat_id, limit).await {
        return Ok(files);
    }
    list_via_channel_folder(client, chat_id, limit).await
}

async fn list_via_chat_messages(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
) -> Result<Vec<SharedFile>> {
    let path = format!("/me/chats/{}/messages?$top={}", chat_id, limit.max(1));
    let resp = client.graph_get(&path).await?;
    let msgs: ChatMessagesResponse = resp
        .json()
        .await
        .context("Failed to parse chat messages response")?;

    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for msg in &msgs.value {
        let sender = sender_of(msg);
        for att in &msg.attachments {
            if att.content_type.as_deref() != Some("reference") {
                continue;
            }
            let Some(url) = att.content_url.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            let share_id = encode_share_id(url);
            let spath = format!("/shares/{}/driveItem", share_id);
            let item: DriveItem = match client.graph_get(&spath).await {
                Ok(r) => match r.json().await {
                    Ok(it) => it,
                    Err(e) => {
                        tracing::warn!("Shares resolve parse failed for {}: {:#}", url, e);
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!("Shares resolve failed for {}: {:#}", url, e);
                    continue;
                }
            };
            if !seen.insert(item.id.clone()) {
                continue;
            }
            // Fall back to the attachment name when the item omits it.
            let mut file = shared_from_item(item, sender.clone());
            if file.name == "[unnamed]" {
                if let Some(n) = att.name.clone() {
                    file.name = n;
                }
            }
            files.push(file);
        }
    }
    Ok(files)
}

async fn list_via_channel_folder(
    client: &TeamsClient,
    channel_id: &str,
    limit: usize,
) -> Result<Vec<SharedFile>> {
    let team_id = find_team_for_channel(client, channel_id).await?;
    let fpath = format!("/teams/{}/channels/{}/filesFolder", team_id, channel_id);
    let resp = client.graph_get(&fpath).await?;
    let folder: DriveItem = resp
        .json()
        .await
        .context("Failed to parse filesFolder response")?;
    let drive_id = folder
        .parent
        .as_ref()
        .and_then(|p| p.drive_id.clone())
        .context("filesFolder response missing parent driveId")?;
    let cpath = format!(
        "/drives/{}/items/{}/children?$top={}",
        drive_id,
        folder.id,
        limit.max(1)
    );
    let resp = client.graph_get(&cpath).await?;
    let children: DriveChildrenResponse = resp
        .json()
        .await
        .context("Failed to parse drive children response")?;
    Ok(children
        .value
        .into_iter()
        .filter(|it| it.folder.is_none())
        .map(|it| shared_from_item(it, None))
        .collect())
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

/// List shared files (prints to stdout).
pub async fn list_files(chat_id: &str, limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let files = list_chat_files_data(&client, chat_id, limit).await?;

    println!("\nShared Files:");
    println!("{:-<60}", "");
    if files.is_empty() {
        println!("  (no shared files found)");
        return Ok(());
    }
    for f in &files {
        println!("{}", f.name);
        println!("  ID:   {}", f.id);
        if let Some(ref d) = f.drive_id {
            println!("  Drive: {}", d);
        }
        println!("  Size: {} bytes", f.size);
        if let Some(ref m) = f.mime {
            println!("  Type: {}", m);
        }
        if let Some(ref s) = f.sender {
            println!("  From: {}", s);
        }
        if let Some(ref u) = f.web_url {
            println!("  URL:  {}", u);
        }
        println!();
    }
    Ok(())
}

// -- Download --

/// Download one driveItem's content to `dest_path`. Returns bytes written.
pub async fn download_file_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
    dest_path: &str,
) -> Result<u64> {
    let path = format!("/drives/{}/items/{}/content", drive_id, item_id);
    let resp = client.graph_get(&path).await?;
    let bytes = resp.bytes().await.context("Failed to read file content")?;
    std::fs::write(dest_path, &bytes)
        .with_context(|| format!("Failed to write {}", dest_path))?;
    Ok(bytes.len() as u64)
}

/// Download a shared file by drive+item id (prints to stdout).
pub async fn download_file(drive_id: &str, item_id: &str, dest_path: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let n = download_file_data(&client, drive_id, item_id, dest_path).await?;
    println!("Downloaded {} bytes to {}", n, dest_path);
    Ok(())
}

// -- Upload --

/// Upload a local file to a chat or channel and post it as a `reference`
/// attachment message. Small files only (<4 MB). Returns the uploaded item.
pub async fn upload_file_data(
    client: &TeamsClient,
    chat_id: &str,
    local_path: &str,
) -> Result<SharedFile> {
    let bytes = std::fs::read(local_path)
        .with_context(|| format!("Failed to read {}", local_path))?;
    if bytes.len() as u64 > MAX_SIMPLE_UPLOAD {
        bail!(
            "File is {} bytes; simple upload supports <4 MB only (upload sessions not implemented)",
            bytes.len()
        );
    }
    let filename = std::path::Path::new(local_path)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .context("Local path has no file name")?;

    // Channel ids upload to the channel folder; everything else to the
    // sender's OneDrive chat-files folder.
    if let Ok(team_id) = find_team_for_channel(client, chat_id).await {
        upload_to_channel(client, &team_id, chat_id, filename, bytes).await
    } else {
        upload_to_chat(client, chat_id, filename, bytes).await
    }
}

async fn upload_to_chat(
    client: &TeamsClient,
    chat_id: &str,
    filename: &str,
    bytes: Vec<u8>,
) -> Result<SharedFile> {
    let folder = encode_segment(CHAT_FILES_FOLDER);
    let fname = encode_segment(filename);
    let upath = format!("/me/drive/root:/{}/{}:/content", folder, fname);
    let resp = client
        .graph_put_bytes(&upath, bytes, "application/octet-stream")
        .await?;
    let item: DriveItem = resp.json().await.context("Failed to parse upload response")?;
    post_reference_message(
        client,
        &format!("/me/chats/{}/messages", chat_id),
        &item,
        filename,
    )
    .await?;
    Ok(shared_from_item(item, None))
}

async fn upload_to_channel(
    client: &TeamsClient,
    team_id: &str,
    channel_id: &str,
    filename: &str,
    bytes: Vec<u8>,
) -> Result<SharedFile> {
    let fpath = format!("/teams/{}/channels/{}/filesFolder", team_id, channel_id);
    let resp = client.graph_get(&fpath).await?;
    let folder: DriveItem = resp
        .json()
        .await
        .context("Failed to parse filesFolder response")?;
    let drive_id = folder
        .parent
        .as_ref()
        .and_then(|p| p.drive_id.clone())
        .context("filesFolder response missing parent driveId")?;
    let fname = encode_segment(filename);
    let upath = format!(
        "/drives/{}/items/{}:/{}:/content",
        drive_id, folder.id, fname
    );
    let resp = client
        .graph_put_bytes(&upath, bytes, "application/octet-stream")
        .await?;
    let item: DriveItem = resp.json().await.context("Failed to parse upload response")?;
    post_reference_message(
        client,
        &format!("/teams/{}/channels/{}/messages", team_id, channel_id),
        &item,
        filename,
    )
    .await?;
    Ok(shared_from_item(item, None))
}

fn reference_attachment(item: &DriveItem, filename: &str) -> Result<serde_json::Value> {
    let etag = item.etag.as_deref().unwrap_or("");
    let attach_id =
        guid_from_etag(etag).with_context(|| format!("Upload response eTag has no GUID: {:?}", etag))?;
    let content_url = item
        .web_dav_url
        .clone()
        .or_else(|| item.web_url.clone())
        .context("Upload response has no webDavUrl/webUrl")?;
    Ok(serde_json::json!({
        "id": attach_id,
        "contentType": "reference",
        "contentUrl": content_url,
        "name": filename,
    }))
}

async fn post_reference_message(
    client: &TeamsClient,
    path: &str,
    item: &DriveItem,
    filename: &str,
) -> Result<()> {
    let attachment = reference_attachment(item, filename)?;
    let body = serde_json::json!({
        "body": { "contentType": "html", "content": format!("<attachment id=\"{}\"></attachment>", attachment["id"].as_str().unwrap_or("")) },
        "attachments": [attachment],
    });
    client.graph_post(path, &body).await?;
    Ok(())
}

/// Upload a local file (prints to stdout).
pub async fn upload_file(chat_id: &str, local_path: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let file = upload_file_data(&client, chat_id, local_path).await?;
    println!("Uploaded {} ({} bytes, id {})", file.name, file.size, file.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_id_roundtrips_via_url_safe_base64() {
        use base64::Engine;
        let url = "https://contoso.sharepoint.com/personal/a_b/Documents/file.docx";
        let id = encode_share_id(url);
        assert!(id.starts_with("u!"));
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&id[2..])
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), url);
    }

    #[test]
    fn segment_encoding_keeps_unreserved() {
        assert_eq!(encode_segment("aB09-_.~"), "aB09-_.~");
        assert_eq!(encode_segment("a b/c"), "a%20b%2Fc");
        assert_eq!(encode_segment("Microsoft Teams Chat Files"), "Microsoft%20Teams%20Chat%20Files");
    }

    #[test]
    fn guid_scan_finds_first_guid() {
        let etag = "\"c:{3F2504E0-4F89-11D3-9A0C-0305E82C3301},1\"";
        assert_eq!(
            guid_from_etag(etag).as_deref(),
            Some("3F2504E0-4F89-11D3-9A0C-0305E82C3301")
        );
        assert_eq!(guid_from_etag("no-guid-here"), None);
        assert_eq!(guid_from_etag("short"), None);
        // Lowercase hex also matches.
        assert_eq!(
            guid_from_etag("x550e8400-e29b-41d4-a716-446655440000y").as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn drive_children_parse_skips_folders() {
        let body: DriveChildrenResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"f1","name":"a.pdf","size":12,"file":{"mimeType":"application/pdf"},
                 "webUrl":"https://w/a","@microsoft.graph.downloadUrl":"https://d/a",
                 "parentReference":{"driveId":"D1"}},
                {"id":"dir1","name":"sub","folder":{}}
            ]}"#,
        )
        .unwrap();
        let files: Vec<SharedFile> = body
            .value
            .into_iter()
            .filter(|it| it.folder.is_none())
            .map(|it| shared_from_item(it, None))
            .collect();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, "f1");
        assert_eq!(files[0].name, "a.pdf");
        assert_eq!(files[0].size, 12);
        assert_eq!(files[0].mime.as_deref(), Some("application/pdf"));
        assert_eq!(files[0].drive_id.as_deref(), Some("D1"));
        assert_eq!(files[0].download_url.as_deref(), Some("https://d/a"));
    }

    #[test]
    fn chat_message_attachments_filter_reference_only() {
        let body: ChatMessagesResponse = serde_json::from_str(
            r#"{"value":[
                {"from":{"user":{"displayName":"A Uzer"}},
                 "attachments":[
                   {"contentType":"reference","contentUrl":"https://sp/f","name":"f.docx"},
                   {"contentType":"messageReference","contentUrl":"https://x","name":"quote"}
                 ]},
                {"from":{},"attachments":[]}
            ]}"#,
        )
        .unwrap();
        assert_eq!(body.value.len(), 2);
        assert_eq!(sender_of(&body.value[0]).as_deref(), Some("A Uzer"));
        let refs: Vec<&GraphAttachment> = body.value[0]
            .attachments
            .iter()
            .filter(|a| a.content_type.as_deref() == Some("reference"))
            .collect();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].content_url.as_deref(), Some("https://sp/f"));
        assert!(sender_of(&body.value[1]).is_none());
    }

    #[test]
    fn reference_attachment_prefers_web_dav() {
        let item: DriveItem = serde_json::from_str(
            r#"{"id":"i1","eTag":"\"c:{550E8400-E29B-41D4-A716-446655440000},2\"",
                "webUrl":"https://web/f","webDavUrl":"https://dav/f"}"#,
        )
        .unwrap();
        let v = reference_attachment(&item, "f").unwrap();
        assert_eq!(v["id"], "550E8400-E29B-41D4-A716-446655440000");
        assert_eq!(v["contentType"], "reference");
        assert_eq!(v["contentUrl"], "https://dav/f");
        assert_eq!(v["name"], "f");
    }

    #[test]
    fn reference_attachment_requires_guid() {
        let item: DriveItem = serde_json::from_str(r#"{"id":"i1","eTag":"nope"}"#).unwrap();
        assert!(reference_attachment(&item, "f").is_err());
    }
}
