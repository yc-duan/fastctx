// A conversation's page: who it is with and what they are doing, the
// conversation itself in a region that scrolls on its own, and the words
// the user is about to send, always at the foot.

import type { Words } from "../words";
import { glyph } from "./glyphs";
import type { Conversation } from "./model";
import { escape } from "./prose";
import { ConversationView } from "./view";

export interface Someone {
  /** "Claude" or "Codex": the agent by the name it goes by. */
  agent: string;
  machine: string;
  model: string | null;
  town: string | null;
}

export interface PageOptions {
  words: Words;
  here: string;
  someone: Someone;
  /** The head every page has: the way back to the sidebar where it is
   *  hidden, a title, and actions. */
  head(title: string, actions?: string): string;
  /** Where another resident's conversation opens. */
  link?: (address: { resident: string; machine: string }) => string;
  conversation: Conversation;
  /** Sends the user's words; resolves once they are on their way. */
  send(text: string): void;
  stop(): void;
}

export class ConversationPage {
  readonly element: HTMLElement;
  private view: ConversationView;
  private state: HTMLElement;
  private context: HTMLElement;
  private input: HTMLTextAreaElement;
  private action: HTMLButtonElement;
  private scroller: HTMLElement;
  private following = true;
  private gone: string | null = null;

  constructor(private readonly options: PageOptions) {
    const { words, someone } = options;
    this.element = document.createElement("div");
    this.element.className = "conversation-page";
    this.element.innerHTML = `
      ${options.head(`<div class="who">
        <h1 class="who-name">${escape(someone.agent)}<span class="who-at"> · ${escape(someone.machine)}</span></h1>
        <div class="who-state"></div>
      </div>`)}
      <div class="talk-scroller scroller"><div class="conversation"></div></div>
      <footer class="composer-dock">
        <ul class="running" hidden></ul>
        <form class="composer">
          <textarea rows="1" enterkeyhint="send" placeholder="${escape(words.sendPlaceholder(someone.agent))}"></textarea>
          <div class="composer-row">
            <span class="composer-context"></span>
            <button class="composer-action" type="submit"></button>
          </div>
        </form>
      </footer>`;
    this.scroller = this.element.querySelector<HTMLElement>(".talk-scroller")!;
    const main = this.element.querySelector<HTMLElement>(".conversation")!;
    this.state = this.element.querySelector<HTMLElement>(".who-state")!;
    this.context = this.element.querySelector<HTMLElement>(".composer-context")!;
    this.input = this.element.querySelector("textarea")!;
    this.action = this.element.querySelector<HTMLButtonElement>(".composer-action")!;
    this.view = new ConversationView(main, options.conversation, {
      words,
      here: options.here,
      residentMachine: someone.machine,
      link: options.link,
    });

    const form = this.element.querySelector("form")!;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      this.submit();
    });
    this.input.addEventListener("input", () => {
      this.fit();
      this.drawAction();
    });
    this.input.addEventListener("keydown", (event) => {
      // Enter sends where there is a keyboard; on a touch screen it is a new
      // line, and the button sends.
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !matchMedia("(pointer: coarse)").matches) {
        event.preventDefault();
        this.submit();
      }
    });
    this.scroller.addEventListener("scroll", () => {
      const { scrollTop, clientHeight, scrollHeight } = this.scroller;
      this.following = scrollTop + clientHeight >= scrollHeight - 48;
      this.element.classList.toggle("scrolled", scrollTop > 0);
    });
  }

  /** Brings the page up to the conversation; keeps the newest in view when
   *  the reader was already there. */
  update(): void {
    const follow = this.following;
    this.view.update();
    this.drawState();
    this.drawAction();
    if (follow) this.scroller.scrollTop = this.scroller.scrollHeight;
  }

  private submit(): void {
    const text = this.input.value.trim();
    if (!text) {
      if (this.options.conversation.moving()) this.options.stop();
      return;
    }
    this.input.value = "";
    this.fit();
    this.following = true;
    this.options.send(text);
  }

  private fit(): void {
    this.input.style.height = "auto";
    this.input.style.height = `${this.input.scrollHeight}px`;
  }

  /** What the resident started that still runs, hung above the composer:
   *  when one ends, the resident is told and the conversation goes on. */
  running(jobs: { job: string; machine: string; started_at: string; command: string }[], now: Date): void {
    const { words } = this.options;
    const list = this.element.querySelector<HTMLElement>(".running")!;
    list.hidden = jobs.length === 0;
    list.innerHTML = jobs.length
      ? `<li class="running-title">${escape(words.stillRunning)}</li>${jobs
          .map(
            (job) =>
              `<li class="running-job"><span class="pulse" aria-hidden="true"></span><span class="running-what">${escape(words.jobRunning(job.job, job.machine, lasted(now.getTime() - Date.parse(job.started_at), words)))}</span><code class="running-command">${escape(job.command)}</code></li>`,
          )
          .join("")}`
      : "";
  }

  /** The watch on the session is over: what is shown stays, and the bar
   *  says it is no longer live. */
  lost(why: string | null): void {
    this.gone = this.options.words.watchEnded(why);
    this.drawState();
  }

  private drawState(): void {
    const { words, someone, conversation } = this.options;
    const working = !this.gone && conversation.moving();
    const facts = [this.gone ?? (working ? words.working : words.idle), someone.town, someone.model].filter(
      Boolean,
    ) as string[];
    this.state.innerHTML = `${working ? `<span class="pulse" aria-hidden="true"></span>` : ""}<span>${facts
      .map(escape)
      .join(" · ")}</span>`;
    this.state.classList.toggle("working", working);
    this.state.classList.toggle("gone", this.gone !== null);
    const used = conversation.context;
    this.context.textContent =
      used && used.window ? words.context(Math.round((used.used / used.window) * 100)) : "";
  }

  private drawAction(): void {
    const { words, conversation } = this.options;
    const typed = this.input.value.trim() !== "";
    const stop = !typed && conversation.moving();
    this.action.innerHTML = glyph(stop ? "stop" : "send", 18);
    this.action.setAttribute("aria-label", stop ? words.stop : words.send);
    this.action.classList.toggle("stop", stop);
    this.action.disabled = !typed && !stop;
  }
}

/** A span of time as the interface says it elsewhere: minutes, hours. */
function lasted(ms: number, words: Words): string {
  const minutes = Math.max(0, Math.round(ms / 60_000));
  return minutes < 60 ? words.minutes(minutes) : words.hours(Math.floor(minutes / 60), minutes % 60);
}
