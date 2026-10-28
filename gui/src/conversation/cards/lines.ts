// A thing a stretch did as its row (`world-gui.md`, 对话): as every row of a
// stretch is, its glyph in the rail, a quiet verb, what it was done to, its
// figures beside it and where it happened; opened, a card that draws it
// (`deeds/draw.ts`). Things gathered (`bunch.ts`) are one row that opens on
// theirs.

import "./lines.css";
import { escape } from "../prose";
import { glyph, type GlyphName } from "../glyphs";
import { cargoSubject, shortCommand, testSubject } from "../deeds/rows";
import { troubled } from "../deeds/fan";
import { scriptSaid, wordsOf } from "../deeds/script";
import { cardsIn, type Bunch } from "./bunch";
import type { Card } from "./cards";
import type { Words } from "../../words";

interface Said {
  glyph: GlyphName;
  verb: string;
  object: string;
  /** Lines added and removed, as markup. */
  stats: string;
  /** How it came out where that is worth a word on the row: a failure. */
  outcome: string;
  /** Where it happened, as markup. */
  where: string;
  /** Markup before what it was done to: the hook of a command fed by the
   *  row above. */
  lead?: string;
  /** What it was done to as markup, where it says more than its words. */
  objectHtml?: string;
}

export interface LineLook {
  words: Words;
  open: boolean;
  enter: string;
}

/** A row for a thing or for things gathered. */
export function lineHtml(part: Card | Bunch, look: LineLook): string {
  const said = part.kind === "bunch" ? bunchSaid(part, look.words) : cardSaid(part, look.words);
  const cards = part.kind === "bunch" ? cardsIn(part) : [part];
  const running = cards.some((card) => card.running);
  const failed = cards.some((card) => card.failed);
  const mark = running ? `<span class="pulse" aria-hidden="true"></span>` : glyph(said.glyph);
  const tense = failed ? "failed" : running ? "running" : "done";
  const meta = `<span class="row-meta">${said.stats}${said.outcome}${said.where}<span class="row-chevron">${glyph("chevron", 16)}</span></span>`;
  const key = escape(part.key);
  return `<div class="row row-tool row-${tense}${part.kind === "bunch" ? " row-bunch" : ""}${look.enter}" data-row="${key}"><span class="rail">${mark}</span><button class="row-line" type="button" data-open="${key}" aria-expanded="${look.open}"><span class="row-action">${escape(said.verb)}</span><span class="row-rest"><span class="row-object">${said.lead ?? ""}${said.objectHtml ?? escape(said.object)}</span>${meta}</span></button><div class="fold-box"${look.open ? "" : ' style="height:0" inert'}><div class="fold-content"></div></div></div>`;
}

/** A row's words, as finding reads them. */
export function lineText(part: Card | Bunch, words: Words): string {
  const said = part.kind === "bunch" ? bunchSaid(part, words) : cardSaid(part, words);
  return [said.verb, said.object, plain(said.stats), plain(said.outcome), plain(said.where)].filter(Boolean).join(" ");
}

