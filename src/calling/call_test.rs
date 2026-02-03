//! Outgoing call test — places a call to the av-test channel via two-phase
//! conversation API (epconv + conversationController).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::calling::{ice, recording, rtcp, rtp, sdp, signaling, srtp, test_tone, video};
#[cfg(feature = "video-capture")]
use crate::calling::{camera, codec, display};
use crate::config::Config;
use crate::trouter::{registrar, session, websocket};

/// Result of a call test.
#[derive(Debug)]
pub struct CallTestResult {
    pub call_placed: bool,
    pub call_accepted: bool,
    pub rejection_reason: Option<String>,
    pub packets_sent: u32,
    pub packets_received: u32,
    pub video_packets_sent: u32,
    pub video_packets_received: u32,
    pub incoming_audio_pkts_sent: u32,
    pub incoming_audio_pkts_recv: u32,
    pub incoming_video_pkts_sent: u32,
    pub incoming_video_pkts_recv: u32,
    pub echo_detected: bool,
    pub echo_delay_ms: f64,
    pub echo_correlation: f64,
}

impl CallTestResult {
    fn failed(rejection_reason: Option<String>) -> Self {
        Self {
            call_placed: true,
            call_accepted: false,
            rejection_reason,
            packets_sent: 0,
            packets_received: 0,
            video_packets_sent: 0,
            video_packets_received: 0,
            incoming_audio_pkts_sent: 0,
            incoming_audio_pkts_recv: 0,
            incoming_video_pkts_sent: 0,
            incoming_video_pkts_recv: 0,
            echo_detected: false,
            echo_delay_ms: 0.0,
            echo_correlation: 0.0,
        }
    }
}

