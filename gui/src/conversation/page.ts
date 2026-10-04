// A conversation's page: who it is with and what they are doing, the
// conversation itself, and the words the user is about to send.

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
  /** Where the reader came from, to go back to. */
  back: { href: string; label: string } | null;
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
  private following = true;

  constructor(private readonly options: PageOptions) {
    const { words, someone } = options;
    this.element = document.createElement("div");
    this.element.className = "page conversation-page";
    this.element.innerHTML = `
      <header class="bar">
        <div class="bar-inner">
          ${options.back ? `<a class="bar-back" href="${escape(options.back.href)}" aria-label="${escape(options.back.label)}">${glyph("chevron", 20)}</a>` : ""}
          <div class="who">
            <h1 class="who-name">${escape(someone.agent)}<span class="who-at"> · ${escape(someone.machine)}</span></h1>
            <div class="who-state"></div>
          </div>
        </div>
      </header>
      <main class="conversation"></main>
      <footer class="composer-dock">
        <form class="composer">
          <textarea rows="1" enterkeyhint="send" placeholder="${escape(words.sendPlaceholder(someone.agent))}"></textarea>
          <div class="composer-row">
            <span class="composer-context"></span>
            <button class="composer-action" type="submit"></button>
          </div>
        </form>
      </footer>`;
    const main = this.element.querySelector<HTMLElement>(".conversation")!;
    this.state = this.element.querySelector<HTMLElement>(".who-state")!;
    this.context = this.element.querySelector<HTMLElement>(".composer-context")!;
    this.input = this.element.querySelector("textarea")!;
    this.action = this.element.querySelector<HTMLButtonElement>(".composer-action")!;
    this.view = new ConversationView(main, options.conversation, {
      words,
      here: options.here,
      residentMachine: someone.machine,
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
    addEventListener("scroll", () => {
      this.following = innerHeight + scrollY >= document.documentElement.scrollHeight - 48;
      this.element.classList.toggle("scrolled", scrollY > 0);
    });
  }

  /** Brings the page up to the conversation; keeps the newest in view when
   *  the reader was already there. */
  update(): void {
    const follow = this.following;
    this.view.update();
    this.drawState();
    this.drawAction();
    if (follow) scrollTo({ top: document.documentElement.scrollHeight });
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

  private drawState(): void {
    const { words, someone, conversation } = this.options;
    const working = conversation.moving();
    const facts = [working ? words.working : words.idle, someone.town, someone.model].filter(Boolean) as string[];
    this.state.innerHTML = `${working ? `<span class="pulse" aria-hidden="true"></span>` : ""}<span>${facts
      .map(escape)
      .join(" · ")}</span>`;
    this.state.classList.toggle("working", working);
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
