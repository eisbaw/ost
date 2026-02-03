//! RTP packet encoding/decoding and G.711 mu-law (PCMU) codec.
//!
//! RTP header format (RFC 3550):
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |V=2|P|X|  CC   |M|     PT      |       sequence number         |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                           timestamp                           |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |           synchronization source (SSRC) identifier            |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```

use anyhow::{bail, Result};

/// Minimum RTP header size in bytes (no CSRC, no extension).
pub const RTP_HEADER_SIZE: usize = 12;

/// Compute the full RTP header length from raw packet bytes.
///
/// Accounts for CSRC entries and header extensions (RFC 3550 §5.3.1).
/// Returns `None` if the packet is too short.
pub fn full_header_len(data: &[u8]) -> Option<usize> {
    if data.len() < RTP_HEADER_SIZE {
        return None;
    }
    let csrc_count = (data[0] & 0x0F) as usize;
    let has_extension = (data[0] >> 4) & 0x01 != 0;
    let mut len = RTP_HEADER_SIZE + csrc_count * 4;
    if data.len() < len {
        return None;
    }
    if has_extension {
        // Extension header: 2 bytes profile + 2 bytes length (in 32-bit words)
        if data.len() < len + 4 {
            return None;
        }
        let ext_words = u16::from_be_bytes([data[len + 2], data[len + 3]]) as usize;
        len += 4 + ext_words * 4;
        if data.len() < len {
            return None;
        }
    }
    Some(len)
}

/// PCMU payload type (RFC 3551).
pub const PT_PCMU: u8 = 0;

/// Samples per 20ms packet at 8000 Hz.
pub const SAMPLES_PER_PACKET: usize = 160;

/// Packet interval in milliseconds.
pub const PACKET_INTERVAL_MS: u64 = 20;

/// Timestamp increment per packet (8000 Hz * 20ms = 160).
pub const TIMESTAMP_INCREMENT: u32 = 160;

/// Parsed RTP packet.
#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub version: u8,
    pub padding: bool,
    pub extension: bool,
    pub csrc_count: u8,
    pub marker: bool,
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: Vec<u8>,
}

/// Encode an RTP packet into bytes.
pub fn encode(payload_type: u8, seq: u16, timestamp: u32, ssrc: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(RTP_HEADER_SIZE + payload.len());

    // Byte 0: V=2, P=0, X=0, CC=0 -> 0x80
    buf.push(0x80);
    // Byte 1: M=0, PT
    buf.push(payload_type & 0x7F);
    // Bytes 2-3: sequence number (big-endian)
    buf.extend_from_slice(&seq.to_be_bytes());
    // Bytes 4-7: timestamp (big-endian)
    buf.extend_from_slice(&timestamp.to_be_bytes());
    // Bytes 8-11: SSRC (big-endian)
    buf.extend_from_slice(&ssrc.to_be_bytes());
    // Payload
    buf.extend_from_slice(payload);

    buf
}

/// Decode bytes into an RTP packet.
pub fn decode(data: &[u8]) -> Result<RtpPacket> {
    if data.len() < RTP_HEADER_SIZE {
        bail!("RTP packet too short: {} bytes", data.len());
    }

    let version = (data[0] >> 6) & 0x03;
    if version != 2 {
        bail!("Unsupported RTP version: {}", version);
    }

    let padding = (data[0] >> 5) & 0x01 != 0;
    let extension = (data[0] >> 4) & 0x01 != 0;
    let csrc_count = data[0] & 0x0F;
    let marker = (data[1] >> 7) & 0x01 != 0;
    let payload_type = data[1] & 0x7F;
    let sequence_number = u16::from_be_bytes([data[2], data[3]]);
    let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

    let header_len = RTP_HEADER_SIZE + (csrc_count as usize) * 4;
    if data.len() < header_len {
        bail!(
            "RTP packet too short for {} CSRCs: {} bytes",
            csrc_count,
            data.len()
        );
    }

    let payload = data[header_len..].to_vec();

    Ok(RtpPacket {
        version,
        padding,
        extension,
        csrc_count,
        marker,
        payload_type,
        sequence_number,
        timestamp,
        ssrc,
        payload,
    })
}

// --- G.711 mu-law codec (PCMU) ---
//
// ITU-T G.711. Encodes 16-bit linear PCM to 8-bit mu-law and back.
// The mu-law companding curve: F(x) = sgn(x) * ln(1 + mu*|x|) / ln(1 + mu)
// where mu = 255.
//
// In practice this is implemented with a segment/quantization lookup.

/// Encode a 16-bit linear PCM sample to 8-bit mu-law (ITU-T G.711).
///
/// Uses the standard algorithm from the G.711 reference code (Sun Microsystems).
pub fn linear_to_ulaw(sample: i16) -> u8 {
    const BIAS: i16 = 0x84; // 132
    const CLIP: i16 = 32635;

    // Get the sign and the magnitude of the value.
    let sign: i16;
    let mut mag: i16;

    if sample < 0 {
        // Careful: -(-32768) overflows, but we clip before it matters.
        mag = if sample == i16::MIN { CLIP } else { -sample };
        sign = 0x80;
    } else {
        mag = sample;
        sign = 0;
    }

    if mag > CLIP {
        mag = CLIP;
    }
    mag += BIAS;

    // Find the segment (exponent). The biased magnitude fits in 14 bits + sign.
    let mut exponent: u8 = 7;
    let mut exp_mask: i16 = 0x4000;
    while exponent > 0 && (mag & exp_mask) == 0 {
        exponent -= 1;
        exp_mask >>= 1;
    }

    // Extract 4-bit mantissa
    let mantissa = ((mag >> (exponent as i16 + 3)) & 0x0F) as u8;

    // Compose the byte and complement it (mu-law convention)
    let mut ulaw_byte = (sign as u8) | (exponent << 4) | mantissa;
    ulaw_byte = !ulaw_byte;
    ulaw_byte
}

/// Decode an 8-bit mu-law sample to 16-bit linear PCM (ITU-T G.711).
///
/// Inverse of `linear_to_ulaw`. The encoder adds BIAS=132 to the magnitude,
/// finds the segment (exponent) as the position of the leading bit, and
/// extracts 4 mantissa bits. This decoder reconstructs the midpoint of the
/// quantization step.
pub fn ulaw_to_linear(sample: u8) -> i16 {
    // Complement to recover the encoded sign/exponent/mantissa byte
    let ulaw = !sample;
    let sign = (ulaw & 0x80) != 0;
    let exponent = ((ulaw >> 4) & 0x07) as u32;
    let mantissa = (ulaw & 0x0F) as i32;

    // Reconstruct the biased magnitude then subtract bias.
    // The encoder extracts mantissa = (biased >> (exp+3)) & 0xF, discarding the
    // leading 1 bit. To decode, restore the leading 1 (0x10), combine with mantissa,
    // shift back, and add half an LSB for midpoint reconstruction.
    let biased = ((mantissa | 0x10) << (exponent + 3)) + (1i32 << (exponent + 2));
    let mag = biased - 132;
    let mag = mag.max(0);

    if sign {
        -(mag as i16)
    } else {
        mag as i16
    }
}

/// Generate a silence payload for PCMU (160 bytes of mu-law silence = 0xFF).
/// 0xFF in mu-law decodes to approximately 0 (very small positive value near zero).
pub fn silence_payload() -> Vec<u8> {
    vec![0xFF; SAMPLES_PER_PACKET]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let payload = vec![0xFF; 160];
        let encoded = encode(PT_PCMU, 1, 160, 0x12345678, &payload);
        assert_eq!(encoded.len(), RTP_HEADER_SIZE + 160);

        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.version, 2);
        assert_eq!(decoded.payload_type, PT_PCMU);
        assert_eq!(decoded.sequence_number, 1);
        assert_eq!(decoded.timestamp, 160);
        assert_eq!(decoded.ssrc, 0x12345678);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn test_decode_too_short() {
        assert!(decode(&[0x80, 0x00]).is_err());
    }

    #[test]
    fn test_decode_wrong_version() {
        let mut data = [0u8; 12];
        data[0] = 0x00; // version 0
        assert!(decode(&data).is_err());
    }

    #[test]
    fn test_ulaw_roundtrip_zero() {
        let encoded = linear_to_ulaw(0);
        let decoded = ulaw_to_linear(encoded);
        // mu-law zero is not exactly zero, but close
        assert!(decoded.abs() < 4, "decoded zero: {}", decoded);
    }

    #[test]
    fn test_ulaw_roundtrip_positive() {
        // mu-law is lossy with ~33dB SNR. For small values the quantization
        // error can be large relative to the value. For large values it's
        // proportionally smaller. We just verify the decoded value has the
        // same sign and the right order of magnitude.
        for &sample in &[100i16, 1000, 10000, 30000] {
            let encoded = linear_to_ulaw(sample);
            let decoded = ulaw_to_linear(encoded);
            assert!(
                decoded > 0,
                "sign mismatch for sample={}: decoded={}",
                sample,
                decoded
            );
            let s = sample as i32;
            let d = decoded as i32;
            assert!(
                d >= s / 8 && d <= s * 2,
                "sample={}, decoded={} out of range",
                sample,
                decoded,
            );
        }
    }

    #[test]
    fn test_ulaw_roundtrip_negative() {
        for &sample in &[-100i16, -1000, -10000, -30000] {
            let encoded = linear_to_ulaw(sample);
            let decoded = ulaw_to_linear(encoded);
            assert!(
                decoded < 0,
                "sign mismatch for sample={}: decoded={}",
                sample,
                decoded
            );
            let abs_sample = sample.unsigned_abs() as i32;
            let abs_decoded = decoded.unsigned_abs() as i32;
            assert!(
                abs_decoded >= abs_sample / 8 && abs_decoded <= abs_sample * 2,
                "sample={}, decoded={} out of range",
                sample,
                decoded,
            );
        }
    }

    #[test]
    fn test_silence_payload() {
        let silence = silence_payload();
        assert_eq!(silence.len(), SAMPLES_PER_PACKET);
        assert!(silence.iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn test_ulaw_silence_decodes_near_zero() {
        let decoded = ulaw_to_linear(0xFF);
        assert!(decoded.abs() < 4, "silence decoded to: {}", decoded);
    }
}
