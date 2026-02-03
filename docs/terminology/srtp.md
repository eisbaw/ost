# SRTP - Secure Real-time Transport Protocol

Encrypted RTP (RFC 3711). Provides confidentiality, message authentication, and replay protection for RTP traffic.

## Default Cipher Suite

Teams uses `AES_CM_128_HMAC_SHA1_80`:
- AES-128 in Counter Mode for encryption
- HMAC-SHA1 truncated to 80 bits for authentication

## Key Derivation

Keys derived from master key and master salt using RFC 3711 key derivation function.

## Related Terms

- [RTP](rtp.md)
- [DTLS-SRTP](dtls-srtp.md)
- [AES-CM](aes-cm.md)
