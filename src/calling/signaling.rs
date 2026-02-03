//! Call signaling HTTP operations — accept, send media answer, end call.
//!
//! Includes the two-phase conversation API for call placement via epconv.

use anyhow::{Context, Result};

use super::CallNotification;
use uuid;

// Common headers for Teams calling API requests.
// TODO: region headers are hardcoded to AMER — derive from config for other tenants.
pub(crate) const SKYPE_CLIENT_HEADER: &str =
    "SkypeSpaces/1415/teams-cli/TsCallingVersion=2025.49.01.15";
pub(crate) const TEAMS_PARTITION: &str = "amer03";
pub(crate) const TEAMS_REGION: &str = "amer";
pub(crate) const TEAMS_RING: &str = "general";

/// Response from phase 1 (create conversation).
#[derive(Debug)]
pub struct ConversationCreated {
    /// URL to POST phase 2 to (the conversationController).
    pub conversation_controller: String,
    /// The exact addParticipant URL from the response links (if present).
    pub add_participant_url: Option<String>,
    /// Full response body for debugging.
    pub response_body: String,
}

/// Response from phase 2 (join with SDP).
#[derive(Debug)]
pub struct ConversationJoined {
    /// CC active URL from response headers (x-microsoft-skype-proxy-cluster-context).
    pub cc_active_url: Option<String>,
    /// Full response body for debugging.
    pub response_body: String,
}

/// Parameters for the two-phase conversation call placement.
pub struct ConversationCallParams<'a> {
    pub ic3_token: &'a str,
    pub trouter_surl: &'a str,
    pub caller_mri: &'a str,
    pub caller_display_name: &'a str,
    pub endpoint_id: &'a str,
    pub participant_id: &'a str,
    pub thread_id: &'a str,
    pub chain_id: &'a str,
    pub message_id: &'a str,
    /// OID (object ID) extracted from caller MRI, e.g. the GUID part of "8:orgid:{guid}".
    pub caller_oid: &'a str,
    pub tenant_id: &'a str,
}

/// Build a Trouter callback URL for a specific path.
///
/// Each callback gets a unique 8-hex-char hash (matches Teams web client pattern).
/// Pattern: `{trouter_surl}callAgent/{endpoint_id}/{hash}/{path}`
pub(crate) fn trouter_callback(trouter_surl: &str, endpoint_id: &str, path: &str) -> String {
    let hash = format!("{:08x}", {
        // Simple hash from endpoint_id + path to produce unique per-path values
        let mut h: u32 = 0x811c9dc5; // FNV-1a init
        for b in endpoint_id.bytes().chain(path.bytes()) {
            h ^= b as u32;
            h = h.wrapping_mul(0x01000193);
        }
        h
    });
    format!(
        "{}callAgent/{}/{}/{}",
        trouter_surl, endpoint_id, hash, path
    )
}

