import type { JsonValue } from "../world/wire";

/** What the host's own usage report says beyond the plain counts: the
 *  context by part, as Claude Code's /context lists it. Absent where the
 *  report does not carry it; what each request cost is read from the
 *  conversation's own events (`requests.ts`). */
export interface UsageDetail {
  parts: { name: string; tokens: number }[];
}

type Fields = { [key: string]: JsonValue };

function fields(value: JsonValue | undefined): Fields | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : null;
}

function count(value: JsonValue | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

export function usageDetail(raw: JsonValue | undefined): UsageDetail {
  const report = fields(raw);
  const parts: UsageDetail["parts"] = [];
  const breakdown = fields(fields(report?.context)?.breakdown);
  // Only what occupies the window: free space, the compaction reserve and
  // schemas loaded on demand are not part of what is used.
  for (const row of Array.isArray(breakdown?.categories) ? breakdown.categories : []) {
    const category = fields(row);
    const tokens = count(category?.tokens);
    if (category?.kind === "used" && typeof category.name === "string" && tokens) parts.push({ name: category.name, tokens });
  }
  return { parts };
}
