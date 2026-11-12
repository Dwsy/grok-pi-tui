/**
 * Project/session environment variables for grok-pi's managed Bash.
 *
 * Sources are merged in order: Pi's base env < `<cwd>/.env` < `<cwd>/.pi/env.json`
 * < session-scoped incremental events. Session state lives in Pi custom entries
 * so `/resume` and session-tree navigation replay the active branch.
 */
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import * as nodeUtil from "node:util";

import { StringEnum, Type } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export const BASH_ENV_TOOL_NAME = "bash_env_tool";
export const SESSION_ENV_ENTRY_TYPE = "pi-grok-session-env/v1";
export const DOTENV_RELATIVE_PATH = ".env";
export const PROJECT_ENV_RELATIVE_PATH = ".pi/env.json";

const ENV_PREFIX = "ENV_";
const KEY_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/;
const RESERVED_PREFIXES = ["PI_", "GROK_"];
const RESERVED_NAMES = new Set([
	"PATH", "HOME", "SHELL", "PWD", "TMPDIR", "USER", "LOGNAME",
	"SYSTEMROOT", "COMSPEC", "PATHEXT", "WINDIR",
]);

type EnvMap = Record<string, string>;
type EnvScope = "project" | "session";
type BranchEntry = { type?: string; customType?: string; data?: unknown };
type SessionContext = {
	sessionManager?: {
		getBranch?: () => readonly BranchEntry[];
		getLeafId?: () => string | null;
	};
};
type SessionEnvEvent = { version: 1; set?: EnvMap; delete?: string[] };

/** Node's built-in dotenv parser; available since Node 20.12 / 21.7. */
const parseEnvFile = (
	nodeUtil as unknown as { parseEnv?: (content: string) => Record<string, string> }
).parseEnv;

function jsonContent(value: unknown) {
	return [{ type: "text" as const, text: JSON.stringify(value, null, 2) }];
}

function validateKey(key: string): void {
	if (!KEY_PATTERN.test(key)) {
		throw new Error(`Invalid environment variable name ${JSON.stringify(key)}; expected ${KEY_PATTERN}`);
	}
	const upper = key.toUpperCase();
	if (RESERVED_PREFIXES.some((prefix) => upper.startsWith(prefix)) || RESERVED_NAMES.has(upper)) {
		throw new Error(`Environment variable ${JSON.stringify(key)} is reserved by Pi/grok-pi`);
	}
}

function validateValue(value: string): void {
	if (value.includes("\0")) throw new Error("Environment variable value must not contain NUL");
}

function isStringRecord(value: unknown): value is EnvMap {
	return Boolean(value) && typeof value === "object" && !Array.isArray(value)
		&& Object.values(value as Record<string, unknown>).every((item) => typeof item === "string");
}

function projectEnvFile(cwd: string): string {
	return join(cwd, PROJECT_ENV_RELATIVE_PATH);
}

function readProjectEnv(cwd: string): EnvMap {
	const file = projectEnvFile(cwd);
	if (!existsSync(file)) return {};
	let parsed: unknown;
	try {
		parsed = JSON.parse(readFileSync(file, "utf8"));
	} catch (error) {
		throw new Error(`Invalid ${file}: ${String(error)}`);
	}
	const candidate = parsed as { version?: unknown; env?: unknown } | null;
	if (!candidate || candidate.version !== 1 || !isStringRecord(candidate.env)) {
		throw new Error(`Invalid ${file}: expected {"version":1,"env":{"KEY":"value"}}`);
	}
	const env = { ...candidate.env };
	for (const [key, value] of Object.entries(env)) {
		validateKey(key);
		validateValue(value);
	}
	return env;
}

