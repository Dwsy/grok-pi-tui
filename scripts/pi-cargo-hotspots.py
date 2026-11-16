#!/usr/bin/env python3
"""Summarize real per-crate Cargo --timings HTML without external dependencies.

Usage: python3 scripts/pi-cargo-hotspots.py target/cargo-timings/cargo-timing.html
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re

MARKER = "const UNIT_DATA = "


def read_timings(path: Path) -> tuple[float, list[dict]]:
    page = path.read_text(encoding="utf-8")
    marker_index = page.find(MARKER)
    if marker_index < 0:
        raise ValueError(f"{path} lacks Cargo UNIT_DATA")
    units, _ = json.JSONDecoder().raw_decode(page[marker_index + len(MARKER):])
    total = re.search(r"^DURATION\s*=\s*([0-9.]+)\s*;", page, re.M)
    if total is None:
        raise ValueError(f"{path} lacks Cargo DURATION")
    return float(total.group(1)), units


def sections(unit: dict) -> dict[str, float]:
    return {
        name: round(data["end"] - data["start"], 2)
        for name, data in unit.get("sections") or []
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("file", type=Path, help="Cargo --timings HTML file")
    parser.add_argument("--limit", type=int, default=16)
    parser.add_argument("--focus", default=None, help="Only show this crate name")
    args = parser.parse_args()

    duration, units = read_timings(args.file)
    print(f"Wall-clock: {duration:.1f}s; units: {len(units)}")
    print("Crate / unit\tTotal (s)\tFrontend (s)\tCodegen (s)")
    for unit in sorted(units, key=lambda u: u.get("duration", 0), reverse=True):
        if args.focus and unit["name"] != args.focus:
            continue
        if args.limit <= 0:
            break
        phases = sections(unit)
        name = unit["name"]
        if unit["mode"] != "todo":
            name += f" [{unit['mode']}]"
        print(
            f"{name}\t{unit['duration']:.2f}\t"
            f"{phases.get('frontend', 0):.2f}\t{phases.get('codegen', 0):.2f}"
        )
        args.limit -= 1


if __name__ == "__main__":
    main()
