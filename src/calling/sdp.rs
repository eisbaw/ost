//! Minimal SDP answer generator for audio-only calls.
//!
//! Parses enough of the incoming SDP offer to extract ICE credentials and
//! crypto keys, then generates a minimal SDP answer with PCMU codec.

use anyhow::Result;
use base64::Engine;

/// Extracted fields from the incoming SDP offer.
#[derive(Debug, Clone)]
pub struct SdpOfferInfo {
    pub ice_ufrag: String,
    pub ice_pwd: String,
    /// First crypto line from the offer (we echo it back).
    pub crypto_line: String,
    /// All crypto lines from the offer (for SRTP key extraction).
    pub crypto_lines: Vec<String>,
    /// First candidate address and port from the offer.
    pub candidate_ip: String,
    pub candidate_port: u16,
    /// Video section info (if present in the offer).
    pub video: Option<SdpVideoInfo>,
}

/// Extracted video-specific fields from the SDP offer.
#[derive(Debug, Clone)]
pub struct SdpVideoInfo {
    pub ice_ufrag: String,
    pub ice_pwd: String,
    pub crypto_lines: Vec<String>,
    pub candidate_ip: String,
    pub candidate_port: u16,
}

/// Parse the SDP offer blob to extract ICE credentials, crypto, and candidates.
///
/// The blob may be compressed (auf::compress with raw DEFLATE + base64 encoding).
/// This function automatically decompresses if needed via `sdp_compress::decompress_sdp`.
pub fn parse_sdp_offer(blob: &str) -> Result<SdpOfferInfo> {
    let blob = super::sdp_compress::decompress_sdp(blob).unwrap_or_else(|_| blob.to_string());
    let mut ice_ufrag = String::new();
    let mut ice_pwd = String::new();
    let mut crypto_line = String::new();
    let mut crypto_lines = Vec::new();
    let mut candidate_ip = String::new();
    let mut candidate_port: u16 = 0;

    // Video section fields
    let mut vid_ice_ufrag = String::new();
    let mut vid_ice_pwd = String::new();
    let mut vid_crypto_lines = Vec::new();
    let mut vid_candidate_ip = String::new();
    let mut vid_candidate_port: u16 = 0;
    let mut has_video = false;

    #[derive(PartialEq)]
    enum Section {
        Session,
        Audio,
        Video,
        Other,
    }
    let mut section = Section::Session;

    for line in blob.lines() {
        let line = line.trim();

        if line.starts_with("m=audio") {
            section = Section::Audio;
        } else if line.starts_with("m=video") {
            section = Section::Video;
            has_video = true;
        } else if line.starts_with("m=") {
            section = Section::Other;
        }

        // ICE credentials
        if line.starts_with("a=ice-ufrag:") {
            let val = line.trim_start_matches("a=ice-ufrag:").to_string();
            match section {
                Section::Audio => ice_ufrag = val,
                Section::Video => vid_ice_ufrag = val,
                Section::Session if ice_ufrag.is_empty() => ice_ufrag = val,
                _ => {}
            }
        }
        if line.starts_with("a=ice-pwd:") {
            let val = line.trim_start_matches("a=ice-pwd:").to_string();
            match section {
                Section::Audio => ice_pwd = val,
                Section::Video => vid_ice_pwd = val,
                Section::Session if ice_pwd.is_empty() => ice_pwd = val,
                _ => {}
            }
        }

        // Crypto lines
        if line.starts_with("a=crypto:") || line.starts_with("a=cryptoscale:") {
            if crypto_line.is_empty() {
                crypto_line = line.to_string();
            }
            match section {
                Section::Audio => crypto_lines.push(line.to_string()),
                Section::Video => vid_crypto_lines.push(line.to_string()),
                _ => {}
            }
        }

        // Candidates
        if line.starts_with("a=candidate:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 6 {
                let ip = parts[4].to_string();
                let port: u16 = parts[5].parse().unwrap_or(0);
                match section {
                    Section::Audio if candidate_ip.is_empty() => {
                        candidate_ip = ip;
                        candidate_port = port;
                    }
                    Section::Video if vid_candidate_ip.is_empty() => {
                        vid_candidate_ip = ip;
                        vid_candidate_port = port;
                    }
                    _ => {}
                }
            }
        }
    }

    if ice_ufrag.is_empty() {
        anyhow::bail!("Could not extract ice-ufrag from SDP offer");
    }

    let video = if has_video {
        Some(SdpVideoInfo {
            ice_ufrag: if vid_ice_ufrag.is_empty() {
                ice_ufrag.clone()
            } else {
                vid_ice_ufrag
            },
            ice_pwd: if vid_ice_pwd.is_empty() {
                ice_pwd.clone()
            } else {
                vid_ice_pwd
            },
            crypto_lines: if vid_crypto_lines.is_empty() {
                crypto_lines.clone()
            } else {
                vid_crypto_lines
            },
            candidate_ip: if vid_candidate_ip.is_empty() {
                candidate_ip.clone()
            } else {
                vid_candidate_ip
            },
            candidate_port: if vid_candidate_port == 0 {
                candidate_port
            } else {
                vid_candidate_port
            },
        })
    } else {
        None
    };

    Ok(SdpOfferInfo {
        ice_ufrag,
        ice_pwd,
        crypto_line,
        crypto_lines,
        candidate_ip,
        candidate_port,
        video,
    })
}