function writeProjectEnv(cwd: string, env: EnvMap): void {
	const file = projectEnvFile(cwd);
	const sorted = Object.fromEntries(Object.entries(env).sort(([a], [b]) => a.localeCompare(b)));
	mkdirSync(dirname(file), { recursive: true });
	writeFileSync(file, `${JSON.stringify({ version: 1, env: sorted }, null, 2)}\n`, { mode: 0o600 });
	try {
		chmodSync(file, 0o600);
	} catch {
		// Windows and restricted filesystems do not enforce POSIX modes.
	}
}

/** `.env` keys are exposed as `ENV_<KEY>` so they cannot shadow Pi/system variables. */
function readDotEnv(cwd: string): EnvMap {
	const file = join(cwd, DOTENV_RELATIVE_PATH);
	if (!existsSync(file)) return {};
	if (!parseEnvFile) {
		throw new Error("Loading .env requires Node.js util.parseEnv (Node >= 20.12)");
	}
	const parsed = parseEnvFile(readFileSync(file, "utf8"));
	const env: EnvMap = {};
	for (const [key, value] of Object.entries(parsed)) {
		if (!KEY_PATTERN.test(key)) {
			throw new Error(`Invalid key in ${file}: ${JSON.stringify(key)}`);
		}
		const envKey = key.toUpperCase().startsWith(ENV_PREFIX) ? key : `${ENV_PREFIX}${key}`;
		validateKey(envKey);
		validateValue(String(value));
		env[envKey] = String(value);
	}
	return env;
}

/** Replay session env events in root-to-leaf order (getBranch() path order). */
export function replaySessionEnv(branch: readonly BranchEntry[]): EnvMap {
	const env: EnvMap = {};
	for (const entry of branch) {
		if (entry.type !== "custom" || entry.customType !== SESSION_ENV_ENTRY_TYPE) continue;
		const data = entry.data as SessionEnvEvent | undefined;
		if (!data || typeof data !== "object" || data.version !== 1) continue;
		for (const key of data.delete ?? []) delete env[key];
		for (const [key, value] of Object.entries(data.set ?? {})) {
			if (typeof value === "string") env[key] = value;
		}
	}
	return env;
}

export type EnvManager = {
	sync(ctx: unknown): void;
	merge(base: NodeJS.ProcessEnv, cwd: string): NodeJS.ProcessEnv;
	set(scope: EnvScope, key: string, value: string, cwd: string): void;
	delete(scope: EnvScope, key: string, cwd: string): void;
	keys(scope: EnvScope, cwd: string): string[];
};

export function createEnvManager(pi: ExtensionAPI): EnvManager {
	let sessionEnv: EnvMap = {};
	let lastLeafId: string | null | undefined;
	let replayed = false;

	const appendSessionEvent = (event: SessionEnvEvent) => {
		pi.appendEntry(SESSION_ENV_ENTRY_TYPE, event);
	};

	return {
		sync(ctx: unknown) {
			const manager = (ctx as SessionContext | null | undefined)?.sessionManager;
			// No session context means startup or a headless test; keep the in-memory state.
			if (!manager?.getBranch) return;
			const leafId = manager.getLeafId?.();
			// Replay on the first call and whenever the active branch changes (/resume, navigateTree,
			// or a new custom entry). The in-memory state already includes our own writes.
			if (replayed && leafId === lastLeafId) return;
			sessionEnv = replaySessionEnv(manager.getBranch());
			lastLeafId = leafId;
			replayed = true;
		},
		merge(base: NodeJS.ProcessEnv, cwd: string) {
			return {
				...base,
				...readDotEnv(cwd),
				...readProjectEnv(cwd),
				...sessionEnv,
			};
		},
		set(scope, key, value, cwd) {
			if (scope === "project") {
				const env = readProjectEnv(cwd);
				env[key] = value;
				writeProjectEnv(cwd, env);
				return;
			}
			sessionEnv[key] = value;
			appendSessionEvent({ version: 1, set: { [key]: value }, delete: [] });
		},
		delete(scope, key, cwd) {
			if (scope === "project") {
				const env = readProjectEnv(cwd);
				delete env[key];
				writeProjectEnv(cwd, env);
				return;
			}
			delete sessionEnv[key];
			appendSessionEvent({ version: 1, set: {}, delete: [key] });
		},
		keys(scope, cwd) {
			if (scope === "project") return Object.keys(readProjectEnv(cwd));
			return Object.keys(sessionEnv);
		},
	};
}

