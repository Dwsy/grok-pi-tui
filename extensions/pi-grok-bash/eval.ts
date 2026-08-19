import { type ChildProcess, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";

import type { ExtensionCompletionOptions } from "@earendil-works/pi-coding-agent";

import { MAX_OUTPUT_BYTES, MAX_TIMEOUT_SECONDS, killChildProcess } from "./shared.ts";
import type { EvalToolMetadata } from "./tool-bridge.ts";

export type EvalSkillMetadata = {
	name: string;
	description: string;
	filePath: string;
	source?: unknown;
};

export type EvalLanguage = "py" | "js";
export type EvalVersion = "v1" | "v2";

export type EvalParams = {
	language: EvalLanguage;
	code: string;
	title?: string;
	timeout?: number;
	reset?: boolean;
	is_background?: boolean;
};

export type EvalExecution = {
	output: string;
	truncated: boolean;
};

type EvalWorkerReply = {
	id: string;
	ok: boolean;
	value?: string;
	error?: string;
};

type EvalV2ToolHostCall = {
	type: "host_call";
	id: string;
	evalId: string;
	method: "tool";
	tool: string;
	args: Record<string, unknown>;
};

type EvalV2CompletionHostCall = {
	type: "host_call";
	id: string;
	evalId: string;
	method: "completion";
	prompt: string;
	options: ExtensionCompletionOptions;
};

type EvalV2SkillHostCall = {
	type: "host_call";
	id: string;
	evalId: string;
	method: "skill";
	operation: "read";
	name: string;
};

type EvalV2HostCall = EvalV2ToolHostCall | EvalV2CompletionHostCall | EvalV2SkillHostCall;

type EvalV2Result = EvalWorkerReply & { type: "eval_result" };
type EvalV2WorkerMessage = EvalV2HostCall | EvalV2Result;

export type EvalHostCallHandler = (call: EvalV2HostCall, signal: AbortSignal) => Promise<unknown>;

type PendingEval = {
	id: string;
	resolve: (result: EvalExecution) => void;
	reject: (error: Error) => void;
	output: Buffer;
	truncated: boolean;
	timer?: ReturnType<typeof setTimeout>;
	outerSignal?: AbortSignal;
	abortHandler?: () => void;
	runController: AbortController;
};

export type HostCallExecutionMode = "parallel" | "sequential";

type HostCallGateJob = {
	mode: HostCallExecutionMode;
	signal: AbortSignal;
	work: () => Promise<unknown>;
	resolve: (value: unknown) => void;
	reject: (error: unknown) => void;
	started: boolean;
	aborted: boolean;
	abortHandler?: () => void;
};

function evalAbortError(signal: AbortSignal) {
	const reason = signal.reason;
	if (reason instanceof Error) return reason;
	return new Error(reason === undefined ? "Eval host call aborted" : String(reason));
}

export class HostCallGate {
	private readonly queue: HostCallGateJob[] = [];
	private parallelRunning = 0;
	private sequentialRunning = false;

	constructor(private readonly parallelLimit: number) {
		if (!Number.isInteger(parallelLimit) || parallelLimit <= 0) {
			throw new Error("Eval HostCallGate parallel limit must be a positive integer");
		}
	}

	run(mode: HostCallExecutionMode, signal: AbortSignal, work: () => Promise<unknown>): Promise<unknown> {
		if (signal.aborted) return Promise.reject(evalAbortError(signal));
		return new Promise((resolve, reject) => {
			const job: HostCallGateJob = {
				mode,
				signal,
				work,
				resolve,
				reject,
				started: false,
				aborted: false,
			};
			job.abortHandler = () => {
				if (job.started || job.aborted) return;
				job.aborted = true;
				reject(evalAbortError(signal));
				this.pump();
			};
			signal.addEventListener("abort", job.abortHandler, { once: true });
			this.queue.push(job);
			this.pump();
		});
	}

	private pump() {
		if (this.sequentialRunning) return;
		while (this.queue.length > 0) {
			while (this.queue[0]?.aborted) this.queue.shift();
			const next = this.queue[0];
			if (!next) return;
			if (next.mode === "sequential") {
				if (this.parallelRunning > 0) return;
				this.queue.shift();
				this.sequentialRunning = true;
				this.start(next);
				return;
			}
			if (this.parallelRunning >= this.parallelLimit) return;
			this.queue.shift();
			this.parallelRunning += 1;
			this.start(next);
		}
	}

	private start(job: HostCallGateJob) {
		job.started = true;
		if (job.abortHandler) job.signal.removeEventListener("abort", job.abortHandler);

		let runningAbortHandler: (() => void) | undefined;
		const aborted = new Promise<never>((_resolve, reject) => {
			if (job.signal.aborted) {
				reject(evalAbortError(job.signal));
				return;
			}
			runningAbortHandler = () => reject(evalAbortError(job.signal));
			job.signal.addEventListener("abort", runningAbortHandler, { once: true });
		});
		const work = Promise.resolve().then(() => {
			if (job.signal.aborted) throw evalAbortError(job.signal);
			return job.work();
		});

		// Cancellation settles the caller immediately, but it must not release the
		// gate slot until the underlying host work has actually stopped. A host
		// implementation may observe AbortSignal asynchronously (or ignore it), and
		// releasing here on the abort race would let later work exceed the parallel
		// cap or cross a sequential barrier while the cancelled call is still live.
		void Promise.race([work, aborted]).then(job.resolve, job.reject);
		const release = () => {
			if (runningAbortHandler) job.signal.removeEventListener("abort", runningAbortHandler);
			if (job.mode === "parallel") this.parallelRunning -= 1;
			else this.sequentialRunning = false;
			this.pump();
		};
		void work.then(release, release);
	}
}

const JS_EVAL_WORKER_V1 = String.raw`
const fs = require("node:fs");
const repl = require("node:repl");
const readline = require("node:readline");
const util = require("node:util");
const { PassThrough } = require("node:stream");

const protocol = fs.createWriteStream(null, { fd: 3, autoClose: false });
const input = new PassThrough();
const server = repl.start({
  prompt: "",
  input,
  output: process.stdout,
  terminal: false,
  useGlobal: false,
  ignoreUndefined: true,
});
server.context.display = value => {
  console.log(util.inspect(value, { depth: 8, colors: false, maxArrayLength: 200 }));
};

function reply(message) {
  protocol.write(JSON.stringify(message) + "\n");
}

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", line => {
  let request;
  try {
    request = JSON.parse(line);
  } catch (error) {
    reply({ id: "", ok: false, error: String(error) });
    return;
  }
  server.eval(request.code, server.context, "eval", (error, value) => {
    if (error) {
      reply({ id: request.id, ok: false, error: error.stack || error.message || String(error) });
      return;
    }
    reply({
      id: request.id,
      ok: true,
      value: value === undefined
        ? ""
        : util.inspect(value, { depth: 8, colors: false, maxArrayLength: 200 }),
    });
  });
});
`;

const PYTHON_EVAL_WORKER_V1 = String.raw`
import ast
import asyncio
import inspect
import json
import os
import sys
import traceback

protocol = os.fdopen(3, "w", buffering=1)
namespace = {"__name__": "__eval__"}

def display(value):
    print(repr(value))

namespace["display"] = display

async def evaluate(code):
    tree = ast.parse(code, filename="<eval>", mode="exec")
    last_expression = None
    if tree.body and isinstance(tree.body[-1], ast.Expr):
        last_expression = tree.body.pop()
    if tree.body:
        ast.fix_missing_locations(tree)
        compiled = compile(tree, "<eval>", "exec", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
        result = eval(compiled, namespace)
        if inspect.isawaitable(result):
            await result
    if last_expression is None:
        return None
    expression = ast.Expression(last_expression.value)
    ast.fix_missing_locations(expression)
    compiled = compile(expression, "<eval>", "eval", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
    result = eval(compiled, namespace)
    if inspect.isawaitable(result):
        result = await result
    return result

async def main():
    loop = asyncio.get_running_loop()
    while True:
        line = await loop.run_in_executor(None, sys.stdin.readline)
        if not line:
            return
        try:
            request = json.loads(line)
            value = await evaluate(request["code"])
            protocol.write(json.dumps({
                "id": request["id"],
                "ok": True,
                "value": "" if value is None else repr(value),
            }, ensure_ascii=False) + "\n")
        except BaseException:
            request_id = request.get("id", "") if isinstance(request, dict) else ""
            protocol.write(json.dumps({
                "id": request_id,
                "ok": False,
                "error": traceback.format_exc(),
            }, ensure_ascii=False) + "\n")

asyncio.run(main())
`;

const JS_EVAL_WORKER_V2 = String.raw`
const fs = require("node:fs");
const repl = require("node:repl");
const readline = require("node:readline");
const util = require("node:util");
const crypto = require("node:crypto");
const { PassThrough } = require("node:stream");

const protocol = fs.createWriteStream(null, { fd: 3, autoClose: false });
const input = new PassThrough();
const pendingHost = new Map();
const storedValues = new Map();
let currentEvalId = "";

function reply(message) {
  protocol.write(JSON.stringify(message) + "\n");
}

function hostCall(method, payload) {
  if (!currentEvalId) return Promise.reject(new Error("host calls require an active eval cell"));
  const id = crypto.randomUUID();
  const promise = new Promise((resolve, reject) => pendingHost.set(id, { resolve, reject }));
  reply({ type: "host_call", id, evalId: currentEvalId, method, ...payload });
  return promise;
}

const server = repl.start({
  prompt: "",
  input,
  output: process.stdout,
  terminal: false,
  useGlobal: false,
  ignoreUndefined: true,
});
let activeToolCatalog = new Map();
let activeSkillCatalog = new Map();
const toolFunctionCache = new Map();
function toolMetadata(name) {
  return activeToolCatalog.get(String(name));
}
function toolFunction(name) {
  const key = String(name);
  let fn = toolFunctionCache.get(key);
  if (fn) return fn;
  fn = async (args = {}) => {
    if (!args || typeof args !== "object" || Array.isArray(args)) {
      throw new TypeError("tool.<name>(args) expects a JSON object");
    }
    return hostCall("tool", { tool: key, args });
  };
  Object.defineProperties(fn, {
    meta: { enumerable: true, get: () => toolMetadata(key) },
    schema: { enumerable: true, get: () => toolMetadata(key)?.schema },
    description: { enumerable: true, get: () => toolMetadata(key)?.description },
  });
  toolFunctionCache.set(key, fn);
  return fn;
}

function cloneStoredValue(value, key) {
  let serialized;
  try {
    serialized = JSON.stringify(value);
  } catch (error) {
    throw new TypeError("Unable to store " + JSON.stringify(key) + ": " + (error && error.message || String(error)));
  }
  if (serialized === undefined) {
    throw new TypeError("Unable to store " + JSON.stringify(key) + ". Only JSON-serializable values can be stored.");
  }
  return JSON.parse(serialized);
}

function installContextGlobals() {
  const context = server.context;
  context.display = value => {
    console.log(util.inspect(value, { depth: 8, colors: false, maxArrayLength: 200 }));
  };
  context.output = context.display;
  context.tool = new Proxy({}, {
    get(_target, name) {
      if (name === "then") return undefined;
      if (typeof name !== "string") return undefined;
      return toolFunction(name);
    },
    ownKeys() {
      return [...activeToolCatalog.keys()];
    },
    getOwnPropertyDescriptor(_target, name) {
      if (typeof name !== "string" || !activeToolCatalog.has(name)) return undefined;
      return { configurable: true, enumerable: true, writable: false, value: toolFunction(name) };
    },
    has(_target, name) {
      return typeof name === "string" && activeToolCatalog.has(name);
    },
  });
  context.tools = Object.freeze({
    list() {
      return [...activeToolCatalog.values()];
    },
    describe(name) {
      if (typeof name !== "string" || !name.trim()) throw new TypeError("tools.describe(name) requires a non-empty tool name");
      return activeToolCatalog.get(name) ?? null;
    },
    search(query) {
      if (typeof query !== "string") throw new TypeError("tools.search(query) requires a string");
      const needle = query.trim().toLowerCase();
      if (!needle) return [...activeToolCatalog.values()];
      return [...activeToolCatalog.values()].filter(item =>
        item.name.toLowerCase().includes(needle) || String(item.description || "").toLowerCase().includes(needle)
      );
    },
  });
  context.skills = Object.freeze({
    list() {
      return [...activeSkillCatalog.values()];
    },
    describe(name) {
      if (typeof name !== "string" || !name.trim()) throw new TypeError("skills.describe(name) requires a non-empty skill name");
      return activeSkillCatalog.get(name) ?? null;
    },
    search(query) {
      if (typeof query !== "string") throw new TypeError("skills.search(query) requires a string");
      const needle = query.trim().toLowerCase();
      if (!needle) return [...activeSkillCatalog.values()];
      return [...activeSkillCatalog.values()].filter(item =>
        item.name.toLowerCase().includes(needle) || String(item.description || "").toLowerCase().includes(needle)
      );
    },
    async read(name) {
      if (typeof name !== "string" || !name.trim()) throw new TypeError("skills.read(name) requires a non-empty skill name");
      if (!activeSkillCatalog.has(name)) throw new Error("Skill " + JSON.stringify(name) + " is not available in this session");
      return hostCall("skill", { operation: "read", name });
    },
  });
  context.parallel = async thunks => {
    if (!Array.isArray(thunks)) throw new TypeError("parallel(thunks) expects an array");
    return Promise.all(thunks.map((thunk, index) => {
      if (typeof thunk !== "function") throw new TypeError("parallel item " + index + " is not a function");
      return thunk();
    }));
  };
  context.pipeline = async (items, ...stages) => {
    let current = Array.from(items ?? []);
    for (const stage of stages) {
      if (typeof stage !== "function") throw new TypeError("pipeline stages must be functions");
      current = await Promise.all(current.map(item => stage(item)));
    }
    return current;
  };
  context.completion = async (prompt, options = {}) => {
    if (typeof prompt !== "string" || !prompt.trim()) throw new TypeError("completion(prompt) requires a non-empty prompt");
    if (!options || typeof options !== "object" || Array.isArray(options)) throw new TypeError("completion options must be an object");
    return hostCall("completion", { prompt, options });
  };
  context.agent = async (prompt, options = {}) => {
    if (typeof prompt !== "string" || !prompt.trim()) throw new TypeError("agent(prompt) requires a non-empty prompt");
    if (!options || typeof options !== "object" || Array.isArray(options)) throw new TypeError("agent options must be an object");
    const args = {
      prompt,
      description: options.description || options.label || "Eval v2 subagent",
    };
    for (const key of ["subagent_type", "model", "max_turns", "capability_mode", "background"]) {
      if (Object.prototype.hasOwnProperty.call(options, key)) args[key] = options[key];
    }
    return hostCall("tool", { tool: "spawn_subagent", args });
  };
  context.store = (key, value) => {
    const name = String(key);
    const stored = cloneStoredValue(value, name);
    storedValues.set(name, stored);
    return value;
  };
  context.load = key => {
    const name = String(key);
    if (!storedValues.has(name)) return undefined;
    return cloneStoredValue(storedValues.get(name), name);
  };
}

installContextGlobals();

function finishEval(id, message) {
  if (currentEvalId !== id) return false;
  currentEvalId = "";
  pendingHost.clear();
  reply({ type: "eval_result", id, ...message });
  return true;
}

// Node's REPL callback is not invoked when a top-level await rejects; the
// rejection is routed through the REPL's domain instead. Convert that path
// back into the Eval protocol so a failed host call cannot hang until timeout.
if (server._domain && typeof server._domain.on === "function") {
  server._domain.on("error", error => {
    const id = currentEvalId;
    if (!id) return;
    finishEval(id, { ok: false, error: error && (error.stack || error.message) || String(error) });
  });
}

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", line => {
  let message;
  try {
    message = JSON.parse(line);
  } catch (error) {
    reply({ type: "eval_result", id: "", ok: false, error: String(error) });
    return;
  }

  if (message.type === "host_result") {
    const pending = pendingHost.get(message.id);
    if (!pending) return;
    pendingHost.delete(message.id);
    if (message.ok) pending.resolve(message.value);
    else pending.reject(new Error(message.error || "host call failed"));
    return;
  }

  if (message.type !== "eval") {
    reply({ type: "eval_result", id: message.id || "", ok: false, error: "unsupported eval v2 message" });
    return;
  }
  if (currentEvalId) {
    reply({ type: "eval_result", id: message.id, ok: false, error: "eval v2 kernel is already executing a cell" });
    return;
  }

  activeToolCatalog = new Map(
    (Array.isArray(message.tools) ? message.tools : [])
      .filter(item => item && typeof item === "object" && typeof item.name === "string")
      .map(item => [item.name, item]),
  );
  activeSkillCatalog = new Map(
    (Array.isArray(message.skills) ? message.skills : [])
      .filter(item => item && typeof item === "object" && typeof item.name === "string")
      .map(item => [item.name, item]),
  );
  // Match Codex code-mode semantics: lexical bindings belong to one cell.
  // Persistent cross-cell state is explicit and JSON-serializable via store/load.
  server.resetContext();
  installContextGlobals();
  currentEvalId = message.id;
  server.eval(message.code, server.context, "eval", (error, value) => {
    const id = message.id;
    if (error) {
      finishEval(id, { ok: false, error: error.stack || error.message || String(error) });
      return;
    }
    finishEval(id, {
      ok: true,
      value: value === undefined
        ? ""
        : util.inspect(value, { depth: 8, colors: false, maxArrayLength: 200 }),
    });
  });
});
`;

export function resolveEvalVersion(): EvalVersion {
	const raw = (process.env.PI_GROK_EVAL_VERSION ?? "v1").trim().toLowerCase();
	if (raw === "v1" || raw === "v2") return raw;
	throw new Error(`Invalid PI_GROK_EVAL_VERSION=${JSON.stringify(raw)}; expected "v1" or "v2"`);
}

function validateEvalTimeout(timeout: number) {
	if (
		!Number.isFinite(timeout) ||
		timeout < 0 ||
		timeout > MAX_TIMEOUT_SECONDS
	) {
		throw new Error(
			`Invalid eval timeout: must be between 0 and ${MAX_TIMEOUT_SECONDS} seconds`,
		);
	}
}

export class PersistentEvalKernel {
	private child?: ChildProcess;
	private input?: NonNullable<ChildProcess["stdin"]>;
	private cwd?: string;
	private protocolBuffer = "";
	private pending?: PendingEval;

	constructor(
		private readonly language: EvalLanguage,
		private readonly version: EvalVersion,
		private readonly hostCall?: EvalHostCallHandler,
	) {}

	async execute(
		code: string,
		cwd: string,
		timeout: number,
		signal: AbortSignal | undefined,
		reset: boolean,
		tools: EvalToolMetadata[] = [],
		skills: EvalSkillMetadata[] = [],
	): Promise<EvalExecution> {
		validateEvalTimeout(timeout);
		if (this.pending) {
			throw new Error(`${this.language} eval ${this.version} kernel is already executing a cell`);
		}
		if (reset || (this.cwd !== undefined && this.cwd !== cwd)) this.resetKernel();
		this.ensureStarted(cwd);
		if (signal?.aborted) {
			this.resetKernel();
			throw new Error("aborted");
		}

		const id = randomUUID();
		return new Promise<EvalExecution>((resolve, reject) => {
			const pending: PendingEval = {
				id,
				resolve,
				reject,
				output: Buffer.alloc(0),
				truncated: false,
				outerSignal: signal,
				runController: new AbortController(),
			};
			this.pending = pending;
			if (timeout > 0) {
				pending.timer = setTimeout(() => {
					this.resetKernel(
						new Error(`Eval timed out after ${timeout} seconds; ${this.language} ${this.version} kernel reset`),
					);
				}, timeout * 1000);
			}
			if (signal) {
				pending.abortHandler = () => this.resetKernel(new Error("aborted; eval kernel reset"));
				signal.addEventListener("abort", pending.abortHandler, { once: true });
			}
			this.writeWorkerMessage(
				this.version === "v2" ? { type: "eval", id, code, tools, skills } : { id, code },
			);
		});
	}

	close() {
		this.resetKernel(new Error("session_shutdown"));
	}

	private writeWorkerMessage(message: unknown) {
		let line: string;
		try {
			line = `${JSON.stringify(message)}\n`;
		} catch (error) {
			this.resetKernel(new Error(`Failed to encode ${this.language} eval protocol message: ${String(error)}`));
			return;
		}
		this.input?.write(line, (error) => {
			if (error) this.resetKernel(error);
		});
	}

	private ensureStarted(cwd: string) {
		if (this.child) return;
		if (this.version === "v2" && this.language !== "js") {
			throw new Error("Eval Bridge v2 supports JavaScript only");
		}
		const command =
			this.language === "js"
				? process.execPath
				: process.env.PI_GROK_PYTHON || (process.platform === "win32" ? "python" : "python3");
		const worker =
			this.version === "v2"
				? JS_EVAL_WORKER_V2
				: this.language === "js"
					? JS_EVAL_WORKER_V1
					: PYTHON_EVAL_WORKER_V1;
		const args = this.language === "js" ? ["-e", worker] : ["-u", "-c", worker];
		const child = spawn(command, args, {
			cwd,
			env: process.env,
			detached: process.platform !== "win32",
			stdio: ["pipe", "pipe", "pipe", "pipe"],
			windowsHide: true,
		});
		const input = child.stdin;
		const stdout = child.stdout;
		const stderr = child.stderr;
		const protocol = child.stdio[3] as NodeJS.ReadableStream | null;
		if (!input || !stdout || !stderr || !protocol) {
			killChildProcess(child);
			throw new Error(`Failed to start ${this.language} eval ${this.version} kernel pipes`);
		}
		this.child = child;
		this.input = input;
		this.cwd = cwd;
		this.protocolBuffer = "";
		stdout.on("data", (chunk) => this.appendOutput(chunk));
		stderr.on("data", (chunk) => this.appendOutput(chunk));
		protocol.on("data", (chunk) => this.consumeProtocol(chunk));
		child.once("error", (error) => {
			if (this.child === child) this.resetKernel(error);
		});
		child.once("close", (code, childSignal) => {
			if (this.child !== child) return;
			const reason = `Eval ${this.version} kernel exited (code=${code ?? "none"}, signal=${childSignal ?? "none"})`;
			this.child = undefined;
			this.input = undefined;
			this.cwd = undefined;
			this.protocolBuffer = "";
			this.takePending()?.reject(new Error(reason));
		});
	}

	private appendOutput(chunk: Buffer | string) {
		const pending = this.pending;
		if (!pending) return;
		const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
		const joined = Buffer.concat([pending.output, bytes]);
		if (joined.length > MAX_OUTPUT_BYTES) {
			pending.output = joined.subarray(joined.length - MAX_OUTPUT_BYTES);
			pending.truncated = true;
			return;
		}
		pending.output = joined;
	}

	private consumeProtocol(chunk: Buffer | string) {
		this.protocolBuffer += Buffer.isBuffer(chunk) ? chunk.toString("utf8") : chunk;
		let newline = this.protocolBuffer.indexOf("\n");
		while (newline >= 0) {
			const line = this.protocolBuffer.slice(0, newline);
			this.protocolBuffer = this.protocolBuffer.slice(newline + 1);
			if (line.trim()) this.handleProtocolMessage(line);
			newline = this.protocolBuffer.indexOf("\n");
		}
	}

	private handleProtocolMessage(line: string) {
		let message: EvalWorkerReply | EvalV2WorkerMessage;
		try {
			message = JSON.parse(line) as EvalWorkerReply | EvalV2WorkerMessage;
		} catch (error) {
			this.resetKernel(
				new Error(`Invalid ${this.language} eval ${this.version} protocol reply: ${String(error)}`),
			);
			return;
		}
		if (this.version === "v2" && "type" in message && message.type === "host_call") {
			this.handleHostCall(message);
			return;
		}
		this.handleEvalResult(message as EvalWorkerReply);
	}

	private handleHostCall(call: EvalV2HostCall) {
		const pending = this.pending;
		if (!pending || call.evalId !== pending.id) {
			this.writeWorkerMessage({
				type: "host_result",
				id: call.id,
				ok: false,
				error: "host call does not belong to the active eval cell",
			});
			return;
		}
		if (!this.hostCall) {
			this.writeWorkerMessage({ type: "host_result", id: call.id, ok: false, error: "eval v2 host bridge unavailable" });
			return;
		}

		void this.hostCall(call, pending.runController.signal)
			.then((value) => {
				if (this.pending !== pending) return;
				this.writeWorkerMessage({ type: "host_result", id: call.id, ok: true, value });
			})
			.catch((error) => {
				if (this.pending !== pending) return;
				this.writeWorkerMessage({
					type: "host_result",
					id: call.id,
					ok: false,
					error: error instanceof Error ? error.message : String(error),
				});
			});
	}

	private handleEvalResult(reply: EvalWorkerReply) {
		if (!this.pending || reply.id !== this.pending.id) return;
		const pending = this.takePending();
		if (!pending) return;
		const streamed = pending.output.toString("utf8");
		const output = [streamed, reply.value ?? ""]
			.filter((part) => part.length > 0)
			.join(streamed && reply.value ? (streamed.endsWith("\n") ? "" : "\n") : "");
		const rendered = pending.truncated
			? `[output truncated to ${MAX_OUTPUT_BYTES} bytes]\n${output}`
			: output;
		if (reply.ok) {
			pending.resolve({ output: rendered, truncated: pending.truncated });
			return;
		}
		pending.reject(new Error([reply.error || "Eval failed", rendered].filter(Boolean).join("\n")));
	}

	private takePending(reason: Error = new Error("eval cell settled")): PendingEval | undefined {
		const pending = this.pending;
		this.pending = undefined;
		if (!pending) return undefined;
		if (pending.timer) clearTimeout(pending.timer);
		if (pending.outerSignal && pending.abortHandler) {
			pending.outerSignal.removeEventListener("abort", pending.abortHandler);
		}
		if (!pending.runController.signal.aborted) pending.runController.abort(reason);
		return pending;
	}

	private resetKernel(error?: Error) {
		const child = this.child;
		this.child = undefined;
		this.input = undefined;
		this.cwd = undefined;
		this.protocolBuffer = "";
		const pending = this.takePending(error ?? new Error("eval kernel reset"));
		if (child) killChildProcess(child);
		if (pending && error) pending.reject(error);
	}
}


export function evalHostToolValue(content: Array<{ type: string; text?: string; [key: string]: unknown }>) {
	const text = content
		.filter((item) => item.type === "text" && typeof item.text === "string")
		.map((item) => item.text)
		.join("\n");
	return { text, content };
}
