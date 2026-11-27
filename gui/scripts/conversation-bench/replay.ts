// The page's own conversation model run over a recording of what a page was
// given (`crates/fastctx-client/examples/tape.rs`, or the page's own tape),
// with what it would show checked after every piece against what the same
// timeline shows when it is opened anew. Nothing here draws: the model is
// the part that decides what the page shows and in what order.

import { Conversation } from "../../src/conversation/conversation";
import type { KeptWords } from "../../src/conversation/model";
import { rowsOf, type Row } from "../../src/conversation/rows";
import type { ReplicaPart, ReplicaRecord } from "../../src/conversation/replica";
import type { SessionEvent } from "../../src/world/events";

export type Piece =
  | { kind: "replica" | "final"; part: ReplicaPart }
  | { kind: "records"; generation: number; records: ReplicaRecord[] }
  | { kind: "live"; event: SessionEvent }
  | { kind: "now"; busy: boolean; turn: string | null; kept: KeptWords[]; halted: string | null }
  | { kind: "waiting"; events: SessionEvent[] }
  | { kind: "asked"; event: SessionEvent }
  | { kind: "preparing"; preparing: boolean }
  | { kind: "interrupted"; error?: string }
  | { kind: "resumed" | "moved" | "note" | "unshown" };

export interface Taped { t: number; piece: Piece }

/** What the page is given, in order, into a conversation model, as the
 *  application gives it (`main.ts`, `watched.open`). */
export function feed(conversation: Conversation, piece: Piece): void {
  switch (piece.kind) {
    case "replica":
      conversation.openReplica(piece.part, null);
      return;
    case "records":
      conversation.records(piece.generation, piece.records);
      return;
    case "live":
      conversation.ahead(piece.event);
      return;
    case "now":
      conversation.now(piece.busy, piece.turn, piece.kept ?? [], piece.halted ?? null);
      return;
    case "waiting":
      conversation.waits(piece.events);
      return;
    case "asked":
      conversation.asked(piece.event);
      return;
    case "preparing":
      conversation.prepares(piece.preparing);
      return;
    case "interrupted":
      conversation.broke();
      return;
    default:
      return;
  }
}

/** A conversation opened on `part` alone, as one opened anew shows it, told
 *  what its machine says holds now. */
export function opened(part: ReplicaPart, now: Extract<Piece, { kind: "now" }> | null): Conversation {
  const conversation = new Conversation();
  conversation.openReplica(part, null);
  if (now) conversation.now(now.busy, now.turn, now.kept ?? [], now.halted ?? null);
  return conversation;
}

export interface Finding {
  law: string;
  at: number;
  said: string;
}

export interface Report {
  pieces: number;
  findings: Finding[];
  /** How the page stood at the end against the same timeline opened anew;
   *  `young` names the rows still under way, or ended too lately for their
   *  record to be due yet. */
  end: { live: Row[]; anew: Row[]; young: string[] } | null;
  counts: Record<string, number>;
}

/** Rows in `rows` whose items `anew` does not hold. */
function unheld(rows: readonly Row[], anew: readonly Row[]): Row[] {
  const known = new Set(anew.map((row) => row.id));
  return rows.filter((row) => row.kind !== "group-end" && row.kind !== "gap" && !known.has(row.id));
}

/** Pairs of items both lists hold whose order differs. */
function crossed(rows: readonly Row[], anew: readonly Row[]): [string, string][] {
  const at = new Map(anew.map((row, index) => [row.id, index]));
  const shared = rows.filter((row) => at.has(row.id) && row.kind !== "group-end" && row.kind !== "gap");
  const found: [string, string][] = [];
  for (let index = 1; index < shared.length; index += 1) {
    if (at.get(shared[index - 1]!.id)! > at.get(shared[index]!.id)!) found.push([shared[index - 1]!.id, shared[index]!.id]);
  }
  return found;
}

/** When each item began and, as told live, ended: by `turn/id`. */
class Times {
  began = new Map<string, number>();
  ended = new Map<string, number>();
  take(event: SessionEvent, live: boolean): void {
    if (event.event !== "began" && event.event !== "finished" && event.event !== "updated") return;
    const id = `${event.turn}/${event.item.id}`;
    const started = event.item.started_at ? Date.parse(event.item.started_at) : Date.parse(event.at);
    if (!this.began.has(id) && Number.isFinite(started)) this.began.set(id, started);
    if (event.item.draft) {
      const draft = `${event.turn}/${event.item.draft}`;
      const was = this.began.get(draft);
      if (was !== undefined && (!this.began.has(id) || this.began.get(id)! > was)) this.began.set(id, was);
    }
    if (live && event.event === "finished" && !this.ended.has(id)) this.ended.set(id, Date.parse(event.at));
  }
  /** When a row began: its first part's. */
  of(row: Row): number | undefined {
    return this.began.get(row.id.split("+")[0]!);
  }
  /** When a row ended as told live: its last part's. */
  end(row: Row): number | undefined {
    const parts = row.id.split("+");
    return this.ended.get(parts[parts.length - 1]!);
  }
}

