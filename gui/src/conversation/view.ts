// A conversation drawn in the page (`world-gui.md`, 对话). Only the blocks
// near the reader are on the page: a screen of them above and two below
// what is in view, between two spaces that stand for the rest, each as tall
// as the blocks it stands for (`heights.ts`: measured where a block was
// drawn, taken to be what it holds where not). So a conversation of
// thousands of turns is as light to read, scroll and stream into as a short
// one: the page holds the same few dozen blocks, and an event redraws only
// the block it changed. The reader never sees anything move: when what is
// above them changes height, the view moves by as much, the block they read
// staying where it is.
//
// Turns the page does not hold (`conversation.ts`) stand as a gap; when the
// reader comes near one, its turns are read, from the side the reader comes
// from.
//
// What a turn did between the things it said is a stretch of rows under one
// heading. The heading stands as long as there is a row under it, keeps its
// element while rows come and go, and says what is happening, or once done
// what the stretch did. Everything that folds starts folded and opens only
// when the reader opens it, and its rows are made only then; it opens and
// closes in motion, the line that was pressed staying where it is
// (`../longview/anchor.ts`). Long text under a row shows its first lines,
// then grows into a box that draws only what is in view.
//
// While the model works, the turn's line stands at the foot (`turn-line.ts`).
// Words the user sends rise from the input into their place (`flight.ts`).
//
// What the World told the session is drawn from the facts it carries, as a
// quiet line on the left; what was done to a turn (copy, ask again, fork)
// appears only once the turn is over.

import { diffCard, unifiedDiff, type DiffCard, type DiffKind } from "./diff-card";
import { handOnAtEdge } from "../scroll-handoff";
import { reducedMotion } from "../motion";
import { ActsMenu } from "../app/acts-menu";
import { onHeld } from "../app/press";
import type { AgentActivity, Content, Health, MachineName, Notice, Speaker, Tool, TurnEnd } from "../world/events";
import type { Attachment } from "../world/attachments";
import type { Decoding } from "../world/remote-lines";
import type { RunFacts, WriteFact } from "../world/run-facts";
import type { JsonValue } from "../world/wire";
import { folderName, type Focus } from "../world/focus";
import { answerOf, drawAsk, readAsk, type AskWords } from "./asks";
import type { Words } from "../words";
import { forkCard, type ForkFrom } from "./fork-card";
import { glyph, type GlyphName } from "./glyphs";
import { controlName } from "./control-queue";
import { saidHtml } from "./said";
import { effortLabel } from "./effort-slider";
import { isThinking, runsOf, startsAgent, type Activity, type Block, type Entry, type Gap, type Say, type SendState, type ShownAttachment } from "./model";
import { AgentBox } from "./agent-box";
import { toldSaid, type AgentRun } from "./agents";
import type { Conversation } from "./conversation";
import { escape, plain, render } from "./prose";
import { askedHtml, askedOf } from "./goal-notice";
import { numbered, parse } from "./diff";
import { DiffLines, RemoteDiff, diffPaint, encodeRow, keptDiff, memoryDiff } from "./diff-lines";
import { readRun } from "./deeds/read";
import { scriptSaid } from "./deeds/script";
import { drawDeeds, goingOn, type DeedTools } from "./deeds/draw";
import type { Deed, PlainDeed } from "./deeds/types";
import { troubled } from "./deeds/fan";
import { shortCommand } from "./deeds/rows";
import { cardsOf, type Card } from "./cards/cards";
import { bunched, cardsIn, type Bunch } from "./cards/bunch";
import { lineHtml, lineText } from "./cards/lines";
import { headline, onlyWrites, tenseOf, writeHeadline } from "./tool";
import { drawDiagrams } from "./diagrams";
import { readTold, undelivered, worldSaid, type UndeliveredWhy, type WorldSaid } from "./told";
import { allowanceOf, detailOf, heldOf } from "../app/problem";
import { AnchorLayout, AnchoredHeight, MemoryLines, type LinePaint, type ViewerSource } from "../longview";
import { FEW, LongText, type LongTextWords, type OpenedText, type OtherSource } from "./long-text";
import { Printout, givenLines, type PrintoutEnd } from "./printout";
import { Said, WholeOutput, outputPaint, type OpenedLines } from "./run-output";
import "./say.css";
import { StreamedProse } from "./stream-prose";
import { TurnLine, time, type LineSays } from "./turn-line";
import { fly, type Departure } from "./flight";
import { estimate } from "./estimate";
import { Heights } from "./heights";
import { FindContent, displayedDiff, toolText } from "./find-content";
import { clockSays, stretchSays, thoughtHeads, thoughtOnly, thoughtText, thoughtTook } from "./stretch-says";
import { displayedText, htmlText as displayedTextHtml, type TextMatch } from "./find-text";
import { GAP, type FindChanges, type FindMatch, type FindPart } from "./find-index";
import { composing } from "../typing";

export interface Context {
  words: Words;
  english: boolean;
  /** The machine this interface runs on. */
  here: MachineName;
  /** The machine the resident lives on: where the agents it starts work,
   *  which name a machine only where they worked elsewhere. */
  residentMachine: MachineName;
  /** The agent by the name it goes by ("Claude", "Codex"), where it is
   *  known: words whose host does not start name it. */
  agent?: string | undefined;
  /** The region the conversation scrolls in. */
  scroller: HTMLElement;
  /** Opens one of the host's agents in the panel beside the conversation,
   *  rather than under the row that sent it; false where it cannot. */
  openAgent?: ((id: string) => boolean) | undefined;
  /** The reader opened or closed something: the page stops keeping the
   *  newest in view, so nothing moves under their hand. */
  reading(): void;
  /** Whether the page keeps the newest in view: then the foot is what is
   *  drawn. Always, where none says. */
  following?: (() => boolean) | undefined;
  /** A gap came near the reader: its turns are read, from its end when the
   *  reader is below it, from its start when above. */
  near?: ((gap: Gap, side: "end" | "start") => void) | undefined;
  /** Where words being sent leave from (`flight.ts`). */
  departure?: (() => Departure | null) | undefined;
  /** Words just sent are on the page, laid out: the newest of them. */
  sent?: ((key: string) => void) | undefined;
  /** Where another resident's conversation opens, where it can. */
  link?: ((address: { resident: string; machine: MachineName }) => string) | undefined;
  /** How the list names another conversation, and its family, for the card
   *  of where a fork came from. */
  named?: ((address: { resident: string; machine: MachineName }) => { name: string; agent: "claude" | "codex" }) | undefined;
  /** Answering what the session waits on the user for, where the page
   *  can. */
  asks?:
    | {
        words: AskWords;
        answer(id: JsonValue, answer: JsonValue): Promise<void>;
        /** Opens a link outside the app, in the system's browser. */
        open(url: string): Promise<void>;
        /** A request was settled here. */
        settled(): void;
      }
    | undefined;
  /** Answering questions asked without waiting: the words go in at once,
   *  into the turn if one runs. */
  /** The user asked to start again from their words in `turn`, to ask the
   *  same again, or to fork from `turn`; offered only where the
   *  conversation takes it. */
  turnAct?: ((act: "rewind" | "regenerate" | "fork", turn: string, words: string | null, kept: Attachment[]) => void) | undefined;
  /** A turn's requests one by one, on the usage page of its login. */
  turnCalls?: ((turn: string) => void) | undefined;
  /** Lines of a file or a job's output on the machine holding them, read a
   *  screen at a time: a write's change, a command's whole output. */
  lines?: ((machine: MachineName, path: string, decoding: Decoding) => Promise<OpenedLines>) | undefined;
  /** One of the host's own agents' own record, read from its
   *  conversation's machine, as its box opens (`agent-box.ts`). */
  agentRecord?: ((agent: string) => Promise<AgentActivity>) | undefined;
  /** The view is one agent's own conversation, in its box under the call
   *  that started it: its stretches stand open, what it was asked is a
   *  quiet block, and nothing is done to its turns from here. */
  nested?: boolean | undefined;
}

/** A row on the page and what it is drawn from. */
interface Row {
  key: string;
  signature: string;
  html(): string;
  text(): string;
}

/** A write's row is keyed by its run's row and the write. */
const WRITE = "~";
/** The most bytes of rows a change is read ahead in, not kept with the
 *  conversation: larger ones are read as they are opened. */
const PREFETCH = 1024 * 1024;

/** How far beyond what is in view blocks are drawn, in screens: one above,
 *  as a reader going up meets what was just there; two below, where the
 *  newest comes in and a reader going down, back to it, goes faster. */
const ABOVE = 1;
const BELOW = 2;

/** How far from what is drawn a block the reader has focus or a selection
 *  in may be and still be kept drawn, in blocks. */
const HOLD = 200;

/** How many of its latest rows a stretch opens on, and how many more each
 *  ask for the earlier brings: three screens of rows. */
const STRETCH_ROWS = 60;

type ToolEntry = Extract<Entry, { kind: "tool" }>;
type AskEntry = Extract<Entry, { kind: "ask" }>;
type HeardEntry = Extract<Entry, { kind: "heard" }>;
type ProseEntry = Extract<Entry, { kind: "prose" }>;

/** One stretch on the page: its heading, its box and the rows in it, made
 *  the first time it is opened. */
interface Stretch {
  head: HTMLButtonElement;
  box: HTMLElement;
  rows: HTMLElement;
  drawn: Map<string, { signature: string; element: HTMLElement }>;
  /** What its heading was last drawn from. */
  heading: string;
}

/** A block's place in the page, while it is near the reader. */
interface Slot {
  key: string;
  element: HTMLElement;
  block: Block;
  /** What it was last drawn from; null while it is not drawn. */
  signature: string | null;
  /** A message's words, drawn block by block as they grow. */
  prose?: StreamedProse;
  /** What is done to a message's turn, under its words. */
  acts?: HTMLElement;
  /** The interface's own line, where the World's hold answered in the
   *  model's place. */
  held?: HTMLElement;
  stretch?: Stretch;
  findRevision?: number;
}

/** A change of the machines and folders, as one line. */
function focusSaid(focus: Focus, words: Words): string {
  const items = focus.machines.flatMap((chosen) =>
    chosen.folders.length ? chosen.folders.map((folder) => `${chosen.machine}: ${folderName(folder)}`) : [chosen.machine],
  );
  return items.length ? words.focusChanged(items.join(words.listSeparator)) : words.focusCleared;
}

/** Why something could not be delivered, in the interface's words where the
 *  World's are known; its own words otherwise. */
function undeliveredWhy(why: UndeliveredWhy, words: Words): string {
  switch (why.kind) {
    case "gone":
      return words.goneFromNetwork(why.machine);
    case "left":
      return words.networksLeft;
    case "other":
      return why.text;
  }
}

/** The menu the user's words raise when held on a touch screen. */
const heldMenu = new ActsMenu();

/** The mark an action of words held has in their menu, as it has under
 *  them. */
function heldIcon(button: HTMLElement): GlyphName {
  if (button.dataset.copy !== undefined || button.dataset.copyTurn !== undefined) return "copy";
  const act = button.dataset.turnAct;
  return act === "regenerate" || act === "fork" ? act : "edit";
}

export class ConversationView {
  private openRows = new Set<string>();
  /** The user's long messages opened whole. */
  private readonly unfolded = new Set<string>();
  private openStretches = new Set<string>();
  /** How many of their latest rows long stretches show, where more than
   *  `STRETCH_ROWS` were asked for. */
  private stretchRows = new Map<string, number>();
  private slots = new Map<string, Slot>();
  private byElement = new WeakMap<HTMLElement, Slot>();
  /** Long texts open under rows, by row, to be let go when the row is. */
  private longs = new Map<string, OpenedText[]>();
  /** What each run did, as read from it (`deeds`), while it stays as it
   *  was read. */
  private deedsRead = new Map<string, { stamp: number; deeds: Deed[] | null }>();
  /** The things a stretch did as their rows show them, and the rows that
   *  gather several, by key: what opening one fills it with. */
  private cards = new Map<string, Card>();
  private bunches = new Map<string, Bunch>();
  /** What else a row holds open on another machine (lines being read),
   *  let go with it. */
  private closers = new Map<string, (() => void)[]>();
  private filling = new Map<string, Promise<void>>();
  private motions = new WeakMap<HTMLElement, AnchoredHeight>();
  /** Rows already on the page once; only rows new to a live conversation
   *  make an entrance. */
  private seen = new Set<string>();
  /** The page has been drawn once: what comes after is live. */
  private live = false;
  /** Words already sent up from the input, by key. */
  private flown = new Set<string>();
  /** What finding last read of each block, and blocks gone since. */
  private findRead = new Map<string, string>();
  private findRemoved = new Set<string>();
  /** The rows of changes read whole, by row: what finding and opening
   *  them again draw from, with no request. */
  private findWrites = new Map<string, { signature: string; lines: readonly string[] }>();
  /** Reading in the background the changes that are not kept with the
   *  conversation but are small enough to read whole (`prefetch`). */
  private prefetching: AbortController | null = null;
  private readAhead = new Set<string>();
  private readonly findContent: FindContent;
  private readonly layoutAnchor: AnchorLayout;
  private readonly line: TurnLine;
  /** The clocks of stretches running, brought up to date each second. */
  private clocks = new Set<HTMLElement>();
  private ticking = 0;
  /** The blocks laid out, their heights, and the ones on the page. */
  private blocks: Block[] = [];
  private readonly heights = new Heights();
  private measured = new Map<string, number>();
  private guesses = new Map<string, { stamp: number; width: number; height: number }>();
  private shown: Slot[] = [];
  /** Which view of the conversation is drawn (`world-voice.md`, 两种看法):
   *  only what was said, or every step. */
  private mode: "said" | "steps" = "steps";
  /** The block kept where it is on the screen across a change of view,
   *  once. */
  private pin: { slot: Slot; top: number } | null = null;
  /** The words both views show that the reader was at as a change of view
   *  began, and where they stood on the screen: held there until it ends,
   *  whatever opens or closes above them, the room above their own block
   *  among it. */
  private steady: { slot: Slot; top: number } | null = null;
  /** The fraction of a pixel the view was last moved by that a scroll, in
   *  whole pixels, could not take: the root's top margin carries it, so
   *  what is kept in view stays exactly where it was. Scrolled whole
   *  pixels alone, it would be up to half a pixel off each frame, and
   *  everything moving round it would shake a pixel back and forth. */
  private trim = 0;
  /** Following, where the end of what is drawn stood on the page when last
   *  seen. */
  private foot: number | null = null;
  /** Blocks a change of view brings in: drawn folded near the reader, and
   *  unfolding once. */
  private unfolding = new Set<string>();
  /** A change of view under way: ends it at once, as another one comes. */
  private changing: (() => void) | null = null;
  /** The spaces that stand for the blocks above and below those drawn. */
  private readonly above: HTMLElement;
  private readonly below: HTMLElement;
  private spaceAbove = -1;
  private spaceBelow = -1;
  private readonly sizes: ResizeObserver;
  private laying = 0;
  /** Gaps asked to be read, so a gap near the reader is asked once. */
  private asked = new Set<string>();
  /** Rows of calls that started one of the host's own agents, drawn in
   *  open stretches, brought up to date in place as their agents move; and
   *  the boxes open under them. */
  private agentRows = new Map<string, HTMLElement>();
  /** What each such row was last brought up to date with. */
  private agentDrawn = new WeakMap<HTMLElement, { run: AgentRun | null; stamp: number; call: number }>();
  private boxes = new Map<string, AgentBox>();
  private agentsSeen = -1;
  /** Stretches the reader closed, in a view whose stretches stand open. */
  private closedStretches = new Set<string>();

  constructor(
    private readonly root: HTMLElement,
    private readonly conversation: Conversation,
    private readonly context: Context,
  ) {
    root.classList.add("transcript");
    root.dataset.view = this.mode;
    this.above = document.createElement("div");
    this.below = document.createElement("div");
    this.above.className = this.below.className = "transcript-space";
    root.append(this.above, this.below);
    this.findContent = new FindContent({
      words: context.words,
      attachment: (path) => conversation.attachmentName(path),
      entry: (entry) => {
        const slot = this.slots.get(entry.key);
        if (slot?.signature !== null && slot?.element.isConnected) {
          const root = entry.kind === "heard" && this.noticeOf(entry) ? slot.element.querySelector<HTMLElement>(".notice-line") : slot.prose?.element;
          return displayedText(root ?? slot.element).text;
        }
        if (entry.kind === "prose") {
          const text = this.findContent.prose(entry.text, entry.writing);
          return entry.prose === "plan" ? `${context.words.plan}\n${text}` : text;
        }
        return displayedTextHtml(this.entry(entry));
      },
      rows: (run) => this.rowsOf(run),
      cards: (tool) => cardsOf(tool, (entry) => this.deedsOf(entry), (job) => this.jobCommand(job)),
      tokens: (run) => conversation.thinkingTokens(run),
      agents: {
        of: (tool) => conversation.agentOf(tool),
        named: (tool) => this.agentsObject(tool, null),
      },
      notice: (entry) => !!this.noticeOf(entry),
      write: (key, signal) => this.findWrite(key, signal),
    });
    this.layoutAnchor = new AnchorLayout(context.scroller);
    this.line = new TurnLine(context.words);
    // A block drawn grows or shrinks (its pictures arrive, it is opened, the
    // column narrows): its height is kept, and what is above the reader
    // moving moves the view by as much.
    this.sizes = new ResizeObserver((entries) => this.resized(entries));
    context.scroller.addEventListener("scroll", this.scrolled, { passive: true });
    // The reader moving the view themselves during a change of view lets go
    // of the words it holds.
    for (const kind of ["wheel", "touchstart", "pointerdown", "keydown"]) {
      context.scroller.addEventListener(kind, () => (this.steady = null), { passive: true });
    }
    // What happens in an agent's box under one of its rows is that box's
    // own view's to answer.
    root.addEventListener("click", (event) => {
      if (this.own(event)) this.click(event);
    });
    // On a touch screen the latest answer has its actions under it; an
    // earlier answer's, and the user's own words', are not shown, and held,
    // they float up beside them.
    onHeld(root, ".bubble", (bubble, event, input) => {
      if (input !== "touch" || !this.own(event)) return false;
      const acts = bubble.closest<HTMLElement>(".block")?.querySelector<HTMLElement>(".user-acts");
      if (!acts || (event.target as HTMLElement).closest("a, button, pre")) return false;
      return this.heldMenu(bubble, acts);
    });
    onHeld(root, ".block", (block, event, input) => {
      if (input !== "touch" || !this.own(event)) return false;
      const acts = block.querySelector<HTMLElement>(":scope > .answer-acts:not(.latest), :scope > .message-acts > .answer-acts:not(.latest)");
      // Code, links and pictures keep the system's own long press.
      if (!acts || (event.target as HTMLElement).closest("a, button, pre, code, img, .katex, .turn-acts")) return false;
      return this.heldMenu(block, acts);
    });
    // Enter in an answer's field answers, as the card's first button would.
    root.addEventListener("keydown", (event) => {
      if (!this.own(event)) return;
      const field = (event.target as HTMLElement).closest(".ask-card .ask-input");
      if (!field || event.key !== "Enter" || composing(event)) return;
      event.preventDefault();
      field.closest(".ask-card")?.querySelector<HTMLButtonElement>(".ask-primary")?.click();
    });
  }

  /** Whether an event happened in this view, not in the view of an
   *  agent's box drawn inside it. */
  private own(event: Event): boolean {
    return (event.target as HTMLElement).closest(".transcript") === this.root;
  }

  /** The drawn element of a block, where it is on the page now. */
  elementOf(key: string): HTMLElement | null {
    const element = this.slots.get(key)?.element;
    return element?.isConnected ? element : null;
  }

