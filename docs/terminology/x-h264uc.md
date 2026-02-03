# X-H264UC - Microsoft Extended H.264 Unified Communications

Microsoft's proprietary H.264 SVC variant with temporal scalability. This is NOT standard H.264.

## Key Differences from Standard H.264

- `packetization-mode` fmtp has different semantics
- Uses temporal SVC layers
- Standard H.264 is rejected by Teams

## SDP Example

```
a=rtpmap:122 X-H264UC/90000
a=fmtp:122 packetization-mode=1;mst-mode=NI-TC
```

## Related Terms

- [H.264](h264.md)
- [SVC](svc.md)
