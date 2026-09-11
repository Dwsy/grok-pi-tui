/**
 * Loopback HTTP server for the grok-pi web config surface.
 *
 * Bound to 127.0.0.1 by default with a random free port and a per-start token,
 * so only the browser tab grok-pi opened can drive it.
 */
import { randomBytes, timingSafeEqual } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import {
	createServer,
	type IncomingMessage,
	type Server,
	type ServerResponse,
} from "node:http";

const MAX_BODY_BYTES = 2 * 1024 * 1024;

export interface WebConfigServerDeps {
	host: string;
	/** 0 picks a random free port. */
	port: number;
	/** Absolute path of the injected HTML template; sibling CSS/JS/JSON are assembled at startup. */
	uiHtmlPath: string;
	loadState: () => Promise<unknown>;
	saveModels: (doc: unknown) => void | Promise<void>;
	saveSettings: (doc: unknown) => void | Promise<void>;
	useModel: (provider: string, modelId: string) => Promise<void>;
	reload: () => Promise<void>;
	/** Persist grok-pi `[ui]` F2 settings (TOML scalars only). */
	saveHostUi: (updates: unknown) => void | Promise<void>;
	/** Called once the server stops (page "Stop server" button). */
	onClose?: () => void;
}

export interface WebConfigServer {
	url: string;
	port: number;
	close: () => Promise<void>;
}

type RouteResult = { status?: number; payload?: unknown; close?: boolean };

type Route = (body: unknown, url: URL) => Promise<RouteResult | void>;

function sendJson(res: ServerResponse, status: number, payload: unknown): void {
	const text = JSON.stringify(payload);
	res.writeHead(status, {
		"content-type": "application/json; charset=utf-8",
		"cache-control": "no-store",
	});
	res.end(text);
}

function tokensMatch(expected: string, actual: string | undefined): boolean {
	if (!actual) return false;
	const a = Buffer.from(expected);
	const b = Buffer.from(actual);
	return a.length === b.length && timingSafeEqual(a, b);
}

/** Bind host check: refuses requests that a remote (rebound) host would send. */
function hostAllowed(req: IncomingMessage): boolean {
	const host = req.headers.host;
	if (!host) return true;
	const name = host.replace(/:\d+$/, "").toLowerCase();
	return name === "localhost" || name === "127.0.0.1" || name === "[::1]" || name === "::1";
}

function readBody(req: IncomingMessage): Promise<unknown> {
	return new Promise((resolve, reject) => {
		const chunks: Buffer[] = [];
		let size = 0;
		req.on("data", (chunk: Buffer) => {
			size += chunk.length;
			if (size > MAX_BODY_BYTES) {
				reject(new Error("request body too large"));
				req.destroy();
				return;
			}
			chunks.push(chunk);
		});
		req.on("end", () => {
			const raw = Buffer.concat(chunks).toString("utf8").trim();
			if (!raw) {
				resolve(undefined);
				return;
			}
			try {
				resolve(JSON.parse(raw));
			} catch {
				reject(new Error("request body is not valid JSON"));
			}
		});
		req.on("error", reject);
	});
}

