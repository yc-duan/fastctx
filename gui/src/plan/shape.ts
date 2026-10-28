// What a plan says in brief, for a drawing of the conversation it belongs
// to (`side/map.ts`): its title, the stops of its first flow with where the
// work stands among them, and the questions it waits on. The plan itself is
// read whole in its own page.

import { stops, type Stop } from "./flow";
import { readAsk, readFlow } from "./source";

export interface PlanShape {
  title: string | null;
  stops: Stop[];
  /** The questions the plan asks the user, each still to be answered. */
  asks: string[];
}

const FENCE = /^```(\w+)[^\n]*\n([\s\S]*?)^```/gm;

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
  return { title, stops: route, asks };
}
