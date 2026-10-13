import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { spawn } from "node:child_process";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createConnection, createServer as createSocketServer, type Socket } from "node:net";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import path from "node:path";
import { access, chmod, mkdir, open, readFile, unlink, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import type { EvalParams } from "./eval.ts";
import type { EvalMcpOptions } from "./eval-pi-mcp.ts";
import { EvalTokenCounter } from "./eval-token-count.ts";
import { MCPEVAL_PROTOCOL, MAX_MCPEVAL_FRAME_BYTES, attachJsonLines, writeJsonLine } from "./mcpeval-protocol.ts";
import { eventMatches, normalizeEvent, MAX_EVENT_QUEUE_GLOBAL, MAX_EVENT_QUEUE_PER_SUBSCRIPTION, type McpevalEvent } from "./mcpeval-events.ts";
import { daemonDescriptor, daemonPidAlive, daemonPort, isUuid, mcpevalRoot, mcpevalSocketPath, mcpevalStatePath, type DaemonState } from "./mcpeval-registry.ts";
import { createMcpevalMcpServer } from "./mcpeval-mcp-facade.ts";
import { handleMcpevalWebRequest } from "./mcpeval-web.ts";
import { createMcpevalOAuthProvider, handleMcpevalOAuthRequest, verifyMcpevalBearer, type McpevalOAuthProvider } from "./mcpeval-oauth.ts";

type ImageRecord = { uri: string; name: string; mimeType: string; blob: string; bytes: number };

/**
 * Resident mcpeval transport.
 *
 * The original `startEvalPiMcp` endpoint is intentionally kept intact for
 * existing integrations.  The daemon below is the opt-in registration path:
 * one long-lived HTTP/MCP endpoint owns discovery, event queues and auth while
 * each Pi session registers over a private Unix socket.  Eval code still runs
 * in the registering Pi process; the daemon only forwards requests and never
 * creates another Pi, kernel, MCP client or tool pipeline.
 *
 * The wire protocol is deliberately small and versioned (`mcpeval/1`).  Frames
 * are newline-delimited JSON and capped at 1 MiB.  A registered Pi keeps its
 * socket open, receives `invoke` frames, and replies with matching `response`
 * frames.  Closing that socket unregisters the Pi; the daemon HTTP endpoint
 * remains alive and reports the binding as offline.  This is the lifecycle
 * boundary required for Herdr/system-service wakeups.
 */


export type McpevalDaemonOptions = {
	root?: string;
	port?: number;
	host?: string;
	/** Test/integration override. Production remains loopback-only by default. */
	allowNonLoopback?: boolean;
};

export type McpevalPiOptions = EvalMcpOptions & {
	root?: string;
	/** Optional daemon state path; defaults to `$GROK_HOME/eval-pi-mcp/daemon.json`. */
	daemonStatePath?: string;
};



type DaemonRegistration = {
	bindingId: string;
	epoch: string;
	secret: string;
	socket: Socket;
	metadata: Record<string, unknown>;
	pending: Map<string, { resolve: (value: unknown) => void; reject: (error: Error) => void; timer: NodeJS.Timeout }>;
};

type DaemonSubscription = {
	id: string;
	bindingId: string;
	filter: Record<string, unknown>;
	queue: McpevalEvent[];
};


function loopbackHost(host: string): boolean {
	return ["127.0.0.1", "localhost", "::1", "[::1]"].includes(host);
}

function sameSecret(provided: string, expected: string): boolean {
	const actual = Buffer.from(provided);
	const target = Buffer.from(expected);
	const valid = actual.length === target.length;
	return timingSafeEqual(valid ? actual : target, target) && valid;
}

function allowedOrigin(origin: string | undefined): boolean {
	if (!origin) return true;
	try {
		const url = new URL(origin);
		return ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) && ["http:", "https:"].includes(url.protocol);
	} catch {
		return false;
	}
}

