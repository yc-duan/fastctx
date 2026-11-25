// A conversation as it reads: the session's events folded into the things
// shown, in the order they first appeared. The same reading as the
// terminal's (`crates/fastctx-tui/src/transcript.rs`), so every interface
// shows a session alike.
//
// Each entry keeps a key made of what it is (its turn and item, the words'
// delivery), so reading the same history again gives the same keys and a
// view keeps what it drew; and a stamp that moves whenever the entry does,
// so a view draws again only what changed. An event costs the same however
// long the conversation is: entries are found by index, never by search.
//
// A fold reads one part of a conversation: a page holds its latest turns
// and whatever earlier parts its reader came to, each folded on its own
// (`conversation.ts`). What a key cannot name of its own (a health note, an
// item without an id) is named by its place in its fold, under the fold's
// scope, so two folds never name two things alike.

import type { Health, HostAgentEnd, MachineName, Notice, SessionChange, SessionEvent, Speaker, Step, Tool, TurnEnd } from "../world/events";
import type { Attachment, AttachmentSource } from "../world/attachments";
import type { JsonValue } from "../world/wire";
import { mergeSteps, type JobFacts, type RunFacts } from "../world/run-facts";
import { askKey, forPeople, type AskState } from "./ask-state";
import { userWords } from "./told";
import { Agents, type AgentRun } from "./agents";
import { Requests } from "./requests";
import { eventBase, sameProjection } from "./timeline-fold";
import type { PendingControl } from "../world/control";

export type Use = "done" | "failed" | "declined" | "cut" | "unknown";

export type SendState =
  /** On their way. `retry` is set once their host has failed to start for
   *  a minute and is being started again: why it did not start. `starting`
   *  while their host is being started for them, as its machine tells. */
  | { state: "going"; retry?: string; starting?: Starting }
  /** No answer came yet; they are said again, under the same id, until one
   *  does: why the last try got none. */
  | { state: "waiting"; why: string }
  /** In the World, not yet in a turn: `how` is the way they landed
   *  (`waiting`, `sent`, `uncertain`), as the node said. */
  | { state: "landed"; how: string; machine: MachineName; why?: string | null }
  /** Kept for a turn their agent login, or their machine's memory, has no
   *  room for yet: `ahead` turns go before it. */
  | { state: "queued"; ahead: number; memory?: boolean; callee?: boolean }
  /** Its machine keeps them no more and no turn runs: they went in, and
   *  stand as said until the session's own telling of them comes. */
  | { state: "settled" }
  /** No answer came within `ANSWER_WITHIN` (`world/again.ts`): they may or
   *  may not have gone in. They are still said again, under the same id,
   *  when the way opens. */
  | { state: "unknown"; why: string }
  | { state: "failed"; why: string };

/** What can be done with words that did not go, or of which it is not known
 *  whether they went (`specs/world-foundation.md`, 宪法第十一条): sent again
 *  under the same id, which goes in once, or taken away. */
export interface Settler {
  again?: () => void;
  remove: () => void;
}

/** Words a conversation's machine keeps that no turn took yet, as it tells
 *  them (`Watched::Now`). */
export interface KeptWords {
  id: string;
  state: "waiting" | "handing" | "unknown";
  /** Present once their host has failed to start for a minute; they are
   *  still on their way (`world-lifecycle.md`, 用户看到的只是在发送). */
  host_retry?: { since: string; reason: string };
  /** How many turns of their agent login go before the one they would
   *  start, while it waits for room (`world-usage.md`, 一个登录同时开几轮). */
  queued?: number;
  /** Whether that room is in their machine's memory rather than in their
   *  login (`world-lifecycle.md`, 内存). */
  for_memory?: boolean;
  /** Whether they wait for one of their callee's colleagues at work to
   *  end (`world-colleagues.md`, 同事从哪里来、能去哪里). */
  for_callee?: boolean;
  /** While their host is being started for them: how far that came. */
  starting?: Starting;
}

/** A host being started for kept words, as its machine tells it. */
export interface Starting {
  /** When the World first asked for the host (an instant, as text). */
  since: string;
  /** `preparing`, `launching`, `connecting`, `retrying`, or one of a newer
   *  version. */
  phase: string;
  /** Starts that failed and are tried again. */
  failures?: number;
  /** Why the last one failed. */
  reason?: string;
  /** When the next try begins, while one waits. */
  next?: string;
}

/** How the start of a conversation's host shows on its turn line: since
 *  when (null where not told), how many tries failed and why the last did,
 *  and, while it waits in line, how many go before it and for what. */
export interface StartShown {
  since: number | null;
  failures: number;
  reason: string | null;
  ahead?: number;
  memory?: boolean;
  callee?: boolean;
}

/** Something the user attached to their words, as it is shown with them. */
export interface ShownAttachment {
  name: string;
  size: number | null;
  image: boolean;
  preview: string | null;
  /** Its bytes' digest, where it is known before the World's telling of
   *  the words comes: what a picture on its way is known by
   *  (`picture-frame.ts`). */
  digest?: string;
  /** The file as the World kept it, once it has: what a small picture is
   *  asked for and the original opened from. */
  kept?: Attachment;
  /** A picture the World never kept, as its agent's host recorded it. */
  recorded?: AttachmentSource;
}

export type Entry = {
  stamp: number;
  /** Where in its fold the event that made it, or last placed it, was. */
  at: number;
} & (
  | {
      kind: "heard";
      key: string;
      from: Speaker | null;
      text: string;
      turn: string | null;
      /** When it was said, in milliseconds since the epoch: as the machine
       *  the conversation lives on recorded it, or as it was sent from here
       *  until then. */
      time?: number;
      sending?: SendState;
      /** Words sent from here, once the World answered for them: their id
       *  on the machine they went to, `null` where it gave none. */
      delivery?: string | null;
      messageId?: string;
      /** Words sent from here while a turn was under way: they wait below
       *  the turn's line until a turn takes them in. Words sent to a
       *  conversation at rest begin the next turn, above it. */
      intoTurn?: boolean;
      attached?: ShownAttachment[];
      /** The attachments as the World answered for them, kept where they
       *  arrived: words taken back to send again take these with them. */
      kept?: Attachment[];
      /** What the World told the session, as facts, when it was the World
       *  or another agent that spoke. */
      notice?: Notice;
    }
  | {
      kind: "prose";
      key: string;
      item: string;
      turn: string;
      prose: "message" | "thinking" | "plan";
      text: string;
      writing: boolean;
      /** Bumped whenever the text changes, so a view redraws only what did. */
      version: number;
      /** Thinking: when it began, as far as is known. */
      began?: number;
      /** Thinking: how long it took, where the host said. */
      took?: number;
      /** Thinking whose words are not shown (GPT's): what it said, kept only
       *  for the title of the section it is in while it thinks. */
      heads?: string;
      /** Thinking: the model response it was part of, whose thinking
       *  tokens `Fold.thinkingTokens` knows where they were told. */
      response?: string;
    }
  | { kind: "tool"; key: string; item: string; turn: string; tool: Tool; began: number; ended: { at: number; use: Use } | null }
  | { kind: "steps"; key: string; turn: string; steps: Step[]; note: string | null }
  | { kind: "compaction"; key: string; item: string; turn: string }
  | { kind: "other"; key: string; item: string; turn: string; hostKind: string }
  | { kind: "activity"; key: string; item: string; turn: string; text: string }
  | { kind: "ended"; key: string; turn: string; end: TurnEnd }
  | { kind: "health"; key: string; health: Health }
  | { kind: "uncertain"; key: string; why: string }
  /** This conversation was forked from another one: from where in it, and
   *  the last words between the user and the agent it carries on from,
   *  where its record holds them. */
  | { kind: "forked"; key: string; source: import("../world/wire").Address; through: string | null; said: string | null; answer: string | null }
  /** A request the session waits on the user for, as its host made it. */
  | { kind: "ask"; key: string; id: JsonValue; turn: string | null; request: string; params: JsonValue; state: AskState; version: number }
  /** What the agent asked the user without stopping its work, with Codex's
   *  `request_user_input_async`, which a World thread cannot be without:
   *  words to the user, as `message_user`'s are, the questions and the
   *  answers they offer written out; the user answers in words of their
   *  own, whenever they like. */
  | { kind: "question"; key: string; stamp: number; at: number; item: string; turn: string; text: string }
  /** One of the host's own agents that went on in the background ended,
   *  and the conversation was told of it here: by the host's notice, or,
   *  in Codex, by an item of the turn. */
  | { kind: "agent-told"; key: string; agent: string; end: HostAgentEnd; item?: string; turn?: string }
  /** A command the user gave it, from when it began: while it runs, and
   *  what came of it (`world-gui.md`, 对话). */
  | { kind: "command"; key: string; row: PendingControl }
);

/** One stretch of what a turn did between things it said: tools and
 *  thinking in a row, folded under one heading. `stamp` is the latest of
 *  its members'. */
export interface Activity {
  kind: "activity-group";
  key: string;
  turn: string;
  members: Entry[];
  partialBefore?: boolean;
  partialAfter?: boolean;
  stamp: number;
  /** One of its tools started or names one of the host's own agents,
   *  which move on their own (`agentStamp`). */
  agents: boolean;
}

/** Turns a page does not hold, standing where they are: before everything
 *  it holds, or between two parts of it (`conversation.ts`). */
export interface Gap {
  kind: "gap";
  key: string;
  /** Moves when what it says does. */
  stamp: number;
  /** The events it stands for. */
  start: number;
  end: number;
  turns: number;
  /** Nothing is held before it. */
  first: boolean;
  /** Its turns are being read. */
  loading: boolean;
  later?: boolean;
  /** Of a conversation shown from its timeline: what it stands for is
   *  counted as `replica.ts` counts it, and it is drawn as tall as that
   *  would be. */
  paged?: boolean;
  /** The pixels the page measured of what it stands for, when it held it. */
  measured?: number;
  /** How many events' worth of it was never measured. */
  unmeasured?: number;
}

/** What the agent said to the user through the World's `message_user`
 *  (`world-voice.md`): drawn as its answer, apart from the work around it. */
export interface Say {
  kind: "say";
  key: string;
  item: string;
  turn: string;
  stamp: number;
  /** Where in its fold the call that said it was. */
  at: number;
  /** When it was said, in milliseconds since the epoch, where known. */
  time: number | null;
  text: string;
  status: "reply" | "info" | "urgent";
  attachments: { machine: string; path: string }[];
}

export type Block = Entry | Activity | Gap | Say;

/** Whether `server` names the World's own tools, as either host lists them:
 *  Codex as they are served, Claude Code under its plugin. */
export function worldServer(server: string | null): boolean {
  return server === "fastctx" || (server?.endsWith("_fastctx") ?? false);
}

/** The words for the user a `message_user` call carries; null for any other
 *  tool, and for one that failed, which reached no one. */
export function sayOf(entry: Extract<Entry, { kind: "tool" }>): Say | null {
  const tool = entry.tool;
  if (tool.name !== "message_user" || !worldServer(tool.server) || tool.status === "failed") return null;
  const input = (tool.input && typeof tool.input === "object" && !Array.isArray(tool.input) ? tool.input : {}) as Record<string, JsonValue>;
  const status = input.status === "info" || input.status === "urgent" ? input.status : "reply";
  const attachments = Array.isArray(input.attachments)
    ? input.attachments.flatMap((one) =>
        one && typeof one === "object" && !Array.isArray(one) && typeof one.machine === "string" && typeof one.path === "string" ? [{ machine: one.machine, path: one.path }] : [],
      )
    : [];
  return { kind: "say", key: entry.key, item: entry.item, turn: entry.turn, stamp: entry.stamp, at: entry.at, time: Number.isFinite(entry.began) ? entry.began : null, text: typeof input.message === "string" ? input.message : "", status, attachments };
}

