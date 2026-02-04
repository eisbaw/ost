//! Call recording via recorder bot injection.
//!
//! Implements the recording flow:
//! 1. Add recorder bot as call participant via conversation API
//! 2. Start transcription via call recorder service
//! 3. Start recording via call recorder service

use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::time::Duration;

use super::call_test::extract_call_payload;
use super::signaling::{
    self, trouter_callback, ConversationCallParams, SKYPE_CLIENT_HEADER, TEAMS_PARTITION,
    TEAMS_REGION, TEAMS_RING,
};
use crate::trouter::websocket::TrouterSocket;

/// Microsoft's well-known recorder bot MRI (from captured Teams client traffic).
const RECORDER_BOT_MRI: &str = "28:bdd75849-e0a6-4cce-8fc1-d7c0d4da43e5";

/// FlightProxy recorder service base URL.
/// Captured from USEA region; other regions use different hostnames
/// (e.g. aks-prod-euwe-* for West Europe). TODO: derive from region config.
const RECORDER_SERVICE_BASE: &str =
    "https://api.flightproxy.teams.microsoft.com/api/v2/ep/aks-prod-usea-p08-api.callrecorder.teams.cloud.microsoft:23444";

/// Delay between transcription start and recording start.
/// Teams web client waits ~11s, but we use 2s to race against solo-call teardown.
const TRANSCRIPTION_TO_RECORDING_DELAY_SECS: u64 = 2;

/// Parameters needed for the recording flow.
pub struct RecordingParams<'a> {
    pub caller_mri: &'a str,
    pub participant_id: &'a str,
    pub endpoint_id: &'a str,
    pub chain_id: &'a str,
    pub message_id: &'a str,
    pub thread_id: &'a str,
    pub display_name: &'a str,
    pub trouter_surl: &'a str,
    pub ic3_token: &'a str,
    pub recorder_token: &'a str,
    pub skype_token: &'a str,
    /// The conversation ID from the phase 1/2 response (extracted from conversationController URL).
    pub conversation_id: &'a str,
    /// The addParticipantAndModality URL (derived from conversationController).
    pub add_participant_url: &'a str,
}

/// Base recorder feature flags (shared between bot invitation and recording start).
fn recorder_features() -> serde_json::Value {
    serde_json::json!({
        "enablePPTSharing": true,
        "intermediateLiveCaptions": false,
        "actionItemsEnabled": false,
        "enableEmailAndMeetingLanguageModel": true,
        "ceoSummit": false,
        "useUnmixedAudio": true,
        "enableTranscriptMeetingChaptering": false
    })
}

/// Recording-specific features (base flags + recordingMode).
fn recording_features() -> serde_json::Value {
    let mut map = recorder_features();
    map["recordingMode"] = serde_json::json!("Normal");
    map
}