/// Run an outgoing call test.
///
/// Modes:
/// - `echo=true`: Call the Echo / Call Quality Tester bot
/// - `thread_override=Some(id)`: Call a specific 1:1 chat thread
/// - Otherwise: Call the av-test channel (requires TEAMS_AV_TEST_THREAD_ID env var)
pub async fn run_call_test(duration_secs: u64, record: bool, echo: bool, thread_override: Option<String>, use_camera: bool, use_display: bool, tone_mode: bool) -> Result<CallTestResult> {
    let config = Config::load().context("Failed to load config")?;
    let skype_token = config
        .get_skype_token()
        .context("No skype token. Run `teams-cli login` first.")?;
    anyhow::ensure!(
        !skype_token.is_expired(),
        "Skype token expired. Run `teams-cli login`."
    );
    let skype_token_str = &skype_token.token;

    let ic3_token = config
        .get_ic3_token()
        .context("No IC3 token. Run `teams-cli login` first.")?;
    anyhow::ensure!(
        !ic3_token.is_expired(),
        "IC3 token expired. Run `teams-cli login`."
    );
    let ic3_token_str = &ic3_token.token;

    let recorder_token_str = if record {
        let recorder_token = config
            .get_recorder_token()
            .context("No recorder token. Run `teams-cli login` first.")?;
        anyhow::ensure!(
            !recorder_token.is_expired(),
            "Recorder token expired. Run `teams-cli login`."
        );
        Some(recorder_token.token)
    } else {
        None
    };

    let graph_token = config.get_graph_token();
    let region_gtms = config
        .get_region_gtms()
        .context("No region_gtms in config. Run `teams-cli login` first.")?;
    let http = reqwest::Client::new();

    // Extract caller MRI from skype token (JWT)
    let caller_mri = extract_mri_from_skype_token(skype_token_str)
        .context("Cannot extract MRI from skype token")?;
    tracing::info!("Caller MRI: {}", caller_mri);

    // Extract OID from MRI (e.g. "8:orgid:{guid}" -> "{guid}")
    let caller_oid = caller_mri
        .strip_prefix("8:orgid:")
        .context("MRI does not have expected 8:orgid: prefix")?;

    // Determine thread ID and call mode based on options:
    // 1. --echo: call the Echo bot
    // 2. --thread <id>: call a specific 1:1 thread
    // 3. otherwise: channel call via TEAMS_AV_TEST_THREAD_ID env var
    let (thread_id, callee_mri) = if echo {
        (signaling::echo_thread_id(caller_oid), None)
    } else if let Some(ref tid) = thread_override {
        // Extract callee OID from 1:1 thread format: 19:{oid1}_{oid2}@unq.gbl.spaces
        let callee_oid = extract_callee_oid_from_thread(tid, caller_oid);
        let callee_mri = callee_oid.map(|oid| format!("8:orgid:{}", oid));
        (tid.clone(), callee_mri)
    } else {
        let tid = std::env::var("TEAMS_AV_TEST_THREAD_ID").context(
            "TEAMS_AV_TEST_THREAD_ID env var not set. Set it to the thread ID of the av-test channel.",
        )?;
        anyhow::ensure!(!tid.is_empty(), "TEAMS_AV_TEST_THREAD_ID is empty");
        (tid, None)
    };

    // Determine if this is a 1:1 call (echo or thread override with callee)
    let is_1to1_call = echo || callee_mri.is_some();

    // Get tenant ID
    let tenant_id = config
        .tenant_id
        .as_deref()
        .context("No tenant_id in config. Run `teams-cli login` first.")?;

    // Fetch user profile for display name
    let (display_name, mail) = match &graph_token {
        Some(gt) if !gt.is_expired() => {
            let me = fetch_me(&http, &gt.token).await.ok();
            (
                me.as_ref()
                    .and_then(|m| m.display_name.clone())
                    .unwrap_or_else(|| "(unknown)".into()),
                me.as_ref()
                    .and_then(|m| m.mail.clone())
                    .unwrap_or_else(|| "(unknown)".into()),
            )
        }
        _ => {
            tracing::warn!("Graph token expired or missing — skipping profile fetch");
            ("(unknown)".into(), "(unknown)".into())
        }
    };

    println!();
    if echo {
        println!("=== Call Test (Echo / Call Quality Tester) ===");
    } else if callee_mri.is_some() {
        println!("=== Call Test (1:1 Call) ===");
    } else {
        println!("=== Call Test (av-test channel) ===");
    }
    println!("Caller:   {} ({})", display_name, mail);
    println!("Thread:   {}", thread_id);
    if let Some(ref mri) = callee_mri {
        println!("Callee:   {}", mri);
    }
    println!("Duration: {}s", duration_secs);
    if record {
        println!("Record:   enabled");
    }

    // 1. Connect Trouter
    tracing::info!("Negotiating Trouter session...");
    let (trouter_session, epid) = session::negotiate(&http, skype_token_str).await?;
    let session_id =
        session::get_session_id(&http, &trouter_session, skype_token_str, &epid).await?;
    let mut ws = websocket::TrouterSocket::connect(&trouter_session, &session_id, &epid).await?;

    // Wait for handshake
    let frame = ws
        .recv_frame()
        .await?
        .context("WS closed before handshake")?;
    if !frame.starts_with("1::") {
        tracing::warn!("Expected 1:: handshake, got: {}", frame);
    }

    // 2. Register paths
    if let Some(ref reg_url) = trouter_session.registrar_url {
        registrar::register(&http, skype_token_str, reg_url, &trouter_session.surl).await?;
    }

    // 3. Bind UDP sockets for audio and video, gather ICE candidates
    let audio_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let audio_port = audio_socket.local_addr()?.port();
    let video_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let video_port = video_socket.local_addr()?.port();
    let local_ip = sdp::get_local_ip();

    let make_host_candidate = |port: u16| ice::IceCandidate {
        foundation: "1".to_string(),
        component: 1,
        transport: ice::Transport::Udp,
        priority: 2130706431,
        address: local_ip.clone(),
        port,
        candidate_type: ice::CandidateType::Host,
        raddr: None,
        rport: None,
    };

    let mut audio_candidates = vec![make_host_candidate(audio_port)];
    let mut video_candidates = vec![make_host_candidate(video_port)];

    if let Some(srflx) = ice::gather_srflx_candidate(&audio_socket, ice::DEFAULT_STUN_SERVER).await
    {
        tracing::info!(
            "Gathered audio srflx candidate: {}:{}",
            srflx.address,
            srflx.port
        );
        audio_candidates.push(srflx);
    }
    if let Some(srflx) = ice::gather_srflx_candidate(&video_socket, ice::DEFAULT_STUN_SERVER).await
    {
        tracing::info!(
            "Gathered video srflx candidate: {}:{}",
            srflx.address,
            srflx.port
        );
        video_candidates.push(srflx);
    }

    // 4. Generate AV SDP offer (audio + video)
    let our_audio_ufrag = sdp::generate_ice_ufrag();
    let our_audio_pwd = sdp::generate_ice_pwd();
    let our_video_ufrag = sdp::generate_ice_ufrag();
    let our_video_pwd = sdp::generate_ice_pwd();
    // Generate SSRCs early so we can include them in SDP x-ssrc-range attributes.
    let video_ssrc = video::generate_ssrc();
    let audio_ssrc = video::generate_ssrc();
    let offer_result = sdp::generate_av_sdp_offer(&sdp::AvSdpParams {
        local_ip: &local_ip,
        audio_port,
        video_port,
        audio_ufrag: &our_audio_ufrag,
        audio_pwd: &our_audio_pwd,
        video_ufrag: &our_video_ufrag,
        video_pwd: &our_video_pwd,
        audio_candidates: &audio_candidates,
        video_candidates: &video_candidates,
        video_ssrc_base: video_ssrc,
        audio_ssrc,
    });

    tracing::info!(
        "AV SDP offer ({} bytes):\n{}",
        offer_result.sdp.len(),
        offer_result.sdp
    );

    // 5. Derive epconv URL from region_gtms
    let epconv_url =
        derive_epconv_url(&region_gtms).context("Cannot derive epconv URL from region_gtms")?;

    // 6. Two-phase call placement
    let endpoint_id = uuid::Uuid::new_v4().to_string();
    let participant_id = uuid::Uuid::new_v4().to_string();
    let chain_id = uuid::Uuid::new_v4().to_string();
    let message_id = uuid::Uuid::new_v4().to_string();

    let conv_params = signaling::ConversationCallParams {
        ic3_token: ic3_token_str,
        trouter_surl: &trouter_session.surl,
        caller_mri: &caller_mri,
        caller_display_name: &display_name,
        endpoint_id: &endpoint_id,
        participant_id: &participant_id,
        thread_id: &thread_id,
        chain_id: &chain_id,
        message_id: &message_id,
        caller_oid,
        tenant_id,
    };

    // Place the call: 1:1 calls (echo or thread) use single-shot epconv, channel uses two-phase
    let (phase1, _phase2) = if is_1to1_call {
        // 1:1 call: single POST to epconv with SDP
        tracing::info!("Creating 1:1 call (single-shot epconv with SDP)...");
        let (created, joined) = signaling::create_1to1_call(
            &http, &epconv_url, &conv_params, &offer_result.sdp,
        ).await?;
        (created, joined)
    } else {
        // Channel call: two-phase (create conversation, then join with SDP)
        tracing::info!("Phase 1: Creating conversation...");
        let created = signaling::create_conversation(&http, &epconv_url, &conv_params).await?;
        tracing::info!("Phase 1 complete: conversationController = {}", created.conversation_controller);

        tracing::info!("Phase 2: Joining conversation with SDP...");
        let joined = signaling::join_conversation_with_sdp(
            &http, &created.conversation_controller, &conv_params, &offer_result.sdp,
        ).await?;
        tracing::info!("Phase 2 complete. CC active URL: {:?}", joined.cc_active_url);
        (created, joined)
    };
    println!("call_placed=true");

    // 1:1 calls: invite the callee after creating the conversation
    if echo {
        tracing::info!("Inviting Echo bot...");
        signaling::invite_echo_bot(&http, &phase1.conversation_controller, &conv_params).await?;
    } else if let Some(ref mri) = callee_mri {
        tracing::info!("Inviting user {}...", mri);
        signaling::invite_user(&http, &phase1.conversation_controller, &conv_params, mri, use_camera).await?;
    }

    // 7. Wait for mediaAnswer on Trouter
    tracing::info!("Waiting for media answer on Trouter...");
    let acceptance = match wait_for_call_acceptance(&mut ws, Duration::from_secs(30)).await {
        Ok(acc) => {
            if let Some(ref reason) = acc.rejection_reason {
                tracing::warn!("Call rejected: {}", reason);
                println!("call_accepted=false");
                println!("rejection_reason={}", reason);
                return Ok(CallTestResult::failed(Some(reason.clone())));
            }
            acc
        }
        Err(e) => {
            tracing::warn!("No call acceptance received: {:#}", e);
            println!("call_accepted=false");
            return Ok(CallTestResult::failed(None));
        }
    };
    println!("call_accepted=true");

    // 7b. Phase 3: Acknowledge call acceptance and register CC callbacks
    if let Some(ref ack_url) = acceptance.acknowledgement_url {
        if let Err(e) =
            signaling::acknowledge_call_acceptance(&http, ack_url, &conv_params).await
        {
            tracing::warn!("Failed to acknowledge call acceptance: {:#}", e);
        }
    } else {
        tracing::error!("No acknowledgement URL in callAcceptance — call WILL time out (430/10065)");
    }

    if let Some(ref leg_url) = acceptance.call_leg_url {
        if let Err(e) =
            signaling::register_cc_callbacks(&http, leg_url, &conv_params).await
        {
            tracing::warn!("Failed to register CC callbacks: {:#}", e);
        }
    } else {
        tracing::warn!("No callLeg URL in callAcceptance — skipping CC callback registration");
    }

    // 8. Set up media leg
    let mut outgoing_leg = setup_media_leg(
        &offer_result.audio_crypto_line,
        &offer_result.video_crypto_line,
        &our_audio_ufrag,
        &our_audio_pwd,
        &our_video_ufrag,
        &our_video_pwd,
        &acceptance.sdp_blob.clone().unwrap_or_default(),
        audio_socket,
        video_socket,
        "outgoing",
        true,
        video_ssrc,
    )
    .await
    .context("Failed to set up outgoing media leg")?;

    // 9. Initialize audio FIRST — before SDL2 display, which can interfere with
    // audio device enumeration on Linux (PulseAudio/ALSA).
    let recorder = Arc::new(Mutex::new(test_tone::AudioRecorder::new(
        (duration_secs as usize) * 8000,
    )));

    // Open speaker output for received audio (requires --features audio)
    #[cfg(feature = "audio")]
    let (_audio_playback, speaker_tx) = {
        match super::audio::AudioPlayback::start() {
            Some((playback, tx)) => {
                tracing::info!("Audio playback initialized successfully");
                (Some(playback), Some(tx))
            }
            None => {
                tracing::warn!("No audio output device — received audio will not be rendered");
                (None, None)
            }
        }
    };
    #[cfg(not(feature = "audio"))]
    let speaker_tx: Option<std::sync::mpsc::SyncSender<Vec<i16>>> = None;

    // Open microphone capture (requires --features audio, and not --tone)
    #[cfg(feature = "audio")]
    let (_audio_capture, mic_rx) = if !tone_mode {
        match super::audio::AudioCapture::start() {
            Some((cap, rx)) => {
                tracing::info!("Microphone capture started — sending real audio");
                (Some(cap), Some(rx))
            }
            None => {
                tracing::warn!("No audio input device — falling back to 1kHz tone");
                (None, None)
            }
        }
    } else {
        tracing::info!("Tone mode: sending 1kHz test tone");
        (None, None)
    };
    #[cfg(not(feature = "audio"))]
    let mic_rx: Option<std::sync::mpsc::Receiver<Vec<i16>>> = None;

    // 9a. Initialize camera and display AFTER audio (SDL2 can interfere with audio)
    #[cfg(feature = "video-capture")]
    {
        if use_camera {
            match camera::CameraCapture::start(None, 320, 240, 15) {
                Ok((_capture, rx)) => {
                    tracing::info!("Camera capture started (320x240 @ 15fps)");
                    outgoing_leg.camera_rx = Some(rx);
                    // Keep capture handle alive by leaking it (it lives until process exit)
                    std::mem::forget(_capture);
                }
                Err(e) => {
                    tracing::warn!("Failed to start camera: {:#}. Falling back to black frames.", e);
                }
            }
        }
        if use_display {
            match display::VideoDisplay::start("Teams Video - Received") {
                Ok((_display, tx)) => {
                    tracing::info!("Video display window opened");
                    outgoing_leg.display_tx = Some(tx);
                    std::mem::forget(_display);
                }
                Err(e) => {
                    tracing::warn!("Failed to open video display: {:#}", e);
                }
            }
        }
    }
    #[cfg(not(feature = "video-capture"))]
    {
        if use_camera || use_display {
            tracing::warn!("Video capture/display requested but binary was not built with --features video-capture");
        }
    }

    let outgoing_handles =
        spawn_media_leg(outgoing_leg, &caller_mri, Some(recorder.clone()), false, speaker_tx, mic_rx);

    // 8b. Start recording in background after a short delay to let audio establish.
    // Recording is non-blocking: if it fails, the call continues normally.
    let mut recording_handle = None;
    if record {
        if let Some(rec_token) = recorder_token_str {
            let http = http.clone();
            let conversation_controller = phase1.conversation_controller.clone();
            let add_participant_url_override = phase1.add_participant_url.clone();
            let caller_mri = caller_mri.clone();
            let participant_id = participant_id.clone();
            let endpoint_id = endpoint_id.clone();
            let chain_id = chain_id.clone();
            let message_id = message_id.clone();
            let thread_id = thread_id.clone();
            let display_name = display_name.clone();
            let trouter_surl = trouter_session.surl.clone();
            let ic3_token = ic3_token_str.to_string();
            let skype_token = skype_token_str.to_string();
            recording_handle = Some(tokio::spawn(async move {
                // Let audio flow for 3 seconds before injecting the recorder
                tokio::time::sleep(Duration::from_secs(3)).await;
                tracing::info!("Starting call recording (background, after 3s media warm-up)...");
                match recording::start_call_recording(
                    &http,
                    &mut ws,
                    &conversation_controller,
                    &caller_mri,
                    &participant_id,
                    &endpoint_id,
                    &chain_id,
                    &message_id,
                    &thread_id,
                    &display_name,
                    &trouter_surl,
                    &ic3_token,
                    &rec_token,
                    &skype_token,
                    add_participant_url_override.as_deref(),
                )
                .await
                {
                    Ok(session) => {
                        println!("recording=true");
                        Some(session)
                    }
                    Err(e) => {
                        tracing::warn!("Recording flow failed (non-fatal): {:#}", e);
                        None
                    }
                }
            }));
        }
    }

    // 10. Wait for test duration
    tracing::info!("Call active, running for {}s...", duration_secs);
    tokio::time::sleep(Duration::from_secs(duration_secs)).await;

    // 11. Stop media
    outgoing_handles.abort_all();

    // 11b. Stop recording if active
    if let Some(handle) = recording_handle {
        match handle.await {
            Ok(Some(session)) => {
                if let Err(e) = recording::stop_call_recording(&http, &session).await {
                    tracing::warn!("Stop recording failed (non-fatal): {:#}", e);
                }
            }
            Ok(None) => {} // recording never started successfully
            Err(e) => tracing::warn!("Recording task panicked: {:#}", e),
        }
    }

    // 12. End call
    let end_url = acceptance.end_url.clone().or_else(|| {
        acceptance.call_leg_url.as_ref().map(|u| {
            // Insert /end before query string: .../path?q=x -> .../path/end?q=x
            if let Some(idx) = u.find('?') {
                format!("{}/end{}", &u[..idx], &u[idx..])
            } else {
                format!("{}/end", u)
            }
        })
    });
    if let Some(ref url) = end_url {
        end_call_by_url(&http, skype_token_str, url).await.ok();
    } else {
        tracing::warn!("No end URL or call_leg_url available — cannot hang up");
    }

    // 13. Analyze echo and collect stats
    let rec = recorder.lock().await;
    let echo_result = test_tone::detect_echo(rec.samples(), 1000.0, 8000.0);

    // Dump received audio to raw files for offline analysis
    {
        let pcm_path = "/tmp/received_audio.pcm";
        let samples = rec.samples();
        let mut f = std::fs::File::create(pcm_path).ok();
        if let Some(ref mut f) = f {
            use std::io::Write;
            for &s in samples {
                let _ = f.write_all(&s.to_le_bytes());
            }
            tracing::info!("Raw PCM i16 LE saved to {} ({} samples, {:.1}s)",
                pcm_path, samples.len(), samples.len() as f64 / 8000.0);
        }
    }

    let out_ss = outgoing_handles.send_stats.lock().await;
    let out_rs = outgoing_handles.recv_stats.lock().await;
    let out_vid_ss = outgoing_handles.video_send_stats.lock().await;
    let out_vid_rs = outgoing_handles.video_recv_stats.lock().await;

    let result = CallTestResult {
        call_placed: true,
        call_accepted: true,
        rejection_reason: None,
        packets_sent: out_ss.packets_sent,
        packets_received: out_rs.packets_received,
        video_packets_sent: out_vid_ss.packets_sent,
        video_packets_received: out_vid_rs.packets_received,
        incoming_audio_pkts_sent: 0,
        incoming_audio_pkts_recv: 0,
        incoming_video_pkts_sent: 0,
        incoming_video_pkts_recv: 0,
        echo_detected: echo_result.detected,
        echo_delay_ms: echo_result.delay_ms,
        echo_correlation: echo_result.correlation_peak,
    };

    println!("audio_packets_sent={}", result.packets_sent);
    println!("audio_packets_received={}", result.packets_received);
    println!("video_packets_sent={}", result.video_packets_sent);
    println!("video_packets_received={}", result.video_packets_received);
    println!("echo_detected={}", result.echo_detected);
    println!("echo_delay_ms={:.1}", result.echo_delay_ms);
    println!("echo_correlation={:.3}", result.echo_correlation);

    Ok(result)
}

