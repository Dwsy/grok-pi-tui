/**
 * grok-pi built-in: reply emphasis.
 *
 * Two halves, and this extension owns both:
 *
 * 1. **The rules.** `before_agent_start` adds a `reply_style` section to the system prompt, telling
 *    the agent to write `[[Done]]`, `[[Failed]]` and `[[Needs you]]` beside the things they describe.
 * 2. **The promise that they get drawn.** The rules are only injected when something will draw the
 *    pills. Under grok-pi that is the Rust pager (`xai-grok-markdown`'s `pill` module), not Pi's
 *    own terminal UI, so this extension must not gate on `mode !== "tui"` the way the upstream
 *    Pi extension does: Pi runs in `rpc` mode here and its markdown transformers never fire.
 *
 * Nothing here draws. The pager renderer reads the same vocabulary from `pill.rs`, so a word this
 * file teaches always has a tone to draw.
 *
 * Display only: `[[Done]]` stays `[[Done]]` in the session file, in copied text and in exports.
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** The words the agent must use, grouped by the tone the pager draws them in. */
const VOCABULARY = {
  error: {
    core: ["Failed", "Blocked", "Broken", "Unsafe"],
    also: ["fail", "failing", "error", "rejected", "down"],
  },
  warning: {
    core: [
      "Needs you",
      "Decision",
      "Unverified",
      "Partial",
      "Skipped",
      "In progress",
      "Pending",
      "At risk",
    ],
    also: ["needs review", "waiting", "review", "todo", "stale", "draft"],
  },
  success: {
    core: ["Done", "Passed", "Observed", "Fixed", "Live", "Recommended", "Keep"],
    also: [
      "pass",
      "ready",
      "merged",
      "safe",
      "verified",
      "shipped",
      "ok",
      "approved",
      "complete",
      "completed",
      "finished",
      "healthy",
    ],
  },
  danger: { core: ["Costs money", "Destructive", "Irreversible"], also: [] as string[] },
  mode: { core: ["Brainstorm", "Investigate", "Preview", "Implement", "Qualify"], also: [] as string[] },
  owner: { core: ["Owned", "Shared", "Foreign"], also: [] as string[] },
} as const;

/** Soft blue like any unknown label, but with an icon and a place in the rules. */
const CONTEXT_WORDS = ["Inferred", "New", "Changed", "Optional"];

/** Words that open a heading or a leading bold label and take an icon from that word. */
const LABEL_WORDS = [
  "Finding", "Result", "Status", "Evidence", "Cause", "Effect", "Fix", "Risk", "Limits", "Cost",
  "Decision", "Options", "Plan", "Next", "Why", "How", "Rule", "Example", "Checked", "Verdict",
  "Rollback", "Undo", "Review", "Scoreboard", "Terminal", "Folder", "File", "Branch", "Ideas",
  "Pills", "Colors", "Icons",
];

/** Bold lead-ins that name the action a pill calls for. The colon is required. */
const ACTION_WORDS = ["Check", "Do", "Next", "Unblock", "Left", "Run", "Guard", "Undo", "See"];

const pills = (words: readonly string[]) => words.map((w) => `[[${w}]]`).join(", ");

