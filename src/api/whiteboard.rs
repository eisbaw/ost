//! Whiteboard boards via the RE-mapped REST+ODSP surface (rewhiteboard).
//!
//! Legacy Azure boards live behind `https://whiteboard.microsoft.com/api/`
//! (`WhiteboardServiceInternal`, `lz3_72609/776907`); current Fluid boards
//! are `{name}.whiteboard` files in OneDrive `Whiteboards/` synced over
//! ODSP (`opStream/joinSession` + socket.io, NOT the vestigial
//! `wss://whiteboard.microsoft.com/sync` which has no client consumer).
//! Graph carries the file/share calls, SharePoint the proof-token and
//! redeem calls. Auth is user-delegated AAD (client
//! `95de633a-083e-42f5-b444-a4295d8e9314`, scope
//! `https://whiteboard.microsoft.com/.default` for legacy REST,
//! `https://graph.microsoft.com/.default` for Graph,
//! `{siteOrigin}/Files.ReadWrite.All` for SPO). Our Teams token may lack
//! the whiteboard scope, so every shared-board call is live-gated: Swift
//! disables with the server's reason when unreachable, local ink stays
//! the offline fallback.
//!
//! CAPTURE-TIME PINS (phase-2 P1-P8 close these on the trial tenant):
//! host paths below are verbatim from bundles; `X-Session-Token` mint is
//! untraced (G4, read off the wire); Fluid snapshot/op bytes need a live
//! file (G3); default `hi.MEETING/ANNOTATION/TABAPP` names live in an
//! unfetched chunk (G1).

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

// -- Identity (shell env map + `lz2_72684/931384`, prod) --

/// Legacy REST base (`U.Z.whiteboard`).
pub const WHITEBOARD_BASE: &str = "https://whiteboard.microsoft.com/api/";
/// Whiteboard web app origin (tab/meeting URLs).
pub const WHITEBOARD_ORIGIN: &str = "https://app.whiteboard.microsoft.com";
/// First-party client id (`msalClientId`, `wb_app.shell.js`).
pub const WHITEBOARD_APP_ID: &str = "95de633a-083e-42f5-b444-a4295d8e9314";
/// Legacy REST resource + default scope.
pub const WHITEBOARD_RESOURCE: &str = "https://whiteboard.microsoft.com";
pub const WHITEBOARD_SCOPE_DEFAULT: &str = "https://whiteboard.microsoft.com/.default";
/// OneDrive folder + extension (`674532.xV`, `83802`).
pub const WHITEBOARD_FOLDER: &str = "Whiteboards";
pub const WHITEBOARD_EXT: &str = "whiteboard";
pub const WHITEBOARD_MIGRATION_DIR: &str = "Whiteboards/Migration";
/// Admin module identity (`WhiteboardAdmin.psm1`, MSAL public client).
pub const WHITEBOARD_ADMIN_CLIENT_ID: &str = "1950a258-227b-4e31-a9cf-717495945fc2";
pub const WHITEBOARD_ADMIN_SCOPE: &str = "https://whiteboard.microsoft.com/Whiteboards.ReadWrite";

// -- Wire types (legacy REST) --

