//! Audio capture and playback using cpal.
//!
//! Opens the default input/output devices at 8000 Hz mono i16 (PCMU native rate).
//! If the device doesn't support 8000 Hz, captures/plays at the device's native
//! rate and resamples with simple linear interpolation.
//!
//! Gated behind `#[cfg(feature = "audio")]` — when the feature is off, the
//! public types are not compiled and media.rs falls back to silence mode.

use std::sync::mpsc;
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SampleRate, StreamConfig};

/// Number of PCM samples per 20ms frame at 8000 Hz.
const FRAME_SAMPLES: usize = 160;

/// Target sample rate for PCMU.
const TARGET_RATE: u32 = 8000;

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
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<i16>>(50);
        // Channel to keep the stream-owning thread alive; dropping sender kills it.
        let (keep_tx, keep_rx) = mpsc::channel::<()>();

        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started2 = started.clone();

        thread::spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    tracing::warn!("No audio input device found — mic capture disabled");
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
            started2.store(true, std::sync::atomic::Ordering::SeqCst);

            // Park this thread; the stream stays alive until keep_rx is dropped.
            let _ = keep_rx.recv();
            drop(stream);
        });

        // Give the thread a moment to initialize.
        thread::sleep(std::time::Duration::from_millis(100));

        if started.load(std::sync::atomic::Ordering::SeqCst) {
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
        let (frame_tx, frame_rx) = mpsc::sync_channel::<Vec<i16>>(50);
        let (keep_tx, keep_rx) = mpsc::channel::<()>();

        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started2 = started.clone();

        thread::spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_output_device() {
                Some(d) => d,
                None => {
                    tracing::warn!("No audio output device found — speaker playback disabled");
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
            started2.store(true, std::sync::atomic::Ordering::SeqCst);

            // Park thread; stream stays alive until keep_rx is dropped.
            let _ = keep_rx.recv();
            drop(stream);
        });

        thread::sleep(std::time::Duration::from_millis(100));

        if started.load(std::sync::atomic::Ordering::SeqCst) {
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
// mic_test — capture 3 seconds, then play back
// ---------------------------------------------------------------------------

/// Capture 3 seconds of microphone audio, then play it back through the speaker.
///
/// Prints a VU meter bar every 100ms during capture so you can see the level.
pub fn mic_test() -> anyhow::Result<()> {
    use anyhow::bail;

    println!("=== Microphone Test ===");
    println!("Recording for 3 seconds — speak now!\n");

    let (capture, mic_rx) = match AudioCapture::start() {
        Some(c) => c,
        None => bail!("No audio input device found"),
    };

    // Capture 3 seconds of 160-sample frames (50 frames/sec * 3 = 150 frames)
    let mut frames: Vec<Vec<i16>> = Vec::with_capacity(150);
    let start = std::time::Instant::now();
    let mut last_vu = start;

    while start.elapsed() < std::time::Duration::from_secs(3) {
        match mic_rx.recv_timeout(std::time::Duration::from_millis(25)) {
            Ok(frame) => {
                // VU meter: compute RMS level
                if last_vu.elapsed() >= std::time::Duration::from_millis(100) {
                    let rms = rms_level(&frame);
                    let db = if rms > 0.0 { 20.0 * rms.log10() } else { -60.0 };
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
    println!("\n\nCaptured {} frames ({:.1}s)", frames.len(), frames.len() as f64 * 0.02);

    // Play back
    println!("Playing back...\n");
    let (playback, speaker_tx) = match AudioPlayback::start() {
        Some(p) => p,
        None => bail!("No audio output device found"),
    };

    for frame in &frames {
        let _ = speaker_tx.send(frame.clone());
        thread::sleep(std::time::Duration::from_millis(20));
    }
    // Drain: let playback finish
    thread::sleep(std::time::Duration::from_millis(200));
    drop(playback);

    println!("Done.");
    Ok(())
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
}
