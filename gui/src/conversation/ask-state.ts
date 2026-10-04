// What the conversation's model needs of a request, apart from drawing it:
// which requests a person answers, a request's identity, and how it stands.

import type { JsonValue } from "../world/wire";

/** Requests the node answers itself, never a person. */
const machineOnly = new Set(["account/chatgptAuthTokens/refresh", "attestation/generate", "item/tool/call", "currentTime/read"]);

/** Whether a request is one a person answers. */
export function forPeople(request: string): boolean {
  return !machineOnly.has(request);
}

/** A request's id as a key: the same id however its keys were ordered. */
export function askKey(id: JsonValue): string {
  return JSON.stringify(sorted(id));
}

function sorted(value: JsonValue): JsonValue {
  if (Array.isArray(value)) return value.map(sorted);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sorted((value as Record<string, JsonValue>)[key]!)]));
  }
  return value;
}

/** How a request stands. While an answer is on its way the card says so
 *  itself; the request is settled only once the host has it. */
export type AskState = { state: "waiting" } | { state: "answered"; said: string } | { state: "gone" };