type Keyed = Extract<Entry, { item: string; turn: string }>;
type Heard = Extract<Entry, { kind: "heard" }>;
type ToolEntry = Extract<Entry, { kind: "tool" }>;

/** What is read off the entries as a whole, once for each state of them. */
interface Derived {
  clock: number;
  /** The last message of each turn, by key. */
  lastMessage: Map<string, string>;
  /** The turn of the last ending shown. */
  latest: string | null;
  moving: boolean;
  waiting: boolean;
  /** The start the first words kept wait on (`starting`). */
  start: StartShown | null;
  /** Whether anything of the agent's own has been shown: a turn's work or
   *  words, its end, a request. */
  heardFrom: boolean;
  /** The turns in which it said something to the user through
   *  `message_user`: the rest of their words are its working notes. */
  spoke: Set<string>;
}

export class Fold {
  entries: Entry[] = [];
  /** What names this fold's own keys apart from every other fold's. */
  constructor(private readonly scope = "") {}
  /** How much of the context the host last reported in use, and when. */
  context: { used: number; window: number | null; at: number } | null = null;
  title: string | null = null;
  /** The model the host last answered with, where it says (Claude's
   *  answers do): what the conversation shows, not the one chosen. */
  answeredBy: string | null = null;
  /** The host's own agents its calls started (`agents.ts`). */
  readonly agents = new Agents();
  /** Each model request and what it cost, as the host recorded it. */
  readonly requests = new Requests();
  /** Moves whenever one of the agents does. */
  get agentStamp(): number {
    return this.agents.stamp;
  }
  /** Attachments by where the World keeps them, with the names they came
   *  with: the World stores each under a name of its own. */
  private attachments = new Map<string, string>();

  /** The name an attachment came with, by where it is kept. */
  attachmentName(path: string): string | null {
    return this.attachments.get(path.replaceAll("\\", "/")) ?? null;
  }
  /** Whether its thinking's words are kept: Codex's are not, whatever it
   *  hands over (`world-gui.md`, 对话); its thinking is a line without them. */
  thoughtWords = true;
  /** The host replaced or removed turns: what is shown must be read again. */
  stale = false;
  /** Moves with every change to what is shown; an entry's stamp is the
   *  clock when it last changed. */
  clock = 0;
  /** Each turn's stretches of activity, in order, by the key of their first
   *  entry: a stretch keeps its place and key when its first entry goes. */
  private stretches = new Map<string, string[]>();
  /** How many events this fold has read, and the place of the one being
   *  read. */
  private folded = 0;
  private current = 0;
  /** Words sent from here, numbered apart: they outlive a reading. */
  private sends = 0;
  /** The user's words heard as they came that no words of this window's
   *  queue have yet been told apart by, numbered as they came: the words
   *  a shared queue says are on their way may have been heard first. */
  private heardSeq = 0;
  private unclaimed: { key: string; text: string; seq: number }[] = [];
  private claimedHeard = new Set<string>();
  /** Entries by key, and by their turn and item; tools by their call. */
  private byKey = new Map<string, Entry>();
  /** How words being sent are sent again or taken away, by their key. */
  private settlers = new Map<string, Settler>();
  private byItem = new Map<string, Keyed>();
  private byCall = new Map<string, ToolEntry>();
  /** Each turn's entries, in the order they came. */
  private byTurn = new Map<string, Entry[]>();
  /** How many of the user's heard words each turn holds, for their keys. */
  private heardIn = new Map<string, number>();
  private endedTurns = new Set<string>();
  /** Items their host has told whole as they ended: a snapshot of one told
   *  after that, as it began or ran, is behind it and changes nothing. */
  private over = new Set<string>();
  /** Turns begun and not yet ended, as the World tells them, with when
   * each began. */
  private running = new Set<string>();
  private began = new Map<string, number>();
  /** How many turns have ended while watched (history read again is not
   *  a turn ending), and how the last one did. */
  private endings = 0;
  private lastEnding: TurnEnd | null = null;
  private replaying = false;
  /** What the user asked to stop, while it has not ended (`halt`): the
   *  turns then running, or, when none was, the turn the words on their way
   *  start once it does. */
  private halted: Set<string> | null = null;
  /** The turns the conversation's machine says were asked to stop, by this
   *  device or another (`now`): calling off this device's stop leaves them. */
  private machineHalted = new Set<string>();
  /** The words on their way when a stop was asked with no turn running:
   *  the stop is of the turn that takes them. Each is known by its entry,
   *  by its delivery once the World answers for it (`named`), and by what
   *  it says only while it has no delivery (`takesHalted`). */
  private haltedWords: { key: string; delivery: string | null | undefined; text: string }[] = [];
  /** A reading did not show those words taken: the stop waits for them, and
   *  is spent once the machine no longer keeps them (`now`). */
  private haltedUnseen = false;
  private derivedCache: Derived | null = null;
  private blocksCache: { clock: number; blocks: Block[] } | null = null;
  /** In a fork, what this fold held before the point it was made at: what
   *  it carried over, which the card at its head stands for. */
  private inherited: Set<string> | null = null;

  /** Whether this fold holds the point its conversation was forked at. */
  get holdsFork(): boolean {
    return this.entries[0]?.kind === "forked";
  }

  /** Marks an entry changed. */
  private touch(entry: Entry): void {
    entry.stamp = ++this.clock;
  }

  private preferredKeys: Map<string, string> | null = null;
  private add(entry: Entry): void {
    if ("item" in entry && "turn" in entry && typeof entry.turn === "string") entry.key = this.preferredKeys?.get(itemKey(entry.turn, entry.item)) ?? entry.key;
    if (entry.kind === "heard") entry.key = (entry.delivery ? this.preferredKeys?.get(`heard:d:${entry.delivery}`) : undefined)
      ?? (entry.messageId ? this.preferredKeys?.get(`heard:m:${entry.messageId}`) : undefined) ?? entry.key;
    entry.stamp = ++this.clock;
    entry.at = this.current;
    this.entries.push(entry);
    this.index(entry);
  }

  private index(entry: Entry): void {
    this.byKey.set(entry.key, entry);
    if (entry.kind === "tool") this.byCall.set(entry.item, entry);
    if ("item" in entry && "turn" in entry && typeof entry.turn === "string") this.byItem.set(itemKey(entry.turn, entry.item), entry as Keyed);
    const turn = "turn" in entry ? entry.turn : null;
    if (typeof turn === "string") {
      const list = this.byTurn.get(turn);
      if (list) list.push(entry);
      else this.byTurn.set(turn, [entry]);
    }
  }

  /** Takes entries out of what is shown. */
  private remove(gone: (entry: Entry) => boolean): void {
    const kept: Entry[] = [];
    let removed = false;
    for (const entry of this.entries) {
      if (gone(entry)) removed = true;
      else kept.push(entry);
    }
    if (!removed) return;
    this.entries = kept;
    this.reindex();
  }

  private reindex(): void {
    this.byKey.clear();
    this.byItem.clear();
    this.byTurn.clear();
    this.byCall.clear();
    for (const entry of this.entries) this.index(entry);
    this.clock += 1;
  }

  /** What came before, replacing what is shown; `carry`, what the part's
   *  turns read of what came before it (its title, its context, the model
   *  that answered), read first as nothing of its own. */
  history(events: SessionEvent[], carry: SessionEvent[] = [], places?: readonly string[], keepWaiting = true, authoritativePending?: SessionEvent[]): void {
    // Requests still waiting are kept: the host asked them, and they are
    // not part of the history it gives back. So are words sent from here
    // that this reading does not hold yet.
    this.preferredKeys = places ? new Map(this.entries.flatMap((entry): [string, string][] => {
      if ("item" in entry && "turn" in entry && typeof entry.turn === "string") return [[itemKey(entry.turn, entry.item), entry.key]];
      if (entry.kind === "heard") return [...(entry.delivery ? [[`heard:d:${entry.delivery}`, entry.key] as [string, string]] : []), ...(entry.messageId ? [[`heard:m:${entry.messageId}`, entry.key] as [string, string]] : [])];
      return [];
    })) : null;
    const waiting = keepWaiting ? this.entries.filter((entry) => entry.kind === "ask" && entry.state.state === "waiting") : [];
    const pending = this.entries.filter((entry): entry is Heard => entry.kind === "heard" && entry.sending !== undefined);
    const known = new Set(this.entries.filter((entry) => entry.kind === "heard").map((entry) => entry.key));
    const halted = this.halted;
    this.entries = [];
    this.byCall.clear();
    if (places) {
      this.attachments.clear();
      this.controlStates.clear();
      this.requests.clear();
    }
    this.placedEntries.clear();
    this.eventBases.clear();
    this.eventSources.clear();
    this.pendingIds = authoritativePending === undefined ? null : new Set(authoritativePending.flatMap((event) => event.event === "changed" && event.change.kind === "request" ? [askKey(event.change.id)] : []));
    this.byKey.clear();
    this.byItem.clear();
    this.byTurn.clear();
    this.heardIn.clear();
    this.endedTurns.clear();
    this.over.clear();
    this.stretches.clear();
    this.pagedGroups = places !== undefined;
    if (!this.pagedGroups) this.previousGroups.clear();
    this.sequences.clear();
    this.stale = false;
    this.halted = null;
    // What the machine said was asked to stop stands through a reading
    // again: it goes when the turn ends or no turn runs.
    this.running.clear();
    this.began.clear();
    this.thoughtTokens.clear();
    this.agents.clear();
    this.codexTotal = 0;
    this.codexResponses = 0;
    this.folded = 0;
    this.current = 0;
    this.clock += 1;
    this.replaying = true;
    try {
      for (const event of carry) this.read(event);
    } finally {
      this.replaying = false;
    }
    this.ignoreHistoricalRequests = authoritativePending !== undefined;
    this.extend(events, places);
    this.ignoreHistoricalRequests = false;
    if (authoritativePending) this.extend(authoritativePending);
    this.preferredKeys = null;
    // A history the node gives whole is as it stands after every change it
    // holds: a change of history among its events is shown already, and is
    // no reason to read it again. One that comes after it is.
    this.stale = false;
    // A stop asked of what still runs, or of words still on their way,
    // stands through a reading again: only on the turns it was asked of,
    // never on one the reading shows starting after they ended. Words on
    // their way are stopped in the turn the reading shows taking them:
    // stopped if it still runs, spent if it ended. A reading that does not
    // show them taken — an older page, a window that begins later — leaves
    // the stop waiting for them as before.
    if (halted?.size === 0) {
      const target = [...carry, ...events, ...(authoritativePending ?? [])].flatMap((event) => {
        if (event.event !== "heard" || !event.turn) return [];
        const said = (event.from ? event.text : userWords(event.text)?.body ?? event.text).trim();
        return this.takesHalted(event.delivery, said) ? [event.turn] : [];
      })[0];
      this.haltedUnseen = target === undefined;
      if (target === undefined) this.halted = new Set();
      else if (this.running.has(target)) this.haltTurn(target);
    } else for (const turn of halted ?? []) if (this.running.has(turn)) this.haltTurn(turn);
    for (const turn of this.machineHalted) if (this.running.has(turn)) this.haltTurn(turn);
    for (const ask of waiting) {
      if (ask.kind === "ask" && !this.entries.some((entry) => entry.kind === "ask" && askKey(entry.id) === askKey(ask.id))) this.add(ask);
    }
    // Words sent from here are the reading's own once it holds them: words
    // the conversation had not shown before, the same as these.
    const claimed = new Set<string>();
    for (const sent of pending) {
      const said = sent.text.trim();
      const heard = this.entries.find(
        (entry): entry is Heard =>
          entry.kind === "heard" &&
          !known.has(entry.key) &&
          !claimed.has(entry.key) &&
          entry.sending === undefined &&
          (!entry.from || entry.from.kind === "user") &&
          entry.text.trim() === said,
      );
      if (heard) claimed.add(heard.key);
      else this.add(sent);
    }
  }

