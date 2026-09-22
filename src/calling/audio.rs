//! Audio capture and playback using cpal.
//!
//! Opens the default input/output devices at 8000 Hz mono i16 (PCMU native rate).
//! If the device doesn't support 8000 Hz, captures/plays at the device's native
//! rate and resamples with simple linear interpolation.
//!
//! Gated behind `#[cfg(feature = "audio")]` — when the feature is off, the
//! public types are not compiled and media.rs falls back to silence mode.

use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SampleRate, StreamConfig};

/// Number of PCM samples per 20ms frame at 8000 Hz.
const FRAME_SAMPLES: usize = 160;

/// Target sample rate for PCMU.
const TARGET_RATE: u32 = 8000;

// ---------------------------------------------------------------------------
// Audio setup lock: every CoreAudio AudioUnit touch (config probe, stream
// build/teardown) serializes here. Observed: concurrent probes against a
// virtual driver (BoomAudio) wedge in a HAL mutex and never return —
// single-threaded the same calls complete. All acquisition is bounded,
// so a wedged HAL degrades to None/empty (caller retries) instead of a
// stuck thread.
// ---------------------------------------------------------------------------

fn audio_setup() -> &'static std::sync::Mutex<()> {
    static S: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(()))
}

/// Acquire the setup lock, waiting at most `budget`. None on timeout.
fn lock_audio_for(budget: Duration) -> Option<std::sync::MutexGuard<'static, ()>> {
    let start = Instant::now();
    loop {
        match audio_setup().try_lock() {
            Ok(g) => return Some(g),
            Err(std::sync::TryLockError::Poisoned(e)) => return Some(e.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if start.elapsed() >= budget {
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// Resampling helpers (public for testing)
// ---------------------------------------------------------------------------

/// Downsample from `src_rate` to `dst_rate` using simple linear interpolation.
///
/// Both rates must be > 0. Returns a new buffer at the target rate.
pub fn resample(samples: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if src_rate == dst_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((samples.len() as f64) / ratio).round() as usize;
    if out_len == 0 {
        return vec![];
    }
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        let frac = pos - idx as f64;
        let s0 = samples[idx.min(samples.len() - 1)] as f64;
        let s1 = samples[(idx + 1).min(samples.len() - 1)] as f64;
        let val = s0 + frac * (s1 - s0);
        out.push(val.round() as i16);
    }
    out
}

// ---------------------------------------------------------------------------
// AudioCapture
// ---------------------------------------------------------------------------

/// Captures audio from the default input device and delivers 160-sample
/// (20ms at 8kHz) frames via a channel.
///
/// The cpal Stream is kept alive on a dedicated OS thread so that
/// `AudioCapture` is Send + Sync (required for MediaSession which lives
/// across await points in tokio::spawn).
pub struct AudioCapture {
    _keep_alive: std::sync::mpsc::Sender<()>,
}

impl AudioCapture {
    /// Try to open the default input device. Returns `None` (with a warning log)
    /// if no device is available or the stream cannot be built.
    ///
    /// The returned `Receiver` yields `Vec<i16>` frames of approximately 160
    /// samples (20ms at 8000 Hz).
    pub fn start() -> Option<(Self, mpsc::Receiver<Vec<i16>>)> {
        Self::start_on(None)
    }

    /// Open the named input device (`None`/empty = system default).
    /// Returns `None` when the named device does not exist.
    pub fn start_on(name: Option<&str>) -> Option<(Self, mpsc::Receiver<Vec<i16>>)> {
        // Resolve before spawning: unknown names and headless fail fast
        // without parking a thread on the setup lock.
        let device = match resolve_device(true, name) {
            Some(d) => d,
            None => {
                tracing::warn!("No audio input device found — mic capture disabled");
                return None;
            }
        };
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<i16>>(50);
        // Channel to keep the stream-owning thread alive; dropping sender kills it.
        let (keep_tx, keep_rx) = mpsc::channel::<()>();

        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started2 = started.clone();

        thread::spawn(move || {
            Self::run_on(device, frame_tx, keep_rx, started2);
        });

        // Poll for setup (replaces the fixed 100ms sleep: slow devices no
        // longer read as missing, wedged ones time out instead of hanging).
        if poll_started(&started, Duration::from_secs(10)) {
            Some((
                AudioCapture {
                    _keep_alive: keep_tx,
                },
                frame_rx,
            ))
        } else {
            None
        }
    }

    /// Stream setup + park on the owning thread. All AudioUnit work runs
    /// under the setup lock; every wait is bounded.
    fn run_on(
        device: Device,
        frame_tx: mpsc::SyncSender<Vec<i16>>,
        keep_rx: mpsc::Receiver<()>,
        started: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let guard = match lock_audio_for(Duration::from_secs(8)) {
            Some(g) => g,
            None => {
                tracing::warn!("Audio input setup timed out waiting for the setup lock");
                return;
            }
        };

        let dev_name = device.name().unwrap_or_else(|_| "unknown".into());
        tracing::info!("Audio input device: {}", dev_name);

        let (config, device_rate) = match pick_config(&device, true) {
            Some(c) => c,
            None => {
                tracing::warn!("Cannot find suitable input config for {}", dev_name);
                return;
            }
        };

        let frame_device_samples = (device_rate as usize * 20) / 1000;
        let need_resample = device_rate != TARGET_RATE;

        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i16>::with_capacity(
            frame_device_samples * 2,
        )));
        let buf2 = buf.clone();

        let stream = match device.build_input_stream(
            &config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                let mut acc = buf2.lock().unwrap();
                acc.extend_from_slice(data);
                while acc.len() >= frame_device_samples {
                    let chunk: Vec<i16> = acc.drain(..frame_device_samples).collect();
                    let frame = if need_resample {
                        resample(&chunk, device_rate, TARGET_RATE)
                    } else {
                        chunk
                    };
                    let _ = frame_tx.try_send(frame);
                }
            },
            move |err| {
                tracing::warn!("Audio input stream error: {}", err);
            },
            None,
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to build audio input stream: {}", e);
                return;
            }
        };

        if let Err(e) = stream.play() {
            tracing::warn!("Failed to start audio input stream: {}", e);
            return;
        }

        tracing::info!(
            "Audio capture started (device {}Hz, target {}Hz)",
            device_rate,
            TARGET_RATE
        );
        started.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(guard);

        // Park this thread; the stream stays alive until keep_rx is dropped.
        let _ = keep_rx.recv();
        // Teardown serializes too (bounded; on timeout the unit leaks
        // rather than wedging the next setup behind it).
        if lock_audio_for(Duration::from_secs(8)).is_none() {
            tracing::warn!("Audio input teardown timed out; leaking the stream");
            std::mem::forget(stream);
        }
    }
}

