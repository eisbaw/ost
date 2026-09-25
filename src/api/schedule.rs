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
