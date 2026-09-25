//! Upcoming Teams meetings from the Graph calendar + join-URL parsing.
//!
//! Lists the signed-in user's upcoming events with Teams join URLs
//! (Graph `calendarView`); the join-link parser accepts a pasted Teams
//! link (or raw thread id). Auth reuses the existing Graph token
//! (`https://graph.microsoft.com/.default`); no scope widening: the
//! first-party Teams client id already consents `Calendars.Read`, and a
//! 403 surfaces as the call's detail.
//!
//! Also home to the pure lobby state machine (`LobbyState`): joining a
//! meeting can park the caller in the lobby/waiting room until admitted,
//! so callers drive `idle → joining → lobby → admitted|failed`
//! off call-signaling events. No network here — just transitions.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

// ---------------------------------------------------------------------------
// Graph calendarView wire shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CalendarViewResponse {
    value: Vec<CalendarEvent>,
}

#[derive(Debug, Deserialize)]
struct DateTimeTimeZone {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    #[serde(rename = "timeZone")]
    time_zone: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnlineMeetingInfo {
    #[serde(rename = "joinUrl")]
    join_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OrganizerInfo {
    emailAddress: Option<EmailAddress>,
}

#[derive(Debug, Deserialize)]
struct EmailAddress {
    name: Option<String>,
    address: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CalendarEvent {
    id: String,
    subject: Option<String>,
    #[serde(rename = "isOnlineMeeting")]
    is_online_meeting: Option<bool>,
    #[serde(rename = "onlineMeeting")]
    online_meeting: Option<OnlineMeetingInfo>,
    start: Option<DateTimeTimeZone>,
    end: Option<DateTimeTimeZone>,
    organizer: Option<OrganizerInfo>,
    #[serde(rename = "webLink")]
    web_link: Option<String>,
}

// ---------------------------------------------------------------------------
// Meeting record
// ---------------------------------------------------------------------------

/// One upcoming calendar event with meeting-join metadata.
pub struct MeetingInfo {
    pub id: String,
    pub subject: String,
    /// `start.dateTime` verbatim (`YYYY-MM-DDTHH:MM:SS.fffffff`), when set.
    pub start: Option<String>,
    /// `end.dateTime` verbatim, when set.
    pub end: Option<String>,
    /// Teams join URL (`onlineMeeting.joinUrl`), when the event has one.
    pub join_url: Option<String>,
    /// Organizer display name, when set.
    pub organizer: Option<String>,
    /// True when Graph flags the event as an online meeting.
    pub is_online: bool,
}

fn meeting_info(e: CalendarEvent) -> MeetingInfo {
    let join_url = e.online_meeting.and_then(|m| m.join_url).filter(|u| {
        let t = u.trim();
        !t.is_empty()
    });
    let subject = e
        .subject
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "(no subject)".to_string());
    MeetingInfo {
        id: e.id,
        subject,
        start: e.start.and_then(|s| s.date_time),
        end: e.end.and_then(|s| s.date_time),
        join_url,
        organizer: e
            .organizer
            .and_then(|o| o.emailAddress)
            .and_then(|a| a.name),
        is_online: e.is_online_meeting.unwrap_or(false),
    }
}

/// Parse one Graph `calendarView` payload into meeting records.
/// Pure (no network) so tests pin the shape.
pub fn parse_calendar_view(json: &str) -> Result<Vec<MeetingInfo>> {
    let resp: CalendarViewResponse =
        serde_json::from_str(json).context("Failed to parse calendarView response")?;
    Ok(resp.value.into_iter().map(meeting_info).collect())
}

// ---------------------------------------------------------------------------
// UTC date formatting without chrono (Howard Hinnant days-from-civil)
// ---------------------------------------------------------------------------

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ` (UTC, std only).
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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Build the `calendarView` path for the window `[now, now+days]`.
/// Pure so tests pin the query shape.
pub fn calendar_view_path(now: u64, days: u64, limit: usize) -> String {
    let start = unix_to_iso8601(now);
    let end = unix_to_iso8601(now + days.saturating_mul(86_400));
    format!(
        "/me/calendar/calendarView?startDateTime={}&endDateTime={}&$top={}\
         &$orderby=start/dateTime\
         &$select=id,subject,isOnlineMeeting,onlineMeeting,start,end,organizer,webLink",
        encode_param(&start),
        encode_param(&end),
        limit
    )
}

// ---------------------------------------------------------------------------
// Data-returning API functions (no printing; for reuse by other callers)
// ---------------------------------------------------------------------------

/// Fetch upcoming meetings (next 7 days, soonest first).
pub async fn list_upcoming_meetings_data(
    client: &TeamsClient,
    limit: usize,
) -> Result<Vec<MeetingInfo>> {
    let path = calendar_view_path(now_secs(), 7, limit);
    let resp = client.graph_get(&path).await?;
    let body = resp
        .text()
        .await
        .context("Failed to read calendarView response")?;
    parse_calendar_view(&body)
}

/// List upcoming meetings (prints to stdout).
pub async fn list_upcoming_meetings(limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let meetings = list_upcoming_meetings_data(&client, limit).await?;

    println!("\nUpcoming meetings:");
    println!("{:-<60}", "");

    if meetings.is_empty() {
        println!("  (none in the next 7 days)");
        return Ok(());
    }

    for m in &meetings {
        let when = match (&m.start, &m.end) {
            (Some(s), Some(e)) => format!("{} – {}", s, e),
            (Some(s), None) => s.clone(),
            _ => "unscheduled".to_string(),
        };
        let org = m
            .organizer
            .as_deref()
            .map(|o| format!(" (org: {})", o))
            .unwrap_or_default();
        let online = if m.is_online { " [online]" } else { "" };
        println!("{}{}{}", m.subject, org, online);
        println!("  When: {}", when);
        match &m.join_url {
            Some(u) => println!("  Join: {}", u),
            None => println!("  Join: (no Teams link)"),
        }
        println!();
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Join-URL parsing (pure)
// ---------------------------------------------------------------------------

/// Where a pasted join string leads.
/// `kind` is one of `thread` (direct call leg), `meeting-id`
/// (browser/deep-link join), `url` (other https link, opened as-is),
/// `unknown` (not joinable — show a hint, never dial).
pub struct JoinTarget {
    pub kind: String,
    pub thread_id: Option<String>,
    pub meeting_id: Option<String>,
    pub url: String,
}

/// Percent-decode `%XX` runs (best-effort; malformed runs pass through).
fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hex = |c: u8| {
                (c as char).to_digit(16).unwrap_or(0) as u8
            };
            out.push(hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// True for native chat-service thread ids (`19:…@thread.v2`,
/// `19:…@thread.tacv2`, `48:…`, `8:…`).
fn looks_like_thread_id(s: &str) -> bool {
    let t = s.trim();
    (t.starts_with("19:") || t.starts_with("48:") || t.starts_with("8:"))
        && !t.chars().any(char::is_whitespace)
}

/// Pull the first `19:…@thread…` span out of a (decoded) URL.
/// Stops at `/`, `?`, `&`, `#`, whitespace, or end.
fn extract_thread_id(decoded: &str) -> Option<String> {
    let start = decoded.find("19:")?;
    let tail = &decoded[start..];
    let end = tail
        .char_indices()
        .find(|(_, c)| matches!(c, '/' | '?' | '&' | '#' | '"' | '\'' | ' ' | '\t' | '\n'))
        .map(|(i, _)| i)
        .unwrap_or(tail.len());
    let id = tail[..end].trim_end_matches(['.', ',', ')', ']']).to_string();
    if id.contains('@') { Some(id) } else { None }
}

/// Pull a `teams.live.com/meet/<id>` meeting id out of a URL.
fn extract_live_meet_id(url: &str) -> Option<String> {
    let marker = "teams.live.com/meet/";
    let start = url.find(marker)? + marker.len();
    let tail = &url[start..];
    let end = tail
        .char_indices()
        .find(|(_, c)| matches!(c, '/' | '?' | '&' | '#' | '"' | '\'' | ' ' | '\t' | '\n'))
        .map(|(i, _)| i)
        .unwrap_or(tail.len());
    let id = tail[..end].trim().to_string();
    if id.is_empty() { None } else { Some(id) }
}

/// Classify a pasted join string. Pure; never dials.
pub fn parse_join_url(raw: &str) -> JoinTarget {
    let trimmed = raw.trim().trim_matches(['<', '>', '"', '\'']).trim().to_string();
    if trimmed.is_empty() {
        return JoinTarget {
            kind: "unknown".to_string(),
            thread_id: None,
            meeting_id: None,
            url: String::new(),
        };
    }
    // Bare thread id pastes straight through to the call leg.
    if looks_like_thread_id(&trimmed) {
        return JoinTarget {
            kind: "thread".to_string(),
            thread_id: Some(trimmed.clone()),
            meeting_id: None,
            url: trimmed,
        };
    }
    let decoded = pct_decode(&trimmed);
    let lower = decoded.to_ascii_lowercase();
    if lower.contains("teams.microsoft.com/l/meetup-join")
        || lower.contains("teams.microsoft.com/meet")
    {
        if let Some(tid) = extract_thread_id(&decoded) {
            return JoinTarget {
                kind: "thread".to_string(),
                thread_id: Some(tid),
                meeting_id: None,
                url: trimmed,
            };
        }
        // meetup-join shape without an extractable thread: open as-is.
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            return JoinTarget {
                kind: "url".to_string(),
                thread_id: None,
                meeting_id: None,
                url: trimmed,
            };
        }
    }
    if lower.contains("teams.live.com/meet/") {
        if let Some(mid) = extract_live_meet_id(&trimmed) {
            return JoinTarget {
                kind: "meeting-id".to_string(),
                thread_id: None,
                meeting_id: Some(mid),
                url: trimmed,
            };
        }
    }
    if trimmed.starts_with("https://") {
        return JoinTarget {
            kind: "url".to_string(),
            thread_id: None,
            meeting_id: None,
            url: trimmed,
        };
    }
    JoinTarget {
        kind: "unknown".to_string(),
        thread_id: None,
        meeting_id: None,
        url: trimmed,
    }
}

// ---------------------------------------------------------------------------
// Lobby state machine (pure)
// ---------------------------------------------------------------------------

/// Join/lobby states: `idle → joining → lobby → admitted`, with `failed`
/// reachable from `joining`/`lobby` (rejected, timed out, or organizer
/// declined). `admitted` is terminal for the join flow (the call slot
/// owns `connected` onwards); `reset` returns to `idle` from anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobbyState {
    Idle,
    Joining,
    Lobby,
    Admitted,
    Failed,
}