// ---------------------------------------------------------------------------
// AudioPlayback
// ---------------------------------------------------------------------------

/// Plays audio to the default output device. Accepts 160-sample (20ms at 8kHz)
/// frames via a channel.
///
/// Like AudioCapture, the cpal Stream lives on a dedicated OS thread.
pub struct AudioPlayback {
    _keep_alive: std::sync::mpsc::Sender<()>,
}

impl AudioPlayback {
    /// Try to open the default output device. Returns `None` (with a warning log)
    /// if no device is available or the stream cannot be built.
    ///
    /// Send `Vec<i16>` frames of 160 samples (20ms at 8000 Hz) into the
    /// returned `SyncSender`.
    pub fn start() -> Option<(Self, mpsc::SyncSender<Vec<i16>>)> {
        Self::start_on(None)
    }

    /// Open the named output device (`None`/empty = system default).
    /// Returns `None` when the named device does not exist.
    pub fn start_on(name: Option<&str>) -> Option<(Self, mpsc::SyncSender<Vec<i16>>)> {
        // Resolve before spawning: unknown names and headless fail fast
        // without parking a thread on the setup lock.
        let device = match resolve_device(false, name) {
            Some(d) => d,
            None => {
                tracing::warn!("No audio output device found — speaker playback disabled");
                return None;
            }
        };
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<i16>>(50);
        let (keep_tx, keep_rx) = mpsc::channel::<()>();

        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started2 = started.clone();

        thread::spawn(move || {
            Self::run_on(device, frame_rx, keep_rx, started2);
        });

        if poll_started(&started, Duration::from_secs(10)) {
            Some((
                AudioPlayback {
                    _keep_alive: keep_tx,
                },
                frame_tx,
            ))
        } else {
            None
        }
    }

