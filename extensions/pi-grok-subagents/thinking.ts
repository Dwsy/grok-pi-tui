/**
 * Pi thinking-level capability mirror.
 *
 * Pi exposes `getSupportedThinkingLevels` / `clampThinkingLevel` only inside
 * `@earendil-works/pi-ai`, which an extension cannot import without dragging in
 * every provider. This module reimplements that pure data rule from `Model`
 * fields so preset validation and the team TUI can offer exactly the levels Pi
 * will accept. Keep it in sync with `pi-main/packages/ai/src/models.ts`
 * (`EXTENDED_THINKING_LEVELS`, `getSupportedThinkingLevels`,
 * `clampThinkingLevel`).
 */

export const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"] as const;

export type ThinkingLevel = (typeof THINKING_LEVELS)[number];

/** The subset of `Model` this module reads. */
export type ThinkingCapableModel = {
  reasoning: boolean;
  thinkingLevelMap?: Partial<Record<ThinkingLevel, string | null>>;
};

export function isThinkingLevel(value: unknown): value is ThinkingLevel {
  return typeof value === "string" && (THINKING_LEVELS as readonly string[]).includes(value);
}

/** Levels the model actually accepts, in Pi's canonical order. */
export function supportedThinkingLevels(model: ThinkingCapableModel | undefined): ThinkingLevel[] {
  if (!model?.reasoning) return ["off"];
  return THINKING_LEVELS.filter((level) => {
    // A null mapping explicitly marks the level unsupported. `xhigh`/`max`
    // also require an explicit mapping; the rest fall back to provider defaults.
    if (model.thinkingLevelMap?.[level] === null) return false;
    if (level === "xhigh" || level === "max") return model.thinkingLevelMap?.[level] !== undefined;
    return true;
  });
}

/** Clamp a requested level to what the model supports, matching Pi's search order. */
export function clampThinkingLevel(model: ThinkingCapableModel | undefined, level: ThinkingLevel): ThinkingLevel {
  const available = supportedThinkingLevels(model);
  if (available.includes(level)) return level;
  const requested = THINKING_LEVELS.indexOf(level);
  if (requested === -1) return available[0] ?? "off";
  for (let index = requested; index < THINKING_LEVELS.length; index += 1) {
    const candidate = THINKING_LEVELS[index];
    if (available.includes(candidate)) return candidate;
  }
  for (let index = requested - 1; index >= 0; index -= 1) {
    const candidate = THINKING_LEVELS[index];
    if (available.includes(candidate)) return candidate;
  }
  return available[0] ?? "off";
}

/** Parse an untrusted `thinking_level` value. Undefined means "inherit Pi settings". */
export function parseThinkingLevel(value: unknown, context: string): ThinkingLevel | undefined {
  if (value === undefined || value === null) return undefined;
  const text = typeof value === "string" ? value.trim().toLowerCase() : "";
  if (!text || text === "inherit" || text === "default") return undefined;
  if (!isThinkingLevel(text)) {
    throw new Error(`${context}: thinking_level must be one of ${THINKING_LEVELS.join(", ")} (got ${JSON.stringify(value)})`);
  }
  return text;
}

/** Human label for TUI rows. */
export function thinkingLevelLabel(level: ThinkingLevel | undefined): string {
  return level ?? "inherit Pi settings";
}
