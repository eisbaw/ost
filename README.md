# Teams CLI

A command-line client for Microsoft Teams written in Rust.

## Features

- **Authentication**: OAuth2 device code flow for work/school and personal accounts
- **Messaging**: List chats, read messages, send messages (stable)
- **Teams**: List joined teams and channels (stable)
- **Real-time**: WebSocket connection for push notifications (Trouter)
- **Calling**: Audio and video calls with RTP/SRTP media
- **Audio** (optional): Microphone capture and speaker playback (working)
- **Video** (optional): Camera capture via V4L2 and SDL2 display (WIP)

## Status

| Feature | Status |
|---------|--------|
| Authentication | Stable |
| Chat / Messaging | Stable |
| Teams / Channels | Stable |
| Trouter (push) | Stable |
| Audio calls | Working |
| Video calls | WIP - may cause audio issues |

**Note**: Video support is work-in-progress. Building with `--features video-capture` may interfere with audio functionality. For reliable audio calls, use `--features audio` only.

## Requirements

- Rust 1.70+
- Linux (for audio/video features)
- Nix (recommended) or manual dependency installation

### Dependencies

- **Audio**: ALSA development libraries
- **Video**: V4L2, SDL2, OpenH264

## Installation

### Using Nix (recommended)

```bash
nix-shell
just build
```

### Manual

Install dependencies, then:

```bash
cargo build
```

For audio support:
```bash
cargo build --features audio
```

For video support:
```bash
cargo build --features video-capture
```

For full A/V support:
```bash
cargo build --features "audio,video-capture"
```

## Usage

### Authentication

Login with device code flow:

```bash
teams-cli login
```

Check authentication status:

```bash
teams-cli status
teams-cli whoami
```

### Messaging

List recent chats:

```bash
teams-cli chats
```

Read messages from a chat:

```bash
teams-cli read <chat-id> --limit 20
```

Send a message:

```bash
teams-cli send --to <chat-id> "Hello from CLI!"
```

### Teams

List joined teams and channels:

```bash
teams-cli teams
```

### Real-time Notifications

Connect to Trouter for push notifications:

```bash
teams-cli trouter
```

### Calling

Test microphone (requires `--features audio`):

```bash
teams-cli mic-test
```

Test camera (requires `--features video-capture`):

```bash
teams-cli cam-test
```

Place a test call to Echo bot:

```bash
teams-cli call-test --echo --duration 20
```

## CLI Reference

```
teams-cli [OPTIONS] <COMMAND>

Options:
  -v, --verbose  Enable debug logging

Commands:
  login      OAuth2 device code authentication
  logout     Clear stored credentials
  status     Show token expiry status
  whoami     Verify authentication
  chats      List recent chats
  read       Read messages from a chat
  send       Send a message
  teams      List joined teams and channels
  presence   Get/set presence status
  trouter    Connect to push notification service
  call-test  Place a test call
  mic-test   Test microphone (audio feature)
  cam-test   Test camera (video-capture feature)
```

## Configuration

Tokens are stored in `~/.config/teams-cli/config.toml` with restricted permissions (0600).

## Documentation

See the `docs/` folder for:
- `terminology/` - Glossary of protocols and terms (RTP, SRTP, ICE, SDP, etc.)
- `GUIDs.md` - Reference of known Microsoft GUIDs

## License

MIT License - see LICENSE file.

## Disclaimer

This is an unofficial client. Use at your own risk. Not affiliated with Microsoft.

## Related Projects

- [purple-teams](https://github.com/EionRobb/purple-teams/) - Teams plugin for libpurple (Pidgin, Finch, etc.)
