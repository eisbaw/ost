//! Breakout rooms management (Teams MiddleTier REST, RE-mapped).
//!
//! Teams web v2 manages rooms via MiddleTier `beta/breakoutRoom/...`
//! (NOT Graph, NOT ACS, NOT Call Automation). Shapes below are verbatim
//! from the frozen web bundle module 287905 (lane rebreakout,
//! `STATIC-MAPPED`): every request builder is pinned by the tests in
//! this file, so a drift from the mapped shape fails the suite.
//!
//! Base URL: `{middleTier}` is `region_gtms.middleTier` (per-user
//! Discover value); the web fallback literal is
//! `https://teams.microsoft.com/api/mt/part/dogfood`. ASSUMPTION (flagged
//! per brief; capture-time pin): older public RE (fossteams) and our own
//! live-proven MT calls (apps catalog, team create) use
//! `.../api/mt/<region>` with NO `/part/` segment. The executors below use
//! the Discover value when present (authoritative either way) and the
//! mapped `/part/dogfood` literal only when Discover is missing; the
//! phase-2 capture decides which fallback is real.
//!
//! Auth: the web shell sends the AAD CAE user token plus STM headers with
//! source `CMDStable` and NO skype token post-migration. ASSUMPTION
//! (flagged; capture-time pin): the exact STM wire header names are
//! unpinned until the phase-2 capture, so the executors reuse the proven
//! `TeamsClient::mt_*` auth (Bearer AAD + `X-Skypetoken` +
//! `x-ms-client-type: desktop`) — the same calls the apps catalog and
//! team create already make live. If the capture shows MT refusing the
//! skype header on this route family, that is a one-line transport swap;
//! every path/body builder stays as pinned here.
//!
//! Identity: the signed-in user must be the meeting organizer,
//! co-organizer, or an appointed rooms manager, or MT refuses the writes.
//!
//! Op catalogue (`<C>` = encoded coords, `?newVersion=` = requestVersion
//! suffix where the bundle sends one):
//!  1 list       GET    beta/breakoutRoom/bulk/<mainC>
//!  2 create     POST   beta/breakoutRoom/<mainC>[?newVersion=]
//!  3 add one    POST   beta/breakoutRoom/add/<mainC>[?newVersion=]
//!  4 get one    GET    beta/breakoutRoom/details/<roomC>
//!  5 update one PUT    beta/breakoutRoom/<roomC>[?newVersion=]
//!  6 delete one DELETE beta/breakoutRoom/<roomC>[?newVersion=]
//!  7 delete all DELETE beta/breakoutRoom/rooms/all/<mainC>[?newVersion=]
//!  8 bulk       PUT    beta/breakoutRoom/bulk[/v2]/<mainC>[?newVersion=]
//!  9 open       POST   beta/breakoutRoom/open/<mainC>[?newVersion=]
//! 10 close      POST   beta/breakoutRoom/close/<mainC>[?newVersion=]
//! 11 get set    GET    beta/breakoutRoom/settings/<C>
//! 12 put set    PUT    beta/breakoutRoom/settings/<C>[?newVersion=]
//! 13 assignees  GET    beta/breakoutRoom/assignees/<C>?<params>
//! 14 announce   POST   beta/breakoutRoom/announce/<mainC>   (NO newVersion)

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::client::TeamsClient;

/// Mapped web fallback for the MiddleTier base (capture-time pin — see
/// module docs). Used ONLY when Discover has no `middleTier` value.
pub const BREAKOUT_FALLBACK_MT: &str = "https://teams.microsoft.com/api/mt/part/dogfood";

/// Teams caps (web config `coreSettings.BreakoutRoom`).
pub const MAX_ROOMS: usize = 50;
pub const TOTAL_PARTICIPANTS_LIMIT: usize = 300;

/// The five-part meeting coordinates every breakout URL embeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingCoords {
    pub tenant_id: String,
    pub organizer_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub additional_message_id: Option<String>,
}

impl MeetingCoords {
    /// Parse from the JSON the CLI/FFI accept (`tenant_id`,
    /// `organizer_id`, `thread_id`, `message_id`,
    /// `additional_message_id?`). Blank required parts are rejected
    /// before any network.
    pub fn from_json(v: &Value) -> Result<Self> {
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .map(|x| x.trim().to_string())
                .unwrap_or_default()
        };
        let c = MeetingCoords {
            tenant_id: s("tenant_id"),
            organizer_id: s("organizer_id"),
            thread_id: s("thread_id"),
            message_id: s("message_id"),
            additional_message_id: {
                let a = s("additional_message_id");
                if a.is_empty() { None } else { Some(a) }
            },
        };
        for (what, val) in [
            ("tenant_id", &c.tenant_id),
            ("organizer_id", &c.organizer_id),
            ("thread_id", &c.thread_id),
            ("message_id", &c.message_id),
        ] {
            if val.is_empty() {
                bail!("breakout coords: empty {}", what);
            }
            if val.contains('/') || val.contains('?') || val.contains('#') {
                bail!("breakout coords: {} breaks the path", what);
            }
        }
        if let Some(a) = &c.additional_message_id {
            if a.contains('/') || a.contains('?') || a.contains('#') {
                bail!("breakout coords: additional_message_id breaks the path");
            }
        }
        Ok(c)
    }
}

/// Bundle `w()`: thread ids ride the path with `:` rewritten to `_`.
pub fn encode_thread_id(thread_id: &str) -> String {
    thread_id.replace(':', "_")
}

/// Bundle `B()`: decode an encoded thread id back (`19_…` → `19:…`,
/// preserving `19_<scope>_` prefixes like `19:gcch:`).
pub fn decode_thread_id(encoded: &str) -> String {
    if let Some(rest) = encoded.strip_prefix("19_") {
        for scope in ["gcch_", "dod_", "gal_", "fr_"] {
            if let Some(inner) = rest.strip_prefix(scope) {
                let name = scope.trim_end_matches('_');
                return format!("19:{}:{}", name, inner);
            }
        }
        return format!("19:{}", rest);
    }
    encoded.to_string()
}

/// Bundle `h()`: the `<C>` path segment — `E()` rewrites `:`→`_` in the
/// thread id, then join `tenant/organizer/thread/message[/additional]`.
pub fn encode_coords(c: &MeetingCoords) -> String {
    let thread = encode_thread_id(&c.thread_id);
    let mut out = [c.tenant_id.as_str(), c.organizer_id.as_str(), thread.as_str(), c.message_id.as_str()].join("/");
    if let Some(a) = &c.additional_message_id {
        out.push('/');
        out.push_str(a);
    }
    out
}

/// Bundle `g()`: `?newVersion=<requestVersion>` when the caller passes a
/// version, else the empty string.
pub fn version_suffix(request_version: Option<&str>) -> String {
    match request_version {
        Some(v) if !v.trim().is_empty() => format!("?newVersion={}", v.trim()),
        _ => String::new(),
    }
}

/// Join an MT base (Discover value or the mapped fallback) with a
/// relative `beta/breakoutRoom/...` path. Pure so tests pin it.
pub fn mt_join(mt_base: &str, rel: &str) -> String {
    format!("{}/{}", mt_base.trim_end_matches('/'), rel.trim_start_matches('/'))
}

/// The MT base the executors use: Discover's `middleTier` when present,
/// else the mapped fallback literal (capture-time pin — module docs).
pub fn mt_base_for(client: &TeamsClient) -> String {
    client.middle_tier_base_or(BREAKOUT_FALLBACK_MT)
}

/// One breakout participant as the wire carries it. All three fields
/// are optional on the way in; [`normalize_participant`] (bundle `P()`)
/// fills the gaps the way the web shell does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BreakoutParticipant {
    pub mri: Option<String>,
    pub upn: Option<String>,
    pub object_id: Option<String>,
}

impl BreakoutParticipant {
    pub fn from_json(v: &Value) -> Self {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
        BreakoutParticipant { mri: s("mri"), upn: s("upn"), object_id: s("objectId") }
    }

