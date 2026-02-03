# PLC - Packet Loss Concealment

Audio processing technique to mask the effects of lost packets by interpolating, repeating, or extrapolating audio.

## Techniques

- **Repetition** - Repeat last good frame
- **Interpolation** - Blend adjacent frames
- **Extrapolation** - Predict based on recent audio
- **Codec-specific** - Built into codec (e.g., SILK FEC)

## Related Terms

- [FEC](fec.md)
- [NACK](nack.md)
