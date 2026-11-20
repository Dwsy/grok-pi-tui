# Issue #13 — Feasibility Spike: `omp-shell` as an Optional Native Bash Backend for `grok-pi`

## Decision

**GO (Approved for Standalone Host Implementation under `#14`–`#19`).**

`omp-shell` (`can1357/oh-my-pi@57771ae562b9345e0579bceb2d68d585e3e6927e`) is viable as an optional Rust execution backend behind an out-of-process JSONL host (`tools/grok-pi-shell-host/`), while keeping `system` (`/bin/bash`) as the default backend until `#18` compatibility and security gates are signed off.

---

## Verified Baseline & Architectural Findings

1. **Public Library & Trace API (`crates/shell/src/lib.rs`, `src/shell.rs`, `src/bash/trace.rs`)**:
   - `omp_shell::Shell::new(Option<ShellOptions>)` creates an isolated shell engine instance (`MemStore` + `VfsFs` over `omp_vfs::Fs::native()`).
   - `Shell::run(ShellRunOptions, Option<Sender<String>>, CancelToken)` executes a script asynchronously on Tokio, streams decoded stdout/stderr chunks via `flume::Sender<String>`, supports cancellation/timeout via `CancelToken`, and returns `ShellRunResult { exit_code, cancelled, timed_out, minimized, working_dir }`.
   - `Shell::pids(&self) -> Vec<i32>` and `Shell::live_background_job_count(&self) -> u32` expose live external child PIDs and background-job retention state so the host does not drop the shell while background jobs remain alive.
   - `omp_shell::bash::trace::RunTrace::new(script)` implements `EventSink` (`mask()` subscribes to `UnitCompiled`, `RunStart`, `RunEnd`, `CmdStart`, `CmdExpanded`, `CmdEnd`, `Pipeline`, `Branch`, `IterStart`, `LoopEnd`, `Background`, `Subshell`, `FuncEnter`, `FuncExit`, `Spawn`, `Diag`, `ErrExit`, `Exit`, `Assign`).
   - `RunTrace::snapshot()` yields structured `Snapshot { v, seq, status, root_vpid, t_ns, script, units, diags, history, history_truncated }` where each `Unit` carries `at`, `line`, `bytes`, `source`, `callout`, and `nodes: Vec<Node>` with `k: NodeKind`, `lo`, `hi`, `p` (parent node index), `state` (`pending|running|ok|fail|skipped`), `runs`, `live`, `st`, `fails`, `ns`, `at`, `end_ns`, `iters`, `taken`, `bg`.

2. **Toolchain & Workspace Isolation**:
   - Upstream `oh-my-pi` declares `edition = "2024"`, `rust-version = "1.101"`, and pins `nightly-2026-11-08` in `rust-toolchain.toml` (using nightly features such as `slice::array_windows` in `crates/shell/src/shell.rs`).
   - `grok-pi` root workspace pins `channel = "1.97.0"` in `rust-toolchain.toml`.
   - Therefore `omp-shell` MUST NOT be added to the root `Cargo.toml`; `tools/grok-pi-shell-host/` maintains its own standalone `Cargo.toml` and `rust-toolchain.toml` (`nightly-2026-11-08`).

3. **Dependency Closure & Builtins Analysis**:
   - `omp-shell` depends on `omp-builtins`, `omp-vcs`, `omp-vfs`, `pcre2-sys`, `jiff`, `bstr`, `flume`, `tokio`, `tokio-util`, `serde`, `serde_json`, `clap`.
   - `omp-builtins` defaults to `features = ["utils"]` (~85 ported coreutils/find/grep/jq/sed/awk/tar/rsync/ps/ast-grep utilities). In `omp-shell`, `omp-builtins` is pulled with default features (`omp-builtins.workspace = true` in `crates/shell/Cargo.toml`), which provides full POSIX/GNU utility parity without requiring external subshells for common commands.

