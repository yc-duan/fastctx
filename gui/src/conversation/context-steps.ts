// What each step of a conversation added to its context (`world-gui.md`,
// 对话): between two model requests the prompt grows by what came back in
// between — the model's own answer and calls, what the tools it called
// returned, and anything else put in (the user's words, the World's
// notices). The growth is read from the requests the host reported; how it
// splits between the tool calls of that step is estimated by how much each
// returned. Claude Code names the calls each answer made, so a step's calls
// are known. Codex names none, and reports a request only once the calls
// its answer made have returned: a step's calls are those that ended
// between the two requests before it. A prompt that shrinks was compacted.

import { contextOf, type ModelRequest } from "./requests";

/** A tool call as the steps need it: which it is, when it ended, and how
 *  much it returned to the model. */
export interface CallSpan {
  key: string;
  item: string;
  turn: string;
  ended: number | null;
  /** What it returned, in characters: the share of a step it takes. */
  size: number;
}

export type Cause =
  | { kind: "call"; key: string; tokens: number }
  | { kind: "answer"; tokens: number }
  | { kind: "other"; tokens: number };

export interface ContextStep {
  /** The request this step ends at, and where it stands among them. */
  request: ModelRequest;
  index: number;
  /** The context it was answered over. */
  context: number;
  /** How much larger than the request before: negative where it shrank. */
  grew: number;
  /** What the growth was made of, largest first; none where it shrank. */
  causes: Cause[];
  /** The context shrank by more than a tenth: it was compacted. */
  compacted: boolean;
}

/** Each request with what grew its context since the one before; the first
 *  request's whole context is its own beginning. */
export function contextSteps(requests: readonly ModelRequest[], calls: readonly CallSpan[]): ContextStep[] {
  const byItem = new Map(calls.map((call) => [call.item, call]));
  const steps: ContextStep[] = [];
  for (let index = 0; index < requests.length; index += 1) {
    const request = requests[index]!;
    const context = contextOf(request);
    const before = requests[index - 1];
    if (!before) {
      steps.push({ request, index, context, grew: context, causes: context ? [{ kind: "other", tokens: context }] : [], compacted: false });
      continue;
    }
    const grew = context - contextOf(before);
    if (grew <= 0) {
      steps.push({ request, index, context, grew, causes: [], compacted: grew < -0.1 * contextOf(before) });
      continue;
    }
    // The answer before is in the prompt now, as far as it goes; the rest
    // is what came back to it and what else was put in.
    const answer = Math.min(before.output, grew);
    const rest = grew - answer;
    const earlier = requests[index - 2]?.at ?? -Infinity;
    const between = before.items.length
      ? before.items.map((item) => byItem.get(item)).filter((call): call is CallSpan => !!call)
      : calls.filter((call) => call.ended !== null && call.ended > earlier && call.ended <= before.at && (!before.turn || call.turn === before.turn));
    const causes: Cause[] = [];
    if (answer > 0) causes.push({ kind: "answer", tokens: answer });
    const weight = between.reduce((sum, call) => sum + Math.max(1, call.size), 0);
    if (rest > 0 && between.length) {
      let given = 0;
      between.forEach((call, at) => {
        // The last takes what rounding left, so the parts add up.
        const tokens = at === between.length - 1 ? rest - given : Math.round((rest * Math.max(1, call.size)) / weight);
        given += tokens;
        if (tokens > 0) causes.push({ kind: "call", key: call.key, tokens });
      });
    } else if (rest > 0) causes.push({ kind: "other", tokens: rest });
    causes.sort((a, b) => b.tokens - a.tokens);
    steps.push({ request, index, context, grew, causes, compacted: false });
  }
  return steps;
}

/** The calls that grew the context most over `steps`, largest first. */
export function heaviestCalls(steps: readonly ContextStep[], count: number): { key: string; tokens: number; step: number }[] {
  const calls = new Map<string, { key: string; tokens: number; step: number }>();
  for (const step of steps) {
    for (const cause of step.causes) {
      if (cause.kind !== "call") continue;
      const known = calls.get(cause.key);
      if (known) known.tokens += cause.tokens;
      else calls.set(cause.key, { key: cause.key, tokens: cause.tokens, step: step.index });
    }
  }
  return [...calls.values()].sort((a, b) => b.tokens - a.tokens).slice(0, count);
}
