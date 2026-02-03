# Trouter - Trouter Push Service

Microsoft's real-time push notification service for Teams. Delivers incoming call notifications, messages, and other events via WebSocket.

## Endpoint

```
wss://go.trouter.teams.microsoft.com
```

## Protocol

- WebSocket with Socket.IO framing
- Message types: `1::` (connect), `5:::` (event), `6:::` (ack)
- Requires IC3 token authentication

## Notification Paths

- `msg` - Messages
- `call` - Incoming calls
- `callinfo` - Call state updates

## Related Terms

- [IC3](ic3.md)
- [Socket.IO](socketio.md)
- [Registrar](registrar.md)
