# REMB - Receiver Estimated Maximum Bitrate

RTCP feedback message communicating the receiver's bandwidth estimate to the sender.

## Format

```
RTCP APP packet with name "REMB"
Contains: bitrate estimate, SSRC list
```

## Usage

Receiver calculates available bandwidth and tells sender the maximum bitrate it can handle.

## Related Terms

- [BWE](bwe.md)
- [RTCP](rtcp.md)
- [GCC](gcc.md)
