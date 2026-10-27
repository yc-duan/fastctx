// What a stretch did, one thing at a time, in the order it was done
// (`world-gui.md`, 对话): each command of a run, each file a run wrote,
// each change a host's edit made, each file read, each search, each copy
// between machines. A test run carries what its build warned of; a commit
// carries the push that sent it right after. Where each came from (the
// call, and the deeds read from it) stays with it, to be opened.

import { counts } from "../diff";
import { ownCommands, scriptCommands, wordsOf } from "../deeds/script";
import type { ChecksDeed, CommitDeed, CopyDeed, Deed, FanDeed, FileDeed, JobDeed, LogDeed, OnSaid, PageDeed, PictureDeed, ProjectDeed, PushDeed, StatusDeed, StopDeed, TestsDeed, WorldDeed } from "../deeds/types";
import { troubled } from "../deeds/fan";
import type { Entry } from "../model";
import { viewing } from "../tool";
import type { ChangeHow, Tool } from "../../world/events";

/** Where a card's thing came from: a deed read from a run, a file a run
 *  wrote, a change a host's edit made, or a call as a whole. */
export type Source = { entry: string; deed: number } | { entry: string; write: string } | { entry: string; change: number } | { entry: string };

export type FileHow = "added" | "modified" | "deleted" | "moved";

/** A file on a machine, as a copy names it. */
export interface Place {
  machine: string | null;
  path: string;
}

interface Base {
  key: string;
  /** Where it happened, where the call says. */
  machine: string | null;
  sources: Source[];
  running: boolean;
  failed: boolean;
}

export type Look = Base &
  (
    | { kind: "read"; path: string; deed: FileDeed | null }
    | { kind: "search"; pattern: string; hits: number | null; files: number | null }
    | { kind: "tree"; deed: StatusDeed }
    | { kind: "history"; deed: LogDeed }
  );

export type Card =
  | Look
  | (Base &
      (
        | { kind: "tests"; deed: TestsDeed; warnings: number; errors: number }
        | { kind: "check"; deed: ChecksDeed }
        | { kind: "commit"; deed: CommitDeed; push: PushDeed | null }
        | { kind: "push"; deed: PushDeed }
        /** `on` is what the `on` that carried it told; `commands`, where
         *  there are more than one, the commands of one scope whose output
         *  cannot be told apart, gathered on one row. */
        | { kind: "command"; command: string; commands?: string[]; exit: number | null; on: OnSaid | null }
        | { kind: "fan"; deed: FanDeed }
        | { kind: "copy"; from: Place[]; to: Place; deed: CopyDeed | null }
        | { kind: "job"; deed: JobDeed; of: string | null }
        | { kind: "stop"; deed: StopDeed; of: string | null }
        | { kind: "page"; deed: PageDeed }
        | { kind: "project"; deed: ProjectDeed }
        | { kind: "picture"; deeds: PictureDeed[] }
        | { kind: "world"; deed: WorldDeed }
        | { kind: "change"; path: string; how: FileHow; added: number | null; removed: number | null }
        | { kind: "web"; fetched: boolean; label: string }
        | { kind: "tool"; name: string }
        | { kind: "image"; path: string | null }
        | { kind: "tasks" }
      ));

type ToolEntry = Extract<Entry, { kind: "tool" }>;

/** Between a call's key and its later cards'. */
export const CARD = "#";
/** Between a run's key and a file it wrote (as its rows have it). */
const WRITE = "~";

/** The cards of one call; empty for a call that is drawn as a row of its
 *  own (one of the host's agents, a colleague). */
