import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { registerV1 } from "./v1.ts";
import { registerV2 } from "./v2.ts";

export type TodoVersion = "v1" | "v2";

export function resolveTodoVersion(raw?: string): TodoVersion {
  const value = (raw ?? process.env.PI_GROK_TODO_VERSION ?? "v1").trim().toLowerCase();
  if (value === "v1" || value === "v2") return value;
  throw new Error(`Invalid PI_GROK_TODO_VERSION=${JSON.stringify(value)}; expected "v1" or "v2"`);
}

export default function (pi: ExtensionAPI) {
  if (resolveTodoVersion() === "v1") {
    registerV1(pi);
    return;
  }
  registerV2(pi);
}
