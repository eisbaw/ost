//! RTCP packet building and parsing (RFC 3550).
//!
//! Builds compound RTCP packets (SR/RR + SDES) for periodic reporting,
//! and parses incoming RTCP packets to extract sender report timing info.

use std::time::{SystemTime, UNIX_EPOCH};

/// NTP epoch offset: seconds between 1900-01-01 and 1970-01-01.
const NTP_EPOCH_OFFSET: u64 = 2_208_988_800;

/// RTCP packet types (RFC 3550 section 12.1).
const PT_SR: u8 = 200;
const PT_RR: u8 = 201;
const PT_SDES: u8 = 202;

/// SDES item types.
const SDES_CNAME: u8 = 1;

/// Statistics for tracking received RTP packets (used to build RR blocks).
#[derive(Debug, Clone, Default)]
pub struct RtpRecvStats {
    pub packets_received: u32,
    pub highest_seq: u32, // extended highest sequence number
    pub jitter: u32,      // interarrival jitter (RFC 3550 A.8), fixed-point
    pub last_sr_ntp: u32, // middle 32 bits of NTP timestamp from last SR
    pub last_sr_recv_time: Option<std::time::Instant>,
    pub packets_lost: u32,
    pub expected_prior: u32,
    pub received_prior: u32,
    /// Previous transit time for jitter calculation.
    prev_transit: i64,
}

/// Statistics for tracking sent RTP packets (used to build SR).
#[derive(Debug, Clone, Default)]
pub struct RtpSendStats {
    pub packets_sent: u32,
    pub bytes_sent: u32,
    pub ssrc: u32,
    pub last_rtp_timestamp: u32,
}

/// Parsed RTCP block from incoming compound packet.
#[derive(Debug)]
pub enum RtcpBlock {
    SenderReport {
        ssrc: u32,
        ntp_timestamp: u64,
        rtp_timestamp: u32,
        sender_packet_count: u32,
        sender_octet_count: u32,
    },
    ReceiverReport {
        ssrc: u32,
    },
    Sdes,
    Unknown(u8),
}

/// Check if a UDP packet is RTCP (demux from RTP/STUN on same port).
///
/// RTCP packets have payload type 200-204 in byte[1].
/// RTP packets use payload types 0-127 (or with marker bit: 128-255 but PT field is 0-127).
/// We check byte[1] (which in RTP is M|PT, in RTCP is PT directly).
pub fn is_rtcp_packet(data: &[u8]) -> bool {
    if data.len() < 8 {
        return false;
    }
    let pt = data[1];
    pt >= 200 && pt <= 204
}

/// Get current NTP timestamp (seconds since 1900-01-01, 32.32 fixed point).
pub fn ntp_timestamp() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() + NTP_EPOCH_OFFSET;
    let frac = ((now.subsec_nanos() as u64) << 32) / 1_000_000_000;
    (secs << 32) | frac
}

/// Update interarrival jitter estimate (RFC 3550, appendix A.8).
///
/// `rtp_timestamp` is the RTP timestamp from the received packet.
/// `arrival_clock` is a monotonic clock value in the same units as RTP timestamp (8kHz for PCMU).
pub fn update_jitter(stats: &mut RtpRecvStats, rtp_timestamp: u32, arrival_clock: u32) {
    let transit = arrival_clock as i64 - rtp_timestamp as i64;
    if stats.prev_transit != 0 {
        let d = (transit - stats.prev_transit).unsigned_abs() as u32;
        // jitter += (1/16) * (|d| - jitter)
        stats.jitter = stats
            .jitter
            .wrapping_add(d.wrapping_sub(stats.jitter).wrapping_add(8) >> 4);
    }
    stats.prev_transit = transit;
}

