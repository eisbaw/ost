# FU-A - Fragmentation Unit Type A

H.264 NAL unit type (28) for fragmenting large NAL units across multiple RTP packets when they exceed the MTU.

## Structure

```
+---------------+---------------+
| FU Indicator  | FU Header     | Payload...
+---------------+---------------+
| Type=28       | S|E|R| Type   |
+---------------+---------------+
```

- **S** - Start bit (first fragment)
- **E** - End bit (last fragment)
- **R** - Reserved
- **Type** - Original NAL unit type

## Related Terms

- [NAL](nal.md)
- [H.264](h264.md)
- [RTP](rtp.md)
