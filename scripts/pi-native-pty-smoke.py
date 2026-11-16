#!/usr/bin/env python3
"""Smoke-test the actual ARM64 Pager -> ACP -> Pi JSONL RPC -> TUI path.

Requires a native macOS ARM64 host. Uses the deterministic Pi fixture instead
of a real account or network, and keeps the user's Grok home untouched.
"""
from __future__ import annotations

import argparse
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
MOCK = ROOT / "crates/codegen/pi-grok-adapter/tests/mock_pi_rpc.py"


def readable(data: bytes) -> str:
    text = data.decode("utf-8", "replace")
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    return re.sub(r"\x1b\][^\x07]*(?:\x07|\x1b\\)", "", text)


def smoke(binary: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="grok-pi-rpc-smoke-") as tmp:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 38, 125, 0, 0))
        env = dict(os.environ, TERM="xterm-256color", GROK_HOME=tmp + "/home", PI_OFFLINE="1")
        cmd = [
            str(binary), "--pi-bin", sys.executable, "--pi-prefix-arg", str(MOCK),
            "--pi-cwd", tmp, "--offline", "--no-extensions",
            "--no-bridge-extensions", "--no-skills", "--no-context-files",
            "--approve", "--no-alt-screen",
        ]
        proc = subprocess.Popen(
            cmd, stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True
        )
        os.close(slave)
        output = bytearray()

        def receive(seconds: float, marker: bytes | None = None) -> bool:
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if marker and marker in output:
                    return True
                if proc.poll() is not None:
                    break
                ready, _, _ = select.select([master], [], [], 0.2)
                if ready:
                    try:
                        output.extend(os.read(master, 8192))
                    except OSError:
                        break
            return marker in output if marker else True

        try:
            if not receive(30, b"Mock Pi"):
                raise AssertionError("Pi RPC did not initialize the native TUI")
            # Wait for the first fully rendered prompt; avoid racing startup UI updates.
            receive(2)
            os.write(master, b"Hello mocked Pi\r")
            if not receive(35, b"Codemode"):
                raise AssertionError("mocked Pi prompt did not reach nested tool rendering")
            receive(2)
            text = readable(bytes(output))
            for marker in ("Mock Pi", "Codemode", "README", "line one", "line two"):
                if marker not in text:
                    raise AssertionError(f"missing rendered marker: {marker}")
            if "panicked at" in text:
                raise AssertionError("Rust panic in terminal output")
            print("PASS: ARM64 native Pager + mocked Pi RPC bootstrap, prompt, tools, UI")
        except Exception:
            print("Terminal output tail:", repr(readable(bytes(output))[-1500:]), file=sys.stderr)
            raise
        finally:
            if proc.poll() is None:
                os.write(master, b"\x03")
                receive(2)
            if proc.poll() is None:
                proc.terminate()
            try:
                proc.wait(timeout=4)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
            os.close(master)
        if proc.returncode != 0:
            raise AssertionError(f"Pager did not exit cleanly: {proc.returncode}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "binary", type=Path, nargs="?",
        default=ROOT / "target/aarch64-apple-darwin/release-dist/grok-pi",
    )
    smoke(parser.parse_args().binary.resolve())