/** The rules, with every word list read from the tables that also draw them. */
export function replyGuidance(): string {
  const v = VOCABULARY;
  return `These rules shape replies written to the user in this terminal. It draws [[Word]] as a coloured chip with an icon, adds icons to known headings, and cannot change text size.

- **Label every reply.** Start the first substantive line with a bold label, even for a one-line update, and give each subject in a multi-part reply a short heading or a bullet with a bold lead label. Write the label as a plain word; the terminal adds its icon. Never type icon glyphs. Labels with an icon: ${LABEL_WORDS.join(", ")}. Prefer these; any other label shows without an icon.
- **Shape follows information.** Use \`├──\` and \`└──\` for what belongs to what, \`→\` for a sequence, \`✓\` or \`✗\` beside a checked item. Write tree rows as indented lines directly under their parent with no list dash, one line per row. A one-line answer can stay one line.
- **Bold the anchor terms.** Treat the projects, components, choices, options, and statuses under discussion as the nodes of a graph, and bold them where they carry the point: a first mention in a section, a bullet's lead label such as **Cause:**, and the recommendation or decision the user must make. One to three bold spans per paragraph or bullet; bolding every noun erases the signal. Italicize sparingly, for contrast and meaning-changing words such as *not*, *only*, or *unverified*.
- **Pills are required, not optional.** A pill is \`[[Word]]\` beside the subject it describes; the terminal colours it and adds its icon from the word. A pill names a state or an ask, never an adjective. Use exactly these words so the same state always reads the same:
  - **Red, something is wrong:** ${pills(v.error.core)}. Every failure and blocker carries one.
  - **Yellow, look here:** ${pills(v.warning.core)}. [[Needs you]] is an action only the user can take; [[Decision]] is a call only the user makes. Every ask and every unchecked claim carries one.
  - **Green, settled:** ${pills(v.success.core)}.
  - **Solid red, consequences:** ${pills(v.danger.core)}. Required beside any action that has one, before taking it.
  - **Cyan, kind of work:** open every substantive reply with the kind of work you understood the request to be: ${pills(v.mode.core)}. Brainstorm is options and a recommendation, Investigate is evidence, Preview and Implement are a working result, Qualify is rigorous proof.
  - **Blue, ownership:** ${pills(v.owner.core)} beside any program, file, service, or configuration you changed or propose to change outside the task's own files: created by this task, also used by others, or belonging to someone else.
  - **Soft blue, context:** ${pills(CONTEXT_WORDS)}, and any other short label.
- **Pill placement.** Mark evidence claims with [[Observed]] (witnessed in the real system), [[Inferred]] (reasoned from other signals), or [[Unverified]] (not checked). A known word may carry short detail: [[Passed 12/12]], [[Failed: 3 tests]]. One pill per bullet or table cell, with two exceptions: a sequence line joined by \`→\`, and a scoreboard, which is one line of counts such as [[Done 6]] [[Failed 1]] [[Needs you 1]] under the opening sentence of a long report. In a long reply, repeat every [[Needs you]] and [[Decision]] in a closing line. Never put pills in headings, and do not invent new status words.
- **Every attention pill carries its action.** Follow the pill with the one fact the user needs to act on it, introduced by a bold lead-in word ending in a colon. These lead-ins get an icon: ${ACTION_WORDS.join(", ")}. Required for every red, yellow, and solid red pill and for [[Inferred]]; optional for green ones.
  - [[Unverified]] means you *could not* check. If you can run the check yourself, run it and report the result instead. Otherwise give the fastest check and who can run it: "the new colour on your screen. **Check:** reload and look at the first pill."
  - [[Needs you]]: the exact action and what it unblocks. "**Do:** sign in to the staging dashboard; then I can read the deploy log."
  - [[Decision]]: the options, your pick, and what you will do meanwhile. "Backfill, or make the column optional? I'd backfill. **Next:** I'll prepare it as a dry run."
  - [[Failed]] or [[Broken]]: the cause and what you are doing about it. "Migration stopped on 41 empty rows. **Next:** preparing a backfill."
  - [[Blocked]]: what blocks it and the smallest thing that unblocks it. "No access to the billing repository. **Unblock:** read access for this session."
  - [[Partial]] or [[In progress]]: what remains. "3 of 5 pages converted. **Left:** pricing and checkout."
  - [[Skipped]]: why, and how to run it anyway. "Load test, which you waived. **Run:** \`bun run load\`."
  - [[At risk]] or [[Unsafe]]: what would make it fail and the guard against it. "Breaks if billing reads the old column. **Guard:** keep both columns for one release."
  - [[Costs money]]: the amount, before acting. "About $4 for 20 video generations."
  - [[Destructive]] or [[Irreversible]]: what is lost and how to get it back, or that nothing can. "Deletes 41 order rows. **Undo:** none; export them first."
  - [[Shared]] or [[Foreign]]: who else is affected. "Every session picks this up on its next reload."
  - [[Inferred]]: what it was inferred from. "From the last three deploy logs; I did not reproduce it."
  - [[Done]], [[Fixed]], or [[Live]]: where to see it. "**See:** the address, file, or command that shows it."
- **Headings and code.** Use \`##\` headings; \`###\` and deeper show their hash marks. Reserve inline code for literals the user may copy or search (paths, commands, exact identifiers); otherwise name subjects in plain words. Blockquotes render dim: asides only, never a caveat the user must see.
- **Copyable text stays clean.** Keep pills out of paths, commands, code, literal output, and machine-readable data. Omit pills and tree shapes when the user asks for plain text, ASCII, or an exact format.`;
}

export default function emphasis(pi: ExtensionAPI) {
  pi.on("before_agent_start", (event) => {
    const sections = (event.systemPromptOptions.sections ??= {});
    if (process.env.PI_GROK_EMPHASIS_GUIDANCE === "0") {
      delete sections.reply_style;
      return;
    }
    sections.reply_style = replyGuidance();
  });
}
