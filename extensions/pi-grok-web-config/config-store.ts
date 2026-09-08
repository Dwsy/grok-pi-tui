/**
 * Reads and writes Pi's own config files (models.json / settings.json) and
 * reports the resources Pi would load for this project.
 *
 * Everything goes through Pi's own `getAgentDir()` so the web UI edits exactly
 * the files the running Pi session uses -- including a custom
 * `PI_CODING_AGENT_DIR`.
 */
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join } from "node:path";

import {
	DefaultResourceLoader,
	getAgentDir,
} from "@earendil-works/pi-coding-agent";
import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

import {
	emptyModelsDoc,
	isJsonObject,
	HOST_CATALOG_ENV,
	type HostConfigState,
	type HostSettingEntry,
	type JsonObject,
	type ModelsDoc,
	type ProviderAuth,
	type ProviderEntry,
	type ResourceEntry,
	type ResourceLists,
	type WebConfigState,
} from "./shared.ts";

type LoadedFile = { value: unknown; error?: string };

type ModelRuntimeLike = {
	getProviders?: () => Array<{ id: string; name: string }>;
	getProviderAuthStatus?: (id: string) => {
		configured?: boolean;
		source?: string;
		label?: string;
	};
};

type ThemeLike = { name?: string; path?: string; filePath?: string };

/** Strip `//` and `/* *\/` comments outside strings so JSONC configs parse. */
export function stripJsonComments(input: string): string {
	let out = "";
	let inString = false;
	let quoted = false;
	for (let i = 0; i < input.length; i += 1) {
		const char = input[i]!;
		const next = input[i + 1];
		if (inString) {
			out += char;
			if (quoted) {
				quoted = false;
			} else if (char === "\\") {
				quoted = true;
			} else if (char === '"') {
				inString = false;
			}
			continue;
		}
		if (char === '"') {
			inString = true;
			out += char;
			continue;
		}
		if (char === "/" && next === "/") {
			while (i < input.length && input[i] !== "\n") i += 1;
			out += "\n";
			continue;
		}
		if (char === "/" && next === "*") {
			i += 2;
			while (i < input.length && !(input[i] === "*" && input[i + 1] === "/")) i += 1;
			i += 1;
			continue;
		}
		out += char;
	}
	return out;
}

function readJsonFile(path: string): LoadedFile {
	if (!existsSync(path)) return { value: undefined };
	try {
		return { value: JSON.parse(stripJsonComments(readFileSync(path, "utf8"))) };
	} catch (error) {
		return {
			value: undefined,
			error: error instanceof Error ? error.message : String(error),
		};
	}
}

/** Pretty-print + atomic replace; a crash can never leave a half-written file. */
export function writeJsonFile(path: string, value: unknown): void {
	const tmp = `${path}.tmp`;
	writeFileSync(tmp, `${JSON.stringify(value, null, 2)}\n`, "utf8");
	renameSync(tmp, path);
}

export function agentPaths(cwd: string): {
	agentDir: string;
	cwd: string;
	paths: { models: string; settings: string };
} {
	const agentDir = getAgentDir();
	return {
		agentDir,
		cwd,
		paths: {
			models: join(agentDir, "models.json"),
			settings: join(agentDir, "settings.json"),
		},
	};
}

function loadModelsDoc(path: string): { doc: ModelsDoc; error?: string } {
	const file = readJsonFile(path);
	if (file.error) return { doc: emptyModelsDoc(), error: file.error };
	return { doc: normalizeModelsDoc(file.value) };
}

function normalizeModelsDoc(value: unknown): ModelsDoc {
	if (!isJsonObject(value) || !isJsonObject(value.providers)) return emptyModelsDoc();
	const providers: Record<string, ProviderEntry> = {};
	for (const [id, provider] of Object.entries(value.providers)) {
		if (isJsonObject(provider)) providers[id] = provider as ProviderEntry;
	}
	return { providers };
}

export function validateModelsDoc(value: unknown): string | undefined {
	if (!isJsonObject(value)) return "models.json must be a JSON object";
	if (!isJsonObject(value.providers)) return "models.json needs a `providers` object";
	for (const [id, provider] of Object.entries(value.providers)) {
		if (!isJsonObject(provider)) return `provider "${id}" must be an object`;
		if (provider.models !== undefined) {
			if (!Array.isArray(provider.models)) return `provider "${id}": models must be an array`;
			for (const model of provider.models) {
				if (!isJsonObject(model) || typeof model.id !== "string" || model.id.length === 0) {
					return `provider "${id}" has a model without a string id`;
				}
			}
		}
	}
	return undefined;
}

