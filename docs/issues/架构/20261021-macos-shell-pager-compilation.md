# macOS compile hotspots: xai-grok-shell and xai-grok-pager (2026-10-21)

## Goal and scope

Reduce GitHub-hosted macOS release compile wall time without changing product behavior or compiling Rust on the developer Mac. Keep `v0.1.11` stable release immutable. Use **isolated** `workflow_dispatch` experiments based on the same tagged source. Do not publish any artifacts as a Release. No production Rust code refactor until measured evidence justifies it.

## Source evidence — v0.1.11-beta.3 (successful GitHub Action)

- Run: https://github.com/Dwsy/grok-pi-tui/actions/runs/39536768591
- Rust pinned at 1.94.0, release-dist profile overridden in CI to `CGU=16`, `LTO=off`, `debug=0`, `incremental=0`.
- macOS ARM64 Cargo wall 2162s (~36.0m); Intel cross target Cargo wall 2222s (~37.0m).
- macOS ARM64: `xai-grok-shell` **970.5s** = 120.9s frontend + **849.6s codegen (88%)**; `xai-grok-pager` **886.5s** = **670.5s frontend (76%)** + 216.0s codegen.
- macOS Intel: `xai-grok-shell` **1102.4s** = 169.2s frontend + **933.2s codegen (85%)**; `xai-grok-pager` **1006.8s** = **840.5s frontend (83%)** + 166.3s codegen.
- The unit durations are not additive. Cargo compiles multiple units concurrently, and target crates share dependencies. All numbers are direct reads from the uploaded Cargo timing HTML.
- Existing code organization (including tests): Shell `src/` ~16 MB and 594 `.rs` files, with ~9 MB in `session/` and ~3.6 MB in `agent/`. Pager `src/` ~22 MB and 634 `.rs` files, with ~10.4 MB in `app/`, ~6.4 MB in `views/`, and ~2.3 MB in `scrollback/`. These file sizes include test code and are **not** a measurement of compiled release LOC.

## Other observed release-build costs

- In the first ARM64 timing reports, other large compilation units were `pdf_oxide` (259.8–275.0s), `aws-sdk-s3` (181.3–197.1s), `xai-grok-workspace` (125.9–136.3s), and `rmcp` (99.6–115.2s). These package timings overlap with each other and with the overall build; do not sum them as wall-clock time.
- `pdf_oxide` is used by `xai-grok-tools` for PDF text extraction **and** page rendering, so it cannot be removed without losing user-facing functionality. The workspace dependency enables `rendering` but currently retains upstream defaults; the resolved feature set also includes ICC color management and legacy crypto. Do not disable those without checking encrypted-PDF compatibility and rendered color fidelity.
- The pinned registry manifest for `pdf_oxide 0.3.74` declares `crate-type = ["cdylib", "rlib", "staticlib"]`. This is a candidate for investigating unnecessary FFI artifacts, but the current evidence does not prove that Cargo emits every crate type when the package is used only as a dependency. Confirm the actual rustc command/output files on a GitHub runner before considering a fork or patch.
- The first ARM64 CI log measured `target/` at **3.7 GB** and the final `grok-pi` executable at **71–73 MB**. Keep intermediate Cargo targets, build caches, and release artifacts as separate size metrics.

## Hypotheses and constrained experiments

| Hypothesis | Predicted change | Controlled test | Risk |
| --- | --- | --- | --- |
| Shell LLVM optimization/monomorphization is dominant | More CGUs may distribute codegen work | `cgu32` (profile CGU 32, otherwise stable CI settings) | Runtime efficiency, binary size, memory |
| Shell `-O3` LLVM cost is high | Shell-only `opt-level=2` may reduce codegen significantly | `shell-o2` (CGU 16, per-package override for shell only) | Runtime speed of shell hot loops |
| Pager frontend dominates, not LLVM | CGU changes may bring little improvement to Pager itself | Compare Pager frontend/codegen numbers in both variants | Changes can backfire from less optimization |
| Pager crate contains too many tightly coupled modules | Extract a low-coupling leaf boundary only after dependency analysis | Follow-up architectural review of `app`, `views`, `scrollback` and their APIs | High source regression and upstream-sync costs |