  /** More of what came before, after what is shown: a reading given in
   *  parts, as a cache answers and then what is new since. */
  extend(events: SessionEvent[], places?: readonly string[]): void {
    this.replaying = true;
    try {
      for (const [index, event] of events.entries()) this.event(event, places?.[index]);
    } finally {
      this.replaying = false;
    }
  }

  /** One more event of this fold's part. */
  private placedEntries = new Map<string, Set<string>>();
  private eventBases = new Map<string, Entry[]>();
  private eventSources = new Map<string, SessionEvent>();

  /** Item-only changes update their own entries and dependent facts. */
  reviseItems(events: readonly SessionEvent[], places: readonly string[], changed: readonly string[], removed: readonly string[]): void {
    this.evictPlaces(removed);
    const positions = new Map(places.map((place, at) => [place, at]));
    this.replaying = true;
    try {
      for (const place of changed) {
        const at = positions.get(place)!;
        const event = events[at]!;
        this.folded = at;
        this.event(event, place);
        if (event.event !== "finished") continue;
        for (const fact of events) {
          if (fact.event !== "changed") continue;
          const change = fact.change;
          if ((change.kind === "run_facts_updated" && change.update.call_id === event.item.id)
            || (change.kind === "message_usage" && change.items.includes(event.item.id))
            || (change.kind === "usage" && fact.turn === event.turn && event.item.kind === "thinking")) this.late(fact);
        }
        const entry = this.find(event.turn, event.item.id);
        if (entry?.kind === "tool" && entry.ended === null && this.endedTurns.has(event.turn)) {
          const end = events.find((event) => event.event === "turn_ended" && event.turn === entry.turn);
          entry.ended = { at: end ? Date.parse(end.at) : Date.parse(event.at), use: "cut" };
          this.touch(entry);
        }
      }
    } finally { this.replaying = false; }
    const ordered = new Map<string, number>();
    for (const [place, owned] of this.placedEntries) {
      const at = positions.get(place);
      if (at === undefined) continue;
      for (const key of owned) ordered.set(key, Math.min(at, ordered.get(key) ?? Infinity));
    }
    this.entries.sort((a, b) => (ordered.get(a.key) ?? Infinity) - (ordered.get(b.key) ?? Infinity));
    for (const entry of this.entries) entry.at = ordered.get(entry.key) ?? events.length;
    this.folded = events.length;
    this.reindex();
  }

  /** Reproject changed payloads and the small contextual facts. Unchanged
   * payloads reuse their pre-fact entries; no transcript is refolded. */
  revised(before: readonly SessionEvent[], previous: readonly string[], events: readonly SessionEvent[], places: readonly string[], pending: readonly SessionEvent[]): Fold {
    const old = new Map(previous.map((place, at) => [place, before[at]]));
    const next = new Fold(this.scope);
    next.thoughtWords = this.thoughtWords;
    next.clock = this.clock;
    next.pagedGroups = true;
    next.title = this.title; next.context = this.context; next.answeredBy = this.answeredBy;
    next.previousGroups = new Map(this.previousGroups);
    next.stretches = new Map([...this.stretches].map(([turn, keys]) => [turn, [...keys]]));
    next.halted = this.halted ? new Set(this.halted) : null;
    next.machineHalted = new Set(this.machineHalted);
    next.haltedWords = this.haltedWords.map((word) => ({ ...word }));
    next.haltedUnseen = this.haltedUnseen;
    next.sends = this.sends; next.heardSeq = this.heardSeq;
    next.settlers = new Map(this.settlers);
    next.unclaimed = [...this.unclaimed]; next.claimedHeard = new Set(this.claimedHeard);
    next.pendingIds = new Set(pending.flatMap((event) => event.event === "changed" && event.change.kind === "request" ? [askKey(event.change.id)] : []));
    next.ignoreHistoricalRequests = true;
    next.replaying = true;
    next.preferredKeys = new Map(this.entries.flatMap((entry): [string, string][] => {
      if ("item" in entry && "turn" in entry && typeof entry.turn === "string") return [[itemKey(entry.turn, entry.item), entry.key]];
      if (entry.kind === "heard") return [...(entry.delivery ? [[`heard:d:${entry.delivery}`, entry.key] as [string, string]] : []), ...(entry.messageId ? [[`heard:m:${entry.messageId}`, entry.key] as [string, string]] : [])];
      return [];
    }));
    for (let at = 0; at < events.length; at += 1) {
      const event = events[at]!; const place = places[at]!;
      next.folded = at; next.current = at;
      const base = old.get(place) === event && this.eventSources.get(place) === event ? this.eventBases.get(place) : undefined;
      if (base && (event.event === "finished" || event.event === "began" || event.event === "updated" || event.event === "heard")) {
        next.currentPlace = place; next.placed = true;
        if (event.event === "heard") {
          for (const file of event.message?.attachments ?? []) next.attachments.set(file.path.replaceAll("\\", "/"), file.name);
          const turn = event.turn ?? "";
          next.heardIn.set(turn, (next.heardIn.get(turn) ?? 0) + 1);
        } else {
          if (event.item.draft) next.renamed(event.turn, event.item.draft, event.item.id);
          if (["message", "thinking", "plan", "tool", "compaction"].includes(event.item.kind) && !next.find(event.turn, event.item.id)) next.overtaken(event.turn);
          if (event.event === "finished" && event.item.id) next.over.add(itemEntryKey(event.turn, event.item.id));
        }
        const owned = new Set<string>();
        for (const entry of base) {
          const restored = { ...entry, at };
          const held = next.byKey.get(restored.key);
          if (held?.kind === "tool" && restored.kind === "tool") {
            restored.began = held.began;
            restored.ended ??= held.ended;
            if (!restored.tool.run && held.tool.run) restored.tool = { ...restored.tool, run: held.tool.run };
          } else if (held?.kind === "prose" && restored.kind === "prose") {
            if (held.began !== undefined) restored.began = held.began;
            if (restored.took === undefined && held.took !== undefined) restored.took = held.took;
            restored.writing = held.writing && restored.writing;
          }
          if (held) next.replace(held, restored);
          else next.add(restored);
          owned.add(restored.key);
        }
        next.placedEntries.set(place, owned);
        next.eventBases.set(place, base);
        next.eventSources.set(place, event);
        next.folded = at + 1;
      } else next.event(event, place);
    }
    next.ignoreHistoricalRequests = false;
    next.extend([...pending]);
    next.replaying = false;
    next.preferredKeys = null;
    next.stale = false;
    next.endings = this.endings; next.lastEnding = this.lastEnding;
    // Words on their way stay until the record of them is there: the one
    // with their own delivery, or, while neither names one, words alike
    // that the records gained just now. Words said before in the same words
    // are not theirs.
    for (const sent of this.entries.filter((entry): entry is Heard => entry.kind === "heard" && entry.sending !== undefined)) {
      const arrived = next.entries.some((entry) => {
        if (entry.kind !== "heard" || entry.sending !== undefined) return false;
        if (typeof sent.delivery === "string" && typeof entry.delivery === "string") return entry.delivery === sent.delivery;
        return entry.text.trim() === sent.text.trim() && !this.byKey.has(entry.key);
      });
      if (!arrived) next.add(sent);
    }
    next.entries = next.entries.map((entry) => {
      const prior = this.byKey.get(entry.key);
      if (prior && sameProjection(prior, entry)) { prior.at = entry.at; return prior; }
      return entry;
    });
    next.reindex();
    return next;
  }
  private pendingIds: Set<string> | null = null;
  private ignoreHistoricalRequests = false;
  evictPlaces(places: readonly string[]): void {
    if (places.length === 0) return;
    const gone = new Set<string>();
    for (const place of places) {
      for (const key of this.placedEntries.get(place) ?? []) gone.add(key);
      this.placedEntries.delete(place);
      this.eventBases.delete(place);
      this.eventSources.delete(place);
    }
    for (const keys of this.placedEntries.values()) for (const key of keys) gone.delete(key);
    if (![...gone].some((key) => this.byKey.has(key))) return;
    this.remove((entry) => gone.has(entry.key));
    const turns = new Set(this.entries.flatMap((entry) => "turn" in entry && entry.turn ? [entry.turn] : []));
    for (const map of [this.began, this.heardIn, this.stretches]) for (const turn of map.keys()) if (!turns.has(turn)) map.delete(turn);
    const items = new Set(this.entries.flatMap((entry) => "turn" in entry && "item" in entry && typeof entry.turn === "string" ? [itemEntryKey(entry.turn, entry.item)] : []));
    for (const key of this.over) if (!items.has(key)) this.over.delete(key);
    for (const turn of this.endedTurns) if (!turns.has(turn)) this.endedTurns.delete(turn);
  }
  placeOf(key: string): string | null {
    for (const [place, entries] of this.placedEntries) if (entries.has(key)) return place;
    return null;
  }
  /** The entries placed at each place, in the order `placeOf` reads them. */
  placements(): ReadonlyMap<string, ReadonlySet<string>> {
    return this.placedEntries;
  }
  livePending(event: SessionEvent): void {
    if (event.event !== "changed") return;
    const change = event.change;
    if (change.kind === "request") {
      this.pendingIds?.add(askKey(change.id));
      const existing = this.ask(change.id);
      if (existing) {
        if (existing.turn !== event.turn || existing.request !== change.request || JSON.stringify(existing.params) !== JSON.stringify(change.params)) {
          existing.turn = event.turn; existing.request = change.request; existing.params = change.params;
          existing.state = { state: "waiting" }; existing.version += 1; this.touch(existing);
        }
        return;
      }
    }
    this.event(event);
    if (change.kind === "resolved") {
      this.pendingIds?.delete(askKey(change.id));
      this.remove((entry) => entry.kind === "ask" && askKey(entry.id) === askKey(change.id));
    }
  }
  private currentPlace = "";
  private placed = false;
  event(event: SessionEvent, place?: string): void {
    this.current = this.folded;
    this.placed = place !== undefined;
    this.currentPlace = place ?? `${this.scope}@${this.current}`;
    this.folded += 1;
    const before = this.clock;
    this.read(event);
    if (place !== undefined) {
      this.eventSources.set(place, event);
      let base: Entry | undefined;
      if (event.event === "finished" || event.event === "began" || event.event === "updated") base = event.item.id ? this.find(event.turn, event.item.id) : this.byKey.get(`n:${place}`);
      else if (event.event === "grew") base = this.find(event.turn, event.item);
      else if (event.event === "heard") base = [...this.entries].reverse().find((entry) => entry.kind === "heard" && entry.stamp > before);
      if (base) this.eventBases.set(place, [eventBase(event, base)]);
      else this.eventBases.delete(place);
      if (event.event === "grew") {
        const entry = this.find(event.turn, event.item);
        if (entry && !this.placedEntries.has(place)) this.placedEntries.set(place, new Set([entry.key]));
      } else this.placedEntries.set(place, new Set(this.entries.filter((entry) => entry.stamp > before).map((entry) => entry.key)));
    }
  }

  /** How many events of its part this fold has read. */
  get size(): number {
    return this.folded;
  }

  /** An event told after this fold's part about something in it, as the
   *  facts a run's job tells as it goes on: read in place, the part still
   *  ending where it did. */
  late(event: SessionEvent): void {
    this.lateReading = true;
    try {
      this.read(event);
    } finally {
      this.lateReading = false;
    }
  }
  /** What is read comes after this fold's part: it changes what is there,
   *  and adds nothing in its place. */
  private lateReading = false;