export function validateSettingsDoc(value: unknown): string | undefined {
	if (!isJsonObject(value)) return "settings.json must be a JSON object";
	return undefined;
}

/** Resource paths Pi received on its own command line (read-only here). */
function cliResourcePaths(): Map<string, string> {
	const flags = new Map<string, string>([
		["--extension", "extensions"],
		["--skill", "skills"],
		["--prompt-template", "prompts"],
		["--theme", "themes"],
	]);
	const found = new Map<string, string>();
	const argv = process.argv;
	for (let i = 0; i < argv.length; i += 1) {
		const kind = flags.get(argv[i] ?? "");
		const value = argv[i + 1];
		if (kind && value && !value.startsWith("-")) found.set(value, kind);
	}
	return found;
}

function settingPaths(settings: JsonObject, key: string): string[] {
	const value = settings[key];
	return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === "string") : [];
}

async function listResources(
	cwd: string,
	agentDir: string,
	settings: JsonObject,
): Promise<ResourceLists> {
	const cli = cliResourcePaths();
	const extensions: ResourceEntry[] = [
		...settingPaths(settings, "extensions").map((path) => ({
			name: basename(path),
			path,
			source: "settings" as const,
		})),
		...[...cli.entries()]
			.filter(([, kind]) => kind === "extensions")
			.map(([path]) => ({ name: basename(path), path, source: "cli" as const })),
	];

	const loader = new DefaultResourceLoader({
		cwd,
		agentDir,
		noExtensions: true,
		noContextFiles: true,
	});
	await loader.reload();
	const { skills } = loader.getSkills();
	const { prompts } = loader.getPrompts();
	const { themes } = loader.getThemes();

	return {
		extensions: dedupe(extensions),
		skills: skills.map((skill) => ({
			name: skill.name,
			description: skill.description,
			path: skill.filePath,
			source: cli.get(skill.filePath) === "skills" ? ("cli" as const) : ("discovered" as const),
		})),
		prompts: prompts.map((prompt) => ({
			name: prompt.name,
			description: prompt.description,
			path: prompt.filePath,
			source: "discovered" as const,
		})),
		themes: themes.map((theme) => {
			const entry = theme as unknown as ThemeLike;
			return {
				name: entry.name ?? basename(entry.path ?? entry.filePath ?? "theme"),
				path: entry.path ?? entry.filePath ?? "",
				source: "discovered" as const,
			};
		}),
	};
}

function dedupe(entries: ResourceEntry[]): ResourceEntry[] {
	const seen = new Set<string>();
	return entries.filter((entry) => {
		if (seen.has(entry.path)) return false;
		seen.add(entry.path);
		return true;
	});
}

export async function collectState(
	ctx: ExtensionCommandContext | undefined,
): Promise<WebConfigState> {
	const cwd = ctx?.cwd ?? process.cwd();
	const { agentDir, paths } = agentPaths(cwd);
	const models = loadModelsDoc(paths.models);
	const settingsFile = readJsonFile(paths.settings);
	const settings = isJsonObject(settingsFile.value) ? settingsFile.value : {};

	const runtime = (ctx?.modelRegistry as { runtime?: ModelRuntimeLike } | undefined)?.runtime;
	const providerAuth: Record<string, ProviderAuth> = {};
	for (const provider of runtime?.getProviders?.() ?? []) {
		const status = runtime?.getProviderAuthStatus?.(provider.id);
		providerAuth[provider.id] = {
			configured: status?.configured === true,
			source: status?.source,
			label: status?.label,
		};
	}

	let resources: ResourceLists = {
		extensions: [],
		skills: [],
		prompts: [],
		themes: [],
	};
	try {
		resources = await listResources(cwd, agentDir, settings);
	} catch (error) {
		resources.error = error instanceof Error ? error.message : String(error);
	}

	return {
		agentDir,
		cwd,
		paths,
		models: models.doc,
		modelsError: models.error,
		settings,
		settingsError: settingsFile.error,
		current: ctx?.model
			? {
					provider: ctx.model.provider,
					modelId: ctx.model.id,
					name: ctx.model.name,
				}
			: null,
		defaults: {
			provider: typeof settings.defaultProvider === "string" ? settings.defaultProvider : undefined,
			modelId: typeof settings.defaultModel === "string" ? settings.defaultModel : undefined,
			thinkingLevel:
				typeof settings.defaultThinkingLevel === "string" ? settings.defaultThinkingLevel : undefined,
		},
		providerAuth,
		resources,
		host: collectHostState(),
	};
}

