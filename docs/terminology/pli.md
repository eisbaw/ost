# PLI - Picture Loss Indication

RTCP feedback message indicating the decoder has lost synchronization due to lost video data and needs a keyframe.

## Difference from FIR

- **PLI** - Reports a problem (reactive)
- **FIR** - Requests action (imperative)

In practice, both result in keyframe generation.

## Related Terms

- [RTCP](rtcp.md)
- [FIR](fir.md)
- [NACK](nack.md)
