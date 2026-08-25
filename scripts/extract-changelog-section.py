#!/usr/bin/env python3
"""Extract Keep-a-Changelog section(s) from CHANGELOG.MD for GitHub Releases.

Usage:
  # Single version (default for tag releases)
  extract-changelog-section.py 0.0.8 CHANGELOG.MD -o notes.md --strict

  # All sections after a baseline (exclusive), up to and including target
  # e.g. tag v0.0.8 with --since 0.0.6 → emits 0.0.8 then 0.0.7 bodies
  extract-changelog-section.py 0.0.8 CHANGELOG.MD --since 0.0.6 -o notes.md

  # Self-check against repo CHANGELOG.MD
  extract-changelog-section.py --self-test

Version may be "0.0.8" or "v0.0.8". Heading form: "## [0.0.8] - 2026-07-22".
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

SECTION_RE = re.compile(
    r"(?ms)^(## \[([^\]]+)\][^\n]*\n.*?)(?=^## \[|\Z)"
)


def normalize_version(raw: str) -> str:
    v = raw.strip()
    if v.startswith(("v", "V")):
        v = v[1:]
    if not v:
        raise SystemExit("version is empty")
    return v


def parse_semver_tuple(version: str) -> tuple[int, ...]:
    """Best-effort numeric tuple for ordering (0.0.8 → (0,0,8))."""
    core = version.split("+", 1)[0].split("-", 1)[0]
    parts: list[int] = []
    for p in core.split("."):
        if p.isdigit():
            parts.append(int(p))
        else:
            m = re.match(r"(\d+)", p)
            parts.append(int(m.group(1)) if m else 0)
    return tuple(parts) if parts else (0,)


def iter_sections(text: str) -> list[tuple[str, str]]:
    """Return [(version, full_section_md), ...] in file order (newest first)."""
    out: list[tuple[str, str]] = []
    for m in SECTION_RE.finditer(text):
        body = m.group(1).rstrip()
        while body.endswith("\n---") or body.endswith("\n***"):
            body = body[:-4].rstrip()
        out.append((m.group(2).strip(), body + "\n"))
    return out


def extract_section(text: str, version: str) -> str | None:
    for ver, body in iter_sections(text):
        if ver == version:
            return body
    return None


def extract_range(text: str, target: str, since: str) -> str | None:
    """Sections with since < version <= target, file order preserved."""
    t = parse_semver_tuple(target)
    s = parse_semver_tuple(since)
    chunks: list[str] = []
    for ver, body in iter_sections(text):
        v = parse_semver_tuple(ver)
        if s < v <= t:
            chunks.append(body.rstrip() + "\n")
    if not chunks:
        return None
    return "\n---\n\n".join(chunks) + "\n"


def fallback_notes(version: str, repo: str) -> str:
    return (
        f"## [{version}]\n\n"
        f"No matching section found in `CHANGELOG.MD` for this tag.\n\n"
        f"See the full changelog: "
        f"https://github.com/{repo}/blob/main/CHANGELOG.MD\n"
    )


def install_footer(repo: str, tag: str) -> str:
    return (
        "\n### Install\n\n"
        f"```bash\n"
        f"# macOS / Linux\n"
        f"curl -fsSL https://github.com/{repo}/releases/download/{tag}/install.sh | "
        f"GROK_PI_VERSION={tag} sh\n"
        f"```\n\n"
        f"```powershell\n"
        f"# Windows\n"
        f"$env:GROK_PI_VERSION='{tag}'; "
        f"irm https://github.com/{repo}/releases/download/{tag}/install.ps1 | iex\n"
        f"```\n\n"
        "Release assets: macOS aarch64/x86_64, Linux x86_64/aarch64, Windows x86_64/aarch64.\n"
    )


def self_test(changelog: Path) -> int:
    if not changelog.is_file():
        print(f"self-test: missing {changelog}", file=sys.stderr)
        return 2
    text = changelog.read_text(encoding="utf-8")
    sections = iter_sections(text)
    release_sections = [
        (version, body)
        for version, body in sections
        if version.casefold() != "unreleased"
    ]
    if len(release_sections) < 2:
        print("self-test: expected >=2 versioned sections", file=sys.stderr)
        return 1
    ver0 = release_sections[0][0]
    one = extract_section(text, ver0)
    assert one is not None and one.startswith(f"## [{ver0}]")
    # Range: from second-newest exclusive lower bound if possible.
    older = release_sections[1][0]
    ranged = extract_range(text, ver0, older)
    # since older exclusive → only ver0 when they are adjacent
    assert ranged is not None and f"## [{ver0}]" in ranged
    miss = extract_section(text, "9.9.9.9")
    assert miss is None
    print(f"self-test ok: {len(sections)} sections, head={ver0}", file=sys.stderr)
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument(
        "version",
        nargs="?",
        help="Release version (e.g. 0.0.8 or v0.0.8)",
    )
    p.add_argument(
        "changelog",
        nargs="?",
        default="CHANGELOG.MD",
        help="Path to changelog (default: CHANGELOG.MD)",
    )
    p.add_argument("-o", "--output", default="-", help="Output path (default: stdout)")
    p.add_argument(
        "--repo",
        default="Dwsy/grok-pi",
        help="GitHub repo for fallback/install links",
    )
    p.add_argument(
        "--strict",
        action="store_true",
        help="Exit non-zero when the requested section(s) are missing",
    )
    p.add_argument(
        "--since",
        metavar="VERSION",
        help="Also include every section with version > SINCE and <= target "
        "(for cumulative notes since a baseline, e.g. --since 0.0.6)",
    )
    p.add_argument(
        "--with-install",
        action="store_true",
        help="Append one-line install snippet for the tag",
    )
    p.add_argument(
        "--self-test",
        action="store_true",
        help="Run built-in checks against the changelog path",
    )
    args = p.parse_args()

    if args.self_test:
        return self_test(Path(args.changelog))

    if not args.version:
        p.error("version is required unless --self-test")

    version = normalize_version(args.version)
    path = Path(args.changelog)
    if not path.is_file():
        print(f"error: changelog not found: {path}", file=sys.stderr)
        return 2

    text = path.read_text(encoding="utf-8")
    if args.since:
        since = normalize_version(args.since)
        body = extract_range(text, version, since)
        label = f"{since} < v <= {version}"
    else:
        body = extract_section(text, version)
        label = version

    if body is None:
        print(f"warning: no CHANGELOG section for [{label}]", file=sys.stderr)
        if args.strict:
            return 1
        body = fallback_notes(version, args.repo)
    else:
        print(
            f"extracted CHANGELOG [{label}] ({len(body)} bytes)",
            file=sys.stderr,
        )

    if args.with_install:
        tag = f"v{version}"
        body = body.rstrip() + "\n" + install_footer(args.repo, tag)

    if args.output == "-":
        sys.stdout.write(body if body.endswith("\n") else body + "\n")
    else:
        out = Path(args.output)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(
            body if body.endswith("\n") else body + "\n", encoding="utf-8"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
