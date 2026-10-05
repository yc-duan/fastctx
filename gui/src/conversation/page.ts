// A conversation's page: who it is with and what they are doing, the
// conversation itself in a region that scrolls on its own, and the words
// the user is about to send, always at the foot.

import type { ComposerWords } from "../app/words";
import type { Words } from "../words";
import type { Control, Controlled, ModelOffer, Offers } from "../world/control";
import type { Attachment } from "../world/attachments";
import type { ResidentDetail, ResidentNow } from "../world/world";
import { AttachTray, type AttachSeam, type AttachWords, type Attached } from "./attach";
import { SendQueue, type Queued } from "./queue";
import { Menu, commandRow, commandsOf, modelRow, type MenuCommand } from "./commands";
import { fileRow, mentionAt, mentioned, type FileWords, type FoundFile } from "./mention";
import type { Details } from "./details";
import { glyph } from "./glyphs";
import type { Conversation } from "./model";
import { escape } from "./prose";
import { shownOf, skillsHtml } from "./said";
import { Finder } from "./find";
import { ConversationView } from "./view";

export interface Someone {
  /** "Claude" or "Codex": the agent by the name it goes by. */
  agent: string;
  machine: string;
  model: string | null;
  /** The project it is in, by the name the user gave it. */
  project: string | null;
}

/** What came of words sent: a turn this conversation will see end; taken,
 *  but into a turn whose end it will not see (delivery uncertain, or a host
 *  that tells the World nothing of its turns); or not taken. */
