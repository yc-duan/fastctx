// A conversation's page: who it is with and what they are doing, the
// conversation itself in a region that scrolls on its own, and the words
// the user is about to send, always at the foot.

import type { ComposerWords } from "../app/words";
import type { Words } from "../words";
import type { Control, Controlled, Offers } from "../world/control";
import type { Attachment } from "../world/attachments";
import type { AttachmentSource } from "../world/files";
import type { Decoding } from "../world/remote-lines";
import type { ResidentDetail, ResidentNow } from "../world/world";
import { AttachTray, type AttachSeam, type AttachWords, type Attached } from "./attach";
import { SendQueue, type Queued } from "./queue";
import { bindComposer, type ComposerSession, type ComposerSessionState } from "./composer";
import type { ComposerUpdated } from "../world/composer";
import { NodeQueue } from "./node-queue";
import { NodeTray, type LentFiles } from "./node-tray";
import { Menu, commandFor, commandRow, commandsOf, usageOf, type MenuCommand } from "./commands";
import { ModelPanel, enModelPanel, zhModelPanel } from "./model-panel";
import { fileRow, mentionAt, mentioned, type FileWords, type FoundFile } from "./mention";
import type { Details } from "./details";
import { FileLinks } from "./file-links";
import { JobsPanel, type Job, type OutputLines } from "./jobs-panel";
import { Pictures, type PictureSeams } from "./pictures";
import { glyph } from "./glyphs";
import type { Conversation } from "./model";
import { escape, render } from "./prose";
import { shownOf, skillsHtml } from "./said";
import { Finder } from "./find";
import { usageDetail } from "./usage-detail";
import { ConversationView } from "./view";
import { departureOf } from "./flight";
import { detailOf } from "../app/problem";

