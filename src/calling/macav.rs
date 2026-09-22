//! macOS A/V bridge — Rust side of the native (AVFoundation/VideoToolbox/SwiftUI) path.
//!
//! Capture and display live in Swift; this module owns the format logic and
//! offline self-tests so both stay unit-testable without hardware:
//! - pixel conversion into planar I420 (the codec/display interchange format)
//! - camera pump: latest-frame slot + fps stats, fed by Swift AVCapture
//! - remote slot: latest decoded frame for the SwiftUI video view
//! - tone echo check + black-frame NAL source (VideoToolbox decode target)
//! - call dry-run: tone -> PCMU -> RTP -> SRTP -> RTP -> PCMU -> echo detect,
//!   plus black IDR -> packetize -> depacketize (no network, no auth)

use anyhow::{bail, Context, Result};
use base64::Engine;

use super::{rtp, srtp, test_tone, video};

/// Planar I420 frame (Y w*h + U w*h/4 + V w*h/4).
#[derive(Clone, Debug)]
pub struct I420Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl I420Frame {
    pub fn expected_len(width: u32, height: u32) -> usize {
        (width as usize) * (height as usize) * 3 / 2
    }
}

/// Pixel formats Swift may push (AVCapture native outputs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixFmt {
    I420,
    Nv12,
    Bgra,
}

impl PixFmt {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "i420" | "yu12" | "yuv420" => Ok(PixFmt::I420),
            "nv12" | "420v" => Ok(PixFmt::Nv12),
            "bgra" | "32bgra" => Ok(PixFmt::Bgra),
            other => bail!("unsupported pixel format: {}", other),
        }
    }
}

/// Convert NV12 (Y + interleaved UV) to planar I420.
pub fn nv12_to_i420(nv12: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
        bail!("nv12 needs even nonzero dims, got {}x{}", width, height);
    }
    let y_size = w * h;
    let uv_size = y_size / 2;
    if nv12.len() < y_size + uv_size {
        bail!(
            "nv12 too small: {} bytes, need {}",
            nv12.len(),
            y_size + uv_size
        );
    }
    let mut out = vec![0u8; y_size + uv_size];
    out[..y_size].copy_from_slice(&nv12[..y_size]);
    let uv = &nv12[y_size..y_size + uv_size];
    let (u_plane, v_plane) = out[y_size..].split_at_mut(uv_size / 2);
    for (i, pair) in uv.chunks_exact(2).enumerate() {
        u_plane[i] = pair[0];
        v_plane[i] = pair[1];
    }
    Ok(out)
}

/// Convert packed BGRA to planar I420 (BT.601, integer math).
pub fn bgra_to_i420(bgra: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
        bail!("bgra needs even nonzero dims, got {}x{}", width, height);
    }
    if bgra.len() < w * h * 4 {
        bail!(
            "bgra too small: {} bytes, need {}",
            bgra.len(),
            w * h * 4
        );
    }
    let y_size = w * h;
    let half_w = w / 2;
    let half_h = h / 2;
    let mut out = vec![0u8; y_size + half_w * half_h * 2];
    let (y_plane, uv) = out.split_at_mut(y_size);
    let (u_plane, v_plane) = uv.split_at_mut(half_w * half_h);

    for row in 0..h {
        for col in 0..w {
            let px = &bgra[(row * w + col) * 4..];
            let (b, g, r) = (px[0] as i32, px[1] as i32, px[2] as i32);
            // BT.601 full-range-ish: Y = (77R + 150G + 29B) >> 8
            let y = ((77 * r + 150 * g + 29 * b) >> 8).clamp(0, 255) as u8;
            y_plane[row * w + col] = y;
            if row % 2 == 0 && col % 2 == 0 {
                // Subsample chroma from the top-left pixel of each 2x2 block.
                let u = (((-43 * r - 85 * g + 128 * b) >> 8) + 128).clamp(0, 255) as u8;
                let v = (((128 * r - 107 * g - 21 * b) >> 8) + 128).clamp(0, 255) as u8;
                u_plane[(row / 2) * half_w + col / 2] = u;
                v_plane[(row / 2) * half_w + col / 2] = v;
            }
        }
    }
    Ok(out)
}

/// Validate + convert one pushed camera buffer into I420.
pub fn convert_to_i420(data: &[u8], width: u32, height: u32, fmt: PixFmt) -> Result<Vec<u8>> {
    match fmt {
        PixFmt::I420 => {
            let need = I420Frame::expected_len(width, height);
            if data.len() < need {
                bail!("i420 too small: {} bytes, need {}", data.len(), need);
            }
            Ok(data[..need].to_vec())
        }
        PixFmt::Nv12 => nv12_to_i420(data, width, height),
        PixFmt::Bgra => bgra_to_i420(data, width, height),
    }
}

