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

- In the first ARM64 timing reports, other large compilation units were `pdf_oxide` (259.8–275.0s), `aws-sdk-s3` (181.3–197.1s), `xai-grok-workspace` (125.9–136.3s), and `rmcp` (99.6–115.2s). These package timings overlap with each other and with the overall build; do not sum them as wall-clock time. `xai-file-utils` already declares `aws-sdk-s3` with `default-features = false` and only `rt-tokio`, so broad default-feature pruning is not an obvious next step there.
- `pdf_oxide` is used by `xai-grok-tools` for PDF text extraction **and** page rendering, so it cannot be removed without losing user-facing functionality. The installed `0.3.74` source is about **21 MB / 432,000 Rust lines** across 370 files. The workspace dependency enables `rendering` but currently retains upstream defaults; the resolved feature set also includes ICC color management and legacy crypto. Do not disable those without checking encrypted-PDF compatibility and rendered color fidelity.
- The pinned registry manifest for `pdf_oxide 0.3.74` declares `crate-type = ["cdylib", "rlib", "staticlib"]`. This is a candidate for investigating unnecessary FFI artifacts, but the current evidence does not prove that Cargo emits every crate type when the package is used only as a dependency. Confirm the actual rustc command/output files on a GitHub runner before considering a fork or patch.
- The first ARM64 CI log measured `target/` at **3.7 GB** and the final `grok-pi` executable at **71–73 MB**. Keep intermediate Cargo targets, build caches, and release artifacts as separate size metrics.

## Source-level extraction constraints

- A filename-based source scan counted about **425,600 lines in 515 Pager Rust files** after excluding files whose paths/names identify them as tests. This is an approximate source-size signal, not exact compiled LOC: inline `#[cfg(test)]` blocks remain.
- The Pager's `app` and `views` are strongly intertwined: static scan found 2,359 `crate::app::` references across 257 files, 2,230 `crate::views::` references across 165 files, and 772 `crate::scrollback::` references across 140 files. Splitting `app` or `views` wholesale into new crates would create a large API/cycle risk. Extract leaf crates only after a dependency graph proves a stable boundary.
- Shell's filename-based non-test source scan counted about **133,800 lines in `session/`**, **62,000 in `agent/`**, and **26,100 in `extensions/`**. Its largest individual non-test-name files include `agent/config.rs` (5,212 lines), `agent/mvp_agent/agent_ops.rs` (4,981), `session/acp_session_impl/turn.rs` (4,299), and `session/acp_session_impl/tool_calls.rs` (3,714). Static counts found 1,974 `async fn` declarations and 175 same-line generic function signatures. `session/` and `agent/` are possible future boundaries, but current evidence does not prove they can be separated without broad shared types and cycles.
- Pager's largest individual non-test-name files include `views/extensions_modal.rs` (7,971 lines), `app/app_view.rs` (7,183), `app/event_loop.rs` (7,156), `app/modals.rs` (6,432), `app/effects/mod.rs` (6,394), and `app/agent_view/render.rs` (5,884). Static counts found 196 same-line generic function signatures and 615 explicit `#[cfg(test)]` markers across 505 non-test-name source files. Large files are useful review targets but splitting a file inside the same crate will **not by itself** reduce whole-crate rustc front-end time; a compile-time win needs a stable crate boundary or less compiler work.
- If the matched CGU experiments confirm that Pager frontend remains the wall-time bottleneck, use the isolated GitHub-hosted nightly `-Zself-profile` workflow against only the Pager library to identify expensive compiler queries before choosing any code refactor. Do not introduce nightly into stable release builds and do not do local Rust compilation.

## Hypotheses and constrained experiments

| Hypothesis | Predicted change | Controlled test | Risk |
| --- | --- | --- | --- |
| Shell LLVM optimization/monomorphization is dominant | More CGUs may distribute codegen work | `cgu32` (profile CGU 32, otherwise stable CI settings) | Runtime efficiency, binary size, memory |
| Shell `-O3` LLVM cost is high | Shell-only `opt-level=2` may reduce codegen significantly | `shell-o2` (CGU 16, per-package override for shell only) | Runtime speed of shell hot loops |
| Pager frontend dominates, not LLVM | CGU changes may bring little improvement to Pager itself | Compare Pager frontend/codegen numbers in both variants | Changes can backfire from less optimization |
| Pager crate contains too many tightly coupled modules | Extract a low-coupling leaf boundary only after dependency analysis | Follow-up architectural review of `app`, `views`, `scrollback` and their APIs | High source regression and upstream-sync costs |

Cargo supports `[profile.<name>.package.<crate>]` overrides in the **workspace-root `Cargo.toml`**, not in `.cargo/config.toml` and not through per-package environment variables. The corrected experiment appends the Shell-only `opt-level=2` override to `Cargo.toml` only on the disposable GitHub runner, asserts the parsed TOML, and checks the actual verbose `rustc` command lines for both Shell and Pager before accepting results. No feature gates, `panic` settings, linker hardening, `jemalloc`, or sandbox flags are changed.

## CI test contract

