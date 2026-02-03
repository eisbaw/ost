# STUN - Session Traversal Utilities for NAT

Protocol (RFC 5389) for NAT traversal. Used to discover public IP addresses and port mappings.

## Message Types

- **Binding Request** - Ask server for reflexive address
- **Binding Response** - Server returns observed address

## Usage in ICE

STUN is used to gather server-reflexive candidates and for connectivity checks between ICE candidates.

## Related Terms

- [ICE](ice.md)
- [TURN](turn.md)
- [NAT](nat.md)