  /** Brings the page up to the conversation as it now stands; `read` when
   *  what is new came as history rather than as it happened. */
  update(read = false): void {
    const blocks = this.mode === "said" ? this.conversation.chatBlocks() : this.conversation.blocks();
    this.reading = read;
    // Words already on their way when the page is first drawn (the first
    // words of a conversation just begun) rose where they were sent.
    if (!this.live) for (const block of blocks) if (block.kind === "heard" && block.sending) this.flown.add(block.key);
    this.adopt(blocks);
    const drawn = this.lay(read);
    this.unfolding.clear();
    const says = this.context.nested ? null : this.lineSays(blocks);
    // Words rising this frame land before the line beneath them is drawn.
    const flying = this.flights(blocks);
    // The line stands before the first words waiting below it, else last;
    // only while the foot is drawn.
    const end = this.shown[this.shown.length - 1];
    if (says && end && end.key === blocks[blocks.length - 1]?.key) {
      let lineBefore: HTMLElement = this.below;
      for (let index = this.shown.length - 1; index >= 0; index -= 1) {
        const block = this.shown[index]!.block;
        if (!(block.kind === "heard" && block.sending !== undefined && block.intoTurn)) break;
        lineBefore = this.shown[index]!.element;
      }
      this.line.say(says);
      this.line.place(this.root, lineBefore, this.live && !read, flying ? time(getComputedStyle(this.root), "--send") * 0.6 : 0);
    } else this.line.remove();
    this.live = true;
    this.agentsMoved();
    this.tick();
    this.prefetch();
    for (const element of drawn) void drawDiagrams(element).then(() => {
      const slot = this.byElement.get(element);
      if (slot && element.querySelector(".diagram.drawn")) slot.findRevision = (slot.findRevision ?? 0) + 1;
    });
  }

  /** Takes in the blocks as they now stand: when blocks came or went, the
   *  heights are laid out again, and what went is let go. */
  private adopt(blocks: Block[]): void {
    const old = this.blocks;
    this.blocks = blocks;
    let same = old.length === blocks.length;
    for (let index = 0; same && index < blocks.length; index += 1) same = old[index]!.key === blocks[index]!.key;
    if (same) return;
    const present = new Set<string>();
    for (const block of blocks) present.add(block.key);
    for (const [key, slot] of this.slots) {
      if (present.has(key)) continue;
      this.hide(slot);
      this.slots.delete(key);
      this.measured.delete(key);
      this.guesses.delete(key);
      if (this.findRead.delete(key)) this.findRemoved.add(key);
    }
    this.shown = this.shown.filter((slot) => present.has(slot.key));
    for (const key of this.asked) if (!present.has(key)) this.asked.delete(key);
    this.heights.reset(
      blocks.map((block) => block.key),
      (key, index) => this.measured.get(key) ?? this.guess(blocks[index]!, index),
    );
  }

  /** The blocks near the reader, drawn and in order between the two
   *  spaces; the rest let go. Says which were drawn again. Unless `moved`
   *  (the view was just put somewhere), the block the reader is at stays
   *  where it is on the screen, whatever came or went above it. */
  private lay(read = false, moved = false): HTMLElement[] {
    const scroller = this.context.scroller;
    const n = this.blocks.length;
    const following = this.context.following?.() ?? true;
    // Where the reader is, before anything moves: across a change of
    // view, the block both views show that the reader was at.
    const pinned = this.pin;
    this.pin = null;
    const anchor = following || moved ? null : (pinned ?? this.anchorNow());
    // An agent's box grows with what it holds, up to a height: it is laid
    // out for the tallest it can be, not for how tall it is so far.
    const view = this.context.nested ? Math.max(scroller.clientHeight, innerHeight) : scroller.clientHeight || innerHeight;
    const origin = this.origin();
    let top = following ? Math.max(0, this.heights.total() - view) : scroller.scrollTop - origin;
    const at = anchor ? this.heights.index(anchor.slot.key) : undefined;
    if (anchor && at !== undefined) {
      // Laid out as the heights now say, the reader's block is here: the
      // view goes there first, and is set right once it is drawn.
      top = this.heights.top(at) - (anchor.top - scroller.getBoundingClientRect().top);
      scroller.scrollTop = origin + top;
    }
    let first = n ? this.heights.find(Math.max(0, top - view * ABOVE)) : 0;
    let last = n ? this.heights.find(top + view * (1 + BELOW)) : -1;
    // A block the reader has focus or a selection in stays, near enough.
    for (const slot of this.shown) {
      if (!this.holding(slot)) continue;
      const index = this.heights.index(slot.key);
      if (index === undefined) continue;
      if (index < first && first - index <= HOLD) first = index;
      if (index > last && index - last <= HOLD) last = index;
    }
    const wanted: Slot[] = [];
    for (let index = first; index <= last; index += 1) wanted.push(this.slotOf(this.blocks[index]!));
    const keep = new Set(wanted);
    for (const slot of this.shown) if (!keep.has(slot)) this.hide(slot);
    const drawn: HTMLElement[] = [];
    let previous: Element = this.above;
    for (let at = 0; at < wanted.length; at += 1) {
      const slot = wanted[at]!;
      const index = first + at;
      this.spacing(slot, index);
      if (previous.nextElementSibling !== slot.element) previous.after(slot.element);
      previous = slot.element;
      if (slot.signature === null) {
        this.draw(slot, read);
        // Its whole box: the room above it, its padding, is its height too.
        this.sizes.observe(slot.element, { box: "border-box" });
      } else if (slot.signature !== this.signature(slot.block)) {
        this.draw(slot, read);
        drawn.push(slot.element);
      }
    }
    this.foldIn(wanted.filter((slot) => this.unfolding.has(slot.key)));
    this.shown = wanted;
    const above = n ? this.heights.top(first) : 0;
    const below = n ? this.heights.total() - this.heights.top(last + 1) : 0;
    if (above !== this.spaceAbove) {
      this.above.style.height = `${above}px`;
      this.spaceAbove = above;
    }
    if (below !== this.spaceBelow) {
      this.below.style.height = `${below}px`;
      this.spaceBelow = below;
    }
    if (anchor) {
      const moved = anchor.slot.element.isConnected ? anchor.slot.element.getBoundingClientRect().top - anchor.top : 0;
      if (moved) this.keepStill(moved);
    }
    // A gap near the reader is read, from the side the reader comes from.
    const reader = following ? n : this.heights.find(Math.max(0, top));
    for (const slot of wanted) {
      const block = slot.block;
      if (block.kind !== "gap" || block.loading || this.asked.has(block.key)) continue;
      this.asked.add(block.key);
      const index = this.heights.index(block.key) ?? 0;
      this.context.near?.(block, index <= reader ? "end" : "start");
    }
    return drawn;
  }

  /** Lays out again where the reader scrolled, once a frame. */
  private scrolled = (): void => {
    if (this.laying) return;
    this.laying = requestAnimationFrame(() => {
      this.laying = 0;
      if (this.root.isConnected && this.scrollNeedsLayout()) this.lay();
    });
  };

  /** Scrolling inside the mounted range changes no blocks or spacing. A
   * changed range or spacer still takes the full anchored layout path. */
  private scrollNeedsLayout(): boolean {
    if (this.pin || this.unfolding.size) return true;
    const n = this.blocks.length;
    if (!n) return this.shown.length > 0;
    const scroller = this.context.scroller;
    const following = this.context.following?.() ?? true;
    const view = this.context.nested ? Math.max(scroller.clientHeight, innerHeight) : scroller.clientHeight || innerHeight;
    const anchor = following ? null : this.anchorNow();
    const at = anchor ? this.heights.index(anchor.slot.key) : undefined;
    const top = anchor && at !== undefined
      ? this.heights.top(at) - (anchor.top - scroller.getBoundingClientRect().top)
      : following ? Math.max(0, this.heights.total() - view) : scroller.scrollTop - this.origin();
    let first = this.heights.find(Math.max(0, top - view * ABOVE));
    let last = this.heights.find(top + view * (1 + BELOW));
    for (const slot of this.shown) {
      if (!this.holding(slot)) continue;
      const index = this.heights.index(slot.key);
      if (index === undefined) continue;
      if (index < first && first - index <= HOLD) first = index;
      if (index > last && index - last <= HOLD) last = index;
    }
    return this.shown.length !== last - first + 1
      || this.shown[0]?.key !== this.blocks[first]?.key
      || this.shown[this.shown.length - 1]?.key !== this.blocks[last]?.key
      || this.spaceAbove !== this.heights.top(first)
      || this.spaceBelow !== this.heights.total() - this.heights.top(last + 1);
  }

  /** Heights that changed under blocks drawn: kept, and what changed above
   *  the reader moves the view by as much. */
  private resized(entries: ResizeObserverEntry[]): void {
    const scroller = this.context.scroller;
    const following = this.context.following?.() ?? true;
    const reader = following ? Infinity : this.heights.find(Math.max(0, scroller.scrollTop - this.origin()));
    let moved = 0;
    for (const entry of entries) {
      const slot = this.byElement.get(entry.target as HTMLElement);
      if (!slot || !slot.element.isConnected || slot.signature === null) continue;
      const index = this.heights.index(slot.key);
      if (index === undefined) continue;
      const height = entry.borderBoxSize[0]?.blockSize ?? (entry.target as HTMLElement).offsetHeight;
      this.measured.set(slot.key, height);
      const delta = this.heights.set(index, height);
      if (index < reader) moved += delta;
    }
    if (following) {
      this.holdFoot();
      return;
    }
    this.foot = null;
    // Across a change of view, the words held stay where they stood.
    const held = this.steady;
    if (held?.slot.element.isConnected) {
      const words = wordsOf(held.slot.element);
      const went = words.getBoundingClientRect().top - held.top;
      if (went) this.keepStill(went);
      return;
    }
    if (moved) this.keepStill(moved);
  }

  /** Following, the foot stays where it is on the screen: the page scrolls
   *  to it by the whole pixels its end moved down the page, as it stands
   *  now, the spaces for what is not drawn with it, and the fraction is
   *  carried here. What is shorter than the view stands from its top, and
   *  nothing is carried. */
  private holdFoot(): void {
    const scroller = this.context.scroller;
    const end = this.below.getBoundingClientRect().bottom - scroller.getBoundingClientRect().top + scroller.scrollTop;
    const was = this.trim;
    if (this.foot !== null && end !== this.foot && scroller.scrollHeight > scroller.clientHeight) this.carry(end - this.foot);
    this.foot = end + this.trim - was;
  }

  /** What is kept in view moved `by` pixels down the screen, the room
   *  above it changed: the view follows it by as much, so it stays. */
  private keepStill(by: number): void {
    const scroller = this.context.scroller;
    const whole = this.whole(by);
    const from = scroller.scrollTop;
    if (whole) scroller.scrollTop = from + whole;
    // Where the view is at an end and the scroll took less, the fraction is
    // not carried either: it would move what the scroll could not.
    if (scroller.scrollTop - from === whole) this.carry(by);
  }

  /** The whole pixels of `by` a scroll is to take, the fraction carried
   *  as it stands taking the rest. */
  private whole(by: number): number {
    return Math.ceil(by - this.trim - 1e-4);
  }

  /** Takes in `by` pixels that what is drawn moved down the page, the whole
   *  pixels of it scrolled: the fraction left is carried in the root's top
   *  margin, within a pixel, the root never above its own place. */
  private carry(by: number): void {
    this.trim = Math.max(0, Math.round((this.trim + this.whole(by) - by) * 64) / 64);
    this.root.style.marginTop = this.trim ? `${this.trim}px` : "";
  }

  /** The first block drawn and in view whose foot is below the top of the
   *  view, and where its top is; none when the view was moved where
   *  nothing is drawn. */
  private anchorNow(): { slot: Slot; top: number } | null {
    const view = this.context.scroller.getBoundingClientRect();
    for (const slot of this.shown) {
      if (slot.block.kind === "gap" || !slot.element.isConnected) continue;
      const box = slot.element.getBoundingClientRect();
      if (box.bottom > view.top) return box.top < view.bottom ? { slot, top: box.top } : null;
    }
    return null;
  }

  /** Shows only what was said, or every step (`world-voice.md`, 两种看法),
   *  over one glide. What both show stays where it is on the screen: the
   *  agent's words to the user take their ground or leave it, the user's
   *  slide out to face them or back, and the work between them unfolds or
   *  folds away around them (`say.css`). */
  showing(mode: "said" | "steps"): void {
    this.changing?.();
    if (mode === this.mode) return;
    const glide = reducedMotion() || !this.root.isConnected ? 0 : msOf(getComputedStyle(this.root).getPropertyValue("--glide"));
    if (!glide) {
      this.root.dataset.view = mode;
      this.switchTo(mode);
      return;
    }
    // The words both views show that the reader is at are held, unless the
    // view follows the foot or stands at the very top, where what opens
    // above them is in view and pushes them down instead.
    const kept = this.keptInView(new Set(this.conversation.chatBlocks().map((block) => block.key)));
    const free = (this.context.following?.() ?? true) || this.context.scroller.scrollTop <= 0;
    const held = kept && !free ? { slot: kept.slot, top: wordsOf(kept.slot.element).getBoundingClientRect().top } : null;
    this.steady = held;
    if (mode === "steps") {
      // The work comes in folded and unfolds; the words leave their ground
      // as it does, and their own shape once it has.
      this.root.classList.add("view-leaving");
      this.switchTo("steps", true);
      this.after(glide, held, () => {
        this.root.classList.remove("view-leaving");
        this.root.dataset.view = "steps";
        for (const element of this.root.querySelectorAll<HTMLElement>(":scope > .view-fold")) unfold(element);
      });
    } else {
      // The words take their shape at once, their ground unseen yet, and it
      // comes up as the work folds away; the work goes once it has.
      this.root.dataset.view = "said";
      this.foldAway(new Set(this.conversation.chatBlocks().map((block) => block.key)));
      this.after(glide, held, () => this.switchTo("said"));
    }
  }