  private read(event: SessionEvent): void {
    const at = Date.parse(event.at);
    switch (event.event) {
      case "turn_started":
        if (!this.running.has(event.turn) && !this.replaying) this.started(event.turn);
        this.running.add(event.turn);
        if (!this.began.has(event.turn)) this.began.set(event.turn, at);
        this.clock += 1;
        return;
      case "realtime":
      case "realtime_grew":
        return;
      case "heard": {
        // Without the World's record of who spoke, the user's words still
        // carry the head note the World wrapped them in.
        const user = event.from ? null : userWords(event.text);
        const heard = user ? { ...event, from: { kind: "user" as const, machine: user.machine }, text: user.body } : event;
        // Words a stop waits for, taken into a turn a reading did not show:
        // that turn is the one asked to stop.
        if (heard.turn && !this.replaying && this.halted?.size === 0 && this.haltedUnseen && this.takesHalted(heard.delivery, heard.text.trim())) {
          this.haltedUnseen = false;
          this.haltTurn(heard.turn);
        }
        // Words taken into a turn say the turn is under way, whichever of
        // the two the World tells first.
        if (heard.turn) this.underWay(heard.turn, at);
        // Words sent from here are now heard: they keep their place. The
        // bubble shows them without the blank lines around them, while a
        // shared input sends them as typed (a phone's Enter adds a line).
        // Words once said not to be known to have gone in did: the host
        // tells them now (`world-concurrency.md`, 一句话从发出到落定).
        if (heard.delivery) this.remove((entry) => entry.kind === "uncertain" && entry.key === `u:${heard.delivery}`);
        const identified = this.placed ? this.entries.find((entry): entry is Heard => entry.kind === "heard" &&
          (!!heard.delivery && entry.delivery === heard.delivery || !!heard.message?.id && entry.messageId === heard.message.id)) : undefined;
        if (identified) {
          identified.text = heard.text; identified.turn = heard.turn; identified.from = heard.from; identified.time = at;
          if (heard.delivery) identified.delivery = heard.delivery;
          if (heard.message?.id) identified.messageId = heard.message.id;
          if (heard.notice) identified.notice = heard.notice;
          const attached = this.attachedOf(heard);
          if (attached.length) { identified.attached = attached; identified.kept = heard.message?.attachments ?? []; }
          delete identified.sending; delete identified.intoTurn;
          this.reindex(); this.touch(identified);
          return;
        }
        if (heard.from?.kind === "user") {
          const said = heard.text.trim();
          // Known by the id the World gave them where it did, else by what
          // they say.
          const sent =
            this.entries.find((entry): entry is Heard => entry.kind === "heard" && entry.sending !== undefined && !!heard.delivery && entry.delivery === heard.delivery) ??
            this.entries.find((entry): entry is Heard => entry.kind === "heard" && entry.sending !== undefined && entry.text.trim() === said);
          if (sent) {
            delete sent.sending;
            delete sent.intoTurn;
            sent.turn = heard.turn;
            sent.from = heard.from;
            sent.at = this.current;
            sent.time = at;
            // Its files as the World kept them, which its pictures are drawn
            // from: sent, they were known only by name.
            const attached = this.attachedOf(heard);
            if (attached.length) {
              sent.attached = attached;
              sent.kept = heard.message?.attachments ?? [];
            }
            // Heard now: it stands where the session took it in, after what
            // the turn did before it read them.
            this.entries.splice(this.entries.indexOf(sent), 1);
            this.entries.push(sent);
            this.reindex();
            this.touch(sent);
            return;
          }
        }
        const turnKey = heard.turn ?? "";
        const known =
          !this.placed && heard.turn !== null &&
          (this.byTurn.get(heard.turn) ?? []).some((entry) => entry.kind === "heard" && entry.text.trim() === heard.text.trim());
        if (!known) {
          const attached = this.attachedOf(heard);
          const ordinal = this.heardIn.get(turnKey) ?? 0;
          this.heardIn.set(turnKey, ordinal + 1);
          // Words in a turn are told apart by their place in it; words in no
          // turn, by their place in this fold.
          const key = event.delivery
            ? `h:${event.delivery}`
            : heard.message?.id
              ? `h:m:${heard.message.id}`
              : this.placed ? `h:${this.currentPlace}` : turnKey
                ? `h:${turnKey}:${ordinal}`
                : `h:${this.currentPlace}`;
          const shown = this.byKey.has(key) ? `${key}:${this.currentPlace}` : key;
          this.add({
            kind: "heard",
            key: shown,
            stamp: 0,
            at: 0,
            time: at,
            from: heard.from,
            text: heard.text,
            turn: heard.turn,
            ...(heard.delivery ? { delivery: heard.delivery } : {}),
            ...(heard.message?.id ? { messageId: heard.message.id } : {}),
            ...(attached.length ? { attached, kept: heard.message?.attachments ?? [] } : {}),
            ...(event.event === "heard" && event.notice ? { notice: event.notice } : {}),
          });
          if (!this.replaying && heard.from?.kind === "user") {
            this.unclaimed.push({ key: shown, text: heard.text.trim(), seq: ++this.heardSeq });
            if (this.unclaimed.length > 64) this.unclaimed.shift();
          }
        }
        return;
      }
      case "began":
      case "updated":
        this.underWay(event.turn, at);
        // Asked to stop: what it writes no longer grows here, and what it
        // starts shows cut off at once; its host's final words still come.
        if (this.halted?.has(event.turn) && !this.replaying) {
          if (writes(event.item)) return;
          this.item(event.turn, event.item, at, false);
          this.cutShort(event.turn, event.item.id, at);
          return;
        }
        this.item(event.turn, event.item, at, false);
        return;
      case "finished":
        this.item(event.turn, event.item, at, true);
        return;
      case "grew": {
        this.underWay(event.turn, at);
        if (this.halted?.has(event.turn) && !this.replaying) return;
        const shown = this.find(event.turn, event.item);
        if (shown?.kind === "prose" && event.part === "text") {
          if (shown.prose === "thinking" && !this.thoughtWords) {
            shown.heads = (shown.heads ?? "") + event.text;
            shown.version += 1;
            this.touch(shown);
            return;
          }
          shown.text += event.text;
          shown.version += 1;
          this.touch(shown);
          if (shown.prose === "message") this.lastWordsTaken(event.turn, shown.text);
        } else if (!shown && event.part === "text") {
          // A message's first words can come before any word of it.
          this.lastWordsTaken(event.turn, event.text);
          this.overtaken(event.turn);
          this.add({
            kind: "prose",
            key: itemEntryKey(event.turn, event.item),
            stamp: 0,
            at: 0,
            item: event.item,
            turn: event.turn,
            prose: "message",
            text: event.text,
            writing: true,
            version: 0,
          });
        }
        return;
      }
      case "steps": {
        const shown = lastOf(this.byTurn.get(event.turn) ?? [], (entry) => entry.kind === "steps");
        if (shown?.kind === "steps") {
          shown.steps = event.steps;
          shown.note = event.note;
          this.touch(shown);
        } else {
          this.add({ kind: "steps", key: `p:${event.turn}`, stamp: 0, at: 0, turn: event.turn, steps: event.steps, note: event.note });
        }
        return;
      }
      case "context":
        this.context = { used: event.used, window: event.window, at };
        return;
      case "titled":
        this.title = event.title;
        return;
      case "turn_ended": {
        this.running.delete(event.turn);
        this.began.delete(event.turn);
        if (this.halted?.delete(event.turn) && this.halted.size === 0) this.halted = null;
        this.machineHalted.delete(event.turn);
        this.clock += 1;
        if (this.endedTurns.has(event.turn)) return;
        if (!this.replaying) this.endings += 1;
        this.lastEnding = event.end;
        for (const entry of this.byTurn.get(event.turn) ?? []) {
          if (entry.kind === "tool" && entry.ended === null) {
            entry.ended = { at, use: "cut" };
            this.touch(entry);
          }
          if (entry.kind === "prose" && entry.writing) {
            entry.writing = false;
            this.touch(entry);
          }
        }
        this.endedTurns.add(event.turn);
        this.lastWords(event.turn, event.end);
        this.add({ kind: "ended", key: `e:${event.turn}`, stamp: 0, at: 0, turn: event.turn, end: event.end });
        return;
      }
      case "delivery_uncertain":
        // Told once for each delivery; a reading and the live event may
        // both carry it.
        if (!this.byKey.has(`u:${event.delivery}`)) this.add({ kind: "uncertain", key: `u:${event.delivery}`, stamp: 0, at: 0, why: event.why });
        return;
      case "health":
        this.health(event.health);
        return;
      case "changed": {
        const change = event.change;
        if (this.ignoreHistoricalRequests && (change.kind === "request" || change.kind === "resolved")) return;
        if (change.kind === "request") {
          if (this.pendingIds !== null && !this.pendingIds.has(askKey(change.id))) return;
          if (!forPeople(change.request) || this.ask(change.id)) return;
          this.add({
            kind: "ask",
            key: `a:${askKey(change.id)}`,
            stamp: 0,
            at: 0,
            id: change.id,
            turn: event.turn,
            request: change.request,
            params: change.params,
            state: { state: "waiting" },
            version: 0,
          });
          return;
        }
        if (change.kind === "resolved") {
          const ask = this.ask(change.id);
          if (ask && ask.state.state !== "answered") this.settle(ask, { state: "gone" });
          return;
        }
        if (change.kind === "forked") {
          if (!this.entries.some((entry) => entry.kind === "forked")) {
            // What the record held before this point it carried over: its
            // last answer to the user and the words that asked for it stand
            // on the card, the rest folds behind it.
            const answer = [...this.entries].reverse().find((entry) => (entry.kind === "prose" && entry.prose === "message") || (entry.kind === "tool" && sayOf(entry) !== null));
            const turn = answer && "turn" in answer ? answer.turn : null;
            const said = [...this.entries].reverse().find((entry): entry is Heard => entry.kind === "heard" && !entry.notice && (!entry.from || entry.from.kind === "user") && (turn === null || entry.turn === turn));
            const answered = answer?.kind === "prose" ? answer.text : answer?.kind === "tool" ? (sayOf(answer)?.text ?? null) : null;
            const forked: Entry = { kind: "forked", key: "f", stamp: ++this.clock, at: this.current, source: change.source, through: change.through ?? null, said: said?.text ?? null, answer: answered };
            this.inherited = this.entries.length ? new Set(this.entries.map((entry) => entry.key)) : null;
            this.entries.unshift(forked);
            this.index(forked);
          }
          return;
        }
        // A control the node took to do later moved on: its latest word.
        if (change.kind === "metadata") {
          const row = (change.metadata as { fastctx_control?: PendingControl } | null)?.fastctx_control;
          if (row && typeof row.id === "string") {
            this.controlStates.set(row.id, row);
            this.controlsChanged = true;
            this.command(row);
          }
          return;
        }
        if (change.kind === "history_changed" || change.kind === "history_cut") {
          this.stale = true;
          return;
        }
        if (change.kind === "model_response" && change.usage) {
          this.answeredBy = change.usage.model;
          return;
        }
        if (change.kind === "message_usage") {
          if (change.model) this.answeredBy = change.model;
          this.requests.claude(at, change.response, change.usage, event.turn, change.items);
          this.claudeThought(event.turn, change.items, change.response, change.usage);
          return;
        }
        if (change.kind === "usage") {
          this.requests.codex(at, change.usage, event.turn);
          this.codexThought(event.turn, change.usage);
          return;
        }
        // The host's own agents move on their own: their rows follow them
        // (`agentStamp`), and what the conversation holds does not change.
        if (change.kind === "agents") {
          this.agents.listed(change.agents, at);
          return;
        }
        if (change.kind === "agent_started") {
          // A start told late, as the host's record has it, began with
          // the call that started it.
          const call = change.agent.call ? this.byCall.get(change.agent.call) : undefined;
          this.toAncestors(this.agents.started(change.agent, call?.began ?? at), event);
          return;
        }
        if (change.kind === "agent_event") {
          this.toAncestors(this.agents.event(change.agent_id, { at: event.at, ...change.happened } as SessionEvent), event);
          return;
        }
        if (change.kind === "agent_ended") {
          this.toAncestors(this.agents.ended(change.agent_id, change.end, at), event);
          // Where the conversation was told of it, a line says so.
          const key = `t:${change.agent_id}:${event.at}`;
          if (change.end.told && !this.lateReading && !this.byKey.has(key)) {
            this.add({ kind: "agent-told", key, stamp: 0, at: 0, agent: change.agent_id, end: change.end });
          }
          return;
        }
        if (change.kind === "commands_changed") {
          this.offersChanged = true;
          return;
        }
        if (change.kind === "run_facts_updated") {
          // A run one of the agents made is told to that agent.
          const agent = this.byCall.has(change.update.call_id) ? undefined : this.agents.ofTool(change.update.call_id);
          if (agent) this.toAncestors(this.agents.event(agent.id, event), event);
          else this.runFacts(change.update);
          return;
        }
        const health = changeHealth(change);
        if (health) this.health(health);
        return;
      }
    }
  }

