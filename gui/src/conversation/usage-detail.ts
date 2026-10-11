import type { JsonValue } from "../world/wire";

/** What the host's own usage report says beyond the plain counts: the
 *  context by part (Claude Code's breakdown, as its /context lists it) and
 *  the last request's tokens (Claude Code's `apiUsage`, Codex's
 *  `tokenUsage.last`). Absent where the report does not carry them. */
export interface UsageDetail {
  parts: { name: string; tokens: number }[];
  last: { input: number; cached: number; output: number; reasoning: number | null } | null;
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
  let last: UsageDetail["last"] = null;
  const breakdown = fields(fields(report?.context)?.breakdown);
  if (breakdown) {
    // Only what occupies the window: free space, the compaction reserve and
    // schemas loaded on demand are not part of what is used.
    for (const row of Array.isArray(breakdown.categories) ? breakdown.categories : []) {
      const category = fields(row);
      const tokens = count(category?.tokens);
      if (category?.kind === "used" && typeof category.name === "string" && tokens) parts.push({ name: category.name, tokens });
    }
    const api = fields(breakdown.apiUsage);
    const uncached = count(api?.input_tokens);
    const read = count(api?.cache_read_input_tokens);
    const written = count(api?.cache_creation_input_tokens);
    const output = count(api?.output_tokens);
    if (uncached !== null && read !== null && written !== null && output !== null) {
      last = { input: uncached + read + written, cached: read, output, reasoning: null };
    }
  }
  const codex = fields(fields(report?.tokenUsage)?.last);
  if (codex) {
    // Codex counts cached input within its input, and reasoning within its
    // output.
    const input = count(codex.inputTokens);
    const cached = count(codex.cachedInputTokens);
    const output = count(codex.outputTokens);
    if (input !== null && cached !== null && output !== null) {
      last = { input, cached, output, reasoning: count(codex.reasoningOutputTokens) };
    }
  }
  return { parts, last };
}
