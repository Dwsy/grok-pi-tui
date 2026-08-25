/**
 * Host-environment detection and Pi theme bootstrap.
 *
 * grok-pi RPC needs a custom() host; native Pi TUI already has a real one.
 */

import { basename, dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { realpathSync } from "node:fs";

export function shouldInstallRemoteHost(): boolean {
  const flag = process.env.PI_GROK_REMOTE_TUI?.toLowerCase();
  if (flag === "0" || flag === "false" || flag === "off" || flag === "no") {
    return false;
  }
  // grok-pi child always sets PI_GROK=1; native `pi` does not.
  return process.env.PI_GROK === "1" || flag === "1" || flag === "true" || flag === "on" || flag === "yes";
}

export function hostUrl(relativePath: string): string {
  const entryDir = dirname(realpathSync(process.argv[1]!));
  const hostDistDir = basename(entryDir) === "bundle" ? dirname(entryDir) : entryDir;
  return new URL(relativePath, pathToFileURL(hostDistDir).href + "/").href;
}

/** Pi interactive components (OAuthSelector/LoginDialog) touch global theme in constructors. */
let themeReady: Promise<void> | null = null;

export async function ensurePiTheme(): Promise<void> {
  if (themeReady) return themeReady;
  themeReady = (async () => {
    // Runtime-selected module: resolved against the running Pi host's dist
    // directory (process.argv[1]), not a spec known at author time.
    const mod = (await import(hostUrl("modes/interactive/theme/theme.js"))) as {
      theme?: { name?: string };
      initTheme?: (name?: string, enableWatcher?: boolean) => void;
    };
    try {
      void mod.theme?.name;
    } catch {
      if (typeof mod.initTheme !== "function") {
        throw new Error("Pi theme.initTheme missing");
      }
      mod.initTheme(undefined, false);
      void mod.theme?.name;
    }
  })().catch((error) => {
    themeReady = null;
    throw error;
  });
  return themeReady;
}
