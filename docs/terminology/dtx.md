# DTX - Discontinuous Transmission

Voice codec feature that reduces or stops transmission during silence periods to save bandwidth.

## How It Works

1. VAD detects silence
2. Encoder stops sending speech frames
3. CN packets sent periodically with noise parameters
4. Receiver generates comfort noise locally

## Related Terms

- [VAD](vad.md)
- [CN](cn.md)
- [CNG](cng.md)
