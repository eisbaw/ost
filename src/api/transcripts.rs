//! Meeting transcripts in OneDrive + SharePoint (Graph driveItems).
//!
//! One searchable list of every meeting
//! transcript the user can access, with speaker-turn display. Teams
//! stores non-channel meeting transcripts in the organizer's OneDrive
//! `Recordings` folder (`.vtt` next to the `.mp4`) and channel-meeting
//! transcripts in the channel's SharePoint filesFolder `Recordings`
//! folder; both are plain `.vtt` driveItems, so list/download reuse
//! the files-stack shapes.
//!
//! Mirrors the recordings module: same Graph paths, same bounded channel
//! fan-out, `.vtt` filter instead of the video filter. VTT bytes ride
//! the existing files download (`/drives/{d}/items/{i}/content`);
//! cue parsing is left to the caller.
//!
//! Docs grounding (Microsoft Graph v1.0; endpoints verified live,
//! 200 + empty on a test tenant):
//! - OneDrive folder children:
//!   `GET /me/drive/root:/Recordings:/children?$top=N`
//!   (missing folder 404s -> empty list, never an error)
//! - OneDrive search: `GET /me/drive/root/search(q='{q}')?$top=N`
//! - per-drive search: `GET /drives/{id}/root/search(q='{q}')?$top=N`
//! - channel filesFolder:
//!   `GET /teams/{team}/channels/{channel}/filesFolder`
//! - folder children: `GET /drives/{d}/items/{i}/children?$top=N`
//!
//! Auth: existing Graph token (`/.default`). No scope widening. The
//! Graph transcript API path (`OnlineMeetingTranscript.Read.All`)
//! is not used (it needs tenant admin consent); this module is
//! drive-backed only. A 403 surfaces as the call's detail.
//!
//! `.docx` twin: a live probe of a OneDrive found zero Teams-written
//! transcript twins (7 unrelated `.docx`, none in `Recordings`, none
//! transcript-named). This module accepts `.docx`/`.doc` NARROWLY:
//! only when the stem contains `transcript` (case-insensitive; covers `transcription`),
//! so those 7 unrelated files still filter out. Exact rules live on
//! [`is_transcript`].
//!
//! Full-drive search fallback: meetings the
//! user did NOT organize keep their `.vtt`/transcript-`.docx` in the
//! organizer's OneDrive — unreachable from the signed-in drive (see
//! HONEST GAP below). But transcript-ish files can also sit OUTSIDE
//! `Recordings` on reachable drives (renames, moves, non-Teams
//! writers). When the folder scan finds zero rows,
//! [`list_transcripts_data`] falls back to drive-search windows (see
//! [`FALLBACK_QUERY_ONEDRIVE_VTT`], [`FALLBACK_QUERY_TRANSCRIPT`]):
//! 2 queries on OneDrive + `transcript` on each resolved channel
//! drive, parsed by the same predicate, deduped. Bounded, and only
//! on empty folder scans (the common hit path pays zero extra).
//!
//! HONEST GAP (organizer-owned transcripts): non-channel meetings
//! store the transcript in the ORGANIZER's OneDrive `Recordings`
//! folder. The signed-in user's drive search cannot see another
//! user's drive, and the Graph transcript API
//! (`GET /me/onlineMeetings/{id}/transcripts`, app permission
//! `OnlineMeetingTranscript.Read.All`) needs tenant-admin consent,
//! which is often not granted. So: transcripts of meetings organized
//! by someone else are unreachable from this module, by design.
//! Callers should surface this in their empty state; nothing here
//! retries or fabricates them.
//!
//! Cost of the raised caps: worst case per
//! list call is 1 teams scan + 100 filesFolder lookups + 100 folder
//! scans + 100 Recordings lists = ~301 requests (was ~151 at the
//! 50/50 caps), all sequential, each small JSON. The search path is
//! 1 OneDrive search + 100 drive searches. The fallback adds at most
//! 2 + 100 requests and only fires on empty folder scans.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

/// Teams' on-disk recordings folder name (OneDrive + channel folders).
/// Transcripts live next to the recordings (same stem, `.vtt`).
pub const TRANSCRIPTS_FOLDER: &str = "Recordings";

