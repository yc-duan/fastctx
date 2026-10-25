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
import { readReply, type AsyncQuestion } from "./async-ask";
import { Agents, type AgentRun } from "./agents";
import { Requests } from "./requests";
import type { PendingControl } from "../world/control";

export type Use = "done" | "failed" | "declined" | "cut" | "unknown";

export type SendState =
  /** On their way. `retry` is set once their host has failed to start for
   *  a minute and is being started again: why it did not start. */
  | { state: "going"; retry?: string }
  /** No answer came yet; they are said again, under the same id, until one
   *  does: why the last try got none. */
  | { state: "waiting"; why: string }
  /** In the World, not yet in a turn: `how` is the way they landed
   *  (`waiting`, `sent`, `uncertain`), as the node said. */
  | { state: "landed"; how: string; machine: MachineName; why?: string | null }
  /** Its machine keeps them no more and no turn runs: they went in, and
   *  stand as said until the session's own telling of them comes. */
  | { state: "settled" }
  | { state: "failed"; why: string };

/** Words a conversation's machine keeps that no turn took yet, as it tells
 *  them (`Watched::Now`). */
export interface KeptWords {
  id: string;
  state: "waiting" | "handing" | "unknown";
  /** Present once their host has failed to start for a minute; they are
   *  still on their way (`world-lifecycle.md`, 用户看到的只是在发送). */
  host_retry?: { since: string; reason: string };
}

/** Something the user attached to their words, as it is shown with them. */
export interface ShownAttachment {
  name: string;
  size: number | null;
  image: boolean;
  preview: string | null;
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
      sending?: SendState;
      /** Words sent from here, once the World answered for them: their id
       *  on the machine they went to, `null` where it gave none. */
      delivery?: string | null;
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
      /** The user's answers to a question the agent asked without waiting,
       *  sent as the official client writes them: shown as answers, never
       *  as the envelope. */
      reply?: { item: string; answers: string[] };
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
  /** This conversation was forked from another one. */
  | { kind: "forked"; key: string; source: import("../world/wire").Address }
  /** A request the session waits on the user for, as its host made it. */
  | { kind: "ask"; key: string; id: JsonValue; turn: string | null; request: string; params: JsonValue; state: AskState; version: number }
  /** Questions the agent asked without stopping its work (Codex's
   *  `request_user_input_async`): answered whenever the user likes, or not
   *  at all. `answers` holds what was answered, by question. */
  | { kind: "question"; key: string; item: string; turn: string; questions: AsyncQuestion[]; answers: (string | null)[]; version: number }
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
  text: string;
  status: "reply" | "info" | "urgent";
  attachments: { machine: string; path: string }[];
}

export type Block = Entry | Activity | Gap | Say;

/** Whether `server` names the World's own tools, as either host lists them:
 *  Codex as they are served, Claude Code under its plugin. */