/// Derive the epconv URL from region_gtms.
///
/// Uses calling_conversationServiceUrl directly if available,
/// otherwise falls back to extracting the regional base from potentialCallRequestUrl.
fn derive_epconv_url(region_gtms: &serde_json::Value) -> Option<String> {
    // Allow env var override for testing different regions
    if let Ok(url) = std::env::var("TEAMS_EPCONV_URL") {
        return Some(url);
    }
    // Prefer the explicit conversationServiceUrl from GTMS
    if let Some(url) = region_gtms
        .get("calling_conversationServiceUrl")
        .and_then(|v| v.as_str())
    {
        return Some(url.to_string());
    }

    // Fallback: derive from potentialCallRequestUrl
    let potential_url = region_gtms
        .get("calling_potentialCallRequestUrl")
        .and_then(|v| v.as_str())?;

    if let Some(idx) = potential_url.find("/api/v2/") {
        let base = &potential_url[..idx];
        Some(format!("{}/api/v2/epconv", base))
    } else {
        Some(potential_url.replace("/cc/v1/potentialcall", "/epconv"))
    }
}

/// A prepared media leg with sockets, SRTP contexts, and resolved remote addresses.
struct MediaLeg {
    label: String,
    audio_socket: Arc<tokio::net::UdpSocket>,
    audio_srtp_ctx: Arc<Mutex<srtp::SrtpContext>>,
    audio_remote_addr: std::net::SocketAddr,
    audio_local_pwd: String,
    video_socket: Arc<tokio::net::UdpSocket>,
    video_srtp_ctx: Option<Arc<Mutex<srtp::SrtpContext>>>,
    video_remote_addr: Option<std::net::SocketAddr>,
    video_local_pwd: String,
    /// Video SSRC matching the SDP x-ssrc-range.
    video_ssrc: u32,
    /// Camera frame receiver (when --camera is active).
    #[cfg(feature = "video-capture")]
    camera_rx: Option<std::sync::mpsc::Receiver<camera::YuvFrame>>,
    /// Display frame sender (when --display is active).
    #[cfg(feature = "video-capture")]
    display_tx: Option<std::sync::mpsc::SyncSender<display::DisplayFrame>>,
}

