/** Pi child-session lifecycle owner for grok-pi subagents. */

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { SubagentRuntime } from "./runtime.ts";
import { registerV1AgentTools, registerV1Commands } from "./tools-v1.ts";
import { registerV2Tools } from "./v2.ts";

const BUILTIN_SKILL_PATH = join(
  dirname(fileURLToPath(import.meta.url)),
  "skills",
  "multi-agent-proactive",
  "SKILL.md",
);

export default function piGrokSubagents(pi: ExtensionAPI): void {
  if (process.env.PI_GROK_SUBAGENTS !== "1") return;

  pi.on("resources_discover", () => ({ skillPaths: [BUILTIN_SKILL_PATH] }));

  const runtime = new SubagentRuntime(pi);
  // V2 is a pure team surface: the single-agent tools would otherwise compete
  // with team semantics (spawn_subagent blocks the root, team mode must not).
  const teamMode = process.env.PI_GROK_SUBAGENTS_V2 === "1";
  registerV1Commands(pi, runtime);
  if (teamMode) registerV2Tools(pi, runtime);
  else registerV1AgentTools(pi, runtime);

  pi.on("session_start", (_event, ctx) => runtime.onSessionStart(ctx));
  pi.on("session_shutdown", () => runtime.shutdown());
}
