//! Meetings-plus: attendance reports + server capture options (Graph).
//!
//! GAPHUNT row 5 (meetings extras): Recap reads recordings/transcripts
//! from drives; there is NO attendance UI, NO recording control beyond
//! meetrecord's in-call legs, and NO server caption/transcription toggle.
//! This module is the Graph data path for all three:
//!
//! - attendance: `GET /me/onlineMeetings/{id}/attendanceReports` then
//!   `GET …/attendanceReports/{reportId}/attendanceRecords`
//!   (scope `OnlineMeetings.Read` — NOT granted to the Teams token, so
//!   live reads 403; the Swift view shows the live-disabled reason).
//! - capture options: `GET/PATCH /communications/onlineMeetings/{id}`
//!   with `allowRecording` / `allowTranscription` / `recordAutomatically`
//!   (scope `OnlineMeetings.ReadWrite` — likewise not granted; meetrecord
//!   §106 already documents that no delegated mid-call start/stop endpoint
//!   exists, so these options are the only server toggles).
//!
//! Every failed read returns its HTTP status in the error text (never an
//! empty success), so callers can tell a scope 403 from a real absence.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

/// Attendance reports list path for one online meeting. Pure.
pub fn attendance_reports_path(meeting_id: &str) -> String {
    format!("/me/onlineMeetings/{}/attendanceReports", meeting_id.trim())
}

/// Attendance records path for one report. Pure.
pub fn attendance_records_path(meeting_id: &str, report_id: &str) -> String {
    format!(
        "/me/onlineMeetings/{}/attendanceReports/{}/attendanceRecords",
        meeting_id.trim(),
        report_id.trim()
    )
}

/// Server capture-options path for one online meeting (GET + PATCH). Pure.
pub fn capture_options_path(meeting_id: &str) -> String {
    format!("/communications/onlineMeetings/{}", meeting_id.trim())
}

/// One attendance record: who joined, in what role, for how long.
/// Graph `attendanceRecord` (identity + role + join/leave intervals).
#[derive(Debug, Clone, PartialEq)]
pub struct AttendanceEntry {
    pub display_name: String,
    pub role: String,
    pub join_dt: Option<String>,
    pub leave_dt: Option<String>,
    /// `totalAttendanceInSeconds` when Graph reports it, else derived
    /// from the first interval (best-effort; 0 when unknown).
    pub duration_secs: u64,
}

#[derive(Debug, Deserialize)]
struct AttendanceRecordsBody {
    #[serde(default)]
    value: Vec<AttendanceRecord>,
}

#[derive(Debug, Deserialize)]
struct AttendanceRecord {
    #[serde(rename = "emailAddress")]
    email: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(rename = "attendanceIntervals", default)]
    intervals: Vec<AttendanceInterval>,
    #[serde(rename = "totalAttendanceInSeconds")]
    total_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct AttendanceInterval {
    #[serde(rename = "joinDateTime")]
    join_dt: Option<String>,
    #[serde(rename = "leaveDateTime")]
    leave_dt: Option<String>,
    #[serde(rename = "durationInSeconds")]
    duration_secs: Option<u64>,
}

/// Parse an attendanceRecords list body. Pure; empty `value` = no rows
/// (the meeting has no report yet), never an error.
pub fn parse_attendance_records(json: &str) -> Result<Vec<AttendanceEntry>> {
    let body: AttendanceRecordsBody =
        serde_json::from_str(json).context("Failed to parse attendanceRecords response")?;
    Ok(body
        .value
        .into_iter()
        .map(|r| {
            let first = r.intervals.first();
            let name = r
                .display_name
                .filter(|s| !s.trim().is_empty())
                .or(r.email)
                .unwrap_or_else(|| "(unknown)".to_string());
            AttendanceEntry {
                display_name: name,
                role: r.role.unwrap_or_else(|| "attendee".to_string()),
                join_dt: first.and_then(|i| i.join_dt.clone()),
                leave_dt: r
                    .intervals
                    .last()
                    .and_then(|i| i.leave_dt.clone())
                    .or_else(|| first.and_then(|i| i.leave_dt.clone())),
                duration_secs: r.total_secs.unwrap_or_else(|| {
                    first.and_then(|i| i.duration_secs).unwrap_or(0)
                }),
            }
        })
        .collect())
}