export type Sent = "turn" | "unseen" | "failed";

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
  /** What the World knows of the agent, shown in a card opened from the
   *  head, and how to ask for it. */
  details?: {
    card: Details;
    label: string;
    look(): Promise<ResidentDetail | null>;
    colleagues(): ResidentNow[];
  };
  /** What the conversation offers, and doing it: its commands, its
   *  model, its usage. */
  controls?: {
    words: ComposerWords;
    offers(): Promise<Offers>;
    control(what: Control): Promise<Controlled>;
    /** A fork was made: where it lives. */
    forked(resident: { resident: string; machine: string }): void;
    /** Opens a link in the system's browser. */
    open(url: string): Promise<void>;
  };
  /** Where the words being written are kept between visits. */
  draftKey?: string;
  /** Attaching files to the words, where the World can carry them. */
  attach?: {
    words: AttachWords;
    seam: AttachSeam;
    /** Files dropped onto the window, by path, where the shell gives
     *  them so; returns how to stop listening. */
    drops?(on: { over(over: boolean): void; dropped(paths: string[]): void }): () => void;
  };
  /** Files on the agent's machine, found as the user types `@` and a few
   *  letters; the one chosen goes into the words as its path. */
  files?: {
    words: FileWords;
    find(query: string): Promise<FoundFile[]>;
    /** Why there is nowhere to look, when there is not: it is in no
     *  project, or its project lists no place on its machine. */
    nowhere?(): "outside" | "unlisted" | null;
  };
  /** Sends the user's words, and what arrived of what was attached;
   *  resolves with what came of them. */
  send(text: string, attached: { refs: unknown[]; shown: Attached[] }): Promise<Sent> | void;
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
  private name: HTMLElement;
  private looking = 0;
  private following = true;
  private gone: string | null = null;
  private offered: Offers | null = null;
  private wasMoving = false;
  private commandMenu: Menu<MenuCommand> | null = null;
  private modelMenu: Menu<ModelOffer> | null = null;
  private effortMenu: Menu<string> | null = null;
  private fileMenu: Menu<FoundFile> | null = null;
  /** The latest `@` question; an answer to an earlier one is not shown. */
  private mentionAsked = 0;
  private note: HTMLElement;
  private chip: HTMLButtonElement;
  private finder: Finder;
  /** Ctrl+F finds in the conversation while its page is shown. */
  private readonly findKey = (event: KeyboardEvent) => {
    if (!this.element.isConnected || !(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "f" || event.altKey) return;
    event.preventDefault();
    this.finder.show();
  };

  constructor(private readonly options: PageOptions) {
    const { words, someone } = options;
    this.element = document.createElement("div");
    this.element.className = "conversation-page";
    this.element.innerHTML = `
      ${options.head(
        `<div class="who">
        <h1 class="who-name">${escape(someone.agent)}<span class="who-at"> · ${escape(someone.machine)}</span></h1>
        <div class="who-state"></div>
      </div>`,
        `<button class="icon-button find-toggle" type="button" aria-label="${escape(words.find.find)}" title="${escape(words.find.find)}">${glyph("search", 18)}</button>${
          options.details
            ? `<button class="icon-button details-toggle" type="button" aria-expanded="false" aria-label="${escape(options.details.label)}" title="${escape(options.details.label)}">${glyph("more", 20)}</button>`
            : ""
        }`,
      )}
      <div class="talk-scroller scroller"><div class="conversation"></div></div>
      <footer class="composer-dock">
        <ul class="running" hidden></ul>
        <p class="composer-gone" hidden></p>
        <form class="composer">
          <div class="composer-menus"><div class="composer-menu usage-pop" hidden></div><div class="composer-menu said-pop" role="dialog" hidden></div></div>
          <div class="composer-confirm" hidden></div>
          <div class="composer-queue"></div>
          <div class="composer-tray"></div>
          <textarea rows="1" enterkeyhint="send" placeholder="${escape(words.sendPlaceholder(someone.agent))}" aria-label="${escape(words.sendPlaceholder(someone.agent))}"></textarea>
          <div class="composer-row">
            <div class="composer-tools"><button class="icon-button composer-attach" type="button" hidden></button><input class="composer-pick" type="file" multiple hidden><button class="icon-button composer-photo" type="button" hidden></button><input class="composer-shot" type="file" accept="image/*" capture="environment" hidden><button class="composer-chip" type="button" hidden></button></div>
            <span class="composer-note" aria-live="polite"></span>
            <button class="composer-context" type="button" hidden></button>
            <button class="composer-action" type="submit" aria-label="${escape(words.send)}"></button>
          </div>
        </form>
      </footer>`;
    this.scroller = this.element.querySelector<HTMLElement>(".talk-scroller")!;
    const transcript = this.element.querySelector<HTMLElement>(".conversation")!;
    this.finder = new Finder(transcript, this.scroller, words.find, () => {
      this.following = false;
    });
    this.scroller.before(this.finder.element);
    this.element.querySelector(".find-toggle")!.addEventListener("click", () => (this.finder.open ? this.finder.close() : this.finder.show()));
    document.addEventListener("keydown", this.findKey);
    this.name = this.element.querySelector<HTMLElement>(".who-name")!;
    // The title is renamed in place, where the conversation takes a name.
    this.name.addEventListener("click", () => this.rename());
    this.name.addEventListener("keydown", (event) => {
      if ((event.key === "Enter" || event.key === " ") && !this.renaming && event.target === this.name) {
        event.preventDefault();
        this.rename();
      }
    });
    if (options.details) {
      const { card } = options.details;
      this.element.append(card.element);
      const toggle = this.element.querySelector<HTMLButtonElement>(".details-toggle")!;
      toggle.addEventListener("click", () => this.toggleDetails(!card.open));
      this.element.addEventListener("keydown", (event) => {
        if (event.key === "Escape" && card.open) this.toggleDetails(false);
      });
    }
    const main = this.element.querySelector<HTMLElement>(".conversation")!;
    this.state = this.element.querySelector<HTMLElement>(".who-state")!;
    this.spoken = document.createElement("p");
    this.spoken.className = "spoken";
    this.spoken.setAttribute("role", "status");
    this.element.append(this.spoken);
    this.context = this.element.querySelector<HTMLElement>(".composer-context")!;
    this.input = this.element.querySelector("textarea")!;
    this.action = this.element.querySelector<HTMLButtonElement>(".composer-action")!;
    this.note = this.element.querySelector<HTMLElement>(".composer-note")!;
    this.queue = new SendQueue(options.draftKey ?? "", words.queue, {
      now: (item) => this.sendQueued(item, true),
      changed: () => this.drawAction(),
    });
    this.element.querySelector(".composer-queue")!.replaceWith(this.queue.element);
    this.chip = this.element.querySelector<HTMLButtonElement>(".composer-chip")!;
    this.input.value = this.draft() ?? "";
    if (options.controls) this.menus(options.controls);
    if (options.files) this.fileMenuOf();
    this.context.addEventListener("click", () => this.toggleUsage());
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && !this.element.querySelector<HTMLElement>(".said-pop")!.hidden) {
        this.hideSaid();
        return;
      }
      const pop = this.element.querySelector<HTMLElement>(".usage-pop")!;
      if (event.key !== "Escape" || pop.hidden) return;
      const inside = pop.contains(document.activeElement) || document.activeElement === this.context;
      pop.hidden = true;
      this.context.setAttribute("aria-expanded", "false");
      if (inside) this.context.focus({ preventScroll: true });
    });
    if (options.attach) this.attaching(options.attach);
    const controls = options.controls;
    this.view = new ConversationView(main, options.conversation, {
      words,
      here: options.here,
      residentMachine: someone.machine,
      link: options.link,
      asks: controls && {
        words: controls.words.asks,
        answer: async (request, answer) => {
          await controls.control({ do: "answer", request, answer });
        },
        open: controls.open,
        settled: () => this.update(),
      },
      turnAct: controls && ((act, turn, words, kept) => this.turnAct(act, turn, words, kept)),
    });

    const form = this.element.querySelector("form")!;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      this.submit();
    });
    this.input.addEventListener("input", () => {
      this.fit();
      this.drawAction();
      this.keepDraft();
      this.slash();
      this.mention();
    });
    this.input.addEventListener("blur", () => {
      this.commandMenu?.hide();
      this.fileMenu?.hide();
    });
    this.input.addEventListener("keydown", (event) => {
      for (const menu of [this.fileMenu, this.effortMenu, this.modelMenu, this.commandMenu]) {
        if (menu?.key(event)) {
          event.preventDefault();
          return;
        }
      }
      // Enter sends where there is a keyboard; on a touch screen it is a new
      // line, and the button sends.
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !matchMedia("(pointer: coarse)").matches) {
        event.preventDefault();
        this.submit();
      }
    });
    // Whatever grows after it is drawn (fonts, formulas, diagrams, a card
    // being answered) keeps the newest in view while the reader follows.
    // The region itself shrinks when the composer grows (an attachment, a
    // longer draft): the same holds.
    const keep = new ResizeObserver(() => {
      if (this.following) this.scroller.scrollTop = this.scroller.scrollHeight;
    });
    keep.observe(main);
    keep.observe(this.scroller);
    // Only a scroll the reader made says whether they still follow: one
    // that comes with the region or its content changing size (the
    // browser keeping the view anchored) does not.
    let sized = { client: 0, content: 0 };
    this.scroller.addEventListener("scroll", () => {
      const { scrollTop, clientHeight, scrollHeight } = this.scroller;
      this.element.classList.toggle("scrolled", scrollTop > 0);
      if (clientHeight !== sized.client || scrollHeight !== sized.content) {
        sized = { client: clientHeight, content: scrollHeight };
        if (this.following) this.scroller.scrollTop = scrollHeight;
        return;
      }
      this.following = scrollTop + clientHeight >= scrollHeight - 48;
    });
  }

  /** Brings the page up to the conversation; keeps the newest in view when
   *  the reader was already there. */
  update(): void {
    const follow = this.following;
    this.view.update();
    this.finder.refresh();
    this.drawState();
    this.drawAction();
    if (follow) this.scroller.scrollTop = this.scroller.scrollHeight;
    // What it offers is asked again whenever a turn ends: a command, a
    // compaction or a new model may have changed it.
    const moving = this.options.conversation.moving();
    if (this.wasMoving && !moving) void this.askOffers();
    this.wasMoving = moving;
    this.releaseQueued();
  }

  /** Stops what the page keeps asking for. */
  leave(): void {
    clearTimeout(this.looking);
    this.looking = -1;
    document.removeEventListener("keydown", this.findKey);
    this.stopDrops?.();
  }

  private toggleDetails(open: boolean): void {
    const details = this.options.details;
    if (!details) return;
    const toggle = this.element.querySelector<HTMLElement>(".details-toggle");
    const inside = details.card.element.contains(document.activeElement);
    details.card.open = open;
    toggle?.setAttribute("aria-expanded", String(open));
    clearTimeout(this.looking);
    if (open) {
      details.card.element.focus({ preventScroll: true });
      void this.lookAgain();
    } else if (inside) {
      toggle?.focus({ preventScroll: true });
    }
  }

  /** Asks the World about the agent while the card is open, every few
   *  seconds, as the front page asks about everyone. */
  private async lookAgain(): Promise<void> {
    const details = this.options.details;
    if (!details || !details.card.open || this.looking === -1) return;
    try {
      const detail = await details.look();
      if (detail) details.card.show(detail, details.colleagues(), new Date());
      else details.card.unknown(null);
    } catch (error) {
      details.card.unknown(String(error));
    }
    if (details.card.open && this.looking !== -1) this.looking = window.setTimeout(() => void this.lookAgain(), 3000);
  }

  private submit(): void {
    const text = this.input.value.trim();
    if (this.tray?.sending) {
      this.say(this.options.attach!.words.wait);
      return;
    }
    const attached = this.tray?.count ? this.tray.take() : { refs: [], shown: [] };
    if (!text && !attached.refs.length) {
      if (this.options.conversation.moving()) this.options.stop();
      return;
    }
    if (this.options.controls && text.startsWith("/")) {
      const space = text.indexOf(" ");
      const name = (space < 0 ? text.slice(1) : text.slice(1, space)).trim();
      const args = space < 0 ? "" : text.slice(space + 1).trim();
      const command = commandsOf(this.offered, this.options.controls.words.commands).find((c) => c.name === name);
      if (command) {
        this.clear();
        void this.run(command, args);
        return;
      }
      if (this.offered) {
        this.say(this.options.controls.words.unknown(name));
        return;
      }
    }
    this.clear();
    // What the last command did is behind the conversation now.
    this.say("");
    this.hideSaid();
    this.following = true;
    // While a turn is under way, or a queued one is on its way, words wait
    // their turn instead of cutting into it.
    if (this.options.conversation.busy() || this.released !== null) {
      this.queue.add(text, attached);
      return;
    }
    // Words the user sends while the queue waits on them: once their turn
    // ends as it should, the queue goes on.
    this.failed = null;
    void this.options.send(text, attached);
  }

  /** Sends what waits: the first, once a turn has ended as it should and
   *  nothing else is on its way; a turn stopped or failed holds them. */
  private releaseQueued(): void {
    const conversation = this.options.conversation;
    // The one sent last is on its way until a turn after it has ended.
    if (this.released !== null && conversation.ended() > this.released) this.released = null;
    const last = conversation.lastEnd();
    const idle = !conversation.busy() && !conversation.waiting();
    const stopped = idle && last !== null && last.how !== "completed";
    this.queue.hold(this.failed ?? (stopped ? "stopped" : null));
    if (!idle || this.released !== null || this.failed || stopped) return;
    const head = this.queue.head();
    if (head && this.queue.take(head.key)) this.sendQueued(head, false);
  }

  /** Sends one that waited: `now` when the user asked, into the turn under
   *  way; otherwise as the next turn. */
  private sendQueued(item: Queued, now: boolean): void {
    const conversation = this.options.conversation;
    if (!now) this.released = conversation.ended();
    this.failed = null;
    this.following = true;
    const sent = this.options.send(item.text, item.attached);
    void Promise.resolve(sent).then((came) => {
      // No turn to wait for: the rest wait for the user, said above them.
      if (came === "failed" || came === "unseen") {
        this.released = null;
        this.failed = came === "failed" ? "not_taken" : "unsure";
      }
      this.update();
    });
    this.update();
  }

  private clear(): void {
    this.input.value = "";
    this.fit();
    this.keepDraft();
    this.commandMenu?.hide();
    this.drawAction();
  }

  /** A line beside the composer: what a command did, or why it did not. */
  private say(text: string): void {
    this.note.textContent = text;
  }

  /** What a command answered at length, in a panel above the composer
   *  until it is closed or the user sends something. */
  private showSaid(title: string, html: string): void {
    const pop = this.element.querySelector<HTMLElement>(".said-pop")!;
    const close = this.options.controls!.words.said.close;
    pop.innerHTML = `<div class="said-head"><span class="usage-title">${escape(title)}</span><button class="icon-button said-close" type="button" aria-label="${escape(close)}" title="${escape(close)}">${glyph("close", 16)}</button></div><div class="said-body">${html}</div>`;
    pop.setAttribute("aria-label", title);
    pop.hidden = false;
    pop.querySelector(".said-close")!.addEventListener("click", () => this.hideSaid(), { once: true });
  }

  private hideSaid(): void {
    const pop = this.element.querySelector<HTMLElement>(".said-pop")!;
    pop.hidden = true;
    pop.innerHTML = "";
  }

  private draft(): string | null {
    if (!this.options.draftKey) return null;
    try {
      return localStorage.getItem(this.options.draftKey);
    } catch {
      return null;
    }
  }

  private keepDraft(): void {
    if (!this.options.draftKey) return;
    try {
      if (this.input.value) localStorage.setItem(this.options.draftKey, this.input.value);
      else localStorage.removeItem(this.options.draftKey);
    } catch {
      // Not kept: the words are lost if the page is left.
    }
  }

  private menus(controls: NonNullable<PageOptions["controls"]>): void {
    const holder = this.element.querySelector<HTMLElement>(".composer-menus")!;
    const words = controls.words;
    this.commandMenu = new Menu<MenuCommand>("commands", (command) => commandRow(command), (command) => {
      this.commandMenu!.hide();
      if (command.args) {
        this.input.value = `/${command.name} `;
        this.input.focus();
        this.drawAction();
      } else {
        this.clear();
        void this.run(command, "");
      }
    }, this.input);
    this.modelMenu = new Menu<ModelOffer>(
      "models",
      (model) => modelRow(model, this.offered?.model ?? null, words.commands),
      (model) => {
        this.modelMenu!.hide();
        if (model.efforts.length > 1) {
          this.effortMenu!.show(model.efforts, "");
          this.chosenModel = model.id;
        } else void this.setModel(model.id, model.default_effort);
      },
      this.input,
    );
    this.effortMenu = new Menu<string>(
      "efforts",
      (effort) => `<span class="menu-name">${escape(effort)}${effort === this.offered?.effort ? `<span class="menu-current"> · ${escape(words.commands.current)}</span>` : ""}</span>`,
      (effort) => {
        this.effortMenu!.hide();
        void this.setModel(this.chosenModel, effort);
      },
      this.input,
    );
    holder.append(this.commandMenu.element, this.modelMenu.element, this.effortMenu.element);
    this.chip.addEventListener("click", () => {
      if (this.modelMenu!.open) return this.modelMenu!.hide();
      this.modelMenu!.show(this.offered?.models ?? [], words.noModels);
      this.input.focus();
    });
    void this.askOffers();
  }

  private chosenModel: string | null = null;
  private renaming = false;

  /** Renames the conversation: its title becomes a field, Enter keeps the
   *  new name, Escape or leaving it keeps the old. */
  private rename(): void {
    const controls = this.options.controls;
    if (this.renaming || !controls || !this.offered?.controls.includes("rename")) return;
    this.renaming = true;
    const field = document.createElement("input");
    field.className = "who-rename";
    field.value = this.options.conversation.title ?? "";
    field.placeholder = `${this.options.someone.agent} · ${this.options.someone.machine}`;
    field.setAttribute("aria-label", controls.words.commands.rename);
    this.name.replaceChildren(field);
    field.focus();
    field.select();
    let done = false;
    const finish = (keep: boolean) => {
      if (done) return;
      done = true;
      const name = field.value.trim();
      this.renaming = false;
      if (keep && name && name !== this.options.conversation.title) {
        // Shown at once; the host's own report of the title follows, and a
        // name the host did not take gives way to the one it has.
        const before = this.options.conversation.title;
        this.options.conversation.title = name;
        void this.run({ name: "rename", description: "", args: null, control: "rename", source: null }, "", { do: "rename", name }).then((done) => {
          if (!done && this.options.conversation.title === name) {
            this.options.conversation.title = before;
            this.drawState();
          }
        });
      }
      this.name.innerHTML = "";
      this.drawState();
    };
    field.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.isComposing) {
        event.preventDefault();
        finish(true);
        this.name.focus();
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        finish(false);
        this.name.focus();
      }
    });
    field.addEventListener("blur", () => finish(true));
  }
  private tray: AttachTray | null = null;
  private queue!: SendQueue;
  /** How many turns had ended when the last queued words were sent. */
  private released: number | null = null;
  /** The last queued words were not taken, or went where their turn's end
   *  will not be seen: the rest wait for the user. */
  private failed: "not_taken" | "unsure" | null = null;
  private stopDrops: (() => void) | null = null;

  /** Files come with the words: picked with the paperclip, pasted, or
   *  dropped anywhere on the page. */
  private attaching(attach: NonNullable<PageOptions["attach"]>): void {
    const tray = new AttachTray(attach.words, attach.seam, () => this.drawAction());
    this.tray = tray;
    this.element.querySelector(".composer-tray")!.replaceWith(tray.element);
    const button = this.element.querySelector<HTMLButtonElement>(".composer-attach")!;
    const pick = this.element.querySelector<HTMLInputElement>(".composer-pick")!;
    button.hidden = false;
    button.innerHTML = glyph("attach", 18);
    button.setAttribute("aria-label", attach.words.attach);
    button.title = attach.words.attach;
    button.addEventListener("click", () => pick.click());
    pick.addEventListener("change", () => {
      if (pick.files) tray.addFiles(pick.files);
      pick.value = "";
      this.input.focus();
    });
    // On a phone the camera is one tap away. Elsewhere an input that asks
    // for a capture is only another file picker, so it is not offered.
    if (matchMedia("(pointer: coarse)").matches) {
      const photo = this.element.querySelector<HTMLButtonElement>(".composer-photo")!;
      const shot = this.element.querySelector<HTMLInputElement>(".composer-shot")!;
      photo.hidden = false;
      photo.innerHTML = glyph("camera", 18);
      photo.setAttribute("aria-label", attach.words.photo);
      photo.title = attach.words.photo;
      photo.addEventListener("click", () => shot.click());
      shot.addEventListener("change", () => {
        if (shot.files) tray.addFiles(shot.files);
        shot.value = "";
        this.input.focus();
      });
    }
    this.input.addEventListener("paste", (event) => {
      const files = [...(event.clipboardData?.files ?? [])];
      if (!files.length) return;
      // Pasted files are attached; pasted words still go into the words.
      if (!event.clipboardData?.getData("text/plain")) event.preventDefault();
      tray.addFiles(files);
    });
    const veil = document.createElement("div");
    veil.className = "drop-veil";
    veil.hidden = true;
    veil.innerHTML = `<div class="drop-card">${glyph("attach", 22)}<span>${escape(attach.words.drop(this.options.someone.agent))}</span></div>`;
    this.element.append(veil);
    // In a browser, files dragged over the page; in the shell, the window
    // takes the drop and gives their paths.
    let depth = 0;
    this.element.addEventListener("dragenter", (event) => {
      if (!event.dataTransfer?.types.includes("Files")) return;
      depth += 1;
      veil.hidden = false;
    });
    this.element.addEventListener("dragleave", () => {
      depth = Math.max(0, depth - 1);
      if (!depth) veil.hidden = true;
    });
    this.element.addEventListener("dragover", (event) => {
      if (event.dataTransfer?.types.includes("Files")) event.preventDefault();
    });
    this.element.addEventListener("drop", (event) => {
      depth = 0;
      veil.hidden = true;
      if (!event.dataTransfer?.files.length) return;
      event.preventDefault();
      tray.addFiles(event.dataTransfer.files);
    });
    this.stopDrops =
      attach.drops?.({
        over: (over) => {
          if (this.element.isConnected && !document.querySelector("dialog[open]")) veil.hidden = !over;
        },
        dropped: (paths) => {
          veil.hidden = true;
          // A sheet open over the conversation takes what is dropped on it.
          if (this.element.isConnected && !document.querySelector("dialog[open]")) tray.addPaths(paths);
        },
      }) ?? null;
  }

  private async askOffers(): Promise<void> {
    const controls = this.options.controls;
    if (!controls) return;
    try {
      this.offered = await controls.offers();
    } catch {
      this.offered = null;
    }
    this.drawChip();
    this.drawState();
    // What the conversation takes decides which of its turns' actions show.
    const can = new Set(this.offered?.controls ?? []);
    const transcript = this.element.querySelector(".conversation")!;
    transcript.classList.toggle("can-rewind", can.has("rewind"));
    // Forking from a turn is forking and then cutting the new history at
    // that turn: a host that cannot rewind (Claude) forks only the whole
    // conversation, from the command menu.
    transcript.classList.toggle("can-fork", can.has("fork") && can.has("rewind"));
  }

  /** What the conversation has used, as its host says: its context, the
   *  account's limits and when they reset, what it cost. */
  private toggleUsage(): void {
    const pop = this.element.querySelector<HTMLElement>(".usage-pop")!;
    if (!pop.hidden) {
      pop.hidden = true;
      this.context.setAttribute("aria-expanded", "false");
      return;
    }
    const words = this.options.controls?.words.usage;
    if (!words) return;
    const usage = this.offered?.usage ?? null;
    const context = this.options.conversation.context ?? usage?.context ?? null;
    const rows: string[] = [];
    if (context) {
      const share = context.window ? Math.min(1, context.used / context.window) : null;
      rows.push(
        `<div class="usage-row"><span class="usage-name">${escape(words.context)}</span><span class="usage-fact">${escape(words.contextUsed(context.used, context.window))}${share !== null ? ` · ${Math.round(share * 100)}%` : ""}</span>${share !== null ? `<span class="usage-bar"><span style="width:${(share * 100).toFixed(1)}%"></span></span>` : ""}</div>`,
      );
    }
    for (const limit of usage?.limits ?? []) {
      const percent = limit.used_percent;
      rows.push(
        `<div class="usage-row"><span class="usage-name">${escape(words.window(limit.name))}</span><span class="usage-fact">${percent !== null ? escape(words.used(String(percent))) : ""}${limit.resets_at ? ` · ${escape(words.resets(new Date(limit.resets_at)))}` : ""}</span>${percent !== null ? `<span class="usage-bar"><span style="width:${Math.min(100, Number(percent))}%"></span></span>` : ""}</div>`,
      );
    }
    if (usage?.cost_usd !== null && usage?.cost_usd !== undefined) {
      rows.push(`<div class="usage-row"><span class="usage-fact">${escape(words.cost(String(usage.cost_usd)))}</span></div>`);
    }
    pop.innerHTML = `<div class="usage-title">${escape(words.title)}</div>${rows.join("") || `<div class="menu-empty">${escape(words.none)}</div>`}`;
    pop.hidden = false;
    this.context.setAttribute("aria-expanded", "true");
  }

  /** Starting again from the user's words, or forking from a turn: said
   *  plainly first, done once the user agrees. */
  private turnAct(act: "rewind" | "fork", turn: string, words: string | null, kept: Attachment[]): void {
    const controls = this.options.controls;
    if (!controls) return;
    const said = controls.words;
    const confirm = this.element.querySelector<HTMLElement>(".composer-confirm")!;
    confirm.hidden = false;
    confirm.innerHTML = `<p>${escape(act === "rewind" ? said.rewindAsk : said.forkAsk)}</p><div class="composer-confirm-acts"><button type="button" class="ask-button ask-primary" data-go>${escape(act === "rewind" ? said.rewindGo : said.forkGo)}</button><button type="button" class="ask-button" data-never>${escape(said.never)}</button></div>`;
    const close = () => {
      confirm.hidden = true;
      confirm.innerHTML = "";
      confirm.onkeydown = null;
    };
    confirm.querySelector("[data-never]")!.addEventListener("click", close, { once: true });
    confirm.onkeydown = (event) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      close();
      this.input.focus();
    };
    confirm.querySelector<HTMLElement>("[data-go]")!.focus();
    confirm.querySelector("[data-go]")!.addEventListener(
      "click",
      () => {
        close();
        if (act === "rewind") {
          void this.run({ name: "rewind", description: "", args: null, control: "rewind", source: null }, "", { do: "rewind", from: turn }).then(() => {
            if (words !== null && !this.input.value.trim()) {
              this.input.value = words;
              // What came with the words comes back with them.
              if (kept.length && this.tray && !this.tray.count) this.tray.restore(kept);
              this.fit();
              this.drawAction();
              this.keepDraft();
              this.input.focus();
            }
          });
        } else {
          void this.run({ name: "fork", description: "", args: null, control: "fork", source: null }, "", { do: "fork", through: turn });
        }
      },
      { once: true },
    );
  }

  private drawChip(): void {
    const offered = this.offered;
    const shown = !!offered && offered.controls.includes("model");
    this.chip.hidden = !shown;
    if (!shown) return;
    const model = offered.models.find((m) => m.id === offered.model);
    this.chip.innerHTML = `<span>${escape(model?.name ?? offered.model ?? this.options.controls!.words.modelChip)}</span>${offered.effort ? `<span class="chip-effort">${escape(offered.effort)}</span>` : ""}${glyph("chevron", 14)}`;
  }

  /** Opens the command menu while the words are a command being typed. */
  private slash(): void {
    const menu = this.commandMenu;
    if (!menu || !this.options.controls) return;
    const typed = /^\/(\S*)$/.exec(this.input.value);
    if (!typed) return menu.hide();
    const prefix = typed[1]!.toLowerCase();
    const found = commandsOf(this.offered, this.options.controls.words.commands).filter((command) =>
      command.name.toLowerCase().startsWith(prefix),
    );
    menu.show(found, this.options.controls.words.commands.noCommands);
  }

  private fileMenuOf(): void {
    this.fileMenu = new Menu<FoundFile>("files", (file) => fileRow(file), (file) => {
      this.fileMenu!.hide();
      const caret = this.input.selectionStart ?? this.input.value.length;
      const at = mentionAt(this.input.value, caret);
      if (!at) return;
      const put = mentioned(file);
      this.input.value = this.input.value.slice(0, at.from) + put + this.input.value.slice(caret);
      const end = at.from + put.length;
      this.input.setSelectionRange(end, end);
      this.fit();
      this.drawAction();
      this.keepDraft();
    }, this.input);
    this.element.querySelector<HTMLElement>(".composer-menus")!.append(this.fileMenu.element);
  }

  /** Asks for files while a `@` mention is being typed. */
  private mention(): void {
    const files = this.options.files;
    const menu = this.fileMenu;
    if (!files || !menu) return;
    const at = mentionAt(this.input.value, this.input.selectionStart ?? this.input.value.length);
    if (!at) return menu.hide();
    const nowhere = files.nowhere?.() ?? null;
    if (nowhere) return menu.show([], nowhere === "outside" ? files.words.outside : files.words.nowhere);
    if (!at.query) return menu.show([], files.words.hint);
    const asked = ++this.mentionAsked;
    if (!menu.open) menu.show([], files.words.searching);
    files.find(at.query).then(
      (found) => {
        if (asked === this.mentionAsked && menu.open) menu.show(found, files.words.none);
      },
      (error: unknown) => {
        if (asked === this.mentionAsked && menu.open) menu.show([], String(error));
      },
    );
  }

  private async setModel(model: string | null, effort: string | null): Promise<void> {
    await this.run({ name: "model", description: "", args: null, control: "model", source: null }, "", { do: "model", model, effort });
  }

  /** Does a command, the World's own control or the host's; says whether
   *  it was done. */
  private async run(command: MenuCommand, args: string, given?: Control): Promise<boolean> {
    const controls = this.options.controls;
    if (!controls) return false;
    const words = controls.words;
    if (command.control === "stop") {
      this.options.stop();
      return true;
    }
    if (command.control === "model" && !given && !args) {
      this.modelMenu!.show(this.offered?.models ?? [], words.noModels);
      return false;
    }
    // What the interface already holds is shown from here, not asked for.
    if (command.source === "codex" && !given && !args) {
      if (command.name === "status" || command.name === "usage") {
        this.say("");
        if (this.element.querySelector<HTMLElement>(".usage-pop")!.hidden) this.toggleUsage();
        return true;
      }
      if (command.name === "skills") {
        this.say("");
        this.showSaid(`/${command.name}`, skillsHtml(this.offered, words.said));
        return true;
      }
    }
    const what: Control =
      given ??
      (command.control === "compact"
        ? { do: "compact" }
        : command.control === "model"
          ? { do: "model", model: args, effort: null }
          : command.control === "rename"
            ? { do: "rename", name: args }
            : command.control === "fork"
              ? { do: "fork", through: null }
              : { do: "command", name: command.name, args, ...(command.path ? { path: command.path } : {}) });
    this.say(words.running(command.name));
    try {
      const done = await controls.control(what);
      if (done.kind === "forked") {
        this.say(words.done(command.name));
        controls.forked(done.resident);
        return true;
      }
      const model = what.do === "model" ? this.offered?.models.find((m) => m.id === what.model) : undefined;
      const shown = shownOf(command.name, what.do === "model" && model ? { ...what, model: model.name ?? model.id } : what, done.said, words.said);
      if (shown?.kind === "panel") {
        this.say("");
        this.showSaid(shown.title, shown.html);
      } else this.say(shown?.text ?? words.done(command.name));
      void this.askOffers();
      return true;
    } catch (error) {
      this.say(words.failed(String(error)));
      return false;
    }
  }

  private fit(): void {
    this.input.style.height = "auto";
    this.input.style.height = `${this.input.scrollHeight}px`;
    // A scrollbar only once the words outgrow the box's tallest; below that
    // a rounding of the line height would show one for nothing.
    this.input.style.overflowY = this.input.scrollHeight > this.input.clientHeight + 1 ? "auto" : "hidden";
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

  private away: "asleep" | "removed" | null = null;

  /** Whether the World holds it asleep, or the user removed it, as the
   *  World last listed it. Removed, it is kept to read: the composer
   *  gives way to a line saying so, since words no longer reach it. */
  presence(away: "asleep" | "removed" | null): void {
    if (this.away === away) return;
    this.away = away;
    const removed = away === "removed";
    this.element.querySelector<HTMLElement>(".composer")!.hidden = removed;
    const gone = this.element.querySelector<HTMLElement>(".composer-gone")!;
    gone.hidden = !removed;
    gone.textContent = removed ? this.options.words.removedNote : "";
    this.drawState();
  }

  /** Who it is, as the World lists it: a conversation opened before then,
   *  as one just started, goes by its agent's id until this; its project
   *  and model follow it when they change. */
  identify(who: Someone): void {
    Object.assign(this.options.someone, who);
    const said = this.options.words.sendPlaceholder(who.agent);
    this.input.placeholder = said;
    this.input.setAttribute("aria-label", said);
    this.drawState();
  }

  /** The watch on the session is over: what is shown stays, and the bar
   *  says it is no longer live. */
  lost(why: string | null): void {
    this.gone = this.options.words.watchEnded(why);
    this.interrupted = false;
    this.drawState();
  }

  /** The live stream broke and waits to be read again: what is shown
   *  stays, and the bar says it is waiting. */
  broken(why: string | null): void {
    this.gone = this.options.words.watchInterrupted(why);
    this.interrupted = true;
    this.drawState();
  }

  /** The stream came back after it broke. */
  resumed(): void {
    if (!this.interrupted) return;
    this.interrupted = false;
    this.gone = null;
    this.drawState();
  }

  private interrupted = false;

  /** The state last said, and where it is said to a screen reader. */
  private said: string | null = null;
  private spoken!: HTMLElement;

  private drawState(): void {
    const { words, someone, conversation } = this.options;
    // The host's own title for the conversation, once it gives one, with
    // who and where in the line under it.
    const title = conversation.title;
    const name = title
      ? `${escape(title)}`
      : `${escape(someone.agent)}<span class="who-at"> · ${escape(someone.machine)}</span>`;
    if (!this.renaming && this.name.innerHTML !== name) this.name.innerHTML = name;
    const renamable = !!this.offered?.controls.includes("rename");
    this.name.classList.toggle("renamable", renamable);
    if (renamable) {
      this.name.tabIndex = 0;
      this.name.title = this.options.controls?.words.commands.rename ?? "";
    } else {
      this.name.removeAttribute("tabindex");
      this.name.removeAttribute("title");
    }
    const waiting = !this.gone && conversation.waiting();
    // Working for the whole of a turn, not only while something shows.
    const working = !this.gone && !waiting && conversation.busy();
    const now =
      this.gone ??
      (this.away === "removed"
        ? words.removed
        : waiting
          ? words.waitingForYou
          : working
            ? words.working
            : this.away === "asleep"
              ? words.asleep
              : words.idle);
    // A screen reader hears the conversation change state, once each time:
    // its words as they stream would be too many to hear.
    if (this.said !== null && this.said !== now) this.spoken.textContent = `${someone.agent} · ${now}`;
    this.said = now;
    const facts = [
      now,
      title ? `${someone.agent} · ${someone.machine}` : null,
      someone.project,
      someone.model,
    ].filter(Boolean) as string[];
    const mark = waiting ? `<span class="ask-mark" aria-hidden="true"></span>` : working ? `<span class="pulse" aria-hidden="true"></span>` : "";
    // Waiting for the user is said in the accent; the rest stays quiet.
    const [first, ...rest] = facts.map(escape);
    this.state.innerHTML = `${mark}<span><span class="who-first">${first}</span>${rest.map((fact) => ` · ${fact}`).join("")}</span>`;
    this.state.classList.toggle("working", working);
    this.state.classList.toggle("waiting", waiting);
    this.state.classList.toggle("gone", this.gone !== null);
    // How much of the context is used: the host's latest report, from the
    // conversation's events, or from what it offers when it says.
    const used = conversation.context ?? this.offered?.usage?.context ?? null;
    if (used && used.window) {
      const share = Math.min(1, used.used / used.window);
      const percent = Math.round(share * 100);
      const label = this.options.controls
        ? this.options.controls.words.context(percent, used.used, used.window)
        : words.context(percent);
      const arc = (2 * Math.PI * 7).toFixed(2);
      this.context.innerHTML = `<svg class="context-ring" width="18" height="18" viewBox="0 0 18 18" aria-hidden="true"><circle cx="9" cy="9" r="7"/><circle class="used" cx="9" cy="9" r="7" stroke-dasharray="${(share * 2 * Math.PI * 7).toFixed(2)} ${arc}"/></svg><span>${percent}%</span>`;
      this.context.title = label;
      this.context.setAttribute("aria-label", label);
      this.context.classList.toggle("full", share >= 0.85);
      this.context.hidden = false;
    } else {
      this.context.innerHTML = "";
      this.context.removeAttribute("title");
      this.context.removeAttribute("aria-label");
      // Without a context to show, the limits still have a way in.
      this.context.hidden = !this.offered?.usage?.limits.length;
      if (!this.context.hidden) this.context.textContent = this.options.controls?.words.usage.title ?? "";
    }
  }

  private drawAction(): void {
    const { words, conversation } = this.options;
    const typed = this.input.value.trim() !== "" || (this.tray?.count ?? 0) > 0;
    const busy = conversation.busy();
    const stop = !typed && busy;
    this.action.innerHTML = glyph(stop ? "stop" : "send", 18);
    const label = stop ? words.stop : typed && (busy || this.released !== null) ? words.queue.queue : words.send;
    this.action.setAttribute("aria-label", label);
    this.action.title = label;
    this.action.classList.toggle("stop", stop);
    this.action.disabled = !typed && !stop;
  }
}

/** A span of time as the interface says it elsewhere: minutes, hours. */
function lasted(ms: number, words: Words): string {
  const minutes = Math.max(0, Math.round(ms / 60_000));
  return minutes < 60 ? words.minutes(minutes) : words.hours(Math.floor(minutes / 60), minutes % 60);
}
