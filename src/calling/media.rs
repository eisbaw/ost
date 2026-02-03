//! Media session orchestrator — binds UDP socket, sends/receives SRTP audio.
//!
//! The `MediaSession` ties together RTP encoding, SRTP encryption, and UDP
//! transport into a running audio stream. For now it sends silence (PCMU)
//! and logs received packet statistics.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time;

use super::{ice, rtp, srtp, video};

#[cfg(feature = "audio")]
use super::audio;

/// Statistics for received media packets.
#[derive(Debug, Default)]
pub struct MediaStats {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_received: u64,
    pub last_seq: u16,
    pub last_timestamp: u32,
}

/// A running media session for a single audio stream.
pub struct MediaSession {
    socket: Arc<UdpSocket>,
    remote_addr: SocketAddr,
    srtp_ctx: Arc<Mutex<srtp::SrtpContext>>,
    ssrc: u32,
    stats: Arc<Mutex<MediaStats>>,
    /// Handle to the send task (for shutdown).
    send_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle to the recv task (for shutdown).
    recv_handle: Option<tokio::task::JoinHandle<()>>,
    /// Audio capture handle (kept alive so the stream stays open).
    #[cfg(feature = "audio")]
    _audio_capture: Option<audio::AudioCapture>,
    /// Audio playback handle (kept alive so the stream stays open).
    #[cfg(feature = "audio")]
    _audio_playback: Option<audio::AudioPlayback>,
}

impl MediaSession {
    /// Create and start a new media session with ICE connectivity checks.
    ///
    /// Binds a UDP socket, runs ICE checks against remote candidates, then starts
    /// the send/recv loops to the verified remote address.
    pub async fn start_with_ice(
        local_port: u16,
        remote_candidates: &[ice::IceCandidate],
        local_creds: &ice::IceCredentials,
        remote_creds: &ice::IceCredentials,
        local_material: &srtp::SrtpKeyingMaterial,
        remote_material: &srtp::SrtpKeyingMaterial,
    ) -> Result<Self> {
        let bind_addr = format!("0.0.0.0:{}", local_port);
        let socket = UdpSocket::bind(&bind_addr)
            .await
            .with_context(|| format!("Failed to bind UDP socket on {}", bind_addr))?;

        let local_addr = socket.local_addr()?;
        tracing::info!("Media session bound to {} for ICE checks", local_addr);

        let socket = Arc::new(socket);

        // Run ICE connectivity checks
        let agent = ice::IceAgent::new(
            local_creds.clone(),
            remote_creds.clone(),
            false, // answerer = controlled
        );

        let ice_result = agent
            .check_connectivity(socket.clone(), remote_candidates)
            .await;

        let remote_addr = match ice_result {
            Ok(result) => {
                tracing::info!(
                    "ICE check succeeded: remote={}, mapped={}",
                    result.remote_addr,
                    result.mapped_addr
                );
                result.remote_addr
            }
            Err(e) => {
                tracing::warn!("ICE checks failed ({}), falling back to best candidate", e);
                ice::select_remote_candidate(remote_candidates)
                    .context("No candidate available for fallback")?
            }
        };

        let srtp_ctx = srtp::create_context(local_material, remote_material)?;
        let ssrc = {
            let id = uuid::Uuid::new_v4();
            let bytes = id.as_bytes();
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        };

        let srtp_ctx = Arc::new(Mutex::new(srtp_ctx));
        let stats = Arc::new(Mutex::new(MediaStats::default()));

        // Initialize audio devices (optional — graceful fallback to silence).
        #[cfg(feature = "audio")]
        let (audio_capture, mic_rx) = match audio::AudioCapture::start() {
            Some((cap, rx)) => (Some(cap), Some(rx)),
            None => (None, None),
        };
        #[cfg(not(feature = "audio"))]
        let mic_rx: Option<std::sync::mpsc::Receiver<Vec<i16>>> = None;

        #[cfg(feature = "audio")]
        let (audio_playback, spk_tx) = match audio::AudioPlayback::start() {
            Some((pb, tx)) => (Some(pb), Some(tx)),
            None => (None, None),
        };
        #[cfg(not(feature = "audio"))]
        let spk_tx: Option<std::sync::mpsc::SyncSender<Vec<i16>>> = None;

        let send_handle = {
            let socket = socket.clone();
            let srtp_ctx = srtp_ctx.clone();
            let stats = stats.clone();
            tokio::spawn(send_loop(
                socket,
                remote_addr,
                srtp_ctx,
                ssrc,
                stats,
                mic_rx,
            ))
        };

        let local_pwd = local_creds.pwd.clone();
        let recv_handle = {
            let socket = socket.clone();
            let srtp_ctx = srtp_ctx.clone();
            let stats = stats.clone();
            tokio::spawn(recv_loop_with_stun(
                socket, srtp_ctx, stats, local_pwd, spk_tx,
            ))
        };

        Ok(MediaSession {
            socket,
            remote_addr,
            srtp_ctx,
            ssrc,
            stats,
            send_handle: Some(send_handle),
            recv_handle: Some(recv_handle),
            #[cfg(feature = "audio")]
            _audio_capture: audio_capture,
            #[cfg(feature = "audio")]
            _audio_playback: audio_playback,
        })
    }