#[derive(Debug, Deserialize)]
struct WireBoard {
    id: Option<String>,
    // `folioId` also arrives (2021 capture); no consumer reads it.
    title: Option<String>,
    #[serde(rename = "ownerId")]
    owner_id: Option<String>,
    #[serde(rename = "ownerTenantId")]
    owner_tenant_id: Option<String>,
    #[serde(rename = "isShared")]
    is_shared: Option<bool>,
    #[serde(rename = "fluidFileId")]
    fluid_file_id: Option<String>,
    #[serde(rename = "meetingId")]
    meeting_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireShare {
    #[serde(rename = "shareUri")]
    share_uri: Option<String>,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireCollaborators {
    users: Option<Vec<WireCollaborator>>,
}

#[derive(Debug, Deserialize)]
struct WireCollaborator {
    #[serde(rename = "userId")]
    user_id: Option<String>,
    #[serde(rename = "tenantId")]
    tenant_id: Option<String>,
    // `permissions` also arrives; no consumer reads it.
}

#[derive(Debug, Deserialize)]
struct WireMyPermissions {
    permissions: Option<WirePerms>,
}

#[derive(Debug, Deserialize)]
struct WirePerms {
    #[serde(rename = "CanEdit")]
    can_edit: Option<bool>,
    #[serde(rename = "CanRead")]
    can_read: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct WireRedeem {
    id: Option<String>,
    #[serde(rename = "fluidShareLink")]
    fluid_share_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireContentPage {
    gchs: Option<serde_json::Value>,
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireTreeInit {
    #[serde(rename = "treeInit")]
    tree_init: Option<serde_json::Value>,
}

// -- Public model --

/// One legacy Azure board (list/get/create projection).
pub struct WhiteboardInfo {
    pub id: String,
    pub title: String,
    pub owner_id: Option<String>,
    /// Owner tenant (share tokens read `{shareGuid}_{tenantGuid}`).
    pub owner_tenant_id: Option<String>,
    pub is_shared: bool,
    pub fluid_file_id: Option<String>,
    /// Meeting id when the board was created in a meeting.
    pub meeting_id: Option<String>,
}

/// One share link (`PUT .../share` / `GET v2.0/.../share`).
pub struct WhiteboardShare {
    pub share_uri: Option<String>,
    pub token: Option<String>,
}

/// One collaborator (`GET .../collaborators`).
pub struct WhiteboardCollaborator {
    pub user_id: String,
    pub tenant_id: Option<String>,
}

/// My permissions on a board (`.../collaborators/me/permissions`).
pub struct WhiteboardMyPerms {
    pub can_edit: bool,
    pub can_read: bool,
}

/// Redeemed share token (`GET .../redeem/{token}`).
pub struct WhiteboardRedeem {
    pub id: String,
    pub fluid_share_link: Option<String>,
}

/// One legacy content page (`GET v1.0/content/{id}?batch=20`).
pub struct WhiteboardContent {
    pub gchs: serde_json::Value,
    pub next: Option<String>,
}

/// Session tree init (`GET v1.0/content/sessions/{id}`).
pub struct WhiteboardTreeInit {
    pub tree_init: serde_json::Value,
}

/// ODSP join-session answer (`POST .../opStream/joinSession`).
pub struct WhiteboardJoin {
    pub socket_url: String,
    pub socket_token: Option<String>,
    pub refresh_secs: Option<u64>,
}

/// Fluid file locator for open/sync (drive + item + site).
pub struct FluidFileInfo {
    pub site_url: String,
    pub drive_id: String,
    pub item_id: String,
}

fn board_from_wire(w: WireBoard) -> Result<WhiteboardInfo> {
    let id = w.id.unwrap_or_default().trim().to_string();
    if id.is_empty() {
        bail!("whiteboard response has no id");
    }
    Ok(WhiteboardInfo {
        id,
        title: w.title.unwrap_or_default(),
        owner_id: w.owner_id.filter(|s| !s.is_empty()),
        owner_tenant_id: w.owner_tenant_id.filter(|s| !s.is_empty()),
        is_shared: w.is_shared.unwrap_or(false),
        fluid_file_id: w.fluid_file_id.filter(|s| !s.is_empty()),
        meeting_id: w.meeting_id.filter(|s| !s.is_empty()),
    })
}

// -- Legacy REST paths (pure so tests pin them verbatim) --
//
// Verbatim anchors: `const L="v1.0/whiteboards",W="v3.0/whiteboards"`;
// `createBoard`: `new URL(u.Z.whiteboard+"v2.0/whiteboards")`;
// `createShareLink`: `` `${u.Z.whiteboard}v1.0/whiteboards/${e}/share` ``;
// `redeemShareToken`: `` `${u.Z.whiteboard}v1.0/whiteboards/redeem/${e}` ``.

/// POST: create one Azure board. Body: `{Whiteboard:{Title}}` or `{}`.
pub fn wb_create_path() -> String {
    "v2.0/whiteboards".to_string()
}

/// GET: list my Azure boards.
pub fn wb_list_path() -> String {
    "v1.0/whiteboards".to_string()
}

/// GET/PATCH/DELETE: one Azure board.
pub fn wb_board_path(board_id: &str) -> String {
    format!("v1.0/whiteboards/{}", board_id.trim())
}

/// PUT: mint a share link. DELETE on the same path revokes it.
pub fn wb_share_path(board_id: &str) -> String {
    format!("v1.0/whiteboards/{}/share", board_id.trim())
}

/// GET: fetch the share link (+token per 2021 capture).
pub fn wb_share_fetch_path(board_id: &str) -> String {
    format!("v2.0/whiteboards/{}/share", board_id.trim())
}

/// GET/DELETE: collaborators (`{users:[{userId,tenantId,permissions}]}`).
pub fn wb_collaborators_path(board_id: &str) -> String {
    format!("v1.0/whiteboards/{}/collaborators", board_id.trim())
}

/// GET: my permissions (`{permissions:{CanEdit,CanRead}}`).
pub fn wb_my_permissions_path(board_id: &str) -> String {
    format!("v1.0/whiteboards/{}/collaborators/me/permissions", board_id.trim())
}

/// PUT: all-collaborators permissions (JSON perms body).
pub fn wb_all_permissions_path(board_id: &str) -> String {
    format!("v1.0/whiteboards/{}/collaborators/all/permissions", board_id.trim())
}

/// GET: share-token permissions (v3).
pub fn wb_share_permissions_path(board_id: &str, token: &str) -> String {
    format!("v3.0/whiteboards/{}/share/{}/permissions", board_id.trim(), token.trim())
}

/// PUT: update share-token permissions (v3, JSON perms body).
pub fn wb_share_update_path(board_id: &str, token: &str) -> String {
    format!("v3.0/whiteboards/{}/share/{}", board_id.trim(), token.trim())
}

/// GET: redeem a share token (`{id,fluidShareLink}`).
pub fn wb_redeem_path(token: &str) -> String {
    format!("v1.0/whiteboards/redeem/{}", token.trim())
}

/// GET thumbnail / POST png (`sendThumbnail` posts `image/png`).
pub fn wb_thumbnail_path(board_id: &str) -> String {
    format!("v1.0/whiteboards/{}/thumbnail", board_id.trim())
}

/// GET: legacy content page. Headers `X-MinimumReadVersion`,
/// `X-MaximumReadVersion`, `X-WriteVersion`; next page via
/// `next.substring(5)` when present.
pub fn wb_content_path(board_id: &str) -> String {
    format!("v1.0/content/{}?batch=20", board_id.trim())
}

/// GET: session tree init (`{treeInit}`).
pub fn wb_session_tree_path(session_id: &str) -> String {
    format!("v1.0/content/sessions/{}", session_id.trim())
}

/// GET: Azure board for migration.
pub fn wb_migration_path(board_id: &str) -> String {
    format!("v1.0/migration/azureWhiteboards/{}", board_id.trim())
}

/// POST/DELETE: fluid file info (`{FileId,version,ShareLink?}`).
pub fn wb_fluid_file_info_path(board_id: &str) -> String {
    format!("v1.0/whiteboards/{}/fluidFileInfo", board_id.trim())
}

/// DELETE: my user-level fluid file info.
pub fn wb_user_fluid_file_info_path() -> String {
    "v1.0/whiteboards/fluidFileInfo".to_string()
}

/// POST: abuse report (`/report` or `/report/anonymous`).
pub fn wb_report_path(board_id: &str, anonymous: bool) -> String {
    if anonymous {
        format!("v1.0/whiteboards/{}/report/anonymous", board_id.trim())
    } else {
        format!("v1.0/whiteboards/{}/report", board_id.trim())
    }
}

/// GET (also POST in admin): tenant enabled settings.
pub fn wb_enabled_path() -> String {
    "v1.0/whiteboards/enabled".to_string()
}

// -- Graph board-file paths (pure so tests pin them verbatim) --
//
// Verbatim anchors (`GraphServiceInternal`, `lz3_32827/832827`):
// `getMyDrive`: `me/drive?select=owner,id,sharepointIds`;
// `getFilesFolder`: `/teams/{gid}/channels/{cid}/filesFolder?
// $select=parentReference,webUrl`; `getFluidBoardMetadata`:
// `drives/{d}/items/{i}?$select=id,name,createdBy,
// createdDateTime,lastModifiedDateTime,webUrl,sharepointIds`;
// `createFluidShareLink`: `drives/{d}/items/{i}/createLink` with
// `{type,scope,recipients,sendNotification}` (+`Prefer:
// PartialSuccess` when scope=users; beta host when recipients —
// phase-2 P4 pins); `getPermissionForEncodedSharingURL`:
// `shares/{enc}/permission?$select=link` (+`Prefer:
// redeemSharingLinkIfNecessary` — phase-2 P4 pin);
// `getServicePlanNames`: `me/licenseDetails?$select=servicePlans`.

/// GET: my drive locator (`{url:siteUrl,driveId}`, memoized).
pub fn wb_graph_my_drive_path() -> String {
    "/me/drive?$select=owner,id,sharepointIds".to_string()
}

/// GET: channel files folder (board drive for channel boards).
pub fn wb_graph_files_folder_path(group_id: &str, channel_id: &str) -> String {
    format!(
        "/teams/{}/channels/{}/filesFolder?$select=parentReference,webUrl",
        group_id.trim(),
        channel_id.trim()
    )
}

/// GET: one Fluid board file's metadata.
pub fn wb_graph_fluid_metadata_path(drive_id: &str, item_id: &str) -> String {
    format!(
        "/drives/{}/items/{}?$select=id,name,createdBy,createdDateTime,lastModifiedDateTime,webUrl,sharepointIds",
        drive_id.trim(),
        item_id.trim()
    )
}

/// POST: mint a Fluid share link on one board file.
pub fn wb_graph_create_link_path(drive_id: &str, item_id: &str) -> String {
    format!("/drives/{}/items/{}/createLink", drive_id.trim(), item_id.trim())
}

/// GET: permission (link facet) for an encoded sharing URL.
pub fn wb_graph_share_permission_path(encoded_url: &str) -> String {
    format!("/shares/{}/permission?$select=link", encoded_url.trim())
}

/// DELETE: one permission (share link) on a board file.
pub fn wb_graph_delete_permission_path(drive_id: &str, item_id: &str, perm_id: &str) -> String {
    format!(
        "/drives/{}/items/{}/permissions/{}",
        drive_id.trim(),
        item_id.trim(),
        perm_id.trim()
    )
}

/// DELETE/PATCH: one board file (delete, or rename via PATCH).
pub fn wb_graph_drive_item_path(drive_id: &str, item_id: &str) -> String {
    format!("/drives/{}/items/{}", drive_id.trim(), item_id.trim())
}

/// GET: license service plans (ODB-license gate: "No valid ODB
/// license" without OneDrive).
pub fn wb_graph_license_path() -> String {
    "/me/licenseDetails?$select=servicePlans".to_string()
}

/// POST body for `createLink`: `{type,scope,recipients,
/// sendNotification}`. `scope` is `organization` for a company link
/// or `users` for participant links (`360729.mQ`:
/// CompanyShareLink→organization, ParticipantShareLink→users);
/// `recipients` carries `[{objectId}]` for `users` scope, empty
/// otherwise. Pure so tests pin it.
pub fn wb_create_link_body(
    link_type: &str,
    scope: &str,
    recipient_object_ids: &[&str],
    send_notification: bool,
) -> Result<serde_json::Value> {
    let link_type = link_type.trim();
    let scope = scope.trim();
    if link_type.is_empty() {
        bail!("empty link type");
    }
    if scope != "organization" && scope != "users" {
        bail!("scope must be 'organization' or 'users'");
    }
    let recipients: Vec<serde_json::Value> = recipient_object_ids
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| serde_json::json!({ "objectId": s }))
        .collect();
    if scope == "users" && recipients.is_empty() {
        bail!("users scope needs at least one recipient");
    }
    Ok(serde_json::json!({
        "type": link_type,
        "scope": scope,
        "recipients": recipients,
        "sendNotification": send_notification,
    }))
}

/// PATCH body for renaming a board file:
/// `{name:"${title}.whiteboard"}`. Pure so tests pin it.
pub fn wb_graph_rename_body(title: &str) -> Result<serde_json::Value> {
    let title = title.trim();
    if title.is_empty() {
        bail!("empty board title");
    }
    Ok(serde_json::json!({ "name": format!("{}.{}", title, WHITEBOARD_EXT) }))
}

/// The `Prefer` header value the client sends with a `createLink`
/// call for `scope`, if any (`PartialSuccess` for users scope;
/// None for organization). The transport does not send it yet —
/// phase-2 P4 wires headers — so this pins the VALUE now for the
/// wiring to consume. Pure so tests pin it.
pub fn wb_create_link_prefer(scope: &str) -> Option<&'static str> {
    match scope.trim() {
        "users" => Some("PartialSuccess"),
        _ => None,
    }
}

// -- Legacy REST bodies (pure so tests pin them verbatim) --

/// POST body for `createBoard`: `{Whiteboard:{Title}}`, or `{}` for
/// an untitled board (`const t=e?{Whiteboard:{Title:e}}:{}`).
pub fn wb_create_body(title: Option<&str>) -> serde_json::Value {
    match title.map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => serde_json::json!({ "Whiteboard": { "Title": t } }),
        None => serde_json::json!({}),
    }
}

/// PATCH body for `renameBoard`: `{title}`.
pub fn wb_rename_body(title: &str) -> Result<serde_json::Value> {
    let title = title.trim();
    if title.is_empty() {
        bail!("empty board title");
    }
    Ok(serde_json::json!({ "title": title }))
}

/// POST body for `setFluidFileInfo`:
/// `{FileId,version,ShareLink?}` (mark migrated).
pub fn wb_fluid_file_info_body(
    file_id: &str,
    version: &str,
    share_link: Option<&str>,
) -> Result<serde_json::Value> {
    let file_id = file_id.trim();
    let version = version.trim();
    if file_id.is_empty() {
        bail!("empty file id");
    }
    if version.is_empty() {
        bail!("empty version");
    }
    let mut body = serde_json::json!({ "FileId": file_id, "version": version });
    if let Some(link) = share_link.map(str::trim).filter(|s| !s.is_empty()) {
        body["ShareLink"] = serde_json::Value::String(link.to_string());
    }
    Ok(body)
}

/// The next legacy content page cursor: `next.substring(5)` — the
/// server's `next` value with its 5-char prefix stripped. None when
/// there is no next page. Pure so tests pin it.
pub fn wb_next_cursor(next: Option<&str>) -> Option<String> {
    let n = next?.trim();
    if n.is_empty() {
        return None;
    }
    Some(n.chars().skip(5).collect())
}

// -- SharePoint REST URLs (pure so tests pin them verbatim) --
//
// Verbatim anchors (`SharepointServiceInternal`,
// `lz3_27244/127244`). `{site}`/`{origin}` come from the server's
// own drive locators (`getMyDrive`), never hardcoded: only the
// path shapes below are pinned.

// Trailing-slash-tolerant site join for the SPO builders.
fn spo_join(site: &str, rest: &str) -> Result<String> {
    let site = site.trim().trim_end_matches('/');
    if site.is_empty() {
        bail!("empty site URL");
    }
    Ok(format!("{}{}", site, rest))
}

/// GET: board files under one drive path
/// (`getMyDriveItemsByPath`).
pub fn wb_spo_children_url(site_url: &str, drive_id: &str, path: &str) -> Result<String> {
    if drive_id.trim().is_empty() {
        bail!("empty drive id");
    }
    if path.trim().is_empty() {
        bail!("empty path");
    }
    spo_join(
        site_url,
        &format!(
            "/_api/v2.1/drives/{}/root:/{}:/children?$select=id,createdDateTime,name,size,sharepointIds,parentReference",
            drive_id.trim(),
            path.trim().trim_matches('/')
        ),
    )
}

/// DELETE/PATCH: one board file (`deleteBoard` /
/// `updateFluidBoardMetadata`; PATCH appends
/// `?@microsoft.graph.conflictBehavior=`).
pub fn wb_spo_item_url(site_url: &str, drive_id: &str, item_id: &str) -> Result<String> {
    if drive_id.trim().is_empty() {
        bail!("empty drive id");
    }
    if item_id.trim().is_empty() {
        bail!("empty item id");
    }
    spo_join(
        site_url,
        &format!("/_api/v2.1/drives/{}/items/{}", drive_id.trim(), item_id.trim()),
    )
}

/// GET: tenant Fluid toggle (`getTenantToggle`) — the honest
/// live-gate probe for ODSP sync.
pub fn wb_spo_fluid_enabled_url(site_url: &str, drive_id: &str) -> Result<String> {
    if drive_id.trim().is_empty() {
        bail!("empty drive id");
    }
    spo_join(
        site_url,
        &format!("/_api/v2.1/drives/{}/root/opStream/fluidEnabled", drive_id.trim()),
    )
}

/// GET: board thumbnail bytes
/// (`thumbnailRequestEndpointForBoard`).
pub fn wb_spo_thumbnail_url(
    origin: &str,
    drive_id: &str,
    item_id: &str,
    stream_id: &str,
) -> Result<String> {
    for (v, what) in [(drive_id, "drive id"), (item_id, "item id"), (stream_id, "stream id")] {
        if v.trim().is_empty() {
            bail!("empty {}", what);
        }
    }
    spo_join(
        origin,
        &format!(
            "/_api/v2.1/drives/{}/items/{}/streams/{}/content",
            drive_id.trim(),
            item_id.trim(),
            stream_id.trim()
        ),
    )
}

/// GET: sharing information for one file URL
/// (`getSharingInformation`; feeds `getCanShareEditCSL`).
pub fn wb_spo_sharing_info_url(site_url: &str, file_url: &str) -> Result<String> {
    if file_url.trim().is_empty() {
        bail!("empty file URL");
    }
    spo_join(
        site_url,
        &format!(
            "/_api/web/GetFileByUrl(@a1)/ListItemAllFields/GetSharingInformation?@a1='{}'",
            file_url.trim()
        ),
    )
}

/// GET: WebDAV URL for one board file (`getWebDavUrl`).
pub fn wb_spo_webdav_url(origin: &str, drive_id: &str, item_id: &str) -> Result<String> {
    if drive_id.trim().is_empty() {
        bail!("empty drive id");
    }
    if item_id.trim().is_empty() {
        bail!("empty item id");
    }
    spo_join(
        origin,
        &format!("/_api/v2.0/drives/{}/items/{}?select=webDavUrl", drive_id.trim(), item_id.trim()),
    )
}

/// GET: redeem an encoded share link
/// (`redeemEncodedShareLink`, +`Prefer: redeemSharingLink` —
/// phase-2 P4 pin) → `{driveId,itemId,shareId,siteUrl,…}`.
pub fn wb_spo_redeem_url(origin: &str, encoded_url: &str) -> Result<String> {
    if encoded_url.trim().is_empty() {
        bail!("empty encoded share link");
    }
    spo_join(
        origin,
        &format!(
            "/_api/v2.0/shares/{}/?$expand=driveItem,permission,sharePointIds&$select=driveItem/id,driveItem/parentReference",
            encoded_url.trim()
        ),
    )
}

/// POST: proof token for socket/ad-hoc access
/// (`getProofToken`) → `tempauth` query param of `value[0]`.
pub fn wb_spo_preauth_url(site_url: &str, drive_id: &str) -> Result<String> {
    if drive_id.trim().is_empty() {
        bail!("empty drive id");
    }
    spo_join(
        site_url,
        &format!("/_api/v2.0/drives/{}/getPreAuthorizedUrls", drive_id.trim()),
    )
}

/// POST body for `getProofToken`: `{items,generateGraphUrls,
/// proofTokenScenario,permissionScopes,
/// enforceSPOExternalSharingPolicyCheck}`. Pure so tests pin it.
pub fn wb_preauth_body(
    origin: &str,
    drive_id: &str,
    item_id: &str,
    permission_scopes: &[&str],
) -> Result<serde_json::Value> {
    let origin = origin.trim().trim_end_matches('/');
    if origin.is_empty() {
        bail!("empty origin");
    }
    if drive_id.trim().is_empty() {
        bail!("empty drive id");
    }
    if item_id.trim().is_empty() {
        bail!("empty item id");
    }
    let scopes: Vec<&str> = permission_scopes.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if scopes.is_empty() {
        bail!("empty permission scopes");
    }
    Ok(serde_json::json!({
        "items": [format!("{}/_api/v2.1/drives/{}/items/{}", origin, drive_id.trim(), item_id.trim())],
        "generateGraphUrls": false,
        "proofTokenScenario": "WhiteboardApp",
        "permissionScopes": scopes,
        "enforceSPOExternalSharingPolicyCheck": true,
    }))
}

// -- ODSP/Fluid URLs + bodies (pure so tests pin them verbatim) --

/// POST: join a board file's delta session
/// (`OdspDelayLoadedDeltaStream.joinSessionCore`):
/// `{apiRoot}/drives/{d}/items/{i}/opStream/joinSession?ump=1`.
/// CAPTURE-TIME PIN: `apiRoot` is derived from the resolved
/// `siteUrl` in the bundle (`st.cB(new URL(...))`); phase-2 P3
/// confirms the derivation — until then the caller's site URL is
/// used verbatim.
pub fn wb_join_session_url(site_url: &str, drive_id: &str, item_id: &str) -> Result<String> {
    if drive_id.trim().is_empty() {
        bail!("empty drive id");
    }
    if item_id.trim().is_empty() {
        bail!("empty item id");
    }
    spo_join(
        site_url,
        &format!(
            "/drives/{}/items/{}/opStream/joinSession?ump=1",
            drive_id.trim(),
            item_id.trim()
        ),
    )
}

/// POST: snapshot-upload a new Fluid board file
/// (`createFile.ts:359-363`): `{apiRoot}/drives/{d}/items/root:/
/// {path}/{file}:/opStream/snapshots/snapshot[?shareLink]`.
pub fn wb_snapshot_url(
    api_root: &str,
    drive_id: &str,
    dir: &str,
    file_name: &str,
    share_link: Option<&str>,
) -> Result<String> {
    for (v, what) in [(drive_id, "drive id"), (dir, "directory"), (file_name, "file name")] {
        if v.trim().is_empty() {
            bail!("empty {}", what);
        }
    }
    let mut url = spo_join(
        api_root,
        &format!(
            "/drives/{}/items/root:/{}/{}:/opStream/snapshots/snapshot",
            drive_id.trim(),
            dir.trim().trim_matches('/'),
            file_name.trim()
        ),
    )?;
    if let Some(link) = share_link.map(str::trim).filter(|s| !s.is_empty()) {
        url.push_str("?shareLink=");
        url.push_str(link);
    }
    Ok(url)
}

/// PUT: create a new empty Fluid file (`createFile.ts:225-229`).
pub fn wb_empty_file_url(
    api_root: &str,
    drive_id: &str,
    dir: &str,
    file_name: &str,
) -> Result<String> {
    for (v, what) in [(drive_id, "drive id"), (dir, "directory"), (file_name, "file name")] {
        if v.trim().is_empty() {
            bail!("empty {}", what);
        }
    }
    spo_join(
        api_root,
        &format!(
            "/drives/{}/items/root:/{}/{}.tmp:/content?@name.conflictBehavior=rename&select=id,name,parentReference",
            drive_id.trim(),
            dir.trim().trim_matches('/'),
            file_name.trim()
        ),
    )
}

/// The hand-built joinSession request body, byte-verbatim from
/// `lz_socketModule` (`.../joinSession`, `JoinSession` event):
/// `--{b}\r\nAuthorization: {auth}\r\nX-HTTP-Method-Override:
/// POST\r\nContent-Type: application/json\r\n[prefer:
/// FluidRemoveCheckAccess\r\n]_post: 1\r\n[\r\n{json}\r\n]\
/// r\n--{b}--`, where `{json}` is `{requestSocketToken:true?,
/// displayName?}`. `auth_header` is the full `Authorization` value
/// (`Bearer …`); `check_access` false sends the `prefer:` line.
/// Pure so tests pin it byte for byte.
pub fn wb_join_session_body(
    boundary: &str,
    auth_header: &str,
    request_socket_token: bool,
    display_name: Option<&str>,
    check_access: bool,
) -> Result<Vec<u8>> {
    let boundary = boundary.trim();
    if boundary.is_empty() {
        bail!("empty boundary");
    }
    if auth_header.trim().is_empty() {
        bail!("empty auth header");
    }
    let mut part = String::new();
    part.push_str(&format!("--{}\r\n", boundary));
    part.push_str(&format!("Authorization: {}\r\n", auth_header.trim()));
    part.push_str("X-HTTP-Method-Override: POST\r\n");
    part.push_str("Content-Type: application/json\r\n");
    if !check_access {
        part.push_str("prefer: FluidRemoveCheckAccess\r\n");
    }
    part.push_str("_post: 1\r\n");
    let mut json = serde_json::Map::new();
    if request_socket_token {
        json.insert("requestSocketToken".to_string(), serde_json::Value::Bool(true));
    }
    if let Some(name) = display_name.map(str::trim).filter(|s| !s.is_empty()) {
        json.insert("displayName".to_string(), serde_json::Value::String(name.to_string()));
    }
    if !json.is_empty() {
        part.push_str(&format!("\r\n{}\r\n", serde_json::Value::Object(json)));
    }
    part.push_str(&format!("\r\n--{}--", boundary));
    Ok(part.into_bytes())
}

/// Content-Type for [`wb_join_session_body`]:
/// `multipart/form-data;boundary={b}` (verbatim: no space after
/// the semicolon). Pure so tests pin it.
pub fn wb_join_session_content_type(boundary: &str) -> Result<String> {
    let boundary = boundary.trim();
    if boundary.is_empty() {
        bail!("empty boundary");
    }
    Ok(format!("multipart/form-data;boundary={}", boundary))
}

// -- Teams tab-config + meeting URLs (pure so tests pin them) --

/// Azure tab path (`TabAppConfigServiceAzure`): content URL
/// `{origin}/me/whiteboards/{id}?embed=1&token={token}&isOpenInTab=1`.
pub fn wb_tab_azure_content_url(board_id: &str, token: &str) -> Result<String> {
    if board_id.trim().is_empty() {
        bail!("empty board id");
    }
    if token.trim().is_empty() {
        bail!("empty share token");
    }
    Ok(format!(
        "{}/me/whiteboards/{}?embed=1&token={}&isOpenInTab=1",
        WHITEBOARD_ORIGIN,
        board_id.trim(),
        token.trim()
    ))
}

/// Azure tab path: website URL `{origin}/me/whiteboards/{id}`.
pub fn wb_tab_azure_website_url(board_id: &str) -> Result<String> {
    if board_id.trim().is_empty() {
        bail!("empty board id");
    }
    Ok(format!("{}/me/whiteboards/{}", WHITEBOARD_ORIGIN, board_id.trim()))
}

/// Standard-base64 (btoa) encode for `encodedShareLink` params.
pub fn wb_b64encode(raw: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
}

/// Fluid tab path (`TabAppConfigServiceFluid`): content URL
/// `{origin}/tabappboard?encodedShareLink={b64(webUrl)}&embed=1&
/// isOpenInTab=1`.
pub fn wb_tab_fluid_content_url(board_web_url: &str) -> Result<String> {
    if board_web_url.trim().is_empty() {
        bail!("empty board web URL");
    }
    Ok(format!(
        "{}/tabappboard?encodedShareLink={}&embed=1&isOpenInTab=1",
        WHITEBOARD_ORIGIN,
        wb_b64encode(board_web_url.trim())
    ))
}

/// Fluid tab path: website URL
/// `{origin}/tabappboard?encodedShareLink={b64(webUrl)}`.
pub fn wb_tab_fluid_website_url(board_web_url: &str) -> Result<String> {
    if board_web_url.trim().is_empty() {
        bail!("empty board web URL");
    }
    Ok(format!(
        "{}/tabappboard?encodedShareLink={}",
        WHITEBOARD_ORIGIN,
        wb_b64encode(board_web_url.trim())
    ))
}

/// Meeting participant URL (`732478.D0/f`):
/// `{origin}/meeting?embed=1&encodedShareLink={b64(webUrl)}&
/// shareLinkId=&canEditBoardUsingProofToken=1/0[&proofToken]
/// [&fluidFileId][&annotation=true]`.
pub fn wb_meeting_url(
    board_web_url: &str,
    can_edit: bool,
    proof_token: Option<&str>,
    fluid_file_id: Option<&str>,
    annotation: bool,
) -> Result<String> {
    if board_web_url.trim().is_empty() {
        bail!("empty board web URL");
    }
    let mut url = format!(
        "{}/meeting?embed=1&encodedShareLink={}&shareLinkId=&canEditBoardUsingProofToken={}",
        WHITEBOARD_ORIGIN,
        wb_b64encode(board_web_url.trim()),
        if can_edit { "1" } else { "0" }
    );
    if let Some(t) = proof_token.map(str::trim).filter(|s| !s.is_empty()) {
        url.push_str("&proofToken=");
        url.push_str(t);
    }
    if let Some(f) = fluid_file_id.map(str::trim).filter(|s| !s.is_empty()) {
        url.push_str("&fluidFileId=");
        url.push_str(f);
    }
    if annotation {
        url.push_str("&annotation=true");
    }
    Ok(url)
}

// -- Graph + SPO + ODSP public models --

/// My drive locator (`getMyDrive`): site URL + drive id.
pub struct WhiteboardDrive {
    pub site_url: String,
    pub drive_id: String,
}

/// One Fluid board file's metadata (`getFluidBoardMetadata`).
pub struct WhiteboardFluidMeta {
    pub id: String,
    pub name: String,
    pub web_url: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

/// One minted Fluid share link (`createFluidShareLink`).
pub struct WhiteboardFluidLink {
    pub web_url: Option<String>,
    pub share_id: Option<String>,
}

/// Service-plan names on my license (`getServicePlanNames`).
/// Empty `plans` (or missing the OneDrive plan) means the
/// ODB-license gate fails: "No valid ODB license".
pub struct WhiteboardLicense {
    pub plans: Vec<String>,
}

/// One board file under a drive path (`getMyDriveItemsByPath`).
pub struct WhiteboardDriveItem {
    pub id: String,
    pub name: String,
    pub created: Option<String>,
    pub size: Option<i64>,
}

/// A redeemed encoded share link (`redeemEncodedShareLink`):
/// where the board file lives + what the link grants.
pub struct WhiteboardRedeemed {
    pub drive_id: String,
    pub item_id: String,
    pub share_id: Option<String>,
    pub site_url: Option<String>,
    pub link_type: Option<String>,
    pub scope: Option<String>,
}

/// One channel files folder (`getFilesFolder`).
pub struct WhiteboardFilesFolder {
    pub web_url: Option<String>,
    pub drive_id: Option<String>,
}

/// The `link` facet of `shares/{enc}/permission`.
pub struct WhiteboardSharePermission {
    pub web_url: Option<String>,
    pub scope: Option<String>,
    pub link_type: Option<String>,
}

// -- Response parsers (pure so tests pin them) --

/// Parse one legacy board (`createBoard`/`getBoard` shape; tolerant:
/// the 2021 capture carries extra keys).
pub fn parse_whiteboard(body: &str) -> Result<WhiteboardInfo> {
    let wire: WireBoard = serde_json::from_str(body).context("Failed to parse whiteboard response")?;
    board_from_wire(wire)
}

/// Parse a legacy board list (`getBoards` shape `{value:[…]}` or a
/// bare array — the bundle only names the call, so both win).
pub fn parse_whiteboard_list(body: &str) -> Result<Vec<WhiteboardInfo>> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Failed to parse whiteboard list")?;
    let items = match &value {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(m) => m
            .get("value")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    items
        .into_iter()
        .map(|v| {
            let wire: WireBoard =
                serde_json::from_value(v).context("Failed to parse whiteboard row")?;
            board_from_wire(wire)
        })
        .collect()
}

/// Parse a share link (`createShareLink`/`fetchShareLink` shape
/// `{shareUri}` / `{token,shareUri}`).
pub fn parse_share(body: &str) -> Result<WhiteboardShare> {
    let wire: WireShare = serde_json::from_str(body).context("Failed to parse share link")?;
    Ok(WhiteboardShare {
        share_uri: wire.share_uri.filter(|s| !s.is_empty()),
        token: wire.token.filter(|s| !s.is_empty()),
    })
}

/// Parse collaborators (`{users:[{userId,tenantId,permissions}]}`).
pub fn parse_collaborators(body: &str) -> Result<Vec<WhiteboardCollaborator>> {
    let wire: WireCollaborators =
        serde_json::from_str(body).context("Failed to parse collaborators")?;
    Ok(wire
        .users
        .unwrap_or_default()
        .into_iter()
        .filter_map(|u| {
            let id = u.user_id?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            Some(WhiteboardCollaborator {
                user_id: id,
                tenant_id: u.tenant_id.filter(|s| !s.is_empty()),
            })
        })
        .collect())
}

/// Parse my permissions (`{permissions:{CanEdit,CanRead}}`).
pub fn parse_my_permissions(body: &str) -> Result<WhiteboardMyPerms> {
    let wire: WireMyPermissions =
        serde_json::from_str(body).context("Failed to parse board permissions")?;
    let perms = wire.permissions.unwrap_or(WirePerms { can_edit: None, can_read: None });
    Ok(WhiteboardMyPerms {
        can_edit: perms.can_edit.unwrap_or(false),
        can_read: perms.can_read.unwrap_or(false),
    })
}

/// Parse a redeemed legacy share token (`{id,fluidShareLink}`).
pub fn parse_redeem(body: &str) -> Result<WhiteboardRedeem> {
    let wire: WireRedeem = serde_json::from_str(body).context("Failed to parse redeem response")?;
    let id = wire.id.unwrap_or_default().trim().to_string();
    if id.is_empty() {
        bail!("redeem response has no id");
    }
    Ok(WhiteboardRedeem {
        id,
        fluid_share_link: wire.fluid_share_link.filter(|s| !s.is_empty()),
    })
}

/// Parse one legacy content page (`{gchs,next}`).
pub fn parse_content_page(body: &str) -> Result<WhiteboardContent> {
    let wire: WireContentPage =
        serde_json::from_str(body).context("Failed to parse board content")?;
    Ok(WhiteboardContent {
        gchs: wire.gchs.unwrap_or(serde_json::Value::Null),
        next: wire.next.filter(|s| !s.is_empty()),
    })
}

/// Parse a session tree init (`{treeInit}`).
pub fn parse_tree_init(body: &str) -> Result<WhiteboardTreeInit> {
    let wire: WireTreeInit =
        serde_json::from_str(body).context("Failed to parse session tree")?;
    Ok(WhiteboardTreeInit {
        tree_init: wire.tree_init.unwrap_or(serde_json::Value::Null),
    })
}

/// Parse a joinSession answer: the bundle reads
/// `p.content.deltaStreamSocketUrl` (+`socketToken`,
/// `refreshSessionDurationSeconds`, `tenantId`/`runtimeTenantId`).
/// Both the wrapped (`{content:{…}}`) and bare shapes win.
pub fn parse_join_session(body: &str) -> Result<WhiteboardJoin> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Failed to parse joinSession response")?;
    let content = value.get("content").unwrap_or(&value);
    let socket_url = content
        .get("deltaStreamSocketUrl")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if socket_url.is_empty() {
        bail!("joinSession response has no deltaStreamSocketUrl");
    }
    Ok(WhiteboardJoin {
        socket_url,
        socket_token: content
            .get("socketToken")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
        refresh_secs: content.get("refreshSessionDurationSeconds").and_then(|v| v.as_u64()),
    })
}

/// Parse `getMyDrive` (`{url:siteUrl,driveId}` projection; the raw
/// drive object carries `parentReference.siteUrl` + `id`, so both
/// shapes win).
pub fn parse_my_drive(body: &str) -> Result<WhiteboardDrive> {
    let value: serde_json::Value = serde_json::from_str(body).context("Failed to parse my drive")?;
    let site_url = value
        .get("url")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            value
                .get("parentReference")
                .and_then(|p| p.get("siteUrl"))
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .unwrap_or_default();
    let drive_id = value
        .get("driveId")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("id").and_then(|v| v.as_str()))
        .unwrap_or_default()
        .trim()
        .to_string();
    if site_url.trim().is_empty() {
        bail!("my drive response has no site URL");
    }
    if drive_id.is_empty() {
        bail!("my drive response has no drive id");
    }
    Ok(WhiteboardDrive { site_url: site_url.trim().to_string(), drive_id })
}

/// Parse `getFluidBoardMetadata`.
pub fn parse_fluid_metadata(body: &str) -> Result<WhiteboardFluidMeta> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Failed to parse board metadata")?;
    let id = value.get("id").and_then(|v| v.as_str()).unwrap_or_default().trim().to_string();
    if id.is_empty() {
        bail!("board metadata has no id");
    }
    let text = |k: &str| {
        value.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)
    };
    Ok(WhiteboardFluidMeta {
        id,
        name: text("name").unwrap_or_default(),
        web_url: text("webUrl"),
        created: text("createdDateTime"),
        modified: text("lastModifiedDateTime"),
    })
}