// ---------------------------------------------------------------------------
// Camera pump (fed by Swift AVCapture, drained by stats + latest frame)
// ---------------------------------------------------------------------------

/// Snapshot of camera pump counters.
#[derive(Clone, Debug)]
pub struct CameraStats {
    pub running: bool,
    pub width: u32,
    pub height: u32,
    pub fps_want: u32,
    pub frames: u64,
    pub dropped: u64,
    pub fps_actual: f64,
    pub last_bytes: usize,
}

pub struct CameraPump {
    running: bool,
    width: u32,
    height: u32,
    fps_want: u32,
    frames: u64,
    dropped: u64,
    started: Option<std::time::Instant>,
    latest: Option<I420Frame>,
}

impl CameraPump {
    pub fn new() -> Self {
        Self {
            running: false,
            width: 0,
            height: 0,
            fps_want: 0,
            frames: 0,
            dropped: 0,
            started: None,
            latest: None,
        }
    }

    /// Begin a capture run. Resets counters.
    pub fn begin(&mut self, width: u32, height: u32, fps: u32) {
        self.running = true;
        self.width = width;
        self.height = height;
        self.fps_want = fps;
        self.frames = 0;
        self.dropped = 0;
        self.started = Some(std::time::Instant::now());
        self.latest = None;
    }

    pub fn end(&mut self) {
        self.running = false;
        self.latest = None;
    }

    /// Push one frame from Swift. Convert failures count as drops.
    pub fn push(&mut self, data: &[u8], width: u32, height: u32, fmt: PixFmt) {
        if !self.running {
            self.dropped += 1;
            return;
        }
        match convert_to_i420(data, width, height, fmt) {
            Ok(i420) => {
                self.frames += 1;
                self.latest = Some(I420Frame {
                    width,
                    height,
                    data: i420,
                });
            }
            Err(e) => {
                tracing::debug!("camera push dropped: {:#}", e);
                self.dropped += 1;
            }
        }
    }

    pub fn stats(&self) -> CameraStats {
        let elapsed = self.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        CameraStats {
            running: self.running,
            width: self.width,
            height: self.height,
            fps_want: self.fps_want,
            frames: self.frames,
            dropped: self.dropped,
            fps_actual: if elapsed > 0.0 {
                self.frames as f64 / elapsed
            } else {
                0.0
            },
            last_bytes: self.latest.as_ref().map(|f| f.data.len()).unwrap_or(0),
        }
    }

    pub fn latest(&self) -> Option<&I420Frame> {
        self.latest.as_ref()
    }
}

impl Default for CameraPump {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Remote frame slot (decoded video for the SwiftUI view)
// ---------------------------------------------------------------------------

/// Latest decoded remote frame. `take` drains (display owns pacing).
#[derive(Default)]
pub struct RemoteSlot {
    latest: Option<I420Frame>,
}

impl RemoteSlot {
    pub fn new() -> Self {
        Self { latest: None }
    }

    pub fn push(&mut self, frame: I420Frame) {
        self.latest = Some(frame);
    }

    pub fn take(&mut self) -> Option<I420Frame> {
        self.latest.take()
    }