export function saveModelsDoc(path: string, doc: unknown): void {
	writeJsonFile(path, doc);
}

export function saveSettingsDoc(path: string, doc: unknown): void {
	writeJsonFile(path, doc);
}

// ── grok-pi F2 settings (`$GROK_HOME/config.toml` `[ui]` table) ──────────────

export function grokConfigPath(): string {
	const grokHome = process.env.GROK_HOME || join(homedir(), ".grok-pi");
	return join(grokHome, "config.toml");
}

type UiEntry = { key: string; value: unknown; start: number; end: number };

function bracketsBalanced(text: string): boolean {
	let depth = 0;
	let inString = false;
	let quoted = false;
	for (const char of text) {
		if (inString) {
			if (quoted) quoted = false;
			else if (char === "\\") quoted = true;
			else if (char === '"') inString = false;
			continue;
		}
		if (char === '"') inString = true;
		else if (char === "[") depth += 1;
		else if (char === "]") depth -= 1;
	}
	return depth <= 0 && !inString;
}

function parseTomlValue(raw: string): unknown {
	const text = raw.trim();
	if (text === "true") return true;
	if (text === "false") return false;
	if (text.length >= 2 && text.startsWith("'") && text.endsWith("'")) {
		return text.slice(1, -1);
	}
	try {
		return JSON.parse(text);
	} catch {
		return text;
	}
}

export function serializeTomlValue(value: unknown): string {
	if (typeof value === "boolean" || typeof value === "number") return String(value);
	return JSON.stringify(value);
}

function parseUiTable(text: string): {
	ui: Record<string, unknown>;
	uiTables: Record<string, Record<string, unknown>>;
	entries: UiEntry[];
	uiHeaderLine: number;
	sectionEndLine: number;
} {
	const lines = text.split("\n");
	const ui: Record<string, unknown> = {};
	const uiTables: Record<string, Record<string, unknown>> = {};
	const entries: UiEntry[] = [];
	let section = "";
	let uiHeaderLine = -1;
	let sectionEndLine = lines.length;
	for (let i = 0; i < lines.length; i += 1) {
		const line = lines[i]!;
		const header = line.match(/^\s*\[\[?([^\]]+)\]\]?/);
		if (header) {
			if (section === "ui" && uiHeaderLine >= 0 && sectionEndLine === lines.length) {
				sectionEndLine = i;
			}
			section = header[1]!.trim();
			if (section === "ui" && uiHeaderLine < 0) uiHeaderLine = i;
			continue;
		}
		if (section !== "ui" && !section.startsWith("ui.")) continue;
		const pair = line.match(/^\s*([A-Za-z0-9_.-]+)\s*=\s*(.*)$/);
		if (!pair) continue;
		let valueText = pair[2]!;
		let end = i;
		while (!bracketsBalanced(valueText) && end + 1 < lines.length) {
			end += 1;
			valueText += `\n${lines[end]}`;
		}
		if (section === "ui") {
			ui[pair[1]!] = parseTomlValue(valueText);
			entries.push({ key: pair[1]!, value: ui[pair[1]!], start: i, end });
		} else {
			const sub = section.slice("ui.".length);
			(uiTables[sub] ??= {})[pair[1]!] = parseTomlValue(valueText);
		}
		i = end;
	}
	return { ui, uiTables, entries, uiHeaderLine, sectionEndLine };
}

/**
 * Rewrite only the `[ui]` scalar keys; every other byte of config.toml is
 * preserved so host-owned sections ([voice], [[...]], comments) survive.
 */
