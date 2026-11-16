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
//
// The page holds only some of a long conversation's calls. The machine it
// lives on keeps every change beside its timeline (`conversation.files`),
// read from where the last reading stopped, as the conversation opens and
// whenever its calls moved, a while apart: those the page does not hold
// come from there, named as the page names them, so the two join. One of
// an earlier release keeps none: the page's own calls are all there is.

import type { ChangedFile, FileChange, How } from "../files/model";
import { filesOf } from "../files/model";
import type { Tool } from "../world/events";
import type { JsonValue } from "../world/wire";
import type { DiffLine, DiffRef, WriteFact } from "../world/run-facts";
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
const kindOf = (how: How): WriteFact["kind"] => (how === "added" ? "added" : how === "removed" ? "deleted" : "modified");

/** The page's changes joined by the kept ones: those the page does not
 *  hold are added; a host's edit the page holds is told by the kept record,
 *  whole where the page may hold a shortened stand-in of its call, with the
 *  page's lines where the record keeps none. A run's write is the page's,
 *  told as its facts come, ahead of the record. */
function joined(held: FileChange[], kept: Iterable<FileChange>): FileChange[] {
  const index = new Map<string, number>();
  held.forEach((change, at) => index.set(change.id, at));
  const out = held.slice();
  for (const change of kept) {
    const at = index.get(change.id);
    if (at === undefined) out.push(change);
    else if (change.id.startsWith("i:")) out[at] = { ...change, rows: change.rows ?? out[at]!.rows };
  }
  return out;
}

/** How long apart the kept changes are read again as the calls move. */
const PULL_APART_MS = 10_000;
/** The most readings one pull makes. */
const PULL_PAGES = 64;

export class TalkChanges {
  /** A call's changes, kept while the call stands as it was read. */
  private readonly held = new WeakMap<Tool, FileChange[]>();
  /** What was last put together, and from what. */
  private all: { sign: string; files: ChangedFile[] } | null = null;
  private readonly turns = new Map<string, { sign: string; files: ChangedFile[] }>();
  /** The changes the machine it lives on keeps, by name and by turn, and
   *  where their reading stopped. */
  private readonly kept = new Map<string, FileChange>();
  private readonly keptTurns = new Map<string, Map<string, FileChange>>();
  private keptVersion = 0;
  private through = "";
  private pulling = false;
  private pullTimer: ReturnType<typeof setTimeout> | null = null;
  private pulledAt = 0;
  /** The calls as they stood when the kept changes were last read. */
  private pulledSign: number | null = null;
  /** Read again a while on, whatever the calls: the last reading failed,
   *  stopped short, or found more (kept as the history is made). */
  private again = false;
  private closed = false;

  constructor(
    private readonly conversation: Conversation,
    private readonly maker: Maker,
  ) {}

  /** Reads the kept changes again once the calls moved, a while apart;
   *  `drawn` once something new came. */
  want(drawn: () => void): void {
    if (this.closed || this.pullTimer || this.pulling) return;
    if (!this.again && this.callsSign() === this.pulledSign) return;
    const wait = this.pulledAt === 0 ? 400 : Math.max(0, this.pulledAt + PULL_APART_MS - Date.now());
    this.pullTimer = setTimeout(() => {
      this.pullTimer = null;
      this.pulledSign = this.callsSign();
      this.again = false;
      void this.pull().then(({ changed, again }) => {
        this.pulledAt = Date.now();
        this.again = again;
        if (this.closed) return;
        if (changed) drawn();
        // The calls moved while it read, or it is to read again.
        this.want(drawn);
      });
    }, wait);
  }

  private callsSign(): number {
    const tools = this.conversation.toolsOf(null);
    return tools.reduce((sum, entry) => sum + entry.stamp, tools.length);
  }

  /** Stops reading the kept changes. */
  close(): void {
    this.closed = true;
    if (this.pullTimer) clearTimeout(this.pullTimer);
    this.pullTimer = null;
  }

