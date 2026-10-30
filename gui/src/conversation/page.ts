// A conversation's page: who it is with and what they are doing, the
// conversation itself in a region that scrolls on its own, and the words
// the user is about to send, always at the foot.

import { leave, leaving, stay } from "../app/leave";
import { keepChoice } from "../app/model-memory";
import { layerOpened } from "../app/back";
import { FocusPanel, type FocusServers, focusSummary, type FocusPlace } from "../app/focus-panel";
import type { ComposerWords, FocusWords } from "../app/words";
import { sameFocus, type Focus, type Folders } from "../world/focus";
import type { Words } from "../words";
import type { Control, Controlled, Offers } from "../world/control";
import type { Attachment } from "../world/attachments";
import type { AttachmentSource } from "../world/files";
import type { Decoding } from "../world/remote-lines";
import { AttachTray, type AttachSeam, type AttachWords, type Attached } from "./attach";
import { wireAttach } from "./attach-way";
import { SendQueue, type Queued } from "./queue";
import { bindComposer, Withdrawn, type ComposerSession, type ComposerSessionState } from "./composer";
import type { ComposerSnapshot, ComposerUpdated } from "../world/composer";
import { NodeQueue } from "./node-queue";
import { ControlQueue, controlName } from "./control-queue";
import type { PendingControl } from "../world/control";
import { NodeTray, type AssistantFiles } from "./node-tray";
import { Menu, commandFor, commandRow, commandsOf, usageOf, type MenuCommand } from "./commands";
import { ModelPanel, enModelPanel, zhModelPanel } from "./model-panel";
import { effortLabel } from "./effort-slider";
import { fileRow, mentionAt, mentioned, type FileWords, type FoundFile } from "./mention";
import { FileLinks } from "./file-links";
import { JobsPanel, type Job, type OutputLines } from "./jobs-panel";
import { AgentBox } from "./agent-box";
import { SideParts } from "../side/parts";
import type { Address } from "../world/wire";
import type { SideSeams } from "../side/seams";
import type { ForkFrom } from "./fork-card";
import type { AgentActivity, MachineName, Tool } from "../world/events";
import { Pictures, type PictureSeams } from "./pictures";
import type { HeldRead } from "../world/reads";
import { glyph } from "./glyphs";
import type { Conversation, Held } from "./conversation";
import type { Entry, Gap } from "./model";
import { escape, render } from "./prose";
import { shownOf, skillsHtml } from "./said";
import { Finder } from "./find";
import { FarFind } from "./find-far";
import { UsageBadge, ring } from "./usage-badge";
import { UsagePop } from "./usage-pop";
import { ContextSheet } from "./context-sheet";
import type { CallSpan } from "./context-steps";
import { headline } from "./tool";
import { ConversationView, modelName } from "./view";
import { departureOf } from "./flight";
import { detailOf } from "../app/problem";
import { standsUnder } from "./landing";
import { DraftFit } from "./fit-draft";
import { Popover } from "./popover";
import { Steady } from "../app/steady";
import { composing } from "../typing";

/** How many events the page holds at most, and how many its latest part
 *  holds before what comes before its newest turns is let go, down to how
 *  many (`ConversationPage.bound`). */
const HELD_MOST = 6000;
/** How long a choice cleared of every machine can be taken back, as long as
 *  a change on the machines page can. */
const FOCUS_UNDO_MS = 6_000;
/** How many screens from what is in view a part may be and still be held,
 *  whatever the page holds: the reader goes on into it. */
const KEEP_SCREENS = 3;
const LATEST_MOST = 2400;
const LATEST_KEPT = 800;

/** The mark of an agent named "Claude" or "Codex". */
const markOf = (agent: string) => (/claude/i.test(agent) ? "claude" : "codex");

export interface Someone {
  /** "Claude" or "Codex": the agent by the name it goes by. */
  agent: string;
  /** The machine it lives on. */
  machine: string;
  /** Whose agent it is, for one of another account assisting the user: it
   *  works on no machine by default, each command naming one. */
  assistFrom?: string | null;
  /** The machines and folders it is pointed at; null for a conversation
   *  the user cannot point, one they did not open from here. */
  focus?: Focus | null;
  model: string | null;
  /** The project it is in, by the name the user gave it. */
  project: string | null;
  /** Whether the World started it, and so brings its host back itself:
   *  words to it never wait for it to come back. */
  heldByWorld?: boolean;
}

/** What came of words sent: a turn this conversation will see end; taken,
 *  but into a turn whose end it will not see (delivery uncertain, or a host
 *  that tells the World nothing of its turns); or not taken. */
export type Sent = "turn" | "unseen" | "failed";

/** A phone's width, where the conversation's measure is its dial alone. */
const phoneWide = matchMedia("(max-width: 480px)");

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
    open(machine: string, path: string, held: HeldRead): Promise<void>;
    openAttachment?: ((attachment: AttachmentSource, held: HeldRead) => Promise<void>) | undefined;
    reveal?: ((path: string) => Promise<void>) | undefined;
  };
  /** The conversation's pictures, from the machines holding them. */
  pictures?: PictureSeams;
  conversation: Conversation;
  /** Pointing it at machines and folders, from the strip at the top. */
  focus?: {
    words: FocusWords;
    places(): FocusPlace[];
    folders(machine: string, path: string | null): Promise<Folders>;
    /** Resolves with what was not done of it, in the user's words. */
    set(focus: Focus): Promise<string[]>;
    /** The account's MCP servers and their switches, where it keeps any. */
    servers?: FocusServers | undefined;
  };
  /** What the user does to the conversation, from the "⋯" in its head
   *  (`app/talk-menu.ts`): opening that menu, and bringing it back when it
   *  is archived. */
  manage?: {
    words: { more: string; archived: string; restore: string; deleted: string };
    open(anchor: HTMLElement): void;
    restore(): void;
  };
  /** What the conversation offers, and doing it: its commands, its
   *  model, its usage. */
  controls?: {
    words: ComposerWords;
    offers(): Promise<Offers>;
    control(what: Control): Promise<Controlled>;
    /** A fork is asked for, from a turn or from where the conversation is
    *  now: its place opens beside it at once. */
    forking?(through: string | null): void;
    /** It waits for the turn under way to end. */
    forkQueued?(): void;
    /** None was made; the line beside the input says why. */
    forkFailed?(): void;
    /** A fork was made: where it lives. */
    forked(resident: { resident: string; machine: string }): void;
    /** Opens a link in the system's browser. */
    open(url: string): Promise<void>;
  };
  /** Where the words being written are kept between visits. */
  draftKey?: string;
  /** Where this device keeps which view the conversation was last read
   *  in (`world-voice.md`, 两种看法). */
  viewKey?: string;
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
    /** For an agent assisting the user, which has no machine it works on:
     *  the machine of the user's its files other than images go to. */
    filesOn?: AssistantFiles;
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
  /** Asks the agent to stop; resolves whether a turn was running to stop. */
  stop(): Promise<boolean> | void;
  /** Stops one job it started, on the machine it runs on. */
  stopJob?: ((job: string, machine: string) => Promise<void>) | undefined;
  /** A job's output as lines from the machine running it, read as it
   *  grows. */
  jobOutput?: ((job: Job) => Promise<OutputLines>) | undefined;
  /** The address of a watcher's own page, for its row among the jobs. */
  watchLink?: ((job: Job) => string) | undefined;
  /** Lines of a file or a job's output on the machine holding them: what a
   *  write changed, a command's whole output. */
  lines?: ((machine: string, path: string, decoding: Decoding) => Promise<OutputLines>) | undefined;
  /** One of the host's own agents' own record, read from the
   *  conversation's machine as its box opens. */
  agentRecord?: ((agent: string) => Promise<AgentActivity>) | undefined;
  /** A turn's requests one by one, on its login's usage page. */
  calls?: ((turn: string) => void) | undefined;
  /** The panel beside the conversation: its plan, its goal, and who works
   *  with it (`side/parts.ts`). */
  side?: SideSeams | undefined;
  /** How the list names another conversation, and its family. */
  named?: ((address: Address) => { name: string; agent: "claude" | "codex" }) | undefined;
  /** What the list calls this conversation, where it calls it anything:
   *  its head says so until its host gives a title of its own. */
  listedTitle?: (() => string | null) | undefined;
  /** Goes to the conversation a fork came from, at the turn it was made
   *  from, as the card at the fork's head asks. */
  toSource?: ((address: Address, turn: string | null) => void) | undefined;
}

/** How many times what a conversation offers is asked again after asking
 *  failed, before only a turn's end asks again. */
const OFFERS_TRIES = 5;

/** How long after the reader's own wheel, finger, key or scroll bar the
 *  view is theirs: nothing keeps it at the newest meanwhile. */
const READING_MS = 150;

/** How far below the top of the view words just sent are put, the answer
 *  coming in beneath them. */
const SENT_AT = 24;

/** How long after a send the words sent are looked for to be put at the
 *  top: words that wait in a queue instead are not, coming later. */
const SENT_WITHIN_MS = 4000;

/** Whether something between `target` and `region` scrolls up by itself
 *  before the region would. */
function scrollsUpWithin(target: Element | null, region: HTMLElement): boolean {
  for (let at = target; at && at !== region; at = at.parentElement) {
    if (at.scrollTop > 0 && at.scrollHeight > at.clientHeight && /(auto|scroll)/.test(getComputedStyle(at).overflowY)) return true;
  }
  return false;
}

