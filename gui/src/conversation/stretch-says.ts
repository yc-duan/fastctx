// What a stretch's heading says, for the drawn stretch and for finding in
// it alike: while it runs, the step it is on and its clock; once done, what
// it did, the lines it changed and how long it took. A stretch that is only
// thinking says how long it thought and the tokens it took, and opens only
// where there are words. One of the host's own agents a call of it started
// still works on after the call: the stretch runs while the agent does.

import type { Words } from "../words";
import { counts } from "./diff";
import { isThinking, runsOf, type Activity, type Entry } from "./model";
import { groupHeading, headline } from "./tool";

type ToolEntry = Extract<Entry, { kind: "tool" }>;
export type ProseEntry = Extract<Entry, { kind: "prose" }>;

export interface StretchSaid {
  title: string;
  /** The title in the parts a narrow line hides whole, set apart by the
   *  words' `tallyGap`. */
  parts: string[];
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
  let parts: string[] | null = null;
  let figures = "";
  let added = 0;
  let removed = 0;
  if (runningTool) {
    const line = headline(runningTool.tool, null, words, attachment);
    const object = group.agents && runningTool.tool.does.as === "agent" && runningTool.tool.does.act !== "start" ? (agents.named(runningTool) ?? line.object) : line.object;
    title = [line.action, object].filter(Boolean).join(" ");
  } else if (writing) {
    const last = sections(writing.text).at(-1);
    title = last ? words.thinkingAbout(last) : words.thinking;
  } else if (only) {
    const took = thoughtTook(thinking);
    title = took === null ? words.thought : words.thoughtFor(took);
    const counted = tokens(thinking);
    if (counted !== null) figures = words.thinkingTokens(counted);
  } else {
    parts = groupHeading(tools, thinking.length, words);
    title = parts.join(words.tallyGap);
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
  const time = running
    ? began === null ? "" : clockSays(steps, began, words)
    : only
      ? figures
      : [steps < 2 ? "" : words.steps(steps), began !== null && ended !== null ? words.took(ended - began) : ""].filter(Boolean).join(words.figureGap);
  return { title, parts: parts ?? [title], added, removed, steps, running, only, bare, figures, began, ended, time };
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

/** A running stretch's clock: its steps when more than one, and how long. */
export function clockSays(steps: number, since: number, words: Words): string {
  return [steps < 2 ? "" : words.steps(steps), words.took(Date.now() - since)].filter(Boolean).join(words.figureGap);
}

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

/** The titles a summary of thinking puts its sections under, in bold on a
 *  line of their own. */
export function sections(text: string): string[] {
  return [...text.matchAll(/^\*\*(.+?)\*\*\s*$/gm)].map((match) => match[1]!);
}
