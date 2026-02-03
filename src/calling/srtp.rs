//! SRTP encryption/decryption using AES-128-CM with HMAC-SHA1-80 (RFC 3711).
//!
//! Key derivation and packet protection for RTP streams using the SRTP profile
//! specified in the SDP `a=crypto` line: `AES_CM_128_HMAC_SHA1_80`.

use aes::cipher::{KeyIvInit, StreamCipher};
use anyhow::{bail, Context, Result};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;

use super::rtp;

type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;
type HmacSha1 = Hmac<Sha1>;

/// SRTP auth tag length for HMAC-SHA1-80 (80 bits = 10 bytes).
pub const SRTP_AUTH_TAG_LEN: usize = 10;

/// Master key length for AES-128 (16 bytes).
const MASTER_KEY_LEN: usize = 16;

/// Master salt length (14 bytes per RFC 3711).
const MASTER_SALT_LEN: usize = 14;

/// Total keying material: 16 bytes key + 14 bytes salt = 30 bytes.
const KEYING_MATERIAL_LEN: usize = MASTER_KEY_LEN + MASTER_SALT_LEN;

/// SRTP key derivation labels (RFC 3711, section 4.3.1).
const LABEL_CIPHER_KEY: u8 = 0x00;
const LABEL_AUTH_KEY: u8 = 0x01;
const LABEL_SALT: u8 = 0x02;

/// SRTCP key derivation labels (RFC 3711, section 3.4).
const LABEL_SRTCP_CIPHER_KEY: u8 = 0x03;
const LABEL_SRTCP_AUTH_KEY: u8 = 0x04;
const LABEL_SRTCP_SALT: u8 = 0x05;

/// Parsed SRTP keying material from an SDP crypto line.
#[derive(Debug, Clone)]
pub struct SrtpKeyingMaterial {
    pub master_key: [u8; MASTER_KEY_LEN],
    pub master_salt: [u8; MASTER_SALT_LEN],
    pub tag: u32, // crypto tag number from SDP
}

/// Derived session keys for SRTP.
#[derive(Debug, Clone)]
pub struct SrtpSessionKeys {
    pub cipher_key: [u8; 16],
    pub auth_key: [u8; 20], // HMAC-SHA1 uses 160-bit key
    pub salt: [u8; 14],
}

/// SRTP context for encrypting/decrypting packets on a single stream.
#[derive(Debug, Clone)]
pub struct SrtpContext {
    pub local_keys: SrtpSessionKeys,
    pub remote_keys: SrtpSessionKeys,
    /// Rollover counter for outbound packets.
    pub local_roc: u32,
    /// Rollover counter for inbound packets.
    pub remote_roc: u32,
    /// Highest sequence number seen inbound.
    pub remote_highest_seq: u16,
    /// SRTCP session keys for local (outbound) RTCP.
    pub local_srtcp_keys: SrtpSessionKeys,
    /// SRTCP session keys for remote (inbound) RTCP.
    pub remote_srtcp_keys: SrtpSessionKeys,
    /// SRTCP index for outbound packets (31-bit).
    pub local_srtcp_index: u32,
    /// SRTCP index for inbound packets (31-bit).
    pub remote_srtcp_index: u32,
}