    /// Create and start a new media session (no ICE checks, direct send to remote).
    ///
    /// - `local_port`: UDP port to bind on (0 for auto-assign).
    /// - `remote_addr`: Remote peer's RTP address (from ICE candidate selection).
    /// - `local_material`: Our SRTP keying material (from our SDP answer).
    /// - `remote_material`: Remote SRTP keying material (from their SDP offer).
    pub async fn start(
        local_port: u16,
        remote_addr: SocketAddr,
        local_material: &srtp::SrtpKeyingMaterial,
        remote_material: &srtp::SrtpKeyingMaterial,
    ) -> Result<Self> {
        let bind_addr = format!("0.0.0.0:{}", local_port);
        let socket = UdpSocket::bind(&bind_addr)
            .await
            .with_context(|| format!("Failed to bind UDP socket on {}", bind_addr))?;

        let local_addr = socket.local_addr()?;
        tracing::info!(
            "Media session bound to {}, remote: {}",
            local_addr,
            remote_addr
        );

        let srtp_ctx = srtp::create_context(local_material, remote_material)?;

        // Generate a random-ish SSRC from uuid
        let ssrc = {
            let id = uuid::Uuid::new_v4();
            let bytes = id.as_bytes();
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        };

        let socket = Arc::new(socket);
        let srtp_ctx = Arc::new(Mutex::new(srtp_ctx));
        let stats = Arc::new(Mutex::new(MediaStats::default()));

        // Initialize audio devices (optional — graceful fallback to silence).
        #[cfg(feature = "audio")]
        let (audio_capture, mic_rx) = match audio::AudioCapture::start() {
            Some((cap, rx)) => (Some(cap), Some(rx)),
            None => (None, None),
        };
        #[cfg(not(feature = "audio"))]
        let mic_rx: Option<std::sync::mpsc::Receiver<Vec<i16>>> = None;

        #[cfg(feature = "audio")]
        let (audio_playback, spk_tx) = match audio::AudioPlayback::start() {
            Some((pb, tx)) => (Some(pb), Some(tx)),
            None => (None, None),
        };
        #[cfg(not(feature = "audio"))]
        let spk_tx: Option<std::sync::mpsc::SyncSender<Vec<i16>>> = None;

        let send_handle = {
            let socket = socket.clone();
            let srtp_ctx = srtp_ctx.clone();
            let stats = stats.clone();
            tokio::spawn(send_loop(
                socket,
                remote_addr,
                srtp_ctx,
                ssrc,
                stats,
                mic_rx,
            ))
        };

        let recv_handle = {
            let socket = socket.clone();
            let srtp_ctx = srtp_ctx.clone();
            let stats = stats.clone();
            tokio::spawn(recv_loop(socket, srtp_ctx, stats, spk_tx))
        };

        Ok(MediaSession {
            socket,
            remote_addr,
            srtp_ctx,
            ssrc,
            stats,
            send_handle: Some(send_handle),
            recv_handle: Some(recv_handle),
            #[cfg(feature = "audio")]
            _audio_capture: audio_capture,
            #[cfg(feature = "audio")]
            _audio_playback: audio_playback,
        })
    }

