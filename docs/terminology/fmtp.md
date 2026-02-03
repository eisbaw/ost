# fmtp - Format Parameters

SDP attribute containing codec-specific parameters.

## Syntax

```
a=fmtp:<payload-type> <parameters>
```

## Examples

```
a=fmtp:104 useinbandfec=1
a=fmtp:122 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42C02A
```

## Related Terms

- [SDP](sdp.md)
- [PT](pt.md)
