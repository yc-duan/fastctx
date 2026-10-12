// A conversation drawn in the page (`world-gui.md`, 对话). Each block has an
// element of its own for as long as it is there, and what it shows is drawn
// again only when it changed: an entry's stamp says when. Words streaming
// in redraw the paragraph they add to, not the message, not the
// conversation; a conversation thousands of tool calls long draws only the
// blocks near the reader (`lazy.ts`).
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

import { layerOpened } from "../app/back";
import type { Content, MachineName, Notice, Speaker, Tool, TurnEnd } from "../world/events";
import { STOPPED_BY_USER } from "../world/events";
import type { Attachment } from "../world/attachments";
import type { Decoding } from "../world/remote-lines";
import type { RunFacts, WriteFact } from "../world/run-facts";
import type { JsonValue } from "../world/wire";
import { answerOf, drawAsk, readAsk, type AskWords } from "./asks";
import type { Words } from "../words";
import { glyph } from "./glyphs";
import { effortLabel } from "./effort-slider";
import { isThinking, runsOf, type Activity, type Block, type Conversation, type Entry, type SendState, type ShownAttachment } from "./model";
import { escape, plain, render } from "./prose";
import { numbered, parse } from "./diff";
import { DiffLines, RemoteDiff, diffPaint, memoryDiff } from "./diff-lines";
import { headline, onlyWrites, tenseOf, writeHeadline } from "./tool";
import { drawDiagrams } from "./diagrams";
import { readTold, worldSaid, type WorldSaid } from "./told";
import { detailOf } from "../app/problem";
import { AnchorLayout, AnchoredHeight } from "../longview";
import { FEW, LongText, type LongLines, type LongTextWords, type OtherSource } from "./long-text";
import { Said, WholeOutput, outputPaint, type OpenedLines } from "./run-output";
import { replyOf } from "./async-ask";
import { chosenIn, drawQuestions, type QuestionWords } from "./question-card";
import { StreamedProse } from "./stream-prose";
import { TurnLine, time, type LineSays } from "./turn-line";
import { fly, type Departure } from "./flight";
import { Lazy, estimate } from "./lazy";
import { FindContent, displayedDiff, toolText } from "./find-content";
import { clockSays, sections, stretchSays, thoughtOnly, thoughtText, thoughtTook } from "./stretch-says";
import { displayedText, htmlText as displayedTextHtml, type TextMatch } from "./find-text";
import type { FindChanges, FindMatch } from "./find-index";

export interface Context {
  words: Words;
  english: boolean;
  /** The machine this interface runs on. */
  here: MachineName;
  /** The machine the resident lives on: a tool that acted there needs no
   *  word of where; one that acted elsewhere says so. */
  residentMachine: MachineName;
  /** The region the conversation scrolls in. */
  scroller: HTMLElement;
  /** The reader opened or closed something: the page stops keeping the
   *  newest in view, so nothing moves under their hand. */
  reading(): void;
  /** Where words being sent leave from (`flight.ts`). */
  departure?: (() => Departure | null) | undefined;
  /** Where another resident's conversation opens, where it can. */
  link?: ((address: { resident: string; machine: MachineName }) => string) | undefined;
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
  questions?: { words: QuestionWords; answer(text: string): Promise<void> } | undefined;
  /** The user asked to start again from their words in `turn`, to ask the
   *  same again, or to fork from `turn`; offered only where the
   *  conversation takes it. */
  turnAct?: ((act: "rewind" | "regenerate" | "fork", turn: string, words: string | null, kept: Attachment[]) => void) | undefined;
  /** Lines of a file or a job's output on the machine holding them, read a
   *  screen at a time: a write's change, a command's whole output. */
  lines?: ((machine: MachineName, path: string, decoding: Decoding) => Promise<OpenedLines>) | undefined;
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

type ToolEntry = Extract<Entry, { kind: "tool" }>;
type AskEntry = Extract<Entry, { kind: "ask" }>;
type HeardEntry = Extract<Entry, { kind: "heard" }>;
type QuestionEntry = Extract<Entry, { kind: "question" }>;
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

/** A block's place in the page. */
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
  stretch?: Stretch;
  /** Its height when it was let go, kept while it is not drawn. */
  height: number | null;
  findSignature?: string;
  findRevision?: number;
}

/** Blocks drawn only near the reader; the others cost too little to wait. */
const LAZY = new Set<Block["kind"]>(["activity-group", "prose", "heard", "ask", "question", "steps"]);

export class ConversationView {
  private openRows = new Set<string>();
  private openStretches = new Set<string>();
  private slots = new Map<string, Slot>();
  private byElement = new WeakMap<HTMLElement, Slot>();
  /** Long texts open under rows, by row, to be let go when the row is. */
  private longs = new Map<string, LongText[]>();
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
  /** Questions being answered again, and ones the user skipped. */
  private editing = new Set<string>();
  private skipped = new Set<string>();
  /** Words already sent up from the input, by key. */
  private flown = new Set<string>();
  private findPending = new Set<string>();
  private findRemoved = new Set<string>();
  private findOrder: string[] = [];
  private findWrites = new Map<string, { signature: string; lines: readonly string[] }>();
  private readonly findContent: FindContent;
  private readonly layout: AnchorLayout;
  private readonly lazy: Lazy;
  private readonly line: TurnLine;
  /** The clocks of stretches running, brought up to date each second. */
  private clocks = new Set<HTMLElement>();
  private ticking = 0;

