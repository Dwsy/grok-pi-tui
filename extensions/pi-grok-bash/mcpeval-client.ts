/** Pi-side registration client for the resident mcpeval daemon. */
import { randomUUID } from "node:crypto";
import { createConnection } from "node:net";
import path from "node:path";
import { readFile, unlink } from "node:fs/promises";
import type { EvalParams } from "./eval.ts";
import type { McpevalPiOptions } from "./mcpeval-daemon.ts";
import { ensureMcpevalDaemon } from "./mcpeval-daemon.ts";
import { persistBinding, sessionSnapshot } from "./eval-pi-mcp.ts";
import { MCPEVAL_PROTOCOL, attachJsonLines, writeJsonLine } from "./mcpeval-protocol.ts";
import { isUuid, mcpevalRoot, mcpevalStatePath, type DaemonState } from "./mcpeval-registry.ts";
import type { McpevalEvent } from "./mcpeval-events.ts";

/** Register this Pi's live Eval session with an already-running daemon. */
export async function registerEvalPi(options: McpevalPiOptions): Promise<{ bindingId: string; url: string; close: () => Promise<void>; publish: (event: McpevalEvent) => void }> {
	const root = mcpevalRoot(options.root);
	await ensureMcpevalDaemon(root);
	const statePath = options.daemonStatePath?.trim() || mcpevalStatePath(root);
	const state = JSON.parse(await readFile(statePath, "utf8")) as DaemonState;
	if (state.protocol !== MCPEVAL_PROTOCOL || typeof state.socketPath !== "string" || typeof state.url !== "string") throw new Error("Invalid mcpeval daemon descriptor");
	const socket = createConnection(state.socketPath);
	await new Promise<void>((resolve, reject) => { socket.once("connect", () => resolve()); socket.once("error", reject); });
	let closed = false;
	let bindingId = "";
	let daemonUrl = "";
	const pending = new Map<string, { resolve: (value: unknown) => void; reject: (error: Error) => void; timer: NodeJS.Timeout }>();
	const localControllers = new Map<string, AbortController>();
	const request = (message: Record<string, unknown>): Promise<any> => new Promise((resolve, reject) => {
		const id = String(message.id ?? randomUUID());
		const timer = setTimeout(() => { pending.delete(id); reject(new Error("mcpeval daemon registration request timed out")); }, 300_000);
		pending.set(id, { resolve, reject, timer });
		try { writeJsonLine(socket, { protocol: MCPEVAL_PROTOCOL, ...message, id }); } catch (error) { clearTimeout(timer); pending.delete(id); reject(error instanceof Error ? error : new Error(String(error))); }
	});
	const detach = attachJsonLines(socket, (message) => {
		if (message.type === "registered") {
			bindingId = String(message.bindingId ?? "");
			daemonUrl = String(message.url ?? "");
			const pendingRegistration = pending.get(String(message.id));
			if (pendingRegistration) { clearTimeout(pendingRegistration.timer); pending.delete(String(message.id)); pendingRegistration.resolve(message); }
			return;
		}
		if (message.type === "response" && typeof message.id === "string") {
			const pendingResponse = pending.get(message.id);
			if (!pendingResponse) return;
			clearTimeout(pendingResponse.timer);
			pending.delete(message.id);
			if (message.ok === false) pendingResponse.reject(new Error(typeof message.error === "string" ? message.error : "mcpeval daemon request failed")); else pendingResponse.resolve(message.result);
			return;
		}
		if (message.type === "cancel" && typeof message.id === "string") {
			localControllers.get(message.id)?.abort(new Error("mcpeval request cancelled"));
			return;
		}
		if (message.type !== "invoke" || typeof message.id !== "string") return;
		const id = message.id;
		const controller = new AbortController();
		localControllers.set(id, controller);
		void (async () => {
			try {
				const method = String(message.method ?? "");
				const params = message.params && typeof message.params === "object" ? message.params as Record<string, unknown> : {};
				let result: unknown;
				if (method === "get_context") result = { output: sessionSnapshot(options.context, Number(params.limit ?? 60)) };
				else if (method === "get_desc") result = { output: JSON.stringify({ binding_id: bindingId, eval_description: options.description, eval_guidelines: options.guidelines, languages: options.languages, registered_pi_tools: options.catalog(), skills: options.skills() }) };
				else if (method === "search") {
					const query = String(params.query ?? "").trim().toLowerCase();
					const terms = query ? query.split(/\s+/).filter(Boolean) : [];
					const items = options.catalog().filter((item) => terms.every((term) => `${item.name} ${item.description ?? ""}`.toLowerCase().includes(term))).map((item) => ({ name: item.name, description: item.description ?? "" }));
					result = { items };
				} else if (method === "execute_eval") {
					result = await options.execute(params as EvalParams, controller.signal);
				} else throw new Error(`Unsupported mcpeval Pi method ${JSON.stringify(method)}`);
				writeJsonLine(socket, { protocol: MCPEVAL_PROTOCOL, type: "response", id, ok: true, result });
			} catch (error) {
				writeJsonLine(socket, { protocol: MCPEVAL_PROTOCOL, type: "response", id, ok: false, error: error instanceof Error ? error.message : String(error) });
			} finally { localControllers.delete(id); }
		})();
	}, () => {
		closed = true;
		for (const controller of localControllers.values()) controller.abort(new Error("mcpeval daemon disconnected"));
		for (const item of pending.values()) { clearTimeout(item.timer); item.reject(new Error("mcpeval daemon disconnected")); }
		pending.clear();
	});
	await request({ type: "register", metadata: { pid: process.pid, session_id: options.context.sessionManager.getSessionId(), cwd: options.context.cwd, languages: options.languages } });
	if (!isUuid(bindingId) || !daemonUrl) { detach(); socket.destroy(); throw new Error("mcpeval daemon returned an invalid registration"); }
	const bindingFile = await persistBinding(root, { bindingId, url: daemonUrl }, path.join("bindings", `${bindingId}.json`));
	const publish = (event: McpevalEvent) => { if (!closed) writeJsonLine(socket, { protocol: MCPEVAL_PROTOCOL, type: "event", event }); };
	const dashboardUrl = new URL(daemonUrl);
	dashboardUrl.pathname = "/";
	dashboardUrl.searchParams.delete("binding_id");
	const close = async () => {
		if (closed) return;
		closed = true;
		try { writeJsonLine(socket, { protocol: MCPEVAL_PROTOCOL, type: "unregister" }); } catch { /* already closed */ }
		detach();
		socket.end();
		for (const controller of localControllers.values()) controller.abort(new Error("Pi session closed"));
		for (const item of pending.values()) { clearTimeout(item.timer); item.reject(new Error("Pi session closed")); }
		pending.clear();
		try { const recorded = JSON.parse(await readFile(bindingFile, "utf8")) as { bindingId?: string }; if (recorded.bindingId === bindingId) await unlink(bindingFile); } catch { /* no binding record */ }
	};
	try { options.notify(`mcpeval binding ID: ${bindingId}\nMCP URL (daemon): ${daemonUrl}\nDashboard URL: ${dashboardUrl}\nSend both values to the trusted agent. The daemon is resident; Pi shutdown unregisters this binding only.`); } catch (error) { await close(); throw error; }
	return { bindingId, url: daemonUrl, close, publish };
}
