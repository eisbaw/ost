# GCC - Google Congestion Control

Bandwidth estimation algorithm based on delay gradients. Detects congestion by monitoring inter-packet arrival time variations.

## How It Works

1. Measure one-way delay variations
2. Detect increasing delay trends (congestion signal)
3. Reduce sending rate when congestion detected
4. Probe for more bandwidth when stable

## Related Terms

- [BWE](bwe.md)
- [Transport-CC](transport-cc.md)
- [REMB](remb.md)
