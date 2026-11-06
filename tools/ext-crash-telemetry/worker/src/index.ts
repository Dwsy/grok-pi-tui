/**
 * Cloudflare Worker: extension crash report + stats (D1).
 *
 * Abuse controls:
 * - Writes require REPORT_TOKEN (Bearer). Missing secret → 503 (fail closed).
 * - Per-IP rate limit (Cache API, best-effort).
 * - Max body size, whitelist keys, strict name patterns.
 * - No absolute paths / .. / control chars.
 */

export interface Env {
  DB: D1Database;
  /** Required for POST /v1/report. Fail closed if unset. */
  REPORT_TOKEN?: string;
  /** Required for POST /v1/triage (falls back to REPORT_TOKEN). */
  ADMIN_TOKEN?: string;
  /** Optional override: max reports per IP per window (default 30). */
  RATE_LIMIT_MAX?: string;
  /** Optional override: window seconds (default 3600). */
  RATE_LIMIT_WINDOW_S?: string;
}

const MAX = 128;
const MAX_BODY_BYTES = 2048;
/** package_dir / ext_name: scoped npm-ish labels only */
const NAME_RE = /^@?[A-Za-z0-9][A-Za-z0-9._-]*(\/[A-Za-z0-9][A-Za-z0-9._-]*)?$/;
const ALLOWED_KEYS = new Set([
  "ext_name",
  "package_dir",
  "kind",
  "client",
  "grok_pi_ver",
]);

type Kind = "crash" | "combo" | "unknown";

function clip(s: string): string {
  const t = s.trim();
  return t.length > MAX ? t.slice(0, MAX) : t;
}

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "access-control-allow-origin": "*",
      "access-control-allow-headers": "content-type, authorization",
      "access-control-allow-methods": "GET, POST, OPTIONS",
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
    },
  });
}

function unauthorized(): Response {
  return json({ error: "unauthorized" }, 401);
}

function tooMany(): Response {
  return json({ error: "rate_limited" }, 429);
}

function misconfigured(): Response {
  return json(
    { error: "misconfigured", hint: "REPORT_TOKEN secret not set" },
    503,
  );
}

function bearerOk(req: Request, secret: string | undefined): boolean {
  if (!secret) return false;
  const h = req.headers.get("authorization") ?? "";
  if (!h.startsWith("Bearer ")) return false;
  const got = h.slice("Bearer ".length);
  return timingSafeEqual(got, secret);
}

/** Constant-time string compare (UTF-8). */
function timingSafeEqual(a: string, b: string): boolean {
  const enc = new TextEncoder();
  const ba = enc.encode(a);
  const bb = enc.encode(b);
  if (ba.length !== bb.length) {
    // still walk to reduce length oracle slightly
    let x = ba.length ^ bb.length;
    for (let i = 0; i < ba.length; i++) x |= ba[i]! ^ ba[i]!;
    return false;
  }
  let diff = 0;
  for (let i = 0; i < ba.length; i++) diff |= ba[i]! ^ bb[i]!;
  return diff === 0;
}

function clientIp(req: Request): string {
  return (
    req.headers.get("cf-connecting-ip") ||
    req.headers.get("x-forwarded-for")?.split(",")[0]?.trim() ||
    "unknown"
  );
}

/**
 * Best-effort sliding window via Cache API.
 * Not a hard guarantee under multi-colo, but raises the abuse bar for free tier.
 */
async function rateLimitOk(
  req: Request,
  max: number,
  windowS: number,
): Promise<boolean> {
  const ip = clientIp(req);
  const keyUrl = `https://ext-crash-rate.internal/report/${encodeURIComponent(ip)}`;
  const key = new Request(keyUrl);
  const cache = caches.default;

  let count = 0;
  const hit = await cache.match(key);
  if (hit) {
    count = Number.parseInt(await hit.text(), 10) || 0;
  }
  if (count >= max) return false;

  count += 1;
  await cache.put(
    key,
    new Response(String(count), {
      headers: {
        "cache-control": `max-age=${windowS}`,
        "content-type": "text/plain",
      },
    }),
  );
  return true;
}

function isSafeName(s: string): boolean {
  if (!s || s.length > MAX) return false;
  if (s.includes("..") || s.includes("\\") || s.includes("\0")) return false;
  if (s.startsWith("/") || s.includes("://")) return false;
  // no control / whitespace
  if (/[\x00-\x1f\x7f\s]/.test(s)) return false;
  return NAME_RE.test(s);
}

function sanitizeBody(raw: unknown): {
  ext_name: string;
  package_dir: string;
  kind: Kind;
  client?: string;
  grok_pi_ver?: string;
} | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const o = raw as Record<string, unknown>;

  for (const k of Object.keys(o)) {
    if (!ALLOWED_KEYS.has(k)) return null;
  }

  const kindRaw = o.kind;
  if (kindRaw !== undefined && kindRaw !== null) {
    if (kindRaw !== "crash" && kindRaw !== "combo" && kindRaw !== "unknown") {
      return null;
    }
  }
  const kind: Kind =
    kindRaw === "crash" || kindRaw === "combo" || kindRaw === "unknown"
      ? kindRaw
      : "crash";

  let ext_name = typeof o.ext_name === "string" ? clip(o.ext_name) : "";
  let package_dir =
    typeof o.package_dir === "string" ? clip(o.package_dir) : "";
  if (!ext_name && !package_dir) return null;
  if (!ext_name) ext_name = package_dir;
  if (!package_dir) package_dir = ext_name;

  if (!isSafeName(ext_name) || !isSafeName(package_dir)) return null;

  const out: {
    ext_name: string;
    package_dir: string;
    kind: Kind;
    client?: string;
    grok_pi_ver?: string;
  } = { ext_name, package_dir, kind };

  if (typeof o.client === "string") {
    const c = clip(o.client).slice(0, 32);
    if (!isSafeName(c) && !/^[A-Za-z0-9._-]+$/.test(c)) return null;
    out.client = c;
  }
  if (typeof o.grok_pi_ver === "string") {
    const v = clip(o.grok_pi_ver).slice(0, 32);
    if (!/^[A-Za-z0-9._+-]+$/.test(v)) return null;
    out.grok_pi_ver = v;
  }
  return out;
}