/// Parse an SDP crypto line to extract SRTP keying material.
///
/// Formats handled:
/// - `a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:<base64key>|2^31|1:1`
/// - `a=crypto:3 AES_CM_128_HMAC_SHA1_80 inline:<base64key>|2^31`
/// - `a=cryptoscale:1 client AES_CM_128_HMAC_SHA1_80 inline:<base64key>|2^31|1:1`
pub fn parse_crypto_line(line: &str) -> Result<SrtpKeyingMaterial> {
    let line = line.trim();

    // Extract the tag number and find the inline key
    let tag = if line.starts_with("a=crypto:") {
        let rest = &line["a=crypto:".len()..];
        let tag_end = rest.find(' ').context("malformed crypto line")?;
        rest[..tag_end].parse::<u32>().context("bad crypto tag")?
    } else if line.starts_with("a=cryptoscale:") {
        let rest = &line["a=cryptoscale:".len()..];
        let tag_end = rest.find(' ').context("malformed cryptoscale line")?;
        rest[..tag_end]
            .parse::<u32>()
            .context("bad cryptoscale tag")?
    } else {
        bail!("not a crypto line: {}", line);
    };

    // Find "inline:" prefix
    let inline_pos = line
        .find("inline:")
        .context("no inline: key in crypto line")?;
    let key_part = &line[inline_pos + "inline:".len()..];

    // Key is everything up to the first '|' or end of string
    let b64_key = if let Some(pipe) = key_part.find('|') {
        &key_part[..pipe]
    } else {
        key_part
    };

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64_key)
        .context("failed to base64 decode SRTP key")?;

    if decoded.len() < KEYING_MATERIAL_LEN {
        bail!(
            "SRTP keying material too short: {} bytes (need {})",
            decoded.len(),
            KEYING_MATERIAL_LEN
        );
    }

    let mut master_key = [0u8; MASTER_KEY_LEN];
    let mut master_salt = [0u8; MASTER_SALT_LEN];
    master_key.copy_from_slice(&decoded[..MASTER_KEY_LEN]);
    master_salt.copy_from_slice(&decoded[MASTER_KEY_LEN..KEYING_MATERIAL_LEN]);

    Ok(SrtpKeyingMaterial {
        master_key,
        master_salt,
        tag,
    })
}

/// Derive session keys from master key + salt using AES-128-CM PRF (RFC 3711, 4.3.1).
///
/// key_derivation_rate = 0 (default), so index DIV key_derivation_rate = 0.
pub fn derive_session_keys(material: &SrtpKeyingMaterial) -> Result<SrtpSessionKeys> {
    let cipher_key = prf_derive(
        &material.master_key,
        &material.master_salt,
        LABEL_CIPHER_KEY,
        16,
    )?;
    let auth_key = prf_derive(
        &material.master_key,
        &material.master_salt,
        LABEL_AUTH_KEY,
        20,
    )?;
    let salt = prf_derive(&material.master_key, &material.master_salt, LABEL_SALT, 14)?;

    let mut ck = [0u8; 16];
    let mut ak = [0u8; 20];
    let mut s = [0u8; 14];
    ck.copy_from_slice(&cipher_key);
    ak.copy_from_slice(&auth_key);
    s.copy_from_slice(&salt);

    Ok(SrtpSessionKeys {
        cipher_key: ck,
        auth_key: ak,
        salt: s,
    })
}

/// Derive SRTCP session keys from master key + salt (RFC 3711, labels 0x03-0x05).
pub fn derive_srtcp_session_keys(material: &SrtpKeyingMaterial) -> Result<SrtpSessionKeys> {
    let cipher_key = prf_derive(
        &material.master_key,
        &material.master_salt,
        LABEL_SRTCP_CIPHER_KEY,
        16,
    )?;
    let auth_key = prf_derive(
        &material.master_key,
        &material.master_salt,
        LABEL_SRTCP_AUTH_KEY,
        20,
    )?;
    let salt = prf_derive(
        &material.master_key,
        &material.master_salt,
        LABEL_SRTCP_SALT,
        14,
    )?;

    let mut ck = [0u8; 16];
    let mut ak = [0u8; 20];
    let mut s = [0u8; 14];
    ck.copy_from_slice(&cipher_key);
    ak.copy_from_slice(&auth_key);
    s.copy_from_slice(&salt);

    Ok(SrtpSessionKeys {
        cipher_key: ck,
        auth_key: ak,
        salt: s,
    })
}

/// PRF for key derivation: AES-128-CM with label and index=0 (RFC 3711, 4.3.1).
fn prf_derive(
    master_key: &[u8; MASTER_KEY_LEN],
    master_salt: &[u8; MASTER_SALT_LEN],
    label: u8,
    output_len: usize,
) -> Result<Vec<u8>> {
    // x = label || 0x000000 (padded to 14 bytes, with label at byte index 7)
    // IV = (master_salt XOR x) || 0x0000
    let mut x = [0u8; 14];
    x[7] = label;

    let mut iv = [0u8; 16]; // AES block size
    for i in 0..14 {
        iv[i] = master_salt[i] ^ x[i];
    }
    // iv[14] and iv[15] are 0 (counter starts at 0)

    // Encrypt zeros with AES-128-CTR to produce key stream
    let mut output = vec![0u8; output_len];
    let mut cipher = Aes128Ctr::new(master_key.into(), &iv.into());
    cipher.apply_keystream(&mut output);

    Ok(output)
}