  /** Something happening live in `turn` says it is under way, even where
   *  its start was not told (as a watch begun as it began may miss it).
   *  Only live: a history read again may hold turns whose end was never
   *  told, and none of them is under way for that. */
  private underWay(turn: string, at: number): void {
    if (this.replaying || this.endedTurns.has(turn) || this.running.has(turn)) return;
    this.started(turn);
    this.running.add(turn);
    if (!this.began.has(turn)) this.began.set(turn, at);
    this.clock += 1;
  }

  /** A turn is seen to start. The one words on their way start is the one
   *  asked to stop; any other is new, and shown as it goes. Once a reading
   *  did not show those words taken, a turn starting is not known to be
   *  theirs: only the words' own `heard` binds the stop then. */
  private started(turn: string): void {
    if (!this.halted) return;
    if (this.halted.size === 0) {
      if (!this.haltedUnseen) this.halted.add(turn);
    } else if (!this.halted.has(turn)) this.halted = null;
  }

  /** Whether words heard with `delivery`, saying `said`, are words a stop
   *  waits for: by their delivery where they have one, by what they say
   *  only where they have none. */
  private takesHalted(delivery: string | null | undefined, said: string): boolean {
    return this.haltedWords.some((word) =>
      typeof word.delivery === "string" ? delivery === word.delivery : word.text === said,
    );
  }

  private ask(id: JsonValue): Extract<Entry, { kind: "ask" }> | undefined {
    const found = this.byKey.get(`a:${askKey(id)}`);
    return found?.kind === "ask" ? found : undefined;
  }

  /** How a request now stands: being answered, answered, or no longer
   *  waited on. */
  settle(ask: Extract<Entry, { kind: "ask" }>, state: AskState): void {
    ask.state = state;
    ask.version += 1;
    this.touch(ask);
  }

  /** Whether the session waits on the user for anything. */
  waiting(): boolean {
    return this.derived().waiting;
  }

  /** Something of the session's own health, shown once however often the
   *  host repeats it in a row. */
  private health(health: Health): void {
    const last = this.entries[this.entries.length - 1];
    if (last?.kind === "health" && last.health.kind === health.kind && last.health.text === health.text) return;
    this.add({ kind: "health", key: `x:${this.currentPlace}`, stamp: 0, at: 0, health });
  }

  /** A turn that ended telling its last words, where no message of it after
   *  its last tool shows them — its machine told the end first, and the
   *  record of the words is on its way (codex-d0397b85) — shows them from
   *  its end until that message comes and takes their place (`item`). A
   *  message of other words before them, as one telling how the work goes,
   *  is not they (`sameWords`). */
  private lastWords(turn: string, end: TurnEnd): void {
    const words = end.last_message?.trim();
    if (!words) return;
    const entries = this.byTurn.get(turn) ?? [];
    let tool = -1;
    entries.forEach((entry, at) => {
      if (entry.kind === "tool") tool = at;
    });
    if (entries.some((entry, at) => at > tool && entry.kind === "prose" && entry.prose === "message" && sameWords(entry.text, words))) return;
    this.add({ kind: "prose", key: lastWordsKey(turn), stamp: 0, at: 0, item: LAST_WORDS, turn, prose: "message", text: words, writing: false, version: 0 });
  }

  /** A message of `turn` came, saying `text` so far: the last words shown
   *  from its end, where they are its words, give it their place. */
  private lastWordsTaken(turn: string, text: string): void {
    const key = lastWordsKey(turn);
    const shown = this.byKey.get(key);
    if (shown?.kind === "prose" && sameWords(text, shown.text)) this.remove((entry) => entry.key === key);
  }

  /** An item began or finished: shown where it first appeared, as it now
   *  stands. */
  private item(turn: string, native: import("../world/events").Item, at: number, finished: boolean): void {
    if (native.kind === "message") this.lastWordsTaken(turn, native.text);
    const startedAt = native.started_at ? Date.parse(native.started_at) : at;
    const id = native.id;
    if (id && native.draft) this.renamed(turn, native.draft, id);
    const key = id ? itemEntryKey(turn, id) : `n:${this.currentPlace}`;
    if (id && !finished && this.over.has(key)) return;
    if (id && finished) this.over.add(key);
    const said = native.kind === "message" || native.kind === "thinking" || native.kind === "plan" || native.kind === "tool" || native.kind === "compaction";
    if (said && !(id && this.find(turn, id))) this.overtaken(turn);
    let fresh: Entry;
    const questions = native.kind === "message" ? questionsOf(native.detail) : null;
    if (questions && native.kind === "message") {
      const shown = id ? this.find(turn, id) : undefined;
      if (shown?.kind === "question") return;
      const asked: Entry = { kind: "question", key, stamp: 0, at: 0, item: id, turn, text: askedText(native.text, questions) };
      if (shown) this.replace(shown, asked);
      else this.add(asked);
      return;
    }
    switch (native.kind) {
      case "message":
      case "thinking":
      case "plan":
        fresh = {
          kind: "prose",
          key,
          stamp: 0,
          at: 0,
          item: id,
          turn,
          prose: native.kind,
          text: native.kind === "thinking" && !this.thoughtWords ? "" : native.text,
          ...(native.kind === "thinking" && !this.thoughtWords && native.text ? { heads: native.text } : {}),
          writing: !finished,
          version: 0,
          ...(native.kind === "thinking" ? { began: startedAt, ...(native.duration_ms !== undefined ? { took: native.duration_ms } : {}) } : {}),
        };
        break;
      case "agent_activity":
        // Codex's second way of running agents tells of them as items of the
        // turn: one started is drawn as any agent started is, one that
        // ended as the conversation being told so.
        if (native.activity === "started") {
          fresh = { kind: "tool", key, stamp: 0, at: 0, item: id, turn, tool: startedTool(native.agent, native.path), began: startedAt, ended: { at, use: "done" } };
        } else if (native.activity === "completed" || native.activity === "interrupted") {
          fresh = { kind: "agent-told", key, stamp: 0, at: 0, item: id, turn, agent: native.agent, end: { how: native.activity, task: taskOf(native.path), told: true } };
        } else fresh = { kind: "activity", key, stamp: 0, at: 0, item: id, turn, text: `${native.path}: ${native.activity}` };
        break;
      case "tool": {
        const { kind: _kind, id: _id, started_at: _started, detail: _detail, ...tool } = native;
        const use: Use | null =
          tool.status === "running" ? null : tool.status === "unknown" ? "unknown" : (tool.status as Use);
        fresh = {
          kind: "tool",
          key,
          stamp: 0,
          at: 0,
          item: id,
          turn,
          tool,
          began: startedAt,
          ended: finished && use ? { at, use } : null,
        };
        break;
      }
      case "compaction":
        // Shown once it is done: the context it leaves is what counts.
        if (!finished) return;
        fresh = { kind: "compaction", key, stamp: 0, at: 0, item: id, turn };
        break;
      case "hidden":
        // A completed item can remove a transient placeholder.
        if (id && this.find(turn, id)) this.remove((entry) => "item" in entry && entry.item === id && entry.turn === turn);
        return;
      case "other":
        if (!finished) return;
        fresh = { kind: "other", key, stamp: 0, at: 0, item: id, turn, hostKind: native.host_kind };
        break;
    }
    if (!id) {
      this.add(fresh);
      return;
    }
    const shown = this.find(turn, id);
    if (!shown) {
      this.add(fresh);
      return;
    }
    if (shown.kind === "prose" && fresh.kind === "prose") {
      // Thinking whose words are not shown still says the section it is in.
      const heads = fresh.heads;
      const headed = heads !== undefined && heads !== shown.heads;
      if (headed) {
        shown.heads = heads;
        shown.version += 1;
      }
      // A message still being written keeps what it has until its whole
      // text comes.
      if (shown.writing && !finished && fresh.text === "") {
        if (headed) this.touch(shown);
        return;
      }
      if (shown.text !== fresh.text) shown.version += 1;
      shown.text = fresh.text;
      // What has stopped being written does not start again.
      shown.writing = shown.writing && fresh.writing;
      if (fresh.took !== undefined) shown.took = fresh.took;
      if (shown.began === undefined && fresh.began !== undefined) shown.began = fresh.began;
      this.touch(shown);
      return;
    }
    if (shown.kind === "tool" && fresh.kind === "tool") {
      // Facts gathered while it ran stand until the item brings its own.
      shown.tool = fresh.tool.run || !shown.tool.run ? fresh.tool : { ...fresh.tool, run: shown.tool.run };
      shown.ended = fresh.ended ?? shown.ended;
      this.touch(shown);
      return;
    }
    this.replace(shown, { ...fresh, key: shown.key } as Entry);
  }

  /** The model says one thing at a time: thinking still shown under way
   *  when the next thing it says begins is over, whatever word of its end
   *  is still to come. Agents' comings and goings are told beside the
   *  model's words and end nothing of them. */
  private overtaken(turn: string): void {
    for (const entry of this.byTurn.get(turn) ?? []) {
      if (entry.kind !== "prose" || entry.prose !== "thinking" || !entry.writing) continue;
      entry.writing = false;
      this.touch(entry);
    }
  }

  /** Thinking tokens by model response, as the hosts told them. */
  private thoughtTokens = new Map<string, number>();
  /** The last running total Codex told, so a total told again names no
   *  new response. */
  private codexTotal = 0;
  private codexResponses = 0;

  /** Claude Code's usage of one stored row: the response it was part of,
   *  the row's blocks, and what the response cost, its thinking counted
   *  apart (`output_tokens_details.thinking_tokens`). */
  private claudeThought(turn: string | null, items: readonly string[], response: string | null, usage: JsonValue): void {
    const tokens = property(property(usage, "output_tokens_details") ?? null, "thinking_tokens");
    if (!turn || !response || typeof tokens !== "number") return;
    this.thoughtTokens.set(response, tokens);
    for (const id of items) {
      const entry = this.find(turn, id);
      if (entry?.kind !== "prose" || entry.prose !== "thinking" || entry.response === response) continue;
      entry.response = response;
      this.touch(entry);
    }
  }

