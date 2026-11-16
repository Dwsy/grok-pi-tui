/** Runtime event normalization and bounded queue policy for mcpeval. */
import { randomUUID } from "node:crypto";

export const MAX_EVENT_QUEUE_PER_SUBSCRIPTION = 64;
export const MAX_EVENT_QUEUE_GLOBAL = 256;

export type McpevalEvent = {
  eventId?: string;
  timestamp?: string;
  nativeType: string;
  serviceId?: string;
  threadId?: string;
  native?: Record<string, unknown>;
  [key: string]: unknown;
};

export function eventMatches(filter: Record<string, unknown>, event: McpevalEvent): boolean {
  for (const key of ["serviceId", "threadId", "nativeType"]) {
    const expected = filter[key];
    if (typeof expected === "string" && expected && event[key] !== expected) return false;
  }
  const nativeTypes = filter.nativeTypes;
  return !Array.isArray(nativeTypes) || nativeTypes.length === 0 || nativeTypes.includes(event.nativeType);
}

export function normalizeEvent(event: McpevalEvent): McpevalEvent {
  return {
    ...event,
    eventId: typeof event.eventId === "string" ? event.eventId : randomUUID(),
    timestamp: typeof event.timestamp === "string" ? event.timestamp : new Date().toISOString(),
    nativeType: event.nativeType,
    ...(typeof event.serviceId === "string" ? { serviceId: event.serviceId } : {}),
    ...(typeof event.threadId === "string" ? { threadId: event.threadId } : {}),
    ...(event.native && typeof event.native === "object" ? { native: event.native } : {}),
  };
}
