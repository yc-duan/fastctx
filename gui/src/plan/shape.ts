// What a plan says in brief, for its mark on the conversation's side: its
// checklist, the stops of its first flow, and the questions it waits on. A
// plan need have none of them; the plan itself is read whole in its own page.

import { stops, type Stop } from "./flow";
import { readAsk, readFlow } from "./source";

export interface PlanShape {
  /** Its checklist's items, in order, outside its pictures and code. */
  checks: { text: string; done: boolean }[];
  stops: Stop[];
  /** The questions the plan asks the user, each still to be answered. */
  asks: string[];
}

const FENCE = /^```(\w*)[^\n]*\n([\s\S]*?)^```/gm;
/** A checklist's item: `- [ ] step`, `- [x] step`, or numbered, `1. [x] step`. */
const CHECK = /^\s*(?:[-*+]|\d+[.)])\s+\[([ xX])\]\s+(.+)$/gm;

export function planShape(source: string): PlanShape {
  let route: Stop[] = [];
  const asks: string[] = [];
  for (const [, kind, text = ""] of source.matchAll(FENCE)) {
    if (kind === "flow" && route.length === 0) route = stops(readFlow(text));
    else if (kind === "ask") {
      const question = readAsk(text).question;
      if (question) asks.push(question);
    }
  }
  const checks = [...source.replace(FENCE, "").matchAll(CHECK)].map(([, mark = "", text = ""]) => ({ text: text.trim(), done: mark !== " " }));
  return { checks, stops: route, asks };
}
