# NACK - Negative Acknowledgment

RTCP feedback message requesting retransmission of lost RTP packets.

## Format

Contains a list of lost sequence numbers that the sender should retransmit.

## Limitations

Only effective for low-latency scenarios where retransmission can arrive before the playout deadline.

## Related Terms

- [RTCP](rtcp.md)
- [FEC](fec.md)
- [PLC](plc.md)
