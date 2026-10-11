// A conversation as it reads: the session's events folded into the things
// shown, in the order they first appeared. The same reading as the
// terminal's (`crates/fastctx-tui/src/transcript.rs`), so every interface
// shows a session alike.

import type { Health, MachineName, Notice, SessionChange, SessionEvent, Speaker, Step, Tool, TurnEnd } from "../world/events";
import type { Attachment } from "../world/attachments";
import type { JsonValue } from "../world/wire";
import { askKey, forPeople, type AskState } from "./ask-state";
import { userWords } from "./told";
import { readReply, type AsyncQuestion } from "./async-ask";

export type Use = "done" | "failed" | "declined" | "cut" | "unknown";

export type SendState =
  | { state: "going" }
  /** In the World, not yet in a turn: `how` is the way they landed
   *  (`waiting`, `sent`, `uncertain`), as the node said. */
  | { state: "landed"; how: string; machine: MachineName; why?: string | null }
  | { state: "failed"; why: string };

/** Something the user attached to their words, as it is shown with them. */
export interface ShownAttachment {
  name: string;
  size: number | null;
  image: boolean;
  preview: string | null;
}

export type Entry =
  | {
      kind: "heard";
      key: string;
      from: Speaker | null;
      text: string;
      turn: string | null;
      sending?: SendState;
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
  | { kind: "question"; key: string; item: string; turn: string; questions: AsyncQuestion[]; answers: (string | null)[]; version: number };

/** One stretch of what a turn did between things it said: tools and
 *  thinking in a row, folded under one heading. */
export interface Activity {
  kind: "activity-group";
  key: string;
  turn: string;
  members: Entry[];
}

export type Block = Entry | Activity;

type Keyed = Extract<Entry, { item: string; turn: string }>;

export class Conversation {
  entries: Entry[] = [];
  context: { used: number; window: number | null } | null = null;
  title: string | null = null;
  /** The model the host last answered with, where it says (Claude's
   *  answers do): what the conversation shows, not the one chosen. */
  answeredBy: string | null = null;
  /** Claude Code's own subagents: who, how many steps each has taken, the
   *  one it takes now; `agentStamp` moves whenever one of them does. */
  readonly agents = new Map<string, { kind: string; description: string; status: string; steps: number; doing: string | null }>();
  agentStamp = 0;
  /** Attachments by where the World keeps them, with the names they came
   *  with: the World stores each under a name of its own. */
  private attachments = new Map<string, string>();

  /** The name an attachment came with, by where it is kept. */
  attachmentName(path: string): string | null {
    return this.attachments.get(path.replaceAll("\\", "/")) ?? null;
  }
  /** The host replaced or removed turns: what is shown must be read again. */
  stale = false;
  /** Each turn's stretches of activity, in order, by the key of their first
   *  entry: a stretch keeps its place and key when its first entry goes. */
  private stretches = new Map<string, string[]>();
  private serial = 0;
  /** Turns begun and not yet ended, as the World tells them, with when
  * each began. */
  private running = new Set<string>();
  private began = new Map<string, number>();
  /** How many turns have ended while watched (history read again is not
   *  a turn ending), and how the last one did. */
  private endings = 0;
  private lastEnding: TurnEnd | null = null;
  private replaying = false;

  private key(prefix: string): string {
    this.serial += 1;
    return `${prefix}-${this.serial}`;
  }

  /** What came before, replacing what is shown. */
  history(events: SessionEvent[]): void {
    // Requests still waiting are kept: the host asked them, and they are
    // not part of the history it gives back.
    const waiting = this.entries.filter((entry) => entry.kind === "ask" && entry.state.state === "waiting");
    this.entries = [];
    this.stretches.clear();
    this.stale = false;
    this.running.clear();
    this.began.clear();
    this.replaying = true;
    for (const event of events) this.event(event);
    this.replaying = false;
    for (const ask of waiting) {
      if (ask.kind === "ask" && !this.entries.some((entry) => entry.kind === "ask" && askKey(entry.id) === askKey(ask.id))) this.entries.push(ask);
    }
  }

  event(event: SessionEvent): void {
    const at = Date.parse(event.at);
    switch (event.event) {
      case "turn_started":
        this.running.add(event.turn);
        if (!this.began.has(event.turn)) this.began.set(event.turn, at);
        return;
      case "realtime":
      case "realtime_grew":
        return;
      case "heard": {
        // Without the World's record of who spoke, the user's words still
        // carry the head note the World wrapped them in.
        const user = event.from ? null : userWords(event.text);
        const heard = user ? { ...event, from: { kind: "user" as const, machine: user.machine }, text: user.body } : event;
        // Words sent from here are now heard: they keep their place.
        if (heard.from?.kind === "user") {
          const sent = this.entries.find(
            (entry) => entry.kind === "heard" && entry.sending !== undefined && entry.text === heard.text,
          );
          if (sent && sent.kind === "heard") {
            delete sent.sending;
            sent.turn = heard.turn;
            sent.from = heard.from;
            // Heard now: it stands where the session took it in, after what
            // the turn did before it read them.
            this.entries.splice(this.entries.indexOf(sent), 1);
            this.entries.push(sent);
            return;
          }
        }
        const known = this.entries.some(
          (entry) => entry.kind === "heard" && entry.text === heard.text && heard.turn !== null && entry.turn === heard.turn,
        );
        const reply = heard.from?.kind === "user" ? readReply(heard.text) : null;
        if (reply) this.answered(reply.item, reply.answers);
        if (!known) {
          for (const file of heard.message?.attachments ?? []) this.attachments.set(file.path.replaceAll("\\", "/"), file.name);
          // The files that came with the words, as the World kept them.
          const attached = (heard.message?.attachments ?? []).map((file) => ({
            name: file.name,
            size: file.size,
            image: file.media_type.startsWith("image/"),
            preview: null,
          }));
          this.entries.push({
            kind: "heard",
            key: this.key("heard"),
            from: heard.from,
            text: heard.text,
            turn: heard.turn,
            ...(attached.length ? { attached, kept: heard.message?.attachments ?? [] } : {}),
            ...(event.event === "heard" && event.notice ? { notice: event.notice } : {}),
            ...(reply ? { reply: { item: reply.item, answers: reply.answers.map((one) => one.answer) } } : {}),
          });
        }
        return;
      }
      case "began":
      case "updated":
        this.item(event.turn, event.item, at, false);
        return;
      case "finished":
        this.item(event.turn, event.item, at, true);
        return;
      case "grew": {
        const shown = this.find(event.turn, event.item);
        if (shown?.kind === "prose" && event.part === "text") {
          shown.text += event.text;
          shown.version += 1;
        } else if (!shown && event.part === "text") {
          // A message's first words can come before any word of it.
          this.entries.push({
            kind: "prose",
            key: this.key("prose"),
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
        const shown = [...this.entries].reverse().find((entry) => entry.kind === "steps" && entry.turn === event.turn);
        if (shown?.kind === "steps") {
          shown.steps = event.steps;
          shown.note = event.note;
        } else {
          this.entries.push({ kind: "steps", key: this.key("steps"), turn: event.turn, steps: event.steps, note: event.note });
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
        if (this.entries.some((entry) => entry.kind === "ended" && entry.turn === event.turn)) return;
        if (!this.replaying) this.endings += 1;
        this.lastEnding = event.end;
        for (const entry of this.entries) {
          if (entry.kind === "tool" && entry.turn === event.turn && entry.ended === null) entry.ended = { at, use: "cut" };
          if (entry.kind === "prose" && entry.turn === event.turn) entry.writing = false;
        }
        this.entries.push({ kind: "ended", key: this.key("ended"), turn: event.turn, end: event.end });
        return;
      }
      case "delivery_uncertain":
        this.entries.push({ kind: "uncertain", key: this.key("uncertain"), why: event.why });
        return;
      case "health":
        this.health(event.health);
        return;
      case "changed": {
        const change = event.change;
        if (change.kind === "request") {
          if (!forPeople(change.request) || this.ask(change.id)) return;
          this.entries.push({
            kind: "ask",
            key: this.key("ask"),
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
            this.entries.unshift({ kind: "forked", key: this.key("forked"), source: change.source });
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
        if (change.kind === "message_usage" && change.model) {
          this.answeredBy = change.model;
          return;
        }
        if (change.kind === "agents") {
          for (const agent of change.agents) {
            const known = this.agents.get(agent.id);
            this.agents.set(agent.id, { kind: agent.type, description: agent.description, status: agent.status, steps: known?.steps ?? 0, doing: known?.doing ?? null });
          }
          this.agentStamp += 1;
          return;
        }
        if (change.kind === "agent_event") {
          const known = this.agents.get(change.agent_id) ?? { kind: "", description: "", status: "running", steps: 0, doing: null };
          const happened = change.happened !== null && typeof change.happened === "object" && !Array.isArray(change.happened) ? change.happened : {};
          const item = happened.item !== null && typeof happened.item === "object" && !Array.isArray(happened.item) ? happened.item : {};
          const named = item.kind === "tool" && typeof item.name === "string" ? item.name : null;
          if (happened.event === "began" && named) known.doing = named;
          if (happened.event === "finished" && item.kind === "tool") known.steps += 1;
          if (happened.event === "turn_ended") known.doing = null;
          this.agents.set(change.agent_id, known);
          this.agentStamp += 1;
          return;
        }
        if (change.kind === "commands_changed") {
          this.offersChanged = true;
          return;
        }
        const health = changeHealth(change);
        if (health) this.health(health);
        return;
      }
    }
  }

  private ask(id: JsonValue): Extract<Entry, { kind: "ask" }> | undefined {
    const key = askKey(id);
    return this.entries.find((entry): entry is Extract<Entry, { kind: "ask" }> => entry.kind === "ask" && askKey(entry.id) === key);
  }

  /** How a request now stands: being answered, answered, or no longer
   *  waited on. */
  settle(ask: Extract<Entry, { kind: "ask" }>, state: AskState): void {
    ask.state = state;
    ask.version += 1;
  }

  /** Whether the session waits on the user for anything. */
  waiting(): boolean {
    return this.entries.some((entry) => entry.kind === "ask" && entry.state.state === "waiting");
  }

  /** Something of the session's own health, shown once however often the
   *  host repeats it in a row. */
  private health(health: Health): void {
    const last = this.entries[this.entries.length - 1];
    if (last?.kind === "health" && last.health.kind === health.kind && last.health.text === health.text) return;
    this.entries.push({ kind: "health", key: this.key("health"), health });
  }

  /** An item began or finished: shown where it first appeared, as it now
   *  stands. */
  private item(turn: string, native: import("../world/events").Item, at: number, finished: boolean): void {
    const startedAt = native.started_at ? Date.parse(native.started_at) : at;
    const id = native.id;
    let fresh: Entry;
    const questions = native.kind === "message" ? questionsOf(native.detail) : null;
    if (questions) {
      const shown = this.find(turn, id);
      if (shown?.kind === "question") return;
      const fresh: Entry = { kind: "question", key: this.key("question"), item: id, turn, questions, answers: questions.map(() => null), version: 0 };
      if (shown) this.entries[this.entries.indexOf(shown)] = { ...fresh, key: shown.key };
      else this.entries.push(fresh);
      return;
    }
    switch (native.kind) {
      case "message":
      case "thinking":
      case "plan":
        fresh = {
          kind: "prose",
          key: this.key("prose"),
          item: id,
          turn,
          prose: native.kind,
          text: native.text,
          writing: !finished,
          version: 0,
        };
        break;
      case "agent_activity":
        fresh = { kind: "activity", key: this.key("activity"), item: id, turn, text: `${native.path}: ${native.activity}` };
        break;
      case "tool": {
        const { kind: _kind, id: _id, started_at: _started, detail: _detail, ...tool } = native;
        const use: Use | null =
          tool.status === "running" ? null : tool.status === "unknown" ? "unknown" : (tool.status as Use);
        fresh = {
          kind: "tool",
          key: this.key("tool"),
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
        fresh = { kind: "compaction", key: this.key("compaction"), item: id, turn };
        break;
      case "hidden":
        // A completed item can remove a transient placeholder.
        if (id) this.entries = this.entries.filter((entry) => !("item" in entry && entry.item === id && entry.turn === turn));
        return;
      case "other":
        if (!finished) return;
        fresh = { kind: "other", key: this.key("other"), item: id, turn, hostKind: native.host_kind };
        break;
    }
    if (!id) {
      this.entries.push(fresh);
      return;
    }
    const shown = this.find(turn, id);
    if (!shown) {
      this.entries.push(fresh);
      return;
    }
    if (shown.kind === "prose" && fresh.kind === "prose") {
      // A message still being written keeps what it has until its whole
      // text comes.
      if (shown.writing && !finished && fresh.text === "") return;
      if (shown.text !== fresh.text) shown.version += 1;
      shown.text = fresh.text;
      shown.writing = fresh.writing;
      return;
    }
    if (shown.kind === "tool" && fresh.kind === "tool") {
      shown.tool = fresh.tool;
      shown.ended = fresh.ended;
      return;
    }
    const index = this.entries.indexOf(shown);
    this.entries[index] = { ...fresh, key: shown.key };
  }

  /** Answers the user sent to the questions of `item`, by question. */
  private answered(item: string, answers: { index: number; answer: string }[]): void {
    const question = this.entries.find((entry): entry is Extract<Entry, { kind: "question" }> => entry.kind === "question" && entry.item === item);
    if (!question) return;
    for (const { index, answer } of answers) if (index >= 0 && index < question.answers.length) question.answers[index] = answer;
    question.version += 1;
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
    return this.entries.some((entry) => entry.kind === "ended" && entry.turn === turn);
  }

  private find(turn: string, id: string): Keyed | undefined {
    return this.entries.find((entry): entry is Keyed => "item" in entry && entry.item === id && entry.turn === turn);
  }

  /** Words the user is sending, shown at once; returns their key. */
  sending(text: string, here: MachineName | null, attached: ShownAttachment[] = []): string {
    const key = this.key("heard");
    this.entries.push({
      kind: "heard",
      key,
      from: here ? { kind: "user", machine: here } : null,
      text,
      turn: null,
      sending: { state: "going" },
      ...(attached.length ? { attached } : {}),
      ...replyOf(text),
    });
    return key;
  }

  private offersChanged = false;

  /** Whether what the conversation offers changed since last asked. */
  takeOffersChanged(): boolean {
    const changed = this.offersChanged;
    this.offersChanged = false;
    return changed;
  }

  /** The subagent a tool started, where it is Claude Code's own agent tool:
   *  known by the description it was given. */
  agentOf(entry: Extract<Entry, { kind: "tool" }>): { kind: string; status: string; steps: number; doing: string | null } | null {
    if (entry.tool.name !== "Agent" && entry.tool.name !== "Task") return null;
    const input = entry.tool.input;
    const description = input !== null && typeof input === "object" && !Array.isArray(input) && typeof input.description === "string" ? input.description : null;
    if (!description) return null;
    return [...this.agents.values()].find((agent) => agent.description === description) ?? null;
  }

  /** Words shown as being sent that went into the queue instead. */
  unsend(key: string): void {
    this.entries = this.entries.filter((entry) => entry.key !== key);
  }

  /** What became of words being sent: they are kept, with how they stand,
   *  until the session hears them. */
  sent(key: string, state: SendState): void {
    const entry = this.entries.find((candidate) => candidate.key === key);
    if (entry?.kind === "heard" && entry.sending) entry.sending = state;
  }

  /** Whether a turn is under way: begun and not ended, or anything shown
   *  still happening. */
  busy(): boolean {
    return this.running.size > 0 || this.moving();
  }

  /** When the turn under way began, as the World told it; null when no
   *  turn is known to run. */
  turnBegan(): number | null {
    const times = [...this.running].map((turn) => this.began.get(turn)).filter((at): at is number => at !== undefined);
    return times.length ? Math.min(...times) : null;
  }

  /** What the turn under way is doing now, for the line under it: the last
   *  thing still happening. */
  doing(): Entry | null {
    for (let index = this.entries.length - 1; index >= 0; index -= 1) {
      const entry = this.entries[index]!;
      if ((entry.kind === "tool" && entry.ended === null) || (entry.kind === "prose" && entry.writing)) return entry;
    }
    return null;
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
    return this.entries.some(
      (entry) =>
        (entry.kind === "tool" && entry.ended === null) ||
        (entry.kind === "prose" && entry.writing) ||
        (entry.kind === "heard" && entry.sending?.state === "going"),
    );
  }

  /** The entries as they are laid out: what a turn did between the things
   *  it said, folded into activity stretches. Thinking belongs to the
   *  stretch it sits in; words in between are prose and divide stretches.
   *  A stretch keeps its key while it lasts, whatever comes and goes in it;
   *  it is laid out only while something in it can be seen, so its heading
   *  never comes and goes with a placeholder. A message not yet begun, and
   *  thinking not yet given, are nothing to show. */
  blocks(): Block[] {
    const blocks: Block[] = [];
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
      if (entry.kind === "prose" && entry.prose === "message" && entry.writing && !entry.text.trim()) continue;
      const activity = entry.kind === "tool" || entry.kind === "activity" || (entry.kind === "prose" && entry.prose === "thinking");
      if (activity && "turn" in entry) {
        if (!group || group.turn !== entry.turn) {
          group = { kind: "activity-group", key: keyFor(entry.turn, entry.key), turn: entry.turn, members: [] };
          blocks.push(group);
        }
        group.members.push(entry);
      } else {
        group = null;
        blocks.push(entry);
      }
    }
    return blocks.filter((block) => block.kind !== "activity-group" || block.members.some(visible));
  }
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

/** Whether a member of a stretch has anything to show: thinking only once
 *  it has words or is over. */
export function visible(entry: Entry): boolean {
  return entry.kind !== "prose" || entry.prose !== "thinking" || !!entry.text.trim() || !entry.writing;
}

/** The questions of an agent's message asked without waiting, where it is
 *  one: Codex marks it with `questions` beside its words. */
function questionsOf(detail: JsonValue | undefined): AsyncQuestion[] | null {
  const raw = property(detail ?? null, "questions");
  if (!Array.isArray(raw) || !raw.length) return null;
  const questions: AsyncQuestion[] = [];
  for (const one of raw) {
    const title = property(one, "title");
    if (typeof title !== "string" || !title.trim()) return null;
    const options = property(one, "options");
    questions.push({ title, options: Array.isArray(options) ? options.filter((o): o is string => typeof o === "string") : null });
  }
  return questions;
}
