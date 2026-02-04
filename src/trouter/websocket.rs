//! Trouter WebSocket connection and frame handling

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::session::SessionResponse;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub struct TrouterSocket {
    stream: WsStream,
}

impl TrouterSocket {
    /// Connect to the Trouter WebSocket endpoint.
    ///
    /// Auth is handled by the session ID in the URL (obtained via authenticated GET).
    /// No auth headers or messages needed on the WebSocket itself.
    pub async fn connect(session: &SessionResponse, session_id: &str, epid: &str) -> Result<Self> {
        let ws_url = session.ws_url(session_id, epid);
        let ws_url = ws_url
            .replace("https://", "wss://")
            .replace("http://", "ws://");

        tracing::info!("Connecting WebSocket to {}", ws_url);

        let (stream, response) = connect_async(&ws_url)
            .await
            .context("WebSocket connection failed")?;

        tracing::info!("WebSocket connected (status={})", response.status());

        Ok(Self { stream })
    }

    /// Send a text frame.
    pub async fn send_text(&mut self, msg: &str) -> Result<()> {
        tracing::debug!("WS send: {}", msg);
        self.stream
            .send(Message::Text(msg.to_string()))
            .await
            .context("Failed to send WebSocket message")
    }

    /// Receive the next text frame, ignoring pings/pongs.
    ///
    /// Automatically sends HTTP 200 responses for Trouter data frame deliveries.
    /// Trouter uses HTTP-over-WebSocket: each `3:::` data frame contains an `"id"` field.
    /// The client MUST respond with `3:::{"id":N,"status":200}` to confirm delivery.
    /// Without this, Trouter returns 504 to the sender (e.g. Call Controller),
    /// which kills calls with error 430/10065.
    pub async fn recv_frame(&mut self) -> Result<Option<String>> {
        loop {
            match self.stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    tracing::debug!("WS recv: {}", text);

                    // Auto-respond to Trouter data frame deliveries (3::: HTTP-over-WS)
                    if let Some(req_id) = extract_trouter_request_id(&text) {
                        let resp = format!("3:::{{\"id\":{},\"status\":200}}", req_id);
                        tracing::debug!("Trouter response: {}", resp);
                        if let Err(e) = self.stream.send(Message::Text(resp)).await {
                            tracing::warn!("Failed to send Trouter response: {:#}", e);
                        }
                    }

                    // Auto-ack Socket.IO event frames (5:ID::)
                    // Without acks, the server retries indefinitely and blocks new events.
                    if let Some(ack_id) = extract_socketio_ack_id(&text) {
                        let ack = format!("6:{}::", ack_id);
                        tracing::debug!("Socket.IO ack: {}", ack);
                        if let Err(e) = self.stream.send(Message::Text(ack)).await {
                            tracing::warn!("Failed to send Socket.IO ack: {:#}", e);
                        }
                    }

                    return Ok(Some(text));
                }
                Some(Ok(Message::Ping(data))) => {
                    self.stream
                        .send(Message::Pong(data))
                        .await
                        .context("Failed to send pong")?;
                }
                Some(Ok(Message::Close(frame))) => {
                    tracing::info!("WebSocket closed: {:?}", frame);
                    return Ok(None);
                }
                Some(Ok(other)) => {
                    tracing::debug!("WS frame (ignored): {:?}", other);
                }
                Some(Err(e)) => {
                    return Err(e).context("WebSocket receive error");
                }
                None => {
                    return Ok(None);
                }
            }
        }
    }
}

/// Extract the Trouter request ID from a `3:::` data frame.
///
/// Trouter HTTP-over-WS data frames (`3:::`) contain `"id":NNN` at the JSON top level.
/// Only `3:::` frames use this mechanism; `5:` event frames use Socket.IO acks instead.
fn extract_trouter_request_id(frame: &str) -> Option<i64> {
    let json_str = frame.strip_prefix("3:::")?;
    // Fast path: find "id": near the start of JSON to avoid full parse
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    v.get("id").and_then(|id| id.as_i64())
}

/// Extract Socket.IO event ack ID from a `5:ID::` frame.
///
/// Socket.IO v1 event frames have format `5:ACK_ID:ENDPOINT:JSON`.
/// Returns the numeric ack ID if present.
fn extract_socketio_ack_id(frame: &str) -> Option<u64> {
    let rest = frame.strip_prefix("5:")?;
    let colon_pos = rest.find(':')?;
    let id_part = &rest[..colon_pos];
    if id_part.is_empty() {
        return None;
    }
    id_part.parse().ok()
}
