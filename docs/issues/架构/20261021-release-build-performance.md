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
- **Performance:** beta.3 has completed successfully on all six GitHub runners. Its runtime, archives and final Cargo target footprints are recorded below. Peak in-flight disk use was not recorded, so do not equate post-build target size with peak usage.
- **Cache:** a tag-push build intentionally uses no persistent target cache; same-tag manual retries can therefore be slower than when they previously hit a cache, but subsequent beta releases avoid uploading caches they cannot consume.
- **No local build:** only static manifest/workflow/changelog checks locally, with Rust compile/test running exclusively on GitHub.

## Execution evidence

- PASS: changelog extractor self-test (41 sections), strict beta.3 release notes extraction, YAML structure (6 build targets), all Bash step syntax checks, diagnostics/release artifact separation and scoped `git diff --check`. No Cargo command executed.
- Commit: `5984680da33a4b4111c2dc73400524f7c89a2c2d`. Tag: `v0.1.11-beta.3`.
- GitHub Actions: https://github.com/Dwsy/grok-pi-tui/actions/runs/39536768591 — all six build jobs, changelog gate, artifact packaging, and Release publishing succeeded on 2026-10-21.
- GitHub Release: https://github.com/Dwsy/grok-pi-tui/releases/tag/v0.1.11-beta.3 — six binary archives and two installers published; build-timings reports remain Actions artifacts only.

### Actual beta.3 vs beta.2 improvements

All times below are measured GitHub Actions `Build release binary` steps. Archive sizes are from the published Release assets (decimal MB).

| Platform | Beta.2 build | Beta.3 build | Time reduction | Beta.2 asset | Beta.3 asset |
| --- | ---: | ---: | ---: | ---: | ---: |
| Linux x64 | 35.5m | 23.4m | 34.1% | 122.7 MB | 27.1 MB |
| Linux ARM64 | 25.9m | 16.8m | 35.2% | 111.0 MB | 26.0 MB |
| macOS ARM64 | 47.1m | 36.1m | 23.3% | 27.2 MB | 27.2 MB |
| macOS Intel | 42.6m | 37.1m | 12.8% | 27.7 MB | 27.7 MB |
| Windows x64 | 59.2m | 40.1m | 32.3% | 25.2 MB | 24.1 MB |
| Windows ARM64 | 40.9m | 27.0m | 33.9% | 23.7 MB | 23.2 MB |

- Release wall time dropped from **64m12s to 41m41s** (about 35% less).
- Summed Cargo build steps dropped from about **251.3 to 180.6 minutes**; these jobs are run concurrently.
- Six published archives total **337.4 MB → 155.1 MB** (about 54% smaller); Linux x64 alone shrank about 78%.
- The actual post-build Cargo target sizes were **Linux x64 4.2 GiB, Linux ARM64 4.1 GiB, macOS ARM64 3.7 GiB, macOS Intel 3.7 GiB, Windows x64 4.0 GiB, Windows ARM64 3.9 GiB**. These are not peak usage measurements, but none of the measured final CI target trees approached 100 GiB.
- All six timing HTML artifacts uploaded successfully; each compressed Actions artifact is about 0.29–0.33 MB and is retained seven days.

### GitHub cache housekeeping

- Removed the **six old tag-scoped release target caches**, specifically cache IDs `9217898940`, `9217203740`, `9217010754`, `9216973585`, `8923075337`, `8922614480`, formerly totaling about **10.6 GB**.
- Verified `GET /repos/Dwsy/grok-pi-tui/actions/caches` reports **zero remaining active entries**. The aggregate `actions/cache/usage` endpoint still displayed an older count/size immediately afterward (eventual consistency); use the cache entry listing for immediate deletion confirmation and check aggregate usage again later.
- **No** local `target`, Cargo registry, toolchains, or source files were cleaned.

### Cargo timing bottlenecks (beta.3)

Per-unit durations may overlap in parallel; do not sum them to derive wall time.

| Platform | xai-grok-shell | xai-grok-pager | pdf_oxide | Other notable work |
| --- | ---: | ---: | ---: | --- |
| macOS Intel | 1102s | 1007s | 385s | aws-sdk-s3 237s |
| macOS ARM64 | 971s | 887s | 348s | aws-sdk-s3 256s |
| Windows x64 | 631s | 602s | 528s | libgit2-sys build 395s |
| Linux x64 | 353s | 293s | 363s | aws-lc-sys build 207s |

Next potential experiments, **not included in beta.3**:

1. Investigate compile-critical-path architecture in `xai-grok-shell` and `xai-grok-pager`; only split stable leaf functionality after measuring dependencies and incremental rebuild improvement.
2. Compare macOS CGU 16 vs 32 in isolated GitHub Actions experiments before accepting possible runtime-performance tradeoffs.
3. If beta release frequency warrants it, provide a shared default-branch build context or external sccache (with appropriate storage credentials and no tag-scope cache assumptions).
4. Capture a bounded CI peak disk metric (not just post-build `du`) and a basic startup/feature smoke test before stable promotion. No local Rust build was run.
