/** Pi child-session lifecycle owner for grok-pi subagents. */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { SubagentRuntime } from "./runtime.ts";
import { registerV1Tools } from "./tools-v1.ts";
import { registerV2Tools } from "./v2.ts";

export default function piGrokSubagents(pi: ExtensionAPI): void {
  if (process.env.PI_GROK_SUBAGENTS !== "1") return;

  const runtime = new SubagentRuntime(pi);
  registerV1Tools(pi, runtime);
  if (process.env.PI_GROK_SUBAGENTS_V2 === "1") registerV2Tools(pi, runtime);

  pi.on("session_start", (_event, ctx) => runtime.onSessionStart(ctx));
  pi.on("session_shutdown", () => runtime.shutdown());
}
