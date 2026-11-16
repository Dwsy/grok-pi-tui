/**
 * MCP facade for the resident mcpeval daemon.
 *
 * This module owns MCP schemas and event/resource tools only. Transport,
 * registration and Eval execution remain in the daemon/client modules.
 */
export type McpevalMcpFacadeDeps = {
  mcp: any;
  protocol: string;
  maxCodeChars: number;
  state: any;
  daemonDescriptor: (state: any) => unknown;
  discover: () => unknown[];
  findRegistration: (bindingId: unknown) => any;
  registrations: Map<string, any>;
  invoke: (registration: any, method: string, params: Record<string, unknown>, signal?: AbortSignal) => Promise<any>;
  tokenCounter: any;
  withDaemonUsage: (...args: any[]) => Promise<any>;
  toMcpEvalResult: (...args: any[]) => Promise<any>;
  subscriptions: Map<string, any>;
  images: Map<string, any>;
  randomUUID: () => string;
  getGlobalQueuedEvents: () => number;
  setGlobalQueuedEvents: (value: number) => void;
};

export function createMcpevalMcpServer(deps: McpevalMcpFacadeDeps): any {
  const {
    mcp, protocol, maxCodeChars, state, daemonDescriptor, discover,
    findRegistration, registrations, invoke, tokenCounter, withDaemonUsage,
    toMcpEvalResult, subscriptions, images, randomUUID,
    getGlobalQueuedEvents, setGlobalQueuedEvents,
  } = deps;

		const server = new mcp.McpServer({ name: "mcpeval", version: "1.0.0" }, { instructions: "Use discover first, then pass binding_id to get_desc/get_context/execute_eval. The daemon survives Pi shutdown; rediscover after reconnect." });
		const bindingSchema = { binding_id: mcp.z.string().uuid() };
		const optionalBindingSchema = { binding_id: mcp.z.string().uuid().optional() };
		server.registerTool("discover", { description: "List registered Pi Eval sessions and their liveness.", inputSchema: {} }, async () => ({ content: [{ type: "text", text: JSON.stringify({ protocol: protocol, daemon: daemonDescriptor(state), registrations: discover() }) }] }));
		server.registerTool("search", { description: "Search registered Pi nested tools without exposing schemas until a description is requested.", inputSchema: { query: mcp.z.string(), ...optionalBindingSchema, limit: mcp.z.number().int().min(1).max(100).optional() } }, async ({ query, binding_id, limit }: any) => {
			const selected = binding_id ? [findRegistration(binding_id)] : [...registrations.values()];
			const rows: any[] = [];
			for (const registration of selected) {
				try { const value = await invoke(registration, "search", { query }); for (const item of Array.isArray(value?.items) ? value.items : []) rows.push({ ...item, binding_id: registration.bindingId }); } catch { /* offline between discovery and search */ }
			}
			return { content: [{ type: "text", text: JSON.stringify(rows.slice(0, limit ?? 20)) }] };
		});
		server.registerTool("get_context", { description: "Read the active Pi branch snapshot for one registered session.", inputSchema: { ...bindingSchema, limit: mcp.z.number().int().min(1).max(200).optional() } }, async ({ binding_id, limit }: any) => {
			const value = await invoke(findRegistration(binding_id), "get_context", { limit: limit ?? 60 });
			return { ...(await withDaemonUsage(tokenCounter, JSON.stringify({ limit: limit ?? 60 }), String(value?.output ?? value))) };
		});
		server.registerTool("get_desc", { description: "Read Eval usage, tools and skills for one registered session.", inputSchema: bindingSchema }, async ({ binding_id }: any) => {
			const value = await invoke(findRegistration(binding_id), "get_desc", {});
			return { ...(await withDaemonUsage(tokenCounter, "get_desc", String(value?.output ?? value))) };
		});
		server.registerTool("execute_eval", { description: "Execute a cell in the selected Pi Eval v2 runtime.", inputSchema: { ...bindingSchema, language: mcp.z.enum(["js", "py"]), code: mcp.z.string().min(1).max(maxCodeChars), title: mcp.z.string().min(1).max(150).optional(), timeout: mcp.z.number().min(0).max(86400).optional(), reset: mcp.z.boolean().optional(), is_background: mcp.z.boolean().optional() } }, async ({ binding_id, ...params }: any, extra: { signal?: AbortSignal }) => toMcpEvalResult(binding_id, await invoke(findRegistration(binding_id), "execute_eval", params, extra?.signal), JSON.stringify(params), tokenCounter));
		server.registerTool("events_subscribe", { description: "Subscribe to bounded in-memory runtime events; poll with events_poll. No history replay or webhook delivery in daemon mode.", inputSchema: { ...bindingSchema, filter: mcp.z.record(mcp.z.string(), mcp.z.unknown()).optional() } }, async ({ binding_id, filter }: any) => {
			findRegistration(binding_id);
			const id = randomUUID();
			subscriptions.set(id, { id, bindingId: binding_id, filter: filter ?? {}, queue: [] });
			return { content: [{ type: "text", text: JSON.stringify({ subscription_id: id, cursor: null, delivery: "poll", boundary: "daemon-local; no webhook/history replay" }) }] };
		});
		server.registerTool("events_poll", { description: "Read and drain queued runtime events for a daemon-local subscription.", inputSchema: { subscription_id: mcp.z.string().uuid(), limit: mcp.z.number().int().min(1).max(64).optional() } }, async ({ subscription_id, limit }: any) => {
			const subscription = subscriptions.get(subscription_id);
			if (!subscription) throw new Error("Unknown or expired mcpeval subscription");
			const take = Math.min(limit ?? 64, subscription.queue.length);
			const events = subscription.queue.splice(0, take);
			setGlobalQueuedEvents(getGlobalQueuedEvents() - events.length);
			return { content: [{ type: "text", text: JSON.stringify({ subscription_id, cursor: null, events }) }] };
		});
		server.registerTool("events_unsubscribe", { description: "Cancel a daemon-local event subscription.", inputSchema: { subscription_id: mcp.z.string().uuid() } }, async ({ subscription_id }: any) => {
			const subscription = subscriptions.get(subscription_id);
			if (subscription) { setGlobalQueuedEvents(getGlobalQueuedEvents() - subscription.queue.length); subscriptions.delete(subscription_id); }
			return { content: [{ type: "text", text: JSON.stringify({ subscription_id, removed: Boolean(subscription) }) }] };
		});
		const eventsList = () => ({ events: [{ name: "runtime.changed", delivery: ["poll"], description: "Pi-native runtime evidence; read current context/output to interpret it. Daemon mode has bounded in-memory poll queues, no history replay or webhook delivery.", inputSchema: { type: "object" }, payloadSchema: { type: "object" } }] });
		server.server.registerCapabilities({ tools: {}, events: {} });
		// The current official SDK only accepts its built-in request schemas in
		// setRequestHandler. Keep the Agenvo-shaped event names as a best-effort
		// extension when a future SDK exposes custom schemas; the portable MCP
		// tools above remain the event transport on today's SDK.
		try {
			server.server.setRequestHandler("events/list", { params: mcp.z.object({ cursor: mcp.z.string().optional() }), result: mcp.z.any() }, () => eventsList());
		} catch { /* custom method schema unsupported by this SDK */ }
		try {
			server.server.setRequestHandler("events/subscribe", { params: mcp.z.any(), result: mcp.z.any() }, async (params: any) => {
				const bindingId = params.arguments?.binding_id ?? params.binding_id;
				findRegistration(bindingId);
				const id = randomUUID();
				subscriptions.set(id, { id, bindingId, filter: params.arguments ?? params, queue: [] });
				return { id, cursor: null, delivery: "poll", boundary: "daemon-local; use events_poll" };
			});
		} catch { /* custom method schema unsupported by this SDK */ }
		try {
			server.server.setRequestHandler("events/unsubscribe", { params: mcp.z.any(), result: mcp.z.any() }, async (params: any) => {
				const input = params.arguments ?? params;
				const id = input.subscription_id;
				const subscription = subscriptions.get(id);
				if (subscription) { setGlobalQueuedEvents(getGlobalQueuedEvents() - subscription.queue.length); subscriptions.delete(id); }
				return {};
			});
		} catch { /* custom method schema unsupported by this SDK */ }
		const template = new mcp.ResourceTemplate("eval-pi-mcp://{binding_id}/image/{image_id}", { list: async () => ({ resources: [...images.values()].map(({ uri, name, mimeType }) => ({ uri, name, mimeType })) }) });
		server.registerResource("Eval image", template, { description: "Images returned by registered Pi Eval sessions.", mimeType: "image/*" }, async (uri: URL) => {
			const image = images.get(uri.href);
			if (!image) throw new Error("mcpeval image resource expired or does not exist");
			return { contents: [{ uri: image.uri, mimeType: image.mimeType, blob: image.blob }] };
		});
		return server;
}