function cardSaid(card: Card, words: Words): Said {
  const { deeds } = words;
  const { verbs } = deeds;
  const where = place(card.machine, words);
  const failed = card.failed ? outcome(words.rowFailed) : "";
  const plainSaid = (glyphName: GlyphName, verb: string, object: string, extra: Partial<Said> = {}): Said => ({ glyph: glyphName, verb, object, stats: "", outcome: "", where, ...extra });
  switch (card.kind) {
    case "read":
      return plainSaid("read", verbs.read, leaf(card.path));
    case "search":
      return plainSaid("search", verbs.search, card.pattern);
    case "tree":
      return plainSaid("rows", verbs.status, card.deed.branch ?? deeds.tree);
    case "history":
      return plainSaid("history", verbs.log, deeds.history);
    case "tests":
      return plainSaid("test", verbs.test, testSubject(card.deed), { outcome: card.deed.failed ? outcome(deeds.failed(card.deed.failed)) : card.errors ? outcome(deeds.errors(card.errors)) : "" });
    case "check": {
      const built = /^cargo build\b/.test(card.deed.command ?? "");
      const errors = card.deed.items.filter((item) => item.level === "error").length;
      return plainSaid(built ? "tool" : "shield", built ? verbs.build : verbs.check, cargoSubject(card.deed.command, card.deed.cwd ?? null), { outcome: errors ? outcome(deeds.errors(errors)) : "" });
    }
    case "commit": {
      const refused = card.push?.refs.some((ref) => !ref.ok) ?? false;
      const branches = card.push ? [...new Set(card.push.refs.map((ref) => ref.to))].join(words.listSeparator) : "";
      const sent = refused ? outcome(deeds.rejected) : branches ? `<span class="row-outcome">${escape(deeds.pushedTo(branches))}</span>` : "";
      return plainSaid("commit", verbs.commit, card.deed.subject, { outcome: sent });
    }
    case "push": {
      const branches = [...new Set(card.deed.refs.map((ref) => ref.to))].join(words.listSeparator);
      return plainSaid("push", verbs.push, branches || card.deed.remote || "", { outcome: card.failed ? outcome(deeds.rejected) : "" });
    }
    case "command": {
      // How a command sent elsewhere ended is its row's only when it was
      // stopped or did not run through; its exit code is in its card.
      const end = card.on?.end;
      const ended = end?.kind === "stopped" ? `<span class="row-outcome">${escape(deeds.ended(end))}</span>` : end?.kind === "lost" || end?.kind === "not_run" ? outcome(deeds.ended(end)) : "";
      const lead = card.on?.piped ? `<span class="row-fed" title="${escape(deeds.fed)}">${glyph("fed", 12)}</span>` : "";
      const verb = card.running ? words.act("command", "running") : verbs.run;
      // Commands gathered: the first few, and how many more; all of them
      // once the row is open.
      if (card.commands && card.commands.length > 1) {
        const all = named(card.commands);
        const few = all.length > FEW ? `${all.slice(0, FEW).join(words.listSeparator)} ${deeds.moreCommands(all.length)}` : all.join(words.listSeparator);
        return plainSaid("command", verb, all.join(words.listSeparator), { outcome: ended, lead, objectHtml: `<span class="row-few">${escape(few)}</span><span class="row-all">${escape(all.join(words.listSeparator))}</span>` });
      }
      const look = card.running ? null : lookOf(card.command, deeds);
      if (look) return plainSaid(look.glyph, deeds.world.look, look.object, { outcome: ended, lead });
      return plainSaid("command", verb, shortCommand(card.command), { outcome: ended, lead });
    }
    case "fan": {
      const bad = card.deed.steps.filter((step) => troubled(step.end)).length;
      const many = `<span class="row-machine">${glyph("group", 12)}${escape(deeds.machines(card.deed.steps.length))}</span>`;
      return plainSaid("command", card.running ? words.act("command", "running") : verbs.run, shortCommand(card.deed.command), { outcome: bad ? outcome(deeds.troubled(bad)) : "", where: many });
    }
    case "copy": {
      const from = [...new Set(card.from.map((one) => one.machine ?? ""))].filter(Boolean);
      const names = card.from.map((one) => leaf(one.path));
      const route = `<span class="row-machine">${from.map((machine) => `${glyph("machine", 12)}${escape(words.on(machine))}`).join(" ")} → ${glyph("machine", 12)}${escape(words.on(card.to.machine ?? ""))}</span>`;
      const missed = card.deed?.notCopied.length ? outcome(deeds.notCopied(card.deed.notCopied.length)) : failed;
      return { glyph: "copy", verb: words.act("copy", "done"), object: names.length > 1 ? `${names[0]} +${names.length - 1}` : (names[0] ?? ""), stats: "", outcome: missed, where: route };
    }
    case "job":
      // What the job was running, where this conversation started it.
      return plainSaid("terminal", deeds.world.wait, card.of ? shortCommand(scriptSaid(card.of, card.deed.machine) ?? card.of) : card.deed.job, { outcome: card.deed.failed ? outcome(deeds.ended(card.deed.end)) : "" });
    case "stop":
      return plainSaid("stop", deeds.world.stop, card.of ? shortCommand(card.of) : card.deed.what, { outcome: card.deed.stopped ? "" : outcome(deeds.stopped(card.deed.how)) });
    case "page":
      return plainSaid("page", deeds.world.publish, card.deed.title, { outcome: `<span class="row-outcome">${escape(deeds.version(card.deed.version))}</span>` });
    case "project":
      return plainSaid("folder", deeds.world.enter, card.deed.town);
    case "picture": {
      const first = card.deeds[0]!;
      const object = first.page !== null ? `${leaf(first.path)} ${card.deeds.map((deed) => deeds.page(deed.page ?? 0)).join(words.listSeparator)}` : card.deeds.map((deed) => leaf(deed.path)).join(words.listSeparator);
      return plainSaid("image", deeds.world.look, object);
    }
    case "world": {
      const act = card.deed.act;
      if (act === "plan") return plainSaid("tasks", deeds.world.plan, deeds.thePlan);
      if (act === "sense") return plainSaid("sentinel", deeds.world.sense, card.deed.said ?? "");
      const goal = deeds.goal[act === "goal-complete" ? "complete" : act === "goal-blocked" ? "blocked" : "pause"];
      return plainSaid("goal", goal.verb, goal.object);
    }
    case "change": {
      const act = card.how === "added" ? "create" : card.how === "deleted" ? "delete" : card.how === "moved" ? "move" : "edit";
      return plainSaid("edit", words.act(act, "done"), leaf(card.path), { stats: stats(card.added, card.removed), outcome: failed });
    }
    case "web":
      return plainSaid("web", words.act(card.fetched ? "fetch" : "websearch", "done"), card.label, { outcome: failed });
    case "tool":
      return plainSaid("tool", card.name, "", { outcome: failed });
    case "image":
      return plainSaid("image", words.act("image", "done"), card.path ? leaf(card.path) : "", { outcome: failed });
    case "tasks":
      return plainSaid("tasks", words.act("tasks", "done"), "");
  }
}

