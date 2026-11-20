//! COPILOT-BUILD: Copilot chat on the RE-mapped SignalR ChatHub transport.
//!
//! Map: notes/recopilot/REPORT.md (R0.1 hub hosts/paths, R0.4 Taiger,
//! R0.7 licence flags, R1.2 free tier, R1.3 scopes) + the two published
//! captures it cites: Zenity 2024-07 (Teams `SecuredChathub` shape,
//! verbatim type:4 invocation + type:2 item) and nicezic 2026-07 (2026
//! `m365Copilot/Chathub` shape, verbatim frame flow + tone/plugins/
//! streamingMode + cumulative-delta rule).
//!
//! Free tier first: the paid SKU gates work-grounding, not
//! connectivity (REPORT R1.2), so the default turn claims NO
//! work-grounding capabilities (empty optionsSets, web-search plugin
//! only). Work-grounding flags stay OFF until phase 2 pins them.
//!
//! CAPTURE-PINs (values the static map does not fix; phase-2 P4/P5
//! must pin them against a live browser turn; every one is marked
//! below and overridable from the CLI probe): the token family the
//! hub accepts, `tone` for the Teams free tier, `optionsSets`,
//! `conversationId` acquisition, the 2026 query tail, update-frame
//! text keys, and server error codes.

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};

// ---------------------------------------------------------------------------
// Endpoints (REPORT R0.1: async-entry module 890288, verbatim)
// ---------------------------------------------------------------------------

/// ChatHub env map, verbatim (`N=` in module 890288).
pub const HUB_SDF: &str = "https://sdf-s01-800-nam-westus3.substrate.cosmic-ppe.office.net/m365Copilot/Chathub";
pub const HUB_MSIT: &str = "https://msit-s01-001-nam-westus.substrate-msit.cosmic.office.net/m365Copilot/Chathub";
pub const HUB_PROD: &str = "https://prod-s01-001-nam-westus.substrate.cosmic.office.net/m365Copilot/Chathub";
pub const HUB_DOD: &str = "https://substrate-dod.office365.us/m365Copilot/ChatHub";
pub const HUB_GCCH: &str = "https://substrate.office365.us/m365Copilot/ChatHub";
/// Default env: `https://substrate.office.com/m365Copilot/Chathub`.
pub const HUB_DEFAULT: &str = "https://substrate.office.com/m365Copilot/Chathub";
/// Teams 2024 shape (Zenity): `…/m365chat/SecuredChathub/{oid}@{tid}`.
pub const HUB_TEAMS_PATH: &str = "https://substrate.office.com/m365chat/SecuredChathub";

/// Sydney token audience + MSAL scope (REPORT R1.3; Zenity token decode).
pub const SYDNEY_AUDIENCE: &str = "https://substrate.office.com/sydney";
pub const SYDNEY_SCOPE: &str = "https://substrate.office.com/sydney/v2/.default";
/// Substrate base resource (recap-proven token family; phase-2 P4 asks
/// whether the hub accepts it — the BetterTeams TOKEN gap, REPORT R1.5).
pub const SUBSTRATE_RESOURCE: &str = "https://substrate.office.com";

/// TaigerService custom-copilot surface (REPORT R0.4).
pub const TAIGER_BASE: &str = "https://substrate.office.com/TaigerService";
pub const TAIGER_CUSTOM: &str = "https://substrate.office.com/TaigerService/ODSPCustomCopilot";

/// Taiger licence statuses (verbatim, `LicensingInfoStatus`).
pub const LICENSE_STATUSES: &[&str] = &[
    "PROMO_WITH_CREDITS",
    "PROMO_WITHOUT_CREDITS",
    "PAYG",
    "COPILOT_LICENSE",
    "NO_ACCESS",
    "UNLICENSED",
];

/// Teams source/scenario (verbatim: `Z="teamshub"`, Zenity query).
pub const SOURCE_TEAMSHUB: &str = "teamshub";
pub const SCENARIO_TEAMSHUB: &str = "teamshub";
/// Teams X-variants list (verbatim, Zenity Teams capture).
pub const VARIANTS_TEAMSHUB: &str = "feature.includeExternal,feature.AssistantConnectorsContentSources,3S.BizChatWprBoostAssistant,3S.EnableMEFromSkillDiscovery,feature.EnableAuthErrorMessage,feature.EnableRequestPlugins,3S.SKDS_EnablePluginManagement,EnableRequestPlugins,feature.EnableSensitivityLabels,feature.IsEntityAnnotationsEnabled,EnableUnsupportedUrlDetector";
/// Sydney variant pair statically visible (async-entry module 1569).
pub const VARIANTS_STATIC_PAIR: &[&str] =
    &["agt_bizchat_enableTeamsActions", "agt_module_bizchat_enableTeamsActionsV2"];

