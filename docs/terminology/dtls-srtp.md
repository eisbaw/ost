# DTLS-SRTP - DTLS-based SRTP Key Exchange

Key exchange mechanism where DTLS handshake derives SRTP keys. More secure than inline SDP crypto because keys are not exposed in signaling.

## Flow

1. DTLS handshake over the media path
2. Extract keying material from DTLS
3. Derive SRTP keys using extracted material

## SDP Attribute

```
a=fingerprint:sha-256 <hash>
a=setup:actpass
```

## Related Terms

- [DTLS](dtls.md)
- [SRTP](srtp.md)
- [SDP](sdp.md)