    /// Get the local port this session is bound to.
    pub fn local_port(&self) -> Result<u16> {
        Ok(self.socket.local_addr()?.port())
    }

    /// Get current media statistics.
    pub async fn stats(&self) -> MediaStats {
        let s = self.stats.lock().await;
        MediaStats {
            packets_sent: s.packets_sent,
            packets_received: s.packets_received,
            bytes_received: s.bytes_received,
            last_seq: s.last_seq,
            last_timestamp: s.last_timestamp,
        }
    }

    /// Stop the media session.
    pub async fn stop(&mut self) {
        if let Some(h) = self.send_handle.take() {
            h.abort();
            let _ = h.await;
        }
        if let Some(h) = self.recv_handle.take() {
            h.abort();
            let _ = h.await;
        }
        let stats = self.stats.lock().await;
        tracing::info!(
            "Media session stopped. Sent: {}, Received: {} ({} bytes)",
            stats.packets_sent,
            stats.packets_received,
            stats.bytes_received
        );
    }
}

impl Drop for MediaSession {
    fn drop(&mut self) {
        if let Some(h) = self.send_handle.take() {
            h.abort();
        }
        if let Some(h) = self.recv_handle.take() {
            h.abort();
        }
    }
}

/// Send loop: every 20ms, encode audio (or silence) as PCMU, encrypt with SRTP, send.
async fn send_loop(
    socket: Arc<UdpSocket>,
    remote_addr: SocketAddr,
    srtp_ctx: Arc<Mutex<srtp::SrtpContext>>,
    ssrc: u32,
    stats: Arc<Mutex<MediaStats>>,
    mic_rx: Option<std::sync::mpsc::Receiver<Vec<i16>>>,
) {
    let mut seq: u16 = 0;
    let mut timestamp: u32 = 0;
    let mut interval = time::interval(Duration::from_millis(rtp::PACKET_INTERVAL_MS));

    let has_mic = mic_rx.is_some();
    tracing::info!(
        "Media send loop started (SSRC: {:#010x}, mic: {})",
        ssrc,
        if has_mic { "live" } else { "silence" }
    );

    loop {
        interval.tick().await;

        // Try to get a frame from the microphone; fall back to silence.
        let payload = if let Some(ref rx) = mic_rx {
            match rx.try_recv() {
                Ok(samples) => {
                    let mut encoded = Vec::with_capacity(samples.len());
                    for &s in &samples {
                        encoded.push(rtp::linear_to_ulaw(s));
                    }
                    // Pad or truncate to exactly SAMPLES_PER_PACKET.
                    encoded.resize(rtp::SAMPLES_PER_PACKET, 0xFF);
                    encoded
                }
                Err(_) => rtp::silence_payload(),
            }
        } else {
            rtp::silence_payload()
        };
        let rtp_packet = rtp::encode(rtp::PT_PCMU, seq, timestamp, ssrc, &payload);

        let srtp_packet = {
            let mut ctx = srtp_ctx.lock().await;
            match srtp::protect(&mut ctx, &rtp_packet) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("SRTP protect failed: {:#}", e);
                    continue;
                }
            }
        };

        match socket.send_to(&srtp_packet, remote_addr).await {
            Ok(_) => {
                let mut s = stats.lock().await;
                s.packets_sent += 1;
            }
            Err(e) => {
                tracing::warn!("UDP send failed: {:#}", e);
            }
        }

        seq = seq.wrapping_add(1);
        timestamp = timestamp.wrapping_add(rtp::TIMESTAMP_INCREMENT);
    }
}

/// Statistics for received video packets.
#[derive(Debug, Default)]
pub struct VideoStats {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_received: u64,
    pub frames_sent: u64,
    pub frames_received: u64,
}

/// A running media session for a video stream.
pub struct VideoMediaSession {
    socket: Arc<UdpSocket>,
    remote_addr: SocketAddr,
    stats: Arc<Mutex<VideoStats>>,
    send_handle: Option<tokio::task::JoinHandle<()>>,
    recv_handle: Option<tokio::task::JoinHandle<()>>,
}