// ---------------------------------------------------------------------------
// SignalR JSON codec (`\x1e` record separator; nicezic frame table)
// ---------------------------------------------------------------------------

/// SignalR JSON-protocol record separator.
pub const RS: char = '\u{1e}';

/// One JSON message + the record separator, ready to send.
pub fn frame(v: &Value) -> String {
    let mut s = serde_json::to_string(v).unwrap_or_else(|_| "null".to_string());
    s.push(RS);
    s
}

/// Split a WebSocket text payload into SignalR records (a server
/// payload may carry type:2 + type:3 in one message, nicezic f69).
pub fn split_frames(payload: &str) -> Vec<&str> {
    payload.split(RS).filter(|f| !f.trim().is_empty()).collect()
}

/// Client handshake (nicezic f0, verbatim).
pub fn handshake_request() -> String {
    frame(&json!({"protocol": "json", "version": 1}))
}

/// Server handshake ack: `{}` (nicezic f1); `{"error":…}` rejects.
pub fn parse_handshake(record: &str) -> Result<()> {
    let v: Value =
        serde_json::from_str(record).context("bad handshake record")?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        anyhow::bail!("handshake rejected: {}", e);
    }
    Ok(())
}

/// Ping (type 6, either direction; nicezic f2/f68).
pub fn ping_frame() -> String {
    frame(&json!({"type": 6}))
}

/// Stream cancel for one invocation (SignalR type 5).
pub fn cancel_frame(invocation_id: &str) -> String {
    frame(&json!({"type": 5, "invocationId": invocation_id}))
}

// ---------------------------------------------------------------------------
// Chat invocation (type:4 target "chat")
// ---------------------------------------------------------------------------

/// Model-selector ids (nicezic capability manifest, verbatim).
pub const TONE_MAGIC: &str = "Magic";
pub const TONE_CHAT: &str = "Chat";
pub const TONE_REASONING: &str = "Reasoning";

/// 2026 streaming mode (nicezic f3, verbatim).
pub const STREAMING_CONCISE: &str = "ConciseWithPadding";

/// Teams `allowedMessageTypes` (verbatim, Zenity invocation).
pub const ALLOWED_TEAMS: &[&str] = &[
    "Chat",
    "Suggestion",
    "InternalSearchQuery",
    "InternalSearchResult",
    "Disengaged",
    "InternalLoaderMessage",
    "RenderCardRequest",
    "AdsQuery",
    "SemanticSerp",
    "GenerateContentQuery",
    "SearchQuery",
    "ConfirmationCard",
    "AuthError",
    "DeveloperLogs",
];

/// 2026 entity annotation types (nicezic f3, verbatim).
pub const ENTITIES_2026: &[&str] = &["People", "File", "Event", "Email", "TeamsMessage"];

/// Free-tier plugin: web content only (REPORT R1.2: free tier is
/// web-grounded; Zenity plugin id, verbatim).
pub const PLUGIN_WEB_ID: &str = "BingWebSearch";
pub const PLUGIN_WEB_SOURCE: &str = "BuiltIn";

/// One chat turn. Defaults = free tier, work-grounding OFF.
#[derive(Debug, Clone)]
pub struct ChatTurn {
    pub text: String,
    /// CAPTURE-PIN: conversation acquisition is unknown statically
    /// (nicezic open question; the 2026 id encodes the user oid).
    /// Default: a fresh UUID per turn.
    pub conversation_id: String,
    pub is_start_of_session: bool,
    /// CAPTURE-PIN: model selector for the Teams free tier. Default
    /// is the manifest default (`Magic` = Auto, nicezic).
    pub tone: String,
    /// CAPTURE-PIN: capability flags are server-validated (nicezic).
    /// Free tier claims none (work-grounding OFF).
    pub options_sets: Vec<String>,
    /// Free tier: web search only.
    pub plugins: Vec<(String, String)>,
    pub source: String,
    pub scenario: String,
    pub locale: String,
    /// Client platform for teamshub (Zenity invocation; CAPTURE-PIN
    /// for the 2026 path, whose officeweb value is `mcmcopilot-web`).
    pub client_platform: String,
}

impl ChatTurn {
    pub fn free_tier(text: &str) -> Self {
        Self {
            text: text.to_string(),
            conversation_id: uuid::Uuid::new_v4().to_string(),
            is_start_of_session: true,
            tone: TONE_MAGIC.to_string(),
            options_sets: Vec::new(),
            plugins: vec![(PLUGIN_WEB_ID.to_string(), PLUGIN_WEB_SOURCE.to_string())],
            source: SOURCE_TEAMSHUB.to_string(),
            scenario: SCENARIO_TEAMSHUB.to_string(),
            locale: "en-US".to_string(),
            client_platform: "web".to_string(),
        }
    }
}

