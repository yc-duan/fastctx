// How specimen conversations are written: a clock that moves as things
// happen, and the shapes the node gives a turn's items, runs and files.

import type { Attachment } from "../world/attachments";
import type { Content, Does, Item, Notice, SessionEvent, Speaker, Tool, TurnFacts, UserImage } from "../world/events";
import { printed, ran } from "./run-facts";

export class Script {
  events: SessionEvent[] = [];
  private clock: number;
  private serial = 0;

  constructor(start: string) {
    this.clock = Date.parse(start);
  }

  /** The events as if the conversation's last moment was a minute ago, so
   *  a turn still under way shows minutes, not the days since its date. */
  recent(): SessionEvent[] {
    const shift = Date.now() - 60_000 - this.clock;
    const moved = (at: string) => new Date(Date.parse(at) + shift).toISOString();
    return this.events.map((event) => {
      const item = "item" in event && typeof event.item === "object" && event.item && "started_at" in event.item && typeof event.item.started_at === "string" ? { ...event.item, started_at: moved(event.item.started_at) } : null;
      return { ...event, at: moved(event.at), ...(item ? { item } : {}) } as SessionEvent;
    });
  }

  private at(seconds: number): string {
    this.clock += seconds * 1000;
    return new Date(this.clock).toISOString();
  }

  private id(): string {
    this.serial += 1;
    return `item-${this.serial}`;
  }

  heard(turn: string | null, from: Speaker | null, text: string, after = 4, notice?: Notice, attachments?: Attachment[], images?: UserImage[]): void {
    const message = (attachments || images) && {
      ...(attachments ? { attachments } : {}),
      ...(images ? { images } : {}),
      id: this.id(),
      client_id: null,
      content: [],
      hook_fragments: [],
      presentation: null,
      detail: null,
    };
    this.events.push({ at: this.at(after), event: "heard", delivery: null, turn, from, text, ...(notice ? { notice } : {}), ...(message ? { message } : {}) });
  }

  started(turn: string): void {
    this.events.push({ at: this.at(1), event: "turn_started", turn });
  }

  say(turn: string, kind: "message" | "thinking" | "plan", text: string, after = 3, writing = false): void {
    const item = { id: this.id(), kind, text } as Item;
    this.events.push({ at: this.at(after), event: "began", turn, item: { ...item, text: "" } as Item });
    if (writing) {
      this.events.push({ at: this.at(1), event: "grew", turn, item: item.id, part: "text", text });
      return;
    }
    // Thinking says how long it took, as the hosts now tell it.
    const took = kind === "thinking" ? { duration_ms: 1200 + ((this.serial * 2711) % 9000) } : {};
    this.events.push({ at: this.at(1), event: "finished", turn, item: { ...item, ...took } as Item });
  }

  /** Returns the item's id, which facts told while it runs name. */
  tool(turn: string, tool: Partial<Tool> & { name: string; does: Does }, after = 2, running = false): string {
    const full: Tool = {
      server: "fastctx",
      machine: null,
      input: {},
      output: [],
      status: running ? "running" : "done",
      duration_ms: null,
      ...tool,
    };
    const id = this.id();
    const began = this.at(after);
    this.events.push({
      at: began,
      event: "began",
      turn,
      item: { id, started_at: began, kind: "tool", ...full, status: "running", output: [] },
    });
    if (running) return id;
    this.events.push({ at: this.at(Math.max(1, (full.duration_ms ?? 800) / 1000)), event: "finished", turn, item: { id, kind: "tool", ...full } });
    return id;
  }

  ended(turn: string, how: string, reason: string | null = null, after = 1, took?: { ms: number; facts: TurnFacts }): void {
    this.events.push({
      at: this.at(after),
      event: "turn_ended",
      turn,
      end: { how, reason, duration_ms: took?.ms ?? null, last_message: null, ...(took ? { facts: took.facts } : {}) },
    });
  }

  push(event: import("../world/events").Happened, after = 1): void {
    this.events.push({ at: this.at(after), ...event } as SessionEvent);
  }
}

export const text = (value: string): Content[] => [{ type: "text", text: value }];

/** A World run as its host reports it, the model's text in its output and
 *  the World's facts beside it; a run still going has none yet. */
export function run(command: string, head: string, body: string, extra: Partial<Tool> = {}, node?: string): Partial<Tool> & { name: string; does: Does } {
  const exit = /exited (\d+)/.exec(head);
  const job = /; (j-[0-9a-z]+)\)/.exec(head);
  const facts = exit ? ran(node ?? "laptop", `j-${(command.length * 7919).toString(16).padStart(6, "0").slice(-6)}`, { kind: "ended", status: { code: Number(exit[1]) } }, body ? [printed(1, `${body}\n`)] : []) : undefined;
  return {
    name: "run",
    machine: node ?? "laptop",
    input: node ? { command, node } : { command },
    output: text(`${head}\n${body}`),
    does: { as: "command", command, cwd: null, exit: exit ? Number(exit[1]) : null, job: job ? job[1] : null },
    ...(facts ? { run: facts } : {}),
    ...extra,
  };
}

/** A file the user sent in a specimen, as the World would have kept it. */
export function sent(name: string, media_type: string, size: number): Attachment {
  return {
    id: `n-${name}`,
    name,
    media_type,
    size,
    sha256: name.padEnd(64, "0").slice(0, 64),
    owner: { machine: "laptop", resident: "claude-7f3a" },
    town: "mark50",
    path: `C:/Users/me/.fastctx/world/residents/claude-7f3a/sent/${name}`,
  };
}
