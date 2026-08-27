const BUILTIN_TOOLS = [
	"read",
	"bash",
	"powershell",
	"edit",
	"write",
	"grep",
	"find",
	"ls",
	"eval",
] as const;

/**
 * Applies the host's persisted grok-pi built-in-tool preference at session
 * startup. The host also maps F2-disabled built-in names to Pi's native
 * `--exclude-tools` so those names are absent from the registry/getAllTools;
 * this extension then activates the selected names that are still registered.
 * Non-built-in extension/custom tool names remain untouched.
 *
 * CLI tool restrictions always take priority over F2 preferences:
 * - --no-tools / --no-builtin-tools: extension is not injected at all.
 * - --exclude-tools: PI_GROK_EXCLUDE_TOOLS carries the exclusion list;
 *   excluded builtins are removed from the F2-selected set.
 * - --tools / -t: extension is not injected (authoritative allowlist).
 */
export default function (pi: {
	on(event: "session_start", handler: () => void): void;
	getActiveTools(): string[];
	setActiveTools(toolNames: string[]): void;
}) {
	const configured = process.env.PI_GROK_BUILTIN_TOOLS;
	if (!configured) return;

	const excluded = new Set(
		(process.env.PI_GROK_EXCLUDE_TOOLS ?? "")
			.split(",")
			.map((s) => s.trim())
			.filter(Boolean),
	);

	const selected = configured
		.split(",")
		.filter((name): name is (typeof BUILTIN_TOOLS)[number] =>
			(BUILTIN_TOOLS as readonly string[]).includes(name),
		)
		.filter((name) => !excluded.has(name));

	pi.on("session_start", () => {
		const builtin = new Set<string>(BUILTIN_TOOLS);
		const activeNonBuiltin = pi
			.getActiveTools()
			.filter((name) => !builtin.has(name));
		pi.setActiveTools([...activeNonBuiltin, ...selected]);
	});
}