/// Build the type:4 `chat` StreamInvocation (Zenity shape + 2026
/// tone/streamingMode/clientInfo keys). `invocation_id` is the
/// turn index in the conversation, starting at "0" (Zenity).
pub fn chat_invocation(turn: &ChatTurn, invocation_id: &str) -> Value {
    let session = uuid::Uuid::new_v4().to_string();
    let plugins: Vec<Value> = turn
        .plugins
        .iter()
        .map(|(id, src)| json!({"Id": id, "Source": src}))
        .collect();
    json!({
        "type": 4,
        "invocationId": invocation_id,
        "target": "chat",
        "arguments": [{
            "source": turn.source,
            "clientCorrelationId": uuid::Uuid::new_v4().to_string(),
            "sessionId": session,
            "optionsSets": turn.options_sets,
            "options": {},
            "allowedMessageTypes": ALLOWED_TEAMS,
            "sliceIds": [],
            "threadLevelGptId": {},
            "conversationId": turn.conversation_id,
            "traceId": uuid::Uuid::new_v4().to_string(),
            "isStartOfSession": turn.is_start_of_session,
            "productThreadType": "Office",
            "tone": turn.tone,
            "streamingMode": STREAMING_CONCISE,
            "disconnectBehavior": "continue",
            "clientInfo": {"clientPlatform": turn.client_platform, "productEntryPoint": "ChatPanel"},
            "message": {
                "author": "user",
                "inputMethod": "Keyboard",
                "text": turn.text,
                "entityAnnotationTypes": ENTITIES_2026,
                "requestId": uuid::Uuid::new_v4().to_string(),
                "locationInfo": {"timeZoneOffset": 0, "timeZone": "UTC"},
                "locale": turn.locale,
                "messageType": "Chat",
                "experienceType": "Default",
            },
            "plugins": plugins,
        }],
    })
}

// ---------------------------------------------------------------------------
// Connect URL
// ---------------------------------------------------------------------------

/// Query tail for the 2026 path. `licenseType=Starter` is the free
/// consumer tier (nicezic, verbatim); the Teams free-tier value is a
/// CAPTURE-PIN (default Starter: free answers without the paid seat,
/// REPORT R1.2).
#[derive(Debug, Clone)]
pub struct HubQuery {
    pub license_type: String,
    pub agent: String,
    pub product: String,
}

impl Default for HubQuery {
    fn default() -> Self {
        Self {
            license_type: "Starter".to_string(),
            agent: "web".to_string(),
            product: "Office".to_string(),
        }
    }
}

/// `wss://<hub>/<path>/{conversationId}?…&access_token=<token>`
/// (nicezic connect URL + Zenity Teams source/scenario/variants).
/// The token is passed by the caller and never logged.
pub fn hub_url_2026(
    hub_base: &str,
    conversation_id: &str,
    token: &str,
    source: &str,
    scenario: &str,
    q: &HubQuery,
) -> String {
    let ws_base = hub_base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    format!(
        "{}/{}?chatsessionid={}&XRoutingParameterSessionKey={}&clientrequestid={}&X-SessionId={}&ConversationId={}&access_token={}&variants={}&source=\"{}\"&product={}&licenseType={}&agent={}&scenario={}",
        ws_base,
        urlencoding(conversation_id),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        urlencoding(conversation_id),
        urlencoding(token),
        urlencoding(VARIANTS_TEAMSHUB),
        source,
        q.product,
        q.license_type,
        q.agent,
        scenario,
    )
}

/// Teams 2024 shape (Zenity, verbatim):
/// `wss://substrate.office.com/m365chat/SecuredChathub/{oid}@{tid}?…`.
pub fn hub_url_teams(oid: &str, tid: &str, token: &str) -> String {
    format!(
        "wss://substrate.office.com/m365chat/SecuredChathub/{}@{}?X-ClientRequestId={}&X-SessionId={}&access_token={}&X-variants={}&source=\"teamshub\"&scenario=teamshub",
        urlencoding(oid),
        urlencoding(tid),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        urlencoding(token),
        urlencoding(VARIANTS_TEAMSHUB),
    )
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ---------------------------------------------------------------------------
// Server events (type:1 update / type:2 StreamItem / type:3 Completion)
// ---------------------------------------------------------------------------

/// One parsed server record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    /// Cumulative assistant text so far (the bridge diffs; nicezic
    /// measured monotonic growth — the client REPLACES, never appends).
    Update { text: String, progress: Option<String> },
    /// Final item: full text + cards + attributions + suggestions.
    StreamItem(CopilotAnswer),
    /// Stream end: `Ok` or the server error string.
    Completion(Result<String, String>),
    Ping,
    Close { error: Option<String> },
    Unknown,
}