/// Per-request result cap for list + search windows.
/// Raised 50 -> 100 (channel-heavy
/// tenants truncated at 50; cost is one `$top=100` window per drive).
pub const TRANSCRIPTS_MAX_LIMIT: usize = 100;

/// Upper bound on channel drives fanned out to (list + search stay
/// one teams scan plus bounded folder/search windows).
/// Raised 50 -> 100: tenants with more
/// than 50 channels silently lost transcript folders. Worst-case
/// cost per list call roughly doubles (~151 -> ~301 small
/// sequential requests; see the module docs).
pub const MAX_CHANNEL_DRIVES: usize = 100;

/// Fallback drive-search queries: when the
/// `Recordings`-folder scan finds zero rows, the list falls back to
/// full-drive search so transcript-ish files OUTSIDE `Recordings`
/// still surface. `.vtt` catches bare caption files; `transcript`
/// catches Teams transcript twins (`*transcript*.docx`) by name.
pub const FALLBACK_QUERY_ONEDRIVE_VTT: &str = ".vtt";
pub const FALLBACK_QUERY_TRANSCRIPT: &str = "transcript";

/// Clamp a result limit into the `1..=100` window.
pub fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, TRANSCRIPTS_MAX_LIMIT)
}

/// Minimal query encoder: keeps unreserved chars, percent-encodes the
/// rest (spaces -> %20, etc). Graph single-quote escaping (`''`) is
/// applied by the search path builder before this runs. (Duplicate of
/// the recordings/filesearch encoder: this module is self-contained
/// on purpose.)
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
pub fn transcripts_children_path(limit: usize) -> String {
    format!(
        "/me/drive/root:/{}:/children?$top={}",
        TRANSCRIPTS_FOLDER,
        clamp_limit(limit)
    )
}