export interface Someone {
  /** "Claude" or "Codex": the agent by the name it goes by. */
  agent: string;
  /** The machine it lives on. */
  machine: string;
  /** The machine its commands start on, as written, when not the one it
   *  lives on (`world-run.md`, 这里). */
  worksOn?: string | null;
  /** Who lent it, for an agent another account lent the user: it works on
   *  no machine by default, each command naming one. */
  lentBy?: string | null;
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
  /** Opening the files an agent's links name, where the World can bring
   *  them here; showing one in its folder, where this is a computer. */
  opening?: {
    open(machine: string, path: string): Promise<void>;
    openAttachment?: ((attachment: AttachmentSource) => Promise<void>) | undefined;
    reveal?: ((path: string) => Promise<void>) | undefined;
  };
  /** The conversation's pictures, from the machines holding them. */
  pictures?: PictureSeams;
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
  /** The input shared by every device with this conversation open, its
   *  words and queue kept by the node of the agent's machine; where it
   *  cannot be opened (a node of an older version), the page keeps its own. */
  composer?: () => Promise<ComposerSession>;
  /** Attaching files to the words, where the World can carry them. */
  attach?: {
    words: AttachWords;
    seam: AttachSeam;
    /** Files dropped onto the window, by path, where the shell gives
     *  them so; returns how to stop listening. */
    drops?(on: { over(over: boolean): void; dropped(paths: string[]): void }): () => void;
    /** For an agent lent to the user, which has no machine it works on:
     *  the machine of the user's its files other than images go to. */
    filesOn?: LentFiles;
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
  send(text: string, attached: { refs: unknown[]; shown: Attached[]; filesOn?: string | null }): Promise<Sent> | void;
  stop(): void;
  /** Stops one job it started, on the machine it runs on. */
  stopJob?: ((job: string, machine: string) => Promise<void>) | undefined;
  /** A job's output as lines from the machine running it, read as it
   *  grows. */
  jobOutput?: ((job: Job) => Promise<OutputLines>) | undefined;
  /** Lines of a file or a job's output on the machine holding them: what a
   *  write changed, a command's whole output. */
  lines?: ((machine: string, path: string, decoding: Decoding) => Promise<OutputLines>) | undefined;
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
  /** Why the watch ended or broke, in the kernel's words, for the hover. */
  private goneWhy = "";
  private offered: Offers | null = null;
  private wasMoving = false;
  private commandMenu: Menu<MenuCommand> | null = null;
  private modelPanel: ModelPanel | null = null;
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
        <p class="composer-gone" hidden></p>
        <div class="composer-queue"></div>
        <form class="composer">
          <div class="composer-menus"><div class="composer-menu usage-pop" hidden></div><div class="composer-menu said-pop" role="dialog" hidden></div><div class="jobs-pop" role="dialog" hidden></div></div>
          <div class="composer-confirm" hidden></div>
          <div class="composer-tray"></div>
          <textarea rows="1" enterkeyhint="send" placeholder="${escape(words.sendPlaceholder(someone.agent))}" aria-label="${escape(words.sendPlaceholder(someone.agent))}"></textarea>
          <div class="composer-row">
            <div class="composer-tools"><button class="icon-button composer-attach" type="button" hidden></button><input class="composer-pick" type="file" multiple hidden><button class="icon-button composer-photo" type="button" hidden></button><input class="composer-shot" type="file" accept="image/*" capture="environment" hidden><button class="composer-chip" type="button" hidden></button><button class="composer-jobs" type="button" hidden></button></div>
            <span class="composer-note" aria-live="polite"></span>
            <button class="composer-context" type="button" hidden></button>
            <button class="composer-action" type="submit" aria-label="${escape(words.send)}"></button>
          </div>
        </form>
      </footer>`;
    this.scroller = this.element.querySelector<HTMLElement>(".talk-scroller")!;
    const transcript = this.element.querySelector<HTMLElement>(".conversation")!;
    this.finder = new Finder(
      transcript,
      this.scroller,
      words.find,
      () => {
        this.following = false;
      },
      // Finding reads the whole conversation: every block is drawn while
      // the bar is open.
      (whole) => this.view.wholly(whole),
    );
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
    if (options.controls) this.menus();
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
    if (options.composer) void this.share(options.composer);
    const controls = options.controls;
    this.view = new ConversationView(main, options.conversation, {
      words,
      english: words.english,
      scroller: this.scroller,
      reading: () => {
        this.following = false;
      },
      // Words being sent leave from where they were typed.
      departure: () => departureOf(this.input),
      questions: {
        words: words.questions,
        // Answers go in at once, into the turn if one runs: they never wait
        // in the queue.
        answer: async (text) => {
          const came = await this.options.send(text, { refs: [], shown: [] });
          if (came === "failed") throw new Error(words.notSent(""));
        },
      },
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
      lines: options.lines,
    });
    if (options.opening) {
      new FileLinks(main, {
        words: words.files,
        here: options.here,
        machine: someone.machine,
        open: options.opening.open,
        openAttachment: options.opening.openAttachment,
        reveal: options.opening.reveal,
        say: (text) => this.say(text),
      });
    }
    if (options.pictures) {
      new Pictures(main, { words: words.pictures, here: options.here, machine: someone.machine, seams: options.pictures, say: (text) => this.say(text) });
    }

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
      for (const menu of [this.fileMenu, this.commandMenu]) {
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

  /** Brings the page up to the conversation: what does not wait for the
   *  screen at once, and the drawing on the next frame, however many
   *  changes came before it. Words streaming in at any rate are drawn at
   *  most once a frame. `read` says history came (a reading of what came
   *  before, not things happening now): what it adds is drawn as the
   *  reader comes to it, and nothing of it makes an entrance. */
  update(how: { read?: boolean } = {}): void {
    if (how.read) this.read = true;
    // The commands it offers changed (a plugin loaded, a description came).
    if (this.options.conversation.takeOffersChanged()) void this.askOffers();
    // What it offers is asked again whenever a turn ends: a command, a
    // compaction or a new model may have changed it.
    const moving = this.options.conversation.moving();
    if (this.wasMoving && !moving) void this.askOffers();
    this.wasMoving = moving;
    this.releaseQueued();
    if (!this.frame) this.frame = requestAnimationFrame(() => this.render());
  }

  private frame = 0;
  private read = false;

  /** Draws the page as the conversation now stands; keeps the newest in
   *  view when the reader was already there. A page not in view is drawn
   *  when it is shown again. */
  private render(): void {
    this.frame = 0;
    if (!this.element.isConnected) return;
    const follow = this.following;
    this.view.update(this.read);
    this.read = false;
    this.drawTurnActs();
    this.finder.refresh();
    this.drawState();
    this.drawAction();
    if (follow) this.scroller.scrollTop = this.scroller.scrollHeight;
  }

  /** Where the reader was, kept while the page is out of view: following
   *  the newest, or a block and how far down it they were. */
  private place: { key: string; offset: number } | null = null;

  /** The page goes out of view and stays alive, its conversation still
   *  followed. */
  suspend(): void {
    this.place = this.following ? null : this.view.anchor();
  }

  /** The page is shown again, as it was left and as the conversation now
   *  stands. */
  resume(): void {
    this.render();
    if (this.following) this.scroller.scrollTop = this.scroller.scrollHeight;
    else if (this.place) this.view.restore(this.place);
  }

  /** Going back to a turn or forking from it, only where the host says it
   *  can: some turns cannot be reached again (one compacted away, the very
   *  first of a conversation it cannot empty). */
  private drawTurnActs(): void {
    const checkpoints = this.offered?.checkpoints;
    for (const button of this.element.querySelectorAll<HTMLElement>("[data-turn-act][data-turn]")) {
      const act = button.dataset.turnAct === "fork" ? "fork" : "rewind";
      const point = checkpoints?.find((one) => one.turn === button.dataset.turn);
      button.hidden = !!checkpoints && !point?.[act];
    }
  }

  /** Stops what the page keeps asking for. */
  leave(): void {
    clearTimeout(this.looking);
    cancelAnimationFrame(this.frame);
    this.view.destroy();
    this.looking = -1;
    this.left = true;
    document.removeEventListener("keydown", this.findKey);
    this.stopDrops?.();
    this.unbindShared?.();
    void this.session?.close();
  }

  private left = false;
  /** The node's shared input, once open. */
  private session: ComposerSession | null = null;
  private sharedState: ComposerSessionState | null = null;
  private unbindShared: (() => void) | null = null;
  private nodeQueue: NodeQueue | null = null;
  private nodeTray: NodeTray | null = null;
  /** This device's last send of the shared words, until the node answers. */
  private sendingShared: Promise<unknown> = Promise.resolve();

  /** Opens the shared input: the words typed here before it opened go into
   *  it, the queue and what is attached become the node's. */
  private async share(open: () => Promise<ComposerSession>): Promise<void> {
    let session: ComposerSession;
    try {
      session = await open();
    } catch (error) {
      console.warn("the shared input could not be opened; this page keeps its own", error);
      return;
    }
    if (this.left) {
      void session.close();
      return;
    }
    this.session = session;
    const typed = this.input.value;
    const caret = this.input.selectionStart ?? typed.length;
    try {
      if (this.options.draftKey) localStorage.removeItem(this.options.draftKey);
    } catch {
      // Nothing was kept here.
    }
    this.unbindShared = bindComposer(this.input, session.draft);
    if (typed.trim() && !this.input.value.trim()) this.setWords(typed, caret);
    const say = (why: string) => this.say(why, why, true);
    const lentFiles = this.options.attach?.filesOn ?? null;
    this.nodeQueue = new NodeQueue(this.options.words.queue, this.options.attach?.words ?? null, session, say, lentFiles, (text, attachments, sending) => {
      const shown = attachments.map((one) => ({ name: one.name, size: one.size, image: one.media_type.startsWith("image/"), preview: null }));
      const key = this.options.conversation.sending(text, this.options.here || null, shown);
      this.update();
      this.follow(key, sending);
    });
    this.queue.element.replaceWith(this.nodeQueue.element);
    if (this.options.attach && this.tray) {
      this.nodeTray = new NodeTray(this.options.attach.words, session, session.draft, say, lentFiles);
      this.tray.element.replaceWith(this.nodeTray.element);
    }
    session.subscribe((state) => {
      this.sharedState = state;
      this.nodeQueue?.draw(state);
      this.nodeTray?.draw(state);
      this.fit();
      this.drawAction();
    });
  }

  /** Puts words into the input as if typed, so the shared input has them. */
  private setWords(value: string, caret = value.length): void {
    this.input.value = value;
    this.input.setSelectionRange(caret, caret);
    this.input.dispatchEvent(new Event("input", { bubbles: true }));
  }

  private attachFiles(files: Iterable<File>): void {
    if (this.nodeTray) this.nodeTray.addFiles(files);
    else this.tray?.addFiles(files);
  }

  private attachPaths(paths: string[]): void {
    if (this.nodeTray) this.nodeTray.addPaths(paths);
    else this.tray?.addPaths(paths);
  }

  /** Sends the shared words: the node sends them now, or queues them while
   *  a turn is under way. What goes now is shown at once. */
  private sendShared(session: ComposerSession, text: string): void {
    const { conversation, here } = this.options;
    const draft = this.sharedState?.composer;
    const queued = conversation.busy() || !!draft?.queue.length || !!draft?.awaiting_turn;
    const shown = (draft?.draft.attachments ?? []).map((one) => ({ name: one.name, size: one.size, image: one.media_type.startsWith("image/"), preview: null }));
    const key = queued ? null : conversation.sending(text, here || null, shown);
    this.update();
    // Words sent here while the last ones are still on their way go once
    // the node has those, as the next draft rather than the one already sent.
    // Nothing is sent when those took everything the input held.
    const sent = this.sendingShared.then(() => {
      const draft = session.draft;
      const left = draft.view.characters.some((one) => /\S/u.test(one.text)) || draft.attachments.length > 0 || draft.uploads.length > 0;
      return left ? session.send() : null;
    });
    this.sendingShared = sent.catch(() => {});
    this.follow(key, sent);
  }

  /** Words shown below the turn's line while on their way: where they
   *  landed, or that they failed, as the node answers. */
  private follow(key: string | null, sending: Promise<ComposerUpdated | null>): void {
    const { conversation } = this.options;
    sending.then(
      (updated) => {
        if (!key) return;
        if (!updated) {
          conversation.unsend(key);
          this.update();
          return;
        }
        const landed = updated.landed;
        if (!landed) conversation.unsend(key);
        else if (landed.kind === "waiting" || landed.kind === "sent" || landed.kind === "uncertain") {
          conversation.sent(key, { state: "landed", how: landed.kind, machine: this.options.someone.machine, why: "why" in landed ? landed.why : null });
        }
        this.update();
      },
      (error: unknown) => {
        const why = error instanceof Error ? error.message : String(error);
        if (key) conversation.sent(key, { state: "failed", why });
        else this.say(this.options.words.notSent(why), why, true);
        this.update();
      },
    );
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
    if (!this.session && this.tray?.sending) {
      this.say(this.options.attach!.words.wait);
      return;
    }
    const session = this.session;
    const files = session ? (this.nodeTray?.files ?? 0) : (this.tray?.files ?? 0);
    const lent = this.options.attach?.filesOn;
    const placing = !!lent && files > 0 && lent.lent();
    const filesOn = !placing ? null : this.nodeTray ? this.nodeTray.chosen : lent!.place();
    if (placing && !filesOn) {
      this.say(lent!.nowhere);
      return;
    }
    const attached = !session && this.tray?.count ? { ...this.tray.take(), filesOn } : { refs: [], shown: [], filesOn: null };
    if (!text && !attached.refs.length && !(session && this.nodeTray?.count)) {
      if (this.options.conversation.moving()) this.options.stop();
      return;
    }
    if (this.options.controls && text.startsWith("/")) {
      const space = text.indexOf(" ");
      const name = (space < 0 ? text.slice(1) : text.slice(1, space)).trim();
      const args = space < 0 ? "" : text.slice(space + 1).trim();
      const command = commandFor(commandsOf(this.offered, this.options.controls.words.commands), name, args);
      if (command?.arguments === "required" && !args) {
        this.say(this.options.controls.words.commands.needsArgs(usageOf(command)));
        return;
      }
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
    // What the last command did is behind the conversation now.
    this.say("");
    this.hideSaid();
    this.following = true;
    if (session) {
      this.commandMenu?.hide();
      this.nodeTray?.settle();
      this.sendShared(session, text);
      return;
    }
    this.clear();
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
    // The node releases what waits in a shared input.
    if (this.session) return;
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
    if (this.session) this.setWords("");
    else this.input.value = "";
    this.fit();
    this.keepDraft();
    this.commandMenu?.hide();
    this.drawAction();
  }

  /** A line beside the composer: what a command did, or that it did not,
   *  with the host's own words on hover. */
  private say(text: string, detail = "", refused = false): void {
    this.note.textContent = text;
    this.note.title = detail;
    this.note.classList.toggle("refused", refused);
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
    if (!this.options.draftKey || this.session) return;
    try {
      if (this.input.value) localStorage.setItem(this.options.draftKey, this.input.value);
      else localStorage.removeItem(this.options.draftKey);
    } catch {
      // Not kept: the words are lost if the page is left.
    }
  }

  private menus(): void {
    const holder = this.element.querySelector<HTMLElement>(".composer-menus")!;
    this.commandMenu = new Menu<MenuCommand>("commands", (command) => commandRow(command), (command) => {
      this.commandMenu!.hide();
      if (command.arguments === "required" || (command.arguments === "optional" && !command.control)) {
        if (this.session) this.setWords(`/${command.name} `);
        else this.input.value = `/${command.name} `;
        this.input.focus();
        this.drawAction();
        if (command.args) this.say(usageOf(command));
      } else {
        this.clear();
        void this.run(command, "");
      }
    }, this.input);
    holder.append(this.commandMenu.element);
    this.modelPanel = new ModelPanel(this.chip, this.options.words.english ? enModelPanel : zhModelPanel, this.options.words.english, {
      model: (model, effort) => this.setModel(model, effort),
      tier: async (id) => {
        await this.run({ name: "tier", description: "", args: null, arguments: "optional", control: null, source: null }, id, { do: "command", name: "tier", args: id });
      },
    });
    holder.append(this.modelPanel.element);
    void this.askOffers();
  }

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
        void this.run({ name: "rename", description: "", args: null, arguments: "none", control: "rename", source: null }, "", { do: "rename", name }).then((done) => {
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
      if (pick.files) this.attachFiles(pick.files);
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
        if (shot.files) this.attachFiles(shot.files);
        shot.value = "";
        this.input.focus();
      });
    }
    this.input.addEventListener("paste", (event) => {
      const files = [...(event.clipboardData?.files ?? [])];
      if (!files.length) return;
      // Pasted files are attached; pasted words still go into the words.
      if (!event.clipboardData?.getData("text/plain")) event.preventDefault();
      this.attachFiles(files);
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
      this.attachFiles(event.dataTransfer.files);
    });
    this.stopDrops =
      attach.drops?.({
        over: (over) => {
          if (this.element.isConnected && !document.querySelector("dialog[open]")) veil.hidden = !over;
        },
        dropped: (paths) => {
          veil.hidden = true;
          // A sheet open over the conversation takes what is dropped on it.
          if (this.element.isConnected && !document.querySelector("dialog[open]")) this.attachPaths(paths);
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
    this.drawTurnActs();
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
    const detail = usageDetail(usage?.raw);
    const rows: string[] = [];
    if (context) {
      const share = context.window ? Math.min(1, context.used / context.window) : null;
      // Where the host breaks the context down, the bar is drawn in its
      // parts, each a shade of the same ink, named beneath.
      const window = context.window ?? 0;
      const parts = window ? detail.parts : [];
      const tone = (at: number) => Math.round(100 - (at * 75) / Math.max(1, parts.length - 1));
      const bar = parts.length
        ? `<span class="usage-bar usage-stack">${parts.map((part, at) => `<span style="width:${Math.min(100, (part.tokens / window) * 100).toFixed(2)}%;--tone:${tone(at)}%"></span>`).join("")}</span><span class="usage-parts">${parts.map((part, at) => `<span class="usage-part"><i style="--tone:${tone(at)}%"></i><span>${escape(words.part(part.name))}</span><span>${part.tokens.toLocaleString()}</span></span>`).join("")}</span>`
        : share !== null
          ? `<span class="usage-bar"><span style="width:${(share * 100).toFixed(1)}%"></span></span>`
          : "";
      rows.push(
        `<div class="usage-row"><span class="usage-name">${escape(words.context)}</span><span class="usage-fact">${escape(words.contextUsed(context.used, context.window))}${share !== null ? ` · ${Math.round(share * 100)}%` : ""}</span>${bar}</div>`,
      );
    }
    if (detail.last) {
      const { input, cached, output, reasoning } = detail.last;
      rows.push(`<div class="usage-row"><span class="usage-name">${escape(words.lastCall)}</span><span class="usage-fact">${escape(words.last(input, cached, output, reasoning))}</span></div>`);
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
  private turnAct(act: "rewind" | "regenerate" | "fork", turn: string, words: string | null, kept: Attachment[]): void {
    const controls = this.options.controls;
    if (!controls) return;
    const said = controls.words;
    const confirm = this.element.querySelector<HTMLElement>(".composer-confirm")!;
    confirm.hidden = false;
    const ask = act === "rewind" ? said.rewindAsk : act === "regenerate" ? said.regenerateAsk : said.forkAsk;
    const go = act === "rewind" ? said.rewindGo : act === "regenerate" ? said.regenerateGo : said.forkGo;
    confirm.innerHTML = `<p>${escape(ask)}</p><div class="composer-confirm-acts"><button type="button" class="ask-button ask-primary" data-go>${escape(go)}</button><button type="button" class="ask-button" data-never>${escape(said.never)}</button></div>`;
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
        if (act === "rewind" || act === "regenerate") {
          void this.run({ name: "rewind", description: "", args: null, arguments: "none", control: "rewind", source: null }, "", { do: "rewind", from: turn }).then((rewound) => {
            // Not back before that turn: nothing is sent again or put back,
            // and the line beside the composer says why.
            if (!rewound) return;
            // Asking again sends the same words, with what came with them,
            // as soon as the conversation is back before that turn.
            if (act === "regenerate" && words !== null) {
              this.following = true;
              void this.options.send(words, kept.length ? { refs: kept, shown: [] } : { refs: [], shown: [] });
              return;
            }
            if (words !== null && !this.input.value.trim()) {
              if (this.session) this.setWords(words);
              else this.input.value = words;
              // What came with the words comes back with them.
              if (this.session && kept.length && !this.nodeTray?.count) {
                for (const attachment of kept) void this.session.attach(attachment as Attachment).catch((error: unknown) => this.say(String(error)));
              } else if (kept.length && this.tray && !this.tray.count) this.tray.restore(kept);
              this.fit();
              this.drawAction();
              this.keepDraft();
              this.input.focus();
            }
          });
        } else {
          void this.run({ name: "fork", description: "", args: null, arguments: "none", control: "fork", source: null }, "", { do: "fork", through: turn });
        }
      },
      { once: true },
    );
  }

  private drawChip(): void {
    this.modelPanel?.update(this.offered);
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
      const value = this.input.value.slice(0, at.from) + put + this.input.value.slice(caret);
      const end = at.from + put.length;
      if (this.session) this.setWords(value, end);
      else {
        this.input.value = value;
        this.input.setSelectionRange(end, end);
      }
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
    await this.run({ name: "model", description: "", args: null, arguments: "optional", control: "model", source: null }, "", { do: "model", model, effort });
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
      this.modelPanel?.show(true);
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
      // The fork exists but not all of it came over: it opens, and why is said.
      if (done.kind === "fork_incomplete") {
        this.say(words.forkIncomplete, done.why, true);
        controls.forked(done.resident);
        return true;
      }
      if (done.kind === "side_answer") {
        const reply = done.reply !== null && typeof done.reply === "object" && !Array.isArray(done.reply) ? done.reply : {};
        const text = typeof reply.text === "string" ? reply.text : JSON.stringify(done.reply, null, 2);
        this.say("");
        this.showSaid(`/${command.name}`, `<div class="prose">${render(text)}</div>`);
        return true;
      }
      if (done.kind === "agents" || done.kind === "agent_messages") return true;
      if (done.kind === "model_menu") {
        this.say("");
        if (this.offered) this.offered = { ...this.offered, models: done.models, model: done.model, effort: done.effort, service_tier: done.service_tier };
        this.drawChip();
        this.modelPanel?.show(true);
        return true;
      }
      if (done.kind === "refused") {
        this.say(done.current_model ? words.refusedOn(done.said, done.current_model) : done.said, "", true);
        void this.askOffers();
        return false;
      }
      if (done.kind === "reported") {
        const shown = done.said === null ? null : shownOf(command.name, what, done.said, words.said);
        if (shown?.kind === "panel") {
          this.say("");
          this.showSaid(shown.title, shown.html);
        } else this.say(shown?.text || words.reported(command.name));
        void this.askOffers();
        return true;
      }
      const model = what.do === "model" ? this.offered?.models.find((m) => m.id === what.model) : undefined;
      const said = done.kind === "done" ? done.said : null;
      const shown = shownOf(command.name, what.do === "model" && model ? { ...what, model: model.name ?? model.id } : what, said, words.said);
      if (shown?.kind === "panel") {
        this.say("");
        this.showSaid(shown.title, shown.html);
      } else this.say(shown?.text ?? words.done(command.name));
      void this.askOffers();
      return true;
    } catch (error) {
      this.say(words.failed(detailOf(error)), detailOf(error), true);
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

  private jobs: JobsPanel | null = null;

  /** What the resident started that still runs (`jobs-panel.ts`). */
  running(jobs: Job[], now: Date): void {
    const { words } = this.options;
    this.jobs ??= new JobsPanel(this.element.querySelector<HTMLButtonElement>(".composer-jobs")!, this.element.querySelector<HTMLElement>(".jobs-pop")!, {
      words: {
        title: words.stillRunning,
        stop: words.stop,
        stopAsk: words.stopJobAsk,
        notStopped: words.notStopped,
        cancel: this.options.controls?.words.never ?? words.cancel,
        output: words.jobOutput,
        loading: words.jobOutputLoading,
        failed: words.jobOutputFailed,
        ended: words.jobOver,
        lasted: (ms) => lasted(ms, words),
      },
      stop: this.options.stopJob,
      output: this.options.jobOutput,
      detail: detailOf,
    });
    this.jobs.update(jobs, now);
  }

  private away: "removed" | null = null;

  /** Whether the World holds it asleep, or the user removed it, as the
   *  World last listed it. Removed, it is kept to read: the composer
   *  gives way to a line saying so, since words no longer reach it. */
  presence(away: "removed" | null): void {
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
    this.goneWhy = why ?? "";
    this.interrupted = false;
    this.drawState();
  }

  /** The live stream broke and waits to be read again: what is shown
   *  stays, and the bar says it is waiting. */
  broken(why: string | null): void {
    this.gone = this.options.words.watchInterrupted(why);
    this.goneWhy = why ?? "";
    this.interrupted = true;
    this.drawState();
  }

  /** The stream came back after it broke. */
  resumed(): void {
    if (!this.interrupted) return;
    this.interrupted = false;
    this.gone = null;
    this.goneWhy = "";
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
            : words.idle);
    // A screen reader hears the conversation change state, once each time:
    // its words as they stream would be too many to hear.
    if (this.said !== null && this.said !== now) this.spoken.textContent = `${someone.agent} · ${now}`;
    this.said = now;
    // Under the title, where it lives and, when elsewhere, where it works.
    const facts = [
      now,
      title ? `${someone.agent} · ${someone.machine}` : null,
      someone.lentBy ? words.lentBy(someone.lentBy) : someone.worksOn ? words.worksOn(someone.worksOn) : null,
      someone.project,
      this.options.conversation.answeredBy ?? someone.model,
    ].filter(Boolean) as string[];
    const mark = waiting ? `<span class="ask-mark" aria-hidden="true"></span>` : working ? `<span class="pulse" aria-hidden="true"></span>` : "";
    // Waiting for the user is said in the accent; the rest stays quiet.
    const [first, ...rest] = facts.map(escape);
    this.state.innerHTML = `${mark}<span><span class="who-first">${first}</span>${rest.map((fact) => ` · ${fact}`).join("")}</span>`;
    this.state.classList.toggle("working", working);
    this.state.classList.toggle("waiting", waiting);
    this.state.classList.toggle("gone", this.gone !== null);
    if (this.goneWhy) this.state.title = this.goneWhy;
    else this.state.removeAttribute("title");
    // How much of the context is used: the host's latest report, from the
    // conversation's events, or from what it offers when it says.
    const used = conversation.context ?? this.offered?.usage?.context ?? null;
    if (used && used.window) {
      const share = Math.min(1, used.used / used.window);
      const percent = Math.round(share * 100);
      const label = this.options.controls
        ? this.options.controls.words.context(percent, used.used, used.window)
        : words.context(percent);
      this.context.innerHTML = `<svg class="context-ring" width="18" height="18" viewBox="0 0 18 18" aria-hidden="true"><circle cx="9" cy="9" r="7.4"/>${wedge(share)}</svg><span>${percent}%</span>`;
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
    const typed = this.input.value.trim() !== "" || ((this.session ? this.nodeTray?.count : this.tray?.count) ?? 0) > 0;
    const busy = conversation.busy();
    const stop = !typed && busy;
    this.action.innerHTML = glyph(stop ? "stop" : "send", 18);
    const waits = busy || this.released !== null || !!this.sharedState?.composer.queue.length;
    const label = stop ? words.stop : typed && waits ? words.queue.queue : words.send;
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

/** The context used, as a filled wedge inside the ring from twelve o'clock
 *  round: a gauge read at a glance, never taken for something loading. */
function wedge(share: number): string {
  const r = 5;
  if (share <= 0) return "";
  if (share >= 0.999) return `<circle class="used" cx="9" cy="9" r="${r}"/>`;
  const angle = share * 2 * Math.PI;
  const x = (9 + r * Math.sin(angle)).toFixed(2);
  const y = (9 - r * Math.cos(angle)).toFixed(2);
  return `<path class="used" d="M9 9V${9 - r}A${r} ${r} 0 ${share > 0.5 ? 1 : 0} 1 ${x} ${y}Z"/>`;
}