export function updateUiTable(text: string, updates: Record<string, unknown>): string {
	const { entries, uiHeaderLine, sectionEndLine } = parseUiTable(text);
	const lines = text.split("\n");
	const applied = new Set<string>();
	const out: string[] = [];
	for (let i = 0; i < lines.length; i += 1) {
		const entry = entries.find((candidate) => candidate.start === i);
		if (entry && entry.key in updates) {
			out.push(`${entry.key} = ${serializeTomlValue(updates[entry.key])}`);
			applied.add(entry.key);
			i = entry.end;
			continue;
		}
		out.push(lines[i]!);
	}
	const pending = Object.keys(updates).filter((key) => !applied.has(key));
	if (pending.length > 0) {
		const insertAt = uiHeaderLine >= 0 ? sectionEndLine : out.length;
		const block = pending.map((key) => `${key} = ${serializeTomlValue(updates[key])}`);
		if (uiHeaderLine >= 0) {
			out.splice(insertAt, 0, ...block);
		} else {
			if (out.length > 0 && out.at(-1) !== "") out.push("");
			out.push("[ui]", ...block);
		}
	}
	return `${out.join("\n").replace(/\n*$/, "\n")}`;
}

function loadHostCatalog(catalogPath: string | undefined): {
	catalog: HostSettingEntry[];
	error?: string;
} {
	if (!catalogPath) return { catalog: [] };
	if (!existsSync(catalogPath)) return { catalog: [] };
	try {
		const parsed = JSON.parse(readFileSync(catalogPath, "utf8"));
		const catalog: HostSettingEntry[] = [];
		for (const item of Array.isArray(parsed) ? parsed : []) {
			const source = typeof item?.source === "string" ? item.source : "";
			const manifest = item?.manifest;
			if (!isJsonObject(manifest) || !Array.isArray(manifest.settings)) continue;
			for (const setting of manifest.settings) {
				if (!isJsonObject(setting) || typeof setting.key !== "string") continue;
				const f2 = isJsonObject(setting.f2) ? setting.f2 : {};
				catalog.push({
					key: setting.key,
					label: typeof setting.label === "string" ? setting.label : undefined,
					description: typeof setting.description === "string" ? setting.description : undefined,
					kind: typeof setting.kind === "string" ? setting.kind : undefined,
					default: setting.default,
					restartRequired: setting.restartRequired === true,
					category: typeof f2.category === "string" ? f2.category : undefined,
					section: typeof f2.section === "string" ? f2.section : undefined,
					order: typeof f2.order === "number" ? f2.order : undefined,
					source: source || undefined,
				});
			}
		}
		catalog.sort((a, b) => {
			const section = (a.section ?? "").localeCompare(b.section ?? "");
			if (section !== 0) return section;
			return (a.order ?? 0) - (b.order ?? 0) || a.key.localeCompare(b.key);
		});
		return { catalog };
	} catch (error) {
		return {
			catalog: [],
			error: error instanceof Error ? error.message : String(error),
		};
	}
}

export function collectHostState(): HostConfigState {
	const configPath = grokConfigPath();
	const loaded = loadHostCatalog(process.env[HOST_CATALOG_ENV]);
	let ui: Record<string, unknown> = {};
	let uiTables: Record<string, Record<string, unknown>> = {};
	let error = loaded.error;
	if (existsSync(configPath)) {
		try {
			const parsed = parseUiTable(readFileSync(configPath, "utf8"));
			ui = parsed.ui;
			uiTables = parsed.uiTables;
		} catch (readError) {
			error = readError instanceof Error ? readError.message : String(readError);
		}
	}
	return {
		grokHome: process.env.GROK_HOME || join(homedir(), ".grok-pi"),
		configPath,
		ui,
		uiTables,
		catalog: loaded.catalog,
		error,
	};
}

/** Apply a validated `[ui]` update; values must be TOML scalars. */
export function saveHostUi(configPath: string, updates: JsonObject): void {
	for (const [key, value] of Object.entries(updates)) {
		if (!/^[A-Za-z0-9_-]+$/.test(key)) throw new Error(`invalid setting key "${key}"`);
		if (typeof value !== "boolean" && typeof value !== "number" && typeof value !== "string") {
			throw new Error(`setting "${key}" must be a boolean, number or string`);
		}
	}
	const text = existsSync(configPath) ? readFileSync(configPath, "utf8") : "";
	writeFileSync(configPath, updateUiTable(text, updates));
}