    pub fn to_json(&self) -> Value {
        json!({ "mri": self.mri, "upn": self.upn, "objectId": self.object_id })
    }
}

/// Looks like a UPN (email-shaped). The bundle's `VU()` predicate is a
/// minified classifier we cannot transcribe; this heuristic (contains
/// `@` with a dot after it) is flagged as an assumption.
fn looks_like_upn(s: &str) -> bool {
    match s.find('@') {
        Some(at) => s[at + 1..].contains('.'),
        None => false,
    }
}

/// Looks like a guest or phone MRI (bundle `Ku()`/`W4()` inputs).
/// ASSUMPTION (flagged): exact classifiers are minified away; guests
/// carry `#EXT#`, phone/SMS threads ride `tel:` or bare digits.
fn looks_like_guest_or_phone_mri(mri: &str) -> bool {
    mri.contains("#EXT#") || mri.starts_with("tel:") || mri.chars().all(|c| c.is_ascii_digit() || c == '+')
}

/// Bundle `P()`: normalize one participant for a write body. Guest/phone
/// MRIs get a generated object id when missing (the bundle also rewrites
/// the mri to an `8:orgid:` composite from minified constants — NOT
/// reproduced: capture-time pin, flagged; we keep the caller's mri and
/// only fill object id + upn). Otherwise upn defaults from an
/// email-shaped mri.
pub fn normalize_participant(p: &BreakoutParticipant) -> BreakoutParticipant {
    if let Some(mri) = &p.mri {
        if looks_like_guest_or_phone_mri(mri) {
            return BreakoutParticipant {
                mri: Some(mri.clone()),
                upn: Some(mri.clone()),
                object_id: Some(p.object_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string())),
            };
        }
        if looks_like_upn(mri) && p.upn.is_none() {
            return BreakoutParticipant {
                mri: Some(mri.clone()),
                upn: Some(mri.clone()),
                object_id: p.object_id.clone(),
            };
        }
    }
    p.clone()
}