    pub fn has_frame(&self) -> bool {
        self.latest.is_some()
    }
}

// ---------------------------------------------------------------------------
// Deterministic self-tests: tone echo, black iframe, call dry-run
// ---------------------------------------------------------------------------

/// Generate a 1kHz tone over silence and run echo detection on it.
/// Always detects (synthetic, no hardware).
pub fn tone_check() -> test_tone::EchoResult {
    let mut gen = test_tone::ToneGenerator::new();
    let mut samples = vec![0i16; 400];
    for _ in 0..25 {
        samples.extend_from_slice(&gen.next_frame());
    }
    test_tone::detect_echo(&samples, 1000.0, 8000.0)
}

/// Black 176x144 IDR access unit as base64 NALs (no start codes).
/// Swift feeds these to VideoToolbox to prove the decode path.
pub fn black_iframe_b64() -> Vec<String> {
    video::generate_black_iframe()
        .iter()
        .map(|nal| base64::engine::general_purpose::STANDARD.encode(nal))
        .collect()
}

/// Fixed SRTP keying material for the dry-run (NOT for real calls).
fn dry_run_material(tag: u32, fill: u8) -> Result<srtp::SrtpKeyingMaterial> {
    let raw = vec![fill; 30];
    let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
    let line = format!(
        "a=crypto:{} AES_CM_128_HMAC_SHA1_80 inline:{}|2^31",
        tag, b64
    );
    srtp::parse_crypto_line(&line).context("dry-run crypto line")
}

/// Offline call pipeline result (no network, no auth, no hardware).
#[derive(Clone, Debug)]
pub struct DryRunResult {
    pub audio_sent: u32,
    pub audio_received: u32,
    pub echo_detected: bool,
    pub echo_delay_ms: f64,
    pub echo_correlation: f64,
    pub video_packets: usize,
    pub video_nals: usize,
}

/// Run the full media pipeline looped back locally:
/// tone -> PCMU -> RTP -> SRTP protect -> SRTP unprotect -> RTP -> PCMU ->
/// echo detect, plus black IDR -> packetize -> depacketize.
pub fn call_dry_run() -> Result<DryRunResult> {
    // Two SRTP contexts: A protects (local keys), B unprotects (swapped).
    let mat_a = dry_run_material(1, 0x11)?;
    let mat_b = dry_run_material(2, 0x22)?;
    let mut ctx_send = srtp::create_context(&mat_a, &mat_b)?;
    let mut ctx_recv = srtp::create_context(&mat_b, &mat_a)?;

    let ssrc = 0x1122_3344u32;
    let mut gen = test_tone::ToneGenerator::new();
    let mut received_pcm: Vec<i16> = Vec::new();
    let mut audio_sent = 0u32;
    let mut audio_received = 0u32;

    // 25 frames = 500ms of tone (matches tone_check window shape).
    for i in 0..25u32 {
        let frame = gen.next_frame();
        let payload: Vec<u8> = frame.iter().map(|&s| rtp::linear_to_ulaw(s)).collect();
        let rtp_pkt = rtp::encode(rtp::PT_PCMU, i as u16, i * 160, ssrc, &payload);
        let srtp_pkt = srtp::protect(&mut ctx_send, &rtp_pkt)?;
        let rtp_back = srtp::unprotect(&mut ctx_recv, &srtp_pkt)?;
        let decoded = rtp::decode(&rtp_back)?;
        audio_sent += 1;
        if decoded.payload_type == rtp::PT_PCMU && decoded.payload.len() == 160 {
            audio_received += 1;
            received_pcm.extend(decoded.payload.iter().map(|&b| rtp::ulaw_to_linear(b)));
        }
    }

    let echo = test_tone::detect_echo(&received_pcm, 1000.0, 8000.0);

    // Video leg: black IDR through packetizer + depacketizer.
    let nals = video::generate_black_iframe();
    let mut packetizer = video::VideoPacketizer::new(ssrc);
    let packets = packetizer.packetize_frame(&nals);
    let mut depacketizer = video::VideoDepacketizer::new();
    let mut video_nals = 0usize;
    for pkt in &packets {
        if pkt.len() <= rtp::RTP_HEADER_SIZE {
            continue;
        }
        let marker = pkt[1] & 0x80 != 0;
        if depacketizer
            .depacketize(&pkt[rtp::RTP_HEADER_SIZE..], marker)?
            .is_some()
        {
            video_nals += 1;
        }
    }

    Ok(DryRunResult {
        audio_sent,
        audio_received,
        echo_detected: echo.detected,
        echo_delay_ms: echo.delay_ms,
        echo_correlation: echo.correlation_peak,
        video_packets: packets.len(),
        video_nals,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixfmt_parses_avfoundation_names() {
        assert_eq!(PixFmt::parse("i420").unwrap(), PixFmt::I420);
        assert_eq!(PixFmt::parse("420v").unwrap(), PixFmt::Nv12);
        assert_eq!(PixFmt::parse("32BGRA").unwrap(), PixFmt::Bgra);
        assert!(PixFmt::parse("mjpeg").is_err());
    }

    #[test]
    fn nv12_splits_uv_planes() {
        // 2x2: Y=4 bytes, UV interleaved 2 bytes
        let nv12 = vec![10, 11, 12, 13, 100, 150];
        let i420 = nv12_to_i420(&nv12, 2, 2).unwrap();
        assert_eq!(i420.len(), 6);
        assert_eq!(&i420[..4], &[10, 11, 12, 13]);
        assert_eq!(i420[4], 100); // U
        assert_eq!(i420[5], 150); // V
    }

    #[test]
    fn nv12_rejects_odd_or_short() {
        assert!(nv12_to_i420(&[0u8; 6], 3, 2).is_err());
        assert!(nv12_to_i420(&[0u8; 3], 2, 2).is_err());
    }

    #[test]
    fn bgra_black_and_white_map() {
        // 2x2 black then 2x2 white
        let black = vec![0u8, 0, 0, 255].repeat(4);
        let i420 = bgra_to_i420(&black, 2, 2).unwrap();
        assert_eq!(i420.len(), 6);
        assert_eq!(&i420[..4], &[0, 0, 0, 0]);
        assert_eq!(i420[4], 128); // U neutral
        assert_eq!(i420[5], 128); // V neutral

        let white = vec![255u8, 255, 255, 255].repeat(4);
        let i420 = bgra_to_i420(&white, 2, 2).unwrap();
        assert!(i420[..4].iter().all(|&y| y >= 250));
    }

    #[test]
    fn bgra_red_has_warm_chroma() {
        // pure red 2x2: V should exceed U
        let red = vec![0u8, 0, 255, 255].repeat(4);
        let i420 = bgra_to_i420(&red, 2, 2).unwrap();
        assert!(i420[5] > 128, "V={} U={}", i420[5], i420[4]);
        assert!(i420[4] < 128, "V={} U={}", i420[5], i420[4]);
    }

    #[test]
    fn pump_counts_frames_and_drops() {
        let mut pump = CameraPump::new();
        // Push before begin counts as drop.
        pump.push(&[0u8; 6], 2, 2, PixFmt::I420);
        assert_eq!(pump.stats().dropped, 1);

        // begin() resets counters.
        pump.begin(320, 240, 15);
        assert_eq!(pump.stats().dropped, 0);
        let i420 = vec![0u8; 320 * 240 * 3 / 2];
        pump.push(&i420, 320, 240, PixFmt::I420);
        pump.push(&[0u8; 4], 320, 240, PixFmt::I420); // short -> drop
        // NV12 + BGRA convert paths
        pump.push(&vec![0u8; 320 * 240 * 3 / 2], 320, 240, PixFmt::Nv12);
        pump.push(&vec![0u8; 320 * 240 * 4], 320, 240, PixFmt::Bgra);

        let s = pump.stats();
        assert!(s.running);
        assert_eq!(s.frames, 3);
        assert_eq!(s.dropped, 1); // short buffer
        assert_eq!(s.last_bytes, 320 * 240 * 3 / 2);
        assert!(pump.latest().is_some());

        pump.end();
        assert!(!pump.stats().running);
        assert!(pump.latest().is_none());
    }

    #[test]
    fn remote_slot_take_drains() {
        let mut slot = RemoteSlot::new();
        assert!(!slot.has_frame());
        assert!(slot.take().is_none());
        slot.push(I420Frame {
            width: 2,
            height: 2,
            data: vec![1, 2, 3, 4, 5, 6],
        });
        assert!(slot.has_frame());
        let f = slot.take().unwrap();
        assert_eq!(f.width, 2);
        assert_eq!(f.data, vec![1, 2, 3, 4, 5, 6]);
        assert!(!slot.has_frame());
    }

    #[test]
    fn tone_check_detects() {
        let r = tone_check();
        assert!(r.detected, "peak={}", r.correlation_peak);
        assert!(r.correlation_peak.abs() > 0.3);
    }

    #[test]
    fn black_iframe_is_sps_pps_idr() {
        let nals = black_iframe_b64();
        assert_eq!(nals.len(), 3);
        let raw: Vec<Vec<u8>> = nals
            .iter()
            .map(|s| base64::engine::general_purpose::STANDARD.decode(s).unwrap())
            .collect();
        assert_eq!(raw[0][0] & 0x1F, 7); // SPS
        assert_eq!(raw[1][0] & 0x1F, 8); // PPS
        assert_eq!(raw[2][0] & 0x1F, 5); // IDR
    }

    #[test]
    fn dry_run_loops_audio_and_video() {
        let r = call_dry_run().unwrap();
        assert_eq!(r.audio_sent, 25);
        assert_eq!(r.audio_received, 25);
        assert!(r.echo_detected, "corr={}", r.echo_correlation);
        // PACSI + SPS + PPS + prefix + IDR = 5 packets; depacketizer
        // yields PACSI, SPS, PPS, prefix, IDR = 5 NALs.
        assert_eq!(r.video_packets, 5);
        assert_eq!(r.video_nals, 5);
    }
}
