//! V4L2 camera capture — reads YUV420 (or YUYV) frames from /dev/video0.
//!
//! Uses the `v4l` crate with mmap streaming. Converts YUYV to I420 if needed.
//! Runs a capture thread that sends raw YUV frames over a channel.

use anyhow::{Context, Result};
use std::sync::mpsc;
use v4l::buffer::Type;
use v4l::io::mmap::Stream;
use v4l::io::traits::CaptureStream;
use v4l::video::Capture;
use v4l::{Device, FourCC};

/// A captured YUV420 (I420) frame.
pub struct YuvFrame {
    pub width: u32,
    pub height: u32,
    /// Planar I420: Y plane (w*h), U plane (w*h/4), V plane (w*h/4).
    pub data: Vec<u8>,
}

/// Camera capture handle. Keeps the capture thread alive.
pub struct CameraCapture {
    _handle: std::thread::JoinHandle<()>,
}

impl CameraCapture {
    /// Open the camera and start capturing frames.
    ///
    /// Returns the capture handle and a receiver for YUV frames.
    /// `device_path` defaults to "/dev/video0" if None.
    /// Target resolution is 320x240 at 15fps (low bandwidth for Teams).
    pub fn start(
        device_path: Option<&str>,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<(Self, mpsc::Receiver<YuvFrame>)> {
        let path = device_path.unwrap_or("/dev/video0");
        let dev = Device::with_path(path)
            .with_context(|| format!("Failed to open camera at {}", path))?;

        // Try to set format — prefer YUYV (widely supported), fall back to whatever works
        let mut fmt = dev.format().context("Failed to get camera format")?;
        fmt.width = width;
        fmt.height = height;

        // Try YUYV first (most USB cameras support it)
        fmt.fourcc = FourCC::new(b"YUYV");
        let actual_fmt = match dev.set_format(&fmt) {
            Ok(f) => f,
            Err(_) => {
                // Try MJPG as fallback — we won't use it but log what we get
                fmt.fourcc = FourCC::new(b"MJPG");
                dev.set_format(&fmt)
                    .context("Failed to set camera format (tried YUYV and MJPG)")?
            }
        };

        let actual_fourcc = actual_fmt.fourcc;
        let actual_w = actual_fmt.width;
        let actual_h = actual_fmt.height;
        tracing::info!(
            "Camera opened: {}x{} fourcc={} (requested {}x{} @ {}fps)",
            actual_w,
            actual_h,
            actual_fourcc,
            width,
            height,
            fps,
        );

        // Set frame rate via stream parameters
        if let Ok(mut params) = dev.params() {
            params.interval = v4l::Fraction::new(1, fps);
            let _ = dev.set_params(&params);
        }

        let (tx, rx) = mpsc::sync_channel::<YuvFrame>(2); // small buffer, drop old frames

        let handle = std::thread::spawn(move || {
            if let Err(e) = capture_loop(dev, actual_w, actual_h, actual_fourcc, tx) {
                tracing::error!("Camera capture loop exited: {:#}", e);
            }
        });

        Ok((CameraCapture { _handle: handle }, rx))
    }
}

/// Main capture loop — runs on a dedicated thread.
fn capture_loop(
    dev: Device,
    width: u32,
    height: u32,
    fourcc: FourCC,
    tx: mpsc::SyncSender<YuvFrame>,
) -> Result<()> {
    let mut stream = Stream::with_buffers(&dev, Type::VideoCapture, 4)
        .context("Failed to start V4L2 mmap stream")?;

    loop {
        let (buf, _meta) = stream.next().context("Failed to read camera frame")?;

        let yuv_data = if fourcc == FourCC::new(b"YUYV") {
            yuyv_to_i420(buf, width, height)
        } else {
            // If format is already I420/YU12, use as-is
            buf.to_vec()
        };

        let frame = YuvFrame {
            width,
            height,
            data: yuv_data,
        };

        // Non-blocking send — drop frame if receiver is behind
        match tx.try_send(frame) {
            Ok(()) => {}
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                // Receiver is behind — drop frame, keep going
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                // Channel closed, exit
                break;
            }
        }
    }

    Ok(())
}

