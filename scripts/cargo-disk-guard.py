#!/usr/bin/env python3
"""Run Cargo with free-space and total-target-size safety limits."""

from __future__ import annotations

import argparse
import os
import shutil
import signal
import subprocess
import sys
import time

LOW_SPACE_EXIT = 74


def existing_path(raw_path: str) -> str:
    path = os.path.abspath(os.path.expanduser(raw_path))
    while not os.path.exists(path) and path != os.path.dirname(path):
        path = os.path.dirname(path)
    return path


def filesystem_paths(raw_paths: list[str]) -> list[str]:
    paths: list[str] = []
    devices: set[int] = set()
    for raw_path in raw_paths:
        path = existing_path(raw_path)
        device = os.stat(path).st_dev
        if device not in devices:
            devices.add(device)
            paths.append(path)
    return paths


def free_bytes(path: str) -> int:
    return shutil.disk_usage(path).free


def directory_size_bytes(raw_path: str) -> int:
    path = os.path.abspath(os.path.expanduser(raw_path))
    if not os.path.isdir(path):
        return 0
    total = 0
    seen_files: set[tuple[int, int]] = set()
    stack = [path]
    while stack:
        current = stack.pop()
        try:
            entries = os.scandir(current)
        except OSError:
            continue
        with entries:
            for entry in entries:
                try:
                    if entry.is_symlink():
                        continue
                    if entry.is_dir(follow_symlinks=False):
                        stack.append(entry.path)
                        continue
                    if not entry.is_file(follow_symlinks=False):
                        continue
                    stat = entry.stat(follow_symlinks=False)
                except OSError:
                    continue
                identity = (stat.st_dev, stat.st_ino)
                if identity in seen_files:
                    continue
                seen_files.add(identity)
                total += stat.st_size
    return total


def low_space(paths: list[str], floor: int) -> tuple[str, int] | None:
    for path in paths:
        available = free_bytes(path)
        if available < floor:
            return path, available
    return None


def stop_process(process: subprocess.Popen[object]) -> None:
    try:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGTERM)
        else:
            process.terminate()
    except ProcessLookupError:
        return

    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        except ProcessLookupError:
            pass
        process.wait()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--path",
        action="append",
        required=True,
        help="Filesystem containing Cargo output or caches; repeat for more filesystems",
    )
    parser.add_argument("--min-free-gib", required=True, type=int)
    parser.add_argument("--target-path")
    parser.add_argument("--max-target-gib", type=int)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a command is required after --")
    if args.min_free_gib < 1:
        parser.error("--min-free-gib must be at least 1")
    if (args.target_path is None) != (args.max_target_gib is None):
        parser.error("--target-path and --max-target-gib must be provided together")
    if args.max_target_gib is not None and args.max_target_gib < 1:
        parser.error("--max-target-gib must be at least 1")

    floor = args.min_free_gib * 1024**3
    paths = filesystem_paths(args.path)
    blocked = low_space(paths, floor)
    if blocked:
        path, available = blocked
        print(
            f"error: Cargo blocked; only {available / 1024**3:.1f} GiB is free "
            f"(safety floor: {args.min_free_gib} GiB) on {path}",
            file=sys.stderr,
        )
        print("free space first, or clean only generated Cargo output", file=sys.stderr)
        return LOW_SPACE_EXIT

    target_cap = args.max_target_gib * 1024**3 if args.max_target_gib is not None else None
    target_size = 0
    target_probe = None
    target_free_baseline = 0
    if target_cap is not None and args.target_path is not None:
        target_size = directory_size_bytes(args.target_path)
        if target_size >= target_cap:
            print(
                f"error: Cargo blocked; target already uses {target_size / 1024**3:.1f} GiB "
                f"(cap: {args.max_target_gib} GiB)",
                file=sys.stderr,
            )
            return LOW_SPACE_EXIT
        target_probe = existing_path(args.target_path)
        target_free_baseline = free_bytes(target_probe)

    process = subprocess.Popen(command, start_new_session=(os.name == "posix"))
    try:
        while process.poll() is None:
            blocked = low_space(paths, floor)
            if blocked:
                path, available = blocked
                print(
                    f"error: Cargo stopped; {available / 1024**3:.1f} GiB remains "
                    f"on {path}, below the {args.min_free_gib} GiB free-space floor",
                    file=sys.stderr,
                )
                stop_process(process)
                return LOW_SPACE_EXIT
            if target_cap is not None and target_probe is not None and args.target_path is not None:
                current_free = free_bytes(target_probe)
                estimated_size = target_size + max(0, target_free_baseline - current_free)
                if estimated_size >= target_cap:
                    actual_size = directory_size_bytes(args.target_path)
                    if actual_size >= target_cap:
                        print(
                            f"error: Cargo stopped; target reached {actual_size / 1024**3:.1f} GiB "
                            f"(cap: {args.max_target_gib} GiB)",
                            file=sys.stderr,
                        )
                        stop_process(process)
                        return LOW_SPACE_EXIT
                    target_size = actual_size
                    target_free_baseline = current_free
            time.sleep(0.5)
    except KeyboardInterrupt:
        stop_process(process)
        return 130

    result = process.returncode
    blocked = low_space(paths, floor)
    if result == 0 and blocked:
        path, available = blocked
        print(
            f"error: Cargo finished below the {args.min_free_gib} GiB free-space floor "
            f"({available / 1024**3:.1f} GiB on {path})",
            file=sys.stderr,
        )
        return LOW_SPACE_EXIT
    if target_cap is not None and args.target_path is not None:
        actual_size = directory_size_bytes(args.target_path)
        if actual_size >= target_cap:
            print(
                f"error: Cargo finished with target at {actual_size / 1024**3:.1f} GiB "
                f"(cap: {args.max_target_gib} GiB)",
                file=sys.stderr,
            )
            return LOW_SPACE_EXIT
    return result


if __name__ == "__main__":
    raise SystemExit(main())