impl LobbyState {
    pub fn as_str(&self) -> &'static str {
        match self {
            LobbyState::Idle => "idle",
            LobbyState::Joining => "joining",
            LobbyState::Lobby => "lobby",
            LobbyState::Admitted => "admitted",
            LobbyState::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> LobbyState {
        match s {
            "joining" => LobbyState::Joining,
            "lobby" => LobbyState::Lobby,
            "admitted" => LobbyState::Admitted,
            "failed" => LobbyState::Failed,
            _ => LobbyState::Idle,
        }
    }
}

/// Join-flow events (driven by call-signaling callbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobbyEvent {
    /// User pressed Join (or a parsed thread leg started placing).
    Start,
    /// Signaling placed the leg; the meeting may still hold us.
    Placed,
    /// Server parked us in the waiting room (or the leg is still
    /// placing/ringing past the lobby grace window).
    LobbySignal,
    /// Admitted to the meeting (leg connected).
    Admit,
    /// Rejected / declined / leg ended before admission.
    Reject,
    /// Back to idle (dismiss, retry, new join string).
    Reset,
}

/// Pure transition: `(state, event) → state`.
pub fn lobby_next(state: LobbyState, event: LobbyEvent) -> LobbyState {
    use LobbyEvent as E;
    use LobbyState as S;
    if event == E::Reset {
        return S::Idle;
    }
    match (state, event) {
        (S::Idle, E::Start) => S::Joining,
        (S::Joining, E::Placed) => S::Joining,
        (S::Joining, E::LobbySignal) => S::Lobby,
        (S::Joining, E::Admit) => S::Admitted,
        (S::Joining, E::Reject) => S::Failed,
        (S::Lobby, E::Placed) => S::Lobby,
        (S::Lobby, E::Admit) => S::Admitted,
        (S::Lobby, E::Reject) => S::Failed,
        (S::Failed, E::Start) => S::Joining,
        // Terminal states ignore progress events (stale callbacks).
        (s, _) => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW_JSON: &str = r#"{
        "value": [
            {"id": "E1", "subject": "Standup",
             "isOnlineMeeting": true,
             "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0"},
             "start": {"dateTime": "2026-09-24T09:00:00.0000000", "timeZone": "UTC"},
             "end": {"dateTime": "2026-09-24T09:15:00.0000000", "timeZone": "UTC"},
             "organizer": {"emailAddress": {"name": "Doe, Jane", "address": "j@x.io"}}},
            {"id": "E2", "subject": "  ",
             "isOnlineMeeting": false,
             "start": {"dateTime": "2026-09-25T12:00:00.0000000", "timeZone": "UTC"},
             "end": null, "organizer": null},
            {"id": "E3"}
        ]
    }"#;

    #[test]
    fn calendar_parse_fields() {
        let ms = parse_calendar_view(VIEW_JSON).unwrap();
        assert_eq!(ms.len(), 3);
        assert_eq!(ms[0].id, "E1");
        assert_eq!(ms[0].subject, "Standup");
        assert!(ms[0].is_online);
        assert_eq!(
            ms[0].join_url.as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0")
        );
        assert_eq!(ms[0].start.as_deref(), Some("2026-09-24T09:00:00.0000000"));
        assert_eq!(ms[0].organizer.as_deref(), Some("Doe, Jane"));
        // Blank subject falls back; no join URL, no organizer.
        assert_eq!(ms[1].subject, "(no subject)");
        assert!(!ms[1].is_online);
        assert!(ms[1].join_url.is_none());
        assert!(ms[1].end.is_none());
        // Bare event keeps its id and takes every default.
        assert_eq!(ms[2].id, "E3");
        assert_eq!(ms[2].subject, "(no subject)");
        assert!(ms[2].start.is_none());
    }

    #[test]
    fn calendar_parse_rejects_garbage() {
        assert!(parse_calendar_view("not json").is_err());
        assert!(parse_calendar_view("{\"value\": {}}").is_err());
    }

    #[test]
    fn calendar_path_shape() {
        // 2026-09-24T00:00:00Z = 1790208000.
        let p = calendar_view_path(1_790_208_000, 7, 25);
        assert!(p.starts_with("/me/calendar/calendarView?"), "{}", p);
        assert!(p.contains("startDateTime=2026-09-24T00%3A00%3A00Z"), "{}", p);
        assert!(p.contains("endDateTime=2026-10-01T00%3A00%3A00Z"), "{}", p);
        assert!(p.contains("$top=25"), "{}", p);
        assert!(p.contains("$orderby=start/dateTime"), "{}", p);
    }

    #[test]
    fn unix_to_iso8601_spot_checks() {
        assert_eq!(unix_to_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_to_iso8601(1_790_208_000), "2026-09-24T00:00:00Z");
        assert_eq!(unix_to_iso8601(1_790_208_000 + 86_400), "2026-09-25T00:00:00Z");
        // Leap day 2024-02-29T12:34:56Z.
        assert_eq!(unix_to_iso8601(1_709_210_096), "2024-02-29T12:34:56Z");
    }

    #[test]
    fn join_url_meetup_thread() {
        let t = parse_join_url(
            "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=%7b%7d",
        );
        assert_eq!(t.kind, "thread");
        assert_eq!(t.thread_id.as_deref(), Some("19:meeting_abc@thread.v2"));
    }

    #[test]
    fn join_url_meetup_unencoded() {
        let t = parse_join_url("https://teams.microsoft.com/l/meetup-join/19:meeting_abc@thread.v2/0");
        assert_eq!(t.kind, "thread");
        assert_eq!(t.thread_id.as_deref(), Some("19:meeting_abc@thread.v2"));
    }

    #[test]
    fn join_url_bare_thread_id() {
        let t = parse_join_url("  19:abc_def@thread.v2  ");
        assert_eq!(t.kind, "thread");
        assert_eq!(t.thread_id.as_deref(), Some("19:abc_def@thread.v2"));
    }

    #[test]
    fn join_url_live_meet_id() {
        let t = parse_join_url("https://teams.live.com/meet/9347123456789?p=abc");
        assert_eq!(t.kind, "meeting-id");
        assert_eq!(t.meeting_id.as_deref(), Some("9347123456789"));
    }

    #[test]
    fn join_url_other_https_opens_as_is() {
        let t = parse_join_url("https://example.com/x");
        assert_eq!(t.kind, "url");
        assert_eq!(t.url, "https://example.com/x");
    }

    #[test]
    fn join_url_unknown_shapes() {
        for raw in ["", "   ", "hello", "ftp://x", "19:not a thread id"] {
            let t = parse_join_url(raw);
            assert_eq!(t.kind, "unknown", "input {:?}", raw);
            assert!(t.thread_id.is_none());
        }
    }

    #[test]
    fn lobby_happy_path() {
        let mut s = LobbyState::Idle;
        s = lobby_next(s, LobbyEvent::Start);
        assert_eq!(s, LobbyState::Joining);
        s = lobby_next(s, LobbyEvent::Placed);
        assert_eq!(s, LobbyState::Joining);
        s = lobby_next(s, LobbyEvent::Admit);
        assert_eq!(s, LobbyState::Admitted);
    }

    #[test]
    fn lobby_waiting_room_path() {
        let mut s = lobby_next(LobbyState::Idle, LobbyEvent::Start);
        s = lobby_next(s, LobbyEvent::LobbySignal);
        assert_eq!(s, LobbyState::Lobby);
        s = lobby_next(s, LobbyEvent::Placed); // stale progress ignored
        assert_eq!(s, LobbyState::Lobby);
        s = lobby_next(s, LobbyEvent::Admit);
        assert_eq!(s, LobbyState::Admitted);
    }

    #[test]
    fn lobby_reject_and_retry() {
        let s = lobby_next(LobbyState::Idle, LobbyEvent::Start);
        let s = lobby_next(s, LobbyEvent::LobbySignal);
        let s = lobby_next(s, LobbyEvent::Reject);
        assert_eq!(s, LobbyState::Failed);
        assert_eq!(lobby_next(s, LobbyEvent::Admit), LobbyState::Failed);
        assert_eq!(lobby_next(s, LobbyEvent::Start), LobbyState::Joining);
        assert_eq!(lobby_next(s, LobbyEvent::Reset), LobbyState::Idle);
    }

    #[test]
    fn lobby_reset_from_anywhere_and_str_roundtrip() {
        for s in [
            LobbyState::Idle,
            LobbyState::Joining,
            LobbyState::Lobby,
            LobbyState::Admitted,
            LobbyState::Failed,
        ] {
            assert_eq!(lobby_next(s, LobbyEvent::Reset), LobbyState::Idle);
            assert_eq!(LobbyState::from_str(s.as_str()), s);
        }
        assert_eq!(LobbyState::from_str("bogus"), LobbyState::Idle);
    }
}