/// Parse `createFluidShareLink` (`{link:{webUrl},shareId,…}`).
pub fn parse_create_link(body: &str) -> Result<WhiteboardFluidLink> {
    let value: serde_json::Value = serde_json::from_str(body).context("Failed to parse share link")?;
    let web_url = value
        .get("link")
        .and_then(|l| l.get("webUrl"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let share_id = value
        .get("shareId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok(WhiteboardFluidLink { web_url, share_id })
}

/// Parse `getServicePlanNames` (`{value:[{servicePlanName}]}`).
pub fn parse_license_plans(body: &str) -> Result<WhiteboardLicense> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Failed to parse license details")?;
    let plans = value
        .get("value")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|p| p.get("servicePlanName").and_then(|v| v.as_str()))
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    Ok(WhiteboardLicense { plans })
}

/// Parse `getMyDriveItemsByPath` (`{value:[items]}`).
pub fn parse_spo_children(body: &str) -> Result<Vec<WhiteboardDriveItem>> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Failed to parse drive children")?;
    let items = value.get("value").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    Ok(items
        .into_iter()
        .filter_map(|v| {
            let id = v.get("id")?.as_str()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            Some(WhiteboardDriveItem {
                id,
                name: v.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
                created: v
                    .get("createdDateTime")
                    .and_then(|c| c.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from),
                size: v.get("size").and_then(|s| s.as_i64()),
            })
        })
        .collect())
}