/// Encode one coords OBJECT for a write body (bulk/open/close carry
/// rooms as `{coordinates: {...}}`). Accepts the caller's snake_case
/// (`thread_id`, …) or the wire's camelCase (`threadId`, …); emits the
/// wire object with the bundle `E()` thread rewrite (`:`→`_`) applied.
/// Callers always pass logical (unencoded) coords — the encoding lives
/// here, in one place.
pub fn coords_to_wire(v: &Value) -> Value {
    let pick = |snake: &str, camel: &str| {
        v.get(camel).or_else(|| v.get(snake)).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    let mut o = serde_json::Map::new();
    o.insert("tenantId".to_string(), Value::String(pick("tenant_id", "tenantId")));
    o.insert("organizerId".to_string(), Value::String(pick("organizer_id", "organizerId")));
    o.insert("threadId".to_string(), Value::String(encode_thread_id(&pick("thread_id", "threadId"))));
    o.insert("messageId".to_string(), Value::String(pick("message_id", "messageId")));
    let add = v
        .get("additionalMessageId")
        .or_else(|| v.get("additional_message_id"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if !add.is_empty() {
        o.insert("additionalMessageId".to_string(), Value::String(add.to_string()));
    }
    Value::Object(o)
}

/// Bundle `I()`: normalize one participant from a read response — when
/// upn is present and email-shaped it becomes the mri.
pub fn denormalize_participant(p: &BreakoutParticipant) -> BreakoutParticipant {
    if let Some(upn) = &p.upn {
        if looks_like_upn(upn) {
            return BreakoutParticipant {
                mri: Some(upn.clone()),
                upn: Some(upn.clone()),
                object_id: p.object_id.clone(),
            };
        }
    }
    p.clone()
}

// ---------------------------------------------------------------------------
// Op 1 · list · GET beta/breakoutRoom/bulk/<mainC>
// ---------------------------------------------------------------------------

pub fn list_url(mt_base: &str, main: &MeetingCoords) -> String {
    mt_join(mt_base, &format!("beta/breakoutRoom/bulk/{}", encode_coords(main)))
}

// ---------------------------------------------------------------------------
// Op 2 · create · POST beta/breakoutRoom/<mainC>[?newVersion=]
// ---------------------------------------------------------------------------

pub fn create_url(mt_base: &str, main: &MeetingCoords, request_version: Option<&str>) -> String {
    mt_join(
        mt_base,
        &format!("beta/breakoutRoom/{}{}", encode_coords(main), version_suffix(request_version)),
    )
}

/// Create body: `{breakoutRoomConfigurations:{roomCount},
/// preconfiguration:[...], settings?}`. Each preconfiguration entry
/// carries its (P-normalized) participants plus the caller's extra keys
/// verbatim (the bundle spreads the config object).
pub fn create_body(room_count: usize, preconfiguration: &[Value], settings: Option<&Value>) -> Value {
    let pre: Vec<Value> = preconfiguration
        .iter()
        .map(|entry| {
            let mut e = entry.clone();
            if let Some(parts) = entry.get("participants").and_then(|p| p.as_array()) {
                let norm: Vec<Value> =
                    parts.iter().map(|p| normalize_participant(&BreakoutParticipant::from_json(p)).to_json()).collect();
                e["participants"] = Value::Array(norm);
            }
            e
        })
        .collect();
    let mut body = json!({
        "breakoutRoomConfigurations": { "roomCount": room_count },
        "preconfiguration": pre,
    });
    if let Some(s) = settings {
        body["settings"] = settings_body(s);
    }
    body
}

// ---------------------------------------------------------------------------
// Op 3 · add one · POST beta/breakoutRoom/add/<mainC>[?newVersion=]
// ---------------------------------------------------------------------------

pub fn add_url(mt_base: &str, main: &MeetingCoords, request_version: Option<&str>) -> String {
    mt_join(
        mt_base,
        &format!("beta/breakoutRoom/add/{}{}", encode_coords(main), version_suffix(request_version)),
    )
}

/// Add body: `{name, participants}` verbatim (the bundle does NOT
/// P-normalize here — `participants: s` passes straight through).
pub fn add_body(name: &str, participants: &[Value]) -> Value {
    json!({ "name": name, "participants": participants })
}

// ---------------------------------------------------------------------------
// Op 4 · get one · GET beta/breakoutRoom/details/<roomC>
// ---------------------------------------------------------------------------

pub fn get_url(mt_base: &str, room: &MeetingCoords) -> String {
    mt_join(mt_base, &format!("beta/breakoutRoom/details/{}", encode_coords(room)))
}

// ---------------------------------------------------------------------------
// Op 5 · update one (assign/move) · PUT beta/breakoutRoom/<roomC>[?newVersion=]
// ---------------------------------------------------------------------------

pub fn update_url(mt_base: &str, room: &MeetingCoords, request_version: Option<&str>) -> String {
    mt_join(
        mt_base,
        &format!("beta/breakoutRoom/{}{}", encode_coords(room), version_suffix(request_version)),
    )
}

/// Update body: `{name, participants (P-normalized),
/// restrictChatAccess}`.
pub fn update_body(name: &str, participants: &[Value], restrict_chat_access: bool) -> Value {
    let norm: Vec<Value> =
        participants.iter().map(|p| normalize_participant(&BreakoutParticipant::from_json(p)).to_json()).collect();
    json!({ "name": name, "participants": norm, "restrictChatAccess": restrict_chat_access })
}

// ---------------------------------------------------------------------------
// Op 6 · delete one · DELETE beta/breakoutRoom/<roomC>[?newVersion=]
// ---------------------------------------------------------------------------

pub fn delete_url(mt_base: &str, room: &MeetingCoords, request_version: Option<&str>) -> String {
    mt_join(
        mt_base,
        &format!("beta/breakoutRoom/{}{}", encode_coords(room), version_suffix(request_version)),
    )
}

// ---------------------------------------------------------------------------
// Op 7 · delete all · DELETE beta/breakoutRoom/rooms/all/<mainC>[?newVersion=]
// ---------------------------------------------------------------------------

pub fn delete_all_url(mt_base: &str, main: &MeetingCoords, request_version: Option<&str>) -> String {
    mt_join(
        mt_base,
        &format!("beta/breakoutRoom/rooms/all/{}{}", encode_coords(main), version_suffix(request_version)),
    )
}

// ---------------------------------------------------------------------------
// Op 8 · bulk assign/move · PUT beta/breakoutRoom/bulk[/v2]/<mainC>[?newVersion=]
// ---------------------------------------------------------------------------

pub fn bulk_url(
    mt_base: &str,
    main: &MeetingCoords,
    request_version: Option<&str>,
    total_participant_count: Option<u64>,
) -> String {
    let v2 = if total_participant_count.is_some() { "bulk/v2/" } else { "bulk/" };
    mt_join(
        mt_base,
        &format!("beta/breakoutRoom/{}{}{}", v2, encode_coords(main), version_suffix(request_version)),
    )
}

/// Bulk body. v1 (no total): a bare array of
/// `{coordinates (E-encoded), roomName, participants (P-normalized),
/// settings:{restrictChatAccess}}`. v2 (total known):
/// `{totalParticipantCount, breakoutRooms:[...]}`.
pub fn bulk_body(rooms: &[Value], total_participant_count: Option<u64>) -> Value {
    let norm: Vec<Value> = rooms
        .iter()
        .map(|r| {
            let coords = r
                .get("coordinates")
                .map(coords_to_wire)
                .unwrap_or(Value::Null);
            let parts = r
                .get("participants")
                .and_then(|p| p.as_array())
                .map(|ps| {
                    Value::Array(
                        ps.iter().map(|p| normalize_participant(&BreakoutParticipant::from_json(p)).to_json()).collect(),
                    )
                })
                .unwrap_or(Value::Array(vec![]));
            json!({
                "coordinates": coords,
                "roomName": r.get("roomName").cloned().unwrap_or(Value::Null),
                "participants": parts,
                "settings": { "restrictChatAccess": r.get("restrictChatAccess").cloned().unwrap_or(Value::Null) },
            })
        })
        .collect();
    match total_participant_count {
        Some(n) => json!({ "totalParticipantCount": n, "breakoutRooms": norm }),
        None => Value::Array(norm),
    }
}

// ---------------------------------------------------------------------------
// Op 9 · open (1..n) · POST beta/breakoutRoom/open/<mainC>[?newVersion=]
// ---------------------------------------------------------------------------

pub fn open_url(mt_base: &str, main: &MeetingCoords, request_version: Option<&str>) -> String {
    mt_join(
        mt_base,
        &format!("beta/breakoutRoom/open/{}{}", encode_coords(main), version_suffix(request_version)),
    )
}

/// Open body: `{rooms:[{coordinates (E-encoded)}], useNewTimerFlow,
/// totalParticipantCount}`. The bundle always sends totalParticipantCount
/// (possibly undefined→dropped); we send it only when known.
pub fn open_body(room_coords: &[Value], use_new_timer_flow: bool, total_participant_count: Option<u64>) -> Value {
    let rooms: Vec<Value> = room_coords.iter().map(|c| json!({ "coordinates": coords_to_wire(c) })).collect();
    let mut body = json!({ "rooms": rooms, "useNewTimerFlow": use_new_timer_flow });
    if let Some(n) = total_participant_count {
        body["totalParticipantCount"] = json!(n);
    }
    body
}

// ---------------------------------------------------------------------------
// Op 10 · close (1..n) · POST beta/breakoutRoom/close/<mainC>[?newVersion=]
// ---------------------------------------------------------------------------

pub fn close_url(mt_base: &str, main: &MeetingCoords, request_version: Option<&str>) -> String {
    mt_join(
        mt_base,
        &format!("beta/breakoutRoom/close/{}{}", encode_coords(main), version_suffix(request_version)),
    )
}

/// Close body: `{rooms:[{coordinates (E-encoded)}],
/// totalParticipantCount?}`.
pub fn close_body(room_coords: &[Value], total_participant_count: Option<u64>) -> Value {
    let rooms: Vec<Value> = room_coords.iter().map(|c| json!({ "coordinates": coords_to_wire(c) })).collect();
    let mut body = json!({ "rooms": rooms });
    if let Some(n) = total_participant_count {
        body["totalParticipantCount"] = json!(n);
    }
    body
}

// ---------------------------------------------------------------------------
// Ops 11+12 · settings · GET/PUT beta/breakoutRoom/settings/<C>[?newVersion=]
// ---------------------------------------------------------------------------

pub fn settings_url(mt_base: &str, coords: &MeetingCoords, request_version: Option<&str>) -> String {
    mt_join(
        mt_base,
        &format!("beta/breakoutRoom/settings/{}{}", encode_coords(coords), version_suffix(request_version)),
    )
}

/// Bundle `D()`: the settings write shape. `extensionData` is ALWAYS
/// present (possibly `{}`); its two values are STRINGS (`true`→`"true"`).
/// Reads the caller's friendly keys (`allow_users_to_select_breakout_room`
/// or the wire `allowUsersToSelectBreakoutRoom`, etc.).
pub fn settings_body(settings: &Value) -> Value {
    let pick = |snake: &str, camel: &str| settings.get(snake).or_else(|| settings.get(camel)).cloned();
    let mut ext = serde_json::Map::new();
    for (snake, camel, wire) in [
        ("allow_join_back_to_main_meeting", "allowJoinBackToMainMeeting", "allowJoinBackToMainMeeting"),
        ("allow_move_participants_automatically", "allowMoveParticipantsAutomatically", "allowMoveParticipantsAutomatically"),
    ] {
        if let Some(v) = pick(snake, camel) {
            // Bundle `.toString()`: booleans become "true"/"false".
            let s = match v {
                Value::Bool(b) => b.to_string(),
                Value::String(t) => t,
                other => other.to_string(),
            };
            ext.insert(wire.to_string(), Value::String(s));
        }
    }
    json!({
        "allowUsersToSelectBreakoutRoom": pick("allow_users_to_select_breakout_room", "allowUsersToSelectBreakoutRoom"),
        "roomDuration": pick("room_duration", "roomDuration"),
        "managers": pick("managers", "managers"),
        "extensionData": ext,
    })
}

/// Parsed room settings (bundle `U()`): extensionData strings `"true"` /
/// `"false"` (case-insensitive) become booleans; anything else → None.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoomSettings {
    pub allow_users_to_select_breakout_room: Option<bool>,
    pub room_duration: Option<String>,
    pub allow_join_back_to_main_meeting: Option<bool>,
    pub allow_move_participants_automatically: Option<bool>,
    pub managers: Vec<Value>,
}

pub fn parse_settings(v: &Value) -> RoomSettings {
    let flag = |parent: Option<&Value>, key: &str| {
        parent
            .and_then(|p| p.get(key))
            .and_then(|x| x.as_str())
            .map(|s| s.eq_ignore_ascii_case("true"))
    };
    let ext = v.get("extensionData");
    RoomSettings {
        allow_users_to_select_breakout_room: v.get("allowUsersToSelectBreakoutRoom").and_then(|x| x.as_bool()),
        room_duration: v.get("roomDuration").and_then(|x| x.as_str()).map(|s| s.to_string()),
        allow_join_back_to_main_meeting: flag(ext, "allowJoinBackToMainMeeting"),
        allow_move_participants_automatically: flag(ext, "allowMoveParticipantsAutomatically"),
        managers: v.get("managers").and_then(|m| m.as_array()).cloned().unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Op 13 · assignees · GET beta/breakoutRoom/assignees/<C>?<params>
// ---------------------------------------------------------------------------

/// Assignee query parameters. The bundle appends ONLY the non-empty ones,
/// each as `key=<encodeURIComponent(value)>&` (trailing `&` kept).
#[derive(Debug, Clone, Default)]
pub struct AssigneeQuery {
    pub calendar_event_id: Option<String>,
    pub ical_uid: Option<String>,
    pub event_type: Option<String>,
    pub team_id: Option<String>,
    pub max_nest_level: Option<String>,
    pub user_threshold: Option<String>,
}

fn append_param(url: &mut String, key: &str, val: &Option<String>) {
    if let Some(v) = val {
        if !v.is_empty() {
            url.push_str(key);
            url.push('=');
            url.push_str(&urlencoding_lite(v));
            url.push('&');
        }
    }
}

/// Minimal percent-encoding for query values (matches
/// `encodeURIComponent` for the ASCII the ids carry; UTF-8 bytes are
/// encoded per byte).
fn urlencoding_lite(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

pub fn assignees_url(mt_base: &str, coords: &MeetingCoords, q: &AssigneeQuery) -> String {
    let mut url = mt_join(mt_base, &format!("beta/breakoutRoom/assignees/{0}?", encode_coords(coords)));
    append_param(&mut url, "calendarEventId", &q.calendar_event_id);
    append_param(&mut url, "iCalUid", &q.ical_uid);
    append_param(&mut url, "eventType", &q.event_type);
    append_param(&mut url, "teamId", &q.team_id);
    append_param(&mut url, "maxNestLevel", &q.max_nest_level);
    append_param(&mut url, "userThreshold", &q.user_threshold);
    url
}

/// One assignable user (bundle `ne()`): missing fields become explicit
/// nulls, never dropped keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignee {
    pub email: Option<String>,
    pub mri: Option<String>,
    pub membership: Option<String>,
}

pub fn parse_assignees(v: &Value) -> Vec<Assignee> {
    v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|e| Assignee {
            email: e.get("email").and_then(|x| x.as_str()).map(|s| s.to_string()),
            mri: e.get("mri").and_then(|x| x.as_str()).map(|s| s.to_string()),
            membership: e.get("membership").and_then(|x| x.as_str()).map(|s| s.to_string()),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Op 14 · announce · POST beta/breakoutRoom/announce/<mainC> (NO newVersion)
// ---------------------------------------------------------------------------

pub fn announce_url(mt_base: &str, main: &MeetingCoords) -> String {
    mt_join(mt_base, &format!("beta/breakoutRoom/announce/{}", encode_coords(main)))
}

/// Announce body: `{roomThreadIds, message}`.
pub fn announce_body(room_thread_ids: &[String], message: &str) -> Value {
    json!({ "roomThreadIds": room_thread_ids, "message": message })
}

/// Announce failure detail: MT answers 502 with `{FailedRoomThreadIds}`
/// naming the rooms the message never reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceFailure {
    pub failed_room_thread_ids: Vec<String>,
}

pub fn parse_announce_failure(v: &Value) -> Option<AnnounceFailure> {
    v.get("FailedRoomThreadIds")
        .and_then(|ids| ids.as_array())
        .map(|arr| AnnounceFailure {
            failed_room_thread_ids: arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
        })
}

// ---------------------------------------------------------------------------
// Response parsing (rooms)
// ---------------------------------------------------------------------------

/// One room as parsed callers see it (bundle `_()` + `z()` + `$()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakoutRoomInfo {
    pub room_name: String,
    pub coordinates: Option<MeetingCoords>,
    pub participants: Vec<BreakoutParticipant>,
    pub restrict_chat_access: bool,
    pub is_opened: bool,
}

fn coords_from_wire(v: &Value, tenant_id: &str, organizer_id: &str) -> Option<MeetingCoords> {
    let thread = v.get("threadId").and_then(|x| x.as_str())?;
    Some(MeetingCoords {
        tenant_id: v.get("tenantId").and_then(|x| x.as_str()).unwrap_or(tenant_id).to_string(),
        organizer_id: v
            .get("organizerId")
            .and_then(|x| x.as_str())
            .unwrap_or(organizer_id)
            .to_string(),
        thread_id: decode_thread_id(thread),
        message_id: v.get("messageId").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        additional_message_id: v.get("additionalMessageId").and_then(|x| x.as_str()).map(|s| s.to_string()),
    })
}

fn room_from_wire(r: &Value, tenant_id: &str, organizer_id: &str) -> BreakoutRoomInfo {
    let coord_src = r.get("coordinates").unwrap_or(r);
    let participants: Vec<BreakoutParticipant> = r
        .get("participants")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|p| denormalize_participant(&BreakoutParticipant::from_json(p)))
        .collect();
    let restrict = r.get("restrictChatAccess").and_then(|x| x.as_bool()).unwrap_or(false);
    // Bundle `M()`: opened = has participants AND chat not restricted.
    let is_opened = !participants.is_empty() && !restrict;
    BreakoutRoomInfo {
        room_name: r.get("roomName").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        coordinates: coords_from_wire(coord_src, tenant_id, organizer_id),
        participants,
        restrict_chat_access: restrict,
        is_opened,
    }
}

/// Parse a rooms list answer. The bundle reads `breakoutRoomMeetings`
/// (list/add transforms) or `breakoutMeetings` (create transform);
/// both ride `{tenantId, organizerId}` at the top. Either key parses;
/// neither → empty list.
pub fn parse_rooms_list(v: &Value) -> Vec<BreakoutRoomInfo> {
    let tenant = v.get("tenantId").and_then(|x| x.as_str()).unwrap_or("");
    let org = v.get("organizerId").and_then(|x| x.as_str()).unwrap_or("");
    let rooms = v
        .get("breakoutRoomMeetings")
        .or_else(|| v.get("breakoutMeetings"))
        .or_else(|| v.get("breakoutRooms"))
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    rooms.iter().map(|r| room_from_wire(r, tenant, org)).collect()
}

/// Parse one room detail (op 3 add / op 4 get / op 5 update answers).
/// The add answer nests coords under `meetingCoordinates`; the get
/// answer carries no coords (the request's own coords still apply).
pub fn parse_room_detail(v: &Value) -> BreakoutRoomInfo {
    room_from_wire(v, "", "")
}

impl BreakoutRoomInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "room_name": self.room_name,
            "coordinates": self.coordinates.as_ref().map(|c| json!({
                "tenant_id": c.tenant_id,
                "organizer_id": c.organizer_id,
                "thread_id": c.thread_id,
                "message_id": c.message_id,
                "additional_message_id": c.additional_message_id,
            })),
            "participants": self.participants.iter().map(|p| p.to_json()).collect::<Vec<_>>(),
            "restrict_chat_access": self.restrict_chat_access,
            "is_opened": self.is_opened,
        })
    }
}

