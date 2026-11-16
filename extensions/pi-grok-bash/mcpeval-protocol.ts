/** Versioned JSONL protocol primitives shared by the resident daemon and Pi connector. */
import type { Socket } from "node:net";

export const MCPEVAL_PROTOCOL = "mcpeval/1";
export const MAX_MCPEVAL_FRAME_BYTES = 1024 * 1024;

export function writeJsonLine(socket: Socket, value: unknown): void {
  const line = JSON.stringify(value) + "\n";
  if (Buffer.byteLength(line) > MAX_MCPEVAL_FRAME_BYTES) throw new Error("mcpeval JSONL frame exceeds 1 MiB");
  socket.write(line);
}

export function attachJsonLines(
  socket: Socket,
  onMessage: (message: Record<string, unknown>) => void,
  onClose: () => void,
): () => void {
  let buffer = "";
  let closed = false;
  socket.setEncoding("utf8");
  const fail = () => {
    if (closed) return;
    closed = true;
    onClose();
  };
  socket.on("data", (chunk: string) => {
    buffer += chunk;
    if (Buffer.byteLength(buffer) > MAX_MCPEVAL_FRAME_BYTES) {
      socket.destroy(new Error("mcpeval JSONL frame exceeds 1 MiB"));
      return;
    }
    let boundary = -1;
    while ((boundary = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 1);
      if (!line.trim()) continue;
      try {
        const decoded: unknown = JSON.parse(line);
        if (!decoded || typeof decoded !== "object" || Array.isArray(decoded)) throw new Error("invalid frame");
        onMessage(decoded as Record<string, unknown>);
      } catch {
        socket.destroy(new Error("invalid mcpeval JSONL frame"));
        return;
      }
    }
  });
  socket.on("error", fail);
  socket.on("close", fail);
  return () => {
    closed = true;
    socket.removeAllListeners("data");
    socket.removeListener("error", fail);
    socket.removeListener("close", fail);
  };
}
