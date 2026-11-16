// The files a conversation changed (`gui-files`), read from its own calls:
// each change a host's edit made, and each file one of the World's runs
// wrote, is one change of one file, made in the call's turn. A turn's
// changes are what is listed under its last words once it is over; all the
// turns' together are the conversation's.
//
// A call's changes are worked out once and kept with the call as it stands:
// a call the conversation hears more of (a run's facts as they come) is a
// new thing to work out, every other is the one already held. So a
// conversation that grows is read only where it grew.

import type { ChangedFile, FileChange, How } from "../files/model";
import { filesOf } from "../files/model";
import type { Tool } from "../world/events";
import type { DiffLine, WriteFact } from "../world/run-facts";
import type { Conversation } from "./conversation";
import { counts, parse, type DiffRow } from "./diff";
import { rowOf } from "./diff-lines";
import type { Entry } from "./model";

type ToolEntry = Extract<Entry, { kind: "tool" }>;

/** Who the conversation is, as its changes name it. */
export interface Maker {
  /** `resident@machine`. */
  session: string;
  agent: string | null;
  /** The machine its agent lives on: where the agent's own edits fall. */
  machine: string;
  /** The lines of a write kept on its machine, where they are not held. */
  kept?: ((write: WriteFact) => Promise<DiffLine[] | null>) | undefined;
}

/** A write's lines as rows, a first mark at the very top left out. */
function rowsOf(lines: readonly DiffLine[]): DiffRow[] {
  const skip = lines[0]?.kind === "hunk" && lines[0].before === 1 ? 1 : 0;
  const rows: DiffRow[] = [];
  for (let index = skip; index < lines.length; index += 1) rows.push(rowOf(lines[index]!, lines[index - 1] ?? null, index === 0));
  return rows;
}

const HOW: Record<WriteFact["kind"], How> = { added: "added", modified: "changed", deleted: "removed" };

export class TalkChanges {
  /** A call's changes, kept while the call stands as it was read. */
  private readonly held = new WeakMap<Tool, FileChange[]>();
  /** What was last put together, and from what. */
  private all: { sign: number; files: ChangedFile[] } | null = null;
  private readonly turns = new Map<string, { sign: number; files: ChangedFile[] }>();

  constructor(
    private readonly conversation: Conversation,
    private readonly maker: Maker,
  ) {}

  /** The files `turn` changed, in the order of their machines and paths. */
  turn(turn: string): ChangedFile[] {
    const tools = this.conversation.toolsOf(turn);
    const sign = tools.reduce((sum, entry) => sum + entry.stamp, tools.length);
    const kept = this.turns.get(turn);
    if (kept?.sign === sign) return kept.files;
    const files = filesOf(tools.flatMap((entry) => this.of(entry)));
    this.turns.set(turn, { sign, files });
    if (this.turns.size > 400) this.turns.delete(this.turns.keys().next().value!);
    return files;
  }

  /** The files the conversation changed, as far as it is held here. */
  whole(): ChangedFile[] {
    const tools = this.conversation.toolsOf(null);
    const sign = tools.reduce((sum, entry) => sum + entry.stamp, tools.length);
    if (this.all?.sign === sign) return this.all.files;
    const files = filesOf(tools.flatMap((entry) => this.of(entry)));
    this.all = { sign, files };
    return files;
  }

  /** The turn that changed a file last, as far as the page holds. */
  last(): string | null {
    let latest: { at: number; turn: string | null } | null = null;
    for (const file of this.whole()) {
      const change = file.changes[0];
      if (change && (!latest || change.at >= latest.at)) latest = { at: change.at, turn: change.turn };
    }
    return latest?.turn ?? null;
  }

  /** A file's changes over the whole conversation, the latest first. */
  file(machine: string, path: string): FileChange[] {
    return this.whole().find((file) => file.machine === machine && file.path === path)?.changes ?? [];
  }

  private of(entry: ToolEntry): FileChange[] {
    const { tool } = entry;
    const held = this.held.get(tool);
    if (held) return held;
    const { maker } = this;
    const out: FileChange[] = [];
    const at = entry.ended?.at ?? entry.began;
    const base = { at, seen: false, session: maker.session, agent: maker.agent, from: null, turn: entry.turn };
    // An edit that failed, or that the user declined, changed nothing.
    const undone = entry.ended?.use === "failed" || entry.ended?.use === "declined";
    if (tool.does.as === "edit" && !undone) {
      tool.does.changes.forEach((change, index) => {
        const counted = counts(change.diff);
        const how: How = change.how === "added" ? "added" : change.how === "deleted" ? "removed" : "changed";
        out.push({ ...base, id: `${entry.key}#${index}`, machine: tool.machine ?? maker.machine, path: change.path, how, added: counted.added, removed: counted.removed, rows: async () => parse(change.diff) });
      });
    }
    for (const write of tool.run?.writes ?? []) {
      const inline = write.diff?.lines;
      const kept = maker.kept;
      const rows = inline ? async () => rowsOf(inline) : write.diff && kept ? async () => kept(write).then((lines) => (lines ? rowsOf(lines) : null)) : undefined;
      out.push({ ...base, id: `w:${write.id}`, machine: write.machine, path: write.path, how: HOW[write.kind], added: write.added ?? 0, removed: write.removed ?? 0, uncounted: write.added === null && write.removed === null, rows });
    }
    this.held.set(tool, out);
    return out;
  }
}
