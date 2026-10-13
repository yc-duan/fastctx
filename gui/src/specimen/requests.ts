// What each model request of a specimen conversation cost, as its host would
// tell it: Claude Code's rows of its session file, or Codex's usage after
// every response. The cache serves nearly all of each prompt; the first
// request writes it all, one after a long pause finds it gone, and a
// compaction starts it again smaller. The last answer came a few minutes
// ago, so Claude's cache is still warm.

import type { SessionEvent } from "../world/events";

interface Shape {
  agent: "claude" | "codex";
  count: number;
  /** The context of the first request, and of the last. */
  from: number;
  to: number;
  /** The requests after which the cache was found gone, and after which
   *  the conversation was compacted to `compacted` of what it was. */
  cold: number[];
  compact: number;
  compacted: number;
  turn: string;
}

const MINUTE = 60_000;

export function requestEvents(shape: Shape, now = Date.now()): SessionEvent[] {
  let seed = shape.count * 7919 + shape.from;
  const random = () => {
    seed = (seed * 16807) % 2147483647;
    return seed / 2147483647;
  };
  // Back from a few minutes ago, a request every minute or two, with an
  // hour and a quarter of silence before each time the cache went cold.
  const times: number[] = [];
  let at = now - 3 * MINUTE;
  for (let index = shape.count - 1; index >= 0; index -= 1) {
    times.unshift(at);
    at -= (shape.cold.includes(index) ? 75 : 0.6 + random() * 1.8) * MINUTE;
  }
  const events: SessionEvent[] = [];
  let total = 0;
  let previous = 0;
  for (let index = 0; index < shape.count; index += 1) {
    const share = index / Math.max(1, shape.count - 1);
    let context = Math.round(shape.from + (shape.to - shape.from) * share ** 1.3 + random() * 900);
    if (index > shape.compact) context = Math.round(context * shape.compacted);
    const output = Math.round(300 + random() * 1600);
    // What is new since the last request is written; the rest is read.
    const cold = index === 0 || shape.cold.includes(index - 1) || index === shape.compact + 1;
    const grown = Math.max(40, context - previous);
    const write = cold ? Math.round(context * 0.97) : Math.min(context, Math.round(grown * (0.6 + random() * 0.5)));
    const fresh = cold ? context - write : Math.round(10 + random() * 60);
    const read = Math.max(0, context - write - fresh);
    previous = context;
    total += context + output;
    const when = new Date(times[index]!).toISOString();
    if (shape.agent === "claude") {
      events.push({
        at: when,
        event: "changed",
        turn: shape.turn,
        change: {
          kind: "message_usage",
          items: [],
          response: `msg_cache_${index}`,
          model: "claude-opus-5-5",
          api_error: false,
          usage: { input_tokens: fresh, cache_read_input_tokens: read, cache_creation_input_tokens: write, cache_creation: { ephemeral_1h_input_tokens: write, ephemeral_5m_input_tokens: 0 }, output_tokens: output },
        },
      });
    } else {
      events.push({
        at: when,
        event: "changed",
        turn: shape.turn,
        change: {
          kind: "usage",
          usage: {
            total: { totalTokens: total },
            last: { inputTokens: context, cachedInputTokens: read, outputTokens: output, reasoningOutputTokens: Math.round(output * 0.4), totalTokens: context + output },
            modelContextWindow: 258_400,
          },
        },
      });
    }
  }
  return events;
}