export function cardsOf(entry: ToolEntry, deedsOf: (entry: Entry) => Deed[] | null, jobOf: (job: string) => string | null = () => null): Card[] {
  const { tool } = entry;
  const cards: Card[] = [];
  const running = entry.ended === null;
  const failed = entry.ended?.use === "failed" || entry.ended?.use === "declined";
  const machine = tool.machine;
  const next = (source: Source, machineOf: string | null = machine): Base => ({
    key: cards.length ? `${entry.key}${CARD}${cards.length}` : entry.key,
    machine: machineOf,
    sources: [source],
    running,
    failed: false,
  });
  const whole: Source = { entry: entry.key };
  switch (tool.does.as) {
    case "command": {
      const deeds = deedsOf(entry);
      commandCards(entry, tool.does, cards, next, deeds, jobOf);
      // A run still going is at the command it is on: one sent elsewhere
      // that has not ended, or else its last.
      if (running) {
        const going = (card: Card) => (card.kind === "command" && !!card.on?.going) || (card.kind === "fan" && card.deed.steps.some((step) => step.ended === undefined));
        const any = cards.some(going);
        cards.forEach((card, index) => (card.running = any ? going(card) : index === cards.length - 1));
      }
      break;
    }
    case "edit":
      tool.does.changes.forEach((change, index) => {
        const counted = counts(change.diff);
        cards.push({ ...next({ entry: entry.key, change: index }), kind: "change", path: change.path, how: howOf(change.how), added: counted.added, removed: counted.removed });
      });
      break;
    case "search":
      cards.push({ ...next(whole), kind: "web", fetched: false, label: tool.does.query, failed });
      break;
    case "image":
      cards.push({ ...next(whole), kind: "image", path: tool.does.path, failed });
      break;
    case "call": {
      const card = callCard(tool, next(whole));
      if (card) cards.push({ ...card, failed } as Card);
      break;
    }
    case "colleague":
    case "agent":
      return [];
  }
  return cards;
}