/// Phase 1: Create a conversation by POSTing to /api/v2/epconv.
///
/// Returns the conversationController URL from the response.
pub async fn create_conversation(
    http: &reqwest::Client,
    epconv_url: &str,
    params: &ConversationCallParams<'_>,
) -> Result<ConversationCreated> {
    let tc = |path: &str| trouter_callback(params.trouter_surl, params.endpoint_id, path);
    let cause_id = &params.message_id[..8.min(params.message_id.len())];

    // Note: conversationRequest contains only subject/roster/properties/links.
    // Other fields (groupChat, participants, etc.) are siblings, not nested inside.
    let payload = serde_json::json!({
        "conversationRequest": {
            "subject": null,
            "roster": {
                "type": "Delta",
                "rosterUpdate": tc("conversation/rosterUpdate/")
            },
            "properties": {
                "allowConversationWithoutHost": true,
                "enableGroupCallEventMessages": true,
                "enableGroupCallUpgradeMessage": false,
                "enableGroupCallMeetupGeneration": true
            },
            "links": {
                "conversationEnd": tc("conversation/conversationEnd/"),
                "conversationUpdate": tc("conversation/conversationUpdate/"),
                "localParticipantUpdate": tc("conversation/localParticipantUpdate/"),
                "addParticipantSuccess": tc("conversation/addParticipantSuccess/"),
                "addParticipantFailure": tc("conversation/addParticipantFailure/"),
                "receiveMessage": tc("conversation/receiveMessage/")
            }
        },
        "groupContext": null,
        "groupChat": {
            "threadId": params.thread_id,
            "messageId": null
        },
        "participants": {
            "from": {
                "id": params.caller_mri,
                "displayName": params.caller_display_name,
                "endpointId": params.endpoint_id,
                "participantId": params.participant_id,
                "languageId": "en-US"
            }
        },
        "capabilities": null,
        // Capability bitmasks captured from Teams web client traffic
        "endpointCapabilities": 73463,
        "clientEndpointCapabilities": 9336554,
        "endpointMetadata": { "holographicCapabilities": 3 },
        "meetingInfo": null,
        "endpointState": {
            "endpointStateSequenceNumber": 0,
            "endpointProperties": {
                "additionalEndpointProperties": {
                    "infoShownInReportMode": "FullInformation"
                }
            }
        },
        "debugContent": {
            "ecsEtag": "\"0\"",
            "causeId": cause_id
        }
    });

    tracing::info!("Phase 1: POST {} (create conversation)", epconv_url);
    tracing::debug!(
        "Phase 1 payload: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    let resp = http
        .post(epconv_url)
        .header("Authorization", format!("Bearer {}", params.ic3_token))
        .header("Content-Type", "application/json")
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("x-microsoft-skype-message-id", params.message_id)
        .header("x-microsoft-skype-client", SKYPE_CLIENT_HEADER)
        .header("Referer", "https://teams.microsoft.com/")
        .header("ms-teams-partition", TEAMS_PARTITION)
        .header("ms-teams-region", TEAMS_REGION)
        .header("ms-teams-ring", TEAMS_RING)
        .header("x-ms-migration", "True")
        .json(&payload)
        .send()
        .await
        .context("Phase 1 POST to epconv failed")?;

    let status = resp.status();

    // Extract conversationController from response headers or body
    let headers = resp.headers().clone();
    let body = resp.text().await.unwrap_or_default();

    tracing::info!("Phase 1 response: {} ({} bytes)", status, body.len());
    tracing::debug!("Phase 1 response body: {}", &body[..body.len().min(2000)]);
    for (k, v) in headers.iter() {
        tracing::debug!(
            "Phase 1 header: {}: {}",
            k,
            v.to_str().unwrap_or("(binary)")
        );
    }

    if !status.is_success() {
        anyhow::bail!("Phase 1 epconv failed ({}): {}", status, body);
    }

    // Parse conversationController from response body
    let resp_json: serde_json::Value =
        serde_json::from_str(&body).context("Phase 1 response is not valid JSON")?;

    let conv_controller = resp_json
        .pointer("/conversationController")
        .or_else(|| resp_json.pointer("/conversationResponse/conversationController"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Also check Location header
    let conv_controller = conv_controller
        .or_else(|| {
            headers
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .context("No conversationController in phase 1 response")?;

    let add_participant_url = resp_json
        .pointer("/links/addParticipant")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    tracing::info!(
        "Phase 1 success: conversationController = {}, addParticipant link = {:?}",
        conv_controller,
        add_participant_url
    );

    Ok(ConversationCreated {
        conversation_controller: conv_controller,
        add_participant_url,
        response_body: body,
    })
}

/// Phase 2: Join the conversation with an SDP offer.
///
/// POSTs to the conversationController URL from phase 1.
pub async fn join_conversation_with_sdp(
    http: &reqwest::Client,
    conversation_controller: &str,
    params: &ConversationCallParams<'_>,
    sdp_offer: &str,
) -> Result<ConversationJoined> {
    let tc = |path: &str| trouter_callback(params.trouter_surl, params.endpoint_id, path);
    let cause_id = &params.message_id[..8.min(params.message_id.len())];

    // Same structure as phase 1: conversationRequest contains only
    // subject/roster/properties/links. Other fields are siblings.
    let payload = serde_json::json!({
        "conversationRequest": {
            "conversationType": null,
            "subject": "",
            "suppressDialout": true,
            "roster": {
                "type": "Delta",
                "rosterUpdate": tc("conversation/rosterUpdate/")
            },
            "properties": {
                "allowConversationWithoutHost": true,
                "enableGroupCallEventMessages": true,
                "enableGroupCallUpgradeMessage": false,
                "enableGroupCallMeetupGeneration": true
            },
            "links": {
                "conversationEnd": tc("conversation/conversationEnd/"),
                "conversationUpdate": tc("conversation/conversationUpdate/"),
                "localParticipantUpdate": tc("conversation/localParticipantUpdate/"),
                "addParticipantSuccess": tc("conversation/addParticipantSuccess/"),
                "addParticipantFailure": tc("conversation/addParticipantFailure/"),
                "addModalitySuccess": tc("conversation/addModalitySuccess/"),
                "addModalityFailure": tc("conversation/addModalityFailure/"),
                "confirmUnmute": tc("conversation/confirmUnmute/"),
                "receiveMessage": tc("conversation/receiveMessage/")
            }
        },
        "groupContext": null,
        "groupChat": {
            "threadId": params.thread_id,
            "messageId": null
        },
        "participants": {
            "from": {
                "id": params.caller_mri,
                "displayName": params.caller_display_name,
                "endpointId": params.endpoint_id,
                "participantId": params.participant_id,
                "languageId": "en-US"
            },
            "to": []
        },
        // Capability bitmasks captured from Teams web client traffic
        "capabilities": null,
        "endpointCapabilities": 73463,
        "clientEndpointCapabilities": 9336554,
        "endpointMetadata": { "holographicCapabilities": 3 },
        "meetingInfo": {
            "organizerId": params.caller_oid,
            "tenantId": params.tenant_id
        },
        "endpointState": {
            "endpointStateSequenceNumber": 0,
            "endpointProperties": {
                "preheatProperties": 1,
                "additionalEndpointProperties": {
                    "infoShownInReportMode": "FullInformation"
                }
            }
        },
        "callInvitation": {
            "callModalities": ["Audio"],
            "replaces": null,
            "transferor": null,
            "links": {
                "progress": tc("call/progress/"),
                "mediaAnswer": tc("call/mediaAnswer/"),
                "acceptance": tc("call/acceptance/"),
                "redirection": tc("call/redirection/"),
                "end": tc("call/end/")
            },
            "clientContentForMediaController": {
                "controlVideoStreaming": tc("call/controlVideoStreaming/"),
                "csrcInfo": tc("call/csrcInfo/"),
                "dominantSpeakerInfo": tc("call/dominantSpeakerInfo/")
            },
            "pstnContent": {
                "emergencyCallCountry": "",
                "platformName": "teams-cli",
                "publicApiCall": false
            },
            "mediaContent": {
                "contentType": "application/sdp",
                "blob": sdp_offer
            }
        },
        "debugContent": {
            "ecsEtag": "\"0\"",
            "causeId": cause_id
        }
    });

    tracing::info!("Phase 2: POST {} (join with SDP)", conversation_controller);
    tracing::debug!(
        "Phase 2 payload: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    let resp = http
        .post(conversation_controller)
        .header("Authorization", format!("Bearer {}", params.ic3_token))
        .header("Content-Type", "application/json")
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("x-microsoft-skype-message-id", params.message_id)
        .header("x-microsoft-skype-client", SKYPE_CLIENT_HEADER)
        .header("Referer", "https://teams.microsoft.com/")
        .header("ms-teams-partition", TEAMS_PARTITION)
        .header("ms-teams-region", TEAMS_REGION)
        .header("ms-teams-ring", TEAMS_RING)
        .header("x-ms-migration", "True")
        .json(&payload)
        .send()
        .await
        .context("Phase 2 POST to conversationController failed")?;

    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.text().await.unwrap_or_default();

    tracing::info!("Phase 2 response: {} ({} bytes)", status, body.len());
    tracing::debug!("Phase 2 response body: {}", &body[..body.len().min(2000)]);

    if !status.is_success() {
        anyhow::bail!("Phase 2 join failed ({}): {}", status, body);
    }

    let cc_active_url = headers
        .get("x-microsoft-skype-proxy-cluster-context")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    tracing::info!("Phase 2 success. CC active URL: {:?}", cc_active_url);

    Ok(ConversationJoined {
        cc_active_url,
        response_body: body,
    })
}

/// Accept an incoming call by POSTing to the acceptance URL.
pub async fn accept_call(
    http: &reqwest::Client,
    skype_token: &str,
    notification: &CallNotification,
) -> Result<()> {
    let invitation = notification
        .call_invitation
        .as_ref()
        .context("No callInvitation in notification")?;
    let links = invitation
        .links
        .as_ref()
        .context("No links in callInvitation")?;
    let acceptance_url = links
        .acceptance
        .as_ref()
        .context("No acceptance URL in links")?;

    let payload = serde_json::json!({
        "acceptedCallModalities": ["Audio"],
        "endpointMetadata": {
            "isCallMediaCaptured": false,
            "isMicrophoneOn": false
        }
    });

    tracing::info!("Accepting call -> POST {}", acceptance_url);

    let resp = http
        .post(acceptance_url)
        .header("X-Skypetoken", skype_token)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("Failed to POST acceptance")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status.is_success() {
        tracing::info!("Call accepted ({}): {}", status, body);
        Ok(())
    } else {
        anyhow::bail!("Call acceptance failed ({}): {}", status, body);
    }
}

/// Send the SDP media answer to the mediaAnswer URL.
pub async fn send_media_answer(
    http: &reqwest::Client,
    skype_token: &str,
    notification: &CallNotification,
    sdp_answer: &str,
) -> Result<()> {
    let invitation = notification
        .call_invitation
        .as_ref()
        .context("No callInvitation in notification")?;
    let links = invitation
        .links
        .as_ref()
        .context("No links in callInvitation")?;
    let media_answer_url = links
        .media_answer
        .as_ref()
        .context("No mediaAnswer URL in links")?;

    let payload = serde_json::json!({
        "mediaContent": {
            "blob": sdp_answer,
            "contentType": "application/sdp"
        }
    });

    tracing::info!("Sending media answer -> POST {}", media_answer_url);

    let resp = http
        .post(media_answer_url)
        .header("X-Skypetoken", skype_token)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("Failed to POST media answer")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status.is_success() {
        tracing::info!("Media answer sent ({}): {}", status, body);
        Ok(())
    } else {
        anyhow::bail!("Media answer failed ({}): {}", status, body);
    }
}

/// Build the standard CC signaling callback links for a call leg.
///
/// Used by both `acknowledge_call_acceptance` and `register_cc_callbacks`.
fn cc_call_links(tc: &dyn Fn(&str) -> String) -> serde_json::Value {
    serde_json::json!({
        "links": {
            "mediaAcknowledgement": tc("call/mediaAcknowledgement/"),
            "rejection": tc("call/rejection/"),
            "acknowledgement": tc("call/acknowledgement/"),
            "mediaRenegotiation": tc("call/mediaRenegotiation/"),
            "replacement": tc("call/replacement/"),
            "progress": tc("call/progress/"),
            "mediaAnswer": tc("call/mediaAnswer/"),
            "newMediaOffer": tc("call/newMediaOffer/"),
            "redirection": tc("call/redirection/"),
            "balanceUpdate": tc("call/balanceUpdate/"),
            "acceptance": tc("call/acceptance/"),
            "controlVideoStreaming": tc("call/controlVideoStreaming/"),
            "dominantSpeakerInfo": tc("call/dominantSpeakerInfo/"),
            "csrcInfo": tc("call/csrcInfo/"),
            "end": tc("call/end/"),
            "retargetCompletion": tc("call/retargetCompletion/"),
            "transfer": tc("call/transfer/"),
            "transferAcceptance": tc("call/transferAcceptance/"),
            "transferCompletion": tc("call/transferCompletion/"),
            "holdCompletion": tc("call/holdCompletion/"),
            "resumeCompletion": tc("call/resumeCompletion/"),
            "call": tc("call/updateMediaDescriptions"),
            "monitorCompletion": tc("call/monitorCompletion/")
        },
        "clientContentForMediaController": {
            "controlVideoStreaming": tc("call/controlVideoStreaming/"),
            "csrcInfo": tc("call/csrcInfo/")
        }
    })
}

/// Phase 3: Acknowledge call acceptance.
///
/// POST to the acknowledgement URL from the callAcceptance Trouter callback.
/// This tells the Call Controller we received the SDP answer and keeps the call alive.
/// Without this, CC times out and kills the call (error 430/subCode 10065).
pub async fn acknowledge_call_acceptance(
    http: &reqwest::Client,
    acknowledgement_url: &str,
    params: &ConversationCallParams<'_>,
) -> Result<()> {
    let tc = |path: &str| trouter_callback(params.trouter_surl, params.endpoint_id, path);

    tracing::info!(
        "Phase 3a: Acknowledging call acceptance -> POST {}",
        acknowledgement_url
    );

    let resp = http
        .post(acknowledgement_url)
        .header("Authorization", format!("Bearer {}", params.ic3_token))
        .header("Content-Type", "application/json")
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("x-microsoft-skype-message-id", params.message_id)
        .header("x-microsoft-skype-client", SKYPE_CLIENT_HEADER)
        .header("Referer", "https://teams.microsoft.com/")
        .header("ms-teams-partition", TEAMS_PARTITION)
        .header("ms-teams-region", TEAMS_REGION)
        .header("ms-teams-ring", TEAMS_RING)
        .json(&serde_json::json!({
            "callAcceptanceAcknowledgement": cc_call_links(&tc)
        }))
        .send()
        .await
        .context("Failed to POST call acceptance acknowledgement")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status.is_success() {
        tracing::info!("Call acceptance acknowledged ({}): {}", status, body);
        Ok(())
    } else {
        anyhow::bail!(
            "Call acceptance acknowledgement failed ({}): {}",
            status,
            body
        );
    }
}

/// Phase 3b: Register CC signaling callbacks on the call leg.
///
/// POST callParticipantUpdate with Trouter callback links to the callLeg URL.
/// This tells the Call Controller where to send subsequent signaling events
/// (media renegotiation, call end, transfer, etc.).
pub async fn register_cc_callbacks(
    http: &reqwest::Client,
    call_leg_url: &str,
    params: &ConversationCallParams<'_>,
) -> Result<()> {
    let tc = |path: &str| trouter_callback(params.trouter_surl, params.endpoint_id, path);

    let payload = serde_json::json!({
        "callAcceptanceAcknowledgement": cc_call_links(&tc),
        "callParticipantUpdate": cc_call_links(&tc)
    });

    tracing::info!(
        "Phase 3b: Registering CC callbacks -> POST {}",
        call_leg_url
    );
    tracing::debug!(
        "Phase 3b payload: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    let resp = http
        .post(call_leg_url)
        .header("Authorization", format!("Bearer {}", params.ic3_token))
        .header("Content-Type", "application/json")
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("x-microsoft-skype-message-id", params.message_id)
        .header("x-microsoft-skype-client", SKYPE_CLIENT_HEADER)
        .header("Referer", "https://teams.microsoft.com/")
        .header("ms-teams-partition", TEAMS_PARTITION)
        .header("ms-teams-region", TEAMS_REGION)
        .header("ms-teams-ring", TEAMS_RING)
        .json(&payload)
        .send()
        .await
        .context("Failed to POST CC callback registration")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status.is_success() {
        tracing::info!("CC callbacks registered ({}): {}", status, body);
        Ok(())
    } else {
        anyhow::bail!(
            "CC callback registration failed ({}): {}",
            status,
            body
        );
    }
}

/// Echo bot MRI (Call Quality Tester).
pub const ECHO_BOT_MRI: &str = "28:cf28171e-fcfd-47e4-a1d6-79460b0b3ca0";

/// Echo bot OID (extracted from the MRI).
const ECHO_BOT_OID: &str = "cf28171e-fcfd-47e4-a1d6-79460b0b3ca0";

/// Build the 1:1 thread ID for a call to the Echo bot.
pub fn echo_thread_id(caller_oid: &str) -> String {
    format!("19:{}_{}@unq.gbl.spaces", caller_oid, ECHO_BOT_OID)
}

/// Create an Echo bot call in a single epconv POST (matching real Teams client flow).
///
/// Unlike the two-phase approach for channel calls, the echo call embeds the SDP
/// directly in the epconv request. Returns both ConversationCreated and ConversationJoined.
pub async fn create_echo_call(
    http: &reqwest::Client,
    epconv_url: &str,
    params: &ConversationCallParams<'_>,
    sdp_offer: &str,
) -> Result<(ConversationCreated, ConversationJoined)> {
    let tc = |path: &str| trouter_callback(params.trouter_surl, params.endpoint_id, path);
    let cause_id = &params.message_id[..8.min(params.message_id.len())];

    let payload = serde_json::json!({
        "conversationRequest": {
            "conversationType": null,
            "subject": "",
            "suppressDialout": true,
            "roster": {
                "type": "Delta",
                "rosterUpdate": tc("conversation/rosterUpdate/")
            },
            "properties": {
                "allowConversationWithoutHost": true,
                "enableGroupCallEventMessages": true,
                "enableGroupCallUpgradeMessage": false,
                "enableGroupCallMeetupGeneration": false
            },
            "links": {
                "conversationEnd": tc("conversation/conversationEnd/"),
                "conversationUpdate": tc("conversation/conversationUpdate/"),
                "localParticipantUpdate": tc("conversation/localParticipantUpdate/"),
                "addParticipantSuccess": tc("conversation/addParticipantSuccess/"),
                "addParticipantFailure": tc("conversation/addParticipantFailure/"),
                "addModalitySuccess": tc("conversation/addModalitySuccess/"),
                "addModalityFailure": tc("conversation/addModalityFailure/"),
                "confirmUnmute": tc("conversation/confirmUnmute/"),
                "receiveMessage": tc("conversation/receiveMessage/")
            }
        },
        "scenario": "UserInitiatedTestCall",
        "groupContext": null,
        "groupChat": {
            "threadId": params.thread_id,
            "messageId": null
        },
        "participants": {
            "from": {
                "id": params.caller_mri,
                "displayName": params.caller_display_name,
                "endpointId": params.endpoint_id,
                "participantId": params.participant_id,
                "languageId": "en-US"
            },
            "to": []
        },
        "capabilities": null,
        "endpointCapabilities": 73463,
        "clientEndpointCapabilities": 9336554,
        "endpointMetadata": { "holographicCapabilities": 3 },
        "meetingInfo": null,
        "endpointState": {
            "endpointStateSequenceNumber": 0,
            "endpointProperties": {
                "additionalEndpointProperties": {
                    "infoShownInReportMode": "FullInformation"
                }
            }
        },
        "callInvitation": {
            "callModalities": ["Audio", "Video", "ScreenViewer"],
            "replaces": null,
            "transferor": null,
            "links": {
                "progress": tc("call/progress/"),
                "mediaAnswer": tc("call/mediaAnswer/"),
                "acceptance": tc("call/acceptance/"),
                "redirection": tc("call/redirection/"),
                "end": tc("call/end/")
            },
            "clientContentForMediaController": {
                "controlVideoStreaming": tc("call/controlVideoStreaming/"),
                "csrcInfo": tc("call/csrcInfo/"),
                "dominantSpeakerInfo": tc("call/dominantSpeakerInfo/")
            },
            "pstnContent": {
                "emergencyCallCountry": "",
                "platformName": "teams-cli",
                "publicApiCall": false
            },
            "mediaContent": {
                "contentType": "application/sdp",
                "blob": sdp_offer
            }
        },
        "debugContent": {
            "ecsEtag": "\"0\"",
            "causeId": cause_id
        }
    });

    tracing::info!("Echo call: POST {} (single-shot epconv with SDP, scenario=UserInitiatedTestCall)", epconv_url);

    let resp = http
        .post(epconv_url)
        .header("Authorization", format!("Bearer {}", params.ic3_token))
        .header("Content-Type", "application/json")
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("x-microsoft-skype-message-id", params.message_id)
        .header("x-microsoft-skype-client", SKYPE_CLIENT_HEADER)
        .header("Referer", "https://teams.microsoft.com/")
        .header("ms-teams-partition", TEAMS_PARTITION)
        .header("ms-teams-region", TEAMS_REGION)
        .header("ms-teams-ring", TEAMS_RING)
        .header("x-ms-migration", "True")
        .json(&payload)
        .send()
        .await
        .context("Echo call POST to epconv failed")?;

    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.text().await.unwrap_or_default();

    tracing::info!("Echo call response: {} ({} bytes)", status, body.len());
    tracing::debug!("Echo call response body: {}", &body[..body.len().min(2000)]);
    for (k, v) in headers.iter() {
        tracing::debug!("Echo call header: {}: {}", k, v.to_str().unwrap_or("(binary)"));
    }

    if !status.is_success() {
        anyhow::bail!("Echo call epconv failed ({}): {}", status, body);
    }

    // Parse conversationController from response
    let resp_json: serde_json::Value =
        serde_json::from_str(&body).context("Echo call response is not valid JSON")?;

    let conv_controller = resp_json
        .pointer("/conversationController")
        .or_else(|| resp_json.pointer("/conversationResponse/conversationController"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .context("No conversationController in echo call response")?;

    let add_participant_url = resp_json
        .pointer("/links/addParticipant")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    tracing::info!(
        "Echo call: conversationController = {}, addParticipant link = {:?}",
        conv_controller,
        add_participant_url
    );

    let cc_active_url = resp_json
        .pointer("/links/active")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let created = ConversationCreated {
        conversation_controller: conv_controller,
        add_participant_url,
        response_body: body.clone(),
    };
    let joined = ConversationJoined {
        cc_active_url,
        response_body: body,
    };

    Ok((created, joined))
}

/// Invite the Echo bot into the conversation via POST /conv/{id}/add.
///
/// This is step 3 of the Echo call flow: after creating the conversation (epconv)
/// and joining with SDP, we add the Echo bot as a participant with the 1:1 thread context.
pub async fn invite_echo_bot(
    http: &reqwest::Client,
    conversation_controller: &str,
    params: &ConversationCallParams<'_>,
) -> Result<()> {
    let tc = |path: &str| trouter_callback(params.trouter_surl, params.endpoint_id, path);

    // Derive /add URL from conversation controller
    let add_url = if let Some(idx) = conversation_controller.find('?') {
        let (path, query) = conversation_controller.split_at(idx);
        format!("{}/add{}", path.trim_end_matches('/'), query)
    } else {
        format!("{}/add", conversation_controller.trim_end_matches('/'))
    };

    let echo_participant_id = uuid::Uuid::new_v4().to_string();

    let payload = serde_json::json!({
        "disableUnmute": false,
        "participants": {
            "from": {
                "id": params.caller_mri,
                "displayName": params.caller_display_name,
                "endpointId": params.endpoint_id,
                "participantId": params.participant_id,
                "languageId": "en-US"
            },
            "to": [{
                "id": ECHO_BOT_MRI,
                "participantId": echo_participant_id
            }]
        },
        "participantInvitationData": {},
        "replacementDetails": null,
        "groupContext": null,
        "groupChat": {
            "threadId": params.thread_id,
            "messageId": null
        },
        "links": {
            "addParticipantSuccess": tc("conversation/addParticipantSuccess/"),
            "addParticipantFailure": tc("conversation/addParticipantFailure/")
        }
    });

    // Fresh message-id: the conv server uses this for deduplication; reusing the
    // epconv message-id would cause a cached empty response.
    let echo_bot_msg_id = uuid::Uuid::new_v4().to_string();

    tracing::info!("Inviting Echo bot -> POST {} (msg_id={})", add_url, echo_bot_msg_id);
    tracing::debug!("Echo bot invite payload: {}", serde_json::to_string_pretty(&payload).unwrap_or_default());

    let resp = http
        .post(&add_url)
        .header("Authorization", format!("Bearer {}", params.ic3_token))
        .header("Content-Type", "application/json")
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("x-microsoft-skype-message-id", &echo_bot_msg_id)
        .header("x-microsoft-skype-client", SKYPE_CLIENT_HEADER)
        .header("Referer", "https://teams.microsoft.com/")
        .header("ms-teams-partition", TEAMS_PARTITION)
        .header("ms-teams-region", TEAMS_REGION)
        .header("ms-teams-ring", TEAMS_RING)
        .header("x-ms-migration", "True")
        .json(&payload)
        .send()
        .await
        .context("Failed to POST echo bot invite")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    tracing::info!("Echo bot invite response: {} ({} bytes)", status, body.len());
    tracing::debug!("Echo bot invite response body: {}", body);

    if !status.is_success() {
        anyhow::bail!("Echo bot invite failed ({}): {}", status, body);
    }

    Ok(())
}

/// Create a 1:1 call in a single epconv POST.
///
/// Similar to create_echo_call but without the UserInitiatedTestCall scenario.
/// Used for calling real users by their 1:1 thread ID.
pub async fn create_1to1_call(
    http: &reqwest::Client,
    epconv_url: &str,
    params: &ConversationCallParams<'_>,
    sdp_offer: &str,
) -> Result<(ConversationCreated, ConversationJoined)> {
    let tc = |path: &str| trouter_callback(params.trouter_surl, params.endpoint_id, path);
    let cause_id = &params.message_id[..8.min(params.message_id.len())];

    let payload = serde_json::json!({
        "conversationRequest": {
            "conversationType": null,
            "subject": "",
            "suppressDialout": false,  // false = ring the callee
            "roster": {
                "type": "Delta",
                "rosterUpdate": tc("conversation/rosterUpdate/")
            },
            "properties": {
                "allowConversationWithoutHost": true,
                "enableGroupCallEventMessages": true,
                "enableGroupCallUpgradeMessage": false,
                "enableGroupCallMeetupGeneration": false
            },
            "links": {
                "conversationEnd": tc("conversation/conversationEnd/"),
                "conversationUpdate": tc("conversation/conversationUpdate/"),
                "localParticipantUpdate": tc("conversation/localParticipantUpdate/"),
                "addParticipantSuccess": tc("conversation/addParticipantSuccess/"),
                "addParticipantFailure": tc("conversation/addParticipantFailure/"),
                "addModalitySuccess": tc("conversation/addModalitySuccess/"),
                "addModalityFailure": tc("conversation/addModalityFailure/"),
                "confirmUnmute": tc("conversation/confirmUnmute/"),
                "receiveMessage": tc("conversation/receiveMessage/")
            }
        },
        "groupContext": null,
        "groupChat": {
            "threadId": params.thread_id,
            "messageId": null
        },
        "participants": {
            "from": {
                "id": params.caller_mri,
                "displayName": params.caller_display_name,
                "endpointId": params.endpoint_id,
                "participantId": params.participant_id,
                "languageId": "en-US"
            },
            "to": []
        },
        "capabilities": null,
        "endpointCapabilities": 73463,
        "clientEndpointCapabilities": 9336554,
        "endpointMetadata": { "holographicCapabilities": 3 },
        "meetingInfo": null,
        "endpointState": {
            "endpointStateSequenceNumber": 0,
            "endpointProperties": {
                "additionalEndpointProperties": {
                    "infoShownInReportMode": "FullInformation"
                }
            }
        },
        "callInvitation": {
            "callModalities": ["Audio", "Video", "ScreenViewer"],
            "replaces": null,
            "transferor": null,
            "links": {
                "progress": tc("call/progress/"),
                "mediaAnswer": tc("call/mediaAnswer/"),
                "acceptance": tc("call/acceptance/"),
                "redirection": tc("call/redirection/"),
                "end": tc("call/end/")
            },
            "clientContentForMediaController": {
                "controlVideoStreaming": tc("call/controlVideoStreaming/"),
                "csrcInfo": tc("call/csrcInfo/"),
                "dominantSpeakerInfo": tc("call/dominantSpeakerInfo/")
            },
            "pstnContent": {
                "emergencyCallCountry": "",
                "platformName": "teams-cli",
                "publicApiCall": false
            },
            "mediaContent": {
                "contentType": "application/sdp",
                "blob": sdp_offer
            }
        },
        "debugContent": {
            "ecsEtag": "\"0\"",
            "causeId": cause_id
        }
    });

    tracing::info!("1:1 call: POST {} (single-shot epconv with SDP)", epconv_url);

    let resp = http
        .post(epconv_url)
        .header("Authorization", format!("Bearer {}", params.ic3_token))
        .header("Content-Type", "application/json")
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("x-microsoft-skype-message-id", params.message_id)
        .header("x-microsoft-skype-client", SKYPE_CLIENT_HEADER)
        .header("Referer", "https://teams.microsoft.com/")
        .header("ms-teams-partition", TEAMS_PARTITION)
        .header("ms-teams-region", TEAMS_REGION)
        .header("ms-teams-ring", TEAMS_RING)
        .header("x-ms-migration", "True")
        .json(&payload)
        .send()
        .await
        .context("1:1 call POST to epconv failed")?;

    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.text().await.unwrap_or_default();

    tracing::info!("1:1 call response: {} ({} bytes)", status, body.len());
    tracing::debug!("1:1 call response body: {}", &body[..body.len().min(2000)]);

    if !status.is_success() {
        anyhow::bail!("1:1 call epconv failed ({}): {}", status, body);
    }

    // Parse conversationController from response
    let resp_json: serde_json::Value =
        serde_json::from_str(&body).context("1:1 call response is not valid JSON")?;

    let conv_controller = resp_json
        .pointer("/conversationController")
        .or_else(|| resp_json.pointer("/conversationResponse/conversationController"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .context("No conversationController in 1:1 call response")?;

    let add_participant_url = resp_json
        .pointer("/links/addParticipant")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    tracing::info!(
        "1:1 call: conversationController = {}, addParticipant link = {:?}",
        conv_controller,
        add_participant_url
    );

    let cc_active_url = resp_json
        .pointer("/links/active")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let created = ConversationCreated {
        conversation_controller: conv_controller,
        add_participant_url,
        response_body: body.clone(),
    };
    let joined = ConversationJoined {
        cc_active_url,
        response_body: body,
    };

    Ok((created, joined))
}

/// Invite a user into the conversation via POST /conv/{id}/add.
///
/// This is step 3 of the 1:1 call flow: after creating the conversation (epconv)
/// and joining with SDP, we add the callee as a participant.
///
/// If `include_video` is true, the call invitation will include Video modality.
pub async fn invite_user(
    http: &reqwest::Client,
    conversation_controller: &str,
    params: &ConversationCallParams<'_>,
    callee_mri: &str,
    include_video: bool,
) -> Result<()> {
    let tc = |path: &str| trouter_callback(params.trouter_surl, params.endpoint_id, path);

    // Derive /add URL from conversation controller
    let add_url = if let Some(idx) = conversation_controller.find('?') {
        let (path, query) = conversation_controller.split_at(idx);
        format!("{}/add{}", path.trim_end_matches('/'), query)
    } else {
        format!("{}/add", conversation_controller.trim_end_matches('/'))
    };

    let callee_participant_id = uuid::Uuid::new_v4().to_string();
    let call_modalities: Vec<&str> = if include_video {
        vec!["Audio", "Video"]
    } else {
        vec!["Audio"]
    };

    let payload = serde_json::json!({
        "disableUnmute": false,
        "participants": {
            "from": {
                "id": params.caller_mri,
                "displayName": params.caller_display_name,
                "endpointId": params.endpoint_id,
                "participantId": params.participant_id,
                "languageId": "en-US"
            },
            "to": [{
                "id": callee_mri,
                "participantId": callee_participant_id
            }]
        },
        // Include call invitation data to trigger ringing on callee's device
        "participantInvitationData": {
            "callModalities": call_modalities,
            "callDirection": "Outgoing"
        },
        "callInvitation": {
            "callModalities": call_modalities,
            "replaces": null,
            "transferor": null
        },
        "replacementDetails": null,
        "groupContext": null,
        "groupChat": {
            "threadId": params.thread_id,
            "messageId": null
        },
        "links": {
            "addParticipantSuccess": tc("conversation/addParticipantSuccess/"),
            "addParticipantFailure": tc("conversation/addParticipantFailure/")
        }
    });

    // Fresh message-id for deduplication
    let invite_msg_id = uuid::Uuid::new_v4().to_string();

    tracing::info!("Inviting user {} -> POST {} (msg_id={})", callee_mri, add_url, invite_msg_id);
    tracing::debug!("User invite payload: {}", serde_json::to_string_pretty(&payload).unwrap_or_default());

    let resp = http
        .post(&add_url)
        .header("Authorization", format!("Bearer {}", params.ic3_token))
        .header("Content-Type", "application/json")
        .header("x-microsoft-skype-chain-id", params.chain_id)
        .header("x-microsoft-skype-message-id", &invite_msg_id)
        .header("x-microsoft-skype-client", SKYPE_CLIENT_HEADER)
        .header("Referer", "https://teams.microsoft.com/")
        .header("ms-teams-partition", TEAMS_PARTITION)
        .header("ms-teams-region", TEAMS_REGION)
        .header("ms-teams-ring", TEAMS_RING)
        .header("x-ms-migration", "True")
        .json(&payload)
        .send()
        .await
        .context("Failed to POST user invite")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    tracing::info!("User invite response: {} ({} bytes)", status, body.len());
    tracing::debug!("User invite response body: {}", body);

    if !status.is_success() {
        anyhow::bail!("User invite failed ({}): {}", status, body);
    }

    Ok(())
}

/// End a call by POSTing to the end URL.
pub async fn end_call(
    http: &reqwest::Client,
    skype_token: &str,
    notification: &CallNotification,
) -> Result<()> {
    let invitation = notification
        .call_invitation
        .as_ref()
        .context("No callInvitation in notification")?;
    let links = invitation
        .links
        .as_ref()
        .context("No links in callInvitation")?;
    let end_url = links.end.as_ref().context("No end URL in links")?;

    tracing::info!("Ending call -> POST {}", end_url);

    let resp = http
        .post(end_url)
        .header("X-Skypetoken", skype_token)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .send()
        .await
        .context("Failed to POST end call")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status.is_success() {
        tracing::info!("Call ended ({}): {}", status, body);
        Ok(())
    } else {
        anyhow::bail!("Call end failed ({}): {}", status, body);
    }
}
