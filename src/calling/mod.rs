//! Call signaling module — parse incoming call invitations and manage call lifecycle.
//!
//! This handles signaling only (no media streaming).

#[cfg(feature = "audio")]
pub mod audio;
pub mod call_test;
#[cfg(feature = "video-capture")]
pub mod camera;
#[cfg(feature = "video-capture")]
pub mod codec;
#[cfg(feature = "video-capture")]
pub mod display;
pub mod ice;
pub mod media;
pub mod recording;
pub mod rtcp;
pub mod rtp;
pub mod sdp;
pub mod sdp_compress;
pub mod signaling;
pub mod srtp;
pub mod test_tone;
pub mod turn;
pub mod video;

use serde::Deserialize;

/// Links provided in a callInvitation for signaling actions.
#[derive(Debug, Clone, Deserialize)]
pub struct CallLinks {
    pub acceptance: Option<String>,
    pub end: Option<String>,
    #[serde(rename = "mediaAnswer")]
    pub media_answer: Option<String>,
    #[serde(rename = "p2pForkNotification")]
    pub p2p_fork_notification: Option<String>,
}

/// Media content (SDP blob) from the call invitation.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaContent {
    pub blob: Option<String>,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
}

/// Participant identity.
#[derive(Debug, Clone, Deserialize)]
pub struct Participant {
    pub id: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "endpointId")]
    pub endpoint_id: Option<String>,
    #[serde(rename = "languageId")]
    pub language_id: Option<String>,
}

/// Participants block from the invitation.
#[derive(Debug, Clone, Deserialize)]
pub struct Participants {
    pub from: Option<Participant>,
    pub to: Option<Vec<Participant>>,
}

/// Conversation request links.
#[derive(Debug, Clone, Deserialize)]
pub struct ConversationLinks {
    #[serde(rename = "conversationEnd")]
    pub conversation_end: Option<String>,
    #[serde(rename = "conversationUpdate")]
    pub conversation_update: Option<String>,
    #[serde(rename = "localParticipantUpdate")]
    pub local_participant_update: Option<String>,
}

/// Conversation request from the invitation.
#[derive(Debug, Clone, Deserialize)]
pub struct ConversationRequest {
    pub links: Option<ConversationLinks>,
}

/// Debug content with call/endpoint/operation IDs.
#[derive(Debug, Clone, Deserialize)]
pub struct DebugContent {
    #[serde(rename = "callId")]
    pub call_id: Option<String>,
    #[serde(rename = "endpointId")]
    pub endpoint_id: Option<String>,
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,
}

/// The core call invitation payload from a Trouter push.
#[derive(Debug, Clone, Deserialize)]
pub struct CallInvitation {
    #[serde(rename = "callModalities")]
    pub call_modalities: Option<Vec<String>>,
    pub links: Option<CallLinks>,
    #[serde(rename = "mediaContent")]
    pub media_content: Option<MediaContent>,
}

/// Top-level envelope for a call notification pushed via Trouter.
#[derive(Debug, Clone, Deserialize)]
pub struct CallNotification {
    #[serde(rename = "callInvitation")]
    pub call_invitation: Option<CallInvitation>,
    pub participants: Option<Participants>,
    #[serde(rename = "conversationRequest")]
    pub conversation_request: Option<ConversationRequest>,
    #[serde(rename = "debugContent")]
    pub debug_content: Option<DebugContent>,
}

/// Simple call lifecycle state.
#[derive(Debug, Clone, PartialEq)]
pub enum CallState {
    Idle,
    Ringing,
    Accepting,
    Connected,
    Ended,
}

/// Try to parse a call notification from the Trouter event JSON.
///
/// The Trouter frame body is an HTTP-like request where the body is JSON.
/// We try to extract the JSON and deserialize it.
pub fn parse_call_notification(json_str: &str) -> Option<CallNotification> {
    // The JSON may be the full Trouter event envelope or just the body.
    // Try to find a JSON object containing "callInvitation".
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;

    // The Trouter event JSON has a nested structure. The call notification
    // may be at the top level or nested under a "body" field parsed as string.
    if v.get("callInvitation").is_some() {
        return serde_json::from_value(v).ok();
    }

    // Sometimes the body is a stringified JSON inside the event wrapper.
    // Look for common Trouter event wrapper patterns.
    if let Some(body) = v.get("body") {
        if let Some(body_str) = body.as_str() {
            return parse_call_notification(body_str);
        }
        if body.get("callInvitation").is_some() {
            return serde_json::from_value(body.clone()).ok();
        }
    }

    None
}