/// Generate a minimal SDP answer for audio-only with PCMU codec.
///
/// `local_ip` is the local IP address to put in the SDP.
/// `local_port` is the actual UDP port we are listening on (0 for placeholder).
/// `offer` is the parsed offer info (we echo back ICE credentials and crypto).
pub fn generate_sdp_answer(local_ip: &str, offer: &SdpOfferInfo) -> String {
    generate_sdp_answer_with_port(local_ip, 20000, offer)
}

/// Generate SDP answer with a specific local port.
pub fn generate_sdp_answer_with_port(
    local_ip: &str,
    local_port: u16,
    offer: &SdpOfferInfo,
) -> String {
    let our_ufrag = generate_ice_ufrag();
    let our_pwd = generate_ice_pwd();
    let our_crypto_key = generate_srtp_key();
    let port = local_port;

    let mut sdp = String::new();

    // Session level
    sdp.push_str("v=0\r\n");
    sdp.push_str(&format!("o=- 0 0 IN IP4 {}\r\n", local_ip));
    sdp.push_str("s=session\r\n");
    sdp.push_str(&format!("c=IN IP4 {}\r\n", local_ip));
    sdp.push_str("t=0 0\r\n");

    // Audio media line — PCMU only (payload type 0)
    sdp.push_str(&format!("m=audio {} RTP/SAVP 0\r\n", port));
    sdp.push_str("a=rtpmap:0 PCMU/8000\r\n");
    sdp.push_str("a=sendrecv\r\n");
    sdp.push_str("a=rtcp-mux\r\n");
    sdp.push_str("a=label:main-audio\r\n");
    sdp.push_str("a=x-source:main-audio\r\n");

    // ICE credentials (our own)
    sdp.push_str(&format!("a=ice-ufrag:{}\r\n", our_ufrag));
    sdp.push_str(&format!("a=ice-pwd:{}\r\n", our_pwd));

    // Host candidate
    sdp.push_str(&format!(
        "a=candidate:1 1 UDP 2130706431 {} {} typ host\r\n",
        local_ip, port
    ));

    // SRTP crypto — use standard crypto line
    sdp.push_str(&format!(
        "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31\r\n",
        our_crypto_key
    ));

    sdp
}

/// Generate an SDP answer and return it along with the crypto line we included.
///
/// This is needed so the caller can extract SRTP keying material for the local side.
pub fn generate_sdp_answer_with_crypto(
    local_ip: &str,
    local_port: u16,
    offer: &SdpOfferInfo,
) -> (String, String) {
    let our_ufrag = generate_ice_ufrag();
    let our_pwd = generate_ice_pwd();
    let our_crypto_key = generate_srtp_key();
    let port = local_port;

    let crypto_line = format!(
        "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31",
        our_crypto_key
    );

    let mut sdp = String::new();
    sdp.push_str("v=0\r\n");
    sdp.push_str(&format!("o=- 0 0 IN IP4 {}\r\n", local_ip));
    sdp.push_str("s=session\r\n");
    sdp.push_str(&format!("c=IN IP4 {}\r\n", local_ip));
    sdp.push_str("t=0 0\r\n");
    sdp.push_str(&format!("m=audio {} RTP/SAVP 0\r\n", port));
    sdp.push_str("a=rtpmap:0 PCMU/8000\r\n");
    sdp.push_str("a=ptime:20\r\n");
    sdp.push_str("a=sendrecv\r\n");
    sdp.push_str("a=rtcp-mux\r\n");
    sdp.push_str("a=label:main-audio\r\n");
    sdp.push_str("a=x-source:main-audio\r\n");
    sdp.push_str(&format!("a=ice-ufrag:{}\r\n", our_ufrag));
    sdp.push_str(&format!("a=ice-pwd:{}\r\n", our_pwd));
    sdp.push_str(&format!(
        "a=candidate:1 1 UDP 2130706431 {} {} typ host\r\n",
        local_ip, port
    ));
    sdp.push_str(&crypto_line);
    sdp.push_str("\r\n");

    (sdp, crypto_line)
}