/// Capture ~3s of video from V4L2 camera, then play it back in an SDL2 window.
pub fn cam_test() -> anyhow::Result<()> {
    use super::display::{DisplayFrame, VideoDisplay};
    use anyhow::bail;

    println!("=== Camera Test ===");
    println!("Capturing 3 seconds of video...\n");

    let (capture, cam_rx) =
        CameraCapture::start(None, 320, 240, 15).context("Failed to start camera")?;

    let mut frames: Vec<YuvFrame> = Vec::with_capacity(45);
    let start = std::time::Instant::now();
    let capture_duration = std::time::Duration::from_secs(3);

    while start.elapsed() < capture_duration {
        match cam_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(frame) => {
                if frames.is_empty() {
                    println!("  First frame: {}x{}", frame.width, frame.height);
                }
                frames.push(frame);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Camera disconnected during capture");
            }
        }
    }
    drop(capture);

    let capture_elapsed = start.elapsed().as_secs_f64();
    let capture_fps = frames.len() as f64 / capture_elapsed;
    println!(
        "\nCaptured {} frames in {:.3}s ({:.1} fps)",
        frames.len(),
        capture_elapsed,
        capture_fps,
    );

    if frames.is_empty() {
        bail!("No frames captured");
    }

    println!("Playing back...\n");
    let (display, display_tx) =
        VideoDisplay::start("Camera Test Playback").context("Failed to start video display")?;

    let playback_start = std::time::Instant::now();
    let frame_interval_us = (capture_elapsed * 1_000_000.0 / frames.len() as f64) as u64;
    let frame_interval = std::time::Duration::from_micros(frame_interval_us);
    let n_frames = frames.len();
    for (i, frame) in frames.into_iter().enumerate() {
        let df = DisplayFrame {
            width: frame.width,
            height: frame.height,
            data: frame.data,
        };
        if display_tx.send(df).is_err() {
            break; // window closed
        }
        // Sleep until target wall-clock time to avoid drift from send overhead
        let target = playback_start + frame_interval * (i + 1) as u32;
        let now = std::time::Instant::now();
        if target > now {
            std::thread::sleep(target - now);
        }
    }
    let playback_elapsed = playback_start.elapsed().as_secs_f64();
    let playback_fps = n_frames as f64 / playback_elapsed;

    // Let last frame linger briefly, then close channel so display thread exits cleanly
    std::thread::sleep(std::time::Duration::from_millis(500));
    drop(display_tx);
    // Wait for display thread to finish SDL2 teardown
    display.join();

    println!("Recording : {:.3}s  ({} frames, {:.1} fps)", capture_elapsed, n_frames, capture_fps);
    println!("Playback  : {:.3}s  ({} frames, {:.1} fps)", playback_elapsed, n_frames, playback_fps);
    let ratio = playback_elapsed / capture_elapsed;
    if ratio > 1.05 {
        println!("Playback was {:.0}% slower than recording (frame interval too long)", (ratio - 1.0) * 100.0);
    } else if ratio < 0.95 {
        println!("Playback was {:.0}% faster than recording", (1.0 - ratio) * 100.0);
    } else {
        println!("Playback matched recording duration");
    }
    Ok(())
}

/// Convert YUYV (YUV 4:2:2 packed) to I420 (YUV 4:2:0 planar).
fn yuyv_to_i420(yuyv: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);
    let mut out = vec![0u8; y_size + uv_size * 2];

    let (y_plane, uv_planes) = out.split_at_mut(y_size);
    let (u_plane, v_plane) = uv_planes.split_at_mut(uv_size);

    for row in 0..h {
        for col in (0..w).step_by(2) {
            let yuyv_offset = (row * w + col) * 2;
            if yuyv_offset + 3 >= yuyv.len() {
                break;
            }
            let y0 = yuyv[yuyv_offset];
            let u = yuyv[yuyv_offset + 1];
            let y1 = yuyv[yuyv_offset + 2];
            let v = yuyv[yuyv_offset + 3];

            y_plane[row * w + col] = y0;
            y_plane[row * w + col + 1] = y1;

            // Subsample U/V by 2x2
            if row % 2 == 0 {
                let uv_row = row / 2;
                let uv_col = col / 2;
                u_plane[uv_row * (w / 2) + uv_col] = u;
                v_plane[uv_row * (w / 2) + uv_col] = v;
            }
        }
    }

    out
}
