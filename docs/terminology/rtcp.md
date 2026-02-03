# RTCP - RTP Control Protocol

Companion protocol to RTP providing reception quality feedback and synchronization.

## Packet Types

- **SR** - Sender Report
- **RR** - Receiver Report
- **SDES** - Source Description
- **BYE** - Goodbye
- **APP** - Application-specific

## Feedback Types

- **NACK** - Negative Acknowledgment (request retransmission)
- **PLI** - Picture Loss Indication
- **FIR** - Full Intra Request
- **REMB** - Receiver Estimated Maximum Bitrate
- **Transport-CC** - Transport-wide Congestion Control

## Related Terms

- [RTP](rtp.md)
- [RTCP-mux](rtcp-mux.md)
- [BWE](bwe.md)