/// Generate a random 4-character ICE ufrag.
pub fn generate_ice_ufrag() -> String {
    use std::fmt::Write;
    let bytes: [u8; 3] = rand_bytes();
    let mut s = String::with_capacity(4);
    for b in &bytes {
        write!(s, "{:02x}", b).unwrap();
    }
    s.truncate(4);
    s
}

/// Generate a random 24-character ICE password using only alphanumeric characters.
///
/// RFC 5245 requires ice-pwd to be 22-256 ice-chars. We use hex encoding
/// to avoid any special characters (`+`, `/`, `=`) that some SDP parsers reject.
pub fn generate_ice_pwd() -> String {
    let bytes: [u8; 12] = rand_bytes();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Generate a random 30-byte base64 SRTP master key.
fn generate_srtp_key() -> String {
    let bytes: [u8; 30] = rand_bytes();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Generate cryptographically secure random bytes via OS CSPRNG.
fn rand_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).expect("OS CSPRNG failed");
    buf
}

/// Generate SDP answer with crypto, including a video section if the offer has video.
///
/// Returns (sdp_answer, audio_crypto_line, video_crypto_line).
/// The video_crypto_line is None if the offer had no video.
pub fn generate_sdp_answer_with_video(
    local_ip: &str,
    audio_port: u16,
    video_port: u16,
    offer: &SdpOfferInfo,
) -> (String, String, Option<String>) {
    let our_ufrag = generate_ice_ufrag();
    let our_pwd = generate_ice_pwd();
    let our_crypto_key = generate_srtp_key();

    let audio_crypto_line = format!(
        "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31",
        our_crypto_key
    );

    let mut sdp = String::new();
    sdp.push_str("v=0\r\n");
    sdp.push_str(&format!("o=- 0 0 IN IP4 {}\r\n", local_ip));
    sdp.push_str("s=session\r\n");
    sdp.push_str(&format!("c=IN IP4 {}\r\n", local_ip));
    sdp.push_str("t=0 0\r\n");

    // Video bandwidth hint at session level (if video present)
    if offer.video.is_some() {
        sdp.push_str("a=x-mediabw:main-video send=2000;recv=2000\r\n");
    }

    // Audio section
    sdp.push_str(&format!("m=audio {} RTP/SAVP 0\r\n", audio_port));
    sdp.push_str("a=rtpmap:0 PCMU/8000\r\n");
    sdp.push_str("a=ptime:20\r\n");
    sdp.push_str("a=sendrecv\r\n");
    sdp.push_str("a=rtcp-mux\r\n");
    sdp.push_str("a=label:main-audio\r\n");
    sdp.push_str("a=x-source:main-audio\r\n");
    sdp.push_str(&format!("a=ice-ufrag:{}\r\n", our_ufrag));
    sdp.push_str(&format!("a=ice-pwd:{}\r\n", our_pwd));
    sdp.push_str(&format!(
        "a=candidate:1 1 UDP 2130706431 {} {} typ host\r\n",
        local_ip, audio_port
    ));
    sdp.push_str(&audio_crypto_line);
    sdp.push_str("\r\n");

    // Video section (if offer includes video)
    let video_crypto_line = if offer.video.is_some() {
        let vid_ufrag = generate_ice_ufrag();
        let vid_pwd = generate_ice_pwd();
        let vid_crypto_key = generate_srtp_key();
        let vid_crypto = format!(
            "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31",
            vid_crypto_key
        );

        sdp.push_str(&format!("m=video {} RTP/SAVP 122 121 123\r\n", video_port));
        sdp.push_str("a=rtpmap:122 X-H264UC/90000\r\n");
        sdp.push_str("a=fmtp:122 packetization-mode=1;mst-mode=NI-TC\r\n");
        sdp.push_str("a=rtpmap:121 x-rtvc1/90000\r\n");
        sdp.push_str("a=rtpmap:123 x-ulpfecuc/90000\r\n");
        sdp.push_str("a=rtcp-fb:* x-message app send:src,x-pli recv:src,x-pli\r\n");
        sdp.push_str("a=rtcp-rsize\r\n");
        sdp.push_str("a=sendrecv\r\n");
        sdp.push_str("a=rtcp-mux\r\n");
        sdp.push_str("a=label:main-video\r\n");
        sdp.push_str("a=x-source:main-video\r\n");
        sdp.push_str(&format!("a=ice-ufrag:{}\r\n", vid_ufrag));
        sdp.push_str(&format!("a=ice-pwd:{}\r\n", vid_pwd));
        sdp.push_str(&format!(
            "a=candidate:1 1 UDP 2130706431 {} {} typ host\r\n",
            local_ip, video_port
        ));
        sdp.push_str(&vid_crypto);
        sdp.push_str("\r\n");

        Some(vid_crypto)
    } else {
        None
    };

    (sdp, audio_crypto_line, video_crypto_line)
}