/** Things gathered: what they are and how many, their figures summed, the
 *  failures among them always said. */
function bunchSaid(bunch: Bunch, words: Words): Said {
  const { deeds } = words;
  const cards = cardsIn(bunch);
  const first = cards[0]!;
  const machines = [...new Set(cards.map((card) => card.machine))];
  const where = machines.length === 1 ? place(machines[0]!, words) : "";
  const failures = cards.filter((card) => card.failed).length;
  const failed = failures ? outcome(deeds.failed(failures)) : "";
  if (bunch.family === "again") {
    const one = cardSaid(first, words);
    return { ...one, object: one.object, stats: `<span class="row-outcome">×${cards.length}</span>`, outcome: failed, where };
  }
  if (bunch.family === "changes") {
    const counted = cards.every((card) => card.kind === "change" && card.added !== null && card.removed !== null);
    const added = cards.reduce((sum, card) => sum + (card.kind === "change" ? (card.added ?? 0) : 0), 0);
    const removed = cards.reduce((sum, card) => sum + (card.kind === "change" ? (card.removed ?? 0) : 0), 0);
    return { glyph: "edit", verb: words.act("edit", "done"), object: deeds.files(cards.length), stats: counted ? stats(added, removed) : "", outcome: failed, where };
  }
  // Commands gathered on one card are each a command.
  if (bunch.family === "commands") return { glyph: "command", verb: deeds.verbs.run, object: deeds.commandsRun(cards.reduce((sum, card) => sum + (card.kind === "command" ? (card.commands?.length ?? 1) : 1), 0)), stats: "", outcome: failed, where };
  // A file read again, a search made again, counted once.
  const read = new Set(cards.flatMap((card) => (card.kind === "read" ? [`${card.machine ?? ""}|${card.path}`] : []))).size;
  const searched = new Set(cards.flatMap((card) => (card.kind === "search" ? [card.pattern] : card.kind === "web" ? [card.label] : []))).size;
  const others = cards.filter((card) => card.kind === "tree" || card.kind === "history").length;
  const object = [read ? deeds.files(read) : "", searched ? deeds.searches(searched) : "", others ? deeds.looked(others) : ""].filter(Boolean).join(words.listSeparator);
  return { glyph: read ? "read" : "search", verb: read ? deeds.verbs.read : deeds.verbs.search, object, stats: "", outcome: failed, where };
}

/** How many commands gathered on a row it names before counting the rest. */
const FEW = 3;

/** Commands gathered, as a row names them: each without what only routes
 *  its output, and the World's name said once, before the first of its
 *  commands. */
function named(commands: readonly string[]): string[] {
  let world = false;
  return commands.map((command) => {
    const short = shortCommand(command);
    const own = /^fastctx\s+(.+)$/.exec(short);
    if (!own) return short;
    if (world) return own[1]!;
    world = true;
    return short;
  });
}

/** What one of the World's own commands that only shows something showed,
 *  as its row says it after the verb of looking; null for any other. */
function lookOf(command: string, deeds: Words["deeds"]): { glyph: GlyphName; object: string } | null {
  const words = wordsOf(shortCommand(command));
  if (words[0] !== "fastctx") return null;
  const [first, second] = words.slice(1);
  const asked = words.slice(1).filter((word) => word.startsWith("-"));
  // Asked how it is used.
  if (asked.includes("--help") || asked.includes("-h") || first === "help" || first === undefined && asked.length) {
    const of = words.slice(0, 3).filter((word, index) => index === 0 || (!word.startsWith("-") && word !== "help")).join(" ");
    return { glyph: "info", object: deeds.looks.usage(of) };
  }
  if (asked.length) return null;
  if (first === "nodes" && !second) return { glyph: "machine", object: deeds.looks.machines };
  if (first === "w" && !second) return { glyph: "steps", object: deeds.looks.working };
  if (first === "project" && (second === "list" || second === undefined)) return { glyph: "folder", object: deeds.looks.projects };
  if (first === "plan" && !second) return { glyph: "tasks", object: deeds.looks.plan };
  if (first === "goal" && !second) return { glyph: "goal", object: deeds.looks.goal };
  if (first === "accounts" && !second) return { glyph: "person", object: deeds.looks.accounts };
  return null;
}

function place(machine: string | null, words: Words): string {
  return machine ? `<span class="row-machine">${glyph("machine", 12)}${escape(words.on(machine))}</span>` : "";
}

function outcome(said: string): string {
  return said ? `<span class="row-outcome wrong">${escape(said)}</span>` : "";
}

function stats(added: number | null, removed: number | null): string {
  return added !== null && removed !== null ? `<span class="row-stats"><span class="add">+${added}</span><span class="del">−${removed}</span></span>` : "";
}

function plain(markup: string): string {
  return markup.replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
}

function leaf(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut < 0 ? trimmed : trimmed.slice(cut + 1);
}
