export function cleanRecapMarkdown(raw: string): string {
	let text = raw.trim();
	// Only strip a trailing fence when the response was wrapped in a
	// ```markdown block — otherwise the trailing ``` is the closing fence of
	// the last code block (e.g. mermaid) and stripping it leaves the fence
	// unclosed, which makes the renderer fall back to raw source display.
	if (/^```markdown\s/i.test(text)) {
		text = text.replace(/^```markdown\s*/i, "").replace(/\s*```$/i, "").trim();
	}
	text = text.replace(/^(session\s+)?recap\s*[:—-]\s*/i, "").trim();
	return text.length > 5000 ? `${text.slice(0, 5000).trimEnd()}…` : text;
}

export function cleanRecapText(raw: string): string {
	let text = raw.trim();
	// Strip common wrappers / prefixes.
	text = text.replace(/^["'`]+|["'`]+$/g, "").trim();
	text = text.replace(/^(session\s+)?recap\s*[:—-]\s*/i, "").trim();
	// Collapse whitespace / keep one paragraph.
	text = text.replace(/\s+/g, " ").trim();
	if (text.length > 1200) {
		text = text.slice(0, 1200).trim();
	}
	return text;
}