function requireSdk(): { McpServer: new (...args: any[]) => any; ResourceTemplate: new (...args: any[]) => any; StreamableHTTPServerTransport: new (...args: any[]) => any; z: any } {
	const candidates = [
		process.env.PI_PACKAGE_DIR && path.join(process.env.PI_PACKAGE_DIR, "package.json"),
		path.join(homedir(), ".pi", "agent", "npm", "package.json"),
		path.join(process.cwd(), "package.json"),
	].filter((candidate): candidate is string => Boolean(candidate));
	for (const candidate of candidates) {
		try {
			const fromPi = createRequire(candidate);
			const { McpServer, ResourceTemplate } = fromPi("@modelcontextprotocol/sdk/server/mcp.js");
			const { StreamableHTTPServerTransport } = fromPi("@modelcontextprotocol/sdk/server/streamableHttp.js");
			const { z } = fromPi("zod");
			if (McpServer && ResourceTemplate && StreamableHTTPServerTransport && z) return { McpServer, ResourceTemplate, StreamableHTTPServerTransport, z };
		} catch {
			// Try the next supported Pi npm root.
		}
	}
	throw new Error("mcpeval daemon requires the official @modelcontextprotocol/sdk and zod in Pi's npm tree");
}





/**
 * Start the resident mcpeval daemon.  The returned `close` is only for the
 * daemon owner (normally a launchd/Herdr service), never for a Pi session.
 */