/// Result of SDP answer generation, including ICE credentials and crypto lines.
#[derive(Debug, Clone)]
pub struct SdpAnswerResult {
    pub sdp: String,
    pub audio_crypto_line: String,
    pub video_crypto_line: Option<String>,
    pub audio_ice_ufrag: String,
    pub audio_ice_pwd: String,
    pub video_ice_ufrag: Option<String>,
    pub video_ice_pwd: Option<String>,
}

/// Generate SDP answer with video support, returning ICE credentials for connectivity checks.
///
/// If `local_candidates` is provided, they are included in the audio section.
/// If `video_candidates` is provided, they are included in the video section.
pub fn generate_sdp_answer_full(
    local_ip: &str,
    audio_port: u16,
    video_port: u16,
    offer: &SdpOfferInfo,
    local_candidates: &[super::ice::IceCandidate],
    video_candidates: &[super::ice::IceCandidate],
) -> SdpAnswerResult {
    let our_ufrag = generate_ice_ufrag();
    let our_pwd = generate_ice_pwd();
    let our_crypto_key = generate_srtp_key();

    let audio_crypto_line = format!(
        "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31",
        our_crypto_key
    );

    let mut sdp = String::new();
    sdp.push_str("v=0\r\n");
    sdp.push_str(&format!("o=- 0 0 IN IP4 {}\r\n", local_ip));
    sdp.push_str("s=session\r\n");
    sdp.push_str(&format!("c=IN IP4 {}\r\n", local_ip));
    sdp.push_str("t=0 0\r\n");

    if offer.video.is_some() {
        sdp.push_str("a=x-mediabw:main-video send=2000;recv=2000\r\n");
    }

    // Audio section
    sdp.push_str(&format!("m=audio {} RTP/SAVP 0\r\n", audio_port));
    sdp.push_str("a=rtpmap:0 PCMU/8000\r\n");
    sdp.push_str("a=ptime:20\r\n");
    sdp.push_str("a=sendrecv\r\n");
    sdp.push_str("a=rtcp-mux\r\n");
    sdp.push_str("a=label:main-audio\r\n");
    sdp.push_str("a=x-source:main-audio\r\n");
    sdp.push_str(&format!("a=ice-ufrag:{}\r\n", our_ufrag));
    sdp.push_str(&format!("a=ice-pwd:{}\r\n", our_pwd));

    // Include gathered candidates
    if local_candidates.is_empty() {
        // Fallback: single host candidate
        sdp.push_str(&format!(
            "a=candidate:1 1 UDP 2130706431 {} {} typ host\r\n",
            local_ip, audio_port
        ));
    } else {
        for c in local_candidates {
            sdp.push_str(&format!("a={}\r\n", c.to_sdp_line()));
        }
    }

    sdp.push_str(&audio_crypto_line);
    sdp.push_str("\r\n");

    // Video section
    let (video_crypto_line, video_ice_ufrag, video_ice_pwd) = if offer.video.is_some() {
        let vid_ufrag = generate_ice_ufrag();
        let vid_pwd = generate_ice_pwd();
        let vid_crypto_key = generate_srtp_key();
        let vid_crypto = format!(
            "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31",
            vid_crypto_key
        );

        sdp.push_str(&format!("m=video {} RTP/SAVP 122 121 123\r\n", video_port));
        sdp.push_str("a=rtpmap:122 X-H264UC/90000\r\n");
        sdp.push_str("a=fmtp:122 packetization-mode=1;mst-mode=NI-TC\r\n");
        sdp.push_str("a=rtpmap:121 x-rtvc1/90000\r\n");
        sdp.push_str("a=rtpmap:123 x-ulpfecuc/90000\r\n");
        sdp.push_str("a=rtcp-fb:* x-message app send:src,x-pli recv:src,x-pli\r\n");
        sdp.push_str("a=rtcp-rsize\r\n");
        sdp.push_str("a=sendrecv\r\n");
        sdp.push_str("a=rtcp-mux\r\n");
        sdp.push_str("a=label:main-video\r\n");
        sdp.push_str("a=x-source:main-video\r\n");
        sdp.push_str(&format!("a=ice-ufrag:{}\r\n", vid_ufrag));
        sdp.push_str(&format!("a=ice-pwd:{}\r\n", vid_pwd));

        if video_candidates.is_empty() {
            sdp.push_str(&format!(
                "a=candidate:1 1 UDP 2130706431 {} {} typ host\r\n",
                local_ip, video_port
            ));
        } else {
            for c in video_candidates {
                sdp.push_str(&format!("a={}\r\n", c.to_sdp_line()));
            }
        }

        sdp.push_str(&vid_crypto);
        sdp.push_str("\r\n");

        (Some(vid_crypto), Some(vid_ufrag), Some(vid_pwd))
    } else {
        (None, None, None)
    };

    SdpAnswerResult {
        sdp,
        audio_crypto_line,
        video_crypto_line,
        audio_ice_ufrag: our_ufrag,
        audio_ice_pwd: our_pwd,
        video_ice_ufrag,
        video_ice_pwd,
    }
}