// ---------------------------------------------------------------------------
// Executors (async; auth = proven MT auth until the STM capture — module docs)
// ---------------------------------------------------------------------------

async fn get_json(client: &TeamsClient, url: &str, what: &str) -> Result<Value> {
    client
        .mt_get(url)
        .await?
        .json()
        .await
        .with_context(|| format!("failed to parse {} answer", what))
}

async fn send_json(
    client: &TeamsClient,
    verb: &'static str,
    url: &str,
    body: Option<&Value>,
    what: &str,
) -> Result<Value> {
    let resp = client.mt_call(verb, url, body).await?;
    if verb == "DELETE" && resp.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    resp.json().await.with_context(|| format!("failed to parse {} answer", what))
}

/// Op 1: list the meeting's rooms with details.
pub async fn list_rooms_data(client: &TeamsClient, main: &MeetingCoords) -> Result<Vec<BreakoutRoomInfo>> {
    let url = list_url(&mt_base_for(client), main);
    Ok(parse_rooms_list(&get_json(client, &url, "breakout list").await?))
}

/// Op 2: create the plan (count 1..=50; preconfiguration entries carry
/// participants + the caller's extra keys verbatim).
pub async fn create_rooms_data(
    client: &TeamsClient,
    main: &MeetingCoords,
    room_count: usize,
    preconfiguration: &[Value],
    settings: Option<&Value>,
    request_version: Option<&str>,
) -> Result<Vec<BreakoutRoomInfo>> {
    let n = room_count.clamp(1, MAX_ROOMS);
    let url = create_url(&mt_base_for(client), main, request_version);
    let body = create_body(n, preconfiguration, settings);
    Ok(parse_rooms_list(&send_json(client, "POST", &url, Some(&body), "breakout create").await?))
}

