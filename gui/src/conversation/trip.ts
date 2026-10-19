// A World run as the things it did (`world-gui.md`, 对话): one row for each,
// in the order they began, each saying the machine it happened on, rather
// than one row for the script that did them. What the script did on its own
// machine is its own row, named by the first command that does work; a
// script that did nothing but World things has none. An `on` that only
// carried a World command elsewhere (`on lab fastctx view …`, `on lab edit
// …`) is drawn as what it carried. One command sent to many machines is one
// row and the machines it went to. A command whose input flowed straight
// from the row above, a pipe between machines, is hooked to it.

import type { CopyStep, OnStep, RunStep, ViewStep, WriteFact } from "../world/run-facts";

/** One row a run is drawn as. `fed` says its command's input flowed
 *  straight from the row above. */
export type TripItem =
  | { kind: "own" }
  | { kind: "on"; step: OnStep; fed: boolean }
  | { kind: "fan"; steps: OnStep[] }
  | { kind: "copy"; step: CopyStep }
  | { kind: "view"; step: ViewStep }
  | { kind: "write"; write: WriteFact };

/** How many machines one command goes to before its rows are one. */
export const FAN = 4;

/** The words of the World's own commands, which are its rows. */
const WORLD = /^(?:on|fastctx|edit|write|wait)\b/;
/** Commands that only set the scene: where, how, with what. */
const SCENE = /^(?:cd|set|export|unset|shopt|source|\.|trap|local|declare|true|:)\b/;

/** The rows a run with `steps` is drawn as. */
export function tripItems(script: string, steps: readonly RunStep[], writes: readonly WriteFact[]): TripItem[] {
  const ordered = [...steps].sort((a, b) => Date.parse(a.began) - Date.parse(b.began));
  const within = new Map<string | null, RunStep[]>();
  for (const step of ordered) {
    const list = within.get(step.within) ?? [];
    list.push(step);
    within.set(step.within, list);
  }
  const wrote = new Map<string | null, WriteFact[]>();
  for (const write of writes) {
    const at = write.step && ordered.some((step) => step.id === write.step) ? write.step : null;
    const list = wrote.get(at) ?? [];
    list.push(write);
    wrote.set(at, list);
  }
  const items: TripItem[] = [];
  if (ownWork(script) !== null) items.push({ kind: "own" });
  const level = (parent: string | null): void => {
    const here = within.get(parent) ?? [];
    for (let index = 0; index < here.length; index += 1) {
      const step = here[index]!;
      if (step.kind === "copy") {
        items.push({ kind: "copy", step });
        continue;
      }
      if (step.kind === "view") {
        items.push({ kind: "view", step });
        continue;
      }
      // The same command to many machines, one after another: one row.
      let end = index;
      while (end + 1 < here.length && here[end + 1]!.kind === "on" && (here[end + 1] as OnStep).command === step.command && !within.has(here[end + 1]!.id)) end += 1;
      if (end - index + 1 >= FAN && !within.has(step.id)) {
        items.push({ kind: "fan", steps: here.slice(index, end + 1) as OnStep[] });
        index = end;
        continue;
      }
      // An `on` that only carried a World command is drawn as what it did.
      const carried = WORLD.test(step.command.trim()) && (within.has(step.id) || wrote.has(step.id));
      if (!carried) {
        const previous = items[items.length - 1];
        items.push({ kind: "on", step, fed: previous?.kind === "on" && step.fed === previous.step.id });
      }
      level(step.id);
      for (const write of wrote.get(step.id) ?? []) items.push({ kind: "write", write });
    }
  };
  level(null);
  for (const write of wrote.get(null) ?? []) items.push({ kind: "write", write });
  return items;
}

/** What a script did on its own machine, said by the first command of its
 *  own that does work: not a World command, not setting the scene. Null
 *  where it did nothing of its own. The words of here-docs are not its
 *  commands. */
export function ownWork(script: string): string | null {
  const lines = script.split("\n");
  let until: string | null = null;
  for (const raw of lines) {
    const line = raw.trim();
    if (until !== null) {
      if (line === until) until = null;
      continue;
    }
    const heredoc = /<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1/.exec(line);
    if (heredoc) until = heredoc[2]!;
    for (const part of commandsOf(line)) {
      if (!part || part.startsWith("#") || SCENE.test(part) || WORLD.test(part) || /^(?:for|while|until|do|done|if|then|else|fi|case|esac)\b/.test(part)) continue;
      // A pipe into the World's commands is theirs.
      if (/\|\s*(?:on|fastctx|edit|write)\b/.test(part)) continue;
      return part.replace(/\s+\\$/, "");
    }
  }
  return null;
}

/** The commands of one line, as `&&`, `||` and `;` part them. */
function commandsOf(line: string): string[] {
  return line
    .split(/\s*(?:&&|\|\||;)\s*/)
    .map((part) => part.trim())
    .filter(Boolean);
}

/** A command as its row names it: its first line, a leading `cd …` set
 *  aside. */
export function commandSaid(command: string): string {
  const first = command.split("\n").find((line) => line.trim()) ?? command;
  const parts = commandsOf(first.trim());
  const work = parts.find((part) => !SCENE.test(part));
  return (work ?? parts[0] ?? first).replace(/\s+\\$/, "");
}

/** A path's last part. */
export function leaf(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut < 0 ? trimmed : trimmed.slice(cut + 1);
}