/// Result of SDP offer generation (for outgoing calls).
#[derive(Debug, Clone)]
pub struct SdpOfferResult {
    pub sdp: String,
    pub crypto_line: String,
    pub ice_ufrag: String,
    pub ice_pwd: String,
}

/// Generate an SDP offer for an outgoing audio-only call (PCMU).
pub fn generate_sdp_offer(
    local_ip: &str,
    local_port: u16,
    ice_ufrag: &str,
    ice_pwd: &str,
    candidates: &[super::ice::IceCandidate],
) -> SdpOfferResult {
    let crypto_key = generate_srtp_key();
    let crypto_line = format!(
        "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31",
        crypto_key
    );

    let mut sdp = String::new();
    sdp.push_str("v=0\r\n");
    sdp.push_str(&format!("o=- 0 0 IN IP4 {}\r\n", local_ip));
    sdp.push_str("s=session\r\n");
    sdp.push_str(&format!("c=IN IP4 {}\r\n", local_ip));
    sdp.push_str("t=0 0\r\n");
    sdp.push_str(&format!("m=audio {} RTP/SAVP 0\r\n", local_port));
    sdp.push_str("a=rtpmap:0 PCMU/8000\r\n");
    sdp.push_str("a=ptime:20\r\n");
    sdp.push_str("a=sendrecv\r\n");
    sdp.push_str("a=rtcp-mux\r\n");
    sdp.push_str("a=label:main-audio\r\n");
    sdp.push_str("a=x-source:main-audio\r\n");
    sdp.push_str(&format!("a=ice-ufrag:{}\r\n", ice_ufrag));
    sdp.push_str(&format!("a=ice-pwd:{}\r\n", ice_pwd));

    if candidates.is_empty() {
        sdp.push_str(&format!(
            "a=candidate:1 1 UDP 2130706431 {} {} typ host\r\n",
            local_ip, local_port
        ));
    } else {
        for c in candidates {
            sdp.push_str(&format!("a={}\r\n", c.to_sdp_line()));
        }
    }

    sdp.push_str(&crypto_line);
    sdp.push_str("\r\n");

    SdpOfferResult {
        sdp,
        crypto_line,
        ice_ufrag: ice_ufrag.to_string(),
        ice_pwd: ice_pwd.to_string(),
    }
}

/// Result of audio+video SDP generation (used for both offer and answer).
#[derive(Debug, Clone)]
pub struct AvSdpResult {
    pub sdp: String,
    pub audio_crypto_line: String,
    pub video_crypto_line: String,
    pub audio_ufrag: String,
    pub audio_pwd: String,
    pub video_ufrag: String,
    pub video_pwd: String,
}

/// Parameters for audio+video SDP generation.
pub struct AvSdpParams<'a> {
    pub local_ip: &'a str,
    pub audio_port: u16,
    pub video_port: u16,
    pub audio_ufrag: &'a str,
    pub audio_pwd: &'a str,
    pub video_ufrag: &'a str,
    pub video_pwd: &'a str,
    pub audio_candidates: &'a [super::ice::IceCandidate],
    pub video_candidates: &'a [super::ice::IceCandidate],
    /// Base SSRC for the video x-ssrc-range attribute.
    pub video_ssrc_base: u32,
    /// SSRC for audio x-ssrc-range attribute.
    pub audio_ssrc: u32,
}