/// Handles to spawned media tasks and shared stats for one leg.
struct MediaLegHandles {
    send_stats: Arc<Mutex<rtcp::RtpSendStats>>,
    recv_stats: Arc<Mutex<rtcp::RtpRecvStats>>,
    video_send_stats: Arc<Mutex<rtcp::RtpSendStats>>,
    video_recv_stats: Arc<Mutex<rtcp::RtpRecvStats>>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl MediaLegHandles {
    fn abort_all(&self) {
        for h in &self.handles {
            h.abort();
        }
    }
}

/// Set up a media leg: parse remote SDP, create SRTP contexts, run ICE checks.
///
/// `local_audio_crypto_line` / `local_video_crypto_line` are the crypto lines from our SDP
/// for this leg. `remote_sdp` is the SDP from the other side (the answer for outgoing,
/// or the notification offer for incoming).
async fn setup_media_leg(
    local_audio_crypto_line: &str,
    local_video_crypto_line: &str,
    local_audio_ufrag: &str,
    local_audio_pwd: &str,
    local_video_ufrag: &str,
    local_video_pwd: &str,
    remote_sdp: &str,
    audio_socket: tokio::net::UdpSocket,
    video_socket: tokio::net::UdpSocket,
    label: &str,
    controlling: bool,
    video_ssrc: u32,
) -> Result<MediaLeg> {
    // Decompress and log the full SDP for debugging
    let decompressed = crate::calling::sdp_compress::decompress_sdp(remote_sdp)
        .unwrap_or_else(|_| remote_sdp.to_string());
    tracing::info!("[{}] Remote SDP ({} bytes):\n{}", label, decompressed.len(), decompressed);

    let remote_offer = sdp::parse_sdp_offer(remote_sdp)
        .with_context(|| format!("Failed to parse {} remote SDP", label))?;

    // Audio SRTP
    let local_material = srtp::parse_crypto_line(local_audio_crypto_line)?;
    let remote_material = remote_offer
        .crypto_lines
        .iter()
        .find_map(|l| srtp::parse_crypto_line(l).ok())
        .with_context(|| format!("No audio crypto line in {} remote SDP", label))?;
    let audio_srtp_ctx = Arc::new(Mutex::new(srtp::create_context(
        &local_material,
        &remote_material,
    )?));

    // Video SRTP
    let video_srtp_ctx = if let Some(ref vid) = remote_offer.video {
        let local_vid_material = srtp::parse_crypto_line(local_video_crypto_line)?;
        let remote_vid_material = vid
            .crypto_lines
            .iter()
            .find_map(|l| srtp::parse_crypto_line(l).ok())
            .with_context(|| format!("No video crypto line in {} remote SDP", label))?;
        Some(Arc::new(Mutex::new(srtp::create_context(
            &local_vid_material,
            &remote_vid_material,
        )?)))
    } else {
        tracing::info!("[{}] Remote SDP has no video section", label);
        None
    };

    // Audio ICE
    let remote_candidates = ice::parse_candidates_from_sdp(remote_sdp);
    let remote_creds = ice::IceCredentials {
        ufrag: remote_offer.ice_ufrag.clone(),
        pwd: remote_offer.ice_pwd.clone(),
    };
    let local_creds = ice::IceCredentials {
        ufrag: local_audio_ufrag.to_string(),
        pwd: local_audio_pwd.to_string(),
    };

    let audio_socket = Arc::new(audio_socket);
    let agent = ice::IceAgent::new(local_creds, remote_creds, controlling);
    let audio_remote_addr = match agent
        .check_connectivity(audio_socket.clone(), &remote_candidates)
        .await
    {
        Ok(result) => {
            tracing::info!(
                "[{}] Audio ICE succeeded: remote={}",
                label,
                result.remote_addr
            );
            result.remote_addr
        }
        Err(e) => {
            tracing::warn!("[{}] Audio ICE failed: {:#}, falling back", label, e);
            ice::select_remote_candidate(&remote_candidates)
                .with_context(|| format!("No fallback audio candidate for {}", label))?
        }
    };

    // Video ICE
    let video_socket = Arc::new(video_socket);
    let video_remote_addr = if let Some(ref vid) = remote_offer.video {
        let vid_candidates = ice::parse_candidates_from_sdp_section(remote_sdp, "video");
        tracing::info!("[{}] Video ICE: {} candidates from SDP, remote video ufrag={}, pwd_len={}",
            label, vid_candidates.len(), vid.ice_ufrag, vid.ice_pwd.len());
        let vid_remote_creds = ice::IceCredentials {
            ufrag: vid.ice_ufrag.clone(),
            pwd: vid.ice_pwd.clone(),
        };
        let vid_local_creds = ice::IceCredentials {
            ufrag: local_video_ufrag.to_string(),
            pwd: local_video_pwd.to_string(),
        };
        let vid_agent = ice::IceAgent::new(vid_local_creds, vid_remote_creds, controlling);
        match vid_agent
            .check_connectivity(video_socket.clone(), &vid_candidates)
            .await
        {
            Ok(result) => {
                tracing::info!(
                    "[{}] Video ICE succeeded: remote={}",
                    label,
                    result.remote_addr
                );
                Some(result.remote_addr)
            }
            Err(e) => {
                tracing::warn!("[{}] Video ICE failed: {:#}, falling back", label, e);
                ice::select_remote_candidate(&vid_candidates)
            }
        }
    } else {
        None
    };

    Ok(MediaLeg {
        label: label.to_string(),
        audio_socket,
        audio_srtp_ctx,
        audio_remote_addr,
        audio_local_pwd: local_audio_pwd.to_string(),
        video_socket,
        video_srtp_ctx,
        video_remote_addr,
        video_local_pwd: local_video_pwd.to_string(),
        video_ssrc,
        #[cfg(feature = "video-capture")]
        camera_rx: None,
        #[cfg(feature = "video-capture")]
        display_tx: None,
    })
}

/// Spawn all media send/recv/rtcp loops for a single leg.
///
/// If `recorder` is Some, received audio is decoded and recorded (for echo detection).
/// If `loopback` is true, received audio is decoded and looped back as the send source
/// instead of generating a test tone — used on the incoming leg for echo verification.
/// Returns handles and shared stat counters.
fn spawn_media_leg(
    mut leg: MediaLeg,
    cname: &str,
    recorder: Option<Arc<Mutex<test_tone::AudioRecorder>>>,
    loopback: bool,
    speaker_tx: Option<std::sync::mpsc::SyncSender<Vec<i16>>>,
    mic_rx: Option<std::sync::mpsc::Receiver<Vec<i16>>>,
) -> MediaLegHandles {
    let label = leg.label.clone();
    let mut handles = Vec::new();

    let ssrc = {
        let id = uuid::Uuid::new_v4();
        let b = id.as_bytes();
        u32::from_be_bytes([b[0], b[1], b[2], b[3]])
    };

    let send_stats = Arc::new(Mutex::new(rtcp::RtpSendStats {
        ssrc,
        ..Default::default()
    }));
    let recv_stats = Arc::new(Mutex::new(rtcp::RtpRecvStats::default()));
    let remote_ssrc = Arc::new(Mutex::new(0u32));

    let video_ssrc = leg.video_ssrc;
    let video_send_stats = Arc::new(Mutex::new(rtcp::RtpSendStats {
        ssrc: video_ssrc,
        ..Default::default()
    }));
    let video_recv_stats = Arc::new(Mutex::new(rtcp::RtpRecvStats::default()));
    let video_remote_ssrc = Arc::new(Mutex::new(0u32));

    // Loopback channel: recv loop sends decoded PCM frames, send loop consumes them.
    let (loopback_tx, loopback_rx) = if loopback {
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<i16>>(64);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Dynamic remote address — updated when we receive STUN checks from a new peer.
    // In Teams, the MCU's actual relay address may differ from the SDP candidate.
    let dynamic_remote_addr = Arc::new(Mutex::new(leg.audio_remote_addr));

    // Audio send loop
    {
        let socket = leg.audio_socket.clone();
        let srtp_ctx = leg.audio_srtp_ctx.clone();
        let send_stats = send_stats.clone();
        let dynamic_remote = dynamic_remote_addr.clone();
        let label = label.clone();
        handles.push(tokio::spawn(async move {
            let mut tone = test_tone::ToneGenerator::new();
            let mut seq: u16 = 0;
            let mut timestamp: u32 = 0;
            let mut interval = tokio::time::interval(Duration::from_millis(20));
            let mut loopback_rx = loopback_rx;
            let mic_rx = mic_rx;

            loop {
                interval.tick().await;

                // Priority: loopback > microphone > 1kHz tone
                let samples = if let Some(ref mut rx) = loopback_rx {
                    match rx.try_recv() {
                        Ok(s) => s,
                        Err(_) => vec![0i16; rtp::SAMPLES_PER_PACKET],
                    }
                } else if let Some(ref rx) = mic_rx {
                    match rx.try_recv() {
                        Ok(s) => s,
                        Err(_) => vec![0i16; rtp::SAMPLES_PER_PACKET],
                    }
                } else {
                    tone.next_frame()
                };

                let mut payload = Vec::with_capacity(160);
                for &s in &samples {
                    payload.push(rtp::linear_to_ulaw(s));
                }
                let rtp_packet = rtp::encode(rtp::PT_PCMU, seq, timestamp, ssrc, &payload);

                let srtp_packet = {
                    let mut ctx = srtp_ctx.lock().await;
                    match srtp::protect(&mut ctx, &rtp_packet) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!("[{}] SRTP protect: {:#}", label, e);
                            continue;
                        }
                    }
                };

                let remote_addr = *dynamic_remote.lock().await;
                if socket.send_to(&srtp_packet, remote_addr).await.is_ok() {
                    let mut s = send_stats.lock().await;
                    s.packets_sent += 1;
                    s.bytes_sent += payload.len() as u32;
                    s.last_rtp_timestamp = timestamp;
                }

                seq = seq.wrapping_add(1);
                timestamp = timestamp.wrapping_add(160);
            }
        }));
    }

