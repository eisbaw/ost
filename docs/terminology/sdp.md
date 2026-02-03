# SDP - Session Description Protocol

Text format (RFC 4566) describing multimedia session parameters including codecs, transport addresses, and encryption keys.

## Structure

```
v=0                           (version)
o=- 123 456 IN IP4 0.0.0.0    (origin)
s=-                           (session name)
t=0 0                         (timing)
m=audio 9 UDP/TLS/RTP/SAVPF 104 0  (media line)
a=rtpmap:104 SILK/16000       (codec mapping)
a=fmtp:104 useinbandfec=1     (format parameters)
a=crypto:1 AES_CM_128...      (SRTP keying)
```

## Key Attributes

- `a=rtpmap` - Payload type to codec mapping
- `a=fmtp` - Format-specific parameters
- `a=crypto` - SRTP key material
- `a=candidate` - ICE candidates
- `a=fingerprint` - DTLS certificate fingerprint

## Related Terms

- [PT](pt.md)
- [fmtp](fmtp.md)
- [ICE](ice.md)
