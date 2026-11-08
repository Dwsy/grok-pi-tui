import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const esbuild = require(join(repoRoot, "pi-main/node_modules/esbuild"));

const originalCwd = process.cwd();
const savedEnv = {
	PI_GROK_BASH: process.env.PI_GROK_BASH,
	PI_GROK_EVAL_VERSION: process.env.PI_GROK_EVAL_VERSION,
	PI_GROK_BASH_MAX_WAIT_MINS: process.env.PI_GROK_BASH_MAX_WAIT_MINS,
	PI_GROK_BUILTIN_TOOLS: process.env.PI_GROK_BUILTIN_TOOLS,
	PI_GROK_EXCLUDE_TOOLS: process.env.PI_GROK_EXCLUDE_TOOLS,
	PI_GROK_BASH_CONTROL_META: process.env.PI_GROK_BASH_CONTROL_META,
	PI_GROK_EVAL_V2_ONLY: process.env.PI_GROK_EVAL_V2_ONLY,
	PI_GROK_EVAL_MCP: process.env.PI_GROK_EVAL_MCP,
	PI_PACKAGE_DIR: process.env.PI_PACKAGE_DIR,
};

async function buildExtension() {
	const tempDir = await mkdtemp(join(tmpdir(), "pi-grok-env-tool-"));
	const outfile = join(tempDir, "pi-grok-bash.mjs");
	process.env.PI_PACKAGE_DIR = join(repoRoot, "pi-main/packages/coding-agent");
	await esbuild.build({
		entryPoints: [join(repoRoot, "extensions/pi-grok-bash/index.ts")],
		bundle: true,
		outfile,
		platform: "node",
		format: "esm",
		target: "node22",
		banner: {
			js: 'import { createRequire as __createRequire } from "node:module"; const require = __createRequire(import.meta.url);',
		},
		nodePaths: [
			join(homedir(), ".pi/agent/npm/node_modules"),
			join(repoRoot, "pi-main/node_modules"),
		],
		logLevel: "silent",
	});
	const loaded = await import(`${pathToFileURL(outfile).href}?v=${Date.now()}`);
	return { register: loaded.default, tempDir };
}

function createHarness(register, cwd, options = {}) {
	process.env.PI_GROK_BASH = options.bashEnabled === false ? "0" : "1";
	process.env.PI_GROK_EVAL_VERSION = "v1";
	process.env.PI_GROK_BASH_MAX_WAIT_MINS = "0";
	delete process.env.PI_GROK_BUILTIN_TOOLS;
	delete process.env.PI_GROK_EXCLUDE_TOOLS;
	delete process.env.PI_GROK_BASH_CONTROL_META;
	delete process.env.PI_GROK_EVAL_V2_ONLY;
	delete process.env.PI_GROK_EVAL_MCP;

	const registeredTools = new Map();
	const handlers = new Map();
	const customEntries = [];
	let branch = options.branch ?? [];
	let leafId = options.leafId ?? "leaf-1";
	let branchGeneration = 0;
	const pi = {
		events: { emit() {} },
		sendMessage() {},
		on(event, handler) {
			const existing = handlers.get(event) ?? [];
			existing.push(handler);
			handlers.set(event, existing);
		},
		registerTool(definition) {
			registeredTools.set(definition.name, definition);
		},
		appendEntry(customType, data) {
			customEntries.push({ customType, data });
			branch = [...branch, {
				id: `entry-${branch.length}`,
				type: "custom",
				customType,
				data,
			}];
			leafId = `leaf-${branch.length + 1}`;
		},
		getActiveTools: () => [],
		setActiveTools() {},
		getAllTools: () => [],
	};
	const sessionManager = {
		getBranch: () => branch,
		getLeafId: () => leafId,
		getSessionId: () => "env-tool-test",
		getSessionFile: () => join(cwd, "session.jsonl"),
	};
	const ctx = { cwd, ui: { setStatus() {} }, sessionManager };

	return {
		registeredTools,
		customEntries,
		setBranch(next) {
			branch = next;
			branchGeneration += 1;
			leafId = `leaf-switch-${branchGeneration}`;
		},
		ctx,
		async init() {
			await register(pi);
		},
		async emit(event, payload, eventCtx = ctx) {
			for (const handler of handlers.get(event) ?? []) await handler(payload, eventCtx);
		},
		async bash(command) {
			const tool = registeredTools.get("bash");
			assert(tool, "bash tool must be registered");
			const result = await tool.execute(
				`env-test-${Math.random().toString(16).slice(2)}`,
				{ command, task_name: "env tool check" },
				undefined,
				undefined,
				ctx,
			);
			return result.content.map((block) => block.text ?? "").join("\n");
		},
		async env(params) {
			const tool = registeredTools.get("bash_env_tool");
			assert(tool, "bash_env_tool must be registered");
			const result = await tool.execute(
				`env-set-${Math.random().toString(16).slice(2)}`,
				params,
				undefined,
				undefined,
				ctx,
			);
			return result.content.map((block) => block.text ?? "").join("\n");
		},
		async close() {
			for (const handler of handlers.get("session_shutdown") ?? []) await handler();
		},
	};
}