/// Parse `redeemEncodedShareLink`
/// (`{driveId,itemId,shareId,siteUrl,type,scope,…}`).
pub fn parse_spo_redeem(body: &str) -> Result<WhiteboardRedeemed> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Failed to parse redeemed share link")?;
    let drive_id = value
        .get("driveId")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let item_id = value
        .get("itemId")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if drive_id.is_empty() {
        bail!("redeemed link has no drive id");
    }
    if item_id.is_empty() {
        bail!("redeemed link has no item id");
    }
    let text = |k: &str| {
        value.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)
    };
    Ok(WhiteboardRedeemed {
        drive_id,
        item_id,
        share_id: text("shareId"),
        site_url: text("siteUrl"),
        link_type: text("type"),
        scope: text("scope"),
    })
}

/// Parse `getProofToken`: the `tempauth` query param of `value[0]`.
pub fn parse_preauth(body: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(body).context("Failed to parse proof token")?;
    let first = value
        .get("value")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let tempauth = first.split(['?', '&']).find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        if k.trim() == "tempauth" && !v.trim().is_empty() {
            return Some(v.trim().to_string());
        }
        None
    });
    tempauth.with_context(|| "proof token response has no tempauth param".to_string())
}

/// Parse `getFilesFolder` (`{webUrl,parentReference:{driveId}}`).
pub fn parse_files_folder(body: &str) -> Result<WhiteboardFilesFolder> {
    let value: serde_json::Value = serde_json::from_str(body).context("Failed to parse files folder")?;
    Ok(WhiteboardFilesFolder {
        web_url: value
            .get("webUrl")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
        drive_id: value
            .get("parentReference")
            .and_then(|p| p.get("driveId"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
    })
}

/// Parse the `link` facet of `shares/{enc}/permission`.
pub fn parse_share_permission(body: &str) -> Result<WhiteboardSharePermission> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Failed to parse share permission")?;
    let link = value.get("link").unwrap_or(&serde_json::Value::Null);
    let text = |v: &serde_json::Value, k: &str| {
        v.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(String::from)
    };
    Ok(WhiteboardSharePermission {
        web_url: text(link, "webUrl"),
        scope: text(link, "scope"),
        link_type: text(link, "type"),
    })
}

/// Parse `getTenantToggle` (`.../root/opStream/fluidEnabled`):
/// truthy `value`/`fluidEnabled`/`enabled` means ODSP sync is on.
/// The bundle names only the call, so every plausible shape wins
/// and anything else reads as disabled (honest gate).
pub fn parse_fluid_enabled(body: &str) -> bool {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return body.trim().eq_ignore_ascii_case("true"),
    };
    if let Some(b) = value.as_bool() {
        return b;
    }
    for key in ["value", "fluidEnabled", "enabled"] {
        match value.get(key) {
            Some(serde_json::Value::Bool(b)) => return *b,
            Some(serde_json::Value::String(s)) if s.eq_ignore_ascii_case("true") => return true,
            Some(serde_json::Value::Number(n)) if n.as_i64() == Some(1) => return true,
            _ => {}
        }
    }
    false
}

