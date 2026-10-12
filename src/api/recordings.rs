//! Meeting recordings in OneDrive + SharePoint (Graph driveItems).
//!
//! OstMac (om-recordings): one searchable list of every meeting
//! recording the user can access, with in-app playback. Teams stores
//! non-channel meeting recordings in the organizer's OneDrive
//! `Recordings` folder and channel-meeting recordings in the channel's
//! SharePoint filesFolder `Recordings` folder; both are plain `.mp4`
//! driveItems, so list/download reuse the files-stack shapes.
//!
//! Docs grounding (Microsoft Graph v1.0; the list + search paths
//! below are live-probed 200-empty on a real tenant, but no real
//! `.mp4` exists there so no real-data end-to-end yet):
//! - OneDrive folder children:
//!   `GET /me/drive/root:/Recordings:/children?$top=N`
//!   (missing folder 404s -> empty list, never an error)
//! - OneDrive search: `GET /me/drive/root/search(q='{q}')?$top=N`
//! - per-drive search: `GET /drives/{id}/root/search(q='{q}')?$top=N`
//! - channel filesFolder:
//!   `GET /teams/{team}/channels/{channel}/filesFolder`
//! - folder children: `GET /drives/{d}/items/{i}/children?$top=N`
//! - playback length: the driveItem `video` facet (`durationMillis`)
//! - playback bytes: the pre-authenticated `@microsoft.graph.downloadUrl`
//!   (streams directly) or `/drives/{d}/items/{i}/content` via the
//!   existing files download when it expired.
//!
//! Auth: existing Graph token (`/.default`). No scope widening: the
//! Teams desktop app registration already grants delegated Files
//! scopes. A 403 surfaces as the call's detail.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

/// Teams' on-disk recordings folder name (OneDrive + channel folders).
pub const RECORDINGS_FOLDER: &str = "Recordings";

/// Per-request result cap for list + search windows.
pub const RECORDINGS_MAX_LIMIT: usize = 50;

/// Upper bound on channel drives fanned out to (list + search stay
/// one teams scan plus bounded folder/search windows).
pub const MAX_CHANNEL_DRIVES: usize = 50;

/// Clamp a result limit into the `1..=50` window.
pub fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, RECORDINGS_MAX_LIMIT)
}

/// Minimal query encoder: keeps unreserved chars, percent-encodes the
/// rest (spaces -> %20, etc). Graph single-quote escaping (`''`) is
/// applied by the search path builder before this runs. (Duplicate of
/// the filesearch encoder: this module is self-contained on purpose.)
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
// Graph paths (pure so tests pin them)
// ---------------------------------------------------------------------------

/// OneDrive `Recordings` folder children window. Newest-first order is
/// applied client-side ([`sort_newest`]): `$orderby` on children is
/// unevenly supported, and channel merges sort anyway.
pub fn recordings_children_path(limit: usize) -> String {
    format!(
        "/me/drive/root:/{}:/children?$top={}",
        RECORDINGS_FOLDER,
        clamp_limit(limit)
    )
}

/// One drive-search window, either the signed-in user's OneDrive
/// (`drive_id=None`) or one channel drive (`Some`). Single quotes in
/// the query double up (Graph escaping) before URL encoding.
pub fn recordings_search_path(drive_id: Option<&str>, query: &str, limit: usize) -> String {
    let escaped = query.trim().replace('\'', "''");
    let root = match drive_id {
        Some(d) => format!("/drives/{}/root", d),
        None => "/me/drive".to_string(),
    };
    format!(
        "{}/search(q='{}')?$top={}",
        root,
        encode_query(&escaped),
        clamp_limit(limit)
    )
}

/// One channel filesFolder lookup (resolves the SharePoint drive).
pub fn channel_files_folder_path(team_id: &str, channel_id: &str) -> String {
    format!("/teams/{}/channels/{}/filesFolder", team_id, channel_id)
}

