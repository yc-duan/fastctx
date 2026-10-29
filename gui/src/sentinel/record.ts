// What a watcher did, as its page reads it: its record drawn as the page
// draws a conversation, read only, each wake a turn. Read from the machine it
// lives on when the page opens, then again while it is open: every few
// seconds while it is awake, every half minute while it sleeps; a record that
// has not changed comes back as a word, not as itself.

import type { MachineName, SessionEvent } from "../world/events";
import type { SentinelRecord } from "../world/sentinels";
import type { Words } from "../words";
import { Conversation } from "../conversation/conversation";
import { ConversationView, type Context } from "../conversation/view";
import { handOnAtEdge } from "../scroll-handoff";

export interface RecordContext {
  words: Words;
  english: boolean;
  here: MachineName;
  /** The machine it lives on. */
  home: MachineName;
  read(held: string | null): Promise<SentinelRecord>;
  /** The record read anew, after it changed: what the page draws of it. */
  changed(record: SentinelRecord): void;
  /** Its record could not be read; why, in the World's words. */
  failed(error: unknown): void;
  /** The reader moved through it. */
  moved(): void;
  lines?: Context["lines"];
}

const AWAKE = 3000;
const ASLEEP = 30_000;
const NEAR = 24;

export class RecordColumn {
  readonly element: HTMLElement;
  private readonly conversation = new Conversation();
  private readonly view: ConversationView;
  private version: string | null = null;
  private awake = false;
  private following = true;
  private timer = 0;
  private frame = 0;
  private closed = false;

  constructor(
    private readonly scroller: HTMLElement,
    private readonly context: RecordContext,
  ) {
    this.element = document.createElement("div");
    this.element.className = "sn-transcript conversation";
    scroller.append(this.element);
    handOnAtEdge(scroller);
    this.view = new ConversationView(this.element, this.conversation, {
      words: context.words,
      english: context.english,
      here: context.here,
      residentMachine: context.home,
      scroller,
      reading: () => {
        this.following = false;
      },
      following: () => this.following,
      ...(context.lines ? { lines: context.lines } : {}),
    });
    scroller.addEventListener("scroll", this.scrolled, { passive: true });
    void this.read();
  }

  /** Whether anything of it was drawn. */
  get drawn(): boolean {
    return this.conversation.blocks().length > 0;
  }

  /** Brings the start of the wake `turn` into view. */
  show(turn: string): void {
    const start = this.starts().find((one) => one.turn === turn);
    if (!start) return;
    this.following = false;
    this.view.bring(start.key);
    this.view.elementOf(start.key)?.scrollIntoView({ block: "start", behavior: "smooth" });
  }

  /** The wake being read: the last whose start is above a third of the way
   *  down, or the latest while the reader follows the end. */
  inView(): string | null {
    const starts = this.starts();
    if (!starts.length) return null;
    if (this.following) return starts[starts.length - 1]!.turn;
    const line = this.scroller.getBoundingClientRect().top + this.scroller.clientHeight / 3;
    let current: string | null = null;
    for (const [index, start] of starts.entries()) {
      // Far from view, a wake's blocks are not on the page.
      const element = this.view.elementOf(start.key);
      if (!element) continue;
      if (element.getBoundingClientRect().top <= line) current = start.turn;
      else return current ?? starts[Math.max(0, index - 1)]!.turn;
    }
    return current ?? starts[0]!.turn;
  }

  destroy(): void {
    this.closed = true;
    clearTimeout(this.timer);
    cancelAnimationFrame(this.frame);
    this.scroller.removeEventListener("scroll", this.scrolled);
    this.view.destroy();
  }

  /** The first block of each wake, in order. */
  private starts(): { turn: string; key: string }[] {
    const starts: { turn: string; key: string }[] = [];
    let last: string | null = null;
    for (const block of this.conversation.blocks()) {
      const turn = "turn" in block ? block.turn : undefined;
      if (typeof turn !== "string" || turn === last) continue;
      last = turn;
      starts.push({ turn, key: block.key });
    }
    return starts;
  }

  private async read(): Promise<void> {
    try {
      const record = await this.context.read(this.version);
      if (this.closed) return;
      if (!record.unchanged) {
        this.version = record.version;
        this.awake = awake(record.events);
        this.conversation.history(record.events);
        this.view.update(true);
        if (this.following) this.scroller.scrollTop = this.scroller.scrollHeight;
        this.context.changed(record);
      }
    } catch (error) {
      if (this.closed) return;
      this.context.failed(error);
    }
    this.timer = window.setTimeout(() => void this.read(), this.awake ? AWAKE : ASLEEP);
  }

  private scrolled = (): void => {
    const { scrollTop, clientHeight, scrollHeight } = this.scroller;
    this.following = scrollTop + clientHeight >= scrollHeight - NEAR;
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      if (!this.closed) this.context.moved();
    });
  };
}

/** Whether its last wake recorded is still under way. */
function awake(events: SessionEvent[]): boolean {
  let started: string | null = null;
  for (const event of events) {
    if (event.event === "turn_started") started = event.turn;
    else if (event.event === "turn_ended" && event.turn === started) started = null;
  }
  return started !== null;
}