/// Op 3: add one room.
pub async fn add_room_data(
    client: &TeamsClient,
    main: &MeetingCoords,
    name: &str,
    participants: &[Value],
    request_version: Option<&str>,
) -> Result<BreakoutRoomInfo> {
    if name.trim().is_empty() {
        bail!("breakout add: empty room name");
    }
    let url = add_url(&mt_base_for(client), main, request_version);
    let body = add_body(name.trim(), participants);
    Ok(parse_room_detail(&send_json(client, "POST", &url, Some(&body), "breakout add").await?))
}

/// Op 4: read one room.
pub async fn get_room_data(client: &TeamsClient, room: &MeetingCoords) -> Result<BreakoutRoomInfo> {
    let url = get_url(&mt_base_for(client), room);
    Ok(parse_room_detail(&get_json(client, &url, "breakout get").await?))
}

/// Op 5: update one room (assign/move its roster).
pub async fn update_room_data(
    client: &TeamsClient,
    room: &MeetingCoords,
    name: &str,
    participants: &[Value],
    restrict_chat_access: bool,
    request_version: Option<&str>,
) -> Result<BreakoutRoomInfo> {
    if name.trim().is_empty() {
        bail!("breakout update: empty room name");
    }
    let url = update_url(&mt_base_for(client), room, request_version);
    let body = update_body(name.trim(), participants, restrict_chat_access);
    Ok(parse_room_detail(&send_json(client, "PUT", &url, Some(&body), "breakout update").await?))
}

/// Op 6: delete one room.
pub async fn delete_room_data(
    client: &TeamsClient,
    room: &MeetingCoords,
    request_version: Option<&str>,
) -> Result<()> {
    let url = delete_url(&mt_base_for(client), room, request_version);
    send_json(client, "DELETE", &url, None, "breakout delete").await?;
    Ok(())
}

/// Op 7: delete all rooms (optional rooms-references body, verbatim).
pub async fn delete_all_rooms_data(
    client: &TeamsClient,
    main: &MeetingCoords,
    rooms_references: Option<&Value>,
    request_version: Option<&str>,
) -> Result<()> {
    let url = delete_all_url(&mt_base_for(client), main, request_version);
    send_json(client, "DELETE", &url, rooms_references, "breakout delete-all").await?;
    Ok(())
}

/// Op 8: bulk assign/move across rooms (v2 shape when the total
/// participant count is known).
pub async fn bulk_assign_data(
    client: &TeamsClient,
    main: &MeetingCoords,
    rooms: &[Value],
    total_participant_count: Option<u64>,
    request_version: Option<&str>,
) -> Result<Vec<BreakoutRoomInfo>> {
    let url = bulk_url(&mt_base_for(client), main, request_version, total_participant_count);
    let body = bulk_body(rooms, total_participant_count);
    Ok(parse_rooms_list(&send_json(client, "PUT", &url, Some(&body), "breakout bulk").await?))
}

/// Op 9: open 1..n rooms.
pub async fn open_rooms_data(
    client: &TeamsClient,
    main: &MeetingCoords,
    room_coords: &[Value],
    use_new_timer_flow: bool,
    total_participant_count: Option<u64>,
    request_version: Option<&str>,
) -> Result<()> {
    let url = open_url(&mt_base_for(client), main, request_version);
    let body = open_body(room_coords, use_new_timer_flow, total_participant_count);
    send_json(client, "POST", &url, Some(&body), "breakout open").await?;
    Ok(())
}

/// Op 10: close 1..n rooms.
pub async fn close_rooms_data(
    client: &TeamsClient,
    main: &MeetingCoords,
    room_coords: &[Value],
    total_participant_count: Option<u64>,
    request_version: Option<&str>,
) -> Result<()> {
    let url = close_url(&mt_base_for(client), main, request_version);
    let body = close_body(room_coords, total_participant_count);
    send_json(client, "POST", &url, Some(&body), "breakout close").await?;
    Ok(())
}

/// Op 11: read the meeting's room settings.
pub async fn get_settings_data(client: &TeamsClient, coords: &MeetingCoords) -> Result<RoomSettings> {
    let url = settings_url(&mt_base_for(client), coords, None);
    Ok(parse_settings(&get_json(client, &url, "breakout settings").await?))
}

/// Op 12: write the meeting's room settings.
pub async fn put_settings_data(
    client: &TeamsClient,
    coords: &MeetingCoords,
    settings: &Value,
    request_version: Option<&str>,
) -> Result<RoomSettings> {
    let url = settings_url(&mt_base_for(client), coords, request_version);
    let body = settings_body(settings);
    Ok(parse_settings(&send_json(client, "PUT", &url, Some(&body), "breakout settings").await?))
}

/// Op 13: read the assignable users.
pub async fn assignees_data(
    client: &TeamsClient,
    coords: &MeetingCoords,
    query: &AssigneeQuery,
) -> Result<Vec<Assignee>> {
    let url = assignees_url(&mt_base_for(client), coords, query);
    Ok(parse_assignees(&get_json(client, &url, "breakout assignees").await?))
}

