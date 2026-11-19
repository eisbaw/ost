//! Team schedules (Shifts) via Microsoft Graph (read-only).
//!
//! `GET /teams/{team}/schedule` describes the schedule; `/shifts`,
//! `/timesOff`, and `/timeOffReasons` under it list the week grid rows.
//! There is no balances endpoint in Graph: "time-off balances" are an
//! approved-timesOff-instances-per-reason count computed host-side.
//! Auth: existing Graph token; personal MSA accounts are unsupported by
//! the schedule API. No writes exist in this module (no swap requests,
//! no time-off requests) — display only.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

// -- Wire types --

#[derive(Debug, Deserialize)]
struct ScheduleResponse {
    enabled: Option<bool>,
    #[serde(rename = "timeZone")]
    time_zone: Option<String>,
    #[serde(rename = "provisionStatus")]
    provision_status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ShiftsResponse {
    value: Vec<WireShift>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireShift {
    id: String,
    #[serde(rename = "userId")]
    user_id: Option<String>,
    #[serde(rename = "sharedShift")]
    shared_shift: Option<WireShiftSlot>,
    #[serde(rename = "draftShift")]
    draft_shift: Option<WireShiftSlot>,
}

#[derive(Debug, Deserialize)]
struct WireShiftSlot {
    #[serde(rename = "startDateTime")]
    start: Option<String>,
    #[serde(rename = "endDateTime")]
    end: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    theme: Option<String>,
    notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TimesOffResponse {
    value: Vec<WireTimeOff>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireTimeOff {
    id: String,
    #[serde(rename = "userId")]
    user_id: Option<String>,
    #[serde(rename = "timeOffReasonId")]
    reason_id: Option<String>,
    #[serde(rename = "sharedTimeOff")]
    shared_time_off: Option<WireTimeOffSlot>,
    #[serde(rename = "draftTimeOff")]
    draft_time_off: Option<WireTimeOffSlot>,
}

#[derive(Debug, Deserialize)]
struct WireTimeOffSlot {
    #[serde(rename = "startDateTime")]
    start: Option<String>,
    #[serde(rename = "endDateTime")]
    end: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReasonsResponse {
    value: Vec<WireReason>,
}

#[derive(Debug, Deserialize)]
struct WireReason {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    code: Option<String>,
}

// -- Public model --

/// One team's schedule header: whether Shifts is on and in which zone.
pub struct ScheduleInfo {
    pub enabled: bool,
    pub time_zone: Option<String>,
    pub provision_status: Option<String>,
}

/// One shift row: shared slot wins, draft slot is the fallback.
pub struct ShiftInfo {
    pub id: String,
    pub user_id: Option<String>,
    pub display_name: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub theme: Option<String>,
    pub notes: Option<String>,
    /// True when the row came from the draft (unshared) slot.
    pub is_draft: bool,
}

/// One time-off instance: shared range wins, draft is the fallback.
pub struct TimeOffInfo {
    pub id: String,
    pub user_id: Option<String>,
    pub reason_id: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub is_draft: bool,
}

/// One time-off reason code (hosts group instances by `id` for balances).
pub struct TimeOffReason {
    pub id: String,
    pub name: String,
    pub code: Option<String>,
}

fn schedule_from_wire(resp: ScheduleResponse) -> ScheduleInfo {
    ScheduleInfo {
        enabled: resp.enabled.unwrap_or(false),
        time_zone: resp.time_zone,
        provision_status: resp.provision_status,
    }
}

fn shift_from_wire(shift: WireShift) -> ShiftInfo {
    let (slot, is_draft) = match (shift.shared_shift, shift.draft_shift) {
        (Some(s), _) => (s, false),
        (None, Some(d)) => (d, true),
        (None, None) => (
            WireShiftSlot {
                start: None,
                end: None,
                display_name: None,
                theme: None,
                notes: None,
            },
            false,
        ),
    };
    ShiftInfo {
        id: shift.id,
        user_id: shift.user_id,
        display_name: slot.display_name.unwrap_or_default(),
        start: slot.start,
        end: slot.end,
        theme: slot.theme,
        notes: slot.notes,
        is_draft,
    }
}

fn time_off_from_wire(item: WireTimeOff) -> TimeOffInfo {
    let (slot, is_draft) = match (item.shared_time_off, item.draft_time_off) {
        (Some(s), _) => (s, false),
        (None, Some(d)) => (d, true),
        (None, None) => (
            WireTimeOffSlot {
                start: None,
                end: None,
            },
            false,
        ),
    };
    TimeOffInfo {
        id: item.id,
        user_id: item.user_id,
        reason_id: item.reason_id,
        start: slot.start,
        end: slot.end,
        is_draft,
    }
}

fn reason_from_wire(reason: WireReason) -> TimeOffReason {
    let name = reason.display_name.unwrap_or_else(|| reason.id.clone());
    TimeOffReason {
        id: reason.id,
        name,
        code: reason.code,
    }
}

fn checked_team_id(team_id: &str) -> Result<String> {
    let trimmed = team_id.trim();
    if trimmed.is_empty() {
        bail!("empty team_id");
    }
    Ok(trimmed.to_string())
}

// -- Data-returning API functions (all GET; no writes) --

/// Fetch one team's schedule header. Empty `team_id` bails pre-network.
pub async fn list_schedule_data(client: &TeamsClient, team_id: &str) -> Result<ScheduleInfo> {
    let team_id = checked_team_id(team_id)?;
    let path = format!("/teams/{}/schedule", team_id);
    let resp = client.graph_get(&path).await?;
    let parsed: ScheduleResponse = resp.json().await.context("Failed to parse schedule response")?;
    Ok(schedule_from_wire(parsed))
}

/// List one team's shifts. Empty `team_id` bails pre-network.
pub async fn list_shifts_data(client: &TeamsClient, team_id: &str) -> Result<Vec<ShiftInfo>> {
    let team_id = checked_team_id(team_id)?;
    let path = format!("/teams/{}/schedule/shifts", team_id);
    let resp = client.graph_get(&path).await?;
    let parsed: ShiftsResponse = resp.json().await.context("Failed to parse shifts response")?;
    Ok(parsed.value.into_iter().map(shift_from_wire).collect())
}

/// List one team's time-off instances. Empty `team_id` bails pre-network.
pub async fn list_timesoffs_data(client: &TeamsClient, team_id: &str) -> Result<Vec<TimeOffInfo>> {
    let team_id = checked_team_id(team_id)?;
    let path = format!("/teams/{}/schedule/timesOff", team_id);
    let resp = client.graph_get(&path).await?;
    let parsed: TimesOffResponse = resp
        .json()
        .await
        .context("Failed to parse timesOff response")?;
    Ok(parsed.value.into_iter().map(time_off_from_wire).collect())
}

/// List one team's time-off reasons. Empty `team_id` bails pre-network.
pub async fn list_timeoff_reasons_data(
    client: &TeamsClient,
    team_id: &str,
) -> Result<Vec<TimeOffReason>> {
    let team_id = checked_team_id(team_id)?;
    let path = format!("/teams/{}/schedule/timeOffReasons", team_id);
    let resp = client.graph_get(&path).await?;
    let parsed: ReasonsResponse = resp
        .json()
        .await
        .context("Failed to parse timeOffReasons response")?;
    Ok(parsed.value.into_iter().map(reason_from_wire).collect())
}

// -- Write ops: change requests, shifts, time off, publish --
//
// Every write below needs the delegated `Schedule.ReadWrite.All` scope
// (Graph v1.0; the create calls also accept `Group.ReadWrite.All`).
// The reads above need only directory-read scopes,
// so on a tenant without the write scope each call fails with HTTP 403
// `Missing scope permissions` naming the required scope — never silently.
// All args are validated pre-network like the reads (`checked_team_id`,
// `checked_datetime`); Graph ids travel as path segments verbatim.
//
// Kinds: `swap` (swapShiftsChangeRequests), `offer`
// (offerShiftRequests), `timeoff` (timeOffRequests). Only these three
// request sets exist in v1.0 (no open-shift requests: out of scope).

/// One schedule change request (lenient: each set carries its own
/// fields; only `id`/`state` are universal).
pub struct ChangeRequestInfo {
    pub id: String,
    pub state: Option<String>,
    pub sender_shift_id: Option<String>,
    pub receiver_user_id: Option<String>,
    pub reason_id: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireChangeRequest {
    id: String,
    state: Option<String>,
    #[serde(rename = "senderShiftId")]
    sender_shift_id: Option<String>,
    #[serde(rename = "receiverUserId")]
    receiver_user_id: Option<String>,
    #[serde(rename = "timeOffReasonId")]
    reason_id: Option<String>,
    #[serde(rename = "startDateTime")]
    start: Option<String>,
    #[serde(rename = "endDateTime")]
    end: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChangeRequestsResponse {
    value: Vec<WireChangeRequest>,
}

#[derive(Debug, Deserialize)]
struct CreatedResponse {
    id: Option<String>,
}

fn request_from_wire(w: WireChangeRequest) -> ChangeRequestInfo {
    ChangeRequestInfo {
        id: w.id,
        state: w.state,
        sender_shift_id: w.sender_shift_id,
        receiver_user_id: w.receiver_user_id,
        reason_id: w.reason_id,
        start: w.start,
        end: w.end,
        message: w.message,
    }
}

/// Request-set collection for a kind (`swap`/`offer`/`timeoff`).
/// Unknown kinds bail pre-network.
pub fn request_collection(kind: &str) -> Result<&'static str> {
    match kind.trim() {
        "swap" => Ok("swapShiftsChangeRequests"),
        "offer" => Ok("offerShiftRequests"),
        "timeoff" => Ok("timeOffRequests"),
        other => bail!("unknown shift request kind '{}' (swap|offer|timeoff)", other),
    }
}

fn requests_path(team_id: &str, kind: &str) -> Result<String> {
    Ok(format!("/teams/{}/schedule/{}", checked_team_id(team_id)?, request_collection(kind)?))
}

fn decide_path(team_id: &str, kind: &str, request_id: &str, approve: bool) -> Result<String> {
    let id = request_id.trim();
    if id.is_empty() {
        bail!("empty request_id");
    }
    Ok(format!(
        "{}/{}/{}",
        requests_path(team_id, kind)?,
        id,
        if approve { "approve" } else { "decline" }
    ))
}

/// True when a Graph failure is the missing-scope refusal (the error
/// body names the required scope, e.g. `Schedule.ReadWrite.All`). The
/// host maps this to the scope-pending empty state instead of raw JSON.
pub fn is_missing_scope(text: &str) -> bool {
    text.contains("Missing scope permissions")
}

fn created_id(bytes: &[u8], what: &str) -> Result<String> {
    let parsed: CreatedResponse =
        serde_json::from_slice(bytes).with_context(|| format!("Failed to parse created {} response", what))?;
    parsed.id.filter(|s| !s.trim().is_empty()).context(format!("Created {} response has no id", what))
}

/// List one team's change requests of one kind (no state filter:
/// teams are small; the host shows pending first). Empty `team_id` or
/// unknown `kind` bail pre-network.
pub async fn list_change_requests_data(
    client: &TeamsClient,
    team_id: &str,
    kind: &str,
) -> Result<Vec<ChangeRequestInfo>> {
    let path = requests_path(team_id, kind)?;
    let resp = client.graph_get(&path).await?;
    let parsed: ChangeRequestsResponse = resp
        .json()
        .await
        .with_context(|| format!("Failed to parse {} requests response", kind.trim()))?;
    Ok(parsed.value.into_iter().map(request_from_wire).collect())
}

/// Create a swap request for one shift. `receiver_user_id`/`message`
/// are optional (omitted when blank). Returns the request id.
pub async fn create_swap_request_data(
    client: &TeamsClient,
    team_id: &str,
    sender_shift_id: &str,
    receiver_user_id: Option<&str>,
    message: Option<&str>,
) -> Result<String> {
    let sender = sender_shift_id.trim();
    if sender.is_empty() {
        bail!("empty sender_shift_id");
    }
    let mut body = serde_json::json!({ "senderShiftId": sender });
    if let Some(r) = receiver_user_id.filter(|s| !s.trim().is_empty()) {
        body["receiverUserId"] = serde_json::json!(r.trim());
    }
    if let Some(m) = message.filter(|s| !s.trim().is_empty()) {
        body["message"] = serde_json::json!(m.trim());
    }
    let path = requests_path(team_id, "swap")?;
    let resp = client.graph_post(&path, &body).await?;
    let bytes = resp.bytes().await.context("Failed to read created swap request body")?;
    created_id(&bytes, "swap request")
}

/// Create an offer request (give one shift to one member). Returns the
/// request id.
pub async fn create_offer_request_data(
    client: &TeamsClient,
    team_id: &str,
    sender_shift_id: &str,
    receiver_user_id: Option<&str>,
    message: Option<&str>,
) -> Result<String> {
    let sender = sender_shift_id.trim();
    if sender.is_empty() {
        bail!("empty sender_shift_id");
    }
    let mut body = serde_json::json!({ "senderShiftId": sender });
    if let Some(r) = receiver_user_id.filter(|s| !s.trim().is_empty()) {
        body["receiverUserId"] = serde_json::json!(r.trim());
    }
    if let Some(m) = message.filter(|s| !s.trim().is_empty()) {
        body["message"] = serde_json::json!(m.trim());
    }
    let path = requests_path(team_id, "offer")?;
    let resp = client.graph_post(&path, &body).await?;
    let bytes = resp.bytes().await.context("Failed to read created offer request body")?;
    created_id(&bytes, "offer request")
}

/// Create a time-off request for one reason and range. Returns the
/// request id. Bounds are format-checked pre-network.
pub async fn create_timeoff_request_data(
    client: &TeamsClient,
    team_id: &str,
    reason_id: &str,
    start: &str,
    end: &str,
    message: Option<&str>,
) -> Result<String> {
    let reason = reason_id.trim();
    if reason.is_empty() {
        bail!("empty reason_id");
    }
    let mut body = serde_json::json!({
        "timeOffReasonId": reason,
        "startDateTime": checked_datetime("start", start)?,
        "endDateTime": checked_datetime("end", end)?,
    });
    if let Some(m) = message.filter(|s| !s.trim().is_empty()) {
        body["message"] = serde_json::json!(m.trim());
    }
    let path = requests_path(team_id, "timeoff")?;
    let resp = client.graph_post(&path, &body).await?;
    let bytes = resp.bytes().await.context("Failed to read created time-off request body")?;
    created_id(&bytes, "time-off request")
}

/// Approve or decline one change request (manager only server-side).
/// `message` is sent when non-blank.
pub async fn decide_change_request_data(
    client: &TeamsClient,
    team_id: &str,
    kind: &str,
    request_id: &str,
    approve: bool,
    message: Option<&str>,
) -> Result<()> {
    let path = decide_path(team_id, kind, request_id, approve)?;
    let mut body = serde_json::json!({});
    if let Some(m) = message.filter(|s| !s.trim().is_empty()) {
        body["message"] = serde_json::json!(m.trim());
    }
    client.graph_post(&path, &body).await?;
    Ok(())
}

fn draft_shift_slot(
    start: &str,
    end: &str,
    display_name: Option<&str>,
    theme: Option<&str>,
    notes: Option<&str>,
) -> Result<serde_json::Value> {
    let mut slot = serde_json::json!({
        "startDateTime": checked_datetime("start", start)?,
        "endDateTime": checked_datetime("end", end)?,
    });
    if let Some(n) = display_name.filter(|s| !s.trim().is_empty()) {
        slot["displayName"] = serde_json::json!(n.trim());
    }
    if let Some(t) = theme.filter(|s| !s.trim().is_empty()) {
        slot["theme"] = serde_json::json!(t.trim().to_ascii_lowercase());
    }
    if let Some(n) = notes.filter(|s| !s.trim().is_empty()) {
        slot["notes"] = serde_json::json!(n.trim());
    }
    Ok(slot)
}

/// Create one draft shift (manager only server-side). Blank `user_id`
/// creates an open shift. Returns the shift id.
pub async fn create_shift_data(
    client: &TeamsClient,
    team_id: &str,
    user_id: &str,
    start: &str,
    end: &str,
    display_name: Option<&str>,
    theme: Option<&str>,
    notes: Option<&str>,
) -> Result<String> {
    let mut body = serde_json::json!({
        "draftShift": draft_shift_slot(start, end, display_name, theme, notes)?,
    });
    if !user_id.trim().is_empty() {
        body["userId"] = serde_json::json!(user_id.trim());
    }
    let path = format!("/teams/{}/schedule/shifts", checked_team_id(team_id)?);
    let resp = client.graph_post(&path, &body).await?;
    let bytes = resp.bytes().await.context("Failed to read created shift body")?;
    created_id(&bytes, "shift")
}

/// Patch one shift's draft slot (manager only server-side). Blank
/// fields are left unchanged; at least one field is required.
pub async fn update_shift_data(
    client: &TeamsClient,
    team_id: &str,
    shift_id: &str,
    start: Option<&str>,
    end: Option<&str>,
    display_name: Option<&str>,
    theme: Option<&str>,
    notes: Option<&str>,
) -> Result<()> {
    let id = shift_id.trim();
    if id.is_empty() {
        bail!("empty shift_id");
    }
    let present = [start, end, display_name, theme, notes]
        .iter()
        .any(|f| f.is_some_and(|s| !s.trim().is_empty()));
    if !present {
        bail!("nothing to update (all fields blank)");
    }
    let mut slot = serde_json::json!({});
    if let Some(s) = start.filter(|s| !s.trim().is_empty()) {
        slot["startDateTime"] = serde_json::json!(checked_datetime("start", s)?);
    }
    if let Some(e) = end.filter(|s| !s.trim().is_empty()) {
        slot["endDateTime"] = serde_json::json!(checked_datetime("end", e)?);
    }
    if let Some(n) = display_name.filter(|s| !s.trim().is_empty()) {
        slot["displayName"] = serde_json::json!(n.trim());
    }
    if let Some(t) = theme.filter(|s| !s.trim().is_empty()) {
        slot["theme"] = serde_json::json!(t.trim().to_ascii_lowercase());
    }
    if let Some(n) = notes.filter(|s| !s.trim().is_empty()) {
        slot["notes"] = serde_json::json!(n.trim());
    }
    let path = format!("/teams/{}/schedule/shifts/{}", checked_team_id(team_id)?, id);
    client.graph_patch(&path, &serde_json::json!({ "draftShift": slot })).await?;
    Ok(())
}

/// Delete one shift (manager only server-side).
pub async fn delete_shift_data(client: &TeamsClient, team_id: &str, shift_id: &str) -> Result<()> {
    let id = shift_id.trim();
    if id.is_empty() {
        bail!("empty shift_id");
    }
    let path = format!("/teams/{}/schedule/shifts/{}", checked_team_id(team_id)?, id);
    client.graph_delete(&path).await?;
    Ok(())
}

/// Create one draft time-off instance directly (manager only
/// server-side; members use time-off requests). Returns the id.
pub async fn create_timeoff_data(
    client: &TeamsClient,
    team_id: &str,
    user_id: &str,
    reason_id: &str,
    start: &str,
    end: &str,
) -> Result<String> {
    let user = user_id.trim();
    if user.is_empty() {
        bail!("empty user_id");
    }
    let reason = reason_id.trim();
    if reason.is_empty() {
        bail!("empty reason_id");
    }
    let body = serde_json::json!({
        "userId": user,
        "timeOffReasonId": reason,
        "draftTimeOff": {
            "startDateTime": checked_datetime("start", start)?,
            "endDateTime": checked_datetime("end", end)?,
        },
    });
    let path = format!("/teams/{}/schedule/timesOff", checked_team_id(team_id)?);
    let resp = client.graph_post(&path, &body).await?;
    let bytes = resp.bytes().await.context("Failed to read created time-off body")?;
    created_id(&bytes, "time off")
}

/// Publish draft shifts/time off (`POST .../schedule/share`, manager
/// only server-side). Blank bounds share the whole schedule.
pub async fn share_schedule_data(
    client: &TeamsClient,
    team_id: &str,
    notify_team: bool,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<()> {
    let mut body = serde_json::json!({ "notifyTeam": notify_team });
    if let Some(s) = start.filter(|s| !s.trim().is_empty()) {
        body["startDateTime"] = serde_json::json!(checked_datetime("start", s)?);
    }
    if let Some(e) = end.filter(|s| !s.trim().is_empty()) {
        body["endDateTime"] = serde_json::json!(checked_datetime("end", e)?);
    }
    let path = format!("/teams/{}/schedule/share", checked_team_id(team_id)?);
    client.graph_post(&path, &body).await?;
    Ok(())
}

// -- CLI entry point (prints to stdout) --

// -- Date-range reads (server-side week) --

/// Pages followed per range query before stopping (Graph pages the
/// schedule collections; a week never needs this many).
const RANGE_MAX_PAGES: usize = 20;

/// A range bound travels into `$filter` verbatim: ISO-8601 characters
/// only (digits, `T`, `Z`, `:`, `.`, `-`, `+`), never empty.
fn checked_datetime(what: &str, value: &str) -> Result<String> {
    let v = value.trim();
    if v.is_empty() {
        bail!("empty {}", what);
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, 'T' | 'Z' | ':' | '.' | '-' | '+'))
    {
        bail!("{} must be an ISO-8601 date-time", what);
    }
    Ok(v.to_string())
}

/// Days the server window reaches past each side of `[start, end]`.
/// Graph documents only containment filters for the schedule
/// collections (`startDateTime ge` + `endDateTime le`, each property
/// once), so a row that overlaps the range but crosses one of its
/// bounds is fetched through this margin and kept or dropped by
/// [`overlaps_range`]. Shifts are at most 24 hours long; time off can
/// run for weeks, so its margin covers a quarter.
pub fn range_pad_days(collection: &str) -> i64 {
    if collection == "timesOff" {
        92
    } else {
        1
    }
}

/// Parse a range bound or a row date-time: RFC 3339 (offset or `Z`),
/// a naive `YYYY-MM-DDTHH:MM:SS[.f]` read as UTC, or a bare date
/// (midnight UTC).
fn parse_datetime(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
    let v = value.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(v) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(n) = NaiveDateTime::parse_from_str(v, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(n.and_utc());
    }
    NaiveDate::parse_from_str(v, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|n| n.and_utc())
}

fn checked_bound(what: &str, value: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    let v = checked_datetime(what, value)?;
    match parse_datetime(&v) {
        Some(dt) => Ok(dt),
        None => bail!("{} must be an ISO-8601 date-time", what),
    }
}

/// Collection path for one slot (`sharedShift`, `draftShift`,
/// `sharedTimeOff`, `draftTimeOff`) in the documented filter shape:
/// the slot starts at or after `start` and ends at or before `end`,
/// both widened by [`range_pad_days`] so rows overlapping the range
/// are fetched too (the caller keeps only those, [`overlaps_range`]).
/// Each property appears once. Bounds travel as UTC `...Z`; spaces are
/// `%20`.
pub fn schedule_range_path(
    team_id: &str,
    collection: &str,
    slot: &str,
    start: &str,
    end: &str,
) -> Result<String> {
    let team_id = checked_team_id(team_id)?;
    let pad = chrono::Duration::days(range_pad_days(collection));
    let from = (checked_bound("start", start)? - pad).format("%Y-%m-%dT%H:%M:%SZ");
    let to = (checked_bound("end", end)? + pad).format("%Y-%m-%dT%H:%M:%SZ");
    Ok(format!(
        "/teams/{}/schedule/{}?$filter={}/startDateTime%20ge%20{}%20and%20{}/endDateTime%20le%20{}",
        team_id, collection, slot, from, slot, to
    ))
}

/// True when a row `[row_start, row_end]` overlaps `[start, end]`: it
/// starts before `end` and ends after `start`. A missing or unparseable
/// row date never excludes the row (nothing is hidden on bad data).
pub fn overlaps_range(
    row_start: Option<&str>,
    row_end: Option<&str>,
    start: &str,
    end: &str,
) -> bool {
    let (Some(lo), Some(hi)) = (parse_datetime(start), parse_datetime(end)) else {
        return true;
    };
    if let Some(s) = row_start.and_then(parse_datetime) {
        if s >= hi {
            return false;
        }
    }
    if let Some(e) = row_end.and_then(parse_datetime) {
        if e <= lo {
            return false;
        }
    }
    true
}

/// Follow `@odata.nextLink` pages (bounded) for one filtered query.
async fn fetch_pages<T, R>(
    client: &TeamsClient,
    first_path: &str,
    what: &str,
    split: impl Fn(R) -> (Vec<T>, Option<String>),
) -> Result<Vec<T>>
where
    R: serde::de::DeserializeOwned,
{
    let mut out = Vec::new();
    let mut resp = client.graph_get(first_path).await?;
    for _ in 0..RANGE_MAX_PAGES {
        let parsed: R = resp
            .json()
            .await
            .with_context(|| format!("Failed to parse {} response", what))?;
        let (items, next) = split(parsed);
        out.extend(items);
        match next {
            Some(url) if !url.is_empty() => {
                // nextLink is absolute; only follow it on the Graph v1.0 host.
                let Some(path) = url.strip_prefix("https://graph.microsoft.com/v1.0") else {
                    bail!("Unexpected nextLink host");
                };
                resp = client.graph_get(path).await?
            }
            _ => break,
        }
    }
    Ok(out)
}

/// True when a draft-slot query failed with HTTP 403: Graph lets only
/// schedule managers filter on draft information ("Sorry, you don't
/// have the permission to filter on draft information."), so for a
/// team member the draft slot is simply empty, never a failed week.
/// Shared-slot failures and every other status still fail the call.
pub fn draft_slot_forbidden(slot: &str, err: &anyhow::Error) -> bool {
    slot.starts_with("draft") && err.to_string().starts_with("HTTP 403 ")
}

/// Keep the first row per id (shared-slot query first, so a shift with
/// both slots keeps its shared version).
fn dedupe_by_id<T>(rows: Vec<T>, id: impl Fn(&T) -> &str) -> Vec<T> {
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .filter(|r| seen.insert(id(r).to_string()))
        .collect()
}

/// List one team's shifts overlapping `[start, end]` (padded
/// server-side containment filter, then the overlap test here; shared
/// then draft slot, all pages; a draft slot a member may not read
/// is empty, [`draft_slot_forbidden`]). Bad bounds / empty
/// `team_id` bail pre-network.
pub async fn list_shifts_range_data(
    client: &TeamsClient,
    team_id: &str,
    start: &str,
    end: &str,
) -> Result<Vec<ShiftInfo>> {
    let mut rows = Vec::new();
    for slot in ["sharedShift", "draftShift"] {
        let path = schedule_range_path(team_id, "shifts", slot, start, end)?;
        let wire = match fetch_pages(client, &path, "shifts", |r: ShiftsResponse| {
            (r.value, r.next_link)
        })
        .await
        {
            Ok(wire) => wire,
            Err(e) if draft_slot_forbidden(slot, &e) => Vec::new(),
            Err(e) => return Err(e),
        };
        rows.extend(wire.into_iter().map(shift_from_wire));
    }
    rows.retain(|s| overlaps_range(s.start.as_deref(), s.end.as_deref(), start, end));
    Ok(dedupe_by_id(rows, |s| s.id.as_str()))
}

/// List one team's time-off instances overlapping `[start, end]`
/// (padded server-side containment filter, then the overlap test here;
/// shared then draft slot, all pages; a draft slot a member may not
/// read is empty, [`draft_slot_forbidden`]).
pub async fn list_timesoffs_range_data(
    client: &TeamsClient,
    team_id: &str,
    start: &str,
    end: &str,
) -> Result<Vec<TimeOffInfo>> {
    let mut rows = Vec::new();
    for slot in ["sharedTimeOff", "draftTimeOff"] {
        let path = schedule_range_path(team_id, "timesOff", slot, start, end)?;
        let wire = match fetch_pages(client, &path, "timesOff", |r: TimesOffResponse| {
            (r.value, r.next_link)
        })
        .await
        {
            Ok(wire) => wire,
            Err(e) if draft_slot_forbidden(slot, &e) => Vec::new(),
            Err(e) => return Err(e),
        };
        rows.extend(wire.into_iter().map(time_off_from_wire));
    }
    rows.retain(|t| overlaps_range(t.start.as_deref(), t.end.as_deref(), start, end));
    Ok(dedupe_by_id(rows, |t| t.id.as_str()))
}

/// Print one team's week grid (shifts + time-off, read-only).
pub async fn list_shifts(team_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let schedule = list_schedule_data(&client, team_id).await?;
    let shifts = list_shifts_data(&client, team_id).await?;
    let offs = list_timesoffs_data(&client, team_id).await?;

    println!("\nTeam Schedule (enabled: {}):", schedule.enabled);
    println!("{:-<60}", "");
    if shifts.is_empty() && offs.is_empty() {
        println!("  (no shifts or time-off found)");
        return Ok(());
    }
    for s in &shifts {
        println!(
            "  {} {} -> {} {}",
            s.display_name,
            s.start.as_deref().unwrap_or("?"),
            s.end.as_deref().unwrap_or("?"),
            if s.is_draft { "(draft)" } else { "" }
        );
    }
    for t in &offs {
        println!(
            "  time-off {} -> {} {}",
            t.start.as_deref().unwrap_or("?"),
            t.end.as_deref().unwrap_or("?"),
            if t.is_draft { "(draft)" } else { "" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_path_uses_documented_filter_and_keeps_overlaps() {
        assert_eq!(
            schedule_range_path("t1", "shifts", "sharedShift",
                "2026-09-28T00:00:00Z", "2026-10-05T00:00:00+02:00").unwrap(),
            "/teams/t1/schedule/shifts?$filter=sharedShift/startDateTime%20ge%20\
             2026-09-27T00:00:00Z%20and%20sharedShift/endDateTime%20le%20\
             2026-10-05T22:00:00Z"
        );
        assert_eq!(
            schedule_range_path("t1", "timesOff", "draftTimeOff", "2026-09-28", "2026-10-05").unwrap(),
            "/teams/t1/schedule/timesOff?$filter=draftTimeOff/startDateTime%20ge%20\
             2026-06-28T00:00:00Z%20and%20draftTimeOff/endDateTime%20le%20\
             2027-01-05T00:00:00Z"
        );
        let (w0, w1) = ("2026-09-28T00:00:00Z", "2026-10-05T00:00:00Z");
        // Overnight shift into the week, week-long leave across it: kept.
        assert!(overlaps_range(Some("2026-09-27T22:00:00Z"), Some("2026-09-28T06:00:00Z"), w0, w1));
        assert!(overlaps_range(Some("2026-09-01T00:00:00"), Some("2026-10-20T00:00:00"), w0, w1));
        // Ends at the week start / starts at its end: dropped.
        assert!(!overlaps_range(Some("2026-09-27T09:00:00Z"), Some(w0), w0, w1));
        assert!(!overlaps_range(Some(w1), Some("2026-10-05T08:00:00Z"), w0, w1));
        assert!(overlaps_range(None, Some("junk"), w0, w1));
        for bad in ["", "  ", "2026-09-28 00:00", "x&$top=1", "2026'", "2026-13-40"] {
            assert!(schedule_range_path("t1", "shifts", "sharedShift", bad, "2026-10-05").is_err());
            assert!(schedule_range_path("t1", "timesOff", "draftTimeOff", "2026-09-28", bad).is_err());
        }
        assert!(schedule_range_path(" ", "shifts", "sharedShift", "2026-09-28", "2026-10-05").is_err());
        let rows = dedupe_by_id(vec![("a", 1), ("b", 2), ("a", 3)], |r| r.0);
        assert_eq!(rows, vec![("a", 1), ("b", 2)]);
    }

    #[test]
    fn checked_team_id_rejects_blank_pre_network() {
        assert!(checked_team_id("").is_err());
        assert!(checked_team_id("   ").is_err());
        assert_eq!(checked_team_id("  t1 ").unwrap(), "t1");
    }

    /// Live 2026-09-28 (member account): the draft-slot filter answers
    /// 403 Forbidden "permission to filter on draft information" while
    /// the shared slot answers 200. Only that case is skipped.
    #[test]
    fn draft_slot_403_is_empty_not_fatal() {
        let forbidden = anyhow::anyhow!(
            "HTTP 403 for https://graph.microsoft.com/v1.0/teams/t1/schedule/shifts?$filter=draftShift/startDateTime%20ge%202026-09-27T04:00:00Z: {{\"error\":{{\"code\":\"Forbidden\",\"message\":\"Sorry, you don't have the permission to filter on draft information.\"}}}}"
        );
        assert!(draft_slot_forbidden("draftShift", &forbidden));
        assert!(draft_slot_forbidden("draftTimeOff", &forbidden));
        // Shared slot 403, other statuses, transport errors: still fatal.
        assert!(!draft_slot_forbidden("sharedShift", &forbidden));
        assert!(!draft_slot_forbidden("sharedTimeOff", &forbidden));
        let not_found = anyhow::anyhow!("HTTP 404 for https://graph.microsoft.com/v1.0/teams/t1/schedule/shifts: {{}}");
        assert!(!draft_slot_forbidden("draftShift", &not_found));
        let throttled = anyhow::anyhow!("HTTP 429 for https://graph.microsoft.com/v1.0/teams/t1403/schedule/shifts: {{}}");
        assert!(!draft_slot_forbidden("draftShift", &throttled));
        let unauthorized = anyhow::anyhow!("401 Unauthorized for https://graph.microsoft.com/v1.0/x.");
        assert!(!draft_slot_forbidden("draftShift", &unauthorized));
    }

    #[test]
    fn fixture_schedule_shifts_timesoff_reasons_parse() {
        let schedule: ScheduleResponse = serde_json::from_str(
            r#"{"enabled":true,"timeZone":"America/New_York","provisionStatus":"Completed"}"#,
        )
        .unwrap();
        let info = schedule_from_wire(schedule);
        assert!(info.enabled);
        assert_eq!(info.time_zone.as_deref(), Some("America/New_York"));

        let shifts: ShiftsResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"s1","userId":"u1",
                 "sharedShift":{"startDateTime":"2026-09-28T09:00:00",
                  "endDateTime":"2026-09-28T17:00:00",
                  "displayName":"Morning","theme":"blue","notes":"front desk"}},
                {"id":"s2","userId":"u2",
                 "draftShift":{"startDateTime":"2026-09-29T09:00:00",
                  "endDateTime":"2026-09-29T17:00:00"}}]}"#,
        )
        .unwrap();
        let rows: Vec<ShiftInfo> = shifts.value.into_iter().map(shift_from_wire).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display_name, "Morning");
        assert_eq!(rows[0].theme.as_deref(), Some("blue"));
        assert!(!rows[0].is_draft);
        assert!(rows[1].is_draft);
        assert_eq!(rows[1].display_name, "");

        let offs: TimesOffResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"o1","userId":"u1","timeOffReasonId":"r1",
                 "sharedTimeOff":{"startDateTime":"2026-09-30T00:00:00",
                  "endDateTime":"2026-10-01T00:00:00"}}]}"#,
        )
        .unwrap();
        let off_rows: Vec<TimeOffInfo> =
            offs.value.into_iter().map(time_off_from_wire).collect();
        assert_eq!(off_rows.len(), 1);
        assert_eq!(off_rows[0].reason_id.as_deref(), Some("r1"));
        assert!(!off_rows[0].is_draft);

        let reasons: ReasonsResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"r1","displayName":"Vacation","code":"V"},
                {"id":"r2"}]}"#,
        )
        .unwrap();
        let reason_rows: Vec<TimeOffReason> =
            reasons.value.into_iter().map(reason_from_wire).collect();
        assert_eq!(reason_rows.len(), 2);
        assert_eq!(reason_rows[0].name, "Vacation");
        // Missing displayName falls back to the reason id.
        assert_eq!(reason_rows[1].name, "r2");
    }

    #[test]
    fn write_paths_and_validation_reject_pre_network() {
        assert_eq!(
            requests_path(" t1 ", "swap").unwrap(),
            "/teams/t1/schedule/swapShiftsChangeRequests"
        );
        assert_eq!(
            requests_path("t1", "offer").unwrap(),
            "/teams/t1/schedule/offerShiftRequests"
        );
        assert_eq!(
            requests_path("t1", "timeoff").unwrap(),
            "/teams/t1/schedule/timeOffRequests"
        );
        assert!(requests_path("t1", "open").is_err());
        assert!(requests_path("  ", "swap").is_err());
        assert_eq!(
            decide_path("t1", "swap", " r1 ", true).unwrap(),
            "/teams/t1/schedule/swapShiftsChangeRequests/r1/approve"
        );
        assert_eq!(
            decide_path("t1", "timeoff", "r1", false).unwrap(),
            "/teams/t1/schedule/timeOffRequests/r1/decline"
        );
        assert!(decide_path("t1", "swap", "  ", true).is_err());
        assert!(decide_path("t1", "bogus", "r1", true).is_err());
        // Draft-slot body: blanks omitted, theme lowercased.
        let slot = draft_shift_slot("2026-11-24T09:00:00Z", "2026-11-24T17:00:00Z",
                                    Some(" Front "), Some("Blue"), Some("")).unwrap();
        assert_eq!(slot["startDateTime"], "2026-11-24T09:00:00Z");
        assert_eq!(slot["displayName"], "Front");
        assert_eq!(slot["theme"], "blue");
        assert!(slot.get("notes").is_none());
        assert!(draft_shift_slot("junk", "2026-11-24T17:00:00Z", None, None, None).is_err());
        // Missing-scope refusal is recognized for the scope-pending state.
        assert!(is_missing_scope("HTTP 403 ... \"Missing scope permissions on the request. API requires one of 'Schedule.ReadWrite.All' ...\""));
        assert!(!is_missing_scope("HTTP 404 ..."));
        // Created id parsing: blank/missing ids fail loudly.
        assert_eq!(created_id(br#"{"id":"abc"}"#, "shift").unwrap(), "abc");
        assert!(created_id(br#"{"id":" "}"#, "shift").is_err());
        assert!(created_id(br#"{}"#, "shift").is_err());
        // Request rows parse leniently per set.
        let reqs: ChangeRequestsResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"q1","state":"pending","senderShiftId":"s1","message":"swap?"},
                {"id":"q2","state":"approved","timeOffReasonId":"r1",
                 "startDateTime":"2026-12-24T00:00:00","endDateTime":"2026-12-26T00:00:00"}]}"#,
        )
        .unwrap();
        let rows: Vec<ChangeRequestInfo> = reqs.value.into_iter().map(request_from_wire).collect();
        assert_eq!(rows[0].sender_shift_id.as_deref(), Some("s1"));
        assert_eq!(rows[0].reason_id, None);
        assert_eq!(rows[1].reason_id.as_deref(), Some("r1"));
        assert_eq!(rows[1].state.as_deref(), Some("approved"));
    }

    #[test]
    fn shared_slot_wins_over_draft() {
        let shifts: ShiftsResponse = serde_json::from_str(
            r#"{"value":[{"id":"s9",
                "sharedShift":{"displayName":"Shared"},
                "draftShift":{"displayName":"Draft"}}]}"#,
        )
        .unwrap();
        let rows: Vec<ShiftInfo> = shifts.value.into_iter().map(shift_from_wire).collect();
        assert_eq!(rows[0].display_name, "Shared");
        assert!(!rows[0].is_draft);
    }
}
