# PT - Payload Type

7-bit field in RTP header identifying the codec format of the payload.

## Common Payload Types

| PT | Codec | Sample Rate | Notes |
|----|-------|-------------|-------|
| 0 | PCMU | 8000 Hz | G.711 mu-law |
| 8 | PCMA | 8000 Hz | G.711 A-law |
| 13 | CN | 8000 Hz | Comfort Noise |
| 103 | SILK | 8000 Hz | Narrowband |
| 104 | SILK | 16000 Hz | Wideband (primary) |
| 114 | x-msrta | 16000 Hz | Fixed-rate SILK |
| 115 | x-msrta | 8000 Hz | Fixed-rate SILK |

## Related Terms

- [RTP](rtp.md)
- [SDP](sdp.md)
- [fmtp](fmtp.md)