/// Step 1: Add the recorder bot as a call participant.
pub async fn add_recorder_bot(
    http: &reqwest::Client,
    params: &RecordingParams<'_>,
) -> Result<String> {
    let bot_participant_id = uuid::Uuid::new_v4().to_string();

    // Trouter callback URLs for add-participant success/failure notifications
    let tc = |path: &str| trouter_callback(params.trouter_surl, params.endpoint_id, path);

    // Payload matches real Teams web client capture (reqid 4222).
    // Key differences from addParticipantAndModality: no groupChat/groupContext,
    // has debugContent, uses /addParticipant endpoint.
    let payload = serde_json::json!({
        "disableUnmute": false,
        "participants": {
            "from": {
                "id": params.caller_mri,
                "displayName": params.display_name,
                "endpointId": params.endpoint_id,
                "participantId": params.participant_id,
                "languageId": "en-US"
            },
            "to": [{
                "id": RECORDER_BOT_MRI,
                "participantId": bot_participant_id
            }]
        },
        "participantInvitationData": {
            "botData": {
                "meetingTitle": "",
                "clientInfo": "Teams-R4",
                "callId": params.chain_id,
                "threadId": params.thread_id,
                "recorderFeatures": recorder_features(),
                "mode": "RecordingAndTranscription",
                "iCalUid": null,
                "consumerType": "Teams",
                "spokenLanguage": "en-us",
                "initiatorUserToken": params.recorder_token,
                "exchangeId": null,
                "meetingOrganizer": params.display_name
            }
        },
        "replacementDetails": null,
        "links": {
            "addParticipantSuccess": tc("conversation/addParticipantSuccess/"),
            "addParticipantFailure": tc("conversation/addParticipantFailure/")
        },
        "debugContent": {}
    });

    // Generate a fresh message-id for this request.  The conv server uses
    // x-microsoft-skype-message-id for deduplication; reusing the call-placement
    // message-id causes "cached-response" with an empty body.
    let recorder_message_id = uuid::Uuid::new_v4().to_string();

    tracing::info!(
        "Adding recorder bot -> POST {} (msg_id={})",
        params.add_participant_url,
        recorder_message_id
    );
    tracing::debug!(
        "Recorder bot payload: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    let resp = http
        .post(params.add_participant_url)
        .header("Authorization", format!("Bearer {}", params.ic3_token))
        .header("Content-Type", "application/json")
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("x-microsoft-skype-message-id", &recorder_message_id)
        .header("x-microsoft-skype-client", SKYPE_CLIENT_HEADER)
        .header("ms-teams-partition", TEAMS_PARTITION)
        .header("ms-teams-region", TEAMS_REGION)
        .header("ms-teams-ring", TEAMS_RING)
        .header("x-ms-migration", "True")
        .json(&payload)
        .send()
        .await
        .context("Failed to POST add recorder bot")?;

    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let body = resp.text().await.unwrap_or_default();

    for (k, v) in resp_headers.iter() {
        tracing::debug!(
            "addParticipant resp header: {}: {}",
            k,
            v.to_str().unwrap_or("(binary)")
        );
    }
    if resp_headers
        .get("x-microsoft-skype-cached-response")
        .is_some()
    {
        tracing::warn!("Server returned cached response — message-id may have been reused");
    }

    if !status.is_success() {
        bail!(
            "Add recorder bot failed ({}): {}",
            status,
            &body[..body.len().min(500)]
        );
    }

    tracing::info!("Recorder bot added ({}): {} bytes", status, body.len());
    tracing::debug!(
        "Recorder bot response (first 2000): {}",
        &body[..body.len().min(2000)]
    );
    // Dump full response for protocol analysis
    if let Ok(()) = std::fs::write("/tmp/add_recorder_response.json", &body) {
        tracing::info!("Full add-recorder response saved to /tmp/add_recorder_response.json");
    }
    Ok(body)
}

/// Step 2: Start transcription via the call recorder service.
async fn start_transcription_at(
    http: &reqwest::Client,
    params: &RecordingParams<'_>,
    recorder_base: &str,
) -> Result<()> {
    let url = format!("{}/v2/oncommand/{}", recorder_base, params.conversation_id);

    let payload = serde_json::json!({
        "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "participantMri": params.caller_mri,
        "participantLegId": params.participant_id,
        "action": "start",
        "mode": "transcription",
        "processingModes": ["closedCaptions"],
        "participantSkypeToken": ""
    });

    tracing::info!("Starting transcription -> POST {}", url);

    let resp = http
        .post(&url)
        .header("Authorization", format!("Bearer {}", params.recorder_token))
        .header("x-skypetoken", params.skype_token)
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("Failed to POST start transcription")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        bail!(
            "Start transcription failed ({}): {}",
            status,
            &body[..body.len().min(500)]
        );
    }

    tracing::info!("Transcription started ({})", status);
    Ok(())
}

/// Step 3: Start recording via the call recorder service.
async fn start_recording_at(
    http: &reqwest::Client,
    params: &RecordingParams<'_>,
    recorder_base: &str,
) -> Result<()> {
    let url = format!("{}/v2/oncommand/{}", recorder_base, params.conversation_id);

    let correlation_id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now();
    let date_str = now.format("%Y%m%dT%H%M%S").to_string();
    let file_name = format!("Meeting in \"av-test\"-{}-Meeting Recording", date_str);

    let payload = serde_json::json!({
        "timestamp": now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "participantMri": params.caller_mri,
        "participantLegId": params.participant_id,
        "action": "start",
        "processingModes": ["recording", "realTimeTranscript"],
        "actionParameters": {
            "recordingFeatures": recording_features(),
            "recordingStorageSettings": [{
                "StorageType": "OnedriveForBusiness",
                "StorageLocation": "Recordings",
                "FileName": file_name,
                "GroupId": null
            }],
            "correlationId": correlation_id,
            "meetingTitle": "Meeting in \"av-test\"",
            "spokenLanguage": "en-us",
            "type": "start"
        },
        "participantSkypeToken": ""
    });

    tracing::info!("Starting recording -> POST {}", url);

    let resp = http
        .post(&url)
        .header("Authorization", format!("Bearer {}", params.recorder_token))
        .header("x-skypetoken", params.skype_token)
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("Failed to POST start recording")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        bail!(
            "Start recording failed ({}): {}",
            status,
            &body[..body.len().min(500)]
        );
    }

    tracing::info!("Recording started ({})", status);
    Ok(())
}