  /** Ends the change of view under way after `ms`, or at once when
   *  another comes first; the words it held are let go once what it ended
   *  with has been laid out. */
  private after(ms: number, held: { slot: Slot; top: number } | null, end: () => void): void {
    const timer = window.setTimeout(() => this.changing?.(), ms);
    this.changing = () => {
      window.clearTimeout(timer);
      this.changing = null;
      end();
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          if (this.steady === held) this.steady = null;
        }),
      );
    };
  }

  /** Blocks a change of view brought in near the reader, drawn: folded at
   *  once, their room known first and held by none of it, and unfolding in
   *  the same moment as the rest of the change, along the same curve, so
   *  whatever moves round them moves one way. */
  private foldIn(slots: Slot[]): void {
    if (!slots.length) return;
    const sizes = slots.map((slot) => roomOf(slot.element));
    slots.forEach((slot, at) => {
      foldable(slot.element, sizes[at]!);
      slot.element.classList.add("view-shut");
      const index = this.heights.index(slot.key);
      if (index === undefined) return;
      this.measured.set(slot.key, 0);
      this.heights.set(index, 0);
    });
    // Shut as styled, then let go: the unfolding starts now.
    void getComputedStyle(slots[0]!.element).height;
    for (const slot of slots) slot.element.classList.remove("view-shut");
  }

  /** The work a change of view takes away, near the reader: folding away
   *  where it stands, in the same moment and along the same curve as the
   *  rest of the change, to go once it has. */
  private foldAway(keep: Set<string>): void {
    const going = this.shown.filter((slot) => !keep.has(slot.key) && slot.element.isConnected);
    if (!going.length) return;
    const sizes = going.map((slot) => roomOf(slot.element));
    going.forEach((slot, at) => foldable(slot.element, sizes[at]!));
    void getComputedStyle(going[0]!.element).height;
    for (const slot of going) slot.element.classList.add("view-folded");
  }

  /** The first block in view that both views show, `kept` being the
   *  blocks of the view to come, and where its top stands. */
  private keptInView(kept: Set<string>): { slot: Slot; top: number } | null {
    const view = this.context.scroller.getBoundingClientRect();
    for (const slot of this.shown) {
      if (!kept.has(slot.key) || slot.block.kind === "gap" || !slot.element.isConnected) continue;
      const box = slot.element.getBoundingClientRect();
      if (box.bottom > view.top && box.top < view.bottom) return { slot, top: box.top };
    }
    return null;
  }

  /** Shows `mode`'s blocks at once, the block read at the top kept where it
   *  stands; `unfold`, what they bring in near the reader comes in folded. */
  private switchTo(mode: "said" | "steps", unfold = false): void {
    const next = mode === "said" ? this.conversation.chatBlocks() : this.conversation.blocks();
    this.pin = this.keptInView(new Set(next.map((block) => block.key)));
    const now = new Set(this.blocks.map((block) => block.key));
    this.unfolding = new Set(unfold ? next.filter((block) => !now.has(block.key)).map((block) => block.key) : []);
    this.mode = mode;
    this.update();
  }

  /** Where the first block would begin, in the scroll region's terms. */
  private origin(): number {
    const scroller = this.context.scroller;
    return this.above.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
  }

  /** Whether the reader has focus or a selection in a slot. */
  private holding(slot: Slot): boolean {
    if (slot.element.contains(document.activeElement)) return true;
    const selection = getSelection();
    return !!selection && selection.rangeCount > 0 && !selection.isCollapsed && selection.getRangeAt(0).intersectsNode(slot.element);
  }

  /** A block's place, made when it first comes near. */
  private slotOf(block: Block): Slot {
    let slot = this.slots.get(block.key);
    if (!slot) {
      const element = document.createElement("div");
      element.dataset.key = block.key;
      element.className = `block block-${block.kind === "activity-group" ? "activity" : block.kind}`;
      if (block.kind === "prose") element.classList.add(`prose-${block.prose}`);
      if (block.kind === "heard") {
        const notice = this.noticeOf(block);
        // In an agent's box, words it heard are what it was asked.
        element.classList.add(notice ? "from-world" : this.context.nested ? "from-asked" : !block.from || block.from.kind === "user" ? "from-user" : "from-other");
      }
      slot = { key: block.key, element, block, signature: null };
      this.slots.set(block.key, slot);
      this.byElement.set(element, slot);
    }
    slot.block = block;
    return slot;
  }

  /** The room above a block, as its own: none for the first, more where a
   *  turn begins, a line's space otherwise. The chat view's room goes by
   *  what that view shows before it, in either view, so a change of view
   *  moves from the one room to the other without a step (`say.css`). */
  private spacing(slot: Slot, index: number): void {
    const block = slot.block;
    const user = block.kind === "heard" && slot.element.classList.contains("from-user");
    const begins = (previous: Block | null | undefined) =>
      user || (previous?.kind === "ended" && !this.context.words.turn(previous.end.how, previous.end.reason));
    const previous = this.blocks[index - 1];
    slot.element.classList.toggle("first", index === 0);
    slot.element.classList.toggle("turn-start", index > 0 && begins(previous));
    const chat = this.context.nested ? undefined : this.conversation.chatPrevious(block.key);
    slot.element.classList.toggle("chat-first", chat === null);
    slot.element.classList.toggle("chat-turn", !!chat && begins(chat));
    // Where the agent answers the user's words, the other side begins.
    slot.element.classList.toggle("chat-answer", !!chat && chat.kind === "heard" && (!chat.from || chat.from.kind === "user") && !user);
  }

  /** Lets a block go from the page, keeping its height. */
  private hide(slot: Slot): void {
    this.sizes.unobserve(slot.element);
    this.forget(slot);
    slot.element.replaceChildren();
    slot.element.remove();
    slot.signature = null;
  }

  /** How tall a block not drawn is taken to be, with the room above it:
   *  what it holds at the column's width, and the room its turn's actions
   *  take beneath it where it has them (`estimate.ts`). */
  private guess(block: Block, index: number): number {
    const { line, chars } = this.metrics();
    const known = this.guesses.get(block.key);
    if (known && known.stamp === block.stamp && known.width === chars) return known.height;
    let height: number;
    if (block.kind === "gap") height = this.room().gap;
    else if (block.kind === "ended" && !this.context.words.turn(block.end.how, block.end.reason)) height = 0;
    else {
      const conversation = this.conversation;
      const turn = block.kind === "heard" || block.kind === "prose" ? block.turn : null;
      const has =
        !!turn &&
        conversation.turnEnded(turn) &&
        ((block.kind === "heard" && !block.notice && !block.sending && (!block.from || block.from.kind === "user")) ||
          (block.kind === "prose" && block.prose === "message" && conversation.lastMessageOf(turn) === block.key));
      const touch = matchMedia("(hover: none)").matches;
      const acts = !has ? 0 : !touch ? 32 : conversation.latestEnded() === turn ? 22 : 0;
      const room = this.room();
      const previous = this.blocks[index - 1];
      const start = (block.kind === "heard" && (!block.from || block.from.kind === "user") && !block.notice) || previous?.kind === "ended";
      height = estimate(block, line, chars, acts) + (index === 0 ? 0 : start ? room.turn : room.block);
    }
    this.guesses.set(block.key, { stamp: block.stamp, width: chars, height });
    return height;
  }

  /** The body's line height and how many of its characters a line of the
   *  column holds, for blocks not yet drawn. */
  private metrics(): { line: number; chars: number } {
    if (this.measuredMetrics) return this.measuredMetrics;
    const style = getComputedStyle(this.root);
    const size = parseFloat(style.fontSize) || 15;
    const line = size * (parseFloat(style.getPropertyValue("--leading-prose")) || 1.68);
    const width = this.root.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    const measured = { line, chars: Math.max(20, Math.floor((width > 0 ? width : size * 46) / (size * 0.55))) };
    if (width > 0) this.measuredMetrics = measured;
    return measured;
  }
  private measuredMetrics: { line: number; chars: number } | null = null;

  /** The room above a block and a gap's own height, as the column's style
   *  sets them. */
  private room(): { block: number; turn: number; gap: number } {
    if (this.rooms) return this.rooms;
    const style = getComputedStyle(this.root);
    const length = (name: string, fallback: number) => parseFloat(style.getPropertyValue(name)) || fallback;
    this.rooms = { block: length("--space-4", 16), turn: length("--space-8", 32), gap: length("--gap-row", 56) };
    return this.rooms;
  }
  private rooms: { block: number; turn: number; gap: number } | null = null;

  /** Changed display text, independently of which blocks are drawn. */
  findChanges(): FindChanges {
    const changed: FindChanges["changed"] = [];
    const order: string[] = [];
    for (const block of this.blocks) {
      if (block.kind === "gap") {
        order.push(`${GAP}${block.start}:${block.end}`);
        continue;
      }
      order.push(block.key);
      const slot = this.slots.get(block.key);
      const signature = `${this.findSignature(block)}|${slot?.findRevision ?? 0}`;
      if (this.findRead.get(block.key) === signature) continue;
      this.findRead.set(block.key, signature);
      const parts = this.findContent.parts(block);
      changed.push({ key: block.key, parts: block.kind === "activity-group" ? parts : (function* () {
        for (const part of parts) yield { ...part, stamp: `${signature}|${part.stamp}` };
      })() });
    }
    const removed = [...this.findRemoved];
    this.findRemoved.clear();
    return { order, changed, removed };
  }

  /** What finding reads of a block, as it would read it here. */
  findParts(block: Block): Iterable<FindPart> {
    return this.findContent.parts(block);
  }

  /** Draws and opens just the selected match's ancestry. Existing folds
   * and unrelated line viewports keep their disclosure and scroll state. */
  async revealFind(match: FindMatch): Promise<void> {
    const target = match.document.target;
    const slot = this.reveal(target.block, target.row, target.long !== undefined || target.selector !== undefined);
    if (!slot) return;
    if (target.row) await this.filling.get(target.row);
    if (target.long !== undefined) {
      const long = this.longs.get(target.row ?? slot.key)?.[target.long];
      const line = match.document.text.slice(0, match.start).split("\n").length - 1;
      if (long) await long.reveal(line);
    }
    for (let node = this.findRange(match)?.startContainer.parentElement; node && node !== slot.element; node = node.parentElement) {
      if (node instanceof HTMLDetailsElement) node.open = true;
    }
  }

  /** Brings the row of the call that started one of the host's own agents
   *  into view, as `openRow` does; for an agent another agent started, the
   *  row of the first one's call, whose box holds it. False where the
   *  conversation does not hold that row. */
  openAgent(id: string): boolean {
    let run = this.conversation.agent(id);
    let entry = run?.call ? this.conversation.call(run.call) : null;
    const seen = new Set<string>();
    while (run && !entry && run.parent && !seen.has(run.id)) {
      seen.add(run.id);
      run = this.conversation.agent(run.parent);
      entry = run?.call ? this.conversation.call(run.call) : null;
    }
    return !!entry && this.openRow(entry.key);
  }

  /** Brings a tool call's row, by its key, to the middle of the view and
   *  washes it a moment: the stretch it is folded in opens, the row itself
   *  stays as it was. False where the conversation does not hold that row. */
  openRow(key: string): boolean {
    const group = this.blocks.find((block) => block.kind === "activity-group" && block.members.some((member) => member.key === key));
    if (!group) return false;
    const slot = this.reveal(group.key, key, false);
    const row = slot?.stretch?.drawn.get(key)?.element;
    if (!row) return false;
    this.context.reading();
    const scroller = this.context.scroller;
    const box = row.getBoundingClientRect();
    const top = box.top - scroller.getBoundingClientRect().top;
    scroller.scrollTop += top - Math.max(0, (scroller.clientHeight - box.height) / 2);
    row.classList.remove("row-found");
    void row.offsetWidth;
    row.classList.add("row-found");
    return true;
  }

  /** Draws a block and, for a row of it, opens its stretch and shows the
   *  row; opens the row itself where asked. The block's slot, where it is
   *  laid out. */
  private reveal(key: string, rowKey: string | undefined, open: boolean): Slot | null {
    if (!this.bring(key)) return null;
    const slot = this.slots.get(key);
    if (!slot) return null;
    if (slot.signature === null) this.draw(slot);
    const stretch = slot.stretch;
    if (rowKey && stretch && slot.block.kind === "activity-group" && !this.openStretches.has(slot.key)) {
      this.openStretches.add(slot.key);
      stretch.head.setAttribute("aria-expanded", "true");
      this.seenOpen = false;
      this.rows(stretch, slot.block);
      this.seenOpen = true;
      this.motions.get(stretch.box)?.destroy();
      this.motions.delete(stretch.box);
      stretch.box.inert = false;
      stretch.box.style.height = "";
      slot.signature = this.signature(slot.block);
    }
    // A row among a long stretch's earlier rows: its rows shown back to it.
    if (rowKey && stretch && slot.block.kind === "activity-group" && !stretch.drawn.has(rowKey)) {
      const keys = runsOf(slot.block.members).flatMap((run) => this.rowsOf(run)).map((one) => one.key);
      const from = keys.indexOf(rowKey);
      if (from >= 0 && keys.length - from > (this.stretchRows.get(slot.key) ?? STRETCH_ROWS)) {
        this.stretchRows.set(slot.key, Math.ceil((keys.length - from) / STRETCH_ROWS) * STRETCH_ROWS);
        this.rows(stretch, slot.block);
      }
    }
    const element = rowKey === slot.key ? slot.element : rowKey ? stretch?.drawn.get(rowKey)?.element : undefined;
    if (element && rowKey && open && !this.openRows.has(rowKey)) {
      const button = element.querySelector<HTMLElement>("[data-open]");
      const box = element.querySelector<HTMLElement>(".fold-box");
      if (button && box) {
        this.openRows.add(rowKey);
        button.setAttribute("aria-expanded", "true");
        this.fill(rowKey, element);
        this.motions.get(box)?.destroy();
        this.motions.delete(box);
        box.inert = false;
        box.style.height = "";
      }
    }
    return slot;
  }

  /** Brings a block onto the page, the view moved to it where it was far:
   *  a third of the way down. False when it is not laid out. */
  bring(key: string): boolean {
    const index = this.heights.index(key);
    if (index === undefined) return false;
    const shown = this.slots.get(key);
    if (shown?.signature !== null && shown?.element.isConnected) return true;
    const scroller = this.context.scroller;
    this.context.reading();
    scroller.scrollTop = this.origin() + this.heights.top(index) - (scroller.clientHeight || innerHeight) / 3;
    this.lay(false, true);
    return this.slots.get(key)?.element.isConnected ?? false;
  }

  findRange(match: FindMatch): Range | null {
    const target = match.document.target;
    const slot = this.slots.get(target.block);
    if (!slot || slot.signature === null) return null;
    const row = target.row === slot.key ? slot.element : target.row ? slot.stretch?.drawn.get(target.row)?.element : slot.element;
    if (!row) return null;
    let element: HTMLElement | null = target.selector ? row.querySelector(target.selector) : row;
    let offsets: TextMatch = match;
    if (target.long !== undefined) {
      const long = this.longs.get(target.row ?? slot.key)?.[target.long];
      if (!long) {
        const note = row.querySelector<HTMLElement>(".io-note");
        return note ? displayedText(note).range(match) : null;
      }
      element = long.element;
      const before = match.document.text.slice(0, match.start);
      const line = before.split("\n").length - 1;
      const lines = element.querySelectorAll<HTMLElement>(".lv-line");
      if (lines.length) {
        element = element.querySelector<HTMLElement>(`[data-line="${line}"]`) ?? (lines[line]?.dataset.line === undefined ? lines[line]! : null);
        const start = match.start - before.lastIndexOf("\n") - 1;
        offsets = { start, end: start + match.end - match.start };
      }
    }
    return element ? displayedText(element).range(offsets) : null;
  }

  private async findWrite(key: string, signal: AbortSignal): Promise<string> {
    const split = key.indexOf(WRITE);
    const entry = this.conversation.get(key.slice(0, split));
    const write = entry?.kind === "tool" ? entry.tool.run?.writes.find((one) => one.id === key.slice(split + 1)) : undefined;
    if (!write?.diff) return "";
    const held = this.heldRows(key, write);
    if (held) return held.map((line) => displayedDiff(line, true, this.context.words.unchangedLines)).join("\n");
    if (!this.context.lines) return "";
    let raw: OpenedLines | null = null;
    try {
      raw = await this.context.lines(write.diff.machine, write.diff.path, { kind: "file", encoding: "utf-8" });
      const source = await RemoteDiff.open(raw, signal);
      const shown: string[] = [];
      const kept: string[] = [];
      // Use the same request size as RemoteDiff.find, rather than asking
      // the remote machine to materialize the entire change at once.
      for (let start = 0; start < source.extent.length; start += 2048) {
        const lines = await source.read(start, Math.min(2048, source.extent.length - start), signal);
        lines.forEach((line) => {
          kept.push(line);
          shown.push(displayedDiff(line, true, this.context.words.unchangedLines));
        });
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
        signal.throwIfAborted();
      }
      this.findWrites.set(key, { signature: JSON.stringify(write), lines: kept });
      return shown.join("\n");
    } catch (error) {
      if (signal.aborted) throw signal.reason;
      return gone(error) ? this.context.words.output.diffGone : this.context.words.output.readFailed;
    } finally {
      if (raw) await raw.close().catch((error: unknown) => console.warn("a change's lines could not be let go", error));
    }
  }

  /** Where the reader is: the first block whose foot is below the top of
   *  the view, and how far its top is above that. */
  anchor(): { key: string; offset: number } | null {
    const found = this.anchorNow();
    if (!found) return null;
    return { key: found.slot.key, offset: this.context.scroller.getBoundingClientRect().top - found.top };
  }

  /** Puts the reader back where `anchor` says they were; false when that
   *  block is not laid out. */
  restore(anchor: { key: string; offset: number }): boolean {
    const index = this.heights.index(anchor.key);
    if (index === undefined) return false;
    const scroller = this.context.scroller;
    scroller.scrollTop = this.origin() + this.heights.top(index) + anchor.offset;
    this.lay(false, true);
    const slot = this.slots.get(anchor.key);
    if (slot?.element.isConnected) scroller.scrollTop += slot.element.getBoundingClientRect().top - scroller.getBoundingClientRect().top + anchor.offset;
    return true;
  }

  /** How far, in screens, the blocks from `first` to `last` are from what
   *  is in view: 0 where any of them is in view; null where they are not
   *  laid out. */
  screensFrom(first: string, last: string): number | null {
    const from = this.heights.index(first);
    const to = this.heights.index(last);
    if (from === undefined || to === undefined) return null;
    const scroller = this.context.scroller;
    const view = scroller.clientHeight || innerHeight;
    const top = scroller.scrollTop - this.origin();
    const start = this.heights.top(from);
    const end = this.heights.top(to + 1);
    const apart = end < top ? top - end : start > top + view ? start - top - view : 0;
    return apart / view;
  }

  /** The key of the block the reader is at. */
  readerKey(): string | null {
    const found = this.anchorNow();
    return found?.slot.key ?? null;
  }

  /** Lets go of everything drawn, as the page is left for good. */
  destroy(): void {
    this.prefetching?.abort();
    for (const slot of this.slots.values()) this.forget(slot);
    this.slots.clear();
    this.findWrites.clear();
    this.sizes.disconnect();
    this.context.scroller.removeEventListener("scroll", this.scrolled);
    cancelAnimationFrame(this.laying);
    this.line.remove();
    clearTimeout(this.ticking);
  }

  /** Lets go of what a slot holds open. */
  private forget(slot: Slot): void {
    if (slot.stretch) {
      for (const row of slot.stretch.drawn.keys()) this.letGo(row);
      for (const clock of slot.stretch.head.querySelectorAll<HTMLElement>(".group-time")) this.clocks.delete(clock);
    }
    this.letGo(slot.key);
    delete slot.stretch;
    delete slot.prose;
    delete slot.acts;
  }

  /** What a block is drawn from: its stamp, and what it shows of its
   *  turn. */
  private signature(block: Block): string {
    const conversation = this.conversation;
    switch (block.kind) {
      case "activity-group":
        return `${block.stamp}|${block.members.length}|${this.openStretches.has(block.key) ? "o" : ""}${block.agents ? `|${conversation.agentStamp}` : ""}`;
      case "prose": {
        const last = block.prose === "message" && conversation.lastMessageOf(block.turn) === block.key;
        return `${block.stamp}|${last ? `l${conversation.turnEnded(block.turn) ? "e" : ""}${conversation.latestEnded() === block.turn ? "n" : ""}` : ""}${conversation.spokeIn(block.turn) ? "|s" : ""}`;
      }
      case "say": {
        const last = conversation.lastSayOf(block.turn) === block.key;
        return `${block.stamp}|${last ? `l${conversation.turnEnded(block.turn) ? "e" : ""}${conversation.latestEnded() === block.turn ? "n" : ""}` : ""}|${this.chatMomentSign(block.key)}`;
      }
      case "heard":
        return `${block.stamp}|${block.turn && conversation.turnEnded(block.turn) ? "e" : ""}${block.turn && conversation.latestEnded() === block.turn ? "n" : ""}|${this.chatMomentSign(block.key)}`;
      case "uncertain":
        return `${block.stamp}|${this.conversation.hasTurns() ? "t" : ""}`;
      case "forked":
        return `${block.stamp}|${this.forkSeed ? "s" : ""}`;
      default:
        return String(block.stamp);
    }
  }

  /** Where a fork came from, as the desk saw it as the fork was made: for
   *  a fork whose record does not hold the words it carries on from. */
  private forkSeed: ForkFrom | null = null;

  /** The moment said above a bubble in the chat view, where one is: the
   *  time today, yesterday's and the week's by their day, older ones by
   *  their date, as this device's clock reads the moment the machine
   *  recorded (`Conversation.chatTime`); the whole moment on a pointer's
   *  rest. Drawn in both views, folded away in the view of every step
   *  (`say.css`), so a change of view opens or closes it rather than
   *  drawing the block again. */
  private chatMoment(key: string): string {
    if (this.context.nested) return "";
    const time = this.conversation.chatTime(key);
    if (time === null) return "";
    const at = new Date(time);
    const { words } = this.context;
    return `<div class="chat-moment" title="${escape(words.chatMomentFull(at))}">${escape(words.chatMoment(at, new Date()))}</div>`;
  }

  private chatMomentSign(key: string): string {
    return this.context.nested ? "" : String(this.conversation.chatTime(key) ?? "");
  }

  seedFork(from: ForkFrom): void {
    this.forkSeed = from;
    this.update();
  }

  private findSignature(block: Block): string {
    switch (block.kind) {
      case "activity-group":
        return `${block.stamp}|${block.members.length}${block.agents ? `|${this.conversation.agentStamp}` : ""}`;
      case "uncertain":
        return `${block.stamp}|${this.conversation.hasTurns()}`;
      default:
        return String(block.stamp);
    }
  }

  /** Draws what a slot shows, as its block now stands. */
  private draw(slot: Slot, read = this.reading): void {
    const { block, element } = slot;
    const reading = this.reading;
    this.reading = read;
    try {
      slot.signature = this.signature(block);
      if (block.kind === "say") {
        this.letGo(block.key);
        element.innerHTML = this.chatMoment(block.key) + this.said(block);
        return;
      }
      if (block.kind === "gap") {
        element.innerHTML = this.gapRow(block);
        return;
      }
      if (block.kind === "activity-group") {
        this.stretch(slot, block);
        return;
      }
      if (block.kind === "prose" && block.prose !== "plan") {
        this.message(slot, block);
        return;
      }
      this.letGo(block.key);
      element.innerHTML = (block.kind === "heard" ? this.chatMoment(block.key) : "") + this.entry(block);
      // A plan long enough to scroll in its card hands the wheel on to the
      // conversation at its ends.
      for (const plan of element.querySelectorAll<HTMLElement>(".ask-plan")) handOnAtEdge(plan);
      if (this.openRows.has(block.key)) this.fill(block.key, element);
    } finally {
      this.reading = reading;
    }
  }

  /** Turns not held, as one quiet line: how many, before or between; while
   *  they are read, a mark that moves. */
  private gapRow(gap: Gap): string {
    const { words } = this.context;
    const said = gap.first ? words.turnsEarlier(gap.turns) : words.turnsBetween(gap.turns);
    return `<button class="gap-row${gap.loading ? " loading" : ""}" type="button" data-gap="${escape(gap.key)}"><span class="gap-line" aria-hidden="true"></span><span class="gap-said">${escape(said)}</span><span class="gap-line" aria-hidden="true"></span></button>`;
  }

  /** A message: its words, drawn as they grow, and once its turn is over,
   *  what can be done to the turn. */
  private message(slot: Slot, entry: ProseEntry): void {
    if (!slot.prose) {
      slot.prose = new StreamedProse();
      slot.acts = document.createElement("div");
      slot.acts.className = "message-acts";
      slot.element.replaceChildren(slot.prose.element, slot.acts);
    }
    // The hold's sentence is the World's, not the model's: said as the
    // interface says an allowance reached, its words a hover away.
    const held = entry.writing ? null : heldOf(entry.text);
    slot.prose.element.hidden = !!held;
    if (held) {
      slot.held ??= slot.element.insertBefore(document.createElement("div"), slot.acts!);
      const note = `<div class="note caution" title="${escape(entry.text)}">${escape(this.context.words.allowanceUsed(held))}</div>`;
      if (slot.held.innerHTML !== note) slot.held.innerHTML = note;
    } else {
      slot.held?.remove();
      delete slot.held;
      slot.prose.update(entry.text, entry.writing);
    }
    const acts = this.answerActs(entry);
    if (slot.acts!.innerHTML !== acts) slot.acts!.innerHTML = acts;
    // In a turn that said something to the user, its other words are its
    // working notes, in the quieter ink (`world-voice.md`).
    slot.element.classList.toggle("say-notes", this.conversation.spokeIn(entry.turn) && !this.answers(entry));
  }

  /** Words the agent said to the user: its answer, set as its words are,
   *  an urgent one marked in the margin, the files it showed with them
   *  below; under the turn's last, what can be done to the turn. */
  private said(say: Say): string {
    const { words } = this.context;
    const mark = say.status === "urgent" ? `<span class="say-mark" title="${escape(words.sayUrgent)}"><span aria-hidden="true">※</span><span class="spoken">${escape(words.sayUrgent)}</span></span>` : "";
    const nameOf = (file: { path: string }) => file.path.split(/[\\/]/).pop() || file.path;
    const picture = (file: { path: string }) => /\.(png|jpe?g|gif|webp|bmp)$/i.test(nameOf(file));
    const pictures = say.attachments.filter(picture);
    const others = say.attachments.filter((file) => !picture(file));
    // Pictures are drawn from the machine holding them, as an answer's are
    // (`pictures.ts`); other files open from there when pressed.
    const files =
      (pictures.length
        ? `<div class="say-pictures">${pictures
            .map((file) => `<span class="prose-picture"><img data-picture data-path="${escape(file.path)}" data-machine="${escape(file.machine)}" alt="${escape(nameOf(file))}" tabindex="0"></span>`)
            .join("")}</div>`
        : "") +
      (others.length
        ? `<ul class="say-files">${others
            .map((file) => `<li><a class="say-file" href="#" data-file="${escape(file.path)}" data-file-machine="${escape(file.machine)}">${glyph("file", 16)}<span class="say-file-name">${escape(nameOf(file))}</span></a></li>`)
            .join("")}</ul>`
        : "");
    const last = !this.context.nested && this.conversation.lastSayOf(say.turn) === say.key && this.conversation.turnEnded(say.turn) && !this.conversation.answersAfterSaying(say.turn);
    // The mark opens the first line, where it is read first.
    const body = render(say.text);
    const marked = mark && body.startsWith("<p>") ? `<p>${mark}${body.slice(3)}` : `${mark}${body}`;
    return `<div class="say say-${say.status}"><div class="prose">${marked}</div>${files}</div>${last ? this.turnActs(say.turn) : ""}`;
  }

  /** The session is no longer followed: whether its model still works is
   *  not known, and the line says nothing. */
  unfollowed(): void {
    this.followed = false;
  }
  private followed = true;

  /** Whether the conversation can be heard from at all: out of reach for
   *  good, or deleted, whether a turn goes on is not known, and the line
   *  says nothing until it can again. */
  hearing(can: boolean): void {
    this.unheard = !can;
  }
  private unheard = false;

  /** What the line at the foot says, or null when no turn is under way:
   *  what the model is on is said above, by the stretch that holds it. */
  private lineSays(blocks: readonly Block[]): LineSays | null {
    if (!this.followed || this.unheard) return null;
    const conversation = this.conversation;
    // Asked to stop, it is not said to go on while its host ends it.
    if (conversation.halting()) return null;
    const waiting = conversation.waiting();
    const busy = conversation.busy();
    // The first words are on their way, and nothing has come back.
    if (!conversation.spoke() && busy) {
      const spoken = blocks.some((block) => block.kind === "heard" && (!block.from || block.from.kind === "user") && !block.notice && block.sending?.state !== "failed");
      if (spoken) return { kind: "starting" };
    }
    if (!waiting && !busy) return null;
    let last: Block | undefined;
    for (let index = blocks.length - 1; index >= 0 && !last; index -= 1) {
      const block = blocks[index]!;
      if (!(block.kind === "heard" && block.sending !== undefined && block.intoTurn)) last = block;
    }
    // A card waiting at the end says what it waits for: the line does not
    // say it again.
    if (waiting) return { kind: "waiting", said: !(last?.kind === "ask" && last.state.state === "waiting") };
    return { kind: "working", since: conversation.turnBegan() };
  }

  /** Words just sent rise from the input into their place, once the page
   *  has laid them out; says whether any do. They are the last blocks. The
   *  page is told of the newest first, so it can bring them into view. */
  private flights(blocks: readonly Block[]): boolean {
    const departure = this.context.departure;
    let flying = false;
    let told = false;
    for (let index = blocks.length - 1; index >= 0 && index >= blocks.length - 8; index -= 1) {
      const block = blocks[index]!;
      if (block.kind !== "heard" || block.sending?.state !== "going" || this.flown.has(block.key)) continue;
      this.flown.add(block.key);
      if (!told) {
        told = true;
        this.context.sent?.(block.key);
      }
      if (!departure) continue;
      const from = departure();
      if (!from) continue;
      // Flown once the page has laid this frame out, before it is painted.
      queueMicrotask(() => fly(() => this.slots.get(block.key)?.element.querySelector<HTMLElement>(".bubble") ?? null, from));
      flying = true;
    }
    return flying;
  }

  private entry(entry: Entry): string {
    const { words } = this.context;
    switch (entry.kind) {
      case "heard":
        return this.heardEntry(entry);
      case "prose":
        if (entry.prose === "plan") {
          return `<div class="plan"><div class="plan-label">${escape(words.plan)}</div><div class="prose">${render(entry.text)}</div></div>`;
        }
        return `<div class="prose${entry.writing ? " writing" : ""}">${render(entry.text)}</div>${this.answerActs(entry)}`;
      case "steps":
        return `<ol class="steps">${entry.steps
          .map(
            (step) =>
              `<li class="step step-${step.status}"><span class="step-mark">${step.status === "done" ? glyph("check", 14) : ""}</span><span class="step-text">${escape(step.text)}</span></li>`,
          )
          .join("")}</ol>${entry.note ? `<p class="steps-note">${escape(entry.note)}</p>` : ""}`;
      case "compaction":
        return `<div class="divider"><span>${escape(words.compacted)}</span></div>`;
      case "other":
        return `<div class="note faint">${escape(entry.hostKind)}</div>`;
      case "ended": {
        const said = words.turn(entry.end.how, entry.end.reason);
        if (!said) return "";
        return `<div class="note ${entry.end.how === "failed" ? "wrong" : "caution"}">${escape(said)}</div>`;
      }
      case "health":
        return `<div class="note caution" title="${escape(entry.health.text)}">${escape(healthSaid(entry.health, this.context.words))}</div>`;
      case "uncertain":
        // Words that never began a conversation: it could not start, and
        // why, as it was given, where the first words wait.
        if (!this.conversation.hasTurns()) {
          const why = words.startWhy(entry.why);
          return `<div class="start-failed" title="${escape(entry.why)}"><p class="start-failed-said">${escape(words.startFailed)}</p>${why ? `<p class="start-failed-why">${escape(why)}</p>` : ""}</div>`;
        }
        return `<div class="note caution" title="${escape(entry.why)}">${escape(words.uncertain(entry.why))}</div>`;
      case "ask":
        return this.ask(entry);
      case "forked": {
        // Where it came from, a card at its head standing for what it
        // carried over; its words, where its record does not hold them, as
        // the desk saw them when it was made.
        const named = this.context.named?.(entry.source) ?? { name: entry.source.resident, agent: this.context.agent && /codex/i.test(this.context.agent) ? "codex" : "claude" };
        const seed = this.forkSeed;
        const from: ForkFrom = { name: named.name, agent: named.agent, said: entry.said ?? seed?.said ?? null, answer: entry.answer ?? seed?.answer ?? null };
        return `<div class="fk-from" data-fork-source="${escape(`${entry.source.resident}@${entry.source.machine}`)}"${entry.through ? ` data-fork-turn="${escape(entry.through)}"` : ""}>${forkCard(from, words.backTo(named.name))}</div>`;
      }
      case "agent-told":
        return this.agentTold(entry);
      case "command":
        return this.command(entry);
      case "tool":
      case "activity":
      // Said as words to the user, among the blocks (`askedSay`).
      case "question":
        return "";
    }
  }

  /** A command the user gave: a breathing mark while it runs; then what
   *  came of it, in place, or nothing where the conversation shows it
   *  already (`world-gui.md`, 对话). */
  private command(entry: Extract<Entry, { kind: "command" }>): string {
    const { words } = this.context;
    const row = entry.row;
    const name = controlName(row);
    const line = (said: string, mark: string, failed: boolean, title = "") =>
      `<div class="notice-line command-line${failed ? " failed" : ""}"${title ? ` title="${escape(title)}"` : ""}><span class="notice-mark${failed ? " wrong" : ""}">${mark}</span><span class="notice-said">${escape(said)}</span></div>`;
    const cross = glyph("cross", 14);
    if (row.state === "running") return line(words.commandRunning(name), `<span class="pulse"></span>`, false);
    if (row.state === "failed") return line(words.commandFailed(name), cross, true, row.error ?? "");
    if (row.state === "uncertain") return line(words.commandUnsure(name), glyph("minus", 14), false, row.error ?? "");
    if (row.state !== "completed") return "";
    const result = row.result;
    const control = row.control;
    switch (result?.kind) {
      // The host's own words, as it said them.
      case "refused":
        return line(result.said, cross, true);
      case "rewound":
        return line(words.rewound(result.files_restored), glyph("check", 14), false);
      case "fork_incomplete":
        return line(words.forkPartial, cross, true, result.why);
      case "side_answer": {
        const reply = result.reply !== null && typeof result.reply === "object" && !Array.isArray(result.reply) ? result.reply : {};
        const text = typeof reply.text === "string" ? reply.text : JSON.stringify(result.reply, null, 2);
        return `<div class="prose command-answer">${render(text)}</div>`;
      }
      // What shows itself: the model chosen, the fork opened.
      case "selected":
      case "model_menu":
      case "forked":
      case "agents":
      case "agent_messages":
        return "";
    }
    // The compaction's line, the new name, the turn a review or a skill
    // starts: each shows itself.
    const shows = ["compact", "fork", "rename", "model", "review"].includes(name) || (control.do === "command" && Boolean(control.path));
    if (shows) return "";
    const said = (result?.kind === "done" || result?.kind === "reported" ? result.said : null)?.trim();
    // A short word of the World's is in the hover; a host's output, opened.
    if (!said || (said.length <= 80 && !said.includes("\n"))) return line(words.commandRan(name), glyph("check", 14), false, said ?? "");
    const open = this.openRows.has(entry.key);
    return `<div class="notice-line command-line"><button class="notice-open" type="button" data-open="${escape(entry.key)}" aria-expanded="${open}"><span class="notice-mark">${glyph("check", 14)}</span><span class="notice-said">${escape(words.commandRan(name))}</span>${glyph("chevron", 14)}</button></div><div class="fold-box notice-box"${open ? "" : ' style="height:0" inert'}><div class="fold-content"></div></div>`;
  }

  /** One of the host's own agents that went on in the background ended,
   *  told here: one quiet line, what it handed back folded under it. */
  private agentTold(entry: Extract<Entry, { kind: "agent-told" }>): string {
    const { words } = this.context;
    const run = this.conversation.agent(entry.agent);
    const task = entry.end.task ?? run?.label() ?? null;
    const ok = entry.end.how === "completed";
    const said = toldSaid(entry.end, task, words);
    const line = `<span class="notice-mark${ok ? "" : " wrong"}">${glyph(ok ? "check" : "cross", 14)}</span><span class="notice-said">${escape(said)}</span>`;
    const more = entry.end.answer || entry.end.reason;
    if (!more) return `<div class="notice-line${ok ? "" : " failed"}">${line}</div>`;
    const open = this.openRows.has(entry.key);
    return `<div class="notice-line${ok ? "" : " failed"}"><button class="notice-open" type="button" data-open="${escape(entry.key)}" aria-expanded="${open}">${line}${glyph("chevron", 14)}</button></div><div class="fold-box notice-box"${open ? "" : " style=\"height:0\" inert"}><div class="fold-content"></div></div>`;
  }

  /** Whether a message is the last its turn said, where what was done to
   *  the turn is offered. */
  private lastOfTurn(entry: ProseEntry): boolean {
    return entry.prose === "message" && this.conversation.lastMessageOf(entry.turn) === entry.key && (!this.conversation.spokeIn(entry.turn) || this.conversation.answersAfterSaying(entry.turn));
  }

  /** Whether a message answers the user though its turn spoke through
   *  `message_user`: the final words of a turn that went on working after
   *  it last spoke (`Conversation.answersAfterSaying`). */
  private answers(entry: ProseEntry): boolean {
    return entry.prose === "message" && this.conversation.lastMessageOf(entry.turn) === entry.key && this.conversation.answersAfterSaying(entry.turn);
  }

  /** The turn whose actions stay shown: the last that ended. */
  private latest(turn: string): boolean {
    return this.conversation.latestEnded() === turn;
  }

  /** Under a turn's last message, once the turn is over: copy its answer,
   *  ask again, fork from here, and what the turn took. The latest turn's
   *  stay shown; earlier ones come with the pointer. */
  private answerActs(entry: ProseEntry): string {
    if (this.context.nested || entry.writing || !this.lastOfTurn(entry) || !this.conversation.turnEnded(entry.turn)) return "";
    return this.turnActs(entry.turn);
  }

  /** What can be done to a turn that ended, under what it last said. */
  private turnActs(turn: string): string {
    const entry = { turn };
    const { words } = this.context;
    const ended = this.conversation.get(`e:${entry.turn}`);
    const info = ended?.kind === "ended" ? turnInfo(ended.end, words, this.context.english) : [];
    const icon = (act: string, name: Parameters<typeof glyph>[0], label: string, extra = "") =>
      `<button class="turn-icon" type="button" ${act} aria-label="${escape(label)}" title="${escape(label)}"${extra}>${glyph(name, 16)}</button>`;
    return `<div class="turn-acts answer-acts${this.latest(entry.turn) ? " latest" : ""}">${icon(`data-copy-turn="${escape(entry.turn)}"`, "copy", words.copyAnswer)}${icon(
      `data-turn-act="regenerate" data-turn="${escape(entry.turn)}"`,
      "regenerate",
      words.regenerate,
      " data-needs=\"rewind\"",
    )}${icon(`data-turn-act="fork" data-turn="${escape(entry.turn)}"`, "fork", words.fork, " data-needs=\"fork\"")}${
      info.length
        ? `<span class="turn-icon turn-info" tabindex="0" aria-label="${escape(info.map(({ label, text }) => `${label} ${text}`).join("; "))}">${glyph("info", 16)}<span class="turn-info-card" role="tooltip">${info
            .map(({ label, text, html }) => `<span class="turn-info-label">${escape(label)}</span><span>${html ?? escape(text)}</span>`)
            .join("")}${this.context.turnCalls ? `<button class="turn-info-calls" type="button" data-turn-calls="${escape(entry.turn)}">${escape(words.turnInfo.requests)}</button>` : ""}</span></span>`
        : ""
    }</div>`;
  }

  /** What the user attached to their words, above them: pictures, one
   *  larger or several in a row, and the other files as chips. */
  private attached(items: ShownAttachment[]): string {
    if (!items.length) return "";
    const pictures = items.filter((item) => item.image && (item.preview || item.kept || item.recorded));
    const files = items.filter((item) => !pictures.includes(item));
    const picture = (item: ShownAttachment) =>
      `<li class="bubble-picture"><img data-picture ${item.preview ? `src="${escape(item.preview)}"` : `data-attachment="${escape(JSON.stringify(item.kept ?? item.recorded))}"`} alt="${escape(item.name)}"${item.name ? ` title="${escape(item.name)}"` : ""} tabindex="0"></li>`;
    const file = (item: ShownAttachment) => {
      const label = `${glyph(item.image ? "image" : "file", 16)}<span class="bubble-file-name">${escape(item.name)}</span>${item.size === null ? "" : `<span class="bubble-file-size">${escape(sizeOf(item.size))}</span>`}`;
      return item.kept
        ? `<li><a class="bubble-file" href="#" data-file="${escape(item.kept.path)}" data-file-machine="${escape(item.kept.owner.machine)}" data-attachment="${escape(JSON.stringify(item.kept))}">${label}</a></li>`
        : `<li class="bubble-file">${label}</li>`;
    };
    return `${pictures.length ? `<ul class="bubble-attached pictures${pictures.length === 1 ? " one" : ""}">${pictures.map(picture).join("")}</ul>` : ""}${
      files.length ? `<ul class="bubble-attached">${files.map(file).join("")}</ul>` : ""
    }`;
  }

  /** A request as its card: what it asks, and the answers it takes. */
  private ask(entry: AskEntry): string {
    const asks = this.context.asks;
    if (!asks) return `<div class="note caution">${escape(this.context.words.waitingForYou)}: ${escape(entry.request)}</div>`;
    const ask = readAsk(entry.request, entry.params, asks.words);
    // A change waiting for approval is the edit already shown above: its
    // files are named from there.
    if (ask.kind === "files" && ask.item && !ask.paths.length && entry.turn) {
      const tool = this.conversation.get(`i:${entry.turn}:${ask.item}`);
      if (tool?.kind === "tool" && tool.tool.does.as === "edit") ask.paths = tool.tool.does.changes.map((change) => change.path);
    }
    return drawAsk(entry.key, ask, entry.state, this.context.residentMachine, asks.words);
  }

  /** A turn's actions as a sheet risen from the bottom, each with its
   *  name: a press does what the mark beside the turn would. */
  /** The user's words held on a touch screen: their actions as a small
   *  menu beside them, each doing what its mark does. */
  /** The actions of the words held, as a menu beside them. */
  private heldMenu(bubble: HTMLElement, acts: HTMLElement): boolean {
    const buttons = [...acts.querySelectorAll<HTMLButtonElement>("button.turn-icon")];
    const shown = buttons.filter((button) => !(button.dataset.needs && !this.root.classList.contains(`can-${button.dataset.needs}`)));
    if (!shown.length) return false;
    heldMenu.open(
      bubble,
      "",
      shown.map((button, index) => ({ act: String(index), icon: heldIcon(button), label: button.getAttribute("aria-label") ?? "" })),
      (act) => shown[Number(act)]?.click(),
      true,
    );
    return true;
  }

  /** The user chose in a card: an option is marked, an answer is sent. */
  private async choose(target: HTMLElement): Promise<void> {
    const asks = this.context.asks;
    const card = target.closest<HTMLElement>(".ask-card");
    if (!asks || !card) return;
    if (target.classList.contains("ask-option")) {
      const pressed = target.getAttribute("aria-pressed") !== "true";
      if (!target.dataset.many) {
        const group = `[data-field="${target.dataset.field}"]`;
        for (const other of card.querySelectorAll<HTMLElement>(`.ask-option${group}`)) {
          other.setAttribute("aria-pressed", "false");
          other.classList.remove("chosen");
        }
      }
      target.setAttribute("aria-pressed", String(pressed));
      target.classList.toggle("chosen", pressed);
      return;
    }
    const found = target.dataset.ask ? this.conversation.get(target.dataset.ask) : undefined;
    const entry = found?.kind === "ask" ? found : undefined;
    if (!entry || entry.state.state !== "waiting") return;
    const ask = readAsk(entry.request, entry.params, asks.words);
    const given = target.dataset.decision !== undefined ? { decision: Number(target.dataset.decision) } : { submit: target.dataset.submit ?? "" };
    const status = card.querySelector<HTMLElement>(".ask-status")!;
    const answer = answerOf(card, ask, given, asks.words);
    if ("missing" in answer) {
      status.textContent = answer.missing;
      status.classList.add("wrong");
      return;
    }
    const controls = [...card.querySelectorAll<HTMLButtonElement | HTMLInputElement>("button, input")];
    for (const control of controls) control.disabled = true;
    card.classList.add("busy");
    status.classList.remove("wrong");
    status.textContent = asks.words.sending;
    try {
      if (ask.kind === "link" && given.submit === "open") await asks.open(ask.url);
      await asks.answer(entry.id, answer.answer);
      this.conversation.settle(entry, { state: "answered", said: answer.said });
      this.update();
      asks.settled();
    } catch (error) {
      for (const control of controls) control.disabled = false;
      card.classList.remove("busy");
      status.textContent = asks.words.failed(detailOf(error));
      status.title = detailOf(error);
      status.classList.add("wrong");
    }
  }

  /** The notice a heard entry carries, when it is one the page draws as a
   *  line of its own: the World's facts, a colleague's answer or trouble.
   *  Words another agent wrote stay words; a job's end is not drawn. */
  private noticeOf(entry: HeardEntry): Notice | null {
    const notice = entry.notice;
    if (!notice || entry.from?.kind === "user") return null;
    if (notice.kind === "words") return notice.goal_report?.shown || notice.goal_question ? notice : null;
    return notice.kind === "answer" || notice.kind === "health" || notice.kind === "question" || notice.kind === "world" ? notice : null;
  }

  private heardEntry(entry: HeardEntry): string {
    // In an agent's box, what it was asked, and what its spawner said to it
    // after: a quiet block, its first lines until opened.
    if (this.context.nested) {
      const open = this.openRows.has(entry.key);
      const long = entry.text.length > 220 || entry.text.split("\n").length > 3;
      return `<div class="agent-asked${open ? " open" : ""}"><div class="agent-asked-text">${plain(entry.text)}</div>${
        long ? `<button class="agent-asked-more" type="button" data-asked="${escape(entry.key)}">${escape(open ? this.context.words.agents.less : this.context.words.agents.more)}</button>` : ""
      }</div>`;
    }
    const notice = this.noticeOf(entry);
    if (notice) return this.noticeLine(entry, notice);
    const mine = !entry.from || entry.from.kind === "user";
    const acts = mine && entry.turn && !entry.sending && this.conversation.turnEnded(entry.turn) ? this.userActs(entry) : "";
    // Words sent into the turn under way are said: the turn takes them in
    // where it next can, and only a failure is told.
    const sending = entry.intoTurn && entry.sending?.state !== "failed" ? null : (entry.sending ?? null);
    return this.attached(entry.attached ?? []) + this.heard(entry.from, entry.text, sending, entry.key) + acts;
  }

  /** Under the user's own words once their turn is over: copy them, or
   *  take them back to change and send again. */
  private userActs(entry: HeardEntry): string {
    const { words } = this.context;
    return `<div class="turn-acts user-acts${entry.turn && this.latest(entry.turn) ? " latest" : ""}"><button class="turn-icon" type="button" data-copy="${escape(entry.key)}" aria-label="${escape(words.copyMessage)}" title="${escape(words.copyMessage)}">${glyph("copy", 16)}</button><button class="turn-icon" type="button" data-turn-act="rewind" data-turn="${escape(entry.turn!)}" data-key="${escape(entry.key)}" data-needs="rewind" aria-label="${escape(words.rewind)}" title="${escape(words.rewind)}">${glyph("edit", 16)}</button></div>`;
  }

  /** A notice as one quiet line on the left, a mark before it; what it
   *  holds (last lines, an answer) folded under it until opened. */
  private noticeLine(entry: HeardEntry, notice: Notice): string {
    const { words } = this.context;
    let said: string;
    let ok = true;
    let more = "";
    // Why, where the line itself cannot say it: under the pointer.
    let why = "";
    switch (notice.kind) {
      case "answer": {
        const failed = notice.end.how === "failed";
        ok = !failed;
        said = failed ? words.answerFailed(notice.label) : words.answered(notice.label, notice.end.duration_ms ? words.took(notice.end.duration_ms) : null);
        more = notice.end.last_message ? "answer" : "";
        break;
      }
      case "health":
        ok = false;
        said = `${notice.label}: ${healthSaid(notice.health, words)}`;
        break;
      case "question":
        said = words.colleagueAsks(notice.text.trim().split("\n")[0] ?? "");
        more = "text";
        break;
      case "words": {
        // A colleague's goal settled, or it asks on the side.
        const report = notice.goal_report?.shown;
        if (report) {
          ok = report.status !== "blocked";
          said = words.goalReported(report.label, report.status, words.took(Date.parse(report.ended_at) - Date.parse(report.started_at)));
          more = report.text.trim() ? "text" : "";
          break;
        }
        const asked = askedOf(notice.goal_question?.questions);
        said = asked.length ? words.colleagueAsks(asked[0]!.ask) : words.colleagueAsked;
        more = asked.length ? "text" : "";
        break;
      }
      case "world": {
        const fact = notice.fact;
        switch (fact.fact) {
          case "moved":
            said = fact.to ? words.movedTo(fact.to) : words.movedOut;
            more = "text";
            break;
          case "focus":
            said = focusSaid(fact.focus, words);
            more = "text";
            break;
          case "woke":
            // What its reflex saw opens under the line.
            said = words.woke(fact.why);
            ok = fact.why.kind !== "broken";
            more = notice.text.trim() ? "text" : "";
            break;
          case "desk_free":
            said = words.deskFree(fact.by.resident);
            break;
          case "desk_taken":
            said = words.deskTaken(fact.by.resident);
            break;
          case "desk_outside":
            said = words.deskOutside;
            break;
          case "delivery_failed": {
            ok = false;
            const gone = undelivered(notice.text);
            said = !gone ? words.undelivered : gone.kind === "end" ? words.endUndelivered(gone.machine) : words.stopUndelivered(gone.machine);
            why = gone ? undeliveredWhy(gone.why, words) : notice.text.trim();
            break;
          }
          default:
            said = "";
        }
        break;
      }
      default:
        said = "";
    }
    const asks = notice.kind === "question" || (notice.kind === "words" && !notice.goal_report);
    const paused = notice.kind === "words" && notice.goal_report?.shown?.status === "paused";
    const woke = notice.kind === "world" && notice.fact.fact === "woke";
    const mark = woke ? glyph("sentinel", 14) : asks ? glyph("ask", 14) : paused ? glyph("pause", 14) : ok ? glyph("check", 14) : glyph("cross", 14);
    const line = `<span class="notice-mark${ok ? "" : " wrong"}">${mark}</span><span class="notice-said">${escape(said)}</span>`;
    if (!more) return `<div class="notice-line${ok ? "" : " failed"}"${why ? ` title="${escape(why)}"` : ""}>${line}</div>`;
    const open = this.openRows.has(entry.key);
    return `<div class="notice-line${ok ? "" : " failed"}"><button class="notice-open" type="button" data-open="${escape(entry.key)}" aria-expanded="${open}">${line}${glyph("chevron", 14)}</button></div><div class="fold-box notice-box"${open ? "" : " style=\"height:0\" inert"}><div class="fold-content"></div></div>`;
  }

  /** What a notice holds, opened: a colleague's answer, what the World
   *  handed over. */
  private noticeDetails(entry: HeardEntry, into: HTMLElement): void {
    const notice = entry.notice;
    if (!notice) return;
    if (notice.kind === "answer" && notice.end.last_message) {
      into.innerHTML = `<div class="prose notice-answer">${render(notice.end.last_message)}</div>`;
    } else if (notice.kind === "world") {
      into.innerHTML = `<div class="told-plain">${plain(entry.text)}</div>`;
    } else if (notice.kind === "question") {
      into.innerHTML = `<div class="told-plain">${plain(notice.text)}</div>`;
    } else if (notice.kind === "words" && notice.goal_report?.shown) {
      into.innerHTML = `<div class="prose notice-answer">${render(notice.goal_report.shown.text)}</div>`;
    } else if (notice.kind === "words" && notice.goal_question) {
      into.innerHTML = askedHtml(askedOf(notice.goal_question.questions));
    }
  }

  private heard(from: Speaker | null, text: string, sending: SendState | null, key?: string): string {
    const { words } = this.context;
    if (!from || from.kind === "user") {
      // The user's own words; which machine they came from is said only
      // when it is not this one. Words not yet heard say how they stand.
      const who =
        from && from.machine !== this.context.here ? `<div class="bubble-from">${escape(words.you(from.machine))}</div>` : "";
      const state = sending && sending.state !== "settled" ? ` ${sending.state}` : "";
      let note = "";
      if (sending?.state === "failed") note = `<div class="bubble-note wrong" title="${escape(sending.why)}">${escape(words.notSent(sending.why))}</div>`;
      if (sending?.state === "waiting") note = `<div class="bubble-note" title="${escape(sending.why)}">${escape(words.waitingToSend)}</div>`;
      if (sending?.state === "landed") {
        note = `<div class="bubble-note"${sending.why ? ` title="${escape(sending.why)}"` : ""}>${escape(words.landed(sending.how, sending.machine, sending.why ?? null))}</div>`;
      }
      if (sending?.state === "queued") note = sending.memory
        ? `<div class="bubble-note" title="${escape(words.queuedMemoryWhy)}">${escape(words.queuedMemory(sending.ahead))}</div>`
        : `<div class="bubble-note" title="${escape(words.queuedTurnWhy)}">${escape(words.queuedTurn(sending.ahead))}</div>`;
      // Still on their way: their host has not started for a minute.
      if (sending?.state === "going" && sending.retry) {
        note = `<div class="bubble-note" title="${escape(words.hostRetryWhy(sending.retry))}">${escape(words.hostRetrying(this.context.agent))}</div>`;
      }
      // A long message shows its first lines, the rest a press away.
      if (key && longWords(text)) {
        const open = this.unfolded.has(key);
        return `${who}<div class="bubble long${open ? " open" : ""}${state}" data-own-press><div class="bubble-text">${plain(text)}</div><button class="bubble-more" type="button" data-unfold="${escape(key)}" aria-expanded="${open}">${escape(open ? words.bubbleLess : words.bubbleMore)}</button></div>${note}`;
      }
      return `${who}<div class="bubble${state}" data-own-press>${plain(text)}</div>${note}`;
    }
    // Words from another resident, or the World's own telling from a node
    // too old to say it as facts: never in the user's bubble. The head notes
    // written for the model are said again in the interface's words.
    const world = from.kind === "world";
    const told = readTold(text);
    let label = world ? words.world : from.kind === "page" ? words.aPage(from.page.page) : `${from.address.resident} ${from.address.machine}`;
    let how = "";
    let body: string;
    switch (told.kind) {
      case "job":
        body = `<p class="told-line">${escape(words.jobEnded(told.job, told.machine))}<span class="told-how" title="${escape(told.how)}">${escape(words.ended(told.how))}</span></p><p class="told-path">${escape(told.output)}</p>`;
        break;
      case "colleague":
        label = `${told.colleague} ${told.machine}`;
        how = words.ended(told.how);
        body = `<div class="prose">${render(told.body)}</div>`;
        break;
      default: {
        const said = world ? worldSaid(told.body) : null;
        if (said) body = this.worldSaid(said);
        else body = world ? `<div class="told-plain">${plain(told.body)}</div>` : `<div class="prose">${render(told.body)}</div>`;
      }
    }
    const caption = how ? `<span class="relayed-how">${escape(how)}</span>` : "";
    return `<div class="relayed${world ? " relayed-world" : ""}"><div class="relayed-from">${glyph(world ? "web" : "colleague", 16)}<span>${escape(label)}</span>${caption}</div><div class="relayed-text">${body}</div></div>`;
  }

  /** The World's own words, in the interface's: a move as one line with the
   *  brief it handed over folded under it, the desk as who holds it. */
  private worldSaid(said: WorldSaid): string {
    const { words } = this.context;
    switch (said.kind) {
      case "moved":
        return `<p class="told-line">${escape(said.town ? words.movedTo(said.town) : words.movedOut)}</p>${
          said.brief ? `<details class="told-more"><summary>${escape(words.briefHanded)}</summary><div class="told-plain">${plain(said.brief)}</div></details>` : ""
        }`;
      case "desk_free":
        return `<p class="told-line">${escape(words.deskFree(said.who))}</p>`;
      case "desk_taken":
        return `<p class="told-line">${escape(words.deskTaken(said.who))}</p>`;
      case "desk_outside":
        return `<p class="told-line">${escape(words.deskOutside)}</p>`;
    }
  }

  /** A stretch, made once and brought up to date in place: its heading's
   *  words, and, while it is open, its rows, each drawn again only when it
   *  changed. */
  private stretch(slot: Slot, group: Activity): void {
    let stretch = slot.stretch;
    if (!stretch) {
      if (this.context.nested && !this.closedStretches.has(group.key)) this.openStretches.add(group.key);
      const open = this.openStretches.has(group.key);
      slot.element.innerHTML = `<button class="group-head" type="button" data-fold="${escape(group.key)}" aria-expanded="${open}"><span class="group-mark">${glyph("thinking", 16)}</span><span class="group-title"></span><span class="group-time"></span>${glyph("chevron", 16)}</button><div class="fold-box group-box"${open ? "" : ' style="height:0" inert'}><div class="rows"></div></div>`;
      stretch = {
        head: slot.element.querySelector<HTMLButtonElement>(".group-head")!,
        box: slot.element.querySelector<HTMLElement>(".group-box")!,
        rows: slot.element.querySelector<HTMLElement>(".rows")!,
        drawn: new Map(),
        heading: "",
      };
      slot.stretch = stretch;
    }
    // Its heading says how the agents its calls started stand, as they move.
    const heading = `${group.stamp}|${group.members.length}${group.agents ? `|${this.conversation.agentStamp}` : ""}`;
    if (stretch.heading !== heading) {
      stretch.heading = heading;
      this.heading(stretch, group);
    }
    if (this.openStretches.has(group.key)) this.rows(stretch, group);
  }

  /** A stretch's rows as its members now stand: thinking with nothing
   *  between is one row, and a stretch that is only thinking holds its
   *  words, its heading having said the rest. */
  private rows(stretch: Stretch, group: Activity): void {
    const runs = runsOf(group.members);
    let rows: Row[];
    let hidden = 0;
    if (thoughtOnly(group)) rows = [this.thoughtWords(runs[0] as ProseEntry[])];
    else {
      // What it did, a row a thing, what goes by alike gathered; a stretch
      // of hundreds of rows opens on its latest, the earlier coming a few
      // screens at a time from a line above them.
      const all = bunched(
        runs.flatMap((run) => this.partsOf(run)),
        (part): part is Card => !("html" in part),
      ).map((part) => ("html" in part ? part : this.lineRow(part)));
      const limit = this.stretchRows.get(group.key) ?? STRETCH_ROWS;
      rows = all.length > limit ? all.slice(all.length - limit) : all;
      hidden = all.length - rows.length;
    }
    let earlier = stretch.rows.querySelector<HTMLButtonElement>(":scope > .rows-earlier");
    if (hidden > 0) {
      if (!earlier) {
        earlier = document.createElement("button");
        earlier.type = "button";
        earlier.className = "rows-earlier";
        earlier.dataset.earlier = group.key;
        stretch.rows.prepend(earlier);
      }
      const said = this.context.words.stepsEarlier(hidden);
      if (earlier.textContent !== said) earlier.textContent = said;
    } else {
      earlier?.remove();
      earlier = null;
    }
    const present = new Set<string>();
    let previous: HTMLElement | null = earlier;
    for (const row of rows) {
      present.add(row.key);
      let drawn = stretch.drawn.get(row.key);
      if (!drawn || drawn.signature !== row.signature) {
        const holder = document.createElement("div");
        holder.innerHTML = row.html();
        const fresh = holder.firstElementChild as HTMLElement;
        if (drawn) {
          this.letGo(row.key);
          drawn.element.replaceWith(fresh);
        }
        if (this.openRows.has(row.key)) this.fill(row.key, fresh);
        drawn = { signature: row.signature, element: fresh };
        stretch.drawn.set(row.key, drawn);
        if (fresh.classList.contains("row-agent")) this.agentRows.set(row.key, fresh);
      }
      // A row that started an agent moves with its call, as with its agent.
      if (row.signature === "agent") this.patchAgent(row.key, drawn.element);
      const want: Element | null = previous ? previous.nextElementSibling : stretch.rows.firstElementChild;
      if (want !== drawn.element) {
        if (previous) previous.after(drawn.element);
        else stretch.rows.prepend(drawn.element);
      }
      previous = drawn.element;
    }
    for (const [key, drawn] of stretch.drawn) {
      if (!present.has(key)) {
        this.letGo(key);
        drawn.element.remove();
        stretch.drawn.delete(key);
      }
    }
  }

  /** A stretch's heading, one line read from the left: while it runs, the
   *  step it is on and for how long; once done, what it did (the files it
   *  changed first, the lines added and removed right after them), what
   *  failed, and how long it took. A stretch that is only thinking says how
   *  long it thought and the tokens it took, and opens only where there are
   *  words. The words change in place; the line keeps its height. */
  private heading(stretch: Stretch, group: Activity): void {
    const { words } = this.context;
    const said = stretchSays(
      group,
      words,
      (path) => this.conversation.attachmentName(path),
      (run) => this.conversation.thinkingTokens(run),
      {
        of: (tool) => this.conversation.agentOf(tool),
        named: (tool) => this.agentsObject(tool, null),
      },
      (tool) => cardsOf(tool, (entry) => this.deedsOf(entry), (job) => this.jobCommand(job)),
    );
    const { parts, added, removed, running, only, bare, began } = said;
    const stats = added || removed ? `<span class="row-stats"><span class="add">+${added}</span><span class="del">−${removed}</span></span>` : "";
    stretch.head.classList.toggle("bare", bare);
    stretch.head.classList.toggle("thought", only);
    // Marked rather than disabled: a disabled control swallows the middle
    // button, and with it the page's autoscroll.
    if (bare) stretch.head.setAttribute("aria-disabled", "true");
    else stretch.head.removeAttribute("aria-disabled");
    const titleElement = stretch.head.querySelector<HTMLElement>(".group-title")!;
    // What it did is said in whole parts, so a narrow line ends on an
    // ellipsis after the last that fits.
    const drawn = parts
      .map((part, index) => `${index ? escape(words.tallyGap) : ""}<span class="group-part${part.kind === "failed" ? " failed" : ""}">${escape(part.said)}${part.kind === "edited" ? stats : ""}</span>`)
      .join("");
    if (titleElement.dataset.drawn !== drawn) {
      titleElement.dataset.drawn = drawn;
      // The step in hand changes in place, quietly; the line keeps its
      // height.
      titleElement.innerHTML = drawn;
    }
    stretch.head.classList.toggle("going", running);
    const time = stretch.head.querySelector<HTMLElement>(".group-time")!;
    if (running && began !== null) {
      time.dataset.since = String(began);
      this.clocks.add(time);
    } else {
      delete time.dataset.since;
      this.clocks.delete(time);
      if (time.textContent !== said.time) time.textContent = said.time;
    }
  }

  /** Keeps a long text with what else is held under `key`. */
  private held(key: string, long: LongText): void {
    const longs = this.longs.get(key);
    if (longs) longs.push(long);
    else this.longs.set(key, [long]);
  }

  /** What a card holds: a change of a file in its card, the deeds read
   *  from a run as theirs above the run's own words, folded; anything else
   *  in its own words. */
  private fillCard(key: string, thing: Card, content: HTMLElement): void {
    const longs: OpenedText[] = [];
    const closers: (() => void)[] = [];
    this.longs.set(key, longs);
    this.closers.set(key, closers);
    // The deeds read from one run are drawn together, under its own words.
    const deeds = new Map<string, number[]>();
    const order: (string | { write: string; entry: string } | { change: number; entry: string })[] = [];
    for (const source of thing.sources) {
      if ("deed" in source) {
        const kept = deeds.get(source.entry);
        if (kept) kept.push(source.deed);
        else {
          deeds.set(source.entry, [source.deed]);
          order.push(source.entry);
        }
      } else if ("write" in source || "change" in source) order.push(source);
      else if (!order.includes(source.entry)) order.push(source.entry);
    }
    for (const part of order) {
      const entry = this.conversation.get(typeof part === "string" ? part : part.entry);
      if (entry?.kind !== "tool") continue;
      if (typeof part !== "string" && "write" in part) {
        const write = entry.tool.run?.writes.find((one) => one.id === part.write);
        if (write) this.fillWrite(key, write, content);
        continue;
      }
      if (typeof part !== "string" && "change" in part) {
        if (entry.tool.does.as !== "edit") continue;
        const change = entry.tool.does.changes[part.change];
        if (!change) continue;
        const rows = parse(change.diff);
        const long = this.diffText(memoryDiff(rows).lines, change.path, numbered(rows));
        longs.push(long);
        const how = change.how;
        const { card, shown } = this.changeCard(change.path, entry.tool.machine ?? this.context.residentMachine ?? null, how === "added" ? "added" : how === "deleted" ? "deleted" : "modified");
        card.body.append(long.element);
        shown(long);
        content.append(card.element);
        continue;
      }
      if (startsAgent(entry.tool)) {
        this.fillAgent(entry, content);
        continue;
      }
      content.append(this.commandWords(entry, deeds.get(part) ?? null, longs, closers));
    }
  }

  /** Brings the running stretches' clocks up to date, once a second while
   *  any runs. */
  private tick(): void {
    const { words } = this.context;
    for (const clock of this.clocks) {
      if (!clock.isConnected || clock.dataset.since === undefined) {
        this.clocks.delete(clock);
        continue;
      }
      const said = clockSays(Number(clock.dataset.since), words, clock.dataset.steps === undefined ? undefined : Number(clock.dataset.steps));
      if (clock.textContent !== said) {
        clock.textContent = said;
        // What finding reads of its heading changed with it.
        const block = clock.closest<HTMLElement>(".block");
        const slot = block ? this.byElement.get(block) : undefined;
        if (slot) slot.findRevision = (slot.findRevision ?? 0) + 1;
      }
    }
    clearTimeout(this.ticking);
    if (this.clocks.size) this.ticking = window.setTimeout(() => this.tick(), 1000);
  }

  /** The rows a run of members stands for: thinking with nothing between
   *  is one row; a command's writes have one each, after its own, which a
   *  command that did nothing but write does not have. */
  private rowsOf(run: Entry[]): Row[] {
    return this.partsOf(run).map((part) => ("html" in part ? part : this.cardRow(part)));
  }

  /** What a run of members is drawn as: a call's cards, each thing it did
   *  in its order (`cards/`); rows for what is not drawn as cards. */
  private partsOf(run: Entry[]): (Row | Card)[] {
    const member = run[0]!;
    if (member.kind === "tool" && !startsAgent(member.tool)) {
      const cards = cardsOf(member, (entry) => this.deedsOf(entry), (job) => this.jobCommand(job));
      if (cards.length) return cards;
    }
    return this.rowsOfRun(run);
  }

  /** What a job this conversation started was running: the command of
   *  the run that went on as it. */
  private jobCommand(job: string): string | null {
    if (this.jobsRead !== this.blocks) {
      this.jobsRead = this.blocks;
      this.jobs.clear();
      for (const block of this.blocks) {
        const members = block.kind === "activity-group" ? block.members : [block];
        for (const member of members) {
          if (member.kind !== "tool" || member.tool.does.as !== "command" || !member.tool.does.job) continue;
          const said = scriptSaid(member.tool.does.command, member.tool.machine);
          this.jobs.set(member.tool.does.job, (said ?? member.tool.does.command).replace(/^on\s+\S+\s+/, ""));
        }
      }
    }
    return this.jobs.get(job) ?? null;
  }
  private jobs = new Map<string, string>();
  private jobsRead: Block[] | null = null;

  /** A thing as finding reads it, a row of its own. */
  private cardRow(card: Card): Row {
    this.cards.set(card.key, card);
    return { key: card.key, signature: `card|${JSON.stringify(card)}`, html: () => "", text: () => lineText(card, this.context.words) };
  }

  /** A thing, or things gathered, as their row (`cards/lines.ts`). */
  private lineRow(part: Card | Bunch): Row {
    if (part.kind === "bunch") {
      this.bunches.set(part.key, part);
      for (const card of cardsIn(part)) this.cards.set(card.key, card);
    } else this.cards.set(part.key, part);
    const words = this.context.words;
    return {
      key: part.key,
      signature: `line|${JSON.stringify(part)}`,
      html: () => lineHtml(part, { words, open: this.openRows.has(part.key), enter: this.entering(part.key) }),
      text: () => lineText(part, words),
    };
  }

  /** What things gathered hold, opened: their rows, each opening as its
   *  own. */
  private fillBunch(bunch: Bunch, content: HTMLElement): void {
    const words = this.context.words;
    const rows = document.createElement("div");
    rows.className = "rows row-bunched";
    rows.innerHTML = bunch.parts.map((part) => lineHtml(part, { words, open: this.openRows.has(part.key), enter: "" })).join("");
    content.append(rows);
    for (const part of bunch.parts) {
      if (part.kind === "bunch") this.bunches.set(part.key, part);
      if (!this.openRows.has(part.key)) continue;
      const row = rows.querySelector<HTMLElement>(`[data-row="${CSS.escape(part.key)}"]`);
      if (row) this.fill(part.key, row);
    }
  }

  private rowsOfRun(run: Entry[]): Row[] {
    const member = run[0]!;
    if (isThinking(member)) {
      const thoughts = run as ProseEntry[];
      return [{ key: member.key, signature: this.runSignature(thoughts), html: () => this.thinkingRow(thoughts, this.entering(member.key)), text: () => this.thinkingRowText(thoughts) }];
    }
    const own: Row = { key: member.key, signature: this.memberSignature(member), html: () => this.member(member), text: () => this.findRowText(member) };
    if (member.kind !== "tool") return [own];
    const writes = member.tool.run?.writes ?? [];
    if (!writes.length) return [own];
    return [...(onlyWrites(member.tool) ? [] : [own]), ...writes.map((write) => this.writeRowOf(member, write))];
  }

  /** A file a run wrote, as its row. */
  private writeRowOf(member: ToolEntry, write: WriteFact): Row {
    const key = `${member.key}${WRITE}${write.id}`;
    return {
      key,
      signature: JSON.stringify(write),
      html: () => this.writeRow(key, write),
      text: () => {
        const line = writeHeadline(write, this.context.words);
        const stats = line.added !== null && line.removed !== null ? `+${line.added} −${line.removed}` : "";
        return [line.action, line.object, stats, write.machine !== this.context.residentMachine ? this.context.words.on(write.machine) : ""].filter(Boolean).join(" ");
      },
    };
  }

  /** What a run of thinking is drawn from: its parts, and the tokens told
   *  for them. */
  private runSignature(run: readonly ProseEntry[]): string {
    return `${run.map((entry) => entry.stamp).join(",")}|${this.conversation.thinkingTokens(run) ?? ""}`;
  }

  /** The words of a stretch that is only thinking, drawn where its rows
   *  would be. */
  private thoughtWords(run: ProseEntry[]): Row {
    const key = `${run[0]!.key}~words`;
    return { key, signature: this.runSignature(run), html: () => `<div class="thinking-text prose" data-row="${escape(key)}">${render(thoughtText(run))}</div>`, text: () => "" };
  }

  /** What a row is drawn from. A row naming agents says who they are as
   *  they become known. A row that started one is drawn once and moved in
   *  place as its call and its agent do (`patchAgent`), so the box open
   *  under it stays as the agent works. */
  private memberSignature(entry: Entry): string {
    if (entry.kind !== "tool") return String(entry.stamp);
    if (startsAgent(entry.tool)) return "agent";
    if (entry.tool.does.as === "agent") return `${entry.stamp}a${this.conversation.agentStamp}`;
    return String(entry.stamp);
  }

  /** A run of thinking's line as its row shows it. */
  private thinkingRowText(run: readonly ProseEntry[]): string {
    const { words } = this.context;
    const writing = run.some((entry) => entry.writing);
    const took = writing ? null : thoughtTook(run);
    const titles = thoughtHeads(run);
    const title = writing ? titles.at(-1) : thoughtText(run).trim() ? titles[0] : undefined;
    const tokens = writing ? null : this.conversation.thinkingTokens(run);
    return [writing ? words.thinking : took === null ? words.thought : words.thoughtFor(took), title ?? "", tokens === null ? "" : words.thinkingTokens(tokens)].filter(Boolean).join(" ");
  }

  private findRowText(entry: Entry, said?: string): string {
    const { words } = this.context;
    if (entry.kind === "activity") return entry.text;
    if (entry.kind !== "tool") return "";
    const line = headline(entry.tool, entry.ended, words, (path) => this.conversation.attachmentName(path), true);
    if (said !== undefined) line.object = said;
    const tense = tenseOf(entry.ended);
    const stats = line.added !== null && line.removed !== null ? `+${line.added} −${line.removed}` : "";
    const where = entry.tool.machine && entry.tool.machine !== this.context.residentMachine ? words.on(entry.tool.machine) : "";
    const colleague = worldColleague(entry.tool) && this.context.link ? words.openColleague : "";
    if (startsAgent(entry.tool)) {
      const run = this.conversation.agentOf(entry);
      return [line.action, run?.kind ?? "", run?.task ?? line.object, this.agentMeta(entry, run), where].filter(Boolean).join(" ");
    }
    const object = entry.tool.does.as === "agent" ? this.agentsObject(entry, line.object) : line.object;
    const text = [line.action, object, stats, displayedTextHtml(this.outcome(entry, tense)), where].filter(Boolean).join(" ");
    return `${text}${colleague ? `\n${colleague}` : ""}`;
  }

  /** A file a run wrote: made, changed or removed, its lines added and
   *  taken away beside its name, the machine where it was not here; opened,
   *  the change with its line numbers. */
  private writeRow(key: string, write: WriteFact): string {
    const enter = this.entering(key);
    const { words } = this.context;
    const line = writeHeadline(write, words);
    const open = this.openRows.has(key);
    const expandable = write.diff !== null;
    const stats =
      line.added !== null && line.removed !== null
        ? `<span class="row-stats"><span class="add">+${line.added}</span><span class="del">−${line.removed}</span></span>`
        : "";
    const where = `<span class="row-machine">${glyph("machine", 12)}${escape(words.on(write.machine))}</span>`;
    const chevron = expandable ? `<span class="row-chevron">${glyph("chevron", 16)}</span>` : "";
    const fold = expandable ? `<div class="fold-box"${open ? "" : " style=\"height:0\" inert"}><div class="fold-content"></div></div>` : "";
    // A change that could not be read line by line says why on hover.
    const why = write.unavailable ? ` title="${escape(write.unavailable)}"` : "";
    return `<div class="row row-tool row-done${enter}" data-row="${escape(key)}"><span class="rail">${glyph(line.glyph)}</span><button class="row-line" type="button" ${expandable ? `data-open="${escape(key)}"` : 'aria-disabled="true"'} aria-expanded="${open}"${why}><span class="row-action">${escape(line.action)}</span><span class="row-rest"><span class="row-object">${escape(line.object ?? "")}</span><span class="row-meta">${stats}${where}${chevron}</span></span></button>${fold}</div>`;
  }

  /** Whether a row comes in as it is first drawn: once, for a row new to a
   *  conversation already drawn. A stretch opened for the first time shows
   *  its rows as they are. */
  private entering(key: string): string {
    const enter = this.live && this.seenOpen && !this.reading && !this.seen.has(key) ? " entering" : "";
    this.seen.add(key);
    return enter;
  }
  /** Rows are being drawn into a stretch already open. */
  private seenOpen = true;
  /** What is being drawn came as history. */
  private reading = false;

  /** A member's row; `said` names a run by what its script did of its own,
   *  where its other doings are rows of their own. */
  private member(entry: Entry, said?: string): string {
    if (entry.kind === "tool" && startsAgent(entry.tool)) return this.agentRow(entry);
    const enter = this.entering(entry.key);
    if (entry.kind === "activity") {
      return `<div class="row${enter}" data-row="${escape(entry.key)}"><span class="rail">${glyph("colleague")}</span><div class="row-line static"><span class="row-object">${escape(entry.text)}</span></div></div>`;
    }
    if (entry.kind !== "tool") return `<div data-row="${escape(entry.key)}"></div>`;
    const { words } = this.context;
    const line = headline(entry.tool, entry.ended, words, (path) => this.conversation.attachmentName(path), true);
    if (said !== undefined) line.object = said;
    const tense = tenseOf(entry.ended);
    const open = this.openRows.has(entry.key);
    const expandable = this.hasDetails(entry.tool);
    // Where it ran, this computer too: every script says where it runs.
    const where = entry.tool.machine ? `<span class="row-machine">${glyph("machine", 12)}${escape(words.on(entry.tool.machine))}</span>` : "";
    const stats =
      line.added !== null && line.removed !== null
        ? `<span class="row-stats"><span class="add">+${line.added}</span><span class="del">−${line.removed}</span></span>`
        : "";
    const mark = tense === "running" ? `<span class="pulse" aria-hidden="true"></span>` : glyph(line.glyph);
    const named = entry.tool.does.as === "agent" ? this.agentsObject(entry, line.object) : line.object;
    const object = named ? `<span class="row-object">${escape(named)}</span>` : "";
    const chevron = expandable ? `<span class="row-chevron">${glyph("chevron", 16)}</span>` : "";
    const fold = expandable ? `<div class="fold-box"${open ? "" : " style=\"height:0\" inert"}><div class="fold-content"></div></div>` : "";
    // A colleague it asked lives on in the World: its own conversation is a
    // step away.
    const colleague =
      worldColleague(entry.tool) && entry.tool.does.as === "colleague" && entry.tool.does.to && this.context.link
        ? `<a class="row-link" href="${escape(this.context.link({ resident: entry.tool.does.to, machine: entry.tool.machine ?? this.context.residentMachine }))}">${escape(words.openColleague)}${glyph("chevron", 14)}</a>`
        : "";
    // What it did and on what, its figures beside it, the machine it ran
    // on; how a command ended is in what it holds.
    const meta = `<span class="row-meta">${stats}${this.outcome(entry, tense)}${where}${chevron}</span>`;
    return `<div class="row row-tool row-${tense}${enter}" data-row="${escape(entry.key)}"><span class="rail">${mark}</span><button class="row-line" type="button" ${expandable ? `data-open="${escape(entry.key)}"` : 'aria-disabled="true"'} aria-expanded="${open}"><span class="row-action">${escape(line.action)}</span><span class="row-rest">${object}${meta}</span></button>${colleague}${fold}</div>`;
  }

  /** A call that started one of the host's own agents: what it was asked,
   *  its kind before it, how it stands after; under it while it works, the
   *  step it takes now; opened, what it did (`agent-box.ts`). The parts
   *  that move with the agent are brought up to date in place
   *  (`patchAgent`), so its box stays open as it works. */
  private agentRow(entry: ToolEntry): string {
    const enter = this.entering(entry.key);
    const { words } = this.context;
    const line = headline(entry.tool, entry.ended, words, (path) => this.conversation.attachmentName(path), true);
    const open = this.openRows.has(entry.key);
    const elsewhere =
      entry.tool.machine && entry.tool.machine !== this.context.residentMachine
        ? `<span class="row-machine">${glyph("machine", 12)}${escape(words.on(entry.tool.machine))}</span>`
        : "";
    return `<div class="row row-tool row-agent${enter}" data-row="${escape(entry.key)}"><span class="rail"></span><button class="row-line" type="button" data-open="${escape(entry.key)}" aria-expanded="${open}"><span class="row-action">${escape(line.action)}</span><span class="row-rest"><span class="row-object agent-object"><span class="agent-kind" hidden></span><span class="agent-task">${escape(line.object ?? "")}</span></span><span class="row-meta"><span class="agent-meta"></span>${elsewhere}<span class="row-chevron">${glyph("chevron", 16)}</span></span></span></button><div class="fold-box"${open ? "" : ' style="height:0" inert'}><div class="fold-content"></div></div></div>`;
  }

  /** How a call that started an agent stands: as its agent does, once the
   *  agent is known; as the call does, before. */
  private agentTense(entry: ToolEntry, run: AgentRun | null): "running" | "done" | "failed" {
    if (!run) return tenseOf(entry.ended);
    if (run.going) return "running";
    return run.state === "failed" ? "failed" : tenseOf(entry.ended) === "failed" ? "failed" : "done";
  }

  /** What stands after an agent's task: how many steps and for how long
   *  while it works; once ended, the same, or why it did not finish. */
  private agentMeta(entry: ToolEntry, run: AgentRun | null): string {
    const { words } = this.context;
    if (!run) return displayedTextHtml(this.outcome(entry, tenseOf(entry.ended)));
    if (run.state === "failed") return words.agents.failed;
    if (run.state === "stopped") return words.agents.stopped;
    if (run.state === "waiting") return words.agents.waiting;
    const steps = run.tools ?? run.steps.length;
    if (run.going) return run.began === null ? "" : clockSays(run.began, words, steps);
    const took = run.duration ?? (run.ended !== null && run.began !== null ? run.ended - run.began : null);
    return [steps ? words.steps(steps) : "", took !== null ? words.took(took) : ""].filter(Boolean).join(words.figureGap);
  }

  /** The agents a call names, as the row says them: each by its name, its
   *  kind or its task; how many, where the conversation does not know them;
   *  then what it said to them. */
  private agentsObject(entry: ToolEntry, said: string | null): string | null {
    if (entry.tool.does.as !== "agent") return said;
    const named = entry.tool.does.agents ?? [];
    if (!named.length) return said;
    const known = this.conversation.agentsNamed(entry);
    const who = known.length === named.length ? known.map((run) => run.name ?? run.kind ?? run.label()).join(this.context.words.listSeparator) : this.context.words.agents.unknownAgents(named.length);
    return said && entry.tool.does.act === "message" ? `${who}: ${said}` : who;
  }

  /** Brings a call's row up to date with its agent: its mark, how it
   *  stands, its kind and task, the step it takes now, and its open box. */
  private patchAgent(key: string, row: HTMLElement): void {
    const entry = this.conversation.get(key);
    if (entry?.kind !== "tool") return;
    const run = this.conversation.agentOf(entry);
    const drawn = this.agentDrawn.get(row);
    if (drawn && drawn.run === run && drawn.stamp === (run?.stamp ?? -1) && drawn.call === entry.stamp) return;
    this.agentDrawn.set(row, { run, stamp: run?.stamp ?? -1, call: entry.stamp });
    const tense = this.agentTense(entry, run);
    for (const name of ["row-running", "row-done", "row-failed"]) row.classList.toggle(name, name === `row-${tense}`);
    if (run) row.dataset.agent = run.id;
    const rail = row.querySelector<HTMLElement>(":scope > .rail")!;
    const mark = tense === "running" ? "pulse" : "glyph";
    if (rail.dataset.mark !== mark) {
      rail.dataset.mark = mark;
      rail.innerHTML = tense === "running" ? `<span class="pulse" aria-hidden="true"></span>` : glyph("delegate");
    }
    // Before what it was asked, what it is called: its name where it has
    // one, as Codex's are addressed, else its kind; its kind on the name.
    const kind = row.querySelector<HTMLElement>(".agent-kind")!;
    const kindOf = run?.kind ?? (entry.tool.does.as === "agent" ? entry.tool.does.kind : null);
    const kindSaid = run?.name ?? kindOf ?? "";
    if (kind.textContent !== kindSaid) kind.textContent = kindSaid;
    kind.hidden = !kindSaid;
    kind.classList.toggle("named", !!run?.name);
    const task = row.querySelector<HTMLElement>(".agent-task")!;
    const taskSaid = run?.task ?? headline(entry.tool, null, this.context.words).object ?? "";
    if (task.textContent !== taskSaid) task.textContent = taskSaid;
    const meta = row.querySelector<HTMLElement>(".agent-meta")!;
    if (run?.going && run.began !== null) {
      meta.dataset.since = String(run.began);
      meta.dataset.steps = String(run.tools ?? run.steps.length);
      this.clocks.add(meta);
      const said = clockSays(run.began, this.context.words, run.tools ?? run.steps.length);
      if (meta.textContent !== said) meta.textContent = said;
    } else {
      delete meta.dataset.since;
      this.clocks.delete(meta);
      const said = this.agentMeta(entry, run);
      if (meta.textContent !== said) meta.textContent = said;
    }
    meta.classList.toggle("wrong", tense === "failed");
    // One line while it works, as when it is done: what it does is seen
    // by opening it.
    if (run) this.boxes.get(key)?.update(run);
  }

  /** The host's own agents moved: the rows of the calls that started them
   *  are brought up to date in place. */
  private agentsMoved(): void {
    const stamp = this.conversation.agentStamp;
    if (stamp === this.agentsSeen) return;
    this.agentsSeen = stamp;
    for (const [key, row] of this.agentRows) {
      if (!row.isConnected) {
        this.agentRows.delete(key);
        continue;
      }
      this.patchAgent(key, row);
    }
  }

  /** A run of thinking as one row: "Thinking…" while it goes, "Thought for
   *  4s" once over where the host said how long, "Thought" where not; the title of its words'
   *  section after it, the tokens it took at the end. It opens only where
   *  the host gave words to show; none are made up. GPT's are titles only:
   *  the one it is in shows while it thinks, and nothing opens. */
  private thinkingRow(run: readonly ProseEntry[], enter: string): string {
    const { words } = this.context;
    const key = run[0]!.key;
    const text = thoughtText(run);
    const has = !!text.trim();
    const writing = run.some((entry) => entry.writing);
    const open = has && this.openRows.has(key);
    const took = writing ? null : thoughtTook(run);
    const said = writing ? words.thinking : took === null ? words.thought : words.thoughtFor(took);
    // A summary comes in sections, each under a bold title of the host's:
    // the row says the latest while it thinks, the first after where its
    // words can be opened.
    const titles = thoughtHeads(run);
    const title = writing ? titles.at(-1) : has ? titles[0] : undefined;
    const tokens = writing ? null : this.conversation.thinkingTokens(run);
    const figures = tokens === null ? "" : `<span class="row-figures">${escape(words.thinkingTokens(tokens))}</span>`;
    const chevron = has ? `<span class="row-chevron">${glyph("chevron", 16)}</span>` : "";
    const fold = has ? `<div class="fold-box"${open ? "" : ' style="height:0" inert'}><div class="fold-content"></div></div>` : "";
    return `<div class="row row-thinking${writing ? " row-running" : ""}${enter}" data-row="${escape(key)}"><span class="rail">${glyph("thinking")}</span><button class="row-line" type="button" ${has ? `data-open="${escape(key)}"` : 'aria-disabled="true"'} aria-expanded="${open}"><span class="row-said">${escape(said)}</span>${title ? `<span class="row-object">${escape(title)}</span>` : ""}<span class="row-meta">${figures}${chevron}</span></button>${fold}</div>`;
  }

  /** The run of thinking a row stands for, by the key of its first part. */
  private thoughtRun(key: string): ProseEntry[] | null {
    for (const block of this.conversation.blocks()) {
      if (block.kind !== "activity-group" || !block.members.some((member) => member.key === key)) continue;
      const run = runsOf(block.members).find((one) => one[0]!.key === key);
      return run && isThinking(run[0]!) ? (run as ProseEntry[]) : null;
    }
    return null;
  }

  private outcome(entry: ToolEntry, tense: string): string {
    const { words } = this.context;
    const { tool } = entry;
    // A command's ending is not an outcome of the row: a code other than
    // zero is often what was asked, and one that went on in the background
    // is counted beside the composer; both are said in what it holds.
    if (tool.does.as === "command" && (tool.does.job || tool.does.exit !== null)) return "";
    switch (entry.ended?.use) {
      case "cut":
        return `<span class="row-outcome">${escape(words.cut)}</span>`;
      case "unknown":
        return `<span class="row-outcome">${escape(words.unknown)}</span>`;
      case "declined":
        return `<span class="row-outcome wrong">${escape(words.declined)}</span>`;
      default:
        // The verb no longer says it failed; the row says it in words too.
        return tense === "failed" ? `<span class="row-outcome wrong">${escape(words.rowFailed)}</span>` : "";
    }
  }

  private hasDetails(tool: Tool): boolean {
    if (startsAgent(tool)) return true;
    if (tool.does.as === "edit") return tool.does.changes.length > 0;
    if (tool.does.as === "command") return true;
    const given = tool.input !== null && tool.input !== undefined && JSON.stringify(tool.input) !== "{}";
    return given || tool.output.length > 0;
  }

  private longWords(): LongTextWords {
    const { words, english } = this.context;
    return {
      more: words.moreLines,
      less: words.showLess,
      full: words.fullScreen,
      english,
      failed: (error) => (error instanceof Said ? error.message : words.output.readFailed),
    };
  }

  /** A change as rows to read: its numbers, its sign, its text. */
  private diffText(lines: readonly string[] | RemoteDiff, path: string, numberedRows: boolean, gap: (n: number) => string = this.context.words.unchangedLines, wrap = false): LongText {
    return new LongText(
      {
        lines,
        ...(Array.isArray(lines) ? { source: new DiffLines(lines as readonly string[]) } : {}),
        paint: diffPaint(path, gap, numberedRows),
        kind: "diff",
        scroll: true,
        wrap,
      },
      path,
      this.layoutAnchor,
      this.longWords(),
    );
  }

  /** A change's card, the rows put in it once there are some: `shown`
   *  has them put. */
  private changeCard(path: string, machine: string | null, kind: DiffKind): { card: DiffCard; shown: (long: LongText) => void } {
    const { words } = this.context;
    let held: LongText | null = null;
    const card = diffCard(path, machine, { copy: words.copyDiff, copied: words.copied }, async (signal) => unifiedDiff(path, kind, held ? await held.all(signal) : []));
    return {
      card,
      shown: (long) => {
        held = long;
        const full = long.takeFull();
        if (full) card.act(full);
      },
    };
  }

  /** What a write changed, read from the machine that keeps it, in its
   *  card: a few rows are read whole, more a screen at a time. */
  private fillWrite(key: string, write: WriteFact, into: HTMLElement): void {
    const { card, shown } = this.changeCard(write.path, write.machine, write.kind === "added" ? "added" : write.kind === "deleted" ? "deleted" : "modified");
    into.append(card.element);
    const kept = this.heldRows(key, write);
    if (kept) {
      const long = this.diffText(kept, write.path, true);
      this.held(key, long);
      card.body.append(long.element);
      shown(long);
      return;
    }
    const diff = write.diff;
    const lines = this.context.lines;
    if (!diff || !lines) return;
    const { words } = this.context;
    const note = document.createElement("p");
    note.className = "io-note";
    note.textContent = words.output.reading;
    card.body.append(note);
    const request = new AbortController();
    const held: { raw: OpenedLines | null } = { raw: null };
    const close = () => {
      const raw = held.raw;
      held.raw = null;
      if (raw) void raw.close().catch((error: unknown) => console.warn("a change's lines could not be let go", error));
    };
    const closing = () => {
      request.abort();
      close();
    };
    const closers = this.closers.get(key);
    if (closers) closers.push(closing);
    else this.closers.set(key, [closing]);
    const pending = (async () => {
      try {
        held.raw = await lines(diff.machine, diff.path, { kind: "file", encoding: "utf-8" });
        if (request.signal.aborted) return close();
        const remote = await RemoteDiff.open(held.raw, request.signal);
        let long: LongText;
        if (remote.extent.length <= FEW) {
          const rows = [...(await remote.read(0, remote.extent.length, request.signal))];
          close();
          this.findWrites.set(key, { signature: JSON.stringify(write), lines: rows });
          long = this.diffText(rows, write.path, true);
        } else long = this.diffText(remote, write.path, true);
        if (request.signal.aborted) {
          long.destroy();
          return close();
        }
        this.held(key, long);
        this.arrive(note, () => {
          note.replaceWith(long.element);
          long.settle();
          shown(long);
        });
      } catch (error) {
        if (request.signal.aborted) return;
        close();
        this.arrive(note, () => {
          note.textContent = gone(error) ? words.output.diffGone : words.output.readFailed;
          note.title = detailOf(error);
        });
      }
    })();
    this.filling.set(key, pending);
    void pending.finally(() => {
      if (this.filling.get(key) === pending) this.filling.delete(key);
    });
  }

  /** The rows of a change held here: kept with the conversation, or read
   *  whole before; null when they are to be read. */
  private heldRows(key: string, write: WriteFact): readonly string[] | null {
    if (write.diff?.lines) return keptDiff(write.diff.lines);
    const read = this.findWrites.get(key);
    return read?.signature === JSON.stringify(write) ? read.lines : null;
  }

  /** Reads in the background, one at a time, the changes of what is held
   *  whose rows are not kept with the conversation and take at most
   *  `PREFETCH` bytes, so opening one shows it at once; a larger one is
   *  read as it is opened, a screen at a time (`world-gui.md`, 对话). */
  private prefetch(): void {
    const lines = this.context.lines;
    if (!lines || this.prefetching) return;
    const wanted: [string, WriteFact, NonNullable<WriteFact["diff"]>][] = [];
    for (const block of this.conversation.blocks()) {
      const entries = block.kind === "activity-group" ? block.members : [block];
      for (const entry of entries) {
        if (entry.kind !== "tool") continue;
        for (const write of entry.tool.run?.writes ?? []) {
          const key = `${entry.key}${WRITE}${write.id}`;
          const diff = write.diff;
          if (diff && !diff.lines && diff.bytes !== undefined && diff.bytes <= PREFETCH && !this.readAhead.has(key)) {
            this.readAhead.add(key);
            wanted.push([key, write, diff]);
          }
        }
      }
    }
    if (!wanted.length) return;
    const request = new AbortController();
    this.prefetching = request;
    void (async () => {
      // The latest first: the reader is at the end.
      for (const [key, write, diff] of wanted.reverse()) {
        let raw: OpenedLines | null = null;
        try {
          raw = await lines(diff.machine, diff.path, { kind: "file", encoding: "utf-8" });
          const remote = await RemoteDiff.open(raw, request.signal);
          const rows = [...(await remote.read(0, remote.extent.length, request.signal))];
          this.findWrites.set(key, { signature: JSON.stringify(write), lines: rows });
        } catch (error) {
          if (request.signal.aborted) return;
          console.warn("a change could not be read ahead", error);
        } finally {
          if (raw) await raw.close().catch((error: unknown) => console.warn("a change's lines could not be let go", error));
        }
      }
    })().finally(() => {
      if (this.prefetching === request) this.prefetching = null;
    });
  }

  /** Fills what opens under a row or a notice: a change as a diff, a
   *  command's words and what it printed, anything else's input and output,
   *  thinking's words. */
  private fill(key: string, row: HTMLElement): void {
    const content = row.querySelector<HTMLElement>(":scope > .fold-box > .fold-content") ?? row.querySelector<HTMLElement>(".fold-box > .fold-content");
    if (!content) return;
    this.letGo(key);
    content.replaceChildren();
    const bunch = this.bunches.get(key);
    if (bunch && row.classList.contains("row-bunch")) {
      this.fillBunch(bunch, content);
      return;
    }
    const card = this.cards.get(key);
    if (card) {
      this.fillCard(key, card, content);
      return;
    }
    const split = key.indexOf(WRITE);
    if (split !== -1) {
      const owner = this.conversation.get(key.slice(0, split));
      const write = owner?.kind === "tool" ? owner.tool.run?.writes.find((one) => one.id === key.slice(split + 1)) : undefined;
      if (write) this.fillWrite(key, write, content);
      return;
    }
    const entry = this.conversation.get(key);
    if (!entry) return;
    if (entry.kind === "heard") {
      this.noticeDetails(entry, content);
      return;
    }
    if (entry.kind === "command") {
      const result = entry.row.result;
      const said = result?.kind === "done" || result?.kind === "reported" ? result.said : null;
      if (said) content.innerHTML = saidHtml(said);
      return;
    }
    if (entry.kind === "agent-told") {
      const words = entry.end.answer ?? entry.end.reason ?? "";
      content.innerHTML = `<div class="prose notice-answer">${render(words)}</div>`;
      return;
    }
    if (entry.kind === "tool" && startsAgent(entry.tool)) {
      this.fillAgent(entry, content);
      return;
    }
    if (entry.kind === "prose") {
      content.innerHTML = `<div class="thinking-text prose">${render(thoughtText(this.thoughtRun(key) ?? [entry]))}</div>`;
      return;
    }
    if (entry.kind !== "tool") return;
    const { tool } = entry;
    if (tool.does.as === "edit") {
      // Each file's change in a card of its own, saying which file it is.
      const longs: LongText[] = [];
      for (const change of tool.does.changes) {
        const rows = parse(change.diff);
        const long = this.diffText(memoryDiff(rows).lines, change.path, numbered(rows));
        longs.push(long);
        const how = change.how;
        const { card, shown } = this.changeCard(change.path, tool.machine ?? this.context.residentMachine ?? null, how === "added" ? "added" : how === "deleted" ? "deleted" : "modified");
        card.body.append(long.element);
        shown(long);
        content.append(card.element);
      }
      this.longs.set(key, longs);
      return;
    }
    const longs: OpenedText[] = [];
    const closers: (() => void)[] = [];
    content.append(this.commandWords(entry, null, longs, closers));
    this.longs.set(key, longs);
    this.closers.set(key, closers);
  }

  /** A call as what it did: the things a run did that a person recognises
   *  drawn as cards (`deeds/draw.ts`), what it printed folded at their
   *  foot; a command nothing of which is recognised opens straight on what
   *  it printed (`printout.ts`). Any other call in its own words. `only`
   *  keeps to some of its deeds. */
  private commandWords(entry: ToolEntry, only: readonly number[] | null, longs: OpenedText[], closers: (() => void)[]): HTMLElement {
    const { tool } = entry;
    const deeds = this.deedsOf(entry);
    const kept = (deeds ?? []).filter((_, index) => !only || only.includes(index));
    const drawn = kept.filter((deed) => deed.kind !== "plain" && deed.kind !== "writes");
    const plains = kept.filter((deed): deed is PlainDeed & Deed => deed.kind === "plain");
    // A run that went on after the call came back says so above all it did.
    const job = tool.does.as === "command" && tool.does.job && (!only || drawn.length === 0) ? goingOn(tool.does.job, tool.machine, this.context.words.deeds) : null;
    if (!drawn.length) {
      const words = this.printoutOf(entry, only ? plains : null, deeds, longs, closers, true);
      if (!job) return words;
      const both = document.createElement("div");
      both.className = "dd-deeds";
      both.append(job, words);
      return both;
    }
    const { words } = this.context;
    const failed = tool.does.as === "command" && !tool.does.job && tool.does.exit !== null && tool.does.exit !== 0;
    const ending = failed && tool.does.as === "command" ? `<span class="io-end nonzero">${escape(words.exited(tool.does.exit ?? 0))}</span>` : "";
    // The printout under the foot, which already says how the run ended,
    // is drawn as the foot opens; finding opens it to reach its lines.
    const later: OpenedText[] = [];
    const printed = () => {
      const sides: OpenedText[] = [];
      const element = this.printoutOf(entry, only ? plains : null, deeds, sides, closers, false);
      later.push(...sides);
      return element;
    };
    const cards = drawDeeds(drawn, this.deedTools(tool), printed, ending);
    const raw = cards.element.querySelector<HTMLButtonElement>(".dd-raw");
    const opened = (side: number): OpenedText => ({
      get element() {
        return later[side]?.element ?? cards.element;
      },
      reveal: async (line) => {
        if (raw && raw.getAttribute("aria-expanded") !== "true") raw.click();
        await later[side]?.reveal(line);
      },
      settle: () => later[side]?.settle(),
      destroy: () => later[side]?.destroy(),
    });
    longs.push(opened(0), opened(1), ...cards.longs);
    if (job) cards.element.prepend(job);
    return cards.element;
  }

  /** What a run printed, as its printout: its whole output; for commands
   *  one `on` sent where the run sent more than one, the last lines that
   *  `on` sent back. In the reader, beside them, the whole output on its
   *  machine where the model got less, and the script as it was given.
   *  Any other call in its own words. */
  private printoutOf(entry: ToolEntry, scoped: readonly PlainDeed[] | null, deeds: readonly Deed[] | null, longs: OpenedText[], closers: (() => void)[], ending: boolean): HTMLElement {
    const { tool } = entry;
    if (tool.does.as !== "command") return this.runWords(tool, longs);
    const { words, english } = this.context;
    const does = tool.does;
    const { shown, output } = toolText(tool, words);
    const others: ViewerSource[] = [];
    const whole = this.wholeOf(tool, closers);
    if (whole) others.push(whole);
    others.push({ label: words.output.script, source: new MemoryLines(does.command.replace(/\n$/, "").split("\n"), true) });
    // Commands of one `on`, where the run's output holds more than theirs:
    // what that `on` sent back of its own.
    const scopes = new Set((deeds ?? []).flatMap((deed) => (deed.kind === "plain" ? [deed.scope ?? ""] : [])));
    const own = scoped?.[0];
    const sent = own?.on && own.scope !== "top" && scopes.size > 1 && scoped!.every((deed) => deed.scope === own.scope) ? own.on : null;
    const lines: string[] = [];
    const notes = new Set<number>();
    let paint: LinePaint | undefined;
    let reading: LinePaint | undefined;
    // Line by line: an output's lines are more than a call's arguments can carry.
    const take = (more: string[]) => {
      for (const line of more) lines.push(line);
    };
    if (sent?.tail) take(sent.tail.replace(/\n$/, "").split("\n"));
    else if (!sent && shown) {
      take(shown.lines);
      shown.numbers.forEach((number, at) => {
        if (number === null) notes.add(at);
      });
      // Numbered in the reader by place in the whole output where that is
      // not simply the line's place here; the World's notes said quietly.
      const plainly = !shown.cut && shown.numbers.every((number, at) => number === at + 1);
      if (!plainly) {
        reading = outputPaint(shown);
        paint = outputPaint(shown, false);
      }
    } else if (!sent && output) take(output.replace(/\n$/, "").split("\n"));
    // How it ended, where it did not end well and nothing above says so.
    let end: PrintoutEnd | null = null;
    if (ending && sent?.end) {
      if (sent.end.kind === "stopped") end = { kind: "stopped", said: words.deeds.ended(sent.end) };
      else if (troubled(sent.end)) end = { kind: "failed", said: words.deeds.ended(sent.end) };
    } else if (ending && !sent && !does.job && does.exit !== null && does.exit !== 0) end = { kind: "failed", said: words.exited(does.exit) };
    const pictures = sent ? "" : imagesOf(tool.output);
    // The commands of the row where it is one `on`'s or a few of the
    // run's; else the script as given.
    const command = scoped?.length ? scoped.map((deed) => deed.command) : givenLines(does.command);
    const named = own ? own.command : (scriptSaid(does.command, tool.machine ?? null) ?? does.command.split("\n")[0] ?? "");
    const printout = new Printout({
      label: shortCommand(named) || words.output.printed,
      command,
      lines,
      notes,
      nothing: words.output.nothing,
      end,
      pictures,
      others,
      ...(paint ? { paint } : {}),
      ...(reading ? { reading } : {}),
      layout: this.layoutAnchor,
      words: {
        more: words.moreLines,
        less: words.showLess,
        copy: words.output.copyOutput,
        copied: words.copied,
        whole: words.fullScreen,
        english,
        failed: (error) => (error instanceof Said ? error.message : words.output.readFailed),
      },
    });
    longs.push(...printout.sides);
    return printout.element;
  }

  /** The whole output of a run on its machine, where the model got less
   *  of it than the whole, or it went on after: read as the reader asks. */
  private wholeOf(tool: Tool, closers: (() => void)[]): OtherSource | null {
    const { words } = this.context;
    const run = tool.does.as === "command" && tool.status !== "running" ? tool.run : undefined;
    const kept = run?.output;
    if (!run || !kept || !run.job || !this.context.lines) return null;
    const going = stillGoing(run.state);
    const wentOn = tool.does.as === "command" && tool.does.job !== null;
    if (!(kept.cut || going || wentOn)) return null;
    const open = this.context.lines;
    const job = run.job;
    const whole = new WholeOutput(
      () => open(kept.machine, kept.path, { kind: "output", job }),
      { length: kept.lines, complete: !going },
      (error) => new Said(gone(error) ? words.output.wholeGone : words.output.readFailed, detailOf(error)),
    );
    closers.push(() => whole.close());
    return { label: words.output.whole, source: whole, close: () => whole.close() };
  }

  /** What a run did, as read from its script and what it printed; null for
   *  what is not a run, or a run nothing of which is recognised. */
  private deedsOf(entry: Entry): Deed[] | null {
    if (entry.kind !== "tool" || entry.tool.does.as !== "command") return null;
    const read = this.deedsRead.get(entry.key);
    if (read?.stamp === entry.stamp) return read.deeds;
    const { shown, output } = toolText(entry.tool, this.context.words);
    const lines = shown ? shown.lines.map((line, index) => (shown.numbers[index] === null ? null : line)) : output ? output.split("\n") : [];
    const steps = entry.tool.run?.steps ?? [];
    const deeds = entry.ended || steps.length ? readRun(entry.tool.does.command, entry.tool.machine ?? this.context.residentMachine ?? null, lines, steps) : null;
    this.deedsRead.set(entry.key, { stamp: entry.stamp, deeds });
    return deeds;
  }

  /** How deeds are drawn with this view's own cards. */
  private deedTools(tool: Tool): DeedTools {
    const { words } = this.context;
    return {
      words: words.deeds,
      picture: (deed) => {
        // What the model saw, where its host's record kept every picture
        // the call gave, in their order. A host may keep only a preview in
        // words of a large result (Codex, past a mebibyte), and a run's
        // answer leaves out pictures past its budget: then each is drawn
        // from its file, by the machine holding it.
        const kept = tool.output.filter((content) => content.type === "image" || content.type === "image_url" || content.type === "image_ref");
        const given = (tool.run?.steps ?? []).reduce((count, step) => count + (step.kind === "view" ? (step.pictures?.length ?? 0) : 0), 0);
        if (kept.length === given) return imagesOf(kept.slice(deed.index, deed.index + 1));
        if (deed.page !== null || !/^image\/(png|jpeg|gif|webp)$/.test(deed.mime)) return "";
        return `<img class="io-image" data-picture data-path="${escape(deed.path)}"${deed.machine ? ` data-machine="${escape(deed.machine)}"` : ""} alt="${escape(deed.path.split(/[\\/]/).pop() ?? "")}">`;
      },
      file: (deed) => {
        const rows = deed.rows.map((row) =>
          "text" in row ? encodeRow({ kind: "same", old: row.line, new: row.line, text: row.text }) : encodeRow({ kind: "gap", old: null, new: null, hidden: row.skipped, text: "" }),
        );
        const text = deed.rows.flatMap((row) => ("text" in row ? [row.text] : [])).join("\n");
        const card = diffCard(deed.path, deed.machine, { copy: words.copy, copied: words.copied }, async () => text);
        // Lines passed over in a file read are left out, not unchanged.
        // A file read is not a change: its lines wrap.
        const long = this.diffText(rows, deed.path, deed.rows.some((row) => "line" in row && row.line !== null), (n) => words.deeds.skipped(n), true);
        card.body.append(long.element);
        const full = long.takeFull();
        if (full) card.act(full);
        return { element: card.element, longs: [long] };
      },
    };
  }

  /** Any other call in its own words, on the card a command has: what it
   *  was given above a drawn rule, what it gave back below. */
  private runWords(tool: Tool, longs: OpenedText[]): HTMLElement {
    const { words, english } = this.context;
    const { output, given } = toolText(tool, words);
    const lines = (text: string) => (text ? text.replace(/\n$/, "").split("\n") : []);
    const printout = new Printout({
      label: words.output.printed,
      command: lines(given),
      lines: lines(output),
      nothing: "",
      pictures: imagesOf(tool.output),
      layout: this.layoutAnchor,
      words: {
        more: words.moreLines,
        less: words.showLess,
        copy: words.output.copyOutput,
        copied: words.copied,
        whole: words.fullScreen,
        english,
        failed: (error) => (error instanceof Said ? error.message : words.output.readFailed),
      },
    });
    longs.push(...printout.sides);
    return printout.element;
  }

  /** What one of the host's own agents did, in its box under the row of
   *  the call that started it. Before the agent is known there is nothing
   *  to open; it opens as soon as it is. */
  private fillAgent(entry: ToolEntry, content: HTMLElement): void {
    const run = this.conversation.agentOf(entry);
    if (!run) {
      content.innerHTML = `<p class="agent-note">${escape(this.context.words.agents.nothing)}</p>`;
      return;
    }
    const { words, english, here, residentMachine, lines, agentRecord } = this.context;
    const box = new AgentBox(run, {
      words,
      english,
      here,
      residentMachine,
      ...(lines ? { lines } : {}),
      ...(agentRecord ? { record: agentRecord } : {}),
      reading: () => this.context.reading(),
    });
    content.append(box.element);
    this.boxes.set(entry.key, box);
    this.closers.set(entry.key, [
      () => {
        box.destroy();
        this.boxes.delete(entry.key);
      },
    ]);
  }

  /** Lets go of what a row held open. */
  private letGo(key: string): void {
    this.filling.delete(key);
    for (const long of this.longs.get(key) ?? []) long.destroy();
    this.longs.delete(key);
    for (const close of this.closers.get(key) ?? []) close();
    this.closers.delete(key);
  }

  private motionOf(box: HTMLElement): AnchoredHeight {
    let motion = this.motions.get(box);
    if (!motion) {
      motion = new AnchoredHeight(box, this.layoutAnchor);
      this.motions.set(box, motion);
    }
    return motion;
  }

  /** Opens or closes a box under the line that was pressed, the line
   *  staying where it is: what is below moves down as it opens, up as it
   *  closes. Open, it takes the height of what is in it as that grows. The
   *  same on every screen, with a pointer or a finger. */
  private async fold(box: HTMLElement, anchor: HTMLElement, open: boolean, filled: () => void): Promise<boolean> {
    this.context.reading();
    const motion = this.motionOf(box);
    if (open) {
      filled();
      box.inert = false;
      for (const long of this.longsIn(box)) long.settle();
      const following = this.following(box, anchor);
      const result = await motion.to(natural(box), anchor);
      following.disconnect();
      if (!result.cancelled && anchor.getAttribute("aria-expanded") === "true") box.style.height = "";
      return true;
    }
    box.style.height = `${box.getBoundingClientRect().height}px`;
    const result = await motion.to(0, anchor);
    if (!result.cancelled && anchor.getAttribute("aria-expanded") === "false") box.inert = true;
    return !result.cancelled;
  }

  /** Puts in what an open box was still waiting for, after its first
   *  height was taken: the box opens on to what came, from the height it
   *  stands at, whether it is still opening or open already, the pressed
   *  line where it is. Out of view, it simply takes it. */
  private arrive(at: HTMLElement, put: () => void): void {
    const box = at.closest<HTMLElement>(".fold-box");
    const anchor = box ? foldLine(box) : null;
    if (!box || !anchor || anchor.getAttribute("aria-expanded") !== "true") {
      put();
      return;
    }
    const view = this.context.scroller.getBoundingClientRect();
    const line = anchor.getBoundingClientRect();
    if (line.bottom < view.top || line.top > view.bottom) {
      put();
      return;
    }
    const motion = this.motionOf(box);
    const from = box.getBoundingClientRect().height;
    put();
    const to = natural(box);
    if (motion.moving) {
      motion.resize(to, anchor);
      return;
    }
    if (Math.abs(to - from) < 0.01) return;
    box.style.height = `${from}px`;
    const following = this.following(box, anchor);
    void motion.to(to, anchor).then((result) => {
      following.disconnect();
      if (!result.cancelled && anchor.getAttribute("aria-expanded") === "true") box.style.height = "";
    });
  }

  /** While a box opens, what is in it may settle after its height was
   *  taken (a viewport its rows, a picture its size): it opens on to that. */
  private following(box: HTMLElement, anchor: HTMLElement): ResizeObserver {
    const motion = this.motionOf(box);
    const observer = new ResizeObserver(() => {
      if (motion.moving) motion.resize(natural(box), anchor);
    });
    if (box.firstElementChild) observer.observe(box.firstElementChild);
    return observer;
  }

  private longsIn(box: HTMLElement): OpenedText[] {
    const found: OpenedText[] = [];
    for (const longs of this.longs.values()) for (const long of longs) if (box.contains(long.element)) found.push(long);
    return found;
  }

  /** A long message of the user's opened or closed: what the reader looks
   *  at stays where it is, the rest coming in below or going. */
  private unfold(button: HTMLElement): void {
    const key = button.dataset.unfold!;
    const bubble = button.closest<HTMLElement>(".bubble");
    const text = bubble?.querySelector<HTMLElement>(".bubble-text");
    if (!bubble || !text) return;
    const open = !this.unfolded.has(key);
    if (open) this.unfolded.add(key);
    else this.unfolded.delete(key);
    this.context.reading();
    const { words } = this.context;
    const from = text.offsetHeight;
    bubble.classList.toggle("open", open);
    button.setAttribute("aria-expanded", String(open));
    button.textContent = open ? words.bubbleLess : words.bubbleMore;
    const to = open ? text.scrollHeight : text.offsetHeight;
    text.style.maxHeight = "none";
    text.style.height = `${from}px`;
    // Opening, its first lines stay; closing, the button stays under the
    // pointer where the message's top is above the view.
    const view = this.context.scroller.getBoundingClientRect();
    const anchor = open || bubble.getBoundingClientRect().top >= view.top ? bubble : button;
    void this.motionOf(text)
      .to(to, anchor)
      .then(() => {
        text.style.height = "";
        text.style.maxHeight = "";
      });
  }

  private click(event: Event): void {
    const unfold = (event.target as HTMLElement).closest<HTMLElement>("[data-unfold]");
    if (unfold) {
      this.unfold(unfold);
      return;
    }
    const target = (event.target as HTMLElement).closest<HTMLElement>(
      "[data-open], [data-fold], [data-gap], [data-earlier], [data-asked], .code-copy, .ask-option, [data-ask], [data-copy], [data-copy-turn], [data-turn-act], [data-turn-calls]",
    );
    if (!target) return;
    if (target.dataset.asked !== undefined) {
      // What an agent was asked, in full or its first lines again.
      const key = target.dataset.asked;
      if (this.openRows.has(key)) this.openRows.delete(key);
      else this.openRows.add(key);
      this.context.reading();
      const block = target.closest<HTMLElement>(".block");
      const slot = block ? this.byElement.get(block) : undefined;
      if (slot) this.draw(slot);
      return;
    }
    if (target.dataset.earlier !== undefined) {
      // More of a long stretch's earlier rows, beneath the line asked: it
      // stays where it is, the rest moving down.
      const key = target.dataset.earlier;
      const slot = this.slots.get(key);
      if (slot?.stretch && slot.block.kind === "activity-group") {
        this.context.reading();
        this.stretchRows.set(key, (this.stretchRows.get(key) ?? STRETCH_ROWS) + STRETCH_ROWS);
        this.rows(slot.stretch, slot.block);
      }
      return;
    }
    if (target.dataset.gap !== undefined) {
      // A gap read at once where the reader asks: from its end, nearest what
      // they read below it, unless they are above it.
      const gap = this.slots.get(target.dataset.gap)?.block;
      if (gap?.kind !== "gap" || gap.loading) return;
      const index = this.heights.index(gap.key) ?? 0;
      const reader = this.heights.find(Math.max(0, this.context.scroller.scrollTop - this.origin()));
      this.context.near?.(gap, index <= reader ? "end" : "start");
      return;
    }
    if (target.dataset.turnCalls) {
      this.context.turnCalls?.(target.dataset.turnCalls);
      return;
    }
    if (target.dataset.turnAct) {
      const turn = target.dataset.turn!;
      // Asking again sends the words of the turn's own question.
      const entry = target.dataset.turnAct === "rewind" ? this.conversation.get(target.dataset.key ?? "") : this.conversation.turnWords(turn);
      this.context.turnAct?.(target.dataset.turnAct as "rewind" | "regenerate" | "fork", turn, entry?.kind === "heard" ? entry.text : null, entry?.kind === "heard" ? (entry.kept ?? []) : []);
      return;
    }
    if (target.dataset.copy || target.dataset.copyTurn) {
      // A message is copied as it was written, in its Markdown; a turn's
      // answer as all it said.
      const entry = target.dataset.copy ? this.conversation.get(target.dataset.copy) : undefined;
      const text = target.dataset.copyTurn
        ? this.conversation.turnAnswer(target.dataset.copyTurn)
        : entry?.kind === "prose" || entry?.kind === "heard"
          ? entry.text
          : "";
      void navigator.clipboard?.writeText(text).then(() => {
        target.classList.add("copied");
        target.innerHTML = glyph("check", 16);
        setTimeout(() => {
          target.classList.remove("copied");
          target.innerHTML = glyph("copy", 16);
        }, 1400);
      });
      return;
    }
    if (target.classList.contains("ask-option") || target.dataset.ask) {
      void this.choose(target);
      return;
    }
    if (target.classList.contains("code-copy")) {
      const code = target.parentElement?.querySelector("code")?.textContent ?? "";
      void navigator.clipboard?.writeText(code).then(() => {
        target.classList.add("copied");
        setTimeout(() => target.classList.remove("copied"), 1400);
      });
      return;
    }
    if (target.dataset.fold) {
      const key = target.dataset.fold;
      const slot = this.slots.get(key);
      const stretch = slot?.stretch;
      if (!slot || !stretch || slot.block.kind !== "activity-group" || target.hasAttribute("aria-disabled")) return;
      const group = slot.block;
      const open = !this.openStretches.has(key);
      if (open) this.openStretches.add(key);
      else this.openStretches.delete(key);
      if (open) this.closedStretches.delete(key);
      else this.closedStretches.add(key);
      slot.signature = this.signature(group);
      target.setAttribute("aria-expanded", String(open));
      void this.fold(stretch.box, target, open, () => {
        // Its rows are made as it opens, as they now stand.
        this.seenOpen = false;
        this.rows(stretch, group);
        this.seenOpen = true;
      });
      return;
    }
    const key = target.dataset.open;
    if (!key) return;
    const sent = this.conversation.get(key);
    if (sent?.kind === "tool" && startsAgent(sent.tool) && this.context.openAgent) {
      const run = this.conversation.agentOf(sent);
      if (run && this.context.openAgent(run.id)) return;
    }
    const row = target.closest<HTMLElement>("[data-row], .block-heard, .block-agent-told, .block-command");
    const box = (target.closest(".notice-line")?.nextElementSibling as HTMLElement | null) ?? row?.querySelector<HTMLElement>(":scope > .fold-box") ?? null;
    if (!row || !box) return;
    const open = !this.openRows.has(key);
    if (open) this.openRows.add(key);
    else this.openRows.delete(key);
    target.setAttribute("aria-expanded", String(open));
    void this.fold(box, target, open, () => this.fill(key, row)).then((done) => {
      if (done && !open) this.letGo(key);
    });
  }
}