function commandCards(entry: ToolEntry, does: Extract<Tool["does"], { as: "command" }>, cards: Card[], next: (source: Source, machine?: string | null) => Base, deeds: Deed[] | null, jobOf: (job: string) => string | null): void {
  const writes = entry.tool.run?.writes ?? [];
  const whole: Source = { entry: entry.key };
  const exit = does.job ? null : does.exit;
  let wrote = false;
  const writeAll = () => {
    if (wrote) return;
    wrote = true;
    for (const write of writes) {
      const how: FileHow = write.kind;
      cards.push({ ...next({ entry: entry.key, write: write.id }, write.machine), key: `${entry.key}${WRITE}${write.id}`, kind: "change", path: write.path, how, added: write.added, removed: write.removed });
    }
  };
  /** What a command's text says of it: where an `on` sent it, the command
   *  itself, and whether it is a write or a copy, which are cards of their
   *  own. */
  const read = (said: string, machine: string | null) => {
    const on = ON.exec(said);
    const where = on ? (on[1] ?? on[2] ?? on[3] ?? null) : (machine ?? entry.tool.machine);
    const rest = on ? on[4]!.trim() : said;
    const shown = rest.replace(/<<-?\s*(['"]?)[A-Za-z_][A-Za-z0-9_]*\1/, "").trim() || said;
    return { where, rest, shown, own: !WRITES.test(rest) && !/^(?:fastctx\s+)?copy\s/.test(rest) };
  };
  const command = (said: string, source: Source, last: boolean, machine: string | null = null, sent: OnSaid | null = null, more: { sources: Source[]; commands: string[] } | null = null) => {
    // A command sent elsewhere says how it ended there; the run's own last
    // command says how the run did.
    const failed = sent?.end ? troubled(sent.end) && sent.end.kind !== "stopped" : last && exit !== null && exit !== 0;
    const { where, rest, shown } = read(said, machine);
    // The World's writes are cards of their own, as the files they wrote.
    if (WRITES.test(rest)) return writeAll();
    const copy = /^(?:fastctx\s+)?copy\s+(.*)$/s.exec(rest);
    if (copy) {
      const places = wordsOf(copy[1]!).filter((word) => !word.startsWith("-")).map((word) => placeOf(word, where));
      const to = places.pop();
      if (to && places.length) {
        cards.push({ ...next(source, null), kind: "copy", from: places, to, deed: null, failed });
        return;
      }
    }
    const card: Card = { ...next(source, where), kind: "command", command: shown, exit: last ? exit : null, on: sent, failed };
    if (more && card.kind === "command") {
      card.sources = more.sources;
      card.commands = more.commands;
    }
    cards.push(card);
  };
  if (!deeds) {
    const viewed = viewing(does.command);
    if (viewed) {
      for (const path of viewed) cards.push({ ...next(whole), kind: "read", path, deed: null });
      return;
    }
    // Each command the script gave, in order, where it ran; the last says
    // how the run ended. What they printed is one stream that cannot be
    // parted: commands of their own are one card, at the first of them.
    const commands = ownCommands(scriptCommands(does.command, entry.tool.machine));
    const own = commands.filter((one) => read(one.text, one.machine).own);
    let gathered = false;
    commands.forEach((one, index) => {
      if (own.length > 1 && own.includes(one)) {
        if (gathered) return;
        gathered = true;
        command(one.text, whole, own.includes(commands[commands.length - 1]!), one.machine, null, { sources: [whole], commands: own.map((each) => read(each.text, each.machine).shown) });
        return;
      }
      command(one.text, whole, index === commands.length - 1, one.machine);
    });
    writeAll();
    return;
  }
  // What a test run's build said is the run's: its card bears it.
  const testsOf = new Map<string, TestsDeed>();
  for (const deed of deeds) if (deed.kind === "tests" && deed.command !== null) testsOf.set(deed.command, deed);
  const lastPlain = deeds.map((deed) => deed.kind).lastIndexOf("plain");
  const built = new Map<TestsDeed, { warnings: number; errors: number }>();
  for (const deed of deeds) {
    const tests = deed.kind === "checks" && deed.command !== null ? testsOf.get(deed.command) : undefined;
    if (deed.kind !== "checks" || !tests) continue;
    const said = built.get(tests) ?? { warnings: 0, errors: 0 };
    said.warnings += deed.items.filter((item) => item.level === "warning").length;
    said.errors += deed.items.filter((item) => item.level === "error").length;
    built.set(tests, said);
  }
  /** Where the command that did it ran: where an `on` sent it, or the
   *  run's own machine. */
  const ran = (deed: Deed) => deed.where ?? entry.tool.machine;
  // The commands named alone of one scope printed into one stream that
  // cannot be parted: they are one card, at the first of them.
  const scopes = new Map<string, number[]>();
  deeds.forEach((deed, index) => {
    if (deed.kind !== "plain" || !read(deed.command, deed.machine ?? deed.where ?? null).own) return;
    const scope = deed.scope ?? `deed-${index}`;
    scopes.set(scope, [...(scopes.get(scope) ?? []), index]);
  });
  deeds.forEach((deed, index) => {
    const source: Source = { entry: entry.key, deed: index };
    switch (deed.kind) {
      case "writes":
        writeAll();
        break;
      case "file":
        // A file whose lines the output did not show opens on the run's.
        cards.push({ ...next(source, deed.machine ?? entry.tool.machine), kind: "read", path: deed.path, deed: deed.rows.length ? deed : null });
        break;
      case "search":
        cards.push({ ...next(source, ran(deed)), kind: "search", pattern: deed.pattern ?? "", hits: deed.files.reduce((sum, one) => sum + one.hits.length, 0), files: deed.files.length });
        break;
      case "status":
        cards.push({ ...next(source, ran(deed)), kind: "tree", deed });
        break;
      case "log":
        cards.push({ ...next(source, ran(deed)), kind: "history", deed });
        break;
      case "tests": {
        const said = built.get(deed) ?? { warnings: 0, errors: 0 };
        // The build's own lines open with the run's.
        const sources: Source[] = [...deeds.flatMap((other, at): Source[] => (other.kind === "checks" && other.command === deed.command ? [{ entry: entry.key, deed: at }] : [])), source];
        cards.push({ ...next(source, ran(deed)), sources, kind: "tests", deed, warnings: said.warnings, errors: said.errors, failed: deed.failed > 0 || said.errors > 0 });
        break;
      }
      case "checks":
        if (deed.command !== null && testsOf.has(deed.command)) break;
        cards.push({ ...next(source, ran(deed)), kind: "check", deed, failed: deed.items.some((item) => item.level === "error") });
        break;
      case "commit":
        cards.push({ ...next(source, ran(deed)), kind: "commit", deed, push: null });
        break;
      case "push": {
        const refused = deed.refs.some((ref) => !ref.ok);
        const before = cards[cards.length - 1];
        // A push right after the commit it sends is that commit's.
        if (before?.kind === "commit" && !before.push) {
          before.push = deed;
          before.sources.push(source);
          before.failed = refused;
          break;
        }
        cards.push({ ...next(source, ran(deed)), kind: "push", deed, failed: refused });
        break;
      }
      case "plain": {
        const together = deed.scope ? (scopes.get(deed.scope) ?? []) : [];
        if (together.length > 1 && together.includes(index)) {
          if (together[0] !== index) break;
          const last = together.includes(lastPlain) && lastPlain === deeds.length - 1;
          const named = together.map((at) => {
            const one = deeds[at] as Extract<Deed, { kind: "plain" }>;
            return read(one.command, one.machine ?? one.where ?? null).shown;
          });
          command(deed.command, source, last, deed.machine ?? deed.where ?? null, deed.on ?? null, { sources: together.map((at) => ({ entry: entry.key, deed: at })), commands: named });
          break;
        }
        command(deed.command, source, index === lastPlain && index === deeds.length - 1, deed.machine ?? deed.where ?? null, deed.on ?? null);
        break;
      }
      case "fan":
        cards.push({ ...next(source, null), kind: "fan", deed, failed: deed.steps.some((step) => troubled(step.end)) });
        break;
      case "copy":
        cards.push({ ...next(source, null), kind: "copy", from: deed.from, to: deed.to, deed, failed: deed.notCopied.length > 0 });
        break;
      case "job":
        cards.push({ ...next(source, deed.machine), kind: "job", deed, of: deed.script ?? jobOf(deed.job), failed: deed.failed });
        break;
      case "stop":
        cards.push({ ...next(source, deed.machine ?? ran(deed)), kind: "stop", deed, of: jobOf(deed.what), failed: !deed.stopped });
        break;
      case "page":
        cards.push({ ...next(source, deed.machine), kind: "page", deed });
        break;
      case "project":
        cards.push({ ...next(source, ran(deed)), kind: "project", deed });
        break;
      case "picture": {
        // The pictures one command gave are one card.
        const before = cards[cards.length - 1];
        if (before?.kind === "picture" && before.deeds[0]!.command === deed.command) {
          before.deeds.push(deed);
          before.sources.push(source);
          break;
        }
        cards.push({ ...next(source, deed.machine ?? entry.tool.machine), kind: "picture", deeds: [deed] });
        break;
      }
      case "world":
        cards.push({ ...next(source, ran(deed)), kind: "world", deed, failed: deed.act === "goal-blocked" });
        break;
    }
  });
  writeAll();
}

function callCard(tool: Tool, base: Base): Card | null {
  const given = (key: string) => {
    const value = (tool.input as Record<string, unknown> | null)?.[key];
    return typeof value === "string" && value.trim() ? value : null;
  };
  switch (tool.name) {
    case "Read":
      return { ...base, kind: "read", path: given("file_path") ?? "", deed: null };
    case "Grep":
    case "Glob":
      return { ...base, kind: "search", pattern: given("pattern") ?? tool.name, hits: null, files: null };
    case "WebSearch":
      return { ...base, kind: "web", fetched: false, label: given("query") ?? "" };
    case "WebFetch":
      return { ...base, kind: "web", fetched: true, label: given("url") ?? "" };
    case "TodoWrite":
      return { ...base, kind: "tasks" };
    default:
      return { ...base, kind: "tool", name: tool.server ? `${tool.server} ${tool.name}` : tool.name };
  }
}


/** The World's commands that write files. */
const WRITES = /^(?:fastctx\s+)?(?:edit|write|apply_patch)\b/;

/** An `on` and the machine it sends to. */
const ON = /^(?:fastctx\s+)?on\s+(?:'([^']*)'|"([^"]*)"|(\S+))\s*(.*)$/s;

/** A copy's word for a file: `machine:path`, or a path where the command
 *  ran. A drive letter is not a machine. */
function placeOf(word: string, here: string | null): Place {
  const named = /^([A-Za-z0-9][\w.-]+):(.+)$/.exec(word);
  return named ? { machine: named[1]!, path: named[2]! } : { machine: here, path: word };
}

function howOf(how: ChangeHow): FileHow {
  if (typeof how === "object") return "moved";
  if (how === "added") return "added";
  if (how === "deleted") return "deleted";
  return "modified";
}