/// Run the full recording flow: add bot, start transcription, start recording.
///
/// After adding the recorder bot, waits for the `addParticipantSuccess` Trouter
/// callback which contains the recorder service URL and recording session ID.
/// These are dynamically assigned per call and cannot be hardcoded.
pub async fn start_call_recording(
    http: &reqwest::Client,
    ws: &mut TrouterSocket,
    conversation_controller: &str,
    caller_mri: &str,
    participant_id: &str,
    endpoint_id: &str,
    chain_id: &str,
    message_id: &str,
    thread_id: &str,
    display_name: &str,
    trouter_surl: &str,
    ic3_token: &str,
    recorder_token: &str,
    skype_token: &str,
    add_participant_url_override: Option<&str>,
) -> Result<RecordingSession> {
    // Use the exact addParticipant URL from the epconv response if available,
    // otherwise derive it from conversationController as a fallback.
    let add_url = match add_participant_url_override {
        Some(url) => {
            tracing::info!("Using addParticipant URL from epconv response: {}", url);
            url.to_string()
        }
        None => {
            let derived = derive_add_participant_url_for_bot(conversation_controller);
            tracing::info!(
                "Derived addParticipant URL (no link in response): {}",
                derived
            );
            derived
        }
    };

    let placeholder_conv_id =
        extract_conversation_id(conversation_controller).unwrap_or_else(|| "unknown".to_string());

    let params = RecordingParams {
        caller_mri,
        participant_id,
        endpoint_id,
        chain_id,
        message_id,
        thread_id,
        display_name,
        trouter_surl,
        ic3_token,
        recorder_token,
        skype_token,
        conversation_id: &placeholder_conv_id,
        add_participant_url: &add_url,
    };

    tracing::info!("Starting recording flow (add URL: {})", add_url);

    // Step 1: Add recorder bot — the response body contains the full conversation state
    // including the recorder bot's participant entry with its conversationController URL.
    let add_response = add_recorder_bot(http, &params).await?;

    // Step 2: Extract recorder service URL from the add-participant response body.
    // The response is a conversationUpdate JSON containing participants, one of which
    // is the recorder bot with a conversationController URL pointing at the recorder service.
    let recorder_info = serde_json::from_str::<serde_json::Value>(&add_response)
        .ok()
        .and_then(|v| extract_recorder_from_payload(&v));

    // Fallback: try Trouter callback if the HTTP response didn't contain recorder info
    let recorder_info = match recorder_info {
        Some(info) => Some(info),
        None => {
            tracing::info!("Recorder URL not in HTTP response, waiting on Trouter (30s)...");
            // Build ConversationCallParams for acknowledging callAcceptance frames
            let conv_params = ConversationCallParams {
                ic3_token,
                trouter_surl,
                caller_mri,
                caller_display_name: display_name,
                endpoint_id,
                participant_id,
                thread_id,
                chain_id,
                message_id,
                caller_oid: "", // not needed for acknowledgement
                tenant_id: "",  // not needed for acknowledgement
            };
            wait_for_recorder_info(ws, Duration::from_secs(30), http, &conv_params).await
        }
    };

    let (recorder_base, recorder_conv_id) = match recorder_info {
        Some((base, cid)) => {
            tracing::info!(
                "Recorder service discovered: base={}, conv_id={}",
                base,
                cid
            );
            (base, cid)
        }
        None => {
            bail!(
                "Cannot determine recorder service URL from HTTP response or Trouter callback. \
                 The recorder endpoint and session ID are dynamically assigned per call."
            );
        }
    };

    // Rebuild params with actual recorder conversation ID
    let params = RecordingParams {
        conversation_id: &recorder_conv_id,
        ..params
    };

    // Step 3: Start transcription
    start_transcription_at(http, &params, &recorder_base).await?;

    // Step 4: Wait then start recording
    tracing::info!(
        "Waiting {}s before starting recording...",
        TRANSCRIPTION_TO_RECORDING_DELAY_SECS
    );
    tokio::time::sleep(Duration::from_secs(TRANSCRIPTION_TO_RECORDING_DELAY_SECS)).await;

    start_recording_at(http, &params, &recorder_base).await?;

    tracing::info!("Recording flow complete");
    Ok(RecordingSession {
        recorder_base,
        conversation_id: recorder_conv_id,
        recorder_token: recorder_token.to_string(),
        skype_token: skype_token.to_string(),
        chain_id: chain_id.to_string(),
        caller_mri: caller_mri.to_string(),
        participant_id: participant_id.to_string(),
    })
}

