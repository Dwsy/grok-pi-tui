/**
 * grok-pi /loop — recurring prompt scheduler.
 *
 * The Rust injector materializes this module and its relative-import closure
 * beside the process-private task control file.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { registerLoopCommand } from "./command.ts";
import { controlPath } from "./control.ts";
import { hydrate } from "./scheduler.ts";
import { registerSchedulerTools } from "./tools.ts";

export default function piGrokLoop(pi: ExtensionAPI): void {
  if (process.env.PI_GROK !== "1") return;
  if (!controlPath()) return;

  hydrate(pi);
  registerLoopCommand(pi);
  registerSchedulerTools(pi);
}
