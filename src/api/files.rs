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
    /// True when the driveItem is a folder (has the `folder` facet).
    /// Folders have no size/mime/download_url; callers drill in via the
    /// children endpoint (drive_id + id).
    pub is_folder: bool,
    /// Sharing link from createLink. List paths never fill it (None);
    /// callers cache the created link per file id after creating one.
    pub share_url: Option<String>,
}

fn shared_from_item(item: DriveItem, sender: Option<String>) -> SharedFile {
    let is_folder = item.folder.is_some();
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
        is_folder,
        share_url: None,
    }
}

/// True when a raw driveItem survives the folders filter: files always
/// pass; folders pass only when `include_folders` is set.
fn keep_item(item: &DriveItem, include_folders: bool) -> bool {
    include_folders || item.folder.is_none()
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

/// True when `id` is channel-shaped (`19:...@thread.tacv2`). Chat ids
/// share the `19:` prefix but end `@thread.v2`, so the suffix is the
/// scope discriminator: channels list/upload via the team filesFolder,
/// chats via messages/attachments + the sender's OneDrive chat folder.
/// Unknown shapes return false (chat path first, channel fallback).
pub fn is_channel_id(id: &str) -> bool {
    id.trim().ends_with("@thread.tacv2")
}

// -- List --

/// List shared files for a chat id or a channel id.
///
/// Scope-aware order: channel-shaped ids try the channel filesFolder path
/// first (team scan + `/drives/.../children`), chat-shaped ids the
/// chat-messages/attachments path first (Graph `/me/chats/{id}/messages`
/// + shares resolution); each falls back to the other path when its own
/// fails (unknown id shapes still resolve). Folders are skipped; only
/// file driveItems are returned. Deduplicated by item id.
/// Default shape (stable): same as `_opts` with `include_folders=false`.
pub async fn list_chat_files_data(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
) -> Result<Vec<SharedFile>> {
    list_chat_files_data_opts(client, chat_id, limit, false).await
}

/// List shared files, optionally including folders (om-i5-folders).
/// `include_folders=true` keeps folder driveItems in both list paths;
/// each carries `is_folder` so callers can drill in via
/// [`list_folder_children_data`]. Default callers use
/// [`list_chat_files_data`] (folders filtered, shape unchanged).
pub async fn list_chat_files_data_opts(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
    include_folders: bool,
) -> Result<Vec<SharedFile>> {
    if is_channel_id(chat_id) {
        if let Ok(files) =
            list_via_channel_folder(client, chat_id, limit, include_folders).await
        {
            return Ok(files);
        }
        list_via_chat_messages(client, chat_id, limit, include_folders).await
    } else {
        if let Ok(files) =
            list_via_chat_messages(client, chat_id, limit, include_folders).await
        {
            return Ok(files);
        }
        list_via_channel_folder(client, chat_id, limit, include_folders).await
    }
}

async fn list_via_chat_messages(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
    include_folders: bool,
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
            if !keep_item(&item, include_folders) {
                continue;
            }
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
    include_folders: bool,
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
        .filter(|it| keep_item(it, include_folders))
        .map(|it| shared_from_item(it, None))
        .collect())
}

// -- Folder children (om-i5-folders) --

/// Graph path for one folder's children (`drive_id` + folder `item_id`).
pub fn folder_children_path(drive_id: &str, item_id: &str, limit: usize) -> String {
    format!(
        "/drives/{}/items/{}/children?$top={}",
        drive_id,
        item_id,
        limit.max(1)
    )
}