/** Lines and characters past which the user's own words show their first
 *  lines only, the rest a press away. */
const LONG_LINES = 14;
const LONG_CHARACTERS = 1200;

function longWords(text: string): boolean {
  return text.length > LONG_CHARACTERS || text.split("\n").length > LONG_LINES;
}

/** Whether a call asked one of the World's colleagues in, or spoke to one:
 *  its own conversation can be opened. */
function worldColleague(tool: Tool): boolean {
  return tool.does.as === "colleague" && (tool.name === "ask_colleague" || tool.name === "message_colleague");
}

/** The height a fold's box takes open: what is in it, with the margins its
 *  first and last parts keep inside it. */
function natural(box: HTMLElement): number {
  const held = box.style.height;
  box.style.height = "auto";
  const height = box.getBoundingClientRect().height;
  box.style.height = held;
  return height;
}

/** The line whose press opens and closes `box`: the row's own, or the
 *  notice's above it. */
function foldLine(box: HTMLElement): HTMLElement | null {
  const own = box.parentElement?.querySelector<HTMLElement>(":scope > [aria-expanded]");
  if (own) return own;
  const before = box.previousElementSibling as HTMLElement | null;
  return before?.matches("[aria-expanded]") ? before : (before?.querySelector<HTMLElement>("[aria-expanded]") ?? null);
}

