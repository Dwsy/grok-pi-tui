/** Durable daemon paths, descriptor validation and process liveness helpers. */
import { homedir } from "node:os";
import path from "node:path";
import { MCPEVAL_PROTOCOL } from "./mcpeval-protocol.ts";

function defaultHomeDir(): string {
  return process.env.GROK_HOME?.trim() || path.join(homedir(), ".grok-pi");
}

export function mcpevalRoot(root?: string): string {
  return root?.trim() || path.join(defaultHomeDir(), "eval-pi-mcp");
}

export function mcpevalSocketPath(root?: string): string {
  return path.join(mcpevalRoot(root), "daemon.sock");
}

export function mcpevalStatePath(root?: string): string {
  return path.join(mcpevalRoot(root), "daemon.json");
}

export const DEFAULT_MCPEVAL_PORT = 47631;
export function daemonPort(value?: number): number {
  const candidate = value ?? (Number.parseInt(process.env.PI_GROK_MCPEVAL_PORT ?? "", 10) || DEFAULT_MCPEVAL_PORT);
  if (!Number.isInteger(candidate) || candidate < 0 || candidate > 65535) throw new Error("Invalid mcpeval daemon port " + JSON.stringify(candidate));
  return candidate;
}

export type DaemonState = {
  protocol: typeof MCPEVAL_PROTOCOL;
  pid: number;
  socketPath: string;
  url: string;
  secret: string;
  startedAtMs: number;
};

export function daemonDescriptor(state: DaemonState): { protocol: string; url: string; socketPath: string; pid: number; startedAtMs: number } {
  return { protocol: state.protocol, url: state.url, socketPath: state.socketPath, pid: state.pid, startedAtMs: state.startedAtMs };
}

export function isUuid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
}

export function daemonPidAlive(pid: unknown): boolean {
  if (typeof pid !== "number" || !Number.isInteger(pid) || pid <= 0) return false;
  try { process.kill(pid, 0); return true; } catch { return false; }
}