/// List one folder's children by drive+item id. Returns files AND
/// subfolders (no filtering: browsing needs folders visible); each
/// item carries `is_folder`, and folders drill in via this same call
/// with their own id. Ids come from any [`SharedFile`] (`drive_id`+`id`).
pub async fn list_folder_children_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
    limit: usize,
) -> Result<Vec<SharedFile>> {
    let path = folder_children_path(drive_id, item_id, limit);
    let resp = client.graph_get(&path).await?;
    let children: DriveChildrenResponse = resp
        .json()
        .await
        .context("Failed to parse drive children response")?;
    Ok(children
        .value
        .into_iter()
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
        if f.is_folder {
            println!("{}/", f.name);
        } else {
            println!("{}", f.name);
        }
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

// -- Versions (OneDrive/SharePoint version history) --

#[derive(Debug, Deserialize)]
struct VersionsResponse {
    value: Vec<DriveItemVersion>,
}

#[derive(Debug, Deserialize)]
struct DriveItemVersion {
    id: String,
    size: Option<u64>,
    #[serde(rename = "lastModifiedDateTime")]
    modified: Option<String>,
    #[serde(rename = "lastModifiedBy")]
    modified_by: Option<ModifiedBy>,
}

#[derive(Debug, Deserialize)]
struct ModifiedBy {
    user: Option<MessageUser>,
}

/// One file version (driveItemVersion projection for list/restore/download).
pub struct FileVersion {
    pub id: String,
    pub size: u64,
    pub modified: Option<String>,
    pub modified_by: Option<String>,
}

fn version_from_item(item: DriveItemVersion) -> FileVersion {
    FileVersion {
        id: item.id,
        size: item.size.unwrap_or(0),
        modified: item.modified,
        modified_by: item.modified_by.and_then(|b| b.user).and_then(|u| u.display_name),
    }
}

/// List version history for one driveItem, newest first (Graph order).
pub async fn list_file_versions_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
) -> Result<Vec<FileVersion>> {
    let path = format!("/drives/{}/items/{}/versions", drive_id, item_id);
    let resp = client.graph_get(&path).await?;
    let body: VersionsResponse = resp
        .json()
        .await
        .context("Failed to parse versions response")?;
    Ok(body.value.into_iter().map(version_from_item).collect())
}

/// Restore one version as current (Graph `restoreVersion` action).
pub async fn restore_file_version_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
    version_id: &str,
) -> Result<()> {
    let path = format!(
        "/drives/{}/items/{}/versions/{}/restoreVersion",
        drive_id, item_id, version_id
    );
    client.graph_post(&path, &serde_json::json!({})).await?;
    Ok(())
}