/// Generate an SDP offer with both audio (PCMU) and video (H.264) m-lines.
///
/// Teams uses separate transports per media (no BUNDLE) — each m-line gets its
/// own ICE credentials, port, and SRTP crypto key.
pub fn generate_av_sdp_offer(p: &AvSdpParams) -> AvSdpResult {
    let audio_crypto_key = generate_srtp_key();
    let audio_crypto_line = format!(
        "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31",
        audio_crypto_key
    );
    let video_crypto_key = generate_srtp_key();
    let video_crypto_line = format!(
        "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31",
        video_crypto_key
    );
    let mut sdp = String::new();

    // Session level
    sdp.push_str("v=0\r\n");
    sdp.push_str(&format!("o=- 0 0 IN IP4 {}\r\n", p.local_ip));
    sdp.push_str("s=session\r\n");
    sdp.push_str(&format!("c=IN IP4 {}\r\n", p.local_ip));
    sdp.push_str("b=CT:99980\r\n");
    sdp.push_str("t=0 0\r\n");
    sdp.push_str("a=x-mediabw:main-video send=12000;recv=12000\r\n");

    // Audio m-line
    sdp.push_str(&format!("m=audio {} RTP/SAVP 0\r\n", p.audio_port));
    sdp.push_str(&format!("a=x-ssrc-range:{}-{}\r\n", p.audio_ssrc, p.audio_ssrc));
    sdp.push_str("a=rtcp-fb:* x-message app send:dsh recv:dsh\r\n");
    sdp.push_str("a=rtcp-rsize\r\n");
    sdp.push_str("a=mid:0\r\n");
    sdp.push_str("a=rtpmap:0 PCMU/8000\r\n");
    sdp.push_str("a=ptime:20\r\n");
    sdp.push_str("a=sendrecv\r\n");
    sdp.push_str("a=rtcp-mux\r\n");
    sdp.push_str("a=label:main-audio\r\n");
    sdp.push_str("a=x-source:main-audio\r\n");
    sdp.push_str(&format!("a=ice-ufrag:{}\r\n", p.audio_ufrag));
    sdp.push_str(&format!("a=ice-pwd:{}\r\n", p.audio_pwd));

    if p.audio_candidates.is_empty() {
        sdp.push_str(&format!(
            "a=candidate:1 1 UDP 2130706431 {} {} typ host\r\n",
            p.local_ip, p.audio_port
        ));
    } else {
        for c in p.audio_candidates {
            sdp.push_str(&format!("a={}\r\n", c.to_sdp_line()));
        }
    }

    sdp.push_str(&audio_crypto_line);
    sdp.push_str("\r\n");

    // Video m-line — X-H264UC (Teams proprietary H.264 SVC variant)
    sdp.push_str(&format!("m=video {} RTP/SAVP 122 121 123\r\n", p.video_port));
    sdp.push_str("a=mid:1\r\n");
    sdp.push_str("a=rtpmap:122 X-H264UC/90000\r\n");
    sdp.push_str("a=fmtp:122 packetization-mode=1;mst-mode=NI-TC\r\n");
    sdp.push_str("a=rtpmap:121 x-rtvc1/90000\r\n");
    sdp.push_str("a=rtpmap:123 x-ulpfecuc/90000\r\n");
    sdp.push_str("a=rtcp-fb:* x-message app send:src,x-pli recv:src,x-pli\r\n");
    sdp.push_str("a=rtcp-rsize\r\n");
    sdp.push_str(&format!(
        "a=x-ssrc-range:{}-{}\r\n",
        p.video_ssrc_base,
        p.video_ssrc_base.saturating_add(super::video::VIDEO_SSRC_RANGE_SIZE - 1)
    ));
    sdp.push_str("a=x-caps:121 263:320:240:15.0:250000:1;4359:176:144:15.0:100000:1\r\n");
    sdp.push_str("a=sendrecv\r\n");
    sdp.push_str("a=rtcp-mux\r\n");
    sdp.push_str("a=label:main-video\r\n");
    sdp.push_str("a=x-source:main-video\r\n");
    sdp.push_str(&format!("a=ice-ufrag:{}\r\n", p.video_ufrag));
    sdp.push_str(&format!("a=ice-pwd:{}\r\n", p.video_pwd));

    if p.video_candidates.is_empty() {
        sdp.push_str(&format!(
            "a=candidate:1 1 UDP 2130706431 {} {} typ host\r\n",
            p.local_ip, p.video_port
        ));
    } else {
        for c in p.video_candidates {
            sdp.push_str(&format!("a={}\r\n", c.to_sdp_line()));
        }
    }

    sdp.push_str(&video_crypto_line);
    sdp.push_str("\r\n");

    AvSdpResult {
        sdp,
        audio_crypto_line,
        video_crypto_line,
        audio_ufrag: p.audio_ufrag.to_string(),
        audio_pwd: p.audio_pwd.to_string(),
        video_ufrag: p.video_ufrag.to_string(),
        video_pwd: p.video_pwd.to_string(),
    }
}