export class ConversationPage {
  readonly element: HTMLElement;
  private view: ConversationView;
  private state: HTMLElement;
  private context: HTMLElement;
  /** The conversation's measure in the input's row, and what opens from
   *  it; where the host offers no controls, the context's share alone. */
  private badge: UsageBadge | null = null;
  private usagePop: UsagePop | null = null;
  /** Its button opens it and closes it; a press outside closes it, as the
   *  model's panel does; it keeps the size it opened at. */
  private usagePopover: Popover | null = null;
  private usageSteady: Steady | null = null;
  private steps: ContextSheet | null = null;
  /** What the open pop-up was last drawn from. */
  private usageDrawn = "";
  private input: HTMLTextAreaElement;
  private fitter: DraftFit;
  private action: HTMLButtonElement;
  private scroller: HTMLElement;
  private name: HTMLElement;
  /** Whether the view keeps the newest in sight as it comes. Only when the
   *  conversation opens, until the reader moves the view, and after they
   *  ask for the newest: nothing else moves what they look at. */
  private following = true;
  /** Room under the newest, so words just sent can stand at the top of the
   *  view with the answer coming in beneath them; the answer takes it up
   *  as it grows, and it goes once the reader has scrolled away from it. */
  private room: HTMLElement;
  /** The words sent the room is kept for. */
  private roomFor: string | null = null;
  /** Until when words the user just sent are to be put at the top. */
  private placeUntil = 0;
  /** A stop was asked for and the turn has not ended yet. */
  private stopping = false;
  /** Words of the shared queue on their way into the turn, by queue item,
   *  as the conversation shows them meanwhile. */
  private readonly onTheirWay = new Map<string, string>();
  /** Where the words heard reached as each queue item was first seen
   *  here, null for one first seen already on its way; and the items that
   *  have left the queue, each shown once at most. */
  private readonly joinedAt = new Map<string, number | null>();
  private readonly departed = new Set<string>();
  private gone: string | null = null;
  /** Why the watch ended or broke, in the kernel's words, for the hover. */
  private goneWhy = "";
  private offered: Offers | null = null;
  /** The next asking of what the conversation offers, after one that
   *  failed, and how many failed in a row. */
  private offersAgain = -1;
  private offersFailed = 0;
  private wasMoving = false;
  private commandMenu: Menu<MenuCommand> | null = null;
  private modelPanel: ModelPanel | null = null;
  private fileMenu: Menu<FoundFile> | null = null;
  /** The latest `@` question; an answer to an earlier one is not shown. */
  private mentionAsked = 0;
  private note: HTMLElement;
  /** What the note says on hover, worked out when the pointer comes to it,
   *  for a line that says how long something has been so. */
  private noteDetail: (() => string) | null = null;
  /** The links to files and the pictures, whose reads go with the page. */
  private fileLinks: FileLinks | null = null;
  private pictures: Pictures | null = null;
  private chip: HTMLButtonElement;
  private finder: Finder;
  private far: FarFind;
  /** Ctrl+F finds in the conversation while its page is shown and in
   *  focus on the desk. */
  private readonly findKey = (event: KeyboardEvent) => {
    if (!this.element.isConnected || !this.active || !(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "f" || event.altKey) return;
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
        <h1 class="who-name">${escape(someone.agent)}</h1>
        ${someone.assistFrom ? `<span class="who-home">${glyph("machine", 14)}<span>${escape(words.assistFrom(someone.assistFrom))}</span></span>` : ""}
        <span class="who-state" hidden></span>
      </div>`,
        `<button class="icon-button view-steps" type="button" aria-pressed="false" aria-label="${escape(words.viewSteps)}" title="${escape(words.viewSteps)}">${glyph("steps", 18)}</button><button class="icon-button find-toggle" type="button" aria-label="${escape(words.find.find)}" title="${escape(words.find.find)}">${glyph("search", 18)}</button>${
          options.manage
            ? `<button class="icon-button talk-menu-toggle" type="button" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(options.manage.words.more)}">${glyph("more", 20)}</button>`
            : ""
        }`,
      )}
      <div class="talk-scroller scroller"><div class="conversation"></div><div class="talk-room" aria-hidden="true"></div></div>
      <footer class="composer-dock">
        <button class="to-latest" type="button" hidden aria-label="${escape(words.toLatest)}" title="${escape(words.toLatest)}">${glyph("chevron", 18)}</button>
        <div class="steps-back" hidden><button class="steps-back-go" type="button">${glyph("chevron", 14)}<span></span></button><button class="steps-back-x" type="button">${glyph("close", 12)}</button></div>
        <p class="composer-gone" hidden></p>
        <div class="composer-queue"></div>
        <form class="composer">
          <div class="composer-menus"><div class="composer-menu usage-pop" hidden></div><div class="composer-menu said-pop" role="dialog" hidden></div><div class="jobs-pop" role="dialog" hidden></div></div>
          <div class="composer-confirm" hidden></div>
          <div class="composer-tray"></div>
          <textarea rows="1" enterkeyhint="send" placeholder="${escape(words.sendPlaceholder(someone.agent))}" aria-label="${escape(words.sendPlaceholder(someone.agent))}"></textarea>
          <div class="composer-row">
            <div class="composer-tools"><button class="icon-button composer-attach" type="button" hidden></button><input class="composer-pick" type="file" multiple hidden><input class="composer-shot" type="file" accept="image/*" capture="environment" hidden><button class="composer-chip" type="button" hidden></button><button class="composer-jobs" type="button" hidden></button></div>
            <span class="composer-note" aria-live="polite"></span>
            <button class="composer-context" type="button" hidden></button>
            <button class="composer-action" type="submit" aria-label="${escape(words.send)}"></button>
          </div>
        </form>
      </footer>`;
    this.scroller = this.element.querySelector<HTMLElement>(".talk-scroller")!;
    this.room = this.element.querySelector<HTMLElement>(".talk-room")!;
    const transcript = this.element.querySelector<HTMLElement>(".conversation")!;
    this.finder = new Finder(
      transcript,
      this.scroller,
      words.find,
      () => {
        this.following = false;
      },
      {
        findChanges: () => this.view.findChanges(),
        revealFind: (match) => this.view.revealFind(match),
        findRange: (match) => this.view.findRange(match),
        // What the page does not hold is found by the shell, from text the
        // page reads aside once (`find-far.ts`).
        far: {
          search: (query) => this.far.search(query),
          get more() {
            return far().more;
          },
          get grown() {
            return far().grown;
          },
          held: () => options.conversation.bounds().join(";"),
          reach: async (at) => {
            await options.conversation.reach(at);
            cancelAnimationFrame(this.frame);
            this.render();
          },
          close: () => this.far.close(),
        },
      },
    );
    const far = () => this.far;
    this.far = new FarFind(options.conversation, (block) => this.view.findParts(block), () => this.finder.refresh());
    this.scroller.before(this.finder.element);
    if (options.focus) {
      this.focusStrip(options.focus);
      this.drawFocus();
    }
    this.element.querySelector(".find-toggle")!.addEventListener("click", () => (this.finder.open ? this.finder.close() : this.finder.show()));
    this.element.querySelector(".view-steps")!.addEventListener("click", () => this.toggleSteps());
    document.addEventListener("keydown", this.findKey);
    this.name = this.element.querySelector<HTMLElement>(".who-name")!;
    // Cut or whole again as the head narrows or widens, its hint follows.
    new ResizeObserver(() => this.nameHint()).observe(this.name);
    // The title is renamed in place, where the conversation takes a name.
    this.name.addEventListener("click", () => this.rename());
    this.name.addEventListener("keydown", (event) => {
      if ((event.key === "Enter" || event.key === " ") && !this.renaming && event.target === this.name) {
        event.preventDefault();
        this.rename();
      }
    });
    if (options.manage) {
      const { manage } = options;
      const toggle = this.element.querySelector<HTMLButtonElement>(".talk-menu-toggle")!;
      toggle.addEventListener("click", () => manage.open(toggle));
      this.element.querySelector(".composer-gone")!.addEventListener("click", (event) => {
        if ((event.target as HTMLElement).closest(".composer-restore")) manage.restore();
      });
    }
    const main = this.element.querySelector<HTMLElement>(".conversation")!;
    this.state = this.element.querySelector<HTMLElement>(".who-state")!;
    this.state.addEventListener("pointerenter", () => {
      if (this.interrupted) this.drawState();
    });
    this.spoken = document.createElement("p");
    this.spoken.className = "spoken";
    this.spoken.setAttribute("role", "status");
    this.element.append(this.spoken);
    this.context = this.element.querySelector<HTMLElement>(".composer-context")!;
    this.placeDial();
    phoneWide.addEventListener("change", this.placeDial);
    if (options.controls) {
      this.badge = new UsageBadge(this.context, options.controls.words.usage);
      const usagePop = this.element.querySelector<HTMLElement>(".usage-pop")!;
      this.usagePop = new UsagePop(usagePop, options.controls.words.usage, () => this.openSteps());
      this.usageSteady = new Steady(usagePop);
      this.usagePopover = new Popover(usagePop, this.context, () => this.usageSteady?.free());
    }
    this.input = this.element.querySelector("textarea")!;
    this.fitter = new DraftFit(this.input);
    this.action = this.element.querySelector<HTMLButtonElement>(".composer-action")!;
    this.note = this.element.querySelector<HTMLElement>(".composer-note")!;
    this.note.addEventListener("pointerover", () => {
      if (this.noteDetail) this.note.title = this.noteDetail();
    });
    this.queue = new SendQueue(options.draftKey ?? "", words.queue, {
      now: (item) => this.sendQueued(item, true),
      changed: () => this.drawAction(),
    });
    this.element.querySelector(".composer-queue")!.replaceWith(this.queue.element);
    if (options.controls) {
      this.controlQueue = new ControlQueue(options.controls.words, (id) => void this.withdraw(id));
      this.queue.element.before(this.controlQueue.element);
    }
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
      if (event.key !== "Escape" || !this.usagePopover?.open) return;
      const inside = pop.contains(document.activeElement) || document.activeElement === this.context;
      this.usagePopover.hide();
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
      following: () => this.following,
      // Words the user just sent stand at the top of the view.
      sent: (key) => this.sent(key),
      // Turns not held are read as the reader comes near them.
      near: (gap, side) => void this.fillGap(gap, side),
      // Words being sent leave from where they were typed.
      departure: () => departureOf(this.input),
      here: options.here,
      residentMachine: someone.machine,
      agent: someone.agent,
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
      agentRecord: options.agentRecord,
      turnCalls: options.calls,
      openAgent: options.side ? (id) => this.side?.openAgent(id) ?? false : undefined,
      named: options.named,
    });
    // The card at a fork's head goes back to where it came from.
    main.addEventListener("click", (event) => {
      const target = event.target as Element;
      const from = target.closest<HTMLElement>(".fk-from[data-fork-source]");
      if (!from || !target.closest(".fk-card")) return;
      const source = from.dataset.forkSource!;
      const at = source.lastIndexOf("@");
      options.toSource?.({ resident: source.slice(0, at), machine: source.slice(at + 1) }, from.dataset.forkTurn ?? null);
    });
    // Its parts for the desk's side (`side/parts.ts`): the plan, the goal,
    // and who works with it, the host's own agents among them.
    if (options.side) {
      this.side = new SideParts(this.element, {
        host: markOf(someone.agent) === "codex" ? "codex" : "claude",
        seams: options.side,
        say: (text, detail) => this.say(text, detail ?? ""),
        // Answers go in at once, into the turn if one runs.
        answer: async (text) => {
          const came = await this.options.send(text, { refs: [], shown: [] });
          if (came === "failed") throw new Error(words.notSent(""));
        },
        agentBox: (run) =>
          new AgentBox(run, {
            words,
            english: words.english,
            here: options.here,
            residentMachine: someone.machine,
            ...(options.lines ? { lines: options.lines } : {}),
            ...(options.agentRecord ? { record: options.agentRecord } : {}),
            reading: () => {},
          }),
        jump: (id) => {
          if (this.view.openAgent(id)) this.following = false;
        },
        // What it is pointed at leaves the head for the side's foot.
        env: this.element.querySelector<HTMLElement>(".focus-strip"),
      });
    }
    // For how long a way has been broken, and why, as the note says it.
    const broken = (since: number, why: string) => words.brokenFor(words.took(Date.now() - since), why);
    // A file the agent's words name is on the machine their turn worked on.
    const machineFor = (element: HTMLElement, path: string): MachineName | null => {
      for (let block = element.closest<HTMLElement>("[data-key]"); block; block = block.parentElement?.closest<HTMLElement>("[data-key]") ?? null) {
        if (options.conversation.get(block.dataset.key!)) return options.conversation.machineNamed(block.dataset.key!, path);
      }
      return null;
    };
    if (options.opening) {
      this.fileLinks = new FileLinks(main, {
        words: words.files,
        here: options.here,
        machine: someone.machine,
        machineFor,
        open: options.opening.open,
        openAttachment: options.opening.openAttachment,
        reveal: options.opening.reveal,
        say: (text, detail) => this.say(text, detail ?? ""),
        note: (text, detail) => {
          this.say(text, detail?.() ?? "");
          this.noteDetail = detail;
        },
        unsay: (text) => {
          if (this.note.textContent === text) this.say("");
        },
        broken,
      });
    }
    if (options.pictures) {
      this.pictures = new Pictures(main, {
        words: words.pictures,
        here: options.here,
        machine: someone.machine,
        machineFor,
        seams: options.pictures,
        say: (text, detail) => this.say(text, detail ?? ""),
        broken,
      });
    }

