/**
 * Authenticated, binding-scoped MCP facade around the *actual* Eval v2 tool.
 * No second Eval interpreter, Pi session or extension runner is created here.
 */
import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import path from "node:path";
import { chmod, mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";

import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { EvalParams, EvalSkillMetadata } from "./eval.ts";
import type { EvalToolMetadata } from "./tool-bridge.ts";
import { EvalTokenCounter } from "./eval-token-count.ts";

const MAX_IMAGES = 48;
const MAX_IMAGE_BYTES = 16 * 1024 * 1024;
const MAX_IMAGE_TOTAL_BYTES = 64 * 1024 * 1024;
const MAX_CONTEXT_CHARS = 120_000;
const MAX_CODE_CHARS = 200_000;

export type EvalMcpToolResult = {
	content: Array<{ type: string; text?: string; data?: string; mimeType?: string; [key: string]: unknown }>;
	isError?: boolean;
};

export type EvalMcpOptions = {
	context: ExtensionContext;
	languages: readonly ("js" | "py")[];
	description: string;
	guidelines: readonly string[];
	catalog: () => EvalToolMetadata[];
	skills: () => EvalSkillMetadata[];
	execute: (params: EvalParams, signal: AbortSignal) => Promise<EvalMcpToolResult>;
	notify: (text: string) => void;
};

type ImageRecord = { uri: string; name: string; mimeType: string; blob: string; bytes: number };

function homeDir(): string {
	return process.env.GROK_HOME?.trim() || path.join(homedir(), ".grok-pi");
}

/**
 * Exclusive local file lock; the very short registry critical section makes
 * permanent, never-reused issued IDs durable across Pi process restarts.
 * Fail closed on a stale lock rather than unlink another process's lock.
 */
export async function reserveEvalMcpBindingId(root = path.join(homeDir(), "eval-pi-mcp")): Promise<string> {
	await mkdir(root, { recursive: true, mode: 0o700 });
	await chmod(root, 0o700);
	const lockPath = path.join(root, "bindings.lock");
	let lock: Awaited<ReturnType<typeof open>> | undefined;
	for (let attempt = 0; attempt < 100; attempt++) {
		try {
			lock = await open(lockPath, "wx", 0o600);
			break;
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
			await delay(15 + (attempt % 7) * 7);
		}
	}
	if (!lock) throw new Error(`Eval MCP ID registry lock is busy: ${lockPath} (check for a stale lock after stopping other grok-pi instances)`);
	try {
		const file = path.join(root, "issued-ids.json");
		let issued: string[] = [];
		try {
			const decoded: unknown = JSON.parse(await readFile(file, "utf8"));
			if (!Array.isArray(decoded) || decoded.some((id) => typeof id !== "string")) {
				throw new Error("Eval MCP ID registry is invalid; refusing to reuse IDs");
			}
			issued = decoded as string[];
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
		}
		const used = new Set(issued);
		let id = randomUUID();
		while (used.has(id)) id = randomUUID();
		const next = path.join(root, `issued-ids.${process.pid}.${randomUUID()}.tmp`);
		try {
			const handle = await open(next, "wx", 0o600);
			try {
				await handle.writeFile(JSON.stringify([...issued, id]) + "\n");
				await handle.sync();
			} finally {
				await handle.close();
			}
			await rename(next, file);
		} finally {
			await unlink(next).catch((error: NodeJS.ErrnoException) => {
				if (error.code !== "ENOENT") throw error;
			});
		}
		return id;
	} finally {
		await lock.close();
		await unlink(lockPath);
	}
}

function sameSecret(provided: string, expected: string): boolean {
	const actual = Buffer.from(provided);
	const target = Buffer.from(expected);
	// A mismatched length still incurs a constant-time comparison.
	const valid = actual.length === target.length;
	return timingSafeEqual(valid ? actual : target, target) && valid;
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
			if (McpServer && ResourceTemplate && StreamableHTTPServerTransport && z) {
				return { McpServer, ResourceTemplate, StreamableHTTPServerTransport, z };
			}
		} catch {
			// Try Pi's other supported package roots before reporting missing SDK.
		}
	}
	throw new Error("eval-pi-mcp requires the official @modelcontextprotocol/sdk and zod in Pi's npm tree (npm install --prefix ~/.pi/agent/npm @modelcontextprotocol/sdk zod)");
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