/// Active recording session info needed to stop recording.
pub struct RecordingSession {
    pub recorder_base: String,
    pub conversation_id: String,
    pub recorder_token: String,
    pub skype_token: String,
    pub chain_id: String,
    pub caller_mri: String,
    pub participant_id: String,
}

/// Stop recording and transcription for an active session.
pub async fn stop_call_recording(http: &reqwest::Client, session: &RecordingSession) -> Result<()> {
    let url = format!(
        "{}/v2/oncommand/{}",
        session.recorder_base, session.conversation_id
    );

    // Stop recording first
    let payload = serde_json::json!({
        "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "participantMri": session.caller_mri,
        "participantLegId": session.participant_id,
        "action": "stop",
        "processingModes": ["recording", "realTimeTranscript"],
        "participantSkypeToken": ""
    });

    tracing::info!("Stopping recording -> POST {}", url);
    let resp = http
        .post(&url)
        .header(
            "Authorization",
            format!("Bearer {}", session.recorder_token),
        )
        .header("x-skypetoken", &session.skype_token)
        .header("x-microsoft-skype-chain-id", &session.chain_id)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("Failed to POST stop recording")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    tracing::info!(
        "Stop recording response ({}): {}",
        status,
        &body[..body.len().min(200)]
    );

    // Stop transcription
    let payload = serde_json::json!({
        "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "participantMri": session.caller_mri,
        "participantLegId": session.participant_id,
        "action": "stop",
        "mode": "transcription",
        "processingModes": ["closedCaptions"],
        "participantSkypeToken": ""
    });

    tracing::info!("Stopping transcription -> POST {}", url);
    let resp = http
        .post(&url)
        .header(
            "Authorization",
            format!("Bearer {}", session.recorder_token),
        )
        .header("x-skypetoken", &session.skype_token)
        .header("x-microsoft-skype-chain-id", &session.chain_id)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("Failed to POST stop transcription")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    tracing::info!(
        "Stop transcription response ({}): {}",
        status,
        &body[..body.len().min(200)]
    );

    Ok(())
}

/// Wait for Trouter frames to find the recorder bot's service URL and session ID.
///
/// When the recorder bot successfully joins, the conv service sends an
/// `addParticipantSuccess` callback via Trouter containing a `conversationUpdate`
/// with the recorder bot's participant info. The bot's `conversationController` URL
/// reveals the recorder service endpoint and recording session ID.
async fn wait_for_recorder_info(
    ws: &mut TrouterSocket,
    timeout: Duration,
    http: &reqwest::Client,
    conv_params: &ConversationCallParams<'_>,
) -> Option<(String, String)> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        tokio::select! {
            frame = ws.recv_frame() => {
                match frame {
                    Ok(Some(text)) => {
                        // Respond to heartbeats
                        if text.starts_with("2::") {
                            ws.send_text("2::").await.ok();
                            continue;
                        }

                        // Skip non-data frames
                        if text.starts_with("5:") && !text.contains("callAgent") {
                            continue;
                        }

                        // Log every frame for debugging
                        let trunc: String = text.chars().take(300).collect();
                        tracing::info!("Recording wait frame (len={}): {}", text.len(), trunc);

                        // Decompress and parse every data frame (3::: frames are gzip-compressed,
                        // so checking raw text for strings like RECORDER_BOT_MRI won't work)
                        if let Some(payload) = extract_call_payload(&text) {
                            // Acknowledge any callAcceptance frames (e.g. triggered by recorder bot joining).
                            // Without this, CC kills the call with 430/10065 after ~20s.
                            if let Some(ack_url) = payload
                                .pointer("/callAcceptance/links/acknowledgement")
                                .or_else(|| payload.pointer("/links/acknowledgement"))
                                .and_then(|v| v.as_str())
                            {
                                tracing::info!("Acknowledging callAcceptance from recording wait -> {}", ack_url);
                                match tokio::time::timeout(
                                    Duration::from_secs(5),
                                    signaling::acknowledge_call_acceptance(http, ack_url, conv_params),
                                ).await {
                                    Ok(Ok(())) => {},
                                    Ok(Err(e)) => tracing::warn!("Failed to acknowledge callAcceptance: {:#}", e),
                                    Err(_) => tracing::warn!("Timed out acknowledging callAcceptance (5s)"),
                                }
                            }

                            let payload_str = serde_json::to_string(&payload).unwrap_or_default();

                            // Check if this frame contains the recorder bot or callrecorder URL
                            if payload_str.contains(RECORDER_BOT_MRI)
                                || payload_str.contains("callrecorder")
                                || payload_str.contains("addParticipantSuccess")
                            {
                                tracing::info!("Found recorder-related Trouter frame");
                                // Save full payload for analysis
                                std::fs::write("/tmp/recorder_trouter_payload.json", &payload_str).ok();
                                if let Some(result) = extract_recorder_from_payload(&payload) {
                                    return Some(result);
                                }
                            } else {
                                // Save every payload for offline analysis
                                static FRAME_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                                let n = FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                std::fs::write(format!("/tmp/recorder_frame_{}.json", n), &payload_str).ok();
                                tracing::info!("Parsed frame {} - no recorder match, saved to /tmp/recorder_frame_{}.json", n, n);
                            }
                        }
                    }
                    Ok(None) => {
                        tracing::warn!("WebSocket closed while waiting for recorder info");
                        return None;
                    }
                    Err(e) => {
                        tracing::warn!("WebSocket error while waiting for recorder info: {}", e);
                        return None;
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::warn!("Timeout waiting for recorder info");
                return None;
            }
        }
    }
}

