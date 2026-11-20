# Issue #18 — OMP Shell Compatibility Matrix, Security Boundary Review & Test Gates

**Parent Issue**: [#20](https://github.com/Dwsy/grok-pi-tui/issues/20)  
**Pinned OMP Revision**: `can1357/oh-my-pi@57771ae562b9345e0579bceb2d68d585e3e6927e`  
**Default Backend**: `system` (`/bin/bash`) — opt-in via `PI_GROK_BASH_BACKEND=omp`

---

## 1. Compatibility Matrix (`system` `/bin/bash` 3.2 vs `omp` `omp-shell` Bash 5.3)

| Category | Test Case | `system` (`/bin/bash` 3.2) | `omp` (`omp-shell` Bash 5.3) | Status |
| :--- | :--- | :--- | :--- | :--- |
| Simple & sequencing | `printf 'a\n'; true && printf 'b\n' \|\| printf 'c\n'` | `a\nb\n`, exit `0` | `a\nb\n`, exit `0` | Parity |
| Pipelines & `PIPESTATUS` | `printf 'x\ny\n' \| grep y \| wc -l` | `1`, exit `0` | `1`, exit `0` | Parity |
| Quoting & command substitution | `X='hello world'; echo "$X $(printf '%s' ok)"` | `hello world ok`, exit `0` | `hello world ok`, exit `0` | Parity |
| Control flow & functions | `f() { for i in 1 2; do if [ "$i" -eq 2 ]; then echo "$i"; fi; done; }; f` | `2`, exit `0` | `2`, exit `0` | Parity |
| Here-documents | `cat <<'EOF'\nhello\nEOF` | `hello`, exit `0` | `hello`, exit `0` | Parity |
| Stderr & redirections | `echo out; echo err >&2; exit 7` | merged `out\nerr\n`, exit `7` | merged `out\nerr\n`, exit `7` | Parity |
| External `PATH` lookup & missing cmd | `env \| head -n 1` / `nonexistent_cmd_xyz` | exit `0` / exit `127` | exit `0` / exit `127` | Parity |
| Explicit `cwd` & `env` injection | `pwd && printf '%s' "$CUSTOM_VAR"` | `cwd` + var | `cwd` + var | Parity |
| Per-call state isolation | Call 1: `FOO=bar; cd /tmp`; Call 2: `printf '%s:%s' "${FOO:-unset}" "$PWD"` | `unset:<cwd>` | `unset:<cwd>` (fresh `Shell::new` per request) | Parity |
| Timeout & cancellation | `sleep 30` + `timeout_ms` / `SIGTERM` | process group killed | `CancelToken` abort + `terminate_live_pids` (`SIGTERM` -> `SIGKILL`) | Parity |
| Background jobs | `sleep 0.2 & wait` / background child outliving main command | retained until reaped | `Shell::live_background_job_count()` retained until reaped | Parity |
| Bash 5.3 features | `declare -A map=([k]=v)`, `shopt -s globstar` | Bash 3.2 syntax/option error | Supported natively | Intentional Bash 5.3 upgrade |

---

## 2. Security Boundary & Redaction Audit

### 2.1 Process & Toolchain Isolation
- `grok-pi-shell-host` lives in an independent Cargo workspace (`tools/grok-pi-shell-host/Cargo.toml`) pinned to `nightly-2026-11-08` (`tools/grok-pi-shell-host/rust-toolchain.toml`).
- The root `grok-pi` workspace (`Rust 1.97.0`) never links `omp-shell` or `omp-builtins`; communication crosses a strict stdin/stdout JSONL process boundary (`protocol_version = 1`).

### 2.2 Trace Redaction Invariants (`tools/grok-pi-shell-host/src/trace.rs`)
The `project_trace()` projection enforces:
- **Never reads `Node::env`**: environment assignment values (e.g. `API_KEY=...`) are excluded.
- **Never reads `Node::argv`**: expanded argument values are excluded.
- **Never reads `Node::pid`**: host process IDs are excluded.
- **Never reads `Snapshot::history`**: unbounded output journals are excluded.
- Only static AST source slices (`unit_source[lo..hi]`), node kind, state (`pending|running|ok|fail|skipped`), exit code, duration (`duration_ms`), depth, and background flag are emitted.

### 2.3 Resource & Payload Bounds (`tools/grok-pi-shell-host/src/protocol.rs`)
- `MAX_REQUEST_BYTES`: `2 MiB`
- `MAX_OUTPUT_EVENT_BYTES`: `256 KiB` per JSONL `output` event (split on valid UTF-8 char boundaries)
- `MAX_TRACE_JSON_BYTES`: `32 KiB` hard cap on serialized `ShellTraceSummary`
- `MAX_TRACE_NODES`: `96` nodes max (`truncated = true` when exceeded)
- `MAX_NODE_DISPLAY_CHARS`: `160` characters per node command label

### 2.4 Prompt Context Isolation (`#16`)
- `shell_trace` flows through `HostEvent::Completed.trace` -> `pi-grok-bash` `details.shellTrace` -> `pi-grok-adapter` `bash_tool_output()` -> `BashOutput.shell_trace` -> `xai-grok-pager` `ExecuteToolCallBlock.shell_trace`.
- `BashOutput.output_for_prompt` and `result.content` never include `shell_trace`.

---

## 3. Automated Test Gates

1. **Extension / Protocol Gate (Node.js, no Cargo required)**:
   - `node --test extensions/pi-grok-bash/test-shell-backend.mjs`
2. **Host Crate Gate (macOS ARM64, requires user-authorized Cargo)**:
   - `cd tools/grok-pi-shell-host && cargo test`
3. **Adapter & Pager Gate (requires user-authorized Cargo)**:
   - `./scripts/cargo-shared.sh test -p pi-grok-adapter --lib tool_projection::tests`
   - `./scripts/cargo-shared.sh test -p xai-grok-pager --lib scrollback::blocks::tool::execute::tests`
   - `./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi bash_extension::tests`