  /** Codex's usage after a model response: `last` is that response's own.
   *  The thinking it counts is the turn's thinking finished since the last
   *  response. A total no larger than the last is one told again, as on
   *  joining a thread, and names no response. */
  private codexThought(turn: string | null, usage: JsonValue): void {
    const total = property(property(usage, "total") ?? null, "totalTokens");
    const tokens = property(property(usage, "last") ?? null, "reasoningOutputTokens");
    if (!turn || typeof total !== "number" || typeof tokens !== "number" || total <= this.codexTotal) return;
    this.codexTotal = total;
    const response = `codex:${this.scope}:${++this.codexResponses}`;
    this.thoughtTokens.set(response, tokens);
    for (const entry of this.byTurn.get(turn) ?? []) {
      if (entry.kind !== "prose" || entry.prose !== "thinking" || entry.writing || entry.response) continue;
      entry.response = response;
      this.touch(entry);
    }
  }

  /** The thinking tokens of a run of thinking: each response it was part
   *  of counted once; null unless every part's response told them. */
  thinkingTokens(run: readonly Entry[]): number | null {
    const responses = new Set<string>();
    for (const entry of run) {
      if (entry.kind !== "prose" || !entry.response || !this.thoughtTokens.has(entry.response)) return null;
      responses.add(entry.response);
    }
    let sum = 0;
    for (const response of responses) sum += this.thoughtTokens.get(response)!;
    return sum;
  }

  /** Puts `fresh` where `shown` stood. */
  private replace(shown: Entry, fresh: Entry): void {
    const index = this.entries.indexOf(shown);
    fresh.at = shown.at;
    this.entries[index] = fresh;
    this.reindex();
    this.touch(fresh);
  }

  /** What a run's jobs told while it ran, by job: the latest of each. */
  private sequences = new Map<string, number>();

  /** Facts a run's job told while it ran: files it wrote, how it stands,
   *  where its output is. Writes join those already known; how the run
   *  stands is its own job's to say, the latest word kept. */
  private runFacts(update: JobFacts): void {
    const entry = this.byCall.get(update.call_id);
    if (!entry) return;
    const job = `${update.call_id}|${update.machine}|${update.job}`;
    const fresh = update.sequence > (this.sequences.get(job) ?? -1);
    if (fresh) this.sequences.set(job, update.sequence);
    const run: RunFacts = entry.tool.run
      ? { ...entry.tool.run, writes: [...entry.tool.run.writes], visited: [...entry.tool.run.visited] }
      : { machine: update.machine, state: update.state ?? { kind: "running" }, job: update.job, output: update.output, visited: [], writes: [], spans: [] };
    if (update.primary && fresh) {
      run.machine = update.machine;
      run.job = update.job;
      if (update.state) run.state = update.state;
      if (update.output) run.output = update.output;
    }
    for (const visit of update.visited) if (!run.visited.some(([machine, job]) => machine === visit[0] && job === visit[1])) run.visited.push(visit);
    for (const write of update.writes) if (!run.writes.some((known) => known.id === write.id)) run.writes.push(write);
    if (update.steps?.length) run.steps = mergeSteps(run.steps ?? [], update.steps);
    entry.tool = { ...entry.tool, run };
    this.touch(entry);
  }

  /** Whether a turn has ended, as far as is shown. */
  turnEnded(turn: string): boolean {
    return this.endedTurns.has(turn);
  }

  /** Whether this fold holds anything of `turn`. */
  holds(turn: string): boolean {
    return this.byTurn.has(turn);
  }

  /** `turn`'s entries, in the order they came. */
  turnEntries(turn: string): readonly Entry[] {
    return this.byTurn.get(turn) ?? [];
  }

  /** Whether this fold holds the tool a run's facts are about. */
  holdsCall(call: string): boolean {
    return this.byCall.has(call);
  }

  /** The entry of a call, by the host's item. */
  call(call: string): ToolEntry | undefined {
    return this.byCall.get(call);
  }

  /** Lets go of what the events before `before` made: whole turns, the
   *  fold's part now beginning there. Words still being sent and requests
   *  still waiting stay. */
  forget(before: number): void {
    const kept = this.entries.filter(
      (entry) => entry.at >= before || (entry.kind === "heard" && entry.sending !== undefined) || (entry.kind === "ask" && entry.state.state === "waiting"),
    );
    if (kept.length === this.entries.length) return;
    this.entries = kept;
    this.reindex();
    for (const turn of [...this.endedTurns]) if (!this.byTurn.has(turn)) this.endedTurns.delete(turn);
    for (const turn of [...this.stretches.keys()]) if (!this.byTurn.has(turn)) this.stretches.delete(turn);
    for (const turn of [...this.heardIn.keys()]) if (turn && !this.byTurn.has(turn)) this.heardIn.delete(turn);
    for (const entry of this.entries) entry.at -= before;
    this.folded -= before;
    this.current = Math.max(0, this.current - before);
  }

  /** The last message a turn said, by key. */
  lastMessageOf(turn: string): string | null {
    return this.derived().lastMessage.get(turn) ?? null;
  }

  /** The turn whose actions stay shown: the last that ended. */
  latestEnded(): string | null {
    return this.derived().latest;
  }

  private find(turn: string, id: string): Keyed | undefined {
    return this.byItem.get(itemKey(turn, id));
  }

  /** An item its host kept under a name of its own after it streamed as
   *  `draft`: the drawn draft becomes that item, where it stands; a copy
   *  already read from the host's record stays the one shown. */
  private renamed(turn: string, draft: string, id: string): void {
    const shown = this.find(turn, draft);
    if (!shown) return;
    if (this.find(turn, id)) {
      this.remove((entry) => entry === shown);
      return;
    }
    shown.item = id;
    this.reindex();
    this.touch(shown);
  }

  /** An entry by its key. */
  get(key: string): Entry | undefined {
    return this.byKey.get(key);
  }

  /** The files that came with words, as the World kept them; their names
   *  are known by their paths from here on. */
  private attachedOf(heard: Extract<SessionEvent, { event: "heard" }>): ShownAttachment[] {
    for (const file of heard.message?.attachments ?? []) this.attachments.set(file.path.replaceAll("\\", "/"), file.name);
    return [
      ...(heard.message?.attachments ?? []).map((file) => ({
        name: file.name,
        size: file.size,
        image: file.media_type.startsWith("image/"),
        preview: null,
        kept: file,
      })),
      ...(heard.message?.images ?? []).map((picture) => ({
        name: picture.name ?? "",
        size: null,
        image: true,
        preview: null,
        recorded: picture.source,
      })),
    ];
  }

  /** How far the user's words heard so far reach: words that join the
   *  queue now are heard after this. */
  heardMark(): number {
    return this.heardSeq;
  }

  /** Whether the agent already heard these words, which the queue says are
   *  only now on their way: heard after `mark`, the place they joined the
   *  queue, or, with no mark, as the newest words the user said. A bubble
   *  for them would show them twice. The words heard are taken for these
   *  ones, once. */
  heardAlready(text: string, mark: number | null): boolean {
    const said = text.trim();
    let key: string | undefined;
    if (mark === null) {
      const newest = [...this.entries].reverse().find((entry): entry is Heard => entry.kind === "heard" && entry.sending === undefined && entry.from?.kind === "user");
      if (newest && newest.text.trim() === said && !newest.key.startsWith("s:")) key = newest.key;
    } else {
      key = this.unclaimed.find((one) => one.seq > mark && one.text === said)?.key;
    }
    if (key === undefined || this.claimedHeard.has(key)) return false;
    this.claimedHeard.add(key);
    this.unclaimed = this.unclaimed.filter((one) => one.key !== key);
    if (this.claimedHeard.size > 256) this.claimedHeard.delete(this.claimedHeard.values().next().value as string);
    return true;
  }

  /** Words the user is sending, shown at once; returns their key. */
  sending(text: string, here: MachineName | null, attached: ShownAttachment[] = []): string {
    // What the user says after asking to stop starts the turn after the one
    // stopped (its node waits for that turn's end), so the stopped turn
    // stays stopped here: its later words still do not grow it.
    const key = `s:${++this.sends}`;
    this.add({
      kind: "heard",
      key,
      stamp: 0,
      at: 0,
      time: Date.now(),
      from: here ? { kind: "user", machine: here } : null,
      text,
      turn: null,
      sending: { state: "going" },
      ...(this.running.size > 0 ? { intoTurn: true } : {}),
      ...(attached.length ? { attached } : {}),
    });
    return key;
  }

  private offersChanged = false;
  private controlsChanged = false;
  /** Where each control the node took to do later stands, as its last
   *  event said. */
  private readonly controlStates = new Map<string, PendingControl>();

  /** A command as it stands: shown from when it began, one waiting in the
   *  queue above the input still, one withdrawn never. */
  private command(row: PendingControl): void {
    const key = `c:${row.id}`;
    const entry = this.byKey.get(key);
    if (entry?.kind === "command") {
      entry.row = row;
      this.touch(entry);
      return;
    }
    if (row.state === "waiting" || row.state === "cancelled") return;
    this.add({ kind: "command", key, stamp: 0, at: 0, row });
  }

  /** Whether a control the node took moved on since last asked. */
  takeControlsChanged(): boolean {
    const changed = this.controlsChanged;
    this.controlsChanged = false;
    return changed;
  }

  controlState(id: string): PendingControl | undefined {
    return this.controlStates.get(id);
  }

  /** Whether what the conversation offers changed since last asked. */
  takeOffersChanged(): boolean {
    const changed = this.offersChanged;
    this.offersChanged = false;
    return changed;
  }

  /** What is told of an agent another agent started is also what that one
   *  would be told in its own conversation, and the one that started it,
   *  and so on up: each agent's box shows the agents it started as the
   *  conversation shows its own. One not drawn from what came live reads
   *  them in its record. */
  private toAncestors(run: AgentRun, event: SessionEvent): void {
    const seen = new Set([run.id]);
    for (let parent = run.parent ? this.agents.get(run.parent) : undefined; parent && !seen.has(parent.id); parent = parent.parent ? this.agents.get(parent.parent) : undefined) {
      seen.add(parent.id);
      if (parent.whole) this.agents.event(parent.id, event);
    }
  }

  /** The agent a call started, where it is one that starts the host's own
   *  agents and the agent is known here. */
  agentOf(entry: Extract<Entry, { kind: "tool" }>): AgentRun | null {
    if (!startsAgent(entry.tool)) return null;
    const started = this.agents.ofCall(entry.item)[0];
    if (started) return started;
    const named = entry.tool.does.as === "agent" ? entry.tool.does.agents?.[0] : undefined;
    return named ? (this.agents.get(named) ?? null) : null;
  }

  /** The agents a call names: those it waits for, speaks to or stops. */
  agentsNamed(entry: Extract<Entry, { kind: "tool" }>): AgentRun[] {
    if (entry.tool.does.as !== "agent") return [];
    return (entry.tool.does.agents ?? []).flatMap((name) => this.agents.named(name) ?? []);
  }

  /** The conversation could not begin: why, as it was given, under the
   *  first words. */
  failedToStart(why: string): void {
    this.remove((entry) => entry.key === "u:start");
    // The words stand as they were said; what became of them is said
    // beneath, once.
    for (const entry of this.entries) {
      if (entry.kind === "heard" && entry.sending) {
        delete entry.sending;
        delete entry.intoTurn;
        this.touch(entry);
      }
    }
    this.add({ kind: "uncertain", key: "u:start", stamp: 0, at: 0, why });
  }

  /** Whether any turn has been seen to begin or end. */
  hasTurns(): boolean {
    return this.running.size > 0 || this.endedTurns.size > 0;
  }

  /** Words shown as being sent that went into the queue instead. */
  unsend(key: string): void {
    this.settlers.delete(key);
    this.remove((entry) => entry.key === key);
    // Words that did not go start no turn: a stop waiting for them has
    // nothing of them to find, by id or by what they say.
    this.haltedWords = this.haltedWords.filter((word) => word.key !== key);
  }