/// Op 14: announce a message into rooms. MT answers 502 with
/// `{FailedRoomThreadIds}` when some rooms never got it; the ids are
/// recovered from the error body and named in the returned error.
pub async fn announce_data(
    client: &TeamsClient,
    main: &MeetingCoords,
    room_thread_ids: &[String],
    message: &str,
) -> Result<()> {
    if message.trim().is_empty() {
        bail!("breakout announce: empty message");
    }
    let url = announce_url(&mt_base_for(client), main);
    let body = announce_body(room_thread_ids, message);
    match client.mt_call("POST", &url, Some(&body)).await {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = format!("{:#}", e);
            // check_response formats failures as `HTTP <n> for <url>:
            // <body>`; the body is the JSON answer when the server sent
            // one. Recover the per-room failures from it.
            if let Some(json_start) = msg.find('{') {
                if let Ok(v) = serde_json::from_str::<Value>(&msg[json_start..]) {
                    if let Some(f) = parse_announce_failure(&v) {
                        bail!(
                            "breakout announce reached no room ({}/{})",
                            f.failed_room_thread_ids.len(),
                            room_thread_ids.len()
                        );
                    }
                }
            }
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// CLI printers (`teams-cli breakout-*`; coords arrive as one JSON arg)
// ---------------------------------------------------------------------------

fn parse_coords_arg(coords_json: &str) -> Result<MeetingCoords> {
    let v: Value =
        serde_json::from_str(coords_json).with_context(|| format!("bad --coords JSON: {}", coords_json))?;
    MeetingCoords::from_json(&v)
}

fn parse_json_arg(raw: &str, what: &str) -> Result<Value> {
    serde_json::from_str(raw).with_context(|| format!("bad {} JSON", what))
}

fn print_rooms(rooms: &[BreakoutRoomInfo]) {
    println!("{}", serde_json::to_string_pretty(&rooms.iter().map(|r| r.to_json()).collect::<Vec<_>>()).unwrap());
}

/// Op 1: print the meeting's rooms.
pub async fn show_breakout_rooms(coords_json: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let main = parse_coords_arg(coords_json)?;
    print_rooms(&list_rooms_data(&client, &main).await?);
    Ok(())
}

/// Op 2: create the plan.
pub async fn show_breakout_create(
    coords_json: &str,
    count: usize,
    preconfig_json: Option<&str>,
    settings_json: Option<&str>,
    version: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let main = parse_coords_arg(coords_json)?;
    let pre: Vec<Value> = match preconfig_json {
        Some(raw) => serde_json::from_str(raw).with_context(|| "bad --preconfig JSON (want an array)".to_string())?,
        None => vec![],
    };
    let settings: Option<Value> = settings_json.map(|raw| parse_json_arg(raw, "--settings")).transpose()?;
    print_rooms(&create_rooms_data(&client, &main, count, &pre, settings.as_ref(), version).await?);
    Ok(())
}

/// Op 3: add one room.
pub async fn show_breakout_add(
    coords_json: &str,
    name: &str,
    participants_json: &str,
    version: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let main = parse_coords_arg(coords_json)?;
    let parts: Vec<Value> =
        serde_json::from_str(participants_json).with_context(|| "bad --participants JSON (want an array)".to_string())?;
    let room = add_room_data(&client, &main, name, &parts, version).await?;
    println!("{}", serde_json::to_string_pretty(&room.to_json()).unwrap());
    Ok(())
}

/// Op 4: read one room.
pub async fn show_breakout_get(coords_json: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let room = parse_coords_arg(coords_json)?;
    let info = get_room_data(&client, &room).await?;
    println!("{}", serde_json::to_string_pretty(&info.to_json()).unwrap());
    Ok(())
}

/// Op 5: update one room (assign/move).
pub async fn show_breakout_update(
    coords_json: &str,
    name: &str,
    participants_json: &str,
    restrict_chat_access: bool,
    version: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let room = parse_coords_arg(coords_json)?;
    let parts: Vec<Value> =
        serde_json::from_str(participants_json).with_context(|| "bad --participants JSON (want an array)".to_string())?;
    let info = update_room_data(&client, &room, name, &parts, restrict_chat_access, version).await?;
    println!("{}", serde_json::to_string_pretty(&info.to_json()).unwrap());
    Ok(())
}

/// Op 6: delete one room.
pub async fn show_breakout_delete(coords_json: &str, version: Option<&str>) -> Result<()> {
    let client = TeamsClient::new().await?;
    let room = parse_coords_arg(coords_json)?;
    delete_room_data(&client, &room, version).await?;
    println!("deleted");
    Ok(())
}

/// Op 7: delete all rooms.
pub async fn show_breakout_delete_all(
    coords_json: &str,
    rooms_refs_json: Option<&str>,
    version: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let main = parse_coords_arg(coords_json)?;
    let refs: Option<Value> = rooms_refs_json.map(|raw| parse_json_arg(raw, "--rooms-refs")).transpose()?;
    delete_all_rooms_data(&client, &main, refs.as_ref(), version).await?;
    println!("deleted all");
    Ok(())
}

/// Op 8: bulk assign/move.
pub async fn show_breakout_assign(
    coords_json: &str,
    rooms_json: &str,
    total_participants: Option<u64>,
    version: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let main = parse_coords_arg(coords_json)?;
    let rooms: Vec<Value> =
        serde_json::from_str(rooms_json).with_context(|| "bad --rooms JSON (want an array)".to_string())?;
    print_rooms(&bulk_assign_data(&client, &main, &rooms, total_participants, version).await?);
    Ok(())
}

/// Op 9: open rooms.
pub async fn show_breakout_open(
    coords_json: &str,
    rooms_json: &str,
    new_timer_flow: bool,
    total_participants: Option<u64>,
    version: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let main = parse_coords_arg(coords_json)?;
    let coords: Vec<Value> =
        serde_json::from_str(rooms_json).with_context(|| "bad --rooms JSON (want an array)".to_string())?;
    open_rooms_data(&client, &main, &coords, new_timer_flow, total_participants, version).await?;
    println!("opened");
    Ok(())
}

/// Op 10: close rooms.
pub async fn show_breakout_close(
    coords_json: &str,
    rooms_json: &str,
    total_participants: Option<u64>,
    version: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let main = parse_coords_arg(coords_json)?;
    let coords: Vec<Value> =
        serde_json::from_str(rooms_json).with_context(|| "bad --rooms JSON (want an array)".to_string())?;
    close_rooms_data(&client, &main, &coords, total_participants, version).await?;
    println!("closed");
    Ok(())
}

/// Op 11: read settings.
pub async fn show_breakout_settings(coords_json: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let coords = parse_coords_arg(coords_json)?;
    let s = get_settings_data(&client, &coords).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "allow_users_to_select_breakout_room": s.allow_users_to_select_breakout_room,
            "room_duration": s.room_duration,
            "allow_join_back_to_main_meeting": s.allow_join_back_to_main_meeting,
            "allow_move_participants_automatically": s.allow_move_participants_automatically,
            "managers": s.managers,
        }))
        .unwrap()
    );
    Ok(())
}

/// Op 12: write settings.
pub async fn show_breakout_settings_set(
    coords_json: &str,
    settings_json: &str,
    version: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let coords = parse_coords_arg(coords_json)?;
    let settings = parse_json_arg(settings_json, "--settings")?;
    let s = put_settings_data(&client, &coords, &settings, version).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "allow_users_to_select_breakout_room": s.allow_users_to_select_breakout_room,
            "room_duration": s.room_duration,
        }))
        .unwrap()
    );
    Ok(())
}

/// Op 13: read assignees.
pub async fn show_breakout_assignees(
    coords_json: &str,
    calendar_event_id: Option<&str>,
    ical_uid: Option<&str>,
    event_type: Option<&str>,
    team_id: Option<&str>,
    max_nest_level: Option<&str>,
    user_threshold: Option<&str>,
) -> Result<()> {
    let client = TeamsClient::new().await?;
    let coords = parse_coords_arg(coords_json)?;
    let q = AssigneeQuery {
        calendar_event_id: calendar_event_id.map(|s| s.to_string()),
        ical_uid: ical_uid.map(|s| s.to_string()),
        event_type: event_type.map(|s| s.to_string()),
        team_id: team_id.map(|s| s.to_string()),
        max_nest_level: max_nest_level.map(|s| s.to_string()),
        user_threshold: user_threshold.map(|s| s.to_string()),
    };
    let list = assignees_data(&client, &coords, &q).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &list.iter().map(|a| json!({ "email": a.email, "mri": a.mri, "membership": a.membership })).collect::<Vec<_>>()
        )
        .unwrap()
    );
    Ok(())
}

