//! H.264 encode/decode wrappers around the `openh264` crate (v0.5 API).
//!
//! Encoder: takes I420 YUV frames, produces H.264 NAL units.
//! Decoder: takes H.264 NAL units, produces I420 YUV frames.

use anyhow::{Context, Result};
use openh264::decoder::Decoder;
use openh264::encoder::{Encoder, EncoderConfig};
use openh264::formats::YUVSource;
use openh264::OpenH264API;

/// Wrapper to pass raw I420 data directly to openh264 encoder.
struct RawI420<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
}

impl<'a> YUVSource for RawI420<'a> {
    fn width(&self) -> i32 { self.width as i32 }
    fn height(&self) -> i32 { self.height as i32 }
    fn y(&self) -> &[u8] { &self.data[..self.width * self.height] }
    fn u(&self) -> &[u8] {
        let y_size = self.width * self.height;
        let uv_size = (self.width / 2) * (self.height / 2);
        &self.data[y_size..y_size + uv_size]
    }
    fn v(&self) -> &[u8] {
        let y_size = self.width * self.height;
        let uv_size = (self.width / 2) * (self.height / 2);
        &self.data[y_size + uv_size..y_size + uv_size * 2]
    }
    fn y_stride(&self) -> i32 { self.width as i32 }
    fn u_stride(&self) -> i32 { (self.width / 2) as i32 }
    fn v_stride(&self) -> i32 { (self.width / 2) as i32 }
}

/// Decoded YUV frame ready for display.
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    /// I420 planar data: Y (w*h) + U (w*h/4) + V (w*h/4)
    pub data: Vec<u8>,
}

/// H.264 encoder wrapper.
pub struct H264Encoder {
    encoder: Encoder,
    width: u32,
    height: u32,
}

impl H264Encoder {
    /// Create a new encoder for the given resolution.
    pub fn new(width: u32, height: u32, fps: f32, bitrate_kbps: u32) -> Result<Self> {
        let api = OpenH264API::from_source();
        let config = EncoderConfig::new(width, height)
            .max_frame_rate(fps)
            .set_bitrate_bps(bitrate_kbps * 1000);

        let encoder =
            Encoder::with_config(api, config).context("Failed to create openh264 encoder")?;

        Ok(Self {
            encoder,
            width,
            height,
        })
    }

    /// Encode a raw I420 YUV frame into H.264 NAL units.
    ///
    /// Returns a Vec of NAL unit byte vectors (without start codes).
    pub fn encode(&mut self, yuv_data: &[u8]) -> Result<Vec<Vec<u8>>> {
        let expected_size = (self.width * self.height * 3 / 2) as usize;
        if yuv_data.len() < expected_size {
            anyhow::bail!(
                "YUV frame too small: {} bytes, expected {}",
                yuv_data.len(),
                expected_size
            );
        }

        let yuv = RawI420 {
            data: yuv_data,
            width: self.width as usize,
            height: self.height as usize,
        };

        let bitstream = self.encoder.encode(&yuv).context("openh264 encode failed")?;

        // Extract NAL units from the encoded bitstream
        let mut nals = Vec::new();
        for layer_idx in 0..bitstream.num_layers() {
            let layer = bitstream.layer(layer_idx);
            if let Some(layer) = layer {
                for nal_idx in 0..layer.nal_count() {
                    if let Some(nal_data) = layer.nal_unit(nal_idx) {
                        // openh264 returns NAL units with start codes -- strip them
                        let nal = strip_start_code(nal_data);
                        if !nal.is_empty() {
                            nals.push(nal.to_vec());
                        }
                    }
                }
            }
        }

        Ok(nals)
    }
}

/// H.264 decoder wrapper.
pub struct H264Decoder {
    decoder: Decoder,
}

impl H264Decoder {
    pub fn new() -> Result<Self> {
        let api = OpenH264API::from_source();
        let decoder = Decoder::new(api).context("Failed to create openh264 decoder")?;
        Ok(Self { decoder })
    }

    /// Decode a single H.264 NAL unit.
    ///
    /// Returns a decoded YUV frame if the decoder produced output, None if
    /// it needs more data (e.g., SPS/PPS before IDR).
    pub fn decode(&mut self, nal: &[u8]) -> Result<Option<DecodedFrame>> {
        // openh264 expects NAL units with Annex B start codes
        let mut annexb = vec![0x00, 0x00, 0x00, 0x01];
        annexb.extend_from_slice(nal);

        match self.decoder.decode(&annexb) {
            Ok(Some(yuv)) => {
                let (width, height) = yuv.dimension_rgb();
                let w = width;
                let h = height;

                // Extract I420 planes
                let y_size = w * h;
                let uv_size = (w / 2) * (h / 2);
                let mut data = vec![0u8; y_size + uv_size * 2];

                // Copy Y plane
                let y_stride = yuv.y().len() / h;
                for row in 0..h {
                    let src_start = row * y_stride;
                    let dst_start = row * w;
                    data[dst_start..dst_start + w]
                        .copy_from_slice(&yuv.y()[src_start..src_start + w]);
                }

                // Copy U plane
                let u_stride = yuv.u().len() / (h / 2);
                let half_w = w / 2;
                let half_h = h / 2;
                for row in 0..half_h {
                    let src_start = row * u_stride;
                    let dst_start = y_size + row * half_w;
                    data[dst_start..dst_start + half_w]
                        .copy_from_slice(&yuv.u()[src_start..src_start + half_w]);
                }

                // Copy V plane
                let v_stride = yuv.v().len() / (h / 2);
                for row in 0..half_h {
                    let src_start = row * v_stride;
                    let dst_start = y_size + uv_size + row * half_w;
                    data[dst_start..dst_start + half_w]
                        .copy_from_slice(&yuv.v()[src_start..src_start + half_w]);
                }

                Ok(Some(DecodedFrame {
                    width: w as u32,
                    height: h as u32,
                    data,
                }))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                tracing::debug!("openh264 decode error: {:?}", e);
                Ok(None) // Don't bail -- decoder may recover on next NAL
            }
        }
    }
}

/// Strip Annex B start codes (0x00 0x00 0x01 or 0x00 0x00 0x00 0x01) from NAL data.
fn strip_start_code(data: &[u8]) -> &[u8] {
    if data.starts_with(&[0x00, 0x00, 0x00, 0x01]) {
        &data[4..]
    } else if data.starts_with(&[0x00, 0x00, 0x01]) {
        &data[3..]
    } else {
        data
    }
}