/** The moment by the wall clock a tape's piece came at: the tape counts from
 *  when it began, its events say when they happened, and its first live
 *  event ties the two. */
function wallOf(tape: readonly Taped[]): (t: number) => number {
  for (const { t, piece } of tape) {
    if (piece.kind !== "live") continue;
    const at = Date.parse(piece.event.at);
    if (Number.isFinite(at)) return (when) => at + (when - t);
  }
  const began = Date.now();
  return (when) => began + when;
}

/** Runs `tape` through a conversation model, checking as it goes. The model
 *  reads the clock, and reads the tape's: as the page did when it came. */
export function replay(tape: readonly Taped[], options: { settleMs?: number; recordWithinMs?: number } = {}): Report {
  const wall = wallOf(tape);
  let current = 0;
  const realNow = Date.now;
  Date.now = () => wall(current);
  try {
    return replayed(tape, options, (t) => (current = t));
  } finally {
    Date.now = realNow;
  }
}

function replayed(tape: readonly Taped[], options: { settleMs?: number; recordWithinMs?: number }, at: (t: number) => void): Report {
  const settle = options.settleMs ?? 10_000;
  const recordWithin = options.recordWithinMs ?? 90_000;
  const times = new Times();
  let clock = 0;
  const findings: Finding[] = [];
  const counts: Record<string, number> = {};
  const count = (key: string) => (counts[key] = (counts[key] ?? 0) + 1);
  const conversation = new Conversation();
  let now: Extract<Piece, { kind: "now" }> | null = null;
  let records: ReplicaRecord[] = [];
  let base: ReplicaPart | null = null;
  // Rows shown since when, by id: what stays on the page unheld by its
  // timeline longer than `settle` is a finding, once.
  const unheldSince = new Map<string, number>();
  const told = new Set<string>();
  let final: ReplicaPart | null = null;
  // Since when its machine has said no turn runs: from a while after, the
  // page shows nothing still under way and is not busy.
  let idleSince: number | null = null;
  // While the watch is broken the page cannot know that no turn runs; it
  // is told again as the watch opens.
  let broken = false;
  for (const { t, piece } of tape) {
    at(t);
    if (piece.kind === "final") {
      final = piece.part;
      continue;
    }
    if (piece.kind === "replica") {
      base = piece.part;
      records = [];
    }
    if (piece.kind === "records") {
      records.push(...piece.records);
      for (const record of piece.records) if (record.record === "is") times.take(record.shown.event, false);
    }
    if (piece.kind === "replica") for (const shown of piece.part.events) times.take(shown.event, false);
    if (piece.kind === "live") {
      times.take(piece.event, true);
      clock = Math.max(clock, Date.parse(piece.event.at) || 0);
    }
    if (piece.kind === "now") {
      now = piece;
      broken = false;
      idleSince = piece.busy ? null : (idleSince ?? t);
    }
    if (piece.kind === "interrupted") broken = true;
    feed(conversation, piece);
    count(piece.kind);
    if (!base) continue;
    if (idleSince !== null && !broken && t - idleSince >= 5_000) {
      if (conversation.live.busy() && !told.has(`busy ${idleSince}`)) {
        told.add(`busy ${idleSince}`);
        findings.push({ law: "busy-at-rest", at: t, said: `the page shows the conversation working ${Math.round((t - idleSince) / 1000)}s after its machine said no turn runs` });
      }
      for (const row of rowsOf(conversation.blocks())) {
        if ((row.said === "Thinking…" || row.said.includes("writing")) && !told.has(`writing ${row.id}`)) {
          told.add(`writing ${row.id}`);
          findings.push({ law: "writing-at-rest", at: t, said: `${row.kind} ${row.id} still shown being written ${Math.round((t - idleSince) / 1000)}s after its machine said no turn runs` });
        }
      }
    }
    // The same timeline opened anew now: the part first read, every record
    // since taken in.
    const anew = opened(base, now);
    if (records.length) anew.records(base.generation, records);
    const live = rowsOf(conversation.blocks());
    const cold = rowsOf(anew.blocks());
    const shown = unheld(live, cold);
    const ids = new Set(shown.map((row) => row.id));
    for (const id of [...unheldSince.keys()]) if (!ids.has(id)) unheldSince.delete(id);
    const coldIds = new Set(cold.map((row) => row.id));
    for (const row of shown) {
      const since = unheldSince.get(row.id) ?? t;
      unheldSince.set(row.id, since);
      // Ended live, and no record of it long after: it will stay as it was
      // shown live until the page is opened anew, where it is not.
      if (row.said !== "Thinking…" && !row.said.includes("writing") && t - since >= recordWithin && !told.has(`unheld ${row.id}`)) {
        told.add(`unheld ${row.id}`);
        const fromEnd = live.length - 1 - live.findIndex((one) => one.id === row.id);
        findings.push({ law: "never-recorded", at: t, said: `${row.kind} "${row.said}" ${row.id} shown ${Math.round((t - since) / 1000)}s with no record of it; ${fromEnd} rows from the end` });
      }
      // Drawn after what the timeline holds that began after it ended.
      const ended = times.end(row) ?? times.of(row);
      if (ended === undefined) continue;
      const at = live.findIndex((one) => one.id === row.id);
      const later = live.slice(0, at).find((one) => coldIds.has(one.id) && one.kind !== "group-end" && (times.of(one) ?? 0) > ended + 2000);
      if (later && !told.has(`pile ${row.id}`)) {
        told.add(`pile ${row.id}`);
        findings.push({ law: "out-of-time", at: t, said: `${row.kind} "${row.said}" ${row.id} is drawn after ${later.kind} ${later.id}, which began ${Math.round(((times.of(later) ?? 0) - ended) / 1000)}s after it ended` });
      }
    }
    for (const [a, b] of crossed(live, cold)) {
      if (told.has(`crossed ${a} ${b}`)) continue;
      told.add(`crossed ${a} ${b}`);
      findings.push({ law: "order", at: t, said: `${a} shown before ${b}, which the timeline puts first` });
    }
    // A row both show but say differently, once it has been so a while.
    const coldById = new Map(cold.map((row) => [row.id, row]));
    for (const row of live) {
      const other = coldById.get(row.id);
      if (!other || other.said === row.said || row.kind === "group-end") continue;
      const key = `said ${row.id}`;
      const since = unheldSince.get(key) ?? t;
      unheldSince.set(key, since);
      if (t - since >= settle && !told.has(key)) {
        told.add(key);
        findings.push({ law: "said-differently", at: t, said: `${row.id}: live "${row.said}", anew "${other.said}"` });
      }
    }
  }
  // At rest at the end, however long nothing more came.
  if (idleSince !== null && !broken) {
    if (conversation.live.busy()) findings.push({ law: "busy-at-rest", at: Infinity, said: "the page shows the conversation working at the end, its machine having said no turn runs" });
    for (const row of rowsOf(conversation.blocks())) {
      if ((row.said === "Thinking…" || row.said.includes("writing")) && !told.has(`writing ${row.id}`)) {
        findings.push({ law: "writing-at-rest", at: Infinity, said: `${row.kind} ${row.id} still shown being written at the end, its machine having said no turn runs` });
      }
    }
  }
  let end: Report["end"] = null;
  if (final) {
    const anew = rowsOf(opened(final, now).blocks());
    const live = rowsOf(conversation.blocks());
    // Only what the part read at the end covers is compared, from the first
    // row both hold: the page may hold more of what came before.
    const ids = new Set(live.map((row) => row.id));
    const first = anew.find((row) => ids.has(row.id) && row.kind !== "group-end" && row.kind !== "gap");
    const from = first ? live.findIndex((row) => row.id === first.id) : live.length;
    const young = live
      .filter((row) => underWay(row) || (times.end(row) ?? times.of(row) ?? -Infinity) > clock - recordWithin)
      .map((row) => row.id);
    end = { live: live.slice(from), anew, young };
  }
  return { pieces: tape.length, findings, end, counts };
}

