import { describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import mentions, { MENTIONS_CUSTOM_TYPE } from "./index.ts";

function setupMentionsHarness(opts: {
  cwd: string;
  skills?: Array<{ name: string; description?: string; filePath?: string }>;
  mcpServers?: Array<{ name: string; config?: { url?: string } }>;
}) {
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const fakePi = {
    on: (name: string, fn: (event: unknown, ctx: unknown) => unknown) => {
      handlers.set(name, fn);
    },
    registerCommand: () => {},
    getCommands: () =>
      (opts.skills ?? []).map((s) => ({
        name: `skill:${s.name}`,
        source: "skill",
        description: s.description ?? "",
        sourceInfo: { path: s.filePath },
      })),
    getMcpServers: () => opts.mcpServers ?? [],
    getAllTools: () => [],
  };
  mentions(fakePi as never);
  return {
    fireBeforeAgentStart(prompt: string) {
      const handler = handlers.get("before_agent_start");
      expect(handler).toBeDefined();
      const sections: Record<string, string> = {};
      const result = handler!(
        {
          prompt,
          systemPromptOptions: { sections, skills: opts.skills ?? [] },
        },
        { cwd: opts.cwd },
      ) as
        | {
            message?: {
              customType: string;
              content: string;
              display: boolean;
              details?: { refs?: Array<{ kind: string; id: string }> };
            };
          }
        | undefined;
      return { result, sections };
    },
  };
}

describe("pi-grok-mentions before_agent_start custom_message XML injection", () => {
  test("returns Pi-native custom_message with XML blocks without mutating systemPrompt sections", () => {
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-mentions-xml-"));
    try {
      fs.mkdirSync(path.join(tmpDir, "src"), { recursive: true });
      fs.writeFileSync(path.join(tmpDir, "src", "main.rs"), "fn main() {}\n", "utf8");

      const harness = setupMentionsHarness({
        cwd: tmpDir,
        skills: [
          {
            name: "use-pi-session",
            description: "Inspect past Pi sessions",
            filePath: "/tmp/skills/use-pi-session/SKILL.md",
          },
        ],
        mcpServers: [{ name: "nexusdock", config: { url: "https://example.invalid/mcp" } }],
      });

      const { result, sections } = harness.fireBeforeAgentStart(
        "Check @skill:use-pi-session @mcp:nexusdock @file:src/main.rs and @src",
      );

      // System prompt must stay completely untouched (preserves LLM KV cache).
      expect(sections.referenced_objects).toBeUndefined();
      expect(Object.keys(sections)).toHaveLength(0);

      expect(result?.message?.customType).toBe(MENTIONS_CUSTOM_TYPE);
      expect(result?.message?.display).toBe(false);
      const content = result?.message?.content ?? "";
      expect(content).toContain("<referenced_objects>");
      expect(content).toContain('location="/tmp/skills/use-pi-session/SKILL.md"');
      expect(content).toContain('<mcp name="nexusdock"');
      expect(content).toContain('<file name="');
      expect(content).toContain('path="src/main.rs"');
      expect(content).toContain('<folder path="src"');
      expect(content).toContain("main.rs");
      expect(content).toContain("</referenced_objects>");
    } finally {
      fs.rmSync(tmpDir, { recursive: true, force: true });
    }
  });

  test("returns undefined and keeps sections untouched when prompt has no @ mentions", () => {
    const harness = setupMentionsHarness({ cwd: process.cwd() });
    const { result, sections } = harness.fireBeforeAgentStart("Just a normal prompt");
    expect(result).toBeUndefined();
    expect(sections.referenced_objects).toBeUndefined();
  });
});
