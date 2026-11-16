#!/usr/bin/env python3
"""Offline Herdr integration smoke: real Pi RPC + ARM64 Pager + fake local socket.

Verifies the built-in Pi extension's session/idle lifecycle reporting without
modifying Herdr or contacting a model provider. This is NOT real Herdr acceptance.
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]


def smoke(binary: Path, pi_binary: str) -> None:
    with tempfile.TemporaryDirectory(prefix="pi-herdr-smoke-") as temporary:
        base = Path(temporary)
        socket_path = base / "herdr.sock"
        seen: list[dict] = []
        stop = threading.Event()
        ready = threading.Event()
        errors: list[str] = []

        def serve() -> None:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
                server.bind(str(socket_path))
                server.listen(5)
                server.settimeout(0.2)
                ready.set()
                while not stop.is_set():
                    try:
                        client, _ = server.accept()
                    except socket.timeout:
                        continue
                    except OSError:
                        break
                    with client:
                        client.settimeout(2)
                        try:
                            payload = bytearray()
                            while b"\n" not in payload and len(payload) < 65536:
                                chunk = client.recv(8192)
                                if not chunk:
                                    break
                                payload.extend(chunk)
                            request = json.loads(payload.split(b"\n", 1)[0])
                            seen.append(request)
                            response = {"id": request["id"], "result": {"ok": True}}
                            client.sendall(json.dumps(response).encode() + b"\n")
                        except (OSError, ValueError, KeyError) as exc:
                            errors.append(str(exc))

        thread = threading.Thread(target=serve, daemon=True)
        thread.start()
        if not ready.wait(3):
            raise RuntimeError("fake Herdr socket could not start")

        env = os.environ.copy()
        env.update({
            "TERM": "xterm-256color",
            "GROK_HOME": str(base / "grok-home"),
            "GROK_LEGACY_HOME": str(base / "no-legacy-config"),
            "PI_CODING_AGENT_DIR": str(base / "pi-home"),
            "PI_OFFLINE": "1",
            "HERDR_ENV": "1",
            "HERDR_SOCKET_PATH": str(socket_path),
            "HERDR_PANE_ID": "smoke-pane-isolated",
        })
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 38, 125, 0, 0))
        cmd = [
            str(binary), "--pi-bin", pi_binary, "--pi-cwd", temporary,
            "--no-extensions", "--no-skills", "--no-context-files",
            "--session-id", str(uuid.uuid4()),
            "--session-dir", str(base / "sessions"),
            "--offline", "--approve", "--no-alt-screen",
            "--model", "google/gemini-2.5-pro",
        ]
        proc = subprocess.Popen(
            cmd, stdin=slave, stdout=slave, stderr=slave,
            env=env, start_new_session=True,
        )
        os.close(slave)
        output = bytearray()
        try:
            deadline = time.monotonic() + 40
            while time.monotonic() < deadline and proc.poll() is None:
                methods = {request.get("method") for request in seen}
                states = {
                    request.get("params", {}).get("state")
                    for request in seen if request.get("method") == "pane.report_agent"
                }
                if "pane.report_agent_session" in methods and "idle" in states:
                    break
                available, _, _ = select.select([master], [], [], 0.2)
                if available:
                    try:
                        data = os.read(master, 8192)
                        output.extend(data[-8192:])
                        if len(output) > 128000:
                            del output[:-128000]
                    except OSError:
                        break
            methods = {req.get("method") for req in seen}
            states = [
                req.get("params", {}).get("state")
                for req in seen if req.get("method") == "pane.report_agent"
            ]
            session_reports = [
                req for req in seen if req.get("method") == "pane.report_agent_session"
            ]
            print(
                f"ALIVE={proc.poll() is None} METHODS={sorted(methods)} "
                f"STATES={states} SESSION_REPORTS={len(session_reports)} "
                f"SOCKET_ERRORS={errors}",
                flush=True,
            )
            if "pane.report_agent_session" not in methods or "idle" not in states:
                printable = re.sub(
                    r"\x1b\[[0-?]*[ -/]*[@-~]", "",
                    output.decode("utf-8", "replace"),
                )
                print("Terminal head:", repr(printable[:1800]), file=sys.stderr)
                print("Terminal tail:", repr(printable[-2000:]), file=sys.stderr)
                stderr_path = base / "grok-home/logs/pi-rpc-stderr.log"
                if stderr_path.is_file():
                    print("Pi stderr tail:", stderr_path.read_text(errors="replace")[-3000:], file=sys.stderr)
                raise AssertionError("Pi built-in Herdr extension did not report session and idle")
            for req in seen:
                params = req.get("params", {})
                assert params.get("source") == "herdr:pi", req
                assert params.get("agent") == "pi", req
                assert params.get("pane_id") == "smoke-pane-isolated", req
            print("PASS: real Pi RPC session + built-in Herdr local socket lifecycle")
        finally:
            if proc.poll() is None:
                try:
                    os.write(master, b"\x03")
                except OSError:
                    pass
                time.sleep(0.3)
            if proc.poll() is None:
                proc.terminate()
            try:
                proc.wait(timeout=4)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
            os.close(master)
            stop.set()
            thread.join(timeout=2)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--binary", type=Path,
        default=ROOT / "target/aarch64-apple-darwin/release-dist/grok-pi",
    )
    parser.add_argument("--pi-bin", default=shutil.which("pi"))
    args = parser.parse_args()
    if not args.pi_bin:
        parser.error("Pi executable not found")
    smoke(args.binary.resolve(), args.pi_bin)