function sessionSnapshot(ctx: ExtensionContext, limit: number) {
	const branch = ctx.sessionManager.getBranch();
	const entries = branch.filter((entry) =>
		entry.type === "message" || entry.type === "compaction" || entry.type === "branch_summary"
	);
	// Session JSONL may contain image/file base64. MCP context is a *text*
	// snapshot; binary travels through resource URIs on execute_eval instead.
	const visible = entries.slice(-limit).map((entry) => {
		const serialized = JSON.stringify(entry, (key, value) => {
			if (key === "data" && typeof value === "string") {
				return `[binary payload omitted: ${value.length} chars]`;
			}
			if (key === "text" && typeof value === "string" && value.length > 8000) {
				return `${value.slice(0, 8000)} [text truncated]`;
			}
			return value;
		});
		return serialized.length <= 12_000
			? JSON.parse(serialized) as unknown
			: { type: entry.type, truncated: true, preview: serialized.slice(0, 10_000) };
	});
	const result = {
		session_id: ctx.sessionManager.getSessionId(),
		cwd: ctx.cwd,
		model: ctx.model ? { provider: ctx.model.provider, id: ctx.model.id } : null,
		kind: "active_branch_transcript_snapshot",
		note: "Snapshot of persisted active Pi branch entries, not the exact pending LLM/system prompt.",
		total_entries: entries.length,
		omitted_entries: entries.length - visible.length,
		entries: visible,
		truncated: false,
	};
	let encoded = JSON.stringify(result);
	while (encoded.length > MAX_CONTEXT_CHARS && result.entries.length > 0) {
		result.entries.shift();
		result.omitted_entries += 1;
		result.truncated = true;
		encoded = JSON.stringify(result);
	}
	return encoded;
}