export async function startMcpevalDaemon(options: McpevalDaemonOptions = {}): Promise<{
	url: string;
	socketPath: string;
	statePath: string;
	descriptor: () => ReturnType<typeof daemonDescriptor>;
	discover: () => unknown[];
	publish: (bindingId: string, event: McpevalEvent) => void;
	close: () => Promise<void>;
}> {
	const root = mcpevalRoot(options.root);
	// Resolve runtime dependencies before taking the daemon lock. A missing SDK
	// must not strand a lock that prevents the next Pi from retrying startup.
	const tokenCounter = await EvalTokenCounter.create();
	const mcp = requireSdk();
	await mkdir(root, { recursive: true, mode: 0o700 });
	await chmod(root, 0o700);
	const lockPath = path.join(root, "daemon.lock");
	let lock: Awaited<ReturnType<typeof open>>;
	try {
		lock = await open(lockPath, "wx", 0o600);
		await lock.writeFile(`${process.pid}\n`);
	} catch (error) {
		if (lock) {
			await lock.close().catch(() => {});
			await unlink(lockPath).catch(() => {});
		}
		throw new Error(`mcpeval daemon is already running or lock is stale: ${String(error)}`);
	}
	const socketPath = mcpevalSocketPath(root);
	const statePath = mcpevalStatePath(root);
	const host = options.host?.trim() || "127.0.0.1";
	if (!options.allowNonLoopback && !loopbackHost(host)) {
		await lock.close();
		await unlink(lockPath).catch(() => {});
		throw new Error("mcpeval daemon refuses non-loopback host without allowNonLoopback");
	}
	await unlink(socketPath).catch(() => {});
	const secret = randomBytes(32).toString("hex");
	const registrations = new Map<string, DaemonRegistration>();
	const subscriptions = new Map<string, DaemonSubscription>();
	const seenEventIds = new Map<string, number>();
	let closed = false;
	let globalQueuedEvents = 0;
	let http: ReturnType<typeof createServer> | undefined;
	let socketServer: ReturnType<typeof createSocketServer> | undefined;
	let state: DaemonState | undefined;
	let oauthProvider: McpevalOAuthProvider | undefined;

	const failRegistration = (registration: DaemonRegistration, reason: string) => {
		registrations.delete(registration.bindingId);
		for (const pending of registration.pending.values()) {
			clearTimeout(pending.timer);
			pending.reject(new Error(`mcpeval Pi registration unavailable: ${reason}`));
		}
		registration.pending.clear();
		for (const [id, subscription] of subscriptions) {
			if (subscription.bindingId === registration.bindingId) {
				globalQueuedEvents -= subscription.queue.length;
				subscriptions.delete(id);
			}
		}
	};

	const publish = (bindingId: string, raw: McpevalEvent) => {
		const registration = registrations.get(bindingId);
		if (!registration || !raw || typeof raw.nativeType !== "string" || !raw.nativeType) return;
		const event = {
			...normalizeEvent(raw),
			instanceId: bindingId,
			bindingId,
			epoch: registration.epoch,
		};
		const dedupeKey = `${bindingId}:${event.eventId}`;
		if (seenEventIds.has(dedupeKey)) return;
		seenEventIds.set(dedupeKey, Date.now());
		while (seenEventIds.size > 4096) seenEventIds.delete(seenEventIds.keys().next().value!);
		for (const subscription of subscriptions.values()) {
			if (subscription.bindingId !== bindingId || !eventMatches(subscription.filter, event)) continue;
			if (subscription.queue.length >= MAX_EVENT_QUEUE_PER_SUBSCRIPTION) {
				globalQueuedEvents -= subscription.queue.length;
				subscription.queue = [{ ...event, nativeType: "agenvo.resync_required", native: { reason: "delivery_capacity" } }];
				globalQueuedEvents++;
				continue;
			}
			if (globalQueuedEvents >= MAX_EVENT_QUEUE_GLOBAL) {
				for (const candidate of subscriptions.values()) {
					if (!candidate.queue.length) continue;
					globalQueuedEvents -= candidate.queue.length;
					candidate.queue = [{ ...event, nativeType: "agenvo.resync_required", native: { reason: "global_delivery_capacity" } }];
					globalQueuedEvents++;
					break;
				}
				continue;
			}
			subscription.queue.push(event);
			globalQueuedEvents++;
		}
	};

	const registrationMetadata = () => [...registrations.values()].map((registration) => ({
		binding_id: registration.bindingId,
		...registration.metadata,
		state: "online",
	}));

	const invoke = (registration: DaemonRegistration, method: string, params: Record<string, unknown>, signal?: AbortSignal): Promise<any> => {
		if (closed || !registrations.has(registration.bindingId)) return Promise.reject(new Error("mcpeval Pi registration is offline"));
		const id = randomUUID();
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				registration.pending.delete(id);
				reject(new Error(`mcpeval Pi request timed out: ${method}`));
			}, 300_000);
			registration.pending.set(id, { resolve, reject, timer });
			const abort = () => {
				clearTimeout(timer);
				registration.pending.delete(id);
				try { writeJsonLine(registration.socket, { protocol: MCPEVAL_PROTOCOL, type: "cancel", id }); } catch { /* closed */ }
				reject(new Error("mcpeval Pi request aborted"));
			};
			if (signal?.aborted) {
				abort();
				return;
			}
			signal?.addEventListener("abort", abort, { once: true });
			try {
				writeJsonLine(registration.socket, { protocol: MCPEVAL_PROTOCOL, type: "invoke", id, method, params });
			} catch (error) {
				clearTimeout(timer);
				registration.pending.delete(id);
				signal?.removeEventListener("abort", abort);
				reject(error instanceof Error ? error : new Error(String(error)));
			}
		});
	};

	const discover = () => registrationMetadata();
	const findRegistration = (bindingId: unknown): DaemonRegistration => {
		if (!isUuid(bindingId)) throw new Error("binding_id must be a UUID");
		const registration = registrations.get(bindingId);
		if (!registration) throw new Error("Invalid or offline mcpeval binding_id");
		return registration;
	};

	const images = new Map<string, ImageRecord>();
	let imageBytes = 0;
	const addImage = (bindingId: string, image: { data: string; mimeType: string }): ImageRecord => {
		const bytes = Buffer.byteLength(image.data, "base64");
		if (!image.mimeType.startsWith("image/") || bytes > MAX_IMAGE_BYTES) throw new Error("Eval image exceeds MCP resource limit (16 MiB)");
		const uri = `eval-pi-mcp://${bindingId}/image/${randomUUID()}`;
		const record = { uri, name: `Eval image ${uri.slice(-8)}`, mimeType: image.mimeType, blob: image.data, bytes };
		while (images.size >= MAX_IMAGES || imageBytes + bytes > MAX_IMAGE_TOTAL_BYTES) {
			const oldest = images.keys().next().value;
			if (!oldest) break;
			imageBytes -= images.get(oldest)!.bytes;
			images.delete(oldest);
		}
		images.set(uri, record);
		imageBytes += bytes;
		return record;
	};

	const toMcpEvalResult = async (bindingId: string, raw: any, inputForUsage: string, tokenCounter: EvalTokenCounter) => {
		const result = raw?.result ?? raw;
		if (!result || typeof result !== "object") return withDaemonUsage(tokenCounter, inputForUsage, JSON.stringify(result));
		if (!Array.isArray(result.content)) return withDaemonUsage(tokenCounter, inputForUsage, JSON.stringify(result));
		const content = result.content.map((item: any) => {
			if (item?.type === "image" && typeof item.data === "string" && typeof item.mimeType === "string") {
				const image = addImage(bindingId, item);
				return { type: "resource_link", uri: image.uri, name: image.name, mimeType: image.mimeType, description: "Read via mcpeval resources/read." };
			}
			return item?.type === "text" ? { type: "text", text: item.text ?? "" } : { type: "text", text: `[unsupported Eval content type: ${String(item?.type)}]` };
		});
		const text = content.filter((item: any) => item.type === "text").map((item: any) => item.text).join("\n");
		return { isError: Boolean(result.isError), ...await withDaemonUsage(tokenCounter, inputForUsage, text, content) };
	};

	const port = daemonPort(options.port);
	const makeMcpServer = () => createMcpevalMcpServer({
		mcp,
		protocol: MCPEVAL_PROTOCOL,
		maxCodeChars: 200_000,
		state,
		daemonDescriptor,
		discover,
		findRegistration,
		registrations,
		invoke,
		tokenCounter,
		withDaemonUsage,
		toMcpEvalResult,
		subscriptions,
		images,
		randomUUID,
		getGlobalQueuedEvents: () => globalQueuedEvents,
		setGlobalQueuedEvents: (value) => { globalQueuedEvents = value; },
	});
	const handleSocket = (socket: Socket) => {
		let registration: DaemonRegistration | undefined;
		const detach = attachJsonLines(socket, (message) => {
			if (message.protocol !== MCPEVAL_PROTOCOL) { socket.destroy(new Error("unsupported mcpeval protocol")); return; }
			if (message.type === "register") {
				if (registration) { socket.destroy(new Error("duplicate mcpeval register")); return; }
				const bindingId = randomUUID();
				const bindingSecret = randomBytes(32).toString("hex");
				registration = { bindingId, epoch: randomUUID(), secret: bindingSecret, socket, metadata: typeof message.metadata === "object" && message.metadata ? message.metadata as Record<string, unknown> : {}, pending: new Map() };
				registrations.set(bindingId, registration);
				writeJsonLine(socket, { protocol: MCPEVAL_PROTOCOL, type: "registered", id: message.id, bindingId, url: `${state!.url}&binding_id=${bindingId}`, daemon: daemonDescriptor(state!) });
				return;
			}
			if (!registration) { socket.destroy(new Error("mcpeval register required")); return; }
			if (message.type === "response" && typeof message.id === "string") {
				const pending = registration.pending.get(message.id);
				if (!pending) return;
				clearTimeout(pending.timer);
				registration.pending.delete(message.id);
				if (message.ok === false) pending.reject(new Error(typeof message.error === "string" ? message.error : "mcpeval Pi request failed")); else pending.resolve(message.result);
				return;
			}
			if (message.type === "event" && message.event && typeof message.event === "object") publish(registration.bindingId, message.event as McpevalEvent);
		}, () => {
			if (registration) failRegistration(registration, "socket closed");
			detach();
		});
	};

	socketServer = createSocketServer(handleSocket);
	await new Promise<void>((resolve, reject) => {
		socketServer!.once("error", reject);
		socketServer!.listen(socketPath, () => resolve());
	});
	const authorityHost = host.includes(":") && !host.startsWith("[") ? `[${host}]` : host;
	http = createServer(async (request: IncomingMessage, response: ServerResponse) => {
		if (closed) { response.writeHead(410).end("mcpeval daemon closed"); return; }
		const route = new URL(request.url ?? "/", `http://${authorityHost}`);
		const requestHost = (request.headers.host ?? "").replace(/:\d+$/, "");
		if (!options.allowNonLoopback && (!loopbackHost(requestHost) || !allowedOrigin(request.headers.origin))) { response.writeHead(403).end("Non-loopback mcpeval origin/host refused"); return; }
		if (oauthProvider && await handleMcpevalOAuthRequest(request, response, oauthProvider, { resourcePath: "/mcp" })) return;
		const bearer = typeof request.headers.authorization === "string" && request.headers.authorization.startsWith("Bearer ") ? request.headers.authorization.slice(7) : "";
		const oauthAuth = bearer && oauthProvider ? await verifyMcpevalBearer(oauthProvider, request.headers.authorization) : undefined;
		const keyAuthorized = sameSecret(route.searchParams.get("key") || bearer, secret);
		if (!keyAuthorized && !oauthAuth) {
			response.writeHead(401, { "WWW-Authenticate": "Bearer" }).end("Invalid mcpeval daemon key or OAuth token");
			return;
		}
		if (route.pathname !== "/mcp") {
			if (await handleMcpevalWebRequest(request, response, {
				descriptor: () => daemonDescriptor(state!),
				discover,
				getEventQueueSize: () => globalQueuedEvents,
				oauth: () => oauthProvider ? { enabled: true, ...oauthProvider.metadata() } : { enabled: false },
			})) return;
			response.writeHead(404).end();
			return;
		}
		if (request.method !== "POST") { response.writeHead(405, { Allow: "POST" }).end(); return; }
		const server = makeMcpServer();
		const transport = new mcp.StreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true });
		try { await server.connect(transport); await transport.handleRequest(request, response); }
		catch (error) { if (!response.headersSent) response.writeHead(500).end("mcpeval transport error"); else if (!response.writableEnded) response.end(); console.error("[mcpeval] request failed:", error); }
		finally { await server.close().catch(() => {}); }
	});
	await new Promise<void>((resolve, reject) => {
		http!.once("error", reject);
		http!.listen(port, host, () => resolve());
	});
	const address = http.address();
	if (!address || typeof address === "string") throw new Error("mcpeval daemon failed to get TCP port");
	const actualIssuerUrl = new URL(`http://${authorityHost}:${address.port}`);
	state = { protocol: MCPEVAL_PROTOCOL, pid: process.pid, socketPath, url: `${actualIssuerUrl}mcp?key=${secret}`, secret, startedAtMs: Date.now() };
	if (process.env.PI_GROK_MCPEVAL_OAUTH !== "0") {
		oauthProvider = createMcpevalOAuthProvider({
			issuerUrl: actualIssuerUrl,
			resourceServerUrl: new URL("/mcp", actualIssuerUrl),
			statePath: path.join(root, "oauth.json"),
		});
		await oauthProvider.ready();
	}
	await writeFile(statePath, `${JSON.stringify(state)}\n`, { mode: 0o600 });
	const descriptor = () => daemonDescriptor(state!);
	const close = async () => {
		if (closed) return;
		closed = true;
		for (const registration of [...registrations.values()]) { failRegistration(registration, "daemon shutdown"); registration.socket.destroy(); }
		registrations.clear();
		subscriptions.clear();
		if (http) await new Promise<void>((resolve) => { http!.close(() => resolve()); http!.closeAllConnections(); });
		if (socketServer) await new Promise<void>((resolve) => { socketServer!.close(() => resolve()); });
		await unlink(socketPath).catch(() => {});
		try { const recorded = JSON.parse(await readFile(statePath, "utf8")) as DaemonState; if (recorded.pid === process.pid && recorded.url === state?.url) await unlink(statePath); } catch { /* already cleaned/stale */ }
		await lock.close();
		await unlink(lockPath).catch(() => {});
	};
	return { url: state.url, socketPath, statePath, descriptor, discover, publish, close };
}

