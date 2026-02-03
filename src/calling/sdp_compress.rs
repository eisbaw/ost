//! SDP compression/decompression for Microsoft Teams.
//!
//! Teams compresses SDP blobs using raw DEFLATE (zlib windowBits=-15) with a
//! preset dictionary, then base64-encodes the result. The dictionary is a
//! 20623-byte string extracted from libSkyLib.so containing representative SDP
//! content that deflate uses for better compression ratios.
//!
//! Wire format: `base64(raw_deflate(sdp_text, dictionary))`
//!
//! Compression is only applied when the SDP is >= 1201 bytes.

use anyhow::{Context, Result};
use base64::Engine;
use flate2::{Compress, Decompress, FlushCompress, FlushDecompress, Status};

/// SDP compression dictionary extracted from libSkyLib.so at offset 0x1ff6d4.
/// This 20623-byte string contains representative SDP/HTTP content that the
/// deflate algorithm uses as a preset dictionary for better compression.
const SDP_DICTIONARY: &[u8; 20623] = include_bytes!("sdp_dictionary.bin");

/// Minimum SDP size for compression (from binary analysis at 0x009adaf0).
const COMPRESSION_THRESHOLD: usize = 1201;

/// Decompress an SDP blob from Teams wire format.
///
/// Handles three cases:
/// 1. Already plaintext SDP (starts with "v=") -- returned as-is
/// 2. Base64-encoded raw-deflated SDP without dictionary
/// 3. Base64-encoded raw-deflated SDP with the preset dictionary
///
/// Returns the decompressed SDP string, or an error if decompression fails.
pub fn decompress_sdp(blob: &str) -> Result<String> {
    let trimmed = blob.trim();

    // Already plaintext SDP
    if trimmed.starts_with("v=") {
        return Ok(trimmed.to_string());
    }

    // Base64 decode
    let raw = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .context("SDP blob is not valid base64")?;

    // Try raw inflate without dictionary first
    match raw_inflate(&raw, None) {
        Ok(sdp) if sdp.starts_with("v=") => return Ok(sdp),
        _ => {}
    }

    // Try with the preset dictionary
    match raw_inflate(&raw, Some(SDP_DICTIONARY.as_slice())) {
        Ok(sdp) if sdp.starts_with("v=") => return Ok(sdp),
        Ok(sdp) => anyhow::bail!(
            "Decompressed data does not look like SDP (starts with {:?})",
            &sdp[..sdp.len().min(20)]
        ),
        Err(e) => Err(e).context("Failed to decompress SDP blob"),
    }
}

/// Compress an SDP string into Teams wire format.
///
/// If the SDP is shorter than 1201 bytes, returns None (no compression needed).
/// Otherwise returns base64(raw_deflate(sdp, dictionary)).
pub fn compress_sdp(sdp: &str) -> Result<Option<String>> {
    if sdp.len() < COMPRESSION_THRESHOLD {
        return Ok(None);
    }

    let compressed = raw_deflate(sdp.as_bytes(), Some(SDP_DICTIONARY.as_slice()))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&compressed);
    Ok(Some(encoded))
}

/// Raw inflate (windowBits=-15) with optional preset dictionary.
fn raw_inflate(data: &[u8], dictionary: Option<&[u8]>) -> Result<String> {
    let mut decompress = Decompress::new(false); // false = raw deflate (no zlib header)

    if let Some(dict) = dictionary {
        decompress
            .set_dictionary(dict)
            .context("Failed to set inflate dictionary")?;
    }

    // Start with 4x expansion estimate, grow if needed
    let mut output = vec![0u8; data.len() * 4];
    let mut total_out = 0;

    loop {
        let status = decompress
            .decompress(
                &data[decompress.total_in() as usize..],
                &mut output[total_out..],
                FlushDecompress::Finish,
            )
            .context("Raw inflate failed")?;

        total_out = decompress.total_out() as usize;

        match status {
            Status::StreamEnd => break,
            Status::Ok | Status::BufError => {
                // Need more output space
                if output.len() - total_out < 1024 {
                    output.resize(output.len() * 2, 0);
                }
            }
        }
    }

    output.truncate(total_out);
    String::from_utf8(output).context("Decompressed SDP is not valid UTF-8")
}