/// Legacy content read-version headers, verbatim from
/// `getBoardContents` (`.setHeader("X-MinimumReadVersion",
/// "1.0.0.0").setHeader("X-MaximumReadVersion","1.1.0.0")
/// .setHeader("X-WriteVersion","1.0.0.0")`).
pub const WB_MIN_READ_VERSION: &str = "1.0.0.0";
pub const WB_MAX_READ_VERSION: &str = "1.1.0.0";
pub const WB_WRITE_VERSION: &str = "1.0.0.0";

/// Parse `getSharingInformation` into the share/edit verdicts
/// (`getCanShareEditCSL` + `editPermissionFromSharingInformation`
/// mine these; shapes vary, so mining is tolerant and defaults
/// to no-share/no-edit).
pub fn parse_sharing_can_edit(body: &str) -> (bool, bool) {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return (false, false),
    };
    let can_share = value
        .get("canShare")
        .and_then(|v| v.as_bool())
        .or_else(|| value.get("canShareEdit").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    let can_edit = value
        .get("canEdit")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            value.get("permission").and_then(|p| {
                p.as_str().map(|s| {
                    matches!(s.trim().to_ascii_lowercase().as_str(), "edit" | "owner" | "fullcontrol")
                })
            })
        })
        .unwrap_or(false);
    (can_share, can_edit)
}

// -- Legacy REST data fns --

fn require_id(id: &str, what: &str) -> Result<String> {
    let id = id.trim().to_string();
    if id.is_empty() {
        bail!("empty {}", what);
    }
    Ok(id)
}

/// Create one Azure board (`createBoard`: POST `v2.0/whiteboards`
/// `{Whiteboard:{Title}}`). Empty ids bail pre-network.
pub async fn create_whiteboard_data(
    client: &TeamsClient,
    title: Option<&str>,
) -> Result<WhiteboardInfo> {
    let body = wb_create_body(title);
    let resp = client.wb_post(&wb_create_path(), &body).await?;
    let text = resp.text().await.context("Failed to read create board response")?;
    parse_whiteboard(&text)
}

/// List my Azure boards (`getBoards`: GET `v1.0/whiteboards`).
pub async fn list_whiteboards_data(client: &TeamsClient) -> Result<Vec<WhiteboardInfo>> {
    let resp = client.wb_get(&wb_list_path()).await?;
    let text = resp.text().await.context("Failed to read board list response")?;
    parse_whiteboard_list(&text)
}

/// Open one Azure board (`getBoard`: GET `v1.0/whiteboards/{id}`).
pub async fn get_whiteboard_data(client: &TeamsClient, board_id: &str) -> Result<WhiteboardInfo> {
    let id = require_id(board_id, "board id")?;
    let resp = client.wb_get(&wb_board_path(&id)).await?;
    let text = resp.text().await.context("Failed to read board response")?;
    parse_whiteboard(&text)
}

/// Rename one Azure board (`renameBoard`: PATCH
/// `v1.0/whiteboards/{id}` `{title}`).
pub async fn rename_whiteboard_data(
    client: &TeamsClient,
    board_id: &str,
    title: &str,
) -> Result<WhiteboardInfo> {
    let id = require_id(board_id, "board id")?;
    let body = wb_rename_body(title)?;
    let resp = client.wb_patch(&wb_board_path(&id), &body).await?;
    let text = resp.text().await.context("Failed to read rename board response")?;
    parse_whiteboard(&text)
}

/// Delete one Azure board (`deleteBoard`: DELETE
/// `v1.0/whiteboards/{id}`).
pub async fn delete_whiteboard_data(client: &TeamsClient, board_id: &str) -> Result<()> {
    let id = require_id(board_id, "board id")?;
    client.wb_delete(&wb_board_path(&id)).await?;
    Ok(())
}

/// Mint a share link (`createShareLink`: PUT
/// `v1.0/whiteboards/{id}/share`, no body → `{shareUri[,token]}`).
pub async fn share_whiteboard_data(
    client: &TeamsClient,
    board_id: &str,
) -> Result<WhiteboardShare> {
    let id = require_id(board_id, "board id")?;
    let resp = client.wb_put(&wb_share_path(&id), None).await?;
    let text = resp.text().await.context("Failed to read share link response")?;
    parse_share(&text)
}

/// Fetch the share link (`fetchShareLink(AndToken)`: GET
/// `v2.0/whiteboards/{id}/share`).
pub async fn fetch_share_link_data(
    client: &TeamsClient,
    board_id: &str,
) -> Result<WhiteboardShare> {
    let id = require_id(board_id, "board id")?;
    let resp = client.wb_get(&wb_share_fetch_path(&id)).await?;
    let text = resp.text().await.context("Failed to read share link response")?;
    parse_share(&text)
}

/// Revoke the share link (`revokeShareLink`: DELETE
/// `v1.0/whiteboards/{id}/share`).
pub async fn revoke_share_link_data(client: &TeamsClient, board_id: &str) -> Result<()> {
    let id = require_id(board_id, "board id")?;
    client.wb_delete(&wb_share_path(&id)).await?;
    Ok(())
}

/// List collaborators (`getCollaborators`: GET
/// `v1.0/whiteboards/{id}/collaborators`).
pub async fn whiteboard_collaborators_data(
    client: &TeamsClient,
    board_id: &str,
) -> Result<Vec<WhiteboardCollaborator>> {
    let id = require_id(board_id, "board id")?;
    let resp = client.wb_get(&wb_collaborators_path(&id)).await?;
    let text = resp.text().await.context("Failed to read collaborators response")?;
    parse_collaborators(&text)
}

/// Read my permissions (`getBoardPermissions`: GET
/// `v1.0/whiteboards/{id}/collaborators/me/permissions`).
pub async fn whiteboard_my_permissions_data(
    client: &TeamsClient,
    board_id: &str,
) -> Result<WhiteboardMyPerms> {
    let id = require_id(board_id, "board id")?;
    let resp = client.wb_get(&wb_my_permissions_path(&id)).await?;
    let text = resp.text().await.context("Failed to read board permissions")?;
    parse_my_permissions(&text)
}

/// Redeem a legacy share token (`redeemShareToken`: GET
/// `v1.0/whiteboards/redeem/{token}` → `{id,fluidShareLink}`).
pub async fn redeem_share_token_data(
    client: &TeamsClient,
    token: &str,
) -> Result<WhiteboardRedeem> {
    let token = require_id(token, "share token")?;
    let resp = client.wb_get(&wb_redeem_path(&token)).await?;
    let text = resp.text().await.context("Failed to read redeem response")?;
    parse_redeem(&text)
}

/// Read one legacy content page (`getBoardContents`: GET
/// `v1.0/content/{id}?batch=20` with the read-version headers;
/// follow-ups pass the previous page's `next.substring(5)` as
/// `cursor`). This is the ink-sync READ path.
pub async fn whiteboard_content_data(
    client: &TeamsClient,
    board_id: &str,
    cursor: Option<&str>,
) -> Result<WhiteboardContent> {
    let id = require_id(board_id, "board id")?;
    let path = match cursor.map(str::trim).filter(|s| !s.is_empty()) {
        Some(c) => format!("v1.0/content/{}?batch=20&cursor={}", id, c),
        None => wb_content_path(&id),
    };
    let resp = client
        .wb_get_versioned(&path, WB_MIN_READ_VERSION, WB_MAX_READ_VERSION, WB_WRITE_VERSION)
        .await?;
    let text = resp.text().await.context("Failed to read board content")?;
    parse_content_page(&text)
}

/// Read a session tree init (`getSessionTreeInit`: GET
/// `v1.0/content/sessions/{id}` → `{treeInit}`).
pub async fn whiteboard_session_tree_data(
    client: &TeamsClient,
    session_id: &str,
) -> Result<WhiteboardTreeInit> {
    let id = require_id(session_id, "session id")?;
    let resp = client.wb_get(&wb_session_tree_path(&id)).await?;
    let text = resp.text().await.context("Failed to read session tree")?;
    parse_tree_init(&text)
}

// -- Graph board-file data fns (allowlisted, unproven) --

/// My drive locator (`getMyDrive`: GET `/me/drive?
/// $select=owner,id,sharepointIds`).
pub async fn wb_my_drive_data(client: &TeamsClient) -> Result<WhiteboardDrive> {
    let resp = client.graph_get(&wb_graph_my_drive_path()).await?;
    let text = resp.text().await.context("Failed to read my drive response")?;
    parse_my_drive(&text)
}

/// Channel files folder (`getFilesFolder`: GET
/// `/teams/{gid}/channels/{cid}/filesFolder`).
pub async fn wb_files_folder_data(
    client: &TeamsClient,
    group_id: &str,
    channel_id: &str,
) -> Result<WhiteboardFilesFolder> {
    let group = require_id(group_id, "group id")?;
    let channel = require_id(channel_id, "channel id")?;
    let resp = client.graph_get(&wb_graph_files_folder_path(&group, &channel)).await?;
    let text = resp.text().await.context("Failed to read files folder response")?;
    parse_files_folder(&text)
}