async function withDaemonUsage(tokenCounter: EvalTokenCounter, input: string, output: string, content: Array<Record<string, unknown>> = [{ type: "text", text: output }]) {
	const usage = await tokenCounter.count(input, output);
	return { content: [...content, { type: "text", text: `[mcpeval token_usage] ${JSON.stringify(usage)}` }], structuredContent: { token_usage: usage } };
}

async function probeMcpevalSocket(socketPath: string): Promise<boolean> {
	return await new Promise<boolean>((resolve) => {
		const socket = createConnection(socketPath);
		let settled = false;
		const finish = (value: boolean) => {
			if (settled) return;
			settled = true;
			clearTimeout(timer);
			socket.destroy();
			resolve(value);
		};
		const timer = setTimeout(() => finish(false), 750);
		socket.once("connect", () => finish(true));
		socket.once("error", () => finish(false));
	});
}

/**
 * Ensure the resident daemon exists, starting one detached copy when needed.
 * Pi sessions never own the returned daemon and therefore never close it.
 */
export async function ensureMcpevalDaemon(root?: string): Promise<ReturnType<typeof daemonDescriptor>> {
	const resolvedRoot = mcpevalRoot(root);
	const statePath = mcpevalStatePath(resolvedRoot);
	const socketPath = mcpevalSocketPath(resolvedRoot);
	let staleState = false;
	try {
		const state = JSON.parse(await readFile(statePath, "utf8")) as DaemonState;
		if (state.protocol === MCPEVAL_PROTOCOL && daemonPidAlive(state.pid) && await probeMcpevalSocket(state.socketPath)) {
			return daemonDescriptor(state);
		}
		staleState = true;
	} catch {
		// The first Pi starts the daemon; a concurrent starter is handled by the
		// polling loop below and the daemon lock remains the authority.
	}
	if (staleState) await unlink(statePath).catch(() => {});
	let staleDaemonLock = false;
	try {
		const lockPid = Number.parseInt(await readFile(path.join(resolvedRoot, "daemon.lock"), "utf8"), 10);
		if (!daemonPidAlive(lockPid)) {
			staleDaemonLock = true;
			await unlink(path.join(resolvedRoot, "daemon.lock"));
		}
	} catch { /* no lock or a live starter still owns it */ }
	if (staleState && staleDaemonLock) await unlink(socketPath).catch(() => {});
	await mkdir(resolvedRoot, { recursive: true, mode: 0o700 });
	const source = fileURLToPath(import.meta.url);
	const runtimeArgs = process.execArgv.filter((arg) =>
		arg === "--experimental-strip-types" || arg === "--experimental-transform-types" || arg === "--enable-source-maps",
	);
	const launchArgs = [...runtimeArgs];
	if (source.endsWith(".ts")) {
		const tsxCandidates = [
			process.env.PI_PACKAGE_DIR && path.join(process.env.PI_PACKAGE_DIR, "node_modules", "tsx", "dist", "loader.mjs"),
			path.join(path.dirname(source), "..", "..", "pi-main", "node_modules", "tsx", "dist", "loader.mjs"),
			path.join(process.cwd(), "pi-main", "node_modules", "tsx", "dist", "loader.mjs"),
			path.join(homedir(), ".pi", "agent", "npm", "node_modules", "tsx", "dist", "loader.mjs"),
		].filter((candidate): candidate is string => Boolean(candidate));
		let tsxLoader: string | undefined;
		for (const candidate of tsxCandidates) {
			try { await access(candidate); tsxLoader = candidate; break; } catch { /* try the next Pi runtime root */ }
		}
		if (tsxLoader) launchArgs.push("--import", tsxLoader);
		else if (!launchArgs.includes("--experimental-strip-types")) launchArgs.unshift("--experimental-strip-types");
	}
	const child = spawn(process.execPath, [...launchArgs, source, "--mcpeval-daemon"], {
		cwd: process.cwd(),
		env: { ...process.env, PI_GROK_MCPEVAL_ROOT: resolvedRoot },
		detached: true,
		stdio: "ignore",
	});
	child.unref();
	const deadline = Date.now() + 10_000;
	while (Date.now() < deadline) {
		try {
			const state = JSON.parse(await readFile(statePath, "utf8")) as DaemonState;
			if (state.protocol === MCPEVAL_PROTOCOL && daemonPidAlive(state.pid) && await probeMcpevalSocket(state.socketPath)) {
				return daemonDescriptor(state);
			}
		} catch {
			// The detached child may still be binding the Unix socket and writing
			// the atomic descriptor. Keep the wait bounded and observable.
		}
		await delay(100);
	}
	throw new Error(`mcpeval daemon did not become ready at ${statePath}`);
}

/** Read the resident daemon descriptor without probing or starting a process. */
export async function discoverMcpevalDaemon(root?: string): Promise<ReturnType<typeof daemonDescriptor>> {
	const state = JSON.parse(await readFile(mcpevalStatePath(root), "utf8")) as DaemonState;
	if (state.protocol !== MCPEVAL_PROTOCOL) throw new Error("Unsupported mcpeval daemon protocol");
	return daemonDescriptor(state);
}

if (process.argv.includes("--mcpeval-daemon")) {
	const root = process.env.PI_GROK_MCPEVAL_ROOT?.trim() || undefined;
	void startMcpevalDaemon({ root }).then((daemon) => {
		let stopping: Promise<void> | undefined;
		const stop = () => {
			stopping ??= daemon.close();
			return stopping;
		};
		for (const signal of ["SIGTERM", "SIGINT"] as const) {
			process.once(signal, () => void stop().finally(() => process.exit(0)));
		}
	}).catch((error) => {
		console.error(`[mcpeval] daemon startup failed: ${error instanceof Error ? error.stack ?? error.message : String(error)}`);
		process.exitCode = 1;
	});
}