/// Create an SRTP context from local and remote keying material.
pub fn create_context(
    local_material: &SrtpKeyingMaterial,
    remote_material: &SrtpKeyingMaterial,
) -> Result<SrtpContext> {
    let local_keys = derive_session_keys(local_material)?;
    let remote_keys = derive_session_keys(remote_material)?;
    let local_srtcp_keys = derive_srtcp_session_keys(local_material)?;
    let remote_srtcp_keys = derive_srtcp_session_keys(remote_material)?;

    Ok(SrtpContext {
        local_keys,
        remote_keys,
        local_roc: 0,
        remote_roc: 0,
        remote_highest_seq: 0,
        local_srtcp_keys,
        remote_srtcp_keys,
        local_srtcp_index: 0,
        remote_srtcp_index: 0,
    })
}

/// Encrypt an RTP packet using SRTP (AES-128-CM + HMAC-SHA1-80).
///
/// Returns the SRTP packet: RTP header || encrypted payload || auth tag (10 bytes).
pub fn protect(ctx: &mut SrtpContext, rtp_packet: &[u8]) -> Result<Vec<u8>> {
    let header_len = rtp::full_header_len(rtp_packet)
        .context("RTP packet too short for SRTP protection")?;

    let header = &rtp_packet[..header_len];
    let payload = &rtp_packet[header_len..];

    // Extract SSRC and sequence number from header
    let ssrc = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
    let seq = u16::from_be_bytes([header[2], header[3]]);

    // Build IV for AES-128-CM (RFC 3711, 4.1.1)
    let iv = build_iv(&ctx.local_keys.salt, ssrc, ctx.local_roc, seq);

    // Encrypt payload with AES-128-CM
    let mut encrypted_payload = payload.to_vec();
    let mut cipher = Aes128Ctr::new((&ctx.local_keys.cipher_key).into(), &iv.into());
    cipher.apply_keystream(&mut encrypted_payload);

    // Build SRTP packet: header || encrypted payload
    let mut srtp_packet =
        Vec::with_capacity(header_len + encrypted_payload.len() + SRTP_AUTH_TAG_LEN);
    srtp_packet.extend_from_slice(header);
    srtp_packet.extend_from_slice(&encrypted_payload);

    // Compute auth tag over header || encrypted payload || ROC
    let auth_tag = compute_auth_tag(&ctx.local_keys.auth_key, &srtp_packet, ctx.local_roc);
    srtp_packet.extend_from_slice(&auth_tag);

    // Update ROC on sequence wrap
    if seq == 0xFFFF {
        ctx.local_roc = ctx.local_roc.wrapping_add(1);
    }

    Ok(srtp_packet)
}

/// Decrypt an SRTP packet, verifying the auth tag.
///
/// Returns the decrypted RTP packet (header + plaintext payload).
pub fn unprotect(ctx: &mut SrtpContext, srtp_packet: &[u8]) -> Result<Vec<u8>> {
    if srtp_packet.len() < rtp::RTP_HEADER_SIZE + SRTP_AUTH_TAG_LEN {
        bail!("SRTP packet too short");
    }

    let auth_tag_offset = srtp_packet.len() - SRTP_AUTH_TAG_LEN;
    let received_tag = &srtp_packet[auth_tag_offset..];
    let authenticated_portion = &srtp_packet[..auth_tag_offset];

    // Extract sequence number for ROC estimation
    let seq = u16::from_be_bytes([srtp_packet[2], srtp_packet[3]]);
    let ssrc = u32::from_be_bytes([
        srtp_packet[8],
        srtp_packet[9],
        srtp_packet[10],
        srtp_packet[11],
    ]);

    // Estimate ROC (simple: if seq < highest and diff is large, ROC may have incremented)
    let estimated_roc = estimate_roc(ctx.remote_roc, ctx.remote_highest_seq, seq);

    // Verify auth tag
    let expected_tag = compute_auth_tag(
        &ctx.remote_keys.auth_key,
        authenticated_portion,
        estimated_roc,
    );
    if received_tag != expected_tag.as_slice() {
        bail!("SRTP auth tag mismatch");
    }

    // Compute full header length (fixed header + CSRC + extensions)
    let header_len = rtp::full_header_len(&srtp_packet[..auth_tag_offset])
        .context("SRTP packet has truncated RTP header")?;

    // Decrypt payload (only bytes after the full header are encrypted)
    let header = &srtp_packet[..header_len];
    let encrypted_payload = &srtp_packet[header_len..auth_tag_offset];

    let iv = build_iv(&ctx.remote_keys.salt, ssrc, estimated_roc, seq);
    let mut decrypted = encrypted_payload.to_vec();
    let mut cipher = Aes128Ctr::new((&ctx.remote_keys.cipher_key).into(), &iv.into());
    cipher.apply_keystream(&mut decrypted);

    // Update ROC tracking
    if seq > ctx.remote_highest_seq || estimated_roc > ctx.remote_roc {
        ctx.remote_highest_seq = seq;
        ctx.remote_roc = estimated_roc;
    }

    let mut rtp_packet = Vec::with_capacity(header_len + decrypted.len());
    rtp_packet.extend_from_slice(header);
    rtp_packet.extend_from_slice(&decrypted);

    Ok(rtp_packet)
}

