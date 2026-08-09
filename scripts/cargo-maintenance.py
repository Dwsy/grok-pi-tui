#!/usr/bin/env python3
"""Bound Cargo target growth and periodically remove stale incremental caches."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

try:
    import fcntl
except ImportError:  # pragma: no cover - the supported dev hosts are POSIX
    fcntl = None


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def disk_free_bytes(path: Path) -> int:
    probe = path
    while not probe.exists() and probe != probe.parent:
        probe = probe.parent
    return shutil.disk_usage(probe).free


def target_size_bytes(target: Path) -> int:
    total = 0
    seen_files: set[tuple[int, int]] = set()
    stack = [target]
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
                        stack.append(Path(entry.path))
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


def incremental_roots(target: Path) -> list[Path]:
    roots: list[Path] = []
    for path in target.rglob("incremental"):
        if path.is_symlink() or not path.is_dir():
            continue
        try:
            if path.resolve().is_relative_to(target.resolve()):
                roots.append(path)
        except (OSError, ValueError):
            continue
    return roots


def stale_entries(target: Path, cutoff: float) -> list[Path]:
    candidates: list[Path] = []
    for root in incremental_roots(target):
        try:
            entries = root.iterdir()
        except OSError:
            continue
        for entry in entries:
            if entry.is_symlink() or not entry.is_dir():
                continue
            try:
                if entry.stat().st_mtime <= cutoff:
                    candidates.append(entry)
            except OSError:
                continue
    return sorted(candidates, key=lambda path: path.stat().st_mtime)


def clear_incremental_roots(target: Path, dry_run: bool) -> int:
    removed = 0
    for root in incremental_roots(target):
        if dry_run:
            print(f"cargo maintenance: would remove incremental root {root}")
            removed += 1
            continue
        try:
            shutil.rmtree(root)
            removed += 1
        except OSError as error:
            print(f"cargo maintenance: skipped {root}: {error}", file=sys.stderr)
    return removed


def enforce_target_cap(args: argparse.Namespace, target: Path) -> int:
    cap_bytes = args.max_target_gib * 1024**3
    size = target_size_bytes(target)
    if size < cap_bytes:
        return 0

    print(
        f"cargo maintenance: target uses {size / 1024**3:.1f} GiB, "
        f"above the {args.max_target_gib} GiB cap"
    )
    clear_incremental_roots(target, args.dry_run)
    if args.dry_run:
        print("cargo maintenance: would run cargo clean if the target remained over the cap")
        return 0

    size = target_size_bytes(target)
    if size < cap_bytes:
        print(f"cargo maintenance: target reduced to {size / 1024**3:.1f} GiB")
        return 0
    if not args.manifest_path:
        print("error: --manifest-path is required to clean an over-cap target", file=sys.stderr)
        return 2

    physical_target = Path(os.path.realpath(target))
    result = subprocess.run(
        [
            "cargo",
            "clean",
            "--manifest-path",
            args.manifest_path,
            "--target-dir",
            str(physical_target),
        ],
        check=False,
    )
    if result.returncode != 0:
        print("error: cargo clean failed while enforcing the target size cap", file=sys.stderr)
        return result.returncode
    physical_target.mkdir(parents=True, exist_ok=True)
    print(f"cargo maintenance: cleaned generated target output above {args.max_target_gib} GiB")
    return 0


def write_marker(marker: Path, timestamp: float) -> None:
    temporary = marker.with_name(f"{marker.name}.{os.getpid()}.tmp")
    temporary.write_text(f"{timestamp:.6f}\n", encoding="ascii")
    os.replace(temporary, marker)


def should_run(marker: Path, now: float, interval_seconds: int) -> bool:
    try:
        last_run = float(marker.read_text(encoding="ascii").strip())
    except (FileNotFoundError, ValueError, OSError):
        return True
    return now - last_run >= interval_seconds


def run(args: argparse.Namespace) -> int:
    target = Path(os.path.abspath(os.path.expanduser(args.target_dir)))
    if not target.is_dir():
        return 0

    lock_path = target / ".cargo-maintenance.lock"
    marker = target / ".cargo-maintenance.last"
    try:
        lock_file = lock_path.open("a+")
    except OSError:
        return 0

    try:
        if fcntl is not None:
            try:
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return 0

        cap_result = enforce_target_cap(args, target)
        if cap_result != 0:
            return cap_result

        now = time.time()
        if not args.force and not should_run(marker, now, args.interval_hours * 3600):
            return 0

        available = disk_free_bytes(target)
        age_days = args.stale_days
        if available < args.soft_free_gib * 1024**3:
            age_days = min(age_days, 1)
        cutoff = now - age_days * 24 * 60 * 60
        candidates = stale_entries(target, cutoff)[: args.max_dirs]

        removed = 0
        for entry in candidates:
            if args.dry_run:
                print(f"cargo maintenance: would remove {entry}")
                removed += 1
                continue
            try:
                shutil.rmtree(entry)
                removed += 1
            except OSError as error:
                print(f"cargo maintenance: skipped {entry}: {error}", file=sys.stderr)

        if not args.dry_run:
            try:
                write_marker(marker, now)
            except OSError:
                pass
        if removed:
            action = "would remove" if args.dry_run else "removed"
            print(f"cargo maintenance: {action} {removed} stale incremental cache(s)")
        return 0
    finally:
        lock_file.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", required=True)
    parser.add_argument("--manifest-path")
    parser.add_argument("--max-target-gib", type=positive_int, default=64)
    parser.add_argument("--interval-hours", type=positive_int, default=6)
    parser.add_argument("--stale-days", type=positive_int, default=7)
    parser.add_argument("--soft-free-gib", type=positive_int, default=40)
    parser.add_argument("--max-dirs", type=positive_int, default=8)
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