- Workflow: `.github/workflows/macos-compile-lab.yml`. Manual only; six isolated jobs (three variants x two target architectures) using GitHub-hosted `macos-14` with `max-parallel=2`. Every job checks out the same immutable stable release tag `v0.1.11`.
- Each job logs hardware, pinned SHA, the temporary workspace `Cargo.toml` delta (Shell O2 only), verbose `rustc` flags, `cargo build --timings`, free disk before and after, target size, binary architecture and size. Native ARM64 also tests `grok-pi --version` and `--help`.
- Each job produces a `compile-metrics.json` with source SHA, actual build wall time, binary byte size/SHA-256, and Shell/Pager total/frontend/codegen durations, plus a seven-day Cargo timing HTML artifact. **No GitHub Release**, installers, tags, or release workflow modifications.
- Compare against a same-source stable build or repeat a baseline job if host variance or source mismatch prevents an apples-to-apples comparison.
- Do not merge any changed production build profile without measured speed/size gains and representative functional/runtime smoke tests.

## Findings and action queue

1. **P0** Isolate opt-level/codegen effects in macOS runners. A faster Shell **must not** be misreported as a faster Pager frontend without matching data.
2. **P1** The isolated `.github/workflows/pager-self-profile.yml` was briefly dispatched as run **https://github.com/Dwsy/grok-pi-tui/actions/runs/39573310237** against `v0.1.11`, then cancelled after 4m03s while dependencies were still compiling. No Pager self-profile data was generated. Rerun the Pager compiler-query profile after the six-cell build comparison completes; nightly remains confined to this manual diagnostic workflow, never stable releases.
3. **P2** If the CI experiment is positive, add opt-level override to a macOS-only build configuration and validate both startup and normal Pi session workflow before next beta. If runtime regression is unknown, leave the established release profile unchanged.
4. **P3** Investigate Shell's `session` and `agent` and Pager's `app`/ `views` API boundaries. Existing extracted crates `xai-grok-pager-render`, `xai-grok-pager-diff`, etc. show the intended architecture; avoid creating reverse dependencies or coupling native UI to Pi's RPC engine.

## Checkpoint

- Static project scan: completed.
- Reference timing analysis: completed.
- Initial isolated experiment workflow `c482d052`, run **https://github.com/Dwsy/grok-pi-tui/actions/runs/39563709999**, was cancelled after its ARM64 jobs completed. ARM64 CGU32 reported 1445.6s Cargo wall, Shell 552.7s and Pager 524.1s; the mislabeled `shell-o2` run reported 1494.8s, Shell 704.1s and Pager 628.1s. These were not valid causal comparisons: they had no same-run baseline, and the first `shell-o2` attempt put the profile table in `.cargo/config.toml` rather than the workspace root manifest.
- Corrected workflow commits `c17303fe` + `38fa0726`: six-cell matrix (CGU16 baseline, CGU32, Shell-only O2 for both target architectures), actual `rustc` flag assertions, source SHA, binary size/SHA-256 and per-crate timing capture. Run **https://github.com/Dwsy/grok-pi-tui/actions/runs/39570559419** is in progress. ARM64 CGU32 passed in 1,254s wall / 1,248.6s Cargo timing, with a 76,554,840-byte unstripped executable; Shell was 446.1s (76.0s frontend / 370.1s codegen) and Pager 422.2s (300.0s frontend / 122.2s codegen). The Cargo timing graph shows `pdf_oxide` at 332–585s, `xai-grok-tools` at 585–653s, Shell at 734–1180s, Pager at 813–1235s, and the final `grok-pi` bin unit at 1235–1249s. Shell codegen overlaps Pager compilation; Pager is the final major library on the observed critical path, so the self-profile of its 300s frontend is likely the next high-value diagnostic. The ARM64 CGU16 baseline and Shell-O2 jobs are still compiling; Intel jobs are queued. This is a promising datapoint, **not yet a valid speedup claim** until the same-run baseline completes. Do not change production build settings until all comparable results are available.
- Pager profiler workflow committed and pushed as `80a356e0`; static validation passed (manual-only, read-only permissions, Bash syntax, no release publishing). Run **https://github.com/Dwsy/grok-pi-tui/actions/runs/39573310237** was cancelled after 4m03s while dependency crates (`async-openai`, `reqwest`, `axum`, etc.) were compiling; the uploaded diagnostic log contains no Pager self-profile data. Rerun after the six-cell comparison to avoid overlapping the controlled build measurements. The workflow profiles only the Pager library with nightly `-Zself-profile`, summarizes compiler queries with `measureme`, and retains raw profile data for three days. Stable releases remain pinned to stable Rust and unchanged.
- Timing parser validation passed against the real beta.3 Cargo timing HTML and reproduced Shell (970.5s / 120.9s frontend / 849.6s codegen) and Pager (886.5s / 670.5s frontend / 216.0s codegen) metrics. No local Rust build/check/test was run.
- Source-level refactor: deliberately not undertaken without per-crate experimental measurements.
- User requirement: **no local Cargo build/check/test**; only GitHub-hosted release/experiment compilation.