The Cargo reference supports `[profile.<name>.package.<crate>]` settings for `opt-level`; it does **not** support per-package profile override via environment variables. For the experiment only, the checked-out tag's `.cargo/config.toml` is amended **on the GitHub runner**, not committed or modified in the developer working tree. No feature gates, `panic` settings, linker hardening, `jemalloc`, or sandbox flags are changed.

## CI test contract

- Workflow: `.github/workflows/macos-compile-lab.yml`. Manual only; four isolated jobs (two variants x macOS ARM64/Intel target) using GitHub-hosted `macos-14` with `max-parallel=2`. The source is pinned to existing stable release tag `v0.1.11`.
- Each job logs hardware, pinned SHA, the temporary config delta, `cargo build --timings`, free disk before and after, target size, binary architecture and size. Native ARM64 also tests `grok-pi --version` and `--help`.
- Each job produces a `compile-metrics.json` with Shell/Pager total/frontend/codegen durations and seven-day Cargo timing HTML artifact. **No GitHub Release**, installers, tags, or release workflow modifications.
- Compare against a same-source stable build or repeat a baseline job if host variance or source mismatch prevents an apples-to-apples comparison.
- Do not merge any changed production build profile without measured speed/size gains and representative functional/runtime smoke tests.

## Findings and action queue

1. **P0** Isolate opt-level/codegen effects in macOS runners. A faster Shell **must not** be misreported as a faster Pager frontend without matching data.
2. **P1** Profile Pager frontend specifically (macro expansion/type-checking/borrow-checking) before source extraction. Consider `-Zself-profile`/measureme on a GitHub nightly runner only in a separate non-release job; do not add nightly to the stable CI path.
3. **P2** If the CI experiment is positive, add opt-level override to a macOS-only build configuration and validate both startup and normal Pi session workflow before next beta. If runtime regression is unknown, leave the established release profile unchanged.
4. **P3** Investigate Shell's `session` and `agent` and Pager's `app`/ `views` API boundaries. Existing extracted crates `xai-grok-pager-render`, `xai-grok-pager-diff`, etc. show the intended architecture; avoid creating reverse dependencies or coupling native UI to Pi's RPC engine.

## Checkpoint

- Static project scan: completed.
- Reference timing analysis: completed.
- Initial isolated experiment workflow committed as `c482d052`, run **https://github.com/Dwsy/grok-pi-tui/actions/runs/39563709999**. ARM64 CGU32 reported 1445.6s Cargo wall, Shell 552.7s and Pager 524.1s; the `shell-o2`-labelled variant reported 1494.8s, Shell 704.1s and Pager 628.1s. Both are substantially faster than beta.3's 2162.3s baseline, but host-to-host variance is not controlled, so these numbers are **not yet a valid causal comparison**. Initial `shell-o2` was appended to `.cargo/config.toml`, where Cargo profile keys are not supported; treat it as a likely CGU16 control, not as a verified opt-level experiment. Intel jobs from that run were still running at the latest check.
- Follow-up fix committed and pushed as `c17303fe` + `38fa0726`: add same-run `cgu16-baseline` per target; place the Shell-only `opt-level=2` override in the disposable runner's **workspace root Cargo.toml**; assert actual `rustc` invocation flags; record source SHA, binary byte size/SHA-256, wall time and crate frontend/codegen timings. The unpaired first run was asked to cancel to avoid wasting Intel runner time; corrected six-cell run **https://github.com/Dwsy/grok-pi-tui/actions/runs/39570559419** was queued as of 2026-10-21. Await its measured outputs before changing production build settings.
- Local static validation (initial version) passed: manual-only workflow, four unique target+variant jobs, no publishing action, Bash syntax, and parsing real beta.3 Cargo timing HTML. No local Rust compilation.
- Source-level refactor: deliberately not undertaken without per-crate experimental measurements.
- User requirement: **no local Cargo build/check/test**; only GitHub-hosted release/experiment compilation.