/// Build the AES-128-CM IV for SRTP (RFC 3711, 4.1.1).
///
/// IV = (session_salt XOR (SSRC || packet_index)) padded to 16 bytes.
/// Packet index = ROC << 16 | seq.
fn build_iv(salt: &[u8; 14], ssrc: u32, roc: u32, seq: u16) -> [u8; 16] {
    let mut iv = [0u8; 16];

    // SSRC goes at bytes 4-7 of the "label||r" construct
    let ssrc_bytes = ssrc.to_be_bytes();
    iv[4] = ssrc_bytes[0];
    iv[5] = ssrc_bytes[1];
    iv[6] = ssrc_bytes[2];
    iv[7] = ssrc_bytes[3];

    // Packet index (48-bit): ROC (32-bit) || SEQ (16-bit) at bytes 8-13
    let roc_bytes = roc.to_be_bytes();
    iv[8] = roc_bytes[0];
    iv[9] = roc_bytes[1];
    iv[10] = roc_bytes[2];
    iv[11] = roc_bytes[3];
    let seq_bytes = seq.to_be_bytes();
    iv[12] = seq_bytes[0];
    iv[13] = seq_bytes[1];

    // XOR with session salt (14 bytes)
    for i in 0..14 {
        iv[i] ^= salt[i];
    }

    // iv[14..16] = 0 (block counter starts at 0)
    iv
}

/// Compute HMAC-SHA1-80 auth tag over authenticated_portion || ROC.
fn compute_auth_tag(auth_key: &[u8; 20], authenticated_portion: &[u8], roc: u32) -> Vec<u8> {
    let mut mac = HmacSha1::new_from_slice(auth_key).expect("HMAC key length is valid");
    mac.update(authenticated_portion);
    mac.update(&roc.to_be_bytes());
    let result = mac.finalize().into_bytes();
    // Truncate to 80 bits (10 bytes)
    result[..SRTP_AUTH_TAG_LEN].to_vec()
}

/// Estimate ROC for incoming packet (RFC 3711, appendix A).
fn estimate_roc(current_roc: u32, highest_seq: u16, received_seq: u16) -> u32 {
    // If we haven't seen any packets yet, ROC is 0
    if highest_seq == 0 && current_roc == 0 {
        return 0;
    }

    let diff = (received_seq as i32) - (highest_seq as i32);
    if diff > 0 {
        // Normal progression
        current_roc
    } else if diff < -0x7FFF {
        // Sequence wrapped forward
        current_roc.wrapping_add(1)
    } else if diff > 0x7FFF {
        // Sequence wrapped backward (late packet from previous ROC)
        current_roc.wrapping_sub(1)
    } else {
        current_roc
    }
}

/// Minimum RTCP header size: V/P/RC(1) + PT(1) + length(2) + SSRC(4) = 8 bytes.
const RTCP_HEADER_SIZE: usize = 8;