/// Build a compound RTCP Sender Report + SDES packet.
pub fn build_sender_report(
    send_stats: &RtpSendStats,
    recv_stats: &RtpRecvStats,
    remote_ssrc: u32,
    cname: &str,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);

    let ntp = ntp_timestamp();
    let ntp_hi = (ntp >> 32) as u32;
    let ntp_lo = ntp as u32;

    // --- SR packet ---
    // RC=1 if we have a remote SSRC to report on, else RC=0
    let rc: u8 = if remote_ssrc != 0 && recv_stats.packets_received > 0 {
        1
    } else {
        0
    };

    // Header: V=2, P=0, RC, PT=200
    buf.push(0x80 | rc);
    buf.push(PT_SR);

    // Length placeholder (in 32-bit words minus 1)
    let len_pos = buf.len();
    buf.extend_from_slice(&[0, 0]); // filled later

    // SSRC of sender
    buf.extend_from_slice(&send_stats.ssrc.to_be_bytes());

    // NTP timestamp
    buf.extend_from_slice(&ntp_hi.to_be_bytes());
    buf.extend_from_slice(&ntp_lo.to_be_bytes());

    // RTP timestamp (corresponding to NTP time)
    buf.extend_from_slice(&send_stats.last_rtp_timestamp.to_be_bytes());

    // Sender's packet count
    buf.extend_from_slice(&send_stats.packets_sent.to_be_bytes());

    // Sender's octet count
    buf.extend_from_slice(&send_stats.bytes_sent.to_be_bytes());

    // Report block (if we have received packets from remote)
    if rc == 1 {
        // SSRC_1 (source being reported)
        buf.extend_from_slice(&remote_ssrc.to_be_bytes());

        // Fraction lost + cumulative lost
        let expected = recv_stats
            .highest_seq
            .wrapping_sub(recv_stats.expected_prior);
        let received_interval = recv_stats
            .packets_received
            .wrapping_sub(recv_stats.received_prior);
        let lost_interval = expected.saturating_sub(received_interval);
        let fraction = if expected > 0 {
            ((lost_interval as u64 * 256) / expected as u64) as u8
        } else {
            0
        };
        let cumulative_lost = recv_stats.packets_lost & 0x00FFFFFF;
        buf.push(fraction);
        buf.push((cumulative_lost >> 16) as u8);
        buf.push((cumulative_lost >> 8) as u8);
        buf.push(cumulative_lost as u8);

        // Extended highest sequence number received
        buf.extend_from_slice(&recv_stats.highest_seq.to_be_bytes());

        // Interarrival jitter
        buf.extend_from_slice(&recv_stats.jitter.to_be_bytes());

        // Last SR (middle 32 bits of NTP timestamp from last received SR)
        buf.extend_from_slice(&recv_stats.last_sr_ntp.to_be_bytes());

        // DLSR (delay since last SR, in 1/65536 seconds)
        let dlsr = if let Some(recv_time) = recv_stats.last_sr_recv_time {
            let elapsed = recv_time.elapsed();
            let secs = elapsed.as_secs() as u32;
            let frac = (elapsed.subsec_nanos() as u64 * 65536 / 1_000_000_000) as u32;
            (secs << 16) | frac
        } else {
            0
        };
        buf.extend_from_slice(&dlsr.to_be_bytes());
    }

    // Fill SR length: (bytes after header / 4) - 1... actually length = (total_bytes/4) - 1
    let sr_words = (buf.len() / 4) - 1;
    buf[len_pos] = (sr_words >> 8) as u8;
    buf[len_pos + 1] = sr_words as u8;

    // --- SDES packet ---
    append_sdes(&mut buf, send_stats.ssrc, cname);

    buf
}

/// Build a compound RTCP Receiver Report + SDES packet.
pub fn build_receiver_report(
    local_ssrc: u32,
    recv_stats: &RtpRecvStats,
    remote_ssrc: u32,
    cname: &str,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);

    let rc: u8 = if remote_ssrc != 0 && recv_stats.packets_received > 0 {
        1
    } else {
        0
    };

    // Header
    buf.push(0x80 | rc);
    buf.push(PT_RR);
    let len_pos = buf.len();
    buf.extend_from_slice(&[0, 0]);

    buf.extend_from_slice(&local_ssrc.to_be_bytes());

    if rc == 1 {
        buf.extend_from_slice(&remote_ssrc.to_be_bytes());

        let fraction: u8 = 0;
        let cumulative_lost = recv_stats.packets_lost & 0x00FFFFFF;
        buf.push(fraction);
        buf.push((cumulative_lost >> 16) as u8);
        buf.push((cumulative_lost >> 8) as u8);
        buf.push(cumulative_lost as u8);

        buf.extend_from_slice(&recv_stats.highest_seq.to_be_bytes());
        buf.extend_from_slice(&recv_stats.jitter.to_be_bytes());
        buf.extend_from_slice(&recv_stats.last_sr_ntp.to_be_bytes());

        let dlsr = if let Some(recv_time) = recv_stats.last_sr_recv_time {
            let elapsed = recv_time.elapsed();
            ((elapsed.as_secs() as u32) << 16)
                | (elapsed.subsec_nanos() as u64 * 65536 / 1_000_000_000) as u32
        } else {
            0
        };
        buf.extend_from_slice(&dlsr.to_be_bytes());
    }

    let rr_words = (buf.len() / 4) - 1;
    buf[len_pos] = (rr_words >> 8) as u8;
    buf[len_pos + 1] = rr_words as u8;

    append_sdes(&mut buf, local_ssrc, cname);

    buf
}

