# Socket.IO - Socket.IO Protocol

Real-time bidirectional communication protocol built on WebSocket.

## Frame Types

- `0` - Disconnect
- `1` - Connect
- `2` - Heartbeat
- `3` - Message
- `4` - JSON message
- `5` - Event
- `6` - Ack

## Trouter Usage

Trouter uses Socket.IO framing:
- `1::` - Connect acknowledgment
- `5:::` - Event with JSON payload
- `6:::` - Acknowledgment

## Related Terms

- [Trouter](trouter.md)
- [WSS](wss.md)