/// Encrypt an RTCP packet using SRTCP (AES-128-CM + HMAC-SHA1-80, RFC 3711 §3.4).
///
/// Returns: `rtcp_header(8) || encrypted_payload || E||index(4) || auth_tag(10)`.
pub fn protect_rtcp(ctx: &mut SrtpContext, rtcp_packet: &[u8]) -> Result<Vec<u8>> {
    if rtcp_packet.len() < RTCP_HEADER_SIZE {
        bail!("RTCP packet too short for SRTCP protection");
    }

    let header = &rtcp_packet[..RTCP_HEADER_SIZE];
    let payload = &rtcp_packet[RTCP_HEADER_SIZE..];
    let ssrc = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
    let index = ctx.local_srtcp_index;

    // Build IV: salt XOR (SSRC at bytes 4-7, srtcp_index at bytes 8-11)
    let iv = build_srtcp_iv(&ctx.local_srtcp_keys.salt, ssrc, index);

    // Encrypt payload (header stays in the clear)
    let mut encrypted_payload = payload.to_vec();
    if !encrypted_payload.is_empty() {
        let mut cipher = Aes128Ctr::new((&ctx.local_srtcp_keys.cipher_key).into(), &iv.into());
        cipher.apply_keystream(&mut encrypted_payload);
    }

    // E flag (1) | srtcp_index (31 bits)
    let e_index: u32 = 0x8000_0000 | (index & 0x7FFF_FFFF);

    // Build output: header || encrypted_payload || E||index
    let mut srtcp =
        Vec::with_capacity(RTCP_HEADER_SIZE + encrypted_payload.len() + 4 + SRTP_AUTH_TAG_LEN);
    srtcp.extend_from_slice(header);
    srtcp.extend_from_slice(&encrypted_payload);
    srtcp.extend_from_slice(&e_index.to_be_bytes());

    // Auth tag over everything so far (header || encrypted_payload || E||index)
    let auth_tag = compute_srtcp_auth_tag(&ctx.local_srtcp_keys.auth_key, &srtcp);
    srtcp.extend_from_slice(&auth_tag);

    ctx.local_srtcp_index = index.wrapping_add(1) & 0x7FFF_FFFF;

    Ok(srtcp)
}

/// Decrypt an SRTCP packet, verifying the auth tag.
///
/// Returns the decrypted RTCP packet.
pub fn unprotect_rtcp(ctx: &mut SrtpContext, srtcp_packet: &[u8]) -> Result<Vec<u8>> {
    // Minimum: 8 (header) + 4 (E||index) + 10 (auth tag) = 22
    if srtcp_packet.len() < RTCP_HEADER_SIZE + 4 + SRTP_AUTH_TAG_LEN {
        bail!("SRTCP packet too short");
    }

    let auth_tag_offset = srtcp_packet.len() - SRTP_AUTH_TAG_LEN;
    let received_tag = &srtcp_packet[auth_tag_offset..];
    let authenticated_portion = &srtcp_packet[..auth_tag_offset];

    // Verify auth tag
    let expected_tag =
        compute_srtcp_auth_tag(&ctx.remote_srtcp_keys.auth_key, authenticated_portion);
    if received_tag != expected_tag.as_slice() {
        bail!("SRTCP auth tag mismatch");
    }

    // Extract E||index (4 bytes just before auth tag)
    let ei_offset = auth_tag_offset - 4;
    let e_index = u32::from_be_bytes([
        srtcp_packet[ei_offset],
        srtcp_packet[ei_offset + 1],
        srtcp_packet[ei_offset + 2],
        srtcp_packet[ei_offset + 3],
    ]);
    let encrypted = (e_index & 0x8000_0000) != 0;
    let srtcp_index = e_index & 0x7FFF_FFFF;

    let header = &srtcp_packet[..RTCP_HEADER_SIZE];
    let encrypted_payload = &srtcp_packet[RTCP_HEADER_SIZE..ei_offset];
    let ssrc = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);

    let mut decrypted = encrypted_payload.to_vec();
    if encrypted && !decrypted.is_empty() {
        let iv = build_srtcp_iv(&ctx.remote_srtcp_keys.salt, ssrc, srtcp_index);
        let mut cipher = Aes128Ctr::new((&ctx.remote_srtcp_keys.cipher_key).into(), &iv.into());
        cipher.apply_keystream(&mut decrypted);
    }

    // Update remote index tracking
    if srtcp_index >= ctx.remote_srtcp_index {
        ctx.remote_srtcp_index = srtcp_index.wrapping_add(1) & 0x7FFF_FFFF;
    }

    let mut rtcp = Vec::with_capacity(RTCP_HEADER_SIZE + decrypted.len());
    rtcp.extend_from_slice(header);
    rtcp.extend_from_slice(&decrypted);

    Ok(rtcp)
}