/// One drive-search window, either the signed-in user's OneDrive
/// (`drive_id=None`) or one channel drive (`Some`). Single quotes in
/// the query double up (Graph escaping) before URL encoding.
pub fn transcripts_search_path(drive_id: Option<&str>, query: &str, limit: usize) -> String {
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
    #[serde(rename = "createdDateTime", default)]
    created: Option<String>,
    #[serde(rename = "lastModifiedDateTime", default)]
    modified: Option<String>,
    #[serde(default)]
    file: Option<FileFacet>,
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
struct ParentRef {
    #[serde(rename = "driveId", default)]
    drive_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Public model
// ---------------------------------------------------------------------------

/// Where one transcript lives: the user's own OneDrive `Recordings`
/// folder or one channel's SharePoint `Recordings` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptSource {
    OneDrive,
    Channel { team: String, channel: String },
}

impl TranscriptSource {
    /// Short provenance label for list rows (`OneDrive`,
    /// `Engineering > #general`).
    pub fn label(&self) -> String {
        match self {
            TranscriptSource::OneDrive => "OneDrive".to_string(),
            TranscriptSource::Channel { team, channel } => {
                format!("{} > #{}", team, channel)
            }
        }
    }
}

/// One meeting transcript (driveItem projection for list/search).
/// VTT bytes download via the existing files download (`drive_id` +
/// `id`); cue parsing is left to the caller.
pub struct TranscriptInfo {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub mime: Option<String>,
    pub web_url: Option<String>,
    pub drive_id: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub source: TranscriptSource,
}

/// True when a driveItem name/mime looks like a transcript.
/// Exact rules:
/// 1. `.vtt` extension (any case) accepts, whatever the mime
///    (Graph sometimes omits it or reports `text/plain`).
/// 2. `text/vtt` mime (case-insensitive) accepts, whatever the
///    name (extensionless caption files).
/// 3. `.docx`/`.doc` extension (any case) accepts ONLY when the
///    stem (name minus extension) contains `transcript`
///    (case-insensitive; covers `transcription`). The Word mime
///    alone does NOT accept: a live probe found 7 unrelated
///    `.docx` on a drive, and this name gate keeps them out.
/// Pure so list, search, fallback, and tests share it.
pub fn is_transcript(name: &str, mime: Option<&str>) -> bool {
    if let Some(m) = mime {
        if m.to_ascii_lowercase() == "text/vtt" {
            return true;
        }
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if ext == "vtt" {
        return true;
    }
    if ext == "docx" || ext == "doc" {
        let stem = match name.rfind('.') {
            Some(i) => &name[..i],
            None => name,
        };
        return stem.to_ascii_lowercase().contains("transcript");
    }
    false
}

fn transcript_from_item(item: DriveItem, source: TranscriptSource) -> Option<TranscriptInfo> {
    let id = item.id.filter(|s| !s.trim().is_empty())?;
    let name = item.name.unwrap_or_else(|| "[unnamed]".to_string());
    if item.folder.is_some() {
        return None;
    }
    if item.file.is_none() {
        return None;
    }
    let mime = item.file.and_then(|f| f.mime_type);
    if !is_transcript(&name, mime.as_deref()) {
        return None;
    }
    Some(TranscriptInfo {
        id,
        name,
        size: item.size.unwrap_or(0),
        mime,
        web_url: item.web_url,
        drive_id: item.parent.and_then(|p| p.drive_id),
        created: item.created,
        modified: item.modified,
        source,
    })
}

/// Parse one children/search response into transcripts. Unknown shapes
/// yield no rows (never fatal); folders, non-transcript files, and
/// items without an id are skipped.
pub fn parse_transcripts_response(
    value: &serde_json::Value,
    source: TranscriptSource,
) -> Vec<TranscriptInfo> {
    let resp: DriveChildrenResponse =
        serde_json::from_value(value.clone()).unwrap_or_default();
    resp.value
        .into_iter()
        .filter_map(|it| transcript_from_item(it, source.clone()))
        .collect()
}

/// Sort newest-first by `modified` (ISO-8601 strings sort
/// lexicographically), falling back to `created`, then name. Merges
/// from several drives/folders land in one stable order.
pub fn sort_newest(files: &mut Vec<TranscriptInfo>) {
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
            tracing::warn!("Transcripts teams scan failed: {:#}", e);
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
                        tracing::warn!("Transcripts filesFolder parse failed: {:#}", e);
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!("Transcripts filesFolder failed: {:#}", e);
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

/// Drop duplicate rows by (`drive_id`, `id`): the search fallback
/// can return the same item twice (`.vtt` + `transcript` windows
/// overlap). First occurrence wins; order otherwise preserved. Pure
/// so tests pin it.
pub fn dedupe_transcripts(rows: &mut Vec<TranscriptInfo>) {
    let mut seen = std::collections::HashSet::new();
    rows.retain(|r| seen.insert((r.drive_id.clone(), r.id.clone())));
}

/// List every meeting transcript: the OneDrive `Recordings` folder plus
/// each channel's `Recordings` folder, newest first, truncated to
/// `limit`. Missing folders (404) and per-channel failures read as
/// empty; only a total failure (no drive answered) errors. When the
/// folder scan finds ZERO rows, a bounded full-drive search fallback
/// fires (OneDrive `.vtt` + `transcript`, `transcript` per channel
/// drive) so transcript-ish files outside `Recordings` still surface;
/// organizer-owned drives stay unreachable (see the module HONEST GAP).
pub async fn list_transcripts_data(
    client: &TeamsClient,
    limit: usize,
) -> Result<Vec<TranscriptInfo>> {
    let limit = clamp_limit(limit);
    let mut all = Vec::new();
    let mut answered = false;

    // Own OneDrive first (the common case stands alone).
    match client.graph_get(&transcripts_children_path(limit)).await {
        Ok(resp) => {
            answered = true;
            let value: serde_json::Value = resp
                .json()
                .await
                .context("Failed to parse Transcripts folder response")?;
            all.extend(parse_transcripts_response(&value, TranscriptSource::OneDrive));
        }
        Err(e) if is_not_found(&e) => {
            answered = true; // no folder yet: empty, not fatal
        }
        Err(e) => {
            tracing::warn!("Transcripts OneDrive list failed: {:#}", e);
        }
    }

    // Channel folders: scan each filesFolder for a `Recordings` dir.
    // (Drives resolve once; the fallback below reuses them.)
    let drives = channel_drives(client).await;
    for drive in &drives {
        let source = TranscriptSource::Channel {
            team: drive.team.clone(),
            channel: drive.channel.clone(),
        };
        let kids: DriveChildrenResponse =
            match client.graph_get(&folder_children_path(&drive.drive_id, &drive.folder_id, limit)).await
            {
                Ok(r) => match r.json().await {
                    Ok(k) => k,
                    Err(e) => {
                        tracing::warn!("Transcripts channel scan parse failed: {:#}", e);
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!("Transcripts channel scan failed: {:#}", e);
                    continue;
                }
            };
        answered = true;
        let rec = kids.value.into_iter().find(|it| {
            it.folder.is_some() && it.name.as_deref() == Some(TRANSCRIPTS_FOLDER)
        });
        let Some(rec) = rec else { continue };
        let Some(rec_id) = rec.id else { continue };
        let resp = match client
            .graph_get(&folder_children_path(&drive.drive_id, &rec_id, limit))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Transcripts channel list failed: {:#}", e);
                continue;
            }
        };
        let value: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Transcripts channel parse failed: {:#}", e);
                continue;
            }
        };
        all.extend(parse_transcripts_response(&value, source));
    }

    if !answered {
        bail!("no drive answered the transcripts list");
    }

    // Empty folder scan -> bounded full-drive search fallback.
    // OneDrive gets both fallback queries
    // (`.vtt` catches bare caption files anywhere; `transcript`
    // catches transcript twins by name); each channel drive gets the
    // `transcript` query only (channel `.vtt` stragglers outside
    // `Recordings` are rare; this halves the fallback fan-out).
    // Same predicate, same parse; failures read as empty.
    if all.is_empty() {
        for q in [FALLBACK_QUERY_ONEDRIVE_VTT, FALLBACK_QUERY_TRANSCRIPT] {
            match client.graph_get(&transcripts_search_path(None, q, limit)).await {
                Ok(resp) => match resp.json().await {
                    Ok(v) => {
                        let v: serde_json::Value = v;
                        all.extend(parse_transcripts_response(&v, TranscriptSource::OneDrive));
                    }
                    Err(e) => {
                        tracing::warn!("Transcripts fallback parse failed: {:#}", e);
                    }
                },
                Err(e) => {
                    tracing::warn!("Transcripts fallback search failed: {:#}", e);
                }
            }
        }
        for drive in &drives {
            let source = TranscriptSource::Channel {
                team: drive.team.clone(),
                channel: drive.channel.clone(),
            };
            match client
                .graph_get(&transcripts_search_path(
                    Some(&drive.drive_id),
                    FALLBACK_QUERY_TRANSCRIPT,
                    limit,
                ))
                .await
            {
                Ok(resp) => match resp.json().await {
                    Ok(v) => {
                        let v: serde_json::Value = v;
                        all.extend(parse_transcripts_response(&v, source));
                    }
                    Err(e) => {
                        tracing::warn!("Transcripts fallback parse failed: {:#}", e);
                    }
                },
                Err(e) => {
                    tracing::warn!("Transcripts fallback search failed: {:#}", e);
                }
            }
        }
        dedupe_transcripts(&mut all);
    }

    sort_newest(&mut all);
    all.truncate(limit);
    Ok(all)
}

/// Search every transcript by name: the OneDrive drive-search plus one
/// drive-search per channel drive, predicate-filtered (`.vtt` +
/// transcript `.docx`; see [`is_transcript`]), newest first,
/// truncated to `limit`. Empty queries are rejected before any
/// network.
pub async fn search_transcripts_data(
    client: &TeamsClient,
    query: &str,
    limit: usize,
) -> Result<Vec<TranscriptInfo>> {
    if query.trim().is_empty() {
        bail!("empty query");
    }
    let limit = clamp_limit(limit);
    let mut all = Vec::new();

    // OneDrive answers or the error propagates (`?`): reaching past
    // here means one drive answered.
    let resp = client
        .graph_get(&transcripts_search_path(None, query, limit))
        .await?;
    let value: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse transcripts search response")?;
    all.extend(parse_transcripts_response(&value, TranscriptSource::OneDrive));

    for drive in channel_drives(client).await {
        let source = TranscriptSource::Channel {
            team: drive.team.clone(),
            channel: drive.channel.clone(),
        };
        let resp = match client
            .graph_get(&transcripts_search_path(Some(&drive.drive_id), query, limit))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Transcripts drive search failed: {:#}", e);
                continue;
            }
        };
        let value: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Transcripts drive search parse failed: {:#}", e);
                continue;
            }
        };
        all.extend(parse_transcripts_response(&value, source));
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
        assert_eq!(clamp_limit(100), 100);
        assert_eq!(clamp_limit(500), 100);
    }

    #[test]
    fn caps_are_raised_to_100() {
        // 50 -> 100 on both caps.
        assert_eq!(TRANSCRIPTS_MAX_LIMIT, 100);
        assert_eq!(MAX_CHANNEL_DRIVES, 100);
    }

    #[test]
    fn children_path_shape_matches_graph_contract() {
        assert_eq!(
            transcripts_children_path(999),
            "/me/drive/root:/Recordings:/children?$top=100"
        );
    }

    #[test]
    fn search_paths_shape_matches_graph_contract() {
        assert_eq!(
            transcripts_search_path(None, ".vtt", 999),
            "/me/drive/search(q='.vtt')?$top=100"
        );
        assert_eq!(
            transcripts_search_path(None, "q3 review", 999),
            "/me/drive/search(q='q3%20review')?$top=100"
        );
        assert_eq!(
            transcripts_search_path(Some("d9"), "tom's", 10),
            "/drives/d9/root/search(q='tom%27%27s')?$top=10"
        );
    }

    #[test]
    fn fallback_queries_pin_search_paths() {
        // The list fallback fires these exact windows on empty scans.
        assert_eq!(
            transcripts_search_path(None, FALLBACK_QUERY_ONEDRIVE_VTT, 100),
            "/me/drive/search(q='.vtt')?$top=100"
        );
        assert_eq!(
            transcripts_search_path(None, FALLBACK_QUERY_TRANSCRIPT, 100),
            "/me/drive/search(q='transcript')?$top=100"
        );
        assert_eq!(
            transcripts_search_path(Some("d9"), FALLBACK_QUERY_TRANSCRIPT, 100),
            "/drives/d9/root/search(q='transcript')?$top=100"
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
    fn is_transcript_matches_vtt_ext_then_mime() {
        assert!(is_transcript("a.vtt", None)); // ext, mime missing
        assert!(is_transcript("a.vtt", Some("text/vtt")));
        assert!(is_transcript("a.VTT", Some("text/plain"))); // Graph reality
        assert!(is_transcript("noext", Some("text/vtt"))); // mime fallback
        assert!(!is_transcript("clip.mp4", Some("video/mp4")));
        assert!(!is_transcript("notes.pdf", Some("application/pdf")));
        assert!(!is_transcript("a.vtt.txt", None)); // trailing ext wins
        assert!(!is_transcript("noext", None));
    }

    #[test]
    fn is_transcript_accepts_transcript_docx_only() {
        // Rule 3: docx/doc + `transcript` in the stem.
        assert!(is_transcript("Weekly Sync transcript.docx", None));
        assert!(is_transcript(
            "Weekly Sync transcript.docx",
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        ));
        assert!(is_transcript("Meeting TRANSCRIPTION.doc", None)); // case + -ion
        assert!(is_transcript("a.DOCX", Some("text/vtt"))); // rule 2 still wins
        // The 7 unrelated .docx from the live probe still filter out:
        assert!(!is_transcript("notes.docx", None));
        assert!(!is_transcript(
            "notes.docx",
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        )); // Word mime alone is NOT enough
        assert!(!is_transcript("notes.doc", None));
        assert!(!is_transcript("transcript", None)); // no ext, no mime
        assert!(!is_transcript("transcript.pdf", None)); // wrong ext
    }

    #[test]
    fn parse_keeps_transcripts_skips_rest() {
        let value = json!({
            "value": [
                {"id": "t1", "name": "Weekly Sync-20260924.vtt", "size": 4211,
                 "webUrl": "https://x/t1",
                 "createdDateTime": "2026-09-24T09:00:00Z",
                 "lastModifiedDateTime": "2026-09-24T10:00:00Z",
                 "file": {"mimeType": "text/vtt"},
                 "parentReference": {"driveId": "d1"}},
                {"id": "t2", "name": "Q3 Review transcript.docx", "size": 9001,
                 "file": {"mimeType": "application/vnd.openxmlformats-officedocument.wordprocessingml.document"},
                 "parentReference": {"driveId": "d1"}},
                {"id": "r1", "name": "Weekly Sync-20260924.mp4", "size": 48211,
                 "file": {"mimeType": "video/mp4"}},
                {"id": "f9", "name": "Recordings", "folder": {}},
                {"id": "n2", "name": "notes.docx",
                 "file": {"mimeType": "application/vnd.openxmlformats-officedocument.wordprocessingml.document"}},
                {"name": "no-id-skipped", "file": {"mimeType": "text/vtt"}},
            ]
        });
        let rows = parse_transcripts_response(&value, TranscriptSource::OneDrive);
        assert_eq!(rows.len(), 2);
        let t = &rows[0];
        assert_eq!(t.name, "Weekly Sync-20260924.vtt");
        assert_eq!(t.size, 4211);
        assert_eq!(t.drive_id.as_deref(), Some("d1"));
        assert_eq!(t.source.label(), "OneDrive");
        assert_eq!(rows[1].name, "Q3 Review transcript.docx");
    }

    #[test]
    fn dedupe_drops_same_drive_and_id() {
        fn row(id: &str, drive: Option<&str>, name: &str) -> TranscriptInfo {
            TranscriptInfo {
                id: id.into(),
                name: name.into(),
                size: 0,
                mime: None,
                web_url: None,
                drive_id: drive.map(str::to_string),
                created: None,
                modified: None,
                source: TranscriptSource::OneDrive,
            }
        }
        let mut rows = vec![
            row("t1", Some("d1"), "a.vtt"),
            row("t1", Some("d1"), "a.vtt"), // fallback double-hit
            row("t1", Some("d2"), "a.vtt"), // same id, other drive: kept
            row("t2", Some("d1"), "b.vtt"),
        ];
        dedupe_transcripts(&mut rows);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].drive_id.as_deref(), Some("d1"));
        assert_eq!(rows[1].drive_id.as_deref(), Some("d2"));
        assert_eq!(rows[2].id, "t2");
    }

    #[test]
    fn parse_tolerates_unknown_shapes() {
        for raw in [json!({}), json!({"value": []}), json!(null), json!("bogus")] {
            assert!(parse_transcripts_response(&raw, TranscriptSource::OneDrive).is_empty());
        }
    }

    #[test]
    fn source_labels_channel_rows() {
        let s = TranscriptSource::Channel {
            team: "Engineering".into(),
            channel: "general".into(),
        };
        assert_eq!(s.label(), "Engineering > #general");
    }

    fn row(name: &str, modified: Option<&str>) -> TranscriptInfo {
        TranscriptInfo {
            id: name.into(),
            name: name.into(),
            size: 0,
            mime: None,
            web_url: None,
            drive_id: None,
            created: None,
            modified: modified.map(str::to_string),
            source: TranscriptSource::OneDrive,
        }
    }

    #[test]
    fn sort_newest_orders_by_modified_then_name() {
        let mut rows = vec![
            row("b.vtt", Some("2026-09-20T00:00:00Z")),
            row("a.vtt", Some("2026-09-24T00:00:00Z")),
            row("c.vtt", None),
            row("a2.vtt", Some("2026-09-24T00:00:00Z")),
        ];
        sort_newest(&mut rows);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["a.vtt", "a2.vtt", "b.vtt", "c.vtt"]
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