/// Generate an SDP answer with both audio and video m-lines (for self-call incoming leg).
///
/// Produces fresh ICE credentials and SRTP keys for both media types.
pub fn generate_av_sdp_answer(
    local_ip: &str,
    audio_port: u16,
    video_port: u16,
    audio_candidates: &[super::ice::IceCandidate],
    video_candidates: &[super::ice::IceCandidate],
) -> AvSdpResult {
    let audio_ufrag = generate_ice_ufrag();
    let audio_pwd = generate_ice_pwd();
    let video_ufrag = generate_ice_ufrag();
    let video_pwd = generate_ice_pwd();

    let video_ssrc_base = super::video::generate_ssrc();
    let audio_ssrc = super::video::generate_ssrc();

    generate_av_sdp_offer(&AvSdpParams {
        local_ip,
        audio_port,
        video_port,
        audio_ufrag: &audio_ufrag,
        audio_pwd: &audio_pwd,
        video_ufrag: &video_ufrag,
        video_pwd: &video_pwd,
        audio_candidates,
        video_candidates,
        video_ssrc_base,
        audio_ssrc,
    })
}

/// Get the local IP address (best effort — falls back to 127.0.0.1).
pub fn get_local_ip() -> String {
    // Try to determine local IP by connecting a UDP socket to a public address.
    // This doesn't send any data, just lets the OS pick the outbound interface.
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sdp_offer_basic() {
        let sdp = "\
v=0\r\n\
o=- 0 0 IN IP4 10.0.0.1\r\n\
s=session\r\n\
c=IN IP4 10.0.0.1\r\n\
t=0 0\r\n\
m=audio 21730 RTP/SAVP 0 8\r\n\
a=ice-ufrag:d3sA\r\n\
a=ice-pwd:somepassword\r\n\
a=candidate:1 1 UDP 2130706431 10.0.0.1 21730 typ host\r\n\
a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:somekey|2^31|1:1\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n";

        let info = parse_sdp_offer(sdp).unwrap();
        assert_eq!(info.ice_ufrag, "d3sA");
        assert_eq!(info.ice_pwd, "somepassword");
        assert_eq!(info.candidate_ip, "10.0.0.1");
        assert_eq!(info.candidate_port, 21730);
        assert!(info.crypto_line.contains("AES_CM_128_HMAC_SHA1_80"));
    }

    #[test]
    fn test_generate_sdp_answer() {
        let offer = SdpOfferInfo {
            ice_ufrag: "abcd".into(),
            ice_pwd: "password".into(),
            crypto_line: "a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:key|2^31".into(),
            crypto_lines: vec!["a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:key|2^31".into()],
            candidate_ip: "10.0.0.1".into(),
            candidate_port: 21730,
            video: None,
        };
        let answer = generate_sdp_answer("192.168.1.100", &offer);
        assert!(answer.contains("v=0"));
        assert!(answer.contains("m=audio"));
        assert!(answer.contains("PCMU/8000"));
        assert!(answer.contains("a=ice-ufrag:"));
        assert!(answer.contains("a=crypto:"));
    }

    #[test]
    fn test_generate_av_sdp_offer() {
        let result = generate_av_sdp_offer(&AvSdpParams {
            local_ip: "192.168.1.100",
            audio_port: 20000,
            video_port: 20002,
            audio_ufrag: "auFr",
            audio_pwd: "audioPassword123456789==",
            video_ufrag: "viFr",
            video_pwd: "videoPassword123456789==",
            audio_candidates: &[],
            video_candidates: &[],
            video_ssrc_base: 1000,
            audio_ssrc: 5555,
        });

        let sdp = &result.sdp;

        // Both m-lines present
        assert!(
            sdp.contains("m=audio 20000 RTP/SAVP 0"),
            "missing audio m-line"
        );
        assert!(
            sdp.contains("m=video 20002 RTP/SAVP 122 121 123"),
            "missing video m-line"
        );

        // Session-level attributes
        assert!(sdp.contains("b=CT:99980"), "missing b=CT:99980");
        assert!(sdp.contains("a=x-mediabw:main-video send=12000;recv=12000"), "missing mediabw");

        // Audio attributes
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000"), "missing PCMU rtpmap");
        assert!(sdp.contains("a=ptime:20"), "missing ptime");
        assert!(sdp.contains("a=x-ssrc-range:5555-5555"), "missing audio x-ssrc-range");
        assert!(sdp.contains("a=rtcp-fb:* x-message app send:dsh recv:dsh"), "missing audio rtcp-fb");

        // Video attributes — X-H264UC
        assert!(
            sdp.contains("a=rtpmap:122 X-H264UC/90000"),
            "missing X-H264UC rtpmap"
        );
        assert!(
            sdp.contains("packetization-mode=1;mst-mode=NI-TC"),
            "missing X-H264UC fmtp"
        );
        assert!(
            sdp.contains("a=x-ssrc-range:1000-1099"),
            "missing video x-ssrc-range"
        );
        assert!(
            sdp.contains("a=x-caps:121"),
            "missing x-caps for PT 121"
        );

        // MID attributes
        let audio_idx = sdp.find("m=audio").unwrap();
        let video_idx = sdp.find("m=video").unwrap();
        let audio_section = &sdp[audio_idx..video_idx];
        let video_section = &sdp[video_idx..];
        assert!(audio_section.contains("a=mid:0"), "audio missing mid");
        assert!(video_section.contains("a=mid:1"), "video missing mid");

        // Separate ICE credentials
        assert!(sdp.contains("a=ice-ufrag:auFr"), "missing audio ufrag");
        assert!(sdp.contains("a=ice-ufrag:viFr"), "missing video ufrag");
        assert!(
            sdp.contains("a=ice-pwd:audioPassword123456789=="),
            "missing audio pwd"
        );
        assert!(
            sdp.contains("a=ice-pwd:videoPassword123456789=="),
            "missing video pwd"
        );

        // Separate crypto lines (different keys)
        assert_ne!(
            result.audio_crypto_line, result.video_crypto_line,
            "audio and video should have different SRTP keys"
        );

        // Crypto lines in correct sections
        assert!(
            audio_section.contains(&result.audio_crypto_line),
            "audio crypto not in audio section"
        );
        assert!(
            video_section.contains(&result.video_crypto_line),
            "video crypto not in video section"
        );

        // rtcp-mux on both
        assert!(
            audio_section.contains("a=rtcp-mux"),
            "audio missing rtcp-mux"
        );
        assert!(
            video_section.contains("a=rtcp-mux"),
            "video missing rtcp-mux"
        );

        // Labels
        assert!(audio_section.contains("a=label:main-audio"));
        assert!(video_section.contains("a=label:main-video"));

        // Session-level bandwidth hint
        assert!(sdp.contains("a=x-mediabw:main-video send=12000;recv=12000"));

        // Result struct fields
        assert_eq!(result.audio_ufrag, "auFr");
        assert_eq!(result.video_ufrag, "viFr");
    }

    #[test]
    fn test_generate_av_sdp_answer_fresh_credentials() {
        let r1 = generate_av_sdp_answer("10.0.0.1", 30000, 30002, &[], &[]);
        let r2 = generate_av_sdp_answer("10.0.0.1", 30000, 30002, &[], &[]);

        // Each call should produce different credentials
        assert_ne!(
            r1.audio_ufrag, r2.audio_ufrag,
            "ufrags should differ between calls"
        );
        assert_ne!(
            r1.audio_crypto_line, r2.audio_crypto_line,
            "crypto should differ"
        );

        // Audio and video within same result should differ
        assert_ne!(
            r1.audio_ufrag, r1.video_ufrag,
            "audio/video ufrag should differ"
        );
        assert_ne!(
            r1.audio_crypto_line, r1.video_crypto_line,
            "audio/video crypto should differ"
        );
    }

    #[test]
    fn test_parse_av_sdp_roundtrip() {
        let offer = generate_av_sdp_offer(&AvSdpParams {
            local_ip: "10.0.0.5",
            audio_port: 40000,
            video_port: 40002,
            audio_ufrag: "aUfr",
            audio_pwd: "aPwd1234567890123456==",
            video_ufrag: "vUfr",
            video_pwd: "vPwd1234567890123456==",
            audio_candidates: &[],
            video_candidates: &[],
            video_ssrc_base: 5000,
            audio_ssrc: 9999,
        });

        let parsed = parse_sdp_offer(&offer.sdp).unwrap();

        // Audio section parsed correctly
        assert_eq!(parsed.ice_ufrag, "aUfr");
        assert_eq!(parsed.ice_pwd, "aPwd1234567890123456==");
        assert_eq!(parsed.candidate_ip, "10.0.0.5");
        assert_eq!(parsed.candidate_port, 40000);
        assert!(!parsed.crypto_lines.is_empty());

        // Video section parsed correctly
        let video = parsed.video.expect("should have video section");
        assert_eq!(video.ice_ufrag, "vUfr");
        assert_eq!(video.ice_pwd, "vPwd1234567890123456==");
        assert_eq!(video.candidate_ip, "10.0.0.5");
        assert_eq!(video.candidate_port, 40002);
        assert!(!video.crypto_lines.is_empty());

        // Video crypto differs from audio crypto
        assert_ne!(parsed.crypto_lines[0], video.crypto_lines[0]);
    }
}