/// Build the AES-128-CM IV for SRTCP (RFC 3711, §4.1.1).
///
/// The 48-bit packet index field (bytes 8-13) holds the SRTCP index
/// right-aligned: bytes 8-9 = 0, bytes 10-13 = srtcp_index.
/// This matches SRTP where the field is (ROC<<16)|SEQ.
fn build_srtcp_iv(salt: &[u8; 14], ssrc: u32, srtcp_index: u32) -> [u8; 16] {
    let mut iv = [0u8; 16];

    let ssrc_bytes = ssrc.to_be_bytes();
    iv[4] = ssrc_bytes[0];
    iv[5] = ssrc_bytes[1];
    iv[6] = ssrc_bytes[2];
    iv[7] = ssrc_bytes[3];

    // SRTCP index in low 32 bits of the 48-bit packet index field (bytes 10-13)
    let idx_bytes = srtcp_index.to_be_bytes();
    iv[10] = idx_bytes[0];
    iv[11] = idx_bytes[1];
    iv[12] = idx_bytes[2];
    iv[13] = idx_bytes[3];

    // XOR with session salt (14 bytes)
    for i in 0..14 {
        iv[i] ^= salt[i];
    }

    iv
}

/// Compute HMAC-SHA1-80 auth tag for SRTCP (no ROC appended, unlike SRTP).
fn compute_srtcp_auth_tag(auth_key: &[u8; 20], authenticated_portion: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha1::new_from_slice(auth_key).expect("HMAC key length is valid");
    mac.update(authenticated_portion);
    let result = mac.finalize().into_bytes();
    result[..SRTP_AUTH_TAG_LEN].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_material() -> SrtpKeyingMaterial {
        // 30 bytes of deterministic test data
        let mut key = [0u8; 16];
        let mut salt = [0u8; 14];
        for i in 0..16 {
            key[i] = i as u8;
        }
        for i in 0..14 {
            salt[i] = (16 + i) as u8;
        }
        SrtpKeyingMaterial {
            master_key: key,
            master_salt: salt,
            tag: 2,
        }
    }

    #[test]
    fn test_parse_crypto_line() {
        // 30 bytes = 40 base64 chars
        let b64 = base64::engine::general_purpose::STANDARD.encode([0xABu8; 30]);
        let line = format!("a=crypto:2 AES_CM_128_HMAC_SHA1_80 inline:{}|2^31|1:1", b64);
        let mat = parse_crypto_line(&line).unwrap();
        assert_eq!(mat.tag, 2);
        assert_eq!(mat.master_key, [0xAB; 16]);
        assert_eq!(mat.master_salt, [0xAB; 14]);
    }

    #[test]
    fn test_parse_cryptoscale_line() {
        let b64 = base64::engine::general_purpose::STANDARD.encode([0xCD; 30]);
        let line = format!(
            "a=cryptoscale:1 client AES_CM_128_HMAC_SHA1_80 inline:{}|2^31|1:1",
            b64
        );
        let mat = parse_crypto_line(&line).unwrap();
        assert_eq!(mat.tag, 1);
    }

    #[test]
    fn test_key_derivation() {
        let mat = make_test_material();
        let keys = derive_session_keys(&mat).unwrap();
        // Just verify we get 16/20/14 bytes of non-trivial output
        assert_eq!(keys.cipher_key.len(), 16);
        assert_eq!(keys.auth_key.len(), 20);
        assert_eq!(keys.salt.len(), 14);
        // Keys should not be all zeros (AES-CTR of zeros with a real key)
        assert!(keys.cipher_key.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_protect_unprotect_roundtrip() {
        let mat = make_test_material();
        let mut ctx = create_context(&mat, &mat).unwrap();

        let payload = vec![0xFF; 160];
        let rtp = rtp::encode(rtp::PT_PCMU, 1, 160, 0xDEADBEEF, &payload);

        let srtp = protect(&mut ctx, &rtp).unwrap();
        assert_eq!(srtp.len(), rtp.len() + SRTP_AUTH_TAG_LEN);

        // The encrypted payload should differ from plaintext
        assert_ne!(
            &srtp[rtp::RTP_HEADER_SIZE..srtp.len() - SRTP_AUTH_TAG_LEN],
            payload.as_slice()
        );

        // Reset context for decrypt (same keys since local=remote in test)
        let mut ctx2 = create_context(&mat, &mat).unwrap();
        let decrypted = unprotect(&mut ctx2, &srtp).unwrap();
        assert_eq!(decrypted, rtp);
    }

    #[test]
    fn test_auth_tag_mismatch() {
        let mat = make_test_material();
        let mut ctx = create_context(&mat, &mat).unwrap();

        let rtp = rtp::encode(rtp::PT_PCMU, 1, 160, 0xDEADBEEF, &[0xFF; 160]);
        let mut srtp = protect(&mut ctx, &rtp).unwrap();

        // Corrupt the auth tag
        let len = srtp.len();
        srtp[len - 1] ^= 0xFF;

        let mut ctx2 = create_context(&mat, &mat).unwrap();
        assert!(unprotect(&mut ctx2, &srtp).is_err());
    }

    /// Build a minimal RTCP Sender Report (SR) packet for testing.
    fn make_test_rtcp_sr(ssrc: u32) -> Vec<u8> {
        let mut pkt = vec![0u8; 28]; // minimal SR: 8-byte header + 20-byte sender info
        pkt[0] = 0x80; // V=2, P=0, RC=0
        pkt[1] = 200; // PT=200 (SR)
                      // Length in 32-bit words minus 1: (28/4)-1 = 6
        pkt[2] = 0;
        pkt[3] = 6;
        let ssrc_bytes = ssrc.to_be_bytes();
        pkt[4..8].copy_from_slice(&ssrc_bytes);
        // Rest is zeros (NTP timestamp, RTP timestamp, counts) — fine for testing
        pkt
    }

    #[test]
    fn test_srtcp_key_derivation() {
        let mat = make_test_material();
        let srtp_keys = derive_session_keys(&mat).unwrap();
        let srtcp_keys = derive_srtcp_session_keys(&mat).unwrap();

        // SRTCP keys must differ from SRTP keys (different labels)
        assert_ne!(srtp_keys.cipher_key, srtcp_keys.cipher_key);
        assert_ne!(srtp_keys.auth_key, srtcp_keys.auth_key);
        assert_ne!(srtp_keys.salt, srtcp_keys.salt);

        // SRTCP keys should be non-trivial
        assert!(srtcp_keys.cipher_key.iter().any(|&b| b != 0));
        assert!(srtcp_keys.auth_key.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_protect_unprotect_rtcp_roundtrip() {
        let mat = make_test_material();
        let mut ctx = create_context(&mat, &mat).unwrap();

        let rtcp = make_test_rtcp_sr(0xCAFEBABE);
        let srtcp = protect_rtcp(&mut ctx, &rtcp).unwrap();

        // SRTCP = header(8) + encrypted_payload(20) + E||index(4) + auth_tag(10) = 42
        assert_eq!(srtcp.len(), rtcp.len() + 4 + SRTP_AUTH_TAG_LEN);

        // Header (first 8 bytes) should be unchanged
        assert_eq!(&srtcp[..8], &rtcp[..8]);

        // Encrypted payload should differ from plaintext
        assert_ne!(&srtcp[8..28], &rtcp[8..28]);

        // Decrypt
        let mut ctx2 = create_context(&mat, &mat).unwrap();
        let decrypted = unprotect_rtcp(&mut ctx2, &srtcp).unwrap();
        assert_eq!(decrypted, rtcp);
    }

    #[test]
    fn test_srtcp_auth_tag_mismatch() {
        let mat = make_test_material();
        let mut ctx = create_context(&mat, &mat).unwrap();

        let rtcp = make_test_rtcp_sr(0xDEADC0DE);
        let mut srtcp = protect_rtcp(&mut ctx, &rtcp).unwrap();

        // Corrupt the auth tag
        let len = srtcp.len();
        srtcp[len - 1] ^= 0xFF;

        let mut ctx2 = create_context(&mat, &mat).unwrap();
        assert!(unprotect_rtcp(&mut ctx2, &srtcp).is_err());
    }
}
