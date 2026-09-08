/**
 * `/pi-config web` and `/pi-models web` -- the native Pi config modals as a
 * browser page.
 *
 * grok-pi forwards the `web` argument of its own `/pi-config` and `/pi-models`
 * commands here; this extension owns the server, the page and every write.
 */
import type { Api, Model } from "@earendil-works/pi-ai";
import type {
	ExtensionAPI,
	ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import {
	agentPaths,
	collectState,
	grokConfigPath,
	saveHostUi,
	saveModelsDoc,
	saveSettingsDoc,
	validateModelsDoc,
	validateSettingsDoc,
} from "./config-store.ts";
import { startWebConfigServer, type WebConfigServer } from "./server.ts";
import {
	DEFAULT_HOST,
	UI_HTML_ENV,
	type ModelEntry,
	type ProviderEntry,
} from "./shared.ts";

type WebConfigHost = {
	pi?: ExtensionAPI;
	ctx?: ExtensionCommandContext;
	server?: WebConfigServer;
};

type OpenOptions = {
	host: string;
	port: number;
	open: boolean;
};

const globalRef = globalThis as typeof globalThis & {
	__piGrokWebConfig?: WebConfigHost;
};

/** Survives `ctx.reload()`: Pi re-runs the entry but the module graph is cached. */
function hostState(): WebConfigHost {
	globalRef.__piGrokWebConfig ??= {};
	return globalRef.__piGrokWebConfig;
}

function parseOptions(args: string): OpenOptions {
	const options: OpenOptions = { host: DEFAULT_HOST, port: 0, open: true };
	const tokens = args.trim().split(/\s+/).filter(Boolean);
	for (let i = 0; i < tokens.length; i += 1) {
		const token = tokens[i]!;
		if (token === "--port") {
			const value = Number(tokens[i + 1]);
			if (Number.isInteger(value) && value >= 0 && value <= 65535) options.port = value;
			i += 1;
		} else if (token.startsWith("--port=")) {
			const value = Number(token.slice("--port=".length));
			if (Number.isInteger(value) && value >= 0 && value <= 65535) options.port = value;
		} else if (token === "--host") {
			if (tokens[i + 1]) options.host = tokens[i + 1]!;
			i += 1;
		} else if (token === "--no-open") {
			options.open = false;
		}
	}
	return options;
}

async function openInBrowser(pi: ExtensionAPI, url: string): Promise<void> {
	const args =
		process.platform === "win32"
			? ["/c", "start", "", url]
			: [url];
	const command =
		process.platform === "darwin"
			? "open"
			: process.platform === "win32"
				? "cmd"
				: "xdg-open";
	await pi.exec(command, args, { timeout: 10_000 });
}

function buildModel(
	providerId: string,
	provider: ProviderEntry,
	entry: ModelEntry,
): Model<Api> {
	const contextWindow = entry.contextWindow ?? 200_000;
	return {
		id: entry.id,
		name: entry.name ?? entry.id,
		api: (entry.api ?? provider.api ?? "openai-completions") as Api,
		provider: providerId,
		baseUrl: entry.baseUrl ?? provider.baseUrl ?? "",
		reasoning: entry.reasoning ?? false,
		input: (entry.input ?? ["text"]) as ("text" | "image")[],
		cost: entry.cost ?? { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow,
		maxTokens: entry.maxTokens ?? contextWindow,
		...(entry.headers ? { headers: entry.headers } : {}),
	};
}

export default function piGrokWebConfig(pi: ExtensionAPI): void {
	if (process.env.PI_GROK !== "1") return;
	hostState().pi = pi;

	const open = async (args: string, ctx: ExtensionCommandContext, tab: string): Promise<void> => {
		const host = hostState();
		host.pi = pi;
		host.ctx = ctx;
		const options = parseOptions(args);
		if (host.server) {
			await announce(pi, ctx, host.server, tab, options.open);
			return;
		}

		const uiHtmlPath = process.env[UI_HTML_ENV];
		if (!uiHtmlPath) {
			ctx.ui.notify(
				`/${tab === "models" ? "pi-models" : "pi-config"} web: ${UI_HTML_ENV} is not set; this extension only runs inside grok-pi.`,
				"error",
			);
			return;
		}

		let server: WebConfigServer;
		try {
			server = await startWebConfigServer({
				host: options.host,
				port: options.port,
				uiHtmlPath,
				loadState: () => collectState(hostState().ctx),
				saveModels: (doc) => {
					const invalid = validateModelsDoc(doc);
					if (invalid) throw new Error(invalid);
					saveModelsDoc(agentPaths(hostState().ctx?.cwd ?? process.cwd()).paths.models, doc);
				},
				saveSettings: (doc) => {
					const invalid = validateSettingsDoc(doc);
					if (invalid) throw new Error(invalid);
					saveSettingsDoc(
						agentPaths(hostState().ctx?.cwd ?? process.cwd()).paths.settings,
						doc,
					);
				},
				useModel: async (providerId, modelId) => {
					const { paths } = agentPaths(hostState().ctx?.cwd ?? process.cwd());
					const state = await collectState(hostState().ctx);
					const provider = state.models.providers[providerId];
					const entry = provider?.models?.find((model) => model.id === modelId);
					if (!provider || !entry) {
						throw new Error(`model ${providerId}/${modelId} is not in ${paths.models}`);
					}
					const ok = await pi.setModel(buildModel(providerId, provider, entry));
					if (!ok) throw new Error(`no API key available for ${providerId}`);
				},
				reload: async () => {
					const ctx = hostState().ctx;
					if (!ctx) throw new Error("Pi is not ready; run the command again after startup");
					await ctx.reload();
				},
				saveHostUi: (updates) => {
					saveHostUi(grokConfigPath(), updates as Record<string, unknown>);
				},
				onClose: () => {
					delete hostState().server;
				},
			});
		} catch (error) {
			ctx.ui.notify(
				`web config server failed to start: ${error instanceof Error ? error.message : String(error)}`,
				"error",
			);
			return;
		}

		host.server = server;
		await announce(pi, ctx, server, tab, options.open);
	};

	pi.registerCommand("pi-config-web", {
		description: "Open Pi resource configuration in a browser (web UI)",
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			await open(args, ctx, "resources");
		},
	});

	pi.registerCommand("pi-models-web", {
		description: "Open Pi provider/model configuration in a browser (web UI)",
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			await open(args, ctx, "models");
		},
	});
}

async function announce(
	pi: ExtensionAPI,
	ctx: ExtensionCommandContext,
	server: WebConfigServer,
	tab: string,
	openBrowser: boolean,
): Promise<void> {
	const url = `${server.url}#${tab}`;
	ctx.ui.notify(`Pi web config: ${url}`, "info");
	if (!openBrowser) return;
	try {
		await openInBrowser(pi, url);
	} catch (error) {
		ctx.ui.notify(
			`Open ${url} manually (browser launch failed: ${error instanceof Error ? error.message : String(error)})`,
			"warning",
		);
	}
}
