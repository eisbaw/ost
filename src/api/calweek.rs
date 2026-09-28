//! Calendar week view + schedule-new-meeting via Microsoft Graph.
//!
//! B1 calendar lane (own module; `calendar.rs` untouched): explicit-window
//! `calendarView` reads for the 7-day grid, `POST /me/calendar/events`
//! scheduling with an optional Teams online meeting, and `DELETE`
//! cancellation. Reads reuse `parse_calendar_view`/`MeetingInfo` from the
//! existing join lane; the path builder is local so the week can start on
//! any day (not just "now").
//!
//! Graph docs: `calendar-list-calendarview` (GET ../calendarView with
//! `startDateTime`/`endDateTime`), `user-post-events` example 4 (create
//! with `isOnlineMeeting:true` + `onlineMeetingProvider:teamsForBusiness`,
//! join URL reads back at `onlineMeeting.joinUrl`), `event-delete`.
//! Writes need `Calendars.ReadWrite`; a 403 surfaces as the call detail.
//! Edit/reschedule (PATCH) is out of v1: occurrence PATCH has recurrence
//! semantics that are not trivially safe.

use anyhow::{bail, Context, Result};

use super::client::TeamsClient;
use crate::api::{parse_calendar_view, MeetingInfo};

// ---------------------------------------------------------------------------
// Week-window calendarView read
// ---------------------------------------------------------------------------

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ` (UTC, std only).
/// Local copy (calendar.rs owns the original; this lane adds no edits).
fn unix_to_iso8601(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days + 719_468);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 0000-03-01 → (year, month, day). Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Percent-encode a query value (Graph datetimes carry `:`).
fn encode_param(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Build the `calendarView` path for the window `[start, start+days]`.
/// Same `$select` as the join lane so `parse_calendar_view` handles it.
/// Pure so tests pin the query shape.
pub fn calweek_view_path(start_secs: u64, days: u64, limit: usize) -> String {
    let start = unix_to_iso8601(start_secs);
    let end = unix_to_iso8601(start_secs.saturating_add(days.saturating_mul(86_400)));
    format!(
        "/me/calendar/calendarView?startDateTime={}&endDateTime={}&$top={}\
         &$orderby=start/dateTime\
         &$select=id,subject,isOnlineMeeting,onlineMeeting,start,end,organizer,webLink",
        encode_param(&start),
        encode_param(&end),
        limit
    )
}

/// Fetch meetings in `[start_secs, start_secs+days]`, soonest first.
/// `days` must be 1..=31 (the grid passes 7); bails pre-network.
pub async fn list_week_meetings_data(
    client: &TeamsClient,
    start_secs: u64,
    days: u64,
    limit: usize,
) -> Result<Vec<MeetingInfo>> {
    if days == 0 || days > 31 {
        bail!("days must be 1..=31");
    }
    let path = calweek_view_path(start_secs, days, limit);
    let resp = client.graph_get(&path).await?;
    let body = resp
        .text()
        .await
        .context("Failed to read calendarView response")?;
    parse_calendar_view(&body)
}

// ---------------------------------------------------------------------------
// Schedule-new-meeting (POST /me/calendar/events)
// ---------------------------------------------------------------------------

/// Validate schedule inputs pre-network. `start`/`end` are Graph datetimes
/// (`YYYY-MM-DDTHH:MM:SS`, same shape so lexicographic compare orders them).
pub fn validate_schedule(subject: &str, start: &str, end: &str, time_zone: &str) -> Result<()> {
    if subject.trim().is_empty() {
        bail!("empty subject");
    }
    for (what, v) in [("start", start), ("end", end)] {
        if v.len() < 16 || !v.contains('T') {
            bail!("{} must be a Graph datetime (YYYY-MM-DDTHH:MM:SS)", what);
        }
    }
    if end <= start {
        bail!("end must be after start");
    }
    if time_zone.trim().is_empty() {
        bail!("empty time_zone");
    }
    if time_zone.chars().any(|c| c.is_whitespace()) {
        bail!("time_zone must not contain whitespace");
    }
    Ok(())
}

/// Build the `POST /me/calendar/events` body. `online` requests a Teams
/// meeting (`isOnlineMeeting` + `teamsForBusiness`); Graph generates the
/// join URL. Pure so tests pin the shape.
pub fn schedule_event_body(
    subject: &str,
    start: &str,
    end: &str,
    time_zone: &str,
    online: bool,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "subject": subject,
        "start": {"dateTime": start, "timeZone": time_zone},
        "end": {"dateTime": end, "timeZone": time_zone},
    });
    if online {
        body["isOnlineMeeting"] = serde_json::json!(true);
        body["onlineMeetingProvider"] = serde_json::json!("teamsForBusiness");
    }
    body
}

/// Parse one created-event object into a `MeetingInfo` by wrapping it in
/// the `calendarView` envelope the join lane already parses. Pure.
pub fn parse_created_event(event: &serde_json::Value) -> Result<MeetingInfo> {
    let wrapped = serde_json::json!({"value": [event]}).to_string();
    let mut meetings = parse_calendar_view(&wrapped)?;
    meetings.pop().context("empty created event")
}

/// Schedule one meeting. Returns the created event (with `join_url` when
/// `online` and Graph provisioned the Teams link).
pub async fn schedule_meeting_data(
    client: &TeamsClient,
    subject: &str,
    start: &str,
    end: &str,
    time_zone: &str,
    online: bool,
) -> Result<MeetingInfo> {
    validate_schedule(subject, start, end, time_zone)?;
    let body = schedule_event_body(subject.trim(), start, end, time_zone, online);
    let resp = client.graph_post("/me/calendar/events", &body).await?;
    let event: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse created event response")?;
    parse_created_event(&event)
}

// ---------------------------------------------------------------------------
// Cancel (DELETE /me/calendar/events/{id})
// ---------------------------------------------------------------------------

fn check_event_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("empty event_id");
    }
    if id.contains('/')
        || id.contains('?')
        || id.contains('#')
        || id.chars().any(|c| c.is_whitespace())
    {
        bail!("event_id must not contain '/', '?', '#' or whitespace");
    }
    Ok(())
}

/// Cancel one meeting (DELETE; Graph answers 204, no body to parse).
pub async fn cancel_meeting_data(client: &TeamsClient, event_id: &str) -> Result<()> {
    check_event_id(event_id)?;
    client
        .graph_delete(&format!("/me/calendar/events/{}", event_id))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEK_JSON: &str = r#"{
        "value": [
            {"id": "W1", "subject": "Mon standup",
             "isOnlineMeeting": true,
             "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/x"},
             "start": {"dateTime": "2026-09-28T09:00:00.0000000", "timeZone": "UTC"},
             "end": {"dateTime": "2026-09-28T09:15:00.0000000", "timeZone": "UTC"},
             "organizer": {"emailAddress": {"name": "Doe, Jane", "address": "j@x.io"}}},
            {"id": "W2", "subject": "Fri demo",
             "isOnlineMeeting": false,
             "start": {"dateTime": "2026-10-02T14:00:00.0000000", "timeZone": "UTC"},
             "end": {"dateTime": "2026-10-02T15:00:00.0000000", "timeZone": "UTC"}}
        ]
    }"#;

    #[test]
    fn week_path_pins_window_and_select() {
        // 2026-09-28T00:00:00Z Monday = 1790553600.
        let p = calweek_view_path(1_790_553_600, 7, 50);
        assert!(p.starts_with("/me/calendar/calendarView?"), "{}", p);
        assert!(
            p.contains("startDateTime=2026-09-28T00%3A00%3A00Z"),
            "{}",
            p
        );
        assert!(p.contains("endDateTime=2026-10-05T00%3A00%3A00Z"), "{}", p);
        assert!(p.contains("$top=50"), "{}", p);
        assert!(p.contains("$orderby=start/dateTime"), "{}", p);
        assert!(p.contains("onlineMeeting"), "{}", p);
    }

    #[test]
    fn week_fixture_parses_two_events() {
        let ms = parse_calendar_view(WEEK_JSON).unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].id, "W1");
        assert!(ms[0].is_online);
        assert_eq!(
            ms[0].join_url.as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/x")
        );
        assert_eq!(ms[1].id, "W2");
        assert!(!ms[1].is_online);
        assert!(ms[1].join_url.is_none());
    }

    #[test]
    fn created_event_wrap_parses_join_url() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"id": "C1", "subject": "Sync",
                "isOnlineMeeting": true,
                "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/new"},
                "start": {"dateTime": "2026-09-29T10:00:00", "timeZone": "UTC"},
                "end": {"dateTime": "2026-09-29T10:30:00", "timeZone": "UTC"}}"#,
        )
        .unwrap();
        let m = parse_created_event(&v).unwrap();
        assert_eq!(m.id, "C1");
        assert_eq!(m.subject, "Sync");
        assert_eq!(
            m.join_url.as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/new")
        );
    }

    #[test]
    fn schedule_body_online_requests_teams() {
        let b = schedule_event_body(
            "Sync",
            "2026-09-29T10:00:00",
            "2026-09-29T10:30:00",
            "UTC",
            true,
        );
        assert_eq!(b["subject"], "Sync");
        assert_eq!(b["start"]["dateTime"], "2026-09-29T10:00:00");
        assert_eq!(b["start"]["timeZone"], "UTC");
        assert_eq!(b["isOnlineMeeting"], true);
        assert_eq!(b["onlineMeetingProvider"], "teamsForBusiness");
    }

    #[test]
    fn schedule_body_offline_omits_online_keys() {
        let b = schedule_event_body(
            "Lunch",
            "2026-09-29T12:00:00",
            "2026-09-29T13:00:00",
            "UTC",
            false,
        );
        assert!(b.get("isOnlineMeeting").is_none());
        assert!(b.get("onlineMeetingProvider").is_none());
    }

    #[test]
    fn validate_schedule_rejects_bad_input() {
        assert!(
            validate_schedule("", "2026-09-29T10:00:00", "2026-09-29T10:30:00", "UTC").is_err()
        );
        assert!(
            validate_schedule("  ", "2026-09-29T10:00:00", "2026-09-29T10:30:00", "UTC").is_err()
        );
        assert!(validate_schedule("S", "not-a-date", "2026-09-29T10:30:00", "UTC").is_err());
        assert!(
            validate_schedule("S", "2026-09-29T10:30:00", "2026-09-29T10:00:00", "UTC").is_err()
        );
        assert!(
            validate_schedule("S", "2026-09-29T10:00:00", "2026-09-29T10:00:00", "UTC").is_err()
        );
        assert!(validate_schedule("S", "2026-09-29T10:00:00", "2026-09-29T10:30:00", "").is_err());
        assert!(
            validate_schedule("S", "2026-09-29T10:00:00", "2026-09-29T10:30:00", "U TC").is_err()
        );
        assert!(validate_schedule(
            "S",
            "2026-09-29T10:00:00",
            "2026-09-29T10:30:00",
            "America/New_York"
        )
        .is_ok());
    }

    #[test]
    fn check_event_id_rejects_path_breakers() {
        assert!(check_event_id("").is_err());
        assert!(check_event_id("AAMkAG/a==").is_err());
        assert!(check_event_id("a b").is_err());
        assert!(check_event_id("AAMkAGFj?x").is_err());
        assert!(check_event_id("AAMkAGFjLT0x").is_ok());
    }
}
