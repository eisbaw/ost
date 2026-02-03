# FEC - Forward Error Correction

Error correction technique that adds redundant data to allow recovery of lost packets without retransmission.

## Types

- **In-band FEC** - Redundancy embedded in codec stream (e.g., SILK `useinbandfec=1`)
- **Out-of-band FEC** - Separate FEC packets (e.g., ULP-FEC)

## Trade-offs

- Increases bitrate
- Reduces latency impact of loss
- More effective than NACK for high-latency links

## Related Terms

- [SILK](silk.md)
- [PLC](plc.md)
- [NACK](nack.md)