/// Op 14: announce into rooms (comma-separated room thread ids).
pub async fn show_breakout_announce(coords_json: &str, room_thread_ids_csv: &str, message: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let main = parse_coords_arg(coords_json)?;
    let ids: Vec<String> =
        room_thread_ids_csv.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if ids.is_empty() {
        anyhow::bail!("breakout announce: no room thread ids");
    }
    announce_data(&client, &main, &ids, message).await?;
    println!("announced to {} room(s)", ids.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MT: &str = "https://teams.microsoft.com/api/mt/part/dogfood/";

    fn main_coords() -> MeetingCoords {
        MeetingCoords {
            tenant_id: "tenant-1".to_string(),
            organizer_id: "org-2".to_string(),
            thread_id: "19:main@thread.v2".to_string(),
            message_id: "111".to_string(),
            additional_message_id: None,
        }
    }

    fn room_coords() -> MeetingCoords {
        MeetingCoords {
            tenant_id: "tenant-1".to_string(),
            organizer_id: "org-2".to_string(),
            thread_id: "19:room@thread.v2".to_string(),
            message_id: "222".to_string(),
            additional_message_id: Some("333".to_string()),
        }
    }

    #[test]
    fn coords_encoding_rewrites_colons_and_joins() {
        assert_eq!(encode_thread_id("19:main@thread.v2"), "19_main@thread.v2");
        assert_eq!(
            encode_coords(&main_coords()),
            "tenant-1/org-2/19_main@thread.v2/111"
        );
        assert_eq!(
            encode_coords(&room_coords()),
            "tenant-1/org-2/19_room@thread.v2/222/333"
        );
    }

    #[test]
    fn thread_decode_roundtrips_scopes() {
        assert_eq!(decode_thread_id("19_main@thread.v2"), "19:main@thread.v2");
        assert_eq!(decode_thread_id("19_gcch_x@y"), "19:gcch:x@y");
        assert_eq!(decode_thread_id("19_dod_x@y"), "19:dod:x@y");
        assert_eq!(decode_thread_id("19_gal_x@y"), "19:gal:x@y");
        assert_eq!(decode_thread_id("19_fr_x@y"), "19:fr:x@y");
        // Non-19 threads pass through untouched.
        assert_eq!(decode_thread_id("other"), "other");
    }

    #[test]
    fn version_suffix_only_when_set() {
        assert_eq!(version_suffix(None), "");
        assert_eq!(version_suffix(Some("")), "");
        assert_eq!(version_suffix(Some("  ")), "");
        assert_eq!(version_suffix(Some("7")), "?newVersion=7");
    }

    #[test]
    fn coords_json_rejects_blank_and_path_breaking() {
        let good = json!({
            "tenant_id": "t", "organizer_id": "o",
            "thread_id": "19:x@thread.v2", "message_id": "1",
        });
        assert!(MeetingCoords::from_json(&good).is_ok());
        for bad in [
            json!({"tenant_id": "", "organizer_id": "o", "thread_id": "t", "message_id": "1"}),
            json!({"tenant_id": "t", "organizer_id": "o", "thread_id": "t", "message_id": ""}),
            json!({"tenant_id": "t", "organizer_id": "a/b", "thread_id": "t", "message_id": "1"}),
            json!({"tenant_id": "t", "organizer_id": "o", "thread_id": "t?x", "message_id": "1"}),
            json!({"organizer_id": "o", "thread_id": "t", "message_id": "1"}),
        ] {
            assert!(MeetingCoords::from_json(&bad).is_err(), "coords {:?}", bad);
        }
    }

    #[test]
    fn op1_list_url_is_bulk_get() {
        assert_eq!(
            list_url(MT, &main_coords()),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/bulk/tenant-1/org-2/19_main@thread.v2/111"
        );
    }

    #[test]
    fn op2_create_url_and_body_verbatim() {
        assert_eq!(
            create_url(MT, &main_coords(), Some("3")),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/tenant-1/org-2/19_main@thread.v2/111?newVersion=3"
        );
        assert!(!create_url(MT, &main_coords(), None).contains('?'));
        let pre = [json!({
            "participants": [{ "mri": "ava@example.com" }],
            "roomName": "Room 1",
        })];
        let body = create_body(2, &pre, None);
        assert_eq!(body["breakoutRoomConfigurations"]["roomCount"], 2);
        assert_eq!(body["preconfiguration"][0]["roomName"], "Room 1");
        // P() fills upn from the email-shaped mri.
        assert_eq!(body["preconfiguration"][0]["participants"][0]["upn"], "ava@example.com");
        assert!(body.get("settings").is_none());
        let with_settings = create_body(1, &[], Some(&json!({ "room_duration": "PT10M" })));
        assert_eq!(with_settings["settings"]["roomDuration"], "PT10M");
        assert_eq!(with_settings["settings"]["extensionData"], json!({}));
    }

    #[test]
    fn op3_add_url_and_body_verbatim() {
        assert_eq!(
            add_url(MT, &main_coords(), None),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/add/tenant-1/org-2/19_main@thread.v2/111"
        );
        assert!(add_url(MT, &main_coords(), Some("2")).ends_with("?newVersion=2"));
        // Participants pass through UN-normalized on add (bundle `Q`).
        let body = add_body("Extra", &[json!({ "mri": "ava@example.com" })]);
        assert_eq!(body, json!({ "name": "Extra", "participants": [{ "mri": "ava@example.com" }] }));
    }

    #[test]
    fn op4_get_url_is_details_get() {
        assert_eq!(
            get_url(MT, &room_coords()),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/details/tenant-1/org-2/19_room@thread.v2/222/333"
        );
    }

    #[test]
    fn op5_update_url_and_body_verbatim() {
        assert_eq!(
            update_url(MT, &room_coords(), None),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/tenant-1/org-2/19_room@thread.v2/222/333"
        );
        let body = update_body("Renamed", &[json!({ "mri": "8:orgid:abc" })], true);
        assert_eq!(body["name"], "Renamed");
        assert_eq!(body["restrictChatAccess"], true);
        assert_eq!(body["participants"][0]["mri"], "8:orgid:abc");
    }

    #[test]
    fn op6_delete_url_verbatim() {
        assert_eq!(
            delete_url(MT, &room_coords(), Some("9")),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/tenant-1/org-2/19_room@thread.v2/222/333?newVersion=9"
        );
    }

    #[test]
    fn op7_delete_all_url_verbatim() {
        assert_eq!(
            delete_all_url(MT, &main_coords(), None),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/rooms/all/tenant-1/org-2/19_main@thread.v2/111"
        );
    }

    #[test]
    fn op8_bulk_url_and_body_both_versions() {
        assert_eq!(
            bulk_url(MT, &main_coords(), None, None),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/bulk/tenant-1/org-2/19_main@thread.v2/111"
        );
        assert!(bulk_url(MT, &main_coords(), Some("4"), Some(12)).contains("bulk/v2/"));
        assert!(bulk_url(MT, &main_coords(), Some("4"), Some(12)).ends_with("?newVersion=4"));
        let rooms = [json!({
            "coordinates": { "tenant_id": "t", "organizer_id": "o",
                             "thread_id": "19:room@thread.v2", "message_id": "2" },
            "roomName": "Room 1",
            "participants": [{ "mri": "ava@example.com" }],
            "restrictChatAccess": false,
        })];
        let v1 = bulk_body(&rooms, None);
        assert!(v1.is_array());
        assert_eq!(v1[0]["settings"], json!({ "restrictChatAccess": false }));
        assert_eq!(v1[0]["participants"][0]["upn"], "ava@example.com");
        // Coordinates ride the wire object with the E() thread rewrite.
        assert_eq!(v1[0]["coordinates"]["threadId"], "19_room@thread.v2");
        assert_eq!(v1[0]["coordinates"]["tenantId"], "t");
        let v2 = bulk_body(&rooms, Some(12));
        assert_eq!(v2["totalParticipantCount"], 12);
        assert_eq!(v2["breakoutRooms"][0]["roomName"], "Room 1");
    }

    #[test]
    fn op9_open_url_and_body_verbatim() {
        assert_eq!(
            open_url(MT, &main_coords(), None),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/open/tenant-1/org-2/19_main@thread.v2/111"
        );
        let rc = json!({ "thread_id": "19:room@thread.v2", "tenant_id": "t",
                               "organizer_id": "o", "message_id": "2" });
        let body = open_body(&[rc], true, Some(5));
        assert_eq!(body["rooms"][0]["coordinates"]["threadId"], "19_room@thread.v2");
        assert_eq!(body["useNewTimerFlow"], true);
        assert_eq!(body["totalParticipantCount"], 5);
        assert!(open_body(&[], false, None).get("totalParticipantCount").is_none());
    }

    #[test]
    fn op10_close_url_and_body_verbatim() {
        assert_eq!(
            close_url(MT, &main_coords(), Some("1")),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/close/tenant-1/org-2/19_main@thread.v2/111?newVersion=1"
        );
        let rc = json!({ "threadId": "19:room@thread.v2", "tenantId": "t",
                               "organizerId": "o", "messageId": "2" });
        let body = close_body(&[rc], None);
        // CamelCase input encodes the same wire object.
        assert_eq!(
            body["rooms"][0]["coordinates"],
            json!({ "tenantId": "t", "organizerId": "o",
                    "threadId": "19_room@thread.v2", "messageId": "2" })
        );
    }

    #[test]
    fn ops11_12_settings_url_and_body_verbatim() {
        // GET carries no version; PUT does.
        assert_eq!(
            settings_url(MT, &main_coords(), None),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/settings/tenant-1/org-2/19_main@thread.v2/111"
        );
        assert!(settings_url(MT, &main_coords(), Some("6")).ends_with("?newVersion=6"));
        let body = settings_body(&json!({
            "allow_users_to_select_breakout_room": true,
            "room_duration": "PT15M",
            "allow_join_back_to_main_meeting": true,
            "allow_move_participants_automatically": false,
        }));
        assert_eq!(body["allowUsersToSelectBreakoutRoom"], true);
        assert_eq!(body["roomDuration"], "PT15M");
        // extensionData values are STRINGS, always present.
        assert_eq!(
            body["extensionData"],
            json!({ "allowJoinBackToMainMeeting": "true", "allowMoveParticipantsAutomatically": "false" })
        );
        // Wire-style keys read too; managers pass through.
        let wire = settings_body(&json!({ "managers": ["8:orgid:m"] }));
        assert_eq!(wire["managers"], json!(["8:orgid:m"]));
        assert_eq!(wire["extensionData"], json!({}));
    }

    #[test]
    fn settings_parse_reads_string_flags_case_insensitive() {
        let s = parse_settings(&json!({
            "allowUsersToSelectBreakoutRoom": true,
            "roomDuration": "PT15M",
            "managers": ["m1"],
            "extensionData": { "allowJoinBackToMainMeeting": "TRUE", "allowMoveParticipantsAutomatically": "False" },
        }));
        assert_eq!(s.allow_users_to_select_breakout_room, Some(true));
        assert_eq!(s.room_duration.as_deref(), Some("PT15M"));
        assert_eq!(s.allow_join_back_to_main_meeting, Some(true));
        assert_eq!(s.allow_move_participants_automatically, Some(false));
        assert_eq!(s.managers.len(), 1);
        let empty = parse_settings(&json!({}));
        assert_eq!(empty.allow_users_to_select_breakout_room, None);
        assert_eq!(empty.allow_join_back_to_main_meeting, None);
    }

    #[test]
    fn op13_assignees_url_appends_only_set_params() {
        let bare = assignees_url(MT, &main_coords(), &AssigneeQuery::default());
        assert!(bare.ends_with("/tenant-1/org-2/19_main@thread.v2/111?"), "{}", bare);
        let full = assignees_url(
            MT,
            &main_coords(),
            &AssigneeQuery {
                calendar_event_id: Some("ev 1".to_string()),
                ical_uid: None,
                event_type: Some("".to_string()),
                team_id: Some("team-9".to_string()),
                max_nest_level: None,
                user_threshold: Some("25".to_string()),
            },
        );
        // Empty/unset params are skipped; values are encoded; trailing & kept.
        assert!(full.contains("calendarEventId=ev%201&"), "{}", full);
        assert!(!full.contains("iCalUid"), "{}", full);
        assert!(!full.contains("eventType"), "{}", full);
        assert!(full.contains("teamId=team-9&"), "{}", full);
        assert!(full.ends_with("userThreshold=25&"), "{}", full);
    }

    #[test]
    fn assignees_parse_nulls_missing_fields() {
        let list = parse_assignees(&json!([
            { "email": "a@example.com", "mri": "8:orgid:a", "membership": "member" },
            { "mri": "8:orgid:b" },
            "junk",
        ]));
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].email.as_deref(), Some("a@example.com"));
        assert_eq!(list[1].email, None);
        assert_eq!(list[2].mri, None);
        assert!(parse_assignees(&json!({})).is_empty());
    }

    #[test]
    fn op14_announce_url_has_no_version_and_body_verbatim() {
        assert_eq!(
            announce_url(MT, &main_coords()),
            "https://teams.microsoft.com/api/mt/part/dogfood/beta/breakoutRoom/announce/tenant-1/org-2/19_main@thread.v2/111"
        );
        let body = announce_body(&["19:r1@thread.v2".to_string()], "Back in 5");
        assert_eq!(body, json!({ "roomThreadIds": ["19:r1@thread.v2"], "message": "Back in 5" }));
        let f = parse_announce_failure(&json!({ "FailedRoomThreadIds": ["19:r1@thread.v2"] })).unwrap();
        assert_eq!(f.failed_room_thread_ids, vec!["19:r1@thread.v2"]);
        assert!(parse_announce_failure(&json!({})).is_none());
    }

    #[test]
    fn participant_normalize_fills_upn_and_guest_object_id() {
        let plain = normalize_participant(&BreakoutParticipant {
            mri: Some("ava@example.com".to_string()),
            upn: None,
            object_id: None,
        });
        assert_eq!(plain.upn.as_deref(), Some("ava@example.com"));
        let guest = normalize_participant(&BreakoutParticipant {
            mri: Some("guest#EXT#@tenant".to_string()),
            upn: None,
            object_id: None,
        });
        assert_eq!(guest.upn.as_deref(), Some("guest#EXT#@tenant"));
        assert!(guest.object_id.as_ref().is_some_and(|id| !id.is_empty()));
        // Explicit object ids survive.
        let kept = normalize_participant(&BreakoutParticipant {
            mri: Some("tel:+1555".to_string()),
            upn: None,
            object_id: Some("oid-1".to_string()),
        });
        assert_eq!(kept.object_id.as_deref(), Some("oid-1"));
        // Non-email MRIs pass through untouched.
        let skype = normalize_participant(&BreakoutParticipant {
            mri: Some("8:orgid:abc".to_string()),
            upn: None,
            object_id: None,
        });
        assert_eq!(skype.upn, None);
    }

    #[test]
    fn participant_denormalize_prefers_upn_as_mri() {
        let d = denormalize_participant(&BreakoutParticipant {
            mri: Some("8:orgid:abc".to_string()),
            upn: Some("ava@example.com".to_string()),
            object_id: None,
        });
        assert_eq!(d.mri.as_deref(), Some("ava@example.com"));
        let kept = denormalize_participant(&BreakoutParticipant {
            mri: Some("8:orgid:abc".to_string()),
            upn: Some("not-an-email".to_string()),
            object_id: None,
        });
        assert_eq!(kept.mri.as_deref(), Some("8:orgid:abc"));
    }

    #[test]
    fn rooms_list_parses_both_answer_keys_and_open_flag() {
        for key in ["breakoutRoomMeetings", "breakoutMeetings", "breakoutRooms"] {
            let v = json!({
                "tenantId": "t", "organizerId": "o",
                key: [{
                    "roomName": "Room 1",
                    "threadId": "19_room@thread.v2",
                    "messageId": "5",
                    "participants": [{ "mri": "8:orgid:a", "upn": "ava@example.com" }],
                    "restrictChatAccess": false,
                }],
            });
            let rooms = parse_rooms_list(&v);
            assert_eq!(rooms.len(), 1, "key {}", key);
            assert_eq!(rooms[0].room_name, "Room 1");
            assert!(rooms[0].is_opened);
            // Response participants are I-normalized (upn becomes mri).
            assert_eq!(rooms[0].participants[0].mri.as_deref(), Some("ava@example.com"));
            let coords = rooms[0].coordinates.as_ref().unwrap();
            assert_eq!(coords.tenant_id, "t");
            assert_eq!(coords.thread_id, "19:room@thread.v2");
        }
        // Empty roster or restricted chat reads closed.
        let closed = parse_rooms_list(&json!({
            "tenantId": "t", "organizerId": "o",
            "breakoutRoomMeetings": [
                { "roomName": "R1", "threadId": "19_a", "messageId": "1", "participants": [] },
                { "roomName": "R2", "threadId": "19_b", "messageId": "2",
                  "participants": [{ "mri": "x" }], "restrictChatAccess": true },
            ],
        }));
        assert!(closed.iter().all(|r| !r.is_opened));
        assert!(parse_rooms_list(&json!({"tenantId": "t"})).is_empty());
    }

    #[test]
    fn room_detail_parses_add_answer_shape() {
        let r = parse_room_detail(&json!({
            "roomName": "Extra",
            "participants": [],
            "restrictChatAccess": false,
            "meetingCoordinates": { "threadId": "19_x", "messageId": "7" },
        }));
        assert_eq!(r.room_name, "Extra");
        assert!(!r.is_opened);
        // Add answers nest coords under meetingCoordinates... but the
        // detail parser reads `coordinates` or the top level, so a bare
        // add answer yields no coords (callers keep the request coords).
        assert!(r.coordinates.is_none());
    }
}