/// Extract recorder service base URL and conversation ID from a Trouter callback payload.
///
/// The payload from addParticipantSuccess/conversationUpdate contains participants.
/// The recorder bot (28:bdd75849-...) has a `conversationController` URL like:
/// `https://api.flightproxy.teams.microsoft.com/api/v2/ep/{recorder-host}:{port}/...`
/// We extract the FlightProxy base + ep hostname:port as the recorder service base,
/// and the conversation ID from the URL path.
fn extract_recorder_from_payload(payload: &serde_json::Value) -> Option<(String, String)> {
    // Search the entire payload JSON string for recorder-related URLs
    let payload_str = serde_json::to_string(payload).unwrap_or_default();

    // Look for callrecorder hostname pattern in the payload
    // Pattern: aks-prod-XXXX-pNN-api.callrecorder.teams.cloud.microsoft:NNNNN
    if let Some(idx) = payload_str.find("callrecorder.teams.cloud.microsoft") {
        // Find the start of the hostname (aks-prod-...)
        let before = &payload_str[..idx];
        if let Some(host_start) = before.rfind("aks-prod-") {
            let after = &payload_str[idx..];
            // Find end of port (next / or " or space)
            if let Some(port_end) = after.find(|c: char| c == '/' || c == '"' || c == ' ') {
                let hostname_port = &payload_str[host_start..idx + port_end];
                let recorder_base = format!(
                    "https://api.flightproxy.teams.microsoft.com/api/v2/ep/{}",
                    hostname_port
                );
                tracing::debug!("Found recorder hostname: {}", hostname_port);

                // Now find conversation ID: look for /v2/oncommand/{uuid} or /conv/{id}
                // The conversation ID for the recorder is typically a UUID in the URL
                // Or search for a UUID pattern near callrecorder
                if let Some(conv_id) = extract_recorder_conv_id(&payload_str) {
                    return Some((recorder_base, conv_id));
                }
            }
        }
    }

    // Fallback: search for any conversationController URL with callrecorder
    // Also try to find the recording session conversation ID from other fields
    tracing::debug!(
        "Could not find callrecorder URL in payload, searching for conv ID patterns..."
    );

    // Try to extract conversation ID from conversationController URL of the recorder bot
    // The bot's conv controller has a different conv ID than ours
    if let Some(participants) = payload.get("participants").and_then(|p| p.as_array()) {
        for p in participants {
            let id = p.get("id").and_then(|i| i.as_str()).unwrap_or("");
            if id == RECORDER_BOT_MRI {
                // Found recorder bot participant - look for its conversation details
                if let Some(cc) = p
                    .pointer("/endpoints/0/conversationController")
                    .or_else(|| p.get("conversationController"))
                    .and_then(|c| c.as_str())
                {
                    tracing::debug!("Recorder bot conversationController: {}", cc);
                    // Extract conv ID and recorder service URL from this
                    let conv_id = extract_conversation_id(cc);
                    if let Some(cid) = conv_id {
                        // Derive recorder base from the conv controller hostname
                        return Some((RECORDER_SERVICE_BASE.to_string(), cid));
                    }
                }
            }
        }
    }

    tracing::warn!("Could not extract recorder info from Trouter payload");
    None
}

