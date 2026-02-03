# AEC - Acoustic Echo Cancellation

Audio processing algorithm that removes echoes caused by speaker output being picked up by the microphone.

## How It Works

1. Monitor speaker output (reference signal)
2. Estimate echo path to microphone
3. Subtract estimated echo from microphone input

## Importance

Essential for full-duplex audio communication without headphones.

## Related Terms

- [AGC](agc.md)
- [VAD](vad.md)
- [VQE](vqe.md)