  constructor(
    private readonly root: HTMLElement,
    private readonly conversation: Conversation,
    private readonly context: Context,
  ) {
    root.classList.add("transcript");
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
      tokens: (run) => conversation.thinkingTokens(run),
      notice: (entry) => !!this.noticeOf(entry),
      write: (key, signal) => this.findWrite(key, signal),
    });
    this.layout = new AnchorLayout(context.scroller, root);
    this.line = new TurnLine(context.words);
    this.lazy = new Lazy(context.scroller, {
      near: (element) => {
        const slot = this.byElement.get(element);
        if (slot && slot.signature === null && this.root.isConnected) this.draw(slot);
      },
      far: (element, height) => {
        const slot = this.byElement.get(element);
        if (slot) this.release(slot, height);
      },
    });
    root.addEventListener("click", (event) => this.click(event));
    root.addEventListener("input", (event) => {
      // Words of one's own answer a question instead of an option.
      const free = (event.target as HTMLElement).closest<HTMLInputElement>("[data-question-free]");
      if (!free?.value.trim()) return;
      for (const option of free.closest("fieldset")?.querySelectorAll<HTMLElement>("[data-question-pick]") ?? []) option.setAttribute("aria-pressed", "false");
    });
    // On a touch screen, holding an earlier turn's words brings its actions
    // up from the bottom; the latest turn's are already there.
    root.addEventListener("contextmenu", (event) => {
      if (!matchMedia("(hover: none)").matches) return;
      const block = (event.target as HTMLElement).closest<HTMLElement>(".block");
      const acts = block?.querySelector<HTMLElement>(".turn-acts:not(.latest)");
      if (!acts || (event.target as HTMLElement).closest("a, button, input, textarea, pre")) return;
      event.preventDefault();
      this.sheet(acts);
    });
    // Enter in an answer's field answers, as the card's first button would.
    root.addEventListener("keydown", (event) => {
      const field = (event.target as HTMLElement).closest(".ask-card .ask-input, .question-card .ask-input");
      if (!field || event.key !== "Enter" || event.isComposing) return;
      event.preventDefault();
      field.closest(".ask-card, .question-card")?.querySelector<HTMLButtonElement>(".ask-primary")?.click();
    });
  }

  /** Brings the page up to the conversation as it now stands; `read` when
   *  what is new came as history rather than as it happened. */
  update(read = false): void {
    const blocks = this.conversation.blocks();
    const says = this.lineSays(blocks);
    this.reading = read;
    // Words already on their way when the page is first drawn (the first
    // words of a conversation just begun) rose where they were sent.
    if (!this.live) for (const block of blocks) if (block.kind === "heard" && block.sending) this.flown.add(block.key);
    const present = new Set<string>();
    this.findOrder = blocks.map((block) => block.key);
    const drawn: HTMLElement[] = [];
    let previous: HTMLElement | null = null;
    // The line stands before the first words waiting below it, else last.
    let lineBefore: HTMLElement | null = null;
    for (const block of blocks) {
      present.add(block.key);
      let slot = this.slots.get(block.key);
      if (!slot) slot = this.slot(block, read);
      slot.block = block;
      const signature = this.findSignature(block);
      if (slot.findSignature !== signature) {
        slot.findSignature = signature;
        this.findPending.add(block.key);
      }
      // The turn's line between blocks is passed over: it is placed after.
      let want: Element | null = previous ? previous.nextElementSibling : this.root.firstElementChild;
      if (want === this.line.element) want = want.nextElementSibling;
      if (want !== slot.element) {
        if (previous) previous.after(slot.element);
        else this.root.prepend(slot.element);
      }
      previous = slot.element;
      if (says && !lineBefore && block.kind === "heard" && block.sending !== undefined && block.intoTurn) lineBefore = slot.element;
      if (slot.signature !== null && slot.signature !== this.signature(block)) {
        this.draw(slot);
        drawn.push(slot.element);
      }
    }
    for (const [key, slot] of this.slots) {
      if (present.has(key)) continue;
      this.forget(slot);
      this.lazy.unwatch(slot.element);
      slot.element.remove();
      this.slots.delete(key);
      this.findPending.delete(key);
      this.findRemoved.add(key);
    }
    // Words rising this frame land before the line beneath them is drawn.
    const flying = this.flights(blocks);
    if (says) {
      this.line.say(says);
      this.line.place(this.root, lineBefore, this.live && !read, flying ? time(getComputedStyle(this.root), "--send") * 0.6 : 0);
    } else this.line.remove();
    if (!this.live) {
      this.live = true;
      this.fillFromEnd(blocks);
    }
    this.tick();
    for (const element of drawn) void drawDiagrams(element).then(() => {
      const slot = this.byElement.get(element);
      if (slot && element.querySelector(".diagram.drawn")) {
        slot.findRevision = (slot.findRevision ?? 0) + 1;
        this.findPending.add(slot.key);
      }
    });
  }

  /** Changed display text, independently of which blocks are drawn. */
  findChanges(): FindChanges {
    const changed: FindChanges["changed"] = [];
    for (const key of this.findPending) {
      const slot = this.slots.get(key);
      if (!slot) continue;
      const parts = this.findContent.parts(slot.block);
      const signature = `${slot.findSignature ?? ""}|${slot.findRevision ?? 0}`;
      changed.push({ key, parts: slot.block.kind === "activity-group" ? parts : (function* () {
        for (const part of parts) yield { ...part, stamp: `${signature}|${part.stamp}` };
      })() });
    }
    const removed = [...this.findRemoved];
    this.findPending.clear();
    this.findRemoved.clear();
    return { order: this.findOrder, changed, removed };
  }

  /** Draws and opens just the selected match's ancestry. Existing folds
   * and unrelated line viewports keep their disclosure and scroll state. */
  async revealFind(match: FindMatch): Promise<void> {
    const target = match.document.target;
    const slot = this.slots.get(target.block);
    if (!slot) return;
    if (slot.signature === null) this.draw(slot);
    const stretch = slot.stretch;
    if (target.row && stretch && slot.block.kind === "activity-group" && !this.openStretches.has(slot.key)) {
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
    const row = target.row === slot.key ? slot.element : target.row ? stretch?.drawn.get(target.row)?.element : undefined;
    if (row && (target.long !== undefined || target.selector !== undefined) && !this.openRows.has(target.row!)) {
      const button = row.querySelector<HTMLElement>("[data-open]");
      const box = row.querySelector<HTMLElement>(".fold-box");
      if (button && box) {
        this.openRows.add(target.row!);
        button.setAttribute("aria-expanded", "true");
        this.fill(target.row!, row);
        this.motions.get(box)?.destroy();
        this.motions.delete(box);
        box.inert = false;
        box.style.height = "";
      }
    }
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
    if (!write?.diff || !this.context.lines) return "";
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
    const top = this.context.scroller.getBoundingClientRect().top;
    for (const element of this.root.children) {
      const slot = this.byElement.get(element as HTMLElement);
      if (!slot) continue;
      const box = element.getBoundingClientRect();
      if (box.bottom > top) return { key: slot.key, offset: top - box.top };
    }
    return null;
  }

  /** Puts the reader back where `anchor` says they were. */
  restore(anchor: { key: string; offset: number }): void {
    const slot = this.slots.get(anchor.key);
    if (!slot) return;
    const scroller = this.context.scroller;
    scroller.scrollTop += slot.element.getBoundingClientRect().top - scroller.getBoundingClientRect().top + anchor.offset;
  }

  /** Lets go of everything drawn, as the page is left for good. */
  destroy(): void {
    for (const slot of this.slots.values()) this.forget(slot);
    this.slots.clear();
    this.findWrites.clear();
    this.lazy.close();
    this.line.remove();
    clearTimeout(this.ticking);
  }

  /** A block's place, made the first time it is laid out: drawn at once
   *  when it comes into a live conversation, otherwise when it comes near. */
  private slot(block: Block, read: boolean): Slot {
    const element = document.createElement("div");
    element.dataset.key = block.key;
    element.className = `block block-${block.kind === "activity-group" ? "activity" : block.kind}`;
    if (block.kind === "prose") element.classList.add(`prose-${block.prose}`);
    if (block.kind === "heard") {
      const notice = this.noticeOf(block);
      element.classList.add(notice ? "from-world" : !block.from || block.from.kind === "user" ? "from-user" : "from-other");
      if (block.reply) element.classList.add("from-reply");
    }
    const slot: Slot = { key: block.key, element, block, signature: null, height: null };
    this.slots.set(block.key, slot);
    this.byElement.set(element, slot);
    if ((this.live && !read) || !LAZY.has(block.kind)) this.draw(slot);
    else this.wait(slot);
    if (LAZY.has(block.kind)) this.lazy.watch(element);
    return slot;
  }

  /** A slot not drawn, holding the room it would take. */
  private wait(slot: Slot): void {
    slot.element.classList.add("lazy");
    slot.element.style.height = `${Math.round(slot.height ?? this.estimate(slot.block))}px`;
  }

  /** How tall a block not yet drawn is taken to be (`lazy.ts`), with the
   *  room its turn's actions take where it has them: under a pointer each
   *  ended turn's, on a touch screen only the latest's. */
  private estimate(block: Block): number {
    const { line, chars } = this.metrics();
    const conversation = this.conversation;
    const turn = block.kind === "heard" || block.kind === "prose" ? block.turn : null;
    const has =
      !!turn &&
      conversation.turnEnded(turn) &&
      ((block.kind === "heard" && !block.notice && !block.sending && (!block.from || block.from.kind === "user")) ||
        (block.kind === "prose" && block.prose === "message" && conversation.lastMessageOf(turn) === block.key));
    const touch = matchMedia("(hover: none)").matches;
    const acts = !has ? 0 : !touch ? 32 : conversation.latestEnded() === turn ? 22 : 0;
    return estimate(block, line, chars, acts);
  }

  /** The body's line height and how many of its characters a line of the
   *  column holds, for blocks not yet drawn. */
  private metrics(): { line: number; chars: number } {
    if (this.measured) return this.measured;
    const style = getComputedStyle(this.root);
    const size = parseFloat(style.fontSize) || 15;
    const line = size * (parseFloat(style.getPropertyValue("--leading-prose")) || 1.68);
    const width = this.root.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    const measured = { line, chars: Math.max(20, Math.floor((width > 0 ? width : size * 46) / (size * 0.55))) };
    if (width > 0) this.measured = measured;
    return measured;
  }
  private measured: { line: number; chars: number } | null = null;

  /** Draws the last blocks at once, a couple of screens of them, so the
   *  newest are there the moment the conversation opens; the rest as the
   *  reader comes to them. */
  private fillFromEnd(blocks: readonly Block[]): void {
    // Reckoned in the heights blocks are taken to be, so nothing is laid
    // out until all of them are drawn.
    const screen = this.context.scroller.clientHeight || innerHeight;
    let filled = 0;
    for (let index = blocks.length - 1; index >= 0 && filled < screen * 2; index -= 1) {
      const block = blocks[index]!;
      const slot = this.slots.get(block.key)!;
      if (slot.signature === null) this.draw(slot);
      filled += this.estimate(block);
    }
  }

  /** Lets a slot far from the reader go, keeping its height; one the
   *  reader has opened something in, is in, or has selected in, stays. */
  private release(slot: Slot, height: number): void {
    // A page out of view keeps what it drew, to be shown again as it was.
    if (!this.root.isConnected) return;
    if (slot.signature === null || !LAZY.has(slot.block.kind)) return;
    if (this.openStretches.has(slot.key) || slot.element.querySelector("[aria-expanded=\"true\"]")) return;
    if (slot.element.contains(document.activeElement)) return;
    const selection = getSelection();
    if (selection && selection.rangeCount && !selection.isCollapsed && selection.getRangeAt(0).intersectsNode(slot.element)) return;
    this.forget(slot);
    slot.element.replaceChildren();
    slot.signature = null;
    slot.height = height;
    this.wait(slot);
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
        return `${block.stamp}|${last ? `l${conversation.turnEnded(block.turn) ? "e" : ""}${conversation.latestEnded() === block.turn ? "n" : ""}` : ""}`;
      }
      case "heard":
        return `${block.stamp}|${block.turn && conversation.turnEnded(block.turn) ? "e" : ""}${block.turn && conversation.latestEnded() === block.turn ? "n" : ""}`;
      case "question":
        return `${block.stamp}|${this.editing.has(block.key) ? "e" : ""}${this.skipped.has(block.key) ? "s" : ""}`;
      case "uncertain":
        return `${block.stamp}|${this.conversation.hasTurns() ? "t" : ""}`;
      default:
        return String(block.stamp);
    }
  }

  private findSignature(block: Block): string {
    switch (block.kind) {
      case "activity-group":
        return `${block.stamp}|${block.members.length}${block.agents ? `|${this.conversation.agentStamp}` : ""}`;
      case "question":
        return `${block.stamp}|${this.editing.has(block.key)}|${this.skipped.has(block.key)}`;
      case "uncertain":
        return `${block.stamp}|${this.conversation.hasTurns()}`;
      default:
        return String(block.stamp);
    }
  }

  /** Draws what a slot shows, as its block now stands. */
  private draw(slot: Slot): void {
    const { block, element } = slot;
    if (slot.signature === null) {
      element.classList.remove("lazy");
      element.style.height = "";
    }
    slot.signature = this.signature(block);
    if (block.kind === "activity-group") {
      this.stretch(slot, block);
      return;
    }
    if (block.kind === "prose" && block.prose !== "plan") {
      this.message(slot, block);
      return;
    }
    this.letGo(block.key);
    element.innerHTML = this.entry(block);
    if (this.openRows.has(block.key)) this.fill(block.key, element);
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
    slot.prose.update(entry.text, entry.writing);
    const acts = this.answerActs(entry);
    if (slot.acts!.innerHTML !== acts) slot.acts!.innerHTML = acts;
  }

  /** The session is no longer followed: whether its model still works is
   *  not known, and the line says nothing. */
  unfollowed(): void {
    this.followed = false;
  }
  private followed = true;

  /** What the line at the foot says, or null when no turn is under way:
   *  what the model is on is said above, by the stretch that holds it. */
  private lineSays(blocks: readonly Block[]): LineSays | null {
    if (!this.followed) return null;
    const conversation = this.conversation;
    const waiting = conversation.waiting();
    const busy = conversation.busy();
    // The first words are on their way, and nothing has come back.
    const spoken = blocks.some((block) => block.kind === "heard" && (!block.from || block.from.kind === "user") && !block.notice && block.sending?.state !== "failed");
    if (!conversation.spoke() && spoken && busy) return { kind: "starting" };
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
   *  has laid them out; says whether any do. */
  private flights(blocks: readonly Block[]): boolean {
    const departure = this.context.departure;
    if (!departure) return false;
    let flying = false;
    for (const block of blocks) {
      if (block.kind !== "heard" || block.sending?.state !== "going" || this.flown.has(block.key)) continue;
      this.flown.add(block.key);
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
        return `<div class="note caution" title="${escape(entry.health.text)}">${escape(this.context.words.health[entry.health.kind] ?? entry.health.text)}</div>`;
      case "uncertain":
        // Words that never began a conversation: it could not start, and
        // why, as it was given, where the first words wait.
        if (!this.conversation.hasTurns()) {
          return `<div class="start-failed"><p class="start-failed-said">${escape(words.startFailed)}</p><p class="start-failed-why">${escape(entry.why)}</p></div>`;
        }
        return `<div class="note caution" title="${escape(entry.why)}">${escape(words.uncertain(entry.why))}</div>`;
      case "ask":
        return this.ask(entry);
      case "question":
        return this.question(entry);
      case "forked": {
        const from = `${entry.source.resident} · ${entry.source.machine}`;
        const link = this.context.link ? `<a href="${escape(this.context.link(entry.source))}">${escape(from)}</a>` : escape(from);
        return `<div class="divider"><span>${words.forkedFrom(link)}</span></div>`;
      }
      case "tool":
      case "activity":
        return "";
    }
  }

  /** Whether a message is the last its turn said, where what was done to
   *  the turn is offered. */
  private lastOfTurn(entry: ProseEntry): boolean {
    return entry.prose === "message" && this.conversation.lastMessageOf(entry.turn) === entry.key;
  }

  /** The turn whose actions stay shown: the last that ended. */
  private latest(turn: string): boolean {
    return this.conversation.latestEnded() === turn;
  }

  /** Under a turn's last message, once the turn is over: copy its answer,
   *  ask again, fork from here, and what the turn took. The latest turn's
   *  stay shown; earlier ones come with the pointer. */
  private answerActs(entry: ProseEntry): string {
    if (entry.writing || !this.lastOfTurn(entry) || !this.conversation.turnEnded(entry.turn)) return "";
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
        ? `<span class="turn-icon turn-info" tabindex="0" aria-label="${escape(info.map(([label, value]) => `${label} ${value}`).join("; "))}">${glyph("info", 16)}<span class="turn-info-card" role="tooltip">${info
            .map(([label, value]) => `<span class="turn-info-label">${escape(label)}</span><span>${escape(value)}</span>`)
            .join("")}</span></span>`
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
    if (!asks) return `<div class="note caution">${escape(this.context.words.waitingForYou)} · ${escape(entry.request)}</div>`;
    const ask = readAsk(entry.request, entry.params, asks.words);
    // A change waiting for approval is the edit already shown above: its
    // files are named from there.
    if (ask.kind === "files" && ask.item && !ask.paths.length && entry.turn) {
      const tool = this.conversation.get(`i:${entry.turn}:${ask.item}`);
      if (tool?.kind === "tool" && tool.tool.does.as === "edit") ask.paths = tool.tool.does.changes.map((change) => change.path);
    }
    return drawAsk(entry.key, ask, entry.state, this.context.residentMachine, asks.words);
  }

  /** Questions asked without waiting: a card to answer, answered, or
   *  skipped and a way back to it. */
  private question(entry: QuestionEntry): string {
    const questions = this.context.questions;
    if (!questions) return `<div class="prose">${render(entry.questions.map((q) => q.title).join("\n\n"))}</div>`;
    if (this.skipped.has(entry.key) && !this.editing.has(entry.key)) {
      return `<div class="question-card skipped"><p class="question-title">${plain(entry.questions[0]!.title)}</p><div class="question-foot"><button class="text-button quiet-button" type="button" data-question-change="${escape(entry.key)}">${escape(questions.words.answer)}</button></div></div>`;
    }
    return drawQuestions(entry.key, entry.questions, entry.answers, questions.words, this.editing.has(entry.key));
  }

  private async answerQuestions(target: HTMLElement): Promise<void> {
    const questions = this.context.questions;
    const card = target.closest<HTMLElement>(".question-card");
    const key = target.dataset.questionSend ?? target.dataset.questionSkip ?? target.dataset.questionChange ?? "";
    const found = this.conversation.get(key);
    const entry = found?.kind === "question" ? found : undefined;
    if (!questions || !card || !entry) return;
    if (target.dataset.questionChange !== undefined) {
      this.editing.add(key);
      this.skipped.delete(key);
      this.update();
      return;
    }
    if (target.dataset.questionSkip !== undefined) {
      this.skipped.add(key);
      this.editing.delete(key);
      this.update();
      return;
    }
    const answers = chosenIn(card, entry.questions.length);
    if (answers.every((answer) => answer === null)) {
      card.querySelector<HTMLInputElement>("[data-question-free]")?.focus();
      return;
    }
    const state = card.querySelector<HTMLElement>(".question-state")!;
    const controls = [...card.querySelectorAll<HTMLButtonElement | HTMLInputElement>("button, input")];
    for (const control of controls) control.disabled = true;
    state.textContent = questions.words.sending;
    try {
      await questions.answer(replyOf(entry.item, entry.questions, answers));
      this.conversation.answering(entry.item, answers);
      this.editing.delete(key);
      this.update();
    } catch (error) {
      for (const control of controls) control.disabled = false;
      state.textContent = questions.words.failed(detailOf(error));
      state.title = detailOf(error);
      state.classList.add("wrong");
    }
  }

  /** A turn's actions as a sheet risen from the bottom, each with its
   *  name: a press does what the mark beside the turn would. */
  private sheet(acts: HTMLElement): void {
    const scrim = document.createElement("div");
    scrim.className = "act-sheet-scrim";
    const sheet = document.createElement("div");
    sheet.className = "act-sheet";
    sheet.setAttribute("role", "menu");
    const close = () => {
      scrim.remove();
      sheet.remove();
    };
    for (const original of acts.querySelectorAll<HTMLElement>("button.turn-icon")) {
      if (getComputedStyle(original).display === "none") continue;
      const item = document.createElement("button");
      item.type = "button";
      item.setAttribute("role", "menuitem");
      item.innerHTML = `${original.innerHTML}<span>${escape(original.getAttribute("aria-label") ?? "")}</span>`;
      item.addEventListener("click", () => {
        close();
        original.click();
      });
      sheet.append(item);
    }
    // What the turn took, its rows as the info card has them.
    const card = acts.querySelector<HTMLElement>(".turn-info-card");
    if (card) sheet.insertAdjacentHTML("beforeend", `<div class="act-sheet-info turn-info-rows">${card.innerHTML}</div>`);
    scrim.addEventListener("click", close);
    document.body.append(scrim, sheet);
    layerOpened({ isOpen: () => sheet.isConnected, close });
  }

  /** The user chose in a card: an option is marked, an answer is sent. */
  private async choose(target: HTMLElement): Promise<void> {
    const asks = this.context.asks;
    const card = target.closest<HTMLElement>(".ask-card");
    if (!asks || !card) return;
    if (target.classList.contains("ask-option")) {
      const pressed = target.getAttribute("aria-pressed") !== "true";
      if (!target.dataset.many) {
        const group = target.dataset.question !== undefined ? `[data-question="${target.dataset.question}"]` : `[data-field="${target.dataset.field}"]`;
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
   *  line of its own: the World's facts, a job's end, a colleague's answer
   *  or trouble. Words another agent wrote stay words. */
  private noticeOf(entry: HeardEntry): Notice | null {
    const notice = entry.notice;
    if (!notice || entry.from?.kind === "user") return null;
    return notice.kind === "job_ended" || notice.kind === "answer" || notice.kind === "health" || notice.kind === "world" ? notice : null;
  }

  private heardEntry(entry: HeardEntry): string {
    const notice = this.noticeOf(entry);
    if (notice) return this.noticeLine(entry, notice);
    if (entry.reply) return `<p class="reply-line">${glyph("check", 14)}<span>${escape(entry.reply.answers.join(this.context.words.listSeparator))}</span></p>`;
    const mine = !entry.from || entry.from.kind === "user";
    const acts = mine && entry.turn && !entry.sending && this.conversation.turnEnded(entry.turn) ? this.userActs(entry) : "";
    return this.attached(entry.attached ?? []) + this.heard(entry.from, entry.text, entry.sending ?? null) + acts;
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
    switch (notice.kind) {
      case "job_ended": {
        const byUser = notice.stop?.by === STOPPED_BY_USER;
        const code = notice.status && "code" in notice.status ? notice.status.code : null;
        if (byUser) said = words.jobStoppedByYou;
        else if (notice.how.kind === "failed") {
          said = words.jobCouldNotRun;
          ok = false;
        } else if (notice.how.kind === "interrupted") {
          said = words.jobInterrupted;
          ok = false;
        } else if (notice.how.kind === "stopped") said = words.jobStopped;
        else {
          said = code === null ? words.jobDone(null) : words.jobDone(code);
          ok = code === null || code === 0;
        }
        more = notice.last?.text ? "last" : "";
        break;
      }
      case "answer": {
        const failed = notice.end.how === "failed";
        ok = !failed;
        said = failed ? words.answerFailed(notice.label) : words.answered(notice.label, notice.end.duration_ms ? words.took(notice.end.duration_ms) : null);
        more = notice.end.last_message ? "answer" : "";
        break;
      }
      case "health":
        ok = false;
        said = `${notice.label} · ${words.health[notice.health.kind] ?? notice.health.text}`;
        break;
      case "world": {
        const fact = notice.fact;
        said =
          fact.fact === "moved"
            ? fact.to
              ? words.movedTo(fact.to)
              : words.movedOut
            : fact.fact === "desk_free"
              ? words.deskFree(fact.by.resident)
              : fact.fact === "desk_taken"
                ? words.deskTaken(fact.by.resident)
                : words.deskOutside;
        more = fact.fact === "moved" ? "text" : "";
        break;
      }
      default:
        said = "";
    }
    const mark = ok ? glyph("check", 14) : glyph("cross", 14);
    const line = `<span class="notice-mark${ok ? "" : " wrong"}">${mark}</span><span class="notice-said">${escape(said)}</span>`;
    if (!more) return `<div class="notice-line${ok ? "" : " failed"}">${line}</div>`;
    const open = this.openRows.has(entry.key);
    return `<div class="notice-line${ok ? "" : " failed"}"><button class="notice-open" type="button" data-open="${escape(entry.key)}" aria-expanded="${open}">${line}${glyph("chevron", 14)}</button></div><div class="fold-box notice-box"${open ? "" : " style=\"height:0\" inert"}><div class="fold-content"></div></div>`;
  }

  /** What a notice holds, opened: a job's last lines and where its output
   *  is, a colleague's answer, what the World handed over. */
  private noticeDetails(entry: HeardEntry, into: HTMLElement): void {
    const notice = entry.notice;
    if (!notice) return;
    if (notice.kind === "job_ended") {
      const longs: LongText[] = [];
      if (notice.last?.text) {
        const text = new LongText(notice.last.text, notice.job, this.layout, this.longWords());
        longs.push(text);
        into.append(text.element);
      }
      const where = document.createElement("p");
      where.className = "notice-path";
      where.textContent = `${notice.machine} · ${notice.output}`;
      into.append(where);
      this.longs.set(entry.key, longs);
    } else if (notice.kind === "answer" && notice.end.last_message) {
      into.innerHTML = `<div class="prose notice-answer">${render(notice.end.last_message)}</div>`;
    } else if (notice.kind === "world") {
      into.innerHTML = `<div class="told-plain">${plain(entry.text)}</div>`;
    }
  }

  private heard(from: Speaker | null, text: string, sending: SendState | null): string {
    const { words } = this.context;
    if (!from || from.kind === "user") {
      // The user's own words; which machine they came from is said only
      // when it is not this one. Words not yet heard say how they stand.
      const who =
        from && from.machine !== this.context.here ? `<div class="bubble-from">${escape(words.you(from.machine))}</div>` : "";
      const state = sending ? ` ${sending.state}` : "";
      let note = "";
      if (sending?.state === "failed") note = `<div class="bubble-note wrong" title="${escape(sending.why)}">${escape(words.notSent(sending.why))}</div>`;
      if (sending?.state === "landed") {
        note = `<div class="bubble-note"${sending.why ? ` title="${escape(sending.why)}"` : ""}>${escape(words.landed(sending.how, sending.machine, sending.why ?? null))}</div>`;
      }
      return `${who}<div class="bubble${state}">${plain(text)}</div>${note}`;
    }
    // Words from another resident, or the World's own telling from a node
    // too old to say it as facts: never in the user's bubble. The head notes
    // written for the model are said again in the interface's words.
    const world = from.kind === "world";
    const told = readTold(text);
    let label = world ? words.world : from.kind === "page" ? words.aPage(from.page.page) : `${from.address.resident} · ${from.address.machine}`;
    let how = "";
    let body: string;
    switch (told.kind) {
      case "job":
        body = `<p class="told-line">${escape(words.jobEnded(told.job, told.machine))}<span class="told-how">${escape(words.ended(told.how))}</span></p><p class="told-path">${escape(told.output)}</p>`;
        break;
      case "colleague":
        label = `${told.colleague} · ${told.machine}`;
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
      const open = this.openStretches.has(group.key);
      slot.element.innerHTML = `<button class="group-head" type="button" data-fold="${escape(group.key)}" aria-expanded="${open}"><span class="group-mark">${glyph("thinking", 16)}</span><span class="group-title"></span><span class="row-stats group-stats"></span><span class="group-time"></span>${glyph("chevron", 16)}</button><div class="fold-box group-box"${open ? "" : ' style="height:0" inert'}><div class="rows"></div></div>`;
      stretch = {
        head: slot.element.querySelector<HTMLButtonElement>(".group-head")!,
        box: slot.element.querySelector<HTMLElement>(".group-box")!,
        rows: slot.element.querySelector<HTMLElement>(".rows")!,
        drawn: new Map(),
        heading: "",
      };
      slot.stretch = stretch;
    }
    const heading = `${group.stamp}|${group.members.length}`;
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
    const rows = thoughtOnly(group) ? [this.thoughtWords(runs[0] as ProseEntry[])] : runs.flatMap((run) => this.rowsOf(run));
    const present = new Set<string>();
    let previous: HTMLElement | null = null;
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
      }
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

  /** A stretch's heading: while it runs, the step it is on, how many steps
   *  and for how long; once done, what it did, the lines it changed, and
   *  how long it took. A stretch that is only thinking says how long it
   *  thought and the tokens it took, and opens only where there are words.
   *  The words change in place; the line keeps its height. */
  private heading(stretch: Stretch, group: Activity): void {
    const { words } = this.context;
    const said = stretchSays(group, words, (path) => this.conversation.attachmentName(path), (run) => this.conversation.thinkingTokens(run));
    const { title, added, removed, steps, running, only, bare, began } = said;
    const stats = added || removed ? `<span class="add">+${added}</span><span class="del">−${removed}</span>` : "";
    stretch.head.classList.toggle("bare", bare);
    stretch.head.classList.toggle("thought", only);
    stretch.head.disabled = bare;
    const titleElement = stretch.head.querySelector<HTMLElement>(".group-title")!;
    if (titleElement.textContent !== title) {
      // The step in hand changes in place: the new words fade in, the line
      // keeps its height.
      titleElement.textContent = title;
      if (running) {
        titleElement.classList.remove("fresh");
        void titleElement.offsetWidth;
        titleElement.classList.add("fresh");
      }
    }
    const statsElement = stretch.head.querySelector<HTMLElement>(".group-stats")!;
    if (statsElement.innerHTML !== stats) statsElement.innerHTML = stats;
    stretch.head.classList.toggle("going", running);
    const time = stretch.head.querySelector<HTMLElement>(".group-time")!;
    if (running && began !== null) {
      time.dataset.since = String(began);
      time.dataset.steps = String(steps);
      this.clocks.add(time);
    } else {
      delete time.dataset.since;
      this.clocks.delete(time);
      if (time.textContent !== said.time) time.textContent = said.time;
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
      const said = clockSays(Number(clock.dataset.steps), Number(clock.dataset.since), words);
      if (clock.textContent !== said) {
        clock.textContent = said;
        const block = clock.closest<HTMLElement>(".block");
        if (block?.dataset.key) this.findPending.add(block.dataset.key);
      }
    }
    clearTimeout(this.ticking);
    if (this.clocks.size) this.ticking = window.setTimeout(() => this.tick(), 1000);
  }

  /** The rows a run of members stands for: thinking with nothing between
   *  is one row; a command's writes have one each, after its own, which a
   *  command that did nothing but write does not have. */
  private rowsOf(run: Entry[]): Row[] {
    const member = run[0]!;
    if (isThinking(member)) {
      const thoughts = run as ProseEntry[];
      return [{ key: member.key, signature: this.runSignature(thoughts), html: () => this.thinkingRow(thoughts, this.entering(member.key)), text: () => this.thinkingRowText(thoughts) }];
    }
    const own: Row = { key: member.key, signature: this.memberSignature(member), html: () => this.member(member), text: () => this.findRowText(member) };
    const writes = member.kind === "tool" ? (member.tool.run?.writes ?? []) : [];
    if (!writes.length || member.kind !== "tool") return [own];
    return [
      ...(onlyWrites(member.tool) ? [] : [own]),
      ...writes.map((write) => {
        const key = `${member.key}${WRITE}${write.id}`;
        return {
          key, signature: JSON.stringify(write),
          html: () => this.writeRow(key, write),
          text: () => {
            const line = writeHeadline(write, this.context.words);
            const stats = line.added !== null && line.removed !== null ? `+${line.added} −${line.removed}` : "";
            return [line.action, line.object, stats, write.machine !== this.context.residentMachine ? this.context.words.on(write.machine) : ""].filter(Boolean).join(" ");
          },
        };
      }),
    ];
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

  private memberSignature(entry: Entry): string {
    if (entry.kind === "tool" && this.conversation.agentOf(entry)) return `${entry.stamp}a${this.conversation.agentStamp}`;
    return String(entry.stamp);
  }

  /** A run of thinking's line as its row shows it. */
  private thinkingRowText(run: readonly ProseEntry[]): string {
    const { words } = this.context;
    const writing = run.some((entry) => entry.writing);
    const took = writing ? null : thoughtTook(run);
    const titles = sections(thoughtText(run));
    const title = writing ? titles.at(-1) : titles[0];
    const tokens = writing ? null : this.conversation.thinkingTokens(run);
    return [writing ? words.thinking : took === null ? words.thought : words.thoughtFor(took), title ?? "", tokens === null ? "" : words.thinkingTokens(tokens)].filter(Boolean).join(" ");
  }

  private findRowText(entry: Entry): string {
    const { words } = this.context;
    if (entry.kind === "activity") return entry.text;
    if (entry.kind !== "tool") return "";
    const line = headline(entry.tool, entry.ended, words, (path) => this.conversation.attachmentName(path), true);
    const tense = tenseOf(entry.ended);
    const stats = line.added !== null && line.removed !== null ? `+${line.added} −${line.removed}` : "";
    const where = entry.tool.machine && entry.tool.machine !== this.context.residentMachine ? words.on(entry.tool.machine) : "";
    const agent = this.conversation.agentOf(entry);
    const colleague = entry.tool.does.as === "colleague" && entry.tool.does.to && this.context.link ? words.openColleague : "";
    const text = [line.action, line.object, stats, displayedTextHtml(this.outcome(entry, tense)), where].filter(Boolean).join(" ");
    return `${text}${colleague ? `\n${colleague}` : ""}${agent ? `\n${words.subagent(agent.kind, agent.steps, tense === "running" ? agent.doing : null)}` : ""}`;
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
    const elsewhere = write.machine !== this.context.residentMachine ? `<span class="row-machine">${escape(words.on(write.machine))}</span>` : "";
    const chevron = expandable ? `<span class="row-chevron">${glyph("chevron", 16)}</span>` : "";
    const fold = expandable ? `<div class="fold-box"${open ? "" : " style=\"height:0\" inert"}><div class="fold-content"></div></div>` : "";
    // A change that could not be read line by line says why on hover.
    const why = write.unavailable ? ` title="${escape(write.unavailable)}"` : "";
    return `<div class="row row-tool row-done${enter}" data-row="${escape(key)}"><span class="rail">${glyph(line.glyph)}</span><button class="row-line" type="button" ${expandable ? `data-open="${escape(key)}"` : "disabled"} aria-expanded="${open}"${why}><span class="row-action">${escape(line.action)}</span><span class="row-rest"><span class="row-object" title="${escape(write.path)}">${escape(line.object ?? "")}</span><span class="row-meta">${stats}${elsewhere}${chevron}</span></span></button>${fold}</div>`;
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

  private member(entry: Entry): string {
    const enter = this.entering(entry.key);
    if (entry.kind === "activity") {
      return `<div class="row${enter}" data-row="${escape(entry.key)}"><span class="rail">${glyph("colleague")}</span><div class="row-line static"><span class="row-object">${escape(entry.text)}</span></div></div>`;
    }
    if (entry.kind !== "tool") return `<div data-row="${escape(entry.key)}"></div>`;
    const { words } = this.context;
    const line = headline(entry.tool, entry.ended, words, (path) => this.conversation.attachmentName(path), true);
    const tense = tenseOf(entry.ended);
    const open = this.openRows.has(entry.key);
    const expandable = this.hasDetails(entry.tool);
    const elsewhere =
      entry.tool.machine && entry.tool.machine !== this.context.residentMachine
        ? `<span class="row-machine">${escape(words.on(entry.tool.machine))}</span>`
        : "";
    const stats =
      line.added !== null && line.removed !== null
        ? `<span class="row-stats"><span class="add">+${line.added}</span><span class="del">−${line.removed}</span></span>`
        : "";
    const mark = tense === "running" ? `<span class="pulse" aria-hidden="true"></span>` : glyph(line.glyph);
    const object = line.object
      ? `<span class="row-object"${line.title ? ` title="${escape(line.title)}"` : ""}>${escape(line.object)}</span>`
      : "";
    const chevron = expandable ? `<span class="row-chevron">${glyph("chevron", 16)}</span>` : "";
    const fold = expandable ? `<div class="fold-box"${open ? "" : " style=\"height:0\" inert"}><div class="fold-content"></div></div>` : "";
    // A colleague it asked lives on in the World: its own conversation is a
    // step away.
    const colleague =
      entry.tool.does.as === "colleague" && entry.tool.does.to && this.context.link
        ? `<a class="row-link" href="${escape(this.context.link({ resident: entry.tool.does.to, machine: entry.tool.machine ?? this.context.residentMachine }))}">${escape(words.openColleague)}${glyph("chevron", 14)}</a>`
        : "";
    // Claude Code's own subagent, followed as it goes: its kind, its steps,
    // the one it takes now.
    const agent = this.conversation.agentOf(entry);
    const sub = agent ? `<div class="row-sub">${escape(words.subagent(agent.kind, agent.steps, tense === "running" ? agent.doing : null))}</div>` : "";
    // What it did and on what, its figures beside it, the machine where it
    // was not here; how a command ended is in what it holds.
    const meta = `<span class="row-meta">${stats}${this.outcome(entry, tense)}${elsewhere}${chevron}</span>`;
    return `<div class="row row-tool row-${tense}${enter}" data-row="${escape(entry.key)}"><span class="rail">${mark}</span><button class="row-line" type="button" ${expandable ? `data-open="${escape(entry.key)}"` : "disabled"} aria-expanded="${open}"><span class="row-action">${escape(line.action)}</span><span class="row-rest">${object}${meta}</span></button>${colleague}${sub}${fold}</div>`;
  }

  /** A run of thinking as one row: "Thinking…" while it goes, "Thought for
   *  4s" once over where the host said how long, "Thought" where not; the title of its words'
   *  section after it, the tokens it took at the end. It opens only where
   *  the host gave words; none are made up. */
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
    // the row says the latest while it thinks, the first after.
    const titles = sections(text);
    const title = writing ? titles.at(-1) : titles[0];
    const tokens = writing ? null : this.conversation.thinkingTokens(run);
    const figures = tokens === null ? "" : `<span class="row-figures">${escape(words.thinkingTokens(tokens))}</span>`;
    const chevron = has ? `<span class="row-chevron">${glyph("chevron", 16)}</span>` : "";
    const fold = has ? `<div class="fold-box"${open ? "" : ' style="height:0" inert'}><div class="fold-content"></div></div>` : "";
    return `<div class="row row-thinking${writing ? " row-running" : ""}${enter}" data-row="${escape(key)}"><span class="rail">${glyph("thinking")}</span><button class="row-line" type="button" ${has ? `data-open="${escape(key)}"` : "disabled"} aria-expanded="${open}"><span class="row-said">${escape(said)}</span>${title ? `<span class="row-object">${escape(title)}</span>` : ""}<span class="row-meta">${figures}${chevron}</span></button>${fold}</div>`;
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
  private diffText(lines: readonly string[] | RemoteDiff, path: string, numberedRows: boolean): LongText {
    return new LongText(
      {
        lines,
        ...(Array.isArray(lines) ? { source: new DiffLines(lines as readonly string[]) } : {}),
        paint: diffPaint(path, this.context.words.unchangedLines),
        kind: numberedRows ? "diff" : "diff unnumbered",
      },
      path,
      this.layout,
      this.longWords(),
    );
  }

  /** What a write changed, read from the machine that keeps it: a few rows
   *  are read whole and shown outright, more are read a screen at a time. */
  private fillWrite(key: string, write: WriteFact, into: HTMLElement): void {
    const cached = this.findWrites.get(key);
    if (cached?.signature === JSON.stringify(write)) {
      const long = this.diffText(cached.lines, write.path, true);
      this.longs.set(key, [long]);
      into.append(long.element);
      return;
    }
    const diff = write.diff;
    const lines = this.context.lines;
    if (!diff || !lines) return;
    const { words } = this.context;
    const note = document.createElement("p");
    note.className = "io-note";
    note.textContent = words.output.reading;
    into.append(note);
    const request = new AbortController();
    const held: { raw: OpenedLines | null } = { raw: null };
    const close = () => {
      const raw = held.raw;
      held.raw = null;
      if (raw) void raw.close().catch((error: unknown) => console.warn("a change's lines could not be let go", error));
    };
    this.closers.set(key, [
      () => {
        request.abort();
        close();
      },
    ]);
    const pending = (async () => {
      try {
        held.raw = await lines(diff.machine, diff.path, { kind: "file", encoding: "utf-8" });
        if (request.signal.aborted) return close();
        const remote = await RemoteDiff.open(held.raw, request.signal);
        let long: LongText;
        if (remote.extent.length <= FEW) {
          const rows = [...(await remote.read(0, remote.extent.length, request.signal))];
          close();
          long = this.diffText(rows, write.path, true);
        } else long = this.diffText(remote, write.path, true);
        if (request.signal.aborted) {
          long.destroy();
          return close();
        }
        this.longs.set(key, [long]);
        note.replaceWith(long.element);
      } catch (error) {
        if (request.signal.aborted) return;
        close();
        note.textContent = gone(error) ? words.output.diffGone : words.output.readFailed;
        note.title = detailOf(error);
      }
    })();
    this.filling.set(key, pending);
    void pending.finally(() => {
      if (this.filling.get(key) === pending) this.filling.delete(key);
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
    if (entry.kind === "prose") {
      content.innerHTML = `<div class="thinking-text prose">${render(thoughtText(this.thoughtRun(key) ?? [entry]))}</div>`;
      return;
    }
    if (entry.kind !== "tool") return;
    const { words } = this.context;
    const { tool } = entry;
    if (tool.does.as === "edit") {
      // One file's change needs no name above it: its row has said it.
      const several = tool.does.changes.length > 1;
      const longs: LongText[] = [];
      for (const change of tool.does.changes) {
        const rows = parse(change.diff);
        const long = this.diffText(memoryDiff(rows).lines, change.path, numbered(rows));
        longs.push(long);
        if (several) {
          const head = document.createElement("p");
          head.className = "diff-path";
          head.title = change.path;
          head.textContent = change.path;
          content.append(head);
        }
        content.append(long.element);
      }
      this.longs.set(key, longs);
      return;
    }
    // A World run says what the model got as facts: its own output, apart
    // from what its writes printed of themselves, which their rows show.
    const { run, shown, output, given, ending: endText } = toolText(tool, words);
    const images = imagesOf(tool.output);
    const surface = document.createElement("div");
    surface.className = "surface io";
    const longs: LongText[] = [];
    const closers: (() => void)[] = [];
    const part = (label: string, text: string | LongLines, extra = "") => {
      const holder = document.createElement("div");
      holder.className = "io-part";
      holder.innerHTML = `<span class="io-label">${label}</span>`;
      const out = document.createElement("div");
      out.className = "io-out";
      if (text) {
        const long = new LongText(text, label, this.layout, this.longWords());
        longs.push(long);
        out.append(long.element);
      }
      if (extra) out.insertAdjacentHTML("beforeend", extra);
      holder.append(out);
      surface.append(holder);
    };
    // What the model got, and beside it in the full-screen view the whole
    // output on its machine, where the model got less than the whole or the
    // command went on after the model had its answer.
    let printed: string | LongLines = output;
    if (run && shown) {
      const others: OtherSource[] = [];
      const kept = run.output;
      const going = stillGoing(run.state);
      // A command that went on in the background gave the model only what it
      // wrote before then; the rest is in the file, after it ended as well.
      const wentOn = tool.does.as === "command" && tool.does.job !== null;
      if (kept && run.job && this.context.lines && (kept.cut || going || wentOn)) {
        const open = this.context.lines;
        const job = run.job;
        const whole = new WholeOutput(
          () => open(kept.machine, kept.path, { kind: "output", job }),
          { length: kept.lines, complete: !going },
          (error) => new Said(gone(error) ? words.output.wholeGone : words.output.readFailed, detailOf(error)),
        );
        others.push({ label: words.output.whole, source: whole, close: () => whole.close() });
        closers.push(() => whole.close());
      }
      // Numbered by place in the whole output where that is not simply the
      // line's place here: something was left out, or a write's note was.
      const plain = !shown.cut && shown.numbers.every((kept, index) => kept === index + 1);
      printed = { lines: shown.lines, ...(plain ? {} : { paint: outputPaint(shown) }), name: words.output.modelCopy, others };
    }
    // How a command ended, under what it wrote.
    const ending = endText ? `<p class="io-end${tool.does.as === "command" && !tool.does.job && tool.does.exit !== null && tool.does.exit !== 0 ? " nonzero" : ""}">${escape(endText)}</p>` : "";
    if (given) part("IN", given);
    if (printed || images || ending) part("OUT", printed, images + ending);
    content.append(surface);
    this.longs.set(key, longs);
    this.closers.set(key, closers);
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
      motion = new AnchoredHeight(box, this.layout);
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
      const content = box.firstElementChild as HTMLElement | null;
      const target = content?.offsetHeight ?? 0;
      const result = await motion.to(target, anchor);
      if (!result.cancelled && anchor.getAttribute("aria-expanded") === "true") box.style.height = "";
      return true;
    }
    box.style.height = `${box.offsetHeight}px`;
    const result = await motion.to(0, anchor);
    if (!result.cancelled && anchor.getAttribute("aria-expanded") === "false") box.inert = true;
    return !result.cancelled;
  }

  private longsIn(box: HTMLElement): LongText[] {
    const found: LongText[] = [];
    for (const longs of this.longs.values()) for (const long of longs) if (box.contains(long.element)) found.push(long);
    return found;
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>(
      "[data-open], [data-fold], .code-copy, .ask-option, [data-ask], [data-copy], [data-copy-turn], [data-turn-act], [data-question-pick], [data-question-send], [data-question-skip], [data-question-change]",
    );
    if (!target) return;
    if (target.dataset.questionPick !== undefined) {
      const fieldset = target.closest("fieldset");
      const pressed = target.getAttribute("aria-pressed") !== "true";
      for (const other of fieldset?.querySelectorAll<HTMLElement>("[data-question-pick]") ?? []) other.setAttribute("aria-pressed", "false");
      target.setAttribute("aria-pressed", String(pressed));
      const free = fieldset?.querySelector<HTMLInputElement>("[data-question-free]");
      if (free && pressed) free.value = "";
      return;
    }
    if (target.dataset.questionSend !== undefined || target.dataset.questionSkip !== undefined || target.dataset.questionChange !== undefined) {
      void this.answerQuestions(target);
      return;
    }
    if (target.dataset.turnAct) {
      const turn = target.dataset.turn!;
      // Asking again sends the words of the turn's own question.
      const entry =
        target.dataset.turnAct === "rewind"
          ? this.conversation.get(target.dataset.key ?? "")
          : this.conversation.entries.find((candidate) => candidate.kind === "heard" && candidate.turn === turn && (!candidate.from || candidate.from.kind === "user"));
      this.context.turnAct?.(target.dataset.turnAct as "rewind" | "regenerate" | "fork", turn, entry?.kind === "heard" ? entry.text : null, entry?.kind === "heard" ? (entry.kept ?? []) : []);
      return;
    }
    if (target.dataset.copy || target.dataset.copyTurn) {
      // A message is copied as it was written, in its Markdown; a turn's
      // answer as all it said.
      const entry = target.dataset.copy ? this.conversation.get(target.dataset.copy) : undefined;
      const text = target.dataset.copyTurn
        ? this.conversation.entries
            .filter((candidate): candidate is ProseEntry => candidate.kind === "prose" && candidate.prose === "message" && candidate.turn === target.dataset.copyTurn)
            .map((candidate) => candidate.text)
            .join("\n\n")
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
      if (!slot || !stretch || slot.block.kind !== "activity-group") return;
      const group = slot.block;
      const open = !this.openStretches.has(key);
      if (open) this.openStretches.add(key);
      else this.openStretches.delete(key);
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
    const row = target.closest<HTMLElement>("[data-row], .block-heard");
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

/** What a turn took, as label and value: the model and effort its host
 *  reported, how long, and the tokens; what the host did not report is left
 *  out. */
function turnInfo(end: TurnEnd, words: Words, english: boolean): [string, string][] {
  const { turnInfo: said } = words;
  const facts = end.facts;
  const rows: [string, string][] = [];
  if (facts?.models?.length) rows.push([said.model, facts.models.map(modelName).join(" → ")]);
  const efforts = (facts?.efforts ?? []).filter((effort) => effort.value !== null && effort.value !== undefined);
  if (efforts.length) {
    rows.push([
      said.effort,
      efforts
        .map((effort) => {
          const value = effortLabel(typeof effort.value === "string" ? effort.value : JSON.stringify(effort.value), english);
          return effort.kind === "requested" ? said.requested(value) : value;
        })
        .join(" → "),
    ]);
  }
  if (end.duration_ms) rows.push([said.took, words.took(end.duration_ms)]);
  const tokens = facts?.tokens;
  if (tokens?.host === "codex") rows.push([said.tokens, said.spent(tokens.input_tokens, tokens.cached_input_tokens, tokens.output_tokens, tokens.reasoning_output_tokens)]);
  else if (tokens?.host === "claude") {
    const input = tokens.input_tokens + tokens.cache_read_input_tokens + tokens.cache_creation_input_tokens;
    rows.push([said.tokens, said.spent(input, tokens.cache_read_input_tokens, tokens.output_tokens, 0)]);
  }
  return rows;
}

/** A model as people call it: `claude-opus-5-5` is Opus 5.5; any other
 *  name as its host writes it. */
function modelName(id: string): string {
  const claude = /^claude-([a-z]+)-(\d+)(?:-(\d+))?(?:-\d{8})?$/.exec(id);
  if (!claude) return id;
  const [, family, major, minor] = claude;
  return `${family![0]!.toUpperCase()}${family!.slice(1)} ${major}${minor ? `.${minor}` : ""}`;
}