    /// Stream setup + park on the owning thread. All AudioUnit work runs
    /// under the setup lock; every wait is bounded.
    fn run_on(
        device: Device,
        frame_rx: mpsc::Receiver<Vec<i16>>,
        keep_rx: mpsc::Receiver<()>,
        started: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let guard = match lock_audio_for(Duration::from_secs(8)) {
            Some(g) => g,
            None => {
                tracing::warn!("Audio output setup timed out waiting for the setup lock");
                return;
            }
        };

        let dev_name = device.name().unwrap_or_else(|_| "unknown".into());
        tracing::info!("Audio output device: {}", dev_name);

        let (config, device_rate) = match pick_config(&device, false) {
            Some(c) => c,
            None => {
                tracing::warn!("Cannot find suitable output config for {}", dev_name);
                return;
            }
        };

        let need_resample = device_rate != TARGET_RATE;

        // Ring buffer fed by a feeder thread, drained by the output callback.
        let ring = std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::VecDeque::<i16>::with_capacity(
                (device_rate as usize / 1000) * 200,
            ),
        ));
        let ring2 = ring.clone();

        // Feeder thread: reads frames from channel, resamples, pushes to ring.
        thread::spawn(move || {
            while let Ok(frame) = frame_rx.recv() {
                let samples = if need_resample {
                    resample(&frame, TARGET_RATE, device_rate)
                } else {
                    frame
                };
                let mut r = ring2.lock().unwrap();
                r.extend(samples.iter());
            }
        });

        let stream = match device.build_output_stream(
            &config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                let mut r = ring.lock().unwrap();
                for sample in data.iter_mut() {
                    *sample = r.pop_front().unwrap_or(0);
                }
            },
            move |err| {
                tracing::warn!("Audio output stream error: {}", err);
            },
            None,
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to build audio output stream: {}", e);
                return;
            }
        };

        if let Err(e) = stream.play() {
            tracing::warn!("Failed to start audio output stream: {}", e);
            return;
        }

        tracing::info!(
            "Audio playback started (device {}Hz {}ch, target {}Hz mono)",
            device_rate,
            config.channels,
            TARGET_RATE
        );
        started.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(guard);

        // Park thread; stream stays alive until keep_rx is dropped.
        let _ = keep_rx.recv();
        // Teardown serializes too (bounded; on timeout the unit leaks
        // rather than wedging the next setup behind it).
        if lock_audio_for(Duration::from_secs(8)).is_none() {
            tracing::warn!("Audio output teardown timed out; leaking the stream");
            std::mem::forget(stream);
        }
    }
}

// ---------------------------------------------------------------------------
// Device enumeration + named selection (display names for UI pickers)
// ---------------------------------------------------------------------------

/// Display names of available input devices (empty on headless).
pub fn input_device_names() -> Vec<String> {
    device_names(true)
}

/// Display names of available output devices (empty on headless).
pub fn output_device_names() -> Vec<String> {
    device_names(false)
}

fn device_names(input: bool) -> Vec<String> {
    // Enumeration probes every device's configs (AudioUnit work) — under
    // the lock, bounded; a wedged HAL yields an empty list (caller rescans).
    let _guard = match lock_audio_for(Duration::from_secs(15)) {
        Some(g) => g,
        None => {
            tracing::warn!("Audio enumeration timed out waiting for the setup lock");
            return vec![];
        }
    };
    let host = cpal::default_host();
    let iter = if input {
        host.input_devices()
    } else {
        host.output_devices()
    };
    match iter {
        Ok(devs) => devs.filter_map(|d| d.name().ok()).collect(),
        Err(_) => vec![],
    }
}

/// Resolve an input/output device on the calling thread (under the setup
/// lock, bounded). Unknown names and headless return None fast.
fn resolve_device(input: bool, name: Option<&str>) -> Option<Device> {
    let _guard = lock_audio_for(Duration::from_secs(8))?;
    let host = cpal::default_host();
    pick_device(&host, input, name)
}

