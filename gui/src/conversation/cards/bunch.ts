// Things a stretch did, gathered where many go by alike (`world-gui.md`,
// 对话): the same thing done again and again with nothing between (a log
// looked at every minute, a job waited on) is one row, "×6"; three or more
// of a kind in a row (files changed, files read, searches, commands) are one
// row that opens on them, a file changed several times among them counted
// once, its lines added and removed summed. Nothing else between them may
// part: a thought, a word of the model's, a row of another kind keeps its
// place, so what came first still comes first.

import type { Card } from "./cards";

export type Family = "looks" | "changes" | "commands";

/** Things gathered into one row: a kind of thing in a row, or one thing
 *  done again. */
export interface Bunch {
  kind: "bunch";
  key: string;
  /** `again` for one thing done again and again. */
  family: Family | "again";
  /** What it holds, in order: things, or one thing done again. */
  parts: (Card | Bunch)[];
}

const FAMILY: Partial<Record<Card["kind"], Family>> = {
  read: "looks",
  search: "looks",
  tree: "looks",
  history: "looks",
  web: "looks",
  change: "changes",
  command: "commands",
};

/** How many of a kind in a row are gathered. */
const KIND = 3;
/** How many times one thing done again are gathered. */
const AGAIN = 2;

/** `parts` with what goes by alike gathered; what is not a card stands
 *  where it was. */
export function bunched<T>(parts: readonly (T | Card)[], isCard: (part: T | Card) => part is Card): (T | Card | Bunch)[] {
  const out: (T | Card | Bunch)[] = [];
  let run: Card[] = [];
  const flush = () => {
    if (run.length >= KIND) {
      const family = FAMILY[run[0]!.kind]!;
      const merged = family === "changes" ? byFile(run) : run;
      const parts = again(merged);
      // A run that is all one thing done again is that, not a kind.
      out.push(parts.length === 1 && parts[0]!.kind === "bunch" ? parts[0]! : { kind: "bunch", key: `${run[0]!.key}*`, family, parts });
    } else out.push(...again(run));
    run = [];
  };
  for (const part of parts) {
    const family = isCard(part) ? FAMILY[part.kind] : undefined;
    if (run.length && (!family || FAMILY[run[0]!.kind] !== family)) flush();
    if (family) run.push(part as Card);
    else {
      flush();
      if (isCard(part)) out.push(...again([part]));
      else out.push(part);
    }
  }
  flush();
  return out;
}

/** One thing done again and again with nothing between, as one. */
function again(cards: readonly Card[]): (Card | Bunch)[] {
  const out: (Card | Bunch)[] = [];
  let index = 0;
  while (index < cards.length) {
    const first = cards[index]!;
    let end = index + 1;
    while (end < cards.length && sameThing(first, cards[end]!)) end += 1;
    if (end - index >= AGAIN) out.push({ kind: "bunch", key: `${first.key}*`, family: "again", parts: cards.slice(index, end) });
    else out.push(first);
    index = end;
  }
  return out;
}

/** Whether two things are one thing done again: the same command on the
 *  same machine, the same file read, the same search. */
function sameThing(a: Card, b: Card): boolean {
  if (a.kind !== b.kind || a.machine !== b.machine) return false;
  switch (a.kind) {
    case "command":
      return b.kind === "command" && a.command === b.command;
    case "read":
      return b.kind === "read" && a.path === b.path;
    case "search":
      return b.kind === "search" && a.pattern === b.pattern;
    default:
      return false;
  }
}

/** Changes, a file changed more than once among them counted once, where
 *  it was first changed: its lines summed, what made it all its own. */
function byFile(cards: readonly Card[]): Card[] {
  const out: Card[] = [];
  const seen = new Map<string, Extract<Card, { kind: "change" }>>();
  for (const card of cards) {
    if (card.kind !== "change") {
      out.push(card);
      continue;
    }
    const key = `${card.machine ?? ""}|${card.path}`;
    const held = seen.get(key);
    if (!held) {
      const own = { ...card, sources: [...card.sources] };
      seen.set(key, own);
      out.push(own);
      continue;
    }
    held.sources.push(...card.sources);
    held.added = held.added !== null && card.added !== null ? held.added + card.added : null;
    held.removed = held.removed !== null && card.removed !== null ? held.removed + card.removed : null;
    held.how = held.how === "added" && card.how !== "deleted" ? "added" : card.how;
    held.running ||= card.running;
    held.failed ||= card.failed;
  }
  return out;
}

/** The things a bunch holds, each once. */
export function cardsIn(bunch: Bunch): Card[] {
  return bunch.parts.flatMap((part) => (part.kind === "bunch" ? cardsIn(part) : [part]));
}