    // Audio recv loop
    {
        let socket = leg.audio_socket.clone();
        let srtp_ctx = leg.audio_srtp_ctx.clone();
        let recv_stats = recv_stats.clone();
        let remote_ssrc = remote_ssrc.clone();
        let recorder = recorder.clone();
        let loopback_tx = loopback_tx.clone();
        let local_pwd = leg.audio_local_pwd.clone();
        let label = label.clone();
        let dynamic_remote = dynamic_remote_addr.clone();
        handles.push(tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, from)) => {
                        let data = &buf[..len];

                        if len >= 20 && ice::is_stun_message(data) {
                            if ice::is_stun_request(data) {
                                if let Some(txn_id) = ice::get_transaction_id(data) {
                                    let resp = ice::build_binding_response(
                                        &txn_id,
                                        from,
                                        Some(local_pwd.as_bytes()),
                                    );
                                    let _ = socket.send_to(&resp, from).await;
                                }
                                // Update send target to the address that checked us.
                                // The MCU's actual relay may differ from the SDP candidate.
                                let mut dr = dynamic_remote.lock().await;
                                if *dr != from {
                                    tracing::info!("[{}] Updating remote addr: {} -> {} (peer ICE check)", label, *dr, from);
                                    *dr = from;
                                }
                            }
                            continue;
                        }

                        let srtcp_result = {
                            let mut ctx = srtp_ctx.lock().await;
                            srtp::unprotect_rtcp(&mut ctx, data).ok()
                        };
                        if let Some(rtcp_data) = srtcp_result {
                            tracing::debug!(
                                "[{}] Received SRTCP ({} bytes)",
                                label,
                                rtcp_data.len()
                            );
                            let blocks = rtcp::parse_rtcp(&rtcp_data);
                            let mut rs = recv_stats.lock().await;
                            for block in &blocks {
                                match block {
                                    rtcp::RtcpBlock::SenderReport { ssrc, ntp_timestamp, sender_packet_count, sender_octet_count, .. } => {
                                        tracing::info!(
                                            "[{}] RTCP SR from SSRC={:#010x}: pkt_count={} oct_count={}",
                                            label, ssrc, sender_packet_count, sender_octet_count
                                        );
                                        rs.last_sr_ntp = ((*ntp_timestamp >> 16) & 0xFFFF_FFFF) as u32;
                                        rs.last_sr_recv_time = Some(std::time::Instant::now());
                                    }
                                    _ => {}
                                }
                            }
                            continue;
                        }

                        let srtp_result = {
                            let mut ctx = srtp_ctx.lock().await;
                            srtp::unprotect(&mut ctx, data)
                        };
                        match srtp_result {
                            Ok(rtp_data) => {
                                if let Ok(pkt) = rtp::decode(&rtp_data) {
                                    let mut rs = recv_stats.lock().await;
                                    rs.packets_received += 1;
                                    if pkt.sequence_number as u32 > rs.highest_seq {
                                        rs.highest_seq = pkt.sequence_number as u32;
                                    }
                                    drop(rs);

                                    let mut rssrc = remote_ssrc.lock().await;
                                    if *rssrc == 0 {
                                        *rssrc = pkt.ssrc;
                                        tracing::info!(
                                            "[{}] Remote audio SSRC: {:#010x}",
                                            label,
                                            pkt.ssrc
                                        );
                                    }
                                    drop(rssrc);

                                    // Dump raw PCMU payload to file (before decode)
                                    if let Ok(mut f) = std::fs::OpenOptions::new()
                                        .create(true).append(true)
                                        .open("/tmp/received_audio.ulaw")
                                    {
                                        use std::io::Write;
                                        let _ = f.write_all(&pkt.payload);
                                    }

                                    // Decode PCMU to linear PCM
                                    let samples: Vec<i16> = pkt
                                        .payload
                                        .iter()
                                        .map(|&b| rtp::ulaw_to_linear(b))
                                        .collect();

                                    // Record for echo detection (outgoing leg)
                                    if let Some(ref rec) = recorder {
                                        let mut rec = rec.lock().await;
                                        rec.push_frame(&samples);
                                    }

                                    // Feed speaker output (non-blocking)
                                    if let Some(ref tx) = speaker_tx {
                                        match tx.try_send(samples.clone()) {
                                            Ok(()) => {}
                                            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                                                tracing::debug!("[{}] Speaker channel full, dropping frame", label);
                                            }
                                            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                                                tracing::warn!("[{}] Speaker channel disconnected!", label);
                                            }
                                        }
                                    }

                                    // Feed loopback channel (incoming leg)
                                    if let Some(ref tx) = loopback_tx {
                                        // Drop frames if channel is full (non-blocking)
                                        let _ = tx.try_send(samples);
                                    }
                                }
                            }
                            Err(_) => {
                                tracing::trace!(
                                    "[{}] Cannot decrypt from {} ({} bytes)",
                                    label,
                                    from,
                                    len
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("[{}] UDP recv error: {:#}", label, e);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }));
    }

    // RTCP send loop
    {
        let socket = leg.audio_socket.clone();
        let srtp_ctx = leg.audio_srtp_ctx.clone();
        let send_stats = send_stats.clone();
        let recv_stats = recv_stats.clone();
        let remote_ssrc = remote_ssrc.clone();
        let dynamic_remote = dynamic_remote_addr.clone();
        let cname = cname.to_string();
        let label = label.clone();
        handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.tick().await;
            loop {
                interval.tick().await;
                let ss = send_stats.lock().await;
                let rs = recv_stats.lock().await;
                let rssrc = *remote_ssrc.lock().await;

                let rtcp_packet = if ss.packets_sent > 0 {
                    rtcp::build_sender_report(&ss, &rs, rssrc, &cname)
                } else {
                    rtcp::build_receiver_report(ss.ssrc, &rs, rssrc, &cname)
                };
                drop(ss);
                drop(rs);

                let mut ctx = srtp_ctx.lock().await;
                match srtp::protect_rtcp(&mut ctx, &rtcp_packet) {
                    Ok(srtcp) => {
                        let remote_addr = *dynamic_remote.lock().await;
                        if let Err(e) = socket.send_to(&srtcp, remote_addr).await {
                            tracing::warn!("[{}] Failed to send SRTCP: {:#}", label, e);
                        }
                    }
                    Err(e) => tracing::warn!("[{}] SRTCP protect failed: {:#}", label, e),
                }
            }
        }));
    }

    // Video send loop
    if let (Some(ref vid_srtp), Some(vid_addr)) = (&leg.video_srtp_ctx, leg.video_remote_addr) {
        let socket = leg.video_socket.clone();
        let vid_srtp = vid_srtp.clone();
        let vid_send_stats = video_send_stats.clone();
        let label = label.clone();

        #[cfg(feature = "video-capture")]
        let camera_rx = leg.camera_rx.take();
        #[cfg(not(feature = "video-capture"))]
        let camera_rx: Option<()> = None;

        handles.push(tokio::spawn(async move {
            let mut packetizer = video::VideoPacketizer::new(video_ssrc);
            let mut interval =
                tokio::time::interval(Duration::from_millis(video::FRAME_INTERVAL_MS));

            #[cfg(feature = "video-capture")]
            let mut encoder = camera_rx.as_ref().and_then(|_| {
                match codec::H264Encoder::new(320, 240, 15.0, 256) {
                    Ok(enc) => {
                        tracing::info!("[{}] H.264 encoder initialized (320x240, 256kbps)", label);
                        Some(enc)
                    }
                    Err(e) => {
                        tracing::warn!("[{}] Failed to create H.264 encoder: {:#}", label, e);
                        None
                    }
                }
            });

            tracing::info!(
                "[{}] Video send loop started (SSRC: {:#010x}, camera: {})",
                label,
                video_ssrc,
                if camera_rx.is_some() { "live" } else { "black" },
            );

            loop {
                interval.tick().await;

                // Try camera frame, fall back to black iframe
                let nal_units = {
                    #[cfg(feature = "video-capture")]
                    {
                        if let (Some(ref rx), Some(ref mut enc)) = (&camera_rx, &mut encoder) {
                            match rx.try_recv() {
                                Ok(frame) if frame.width > 0 => {
                                    match enc.encode(&frame.data) {
                                        Ok(nals) if !nals.is_empty() => nals,
                                        Ok(_) => video::generate_black_iframe(),
                                        Err(e) => {
                                            tracing::debug!("[{}] Encode error: {:#}", label, e);
                                            video::generate_black_iframe()
                                        }
                                    }
                                }
                                _ => video::generate_black_iframe(),
                            }
                        } else {
                            video::generate_black_iframe()
                        }
                    }
                    #[cfg(not(feature = "video-capture"))]
                    {
                        let _ = &camera_rx;
                        video::generate_black_iframe()
                    }
                };

                let rtp_packets = packetizer.packetize_frame(&nal_units);

                for rtp_pkt in &rtp_packets {
                    let payload_len = rtp_pkt.len().saturating_sub(rtp::RTP_HEADER_SIZE);
                    let last_ts = if rtp_pkt.len() >= 8 {
                        u32::from_be_bytes([rtp_pkt[4], rtp_pkt[5], rtp_pkt[6], rtp_pkt[7]])
                    } else {
                        0
                    };
                    let srtp_packet = {
                        let mut ctx = vid_srtp.lock().await;
                        match srtp::protect(&mut ctx, rtp_pkt) {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!("[{}] Video SRTP protect: {:#}", label, e);
                                continue;
                            }
                        }
                    };
                    if socket.send_to(&srtp_packet, vid_addr).await.is_ok() {
                        let mut s = vid_send_stats.lock().await;
                        s.packets_sent += 1;
                        s.bytes_sent += payload_len as u32;
                        s.last_rtp_timestamp = last_ts;
                    }
                }
            }
        }));
    }

    // Video recv loop
    if let (Some(ref vid_srtp), Some(_)) = (&leg.video_srtp_ctx, leg.video_remote_addr) {
        let socket = leg.video_socket.clone();
        let vid_srtp = vid_srtp.clone();
        let vid_recv_stats = video_recv_stats.clone();
        let vid_remote_ssrc = video_remote_ssrc.clone();
        let vid_local_pwd = leg.video_local_pwd.clone();
        let label = label.clone();

        #[cfg(feature = "video-capture")]
        let display_tx = leg.display_tx.take();

        handles.push(tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let mut depacketizer = video::VideoDepacketizer::new();

            #[cfg(feature = "video-capture")]
            let mut decoder = display_tx.as_ref().and_then(|_| {
                match codec::H264Decoder::new() {
                    Ok(dec) => {
                        tracing::info!("[{}] H.264 decoder initialized for display", label);
                        Some(dec)
                    }
                    Err(e) => {
                        tracing::warn!("[{}] Failed to create H.264 decoder: {:#}", label, e);
                        None
                    }
                }
            });

            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, from)) => {
                        let data = &buf[..len];

                        if len >= 20 && ice::is_stun_message(data) {
                            if ice::is_stun_request(data) {
                                if let Some(txn_id) = ice::get_transaction_id(data) {
                                    let resp = ice::build_binding_response(
                                        &txn_id,
                                        from,
                                        Some(vid_local_pwd.as_bytes()),
                                    );
                                    let _ = socket.send_to(&resp, from).await;
                                }
                            }
                            continue;
                        }

                        // Try SRTCP unprotect first
                        let srtcp_result = {
                            let mut ctx = vid_srtp.lock().await;
                            srtp::unprotect_rtcp(&mut ctx, data).ok()
                        };
                        if let Some(rtcp_data) = srtcp_result {
                            tracing::debug!(
                                "[{}] Received video SRTCP ({} bytes)",
                                label,
                                rtcp_data.len()
                            );
                            let blocks = rtcp::parse_rtcp(&rtcp_data);
                            let mut rs = vid_recv_stats.lock().await;
                            for block in &blocks {
                                if let rtcp::RtcpBlock::SenderReport { ntp_timestamp, .. } = block {
                                    rs.last_sr_ntp = ((*ntp_timestamp >> 16) & 0xFFFF_FFFF) as u32;
                                    rs.last_sr_recv_time = Some(std::time::Instant::now());
                                }
                            }
                            continue;
                        }

                        let result = {
                            let mut ctx = vid_srtp.lock().await;
                            srtp::unprotect(&mut ctx, data)
                        };
                        match result {
                            Ok(rtp_data) => {
                                if let Ok(pkt) = rtp::decode(&rtp_data) {
                                    let mut rs = vid_recv_stats.lock().await;
                                    rs.packets_received += 1;
                                    if pkt.sequence_number as u32 > rs.highest_seq {
                                        rs.highest_seq = pkt.sequence_number as u32;
                                    }
                                    drop(rs);

                                    let mut rssrc = vid_remote_ssrc.lock().await;
                                    if *rssrc == 0 {
                                        *rssrc = pkt.ssrc;
                                        tracing::info!(
                                            "[{}] Remote video SSRC: {:#010x}",
                                            label,
                                            pkt.ssrc
                                        );
                                    }
                                    drop(rssrc);

                                    // Depacketize and optionally decode + display
                                    let marker = pkt.marker;
                                    match depacketizer.depacketize(&pkt.payload, marker) {
                                        Ok(Some(nal)) => {
                                            #[cfg(feature = "video-capture")]
                                            if let (Some(ref mut dec), Some(ref tx)) = (&mut decoder, &display_tx) {
                                                match dec.decode(&nal) {
                                                    Ok(Some(frame)) => {
                                                        let _ = tx.try_send(display::DisplayFrame {
                                                            width: frame.width,
                                                            height: frame.height,
                                                            data: frame.data,
                                                        });
                                                    }
                                                    Ok(None) => {} // decoder needs more data
                                                    Err(e) => {
                                                        tracing::debug!("[{}] Decode error: {:#}", label, e);
                                                    }
                                                }
                                            }
                                        }
                                        Ok(None) => {} // more fragments needed
                                        Err(e) => {
                                            tracing::debug!("[{}] Depacketize error: {:#}", label, e);
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                tracing::trace!(
                                    "[{}] Cannot decrypt video from {} ({} bytes)",
                                    label,
                                    from,
                                    len
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("[{}] Video UDP recv error: {:#}", label, e);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }));
    }

    // Video RTCP send loop
    if let (Some(ref vid_srtp), Some(vid_addr)) = (&leg.video_srtp_ctx, leg.video_remote_addr) {
        let socket = leg.video_socket.clone();
        let vid_srtp = vid_srtp.clone();
        let vid_send_stats = video_send_stats.clone();
        let vid_recv_stats = video_recv_stats.clone();
        let vid_remote_ssrc = video_remote_ssrc.clone();
        let cname = cname.to_string();
        let label = label.clone();
        handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.tick().await;
            loop {
                interval.tick().await;
                let ss = vid_send_stats.lock().await;
                let rs = vid_recv_stats.lock().await;
                let rssrc = *vid_remote_ssrc.lock().await;

                let rtcp_packet = if ss.packets_sent > 0 {
                    rtcp::build_sender_report(&ss, &rs, rssrc, &cname)
                } else {
                    rtcp::build_receiver_report(ss.ssrc, &rs, rssrc, &cname)
                };
                drop(ss);
                drop(rs);

                let mut ctx = vid_srtp.lock().await;
                match srtp::protect_rtcp(&mut ctx, &rtcp_packet) {
                    Ok(srtcp) => {
                        if let Err(e) = socket.send_to(&srtcp, vid_addr).await {
                            tracing::warn!("[{}] Failed to send video SRTCP: {:#}", label, e);
                        }
                    }
                    Err(e) => tracing::warn!("[{}] Video SRTCP protect failed: {:#}", label, e),
                }
            }
        }));
    }

    MediaLegHandles {
        send_stats,
        recv_stats,
        video_send_stats,
        video_recv_stats,
        handles,
    }
}