/// Poll a setup flag until set or `budget` elapses. Replaces fixed sleeps.
fn poll_started(flag: &std::sync::atomic::AtomicBool, budget: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < budget {
        if flag.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    flag.load(std::sync::atomic::Ordering::SeqCst)
}

/// System default input device name (None on headless).
pub fn default_input_name() -> Option<String> {
    let _guard = lock_audio_for(Duration::from_secs(8))?;
    cpal::default_host()
        .default_input_device()
        .and_then(|d| d.name().ok())
}

/// System default output device name (None on headless).
pub fn default_output_name() -> Option<String> {
    let _guard = lock_audio_for(Duration::from_secs(8))?;
    cpal::default_host()
        .default_output_device()
        .and_then(|d| d.name().ok())
}

/// Resolve a device: exact name match, or the system default when
/// `want` is None/empty. Unknown names return None (never fall back —
/// a stale pick must surface, not silently reroute).
fn pick_device(host: &cpal::Host, input: bool, want: Option<&str>) -> Option<Device> {
    if let Some(name) = want.filter(|n| !n.is_empty()) {
        let iter = if input {
            host.input_devices()
        } else {
            host.output_devices()
        };
        return iter.ok()?.find(|d| d.name().is_ok_and(|n| n == name));
    }
    if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Pick a mono i16 stream config, preferring 8000 Hz but falling back to
/// the device's default rate.
fn pick_config(device: &Device, input: bool) -> Option<(StreamConfig, u32)> {
    // Collect into Vec since input/output iterators are different types.
    let configs: Vec<cpal::SupportedStreamConfigRange> = if input {
        device.supported_input_configs().ok()?.collect()
    } else {
        device.supported_output_configs().ok()?.collect()
    };

    for cfg in &configs {
        if cfg.sample_format() == SampleFormat::I16
            && cfg.channels() == 1
            && cfg.min_sample_rate() <= SampleRate(TARGET_RATE)
            && cfg.max_sample_rate() >= SampleRate(TARGET_RATE)
        {
            let sc = cfg.clone().with_sample_rate(SampleRate(TARGET_RATE));
            return Some((sc.into(), TARGET_RATE));
        }
    }

    // Second pass: any i16 config, use its default/max rate, we'll resample.
    for cfg in &configs {
        if cfg.sample_format() == SampleFormat::I16 {
            // Prefer 48000, then 44100, then max.
            let rate = if cfg.min_sample_rate() <= SampleRate(48000)
                && cfg.max_sample_rate() >= SampleRate(48000)
            {
                48000
            } else if cfg.min_sample_rate() <= SampleRate(44100)
                && cfg.max_sample_rate() >= SampleRate(44100)
            {
                44100
            } else {
                cfg.max_sample_rate().0
            };
            let mut sc: StreamConfig = cfg.clone().with_sample_rate(SampleRate(rate)).into();
            sc.channels = 1;
            return Some((sc, rate));
        }
    }

    // Third pass: any format, we'll force mono i16 and hope for the best.
    if let Some(cfg) = configs.first() {
        let rate = cfg.max_sample_rate().0.min(48000).max(8000);
        let sc = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(rate),
            buffer_size: cpal::BufferSize::Default,
        };
        return Some((sc, rate));
    }

    None
}

// ---------------------------------------------------------------------------
// mic_test — capture N seconds, then play back
// ---------------------------------------------------------------------------

/// Fast device availability probe (no capture). Opens + closes each device.
pub fn audio_probe() -> (bool, bool) {
    let input = AudioCapture::start().is_some();
    let output = AudioPlayback::start().is_some();
    (input, output)
}

/// Summary of a mic capture + playback run (FFI-friendly).
#[derive(Debug)]
pub struct MicTestReport {
    pub frames: usize,
    pub seconds: f64,
    pub peak_db: f64,
    pub played_back: bool,
}

/// Capture `seconds` of microphone audio, then play it back.
///
/// When `vu` is true, prints a VU meter bar every 100ms during capture.
/// Returns a report (empty-input error if no mic, playback skipped if no
/// speaker — `played_back` tells which happened).
pub fn mic_test_report(seconds: u64, vu: bool) -> anyhow::Result<MicTestReport> {
    mic_test_report_on(seconds, vu, None, None)
}

/// Named-device variant: `input`/`output` are display names from
/// `input_device_names`/`output_device_names` (None/empty = default).
/// Unknown names error — never silently rerouted.
pub fn mic_test_report_on(
    seconds: u64,
    vu: bool,
    input: Option<&str>,
    output: Option<&str>,
) -> anyhow::Result<MicTestReport> {
    use anyhow::bail;

    let seconds = seconds.clamp(1, 10);
    let (capture, mic_rx) = match AudioCapture::start_on(input) {
        Some(c) => c,
        None => bail!(named_device_error("input", input)),
    };

    let mut frames: Vec<Vec<i16>> = Vec::with_capacity(seconds as usize * 50);
    let start = std::time::Instant::now();
    let mut last_vu = start;
    let mut peak_db = -60.0f64;

    while start.elapsed() < std::time::Duration::from_secs(seconds) {
        match mic_rx.recv_timeout(std::time::Duration::from_millis(25)) {
            Ok(frame) => {
                let db = frame_db(&frame);
                peak_db = peak_db.max(db);
                if vu && last_vu.elapsed() >= std::time::Duration::from_millis(100) {
                    let bar_len = ((db + 60.0) / 60.0 * 30.0).clamp(0.0, 30.0) as usize;
                    let bar: String = "█".repeat(bar_len) + &"░".repeat(30 - bar_len);
                    print!("\r  [{bar}] {db:5.1} dBFS ");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    last_vu = std::time::Instant::now();
                }
                frames.push(frame);
            }
            Err(_) => continue,
        }
    }
    drop(capture);
    let captured_secs = frames.len() as f64 * 0.02;

    let played_back = match AudioPlayback::start_on(output) {
        Some((playback, speaker_tx)) => {
            for frame in &frames {
                let _ = speaker_tx.send(frame.clone());
                thread::sleep(std::time::Duration::from_millis(20));
            }
            thread::sleep(std::time::Duration::from_millis(200));
            drop(playback);
            true
        }
        None => false,
    };

    Ok(MicTestReport {
        frames: frames.len(),
        seconds: captured_secs,
        peak_db,
        played_back,
    })
}

/// Capture 3 seconds of microphone audio, then play it back through the speaker.
///
/// Prints a VU meter bar every 100ms during capture so you can see the level.
pub fn mic_test() -> anyhow::Result<()> {
    println!("=== Microphone Test ===");
    println!("Recording for 3 seconds — speak now!\n");

    let report = mic_test_report(3, true)?;

    println!(
        "\n\nCaptured {} frames ({:.1}s, peak {:.1} dBFS)",
        report.frames, report.seconds, report.peak_db
    );
    if report.played_back {
        println!("Played back.");
    } else {
        println!("No audio output device — playback skipped.");
    }
    println!("Done.");
    Ok(())
}

/// Play a 1kHz test tone through the speaker for `msecs` milliseconds.
///
/// Returns the number of 20ms frames sent.
pub fn play_tone(msecs: u64) -> anyhow::Result<u32> {
    play_tone_on(msecs, None)
}

/// Named-device variant of `play_tone` (None/empty = default output).
pub fn play_tone_on(msecs: u64, output: Option<&str>) -> anyhow::Result<u32> {
    use anyhow::bail;

    let msecs = msecs.clamp(100, 10_000);
    let (playback, speaker_tx) = match AudioPlayback::start_on(output) {
        Some(p) => p,
        None => bail!(named_device_error("output", output)),
    };

    let mut gen = super::test_tone::ToneGenerator::new();
    let n_frames = (msecs / 20).max(1);
    for _ in 0..n_frames {
        let frame = gen.next_frame();
        if speaker_tx.send(frame).is_err() {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(20));
    }
    thread::sleep(std::time::Duration::from_millis(200));
    drop(playback);
    Ok(n_frames as u32)
}

/// Error text distinguishing "no hardware" from "stale device pick".
fn named_device_error(kind: &str, want: Option<&str>) -> String {
    match want.filter(|n| !n.is_empty()) {
        Some(name) => format!("Unknown audio {} device: {}", kind, name),
        None => format!("No audio {} device found", kind),
    }
}

/// Peak level of one frame in dBFS (floor −60 for silence).
pub fn frame_db(frame: &[i16]) -> f64 {
    let rms = rms_level(frame);
    if rms > 0.0 {
        20.0 * rms.log10()
    } else {
        -60.0
    }
}

/// Sample the mic for `msecs` and return peak dBFS for a live meter.
/// None when no input device (or unknown `input` name). Blocks ≈`msecs`
/// plus ~100ms stream setup — poll off the UI thread.
pub fn mic_level_sample(msecs: u64, input: Option<&str>) -> Option<f64> {
    let msecs = msecs.clamp(50, 1000);
    let (_capture, mic_rx) = AudioCapture::start_on(input)?;
    let start = std::time::Instant::now();
    let mut peak = -60.0f64;
    while start.elapsed() < std::time::Duration::from_millis(msecs) {
        match mic_rx.recv_timeout(std::time::Duration::from_millis(25)) {
            Ok(frame) => peak = peak.max(frame_db(&frame)),
            Err(_) => continue,
        }
    }
    Some(peak)
}

/// Compute RMS level of a frame, normalized to 0.0–1.0 range.
fn rms_level(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt() / 32768.0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_identity() {
        let input: Vec<i16> = (0..160).collect();
        let out = resample(&input, 8000, 8000);
        assert_eq!(out, input);
    }

    #[test]
    fn test_resample_upsample_6x() {
        // 8000 -> 48000 = 6x
        let input: Vec<i16> = vec![0, 1000, 2000, 0];
        let out = resample(&input, 8000, 48000);
        assert_eq!(out.len(), 24);
        // First sample should be 0, last should be near 0
        assert_eq!(out[0], 0);
        // Midpoint-ish samples should interpolate
        assert!(out[3] > 0 && out[3] < 1000);
    }

    #[test]
    fn test_resample_downsample_6x() {
        // 48000 -> 8000 = 1/6
        let input: Vec<i16> = (0..48).map(|i| (i * 100) as i16).collect();
        let out = resample(&input, 48000, 8000);
        assert_eq!(out.len(), 8);
        assert_eq!(out[0], 0);
    }

    #[test]
    fn test_resample_empty() {
        let out = resample(&[], 48000, 8000);
        assert!(out.is_empty());
    }

    #[test]
    fn test_audio_capture_graceful_on_headless() {
        // On CI/headless, this should return None without panicking.
        // On a machine with audio, it returns Some.
        let result = AudioCapture::start();
        // Either outcome is fine — just don't panic.
        if result.is_none() {
            tracing::info!("No audio input device (expected on headless)");
        }
    }

    #[test]
    fn test_audio_playback_graceful_on_headless() {
        let result = AudioPlayback::start();
        if result.is_none() {
            tracing::info!("No audio output device (expected on headless)");
        }
    }

    #[test]
    fn test_frame_db_silence_floor() {
        assert_eq!(frame_db(&[]), -60.0);
        assert_eq!(frame_db(&[0, 0, 0]), -60.0);
    }

    #[test]
    fn test_frame_db_full_scale_near_zero() {
        // Full-scale constant tone: rms = 1.0 -> 0 dBFS.
        let db = frame_db(&[i16::MAX; 160]);
        assert!(db > -0.001 && db <= 0.0, "db={db}");
    }

    #[test]
    fn test_frame_db_half_scale_minus_six() {
        // Half amplitude -> ~-6.02 dBFS.
        let db = frame_db(&[16384; 160]);
        assert!((db + 6.02).abs() < 0.01, "db={db}");
    }

    #[test]
    fn test_device_enumeration_never_panics() {
        // Hardware-free assertion: enumeration returns (possibly empty)
        // lists on any machine, headless or not.
        let _ = (input_device_names(), output_device_names());
        let _ = (default_input_name(), default_output_name());
    }

    #[test]
    fn test_named_device_unknown_errors() {
        // Unknown names error deterministically, with or without hardware.
        assert!(AudioCapture::start_on(Some("ostmac-no-such-device")).is_none());
        assert!(AudioPlayback::start_on(Some("ostmac-no-such-device")).is_none());
        assert!(
            mic_test_report_on(1, false, Some("ostmac-no-such-device"), None).is_err()
        );
        assert!(play_tone_on(100, Some("ostmac-no-such-device")).is_err());
        assert!(mic_level_sample(50, Some("ostmac-no-such-device")).is_none());
    }

    #[test]
    fn test_concurrent_audio_ops_stay_bounded() {
        // Regression: concurrent enumeration + open + sample wedged in a
        // CoreAudio mutex (BoomAudio). Every path must return; the join is
        // timeout-bounded so a regression fails instead of hanging the suite.
        let (tx, rx) = mpsc::channel::<&'static str>();
        for (i, name) in ["enum", "open-in", "open-out", "level"].iter().enumerate() {
            let tx = tx.clone();
            let name = *name;
            thread::spawn(move || {
                match i {
                    0 => {
                        let _ = (input_device_names(), output_device_names());
                    }
                    1 => {
                        let _ = AudioCapture::start_on(Some("ostmac-no-such-device"));
                    }
                    2 => {
                        let _ = AudioPlayback::start_on(Some("ostmac-no-such-device"));
                    }
                    _ => {
                        let _ = mic_level_sample(50, Some("ostmac-no-such-device"));
                    }
                }
                let _ = tx.send(name);
            });
        }
        drop(tx);
        for _ in 0..4 {
            rx.recv_timeout(Duration::from_secs(60))
                .expect("audio op hung (setup-lock regression)");
        }
    }
}