export async function startWebConfigServer(deps: WebConfigServerDeps): Promise<WebConfigServer> {
	const token = randomBytes(24).toString("hex");
	const uiDir = dirname(deps.uiHtmlPath);
	const styleMarker = "__PI_GROK_WEB_CONFIG_STYLES__";
	const appMarker = "__PI_GROK_WEB_CONFIG_APP__";
	const jsonAssets = [
		["__PI_GROK_WEB_CONFIG_UI_CONFIG__", "ui-config.json"],
		["__PI_GROK_WEB_CONFIG_I18N__", "i18n.json"],
	] as const;
	const appFragments = [
		["__PI_GROK_WEB_CONFIG_MODELS__", "models.js"],
		["__PI_GROK_WEB_CONFIG_RESOURCES__", "resources.js"],
		["__PI_GROK_WEB_CONFIG_HOST__", "host.js"],
		["__PI_GROK_WEB_CONFIG_SETTINGS__", "settings.js"],
	] as const;

	let html = readFileSync(deps.uiHtmlPath, "utf8");
	if (!html.includes(styleMarker) || !html.includes(appMarker)) {
		throw new Error("Pi web config UI template is missing CSS/JS assembly markers");
	}

	const styles = readFileSync(join(uiDir, "styles.css"), "utf8");
	if (styles.toLowerCase().includes("</style>")) {
		throw new Error("Pi web config styles must not contain a closing style tag");
	}
	let app = readFileSync(join(uiDir, "app.js"), "utf8");
	for (const [marker, filename] of jsonAssets) {
		if (!app.includes(marker)) {
			throw new Error(`Pi web config app is missing JSON marker ${marker}`);
		}
		const source = readFileSync(join(uiDir, filename), "utf8");
		let parsed: unknown;
		try {
			parsed = JSON.parse(source);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			throw new Error(`Pi web config ${filename} is invalid JSON: ${message}`);
		}
		if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
			throw new Error(`Pi web config ${filename} must contain a JSON object`);
		}
		app = app.replace(marker, JSON.stringify(parsed).replaceAll("<", "\\u003c"));
	}
	for (const [marker, filename] of appFragments) {
		if (!app.includes(marker)) {
			throw new Error(`Pi web config app is missing fragment marker ${marker}`);
		}
		app = app.replace(marker, readFileSync(join(uiDir, filename), "utf8"));
	}
	if (app.toLowerCase().includes("</script>")) {
		throw new Error("Pi web config app fragments must not contain a closing script tag");
	}

	html = html.replace(styleMarker, styles).replace(appMarker, app);
	if (html.includes("__PI_GROK_WEB_CONFIG_TOKEN__")) {
		html = html.replaceAll("__PI_GROK_WEB_CONFIG_TOKEN__", token);
	}

	const routes = new Map<string, Route>([
		[
			"GET /api/state",
			async () => ({ payload: await deps.loadState() }),
		],
		[
			"PUT /api/models",
			async (body) => {
				await deps.saveModels(body);
				await deps.reload();
				return { payload: { ok: true } };
			},
		],
		[
			"PUT /api/settings",
			async (body) => {
				await deps.saveSettings(body);
				await deps.reload();
				return { payload: { ok: true } };
			},
		],
		[
			"POST /api/use-model",
			async (body) => {
				const request = (body ?? {}) as { provider?: unknown; modelId?: unknown };
				if (typeof request.provider !== "string" || typeof request.modelId !== "string") {
					return { status: 400, payload: { error: "provider and modelId are required" } };
				}
				await deps.useModel(request.provider, request.modelId);
				return { payload: { ok: true } };
			},
		],
		[
			"PUT /api/host-ui",
			async (body) => {
				if (!body || typeof body !== "object" || Array.isArray(body)) {
					return { status: 400, payload: { error: "body must be a JSON object of [ui] keys" } };
				}
				await deps.saveHostUi(body);
				return { payload: { ok: true } };
			},
		],
		[
			"POST /api/reload",
			async () => {
				await deps.reload();
				return { payload: { ok: true } };
			},
		],
		["POST /api/shutdown", async () => ({ payload: { ok: true }, close: true })],
	]);

	const server: Server = createServer((req, res) => {
		void (async () => {
			try {
				if (!hostAllowed(req)) {
					sendJson(res, 403, { error: "host not allowed" });
					return;
				}
				const url = new URL(req.url ?? "/", "http://127.0.0.1");
				if (req.method === "GET" && url.pathname === "/") {
					res.writeHead(200, {
						"content-type": "text/html; charset=utf-8",
						"cache-control": "no-store",
					});
					res.end(html);
					return;
				}
				if (!tokensMatch(token, url.searchParams.get("token") ?? headerToken(req))) {
					sendJson(res, 401, { error: "missing or invalid token" });
					return;
				}
				const route = routes.get(`${req.method ?? "GET"} ${url.pathname}`);
				if (!route) {
					sendJson(res, 404, { error: "not found" });
					return;
				}
				const body = req.method === "GET" ? undefined : await readBody(req);
				const result = await route(body, url);
				sendJson(res, result?.status ?? 200, result?.payload ?? { ok: true });
				if (result?.close) shutdown(server, deps);
			} catch (error) {
				sendJson(res, 500, {
					error: error instanceof Error ? error.message : String(error),
				});
			}
		})();
	});

	await new Promise<void>((resolve, reject) => {
		server.once("error", reject);
		server.listen(deps.port, deps.host, resolve);
	});
	const address = server.address();
	const port = typeof address === "object" && address !== null ? address.port : deps.port;
	return {
		url: `http://${deps.host}:${port}/?token=${token}`,
		port,
		close: () =>
			new Promise<void>((resolve) => {
				server.closeAllConnections();
				server.close(() => resolve());
			}),
	};
}

function headerToken(req: IncomingMessage): string | undefined {
	const value = req.headers["x-pi-token"];
	return Array.isArray(value) ? value[0] : value;
}

function shutdown(server: Server, deps: WebConfigServerDeps): void {
	// Deferred so the shutdown response is flushed before the socket closes.
	setTimeout(() => {
		server.closeAllConnections();
		server.close(() => deps.onClose?.());
	}, 50);
}
