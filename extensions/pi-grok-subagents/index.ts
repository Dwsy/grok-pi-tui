/** Pi child-session lifecycle owner for grok-pi subagents. */

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { SubagentRuntime } from "./runtime.ts";
import { registerV1Tools } from "./tools-v1.ts";
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
  registerV1Tools(pi, runtime);
  if (process.env.PI_GROK_SUBAGENTS_V2 === "1") registerV2Tools(pi, runtime);

  pi.on("session_start", (_event, ctx) => runtime.onSessionStart(ctx));
  pi.on("session_shutdown", () => runtime.shutdown());
}
