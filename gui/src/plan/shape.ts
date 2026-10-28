// What a plan says in brief, for the conversation's overview: its title, its
// checklist, the stops of its first flow, and the questions it waits on. The
// plan itself is read whole in its own page.

import { stops, type Stop } from "./flow";
import { readAsk, readFlow } from "./source";

export interface PlanShape {
  title: string | null;
  /** Its checklist's items, in order, outside its pictures. */
  checks: { text: string; done: boolean }[];
  stops: Stop[];
  /** The questions the plan asks the user, each still to be answered. */
  asks: string[];
}

const FENCE = /^```(\w+)[^\n]*\n([\s\S]*?)^```/gm;
/** A checklist's item: `- [ ] step`, `- [x] step`. */
const CHECK = /^\s*[-*+]\s+\[([ xX])\]\s+(.+)$/gm;

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
  const title = /^#{1,3}\s+(.+)$/m.exec(source)?.[1]?.trim() ?? null;
  const checks = [...source.replace(FENCE, "").matchAll(CHECK)].map(([, mark = "", text = ""]) => ({ text: text.trim(), done: mark !== " " }));
  return { title, checks, stops: route, asks };
}