  /** Keeps how the words being sent under `key` are sent again or taken
   *  away; none once nothing is left to do with them. */
  settles(key: string, settler: Settler | null): void {
    if (settler) this.settlers.set(key, settler);
    else this.settlers.delete(key);
  }

  /** How the words being sent under `key` are sent again or taken away. */
  settler(key: string): Settler | undefined {
    return this.settlers.get(key);
  }

  /** What became of words being sent: they are kept, with how they stand,
   *  until the session hears them. */
  sent(key: string, state: SendState): void {
    const entry = this.byKey.get(key);
    if (entry?.kind === "heard" && entry.sending) {
      entry.sending = state;
      this.touch(entry);
    }
  }

  /** The World answered for words being sent: `delivery` is their id where
   *  they went, `null` where it gave none. Words the World has answered for
   *  are no longer on their way to it, only to the session. */
  named(key: string, delivery: string | null): void {
    const entry = this.byKey.get(key);
    if (entry?.kind === "heard" && entry.sending) entry.delivery = delivery;
    // A stop asked before the World answered waits for them by it now.
    for (const word of this.haltedWords) if (word.key === key) word.delivery = delivery;
  }

  /** The tools of `turn`, which runs, that stood as cut off: they run on,
   *  as far as is known (`now`). */
  private uncut(turn: string): void {
    let changed = false;
    for (const entry of this.byTurn.get(turn) ?? []) {
      if (entry.kind === "tool" && entry.ended?.use === "cut") {
        entry.ended = null;
        this.touch(entry);
        changed = true;
      }
    }
    if (changed) this.clock += 1;
  }

  /** The World says no turn runs: what is still shown under way here
   *  ended unseen, and stands as cut off, whichever turn it was in. */
  noTurnRuns(): void {
    this.halted = null;
    this.machineHalted.clear();
    const at = Date.now();
    let changed = this.running.size > 0;
    for (const entry of this.entries) {
      if (entry.kind === "tool" && entry.ended === null) {
        entry.ended = { at, use: "cut" };
        this.touch(entry);
        changed = true;
      }
      if (entry.kind === "prose" && entry.writing) {
        entry.writing = false;
        this.touch(entry);
        changed = true;
      }
    }
    this.running.clear();
    this.began.clear();
    if (changed) this.clock += 1;
  }

  /** The user asked what runs to stop: it shows stopped at once, whatever
   *  its host takes to end it (`world-gui.md`, 停止键). Until the turn ends,
   *  what it writes no longer grows here and what it starts shows cut off;
   *  its items' final words, as its host keeps them, still come. Words still
   *  on their way stay as they are: the turn they start stops too. */
  halt(): void {
    this.halted = new Set(this.running);
    this.haltedUnseen = false;
    const sending = this.entries.filter((entry): entry is Heard => entry.kind === "heard" && entry.sending !== undefined);
    this.haltedWords = sending.map((entry) => ({ key: entry.key, delivery: entry.delivery, text: entry.text.trim() }));
    const at = Date.now();
    for (const entry of this.entries) {
      if (entry.kind === "tool" && entry.ended === null) {
        entry.ended = { at, use: "cut" };
        this.touch(entry);
      }
      if (entry.kind === "prose" && entry.writing) {
        entry.writing = false;
        this.touch(entry);
      }
    }
    this.clock += 1;
  }

  /** The words on their way a stop pressed now, seeing no turn, is for:
   *  those of `keys`, or all on their way. `words` are the ids the World
   *  answered for them with; `unnamed` counts those it has not answered
   *  for yet. Words it answered for without an id are in neither. */
  coming(keys?: readonly string[]): { keys: string[]; words: string[]; unnamed: number } {
    const found = { keys: [] as string[], words: [] as string[], unnamed: 0 };
    const entries = keys ? keys.map((key) => this.byKey.get(key)) : this.entries;
    for (const entry of entries) {
      if (entry?.kind !== "heard" || entry.sending === undefined || entry.sending.state === "settled") continue;
      found.keys.push(entry.key);
      if (typeof entry.delivery === "string") found.words.push(entry.delivery);
      else if (entry.delivery === undefined) found.unnamed += 1;
    }
    return found;
  }

  /** How many of the words a stop waits for the World has not answered for
   *  yet, heard by now or not (`named` keeps their ids as they come). */
  haltedUnnamed(): number {
    return this.haltedWords.filter((word) => word.delivery === undefined).length;
  }

  /** The ids the World answered for the words a stop waits for with,
   *  those that have one by now (`named` keeps them). */
  haltedIds(): string[] {
    return this.haltedWords.flatMap((word) => (typeof word.delivery === "string" ? [word.delivery] : []));
  }

  /** A turn the user asked to stop, as its machine tells every viewer: what
   *  it writes no longer grows here, and what it has under way shows cut
   *  off, as `halt` does for the device where the stop was pressed. */
  private haltTurn(turn: string): void {
    if (this.halted?.has(turn)) return;
    this.halted = new Set([...(this.halted ?? []), turn]);
    const at = Date.now();
    for (const entry of this.byTurn.get(turn) ?? []) {
      if (entry.kind === "tool" && entry.ended === null) {
        entry.ended = { at, use: "cut" };
        this.touch(entry);
      }
      if (entry.kind === "prose" && entry.writing) {
        entry.writing = false;
        this.touch(entry);
      }
    }
  }

  /** The stop did not reach the agent: it goes on, and what was held back
   *  here is read again. */
  unhalt(): void {
    if (!this.halted) return;
    // A turn its machine says was asked to stop, from any device, stays
    // stopped: only this device's own stop is called off.
    const told = [...this.halted].filter((turn) => this.machineHalted.has(turn));
    this.halted = told.length > 0 ? new Set(told) : null;
    this.stale = true;
    this.clock += 1;
  }

  /** A tool shown under way, started after the user asked to stop: cut off. */
  private cutShort(turn: string, item: string | undefined, at: number): void {
    const shown = item ? this.find(turn, item) : undefined;
    if (shown?.kind === "tool" && shown.ended === null) {
      shown.ended = { at, use: "cut" };
      this.touch(shown);
    }
  }

  /** What the conversation's machine holds of it now (`world-concurrency.md`,
   *  一句话从发出到落定): whether it works, on which turn, and which words it
   *  keeps on their way in. What shows under way here gives way to it: a
   *  turn it runs is under way; when none runs, nothing shown is; words the
   *  World answered for stand as it keeps them, or, kept no more, as said.
   *  Words whose host has not started for a minute say why on their way. */
  now(busy: boolean, turn: string | null, kept: KeptWords[], halted: string | null = null): void {
    // Words a reading did not show taken, which the machine keeps no more,
    // were taken by a turn this view did not see start: the stop of them
    // is spent rather than left for the next turn to start. Judged before
    // the turn the machine runs now is taken in, which is not theirs then.
    const words = this.haltedWords;
    const keeps = (word: { delivery: string | null | undefined }) => kept.some((held) => held.id === word.delivery);
    if (this.haltedUnseen && this.halted?.size === 0 && words.length > 0
      && words.every((word) => typeof word.delivery === "string") && !words.some(keeps)) {
      this.halted = null;
      this.haltedUnseen = false;
    }
    if (busy && turn) this.underWay(turn, Date.now());
    // A turn the machine says runs was not cut off: what an earlier word
    // that none ran, laid over a reading that held it, stood as cut in it
    // runs on.
    if (busy && turn && !this.endedTurns.has(turn)) this.uncut(turn);
    if (!busy) {
      // No turn runs, but words the stop waits for may still be on their
      // way: kept by the machine, or not yet answered for. The stop of them
      // stands until a turn takes them.
      const waiting = this.halted?.size === 0
        && words.some((word) => word.delivery === undefined || (typeof word.delivery === "string" && keeps(word)));
      const unseen = this.haltedUnseen;
      this.noTurnRuns();
      if (waiting) {
        this.halted = new Set();
        this.haltedUnseen = unseen;
      }
    }
    // A stop the user asked for, here or on another device, shows on every
    // viewer at once.
    if (busy && halted !== null && halted === turn) {
      this.machineHalted.add(halted);
      this.haltTurn(halted);
    }
    const keeping = new Map(kept.map((words) => [words.id, words]));
    for (const entry of this.entries) {
      if (entry.kind !== "heard" || !entry.sending || entry.delivery === undefined) continue;
      const settling = entry.sending.state === "going" || entry.sending.state === "queued" || (entry.sending.state === "landed" && entry.sending.how === "kept");
      if (!settling) continue;
      const words = entry.delivery === null ? undefined : keeping.get(entry.delivery);
      let next: SendState | null = null;
      if (words?.queued !== undefined)
        next = words.for_memory
          ? { state: "queued", ahead: words.queued, memory: true }
          : words.for_callee
            ? { state: "queued", ahead: words.queued, callee: true }
            : { state: "queued", ahead: words.queued };
      else if (words?.state === "waiting") next = { state: "landed", how: "kept", machine: entry.from?.kind === "user" ? entry.from.machine : "" };
      else if (words === undefined && !busy) next = { state: "settled" };
      else if (words !== undefined)
        next = {
          state: "going",
          ...(words.host_retry ? { retry: words.host_retry.reason } : {}),
          ...(words.starting ? { starting: words.starting } : {}),
        };
      if (next && JSON.stringify(next) !== JSON.stringify(entry.sending)) {
        entry.sending = next;
        this.touch(entry);
      }
    }
    this.clock += 1;
  }

  /** Whether a turn is under way: begun and not ended, or anything shown
   *  still happening. */
  busy(): boolean {
    return this.running.size > 0 || this.moving();
  }

  /** Whether the user asked what runs to stop, and it has not ended yet. */
  halting(): boolean {
    return this.halted !== null;
  }

  /** Whether a turn the World told of is under way. */
  turning(): boolean {
    return this.running.size > 0;
  }

  /** The turn shown under way, the latest to begin; null when none is. */
  runningTurn(): string | null {
    let latest: string | null = null;
    for (const turn of this.running) latest = turn;
    return latest;
  }

  /** When the turn under way began, as the World told it; null when no
   *  turn is known to run. */
  turnBegan(): number | null {
    const times = [...this.running].map((turn) => this.began.get(turn)).filter((at): at is number => at !== undefined);
    return times.length ? Math.min(...times) : null;
  }

  /** Whether the agent has shown anything of its own yet: before then, a
   *  conversation whose user has spoken is still starting. */
  spoke(): boolean {
    return this.derived().heardFrom;
  }

  /** How many turns have ended while watched. */
  ended(): number {
    return this.endings;
  }

  /** How the last turn that ended did: completed, stopped or failed. */
  lastEnd(): TurnEnd | null {
    return this.lastEnding;
  }

  /** Whether anything shown is happening now. */
  moving(): boolean {
    return this.derived().moving;
  }

  /** The start of the agent's host for the first words kept that wait on
   *  one: in line for room, being started, or tried again. Null when no
   *  words wait on a start. */
  starting(): StartShown | null {
    return this.derived().start;
  }