/// One folder's children window (filesFolder scan + Recordings list).
pub fn folder_children_path(drive_id: &str, item_id: &str, limit: usize) -> String {
    format!(
        "/drives/{}/items/{}/children?$top={}",
        drive_id,
        item_id,
        clamp_limit(limit)
    )
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct DriveChildrenResponse {
    #[serde(default)]
    value: Vec<DriveItem>,
}

#[derive(Debug, Deserialize, Default)]
struct DriveItem {
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
    file: Option<FileFacet>,
    #[serde(default)]
    video: Option<VideoFacet>,
    #[serde(default)]
    folder: Option<serde_json::Value>,
    #[serde(rename = "parentReference", default)]
    parent: Option<ParentRef>,
}

#[derive(Debug, Deserialize, Default)]
struct FileFacet {
    #[serde(rename = "mimeType", default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct VideoFacet {
    #[serde(rename = "durationMillis", default)]
    duration_millis: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct ParentRef {
    #[serde(rename = "driveId", default)]
    drive_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Public model
// ---------------------------------------------------------------------------

/// Where one recording lives: the user's own OneDrive `Recordings`
/// folder or one channel's SharePoint `Recordings` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingSource {
    OneDrive,
    Channel { team: String, channel: String },
}

impl RecordingSource {
    /// Short provenance label for list rows (`OneDrive`,
    /// `Engineering > #general`).
    pub fn label(&self) -> String {
        match self {
            RecordingSource::OneDrive => "OneDrive".to_string(),
            RecordingSource::Channel { team, channel } => {
                format!("{} > #{}", team, channel)
            }
        }
    }
}

/// One meeting recording (driveItem projection for list/search/play).
pub struct RecordingInfo {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub mime: Option<String>,
    pub web_url: Option<String>,
    /// Pre-authenticated short-lived URL: Swift streams playback
    /// directly; when expired it falls back to the files download
    /// (`drive_id` + `id`).
    pub download_url: Option<String>,
    pub drive_id: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    /// Playback length from the driveItem `video` facet (None when
    /// Graph omits the facet).
    pub duration_ms: Option<u64>,
    pub source: RecordingSource,
}

/// True when a driveItem name/mime looks like a recording: a video
/// mime, or a video extension (Graph sometimes omits the mime).
/// Pure so list, search, and tests share it.
pub fn is_video(name: &str, mime: Option<&str>) -> bool {
    if let Some(m) = mime {
        if m.to_ascii_lowercase().starts_with("video/") {
            return true;
        }
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "mp4" | "mov" | "m4v")
}

fn recording_from_item(item: DriveItem, source: RecordingSource) -> Option<RecordingInfo> {
    let id = item.id.filter(|s| !s.trim().is_empty())?;
    let name = item.name.unwrap_or_else(|| "[unnamed]".to_string());
    if item.folder.is_some() {
        return None;
    }
    if item.file.is_none() {
        return None;
    }
    let mime = item.file.and_then(|f| f.mime_type);
    if !is_video(&name, mime.as_deref()) {
        return None;
    }
    Some(RecordingInfo {
        id,
        name,
        size: item.size.unwrap_or(0),
        mime,
        web_url: item.web_url,
        download_url: item.download_url,
        drive_id: item.parent.and_then(|p| p.drive_id),
        created: item.created,
        modified: item.modified,
        duration_ms: item.video.and_then(|v| v.duration_millis),
        source,
    })
}

/// Parse one children/search response into recordings. Unknown shapes
/// yield no rows (never fatal); folders, non-video files, and items
/// without an id are skipped.
pub fn parse_recordings_response(
    value: &serde_json::Value,
    source: RecordingSource,
) -> Vec<RecordingInfo> {
    let resp: DriveChildrenResponse =
        serde_json::from_value(value.clone()).unwrap_or_default();
    resp.value
        .into_iter()
        .filter_map(|it| recording_from_item(it, source.clone()))
        .collect()
}

/// Sort newest-first by `modified` (ISO-8601 strings sort
/// lexicographically), falling back to `created`, then name. Merges
/// from several drives/folders land in one stable order.
pub fn sort_newest(files: &mut Vec<RecordingInfo>) {
    files.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| b.created.cmp(&a.created))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// True when a Graph error chain is a 404 (missing `Recordings`
/// folder): the list/search treats that drive as empty, never fatal.
/// Matches the `HTTP 404` status `client::check_response` renders.
pub fn is_not_found(err: &anyhow::Error) -> bool {
    format!("{:#}", err).contains("404")
}

// ---------------------------------------------------------------------------
// Channel drive discovery (bounded fan-out)
// ---------------------------------------------------------------------------

/// One channel's SharePoint drive: filesFolder ids for the Recordings
/// scan plus display names for the row source label.
struct ChannelDrive {
    team: String,
    channel: String,
    drive_id: String,
    folder_id: String,
}

/// Resolve every channel's filesFolder drive (bounded to
/// [`MAX_CHANNEL_DRIVES`]). Teams-scan failure yields no drives (the
/// OneDrive list still stands); per-channel failures skip that
/// channel only.
async fn channel_drives(client: &TeamsClient) -> Vec<ChannelDrive> {
    let teams = match crate::api::list_teams_data(client).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("Recordings teams scan failed: {:#}", e);
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    'teams: for team in &teams {
        for ch in &team.channels {
            if out.len() >= MAX_CHANNEL_DRIVES {
                break 'teams;
            }
            let path = channel_files_folder_path(&team.id, &ch.id);
            let folder: DriveItem = match client.graph_get(&path).await {
                Ok(r) => match r.json().await {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!("Recordings filesFolder parse failed: {:#}", e);
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!("Recordings filesFolder failed: {:#}", e);
                    continue;
                }
            };
            let (Some(drive_id), Some(folder_id)) = (
                folder.parent.and_then(|p| p.drive_id),
                folder.id,
            ) else {
                continue;
            };
            out.push(ChannelDrive {
                team: team.name.clone(),
                channel: ch.name.clone(),
                drive_id,
                folder_id,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// List + search
// ---------------------------------------------------------------------------

/// List every meeting recording: the OneDrive `Recordings` folder plus
/// each channel's `Recordings` folder, newest first, truncated to
/// `limit`. Missing folders (404) and per-channel failures read as
/// empty; only a total failure (no drive answered) errors.
pub async fn list_recordings_data(
    client: &TeamsClient,
    limit: usize,
) -> Result<Vec<RecordingInfo>> {
    let limit = clamp_limit(limit);
    let mut all = Vec::new();
    let mut answered = false;

    // Own OneDrive first (the common case stands alone).
    match client.graph_get(&recordings_children_path(limit)).await {
        Ok(resp) => {
            answered = true;
            let value: serde_json::Value = resp
                .json()
                .await
                .context("Failed to parse Recordings folder response")?;
            all.extend(parse_recordings_response(&value, RecordingSource::OneDrive));
        }
        Err(e) if is_not_found(&e) => {
            answered = true; // no folder yet: empty, not fatal
        }
        Err(e) => {
            tracing::warn!("Recordings OneDrive list failed: {:#}", e);
        }
    }

    // Channel folders: scan each filesFolder for a `Recordings` dir.
    for drive in channel_drives(client).await {
        let source = RecordingSource::Channel {
            team: drive.team.clone(),
            channel: drive.channel.clone(),
        };
        let kids: DriveChildrenResponse =
            match client.graph_get(&folder_children_path(&drive.drive_id, &drive.folder_id, limit)).await
            {
                Ok(r) => match r.json().await {
                    Ok(k) => k,
                    Err(e) => {
                        tracing::warn!("Recordings channel scan parse failed: {:#}", e);
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!("Recordings channel scan failed: {:#}", e);
                    continue;
                }
            };
        answered = true;
        let rec = kids.value.into_iter().find(|it| {
            it.folder.is_some() && it.name.as_deref() == Some(RECORDINGS_FOLDER)
        });
        let Some(rec) = rec else { continue };
        let Some(rec_id) = rec.id else { continue };
        let resp = match client
            .graph_get(&folder_children_path(&drive.drive_id, &rec_id, limit))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Recordings channel list failed: {:#}", e);
                continue;
            }
        };
        let value: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Recordings channel parse failed: {:#}", e);
                continue;
            }
        };
        all.extend(parse_recordings_response(&value, source));
    }

    if !answered {
        bail!("no drive answered the recordings list");
    }
    sort_newest(&mut all);
    all.truncate(limit);
    Ok(all)
}

/// Search every recording by name: the OneDrive drive-search plus one
/// drive-search per channel drive, video-filtered, newest first,
/// truncated to `limit`. Empty queries are rejected before any
/// network.
pub async fn search_recordings_data(
    client: &TeamsClient,
    query: &str,
    limit: usize,
) -> Result<Vec<RecordingInfo>> {
    if query.trim().is_empty() {
        bail!("empty query");
    }
    let limit = clamp_limit(limit);
    let mut all = Vec::new();

    // OneDrive answers or the error propagates (`?`): reaching past
    // here means one drive answered.
    let resp = client
        .graph_get(&recordings_search_path(None, query, limit))
        .await?;
    let value: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse recordings search response")?;
    all.extend(parse_recordings_response(&value, RecordingSource::OneDrive));

    for drive in channel_drives(client).await {
        let source = RecordingSource::Channel {
            team: drive.team.clone(),
            channel: drive.channel.clone(),
        };
        let resp = match client
            .graph_get(&recordings_search_path(Some(&drive.drive_id), query, limit))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Recordings drive search failed: {:#}", e);
                continue;
            }
        };
        let value: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Recordings drive search parse failed: {:#}", e);
                continue;
            }
        };
        all.extend(parse_recordings_response(&value, source));
    }

    sort_newest(&mut all);
    all.truncate(limit);
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn limit_clamps_to_window() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(25), 25);
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(500), 50);
    }

    #[test]
    fn children_path_shape_matches_graph_contract() {
        assert_eq!(
            recordings_children_path(99),
            "/me/drive/root:/Recordings:/children?$top=50"
        );
    }

    #[test]
    fn search_paths_shape_matches_graph_contract() {
        assert_eq!(
            recordings_search_path(None, "q3 review", 99),
            "/me/drive/search(q='q3%20review')?$top=50"
        );
        assert_eq!(
            recordings_search_path(Some("d9"), "tom's", 10),
            "/drives/d9/root/search(q='tom%27%27s')?$top=10"
        );
    }

    #[test]
    fn folder_paths_shape_matches_graph_contract() {
        assert_eq!(
            channel_files_folder_path("t1", "c2"),
            "/teams/t1/channels/c2/filesFolder"
        );
        assert_eq!(
            folder_children_path("d1", "i2", 0),
            "/drives/d1/items/i2/children?$top=1"
        );
    }

    #[test]
    fn is_video_matches_mime_then_extension() {
        assert!(is_video("a.mp4", Some("video/mp4")));
        assert!(is_video("a.MOV", None)); // ext fallback
        assert!(is_video("noext", Some("video/quicktime"))); // mime fallback
        assert!(!is_video("notes.pdf", Some("application/pdf")));
        assert!(!is_video("clip.mp4.txt", None)); // trailing ext wins
        assert!(!is_video("noext", None));
    }

    #[test]
    fn parse_keeps_videos_skips_rest() {
        let value = json!({
            "value": [
                {"id": "r1", "name": "Weekly Sync-20260924.mp4", "size": 48211,
                 "webUrl": "https://x/r1",
                 "@microsoft.graph.downloadUrl": "https://x/dl1",
                 "createdDateTime": "2026-09-24T09:00:00Z",
                 "lastModifiedDateTime": "2026-09-24T10:00:00Z",
                 "file": {"mimeType": "video/mp4"},
                 "video": {"durationMillis": 3723000},
                 "parentReference": {"driveId": "d1"}},
                {"id": "f9", "name": "Recordings", "folder": {}},
                {"id": "n2", "name": "notes.pdf",
                 "file": {"mimeType": "application/pdf"}},
                {"name": "no-id-skipped", "file": {"mimeType": "video/mp4"}},
            ]
        });
        let rows = parse_recordings_response(&value, RecordingSource::OneDrive);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.name, "Weekly Sync-20260924.mp4");
        assert_eq!(r.size, 48211);
        assert_eq!(r.drive_id.as_deref(), Some("d1"));
        assert_eq!(r.duration_ms, Some(3723000));
        assert_eq!(r.download_url.as_deref(), Some("https://x/dl1"));
        assert_eq!(r.source.label(), "OneDrive");
    }

    #[test]
    fn parse_tolerates_unknown_shapes() {
        for raw in [json!({}), json!({"value": []}), json!(null), json!("bogus")] {
            assert!(parse_recordings_response(&raw, RecordingSource::OneDrive).is_empty());
        }
    }

    #[test]
    fn source_labels_channel_rows() {
        let s = RecordingSource::Channel {
            team: "Engineering".into(),
            channel: "general".into(),
        };
        assert_eq!(s.label(), "Engineering > #general");
    }

    fn row(name: &str, modified: Option<&str>) -> RecordingInfo {
        RecordingInfo {
            id: name.into(),
            name: name.into(),
            size: 0,
            mime: None,
            web_url: None,
            download_url: None,
            drive_id: None,
            created: None,
            modified: modified.map(str::to_string),
            duration_ms: None,
            source: RecordingSource::OneDrive,
        }
    }

    #[test]
    fn sort_newest_orders_by_modified_then_name() {
        let mut rows = vec![
            row("b.mp4", Some("2026-09-20T00:00:00Z")),
            row("a.mp4", Some("2026-09-24T00:00:00Z")),
            row("c.mp4", None),
            row("a2.mp4", Some("2026-09-24T00:00:00Z")),
        ];
        sort_newest(&mut rows);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["a.mp4", "a2.mp4", "b.mp4", "c.mp4"]
        );
    }

    #[test]
    fn not_found_matches_graph_404_chain() {
        let e = anyhow::anyhow!("HTTP 404 for https://graph/x: {{\"error\":{{\"code\":\"itemNotFound\"}}}}");
        assert!(is_not_found(&e));
        let e = anyhow::anyhow!("HTTP 403 for https://graph/x: forbidden");
        assert!(!is_not_found(&e));
    }
}