async function readJsonLimited(req: Request): Promise<unknown | null> {
  const cl = req.headers.get("content-length");
  if (cl && Number(cl) > MAX_BODY_BYTES) return null;
  const buf = await req.arrayBuffer();
  if (buf.byteLength > MAX_BODY_BYTES) return null;
  try {
    return JSON.parse(new TextDecoder().decode(buf));
  } catch {
    return null;
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.method === "OPTIONS") {
      return json({ ok: true });
    }

    const url = new URL(req.url);
    const path = url.pathname.replace(/\/+$/, "") || "/";
    const rateMax = Number.parseInt(env.RATE_LIMIT_MAX || "30", 10) || 30;
    const rateWin = Number.parseInt(env.RATE_LIMIT_WINDOW_S || "3600", 10) || 3600;

    try {
      if (req.method === "POST" && path === "/v1/report") {
        if (!env.REPORT_TOKEN) return misconfigured();
        if (!bearerOk(req, env.REPORT_TOKEN)) return unauthorized();
        if (!(await rateLimitOk(req, rateMax, rateWin))) return tooMany();

        const raw = await readJsonLimited(req);
        const body = sanitizeBody(raw);
        if (!body) return json({ error: "invalid payload" }, 400);

        const id = crypto.randomUUID();
        const created_at = new Date().toISOString();
        await env.DB.prepare(
          `INSERT INTO reports (id, created_at, ext_name, package_dir, kind, client, grok_pi_ver)
           VALUES (?, ?, ?, ?, ?, ?, ?)`,
        )
          .bind(
            id,
            created_at,
            body.ext_name,
            body.package_dir,
            body.kind,
            body.client ?? null,
            body.grok_pi_ver ?? null,
          )
          .run();

        return json({ ok: true, id }, 201);
      }

      if (req.method === "GET" && path === "/v1/stats") {
        // Public read — light rate limit (higher ceiling).
        if (!(await rateLimitOk(req, rateMax * 10, rateWin))) return tooMany();

        const top = await env.DB.prepare(
          `SELECT package_dir, ext_name, kind, COUNT(*) AS n
           FROM reports
           GROUP BY package_dir, ext_name, kind
           ORDER BY n DESC
           LIMIT 50`,
        ).all();

        const byDay = await env.DB.prepare(
          `SELECT substr(created_at, 1, 10) AS day, COUNT(*) AS n
           FROM reports
           GROUP BY day
           ORDER BY day DESC
           LIMIT 30`,
        ).all();

        const triage = await env.DB.prepare(
          `SELECT package_dir, status, note, updated_at FROM triage
           ORDER BY updated_at DESC LIMIT 100`,
        ).all();

        const recent = await env.DB.prepare(
          `SELECT created_at, ext_name, package_dir, kind, client, grok_pi_ver
           FROM reports
           ORDER BY created_at DESC
           LIMIT 50`,
        ).all();

        const total = await env.DB.prepare(
          `SELECT COUNT(*) AS n FROM reports`,
        ).first<{ n: number }>();

        return json({
          total: total?.n ?? 0,
          top: top.results ?? [],
          by_day: byDay.results ?? [],
          recent: recent.results ?? [],
          triage: triage.results ?? [],
        });
      }

      if (req.method === "POST" && path === "/v1/triage") {
        const admin = env.ADMIN_TOKEN || env.REPORT_TOKEN;
        if (!admin) return misconfigured();
        if (!bearerOk(req, admin)) return unauthorized();
        if (!(await rateLimitOk(req, Math.min(rateMax, 20), rateWin))) {
          return tooMany();
        }

        const raw = (await readJsonLimited(req)) as Record<
          string,
          unknown
        > | null;
        if (!raw || typeof raw.package_dir !== "string") {
          return json({ error: "package_dir required" }, 400);
        }
        const package_dir = clip(raw.package_dir);
        if (!isSafeName(package_dir)) {
          return json({ error: "invalid package_dir" }, 400);
        }
        const status =
          raw.status === "open" ||
          raw.status === "blocked" ||
          raw.status === "wontfix" ||
          raw.status === "fixed"
            ? raw.status
            : "open";
        const note =
          typeof raw.note === "string" ? clip(raw.note).slice(0, 500) : null;
        if (note && /[\x00-\x08\x0b\x0c\x0e-\x1f]/.test(note)) {
          return json({ error: "invalid note" }, 400);
        }
        const updated_at = new Date().toISOString();

        await env.DB.prepare(
          `INSERT INTO triage (package_dir, status, note, updated_at)
           VALUES (?, ?, ?, ?)
           ON CONFLICT(package_dir) DO UPDATE SET
             status = excluded.status,
             note = excluded.note,
             updated_at = excluded.updated_at`,
        )
          .bind(package_dir, status, note, updated_at)
          .run();

        return json({ ok: true, package_dir, status });
      }

      // `/` is the static dashboard (assets layer); health lives at `/health`.
      if (req.method === "GET" && path === "/health") {
        return json({
          ok: true,
          service: "ext-crash-telemetry",
          write_auth: Boolean(env.REPORT_TOKEN),
        });
      }

      return json({ error: "not found" }, 404);
    } catch (e) {
      return json(
        { error: e instanceof Error ? e.message : String(e) },
        500,
      );
    }
  },
};
