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
/// then draft slot, all pages). Bad bounds / empty
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
        let wire = fetch_pages(client, &path, "shifts", |r: ShiftsResponse| {
            (r.value, r.next_link)
        })
        .await?;
        rows.extend(wire.into_iter().map(shift_from_wire));
    }
    rows.retain(|s| overlaps_range(s.start.as_deref(), s.end.as_deref(), start, end));
    Ok(dedupe_by_id(rows, |s| s.id.as_str()))
}

/// List one team's time-off instances overlapping `[start, end]`
/// (padded server-side containment filter, then the overlap test here;
/// shared then draft slot, all pages).
pub async fn list_timesoffs_range_data(
    client: &TeamsClient,
    team_id: &str,
    start: &str,
    end: &str,
) -> Result<Vec<TimeOffInfo>> {
    let mut rows = Vec::new();
    for slot in ["sharedTimeOff", "draftTimeOff"] {
        let path = schedule_range_path(team_id, "timesOff", slot, start, end)?;
        let wire = fetch_pages(client, &path, "timesOff", |r: TimesOffResponse| {
            (r.value, r.next_link)
        })
        .await?;
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
