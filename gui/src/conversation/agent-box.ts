// What one of the host's own agents did, opened under the row of the call
// that started it (`world-gui.md`, 对话): its own conversation, drawn as the
// page draws one, its steps and its words, in a box of its own that
// scrolls, following its end while it works unless the reader moved up in
// it. What it was asked heads it; what it took sits at its foot.
//
// An agent seen live from its start is drawn from what came as it
// happened (`agents.ts`). Any other is read from its own record, on the
// machine its conversation lives on, when the box opens, and read again
// as it takes more steps, until it ends.

import type { AgentActivity, MachineName, SessionEvent } from "../world/events";
import type { Words } from "../words";
import type { AgentRun } from "./agents";
import { Conversation } from "./conversation";
import { ConversationView, type Context } from "./view";
import { detailOf } from "../app/problem";

export interface AgentBoxContext {
  words: Words;
  english: boolean;
  here: MachineName;
  residentMachine: MachineName;
  lines?: Context["lines"];
  /** Reads an agent's own record; absent where it cannot be read. */
  record?: ((agent: string) => Promise<AgentActivity>) | undefined;
  /** The reader opened or moved in it: the page keeps still around it. */
  reading(): void;
}

/** How long a record read again waits after the last, as an agent not
 *  seen whole takes steps: a few of them at a time rather than each. */
const AGAIN = 1500;

/** How near its end a box is still following it, in pixels. */
const NEAR = 24;

export class AgentBox {
  readonly element: HTMLElement;
  private readonly scroller: HTMLElement;
  private readonly foot: HTMLElement;
  private readonly note: HTMLElement;
  private readonly conversation = new Conversation();
  private readonly view: ConversationView;
  /** How many of the agent's live events are drawn, where they are. */
  private fed = 0;
  /** Drawn from its record, not from what came live. */
  private recorded: boolean;
  private following = true;
  private reading = 0;
  private again = 0;
  /** The steps and state the record was last read at. */
  private readAt = "";
  private frame = 0;
  private closed = false;
  private sized = { client: 0, content: 0 };

  constructor(
    private run: AgentRun,
    private readonly context: AgentBoxContext,
  ) {
    this.element = document.createElement("div");
    this.element.className = "agent-box";
    this.element.innerHTML = `<div class="agent-scroll"><div class="agent-transcript conversation"></div></div><p class="agent-note" hidden></p><p class="agent-foot" hidden></p>`;
    this.scroller = this.element.querySelector<HTMLElement>(".agent-scroll")!;
    this.note = this.element.querySelector<HTMLElement>(".agent-note")!;
    this.foot = this.element.querySelector<HTMLElement>(".agent-foot")!;
    this.view = new ConversationView(this.element.querySelector<HTMLElement>(".agent-transcript")!, this.conversation, {
      words: context.words,
      english: context.english,
      here: context.here,
      residentMachine: context.residentMachine,
      scroller: this.scroller,
      nested: true,
      reading: () => {
        this.following = false;
        context.reading();
      },
      following: () => this.following,
      ...(context.lines ? { lines: context.lines } : {}),
    });
    this.scroller.addEventListener("scroll", this.scrolled, { passive: true });
    run.held += 1;
    this.recorded = !run.whole && !!context.record;
    if (this.recorded) void this.read();
    else this.feed();
    this.drawFoot();
  }

  /** The agent moved: what it did since is drawn, its record read again
   *  where it is drawn from that. */
  update(run: AgentRun): void {
    // The conversation was read again: the agent is told anew, and what
    // came live of it before is gone from what is held.
    if (run !== this.run) {
      this.let(this.run);
      run.held += 1;
      this.fed = 0;
      if (!this.recorded && !run.whole && this.context.record) {
        this.recorded = true;
        this.run = run;
        void this.read();
        this.drawFoot();
        return;
      }
    }
    this.run = run;
    if (this.closed) return;
    if (this.recorded) {
      if (this.readAt !== readMark(run) && !this.again) this.again = window.setTimeout(() => void this.read(), AGAIN);
    } else if (!this.frame) {
      this.frame = requestAnimationFrame(() => {
        this.frame = 0;
        this.feed();
      });
    }
    this.drawFoot();
  }

  destroy(): void {
    this.closed = true;
    cancelAnimationFrame(this.frame);
    clearTimeout(this.again);
    this.scroller.removeEventListener("scroll", this.scrolled);
    this.view.destroy();
    this.let(this.run);
  }

