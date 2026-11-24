import { truncateToWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";

const MAX_FORMAT_LINE_BYTES = 2048;
const MAX_PREVIEW_LINES = 14;
const MAX_FILE_ROWS = 5;
const SUBAGENT_ACTIONS = {
  spawn_subagent: "Spawn subagent",
  send_message_to_subagent: "Send subagent message",
  get_command_or_subagent_output: "Get subagent output",
  kill_command_or_subagent: "Cancel subagent",
  list_subagents: "List subagents",
};
const TEAM_ACTIONS = {
  spawn_team_agent: "Spawn team agent",
  spawn_team: "Spawn team preset",
  team_send_message: "Send team message",
  team_followup_task: "Assign follow-up task",
  team_wait: "Wait for team activity",
  team_list: "List team agents",
  team_interrupt: "Interrupt team agent",
  team_report: "Report team result",
};

export const TIMELINE_TOOL_GROUPS = [
  { id: "subagents", label: "Subagent interactions", tools: Object.keys(SUBAGENT_ACTIONS) },
  { id: "team-subagents", label: "Team subagent interactions", tools: Object.keys(TEAM_ACTIONS) },
];

type ToolStatus = "running" | "success" | "failed" | "cancelled";
interface TimelineTool {
  kind?: "tool";
  callId: string;
  toolName: string;
  status: ToolStatus;
  interaction?: { target?: string | null; mode?: string | null };
}
interface ChangeCounts {
  added: number;
  deleted: number;
  written: number;
}
interface TurnChanges extends ChangeCounts {
  fileCount: number;
  files: Array<ChangeCounts & { path: string }>;
}
type TimelineItem = TimelineTool
  | { kind: "prompt"; prompt: string; reply?: string | null; changes?: TurnChanges | null }
  | { kind: "compaction"; summary: string };

type FormatView = Readonly<{ tickColumns: number; previewColumns: number }>;
type Formatted = { tickAnsi?: string; previewAnsi?: readonly string[] };
export type TimelineRenderer = (entry: Readonly<TimelineTool>, view: FormatView) => Formatted;
const renderers = new Map<string, TimelineRenderer>();

export function registerTimelineRenderer(toolName: string, renderer: TimelineRenderer): void {
  renderers.set(toolName, renderer);
}

const statusColors: Record<ToolStatus, number> = { running: 36, success: 32, failed: 31, cancelled: 33 };
const style = (code: number, text: string) => `\x1b[${code}m${text}\x1b[0m`;

export function plain(text: string): string {
  return text.replace(/\p{Cc}/gu, " ");
}

function synopsis(source: string): string {
  return source
    .replace(/(^|\n)[ \t]*(```|~~~)[\s\S]*?(?:\n[ \t]*\2[ \t]*(?=\n|$)|$)/g, "$1[code] ")
    .replace(/^\s{0,3}#{1,6}\s+/gm, "")
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

function excerptLines(source: string, columns: number, count: number): string[] {
  const text = plain(synopsis(source));
  if (!text || columns < 1) return [];
  const rows = wrapTextWithAnsi(text, columns);
  if (rows.length > count) {
    rows[count - 1] = truncateToWidth(rows[count - 1]!, Math.max(1, columns - 1), "") + "…";
  }
  return rows.slice(0, count);
}

function statusLine(entry: TimelineTool) {
  return style(statusColors[entry.status], entry.status);
}

function dividerLine(columns: number): string {
  return style(2, "─".repeat(Math.max(1, columns)));
}

function styledDiffCounts(change: ChangeCounts, compactWritten: boolean): { ansi: string; plainLen: number } {
  const addText = `+${change.added}`;
  const delText = `-${change.deleted}`;
  const writtenText =
    change.written > 0
      ? compactWritten
        ? ` ${change.written}w`
        : ` · ${change.written} written`
      : "";
  const ansi = `${style(32, addText)} ${style(31, delText)}${writtenText ? style(2, writtenText) : ""}`;
  return { ansi, plainLen: addText.length + 1 + delText.length + writtenText.length };
}

function fileRows(changes: TurnChanges, columns: number): string[] {
  const fileLabel = `${changes.fileCount} ${changes.fileCount === 1 ? "file" : "files"}`;
  const summaryCounts = styledDiffCounts(changes, false);
  const header = truncateToWidth(
    `${style(36, fileLabel)}${style(2, " · ")}${summaryCounts.ansi}`,
    columns,
  );
  const visibleFiles = changes.files.slice(0, MAX_FILE_ROWS);
  const rows: string[] = [
    header,
    ...visibleFiles.map((file) => {
      const counts = styledDiffCounts(file, true);
      const width = Math.max(0, columns - counts.plainLen - 2);
      const path = plain(file.path);
      const name = path.split(/[\\/]/).at(-1) || path;
      const shown = truncateToWidth(path, width) === path ? path : truncateToWidth(name, width);
      return truncateToWidth(`${counts.ansi}  ${shown}`, columns);
    }),
  ];
  const remaining = Math.max(0, changes.fileCount - visibleFiles.length);
  if (remaining > 0) {
    rows.push(
      style(
        2,
        truncateToWidth(`… +${remaining} more ${remaining === 1 ? "file" : "files"}`, columns),
      ),
    );
  }
  return rows;
}

function agentRenderer(glyph: string, color: number, action: string): TimelineRenderer {
  return (entry, { previewColumns }) => {
    const { target, mode } = entry.interaction ?? {};
    const detail = [target, mode?.replaceAll("_", " ")].filter(Boolean).join(" · ");
    return {
      tickAnsi: style(color, glyph),
      previewAnsi: [
        truncateToWidth(action, previewColumns),
        ...(detail ? [style(2, truncateToWidth(plain(detail), previewColumns))] : []),
        statusLine(entry),
      ],
    };
  };
}

for (const [name, action] of Object.entries(SUBAGENT_ACTIONS)) {
  registerTimelineRenderer(name, agentRenderer("S", 36, action));
}
for (const [name, action] of Object.entries(TEAM_ACTIONS)) {
  registerTimelineRenderer(name, agentRenderer("T", 34, action));
}

interface FormatRequest {
  op: "format";
  scope: { host: string; session: string; branch: number; theme: number };
  id: number;
  tickColumns: number;
  previewColumns: number;
  entries: Array<{ markerId?: number; version: string; entry: TimelineItem }>;
}

function formatItem(entry: TimelineItem, view: FormatView): Formatted | undefined {
  switch (entry.kind) {
    case "prompt": {
      if (!entry.reply && !entry.changes) {
        return { previewAnsi: excerptLines(entry.prompt, view.previewColumns, 2) };
      }
      const promptLines = excerptLines(entry.prompt, view.previewColumns, 2);
      const styledPrompt =
        promptLines.length > 0
          ? promptLines.map((line) => style(1, line))
          : [style(1, truncateToWidth("User prompt", view.previewColumns))];
      const replyLines = entry.reply ? excerptLines(entry.reply, view.previewColumns, 3) : [];
      const changeLines = entry.changes ? fileRows(entry.changes, view.previewColumns) : [];
      const out: string[] = [...styledPrompt];
      if (replyLines.length > 0) {
        out.push(dividerLine(view.previewColumns));
        out.push(...replyLines);
      }
      if (changeLines.length > 0) {
        out.push(dividerLine(view.previewColumns));
        out.push(...changeLines);
      }
      return { previewAnsi: out.slice(0, MAX_PREVIEW_LINES) };
    }
    case "compaction":
      return {
        tickAnsi: style(35, "◆"),
        previewAnsi: [
          style(1, truncateToWidth("Compaction summary", view.previewColumns)),
          dividerLine(view.previewColumns),
          ...excerptLines(entry.summary, view.previewColumns, 3),
        ],
      };
    default:
      return renderers.get(entry.toolName)?.(entry, view);
  }
}

export function formatTimelineBatch(request: FormatRequest) {
  const view = { tickColumns: request.tickColumns, previewColumns: request.previewColumns };
  const validLine = (line: unknown): line is string =>
    typeof line === "string" && Buffer.byteLength(line) <= MAX_FORMAT_LINE_BYTES;
  return {
    op: "formatted",
    scope: request.scope,
    id: request.id,
    entries: request.entries.map(({ markerId, entry, version }) => {
      const identity = { markerId, callId: "callId" in entry ? entry.callId : undefined, version };
      try {
        const formatted = formatItem(entry, view);
        if (!formatted) return identity;
        const { tickAnsi, previewAnsi } = formatted;
        if ((tickAnsi !== undefined && !validLine(tickAnsi))
          || (previewAnsi !== undefined && (!Array.isArray(previewAnsi) || previewAnsi.length > MAX_PREVIEW_LINES
            || !previewAnsi.every(validLine)))) {
          return identity;
        }
        return { ...identity, tickAnsi, previewAnsi };
      } catch {
        return identity;
      }
    }),
  };
}

export type { FormatRequest };