export async function startEvalPiMcp(options: EvalMcpOptions): Promise<{ bindingId: string; url: string; close: () => Promise<void> }> {
	const { McpServer, ResourceTemplate, StreamableHTTPServerTransport, z } = requireSdk();
	const tokenCounter = await EvalTokenCounter.create();
	const bindingId = await reserveEvalMcpBindingId();
	const secret = randomBytes(32).toString("hex");
	const images = new Map<string, ImageRecord>();
	let imageBytes = 0;
	let closed = false;
	let port = 0;
	const validateBinding = (id: string) => {
		if (closed || id !== bindingId) throw new Error("Invalid or expired Eval MCP binding_id");
	};
	const toolSchema = { binding_id: z.string().uuid() };
	const withUsage = async (input: string, output: string, content: Array<Record<string, unknown>> = [{ type: "text", text: output }]) => {
		const usage = await tokenCounter.count(input, output);
		return {
			content: [...content, { type: "text", text: `[eval-pi-mcp token_usage] ${JSON.stringify(usage)}` }],
			structuredContent: { token_usage: usage },
		};
	};
	const addImage = (image: { data: string; mimeType: string }): ImageRecord => {
		const bytes = Buffer.byteLength(image.data, "base64");
		if (!image.mimeType.startsWith("image/") || bytes > MAX_IMAGE_BYTES) {
			throw new Error("Eval image exceeds MCP resource limit (16 MiB)");
		}
		const id = randomUUID();
		const record: ImageRecord = {
			uri: `eval-pi-mcp://${bindingId}/image/${id}`,
			name: `Eval image ${id.slice(0, 8)}`,
			mimeType: image.mimeType,
			blob: image.data,
			bytes,
		};
		while (images.size >= MAX_IMAGES || imageBytes + bytes > MAX_IMAGE_TOTAL_BYTES) {
			const oldest = images.keys().next().value;
			if (!oldest) break;
			imageBytes -= images.get(oldest)!.bytes;
			images.delete(oldest);
		}
		images.set(record.uri, record);
		imageBytes += bytes;
		return record;
	};
	const resourceList = () => [...images.values()].map(({ uri, name, mimeType }) => ({
		uri, name, mimeType, description: "Image returned by Eval v2 (including Pi read-image results).",
	}));
	const makeMcpServer = () => {
		const mcp = new McpServer(
			{ name: "eval-pi-mcp", version: "1.0.0" },
			{ instructions: "Use the binding_id supplied by the grok-pi owner on EVERY tool call. The URL key authorizes this connection. Start with get_desc, then get_context and execute_eval." },
		);
		mcp.registerTool("get_context", {
			description: "Read the current Pi session's active-branch transcript snapshot (bounded).",
			inputSchema: { ...toolSchema, limit: z.number().int().min(1).max(200).optional() },
		}, async ({ binding_id, limit }: { binding_id: string; limit?: number }) => {
			validateBinding(binding_id);
			return withUsage(JSON.stringify({ limit: limit ?? 60 }), sessionSnapshot(options.context, limit ?? 60));
		});
		mcp.registerTool("get_desc", {
			description: "Get the Eval v2 usage guide, available languages, Pi-registered nested tools and their schemas, and discoverable skills.",
			inputSchema: toolSchema,
		}, async ({ binding_id }: { binding_id: string }) => {
			validateBinding(binding_id);
			return withUsage("get_desc", JSON.stringify({
				binding_id: bindingId,
				tokenizer: { engine: "tokenizers", ready: tokenCounter.ready, file: tokenCounter.file, error: tokenCounter.error ?? null },
				eval_description: options.description,
				eval_guidelines: options.guidelines,
				languages: options.languages,
				usage: "Pass binding_id to every get_desc/get_context/execute_eval call. In a JavaScript Eval v2 code cell discover tools with tools.list(), tools.search(regex), tools.describe(name), then await tool.<name>({...}); discover skills with skills.list()/search()/describe() and await skills.read(name). Cells have independent lexical scopes; use store(key,value)/load(key) for explicit state. Use is_background for long work and get_task_output/wait_tasks/kill_task for task management when available. Native Pi read ImageContent is automatically converted to an MCP resource_link without display(); use resources/read on its URI. display(imageBlock) also works. Never print base64 images as text.",
				example_js: 'const discovered = tools.search("read"); const result = await tool.read({path:"README.md"}); console.log(result.text);',
				registered_pi_tools: options.catalog(),
				skills: options.skills(),
			}));
		});
		mcp.registerTool("execute_eval", {
			description: "Execute one cell in this live grok-pi session's actual Eval v2 runtime; returns the genuine tool result (images become MCP resources).",
			inputSchema: {
				...toolSchema,
				language: z.enum(["js", "py"]),
				code: z.string().min(1).max(MAX_CODE_CHARS),
				title: z.string().min(1).max(150).optional(),
				timeout: z.number().min(0).max(86400).optional(),
				reset: z.boolean().optional(),
				is_background: z.boolean().optional(),
			},
		}, async ({ binding_id, ...params }: EvalParams & { binding_id: string }, extra: { signal: AbortSignal }) => {
			validateBinding(binding_id);
			if (!options.languages.includes(params.language)) throw new Error(`Eval language ${params.language} is disabled`);
			const result = await options.execute(params, extra.signal ?? new AbortController().signal);
			const contents = result.content.map((item) => {
				if (item.type === "image" && typeof item.data === "string" && typeof item.mimeType === "string") {
					const record = addImage({ data: item.data, mimeType: item.mimeType });
					return {
						type: "resource_link" as const, uri: record.uri, name: record.name, mimeType: record.mimeType,
						description: "Read via MCP resources/read to retrieve the image blob.",
					};
				}
				return item.type === "text"
					? { type: "text" as const, text: item.text ?? "" }
					: { type: "text" as const, text: `[unsupported Eval content type: ${item.type}]` };
			});
			const accounted = await withUsage(
				JSON.stringify(params),
				contents.filter((item) => item.type === "text").map((item) => item.text ?? "").join("\n"),
				contents,
			);
			return {
				isError: result.isError ?? false,
				...accounted,
			};
		});
		const template = new ResourceTemplate(`eval-pi-mcp://${bindingId}/image/{image_id}`, {
			list: async () => ({ resources: resourceList() }),
		});
		mcp.registerResource("Eval image", template, {
			description: "Images from Eval v2 executions, including nested Pi read-image outputs.",
			mimeType: "image/*",
		}, async (uri: URL) => {
			if (closed || uri.hostname !== bindingId) throw new Error("Invalid or expired Eval MCP image binding");
			const image = images.get(uri.href);
			if (!image) throw new Error("Eval MCP image resource expired or does not exist");
			return { contents: [{ uri: image.uri, mimeType: image.mimeType, blob: image.blob }] };
		});
		return mcp;
	};

	// Each POST has its own official *stateless* MCP transport. Shared Eval and
	// resource state belong to this binding, never to an SDK client session.
	const http = createServer(async (req: IncomingMessage, res: ServerResponse) => {
		if (closed) { res.writeHead(410).end("Binding expired"); return; }
		const route = new URL(req.url ?? "/", "http://127.0.0.1");
		if (route.pathname !== "/mcp") { res.writeHead(404).end(); return; }
		if (!allowedOrigin(req.headers.origin) || !["127.0.0.1", "localhost", "[::1]"].includes((req.headers.host ?? "").replace(/:\d+$/, ""))) {
			res.writeHead(403).end("Non-loopback MCP origin/host refused");
			return;
		}
		const header = req.headers.authorization;
		const bearer = typeof header === "string" && header.startsWith("Bearer ") ? header.slice(7) : "";
		const supplied = bearer || route.searchParams.get("key") || "";
		if (!sameSecret(supplied, secret)) { res.writeHead(401, { "WWW-Authenticate": "Bearer" }).end("Invalid MCP key"); return; }
		if (req.method !== "POST") { res.writeHead(405, { Allow: "POST" }).end(); return; }
		const mcp = makeMcpServer();
		const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true });
		try {
			await mcp.connect(transport);
			await transport.handleRequest(req, res);
		} catch (error) {
			if (!res.headersSent) res.writeHead(500).end("Eval MCP transport error");
			else if (!res.writableEnded) res.end();
			console.error("[eval-pi-mcp] request failed:", error);
		} finally {
			await mcp.close().catch(() => {});
		}
	});
	try {
		await new Promise<void>((resolve, reject) => {
			http.once("error", reject);
			http.listen(0, "127.0.0.1", () => { http.off("error", reject); resolve(); });
		});
		const address = http.address();
		if (!address || typeof address === "string") throw new Error("Eval MCP failed to get TCP port");
		port = address.port;
	} catch (error) {
		http.close();
		throw error;
	}
	const url = `http://127.0.0.1:${port}/mcp?key=${secret}`;
	const close = async () => {
		if (closed) return;
		closed = true;
		images.clear();
		imageBytes = 0;
		await new Promise<void>((resolve) => {
			http.close(() => resolve());
			http.closeAllConnections();
		});
	};
	try {
		options.notify(`eval-pi-mcp binding ID: ${bindingId}\nMCP URL (secret): ${url}\nSend both the binding ID and URL to the other agent. Pass binding_id on EVERY MCP tool call; start with get_desc. URL and ID expire when the session ends or switches. Tokenizer: ${tokenCounter.ready ? "ready" : tokenCounter.error}.`);
	} catch (error) {
		// No owner can use a binding that never reached the notification surface.
		await close();
		throw error;
	}
	return { bindingId, url, close };
}