  /** The box no longer shows a run: what came of it live may go. */
  private let(run: AgentRun): void {
    run.held -= 1;
    run.release();
  }

  /** What came live and is not drawn yet. Its first words, where they did
   *  not come, are what it was asked. */
  private feed(): void {
    const events = this.run.events;
    if (this.fed === 0) {
      const asked = this.run.prompt;
      const heard = events.some((event) => event.event === "heard");
      const first: SessionEvent[] = asked && !heard ? [{ at: events[0]?.at ?? new Date(this.run.began ?? Date.now()).toISOString(), event: "heard", delivery: null, turn: null, from: null, text: asked }] : [];
      this.conversation.history([...first, ...events]);
      this.fed = events.length;
    } else if (this.fed < events.length) {
      this.conversation.extend(events.slice(this.fed));
      this.fed = events.length;
    }
    this.draw();
  }

  /** Its record, read from its conversation's machine. */
  private async read(): Promise<void> {
    this.again = 0;
    const record = this.context.record;
    if (!record || this.closed) return;
    const reading = ++this.reading;
    const mark = readMark(this.run);
    const first = !this.readAt;
    if (first) this.say(this.context.words.agents.reading);
    try {
      const activity = await record(this.run.id);
      if (this.closed || reading !== this.reading) return;
      this.readAt = mark;
      const asked = this.run.prompt;
      const heard = activity.events.some((event) => event.event === "heard");
      const head: SessionEvent[] =
        asked && !heard ? [{ at: activity.events[0]?.at ?? new Date().toISOString(), event: "heard", delivery: null, turn: null, from: null, text: asked }] : [];
      this.conversation.history([...head, ...activity.events]);
      this.say(activity.unavailable ? this.context.words.agents.unreadable : null, activity.unavailable ?? "");
      this.draw();
      // It took more steps while it was read.
      if (readMark(this.run) !== this.readAt && !this.again) this.again = window.setTimeout(() => void this.read(), AGAIN);
    } catch (error) {
      if (this.closed || reading !== this.reading) return;
      this.say(this.context.words.agents.unreadable, detailOf(error));
    }
  }

  private draw(): void {
    this.view.update(this.recorded);
    if (!this.conversation.blocks().length && !this.note.textContent) this.say(this.context.words.agents.nothing);
    else if (this.note.textContent === this.context.words.agents.nothing && this.conversation.blocks().length) this.say(null);
    if (this.following) this.scroller.scrollTop = this.scroller.scrollHeight;
  }

  private say(said: string | null, why = ""): void {
    this.note.hidden = !said;
    this.note.textContent = said ?? "";
    this.note.title = why;
  }

  /** What it took, once it ended: its steps, how long, its tokens, its
   *  model; why, where it did not finish. */
  private drawFoot(): void {
    const run = this.run;
    const { words } = this.context;
    const said =
      run.going || run.state === "unknown"
        ? ""
        : [
            run.state === "failed" ? (run.reason ?? words.agents.failed) : run.state === "stopped" ? words.agents.stopped : "",
            words.agents.tools(run.tools ?? run.steps.length),
            run.duration !== null ? words.took(run.duration) : run.ended !== null && run.began !== null ? words.took(run.ended - run.began) : "",
            run.tokens !== null ? words.thinkingTokens(run.tokens) : "",
            run.model ?? "",
          ]
            .filter(Boolean)
            .join(" ");
    this.foot.hidden = !said;
    if (this.foot.textContent !== said) this.foot.textContent = said;
    this.foot.classList.toggle("wrong", run.state === "failed");
  }

  /** The reader scrolled in it: it follows its end only while they are at
   *  the end. */
  private scrolled = (): void => {
    const { scrollTop, clientHeight, scrollHeight } = this.scroller;
    if (clientHeight !== this.sized.client || scrollHeight !== this.sized.content) {
      this.sized = { client: clientHeight, content: scrollHeight };
      if (this.following) this.scroller.scrollTop = scrollHeight;
      return;
    }
    this.following = scrollTop + clientHeight >= scrollHeight - NEAR;
  };
}

/** What a record read again is read for: more steps, or another state. */
function readMark(run: AgentRun): string {
  return `${run.steps.length}|${run.steps.filter((step) => step.ended).length}|${run.state}`;
}