  /** The kept changes written since the last reading: whether any came, and
   *  whether to read again a while on. */
  private async pull(): Promise<{ changed: boolean; again: boolean }> {
    if (this.pulling) return { changed: false, again: false };
    this.pulling = true;
    let changed = false;
    let again = false;
    try {
      let page = 0;
      for (; page < PULL_PAGES; page += 1) {
        const reading = this.conversation.files(this.through);
        if (!reading) break;
        const told = await reading;
        // None kept there: what is held stays as it was read.
        if (!told.known) break;
        if (told.reread) {
          if (this.through === "") break;
          this.kept.clear();
          this.keptTurns.clear();
          this.through = "";
          changed = true;
          continue;
        }
        for (const { item, change } of told.changes) {
          this.forget(item);
          const made = change === undefined ? null : this.keptChange(item, change);
          if (made) this.keep(made);
          changed = true;
        }
        this.through = told.through;
        if (!told.more) break;
      }
      // Stopped short of the end, or more may come as its history is kept.
      again = page >= PULL_PAGES || changed;
    } catch {
      // Not now: the page's own calls stand, and it reads again a while on.
      again = true;
    } finally {
      this.pulling = false;
    }
    if (changed) this.keptVersion += 1;
    return { changed, again };
  }

  private keep(change: FileChange): void {
    this.kept.set(change.id, change);
    if (change.turn === null) return;
    let turn = this.keptTurns.get(change.turn);
    if (!turn) this.keptTurns.set(change.turn, (turn = new Map()));
    turn.set(change.id, change);
  }

  private forget(id: string): void {
    const change = this.kept.get(id);
    if (!change) return;
    this.kept.delete(id);
    if (change.turn !== null) this.keptTurns.get(change.turn)?.delete(id);
  }

  /** A kept change as the page shows one; none where it says too little. */
  private keptChange(item: string, told: JsonValue): FileChange | null {
    if (!told || typeof told !== "object" || Array.isArray(told)) return null;
    const { maker } = this;
    const path = typeof told.path === "string" && told.path ? told.path : null;
    const how = told.how === "added" || told.how === "changed" || told.how === "removed" ? told.how : null;
    if (!path || !how) return null;
    const added = typeof told.added === "number" ? told.added : null;
    const removed = typeof told.removed === "number" ? told.removed : null;
    const machine = typeof told.machine === "string" && told.machine ? told.machine : maker.machine;
    const write = told.source === "write";
    let rows: FileChange["rows"];
    let counted: { added: number; removed: number } | null = null;
    const diff = told.diff;
    if (!write && typeof diff === "string") {
      rows = async () => parse(diff);
      // Counted as the page counts a call's diff, so the two agree.
      counted = counts(diff);
    } else if (write && diff && typeof diff === "object" && !Array.isArray(diff)) {
      const ref = diff as unknown as DiffRef;
      const kept = maker.kept;
      if (Array.isArray(ref.lines)) {
        const lines = ref.lines;
        rows = async () => rowsOf(lines);
      } else if (kept && typeof ref.machine === "string" && typeof ref.path === "string") {
        const fact: WriteFact = { id: item.replace(/^w:/, ""), machine, path, kind: kindOf(how), added, removed, diff: ref };
        rows = async () => kept(fact).then((lines) => (lines ? rowsOf(lines) : null));
      }
    }
    return {
      id: item,
      at: typeof told.at === "number" ? told.at : 0,
      machine,
      path,
      how,
      added: counted?.added ?? added ?? 0,
      removed: counted?.removed ?? removed ?? 0,
      uncounted: write && added === null && removed === null,
      seen: false,
      session: maker.session,
      agent: maker.agent,
      from: null,
      turn: typeof told.turn === "string" ? told.turn : null,
      rows,
    };
  }

  /** The files `turn` changed, in the order of their machines and paths. */
  turn(turn: string): ChangedFile[] {
    const tools = this.conversation.toolsOf(turn);
    const sign = `${tools.reduce((sum, entry) => sum + entry.stamp, tools.length)}:${this.keptVersion}`;
    const kept = this.turns.get(turn);
    if (kept?.sign === sign) return kept.files;
    const files = filesOf(joined(tools.flatMap((entry) => this.of(entry)), this.keptTurns.get(turn)?.values() ?? []));
    this.turns.set(turn, { sign, files });
    if (this.turns.size > 400) this.turns.delete(this.turns.keys().next().value!);
    return files;
  }

  /** The files the conversation changed: those of the calls the page
   *  holds, and those kept where it lives of the calls it does not. */
  whole(): ChangedFile[] {
    const tools = this.conversation.toolsOf(null);
    const sign = `${tools.reduce((sum, entry) => sum + entry.stamp, tools.length)}:${this.keptVersion}`;
    if (this.all?.sign === sign) return this.all.files;
    const files = filesOf(joined(tools.flatMap((entry) => this.of(entry)), this.kept.values()));
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
