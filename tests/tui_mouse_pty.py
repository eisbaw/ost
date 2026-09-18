#!/usr/bin/env python3
"""Verify real SGR mouse input and terminal cleanup using an offline Rust fixture.

Run from the repository root: python3 tests/tui_mouse_pty.py
Requires a Unix PTY and Python 3; never opens a Teams connection.
"""

import fcntl
import json
import os
import pty
import select
import struct
import subprocess
import termios
import time


def test_executable():
    build = subprocess.run(
        ["cargo", "test", "--locked", "--no-run", "--message-format=json"],
        capture_output=True, text=True, check=True,
    )
    for line in build.stdout.splitlines():
        artifact = json.loads(line)
        if artifact.get("executable") and artifact.get("profile", {}).get("test"):
            return artifact["executable"]
    raise RuntimeError("cargo did not produce a test executable")


def check_session(executable, mode):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
    before = termios.tcgetattr(slave)
    process = subprocess.Popen(
        [executable, "tui::app::mouse_tests::terminal_session_fixture", "--exact", "--ignored", "--nocapture"],
        stdin=slave, stdout=slave, stderr=slave,
        env={**os.environ, "TERM": "xterm-256color", "OST_MOUSE_FIXTURE_MODE": mode},
    )
    output = bytearray()

    def read_until(predicate):
        deadline = time.monotonic() + 10
        while not predicate():
            if time.monotonic() >= deadline:
                raise AssertionError(f"{mode}: timed out; output={output!r}")
            if select.select([master], [], [], 0.05)[0]:
                output.extend(os.read(master, 65536))

    try:
        # Wait for the compose placeholder in the first completed frame.
        read_until(lambda: b"Type a message" in output)
        # Click compose, type, then Tab back to sidebar. SGR coordinates are one based.
        os.write(master, b"\x1b[<0;40;22M\x1b[<0;40;22m")
        os.write(master, b"mouse draft\t")
        # One wheel notch scrolls the sidebar three rows without changing focus.
        os.write(master, b"\x1b[<65;5;8M")
        os.write(master, b"q")
        read_until(lambda: process.poll() is not None)
        while select.select([master], [], [], 0.05)[0]:
            output.extend(os.read(master, 65536))
        assert process.returncode == (101 if mode == "panic" else 0), output.decode(errors="replace")
        for code in (1000, 1002, 1003, 1015, 1006):
            enable, disable = f"\x1b[?{code}h".encode(), f"\x1b[?{code}l".encode()
            assert enable in output and disable in output, (mode, code)
            assert output.rfind(disable) > output.find(enable), (mode, code)
        assert b"\x1b[?1049l" in output, "alternate screen was not restored"
        assert termios.tcgetattr(slave) == before, "terminal attributes were not restored"
        if mode == "panic":
            assert b"fixture panic" in output, "the intended panic was not reached"
        print(f"{mode}: SGR clicks/wheel and terminal restoration passed")
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()
        os.close(master)
        os.close(slave)


if __name__ == "__main__":
    binary = test_executable()
    for exit_mode in ("normal", "error", "panic"):
        check_session(binary, exit_mode)