impl VideoMediaSession {
    /// Create and start a new video media session.
    pub async fn start(
        local_port: u16,
        remote_addr: SocketAddr,
        local_material: &srtp::SrtpKeyingMaterial,
        remote_material: &srtp::SrtpKeyingMaterial,
    ) -> Result<Self> {
        let bind_addr = format!("0.0.0.0:{}", local_port);
        let socket = UdpSocket::bind(&bind_addr)
            .await
            .with_context(|| format!("Failed to bind video UDP socket on {}", bind_addr))?;

        let local_addr = socket.local_addr()?;
        tracing::info!(
            "Video session bound to {}, remote: {}",
            local_addr,
            remote_addr
        );

        let srtp_ctx = srtp::create_context(local_material, remote_material)?;

        let ssrc = {
            let id = uuid::Uuid::new_v4();
            let bytes = id.as_bytes();
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        };

        let socket = Arc::new(socket);
        let srtp_ctx = Arc::new(Mutex::new(srtp_ctx));
        let stats = Arc::new(Mutex::new(VideoStats::default()));

        let send_handle = {
            let socket = socket.clone();
            let srtp_ctx = srtp_ctx.clone();
            let stats = stats.clone();
            tokio::spawn(video_send_loop(socket, remote_addr, srtp_ctx, ssrc, stats))
        };

        let recv_handle = {
            let socket = socket.clone();
            let srtp_ctx = srtp_ctx.clone();
            let stats = stats.clone();
            tokio::spawn(video_recv_loop(socket, srtp_ctx, stats))
        };

        Ok(VideoMediaSession {
            socket,
            remote_addr,
            stats,
            send_handle: Some(send_handle),
            recv_handle: Some(recv_handle),
        })
    }

    /// Get the local port this session is bound to.
    pub fn local_port(&self) -> Result<u16> {
        Ok(self.socket.local_addr()?.port())
    }

    /// Get current video statistics.
    pub async fn stats(&self) -> VideoStats {
        let s = self.stats.lock().await;
        VideoStats {
            packets_sent: s.packets_sent,
            packets_received: s.packets_received,
            bytes_received: s.bytes_received,
            frames_sent: s.frames_sent,
            frames_received: s.frames_received,
        }
    }

    /// Stop the video session.
    pub async fn stop(&mut self) {
        if let Some(h) = self.send_handle.take() {
            h.abort();
            let _ = h.await;
        }
        if let Some(h) = self.recv_handle.take() {
            h.abort();
            let _ = h.await;
        }
        let stats = self.stats.lock().await;
        tracing::info!(
            "Video session stopped. Sent: {} pkts/{} frames, Received: {} pkts/{} frames ({} bytes)",
            stats.packets_sent,
            stats.frames_sent,
            stats.packets_received,
            stats.frames_received,
            stats.bytes_received
        );
    }
}

impl Drop for VideoMediaSession {
    fn drop(&mut self) {
        if let Some(h) = self.send_handle.take() {
            h.abort();
        }
        if let Some(h) = self.recv_handle.take() {
            h.abort();
        }
    }
}

/// Video send loop: periodically send black H.264 I-frames.
async fn video_send_loop(
    socket: Arc<UdpSocket>,
    remote_addr: SocketAddr,
    srtp_ctx: Arc<Mutex<srtp::SrtpContext>>,
    ssrc: u32,
    stats: Arc<Mutex<VideoStats>>,
) {
    let mut packetizer = video::VideoPacketizer::new(ssrc);
    let mut interval = time::interval(Duration::from_millis(video::FRAME_INTERVAL_MS));

    tracing::info!(
        "Video send loop started (SSRC: {:#010x}, 15fps black frames)",
        ssrc
    );

    loop {
        interval.tick().await;

        let nal_units = video::generate_black_iframe();
        let rtp_packets = packetizer.packetize_frame(&nal_units);

        for rtp_pkt in &rtp_packets {
            let srtp_packet = {
                let mut ctx = srtp_ctx.lock().await;
                match srtp::protect(&mut ctx, rtp_pkt) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("Video SRTP protect failed: {:#}", e);
                        continue;
                    }
                }
            };

            match socket.send_to(&srtp_packet, remote_addr).await {
                Ok(_) => {
                    let mut s = stats.lock().await;
                    s.packets_sent += 1;
                }
                Err(e) => {
                    tracing::warn!("Video UDP send failed: {:#}", e);
                }
            }
        }

        let mut s = stats.lock().await;
        s.frames_sent += 1;
    }
}