/// Append an SDES chunk to an RTCP compound packet.
fn append_sdes(buf: &mut Vec<u8>, ssrc: u32, cname: &str) {
    let sdes_start = buf.len();

    // Header: V=2, P=0, SC=1, PT=SDES
    buf.push(0x81);
    buf.push(PT_SDES);
    let len_pos = buf.len();
    buf.extend_from_slice(&[0, 0]);

    // SSRC/CSRC chunk
    buf.extend_from_slice(&ssrc.to_be_bytes());

    // CNAME item
    let cname_bytes = cname.as_bytes();
    buf.push(SDES_CNAME);
    buf.push(cname_bytes.len() as u8);
    buf.extend_from_slice(cname_bytes);

    // End item
    buf.push(0);

    // Pad to 4-byte boundary
    while (buf.len() - sdes_start) % 4 != 0 {
        buf.push(0);
    }

    let sdes_words = (buf.len() - sdes_start) / 4 - 1;
    buf[len_pos] = (sdes_words >> 8) as u8;
    buf[len_pos + 1] = sdes_words as u8;
}

/// Parse incoming RTCP compound packet into blocks.
pub fn parse_rtcp(data: &[u8]) -> Vec<RtcpBlock> {
    let mut blocks = Vec::new();
    let mut offset = 0;

    while offset + 4 <= data.len() {
        let pt = data[offset + 1];
        let length_words = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        let packet_len = (length_words + 1) * 4;

        if offset + packet_len > data.len() {
            break;
        }

        let pkt = &data[offset..offset + packet_len];

        match pt {
            PT_SR if pkt.len() >= 28 => {
                let ssrc = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
                let ntp_hi = u32::from_be_bytes([pkt[8], pkt[9], pkt[10], pkt[11]]);
                let ntp_lo = u32::from_be_bytes([pkt[12], pkt[13], pkt[14], pkt[15]]);
                let rtp_ts = u32::from_be_bytes([pkt[16], pkt[17], pkt[18], pkt[19]]);
                let pkt_count = u32::from_be_bytes([pkt[20], pkt[21], pkt[22], pkt[23]]);
                let oct_count = u32::from_be_bytes([pkt[24], pkt[25], pkt[26], pkt[27]]);

                blocks.push(RtcpBlock::SenderReport {
                    ssrc,
                    ntp_timestamp: ((ntp_hi as u64) << 32) | ntp_lo as u64,
                    rtp_timestamp: rtp_ts,
                    sender_packet_count: pkt_count,
                    sender_octet_count: oct_count,
                });
            }
            PT_RR if pkt.len() >= 8 => {
                let ssrc = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
                blocks.push(RtcpBlock::ReceiverReport { ssrc });
            }
            PT_SDES => {
                blocks.push(RtcpBlock::Sdes);
            }
            other => {
                blocks.push(RtcpBlock::Unknown(other));
            }
        }

        offset += packet_len;
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ntp_timestamp_reasonable() {
        let ntp = ntp_timestamp();
        let secs = ntp >> 32;
        // Should be well past year 2020 in NTP epoch
        assert!(secs > NTP_EPOCH_OFFSET + 50 * 365 * 86400);
    }

    #[test]
    fn test_is_rtcp_packet() {
        // SR: byte[1] = 200
        let sr = [0x80, 200, 0, 6, 0, 0, 0, 0];
        assert!(is_rtcp_packet(&sr));

        // RTP PCMU: byte[1] = 0
        let rtp = [0x80, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(!is_rtcp_packet(&rtp));
    }

    #[test]
    fn test_build_and_parse_sr() {
        let send = RtpSendStats {
            ssrc: 0x12345678,
            packets_sent: 100,
            bytes_sent: 16000,
            last_rtp_timestamp: 16000,
        };
        let recv = RtpRecvStats::default();
        let buf = build_sender_report(&send, &recv, 0, "test@example.com");

        // Should be parseable
        let blocks = parse_rtcp(&buf);
        assert!(!blocks.is_empty());
        match &blocks[0] {
            RtcpBlock::SenderReport {
                ssrc,
                sender_packet_count,
                ..
            } => {
                assert_eq!(*ssrc, 0x12345678);
                assert_eq!(*sender_packet_count, 100);
            }
            other => panic!("Expected SenderReport, got {:?}", other),
        }
    }

    #[test]
    fn test_build_and_parse_rr() {
        let recv = RtpRecvStats {
            packets_received: 50,
            highest_seq: 100,
            ..Default::default()
        };
        let buf = build_receiver_report(0xAABBCCDD, &recv, 0x11223344, "test@host");

        let blocks = parse_rtcp(&buf);
        assert!(!blocks.is_empty());
        match &blocks[0] {
            RtcpBlock::ReceiverReport { ssrc } => {
                assert_eq!(*ssrc, 0xAABBCCDD);
            }
            other => panic!("Expected ReceiverReport, got {:?}", other),
        }
    }

    #[test]
    fn test_update_jitter() {
        let mut stats = RtpRecvStats::default();
        // Simulate receiving packets with consistent timing
        for i in 0..10u32 {
            update_jitter(&mut stats, i * 160, i * 160);
        }
        // With perfect timing, jitter should be zero or very small
        assert!(stats.jitter < 10, "jitter={}", stats.jitter);
    }
}
