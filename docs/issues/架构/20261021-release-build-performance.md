# Release build speed and artifact footprint — 2026-10-21

## Scope and constraints

- User requested actual implementation and a beta release built **only on GitHub Actions**; never compile Rust on the developer's Mac for this work.
- Keep all six supported targets and production functionality. Do not mix in concurrent local Pager, adapter, or extension changes.
- CI-only change. Do not alter the default developer `[profile.dev]`, current shared target directory, or delete existing build caches.

## Baseline (GitHub Actions v0.1.11-beta.2)

Reference: https://github.com/Dwsy/grok-pi-tui/actions/runs/39418793282

| Platform | Cargo release build | Download archive |
| --- | ---: | ---: |
| Linux x64 | ~35.5 min | 122.7 MB |
| Linux ARM64 | ~25.9 min | 111.0 MB |
| macOS ARM64 | ~47.1 min | 27.2 MB |
| macOS Intel | ~42.6 min | 27.7 MB |
| Windows x64 | ~59.2 min | 25.2 MB |
| Windows ARM64 | ~40.9 min | 23.7 MB |

- Wall clock was about 64 minutes. Combined build step time exceeded 250 minutes.
- Linux and Windows used CGU=1, Thin LTO, DWARF=1. macOS already overrode these to CGU=16/LTO=off/debug=0 because previous macOS jobs hit the timeout.
- Release tag caches are ref-scoped. GitHub showed six caches consuming ~10.6 GB, including 1.4–2.1 GB uploads from each of four cached targets in beta.2. New release tags cannot consume previous tag caches.
- Developer shared target is currently ~23 GiB (incremental ~14 GiB, deps ~8.5 GiB); that is **not** proof that GitHub Actions produced 100 GiB per runner. CI now measures actual target size.

## Beta.3 implementation

1. Apply the already-proven macOS fast profile to all six platforms in the release workflow only: codegen-units=16, lto=off, debug=0 and incremental=0.
2. Request `strip=symbols` for Linux binaries; macOS and Windows keep `strip=none`. Keep the existing Rust release profile and security-sensitive target flags unchanged outside CI.
3. Remove Rust target-cache uploads from tag builds. Do not install extra cache tools or upload huge archives.
4. Enable `cargo build --timings`; capture duration, disk free space and target size; upload small, short-lived timing HTML reports.
5. Filter the publish job's artifact download to `grok-pi-*` so the new `build-timings-*` reports never ship in the public release.
6. Ship `v0.1.11-beta.3` using the existing tag-triggered GitHub Actions release pipeline, with **no local Rust compilation**.

## Review, risks and acceptance

- **Compatibility:** CGU/LTO changes alter LLVM optimization and potentially binary size/runtime speed; not intended as a source feature change. Beta packaging and startup must pass on each target before promoting to stable.
- **Diagnostics:** `debug=0` and Linux stripping remove production symbols; if offline symbolication becomes important, a future workflow should extract private debug sidecars before stripping.
- **Performance:** beta.3 measured Cargo step duration, job wall time, runner target footprint and archives must be compared to beta.2. Faster CI is a hypothesis until the build completes.
- **Cache:** a tag-push build intentionally uses no persistent target cache; same-tag manual retries can therefore be slower than when they previously hit a cache, but subsequent beta releases avoid uploading caches they cannot consume.
- **No local build:** only static manifest/workflow/changelog checks locally, with Rust compile/test running exclusively on GitHub.

## Execution evidence

- PASS: changelog extractor self-test (41 sections), strict beta.3 release notes extraction, YAML structure (6 build targets), all Bash step syntax checks, diagnostics/release artifact separation and scoped `git diff --check`. No Cargo command executed.
- Git commit and release run URL: to be recorded after push/tag.
- Six-platform build results, archive sizes and duration comparison: pending GitHub Actions execution.
