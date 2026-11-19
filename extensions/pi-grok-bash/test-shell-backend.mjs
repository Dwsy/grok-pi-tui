import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import {
	OMP_HOST_PROTOCOL_VERSION,
	launchBackendProcess,
	resolveBashBackend,
	resolveOmpHostBinary,
} from "./shell-backend.ts";

test("resolveBashBackend defaults to system and honors omp", () => {
	assert.equal(resolveBashBackend(undefined), "system");
	assert.equal(resolveBashBackend(""), "system");
	assert.equal(resolveBashBackend("system"), "system");
	assert.equal(resolveBashBackend("OMP"), "omp");
	assert.equal(resolveBashBackend(" omp "), "omp");
	assert.equal(resolveBashBackend("unknown"), "system");
});

test("resolveOmpHostBinary validates PI_GROK_OMP_SHELL_HOST", () => {
	const prev = process.env.PI_GROK_OMP_SHELL_HOST;
	try {
		process.env.PI_GROK_OMP_SHELL_HOST = "/nonexistent/grok-pi-shell-host-missing";
		assert.throws(() => resolveOmpHostBinary(), /missing or non-executable/);
	} finally {
		if (prev === undefined) delete process.env.PI_GROK_OMP_SHELL_HOST;
		else process.env.PI_GROK_OMP_SHELL_HOST = prev;
	}
});

test("launchBackendProcess speaks JSONL protocol with omp host and projects shell_trace", async () => {
	const dir = mkdtempSync(path.join(tmpdir(), "omp-host-mock-"));
	const mockHost = path.join(dir, "mock-grok-pi-shell-host.mjs");
	const prevHost = process.env.PI_GROK_OMP_SHELL_HOST;
	try {
		writeFileSync(
			mockHost,
			`#!/usr/bin/env node
import { createInterface } from "node:readline";
const rl = createInterface({ input: process.stdin });
rl.once("line", (line) => {
  const req = JSON.parse(line);
  if (req.protocol_version !== ${OMP_HOST_PROTOCOL_VERSION}) {
    process.stdout.write(JSON.stringify({ type: "error", code: "unsupported_protocol_version", message: "bad" }) + "\\n");
    process.exit(1);
  }
  process.stdout.write(JSON.stringify({ type: "output", text: "hello from omp\\n" }) + "\\n");
  process.stdout.write(JSON.stringify({
    type: "completed",
    exit_code: 0,
    cancelled: false,
    timed_out: false,
    working_dir: req.cwd,
    trace: {
      version: 1,
      total_duration_ms: 9,
      exit_code: 0,
      truncated: false,
      nodes: [
        { id: 0, depth: 0, kind: "simple", command: req.command, state: "ok", exit_code: 0, duration_ms: 9 }
      ]
    }
  }) + "\\n");
});
`,
		);
		chmodSync(mockHost, 0o755);
		process.env.PI_GROK_OMP_SHELL_HOST = mockHost;

		const chunks = [];
		let completedEvent;
		const exit = await new Promise((resolve, reject) => {
			launchBackendProcess({
				backend: "omp",
				command: "echo hello",
				cwd: dir,
				env: { ...process.env, TEST_VAR: "1" },
				timeoutSeconds: 5,
				trace: true,
				onOutput: (buf) => chunks.push(buf.toString("utf8")),
				onCompletedEvent: (ev) => {
					completedEvent = ev;
				},
				onExit: (code, signal) => resolve({ code, signal }),
				onError: reject,
			});
		});

		assert.deepEqual(exit, { code: 0, signal: null });
		assert.equal(chunks.join(""), "hello from omp\n");
		assert.equal(completedEvent?.exitCode, 0);
		assert.equal(completedEvent?.trace?.version, 1);
		assert.equal(completedEvent?.trace?.nodes[0]?.command, "echo hello");
	} finally {
		if (prevHost === undefined) delete process.env.PI_GROK_OMP_SHELL_HOST;
		else process.env.PI_GROK_OMP_SHELL_HOST = prevHost;
		rmSync(dir, { recursive: true, force: true });
	}
});