function restoreEnv() {
	for (const [key, value] of Object.entries(savedEnv)) {
		if (value === undefined) delete process.env[key];
		else process.env[key] = value;
	}
}

const { register, tempDir: bundleDir } = await buildExtension();
const projectDir = await mkdtemp(join(tmpdir(), "pi-grok-env-project-"));
let harness;

try {
	process.chdir(projectDir);
	await writeFile(
		join(projectDir, ".env"),
		'FOO=bar\nQUOTED="two words"\nexport LEGACY=ok\n# ignored\nINTERP=${FOO}\n',
	);

	harness = createHarness(register, projectDir);
	await harness.init();

	assert(harness.registeredTools.has("bash_env_tool"), "bash_env_tool must be registered when Bash is enabled");
	const dotenvOutput = await harness.bash(
		'printf "%s|%s|%s|%s" "$ENV_FOO" "$ENV_QUOTED" "$ENV_LEGACY" "$ENV_INTERP"',
	);
	assert.match(dotenvOutput, /bar\|two words\|ok\|\$\{FOO\}/);
	console.log("PASS 1 .env k=v loaded via Node util.parseEnv and exposed as ENV_<KEY>");

	await harness.env({ action: "set", scope: "session", key: "ORDER", value: "session-value" });
	await harness.env({ action: "set", scope: "session", key: "KEEP", value: "keep-value" });
	await harness.env({ action: "set", scope: "project", key: "ORDER", value: "project-value" });
	const projectFile = JSON.parse(await readFile(join(projectDir, ".pi/env.json"), "utf8"));
	assert.equal(projectFile.version, 1);
	assert.equal(projectFile.env.ORDER, "project-value");
	assert.match(await harness.bash('printf "%s" "$ORDER"'), /session-value/);
	console.log("PASS 2 project/session precedence: session overrides project");

	const listed = await harness.env({ action: "list" });
	assert(!listed.includes("session-value"), "list must not echo values");
	assert(!listed.includes("project-value"), "list must not echo values");
	assert(JSON.parse(listed).effective.includes("ENV_FOO"));
	assert(JSON.parse(listed).effective.includes("ORDER"));
	console.log("PASS 3 list returns key names only");

	await harness.env({ action: "delete", scope: "session", key: "ORDER" });
	assert.match(await harness.bash('printf "%s" "$ORDER"'), /project-value/);
	console.log("PASS 4 delete falls back to project value");

	for (const reserved of ["PI_SECRET", "GROK_TOKEN", "PATH", "HOME"]) {
		await assert.rejects(
			() => harness.env({ action: "set", scope: "session", key: reserved, value: "x" }),
			/reserved/,
		);
	}
	await assert.rejects(
		() => harness.env({ action: "set", scope: "session", key: "BAD-KEY", value: "x" }),
		/Invalid environment variable name/,
	);
	console.log("PASS 5 reserved and malformed keys are rejected");

	const persistedBranch = harness.customEntries.map(({ customType, data }, index) => ({
		id: `entry-${index}`,
		type: "custom",
		customType,
		data,
	}));
	await harness.close();

	// Simulate /resume: a fresh extension instance replays the same session branch.
	const resumed = createHarness(register, projectDir, { branch: persistedBranch });
	await resumed.init();
	await resumed.emit("session_start", { type: "session_start", reason: "resume" });
	assert.match(await resumed.bash('printf "%s" "$KEEP"'), /keep-value/);
	assert.match(await resumed.bash('printf "%s" "$ORDER"'), /project-value/);
	console.log("PASS 6 /resume replays session env events over the project file");

	// Navigating to a branch without those entries must not leak the previous env.
	resumed.setBranch([]);
	assert.doesNotMatch(await resumed.bash('printf "%s" "$KEEP"'), /keep-value/);
	console.log("PASS 7 branch change does not leak session env");
	await resumed.close();

	const disabled = createHarness(register, projectDir, { bashEnabled: false });
	await disabled.init();
	assert.equal(disabled.registeredTools.has("bash_env_tool"), false);
	assert.equal(disabled.registeredTools.has("bash"), false);
	console.log("PASS 8 PI_GROK_BASH=0 registers neither bash nor bash_env_tool");
	await disabled.close();

	console.log("bash_env_tool focused regression: PASS");
} finally {
	if (harness) await harness.close().catch(() => {});
	process.chdir(originalCwd);
	restoreEnv();
	await rm(projectDir, { recursive: true, force: true });
	await rm(bundleDir, { recursive: true, force: true });
}