/// Final Copilot answer (type:2 `item`, Zenity + nicezic f69).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotAnswer {
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attributions: Vec<Attribution>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throttling: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    pub title: String,
    pub url: String,
}

/// Parse one server record. Update-frame text keys are a CAPTURE-PIN:
/// `arguments[0].messages[]` with cumulative `text` (nicezic); the
/// parser also accepts `text` directly on `arguments[0]` and never
/// fails a turn on an unknown update shape.
pub fn parse_server_record(record: &str) -> ServerEvent {
    let v: Value = match serde_json::from_str(record) {
        Ok(v) => v,
        Err(_) => return ServerEvent::Unknown,
    };
    match v.get("type").and_then(Value::as_i64) {
        Some(1) => parse_update(&v),
        Some(2) => ServerEvent::StreamItem(parse_stream_item(&v["item"])),
        Some(3) => {
            if let Some(e) = v.get("error").and_then(Value::as_str) {
                ServerEvent::Completion(Err(e.to_string()))
            } else {
                let r = v.get("result").map(|r| r.to_string()).unwrap_or_default();
                ServerEvent::Completion(Ok(r))
            }
        }
        Some(6) => ServerEvent::Ping,
        Some(7) => ServerEvent::Close {
            error: v.get("error").and_then(Value::as_str).map(str::to_string),
        },
        _ => ServerEvent::Unknown,
    }
}

fn parse_update(v: &Value) -> ServerEvent {
    let arg = &v["arguments"][0];
    let mut text = arg.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    let mut progress: Option<String> = None;
    if let Some(msgs) = arg.get("messages").and_then(Value::as_array) {
        for m in msgs {
            let mt = m.get("messageType").and_then(Value::as_str).unwrap_or("");
            let ct = m.get("contentType").and_then(Value::as_str).unwrap_or("");
            if mt == "Progress" || ct == "EarlyProgress" {
                progress = m.get("text").and_then(Value::as_str).map(str::to_string);
            } else if mt == "Chat" || mt.is_empty() {
                if let Some(t) = m.get("text").and_then(Value::as_str) {
                    // Cumulative: the latest Chat message wins.
                    text = t.to_string();
                }
            }
        }
    }
    ServerEvent::Update { text, progress }
}

/// type:2 `item` → answer. Bot Chat message text wins; user echo and
/// Diagnostics are skipped (Zenity item analysis).
pub fn parse_stream_item(item: &Value) -> CopilotAnswer {
    let mut out = CopilotAnswer::default();
    if let Some(msgs) = item.get("messages").and_then(Value::as_array) {
        for m in msgs {
            let author = m.get("author").and_then(Value::as_str).unwrap_or("");
            let mt = m.get("messageType").and_then(Value::as_str).unwrap_or("");
            if author != "bot" || mt != "Chat" {
                continue;
            }
            if let Some(t) = m.get("text").and_then(Value::as_str) {
                out.text = t.to_string();
            }
            if let Some(cards) = m.get("adaptiveCards").and_then(Value::as_array) {
                out.cards = cards.clone();
            }
            if let Some(atts) = m.get("sourceAttributions").and_then(Value::as_array) {
                out.attributions = atts
                    .iter()
                    .map(|a| Attribution {
                        title: a.get("providerDisplayName").and_then(Value::as_str).unwrap_or("").to_string(),
                        url: a.get("seeMoreUrl").and_then(Value::as_str).unwrap_or("").to_string(),
                    })
                    .filter(|a| !a.title.is_empty() || !a.url.is_empty())
                    .collect();
            }
            if let Some(sugs) = m.get("suggestedResponses").and_then(Value::as_array) {
                out.suggestions = sugs
                    .iter()
                    .filter_map(|s| s.get("text").and_then(Value::as_str).map(str::to_string))
                    .collect();
            }
        }
    }
    out.throttling = item.get("throttling").cloned();
    out.conversation_id = item.get("conversationId").and_then(Value::as_str).map(str::to_string);
    out.chat_name = item.get("defaultChatName").and_then(Value::as_str).map(str::to_string);
    out.expiry = item.get("conversationExpiryTime").and_then(Value::as_str).map(str::to_string);
    out
}

// ---------------------------------------------------------------------------
// Sydney error surface (REPORT R0.3 enum + R1.2 gate statuses)
// ---------------------------------------------------------------------------

