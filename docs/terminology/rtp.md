# RTP - Real-time Transport Protocol

Protocol (RFC 3550) for delivering audio and video over IP networks.

## Header Fields

- Version (2 bits)
- Padding (1 bit)
- Extension (1 bit)
- CSRC count (4 bits)
- Marker (1 bit)
- Payload Type (7 bits)
- Sequence number (16 bits)
- Timestamp (32 bits)
- SSRC (32 bits)

## Related Terms

- [RTCP](rtcp.md)
- [SRTP](srtp.md)
- [PT](pt.md)
- [SSRC](ssrc.md)
