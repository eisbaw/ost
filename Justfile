# Teams CLI - Justfile

# List available recipes
default:
    @just --list

# --- Build ---

# Build teams-cli (debug)
build:
    cargo build

# Build teams-cli (release)
build-release:
    cargo build --release

# Build with audio support (microphone/speaker)
build-audio:
    cargo build --features audio

# Build with video capture support (camera/display)
build-video:
    cargo build --features video-capture

# Build with full A/V support
build-full:
    cargo build --features "audio,video-capture"

# --- Quality ---

# Run clippy lints
lint:
    cargo clippy --all-targets --all-features

# Format code
fmt:
    cargo fmt

# Check formatting without changes
fmt-check:
    cargo fmt -- --check

# Run all quality checks
check: fmt-check lint
    cargo test --no-run

# --- Test ---

# Run unit tests
test:
    cargo test

# Run e2e tests (requires valid login session)
e2e: build
    ./tests/e2e_trouter.sh
    ./tests/e2e_read.sh
    ./tests/e2e_chats.sh
    ./tests/e2e_teams.sh

# Run call test (requires valid login + call service access)
e2e-call: build
    ./tests/e2e_echo123.sh

# --- Run ---

# Show CLI help
help:
    cargo run -- --help

# Login with device code flow
login:
    cargo run -- login

# Show authentication status
status:
    cargo run -- status

# Show current user info
whoami:
    cargo run -- whoami

# List recent chats
chats:
    cargo run -- chats

# List joined teams and channels
teams:
    cargo run -- teams

# Connect to Trouter for real-time notifications
trouter:
    cargo run -- trouter

# --- Audio/Video ---

# Test microphone (record 3s, playback)
mic-test: build-audio
    ./target/debug/teams-cli mic-test

# Test camera (capture 3s, display)
cam-test: build-video
    ./target/debug/teams-cli cam-test

# Place audio call to Echo bot (20s)
call-echo: build-audio
    ./target/debug/teams-cli call-test --echo --duration 20

# --- TUI ---

# Launch the terminal UI
tui: build
    ./target/debug/teams-cli tui

# Place A/V call to Echo bot with camera and display (20s)
call-echo-video: build-full
    ./target/debug/teams-cli call-test --echo --camera --display --duration 20
