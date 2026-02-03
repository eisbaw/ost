//! SDL2-based video display window for received video frames.
//!
//! Opens an SDL2 window and renders I420 YUV frames as an SDL texture.
//! Runs on a dedicated thread since SDL2 requires the main thread on some
//! platforms (but on Linux this is fine from any thread).

use anyhow::{Context, Result};
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use std::sync::mpsc;

/// A frame to display (I420 YUV).
pub struct DisplayFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Video display handle. Keeps the display thread alive.
pub struct VideoDisplay {
    handle: Option<std::thread::JoinHandle<()>>,
}

impl VideoDisplay {
    /// Create a display window and return a sender for frames.
    ///
    /// The window title shows "Teams Video". Frames are rendered as fast as
    /// they arrive; the display thread pumps SDL events to keep the window alive.
    pub fn start(title: &str) -> Result<(Self, mpsc::SyncSender<DisplayFrame>)> {
        let (tx, rx) = mpsc::sync_channel::<DisplayFrame>(2);
        let title = title.to_string();

        let handle = std::thread::spawn(move || {
            if let Err(e) = display_loop(&title, rx) {
                tracing::error!("Video display loop exited: {:#}", e);
            }
        });

        Ok((VideoDisplay { handle: Some(handle) }, tx))
    }

    /// Wait for the display thread to finish (call after dropping the frame sender).
    pub fn join(mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn display_loop(title: &str, rx: mpsc::Receiver<DisplayFrame>) -> Result<()> {
    // Disable SDL2 audio to prevent interference with cpal (which uses PulseAudio/ALSA directly).
    // Without this, SDL2 may grab audio devices and cause cpal playback to fail.
    sdl2::hint::set("SDL_AUDIODRIVER", "dummy");

    let sdl_context = sdl2::init().map_err(|e| anyhow::anyhow!("SDL2 init failed: {}", e))?;
    let video_subsystem = sdl_context
        .video()
        .map_err(|e| anyhow::anyhow!("SDL2 video init failed: {}", e))?;

    // Start with a default size; resize when we get the first frame
    let window = video_subsystem
        .window(title, 640, 480)
        .position_centered()
        .resizable()
        .build()
        .context("Failed to create SDL2 window")?;

    let mut canvas = window
        .into_canvas()
        .build()
        .context("Failed to create SDL2 canvas")?;

    let texture_creator = canvas.texture_creator();
    let mut texture: Option<sdl2::render::Texture> = None;
    let mut current_w = 0u32;
    let mut current_h = 0u32;

    let mut event_pump = sdl_context
        .event_pump()
        .map_err(|e| anyhow::anyhow!("SDL2 event pump failed: {}", e))?;

    tracing::info!("Video display window opened: {}", title);

    loop {
        // Pump SDL events (window close, etc.)
        for event in event_pump.poll_iter() {
            use sdl2::event::Event;
            match event {
                Event::Quit { .. } => {
                    tracing::info!("Video display window closed");
                    return Ok(());
                }
                _ => {}
            }
        }

        // Try to receive a frame (non-blocking with short timeout)
        match rx.recv_timeout(std::time::Duration::from_millis(16)) {
            Ok(frame) => {
                if frame.width == 0 || frame.height == 0 {
                    continue;
                }

                // Recreate texture if resolution changed
                if frame.width != current_w || frame.height != current_h {
                    current_w = frame.width;
                    current_h = frame.height;
                    texture = Some(
                        texture_creator
                            .create_texture_streaming(
                                PixelFormatEnum::IYUV,
                                current_w,
                                current_h,
                            )
                            .context("Failed to create YUV texture")?,
                    );
                    tracing::info!(
                        "Display texture created: {}x{}",
                        current_w,
                        current_h
                    );
                }

                if let Some(ref mut tex) = texture {
                    let y_size = (current_w * current_h) as usize;
                    let uv_size = ((current_w / 2) * (current_h / 2)) as usize;
                    let y_stride = current_w as usize;
                    let uv_stride = (current_w / 2) as usize;

                    if frame.data.len() >= y_size + uv_size * 2 {
                        let _ = tex.update_yuv(
                            None,
                            &frame.data[..y_size],
                            y_stride,
                            &frame.data[y_size..y_size + uv_size],
                            uv_stride,
                            &frame.data[y_size + uv_size..],
                            uv_stride,
                        );
                    }

                    canvas.clear();
                    let (win_w, win_h) = canvas.output_size().unwrap_or((640, 480));
                    let dst = Rect::new(0, 0, win_w, win_h);
                    let _ = canvas.copy(tex, None, Some(dst));
                    canvas.present();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // No frame — just keep the window alive
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::info!("Display frame channel closed, shutting down window");
                return Ok(());
            }
        }
    }
}
