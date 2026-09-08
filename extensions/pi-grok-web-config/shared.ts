/**
 * Shared contracts for the grok-pi web config surface.
 *
 * The whole feature is a Pi extension: `/pi-config web` and `/pi-models web`
 * start a loopback HTTP server on a random port and hand the user a browser
 * URL. No Rust code renders or edits config -- Rust only forwards the `web`
 * argument and injects this bundle.
 */

/** Env var set by the grok-pi injector: absolute path of the injected UI. */
export const UI_HTML_ENV = "PI_GROK_WEB_CONFIG_UI";

/** Env var set by the grok-pi injector: baked grok-pi.json settings catalog. */
export const HOST_CATALOG_ENV = "PI_GROK_WEB_CONFIG_CATALOG";

export const DEFAULT_HOST = "127.0.0.1";

export type JsonObject = Record<string, unknown>;

export type ModelCost = {
	input: number;
	output: number;
	cacheRead: number;
	cacheWrite: number;
};

export type ModelEntry = {
	id: string;
	name?: string;
	api?: string;
	baseUrl?: string;
	reasoning?: boolean;
	contextWindow?: number;
	maxTokens?: number;
	input?: string[];
	cost?: ModelCost;
	headers?: Record<string, string>;
	compat?: JsonObject;
};

export type ProviderEntry = {
	name?: string;
	baseUrl?: string;
	apiKey?: string;
	api?: string;
	headers?: Record<string, string>;
	authHeader?: boolean;
	compat?: JsonObject;
	models?: ModelEntry[];
	modelOverrides?: Record<string, JsonObject>;
};

export type ModelsDoc = {
	providers: Record<string, ProviderEntry>;
};

export type ResourceEntry = {
	name: string;
	description?: string;
	path: string;
	/** `settings` entries are editable path lists in settings.json. */
	source: "settings" | "cli" | "discovered";
};

export type ResourceLists = {
	extensions: ResourceEntry[];
	skills: ResourceEntry[];
	prompts: ResourceEntry[];
	themes: ResourceEntry[];
	error?: string;
};

export type ProviderAuth = {
	configured: boolean;
	source?: string;
	label?: string;
};

/**
 * One F2 setting row. Entries sourced from a `grok-pi.json` manifest carry the
 * full F2 metadata; bare `[ui]` keys fall back to a generic editor.
 */
export type HostSettingEntry = {
	key: string;
	label?: string;
	description?: string;
	kind?: string;
	default?: unknown;
	restartRequired?: boolean;
	category?: string;
	section?: string;
	order?: number;
	/** Extension source path (`extensions/<name>/grok-pi.json`) when registered. */
	source?: string;
};

export type HostConfigState = {
	grokHome: string;
	configPath: string;
	/** Current `[ui]` scalar values; absent keys resolve to catalog defaults. */
	ui: Record<string, unknown>;
	/** Read-only `[ui.<sub>]` tables (nested TOML tables are not edited here). */
	uiTables: Record<string, Record<string, unknown>>;
	/** Metadata for registered settings, in F2 section/order. */
	catalog: HostSettingEntry[];
	error?: string;
};

export type WebConfigState = {
	agentDir: string;
	cwd: string;
	paths: { models: string; settings: string };
	models: ModelsDoc;
	modelsError?: string;
	settings: JsonObject;
	settingsError?: string;
	current: { provider?: string; modelId?: string; name?: string } | null;
	defaults: { provider?: string; modelId?: string; thinkingLevel?: string };
	providerAuth: Record<string, ProviderAuth>;
	resources: ResourceLists;
	host: HostConfigState;
};

export function emptyModelsDoc(): ModelsDoc {
	return { providers: {} };
}

export function isJsonObject(value: unknown): value is JsonObject {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}