/** Whether a row shows its item still happening. */
function underWay(row: Row): boolean {
  return row.said === "Thinking…" || row.said.includes("running") || row.said.includes("writing");
}

/** The timeline as a page reads it at a moment: the part first read, with
 *  every record since taken in, in the timeline's order. */
class Timeline {
  private events = new Map<string, { key: string; place: { order: string; item: string }; event: SessionEvent }>();
  private older: ReplicaPart["older"] = null;
  open(part: ReplicaPart): void {
    this.events.clear();
    this.older = part.older;
    for (const shown of part.events) this.events.set(shown.key, { key: shown.key, place: shown.place, event: shown.event });
  }
  take(records: readonly ReplicaRecord[]): void {
    for (const record of records) {
      if (record.record === "gone") this.events.delete(record.key);
      else this.events.set(record.shown.key, { key: record.shown.key, place: record.shown.place, event: record.shown.event });
    }
  }
  part(): ReplicaPart {
    const events = [...this.events.values()].sort((a, b) => (a.place.order < b.place.order ? -1 : a.place.order > b.place.order ? 1 : 0));
    return { generation: 0, events, older: this.older, newer: null, preparing: false };
  }
}

/** What a live watch opened at `at` is told first: the items begun and not
 *  ended by then, with what they said so far, and the messages that ended
 *  in the two minutes before (`fastctx-node`, `session_live.rs`). */