/// First report id in an attendanceReports list body. Pure; None = no
/// report filed yet (common before/while the meeting runs).
pub fn parse_first_report_id(json: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct Reports {
        #[serde(default)]
        value: Vec<Report>,
    }
    #[derive(Deserialize)]
    struct Report {
        #[serde(default)]
        id: Option<String>,
    }
    let body: Reports =
        serde_json::from_str(json).context("Failed to parse attendanceReports response")?;
    Ok(body
        .value
        .into_iter()
        .find_map(|r| r.id.filter(|s| !s.trim().is_empty())))
}

/// Server capture toggles for one online meeting (GET shape = PATCH
/// shape; all three optional on write, omitted = unchanged).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CaptureOptions {
    pub allow_recording: Option<bool>,
    pub allow_transcription: Option<bool>,
    pub record_automatically: Option<bool>,
}

/// PATCH body for the toggles the caller sets (None = untouched).
/// Pure so tests pin the exact Graph shape.
pub fn capture_options_body(opts: &CaptureOptions) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    if let Some(v) = opts.allow_recording {
        body.insert("allowRecording".to_string(), v.into());
    }
    if let Some(v) = opts.allow_transcription {
        body.insert("allowTranscription".to_string(), v.into());
    }
    if let Some(v) = opts.record_automatically {
        body.insert("recordAutomatically".to_string(), v.into());
    }
    serde_json::Value::Object(body)
}

/// Parse a capture-options GET body. Pure; unknown/missing keys = None
/// (the viewer shows them as unknown, never as off).
pub fn parse_capture_options(json: &str) -> Result<CaptureOptions> {
    let v: serde_json::Value =
        serde_json::from_str(json).context("Failed to parse onlineMeeting response")?;
    let b = |k: &str| v.get(k).and_then(|x| x.as_bool());
    Ok(CaptureOptions {
        allow_recording: b("allowRecording"),
        allow_transcription: b("allowTranscription"),
        record_automatically: b("recordAutomatically"),
    })
}

/// Read one meeting's attendance (first report's records). Bails
/// pre-network on a blank id; a scope 403 surfaces as `HTTP 403 …`.
pub async fn meeting_attendance_data(
    client: &TeamsClient,
    meeting_id: &str,
) -> Result<Vec<AttendanceEntry>> {
    if meeting_id.trim().is_empty() {
        anyhow::bail!("meeting id must not be blank");
    }
    let reports = client
        .graph_get(&attendance_reports_path(meeting_id))
        .await?;
    let body = reports
        .text()
        .await
        .context("Failed to read attendanceReports response")?;
    let Some(report_id) = parse_first_report_id(&body)? else {
        return Ok(Vec::new());
    };
    let records = client
        .graph_get(&attendance_records_path(meeting_id, &report_id))
        .await?;
    let records_body = records
        .text()
        .await
        .context("Failed to read attendanceRecords response")?;
    parse_attendance_records(&records_body)
}

/// Read one meeting's server capture toggles. Bails pre-network on a
/// blank id; a scope 403 surfaces as `HTTP 403 …`.
pub async fn meeting_capture_options_data(
    client: &TeamsClient,
    meeting_id: &str,
) -> Result<CaptureOptions> {
    if meeting_id.trim().is_empty() {
        anyhow::bail!("meeting id must not be blank");
    }
    let resp = client.graph_get(&capture_options_path(meeting_id)).await?;
    let body = resp
        .text()
        .await
        .context("Failed to read onlineMeeting response")?;
    parse_capture_options(&body)
}

/// Write one meeting's server capture toggles (only the set fields).
pub async fn set_meeting_capture_options_data(
    client: &TeamsClient,
    meeting_id: &str,
    opts: &CaptureOptions,
) -> Result<CaptureOptions> {
    if meeting_id.trim().is_empty() {
        anyhow::bail!("meeting id must not be blank");
    }
    let body = capture_options_body(opts);
    if !body.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
        anyhow::bail!("no capture option set (nothing to write)");
    }
    let resp = client
        .graph_patch(&capture_options_path(meeting_id), &body)
        .await?;
    let out = resp
        .text()
        .await
        .context("Failed to read patched onlineMeeting response")?;
    parse_capture_options(&out)
}

/// Read one meeting's server capture toggles (read-only; the live 403
/// probe vehicle for the toggle path). Prints the HTTP status on failure.
pub async fn show_meeting_capture(meeting_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let opts = meeting_capture_options_data(&client, meeting_id).await?;
    println!("allowRecording: {:?}", opts.allow_recording);
    println!("allowTranscription: {:?}", opts.allow_transcription);
    println!("recordAutomatically: {:?}", opts.record_automatically);
    Ok(())
}