/** Whether what a run started still runs, its output still growing. */
function stillGoing(state: RunFacts["state"]): boolean {
  return state.kind === "running" || (state.kind === "exited" && state.remaining > 0);
}

/** Whether a machine said that what was asked for is no longer there, as
 *  the node's failure codes it. */
function gone(error: unknown): boolean {
  const code = typeof error === "object" && error !== null && "code" in error ? (error as { code?: unknown }).code : undefined;
  return code === "source_missing" || code === "original_gone" || code === "output_not_retained";
}

function imagesOf(output: Content[]): string {
  return output
    .map((content) => {
      if (content.type === "image") return `<img class="io-image" data-picture alt="" src="data:${escape(content.mime)};base64,${content.base64}">`;
      if (content.type === "image_url") return `<img class="io-image" data-picture alt="" src="${escape(content.url)}">`;
      // Drawn as it scrolls into view, from where the session keeps it.
      if (content.type === "image_ref") return `<img class="io-image" data-picture data-attachment="${escape(JSON.stringify(content.source))}" alt="">`;
      return "";
    })
    .join("");
}

/** A file's size as people say it. */
function sizeOf(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/** One line of what a turn took: its label and value, and the value drawn
 *  where words alone would not do. */
interface TurnInfoRow {
  label: string;
  text: string;
  html?: string;
}

/** What a turn took: the model and effort its host reported, how long, the
 *  tokens, what it cost by the host's own ledger, and the login's windows
 *  as the turn left them, a short bar each; what the host did not report is
 *  left out. */
function turnInfo(end: TurnEnd, words: Words, english: boolean): TurnInfoRow[] {
  const { turnInfo: said } = words;
  const facts = end.facts;
  const rows: TurnInfoRow[] = [];
  const row = (label: string, text: string) => rows.push({ label, text });
  if (facts?.models?.length) row(said.model, facts.models.map(modelName).join(" → "));
  const efforts = (facts?.efforts ?? []).filter((effort) => effort.value !== null && effort.value !== undefined);
  if (efforts.length) {
    row(
      said.effort,
      efforts
        .map((effort) => {
          const value = effortLabel(typeof effort.value === "string" ? effort.value : JSON.stringify(effort.value), english);
          return effort.kind === "requested" ? said.requested(value) : value;
        })
        .join(" → "),
    );
  }
  if (end.duration_ms) row(said.took, words.took(end.duration_ms));
  const tokens = facts?.tokens;
  if (tokens?.host === "codex") row(said.tokens, said.spent(tokens.input_tokens, tokens.cached_input_tokens, tokens.output_tokens, tokens.reasoning_output_tokens));
  else if (tokens?.host === "claude") {
    const input = tokens.input_tokens + tokens.cache_read_input_tokens + tokens.cache_creation_input_tokens;
    row(said.tokens, said.spent(input, tokens.cache_read_input_tokens, tokens.output_tokens, 0));
  }
  if (facts?.cost !== undefined) row(said.cost, said.costOf(facts.cost / 1_000_000));
  // The shorter window first, as the windows are read everywhere else.
  const windows = [...(facts?.limits?.windows ?? [])].sort((a, b) => (a.minutes ?? Infinity) - (b.minutes ?? Infinity));
  if (windows.length) {
    const named = windows.map((window) => ({ name: said.window(window.name, window.minutes ?? null), percent: `${Math.round(window.used_tenths / 10)}%`, window }));
    rows.push({
      label: said.limits,
      text: named.map(({ name, percent }) => `${name} ${percent}`).join(", "),
      html: `<span class="turn-info-limits">${named
        .map(({ name, percent, window }) => {
          const level = window.used_tenths >= 1000 ? " spent" : window.used_tenths >= 900 ? " high" : "";
          return `<span class="turn-info-limit${level}"><span>${escape(name)}</span><span class="turn-info-limit-track"><i style="width:${Math.min(100, window.used_tenths / 10).toFixed(1)}%"></i></span><span class="turn-info-limit-used">${escape(percent)}</span></span>`;
        })
        .join("")}</span>`,
    });
  }
  return rows;
}

/** A host's word on the session's health, in a plain line: a turn stopped
 *  at an assistance's allowance says so and until when. */
function healthSaid(health: Health, words: Words): string {
  const allowance = health.kind === "warning" ? allowanceOf(health.text) : null;
  if (allowance) return words.allowanceUsed(allowance);
  return words.health[health.kind] ?? health.text;
}

/** A model as people call it: `claude-opus-5-5` is Opus 5.5; any other
 *  name as its host writes it. */
export function modelName(id: string): string {
  const claude = /^claude-([a-z]+)-(\d+)(?:-(\d+))?(?:-\d{8})?$/.exec(id);
  if (!claude) return id;
  const [, family, major, minor] = claude;
  return `${family![0]!.toUpperCase()}${family!.slice(1)} ${major}${minor ? `.${minor}` : ""}`;
}

/** A block's room, whole and above its words: what it folds from or
 *  unfolds to. */
function roomOf(element: HTMLElement): { full: number; gap: number } {
  return { full: element.getBoundingClientRect().height, gap: parseFloat(getComputedStyle(element).paddingTop) || 0 };
}

/** What a block shows that both views show: its words, below the moment a
 *  chat says above them. */
function wordsOf(block: HTMLElement): HTMLElement {
  for (const child of block.children) if (!child.classList.contains("chat-moment")) return child as HTMLElement;
  return block;
}

/** Sets a block to fold or unfold within `room`. */
function foldable(element: HTMLElement, room: { full: number; gap: number }): void {
  element.style.setProperty("--full", `${room.full}px`);
  element.style.setProperty("--gap", `${room.gap}px`);
  element.classList.add("view-fold");
}

/** A block unfolded, its room its own again. */
function unfold(element: HTMLElement): void {
  element.classList.remove("view-fold", "view-folded", "view-shut");
  element.style.removeProperty("--full");
  element.style.removeProperty("--gap");
}

/** A CSS time, as a token holds it, in milliseconds. */
function msOf(time: string): number {
  const value = parseFloat(time);
  if (!Number.isFinite(value)) return 0;
  return time.trim().endsWith("ms") ? value : value * 1000;
}