function snapshot(live: readonly { at: number; event: SessionEvent }[], at: number): SessionEvent[] {
  const running = new Map<string, SessionEvent>();
  const ended: { at: number; event: SessionEvent }[] = [];
  for (const one of live) {
    if (one.at > at) break;
    const event = one.event;
    if (event.event === "began") running.set(`${event.turn}/${event.item.id}`, JSON.parse(JSON.stringify(event)));
    else if (event.event === "grew") {
      const held = running.get(`${event.turn}/${event.item}`);
      if (held && held.event === "began" && "text" in held.item && event.part === "text") (held.item as { text: string }).text += event.text;
    } else if (event.event === "finished") {
      running.delete(`${event.turn}/${event.item.id}`);
      if (event.item.draft) running.delete(`${event.turn}/${event.item.draft}`);
      if (event.item.kind === "message") ended.push(one);
    } else if (event.event === "turn_ended") {
      for (const key of [...running.keys()]) if (key.startsWith(`${event.turn}/`)) running.delete(key);
    }
  }
  const told = [...running.values(), ...ended.filter((one) => at - one.at < 120_000).slice(-4).map((one) => one.event)];
  return told.sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
}

export interface Disturbance {
  /** Records reach the page this much later than they came. */
  recordsLateMs?: number;
  /** The live watch breaks at each of these moments for `breakMs`: what it
   *  told meanwhile is lost, and it is told the snapshot as it opens again. */
  breaks?: number[];
  breakMs?: number;
  /** The page reads its latest part anew at each of these moments. */
  rereads?: number[];
}

/** `tape` as a page meets it under `disturbance`. */
export function disturbed(tape: readonly Taped[], disturbance: Disturbance): Taped[] {
  const late = disturbance.recordsLateMs ?? 0;
  const breakMs = disturbance.breakMs ?? 5000;
  const live = tape.filter((one) => one.piece.kind === "live").map((one) => ({ at: one.t, event: (one.piece as { event: SessionEvent }).event }));
  const out: Taped[] = [];
  const lost = (t: number) => (disturbance.breaks ?? []).some((at) => t >= at && t < at + breakMs);
  for (const one of tape) {
    const piece = one.piece;
    if (piece.kind === "final") continue;
    if (piece.kind === "records") {
      out.push({ t: one.t + late, piece });
      continue;
    }
    // What the watch tells, how things stand included, is lost while broken.
    if ((piece.kind === "live" || piece.kind === "now") && lost(one.t)) continue;
    out.push(one);
  }
  const nows = tape.filter((one) => one.piece.kind === "now");
  for (const at of disturbance.breaks ?? []) {
    out.push({ t: at, piece: { kind: "interrupted", error: "disturbed" } });
    // A watch opened again is told first how things stand, then the snapshot.
    const told = nows.filter((one) => one.t <= at + breakMs).pop();
    if (told) out.push({ t: at + breakMs, piece: told.piece });
    for (const event of snapshot(live, at + breakMs)) out.push({ t: at + breakMs, piece: { kind: "live", event } });
  }
  out.sort((a, b) => a.t - b.t);
  // Rereads take the timeline as the page holds it then.
  const rereads = [...(disturbance.rereads ?? [])].sort((a, b) => a - b);
  const timeline = new Timeline();
  const result: Taped[] = [];
  for (const one of out) {
    while (rereads.length && rereads[0]! <= one.t) {
      const at = rereads.shift()!;
      result.push({ t: at, piece: { kind: "replica", part: timeline.part() } });
    }
    if (one.piece.kind === "replica") timeline.open(one.piece.part);
    if (one.piece.kind === "records") timeline.take(one.piece.records);
    result.push(one);
  }
  const final = tape.find((one) => one.piece.kind === "final");
  if (final) result.push({ t: Math.max(final.t, result[result.length - 1]?.t ?? 0) + late + 1, piece: final.piece });
  return result;
}