/// One Fluid board file's metadata (`getFluidBoardMetadata`).
pub async fn wb_fluid_metadata_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
) -> Result<WhiteboardFluidMeta> {
    let drive = require_id(drive_id, "drive id")?;
    let item = require_id(item_id, "item id")?;
    let resp = client.graph_get(&wb_graph_fluid_metadata_path(&drive, &item)).await?;
    let text = resp.text().await.context("Failed to read board metadata response")?;
    parse_fluid_metadata(&text)
}

/// Mint a Fluid share link (`createFluidShareLink`: POST
/// `drives/{d}/items/{i}/createLink`). `link_type` is `view` or
/// `edit`; `scope` is `organization` or `users` (with
/// `recipient_object_ids`).
pub async fn wb_create_fluid_share_link_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
    link_type: &str,
    scope: &str,
    recipient_object_ids: &[&str],
    send_notification: bool,
) -> Result<WhiteboardFluidLink> {
    let drive = require_id(drive_id, "drive id")?;
    let item = require_id(item_id, "item id")?;
    let body = wb_create_link_body(link_type, scope, recipient_object_ids, send_notification)?;
    let resp = client.graph_post(&wb_graph_create_link_path(&drive, &item), &body).await?;
    let text = resp.text().await.context("Failed to read share link response")?;
    parse_create_link(&text)
}

/// The `link` facet of `shares/{enc}/permission`
/// (`getPermissionForEncodedSharingURL`; the `Prefer:
/// redeemSharingLinkIfNecessary` header rides phase-2 P4).
pub async fn wb_share_permission_data(
    client: &TeamsClient,
    encoded_url: &str,
) -> Result<WhiteboardSharePermission> {
    let enc = require_id(encoded_url, "encoded share link")?;
    let resp = client.graph_get(&wb_graph_share_permission_path(&enc)).await?;
    let text = resp.text().await.context("Failed to read share permission response")?;
    parse_share_permission(&text)
}

/// Delete one Fluid board file (`deleteBoard`: DELETE
/// `drives/{d}/items/{i}`).
pub async fn wb_delete_board_file_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
) -> Result<()> {
    let drive = require_id(drive_id, "drive id")?;
    let item = require_id(item_id, "item id")?;
    client.graph_delete(&wb_graph_drive_item_path(&drive, &item)).await?;
    Ok(())
}

/// Rename one Fluid board file (`renameBoard`: PATCH
/// `drives/{d}/items/{i}` `{name:"${title}.whiteboard"}`).
pub async fn wb_rename_board_file_data(
    client: &TeamsClient,
    drive_id: &str,
    item_id: &str,
    title: &str,
) -> Result<WhiteboardFluidMeta> {
    let drive = require_id(drive_id, "drive id")?;
    let item = require_id(item_id, "item id")?;
    let body = wb_graph_rename_body(title)?;
    let resp = client.graph_patch(&wb_graph_drive_item_path(&drive, &item), &body).await?;
    let text = resp.text().await.context("Failed to read rename board response")?;
    parse_fluid_metadata(&text)
}

/// License service plans (`getServicePlanNames`: GET
/// `/me/licenseDetails?$select=servicePlans`).
pub async fn wb_license_plans_data(client: &TeamsClient) -> Result<WhiteboardLicense> {
    let resp = client.graph_get(&wb_graph_license_path()).await?;
    let text = resp.text().await.context("Failed to read license response")?;
    parse_license_plans(&text)
}

// -- SharePoint + ODSP data fns --

/// Board files under one drive path (`getMyDriveItemsByPath`).
pub async fn wb_spo_children_data(
    client: &TeamsClient,
    site_url: &str,
    drive_id: &str,
    path: &str,
) -> Result<Vec<WhiteboardDriveItem>> {
    let url = wb_spo_children_url(site_url, drive_id, path)?;
    let resp = client.spo_get(&url).await?;
    let text = resp.text().await.context("Failed to read drive children response")?;
    parse_spo_children(&text)
}

/// Tenant Fluid toggle (`getTenantToggle`): the honest live-gate
/// probe — false (or any transport failure, surfaced with its
/// reason) means ODSP sync is unavailable.
pub async fn wb_spo_fluid_enabled_data(
    client: &TeamsClient,
    site_url: &str,
    drive_id: &str,
) -> Result<bool> {
    let url = wb_spo_fluid_enabled_url(site_url, drive_id)?;
    let resp = client.spo_get(&url).await?;
    let text = resp.text().await.context("Failed to read fluid toggle response")?;
    Ok(parse_fluid_enabled(&text))
}

/// Redeem an encoded share link (`redeemEncodedShareLink`).
pub async fn wb_spo_redeem_data(
    client: &TeamsClient,
    origin: &str,
    encoded_url: &str,
) -> Result<WhiteboardRedeemed> {
    let url = wb_spo_redeem_url(origin, encoded_url)?;
    let resp = client.spo_get(&url).await?;
    let text = resp.text().await.context("Failed to read redeemed link response")?;
    parse_spo_redeem(&text)
}

/// Proof token for socket/ad-hoc access (`getProofToken` → the
/// `tempauth` param of `value[0]`).
pub async fn wb_spo_proof_token_data(
    client: &TeamsClient,
    site_url: &str,
    drive_id: &str,
    item_id: &str,
    permission_scopes: &[&str],
) -> Result<String> {
    let origin = site_url.trim().trim_end_matches('/');
    let url = wb_spo_preauth_url(site_url, drive_id)?;
    let body = wb_preauth_body(origin, drive_id, item_id, permission_scopes)?;
    let resp = client.spo_send_json("POST", &url, Some(&body)).await?;
    let text = resp.text().await.context("Failed to read proof token response")?;
    parse_preauth(&text)
}

/// Sharing verdicts for one file URL (`getSharingInformation` →
/// `(can_share, can_edit)`).
pub async fn wb_spo_sharing_info_data(
    client: &TeamsClient,
    site_url: &str,
    file_url: &str,
) -> Result<(bool, bool)> {
    let url = wb_spo_sharing_info_url(site_url, file_url)?;
    let resp = client.spo_get(&url).await?;
    let text = resp.text().await.context("Failed to read sharing info response")?;
    Ok(parse_sharing_can_edit(&text))
}

/// Join a board file's delta session (`joinSessionCore`: POST the
/// hand-built body to `.../opStream/joinSession?ump=1`) →
/// `{deltaStreamSocketUrl,socketToken?,…}`. This OPENS the ink-sync
/// channel; op/socket.io streaming rides phase-2 P3 (live bytes).
pub async fn wb_join_session_data(
    client: &TeamsClient,
    site_url: &str,
    drive_id: &str,
    item_id: &str,
    auth_header: &str,
    display_name: Option<&str>,
) -> Result<WhiteboardJoin> {
    let url = wb_join_session_url(site_url, drive_id, item_id)?;
    let boundary = format!("wb{}", uuid_simple());
    let body = wb_join_session_body(&boundary, auth_header, true, display_name, false)?;
    let content_type = wb_join_session_content_type(&boundary)?;
    let resp = client.spo_post_bytes(&url, body, &content_type).await?;
    let text = resp.text().await.context("Failed to read joinSession response")?;
    parse_join_session(&text)
}

// Multipart boundary nonce (the bundle mints one per call).
fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}", nanos)
}

// -- CLI entry points (print to stdout) --

fn print_board(b: &WhiteboardInfo) {
    println!("  {:<30} {}", b.title, b.id);
    if b.is_shared {
        println!("    shared");
    }
    if let Some(ref o) = b.owner_id {
        println!("    owner {}", o);
    }
    if let Some(ref f) = b.fluid_file_id {
        println!("    fluid {}", f);
    }
}

/// List my Azure boards (prints to stdout).
pub async fn list_whiteboards() -> Result<()> {
    let client = TeamsClient::new().await?;
    let boards = list_whiteboards_data(&client).await?;
    println!("\nWhiteboards:");
    println!("{:-<60}", "");
    if boards.is_empty() {
        println!("  (no boards found)");
    }
    for b in &boards {
        print_board(b);
    }
    Ok(())
}

/// Create one Azure board (prints to stdout).
pub async fn create_whiteboard(title: Option<&str>) -> Result<()> {
    let client = TeamsClient::new().await?;
    let board = create_whiteboard_data(&client, title).await?;
    println!("\nCreated board:");
    print_board(&board);
    Ok(())
}

/// Open one Azure board (prints to stdout).
pub async fn show_whiteboard(board_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let board = get_whiteboard_data(&client, board_id).await?;
    println!("\nBoard:");
    print_board(&board);
    Ok(())
}

/// Rename one Azure board (prints to stdout).
pub async fn rename_whiteboard(board_id: &str, title: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let board = rename_whiteboard_data(&client, board_id, title).await?;
    println!("\nRenamed board:");
    print_board(&board);
    Ok(())
}

/// Delete one Azure board (prints confirmation).
pub async fn delete_whiteboard(board_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    delete_whiteboard_data(&client, board_id).await?;
    println!("\nDeleted board {}", board_id.trim());
    Ok(())
}

/// Mint a share link on one Azure board (prints to stdout).
pub async fn share_whiteboard(board_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let share = share_whiteboard_data(&client, board_id).await?;
    println!("\nShare link:");
    if let Some(u) = share.share_uri {
        println!("  {}", u);
    }
    if let Some(t) = share.token {
        println!("  token {}", t);
    }
    Ok(())
}

/// Fetch one Azure board's share link (prints to stdout).
pub async fn show_share_link(board_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let share = fetch_share_link_data(&client, board_id).await?;
    println!("\nShare link:");
    if let Some(u) = share.share_uri {
        println!("  {}", u);
    }
    if let Some(t) = share.token {
        println!("  token {}", t);
    }
    Ok(())
}

/// Revoke one Azure board's share link (prints confirmation).
pub async fn revoke_share_link(board_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    revoke_share_link_data(&client, board_id).await?;
    println!("\nRevoked share link for board {}", board_id.trim());
    Ok(())
}

/// List one Azure board's collaborators (prints to stdout).
pub async fn show_collaborators(board_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let users = whiteboard_collaborators_data(&client, board_id).await?;
    println!("\nCollaborators:");
    println!("{:-<60}", "");
    if users.is_empty() {
        println!("  (none found)");
    }
    for u in &users {
        match &u.tenant_id {
            Some(t) => println!("  {} ({})", u.user_id, t),
            None => println!("  {}", u.user_id),
        }
    }
    Ok(())
}

/// Read my permissions on one Azure board (prints to stdout).
pub async fn show_my_permissions(board_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let perms = whiteboard_my_permissions_data(&client, board_id).await?;
    println!("\nMy permissions on board {}:", board_id.trim());
    println!("  can edit: {}", perms.can_edit);
    println!("  can read: {}", perms.can_read);
    Ok(())
}

/// Redeem a legacy share token (prints to stdout).
pub async fn redeem_whiteboard(token: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let r = redeem_share_token_data(&client, token).await?;
    println!("\nRedeemed token:");
    println!("  id {}", r.id);
    if let Some(f) = r.fluid_share_link {
        println!("  fluid {}", f);
    }
    Ok(())
}