/// Download one old version's content to `dest_path`. Returns bytes written.
pub async fn download_file_version_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
    version_id: &str,
    dest_path: &str,
) -> Result<u64> {
    let path = format!(
        "/drives/{}/items/{}/versions/{}/content",
        drive_id, item_id, version_id
    );
    let resp = client.graph_get(&path).await?;
    let bytes = resp.bytes().await.context("Failed to read version content")?;
    std::fs::write(dest_path, &bytes)
        .with_context(|| format!("Failed to write {}", dest_path))?;
    Ok(bytes.len() as u64)
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

    // Channel-shaped ids upload to the channel folder; chat-shaped ids go
    // straight to the sender's OneDrive chat-files folder with no team scan
    // (the scan costs joinedTeams + one channels call per team).
    if is_channel_id(chat_id) {
        let team_id = find_team_for_channel(client, chat_id).await?;
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

// -- Sharing links --

/// A view-only sharing link for one driveItem (Graph createLink).
pub struct SharedLink {
    pub url: String,
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateLinkResponse {
    link: Option<CreateLinkInner>,
}

#[derive(Debug, Deserialize)]
struct CreateLinkInner {
    #[serde(rename = "webUrl")]
    web_url: Option<String>,
    scope: Option<String>,
}

/// Normalize a createLink scope (the perms surface): `anonymous` (anyone
/// with the link) or `organization` (org-only). Blank/unknown input falls
/// back to `organization` (least privilege). Case-insensitive; `anyone`
/// is accepted as an alias for `anonymous`.
pub fn normalize_link_scope(scope: &str) -> &'static str {
    match scope.trim().to_lowercase().as_str() {
        "anonymous" | "anyone" => "anonymous",
        _ => "organization",
    }
}

/// POST body for createLink (view-only link; edit links not offered).
pub fn create_link_body(scope: &str) -> serde_json::Value {
    serde_json::json!({"type": "view", "scope": normalize_link_scope(scope)})
}

/// Graph path for createLink on one driveItem.
pub fn create_link_path(drive_id: &str, item_id: &str) -> String {
    format!("/drives/{}/items/{}/createLink", drive_id, item_id)
}

/// Pull the shareable URL out of a Graph createLink response body.
pub fn parse_create_link(body: &serde_json::Value) -> Result<SharedLink> {
    let resp: CreateLinkResponse =
        serde_json::from_value(body.clone()).context("Failed to parse createLink response")?;
    let inner = resp
        .link
        .context("createLink response has no link object")?;
    let url = inner
        .web_url
        .filter(|s| !s.is_empty())
        .context("createLink response link has no webUrl")?;
    Ok(SharedLink {
        url,
        scope: inner.scope,
    })
}

/// Create (or fetch the existing) view-only sharing link for one
/// driveItem. Idempotent server-side: the same scope returns the same
/// link. `scope` is normalized via [`normalize_link_scope`].
pub async fn create_link_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
    scope: &str,
) -> Result<SharedLink> {
    let path = create_link_path(drive_id, item_id);
    let body = create_link_body(scope);
    let resp = client.graph_post(&path, &body).await?;
    let value: serde_json::Value = resp
        .json()
        .await
        .context("Failed to read createLink response")?;
    parse_create_link(&value)
}

/// Create a sharing link (prints the URL to stdout).
pub async fn create_link(drive_id: &str, item_id: &str, scope: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let link = create_link_data(&client, drive_id, item_id, scope).await?;
    println!("{}", link.url);
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
    fn keep_item_filters_folders_by_default() {
        let file: DriveItem =
            serde_json::from_str(r#"{"id":"f1","name":"a.pdf","file":{}}"#).unwrap();
        let folder: DriveItem =
            serde_json::from_str(r#"{"id":"d1","name":"sub","folder":{}}"#).unwrap();
        assert!(keep_item(&file, false));
        assert!(keep_item(&file, true));
        assert!(!keep_item(&folder, false));
        assert!(keep_item(&folder, true));
    }

    #[test]
    fn shared_from_item_marks_folder_facet() {
        let folder: DriveItem =
            serde_json::from_str(r#"{"id":"d1","name":"sub","folder":{"childCount":3}}"#)
                .unwrap();
        let f = shared_from_item(folder, None);
        assert!(f.is_folder);
        assert_eq!(f.name, "sub");
        assert_eq!(f.size, 0);
        assert_eq!(f.mime, None);
        let file: DriveItem =
            serde_json::from_str(r#"{"id":"f1","name":"a.pdf","file":{"mimeType":"application/pdf"}}"#)
                .unwrap();
        assert!(!shared_from_item(file, None).is_folder);
    }

    #[test]
    fn folder_children_path_shape() {
        assert_eq!(
            folder_children_path("D1", "root", 20),
            "/drives/D1/items/root/children?$top=20"
        );
        assert_eq!(
            folder_children_path("D1", "abc", 0),
            "/drives/D1/items/abc/children?$top=1"
        );
    }

    #[test]
    fn children_parse_keeps_folders_and_files() {
        // Children endpoint never filters: subfolders stay visible.
        let body: DriveChildrenResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"dir1","name":"sub","folder":{},"parentReference":{"driveId":"D1"}},
                {"id":"f1","name":"a.pdf","size":12,"file":{"mimeType":"application/pdf"},
                 "parentReference":{"driveId":"D1"}}
            ]}"#,
        )
        .unwrap();
        let files: Vec<SharedFile> =
            body.value.into_iter().map(|it| shared_from_item(it, None)).collect();
        assert_eq!(files.len(), 2);
        assert!(files[0].is_folder);
        assert_eq!(files[0].drive_id.as_deref(), Some("D1"));
        assert!(!files[1].is_folder);
        assert_eq!(files[1].size, 12);
    }

    #[test]
    fn link_scope_normalizes_to_least_privilege() {
        assert_eq!(normalize_link_scope("organization"), "organization");
        assert_eq!(normalize_link_scope("  Organization "), "organization");
        assert_eq!(normalize_link_scope("anonymous"), "anonymous");
        assert_eq!(normalize_link_scope("Anyone"), "anonymous");
        assert_eq!(normalize_link_scope(""), "organization");
        assert_eq!(normalize_link_scope("edit"), "organization");
    }

    #[test]
    fn link_body_is_view_only_with_scope() {
        assert_eq!(
            create_link_body("anonymous"),
            serde_json::json!({"type": "view", "scope": "anonymous"})
        );
        assert_eq!(
            create_link_body("bogus"),
            serde_json::json!({"type": "view", "scope": "organization"})
        );
    }

    #[test]
    fn link_path_addresses_drive_item() {
        assert_eq!(
            create_link_path("D1", "I1"),
            "/drives/D1/items/I1/createLink"
        );
    }

    #[test]
    fn link_parse_extracts_web_url_and_scope() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"id":"perm-1","link":{"type":"view","scope":"organization",
                "webUrl":"https://contoso.sharepoint.com/:i:/x/ABC"}}"#,
        )
        .unwrap();
        let link = parse_create_link(&body).unwrap();
        assert_eq!(link.url, "https://contoso.sharepoint.com/:i:/x/ABC");
        assert_eq!(link.scope.as_deref(), Some("organization"));
    }

    #[test]
    fn link_parse_rejects_missing_link_or_url() {
        for raw in [
            r#"{"id":"perm-1"}"#,
            r#"{"link":{"type":"view","scope":"organization"}}"#,
            r#"{"link":{"webUrl":""}}"#,
        ] {
            let body: serde_json::Value = serde_json::from_str(raw).unwrap();
            assert!(parse_create_link(&body).is_err(), "raw {}", raw);
        }
    }

    #[test]
    fn list_never_fills_share_url() {
        let item: DriveItem =
            serde_json::from_str(r#"{"id":"i1","name":"f.docx"}"#).unwrap();
        assert_eq!(shared_from_item(item, None).share_url, None);
    }

    #[test]
    fn versions_parse_newest_first_with_author() {
        let body: VersionsResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"3.0","size":48211,"lastModifiedDateTime":"2026-09-20T10:00:00Z",
                 "lastModifiedBy":{"user":{"displayName":"Priya Nair"}}},
                {"id":"2.0"},
                {"id":"1.0","size":100,"lastModifiedBy":{}}
            ]}"#,
        )
        .unwrap();
        let vs: Vec<FileVersion> = body.value.into_iter().map(version_from_item).collect();
        assert_eq!(vs.len(), 3);
        assert_eq!(vs[0].id, "3.0");
        assert_eq!(vs[0].size, 48211);
        assert_eq!(vs[0].modified.as_deref(), Some("2026-09-20T10:00:00Z"));
        assert_eq!(vs[0].modified_by.as_deref(), Some("Priya Nair"));
        // Sparse versions default size 0, no author (never fatal).
        assert_eq!(vs[1].size, 0);
        assert_eq!(vs[1].modified_by, None);
        assert_eq!(vs[2].id, "1.0");
        assert_eq!(vs[2].modified_by, None);
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

    #[test]
    fn channel_scope_is_tacv2_suffix() {
        assert!(is_channel_id("19:general@thread.tacv2"));
        assert!(is_channel_id("  19:abc@thread.tacv2  "));
        assert!(!is_channel_id("19:abc@thread.v2"));
        assert!(!is_channel_id("19:meeting_xyz@thread.v2"));
        assert!(!is_channel_id(""));
        assert!(!is_channel_id("19:general@thread.tacv2.evil"));
        assert!(!is_channel_id("general"));
    }
}