/// Response from Trouter when call is accepted.
#[derive(Debug)]
struct CallAcceptanceResponse {
    sdp_blob: Option<String>,
    end_url: Option<String>,
    rejection_reason: Option<String>,
    /// URL to POST acknowledgement to (keeps call alive).
    acknowledgement_url: Option<String>,
    /// URL for the active call leg (used for CC callback registration).
    call_leg_url: Option<String>,
    /// URL for applying channel parameters (video send caps). Used later for video setup.
    #[allow(dead_code)]
    apply_channel_params_url: Option<String>,
}

/// Check a parsed JSON value for callEnd and return a descriptive string.
fn check_call_end(v: &serde_json::Value) -> Option<String> {
    let call_end = v.get("callEnd")?;
    let code = call_end.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
    let sub_code = call_end.get("subCode").and_then(|c| c.as_u64()).unwrap_or(0);
    let phrase = call_end
        .get("phrase")
        .and_then(|s| s.as_str())
        .unwrap_or("unknown");
    let reason = call_end
        .get("reason")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let result_cat = call_end
        .get("resultCategories")
        .and_then(|r| {
            if let Some(arr) = r.as_array() {
                Some(
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                )
            } else {
                r.as_str().map(|s| s.to_string())
            }
        })
        .unwrap_or_default();
    Some(format!(
        "Call ended: {} (code={}, subCode={}, reason={}, categories={})",
        phrase, code, sub_code, reason, result_cat
    ))
}