/// Video receive loop: receive and depacketize H.264 NAL units.
async fn video_recv_loop(
    socket: Arc<UdpSocket>,
    srtp_ctx: Arc<Mutex<srtp::SrtpContext>>,
    stats: Arc<Mutex<VideoStats>>,
) {
    let mut buf = [0u8; 2048];
    let mut depacketizer = video::VideoDepacketizer::new();

    tracing::info!("Video recv loop started");

    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, from)) => {
                let data = &buf[..len];

                if len >= 20 && super::ice::is_stun_response(data) {
                    tracing::debug!("Video: STUN response from {}", from);
                    continue;
                }

                let mut ctx = srtp_ctx.lock().await;
                match srtp::unprotect(&mut ctx, data) {
                    Ok(rtp_data) => {
                        if let Ok(pkt) = rtp::decode(&rtp_data) {
                            let mut s = stats.lock().await;
                            s.packets_received += 1;
                            s.bytes_received += rtp_data.len() as u64;

                            let marker = pkt.marker;
                            drop(s); // release lock before depacketize

                            match depacketizer.depacketize(&pkt.payload, marker) {
                                Ok(Some(nal)) => {
                                    let mut s = stats.lock().await;
                                    s.frames_received = depacketizer.frames_received;
                                    if depacketizer.frames_received % 50 == 1 {
                                        tracing::info!(
                                            "Recv video NAL: type={}, size={} bytes (frames: {})",
                                            nal[0] & 0x1F,
                                            nal.len(),
                                            depacketizer.frames_received
                                        );
                                    }
                                }
                                Ok(None) => {} // more fragments needed
                                Err(e) => {
                                    tracing::debug!("Video depacketize error: {:#}", e);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        tracing::trace!(
                            "Could not decrypt video packet from {} ({} bytes)",
                            from,
                            len
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Video UDP recv error: {:#}", e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Receive loop: receive UDP packets, try to decrypt SRTP, decode and play audio.
async fn recv_loop(
    socket: Arc<UdpSocket>,
    srtp_ctx: Arc<Mutex<srtp::SrtpContext>>,
    stats: Arc<Mutex<MediaStats>>,
    spk_tx: Option<std::sync::mpsc::SyncSender<Vec<i16>>>,
) {
    let mut buf = [0u8; 2048];

    tracing::info!("Media recv loop started");

    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, from)) => {
                let data = &buf[..len];

                // Skip STUN packets
                if len >= 20 && super::ice::is_stun_response(data) {
                    tracing::debug!("Received STUN response from {}", from);
                    continue;
                }

                // Try SRTP decrypt
                let mut ctx = srtp_ctx.lock().await;
                match srtp::unprotect(&mut ctx, data) {
                    Ok(rtp_data) => {
                        if let Ok(pkt) = rtp::decode(&rtp_data) {
                            let mut s = stats.lock().await;
                            s.packets_received += 1;
                            s.bytes_received += rtp_data.len() as u64;
                            s.last_seq = pkt.sequence_number;
                            s.last_timestamp = pkt.timestamp;

                            if s.packets_received % 250 == 1 {
                                tracing::info!(
                                    "Recv audio: seq={}, ts={}, pt={}, payload={} bytes (total: {} pkts)",
                                    pkt.sequence_number,
                                    pkt.timestamp,
                                    pkt.payload_type,
                                    pkt.payload.len(),
                                    s.packets_received
                                );
                            }
                            drop(s);

                            // Decode PCMU and send to speaker.
                            if let Some(ref tx) = spk_tx {
                                let samples: Vec<i16> = pkt
                                    .payload
                                    .iter()
                                    .map(|&b| rtp::ulaw_to_linear(b))
                                    .collect();
                                let _ = tx.try_send(samples);
                            }
                        }
                    }
                    Err(_) => {
                        tracing::trace!("Could not decrypt packet from {} ({} bytes)", from, len);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("UDP recv error: {:#}", e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Receive loop that also responds to incoming STUN binding requests.
///
/// This is used when ICE is active — the peer continues sending STUN checks
/// during the media session and expects responses to keep the path alive.
async fn recv_loop_with_stun(
    socket: Arc<UdpSocket>,
    srtp_ctx: Arc<Mutex<srtp::SrtpContext>>,
    stats: Arc<Mutex<MediaStats>>,
    local_pwd: String,
    spk_tx: Option<std::sync::mpsc::SyncSender<Vec<i16>>>,
) {
    let mut buf = [0u8; 2048];

    tracing::info!("Media recv loop started (STUN-aware)");

    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, from)) => {
                let data = &buf[..len];

                // Handle STUN messages
                if len >= 20 && ice::is_stun_message(data) {
                    if ice::is_stun_request(data) {
                        // Respond to STUN binding request from peer
                        if let Some(txn_id) = ice::get_transaction_id(data) {
                            let response = ice::build_binding_response(
                                &txn_id,
                                from,
                                Some(local_pwd.as_bytes()),
                            );
                            let _ = socket.send_to(&response, from).await;
                            tracing::debug!("Responded to STUN request from {} during media", from);
                        }
                    } else {
                        tracing::debug!("Received STUN response from {}", from);
                    }
                    continue;
                }

                // Try SRTP decrypt
                let mut ctx = srtp_ctx.lock().await;
                match srtp::unprotect(&mut ctx, data) {
                    Ok(rtp_data) => {
                        if let Ok(pkt) = rtp::decode(&rtp_data) {
                            let mut s = stats.lock().await;
                            s.packets_received += 1;
                            s.bytes_received += rtp_data.len() as u64;
                            s.last_seq = pkt.sequence_number;
                            s.last_timestamp = pkt.timestamp;

                            if s.packets_received % 250 == 1 {
                                tracing::info!(
                                    "Recv audio: seq={}, ts={}, pt={}, payload={} bytes (total: {} pkts)",
                                    pkt.sequence_number,
                                    pkt.timestamp,
                                    pkt.payload_type,
                                    pkt.payload.len(),
                                    s.packets_received
                                );
                            }
                            drop(s);

                            // Decode PCMU and send to speaker.
                            if let Some(ref tx) = spk_tx {
                                let samples: Vec<i16> = pkt
                                    .payload
                                    .iter()
                                    .map(|&b| rtp::ulaw_to_linear(b))
                                    .collect();
                                let _ = tx.try_send(samples);
                            }
                        }
                    }
                    Err(_) => {
                        tracing::trace!("Could not decrypt packet from {} ({} bytes)", from, len);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("UDP recv error: {:#}", e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_material() -> srtp::SrtpKeyingMaterial {
        let mut key = [0u8; 16];
        let mut salt = [0u8; 14];
        for i in 0..16 {
            key[i] = (i + 1) as u8;
        }
        for i in 0..14 {
            salt[i] = (i + 17) as u8;
        }
        srtp::SrtpKeyingMaterial {
            master_key: key,
            master_salt: salt,
            tag: 2,
        }
    }

    #[tokio::test]
    async fn test_media_session_binds() {
        // Use a non-routable address so send_loop just drops packets
        let remote: SocketAddr = "192.0.2.1:9999".parse().unwrap();
        let mat = test_material();

        let mut session = MediaSession::start(0, remote, &mat, &mat).await.unwrap();
        let port = session.local_port().unwrap();
        assert!(port > 0);

        // Let it run briefly
        tokio::time::sleep(Duration::from_millis(50)).await;

        let stats = session.stats().await;
        // Should have sent at least 1 packet in 50ms (20ms interval)
        assert!(stats.packets_sent >= 1, "sent: {}", stats.packets_sent);

        session.stop().await;
    }
}
