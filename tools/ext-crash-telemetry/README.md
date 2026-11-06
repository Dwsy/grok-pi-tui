# ext-crash-telemetry

Bun + TS CLI for **VSCode-style extension crash bisection**, optional report to
**Cloudflare Workers + D1**, and a static **dashboard**.

Privacy: only `ext_name` + `package_dir` (+ `kind`). No absolute paths.

## Layout

```
src/          CLI + bisect + identity
test/         bun tests
worker/       CF Worker + D1 schema
dashboard/    static stats UI
```

## Live deploy

| Item | Value |
|------|--------|
| Worker | https://ext-crash-telemetry.dwsycode.workers.dev |
| Dashboard | https://ext-crash-telemetry.dwsycode.workers.dev/ (static asset served by the same Worker) |
| Health | https://ext-crash-telemetry.dwsycode.workers.dev/health |
| Stats | https://ext-crash-telemetry.dwsycode.workers.dev/v1/stats |
| Local dashboard | open `dashboard/index.html` directly, then paste the Worker URL |
| D1 | `ext_crash` (`d85c550a-1808-4a57-b6f9-ef90352f6b74`) |

## Abuse protection

| Control | Behavior |
|---------|----------|
| **REPORT_TOKEN** | Required for `POST /v1/report` (fail closed if unset) |
| **ADMIN_TOKEN** | Required for `POST /v1/triage` (defaults to REPORT_TOKEN) |
| Rate limit | ~30 writes / IP / hour (Cache API, best-effort) |
| Body | Max 2KB; whitelist keys only |
| Names | No paths, `..`, schemes; package-like labels only |
| Auth compare | Timing-safe Bearer match |

Token locations (never commit):

- `tools/ext-crash-telemetry/.env` → `REPORT_TOKEN=...`
- `~/.grok-pi/ext-telemetry.token` (one line)
- env `REPORT_TOKEN`

Rotate: `openssl rand -hex 32 | wrangler secret put REPORT_TOKEN`

## CLI

```bash
cd tools/ext-crash-telemetry
bun install
bun test

# Dry-run bisection (mock probe)
# After success: Y / any key = report, N = skip (TTY only)
bun run src/cli.ts probe \
  --paths \
    /x/node_modules/@heyhuynhgiabuu/pi-pretty/index.ts \
    /x/node_modules/ok-ext/index.ts \
  --mock-bad /x/node_modules/@heyhuynhgiabuu/pi-pretty/index.ts
# --no-report  skip prompt   |  --report  force report without prompt

# Report
export REPORT_URL=https://ext-crash-telemetry.dwsycode.workers.dev
# REPORT_TOKEN required (env / .env / ~/.grok-pi/ext-telemetry.token)
bun run src/cli.ts report --json '{"ext_name":"pi-pretty","package_dir":"@heyhuynhgiabuu/pi-pretty","kind":"crash"}'
bun run src/cli.ts top
```

## Worker (CF free)

```bash
cd worker
# wrangler login
wrangler d1 create ext_crash          # paste database_id into wrangler.toml
wrangler d1 execute ext_crash --file=schema.sql
# wrangler secret put REPORT_TOKEN
# wrangler secret put ADMIN_TOKEN
wrangler deploy
```

Endpoints:

| Method | Path | Body |
|--------|------|------|
| POST | `/v1/report` | `{ ext_name, package_dir, kind }` |
| GET | `/v1/stats` | Top + by_day + triage |
| POST | `/v1/triage` | `{ package_dir, status, note }` |

## Dashboard

Open `dashboard/index.html` in a browser (or host on Pages). Paste Worker URL, Refresh.

## Relation to grok-pi

In-process self-heal already lives in `grok-pi` (`bisect_extension_culprit`).
This tool is the **standalone telemetry + stats** track (Issue S1–S3). Optional
S4 hook (env-gated report from self-heal) is out of scope until needed.

## Issue

`docs/issues/tools/20260722-扩展崩溃二分上报与CF仪表盘.md`