/// Check a parsed JSON value for sessionRejection and return a descriptive string.
fn check_session_rejection(v: &serde_json::Value) -> Option<String> {
    let rejection = v.get("sessionRejection")?;
    tracing::debug!(
        "Full sessionRejection: {}",
        serde_json::to_string_pretty(rejection).unwrap_or_default()
    );
    let code = rejection.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
    let sub_code = rejection
        .get("subCode")
        .and_then(|c| c.as_u64())
        .unwrap_or(0);
    let phrase = rejection
        .get("phrase")
        .and_then(|s| s.as_str())
        .unwrap_or("unknown");
    // Include resultCategories and diagnosticContext if present
    let result_cat = rejection
        .get("resultCategories")
        .and_then(|r| r.as_str())
        .unwrap_or("");
    let diag = rejection
        .get("diagnosticContext")
        .and_then(|d| d.as_str())
        .unwrap_or("");
    let mut msg = format!(
        "Call rejected: {} (code={}, subCode={})",
        phrase, code, sub_code
    );
    if !result_cat.is_empty() {
        msg.push_str(&format!(" resultCategories={}", result_cat));
    }
    if !diag.is_empty() {
        msg.push_str(&format!(" diag={}", diag));
    }
    Some(msg)
}

/// Try to extract a call-relevant JSON payload from a frame.
///
/// For 3::: frames the payload is wrapped: `3:::{"id":N,"data":{"body":"{...}"}}`
/// where body is stringified JSON. For 5::: frames the JSON is direct.
pub fn extract_call_payload(frame: &str) -> Option<serde_json::Value> {
    let json_str = extract_json_from_frame(frame)?;
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;

    // Check if body is gzip-compressed
    let is_gzip = v
        .pointer("/headers/X-Microsoft-Skype-Content-Encoding")
        .and_then(|h| h.as_str())
        .map(|h| h.eq_ignore_ascii_case("gzip"))
        .unwrap_or(false);

    // 3::: format: body may be at /body or /data/body depending on Trouter version
    if frame.starts_with("3:::") || frame.starts_with("3::") {
        for path in &["/body", "/data/body"] {
            if let Some(body_str) = v.pointer(path).and_then(|b| b.as_str()) {
                // If gzip, decode base64 then decompress
                if is_gzip {
                    if let Some(decompressed) = decompress_gzip_base64(body_str) {
                        if let Ok(inner) = serde_json::from_str::<serde_json::Value>(&decompressed) {
                            return Some(inner);
                        }
                    }
                }
                // Try as plain JSON string
                if let Ok(inner) = serde_json::from_str::<serde_json::Value>(body_str) {
                    return Some(inner);
                }
            }
            if let Some(body_obj) = v.pointer(path) {
                if body_obj.is_object() {
                    return Some(body_obj.clone());
                }
            }
        }
    }

    // 5::: or fallback: use the JSON directly
    Some(v)
}

/// Decode base64 then decompress gzip data.
fn decompress_gzip_base64(b64: &str) -> Option<String> {
    use std::io::Read as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut output = String::new();
    decoder.read_to_string(&mut output).ok()?;
    Some(output)
}