    // It opens in the view it was last read in here (`world-voice.md`).
    this.viewing(this.storedView());
    // What was read of the conversation from the shell is drawn as it comes.
    options.conversation.changed = () => this.update({ read: true });
    this.latest = this.element.querySelector<HTMLButtonElement>(".to-latest")!;
    this.latest.addEventListener("click", () => this.toLatest());
    // The way back to each step's context, after one of its rows was
    // opened here: the page as it was left.
    const back = this.element.querySelector<HTMLElement>(".steps-back")!;
    back.querySelector(".steps-back-go")!.addEventListener("click", () => {
      back.hidden = true;
      this.openSteps(true);
    });
    back.querySelector(".steps-back-x")!.addEventListener("click", () => (back.hidden = true));

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
      if (event.key === "Enter" && !event.shiftKey && !composing(event) && !matchMedia("(pointer: coarse)").matches) {
        event.preventDefault();
        this.submit();
      }
    });
    // Whatever grows after it is drawn (fonts, formulas, diagrams, a card
    // being answered) keeps the newest in view while the reader follows.
    // The region itself shrinks when the composer grows (an attachment, a
    // longer draft): the same holds.
    const keep = new ResizeObserver(() => {
      this.fitRoom();
      if (this.following && !this.reading()) this.scroller.scrollTop = this.scroller.scrollHeight;
    });
    keep.observe(main);
    keep.observe(this.scroller);
    // A wheel, a finger, a key or the scroll bar moving the view says the
    // reader scrolls: upward, they stop following at once, and for a moment
    // after any of them nothing pulls the view back to the foot. While words
    // stream in, the browser merges the reader's scroll with the one that
    // kept the newest in view, so the scroll alone cannot tell them apart.
    const reading = () => {
      this.readAt = performance.now();
    };
    const up = () => {
      reading();
      this.following = false;
      this.drawLatest();
    };
    // Any move of the reader's own makes the view theirs, down as well as
    // up: what comes after is shown where it lands, not brought to them.
    const moved = () => {
      if (this.following) up();
      else reading();
    };
    // A wheel over something in it that scrolls up by itself (a long code
    // block) moves that, not the view; keys in a field move its caret.
    this.scroller.addEventListener(
      "wheel",
      (event) => (event.deltaY < 0 && !scrollsUpWithin(event.target as Element, this.scroller) ? up() : moved()),
      { passive: true },
    );
    this.scroller.addEventListener("touchmove", moved, { passive: true });
    // The scroll bar is the region itself, not anything in it.
    this.scroller.addEventListener("pointerdown", (event) => {
      if (event.target === this.scroller) moved();
    });
    this.scroller.addEventListener("keydown", (event) => {
      if ((event.target as Element).closest("input, textarea, select, [contenteditable]")) return;
      if (event.key === "PageUp" || event.key === "ArrowUp" || event.key === "Home") up();
      else if (event.key === "PageDown" || event.key === "ArrowDown" || event.key === "End" || event.key === " ") moved();
    });
    // Otherwise only a scroll the reader made says whether they still
    // follow: one that comes with the region or its content changing size
    // (the browser keeping the view anchored) does not.
    let sized = { client: 0, content: 0 };
    this.scroller.addEventListener("scroll", () => {
      const { scrollTop, clientHeight, scrollHeight } = this.scroller;
      this.element.classList.toggle("scrolled", scrollTop > 0);
      const resized = clientHeight !== sized.client || scrollHeight !== sized.content;
      sized = { client: clientHeight, content: scrollHeight };
      if (resized && !this.reading()) {
        if (this.following) this.scroller.scrollTop = scrollHeight;
        return;
      }
      // The room goes once it is wholly below the view: nothing seen moves.
      if (this.roomFor && this.room.offsetTop >= scrollTop + clientHeight) this.dropRoom();
      this.drawLatest();
    });
  }

  /** When the reader last moved the view themselves. */
  private readAt = -Infinity;

  /** Whether the reader is moving the view now: a moment after their last
   *  wheel, finger, key or scroll bar. */
  private reading(): boolean {
    return performance.now() - this.readAt < READING_MS;
  }

  /** The way back to the newest, shown when the reader is far from it: more
   *  than a screen above the foot, or with turns not held between. */
  private latest: HTMLButtonElement;

  private drawLatest(): void {
    const { scrollTop, clientHeight } = this.scroller;
    // What is below the view, the room aside, is something to go to.
    const below = this.room.offsetTop - scrollTop - clientHeight;
    const far = !this.following && (below > 48 || this.options.conversation.gaps().some((gap) => !gap.first));
    if (this.latest.hidden === !far) return;
    this.latest.hidden = !far;
  }

  /** Back to the newest: at once, however far, and following it from
   *  there, as the reader asked, until they move the view again. */
  private toLatest(): void {
    this.following = true;
    this.dropRoom();
    this.render();
    this.scroller.scrollTop = this.scroller.scrollHeight;
    this.drawLatest();
  }

  /** The next words the user sends are put at the top of the view. */
  private placeSent(): void {
    this.following = false;
    this.placeUntil = performance.now() + SENT_WITHIN_MS;
  }

  /** Words just sent are on the page: where the user sent them, they stand
   *  near the top of the view, room made beneath them for the answer. */
  private sent(key: string): void {
    if (performance.now() > this.placeUntil) return;
    this.placeUntil = 0;
    this.roomFor = key;
    this.following = false;
    this.fitRoom();
    const words = this.view.elementOf(key);
    if (!words) return;
    const top = words.getBoundingClientRect().top - this.scroller.getBoundingClientRect().top + this.scroller.scrollTop;
    this.scroller.scrollTop = Math.max(0, top - SENT_AT);
  }

  /** Keeps the room just as tall as lets the words sent stand at the top of
   *  the view: what comes beneath them takes it up, so nothing moves. */
  private fitRoom(): void {
    if (!this.roomFor) return;
    const words = this.view.elementOf(this.roomFor);
    if (!words) {
      this.dropRoom();
      return;
    }
    const top = words.getBoundingClientRect().top - this.scroller.getBoundingClientRect().top + this.scroller.scrollTop;
    const end = this.room.offsetTop;
    const room = Math.max(0, Math.round(this.scroller.clientHeight - SENT_AT - (end - top)));
    const height = `${room}px`;
    if (this.room.style.height !== height) this.room.style.height = height;
  }

  private dropRoom(): void {
    this.roomFor = null;
    if (this.room.style.height) this.room.style.height = "";
  }

  /** Reads the turns of a gap the reader came near, then draws them. */
  private async fillGap(gap: Gap, side: "end" | "start"): Promise<void> {
    await this.options.conversation.fill(gap, side);
    this.update({ read: true });
  }

  /** The page holds a few thousand events of a conversation at most
   *  (`conversation.ts`): past that, it lets go of the parts farthest from
   *  the reader, and of what its latest part holds before its newest turns
   *  while the reader keeps to the newest. Each costs the page about as
   *  much as the events in it; these bounds hold several parts, a few
   *  screens each, and stay well within what a phone's web view gives a
   *  page. */
  private bound(): void {
    const conversation = this.options.conversation;
    if (!conversation.windowed) return;
    const parts = conversation.held();
    const latest = parts[parts.length - 1]!;
    // A turn is never cut in two: where the latest turn alone is longer,
    // it is asked again only once that much more has come.
    if (latest.end < this.trimmedAt) this.trimmedAt = 0;
    const nearNewest = this.room.offsetTop - this.scroller.scrollTop - this.scroller.clientHeight < this.scroller.clientHeight;
    if ((this.following || nearNewest) && latest.end - latest.start > LATEST_MOST && !this.trimming && latest.end - this.trimmedAt > LATEST_KEPT) {
      this.trimming = true;
      this.trimmedAt = latest.end;
      void conversation.trim(LATEST_KEPT).finally(() => {
        this.trimming = false;
        this.update({ read: true });
      });
    }
    let events = parts.reduce((sum, part) => sum + (part.end - part.start), 0);
    if (events <= HELD_MOST) return;
    // Only parts well away from what is in view go, the farthest first:
    // what the reader is near stays, however much it holds.
    const far = new Map<Held, number>();
    for (const part of parts) {
      if (part === latest) continue;
      const blocks = part.fold.blocks();
      const first = blocks[0];
      const last = blocks[blocks.length - 1];
      const screens = first && last ? this.view.screensFrom(first.key, last.key) : null;
      if (screens !== null && screens > KEEP_SCREENS) far.set(part, screens);
    }
    let changed = false;
    for (const [part] of [...far].sort((a, b) => b[1] - a[1])) {
      if (events <= HELD_MOST) break;
      conversation.release(part);
      events -= part.end - part.start;
      changed = true;
    }
    if (changed) this.update({ read: true });
  }
  private trimming = false;
  /** Where the latest part ended when it was last cut, or tried. */
  private trimmedAt = 0;

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
    if (this.options.conversation.takeControlsChanged()) this.syncControls();
    // A host that keeps a checkpoint per turn (Claude) has one more once a
    // turn ends; nothing else it offers changes with a turn, and what does
    // change is said by the host (commands, above).
    const moving = this.options.conversation.moving();
    // So does an answer given without the host's own part (its commands),
    // as when its host was too busy to say in the middle of a turn.
    if (this.wasMoving && !moving && (this.offered?.checkpoints || !this.offered?.commands.length)) void this.askOffers();
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
    this.drawAgents();
    this.drawTurnActs();
    this.finder.refresh();
    this.drawState();
    this.drawAction();
    if (follow && !this.reading()) this.scroller.scrollTop = this.scroller.scrollHeight;
    this.fitRoom();
    this.drawLatest();
    this.bound();
  }

  private side: SideParts | null = null;
  /** In focus on the desk. */
  private active = true;
  /** The desk's own buttons in the head. */
  private tools: HTMLElement[] = [];
  private agentsSeen = "";

  /** The host's own agents, as the conversation now knows them, to their
   *  picture whenever one of them moved or the parts held changed. */
  private drawAgents(): void {
    const conversation = this.options.conversation;
    const seen = `${conversation.agentStamp}|${conversation.bounds().join(";")}`;
    if (seen === this.agentsSeen) return;
    this.agentsSeen = seen;
    this.side?.agents(conversation.agents());
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
    else if (this.place && !this.view.restore(this.place)) this.toLatest();
  }

  /** What is held is read again (`conversation.ts`): the reader stays
   *  where they were, the turns they read read again where they are no
   *  longer held. */
  reopened(apply: () => void): void {
    const place = this.following ? null : this.view.anchor();
    const turn = place ? this.turnOf(place.key) : null;
    apply();
    this.update({ read: true });
    if (this.wantedTurn) return this.bringTurn();
    if (!place) return;
    cancelAnimationFrame(this.frame);
    this.render();
    if (this.view.restore(place)) return;
    if (!turn) return;
    void this.options.conversation.reachTurn(turn).then((reached) => {
      this.render();
      if (!reached || !this.view.restore(place)) this.toLatest();
    });
  }

  /** A turn asked for from elsewhere, as a request in its login's usage,
   *  until it is brought into view. */
  private wantedTurn: string | null = null;

  /** Brings `turn` into view, read first where it is not held; a page
   *  whose conversation has not come yet brings it once it does. */
  showTurn(turn: string): void {
    this.wantedTurn = turn;
    this.bringTurn();
  }

  private bringTurn(): void {
    const turn = this.wantedTurn;
    if (!turn || !this.element.isConnected) return;
    const conversation = this.options.conversation;
    const first = () => conversation.blocks().find((block) => this.turnOf(block.key) === turn);
    const bring = (key: string) => {
      this.wantedTurn = null;
      this.following = false;
      cancelAnimationFrame(this.frame);
      this.render();
      this.view.bring(key);
      this.drawLatest();
    };
    const held = first();
    if (held) return bring(held.key);
    void conversation.reachTurn(turn).then((reached) => {
      if (this.wantedTurn !== turn) return;
      const found = reached ? first() : undefined;
      if (found) bring(found.key);
      // A conversation that came without it does not have it: it opens
      // as it would.
      else if (conversation.blocks().length) this.wantedTurn = null;
    });
  }

  /** The turn a block is in. */
  private turnOf(key: string): string | null {
    const entry = this.options.conversation.get(key);
    if (entry && "turn" in entry && typeof entry.turn === "string" && entry.turn) return entry.turn;
    const block = this.options.conversation.blocks().find((one) => one.key === key);
    return block?.kind === "activity-group" ? block.turn : null;
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

  /** On a phone the dial stands in the head, away from the button that
   *  sends and stops under the thumb; elsewhere it ends the input's row. */
  private readonly placeDial = () => {
    const head = this.element.querySelector<HTMLElement>(".page-head-actions");
    const row = this.element.querySelector<HTMLElement>(".composer-row")!;
    if (phoneWide.matches && head) head.prepend(this.context);
    else row.insertBefore(this.context, row.querySelector(".composer-action"));
  };

  /** Stops what the page keeps asking for. */
  leave(): void {
    phoneWide.removeEventListener("change", this.placeDial);
    clearTimeout(this.offersAgain);
    clearTimeout(this.focusUndoTimer);
    cancelAnimationFrame(this.frame);
    this.view.destroy();
    this.badge?.dispose();
    this.side?.leave();
    this.finder.destroy();
    this.left = true;
    document.removeEventListener("keydown", this.findKey);
    this.stopDrops?.();
    this.unbindShared?.();
    this.fileLinks?.release();
    this.pictures?.release();
    void this.session?.close();
  }

  private left = false;
  /** The node's shared input, once open. */
  private session: ComposerSession | null = null;
  private sharedState: ComposerSessionState | null = null;
  private unbindShared: (() => void) | null = null;
  private nodeQueue: NodeQueue | null = null;
  private controlQueue: ControlQueue | null = null;
  /** The controls the node took to do once a turn ends, by their ids. */
  private readonly pendingControls = new Map<string, PendingControl>();
  /** The asking of what the conversation offers that is out, and the one
   *  that follows it for every wish made meanwhile. */
  private offersAsking: Promise<void> | null = null;
  private offersWanted: Promise<void> | null = null;
  private nodeTray: NodeTray | null = null;
  /** Words sent straight on from here that the agent has not heard yet. */
  private straight = 0;

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
    // What could not be done to waiting words is said in the interface's
    // words, the node's own on the pointer's rest.
    const say = (why: string, said?: string) => this.say(said ?? this.options.words.notDone(why), why, true);
    const assistantFiles = this.options.attach?.filesOn ?? null;
    this.nodeQueue = new NodeQueue(this.options.words.queue, this.options.attach?.words ?? null, session, say, assistantFiles, (id, text, attachments, sending) => {
      // Sent now, they are said at once; the node takes them into the turn.
      this.onItsWay(id, text, attachments);
      this.update();
      sending.catch(() => {
        const key = this.onTheirWay.get(id);
        if (key) this.options.conversation.unsend(key);
        this.onTheirWay.delete(id);
        this.update();
      });
    });
    this.queue.element.replaceWith(this.nodeQueue.element);
    // From now on the commands the node holds stand among the words.
    if (this.options.controls) {
      this.nodeQueue.showControls(this.options.controls.words, (id) => void this.withdraw(id));
      this.controlQueue?.element.remove();
      this.controlQueue = null;
      this.syncControls();
    }
    if (this.options.attach && this.tray) {
      this.nodeTray = new NodeTray(this.options.attach.words, session, session.draft, say, assistantFiles);
      this.tray.element.replaceWith(this.nodeTray.element);
    }
    session.subscribe((state) => {
      this.sharedState = state;
      this.showOnTheirWay(state.composer);
      this.nodeQueue?.draw(state);
      this.nodeTray?.draw(state);
      this.fit();
      this.drawAction();
    });
  }

  /** Shows words of the queue on their way as said, once each. */
  private onItsWay(id: string, text: string, attachments: readonly { name: string; size: number; media_type: string }[]): void {
    if (this.onTheirWay.has(id) || this.departed.has(id)) return;
    // The queue and what the agent heard come by two ways: words the agent
    // heard before the queue said they were going stand already.
    if (this.joinedAt.has(id) && this.options.conversation.heardAlready(text, this.joinedAt.get(id) ?? null)) {
      this.onTheirWay.set(id, "");
      return;
    }
    const shown = attachments.map((one) => ({ name: one.name, size: one.size, image: one.media_type.startsWith("image/"), preview: null }));
    this.onTheirWay.set(id, this.options.conversation.sending(text, this.options.here || null, shown));
  }

  /** Words the node sends from the shared queue, whoever asked, are said
   *  in the conversation until the agent hears them; words back in the
   *  queue, or whose arrival nobody can tell, leave it. */
  private showOnTheirWay(composer: ComposerSnapshot | undefined): void {
    if (!composer) return;
    for (const item of composer.queue) {
      if (!this.joinedAt.has(item.id)) this.joinedAt.set(item.id, item.sending ? null : this.options.conversation.heardMark());
    }
    for (const id of this.joinedAt.keys()) if (!composer.queue.some((item) => item.id === id)) this.joinedAt.delete(id);
    for (const item of composer.queue) if (item.sending) this.onItsWay(item.id, item.draft.text, item.draft.attachments);
    for (const [id, key] of this.onTheirWay) {
      if (composer.queue.some((item) => item.id === id && item.sending)) continue;
      const back = composer.queue.some((item) => item.id === id) || composer.unobserved.some((item) => item.id === id);
      if (back && key) this.options.conversation.unsend(key);
      this.onTheirWay.delete(id);
      if (!back) {
        this.departed.add(id);
        if (this.departed.size > 256) this.departed.delete(this.departed.values().next().value as string);
      }
    }
    this.update();
  }

  /** Asks the agent to stop: what runs shows stopped at once, and the
   *  button says it is stopping until the turn ends. A World that says no
   *  turn runs is believed: what still looked under way here had ended. A
   *  stop that did not get there shows it going on again, and says why. */
  private stop(): void {
    const asked = this.options.stop();
    if (!asked) return;
    this.stopping = true;
    this.options.conversation.halt();
    this.update();
    asked.then(
      (stopped) => {
        if (stopped) return;
        this.stopping = false;
        this.options.conversation.noTurnRuns();
        this.update();
      },
      (error: unknown) => {
        this.stopping = false;
        this.options.conversation.unhalt();
        const why = error instanceof Error ? error.message : String(error);
        this.say(this.options.words.turnNotStopped(why), why, true);
        this.update();
      },
    );
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
    // While the shared input is not connected, the words wait here, shown
    // as waiting to send; the node says once they go whether they queue.
    const connected = session.state.connected;
    const queued = connected && this.queues();
    const going = session.state.attachmentsOnTheirWay;
    const shown = (draft?.draft.attachments ?? []).filter((one) => !going.has(one.id)).map((one) => ({ name: one.name, size: one.size, image: one.media_type.startsWith("image/"), preview: null }));
    const key = queued ? null : conversation.sending(text, here || null, shown);
    if (key && !connected) conversation.sent(key, { state: "waiting", why: session.state.error?.message ?? "" });
    // Words that will wait for the turn to end are in the queue at once, as
    // sent from here, while the node of the conversation's machine takes
    // them in over the network.
    const waiting = queued ? (this.nodeQueue?.pending(text, shown.length) ?? null) : null;
    // The words leave the input as they are sent: what is typed from now is
    // the next words, which go once the node has these.
    const sent = session.send();
    this.update();
    if (waiting) void sent.then(waiting, waiting);
    if (key) {
      this.straight += 1;
      this.nodeQueue?.goingStraight(this.straight);
      const heard = () => {
        this.straight -= 1;
        this.nodeQueue?.goingStraight(this.straight);
      };
      void sent.then(heard, heard);
    }
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
        // The node has them: what they wait on is for its telling of now
        // to say (`Fold.now`), by the id it keeps them under.
        if (landed) conversation.named(key, updated.delivery ?? (landed.kind === "handed" ? landed.id : null));
        if (!landed) conversation.unsend(key);
        else if (standsUnder(landed, this.options.someone.heldByWorld === true)) {
          conversation.sent(key, { state: "landed", how: landed.kind, machine: this.options.someone.machine, why: "why" in landed ? landed.why : null });
        }
        this.update();
      },
      (error: unknown) => {
        const why = error instanceof Error ? error.message : String(error);
        // Words that did not go are where the node keeps them, back in the
        // input or still in the queue: they leave the conversation, and the
        // reason is said by the input, once for words sent after them too.
        if (key) conversation.unsend(key);
        if (!(error instanceof Withdrawn)) this.say(this.options.words.notSent(why), why, true);
        this.update();
      },
    );
  }

  /** Whether words sent now wait in the queue: only while the agent works,
   *  a turn just released has not ended, or other words are on their way,
   *  as the node decides; with it idle they go straight on. */
  private queues(): boolean {
    const composer = this.sharedState?.composer;
    return this.options.conversation.busy() || !!composer?.awaiting_turn || !!composer?.queue.some((item) => item.sending)
      || !!this.session?.state.onTheirWay;
  }

  private submit(): void {
    const text = this.input.value.trim();
    if (!this.session && this.tray?.sending) {
      this.say(this.options.attach!.words.wait);
      return;
    }
    const session = this.session;
    const files = session ? (this.nodeTray?.files ?? 0) : (this.tray?.files ?? 0);
    const assistant = this.options.attach?.filesOn;
    const placing = !!assistant && files > 0 && assistant.assisting();
    const filesOn = !placing ? null : this.nodeTray ? this.nodeTray.chosen : assistant!.place();
    if (placing && !filesOn) {
      this.say(assistant!.nowhere);
      return;
    }
    const attached = !session && this.tray?.count ? { ...this.tray.take(), filesOn } : { refs: [], shown: [], filesOn: null };
    if (!text && !attached.refs.length && !(session && this.nodeTray?.count)) {
      if (this.options.conversation.busy()) this.stop();
      return;
    }
    if (this.options.controls && text.startsWith("/")) {
      void this.typedCommand(text);
      return;
    }
    // What the last command did is behind the conversation now.
    this.say("");
    this.hideSaid();
    this.placeSent();
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
    // Sent by the user's hand, the words come to the top of the view; let
    // go of the queue by itself, they come in where the reader is not.
    if (now) this.placeSent();
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
  /** A short line under the input, such as what an act from the head's
   *  "⋯" came to. */
  notify(text: string, refused = false, detail = ""): void {
    this.say(text, detail, refused);
  }

  /** Why only images can be attached now: an agent of another's assists
   *  the user here, and no machine of theirs can take a file; null when any
   *  file can (`world-run.md`, 受协助的人附的文件). */
  private imagesOnly(): string | null {
    const files = this.options.attach?.filesOn;
    return files?.assisting() && !files.place() ? this.options.attach!.words.imagesOnly : null;
  }

  private say(text: string, detail = "", refused = false): void {
    this.note.textContent = text;
    this.note.title = detail;
    this.noteDetail = null;
    this.note.classList.toggle("refused", refused);
  }

  /** What a command answered at length, in a panel above the composer
   *  until it is closed or the user sends something. */
  private showSaid(title: string, html: string): void {
    const pop = this.element.querySelector<HTMLElement>(".said-pop")!;
    const close = this.options.controls!.words.said.close;
    pop.innerHTML = `<div class="said-head"><span class="usage-title">${escape(title)}</span><button class="icon-button said-close" type="button" aria-label="${escape(close)}" title="${escape(close)}">${glyph("close", 16)}</button></div><div class="said-body">${html}</div>`;
    pop.setAttribute("aria-label", title);
    if (pop.hidden) layerOpened({ isOpen: () => !pop.hidden && pop.isConnected, close: () => this.hideSaid() });
    pop.hidden = false;
    pop.querySelector(".said-close")!.addEventListener("click", () => this.hideSaid(), { once: true });
  }

  private hideSaid(): void {
    const pop = this.element.querySelector<HTMLElement>(".said-pop")!;
    pop.hidden = true;
    pop.innerHTML = "";
  }

  /** Which view this conversation was last read in on this device: only
   *  what was said, unless every step was asked for. */
  private storedView(): "said" | "steps" {
    if (!this.options.viewKey) return "said";
    try {
      return localStorage.getItem(this.options.viewKey) === "steps" ? "steps" : "said";
    } catch {
      return "said";
    }
  }

  /** Shows only what was said, or every step; `keep` remembers it here. */
  private viewing(mode: "said" | "steps", keep = false): void {
    const { words } = this.options;
    const toggle = this.element.querySelector<HTMLButtonElement>(".view-steps")!;
    const steps = mode === "steps";
    const label = steps ? words.viewSaid : words.viewSteps;
    toggle.setAttribute("aria-pressed", String(steps));
    toggle.setAttribute("aria-label", label);
    toggle.title = label;
    this.view.showing(mode);
    if (!keep || !this.options.viewKey) return;
    try {
      if (steps) localStorage.setItem(this.options.viewKey, "steps");
      else localStorage.removeItem(this.options.viewKey);
    } catch {
      // Not kept: the conversation opens in its usual view next time.
    }
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
      tier: (id) => this.setTier(id),
    });
    holder.append(this.modelPanel.element);
    void this.askOffers();
  }

  private renaming = false;

  /** Finds in the conversation: from the head's "⋯" on a phone. */
  showFind(): void {
    this.finder.show();
  }

  /** Whether every step is shown, not only what was said. */
  stepsShown(): boolean {
    return this.element.querySelector(".view-steps")!.getAttribute("aria-pressed") === "true";
  }

  /** The side of the conversation, opened from the head's "⋯" on a phone;
   *  none where it has no side, or the desk's side shows another's. */
  panel(): (() => void) | null {
    const side = this.side;
    return side?.frame ? () => side.frame?.show(side.part ?? side.firstPart()) : null;
  }

  /** Whether its head folded finding and the choice of view into its
   *  "⋯", as a narrow place on the desk or a phone does (`app.css`). */
  folded(): boolean {
    const steps = this.element.querySelector<HTMLElement>(".page-head .view-steps");
    return !!steps && getComputedStyle(steps).display === "none";
  }

  /** Its parts, for the desk's side while it is in focus. */
  get sideParts(): SideParts | null {
    return this.side;
  }

  /** Whether it is the conversation in focus on the desk: the keys that
   *  find in a conversation go to that one. */
  setActive(active: boolean): void {
    this.active = active;
  }

  /** The desk's own buttons at the end of the head (`desk/desk.ts`). */
  paneTools(...tools: HTMLElement[]): void {
    for (const tool of this.tools) tool.remove();
    this.tools = tools;
    this.element.querySelector(".page-head-actions")?.append(...tools);
  }

  /** Where a fork from `turn`, or from where the conversation is now,
   *  begins: the user's words, the answer, and the answer as it is drawn. */
  forkFrom(turn: string | null): { said: string | null; answer: string | null; element: HTMLElement | null } {
    const at = this.options.conversation.excerpt(turn);
    return { said: at.said, answer: at.answer, element: at.key ? this.view.elementOf(at.key) : null };
  }

  /** Where this fork came from, as the desk saw it as it was made. */
  forkSeed(from: ForkFrom): void {
    this.view.seedFork(from);
  }

  /** Shows every step, or only what was said again: by the head's button,
   *  or from its "⋯" on a phone. */
  toggleSteps(): void {
    this.viewing(this.stepsShown() ? "said" : "steps", true);
  }

  /** Renames the conversation in its title, as from its "⋯". */
  startRename(): void {
    this.rename();
  }

  /** Renames the conversation: its title becomes a field, Enter keeps the
   *  new name, Escape or leaving it keeps the old. */
  private rename(): void {
    const controls = this.options.controls;
    if (this.renaming || !controls || !this.offered?.controls.includes("rename")) return;
    this.renaming = true;
    const field = document.createElement("input");
    field.className = "who-rename";
    field.value = this.options.conversation.title ?? "";
    // An agent assisting the user is never named by the computer it runs on.
    field.placeholder = this.options.someone.assistFrom ? this.options.someone.agent : `${this.options.someone.agent} ${this.options.someone.machine}`;
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
      if (event.key === "Enter" && !composing(event)) {
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
    tray.onlyImages(() => this.imagesOnly(), (why) => this.say(why, "", true));
    this.tray = tray;
    this.element.querySelector(".composer-tray")!.replaceWith(tray.element);
    const pick = this.element.querySelector<HTMLInputElement>(".composer-pick")!;
    const shot = this.element.querySelector<HTMLInputElement>(".composer-shot")!;
    wireAttach(this.element, this.element.querySelector<HTMLButtonElement>(".composer-attach")!, this.element.querySelector<HTMLElement>(".composer-menus")!, attach.words, {
      // Where no machine of the user's can take a file, the picker offers
      // images alone.
      files: () => {
        pick.accept = this.imagesOnly() ? "image/*" : "";
        pick.click();
      },
      photo: () => shot.click(),
    });
    for (const input of [pick, shot]) {
      input.addEventListener("change", () => {
        if (input.files) this.attachFiles(input.files);
        input.value = "";
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

  /** A command typed in the composer, run as the conversation offers it.
   *  Where the page has not read what it offers yet, or what it read names
   *  no such command, it is read again first; a command is never sent on
   *  to the agent as words. */
  private async typedCommand(text: string): Promise<void> {
    const controls = this.options.controls!;
    const space = text.indexOf(" ");
    const name = (space < 0 ? text.slice(1) : text.slice(1, space)).trim();
    const args = space < 0 ? "" : text.slice(space + 1).trim();
    const find = (moving: boolean) => commandFor(commandsOf(this.offered, controls.words.commands, moving, this.side?.goals ?? false), name, args);
    if (!find(this.options.conversation.moving())) await this.askOffers();
    if (this.left) return;
    if (!this.offered) {
      this.say(controls.words.noCommands);
      return;
    }
    const command = find(this.options.conversation.moving());
    if (command?.arguments === "required" && !args) {
      this.say(controls.words.commands.needsArgs(usageOf(command)));
      return;
    }
    if (command) {
      this.clear();
      void this.run(command, args);
      return;
    }
    // One that waits for the turn is named as waiting, not unknown.
    const waiting = this.options.conversation.moving() && find(false);
    this.say(waiting ? controls.words.afterTurn(name) : controls.words.unknown(name));
  }

  /** Asks what the conversation offers, one asking at a time. Its commands
   *  can change many times in a second (a host describing every command it
   *  has), and each asking holds a place on its machine: wished for again
   *  while one is out, it asks once more after that one, for every wish
   *  made meanwhile, so whoever waits gets an answer begun after it asked. */
  private askOffers(): Promise<void> {
    if (!this.offersAsking) {
      const asking = this.askOffersNow().finally(() => {
        this.offersAsking = null;
      });
      this.offersAsking = asking;
      return asking;
    }
    const next = () => {
      this.offersWanted = null;
      return this.askOffers();
    };
    this.offersWanted ??= this.offersAsking.then(next, next);
    return this.offersWanted;
  }

  private async askOffersNow(): Promise<void> {
    const controls = this.options.controls;
    if (!controls || this.left) return;
    clearTimeout(this.offersAgain);
    try {
      const offered = await controls.offers();
      this.offered = offered;
      this.offersFailed = 0;
      this.syncControls(offered.pending_controls ?? []);
    } catch {
      // The way to its machine broke for a moment: asked again a few times,
      // soon and then less often; after that, when a turn ends. Its machine
      // offers a model picker whatever its host answers, so only a machine
      // out of reach leaves it out.
      this.offered = null;
      if (this.offersFailed < OFFERS_TRIES) {
        const wait = 1000 * 2 ** this.offersFailed++;
        this.offersAgain = window.setTimeout(() => void this.askOffers(), wait);
      }
    }
    if (this.left) return;
    this.drawChip();
    this.drawState();
    // What the conversation takes decides which of its turns' actions show.
    const can = new Set(this.offered?.controls ?? []);
    const transcript = this.element.querySelector(".conversation")!;
    transcript.classList.toggle("can-rewind", can.has("rewind"));
    // Forking from a turn needs no going back: the host forks through that
    // turn, and which turns it reaches its checkpoints say (`drawTurnActs`).
    transcript.classList.toggle("can-fork", can.has("fork"));
    this.drawTurnActs();
  }

  /** What the conversation has used, as its host says: its context, the
   *  account's limits and when they reset, what it cost. */
  private toggleUsage(): void {
    const popover = this.usagePopover;
    if (!popover) return;
    if (popover.open) return popover.hide();
    void this.showUsage(popover);
  }

  /** Opens the pop-up whole: what the host pushed since the page last
   *  asked comes first, waited for a moment so the pop-up does not open
   *  and then grow; nothing is read afresh from the provider for this. */
  private async showUsage(popover: Popover): Promise<void> {
    const asked = this.askOffers();
    await Promise.race([asked, new Promise((done) => setTimeout(done, 150))]);
    if (popover.open || !this.fillUsage()) return;
    popover.show();
    this.usageSteady?.hold();
    void asked.then(() => {
      if (popover.open) this.fillUsage();
    });
  }

  /** Each step's context on a page of its own, over the conversation: what
   *  the page holds of its requests and its calls (`context-sheet.ts`). A
   *  call's row, or a step on the drawing, brings that call into view. */
  private openSteps(again = false): void {
    const controls = this.options.controls;
    if (!controls) return;
    this.usagePopover?.hide();
    this.element.querySelector<HTMLElement>(".steps-back")!.hidden = true;
    this.steps ??= new ContextSheet(controls.words.usage, (key) => {
      if (this.view.openRow(key)) this.following = false;
      this.offerStepsBack();
    });
    const { conversation, words } = this.options;
    const calls: CallSpan[] = [];
    const tools = new Map<string, Extract<Entry, { kind: "tool" }>>();
    for (const part of conversation.held()) {
      for (const entry of part.fold.entries) {
        if (entry.kind !== "tool") continue;
        tools.set(entry.key, entry);
        calls.push({ key: entry.key, item: entry.item, turn: entry.turn, ended: entry.ended?.at ?? null, size: returnedSize(entry.tool) });
      }
    }
    this.steps.show(
      {
        requests: conversation.requests,
        calls,
        named: (key) => {
          const entry = tools.get(key);
          if (!entry) return null;
          const line = headline(entry.tool, entry.ended, words, () => null, true);
          return { glyph: line.glyph, action: line.action, object: line.object };
        },
      },
      again,
    );
  }

  /** Over the composer, the way back to each step's context. */
  private offerStepsBack(): void {
    const controls = this.options.controls;
    if (!controls) return;
    const { usage } = controls.words;
    const back = this.element.querySelector<HTMLElement>(".steps-back")!;
    back.querySelector(".steps-back-go span")!.textContent = usage.steps;
    const close = back.querySelector<HTMLElement>(".steps-back-x")!;
    close.setAttribute("aria-label", usage.close);
    close.title = usage.close;
    back.hidden = false;
  }

  /** Draws the usage pop-up from what the page holds. */
  private fillUsage(): boolean {
    if (!this.usagePop) return false;
    const { conversation } = this.options;
    const requests = conversation.requests;
    this.usageDrawn = `${requests.length}:${requests[requests.length - 1]?.at}:${conversation.context?.used}`;
    return this.usagePop.fill({ context: conversation.context ?? this.offered?.usage?.context ?? null, usage: this.offered?.usage ?? null, requests });
  }

  /** Starting again from the user's words: said plainly first, done once
   *  the user agrees. Forking from a turn takes nothing away: it is done at
   *  once, the fork opening beside the conversation. */
  private turnAct(act: "rewind" | "regenerate" | "fork", turn: string, words: string | null, kept: Attachment[]): void {
    const controls = this.options.controls;
    if (!controls) return;
    if (act === "fork") {
      void this.run({ name: "fork", description: "", args: null, arguments: "none", control: "fork", source: null }, "", { do: "fork", through: turn });
      return;
    }
    const said = controls.words;
    const confirm = this.element.querySelector<HTMLElement>(".composer-confirm")!;
    const ask = act === "rewind" ? said.rewindAsk : act === "regenerate" ? said.regenerateAsk : said.forkAsk;
    const go = act === "rewind" ? said.rewindGo : act === "regenerate" ? said.regenerateGo : said.forkGo;
    confirm.innerHTML = `<p>${escape(ask)}</p><div class="composer-confirm-acts"><button type="button" class="ask-button ask-primary" data-go>${escape(go)}</button><button type="button" class="ask-button" data-never>${escape(said.never)}</button></div>`;
    const close = () => {
      confirm.hidden = true;
      confirm.innerHTML = "";
      confirm.onkeydown = null;
    };
    layerOpened({ isOpen: () => !confirm.hidden && confirm.isConnected, close });
    confirm.hidden = false;
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
              this.placeSent();
              void this.options.send(words, kept.length ? { refs: kept, shown: [] } : { refs: [], shown: [] });
              return;
            }
            if (words !== null && !this.input.value.trim()) {
              if (this.session) this.setWords(words);
              else this.input.value = words;
              // What came with the words comes back with them.
              if (this.session && kept.length && !this.nodeTray?.count) {
                for (const attachment of kept) {
                  void this.session.attach(attachment as Attachment).catch((error: unknown) => {
                    const why = error instanceof Error ? error.message : String(error);
                    this.say(this.options.words.notDone(why), why, true);
                  });
                }
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
    const found = commandsOf(this.offered, this.options.controls.words.commands, this.options.conversation.moving(), this.side?.goals ?? false).filter((command) =>
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
        if (asked === this.mentionAsked && menu.open) menu.show([], this.options.words.notDone(error instanceof Error ? error.message : String(error)));
      },
    );
  }

  /** A choice in the model panel is shown at once with what the next turn
   *  runs with; the host only keeps it for that turn, and a refusal takes
   *  it back. Taken, it is also what the next new conversation with this
   *  agent begins with (`app/model-memory.ts`). */
  private async setModel(model: string | null, effort: string | null): Promise<void> {
    const before = this.offered;
    if (before) this.choose({ ...before, model: model ?? before.model, effort: effort ?? before.effort });
    const done = await this.run({ name: "model", description: "", args: null, arguments: "optional", control: "model", source: null }, "", { do: "model", model, effort }, true);
    if (!done && before) this.unchoose(before);
    else if (done) keepChoice(this.options.someone.agent, model ?? before?.model ?? null, effort ?? before?.effort ?? null);
  }

  private async setTier(id: string): Promise<void> {
    const before = this.offered;
    if (before) this.choose({ ...before, service_tier: id });
    const done = await this.run({ name: "tier", description: "", args: null, arguments: "optional", control: null, source: null }, id, { do: "command", name: "tier", args: id }, true);
    if (!done && before) this.unchoose(before);
  }

  private choose(next: Offers): void {
    this.offered = next;
    this.drawChip();
    this.sayNextTurn();
  }

  private unchoose(before: Offers): void {
    this.offered = before;
    this.drawChip();
  }

  /** What the next turn runs with, said under the input once the host took
   *  a choice made in the model panel: the model, the effort and a speed
   *  other than the standard one, in the panel's own words. */
  private sayNextTurn(): void {
    const offered = this.offered;
    const words = this.options.controls?.words.said;
    if (!offered || !words) return;
    const model = offered.models.find((one) => one.id === offered.model);
    const tier = offered.service_tier && offered.service_tier !== "default" ? (model?.service_tiers.find((one) => one.id === offered.service_tier)?.name ?? offered.service_tier) : null;
    const effort = offered.effort ? effortLabel(offered.effort, this.options.words.english) : null;
    this.say(words.model(model?.name ?? offered.model ?? "", [effort, tier].filter(Boolean).join(" ") || null));
  }

  /** Does a command, the World's own control or the host's; says whether
   *  it was done. A choice made in the model panel is `quiet`: the panel
   *  shows it at once, so under the input only a refusal is said. */
  private async run(command: MenuCommand, args: string, given?: Control, quiet = false): Promise<boolean> {
    const controls = this.options.controls;
    if (!controls) return false;
    const words = controls.words;
    if (command.control === "stop") {
      this.stop();
      return true;
    }
    if (command.control === "goal") {
      this.side?.setGoal(args);
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
        if (!this.usagePopover?.open) this.toggleUsage();
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
    // A node that tells commands in the conversation shows them there, as
    // they run and as they end; the line beside the input says only what
    // never reached it.
    const told = Boolean(this.offered?.controls_told);
    if (!quiet && !told) this.say(words.running(command.name));
    // A fork's place opens beside the conversation as it is asked for, and
    // closes again where none comes of it.
    const forking = what.do === "fork";
    let forkHeld = false;
    if (what.do === "fork") controls.forking?.(what.through);
    try {
      const done = await controls.control(what);
      // Taken to do once the turn ends: its line above the input says so.
      if (done.kind === "reported" && done.queued) {
        this.pendingControls.set(done.queued, { id: done.queued, control: what, at: new Date().toISOString(), state: "waiting" });
        this.say("");
        this.syncControls();
        if (forking) {
          forkHeld = true;
          controls.forkQueued?.();
        }
        return true;
      }
      // A choice kept for the next turn: the caller already shows it, and
      // nothing else changed for it to be asked again.
      if (quiet && (done.kind === "done" || done.kind === "reported" || done.kind === "selected")) return true;
      if (done.kind === "forked") {
        this.say("");
        forkHeld = true;
        controls.forked(done.resident);
        return true;
      }
      // The fork exists but not all of it came over: it opens, and why is said.
      if (done.kind === "fork_incomplete") {
        if (told) this.say("");
        else this.say(words.forkIncomplete, done.why, true);
        forkHeld = true;
        controls.forked(done.resident);
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
      if (told) {
        this.say("");
        void this.askOffers();
        return done.kind !== "refused";
      }
      if (done.kind === "side_answer") {
        const reply = done.reply !== null && typeof done.reply === "object" && !Array.isArray(done.reply) ? done.reply : {};
        const text = typeof reply.text === "string" ? reply.text : JSON.stringify(done.reply, null, 2);
        this.say("");
        this.showSaid(`/${command.name}`, `<div class="prose">${render(text)}</div>`);
        return true;
      }
      if (done.kind === "refused") {
        this.say(done.current_model ? words.refusedOn(done.said, done.current_model) : done.said, "", true);
        void this.askOffers();
        return false;
      }
      const model = what.do === "model" ? this.offered?.models.find((m) => m.id === what.model) : undefined;
      const said = done.kind === "done" || done.kind === "reported" ? done.said : null;
      const shown = said === null && done.kind === "reported" ? null : shownOf(command.name, what.do === "model" && model ? { ...what, model: model.name ?? model.id } : what, said, words.said);
      if (shown?.kind === "panel") {
        this.say("");
        this.showSaid(shown.title, shown.html);
      } else this.say(shown ? shown.text : done.kind === "reported" ? words.reported(command.name) : words.done(command.name));
      void this.askOffers();
      return true;
    } catch (error) {
      // One the node told in the conversation is said there.
      const failure = typeof error === "object" && error !== null ? (error as { code?: unknown }) : null;
      if (told && (failure?.code === "told" || failure?.code === "control_uncertain")) this.say("");
      else if (failure?.code === "control_uncertain") this.say(words.unknownOutcome(command.name), detailOf(error));
      else this.say(words.failed(detailOf(error)), detailOf(error), true);
      return false;
    } finally {
      if (forking && !forkHeld) controls.forkFailed?.();
    }
  }

  private fit(): void {
    this.fitter.fit();
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
        withdraw: words.withdrawSentinel,
        withdrawAsk: words.withdrawSentinelAsk,
        notStopped: words.notStopped,
        cancel: this.options.controls?.words.never ?? words.cancel,
        wakeAt: words.sentinelWakeAt,
        wakes: words.sentinelWakes,
        output: words.jobOutput,
        loading: words.jobOutputLoading,
        failed: words.jobOutputFailed,
        interrupted: words.watchInterrupted(null),
        ended: words.jobOver,
        lasted: (ms) => lasted(ms, words),
      },
      stop: this.options.stopJob,
      output: this.options.jobOutput,
      watch: this.options.watchLink,
      detail: detailOf,
    });
    this.jobs.update(jobs, now);
  }

  private away: "removed" | "deleted" | "away" | null = null;

  /** Whether the user archived it, or deleted it, as the World last listed
   *  it. Archived, it is kept to read and can be brought back where the
   *  composer was; deleted, what is shown is all that is left of it here. */
  presence(away: "removed" | "deleted" | "away" | null): void {
    if (this.away === away) return;
    this.away = away;
    // Its shared input is no one's now.
    if (away === "deleted") {
      this.unbindShared?.();
      this.unbindShared = null;
      void this.session?.close();
      this.session = null;
    }
    this.element.querySelector<HTMLElement>(".composer")!.hidden = away !== null;
    // Nothing more is heard of it: no turn is said to go on, and its goal's
    // bar, whose acts would reach nothing, steps aside.
    const unheard = away === "away" || away === "deleted";
    this.view.hearing(!unheard);
    this.element.toggleAttribute("data-unheard", unheard);
    const gone = this.element.querySelector<HTMLElement>(".composer-gone")!;
    gone.hidden = away === null;
    const manage = this.options.manage;
    gone.innerHTML =
      away === "removed"
        ? manage
          ? `${glyph("archive", 16)}<span>${escape(manage.words.archived)}</span><button class="text-button composer-restore" type="button">${glyph("unarchive", 16)}<span>${escape(manage.words.restore)}</span></button>`
          : escape(this.options.words.removedNote)
        : away === "deleted"
          ? `${glyph("trash", 16)}<span>${escape(manage?.words.deleted ?? this.options.words.removedNote)}</span>`
          : away === "away"
            ? `${glyph("lock", 16)}<span>${escape(this.options.someone.assistFrom ? this.options.words.assistEndedNote : this.options.words.departedNote)}</span>`
            : "";
    this.element.querySelector<HTMLElement>(".talk-menu-toggle")?.toggleAttribute("hidden", away === "deleted");
    this.drawState();
    this.drawFocus();
    this.update();
  }

  /** Who it is, as the World lists it: a conversation opened before then,
   *  as one just started, goes by its agent's id until this; its project
   *  and model follow it when they change. */
  identify(who: Someone): void {
    // A change being kept stands until the World lists it.
    if (this.focusSaving) who = { ...who, focus: this.options.someone.focus ?? null };
    Object.assign(this.options.someone, who);
    const said = this.options.words.sendPlaceholder(who.agent);
    this.input.placeholder = said;
    this.input.setAttribute("aria-label", said);
    this.drawState();
    this.drawFocus();
  }

  private focusPanel: FocusPanel | null = null;
  /** What the panel holds while it is open. */
  private focusDraft: Focus | null = null;
  private focusSaving = false;
  /** While a choice cleared of every machine can be taken back. */
  private focusUndoTimer = 0;

  /** The strip under the head: the machines and folders it is pointed at,
   *  which open the panel to change them; a change is kept, and told to
   *  the agent, as the panel closes. */
  private focusStrip(focus: NonNullable<PageOptions["focus"]>): void {
    const strip = document.createElement("div");
    strip.className = "focus-strip";
    strip.hidden = true;
    strip.innerHTML = `<button type="button" class="talk-chip focus-chip" aria-haspopup="dialog" aria-expanded="false"></button><span class="focus-state" aria-live="polite"></span><div class="focus-pop" role="dialog" hidden></div>`;
    // In the head's one line, before its tools.
    this.element.querySelector(".page-head-actions")?.prepend(strip);
    const chip = strip.querySelector<HTMLButtonElement>(".focus-chip")!;
    const pop = strip.querySelector<HTMLElement>(".focus-pop")!;
    chip.addEventListener("click", () => (pop.hidden ? this.openFocus(focus) : this.closeFocus(focus)));
    document.addEventListener("pointerdown", (event) => {
      if (pop.hidden || !this.element.isConnected) return;
      const target = event.target as HTMLElement;
      // The window of a machine's folders, opened from the panel, is part of it.
      if (!pop.contains(target) && !chip.contains(target) && !target.closest("dialog")) this.closeFocus(focus);
    });
    strip.addEventListener("keydown", (event) => {
      if (event.key !== "Escape" || pop.hidden) return;
      event.stopPropagation();
      this.closeFocus(focus);
      chip.focus({ preventScroll: true });
    });
  }

  private openFocus(focus: NonNullable<PageOptions["focus"]>): void {
    const current = this.options.someone.focus;
    if (!current || this.focusSaving) return;
    const strip = this.element.querySelector<HTMLElement>(".focus-strip")!;
    const pop = strip.querySelector<HTMLElement>(".focus-pop")!;
    this.focusDraft = structuredClone(current);
    this.focusPanel ??= new FocusPanel(
      {
        words: focus.words,
        places: focus.places,
        folders: focus.folders,
        changed: (next) => {
          this.focusDraft = next;
        },
        done: () => this.closeFocus(focus),
        servers: focus.servers,
      },
      current,
    );
    this.focusPanel.set(current);
    pop.replaceChildren(this.focusPanel.element);
    stay(pop);
    pop.hidden = false;
    strip.querySelector(".focus-chip")!.setAttribute("aria-expanded", "true");
    layerOpened({ isOpen: () => !pop.hidden && this.element.isConnected, close: () => this.closeFocus(focus) });
  }

  private closeFocus(focus: NonNullable<PageOptions["focus"]>): void {
    const strip = this.element.querySelector<HTMLElement>(".focus-strip")!;
    const pop = strip.querySelector<HTMLElement>(".focus-pop")!;
    if (pop.hidden || leaving(pop)) return;
    leave(pop, () => (pop.hidden = true));
    strip.querySelector(".focus-chip")!.setAttribute("aria-expanded", "false");
    const before = this.options.someone.focus;
    const next = this.focusDraft;
    this.focusDraft = null;
    if (!before || !next || sameFocus(before, next)) return;
    // A choice left with no machine at all may be a slip of the hand: it can
    // be taken back for a moment.
    this.saveFocus(focus, before, next, before.machines.length > 0 && next.machines.length === 0);
  }

  private saveFocus(focus: NonNullable<PageOptions["focus"]>, before: Focus, next: Focus, undoable: boolean): void {
    const state = this.element.querySelector<HTMLElement>(".focus-state")!;
    clearTimeout(this.focusUndoTimer);
    this.focusSaving = true;
    this.options.someone.focus = next;
    this.drawFocus();
    state.textContent = focus.words.saving;
    state.classList.remove("refused");
    focus
      .set(next)
      .then((unfinished) => {
        if (undoable && !unfinished.length) this.offerFocusUndo(focus, before, next);
        else state.textContent = unfinished.join(" ");
      })
      .catch((error: unknown) => {
        this.options.someone.focus = before;
        const why = error instanceof Error ? error.message : String(error);
        state.textContent = focus.words.failed(why);
        state.title = why;
        state.classList.add("refused");
      })
      .finally(() => {
        this.focusSaving = false;
        this.drawFocus();
      });
  }

  /** Says the choice was cleared, with the way to take it back, for as long
   *  as the machines page offers its own. */
  private offerFocusUndo(focus: NonNullable<PageOptions["focus"]>, before: Focus, cleared: Focus): void {
    const state = this.element.querySelector<HTMLElement>(".focus-state")!;
    state.innerHTML = `<span class="focus-cleared">${escape(focus.words.cleared)}</span><button class="text-button focus-undo" type="button">${escape(focus.words.undo)}</button>`;
    state.querySelector(".focus-undo")!.addEventListener("click", () => {
      state.textContent = "";
      if (this.focusSaving) return;
      this.saveFocus(focus, cleared, before, false);
    });
    this.focusUndoTimer = window.setTimeout(() => {
      if (state.querySelector(".focus-undo")) state.textContent = "";
    }, FOCUS_UNDO_MS);
  }

  /** The strip as the conversation stands: shown where the user may point
   *  it, and while it is not removed. */
  private drawFocus(): void {
    const words = this.options.focus?.words;
    const strip = this.element.querySelector<HTMLElement>(".focus-strip");
    if (!words || !strip) return;
    const focus = this.options.someone.focus;
    strip.hidden = !focus || this.away !== null;
    if (!focus) return;
    const chip = strip.querySelector<HTMLButtonElement>(".focus-chip")!;
    const summary = focusSummary(focus, words);
    // On a phone the chip is its machine and how many places it holds.
    const count = focus.machines.reduce((sum, chosen) => sum + Math.max(1, chosen.folders.length), 0);
    const named = summary ? summary.html : `<span class="talk-chip-name">${escape(words.none)}</span>`;
    const html = `${glyph("machine", 16)}${named}${count ? `<span class="focus-count">${count}</span>` : ""}${glyph("chevron", 12)}`;
    if (chip.innerHTML !== html) chip.innerHTML = html;
    chip.classList.toggle("quiet", !summary);
    chip.title = summary ? `${words.title}\n${summary.said}` : words.title;
    chip.disabled = this.focusSaving;
  }

  /** The watch on the session is over: what is shown stays, and the bar
   *  says it is no longer live. */
  lost(why: string | null): void {
    this.view.unfollowed();
    this.gone = this.options.words.watchEnded(why);
    this.goneWhy = why ?? "";
    this.interrupted = false;
    this.drawState();
  }

  /** Its machine refused this account: what is shown stays, and the bar
   *  says so; on hover, the machine's words. */
  refused(why: string): void {
    this.view.unfollowed();
    this.gone = this.options.words.watchRefused;
    this.goneWhy = why;
    this.interrupted = false;
    this.drawState();
  }

  /** The live stream broke and is being read again: what is shown stays,
   *  and the bar says it is reconnecting; on hover, why and for how long. */
  broken(why: string | null, since: Date): void {
    this.gone = this.options.words.watchInterrupted(why);
    this.brokenWhy = why;
    this.brokenSince = since;
    this.interrupted = true;
    this.drawState();
  }

  /** The controls the node took to do later, as the conversation has them:
   *  what the node holds now (`snapshot`, with what it offers) and each
   *  one's events since; one that ended is said, and goes. */
  private syncControls(snapshot?: PendingControl[]): void {
    const conversation = this.options.conversation;
    if (snapshot) {
      const held = new Set(snapshot.map((row) => row.id));
      // The node's own rows: its time for each, not this device's.
      for (const row of snapshot) this.pendingControls.set(row.id, row);
      // One no longer held that no event saw end ended out of sight.
      for (const id of [...this.pendingControls.keys()]) if (!held.has(id) && !conversation.controlState(id)) this.pendingControls.delete(id);
    }
    for (const [id, row] of this.pendingControls) {
      const latest = conversation.controlState(id);
      if (!latest || latest.state === row.state) continue;
      if (latest.state === "waiting" || latest.state === "running") this.pendingControls.set(id, latest);
      else {
        this.pendingControls.delete(id);
        this.ended(latest);
      }
    }
    const rows = [...this.pendingControls.values()];
    if (this.nodeQueue) this.nodeQueue.setControls(rows);
    else this.controlQueue?.draw(rows);
  }

  /** Says what came of a control the node did after a turn. */
  private ended(row: PendingControl): void {
    const words = this.options.controls?.words;
    if (!words) return;
    const name = controlName(row);
    const result = row.result;
    // A fork done later opens as one done at once does; one that came to
    // nothing closes the place that waited for it.
    if (result?.kind === "forked" || result?.kind === "fork_incomplete") this.options.controls?.forked(result.resident);
    else if (row.control.do === "fork") this.options.controls?.forkFailed?.();
    if (row.state === "cancelled") return;
    if (this.offered?.controls_told) {
      void this.askOffers();
      return;
    }
    if (row.state === "failed") this.say(words.failed(row.error ?? ""), row.error ?? "", true);
    else if (row.state === "uncertain") this.say(words.unknownOutcome(name), row.error ?? "", true);
    else if (result?.kind === "refused") this.say(result.said, "", true);
    else if (result?.kind === "fork_incomplete") this.say(words.forkIncomplete, result.why, true);
    else if (name === "compact" || result?.kind === "forked") this.say("");
    else if (result?.kind === "reported") this.say(result.said ?? words.reported(name));
    else this.say(words.done(name));
    void this.askOffers();
  }

  /** Takes back a control still waiting for the turn to end. */
  private async withdraw(id: string): Promise<void> {
    const controls = this.options.controls;
    if (!controls) return;
    try {
      await controls.control({ do: "cancel_control", id });
      this.pendingControls.delete(id);
      this.syncControls();
    } catch (error) {
      this.say(controls.words.failed(detailOf(error)), detailOf(error), true);
    }
  }

  /** The stream came back after it broke. */
  resumed(): void {
    // What it offers may have changed while it was away.
    void this.askOffers();
    if (!this.interrupted) return;
    this.interrupted = false;
    this.gone = null;
    this.goneWhy = "";
    this.brokenWhy = null;
    this.brokenSince = null;
    this.drawState();
  }

  private interrupted = false;
  /** Why the live stream broke, and since when, while it is broken. */
  private brokenWhy: string | null = null;
  private brokenSince: Date | null = null;

  /** The state last said, and where it is said to a screen reader. */
  private said: string | null = null;
  private spoken!: HTMLElement;

  /** What resting on the title says: the whole title where the line cuts
   *  it; otherwise, where it can be renamed, that it can. */
  private nameHint(): void {
    const cut = this.name.scrollWidth > this.name.clientWidth;
    const rename = this.name.classList.contains("renamable") ? (this.options.controls?.words.commands.rename ?? "") : "";
    const said = cut ? (this.name.textContent ?? "") : rename;
    if (said) this.name.title = said;
    else this.name.removeAttribute("title");
  }

  private drawState(): void {
    const { words, someone, conversation } = this.options;
    // The host's own title for the conversation, once it gives one; until
    // then what the list calls it, else the agent's name. Where it lives
    // follows, on a computer.
    const title = conversation.title ?? this.options.listedTitle?.() ?? null;
    const name = escape(title ?? someone.agent);
    if (!this.renaming && this.name.innerHTML !== name) this.name.innerHTML = name;
    const renamable = !!this.offered?.controls.includes("rename");
    this.name.classList.toggle("renamable", renamable);
    if (renamable) this.name.tabIndex = 0;
    else this.name.removeAttribute("tabindex");
    this.nameHint();
    // Archived or deleted, it is not said to work or wait.
    const waiting = !this.gone && this.away === null && conversation.waiting();
    // Working for the whole of a turn, not only while something shows.
    const working = !this.gone && this.away === null && !waiting && conversation.busy();
    // Out of reach for good, that is what is said, not that the line broke.
    const now =
      (this.away === "away" ? null : this.gone) ??
      (this.away !== null
        ? this.away === "deleted"
          ? (this.options.manage?.words.deleted ?? words.removed)
          : this.away === "away"
            ? someone.assistFrom
              ? words.assistEnded
              : words.departed
            : words.removed
        : waiting
          ? words.waitingForYou
          : working
            ? conversation.halting()
              ? words.stopping
              : words.working
            : words.idle);
    // A screen reader hears the conversation change state, once each time:
    // its words as they stream would be too many to hear.
    if (this.said !== null && this.said !== now) this.spoken.textContent = `${someone.agent}　${now}`;
    this.said = now;
    // Who it is with and how it stands are on a pointer's rest over the
    // head's name. That it works or waits is the conversation's own to show,
    // below; only what is wrong with the line or the conversation is said in
    // words, after the title.
    const who = this.element.querySelector<HTMLElement>(".who")!;
    this.name.classList.toggle("gone", this.gone !== null || this.away !== null);
    const model = this.options.conversation.answeredBy ? modelName(this.options.conversation.answeredBy) : someone.model;
    // Who it is, where it lives, how it is now and its model: on a phone,
    // where the head has no room for its machine, a long press says it.
    const lives = someone.assistFrom ? this.options.words.assistFrom(someone.assistFrom) : someone.machine;
    who.title = [someone.agent, lives, now, model].filter(Boolean).join("　");
    const trouble = this.away === "away" ? now : (this.gone ?? (this.away !== null ? now : null));
    this.state.hidden = !trouble;
    if (this.state.textContent !== (trouble ?? "")) this.state.textContent = trouble ?? "";
    this.state.classList.toggle("gone", trouble !== null);
    // How long it has been broken is as of when the pointer comes to it.
    const why = this.interrupted && this.brokenSince
      ? words.brokenFor(words.took(Date.now() - this.brokenSince.getTime()), this.brokenWhy)
      : this.goneWhy;
    if (why) this.state.title = why;
    else this.state.removeAttribute("title");
    // How much of the context is used: the host's latest report, from the
    // conversation's events, or from what it offers when it says; with it
    // how the cache served the last request and how long it stays.
    const used = conversation.context ?? this.offered?.usage?.context ?? null;
    if (this.badge) {
      const requests = conversation.requests;
      this.badge.draw(used, requests);
      // Without anything to measure, the limits still have a way in.
      this.context.hidden = !this.badge.shows && !this.offered?.usage?.limits.length;
      // The pop-up open follows each new request.
      const pop = this.element.querySelector<HTMLElement>(".usage-pop")!;
      if (!pop.hidden && this.usageDrawn !== `${requests.length}:${requests[requests.length - 1]?.at}:${conversation.context?.used}`) this.fillUsage();
    } else if (used && used.window) {
      const share = Math.min(1, used.used / used.window);
      const percent = Math.round(share * 100);
      const label = this.options.controls
        ? this.options.controls.words.context(percent, used.used, used.window)
        : words.context(percent);
      this.context.innerHTML = `${ring(share)}<span class="context-share">${percent}%</span>`;
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
    if (!busy) this.stopping = false;
    const stop = !typed && busy;
    this.action.innerHTML = glyph(stop ? "stop" : "send", 18);
    const waits = this.released !== null || (this.session ? this.queues() : busy);
    const label = stop ? (this.stopping ? words.stopping : words.stop) : typed && waits ? words.queue.queue : words.send;
    this.action.setAttribute("aria-label", label);
    this.action.title = label;
    this.action.classList.toggle("stop", stop);
    this.action.classList.toggle("stopping", stop && this.stopping);
    this.action.disabled = !typed && !stop;
  }
}

/** A span of time as the interface says it elsewhere: minutes, hours. */
function lasted(ms: number, words: Words): string {
  const minutes = Math.max(0, Math.round(ms / 60_000));
  return minutes < 60 ? words.minutes(minutes) : words.hours(Math.floor(minutes / 60), minutes % 60);
}

/** What a call returned to the model, in characters: its text, and about a
 *  page's worth for each picture. */
function returnedSize(tool: Tool): number {
  return tool.output.reduce((size, part) => size + (part.type === "text" ? part.text.length : 1_600), 0);
}