/// Read one legacy content page (prints op count + next cursor).
pub async fn show_content(board_id: &str, cursor: Option<&str>) -> Result<()> {
    let client = TeamsClient::new().await?;
    let page = whiteboard_content_data(&client, board_id, cursor).await?;
    let ops = page.gchs.as_array().map(|a| a.len()).unwrap_or(0);
    println!("\nBoard content ({} ops):", ops);
    match wb_next_cursor(page.next.as_deref()) {
        Some(c) => println!("  next cursor: {}", c),
        None => println!("  (last page)"),
    }
    Ok(())
}

/// Join one board file's delta session (prints the channel answer).
pub async fn join_whiteboard(
    site_url: &str,
    drive_id: &str,
    item_id: &str,
    auth_header: &str,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let join = wb_join_session_data(&client, site_url, drive_id, item_id, auth_header, None).await?;
    println!("\nJoined session:");
    println!("  socket {}", join.socket_url);
    println!("  socket token: {}", join.socket_token.is_some());
    if let Some(s) = join.refresh_secs {
        println!("  refresh in {}s", s);
    }
    Ok(())
}

/// Print the Teams tab-config URLs for a board (no network: pure
/// builders over the given ids).
pub async fn show_tab_urls(
    board_id: Option<&str>,
    token: Option<&str>,
    board_web_url: Option<&str>,
) -> Result<()> {
    if let (Some(id), Some(t)) = (board_id, token) {
        println!("\nAzure tab URLs:");
        println!("  content {}", wb_tab_azure_content_url(id, t)?);
        println!("  website {}", wb_tab_azure_website_url(id)?);
    }
    if let Some(w) = board_web_url {
        println!("\nFluid tab URLs:");
        println!("  content {}", wb_tab_fluid_content_url(w)?);
        println!("  website {}", wb_tab_fluid_website_url(w)?);
    }
    if board_id.is_none() && board_web_url.is_none() {
        anyhow::bail!("pass --board + --token (Azure) and/or --web-url (Fluid)");
    }
    Ok(())
}