/// Wait on the Trouter WebSocket for a media answer or call acceptance event.
///
/// For channel calls via the conversation API, we expect a mediaAnswer callback
/// on our Trouter path with the remote SDP. We also handle sessionRejection.
async fn wait_for_call_acceptance(
    ws: &mut websocket::TrouterSocket,
    timeout: Duration,
) -> Result<CallAcceptanceResponse> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        tokio::select! {
            frame = ws.recv_frame() => {
                match frame? {
                    Some(text) => {
                        // Respond to heartbeats
                        if text.starts_with("2::") {
                            ws.send_text("2::").await.ok();
                            continue;
                        }

                        // Log all non-heartbeat frames for debugging
                        let truncated: String = text.chars().take(300).collect();
                        tracing::info!("Trouter frame: {}", truncated);

                        // Extract call payload (handles both 3::: and 5::: formats)
                        if let Some(v) = extract_call_payload(&text) {
                            tracing::debug!("Extracted call payload: {}", serde_json::to_string_pretty(&v).unwrap_or_default());

                            // Log bodies of important frames for debugging
                            if text.contains("call/end") || text.contains("conversationEnd") || text.contains("conversationUpdate") {
                                tracing::info!("Frame body [{}]: {}",
                                    if text.contains("call/end") { "call/end" }
                                    else if text.contains("conversationEnd") { "conversationEnd" }
                                    else { "conversationUpdate" },
                                    serde_json::to_string(&v).unwrap_or_default());
                            }

                            // Check for sessionRejection
                            if let Some(reason) = check_session_rejection(&v) {
                                tracing::warn!("{}", reason);
                                return Ok(CallAcceptanceResponse {
                                    sdp_blob: None,
                                    end_url: None,
                                    rejection_reason: Some(reason),
                                    acknowledgement_url: None,
                                    call_leg_url: None,
                                    apply_channel_params_url: None,
                                });
                            }

                            // Check for callEnd (server-side call termination)
                            if let Some(reason) = check_call_end(&v) {
                                tracing::warn!("{}", reason);
                                return Ok(CallAcceptanceResponse {
                                    sdp_blob: None,
                                    end_url: None,
                                    rejection_reason: Some(reason),
                                    acknowledgement_url: None,
                                    call_leg_url: None,
                                    apply_channel_params_url: None,
                                });
                            }

                            // Check for mediaAnswer or callAcceptance with SDP
                            if let Some(blob) = v.pointer("/callAcceptance/mediaContent/blob")
                                .or_else(|| v.pointer("/mediaContent/blob"))
                                .or_else(|| v.pointer("/mediaAnswer/mediaContent/blob"))
                                .and_then(|b| b.as_str())
                            {
                                let links_base = if v.get("callAcceptance").is_some() {
                                    "/callAcceptance/links"
                                } else {
                                    "/links"
                                };
                                let get_link = |name: &str| -> Option<String> {
                                    v.pointer(&format!("{}/{}", links_base, name))
                                        .and_then(|u| u.as_str())
                                        .map(|s| s.to_string())
                                };

                                let end_url = get_link("end");
                                let acknowledgement_url = get_link("acknowledgement");
                                let call_leg_url = get_link("callLeg");
                                let apply_channel_params_url = get_link("applyChannelParameters");

                                tracing::info!("Received media answer/acceptance with SDP ({} bytes)", blob.len());
                                tracing::info!("  acknowledgement_url: {:?}", acknowledgement_url);
                                tracing::info!("  call_leg_url: {:?}", call_leg_url);

                                return Ok(CallAcceptanceResponse {
                                    sdp_blob: Some(blob.to_string()),
                                    end_url,
                                    rejection_reason: None,
                                    acknowledgement_url,
                                    call_leg_url,
                                    apply_channel_params_url,
                                });
                            }

                            // Acceptance without SDP
                            if v.get("callAcceptance").is_some() {
                                tracing::warn!("Call accepted but no SDP in response");
                                return Ok(CallAcceptanceResponse {
                                    sdp_blob: None,
                                    end_url: v.pointer("/callAcceptance/links/end")
                                        .and_then(|u| u.as_str())
                                        .map(|s| s.to_string()),
                                    rejection_reason: Some("Call accepted without SDP — media setup impossible".to_string()),
                                    acknowledgement_url: None,
                                    call_leg_url: None,
                                    apply_channel_params_url: None,
                                });
                            }
                        }

                        let dbg_trunc: String = text.chars().take(200).collect();
                        tracing::debug!("Trouter frame (not call event): {}", dbg_trunc);
                    }
                    None => anyhow::bail!("WebSocket closed while waiting for acceptance"),
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                anyhow::bail!("Timeout waiting for call acceptance ({}s)", timeout.as_secs());
            }
        }
    }
}

/// Extract JSON payload from a socket.io frame string.
///
/// Socket.io frames: "3:::{...}" or "5:::{...}". We anchor to the start
/// to avoid matching `:::{` inside JSON body content.
fn extract_json_from_frame(frame: &str) -> Option<&str> {
    for prefix in &["3:::", "5:::", "3::", "5::"] {
        if let Some(rest) = frame.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

/// End a call by posting to a specific end URL.
async fn end_call_by_url(http: &reqwest::Client, skype_token: &str, end_url: &str) -> Result<()> {
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
    tracing::info!("Call ended ({}): {}", status, body);

    Ok(())
}

/// Extract user MRI (e.g. "8:orgid:<guid>") from a Skype token.
///
/// Skype tokens are JWTs. The payload contains a "skypeid" claim like
/// "orgid:<guid>" which we prefix with "8:" to form the MRI.
fn extract_mri_from_skype_token(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    // Decode JWT payload (base64url, no padding)
    let payload = parts[1];
    let padded = match payload.len() % 4 {
        2 => format!("{}==", payload),
        3 => format!("{}=", payload),
        _ => payload.to_string(),
    };
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(padded.trim_end_matches('='))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(payload))
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;

    // Try "skypeid" field first, then "oid" (Azure AD object ID)
    if let Some(skypeid) = json.get("skypeid").and_then(|v| v.as_str()) {
        // skypeid is like "orgid:<guid>" — prefix with "8:"
        if skypeid.starts_with("orgid:") || skypeid.starts_with("teamsvisitor:") {
            return Some(format!("8:{}", skypeid));
        }
        return Some(skypeid.to_string());
    }

    // Fallback: use oid claim
    if let Some(oid) = json.get("oid").and_then(|v| v.as_str()) {
        return Some(format!("8:orgid:{}", oid));
    }

    None
}

/// Extract the callee's OID from a 1:1 thread ID.
///
/// Thread format: `19:{oid1}_{oid2}@unq.gbl.spaces`
/// Returns the OID that is NOT the caller's OID.
fn extract_callee_oid_from_thread(thread_id: &str, caller_oid: &str) -> Option<String> {
    // Strip prefix "19:" and suffix "@unq.gbl.spaces"
    let inner = thread_id
        .strip_prefix("19:")
        .and_then(|s| s.strip_suffix("@unq.gbl.spaces"))?;

    // Split by underscore to get the two OIDs
    let parts: Vec<&str> = inner.split('_').collect();
    if parts.len() != 2 {
        return None;
    }

    // Return the OID that doesn't match the caller
    if parts[0] == caller_oid {
        Some(parts[1].to_string())
    } else if parts[1] == caller_oid {
        Some(parts[0].to_string())
    } else {
        // Neither OID matches the caller — unexpected but return first non-caller
        tracing::warn!(
            "Thread ID {} doesn't contain caller OID {}",
            thread_id,
            caller_oid
        );
        Some(parts[1].to_string())
    }
}

/// Graph /me response (subset of fields we care about).
#[derive(Debug, Deserialize)]
struct MeResponse {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    mail: Option<String>,
}

/// Fetch the authenticated user's profile from Graph /me.
async fn fetch_me(http: &reqwest::Client, graph_token: &str) -> Result<MeResponse> {
    let resp = http
        .get("https://graph.microsoft.com/v1.0/me")
        .bearer_auth(graph_token)
        .send()
        .await
        .context("Failed to GET /me")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Graph /me returned {}: {}", status, body);
    }
    resp.json().await.context("Failed to parse /me response")
}

/// Write PCM i16 samples to a WAV file (mono, given sample rate).
fn write_wav(path: &str, samples: &[i16], sample_rate: u32) -> anyhow::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    let data_len = (samples.len() * 2) as u32;
    let file_len = 36 + data_len;
    // RIFF header
    f.write_all(b"RIFF")?;
    f.write_all(&file_len.to_le_bytes())?;
    f.write_all(b"WAVE")?;
    // fmt chunk
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;       // chunk size
    f.write_all(&1u16.to_le_bytes())?;         // PCM format
    f.write_all(&1u16.to_le_bytes())?;         // mono
    f.write_all(&sample_rate.to_le_bytes())?;  // sample rate
    let byte_rate = sample_rate * 2;
    f.write_all(&byte_rate.to_le_bytes())?;    // byte rate
    f.write_all(&2u16.to_le_bytes())?;         // block align
    f.write_all(&16u16.to_le_bytes())?;        // bits per sample
    // data chunk
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for &s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}