/// Try to find a recording session conversation ID in a payload string.
/// Looks for UUID patterns near callrecorder references.
fn extract_recorder_conv_id(payload: &str) -> Option<String> {
    // Look for /v2/oncommand/{uuid} pattern
    if let Some(idx) = payload.find("/v2/oncommand/") {
        let after = &payload[idx + "/v2/oncommand/".len()..];
        let end = after
            .find(|c: char| c == '"' || c == '/' || c == '?' || c == ' ')
            .unwrap_or(after.len());
        let conv_id = &after[..end];
        if !conv_id.is_empty() {
            tracing::debug!("Found recorder conv ID from oncommand URL: {}", conv_id);
            return Some(conv_id.to_string());
        }
    }

    // Look for UUID pattern near callrecorder references
    if let Some(cr_idx) = payload.find("callrecorder") {
        // Search nearby for UUIDs
        let search_start = cr_idx.saturating_sub(200);
        let search_end = (cr_idx + 400).min(payload.len());
        let search_area = &payload[search_start..search_end];

        // Simple UUID finder
        for (i, _) in search_area.match_indices('-') {
            if i >= 8 && i + 28 <= search_area.len() {
                let candidate = &search_area[i - 8..i + 28];
                if candidate.len() == 36
                    && candidate.chars().enumerate().all(|(j, c)| {
                        if j == 8 || j == 13 || j == 18 || j == 23 {
                            c == '-'
                        } else {
                            c.is_ascii_hexdigit()
                        }
                    })
                {
                    tracing::debug!("Found UUID near callrecorder: {}", candidate);
                    return Some(candidate.to_string());
                }
            }
        }
    }

    None
}

/// Extract conversation ID from a conversationController URL.
///
/// The URL looks like:
/// `https://...region.conv.skype.com/conv/{convId}`
/// or `https://...region.conv.skype.com/conv/{convId}/...`
fn extract_conversation_id(url: &str) -> Option<String> {
    // Find "/conv/" and take the next path segment (base64url-encoded UUID)
    let conv_marker = "/conv/";
    let idx = url.find(conv_marker)?;
    let after = &url[idx + conv_marker.len()..];
    // Take until next '/' or '?' or end
    let end = after
        .find(|c: char| c == '/' || c == '?')
        .unwrap_or(after.len());
    let b64_id = &after[..end];
    if b64_id.is_empty() {
        return None;
    }
    // Try to decode base64url to UUID (little-endian bytes)
    decode_base64_uuid(b64_id).or_else(|| Some(b64_id.to_string()))
}

/// Decode a base64url-encoded 16-byte value to a UUID string (little-endian format).
///
/// The conversation controller URL contains a base64url-encoded GUID where the bytes
/// are in Windows/COM little-endian format (first 3 groups byte-swapped).
fn decode_base64_uuid(b64: &str) -> Option<String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let bytes = URL_SAFE_NO_PAD.decode(b64).ok()?;
    if bytes.len() != 16 {
        return None;
    }
    // UUID from little-endian bytes (Data1=LE u32, Data2=LE u16, Data3=LE u16, rest=big-endian)
    Some(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[3], bytes[2], bytes[1], bytes[0],
        bytes[5], bytes[4],
        bytes[7], bytes[6],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

/// Derive the addParticipant URL for bot injection from a conversationController URL.
///
/// Real Teams client uses `/addParticipant` (not `/add`) when injecting bots.
fn derive_add_participant_url_for_bot(conversation_controller: &str) -> String {
    if let Some(idx) = conversation_controller.find('?') {
        let (path, query) = conversation_controller.split_at(idx);
        format!("{}/addParticipant{}", path.trim_end_matches('/'), query)
    } else {
        format!(
            "{}/addParticipant",
            conversation_controller.trim_end_matches('/')
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_conversation_id() {
        let url = "https://amer03-1.conv.skype.com/conv/abc123-def-456";
        assert_eq!(
            extract_conversation_id(url),
            Some("abc123-def-456".to_string())
        );

        let url2 = "https://amer03-1.conv.skype.com/conv/abc123/something";
        assert_eq!(extract_conversation_id(url2), Some("abc123".to_string()));
    }
}
