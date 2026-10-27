// What a stretch's heading says, for the drawn stretch and for finding in
// it alike: while it runs, the step it is on and for how long; once done,
// the two weightiest kinds of thing it did (files changed first, with the
// lines added and removed), how many failed, and how long it took. Its steps
// are its rows, seen when it opens. A stretch that is only thinking says how
// long it thought and the tokens it took, and opens only where there are
// words. One of the host's own agents a call of it started still works on
// after the call: the stretch runs while the agent does.

import type { TallyPart, Words } from "../words";
import { counts } from "./diff";
import { isThinking, runsOf, type Activity, type Entry } from "./model";
import { groupHeading, headline } from "./tool";
import { shortCommand } from "./deeds/rows";

type ToolEntry = Extract<Entry, { kind: "tool" }>;
export type ProseEntry = Extract<Entry, { kind: "prose" }>;

export interface StretchSaid {
  title: string;
  /** The title in the parts a narrow line hides whole, set apart by the
   *  words' `tallyGap`. */
  parts: TallyPart[];
  added: number;
  removed: number;
  /** Its steps: a run of thinking is one. */
  steps: number;
  running: boolean;
  /** Nothing but thinking. */
  only: boolean;
  /** Nothing but thinking, and no words of it to open. */
  bare: boolean;
  /** The thinking's tokens, said, on a stretch that is only thinking. */
  figures: string;
  /** When its first step began and its last ended, in milliseconds. */
  began: number | null;
  ended: number | null;
  /** What its time says now. */
  time: string;
}

export function stretchSays(
  group: Activity,
  words: Words,
  attachment: (path: string) => string | null,
  tokens: (run: readonly ProseEntry[]) => number | null,
  agents: StretchAgents = { of: () => null, named: () => null },
): StretchSaid {
  const tools = group.members.filter((member): member is ToolEntry => member.kind === "tool");
  const thinking = group.members.filter(isThinking);
  const steps = runsOf(group.members).length;
  const writing = thinking.find((member) => member.writing);
  const runningTool = tools.find((tool) => tool.ended === null) ?? (group.agents ? tools.find((tool) => agents.of(tool)?.going) : undefined);
  const running = !!runningTool || !!writing;
  const only = thoughtOnly(group);
  const bare = only && !thinking.some((member) => member.text.trim());
  let title: string;
  let parts: TallyPart[] | null = null;
  let figures = "";
  let added = 0;
  let removed = 0;
  if (runningTool) {
    const tool = runningTool.tool;
    const line = headline(tool, null, words, attachment);
    const object = group.agents && tool.does.as === "agent" && tool.does.act !== "start" ? (agents.named(runningTool) ?? line.object) : line.object;
    // A run doing things through the World says the one it is on.
    // A run doing things elsewhere says the command it is on there.
    const going = (tool.run?.steps ?? []).filter((step) => step.kind === "on" && step.ended === undefined).at(-1);
    const now = going?.kind === "on" ? shortCommand(going.command.split("\n").find((said) => said.trim()) ?? going.command) : null;
    title = now ? `${words.act("command", "running")} ${now}` : [line.action, object].filter(Boolean).join(" ");
  } else if (writing) {
    const last = thoughtHeads([writing]).at(-1);
    title = last ? words.thinkingAbout(last) : words.thinking;
  } else if (only) {
    const took = thoughtTook(thinking);
    title = took === null ? words.thought : words.thoughtFor(took);
    const counted = tokens(thinking);
    if (counted !== null) figures = words.thinkingTokens(counted);
  } else {
    // Two kinds of thing at most, the weightiest; failures always.
    const all = groupHeading(tools, thinking.length, words);
    parts = [...all.filter((part) => part.kind !== "failed").slice(0, KINDS), ...all.filter((part) => part.kind === "failed")];
    title = parts.map((part) => part.said).join(words.tallyGap);
    for (const tool of tools) {
      for (const write of tool.tool.run?.writes ?? []) {
        added += write.added ?? 0;
        removed += write.removed ?? 0;
      }
      if (tool.tool.does.as !== "edit") continue;
      for (const change of tool.tool.does.changes) {
        const counted = counts(change.diff);
        added += counted.added;
        removed += counted.removed;
      }
    }
  }
  // Its clock runs from when its first step began, thinking included.
  let began: number | null = null;
  let last: number | null = null;
  for (const member of group.members) {
    const start = member.kind === "tool" ? member.began : isThinking(member) ? (member.began ?? null) : null;
    if (start !== null && (began === null || start < began)) began = start;
  }
  for (const tool of tools) {
    if (!tool.ended) continue;
    const after = group.agents ? (agents.of(tool)?.ended ?? null) : null;
    const end = after !== null && after > tool.ended.at ? after : tool.ended.at;
    if (last === null || end > last) last = end;
  }
  const ended = !running && tools.length && tools.every((tool) => tool.ended) ? last : null;
  const time = running ? (began === null ? "" : clockSays(began, words)) : only ? figures : began !== null && ended !== null ? words.took(ended - began) : "";
  return { title, parts: parts ?? [{ said: title, kind: "done" }], added, removed, steps, running, only, bare, figures, began, ended, time };
}

/** How an agent one of a stretch's calls started stands. */
export interface StretchAgent {
  going: boolean;
  ended: number | null;
}

/** The host's own agents a stretch's calls start or name: the one a call
 *  started, and the agents a call names, as its row says them. */
export interface StretchAgents {
  of(tool: ToolEntry): StretchAgent | null;
  named(tool: ToolEntry): string | null;
}

/** A running clock: how long it has run, after its steps where they are
 *  counted (an agent's row; a stretch's heading leaves them to its rows). */
export function clockSays(since: number, words: Words, steps?: number): string {
  return [steps === undefined || steps < 2 ? "" : words.steps(steps), words.took(Date.now() - since)].filter(Boolean).join(words.figureGap);
}

/** How many kinds of thing a heading names at most. */
const KINDS = 2;

/** Whether a stretch is nothing but thinking: one run of it, said by its
 *  heading alone. */
export function thoughtOnly(group: Activity): boolean {
  return group.members.every(isThinking);
}

/** The words of a run of thinking, its parts' one after another. */
export function thoughtText(run: readonly ProseEntry[]): string {
  return run
    .map((entry) => entry.text.trim())
    .filter(Boolean)
    .join("\n\n");
}

/** How long a run of thinking took: its parts' together, where the host
 *  said for every one. */
export function thoughtTook(run: readonly ProseEntry[]): number | null {
  let took = 0;
  for (const entry of run) {
    if (entry.took === undefined) return null;
    took += entry.took;
  }
  return took;
}

/** The titles a run of thinking said it was in, its words shown or not. */
export function thoughtHeads(run: readonly ProseEntry[]): string[] {
  return sections(run.map((entry) => entry.text || entry.heads || "").join("\n"));
}

/** The titles a summary of thinking puts its sections under, in bold on a
 *  line of their own. */
export function sections(text: string): string[] {
  return [...text.matchAll(/^\*\*(.+?)\*\*\s*$/gm)].map((match) => match[1]!);
}