/// Print the meeting participant URL for a board (no network).
pub async fn show_meeting_url(
    board_web_url: &str,
    can_edit: bool,
    proof_token: Option<&str>,
    fluid_file_id: Option<&str>,
    annotation: bool,
) -> Result<()> {
    println!(
        "\nMeeting URL:\n  {}",
        wb_meeting_url(board_web_url, can_edit, proof_token, fluid_file_id, annotation)?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;


    #[test]
    fn legacy_paths_pin_verbatim() {
        assert_eq!(wb_create_path(), "v2.0/whiteboards");
        assert_eq!(wb_list_path(), "v1.0/whiteboards");
        assert_eq!(wb_board_path("b1"), "v1.0/whiteboards/b1");
        assert_eq!(wb_share_path("b1"), "v1.0/whiteboards/b1/share");
        assert_eq!(wb_share_fetch_path("b1"), "v2.0/whiteboards/b1/share");
        assert_eq!(wb_collaborators_path("b1"), "v1.0/whiteboards/b1/collaborators");
        assert_eq!(
            wb_my_permissions_path("b1"),
            "v1.0/whiteboards/b1/collaborators/me/permissions"
        );
        assert_eq!(
            wb_all_permissions_path("b1"),
            "v1.0/whiteboards/b1/collaborators/all/permissions"
        );
        assert_eq!(
            wb_share_permissions_path("b1", "tok"),
            "v3.0/whiteboards/b1/share/tok/permissions"
        );
        assert_eq!(wb_share_update_path("b1", "tok"), "v3.0/whiteboards/b1/share/tok");
        assert_eq!(wb_redeem_path("tok"), "v1.0/whiteboards/redeem/tok");
        assert_eq!(wb_thumbnail_path("b1"), "v1.0/whiteboards/b1/thumbnail");
        assert_eq!(wb_content_path("b1"), "v1.0/content/b1?batch=20");
        assert_eq!(wb_session_tree_path("s1"), "v1.0/content/sessions/s1");
        assert_eq!(wb_migration_path("b1"), "v1.0/migration/azureWhiteboards/b1");
        assert_eq!(wb_fluid_file_info_path("b1"), "v1.0/whiteboards/b1/fluidFileInfo");
        assert_eq!(wb_user_fluid_file_info_path(), "v1.0/whiteboards/fluidFileInfo");
        assert_eq!(wb_report_path("b1", false), "v1.0/whiteboards/b1/report");
        assert_eq!(wb_report_path("b1", true), "v1.0/whiteboards/b1/report/anonymous");
        assert_eq!(wb_enabled_path(), "v1.0/whiteboards/enabled");
        // Trims.
        assert_eq!(wb_board_path("  b1 "), "v1.0/whiteboards/b1");
    }

    #[test]
    fn graph_paths_pin_verbatim() {
        assert_eq!(wb_graph_my_drive_path(), "/me/drive?$select=owner,id,sharepointIds");
        assert_eq!(
            wb_graph_files_folder_path("g1", "c1"),
            "/teams/g1/channels/c1/filesFolder?$select=parentReference,webUrl"
        );
        assert_eq!(
            wb_graph_fluid_metadata_path("d1", "i1"),
            "/drives/d1/items/i1?$select=id,name,createdBy,createdDateTime,lastModifiedDateTime,webUrl,sharepointIds"
        );
        assert_eq!(wb_graph_create_link_path("d1", "i1"), "/drives/d1/items/i1/createLink");
        assert_eq!(
            wb_graph_share_permission_path("enc"),
            "/shares/enc/permission?$select=link"
        );
        assert_eq!(
            wb_graph_delete_permission_path("d1", "i1", "p1"),
            "/drives/d1/items/i1/permissions/p1"
        );
        assert_eq!(wb_graph_drive_item_path("d1", "i1"), "/drives/d1/items/i1");
        assert_eq!(wb_graph_license_path(), "/me/licenseDetails?$select=servicePlans");
    }

    #[test]
    fn spo_urls_pin_verbatim() {
        let site = "https://example.test/sites/x";
        assert_eq!(
            wb_spo_children_url(site, "d1", "Whiteboards").unwrap(),
            "https://example.test/sites/x/_api/v2.1/drives/d1/root:/Whiteboards:/children?$select=id,createdDateTime,name,size,sharepointIds,parentReference"
        );
        assert_eq!(
            wb_spo_item_url(site, "d1", "i1").unwrap(),
            "https://example.test/sites/x/_api/v2.1/drives/d1/items/i1"
        );
        assert_eq!(
            wb_spo_fluid_enabled_url(site, "d1").unwrap(),
            "https://example.test/sites/x/_api/v2.1/drives/d1/root/opStream/fluidEnabled"
        );
        assert_eq!(
            wb_spo_thumbnail_url(site, "d1", "i1", "s1").unwrap(),
            "https://example.test/sites/x/_api/v2.1/drives/d1/items/i1/streams/s1/content"
        );
        assert_eq!(
            wb_spo_sharing_info_url(site, "https://example.test/f").unwrap(),
            "https://example.test/sites/x/_api/web/GetFileByUrl(@a1)/ListItemAllFields/GetSharingInformation?@a1='https://example.test/f'"
        );
        assert_eq!(
            wb_spo_webdav_url(site, "d1", "i1").unwrap(),
            "https://example.test/sites/x/_api/v2.0/drives/d1/items/i1?select=webDavUrl"
        );
        assert_eq!(
            wb_spo_redeem_url(site, "enc").unwrap(),
            "https://example.test/sites/x/_api/v2.0/shares/enc/?$expand=driveItem,permission,sharePointIds&$select=driveItem/id,driveItem/parentReference"
        );
        assert_eq!(
            wb_spo_preauth_url(site, "d1").unwrap(),
            "https://example.test/sites/x/_api/v2.0/drives/d1/getPreAuthorizedUrls"
        );
        // Trailing slash on the site joins cleanly.
        assert_eq!(
            wb_spo_item_url("https://example.test/sites/x/", "d1", "i1").unwrap(),
            "https://example.test/sites/x/_api/v2.1/drives/d1/items/i1"
        );
        assert!(wb_spo_item_url("", "d1", "i1").is_err());
        assert!(wb_spo_item_url(site, "", "i1").is_err());
        assert!(wb_spo_item_url(site, "d1", "").is_err());
    }

    #[test]
    fn odsp_urls_pin_verbatim() {
        assert_eq!(
            wb_join_session_url("https://example.test/sites/x", "d1", "i1").unwrap(),
            "https://example.test/sites/x/drives/d1/items/i1/opStream/joinSession?ump=1"
        );
        assert_eq!(
            wb_snapshot_url("https://example.test", "d1", "Whiteboards", "b.whiteboard", None).unwrap(),
            "https://example.test/drives/d1/items/root:/Whiteboards/b.whiteboard:/opStream/snapshots/snapshot"
        );
        assert_eq!(
            wb_snapshot_url("https://example.test", "d1", "Whiteboards", "b.whiteboard", Some("L")).unwrap(),
            "https://example.test/drives/d1/items/root:/Whiteboards/b.whiteboard:/opStream/snapshots/snapshot?shareLink=L"
        );
        assert_eq!(
            wb_empty_file_url("https://example.test", "d1", "Whiteboards", "b.whiteboard").unwrap(),
            "https://example.test/drives/d1/items/root:/Whiteboards/b.whiteboard.tmp:/content?@name.conflictBehavior=rename&select=id,name,parentReference"
        );
    }

    #[test]
    fn tab_and_meeting_urls_pin_verbatim() {
        assert_eq!(
            wb_tab_azure_content_url("b1", "tok").unwrap(),
            "https://app.whiteboard.microsoft.com/me/whiteboards/b1?embed=1&token=tok&isOpenInTab=1"
        );
        assert_eq!(
            wb_tab_azure_website_url("b1").unwrap(),
            "https://app.whiteboard.microsoft.com/me/whiteboards/b1"
        );
        // btoa("https://example.test/w") — fixed vector.
        assert_eq!(wb_b64encode("https://example.test/w"), "aHR0cHM6Ly9leGFtcGxlLnRlc3Qvdw==");
        assert_eq!(
            wb_tab_fluid_content_url("https://example.test/w").unwrap(),
            "https://app.whiteboard.microsoft.com/tabappboard?encodedShareLink=aHR0cHM6Ly9leGFtcGxlLnRlc3Qvdw==&embed=1&isOpenInTab=1"
        );
        assert_eq!(
            wb_tab_fluid_website_url("https://example.test/w").unwrap(),
            "https://app.whiteboard.microsoft.com/tabappboard?encodedShareLink=aHR0cHM6Ly9leGFtcGxlLnRlc3Qvdw=="
        );
        assert_eq!(
            wb_meeting_url("https://example.test/w", true, None, None, false).unwrap(),
            "https://app.whiteboard.microsoft.com/meeting?embed=1&encodedShareLink=aHR0cHM6Ly9leGFtcGxlLnRlc3Qvdw==&shareLinkId=&canEditBoardUsingProofToken=1"
        );
        assert_eq!(
            wb_meeting_url("https://example.test/w", false, Some("pt"), Some("ff"), true).unwrap(),
            "https://app.whiteboard.microsoft.com/meeting?embed=1&encodedShareLink=aHR0cHM6Ly9leGFtcGxlLnRlc3Qvdw==&shareLinkId=&canEditBoardUsingProofToken=0&proofToken=pt&fluidFileId=ff&annotation=true"
        );
        assert!(wb_tab_azure_content_url("", "tok").is_err());
        assert!(wb_tab_azure_content_url("b1", "").is_err());
        assert!(wb_meeting_url("", true, None, None, false).is_err());
    }

    #[test]
    fn bodies_pin_verbatim() {
        assert_eq!(wb_create_body(Some("Sprint")), json!({"Whiteboard": {"Title": "Sprint"}}));
        assert_eq!(wb_create_body(None), json!({}));
        assert_eq!(wb_create_body(Some("  ")), json!({}));
        assert_eq!(wb_rename_body("Sprint 2").unwrap(), json!({"title": "Sprint 2"}));
        assert!(wb_rename_body(" ").is_err());
        assert_eq!(
            wb_fluid_file_info_body("f1", "3", Some("L")).unwrap(),
            json!({"FileId": "f1", "version": "3", "ShareLink": "L"})
        );
        assert_eq!(
            wb_fluid_file_info_body("f1", "3", None).unwrap(),
            json!({"FileId": "f1", "version": "3"})
        );
        assert_eq!(
            wb_create_link_body("edit", "organization", &[], false).unwrap(),
            json!({"type": "edit", "scope": "organization", "recipients": [], "sendNotification": false})
        );
        assert_eq!(
            wb_create_link_body("view", "users", &["oid1", "oid2"], true).unwrap(),
            json!({"type": "view", "scope": "users",
                   "recipients": [{"objectId": "oid1"}, {"objectId": "oid2"}],
                   "sendNotification": true})
        );
        assert!(wb_create_link_body("view", "users", &[], false).is_err());
        assert!(wb_create_link_body("view", "bogus", &[], false).is_err());
        assert!(wb_create_link_body("", "organization", &[], false).is_err());
        assert_eq!(
            wb_graph_rename_body("Sprint").unwrap(),
            json!({"name": "Sprint.whiteboard"})
        );
        assert!(wb_graph_rename_body("").is_err());
        assert_eq!(wb_create_link_prefer("users"), Some("PartialSuccess"));
        assert_eq!(wb_create_link_prefer("organization"), None);
        assert_eq!(
            wb_preauth_body("https://example.test/sites/x", "d1", "i1", &["Files.Read"]).unwrap(),
            json!({
                "items": ["https://example.test/sites/x/_api/v2.1/drives/d1/items/i1"],
                "generateGraphUrls": false,
                "proofTokenScenario": "WhiteboardApp",
                "permissionScopes": ["Files.Read"],
                "enforceSPOExternalSharingPolicyCheck": true,
            })
        );
        assert!(wb_preauth_body("https://example.test", "d1", "i1", &[]).is_err());
    }

    #[test]
    fn join_body_pins_byte_verbatim() {
        let body = wb_join_session_body("B1", "Bearer tok", true, Some("N"), false).unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "--B1\r\nAuthorization: Bearer tok\r\nX-HTTP-Method-Override: POST\r\nContent-Type: application/json\r\nprefer: FluidRemoveCheckAccess\r\n_post: 1\r\n\r\n{\"displayName\":\"N\",\"requestSocketToken\":true}\r\n\r\n--B1--"
        );
        // No json part when neither flag is set; no prefer line when
        // access is checked.
        let body = wb_join_session_body("B1", "Bearer tok", false, None, true).unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "--B1\r\nAuthorization: Bearer tok\r\nX-HTTP-Method-Override: POST\r\nContent-Type: application/json\r\n_post: 1\r\n\r\n--B1--"
        );
        assert_eq!(wb_join_session_content_type("B1").unwrap(), "multipart/form-data;boundary=B1");
        assert!(wb_join_session_body("", "Bearer tok", true, None, false).is_err());
        assert!(wb_join_session_body("B1", "", true, None, false).is_err());
    }

    #[test]
    fn next_cursor_strips_five() {
        assert_eq!(wb_next_cursor(Some("abcdeCURSOR")), Some("CURSOR".to_string()));
        assert_eq!(wb_next_cursor(None), None);
        assert_eq!(wb_next_cursor(Some("")), None);
        assert_eq!(wb_next_cursor(Some("  ")), None);
    }

    #[test]
    fn parses_pin_wire_shapes() {
        // The 2021 createBoard capture shape (extra keys tolerated).
        let board = parse_whiteboard(
            r#"{"baseApi":"us.whiteboard.microsoft.com","id":"92ed9612","folioId":"f8f11252",
                "title":"Whiteboardmo2021_4","ownerId":"b8816ec5","ownerTenantId":"b4c546a4",
                "isShared":false,"meetingId":null}"#,
        )
        .unwrap();
        assert_eq!(board.id, "92ed9612");
        assert_eq!(board.title, "Whiteboardmo2021_4");
        assert_eq!(board.owner_id.as_deref(), Some("b8816ec5"));
        assert_eq!(board.owner_tenant_id.as_deref(), Some("b4c546a4"));
        assert!(!board.is_shared);
        assert!(board.fluid_file_id.is_none());
        assert!(board.meeting_id.is_none());
        let meeting = parse_whiteboard(r#"{"id":"m1","meetingId":"meet9"}"#).unwrap();
        assert_eq!(meeting.meeting_id.as_deref(), Some("meet9"));
        assert!(parse_whiteboard(r#"{"title":"x"}"#).is_err());

        let list = parse_whiteboard_list(r#"{"value":[{"id":"a","title":"A"},{"id":"b"}]}"#).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].title, "");
        let bare = parse_whiteboard_list(r#"[{"id":"a"}]"#).unwrap();
        assert_eq!(bare.len(), 1);

        let share = parse_share(r#"{"token":"t1","shareUri":"https://example.test/s"}"#).unwrap();
        assert_eq!(share.token.as_deref(), Some("t1"));
        assert_eq!(share.share_uri.as_deref(), Some("https://example.test/s"));

        let users = parse_collaborators(
            r#"{"users":[{"userId":"u1","tenantId":"t1","permissions":{}},{"userId":"","tenantId":"t2"}]}"#,
        )
        .unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].user_id, "u1");

        let perms = parse_my_permissions(r#"{"permissions":{"CanEdit":true,"CanRead":true}}"#).unwrap();
        assert!(perms.can_edit && perms.can_read);
        let perms = parse_my_permissions(r#"{}"#).unwrap();
        assert!(!perms.can_edit && !perms.can_read);

        let redeem = parse_redeem(r#"{"id":"b1","fluidShareLink":"https://example.test/f"}"#).unwrap();
        assert_eq!(redeem.id, "b1");
        assert!(redeem.fluid_share_link.is_some());

        let page = parse_content_page(r#"{"gchs":[{"op":1}],"next":"abcdeC"}"#).unwrap();
        assert_eq!(page.gchs.as_array().map(|a| a.len()), Some(1));
        assert_eq!(wb_next_cursor(page.next.as_deref()), Some("C".to_string()));

        let tree = parse_tree_init(r#"{"treeInit":{"root":1}}"#).unwrap();
        assert!(tree.tree_init.is_object());

        let join = parse_join_session(
            r#"{"content":{"deltaStreamSocketUrl":"wss://example.test/s","socketToken":"st","refreshSessionDurationSeconds":3600}}"#,
        )
        .unwrap();
        assert_eq!(join.socket_url, "wss://example.test/s");
        assert_eq!(join.socket_token.as_deref(), Some("st"));
        assert_eq!(join.refresh_secs, Some(3600));
        let bare = parse_join_session(r#"{"deltaStreamSocketUrl":"wss://example.test/s"}"#).unwrap();
        assert_eq!(bare.socket_url, "wss://example.test/s");
        assert!(parse_join_session(r#"{"content":{}}"#).is_err());

        let drive =
            parse_my_drive(r#"{"parentReference":{"siteUrl":"https://example.test/s"},"id":"d1"}"#).unwrap();
        assert_eq!(drive.site_url, "https://example.test/s");
        assert_eq!(drive.drive_id, "d1");

        let meta = parse_fluid_metadata(
            r#"{"id":"i1","name":"b.whiteboard","webUrl":"https://example.test/w",
                "createdDateTime":"c","lastModifiedDateTime":"m"}"#,
        )
        .unwrap();
        assert_eq!(meta.name, "b.whiteboard");
        assert_eq!(meta.web_url.as_deref(), Some("https://example.test/w"));

        let link = parse_create_link(r#"{"link":{"webUrl":"https://example.test/l"},"shareId":"s1"}"#).unwrap();
        assert_eq!(link.web_url.as_deref(), Some("https://example.test/l"));

        let lic = parse_license_plans(
            r#"{"value":[{"servicePlanName":"WHITEBOARD_PLAN3"},{"servicePlanName":""}]}"#,
        )
        .unwrap();
        assert_eq!(lic.plans, vec!["WHITEBOARD_PLAN3".to_string()]);

        let kids = parse_spo_children(
            r#"{"value":[{"id":"i1","name":"b.whiteboard","createdDateTime":"c","size":7},{"id":""}]}"#,
        )
        .unwrap();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].size, Some(7));

        let red = parse_spo_redeem(
            r#"{"driveId":"d1","itemId":"i1","shareId":"s1","siteUrl":"https://example.test/s","type":"view","scope":"organization"}"#,
        )
        .unwrap();
        assert_eq!(red.item_id, "i1");
        assert_eq!(red.scope.as_deref(), Some("organization"));

        let tempauth = parse_preauth(
            r#"{"value":["https://example.test/x?foo=1&tempauth=TAUTH&bar=2"]}"#,
        )
        .unwrap();
        assert_eq!(tempauth, "TAUTH");
        assert!(parse_preauth(r#"{"value":[]}"#).is_err());

        let folder = parse_files_folder(r#"{"webUrl":"https://example.test/f","parentReference":{"driveId":"d1"}}"#).unwrap();
        assert_eq!(folder.drive_id.as_deref(), Some("d1"));

        let perm = parse_share_permission(
            r#"{"link":{"webUrl":"https://example.test/l","scope":"users","type":"edit"}}"#,
        )
        .unwrap();
        assert_eq!(perm.scope.as_deref(), Some("users"));

        assert!(parse_fluid_enabled(r#"{"value":true}"#));
        assert!(parse_fluid_enabled(r#"{"fluidEnabled":1}"#));
        assert!(!parse_fluid_enabled(r#"{"value":false}"#));
        assert!(!parse_fluid_enabled(r#"{}"#));
        assert!(!parse_fluid_enabled("not json"));

        assert_eq!(parse_sharing_can_edit(r#"{"canShare":true,"canEdit":false}"#), (true, false));
        assert_eq!(parse_sharing_can_edit(r#"{"permission":"edit"}"#), (false, true));
        assert_eq!(parse_sharing_can_edit(r#"garbage"#), (false, false));
    }
}