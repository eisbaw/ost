# ICE - Interactive Connectivity Establishment

NAT traversal framework (RFC 8445) for establishing peer-to-peer connections.

## States

1. **New** - Initial state
2. **Gathering** - Collecting candidates
3. **Checking** - Testing connectivity
4. **Connected** - At least one working pair
5. **Completed** - All checks done
6. **Failed** - No connectivity

## Candidate Types

- **host** - Local interface address
- **srflx** - Server reflexive (via STUN)
- **relay** - Relayed (via TURN)

## Related Terms

- [STUN](stun.md)
- [TURN](turn.md)
- [ICE-LITE](ice-lite.md)
