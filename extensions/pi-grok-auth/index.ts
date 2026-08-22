/**
 * Default-on Pi auth for grok-pi (min Pi 0.80.10).
 *
 * Registers Remote TUI-backed `/login` and `/logout`. The Rust injector
 * materializes this entry point with all relative imports as one bundle.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { registerLoginCommand } from "./login.ts";
import { registerLogoutCommand } from "./logout.ts";

export default function piGrokAuth(pi: ExtensionAPI): void {
	if (process.env.PI_GROK !== "1") return;

	registerLoginCommand(pi);
	registerLogoutCommand(pi);
}
