//! Trouter v4 WebSocket push notification client
//!
//! Connects to Microsoft Teams' Trouter service to receive real-time
//! push notifications (messages, presence, calls, etc.).

pub mod registrar;
pub mod session;
pub mod websocket;

use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use tokio::time;

use crate::calling;
use crate::config::Config;

/// Reason the inner connection loop exited.
enum DisconnectReason {
    /// Clean shutdown (Ctrl+C). Do not reconnect.
    Shutdown,
    /// Error or server-initiated close. Should reconnect.
    Error(anyhow::Error),
}

/// Run the Trouter connection with automatic reconnection.
///
/// On transient errors or server-initiated disconnects, reconnects with
/// exponential backoff (1s, 2s, 4s, ... capped at 64s). On clean shutdown
/// (Ctrl+C), exits immediately.
pub async fn connect_and_run() -> Result<()> {
    let mut backoff = 1u64;

    loop {
        match connect_and_run_inner().await {
            Ok(DisconnectReason::Shutdown) => {
                return Ok(());
            }
            Ok(DisconnectReason::Error(e)) => {
                // Connection was stable (>60s), reset backoff before reconnecting.
                backoff = 1;
                tracing::warn!(
                    "Trouter disconnected after stable session: {:#}. Reconnecting in 1s...",
                    e,
                );

                tokio::select! {
                    _ = time::sleep(Duration::from_secs(1)) => {}
                    _ = tokio::signal::ctrl_c() => {
                        println!("Shutting down...");
                        return Ok(());
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Trouter disconnected: {:#}. Reconnecting in {}s...",
                    e,
                    backoff
                );

                tokio::select! {
                    _ = time::sleep(Duration::from_secs(backoff)) => {}
                    _ = tokio::signal::ctrl_c() => {
                        println!("Shutting down...");
                        return Ok(());
                    }
                }

                backoff = (backoff * 2).min(64);
            }
        }
    }
}

/// Run one full Trouter session: negotiate, connect, event loop.
///
/// Returns `DisconnectReason::Shutdown` on clean Ctrl+C, or
/// `DisconnectReason::Error` when the connection should be retried.
async fn connect_and_run_inner() -> Result<DisconnectReason> {
    // Reload config each attempt so we pick up refreshed tokens.
    let config = Config::load().context("Failed to load config")?;

    let skype_token = config
        .get_skype_token()
        .context("No skype token found. Run `teams-cli login` first.")?;
    anyhow::ensure!(
        !skype_token.is_expired(),
        "Skype token expired. Run `teams-cli login` to refresh."
    );

    let skype_token_str = &skype_token.token;
    let http = reqwest::Client::new();

    // 1. Negotiate session (returns session info + epid)
    let (session, epid) = session::negotiate(&http, skype_token_str).await?;

    // 2. Get socket.io session ID (authenticated via X-Skypetoken header)
    let session_id = session::get_session_id(&http, &session, skype_token_str, &epid).await?;

    // 3. Connect WebSocket (auth is via session ID in URL, no headers needed)
    let mut ws = websocket::TrouterSocket::connect(&session, &session_id, &epid).await?;

    // 4. Wait for handshake frame (1::)
    let frame = ws
        .recv_frame()
        .await?
        .context("Connection closed before handshake")?;

    if !frame.starts_with("1::") {
        tracing::warn!("Expected 1:: handshake, got: {}", frame);
    } else {
        tracing::info!("Received handshake frame");
    }

    // 5. Register with registrar
    let registrar_ttl_secs: u64 = 86400;
    if let Some(ref reg_url) = session.registrar_url {
        if let Err(e) = registrar::register(&http, skype_token_str, reg_url, &session.surl).await {
            tracing::warn!("Initial registrar registration failed: {:#}", e);
        }
    }

    // 6. Event loop: recv frames, send heartbeat, re-register before TTL,
    //    force reconnect after session max age.
    let connected_at = Instant::now();
    let mut heartbeat = time::interval(Duration::from_secs(30));
    heartbeat.tick().await; // skip first immediate tick

    // Re-register 30s before TTL expires.
    let re_register_interval = Duration::from_secs(registrar_ttl_secs.saturating_sub(30));
    let mut re_register_deadline = Box::pin(time::sleep(re_register_interval));

    // Force full reconnect after 1 hour to refresh the session.
    // The session TTL is typically ~589000s but rotating more frequently
    // keeps tokens and registrations fresh.
    let session_max_age = Duration::from_secs(3600);
    let mut session_deadline = Box::pin(time::sleep(session_max_age));

    // Stability threshold: reset backoff after 60s of successful connection.
    // We communicate this via the return value — the caller checks timing.
    let stability_threshold = Duration::from_secs(60);

    println!("Trouter connected. Listening for events... (Ctrl-C to stop)");

    let disconnect_reason = loop {
        tokio::select! {
            frame = ws.recv_frame() => {
                match frame {
                    Ok(Some(text)) => handle_frame(&text, &http, skype_token_str).await,
                    Ok(None) => {
                        break DisconnectReason::Error(anyhow::anyhow!("WebSocket closed by server"));
                    }
                    Err(e) => {
                        break DisconnectReason::Error(e.context("WebSocket recv error"));
                    }
                }
            }
            _ = heartbeat.tick() => {
                if let Err(e) = ws.send_text("2::").await {
                    break DisconnectReason::Error(e.context("Heartbeat send failed"));
                }
            }
            _ = &mut re_register_deadline => {
                tracing::info!("Re-registering with registrar (TTL refresh)");
                if let Some(ref reg_url) = session.registrar_url {
                    let http2 = http.clone();
                    let tok = skype_token_str.to_string();
                    let surl = session.surl.clone();
                    let reg = reg_url.clone();
                    tokio::spawn(async move {
                        if let Err(e) = registrar::register(&http2, &tok, &reg, &surl).await {
                            tracing::warn!("Re-registration failed: {:#}", e);
                        }
                    });
                }
                // Reset the timer for another cycle.
                re_register_deadline = Box::pin(time::sleep(re_register_interval));
            }
            _ = &mut session_deadline => {
                tracing::info!("Session max age reached (1h), forcing reconnect for fresh session");
                break DisconnectReason::Error(anyhow::anyhow!("Session max age reached"));
            }
            _ = tokio::signal::ctrl_c() => {
                println!("Shutting down...");
                break DisconnectReason::Shutdown;
            }
        }
    };

    // If we were connected long enough, signal stability so caller resets backoff.
    // We do this by returning Ok (the caller pattern-matches on it).
    if connected_at.elapsed() >= stability_threshold {
        // Reset backoff indirectly: caller sees Ok and resets.
        // But we still need to convey the reason.
        // Use Ok for both shutdown and stable-error cases.
        return Ok(disconnect_reason);
    }

    match disconnect_reason {
        DisconnectReason::Shutdown => Ok(DisconnectReason::Shutdown),
        DisconnectReason::Error(e) => Err(e),
    }
}

/// Handle an incoming socket.io frame.
async fn handle_frame(frame: &str, http: &reqwest::Client, skype_token: &str) {
    // socket.io framing:
    // 1:: — handshake (handled above)
    // 2:: — heartbeat ping (server)
    // 3::: — ephemeral message
    // 5:X::{json} — event
    // 6:X+::{json} — ack event

    if frame.starts_with("2::") {
        tracing::debug!("Heartbeat ping from server");
        return;
    }

    if frame.starts_with("5:::") || frame.starts_with("5:") {
        // Socket.IO v1 event frame: 5:ACK_ID:ENDPOINT:JSON
        // Ack (6:ID::) is sent automatically by recv_frame() in websocket.rs.
        // Here we just extract the JSON payload after the `::` separator.
        let after_5 = &frame[2..]; // skip "5:"
        let json_str = after_5
            .find("::")
            .map(|pos| &after_5[pos + 2..])
            .filter(|s| s.starts_with('{'));

        if let Some(json_str) = json_str {
            let is_call = frame.contains("NGCallManagerWin");
            let prefix = if is_call {
                "[CALL]"
            } else if frame.contains("SkypeSpacesWeb") {
                "[CALL-INFO]"
            } else {
                "[MSG]"
            };
            println!("{} Event: {}", prefix, json_str);

            // If this is a call event, try to parse and auto-answer.
            if is_call {
                handle_call_event(json_str, http, skype_token).await;
            }
        } else {
            println!("Frame: {}", frame);
        }
        return;
    }

    if frame.starts_with("6:") {
        if let Some(json_start) = frame.find(":::{") {
            let json_str = &frame[json_start + 3..];
            println!("Ack: {}", json_str);
        } else {
            println!("Ack frame: {}", frame);
        }
        return;
    }

    println!("Frame: {}", frame);
}

/// Handle a call event from Trouter — parse invitation and auto-answer.
async fn handle_call_event(json_str: &str, http: &reqwest::Client, skype_token: &str) {
    let notification = match calling::parse_call_notification(json_str) {
        Some(n) => n,
        None => {
            tracing::debug!("Could not parse call notification from JSON");
            return;
        }
    };

    // Log caller identity.
    if let Some(ref participants) = notification.participants {
        if let Some(ref from) = participants.from {
            println!(
                "  Incoming call from: {} ({})",
                from.display_name.as_deref().unwrap_or("unknown"),
                from.id.as_deref().unwrap_or("?")
            );
        }
    }

    // Log call modalities.
    if let Some(ref inv) = notification.call_invitation {
        if let Some(ref mods) = inv.call_modalities {
            println!("  Modalities: {:?}", mods);
        }
    }

    // Log call ID from debug content.
    if let Some(ref debug) = notification.debug_content {
        if let Some(ref call_id) = debug.call_id {
            println!("  Call ID: {}", call_id);
        }
    }

    // Determine if video modality is requested.
    let has_video = notification
        .call_invitation
        .as_ref()
        .and_then(|inv| inv.call_modalities.as_ref())
        .map(|mods| mods.iter().any(|m| m.eq_ignore_ascii_case("video")))
        .unwrap_or(false);

    // Auto-answer: generate SDP answer and send acceptance.
    println!("  Auto-answering call...");

    // Try to acquire TURN relay credentials (best-effort, fail gracefully).
    let relay_config = match calling::turn::acquire_relay_credentials(http, skype_token).await {
        Ok(config) => {
            tracing::info!(
                "Acquired relay credentials: {} servers, username={}, ttl={}s",
                config.servers.len(),
                config.username,
                config.ttl
            );
            Some(config)
        }
        Err(e) => {
            tracing::info!(
                "Relay credential acquisition failed (will use direct/srflx only): {:#}",
                e
            );
            None
        }
    };

    // Gather relay candidate if we have credentials.
    let relay_candidate = if let Some(ref config) = relay_config {
        match calling::turn::gather_relay_candidate(config).await {
            Some((candidate, _client)) => {
                tracing::info!(
                    "Gathered relay candidate: {}:{}",
                    candidate.address,
                    candidate.port
                );
                Some(candidate)
            }
            None => {
                tracing::info!("Failed to gather relay candidate (TURN allocate failed)");
                None
            }
        }
    } else {
        None
    };

    // Try to generate and send media answer first (protocol order: media answer before acceptance).
    if let Some(ref inv) = notification.call_invitation {
        if let Some(ref mc) = inv.media_content {
            if let Some(ref blob) = mc.blob {
                match calling::sdp::parse_sdp_offer(blob) {
                    Ok(offer_info) => {
                        let local_ip = calling::sdp::get_local_ip();

                        // Parse remote SRTP keying material from the offer's crypto lines.
                        let remote_audio_crypto = offer_info
                            .crypto_lines
                            .iter()
                            .find_map(|line| calling::srtp::parse_crypto_line(line).ok());

                        let remote_video_crypto = offer_info.video.as_ref().and_then(|v| {
                            v.crypto_lines
                                .iter()
                                .find_map(|line| calling::srtp::parse_crypto_line(line).ok())
                        });

                        // Build local candidates list (relay if available).
                        let mut local_cands: Vec<calling::ice::IceCandidate> = Vec::new();
                        if let Some(ref rc) = relay_candidate {
                            local_cands.push(rc.clone());
                        }

                        // Generate SDP answer with ICE credentials.
                        let answer_result = calling::sdp::generate_sdp_answer_full(
                            &local_ip,
                            0,
                            0,
                            &offer_info,
                            &local_cands,
                            &[],
                        );

                        // Parse our own SRTP keying material from the answer.
                        let local_audio_crypto =
                            calling::srtp::parse_crypto_line(&answer_result.audio_crypto_line).ok();
                        let local_video_crypto = answer_result
                            .video_crypto_line
                            .as_ref()
                            .and_then(|line| calling::srtp::parse_crypto_line(line).ok());

                        tracing::info!(
                            "Generated SDP answer ({} bytes, video={}, ufrag={})",
                            answer_result.sdp.len(),
                            offer_info.video.is_some(),
                            answer_result.audio_ice_ufrag
                        );

                        if let Err(e) = calling::signaling::send_media_answer(
                            http,
                            skype_token,
                            &notification,
                            &answer_result.sdp,
                        )
                        .await
                        {
                            tracing::warn!("Failed to send media answer: {:#}", e);
                        }

                        // Start audio media session with ICE connectivity checks.
                        if let (Some(local_mat), Some(remote_mat)) =
                            (local_audio_crypto, remote_audio_crypto)
                        {
                            let candidates = calling::ice::parse_candidates_from_sdp(blob);
                            if candidates.iter().any(|c| {
                                c.transport == calling::ice::Transport::Udp && c.component == 1
                            }) {
                                let local_creds = calling::ice::IceCredentials {
                                    ufrag: answer_result.audio_ice_ufrag.clone(),
                                    pwd: answer_result.audio_ice_pwd.clone(),
                                };
                                let remote_creds = calling::ice::IceCredentials {
                                    ufrag: offer_info.ice_ufrag.clone(),
                                    pwd: offer_info.ice_pwd.clone(),
                                };
                                tracing::info!(
                                    "Starting audio media session with ICE ({} candidates)",
                                    candidates.len()
                                );
                                tokio::spawn(async move {
                                    match calling::media::MediaSession::start_with_ice(
                                        0,
                                        &candidates,
                                        &local_creds,
                                        &remote_creds,
                                        &local_mat,
                                        &remote_mat,
                                    )
                                    .await
                                    {
                                        Ok(session) => {
                                            tracing::info!(
                                                "Audio session started on port {}",
                                                session.local_port().unwrap_or(0)
                                            );
                                            loop {
                                                tokio::time::sleep(std::time::Duration::from_secs(
                                                    5,
                                                ))
                                                .await;
                                                let stats = session.stats().await;
                                                tracing::info!(
                                                    "Audio stats: sent={}, recv={} ({} bytes)",
                                                    stats.packets_sent,
                                                    stats.packets_received,
                                                    stats.bytes_received
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "Failed to start audio session: {:#}",
                                                e
                                            );
                                        }
                                    }
                                });
                            } else {
                                tracing::warn!("No suitable audio ICE candidate found");
                            }
                        } else {
                            tracing::warn!(
                                "Missing audio SRTP keying material, audio session not started"
                            );
                        }

                        // Start video media session if video modality is present.
                        if has_video {
                            if let (Some(local_mat), Some(remote_mat)) =
                                (local_video_crypto, remote_video_crypto)
                            {
                                let vid_candidates =
                                    calling::ice::parse_candidates_from_sdp_section(blob, "video");
                                if let Some(remote_addr) =
                                    calling::ice::select_remote_candidate(&vid_candidates)
                                {
                                    tracing::info!(
                                        "Starting video media session to remote {}",
                                        remote_addr
                                    );
                                    tokio::spawn(async move {
                                        match calling::media::VideoMediaSession::start(
                                            0,
                                            remote_addr,
                                            &local_mat,
                                            &remote_mat,
                                        )
                                        .await
                                        {
                                            Ok(session) => {
                                                tracing::info!(
                                                    "Video session started on port {}",
                                                    session.local_port().unwrap_or(0)
                                                );
                                                loop {
                                                    tokio::time::sleep(
                                                        std::time::Duration::from_secs(5),
                                                    )
                                                    .await;
                                                    let stats = session.stats().await;
                                                    tracing::info!(
                                                        "Video stats: sent={} pkts/{} frames, recv={} pkts/{} frames ({} bytes)",
                                                        stats.packets_sent,
                                                        stats.frames_sent,
                                                        stats.packets_received,
                                                        stats.frames_received,
                                                        stats.bytes_received
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                tracing::warn!(
                                                    "Failed to start video session: {:#}",
                                                    e
                                                );
                                            }
                                        }
                                    });
                                } else {
                                    tracing::warn!("No suitable video ICE candidate found");
                                }
                            } else {
                                tracing::warn!(
                                    "Missing video SRTP keying material, video session not started"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Could not parse SDP offer (may be compressed): {:#}", e);
                        // Still try acceptance without media answer.
                    }
                }
            }
        }
    }

    // Send acceptance.
    if let Err(e) = calling::signaling::accept_call(http, skype_token, &notification).await {
        tracing::warn!("Failed to accept call: {:#}", e);
    }
}