/// Raw deflate (windowBits=-15) with optional preset dictionary.
fn raw_deflate(data: &[u8], dictionary: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut compress = Compress::new(flate2::Compression::default(), false);

    if let Some(dict) = dictionary {
        compress
            .set_dictionary(dict)
            .context("Failed to set deflate dictionary")?;
    }

    let mut output = vec![0u8; data.len()];
    let mut total_out = 0;

    loop {
        let status = compress
            .compress(
                &data[compress.total_in() as usize..],
                &mut output[total_out..],
                FlushCompress::Finish,
            )
            .context("Raw deflate failed")?;

        total_out = compress.total_out() as usize;

        match status {
            Status::StreamEnd => break,
            Status::Ok | Status::BufError => {
                if output.len() - total_out < 1024 {
                    output.resize(output.len() * 2, 0);
                }
            }
        }
    }

    output.truncate(total_out);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plaintext_passthrough() {
        let sdp = "v=0\r\no=- 0 0 IN IP4 10.0.0.1\r\ns=session\r\n";
        let result = decompress_sdp(sdp).unwrap();
        // trim() strips trailing whitespace, so compare trimmed
        assert_eq!(result, sdp.trim());
    }

    #[test]
    fn test_plaintext_passthrough_with_whitespace() {
        let sdp = "  v=0\r\no=- 0 0 IN IP4 10.0.0.1\r\ns=session\r\n  ";
        let result = decompress_sdp(sdp).unwrap();
        assert!(result.starts_with("v="));
    }

    #[test]
    fn test_compress_below_threshold() {
        let short_sdp = "v=0\r\no=- 0 0 IN IP4 10.0.0.1\r\ns=session\r\n";
        assert!(short_sdp.len() < COMPRESSION_THRESHOLD);
        let result = compress_sdp(short_sdp).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_roundtrip_no_dictionary() {
        // Test raw deflate/inflate roundtrip without dictionary
        let data = b"v=0\r\nsome test data repeated many times\r\n".repeat(50);
        let data_str = String::from_utf8(data).unwrap();

        let compressed = raw_deflate(data_str.as_bytes(), None).unwrap();
        let decompressed = raw_inflate(&compressed, None).unwrap();
        assert_eq!(decompressed, data_str);
    }

    #[test]
    fn test_roundtrip_with_dictionary() {
        // Test raw deflate/inflate roundtrip with the SDP dictionary
        let data = b"v=0\r\nsome test data repeated many times\r\n".repeat(50);
        let data_str = String::from_utf8(data).unwrap();

        let compressed = raw_deflate(data_str.as_bytes(), Some(SDP_DICTIONARY)).unwrap();
        let decompressed = raw_inflate(&compressed, Some(SDP_DICTIONARY)).unwrap();
        assert_eq!(decompressed, data_str);
    }

    #[test]
    fn test_compress_decompress_roundtrip() {
        // Build a realistic SDP that exceeds the threshold
        let mut sdp = String::from("v=0\r\n\
            o=- 0 0 IN IP4 52.114.0.1\r\n\
            s=session\r\n\
            c=IN IP4 52.114.0.1\r\n\
            t=0 0\r\n\
            a=x-mediabw:main-video send=2000;recv=2000\r\n\
            m=audio 21730 RTP/SAVP 0 8 101 103 104 111 112 113 114 115 116\r\n\
            a=rtpmap:0 PCMU/8000\r\n\
            a=rtpmap:8 PCMA/8000\r\n\
            a=rtpmap:101 SILK/16000\r\n\
            a=rtpmap:103 SILK/8000\r\n\
            a=ice-ufrag:testufrag\r\n\
            a=ice-pwd:testpassword1234567890\r\n\
            a=candidate:1 1 UDP 2130706431 52.114.0.1 21730 typ host\r\n\
            a=candidate:2 1 UDP 1694498815 52.114.0.2 21731 typ srflx raddr 10.0.0.1 rport 21730\r\n\
            a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:dGVzdGtleTEyMzQ1Njc4OTAxMjM0NTY3ODkw|2^31\r\n\
            a=sendrecv\r\n\
            a=rtcp-mux\r\n\
            a=label:main-audio\r\n\
            a=x-source:main-audio\r\n");

        // Pad to exceed threshold
        for i in 0..30 {
            sdp.push_str(&format!(
                "a=candidate:{} 1 UDP 100 10.0.{}.{} {} typ relay raddr 10.0.0.1 rport 21730\r\n",
                i + 3,
                i / 256,
                i % 256,
                30000 + i
            ));
        }

        assert!(sdp.len() >= COMPRESSION_THRESHOLD);

        let compressed = compress_sdp(&sdp).unwrap().expect("should compress");
        // Compressed should be base64
        assert!(base64::engine::general_purpose::STANDARD
            .decode(&compressed)
            .is_ok());

        let decompressed = decompress_sdp(&compressed).unwrap();
        assert_eq!(decompressed, sdp);
    }

    #[test]
    fn test_dictionary_improves_compression() {
        // A realistic SDP should compress better with the dictionary
        let mut sdp = String::from(
            "v=0\r\n\
            o=- 0 0 IN IP4 52.114.0.1\r\n\
            s=session\r\n\
            c=IN IP4 52.114.0.1\r\n\
            t=0 0\r\n",
        );

        for i in 0..50 {
            sdp.push_str(&format!(
                "a=candidate:{} 1 UDP 2130706431 52.114.0.{} {} typ host\r\n",
                i,
                i,
                20000 + i
            ));
        }

        let without_dict = raw_deflate(sdp.as_bytes(), None).unwrap();
        let with_dict = raw_deflate(sdp.as_bytes(), Some(SDP_DICTIONARY)).unwrap();

        // Dictionary should help (or at least not hurt)
        assert!(
            with_dict.len() <= without_dict.len(),
            "Dictionary should improve compression: {} vs {} bytes",
            with_dict.len(),
            without_dict.len()
        );
    }

    #[test]
    fn test_invalid_base64_returns_error() {
        let result = decompress_sdp("not-valid-base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_compressed_data_returns_error() {
        // Valid base64 but not valid deflate
        let bogus = base64::engine::general_purpose::STANDARD.encode(b"this is not deflated");
        let result = decompress_sdp(&bogus);
        assert!(result.is_err());
    }
}