/// Print one meeting's attendance table (read-only; the live 403 probe
/// vehicle for the lane). Prints the HTTP status detail on failure.
pub async fn show_meeting_attendance(meeting_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let entries = meeting_attendance_data(&client, meeting_id).await?;
    if entries.is_empty() {
        println!("  (no attendance report filed for this meeting yet)");
        return Ok(());
    }
    println!("\nAttendance ({} present):", entries.len());
    println!("{:-<60}", "");
    for e in &entries {
        let span = match (&e.join_dt, &e.leave_dt) {
            (Some(j), Some(l)) => format!("{} – {}", j, l),
            (Some(j), None) => format!("{} – …", j),
            _ => "time unknown".to_string(),
        };
        println!("{} [{}] · {} · {}s", e.display_name, e.role, span, e.duration_secs);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_pin_graph_shapes() {
        assert_eq!(
            attendance_reports_path("ABC123"),
            "/me/onlineMeetings/ABC123/attendanceReports"
        );
        assert_eq!(
            attendance_records_path("ABC123", "R1"),
            "/me/onlineMeetings/ABC123/attendanceReports/R1/attendanceRecords"
        );
        assert_eq!(
            capture_options_path("ABC123"),
            "/communications/onlineMeetings/ABC123"
        );
    }

    #[test]
    fn blank_ids_bail_before_network() {
        assert!(attendance_reports_path("  ").ends_with("/attendanceReports"));
    }

    const RECORDS: &str = r#"{"value":[
        {"displayName":"Ava Stone","role":"presenter",
         "totalAttendanceInSeconds":1845,
         "attendanceIntervals":[{"joinDateTime":"2026-11-19T14:00:00Z",
         "leaveDateTime":"2026-11-19T14:30:45Z","durationInSeconds":1845}]},
        {"emailAddress":"bob@example.com",
         "attendanceIntervals":[{"joinDateTime":"2026-11-19T14:05:00Z"}]},
        {}]}"#;

    #[test]
    fn records_parse_names_roles_durations() {
        let rows = parse_attendance_records(RECORDS).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].display_name, "Ava Stone");
        assert_eq!(rows[0].role, "presenter");
        assert_eq!(rows[0].duration_secs, 1845);
        assert_eq!(
            rows[0].join_dt.as_deref(),
            Some("2026-11-19T14:00:00Z")
        );
        // Email falls back as the name; missing role = attendee.
        assert_eq!(rows[1].display_name, "bob@example.com");
        assert_eq!(rows[1].role, "attendee");
        assert_eq!(rows[1].duration_secs, 0);
        assert_eq!(rows[2].display_name, "(unknown)");
    }

    #[test]
    fn records_empty_value_is_no_rows_not_error() {
        assert!(parse_attendance_records(r#"{"value":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn first_report_id_picks_first_nonblank() {
        let id = parse_first_report_id(r#"{"value":[{"id":""},{"id":"R9"}]}"#).unwrap();
        assert_eq!(id.as_deref(), Some("R9"));
        assert!(parse_first_report_id(r#"{"value":[]}"#).unwrap().is_none());
    }

    #[test]
    fn capture_body_only_sets_set_fields() {
        let opts = CaptureOptions {
            allow_recording: Some(true),
            allow_transcription: None,
            record_automatically: Some(false),
        };
        let b = capture_options_body(&opts);
        assert_eq!(b, serde_json::json!({"allowRecording": true, "recordAutomatically": false}));
        let empty = capture_options_body(&CaptureOptions::default());
        assert!(empty.as_object().unwrap().is_empty());
    }

    #[test]
    fn capture_parse_unknown_keys_are_none() {
        let o = parse_capture_options(r#"{"allowRecording":true,"subject":"x"}"#).unwrap();
        assert_eq!(o.allow_recording, Some(true));
        assert_eq!(o.allow_transcription, None);
        assert_eq!(o.record_automatically, None);
    }

    #[test]
    fn capture_round_trip() {
        let o = CaptureOptions {
            allow_recording: Some(false),
            allow_transcription: Some(true),
            record_automatically: Some(false),
        };
        let back = parse_capture_options(&capture_options_body(&o).to_string()).unwrap();
        assert_eq!(back, o);
    }

}