  private derived(): Derived {
    if (this.derivedCache?.clock === this.clock) return this.derivedCache;
    const lastMessage = new Map<string, string>();
    const spoke = new Set<string>();
    let latest: string | null = null;
    let moving = false;
    let waiting = false;
    let heardFrom = false;
    let start: StartShown | null = null;
    for (const entry of this.entries) {
      switch (entry.kind) {
        case "prose":
          if (entry.prose === "message") lastMessage.set(entry.turn, entry.key);
          if (entry.writing) moving = true;
          heardFrom = true;
          break;
        case "tool":
          if (entry.ended === null) moving = true;
          if (sayOf(entry)) spoke.add(entry.turn);
          heardFrom = true;
          break;
        case "heard": {
          // Words on their way, or in line for room, keep the turn's line:
          // what they wait on is said there.
          const sending = entry.sending;
          if (sending?.state === "going" || sending?.state === "queued") moving = true;
          if (start === null && sending?.state === "queued")
            start = {
              since: null,
              failures: 0,
              reason: null,
              ahead: sending.ahead,
              ...(sending.memory ? { memory: true } : {}),
              ...(sending.callee ? { callee: true } : {}),
            };
          else if (start === null && sending?.state === "going" && (sending.starting || sending.retry)) {
            const told = sending.starting;
            const since = told ? Date.parse(told.since) : NaN;
            start = {
              since: Number.isFinite(since) ? since : null,
              // A machine from before `starting` tells only that it failed,
              // not how often.
              failures: told ? (told.failures ?? 0) : 0,
              reason: told ? (told.reason ?? null) : (sending.retry ?? null),
            };
          }
          break;
        }
        case "ask":
          if (entry.state.state === "waiting") waiting = true;
          heardFrom = true;
          break;
        case "ended":
          latest = entry.turn;
          heardFrom = true;
          break;
        case "steps":
        case "compaction":
        case "other":
        case "activity":
        case "question":
        case "uncertain":
        case "agent-told":
          heardFrom = true;
          break;
        case "health":
          // Trouble that stopped it from starting ends the start.
          if (entry.health.kind === "error" || entry.health.kind === "tool_server") heardFrom = true;
          break;
        case "forked":
          break;
      }
    }
    this.derivedCache = { clock: this.clock, lastMessage, latest, moving, waiting, heardFrom, spoke, start };
    return this.derivedCache;
  }

  /** Whether the agent said something to the user in `turn` through
   *  `message_user`. */
  spokeIn(turn: string): boolean {
    return this.derived().spoke.has(turn);
  }

  /** The entries as they are laid out: what a turn did between the things
   *  it said, folded into activity stretches. Thinking belongs to the
   *  stretch it sits in; each standalone entry divides stretches so later
   *  tools cannot appear before an intervening notice or compaction.
   *  A stretch keeps its key while it lasts, whatever comes and goes in it;
   *  thinking shows the moment it begins, whether or not its words are
   *  given, so a stretch and its heading are there from its first member.
   *  A message not yet begun is nothing to show. Words sent into a turn
   *  under way and not yet taken in come last, in the order sent: they
   *  stand below the turn's line and divide nothing. */
  private pagedGroups = false;
  private previousGroups = new Map<string, string>();
  blocks(): Block[] {
    if (this.blocksCache?.clock === this.clock) return this.blocksCache.blocks;
    const blocks: Block[] = [];
    const below: Entry[] = [];
    let group: Activity | null = null;
    const ordinal = new Map<string, number>();
    const keyFor = (turn: string, first: string): string => {
      const known = this.stretches.get(turn) ?? [];
      const at = ordinal.get(turn) ?? 0;
      ordinal.set(turn, at + 1);
      if (known[at] === undefined) known[at] = first;
      this.stretches.set(turn, known);
      return `group-${turn}-${known[at]}`;
    };
    for (const entry of this.entries) {
      if (this.inherited?.has(entry.key)) continue;
      if (entry.kind === "prose" && entry.prose === "message" && entry.writing && !entry.text.trim()) continue;
      // A job's end is the World's word to the model alone; the jobs still
      // running are in the composer's panel (`world-gui.md`, 对话).
      if (entry.kind === "heard" && entry.notice?.kind === "job_ended" && entry.from?.kind !== "user") continue;
      if (entry.kind === "heard" && entry.sending !== undefined && entry.intoTurn) {
        below.push(entry);
        continue;
      }
      // Words the agent said to the user stand on their own, as its answer,
      // and divide its work as any words said do (`world-voice.md`); what it
      // asked without stopping is said the same way.
      const said = entry.kind === "tool" ? sayOf(entry) : entry.kind === "question" ? askedSay(entry) : null;
      if (said) {
        group = null;
        blocks.push(said);
        continue;
      }
      const activity = entry.kind === "tool" || entry.kind === "activity" || (entry.kind === "prose" && entry.prose === "thinking");
      if (activity && "turn" in entry) {
        if (!group || group.turn !== entry.turn) {
          group = { kind: "activity-group", key: keyFor(entry.turn, entry.key), turn: entry.turn, members: [], stamp: 0, agents: false };
          blocks.push(group);
        }
        group.members.push(entry);
        if (entry.stamp > group.stamp) group.stamp = entry.stamp;
        if (entry.kind === "tool" && (entry.tool.does.as === "agent" || startsAgent(entry.tool))) group.agents = true;
      } else {
        group = null;
        blocks.push(entry);
      }
    }
    blocks.push(...below);
    if (this.pagedGroups) {
      const reused = new Set<string>();
      const next = new Map<string, string>();
      for (const block of blocks) {
        if (block.kind !== "activity-group") continue;
        const known = block.members.map((member) => this.previousGroups.get(member.key)).find((key) => key !== undefined && !reused.has(key));
        if (known) block.key = known;
        reused.add(block.key);
        for (const member of block.members) next.set(member.key, block.key);
      }
      this.previousGroups = next;
    }
    this.blocksCache = { clock: this.clock, blocks };
    return blocks;
  }
}

/** The last of `list` that `test` takes. */
function lastOf<T>(list: readonly T[], test: (item: T) => boolean): T | undefined {
  for (let index = list.length - 1; index >= 0; index -= 1) if (test(list[index]!)) return list[index];
  return undefined;
}

/** An entry's key by its turn and the host's item. */
function itemEntryKey(turn: string, item: string): string {
  return `i:${turn}:${item}`;
}

function itemKey(turn: string, item: string): string {
  return `${turn}\u0001${item}`;
}

/** What a host's state change says of the session's health: the same
 *  mapping as `SessionChange::health()`. */
export function changeHealth(change: SessionChange): Health | null {
  switch (change.kind) {
    case "rerouted":
      return {
        kind: "rerouted",
        text: `OpenAI moved this turn from ${change.from} to ${change.to} (${change.reason})`,
      };
    case "safety": {
      if (property(change.buffering, "showBufferingUi") !== true) return null;
      // The use and the reason OpenAI gives say what the person who asked may want to change.
      const listed = (key: string) => {
        const value = property(change.buffering, key);
        const words = Array.isArray(value) ? value.filter((word): word is string => typeof word === "string") : [];
        return words.length === 0 ? "unspecified" : words.join(", ");
      };
      return {
        kind: "safety",
        text: `OpenAI is holding this turn's output for extra safety processing (use: ${listed("useCases")}; reason: ${listed("reasons")})`,
      };
    }
    case "error": {
      const message = property(change.error, "message");
      if (typeof message !== "string") return null;
      return { kind: change.will_retry ? "reconnecting" : "error", text: message };
    }
    case "tool_server":
      return change.status === "failed"
        ? { kind: "tool_server", text: `its ${change.server} tools did not start: ${typeof change.error === "string" ? change.error : "no reason given"}` }
        : null;
    case "warning":
      return { kind: "warning", text: change.text };
    default:
      return null;
  }
}

function property(value: JsonValue, key: string): JsonValue | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value[key] : undefined;
}

/** The call an agent Codex tells of by an item started with: it names the
 *  agent, and the task its path names. */
function startedTool(agent: string, path: string): Tool {
  return {
    name: "spawn_agent",
    server: null,
    machine: null,
    input: null,
    output: [],
    status: "done",
    duration_ms: null,
    does: { as: "agent", act: "start", agents: [agent], kind: null, task: taskOf(path) },
  };
}

/** The task an agent's path names, `/root/<spawner>/<task>`: its last part. */
function taskOf(path: string): string {
  return path.split("/").filter(Boolean).at(-1) ?? path;
}

/** Whether a call starts one of the host's own agents: Claude Code's
 *  Agent tool, Codex's spawn, as an older node told them too. */
export function startsAgent(tool: Tool): boolean {
  if (tool.does.as === "agent") return tool.does.act === "start";
  return tool.does.as === "colleague" && (tool.name === "Agent" || tool.name === "Task" || tool.name === "spawnAgent");
}

/** Whether an item is words the agent writes: a message, its thinking, its
 *  plan. */
function writes(item: import("../world/events").Item): boolean {
  return item.kind === "message" || item.kind === "thinking" || item.kind === "plan";
}

/** Whether an entry is the agent's thinking. */
export function isThinking(entry: Entry): entry is Extract<Entry, { kind: "prose" }> {
  return entry.kind === "prose" && entry.prose === "thinking";
}

/** A stretch's members as they are shown: thinking with nothing between is
 *  one run of thinking, as Claude's response that thinks in two blocks, one
 *  without words and one with, is thought once. */
export function runsOf(members: readonly Entry[]): Entry[][] {
  const runs: Entry[][] = [];
  for (const member of members) {
    const last = runs[runs.length - 1];
    if (last && isThinking(member) && isThinking(last[0]!)) last.push(member);
    else runs.push([member]);
  }
  return runs;
}

/** A question of an agent's message asked without stopping: its words and
 *  the answers it offers. */
interface Asked {
  title: string;
  options: string[];
}

/** The questions of an agent's message asked without stopping, where it is
 *  one: Codex marks it with `questions` beside its words. */
function questionsOf(detail: JsonValue | undefined): Asked[] | null {
  const raw = property(detail ?? null, "questions");
  if (!Array.isArray(raw) || !raw.length) return null;
  const questions: Asked[] = [];
  for (const one of raw) {
    const title = property(one, "title");
    if (typeof title !== "string" || !title.trim()) return null;
    const options = property(one, "options");
    questions.push({ title, options: Array.isArray(options) ? options.filter((o): o is string => typeof o === "string") : [] });
  }
  return questions;
}

/** What was asked without stopping, written as words to the user: the
 *  message's own words, then each question with the answers it offers. */
function askedText(words: string, questions: Asked[]): string {
  const asked = questions.map((question) => [`**${question.title.trim()}**`, ...question.options.map((option) => `- ${option}`)].join("\n"));
  return [words.trim(), ...asked].filter(Boolean).join("\n\n");
}

/** What was asked without stopping, as words said to the user. */
function askedSay(entry: Extract<Entry, { kind: "question" }>): Say {
  return { kind: "say", key: entry.key, item: entry.item, turn: entry.turn, stamp: entry.stamp, at: entry.at, time: null, text: entry.text, status: "info", attachments: [] };
}

/** How many of their first characters two messages are told apart by: a
 *  long one is shown cut, and marked so, past many more than these. */
const WORDS_COMPARED = 200;
/** The fewest first characters two messages that are not the same whole
 *  must share to be taken for one: one telling how the work goes shares
 *  none so many with the answer. */
const WORDS_AT_LEAST = 40;

/** Whether a message saying `message` says the words a turn's end told it
 *  ended with: the same, or the same so far — a message still coming in,
 *  or either of them cut short and marked so. One saying nothing yet, or
 *  too little to tell, is not. */
function sameWords(message: string, told: string): boolean {
  const said = message.trim();
  const words = told.trim();
  if (said === "" || words === "") return false;
  if (said === words) return true;
  const compared = Math.min(said.length, words.length, WORDS_COMPARED);
  return compared >= WORDS_AT_LEAST && said.slice(0, compared) === words.slice(0, compared);
}

/** The item a turn's last words shown from its end stand as (`Fold.lastWords`). */
const LAST_WORDS = "last-words";

/** Where a turn's last words shown from its end are kept. */
function lastWordsKey(turn: string): string {
  return `w:${turn}`;
}