export function createEnvTool(pi: ExtensionAPI) {
	const manager = createEnvManager(pi);
	const definition = {
		name: BASH_ENV_TOOL_NAME,
		label: BASH_ENV_TOOL_NAME,
		description:
			"Manage environment variables for grok-pi Bash. `set`/`delete`/`list` with `scope=project` " +
			"(persists to .pi/env.json, gitignored) or `scope=session` (default; current Pi session, restored by /resume). " +
			"Each Bash spawn merges Pi base env < .env (`k=v` keys exposed as `ENV_<KEY>`) < project < session. " +
			"Only the .env layer is prefixed: keys set with this tool are injected verbatim (set FOO -> $FOO). " +
			"Only enhanced Bash sees these values; values are stored in plain text and never echoed in tool results.",
		promptSnippet:
			"Use before Bash when a variable must survive across calls, or to list the variables available to Bash.",
		promptGuidelines: [
			".env entries are read-only and their keys appear as $ENV_FOO in Bash; bash_env_tool cannot change them. It writes the project/session layers instead, whose keys are injected verbatim ($FOO, no prefix).",
			"scope=session is the default and is replayed on /resume; scope=project writes .pi/env.json for the local project. Session overrides project, which overrides .env. Already-running Bash tasks keep the env snapshot they started with.",
			"Reserved keys (PI_*/GROK_* prefixes and PATH/HOME/SHELL/PWD/TMPDIR/USER/LOGNAME/SystemRoot/COMSPEC/PATHEXT/WINDIR) are rejected. Values are plain text in the project file and session JSONL: do not store long-lived credentials.",
		],
		parameters: Type.Object({
			action: StringEnum(["set", "delete", "list"] as const),
			scope: Type.Optional(
				StringEnum(["project", "session"] as const, { description: "Defaults to session." }),
			),
			key: Type.Optional(Type.String({ minLength: 1, description: "Variable name; required for set/delete." })),
			value: Type.Optional(Type.String({ description: "Variable value; required for set." })),
		}),
		async execute(
			_toolCallId: string,
			params: {
				action: "set" | "delete" | "list";
				scope?: EnvScope;
				key?: string;
				value?: string;
			},
			_signal: AbortSignal | undefined,
			_onUpdate: unknown,
			ctx: unknown,
		) {
			manager.sync(ctx);
			const cwd = (ctx as { cwd?: string } | undefined)?.cwd ?? process.cwd();
			const scope = params.scope ?? "session";
			if (params.action === "list") {
				return {
					content: jsonContent({
						dotenv: Object.keys(readDotEnv(cwd)),
						project: manager.keys("project", cwd),
						session: manager.keys("session", cwd),
						effective: Object.keys(manager.merge({}, cwd)),
					}),
				};
			}
			if (!params.key) throw new Error("bash_env_tool key is required for set/delete");
			validateKey(params.key);
			if (params.action === "set") {
				if (params.value === undefined) throw new Error("bash_env_tool value is required for set");
				validateValue(params.value);
				manager.set(scope, params.key, params.value, cwd);
				return { content: jsonContent({ action: "set", scope, key: params.key, status: "set" }) };
			}
			if (params.action === "delete") {
				manager.delete(scope, params.key, cwd);
				return { content: jsonContent({ action: "delete", scope, key: params.key, status: "deleted" }) };
			}
			throw new Error(`Unsupported bash_env_tool action ${JSON.stringify(params.action)}`);
		},
	};
	return { manager, definition };
}