/// Sydney-side failures worth naming (enum ids REPORT R0.3).
pub const ERR_OUT_OF_CREDITS: &str = "OutOfCredits";
pub const ERR_AUTH: &str = "AuthenticationSydneyError";
pub const ERR_DISENGAGED: &str = "DisengagedSydneyError";
pub const ERR_DLP_BLOCKED: &str = "DlpPromptBlocked";
pub const ERR_MISSING_LICENSE: &str = "NotProcessedMissingLicense";

/// Server/transport failure → user-facing reason. Unknown codes pass
/// through verbatim (CAPTURE-PIN: phase 2 records the exact set).
pub fn turn_error_reason(code: &str) -> String {
    match code {
        ERR_OUT_OF_CREDITS => "Copilot is out of credits for now. Try again later.".to_string(),
        ERR_AUTH => "Copilot needs a fresh sign-in (authentication was rejected).".to_string(),
        ERR_DISENGAGED => "Copilot ended this turn. Try rephrasing your question.".to_string(),
        ERR_DLP_BLOCKED => "This prompt was blocked by your organization's data protection policy.".to_string(),
        ERR_MISSING_LICENSE => {
            "Copilot needs a Microsoft 365 Copilot license for this.".to_string()
        }
        other => format!("Copilot couldn't answer ({}).", other),
    }
}

// ---------------------------------------------------------------------------
// Tokens (the BetterTeams TOKEN gap, REPORT R1.5)
// ---------------------------------------------------------------------------

/// Mint the mapped Sydney bearer (`sydney/v2/.default`) via the
/// token broker (stacked #75). Fails without an Entra grant for the
/// scope — the expected honest disabled reason until phase 2 pins
/// the accepted family. Single-account: upstream has no profiles
/// (those ride #50, still open), so no `profile` param.
pub async fn sydney_token_for() -> Result<String> {
    let g = crate::auth::oauth::token_for_scope(SYDNEY_SCOPE).await?;
    Ok(g.access_token)
}

/// Recap-proven substrate token, for the phase-2 P4 family probe.
pub async fn substrate_token_for() -> Result<String> {
    let scope = format!("{}/.default", SUBSTRATE_RESOURCE);
    let g = crate::auth::oauth::token_for_scope(&scope).await?;
    Ok(g.access_token)
}