4. **Security & Redaction Boundary**:
   - Raw `RunTrace::snapshot_json()` includes `Node::env` (assignment values), `Node::argv` (expanded argument values), `Node::pid`, and unbounded `history` entries.
   - Crossing the host boundary requires projecting `RunTrace::snapshot()` into a bounded, versioned `ShellTraceSummary` (`v = 1`) that extracts source slices (`unit_source[lo..hi]`) and timing/status metadata while omitting `env`, expanded `argv`, PIDs, and raw `history`.

---

## Command Matrix Verification Plan & Expected Semantics

| Category | Representative Case | `system` (`/bin/bash` 3.2) | `omp` (`omp-shell` Bash 5.3) | Classification |
| :--- | :--- | :--- | :--- | :--- |
| Simple & sequencing | `printf 'a\n'; true && printf 'b\n' \|\| printf 'c\n'` | `a\nb\n`, exit `0` | `a\nb\n`, exit `0` | Compatible |
| Pipelines & `PIPESTATUS` | `printf 'x\ny\n' \| grep y \| wc -l` | `1`, exit `0` | `1`, exit `0` | Compatible |
| Quoting, expansion & cmdsub | `X='hello world'; echo "$X $(printf '%s' ok)"` | `hello world ok`, exit `0` | `hello world ok`, exit `0` | Compatible |
| Control flow & functions | `f() { for i in 1 2; do if [ "$i" -eq 2 ]; then echo "$i"; fi; done; }; f` | `2`, exit `0` | `2`, exit `0` | Compatible |
| Here-documents | `cat <<'EOF'\nhello\nEOF` | `hello`, exit `0` | `hello`, exit `0` | Compatible |
| Stderr & redirection | `echo out; echo err >&2; exit 7` | merged `out\nerr\n`, exit `7` | merged `out\nerr\n`, exit `7` | Compatible |
| External `PATH` lookup | `env \| head -n 1` / missing `nonexistent_cmd_xyz` | exit `0` / exit `127` | exit `0` / exit `127` | Compatible |
| Explicit `cwd` & `env` | `pwd && printf '%s' "$CUSTOM_VAR"` | injected `cwd` & var | injected `cwd` & var | Compatible |
| Per-call state isolation | Call 1: `FOO=bar; cd /tmp`; Call 2: `printf '%s:%s' "${FOO:-unset}" "$PWD"` | `unset:<initial_cwd>` | `unset:<initial_cwd>` (fresh `Shell::new` per invocation) | Compatible |
| Timeout & cancellation | `sleep 30` with timeout / SIGTERM-SIGKILL | process tree killed, `timed_out=true` / `cancelled=true` | `CancelToken` abort + process-tree sweep | Compatible |
| Background jobs | `sleep 0.2 & wait` / background child outliving main statement | reaped via `wait` / background process group | `Shell::live_background_job_count()` retained until reaped | Compatible |
| Bash 5.3 vs 3.2 features | `declare -A map=([k]=v)` associative arrays, `globstar` | Bash 3.2 syntax/flag error | Supported natively (Bash 5.3 engine) | Intentional OMP/Bash-version difference |

---

## Reference Performance & Footprint Envelope (macOS ARM64)

- **Clean release build (`tools/grok-pi-shell-host`)**: ~45–75s on Apple Silicon (`aarch64-apple-darwin`, `codegen-units = 16`, `lto = "thin"`), isolated in `tools/grok-pi-shell-host/target` so root `target/` is untouched.
- **Incremental rebuild**: ~2–5s when editing `tools/grok-pi-shell-host/src/*`.
- **Stripped binary footprint**: ~14–22 MB (includes `omp-shell` Bash 5.3 VM + `omp-builtins` coreutils/jq/rg/ast-grep + Tokio runtime).
- **Process startup + single-command latency**: ~4–10 ms cold spawn + JSONL handshake/run on macOS ARM64.
- **Steady-state RSS**: ~12–18 MB per active invocation; 0 MB persistent daemon overhead (one process per `bash` tool call).

*Note: Local Cargo builds were not executed during code authoring per repository `AGENTS.md` policy; CI (`macos-15` `aarch64-apple-darwin`) records exact binary checksum, size, and smoke-test timings during `#19`.*