function worldServer(server: string | null): boolean {
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
  return { kind: "say", key: entry.key, item: entry.item, turn: entry.turn, stamp: entry.stamp, at: entry.at, text: typeof input.message === "string" ? input.message : "", status, attachments };
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
  context: { used: number; window: number | null } | null = null;
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
  private derivedCache: Derived | null = null;
  private blocksCache: { clock: number; blocks: Block[] } | null = null;

  /** Marks an entry changed. */
  private touch(entry: Entry): void {
    entry.stamp = ++this.clock;
  }

  private add(entry: Entry): void {
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
  history(events: SessionEvent[], carry: SessionEvent[] = []): void {
    // Requests still waiting are kept: the host asked them, and they are
    // not part of the history it gives back. So are words sent from here
    // that this reading does not hold yet.
    const waiting = this.entries.filter((entry) => entry.kind === "ask" && entry.state.state === "waiting");
    const pending = this.entries.filter((entry): entry is Heard => entry.kind === "heard" && entry.sending !== undefined);
    const known = new Set(this.entries.filter((entry) => entry.kind === "heard").map((entry) => entry.key));
    const halted = this.halted;
    this.entries = [];
    this.byKey.clear();
    this.byItem.clear();
    this.byTurn.clear();
    this.heardIn.clear();
    this.endedTurns.clear();
    this.over.clear();
    this.stretches.clear();
    this.sequences.clear();
    this.stale = false;
    this.halted = null;
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
    this.extend(events);
    // A history the node gives whole is as it stands after every change it
    // holds: a change of history among its events is shown already, and is
    // no reason to read it again. One that comes after it is.
    this.stale = false;
    // A stop asked of what still runs, or of words still on their way,
    // stands through a reading again.
    if (halted && (this.running.size > 0 || halted.size === 0)) this.halt();
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
  extend(events: SessionEvent[]): void {
    this.replaying = true;
    try {
      for (const event of events) this.event(event);
    } finally {
      this.replaying = false;
    }
  }

  /** One more event of this fold's part. */
  event(event: SessionEvent): void {
    this.current = this.folded;
    this.folded += 1;
    this.read(event);
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
        // Words taken into a turn say the turn is under way, whichever of
        // the two the World tells first.
        if (heard.turn) this.underWay(heard.turn, at);
        // Words sent from here are now heard: they keep their place. The
        // bubble shows them without the blank lines around them, while a
        // shared input sends them as typed (a phone's Enter adds a line).
        // Words once said not to be known to have gone in did: the host
        // tells them now (`world-concurrency.md`, 一句话从发出到落定).
        if (heard.delivery) this.remove((entry) => entry.kind === "uncertain" && entry.key === `u:${heard.delivery}`);
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
          heard.turn !== null &&
          (this.byTurn.get(heard.turn) ?? []).some((entry) => entry.kind === "heard" && entry.text.trim() === heard.text.trim());
        const reply = heard.from?.kind === "user" ? readReply(heard.text) : null;
        if (reply) this.answered(reply.item, reply.answers);
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
              : turnKey
                ? `h:${turnKey}:${ordinal}`
                : `h:${this.scope}@${this.current}`;
          const shown = this.byKey.has(key) ? `${key}:${this.scope}@${this.current}` : key;
          this.add({
            kind: "heard",
            key: shown,
            stamp: 0,
            at: 0,
            from: heard.from,
            text: heard.text,
            turn: heard.turn,
            ...(attached.length ? { attached, kept: heard.message?.attachments ?? [] } : {}),
            ...(event.event === "heard" && event.notice ? { notice: event.notice } : {}),
            ...(reply ? { reply: { item: reply.item, answers: reply.answers.map((one) => one.answer) } } : {}),
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
        } else if (!shown && event.part === "text") {
          // A message's first words can come before any word of it.
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
        this.context = { used: event.used, window: event.window };
        return;
      case "titled":
        this.title = event.title;
        return;
      case "turn_ended": {
        this.running.delete(event.turn);
        this.began.delete(event.turn);
        if (this.halted?.delete(event.turn) && this.halted.size === 0) this.halted = null;
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
        if (change.kind === "request") {
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
            const forked: Entry = { kind: "forked", key: "f", stamp: ++this.clock, at: this.current, source: change.source };
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
   *  asked to stop; any other is new, and shown as it goes. */
  private started(turn: string): void {
    if (!this.halted) return;
    if (this.halted.size === 0) this.halted.add(turn);
    else if (!this.halted.has(turn)) this.halted = null;
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
    this.add({ kind: "health", key: `x:${this.scope}@${this.current}`, stamp: 0, at: 0, health });
  }

  /** An item began or finished: shown where it first appeared, as it now
   *  stands. */
  private item(turn: string, native: import("../world/events").Item, at: number, finished: boolean): void {
    const startedAt = native.started_at ? Date.parse(native.started_at) : at;
    const id = native.id;
    if (id && native.draft) this.renamed(turn, native.draft, id);
    const key = id ? itemEntryKey(turn, id) : `n:${this.scope}@${this.current}`;
    if (id && !finished && this.over.has(key)) return;
    if (id && finished) this.over.add(key);
    const said = native.kind === "message" || native.kind === "thinking" || native.kind === "plan" || native.kind === "tool" || native.kind === "compaction";
    if (said && !(id && this.find(turn, id))) this.overtaken(turn);
    let fresh: Entry;
    const questions = native.kind === "message" ? questionsOf(native.detail) : null;
    if (questions) {
      const shown = id ? this.find(turn, id) : undefined;
      if (shown?.kind === "question") return;
      const asked: Entry = { kind: "question", key, stamp: 0, at: 0, item: id, turn, questions, answers: questions.map(() => null), version: 0 };
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

  /** Answers the user sent to the questions of `item`, by question. */
  private answered(item: string, answers: { index: number; answer: string }[]): void {
    const question = this.entries.find((entry): entry is Extract<Entry, { kind: "question" }> => entry.kind === "question" && entry.item === item);
    if (!question) return;
    for (const { index, answer } of answers) if (index >= 0 && index < question.answers.length) question.answers[index] = answer;
    question.version += 1;
    this.touch(question);
  }

  /** Answers about to be sent: shown answered at once. */
  answering(item: string, answers: (string | null)[]): void {
    this.answered(
      item,
      answers.flatMap((answer, index) => (answer === null ? [] : [{ index, answer }])),
    );
  }

  /** Whether a turn has ended, as far as is shown. */
  turnEnded(turn: string): boolean {
    return this.endedTurns.has(turn);
  }

  /** Whether this fold holds anything of `turn`. */
  holds(turn: string): boolean {
    return this.byTurn.has(turn);
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
    // What the user says after asking to stop is shown as it goes, as is
    // the turn it starts.
    this.halted = null;
    const key = `s:${++this.sends}`;
    this.add({
      kind: "heard",
      key,
      stamp: 0,
      at: 0,
      from: here ? { kind: "user", machine: here } : null,
      text,
      turn: null,
      sending: { state: "going" },
      ...(this.running.size > 0 ? { intoTurn: true } : {}),
      ...(attached.length ? { attached } : {}),
      ...replyOf(text),
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
    this.remove((entry) => entry.key === key);
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
  }

  /** The World says no turn runs: what is still shown under way here
   *  ended unseen, and stands as cut off, whichever turn it was in. */
  noTurnRuns(): void {
    this.halted = null;
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

  /** The stop did not reach the agent: it goes on, and what was held back
   *  here is read again. */
  unhalt(): void {
    if (!this.halted) return;
    this.halted = null;
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
  now(busy: boolean, turn: string | null, kept: KeptWords[]): void {
    if (busy && turn) this.underWay(turn, Date.now());
    if (!busy) this.noTurnRuns();
    const keeping = new Map(kept.map((words) => [words.id, words]));
    for (const entry of this.entries) {
      if (entry.kind !== "heard" || !entry.sending || entry.delivery === undefined) continue;
      const settling = entry.sending.state === "going" || (entry.sending.state === "landed" && entry.sending.how === "kept");
      if (!settling) continue;
      const words = entry.delivery === null ? undefined : keeping.get(entry.delivery);
      let next: SendState | null = null;
      if (words?.state === "waiting") next = { state: "landed", how: "kept", machine: entry.from?.kind === "user" ? entry.from.machine : "" };
      else if (words === undefined && !busy) next = { state: "settled" };
      else if (words !== undefined) next = words.host_retry ? { state: "going", retry: words.host_retry.reason } : { state: "going" };
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

  private derived(): Derived {
    if (this.derivedCache?.clock === this.clock) return this.derivedCache;
    const lastMessage = new Map<string, string>();
    const spoke = new Set<string>();
    let latest: string | null = null;
    let moving = false;
    let waiting = false;
    let heardFrom = false;
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
        case "heard":
          if (entry.sending?.state === "going") moving = true;
          break;
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
    this.derivedCache = { clock: this.clock, lastMessage, latest, moving, waiting, heardFrom, spoke };
    return this.derivedCache;
  }

  /** Whether the agent said something to the user in `turn` through
   *  `message_user`. */
  spokeIn(turn: string): boolean {
    return this.derived().spoke.has(turn);
  }

  /** The entries as they are laid out: what a turn did between the things
   *  it said, folded into activity stretches. Thinking belongs to the
   *  stretch it sits in; words in between are prose and divide stretches.
   *  A stretch keeps its key while it lasts, whatever comes and goes in it;
   *  thinking shows the moment it begins, whether or not its words are
   *  given, so a stretch and its heading are there from its first member.
   *  A message not yet begun is nothing to show. Words sent into a turn
   *  under way and not yet taken in come last, in the order sent: they
   *  stand below the turn's line and divide nothing. */
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
    // Questions asked without waiting show their answers in their own card.
    const asked = new Set<string>();
    for (const entry of this.entries) if (entry.kind === "question") asked.add(entry.item);
    for (const entry of this.entries) {
      if (entry.kind === "prose" && entry.prose === "message" && entry.writing && !entry.text.trim()) continue;
      // The words that carry an answer are said by its card, not again
      // below it; they still end the stretch before them, as words do.
      if (entry.kind === "heard" && entry.reply && asked.has(entry.reply.item)) {
        group = null;
        continue;
      }
      // A job's end is the World's word to the model alone; the jobs still
      // running are in the composer's panel (`world-gui.md`, 对话).
      if (entry.kind === "heard" && entry.notice?.kind === "job_ended" && entry.from?.kind !== "user") continue;
      if (entry.kind === "heard" && entry.sending !== undefined && entry.intoTurn) {
        below.push(entry);
        continue;
      }
      // Words the agent said to the user stand on their own, as its answer,
      // and divide its work as any words said do (`world-voice.md`).
      const said = entry.kind === "tool" ? sayOf(entry) : null;
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
        // Only words said, the model's or a person's, begin a new stretch
        // (walkthrough item 33); a note, a notice or a request in between
        // stands after the stretch it came in.
        if ((entry.kind === "heard" && !entry.notice) || (entry.kind === "prose" && entry.prose === "message")) group = null;
        blocks.push(entry);
      }
    }
    blocks.push(...below);
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

/** Answers to questions asked without waiting, as the words that carry
 *  them are shown. */
function replyOf(text: string): { reply?: { item: string; answers: string[] } } {
  const reply = readReply(text);
  return reply ? { reply: { item: reply.item, answers: reply.answers.map((one) => one.answer) } } : {};
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

/** The questions of an agent's message asked without waiting, where it is
 *  one: Codex marks it with `questions` beside its words, and a goal's
 *  Claude question keeps Claude's own questions too, with what each answer
 *  means and whether several may be chosen. */
function questionsOf(detail: JsonValue | undefined): AsyncQuestion[] | null {
  const raw = property(detail ?? null, "questions");
  if (!Array.isArray(raw) || !raw.length) return null;
  const claude = property(property(detail ?? null, "fastctx_goal_question") ?? null, "questions");
  const questions: AsyncQuestion[] = [];
  for (const [index, one] of raw.entries()) {
    const title = property(one, "title");
    if (typeof title !== "string" || !title.trim()) return null;
    const options = property(one, "options");
    const question: AsyncQuestion = { title, options: Array.isArray(options) ? options.filter((o): o is string => typeof o === "string") : null };
    if (property(one, "multiSelect") === true) question.multiple = true;
    const offered = Array.isArray(claude) ? property(claude[index] ?? null, "options") : null;
    if (Array.isArray(offered)) {
      const notes = offered.map((option) => {
        const note = property(option, "description");
        return typeof note === "string" && note.trim() ? note : null;
      });
      if (notes.some((note) => note !== null)) question.notes = notes;
    }
    questions.push(question);
  }
  return questions;
}