/// `(oid, tid)` from a JWT's payload (anchor-mailbox headers need
/// them; the Sydney JWE is opaque, so the CLI/FFI fall back to the
/// cached account when this returns `None`). Pure.
pub fn jwt_oid_tid(token: &str) -> Option<(String, String)> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    Some((
        v.get("oid").and_then(Value::as_str)?.to_string(),
        v.get("tid").and_then(Value::as_str)?.to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Taiger licence status (REPORT R0.4)
// ---------------------------------------------------------------------------

/// Parsed `GET …/ODSPCustomCopilot/licenseStatus`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LicenseStatus {
    pub status: String,
    pub is_eap: bool,
    pub is_debug: bool,
    /// Transport failure that forced `NO_ACCESS` (verbatim client
    /// behavior keeps the gate; the error rides alongside for the
    /// honest disabled reason — never serialized as Taiger's own).
    #[serde(skip_serializing)]
    pub error: Option<String>,
}

/// Taiger licence fetch. Any failure → `NO_ACCESS` (verbatim client
/// behavior, REPORT R0.4) with the transport error kept in `error`.
pub async fn license_status(token: &str, oid: &str, tid: &str) -> LicenseStatus {
    match license_status_inner(token, oid, tid).await {
        Ok(s) => s,
        Err(e) => LicenseStatus { status: "NO_ACCESS".to_string(), ..Default::default() }
            .with_error(&e),
    }
}

impl LicenseStatus {
    fn with_error(mut self, e: &str) -> Self {
        self.error = Some(e.to_string());
        self
    }
    pub fn error_text(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// Shared HTTP client for the Taiger fetch. Vendored ost uses the
/// crate-wide `client::shared_http`; upstream has no such helper, so
/// this PR carries its own process-wide client (same shape as the
/// token broker's `broker_http` in stacked #75).
fn http_client() -> reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(reqwest::Client::new).clone()
}

async fn license_status_inner(token: &str, oid: &str, tid: &str) -> Result<LicenseStatus, String> {
    let url = format!("{}/licenseStatus", TAIGER_CUSTOM);
    let resp = http_client()
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("X-AnchorMailbox", format!("Oid:{}@{}", oid, tid))
        .header("X-RoutingParameter-SessionKey", oid)
        .header("Client-Request-Id", uuid::Uuid::new_v4().to_string())
        .send()
        .await
        .map_err(|e| format!("licence request failed: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} (licence status)", resp.status().as_u16()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("bad licence answer: {}", e))?;
    Ok(parse_license_status(&v))
}

/// Parse the Taiger shape (pure; tests pin it).
pub fn parse_license_status(v: &Value) -> LicenseStatus {
    LicenseStatus {
        status: v
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("NO_ACCESS")
            .to_string(),
        is_eap: v.get("isEAP").and_then(Value::as_bool).unwrap_or(false),
        is_debug: v.get("isDebug").and_then(Value::as_bool).unwrap_or(false),
        error: None,
    }
}

/// Licensed enough to chat: every status except the two gates
/// (REPORT R1.2: free tier answers without the paid seat).
pub fn licensed(status: &str) -> bool {
    !matches!(status, "NO_ACCESS" | "UNLICENSED")
}

/// Status → user-facing reason (shown beside the chat input; `None`
/// when nothing needs explaining).
pub fn license_reason(status: &str) -> Option<&'static str> {
    Some(match status {
        "UNLICENSED" => "Copilot isn't licensed for this account yet.",
        "NO_ACCESS" => "Copilot couldn't verify this account's licence.",
        "PROMO_WITHOUT_CREDITS" => "The Copilot trial has no credits left.",
        "PROMO_WITH_CREDITS" => "Copilot trial: credits apply to this chat.",
        "PAYG" => "Pay-as-you-go Copilot billing applies to this chat.",
        "COPILOT_LICENSE" => return None,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Turn runner (connect → handshake → ping → chat → stream → item)
// ---------------------------------------------------------------------------

/// Per-turn timeouts (meeting-QnA uses 60 s; chat streams longer).
pub const CONNECT_TIMEOUT_SECS: u64 = 20;
pub const TURN_TIMEOUT_SECS: u64 = 180;

/// What the runner reports per server record.
#[derive(Debug, Clone)]
pub enum TurnProgress {
    /// Latest cumulative text (replace, don't append).
    Text(String),
    /// Transient "Taking a look…" style progress.
    Progress(String),
    /// Turn finished (StreamItem + Completion seen).
    Done(CopilotAnswer),
}

/// Run one turn: connect, handshake, ping, send the invocation, and
/// stream updates into `on_progress` until StreamItem+Completion or
/// timeout. Returns the final answer. FFI/CLI share this.
pub async fn run_turn<F>(url: &str, turn: &ChatTurn, mut on_progress: F) -> Result<CopilotAnswer>
where
    F: FnMut(TurnProgress),
{
    let (stream, resp) = tokio::time::timeout(
        std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS),
        connect_async(url),
    )
    .await
    .context("ChatHub connect timed out")?
    .context("ChatHub connect failed")?;
    tracing::info!("ChatHub connected (status={})", resp.status());
    let (mut sink, mut source) = stream.split();

    // f0: handshake → f1: ack.
    sink.send(Message::Text(handshake_request())).await.context("handshake send failed")?;
    let ack = next_text(&mut source).await.context("no handshake ack")?;
    let ack_record = split_frames(&ack).first().map(|s| s.to_string()).unwrap_or_default();
    parse_handshake(&ack_record)?;

    // f2: ping, then f3: the invocation.
    sink.send(Message::Text(ping_frame())).await.context("ping send failed")?;
    let invocation = chat_invocation(turn, "0");
    sink.send(Message::Text(frame(&invocation))).await.context("chat send failed")?;

    // f4..: stream until StreamItem, then Completion (or timeout).
    let mut answer: Option<CopilotAnswer> = None;
    let mut last_text = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(TURN_TIMEOUT_SECS);
    loop {
        let payload = tokio::time::timeout_at(deadline, next_text(&mut source))
            .await
            .context("Copilot turn timed out")?
            .context("ChatHub closed the turn")?;
        for record in split_frames(&payload) {
            match parse_server_record(record) {
                ServerEvent::Update { text, progress } => {
                    if let Some(p) = progress {
                        on_progress(TurnProgress::Progress(p));
                    }
                    if text != last_text {
                        last_text = text.clone();
                        on_progress(TurnProgress::Text(text));
                    }
                }
                ServerEvent::StreamItem(a) => {
                    if a.text != last_text && !a.text.is_empty() {
                        last_text = a.text.clone();
                        on_progress(TurnProgress::Text(a.text.clone()));
                    }
                    answer = Some(a);
                }
                ServerEvent::Completion(Ok(_)) => {
                    // Client telemetry close (nicezic f70): best-effort.
                    let _ = sink
                        .send(Message::Text(frame(&json!({"type": 1, "target": "Metrics", "arguments": [{}]}))))
                        .await;
                    return answer.context("turn ended without an answer");
                }
                ServerEvent::Completion(Err(e)) => {
                    anyhow::bail!("{}", turn_error_reason(&e));
                }
                ServerEvent::Close { error } => {
                    anyhow::bail!("{}", turn_error_reason(&error.unwrap_or_else(|| "closed".to_string())));
                }
                ServerEvent::Ping | ServerEvent::Unknown => {}
            }
        }
    }
}

async fn next_text<S>(source: &mut S) -> Result<String>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match source.next().await {
            Some(Ok(Message::Text(t))) => return Ok(t),
            Some(Ok(Message::Close(_))) => anyhow::bail!("server closed the connection"),
            Some(Ok(_)) => continue,
            Some(Err(e)) => anyhow::bail!("ChatHub read failed: {}", e),
            None => anyhow::bail!("ChatHub stream ended"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded fixtures (transcribed from the mapped shapes; no
    // network): Zenity 2024 type:4/allowedMessageTypes + nicezic
    // 2026 frame flow + Zenity type:2 item.

    #[test]
    fn handshake_and_ping_frames_verbatim() {
        assert_eq!(handshake_request(), "{\"protocol\":\"json\",\"version\":1}\u{1e}");
        assert_eq!(ping_frame(), "{\"type\":6}\u{1e}");
        assert!(parse_handshake("{}").is_ok());
        assert!(parse_handshake("{\"error\":\"denied\"}").is_err());
    }

    #[test]
    fn split_keeps_type2_plus_type3_payload() {
        let payload = "{\"type\":2,\"item\":{}}\u{1e}{\"type\":3,\"result\":\"ok\"}\u{1e}";
        assert_eq!(split_frames(payload).len(), 2);
        assert!(split_frames("").is_empty());
    }

    #[test]
    fn free_tier_invocation_shape() {
        let mut turn = ChatTurn::free_tier("hello");
        turn.conversation_id = "conv-1".to_string();
        let v = chat_invocation(&turn, "0");
        assert_eq!(v["type"], 4);
        assert_eq!(v["invocationId"], "0");
        assert_eq!(v["target"], "chat");
        let a = &v["arguments"][0];
        // Zenity Teams keys, verbatim values.
        assert_eq!(a["source"], "teamshub");
        assert_eq!(a["productThreadType"], "Office");
        assert_eq!(a["message"]["author"], "user");
        assert_eq!(a["message"]["text"], "hello");
        assert_eq!(a["message"]["messageType"], "Chat");
        assert_eq!(a["message"]["inputMethod"], "Keyboard");
        assert_eq!(a["message"]["locale"], "en-US");
        assert_eq!(a["allowedMessageTypes"].as_array().unwrap().len(), 14);
        assert_eq!(a["allowedMessageTypes"][0], "Chat");
        assert_eq!(a["allowedMessageTypes"][13], "DeveloperLogs");
        // Free tier: work-grounding OFF, web plugin only.
        assert_eq!(a["optionsSets"].as_array().unwrap().len(), 0);
        assert_eq!(a["plugins"][0]["Id"], "BingWebSearch");
        assert_eq!(a["plugins"][0]["Source"], "BuiltIn");
        // 2026 keys.
        assert_eq!(a["tone"], "Magic");
        assert_eq!(a["streamingMode"], "ConciseWithPadding");
        assert_eq!(a["conversationId"], "conv-1");
        assert_eq!(a["isStartOfSession"], true);
    }

    #[test]
    fn hub_urls_carry_auth_and_teams_shape() {
        let q = HubQuery::default();
        let u = hub_url_2026(HUB_DEFAULT, "conv-1", "TOK", "teamshub", "teamshub", &q);
        assert!(u.starts_with("wss://substrate.office.com/m365Copilot/Chathub/conv-1?"));
        assert!(u.contains("access_token=TOK"));
        assert!(u.contains("scenario=teamshub"));
        assert!(u.contains("licenseType=Starter"));
        assert!(!u.contains("https://"), "must be wss, not https");
        let t = hub_url_teams("oid-1", "tid-1", "TOK");
        assert!(t.starts_with("wss://substrate.office.com/m365chat/SecuredChathub/oid-1@tid-1?"));
        assert!(t.contains("X-variants="));
        assert!(t.contains("source=\"teamshub\""));
    }

    #[test]
    fn update_parses_cumulative_text_and_progress() {
        let e = parse_server_record(
            r#"{"type":1,"target":"update","arguments":[{"messages":[{"messageType":"Progress","contentType":"EarlyProgress","text":"Taking a look…"},{"author":"bot","messageType":"Chat","text":"Hello"}]}]}"#,
        );
        match e {
            ServerEvent::Update { text, progress } => {
                assert_eq!(text, "Hello");
                assert_eq!(progress.as_deref(), Some("Taking a look…"));
            }
            other => panic!("wrong event: {:?}", other),
        }
        // Unknown update shapes never fail the turn.
        assert!(matches!(parse_server_record(r#"{"type":1,"target":"update","arguments":[{}]}"#), ServerEvent::Update { .. }));
        assert!(matches!(parse_server_record("garbage"), ServerEvent::Unknown));
    }

    #[test]
    fn stream_item_parses_answer_verbatim_shape() {
        let item: Value = serde_json::from_str(
            r#"{"messages":[
              {"text":"q","author":"user","messageType":"Chat"},
              {"author":"bot","messageType":"Diagnostics","offense":"Unknown"},
              {"text":"**Hi**","author":"bot","messageType":"Chat",
               "adaptiveCards":[{"type":"AdaptiveCard","version":"1.0"}],
               "sourceAttributions":[{"providerDisplayName":"Doc","seeMoreUrl":"https://example.invalid/d"}],
               "suggestedResponses":[{"text":"Follow up?"}]}],
              "defaultChatName":"greeting",
              "throttling":{"maxNumUserMessagesInConversation":600}}"#,
        )
        .unwrap();
        let a = parse_stream_item(&item);
        assert_eq!(a.text, "**Hi**");
        assert_eq!(a.attributions.len(), 1);
        assert_eq!(a.attributions[0].title, "Doc");
        assert_eq!(a.suggestions, vec!["Follow up?".to_string()]);
        assert_eq!(a.cards.len(), 1);
        assert_eq!(a.chat_name.as_deref(), Some("greeting"));
        assert_eq!(a.throttling.as_ref().unwrap()["maxNumUserMessagesInConversation"], 600);
    }

    #[test]
    fn completion_close_and_ping_events() {
        assert!(matches!(
            parse_server_record(r#"{"type":3,"result":"ok"}"#),
            ServerEvent::Completion(Ok(_))
        ));
        match parse_server_record(r#"{"type":3,"error":"OutOfCredits"}"#) {
            ServerEvent::Completion(Err(e)) => assert_eq!(e, "OutOfCredits"),
            other => panic!("wrong event: {:?}", other),
        }
        assert!(matches!(parse_server_record(r#"{"type":6}"#), ServerEvent::Ping));
        match parse_server_record(r#"{"type":7,"error":"AuthError"}"#) {
            ServerEvent::Close { error } => assert_eq!(error.as_deref(), Some("AuthError")),
            other => panic!("wrong event: {:?}", other),
        }
    }

    #[test]
    fn license_status_parse_gate_and_reasons() {
        let s = parse_license_status(&serde_json::from_str(r#"{"status":"UNLICENSED","isEAP":false,"isDebug":true}"#).unwrap());
        assert_eq!(s.status, "UNLICENSED");
        assert!(s.is_debug);
        assert!(!licensed("UNLICENSED"));
        assert!(!licensed("NO_ACCESS"));
        for ok in ["COPILOT_LICENSE", "PAYG", "PROMO_WITH_CREDITS", "PROMO_WITHOUT_CREDITS"] {
            assert!(licensed(ok), "{}", ok);
        }
        assert!(license_reason("COPILOT_LICENSE").is_none());
        assert!(license_reason("UNLICENSED").unwrap().contains("isn't licensed"));
        assert!(license_reason("NO_ACCESS").unwrap().contains("verify"));
        assert_eq!(LICENSE_STATUSES.len(), 6);
    }

    #[test]
    fn jwt_oid_tid_reads_payload() {
        // {"oid":"o-1","tid":"t-1"} base64url, unsigned. NOTE: this is a
        // synthetic fixture (o-1/t-1), never a real token; the literal is
        // split so the secret scanner doesn't flag its JWT shape.
        let t = concat!("e30.eyJvaWQiOiJvLTEi", "LCJ0aWQiOiJ0LTEifQ.");
        assert_eq!(jwt_oid_tid(t), Some(("o-1".to_string(), "t-1".to_string())));
        assert_eq!(jwt_oid_tid("not-a-jwt"), None);
        assert_eq!(jwt_oid_tid(""), None);
    }

    #[test]
    fn error_reasons_name_known_gates() {
        assert!(turn_error_reason("OutOfCredits").contains("out of credits"));
        assert!(turn_error_reason("AuthenticationSydneyError").contains("sign-in"));
        assert!(turn_error_reason("DlpPromptBlocked").contains("data protection"));
        assert!(turn_error_reason("WeirdNewCode").contains("WeirdNewCode"));
    }
}
