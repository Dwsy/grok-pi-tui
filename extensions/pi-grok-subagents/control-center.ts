/** Subagents V2/V1 unified TUI control center (mirrors role-persona-zero structure). */

import { getAgentDir, loadProjectContextFiles } from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { configureSubagents } from "./config-ui.ts";
import { loadAgentDefinitions } from "./definitions.ts";
import type { SubagentRuntime } from "./runtime.ts";
import { configureTeamPresets } from "./team-ui.ts";
import { loadTeamDefinitions } from "./teams.ts";
import { piSelect } from "./ui-primitives.ts";

type ControlAction =
  | "status"
  | "definitions"
  | "presets"
  | "injection"
  | "close";

const ACTIONS: Array<{ value: ControlAction; label: string; description: string }> = [
  { value: "status", label: "查看运行状态", description: "V1/V2 模式、运行中子代理、定义与 preset 计数" },
  { value: "definitions", label: "管理 agents 定义", description: "增删改项目/全局 agents.md（含继承开关）" },
  { value: "presets", label: "管理 team preset", description: "增删改 V2 team preset 与成员模型" },
  { value: "injection", label: "查看实际注入预览", description: "选中定义的子会话 skills / AGENTS / systemPrompt" },
  { value: "close", label: "关闭控制中心", description: "返回当前会话" },
];

function modeLine(): string {
  const enabled = process.env.PI_GROK_SUBAGENTS === "1";
  const v2 = process.env.PI_GROK_SUBAGENTS_V2 === "1";
  if (!enabled) return "子代理总开关：关（F2 → Agent → Pi subagents，需重启）";
  return `子代理总开关：开 · 模式：${v2 ? "V2 team（默认）" : "V1 single-agent"}`;
}

export async function openSubagentControlCenter(
  pi: ExtensionAPI,
  runtime: SubagentRuntime,
  ctx: ExtensionCommandContext,
): Promise<void> {
  for (;;) {
    const choice = await piSelect(
      ctx,
      "子代理控制中心",
      ACTIONS.map((action) => `${action.label} — ${action.description}`),
    );
    if (!choice) return;
    const action = ACTIONS.find((entry) => choice.startsWith(entry.label))?.value;
    if (!action || action === "close") return;
    if (action === "status") {
      const records = [...runtime.records.values()];
      const running = records.filter((record) => !record.finished);
      const definitions = [...loadAgentDefinitions(ctx.cwd).values()];
      const teams = [...loadTeamDefinitions(ctx.cwd).values()];
      const lines = [
        `## 子代理状态`,
        ``,
        modeLine(),
        `运行中：${running.length} / 全部：${records.length}`,
        `定义：${definitions.length}（builtin ${definitions.filter((d) => d.scope === "builtin").length} / global ${definitions.filter((d) => d.scope === "global").length} / project ${definitions.filter((d) => d.scope === "project").length}）`,
        `Team preset：${teams.length}（bundled ${teams.filter((t) => t.scope === "bundled").length} / global ${teams.filter((t) => t.scope === "global").length} / project ${teams.filter((t) => t.scope === "project").length}）`,
        ``,
        ...running.slice(0, 10).map(
          (record) =>
            `- ${runtime.shortSubagentId(record.id)} · ${record.description}${record.agentPath ? ` · ${record.agentPath}` : ""}`,
        ),
        running.length > 10 ? `- …（仅显示前 10 个）` : ``,
        ``,
        `V2 team 树（含 quiet gap / decision question）请用 \`team_list\` 工具查看。`,
      ].filter((line) => line !== `` || true);
      ctx.ui.notify(lines.join("\n"), "info");
      continue;
    }
    if (action === "definitions") {
      await configureSubagents(pi, ctx);
      continue;
    }
    if (action === "presets") {
      await configureTeamPresets(ctx);
      continue;
    }
    if (action === "injection") {
      const definitions = [...loadAgentDefinitions(ctx.cwd).values()].sort((a, b) => a.name.localeCompare(b.name));
      const picked = await piSelect(
        ctx,
        "预览哪个定义的实际注入",
        definitions.map((d) => `${d.scope}: ${d.name}`),
      );
      if (!picked) continue;
      const target = definitions.find((d) => `${d.scope}: ${d.name}` === picked);
      if (!target) continue;
      const inheritSkills = target.inheritSkills !== false;
      const inheritContext = target.inheritContext !== false;
      let contextLines: string[] = [];
      try {
        const files = loadProjectContextFiles({ cwd: ctx.cwd, agentDir: getAgentDir() });
        contextLines = inheritContext
          ? files.map((file) => `- ${file.path}`)
          : [`- （已隔离：inherit_context: false，不加载 AGENTS.md）`];
      } catch (error) {
        contextLines = [`- （读取上下文失败：${error instanceof Error ? error.message : String(error)}）`];
      }
      const preview = [
        `## 注入预览：${target.scope}: ${target.name}`,
        ``,
        `skills 继承：${inheritSkills ? "开（全局/项目 skills + 显式叠加）" : "关（仅显式）"}`,
        `显式 skills：${target.skills?.join(", ") || "（无）"}`,
        `显式 extensions：${target.extensions?.join(", ") || "（默认搜索扩展）"}`,
        `AGENTS 上下文继承：${inheritContext ? "开" : "关"}`,
        ...contextLines,
        ``,
        `systemPrompt（前 500 字）：`,
        `${target.systemPrompt.slice(0, 500)}${target.systemPrompt.length > 500 ? "…" : ""}`,
      ].join("\n");
      ctx.ui.notify(preview, "info");
      continue;
    }
  }
}
